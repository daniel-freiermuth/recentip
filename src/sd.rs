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

use crate::command::ServiceAvailability;
use crate::config::{Transport, SD_TTL_INFINITE};
use crate::state::{
    DiscoveredService, OfferedService, PendingServerResponse, PendingSubscriptionKey,
    RuntimeState, ServerSubscription, ServiceKey, SubscriberKey,
};
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
    /// Send a SOME/IP RPC message as a client (uses client RPC socket)
    SendClientMessage { data: Bytes, target: SocketAddr },
    /// Send a SOME/IP RPC message as a server (uses service's RPC socket)
    SendServerMessage {
        service_key: ServiceKey,
        data: Bytes,
        target: SocketAddr,
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
}

// ============================================================================
// SD MESSAGE HANDLERS
// ============================================================================

/// Handle an `OfferService` entry
///
/// When we receive an OfferService, we:
/// 1. Update our discovered services table
/// 2. Notify any pending find requests
/// 3. **[feat_req_recentipsd_428/431/631]** If we have active subscriptions for this service,
///    send SubscribeEventgroup in response (offer-triggered subscription)
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
    let Some((effective_endpoint, effective_transport)) = (if prefer_tcp {
        tcp_endpoint
            .map(|ep| (ep, crate::config::Transport::Tcp))
            .or_else(|| {
                udp_endpoint.map(|ep| (ep, crate::config::Transport::Udp))
            })
    } else {
        udp_endpoint
            .map(|ep| (ep, crate::config::Transport::Udp))
            .or_else(|| {
                tcp_endpoint.map(|ep| (ep, crate::config::Transport::Tcp))
            })
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
            major_version: entry.major_version,
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

    // [feat_req_recentipsd_428/431/631] Offer-triggered subscription renewal
    // If we have active subscriptions for this service, respond with SubscribeEventgroup.
    // Per feat_req_recentipsd_631: "Subscriptions shall NOT be triggered cyclically
    // but SHALL be triggered by OfferService entries."
    
    // Capture values needed for building messages before mutable borrow
    let local_endpoint = state.local_endpoint;
    let client_rpc_port = state.client_rpc_endpoint.port();
    let sd_flags = state.sd_flags(true);
    
    if let Some(subscriptions) = state.subscriptions.get_mut(&key) {
        for sub in subscriptions.iter_mut() {
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

            tracing::debug!(
                "Offer-triggered subscription renewal for {:04x}:{:04x} eventgroup {:04x} via {:?}",
                entry.service_id,
                entry.instance_id,
                sub.eventgroup_id,
                effective_transport
            );

            // Build and send SubscribeEventgroup message
            let msg = build_subscribe_message(
                entry.service_id,
                entry.instance_id,
                sub.eventgroup_id,
                local_endpoint,
                client_rpc_port,
                sd_flags,
                state.config.subscribe_ttl,
                effective_transport,
            );

            actions.push(Action::SendSd {
                message: msg,
                target: from, // Send to the SD source of the offer
            });
        }
    }
}

/// Handle a `StopOfferService` entry
pub fn handle_stop_offer(
    entry: &SdEntry,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
    };

    if state.discovered.remove(&key).is_some() {
        tracing::debug!(
            "Service {:04x}:{:04x} stopped offering",
            entry.service_id,
            entry.instance_id
        );

        actions.push(Action::NotifyFound {
            key,
            availability: ServiceAvailability::Unavailable,
        });

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
pub fn handle_find_request(
    entry: &SdEntry,
    from: SocketAddr,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    // Determine protocol based on transport configuration
    let protocol = match state.config.transport {
        Transport::Tcp => L4Protocol::Tcp,
        Transport::Udp => L4Protocol::Udp,
    };

    for (key, offered) in &state.offered {
        // Only respond if the service is actively announcing
        if !offered.is_announcing {
            continue;
        }

        if entry.service_id == key.service_id
            && (entry.instance_id == 0xFFFF || entry.instance_id == key.instance_id)
        {
            let mut response = SdMessage::new(state.sd_flags(true));
            let opt_idx = response.add_option(SdOption::Ipv4Endpoint {
                addr: match offered.rpc_endpoint {
                    SocketAddr::V4(v4) => *v4.ip(),
                    _ => Ipv4Addr::LOCALHOST,
                },
                port: offered.rpc_endpoint.port(), // Use RPC endpoint port, not SD port!
                protocol,
            });
            response.add_entry(SdEntry::offer_service(
                key.service_id,
                key.instance_id,
                offered.major_version,
                offered.minor_version,
                state.config.offer_ttl,
                opt_idx,
                1,
            ));

            actions.push(Action::SendSd {
                message: response,
                target: from,
            });
        }
    }
}

/// Handle a `SubscribeEventgroup` request
pub fn handle_subscribe_request(
    entry: &SdEntry,
    sd_message: &SdMessage,
    from: SocketAddr,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
    };

    // Get client's event endpoint from SD option, falling back to source address
    // If the endpoint has unspecified IP (0.0.0.0), use the source IP with the option's port
    let client_endpoint = match sd_message.get_udp_endpoint(entry) {
        Some(ep) if !ep.ip().is_unspecified() => ep,
        Some(ep) => SocketAddr::new(from.ip(), ep.port()),
        None => from,
    };

    if let Some(offered) = state.offered.get(&key) {
        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        let _ = offered
            .requests_tx
            .try_send(crate::command::ServiceRequest::Subscribe {
                eventgroup_id: entry.eventgroup_id,
                client: client_endpoint,
                response: response_tx,
            });

        // Track the subscriber - use the endpoint from the option for event delivery
        // Calculate expiration based on client's TTL from the Subscribe message
        // Per feat_req_recentipsd_431: TTL=0xFFFFFF means "until next reboot" (infinite)
        let expires_at = if entry.ttl == SD_TTL_INFINITE {
            None // Infinite TTL - never expires
        } else {
            let ttl_duration = Duration::from_secs(u64::from(entry.ttl));
            Some(Instant::now() + ttl_duration)
        };

        let sub_key = SubscriberKey {
            service_id: entry.service_id,
            instance_id: entry.instance_id,
            eventgroup_id: entry.eventgroup_id,
        };

        // Check if this client already has a subscription - if so, update the expiration
        let subscribers = state.server_subscribers.entry(sub_key).or_default();
        if let Some(existing) = subscribers.iter_mut().find(|s| s.endpoint == client_endpoint) {
            // Renew: update expiration time
            existing.expires_at = expires_at;
            tracing::debug!(
                "Renewed subscription for {:04x}:{:04x} eventgroup {:04x} from {} (TTL={}s)",
                entry.service_id,
                entry.instance_id,
                entry.eventgroup_id,
                client_endpoint,
                entry.ttl
            );
        } else {
            // New subscription
            subscribers.push(ServerSubscription {
                endpoint: client_endpoint,
                expires_at,
            });
            tracing::debug!(
                "New subscription for {:04x}:{:04x} eventgroup {:04x} from {} (TTL={}s)",
                entry.service_id,
                entry.instance_id,
                entry.eventgroup_id,
                client_endpoint,
                entry.ttl
            );
        }

        let mut ack = SdMessage::new(state.sd_flags(true));
        // Per feat_req_recentipsd_614: TTL shall be the same as in the SubscribeEventgroup
        ack.add_entry(SdEntry::subscribe_eventgroup_ack(
            entry.service_id,
            entry.instance_id,
            entry.major_version,
            entry.eventgroup_id,
            entry.ttl, // Echo client's TTL per spec
            entry.counter,
        ));

        // Send ACK to the SD source (not the event endpoint)
        actions.push(Action::SendSd {
            message: ack,
            target: from,
        });
    }
}

/// Handle a `StopSubscribeEventgroup` request
pub fn handle_unsubscribe_request(
    entry: &SdEntry,
    sd_message: &SdMessage,
    from: SocketAddr,
    state: &mut RuntimeState,
) {
    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
    };

    // Get client's event endpoint from SD option (same as subscribe)
    let client_endpoint = match sd_message.get_udp_endpoint(entry) {
        Some(ep) if !ep.ip().is_unspecified() => ep,
        Some(ep) => SocketAddr::new(from.ip(), ep.port()),
        None => from,
    };

    if let Some(offered) = state.offered.get(&key) {
        let _ = offered
            .requests_tx
            .try_send(crate::command::ServiceRequest::Unsubscribe {
                eventgroup_id: entry.eventgroup_id,
                client: client_endpoint,
            });

        // Remove the subscriber
        let sub_key = SubscriberKey {
            service_id: entry.service_id,
            instance_id: entry.instance_id,
            eventgroup_id: entry.eventgroup_id,
        };
        if let Some(subscribers) = state.server_subscribers.get_mut(&sub_key) {
            subscribers.retain(|sub| sub.endpoint != client_endpoint);
        }
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

    // Resolve pending subscription with success
    let pending_key = PendingSubscriptionKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
        eventgroup_id: entry.eventgroup_id,
    };
    if let Some(pending) = state.pending_subscriptions.remove(&pending_key) {
        let _ = pending.response.send(Ok(()));
    }
}

