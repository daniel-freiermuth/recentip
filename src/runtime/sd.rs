//! # Service Discovery Message Handling (Internal)
//!
//! This module handles SOME/IP Service Discovery (SD) protocol messages.
//! It is `pub(crate)` — internal to the library.
//!
//! ## Service Discovery Overview
//!
//! SD runs over UDP multicast (default: 239.255.0.1:30490) and provides:
//!
//! - **Service announcement**: Servers advertise available services
//! - **Service discovery**: Clients find services dynamically
//! - **Subscription management**: Event subscription negotiation
//!
//! ## SD Entry Types
//!
//! | Entry Type | Direction | Purpose |
//! |------------|-----------|---------|
//! | `OfferService` | Server → Network | "I'm offering this service" |
//! | `StopOfferService` | Server → Network | "I'm no longer offering this" |
//! | `FindService` | Client → Network | "Is anyone offering this service?" |
//! | `SubscribeEventgroup` | Client → Server | "I want to receive these events" |
//! | `SubscribeEventgroupAck` | Server → Client | "Subscription accepted" |
//! | `SubscribeEventgroupNack` | Server → Client | "Subscription rejected" |
//! | `StopSubscribeEventgroup` | Client → Server | "I no longer want these events" |
//!
//! ## Module Structure
//!
//! - **Handlers**: `handle_offer()`, `handle_find_request()`, `handle_subscribe_request()`, etc.
//! - **Builders**: `build_offer_message()`, `build_find_message()`, `build_subscribe_ack()`, etc.
//! - **Actions**: [`Action`] enum represents side effects for the runtime to execute
//!
//! ## Action Pattern
//!
//! Handlers don't perform I/O directly. They modify state and return [`Action`]
//! values. The runtime's event loop collects these actions and executes them:
//!
//! ```rust,ignore
//! // (Internal API - not accessible from user code)
//! let actions = handle_offer(&entry, &sd_message, from, &mut state);
//! for action in actions {
//!     match action {
//!         Action::SendSd { message, target } => { /* send via SD socket */ }
//!         Action::NotifyFound { key, availability } => { /* notify handles */ }
//!         // ...
//!     }
//! }
//! ```
//!
//! ## Contributor Notes
//!
//! When adding new SD features:
//! 1. Add handler function for the entry type
//! 2. Add builder function for outgoing messages
//! 3. Add [`Action`] variant if new side effect needed
//! 4. Wire up in `runtime.rs` event loop

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use bytes::Bytes;
use tokio::time::Instant;

use super::command::ServiceAvailability;
use super::state::{
    DiscoveredService, MultiEventgroupSubscriptionKey, OfferedService, PendingServerResponse,
    PendingSubscriptionKey, RuntimeState, ServerSubscription, ServiceKey, SubscriberKey,
};
use crate::config::{Transport, SD_TTL_INFINITE};
use crate::wire::{L4Protocol, SdEntry, SdMessage, SdOption};

// ============================================================================
// ACTION TYPE
// ============================================================================

/// Action to execute after handling an event
pub enum Action {
    /// Send an SD message to a specific target
    SendSd {
        message: SdMessage,
        target: SocketAddr,
    },
    /// Notify find requests about a discovered service
    NotifyFound {
        key: ServiceKey,
        availability: ServiceAvailability,
    },
    /// Send a SOME/IP RPC message as a client (uses client RPC socket or TCP pool)
    SendClientMessage {
        data: Bytes,
        target: SocketAddr,
        /// Transport to use (TCP or UDP), determined by discovered service endpoint
        transport: crate::config::Transport,
    },
    /// Send a SOME/IP RPC message as a server (uses service's RPC socket)
    SendServerMessage {
        service_key: ServiceKey,
        data: Bytes,
        target: SocketAddr,
        /// Transport to use (TCP or UDP) - should match what subscriber used
        transport: crate::config::Transport,
    },
    /// Track a pending server response
    TrackServerResponse {
        context: PendingServerResponse,
        receiver: tokio::sync::oneshot::Receiver<crate::error::Result<Bytes>>,
    },
    /// Emit an SD event to all monitors
    EmitSdEvent { event: crate::SdEvent },
    /// Reset TCP connections to a peer (due to reboot detection)
    ResetPeerTcpConnections { peer: std::net::IpAddr },
    /// Expire all subscriptions from/to a peer (due to reboot detection)
    /// Per `feat_req_someipsd_871`: On peer reboot, cancel subscriptions
    ExpirePeerSubscriptions { peer: std::net::IpAddr },
}

// ============================================================================
// SD MESSAGE HANDLERS
// ============================================================================

/// Handle an `OfferService` entry
///
/// When we receive an `OfferService`, we:
/// 1. Update our discovered services table
/// 2. Notify any pending find requests
/// 3. **[`feat_req_someipsd_428/431/631`]** If we have active subscriptions for this service,
///    send `SubscribeEventgroup` in response (offer-triggered subscription)
pub fn handle_offer(
    entry: &SdEntry,
    sd_message: &SdMessage,
    from: SocketAddr,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    // Get UDP endpoint from SD option, falling back to source address
    // If the endpoint has unspecified IP (0.0.0.0), use the source IP with the option's port
    let udp_endpoint = match sd_message.get_udp_endpoint(entry) {
        Some(ep) if !ep.ip().is_unspecified() => Some(ep),
        Some(ep) => Some(SocketAddr::new(from.ip(), ep.port())),
        None => None,
    };

    // Get TCP endpoint from SD option if present
    let tcp_endpoint = match sd_message.get_tcp_endpoint(entry) {
        Some(ep) if !ep.ip().is_unspecified() => Some(ep),
        Some(ep) => Some(SocketAddr::new(from.ip(), ep.port())),
        None => None,
    };

    // Select endpoint based on client's preferred_transport
    // If preferred is available, use it; otherwise use whatever is available
    let prefer_tcp = state.config.preferred_transport == crate::config::Transport::Tcp;
    // might deduplicated with the DiscoveredService helper
    let Some((effective_endpoint, effective_transport)) = (if prefer_tcp {
        tcp_endpoint
            .map(|ep| (ep, crate::config::Transport::Tcp))
            .or_else(|| udp_endpoint.map(|ep| (ep, crate::config::Transport::Udp)))
    } else {
        udp_endpoint
            .map(|ep| (ep, crate::config::Transport::Udp))
            .or_else(|| tcp_endpoint.map(|ep| (ep, crate::config::Transport::Tcp)))
    }) else {
        tracing::error!(
            "Received OfferService for {:04x}:{:04x} but no transport endpoints offered",
            entry.service_id,
            entry.instance_id
        );
        return;
    };

    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
        major_version: entry.major_version,
    };

    let ttl_duration = Duration::from_secs(u64::from(entry.ttl));

    tracing::debug!(
        "Discovered service {:04x}:{:04x} at udp={:?} tcp={:?} transport={:?} (TTL={})",
        entry.service_id,
        entry.instance_id,
        udp_endpoint,
        tcp_endpoint,
        effective_transport,
        entry.ttl
    );

    let is_new = !state.discovered.contains_key(&key);
    state.discovered.insert(
        key,
        DiscoveredService {
            udp_endpoint,
            tcp_endpoint,
            sd_endpoint: from, // Store the SD source for Subscribe messages
            minor_version: entry.minor_version,
            ttl_expires: Instant::now() + ttl_duration,
        },
    );

    if is_new {
        actions.push(Action::NotifyFound {
            key,
            availability: ServiceAvailability::Available {
                endpoint: effective_endpoint,
                transport: effective_transport,
                instance_id: entry.instance_id,
                major_version: entry.major_version,
            },
        });

        // Emit SD event to monitors
        actions.push(Action::EmitSdEvent {
            event: crate::SdEvent::ServiceAvailable {
                service_id: entry.service_id,
                instance_id: entry.instance_id,
                major_version: entry.major_version,
                minor_version: entry.minor_version,
                endpoint: effective_endpoint,
                ttl: entry.ttl,
            },
        });
    }

    // [feat_req_someipsd_428/431/631] Offer-triggered subscription renewal
    // If we have active subscriptions for this service, respond with SubscribeEventgroup.
    // Per feat_req_someipsd_631: "Subscriptions shall NOT be triggered cyclically
    // but SHALL be triggered by OfferService entries."

    // Check if we have any subscriptions for this service first
    let Some(subscriptions) = state.subscriptions.get(&key) else {
        return;
    };
    if subscriptions.is_empty() {
        return;
    }

    // Determine the actual local IP address to put in the endpoint option
    // Per feat_req_someipsd_814, we must provide a valid routable IP, not 0.0.0.0
    let endpoint_ip = if let Some(advertised) = state.config.advertised_ip {
        advertised
    } else if !state.local_endpoint.ip().is_unspecified() {
        state.local_endpoint.ip()
    } else if !state.client_rpc_endpoint.ip().is_unspecified() {
        state.client_rpc_endpoint.ip()
    } else {
        // No valid IP available - cannot renew subscriptions
        tracing::error!(
            "Cannot renew subscription for {:04x}:{:04x}: \
             no valid IP address configured. Set RuntimeConfig::advertised_ip",
            entry.service_id,
            entry.instance_id
        );
        return;
    };

    let sd_flags = state.sd_flags(true);

    let mut sd_messages_to_be_queued = Vec::new();

    if let Some(subscriptions) = state.subscriptions.get(&key) {
        for sub in subscriptions {
            // Skip if TTL is infinite (0xFFFFFF) - no renewal needed
            if state.config.subscribe_ttl == SD_TTL_INFINITE {
                tracing::trace!(
                    "Skipping renewal for {:04x}:{:04x} eventgroup {:04x} (infinite TTL)",
                    entry.service_id,
                    entry.instance_id,
                    sub.eventgroup_id
                );
                continue;
            }

            // Use this subscription's local endpoint for renewal
            let endpoint_for_subscribe =
                std::net::SocketAddr::new(endpoint_ip, sub.local_endpoint.port());
            tracing::debug!(
                "Queueing offer-triggered subscription renewal for {:04x}:{:04x} v{} eventgroups {:?} via {:?} (port {}) for time-based clustering",
                entry.service_id,
                entry.instance_id,
                entry.major_version,
                sub.eventgroup_id,
                effective_transport,
                endpoint_for_subscribe.port()
            );

            // Build SubscribeEventgroup message with all eventgroups
            let msg = build_subscribe_message(
                entry.service_id,
                entry.instance_id,
                entry.major_version,
                sub.eventgroup_id,
                endpoint_for_subscribe,
                endpoint_for_subscribe.port(),
                sd_flags,
                state.config.subscribe_ttl,
                effective_transport,
            );

            // Queue for time-based clustering instead of sending immediately
            sd_messages_to_be_queued.push((msg, from));
        }
        for (message, target) in sd_messages_to_be_queued {
            state.queue_unicast_sd(message, target);
        }
    }
}