/// Handle a `SubscribeEventgroupNack`
pub fn handle_subscribe_nack(entry: &SdEntry, state: &mut RuntimeState) {
    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
    };

    tracing::debug!(
        "Subscription rejected for {:04x}:{:04x} eventgroup {:04x}",
        entry.service_id,
        entry.instance_id,
        entry.eventgroup_id
    );

    // Resolve pending subscription with error
    let pending_key = PendingSubscriptionKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
        eventgroup_id: entry.eventgroup_id,
    };
    if let Some(pending) = state.pending_subscriptions.remove(&pending_key) {
        let _ = pending.response.send(Err(crate::error::Error::SubscriptionRejected));
    }

    // Also remove the client subscription if it was added
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
pub fn build_offer_message(
    key: &ServiceKey,
    offered: &OfferedService,
    sd_flags: u8,
    ttl: u32,
) -> SdMessage {
    let mut msg = SdMessage::new(sd_flags);
    let mut option_indices = Vec::new();

    // Add UDP endpoint option if present
    if let Some(ep) = offered.udp_endpoint {
        let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
            addr: match ep {
                SocketAddr::V4(v4) => *v4.ip(),
                _ => Ipv4Addr::LOCALHOST,
            },
            port: ep.port(),
            protocol: L4Protocol::Udp,
        });
        option_indices.push(opt_idx);
    }

    // Add TCP endpoint option if present
    if let Some(ep) = offered.tcp_endpoint {
        let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
            addr: match ep {
                SocketAddr::V4(v4) => *v4.ip(),
                _ => Ipv4Addr::LOCALHOST,
            },
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
    key: &ServiceKey,
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
    sd_flags: u8,
    ttl: u32,
) -> SdMessage {
    let mut msg = SdMessage::new(sd_flags);
    msg.add_entry(SdEntry::find_service(
        service_id,
        instance_id,
        0xFF,
        0xFFFFFFFF,
        ttl,
    ));
    msg
}

/// Build a `SubscribeEventgroup` SD message
pub fn build_subscribe_message(
    service_id: u16,
    instance_id: u16,
    eventgroup_id: u16,
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
    let mut entry =
        SdEntry::subscribe_eventgroup(service_id, instance_id, 0xFF, eventgroup_id, ttl, 0);
    entry.index_1st_option = opt_idx;
    entry.num_options_1 = 1;
    msg.add_entry(entry);
    msg
}

/// Build an Unsubscribe (`StopSubscribeEventgroup`) SD message
pub fn build_unsubscribe_message(
    service_id: u16,
    instance_id: u16,
    eventgroup_id: u16,
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
    let mut entry = SdEntry::subscribe_eventgroup(
        service_id,
        instance_id,
        0xFF,
        eventgroup_id,
        0, // TTL=0 indicates unsubscribe
        0,
    );
    entry.index_1st_option = opt_idx;
    entry.num_options_1 = 1;
    msg.add_entry(entry);
    msg
}