/// Handle a `StopOfferService` entry
pub fn handle_stop_offer(entry: &SdEntry, state: &mut RuntimeState, actions: &mut Vec<Action>) {
    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
        major_version: entry.major_version,
    };

    if state.discovered.remove(&key).is_some() {
        tracing::debug!(
            "Service {:04x}:{:04x} v{} stopped offering",
            entry.service_id,
            entry.instance_id,
            entry.major_version
        );

        // Close any client subscriptions for this service
        // When the service goes away, subscription channels should close
        // so that subscription.next() returns None
        state.subscriptions.remove(&key);

        // Emit SD event to monitors
        actions.push(Action::EmitSdEvent {
            event: crate::SdEvent::ServiceUnavailable {
                service_id: entry.service_id,
                instance_id: entry.instance_id,
            },
        });
    }
}

/// Handle a `FindService` request (we may need to respond with an offer)
///
/// Responses are queued for time-based clustering to prevent session ID collisions
/// when multiple `FindService` requests arrive close together.
pub fn handle_find_request(entry: &SdEntry, from: SocketAddr, state: &mut RuntimeState) {
    // Collect responses first to avoid borrowing issues
    let responses: Vec<SdMessage> = state
        .offered
        .iter()
        .filter(|(_, offered)| offered.is_announcing)
        .filter(|(key, _)| {
            entry.service_id == key.service_id
                && (entry.instance_id == 0xFFFF || entry.instance_id == key.instance_id)
        })
        .map(|(key, offered)| {
            build_offer_message(
                *key,
                offered,
                state.sd_flags(true),
                state.config.offer_ttl,
                state.config.advertised_ip,
            )
        })
        .collect();

    // Queue all responses for time-based clustering
    for response in responses {
        state.queue_unicast_sd(response, from);
    }
}

/// Handle a `SubscribeEventgroup` request
///
/// Per `feat_req_someipsd_1144`: If options are in conflict, respond negatively (NACK)
/// Per `feat_req_someipsd_1137`: Respond with `SubscribeEventgroupNack` for invalid subscribe
///
/// ACKs and NACKs are queued for time-based clustering to prevent session ID collisions.
pub fn handle_subscribe_request(
    entry: &SdEntry,
    sd_message: &SdMessage,
    from: SocketAddr,
    state: &mut RuntimeState,
) {
    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
        major_version: entry.major_version,
    };

    // Get client's endpoint options from the SD message
    // Per feat_req_someipsd_814: Use endpoint option address as provided
    let client_udp_endpoint = sd_message.get_udp_endpoint(entry);
    let client_tcp_endpoint = sd_message.get_tcp_endpoint(entry);

    // Validate that endpoint options contain valid, routable IP addresses
    // Reject unspecified (0.0.0.0) addresses - clients must configure their actual IP
    if let Some(ep) = client_udp_endpoint {
        if ep.ip().is_unspecified() {
            tracing::warn!(
                "Rejecting subscription for {:04x}:{:04x} eventgroup {:04x} from {}: \
                 endpoint option has unspecified IP address (0.0.0.0)",
                entry.service_id,
                entry.instance_id,
                entry.eventgroup_id,
                from
            );

            let mut nack = SdMessage::new(state.sd_flags(true));
            nack.add_entry(SdEntry::subscribe_eventgroup_nack(
                entry.service_id,
                entry.instance_id,
                entry.major_version,
                entry.eventgroup_id,
                entry.counter,
            ));

            state.queue_unicast_sd(nack, from);
            return;
        }
    }
    if let Some(ep) = client_tcp_endpoint {
        if ep.ip().is_unspecified() {
            tracing::warn!(
                "Rejecting subscription for {:04x}:{:04x} eventgroup {:04x} from {}: \
                 endpoint option has unspecified IP address (0.0.0.0)",
                entry.service_id,
                entry.instance_id,
                entry.eventgroup_id,
                from
            );

            let mut nack = SdMessage::new(state.sd_flags(true));
            nack.add_entry(SdEntry::subscribe_eventgroup_nack(
                entry.service_id,
                entry.instance_id,
                entry.major_version,
                entry.eventgroup_id,
                entry.counter,
            ));

            state.queue_unicast_sd(nack, from);
            return;
        }
    }

    if let Some(offered) = state.offered.get(&key) {
        // Check transport compatibility per feat_req_someipsd_1144
        // Server offers UDP and/or TCP. Client requests UDP and/or TCP endpoint.
        let Some((client_endpoint, transport)) = offered
            .udp_endpoint
            .and_then(|_| client_udp_endpoint.map(|ep| (ep, crate::config::Transport::Udp)))
            .or_else(|| {
                offered
                    .tcp_endpoint
                    .and_then(|_| client_tcp_endpoint.map(|ep| (ep, crate::config::Transport::Tcp)))
            })
        else {
            // Transport mismatch - send NACK
            tracing::warn!(
                "Rejecting subscription for {:04x}:{:04x} eventgroup {:04x} from {}: \
                 transport mismatch (server offers UDP={}, TCP={}; client wants UDP={}, TCP={})",
                entry.service_id,
                entry.instance_id,
                entry.eventgroup_id,
                from,
                offered.udp_endpoint.is_some(),
                offered.tcp_endpoint.is_some(),
                client_udp_endpoint.is_some(),
                client_tcp_endpoint.is_some()
            );

            let mut nack = SdMessage::new(state.sd_flags(true));
            nack.add_entry(SdEntry::subscribe_eventgroup_nack(
                entry.service_id,
                entry.instance_id,
                entry.major_version,
                entry.eventgroup_id,
                entry.counter,
            ));

            state.queue_unicast_sd(nack, from);
            return;
        };

        // Track the subscriber - use the endpoint from the option for event delivery
        // Calculate expiration based on client's TTL from the Subscribe message
        // Per feat_req_someipsd_431: TTL=0xFFFFFF means "until next reboot" (infinite)
        let expires_at = if entry.ttl == SD_TTL_INFINITE {
            None // Infinite TTL - never expires
        } else {
            let ttl_duration = Duration::from_secs(u64::from(entry.ttl));
            Some(Instant::now() + ttl_duration)
        };

        let sub_key = SubscriberKey {
            service_id: entry.service_id,
            instance_id: entry.instance_id,
            major_version: entry.major_version,
            eventgroup_id: entry.eventgroup_id,
        };

        // Check if this client already has a subscription - if so, update the expiration
        let subscribers = state.server_subscribers.entry(sub_key).or_default();
        let is_new_subscription = !subscribers.iter().any(|s| s.endpoint == client_endpoint);

        if let Some(existing) = subscribers
            .iter_mut()
            .find(|s| s.endpoint == client_endpoint)
        {
            // Renew: update expiration time (and possibly transport)
            existing.expires_at = expires_at;
            existing.transport = transport;
            tracing::debug!(
                "Renewed subscription for {:04x}:{:04x} eventgroup {:04x} from {} via {:?} (TTL={}s)",
                entry.service_id,
                entry.instance_id,
                entry.eventgroup_id,
                client_endpoint,
                transport,
                entry.ttl
            );
        } else {
            // New subscription
            subscribers.push(ServerSubscription {
                endpoint: client_endpoint,
                transport,
                expires_at,
            });
            tracing::debug!(
                "New subscription for {:04x}:{:04x} eventgroup {:04x} from {} via {:?} (TTL={}s)",
                entry.service_id,
                entry.instance_id,
                entry.eventgroup_id,
                client_endpoint,
                transport,
                entry.ttl
            );
        }

        // Only notify the application of NEW subscriptions, not renewals
        // This prevents spurious ServiceEvent::Subscribe for offer-triggered renewals
        if is_new_subscription {
            let _ = offered
                .requests_tx
                .try_send(super::command::ServiceRequest::Subscribe {
                    eventgroup_id: entry.eventgroup_id,
                    client: client_endpoint,
                    transport,
                });
        }

        let mut ack = SdMessage::new(state.sd_flags(true));
        // Per feat_req_someipsd_614: TTL shall be the same as in the SubscribeEventgroup
        ack.add_entry(SdEntry::subscribe_eventgroup_ack(
            entry.service_id,
            entry.instance_id,
            entry.major_version,
            entry.eventgroup_id,
            entry.ttl, // Echo client's TTL per spec
            entry.counter,
        ));

        // Queue ACK for time-based clustering
        state.queue_unicast_sd(ack, from);
    } else {
        tracing::warn!(
            "Received SubscribeEventgroup for unknown service {:04x}:{:04x} eventgroup {:04x} from {}",
            entry.service_id,
            entry.instance_id,
            entry.eventgroup_id,
            from
        );

        let mut nack = SdMessage::new(state.sd_flags(true));
        nack.add_entry(SdEntry::subscribe_eventgroup_nack(
            entry.service_id,
            entry.instance_id,
            entry.major_version,
            entry.eventgroup_id,
            entry.counter,
        ));

        state.queue_unicast_sd(nack, from);
    }
}

/// Handle a `StopSubscribeEventgroup` request
pub fn handle_unsubscribe_request(
    entry: &SdEntry,
    sd_message: &SdMessage,
    from: SocketAddr,
    state: &mut RuntimeState,
) {
    tracing::debug!(
        "Received StopSubscribe for {:04x}:{:04x} eventgroup {:04x} from {}",
        entry.service_id,
        entry.instance_id,
        entry.eventgroup_id,
        from
    );

    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
        major_version: entry.major_version,
    };

    if let Some(offered) = state.offered.get(&key) {
        // Per feat_req_someipsd_814: Use endpoint option address as provided
        let client_udp_endpoint = sd_message.get_udp_endpoint(entry);
        let client_tcp_endpoint = sd_message.get_tcp_endpoint(entry);

        // Validate that endpoint options contain valid, routable IP addresses
        // Reject unspecified (0.0.0.0) addresses
        if let Some(ep) = client_udp_endpoint {
            if ep.ip().is_unspecified() {
                tracing::warn!(
                    "Rejecting unsubscribe for {:04x}:{:04x} eventgroup {:04x} from {}: \
                     endpoint option has unspecified IP address (0.0.0.0)",
                    entry.service_id,
                    entry.instance_id,
                    entry.eventgroup_id,
                    from
                );
                return;
            }
        }
        if let Some(ep) = client_tcp_endpoint {
            if ep.ip().is_unspecified() {
                tracing::warn!(
                    "Rejecting unsubscribe for {:04x}:{:04x} eventgroup {:04x} from {}: \
                     endpoint option has unspecified IP address (0.0.0.0)",
                    entry.service_id,
                    entry.instance_id,
                    entry.eventgroup_id,
                    from
                );
                return;
            }
        }

        let Some((client_endpoint, transport)) = offered
            .udp_endpoint
            .and_then(|_| client_udp_endpoint.map(|ep| (ep, crate::config::Transport::Udp)))
            .or_else(|| {
                offered
                    .tcp_endpoint
                    .and_then(|_| client_tcp_endpoint.map(|ep| (ep, crate::config::Transport::Tcp)))
            })
        else {
            // Transport mismatch - send NACK
            tracing::warn!(
                "Rejecting subscription for {:04x}:{:04x} eventgroup {:04x} from {}: \
                    transport mismatch (server offers UDP={}, TCP={}; client wants UDP={}, TCP={})",
                entry.service_id,
                entry.instance_id,
                entry.eventgroup_id,
                from,
                offered.udp_endpoint.is_some(),
                offered.tcp_endpoint.is_some(),
                client_udp_endpoint.is_some(),
                client_tcp_endpoint.is_some()
            );
            return;
        };

        let _ = offered
            .requests_tx
            .try_send(super::command::ServiceRequest::Unsubscribe {
                eventgroup_id: entry.eventgroup_id,
                client: client_endpoint,
                transport,
            });

        // Remove the subscriber
        let sub_key = SubscriberKey {
            service_id: entry.service_id,
            instance_id: entry.instance_id,
            major_version: entry.major_version,
            eventgroup_id: entry.eventgroup_id,
        };
        if let Some(subscribers) = state.server_subscribers.get_mut(&sub_key) {
            // Remove the subscriber with matching endpoint
            subscribers.retain(|sub| sub.endpoint != client_endpoint);
        }
    } else {
        tracing::warn!(
            "Received Unsubscribe for unknown service {:04x}:{:04x} eventgroup {:04x} from {}",
            entry.service_id,
            entry.instance_id,
            entry.eventgroup_id,
            from
        );
    }
}

/// Handle a `SubscribeEventgroupAck`
pub fn handle_subscribe_ack(entry: &SdEntry, state: &mut RuntimeState) {
    tracing::debug!(
        "Subscription acknowledged for {:04x}:{:04x} eventgroup {:04x}",
        entry.service_id,
        entry.instance_id,
        entry.eventgroup_id
    );

    // Get pending subscriptions for this eventgroup
    let pending_key = PendingSubscriptionKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
        major_version: entry.major_version,
        eventgroup_id: entry.eventgroup_id,
    };

    if let Some(pending_list) = state.pending_subscriptions.remove(&pending_key) {
        for pending in pending_list {
            // Check if this is part of a multi-eventgroup subscription
            let multi_key = MultiEventgroupSubscriptionKey {
                service_id: entry.service_id,
                instance_id: entry.instance_id,
                major_version: entry.major_version,
                subscription_id: pending.subscription_id,
            };

            if let Some(multi_sub) = state.multi_eventgroup_subscriptions.get_mut(&multi_key) {
                // Mark this eventgroup as ACKed
                multi_sub.acked_eventgroups.insert(entry.eventgroup_id);

                // Check if ALL eventgroups in this subscription are now ACKed
                let all_acked = multi_sub
                    .eventgroup_ids
                    .iter()
                    .all(|eg| multi_sub.acked_eventgroups.contains(eg));

                if all_acked {
                    // Remove from multi_eventgroup_subscriptions and send success
                    if let Some(completed) = state.multi_eventgroup_subscriptions.remove(&multi_key)
                    {
                        if let Some(response) = completed.response {
                            let _ = response.send(Ok(pending.subscription_id));
                        }
                    }
                }
                // If not all ACKed yet, wait for more ACKs
            } else if let Some(response) = pending.response {
                // Single-eventgroup subscription - respond immediately
                let _ = response.send(Ok(pending.subscription_id));
            }
        }
    }
}

/// Handle a `SubscribeEventgroupNack`
pub fn handle_subscribe_nack(entry: &SdEntry, state: &mut RuntimeState) {
    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
        major_version: entry.major_version,
    };

    tracing::debug!(
        "Subscription rejected for {:04x}:{:04x} v{} eventgroup {:04x}",
        entry.service_id,
        entry.instance_id,
        entry.major_version,
        entry.eventgroup_id
    );

    // Get pending subscriptions for this eventgroup
    let pending_key = PendingSubscriptionKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
        major_version: entry.major_version,
        eventgroup_id: entry.eventgroup_id,
    };

    if let Some(pending_list) = state.pending_subscriptions.remove(&pending_key) {
        for pending in pending_list {
            // Check if this is part of a multi-eventgroup subscription
            let multi_key = MultiEventgroupSubscriptionKey {
                service_id: entry.service_id,
                instance_id: entry.instance_id,
                major_version: entry.major_version,
                subscription_id: pending.subscription_id,
            };

            if let Some(multi_sub) = state.multi_eventgroup_subscriptions.remove(&multi_key) {
                // NACK received - fail the entire multi-eventgroup subscription immediately
                if let Some(response) = multi_sub.response {
                    let _ = response.send(Err(crate::error::Error::SubscriptionRejected));
                }

                // Clean up any eventgroups that were already added to subscriptions
                if let Some(subs) = state.subscriptions.get_mut(&key) {
                    subs.retain(|s| s.subscription_id != pending.subscription_id);
                }

                // Remove remaining pending subscriptions for other eventgroups of this subscription
                for &eg_id in &multi_sub.eventgroup_ids {
                    if eg_id != entry.eventgroup_id {
                        let other_pending_key = PendingSubscriptionKey {
                            service_id: entry.service_id,
                            instance_id: entry.instance_id,
                            major_version: entry.major_version,
                            eventgroup_id: eg_id,
                        };
                        if let Some(other_pending_list) =
                            state.pending_subscriptions.get_mut(&other_pending_key)
                        {
                            other_pending_list
                                .retain(|p| p.subscription_id != pending.subscription_id);
                        }
                    }
                }
            } else if let Some(response) = pending.response {
                // Single-eventgroup subscription - respond with error
                let _ = response.send(Err(crate::error::Error::SubscriptionRejected));
            }
        }
    }

    // Also remove the client subscriptions for this eventgroup if they were added
    if let Some(subs) = state.subscriptions.get_mut(&key) {
        subs.retain(|s| s.eventgroup_id != entry.eventgroup_id);
    }
}

// ============================================================================
// SD MESSAGE BUILDING
// ============================================================================

/// Build an `OfferService` SD message for the given offered service.
///
/// This advertises all configured endpoints (TCP and/or UDP) in the SD message.
/// If `advertised_ip` is provided, it overrides the IP from the stored endpoints.
pub fn build_offer_message(
    key: ServiceKey,
    offered: &OfferedService,
    sd_flags: u8,
    ttl: u32,
    advertised_ip: Option<std::net::IpAddr>,
) -> SdMessage {
    let mut msg = SdMessage::new(sd_flags);
    let mut option_indices = Vec::new();

    // Helper to get the IP address - use advertised_ip if set, otherwise use endpoint IP
    let get_ip = |ep: SocketAddr| -> Ipv4Addr {
        if let Some(std::net::IpAddr::V4(ip)) = advertised_ip {
            ip
        } else {
            match ep {
                SocketAddr::V4(v4) => *v4.ip(),
                _ => Ipv4Addr::LOCALHOST,
            }
        }
    };

    // Add UDP endpoint option if present
    if let Some(ep) = offered.udp_endpoint {
        let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
            addr: get_ip(ep),
            port: ep.port(),
            protocol: L4Protocol::Udp,
        });
        option_indices.push(opt_idx);
    }

    // Add TCP endpoint option if present
    if let Some(ep) = offered.tcp_endpoint {
        let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
            addr: get_ip(ep),
            port: ep.port(),
            protocol: L4Protocol::Tcp,
        });
        option_indices.push(opt_idx);
    }

    let first_opt_idx = option_indices.first().copied().unwrap_or(0);
    let num_options = option_indices.len() as u8;

    msg.add_entry(SdEntry::offer_service(
        key.service_id,
        key.instance_id,
        offered.major_version,
        offered.minor_version,
        ttl,
        first_opt_idx,
        num_options,
    ));
    msg
}

/// Build a `StopOfferService` SD message
pub fn build_stop_offer_message(
    key: ServiceKey,
    offered: &OfferedService,
    sd_flags: u8,
) -> SdMessage {
    let mut msg = SdMessage::new(sd_flags);
    msg.add_entry(SdEntry::stop_offer_service(
        key.service_id,
        key.instance_id,
        offered.major_version,
        offered.minor_version,
    ));
    msg
}

/// Build a `FindService` SD message
pub fn build_find_message(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    sd_flags: u8,
    ttl: u32,
) -> SdMessage {
    let mut msg = SdMessage::new(sd_flags);
    msg.add_entry(SdEntry::find_service(
        service_id,
        instance_id,
        major_version,
        0xFFFF_FFFF, // Minor version is always ANY on wire per spec
        ttl,
    ));
    msg
}

/// Build a `SubscribeEventgroup` SD message
pub fn build_subscribe_message(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    local_endpoint: SocketAddr,
    client_rpc_port: u16,
    sd_flags: u8,
    ttl: u32,
    transport: Transport,
) -> SdMessage {
    build_subscribe_message_multi(
        service_id,
        instance_id,
        major_version,
        &[eventgroup_id],
        local_endpoint,
        client_rpc_port,
        sd_flags,
        ttl,
        transport,
    )
}

/// Build a Subscribe SD message with multiple eventgroups
///
/// Clusters multiple `SubscribeEventgroup` entries into ONE SD message
/// to prevent duplicate session IDs and reboot detection issues.
pub fn build_subscribe_message_multi(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_ids: &[u16],
    local_endpoint: SocketAddr,
    client_rpc_port: u16,
    sd_flags: u8,
    ttl: u32,
    transport: Transport,
) -> SdMessage {
    let protocol = match transport {
        Transport::Tcp => L4Protocol::Tcp,
        Transport::Udp => L4Protocol::Udp,
    };

    let mut msg = SdMessage::new(sd_flags);
    let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
        addr: match local_endpoint {
            SocketAddr::V4(v4) => *v4.ip(),
            _ => Ipv4Addr::LOCALHOST,
        },
        port: client_rpc_port,
        protocol,
    });

    // Add one SubscribeEventgroup entry per eventgroup, all sharing the same endpoint option
    for &eventgroup_id in eventgroup_ids {
        let mut entry = SdEntry::subscribe_eventgroup(
            service_id,
            instance_id,
            major_version,
            eventgroup_id,
            ttl,
            0,
        );
        entry.index_1st_option = opt_idx;
        entry.num_options_1 = 1;
        msg.add_entry(entry);
    }
    msg
}

/// Build an Unsubscribe (`StopSubscribeEventgroup`) SD message
pub fn build_unsubscribe_message(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    local_endpoint: SocketAddr,
    client_rpc_port: u16,
    sd_flags: u8,
    transport: Transport,
) -> SdMessage {
    build_unsubscribe_message_multi(
        service_id,
        instance_id,
        major_version,
        &[eventgroup_id],
        local_endpoint,
        client_rpc_port,
        sd_flags,
        transport,
    )
}

/// Build an Unsubscribe SD message with multiple eventgroups
///
/// Clusters multiple `StopSubscribeEventgroup` entries into ONE SD message
/// to prevent duplicate session IDs and reboot detection issues.
pub fn build_unsubscribe_message_multi(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_ids: &[u16],
    local_endpoint: SocketAddr,
    client_rpc_port: u16,
    sd_flags: u8,
    transport: Transport,
) -> SdMessage {
    let protocol = match transport {
        Transport::Tcp => L4Protocol::Tcp,
        Transport::Udp => L4Protocol::Udp,
    };

    let mut msg = SdMessage::new(sd_flags);
    // Include the same endpoint option as in Subscribe so server knows which subscriber to remove
    let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
        addr: match local_endpoint {
            SocketAddr::V4(v4) => *v4.ip(),
            _ => Ipv4Addr::LOCALHOST,
        },
        port: client_rpc_port,
        protocol,
    });

    // Add one StopSubscribeEventgroup entry per eventgroup, all sharing the same endpoint option
    for &eventgroup_id in eventgroup_ids {
        let mut entry = SdEntry::subscribe_eventgroup(
            service_id,
            instance_id,
            major_version,
            eventgroup_id,
            0, // TTL=0 indicates unsubscribe
            0,
        );
        entry.index_1st_option = opt_idx;
        entry.num_options_1 = 1;
        msg.add_entry(entry);
    }
    msg
}
