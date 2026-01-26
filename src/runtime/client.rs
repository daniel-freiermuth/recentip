//! # Client-Side Handlers (Internal)
//!
//! This module contains handlers for client-side SOME/IP operations.
//! It is `pub(crate)` — internal to the library.
//!
//! ## Responsibilities
//!
//! | Handler | Command | Purpose |
//! |---------|---------|----------|
//! | `handle_find` | `Find` | Register service discovery request |
//! | `handle_call` | `Call` | Send RPC request, track pending response |
//! | `handle_fire_and_forget` | `FireAndForget` | Send one-way request |
//! | `handle_subscribe` | `Subscribe` | Request eventgroup subscription |
//! | `handle_unsubscribe` | `Unsubscribe` | Cancel subscription |
//! | `handle_incoming_response` | — | Process RPC response from server |
//! | `handle_incoming_notification` | — | Process event from server |
//!
//! ## Data Flow
//!
//! ```text
//! User calls proxy.call()
//!        │
//!        ▼
//! OfferedService sends Command::Call
//!        │
//!        ▼
//! SomeIp receives command
//!        │
//!        ▼
//! handle_call() adds to pending_calls, returns Action::SendClientMessage
//!        │
//!        ▼
//! SomeIp sends message via client RPC socket
//!        │
//!        ▼
//! Server processes and responds
//!        │
//!        ▼
//! SomeIp receives on client RPC socket
//!        │
//!        ▼
//! handle_incoming_response() matches pending_call, sends result via oneshot
//!        │
//!        ▼
//! User's .await completes with Response
//! ```
//!
//! ## Contributor Notes
//!
//! - All handlers take `&mut RuntimeState` and return `Vec<Action>`
//! - No I/O is performed directly; actions are returned to the runtime
//! - Session IDs are allocated from `state.next_client_session_id()`

use std::collections::HashSet;
use std::net::SocketAddr;

use bytes::Bytes;
use tokio::time::Instant;

use super::command::ServiceAvailability;
use super::sd::{
    build_find_message, build_subscribe_message_multi, build_unsubscribe_message, Action,
};
use super::state::{
    CallKey, ClientSubscription, FindRequest, MultiEventgroupSubscription,
    MultiEventgroupSubscriptionKey, PendingCall, PendingSubscription, PendingSubscriptionKey,
    RuntimeState, ServiceKey,
};
use crate::config::{Transport, DEFAULT_FIND_REPETITIONS};
use crate::net::{TcpStream, UdpSocket};
use crate::tcp::TcpConnectionPool;
use crate::wire::{Header, MessageType};
use crate::{Event, EventId, Response, ReturnCode};
use tokio::sync::mpsc;

// ============================================================================
// PER-SUBSCRIPTION UDP SOCKET
// ============================================================================

/// Helper: Spawn a per-subscription UDP socket task
///
/// Creates a dedicated UDP socket for a subscription, binds it to an ephemeral port,
/// and spawns a task to receive events and route them directly to the subscription's
/// `events_tx` channel.
///
/// Returns the local endpoint (IP + ephemeral port) to use in the SD Subscribe message.
async fn spawn_udp_subscription_socket<U: UdpSocket>(
    advertised_ip: std::net::IpAddr,
    events_tx: mpsc::Sender<crate::Event>,
    service_id: crate::ServiceId,
    instance_id: crate::InstanceId,
    eventgroup_id: u16,
) -> std::io::Result<SocketAddr> {
    // Bind to 0.0.0.0:0 (ephemeral port on all interfaces)
    // This works in both production (tokio) and testing (turmoil)
    let bind_addr = SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, 0));
    let socket = U::bind(bind_addr).await?;
    let mut local_endpoint = socket.local_addr()?;

    // Replace 0.0.0.0 with the advertised IP for the SD Subscribe message
    local_endpoint.set_ip(advertised_ip);

    tracing::debug!(
        "Created dedicated UDP socket {} for subscription to {:04x}:{:04x} eventgroup {:04x}",
        local_endpoint,
        service_id.value(),
        instance_id.value(),
        eventgroup_id
    );

    // Spawn task to receive events on this dedicated socket
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((len, from)) => {
                    let data = &buf[..len];
                    let mut cursor = data;

                    // Parse SOME/IP header
                    let Some(header) = Header::parse(&mut cursor) else {
                        tracing::warn!(
                            "Received invalid SOME/IP header on subscription socket {} from {}",
                            local_endpoint,
                            from
                        );
                        continue;
                    };

                    // Only process notifications (events)
                    if header.message_type != MessageType::Notification {
                        tracing::trace!(
                            "Ignoring non-notification message type {:?} on subscription socket {}",
                            header.message_type,
                            local_endpoint
                        );
                        continue;
                    }

                    // Extract event
                    let Some(event_id) = EventId::new(header.method_id) else {
                        tracing::warn!(
                            "Invalid event ID {:04x} on subscription socket {}",
                            header.method_id,
                            local_endpoint
                        );
                        continue;
                    };

                    let payload_len = header.payload_length();
                    if cursor.len() < payload_len {
                        tracing::warn!(
                            "Incomplete payload on subscription socket {}: expected {}, got {}",
                            local_endpoint,
                            payload_len,
                            cursor.len()
                        );
                        continue;
                    }

                    let payload = Bytes::copy_from_slice(&cursor[..payload_len]);
                    let event = Event { event_id, payload };

                    // Route event directly to this subscription's channel
                    if events_tx.try_send(event).is_err() {
                        // Subscription dropped - exit task
                        tracing::debug!(
                            "Subscription dropped for socket {}, shutting down receiver task",
                            local_endpoint
                        );
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!(
                        "Error receiving on subscription socket {}: {}",
                        local_endpoint,
                        e
                    );
                    break;
                }
            }
        }

        tracing::debug!("UDP subscription socket {} task exiting", local_endpoint);
    });

    Ok(local_endpoint)
}

// ============================================================================
// COMMAND HANDLERS (CLIENT-SIDE)
// ============================================================================

/// Determine the routable IP address to use in endpoint options
///
/// Per `feat_req_someipsd_814`, endpoint options must contain valid routable IP addresses.
/// Returns the configured advertised IP, or falls back to `local_endpoint/client_rpc_endpoint`
/// if they have non-unspecified IPs, or None if no valid IP is available.
fn get_endpoint_ip(state: &RuntimeState) -> Option<std::net::IpAddr> {
    state.config.advertised_ip.or_else(|| {
        if state.local_endpoint.ip().is_unspecified() {
            None
        } else {
            Some(state.local_endpoint.ip())
        }
    })
}

/// Handle `Command::Find`
pub fn handle_find(
    service_id: crate::ServiceId,
    instance_id: crate::InstanceId,
    major_version: crate::MajorVersion,
    notify: tokio::sync::mpsc::Sender<ServiceAvailability>,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let key = ServiceKey::new(service_id, instance_id, major_version.value());
    let prefer_tcp = state.config.preferred_transport == Transport::Tcp;

    // Check if we already have a matching discovered service
    // Use wildcard-aware matching via DiscoveredServiceKey::matches()
    let found = state
        .discovered
        .iter()
        .find(|(k, _v)| k.matches(key))
        .and_then(|(k, v)| {
            v.method_endpoint(prefer_tcp)
                .map(|(ep, tr)| (k.instance_id, k.major_version, ep, tr))
        });

    if let Some((discovered_instance_id, discovered_major_version, endpoint, transport)) = found {
        let _ = notify.try_send(ServiceAvailability::Available {
            endpoint,
            transport,
            instance_id: discovered_instance_id,
            major_version: discovered_major_version,
        });
    } else {
        state.find_requests.insert(
            key,
            FindRequest {
                notify,
                repetitions_left: DEFAULT_FIND_REPETITIONS,
                last_find: Instant::now(),
            },
        );

        let msg = build_find_message(
            service_id.value(),
            instance_id.value(),
            major_version.value(),
            state.sd_flags(false), // Multicast FindService, so FLAG_UNICAST=0
            state.config.find_ttl,
        );

        actions.push(Action::SendSd {
            message: msg,
            target: state.config.sd_multicast,
        });
    }
}

/// Handle `Command::StopFind`
pub fn handle_stop_find(
    service_id: crate::ServiceId,
    instance_id: crate::InstanceId,
    major_version: crate::MajorVersion,
    state: &mut RuntimeState,
) {
    let key = ServiceKey::new(service_id, instance_id, major_version.value());
    state.find_requests.remove(&key);
    // TODO: here we probably should only remove our interest. There might be another find
    //       but if we're the last one, we need to do the cleanup
}

/// Handle `Command::Call`
pub fn handle_call(
    service_id: crate::ServiceId,
    method_id: u16,
    payload: &Bytes,
    response: tokio::sync::oneshot::Sender<crate::error::Result<Response>>,
    target_endpoint: SocketAddr,
    target_transport: Transport,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let session_id = state.next_payload_session_id();
    let client_id = state.client_id;

    // Build request message
    let request_data = build_request(
        service_id.value(),
        method_id,
        client_id,
        session_id,
        1, // interface version
        payload,
    );

    // Register pending call
    let call_key = CallKey {
        client_id,
        session_id,
    };
    state
        .pending_calls
        .insert(call_key, PendingCall { response });

    actions.push(Action::SendClientMessage {
        data: request_data,
        target: target_endpoint,
        transport: target_transport,
    });
}

/// Handle `Command::FireAndForget`
pub fn handle_fire_and_forget(
    service_id: crate::ServiceId,
    method_id: u16,
    payload: &Bytes,
    target_endpoint: SocketAddr,
    target_transport: Transport,
    state: &RuntimeState,
    actions: &mut Vec<Action>,
) {
    let session_id = 0;
    let client_id = state.client_id;

    // Build fire-and-forget message (no response tracking needed)
    let request_data = build_fire_and_forget(
        service_id.value(),
        method_id,
        client_id,
        session_id,
        1, // interface version
        payload,
    );

    actions.push(Action::SendClientMessage {
        data: request_data,
        target: target_endpoint,
        transport: target_transport,
    });
}

/// Handle `Command::Subscribe` with async TCP connection support
///
/// This is the async version of `handle_subscribe` that supports establishing
/// a TCP connection before subscribing, as required by `feat_req_someipsd_767`:
///
/// > "The client shall open a TCP connection to the server and should be ready
/// > to receive message on that connection before sending the SubscribeEventgroup entry"
///
/// For UDP subscriptions, this behaves identically to the sync version.
/// For TCP subscriptions, it first establishes a TCP connection to the server,
/// then uses that connection's local address in the subscribe message.
///
/// For UDP subscriptions, creates a dedicated UDP socket with ephemeral port per subscription
/// to enable proper eventgroup isolation (per-subscription sockets like TCP's per-subscription connections).
/// Handle `Command::Subscribe` - subscribe to multiple eventgroups with shared endpoint
pub async fn handle_subscribe_command<U: UdpSocket, T: TcpStream>(
    service_id: crate::ServiceId,
    instance_id: crate::InstanceId,
    major_version: u8,
    eventgroup_ids: Vec<u16>,
    events: tokio::sync::mpsc::Sender<Event>,
    response: tokio::sync::oneshot::Sender<crate::error::Result<u64>>,
    state: &mut RuntimeState,
    tcp_pool: &TcpConnectionPool<T>,
) {
    if eventgroup_ids.is_empty() {
        let _ = response.send(Err(crate::error::Error::Config(
            crate::error::ConfigError::new("At least one eventgroup must be specified"),
        )));
        return;
    }

    let key = ServiceKey::new(service_id, instance_id, major_version);

    // Generate a single subscription ID for all eventgroups
    let subscription_id = state.next_subscription_id();

    let Some(discovered) = state.discovered.get(&key) else {
        tracing::debug!(
            "Cannot subscribe to {:04x}:{:04x} v{} eventgroups {:?}: service not discovered",
            service_id.value(),
            instance_id.value(),
            major_version,
            eventgroup_ids
        );
        let _ = response.send(Err(crate::error::Error::ServiceUnavailable));
        return;
    };

    // Extract sd_endpoint early - we'll need it later after mutable borrows of state
    let sd_endpoint = discovered.sd_endpoint;

    let prefer_tcp = state.config.preferred_transport == Transport::Tcp;
    let Some((_method_endpoint, transport)) = discovered.method_endpoint(prefer_tcp) else {
        tracing::debug!(
            "Trying to subscribe to eventgroups {:?} on service {:04x} instance {:04x}, but server offers no compatible transport endpoints",
            eventgroup_ids,
            service_id.value(),
            instance_id.value()
        );
        let _ = response.send(Err(crate::error::Error::Config(
            crate::error::ConfigError::new("Server offers no compatible transport endpoints"),
        )));
        return;
    };

    // For TCP subscriptions, establish connection BEFORE subscribing (feat_req_someipsd_767)
    //
    // TCP connection sharing strategy (same as UDP endpoint sharing):
    // - Different services on same server: SHARE connection (conn_key=slot)
    //   → Events route correctly because service_id is in header
    // - Same service, different eventgroups: SEPARATE connections (different conn_key)
    //   → Events can't be routed by eventgroup (not in header), so need separate connections
    //
    // Strategy: Use "connection slots" to share connections across services.
    // - conn_key 0: first subscription "slot" for each service (shared)
    // - conn_key 1: second subscription "slot" for each service (shared)
    // - etc.
    //
    // Returns (endpoint, tcp_conn_key) where tcp_conn_key is used for event routing
    let (endpoint_for_subscribe, tcp_conn_key) = if transport == Transport::Tcp {
        let Some(tcp_endpoint) = discovered.tcp_endpoint else {
            tracing::error!(
                "TCP transport selected but no TCP endpoint for {:04x}:{:04x}",
                service_id.value(),
                instance_id.value()
            );
            let _ = response.send(Err(crate::error::Error::Config(
                crate::error::ConfigError::new("TCP transport selected but no TCP endpoint"),
            )));
            return;
        };

        // Find the smallest unused conn_key (slot) for this service
        // This enables sharing connections across services while isolating within same service
        //
        // Why not just count existing subscriptions? Consider this scenario:
        // 1. Sub1 (EG1+EG2) gets conn_key=0, Sub2 (EG3+EG4) gets conn_key=1
        // 2. Sub1 is dropped
        // 3. Sub1_new (EG1+EG2) should get conn_key=0 again (the freed slot), not conn_key=1
        //
        // If we just counted existing subs, Sub1_new would get conn_key=1, sharing with Sub2,
        // causing events to be misrouted.
        let conn_key = {
            let used_conn_keys: std::collections::HashSet<u64> = state
                .subscriptions
                .get(&key)
                .map_or_else(HashSet::default, |subs| {
                    subs.iter().map(|sub| sub.tcp_conn_key).collect()
                });

            // Find smallest unused slot
            let mut slot = 0u64;
            while used_conn_keys.contains(&slot) {
                slot += 1;
            }
            slot
        };

        match tcp_pool.ensure_connected(tcp_endpoint, conn_key).await {
            Ok(local_addr) => {
                tracing::debug!(
                    "TCP connection established to {} (local addr: {}, conn_key: {}) for subscription to {:04x}:{:04x} eventgroups {:?}",
                    tcp_endpoint,
                    local_addr,
                    conn_key,
                    service_id.value(),
                    instance_id.value(),
                    eventgroup_ids
                );

                // Register endpoint usage for consistency with UDP tracking
                state.register_subscription_endpoint(
                    local_addr.port(),
                    service_id.value(),
                    instance_id.value(),
                );

                (local_addr, conn_key)
            }
            Err(e) => {
                tracing::error!(
                    "Failed to establish TCP connection to {} for subscription: {}",
                    tcp_endpoint,
                    e
                );
                let _ = response.send(Err(crate::error::Error::Io(e)));
                return;
            }
        }
    } else {
        // UDP subscription endpoint selection:
        // - Reuse shared endpoint across DIFFERENT services (for efficiency)
        // - Use dedicated socket for SAME service different eventgroup (for correct event routing)
        //
        // The SOME/IP event wire format doesn't include eventgroup info, so if multiple
        // eventgroups of the same service share an endpoint, the server sends all events
        // to that endpoint and the client can't distinguish which subscription should
        // receive which event. By using separate endpoints per service, the server
        // correctly routes events based on the subscription's endpoint.
        let Some(endpoint_ip) = get_endpoint_ip(state) else {
            tracing::error!(
                "Cannot subscribe to {:04x}:{:04x} eventgroups {:?}: \
                    no valid IP address configured. Set RuntimeConfig::advertised_ip",
                service_id.value(),
                instance_id.value(),
                eventgroup_ids
            );
            let _ = response.send(Err(crate::error::Error::Config(
                crate::error::ConfigError::new("No advertised IP configured for subscriptions"),
            )));
            return;
        };

        // Check if this service already has a subscription - if so, need a different endpoint
        // to ensure events are routed to the correct subscription
        let service_already_has_subscription = state
            .subscriptions
            .get(&key)
            .is_some_and(|subs| !subs.is_empty());

        if service_already_has_subscription {
            // This service already has a subscription using some endpoint.
            // Try to REUSE an existing endpoint from a DIFFERENT service first.
            // Only create a new socket if no reusable endpoint exists.
            if let Some(reusable_port) =
                state.find_reusable_subscription_endpoint(service_id.value(), instance_id.value())
            {
                // Found an endpoint used by other services but not this one - reuse it!
                let reused_endpoint = SocketAddr::new(endpoint_ip, reusable_port);
                tracing::debug!(
                    "Reusing existing endpoint {} for subscription to {:04x}:{:04x} eventgroups {:?}",
                    reused_endpoint,
                    service_id.value(),
                    instance_id.value(),
                    eventgroup_ids
                );

                // Register this service's usage of the endpoint
                state.register_subscription_endpoint(
                    reusable_port,
                    service_id.value(),
                    instance_id.value(),
                );

                // Store subscription - NOT a dedicated socket since we're sharing
                let subs = state.subscriptions.entry(key).or_default();
                for &eventgroup_id in &eventgroup_ids {
                    subs.push(ClientSubscription {
                        subscription_id,
                        eventgroup_id,
                        events_tx: events.clone(),
                        local_endpoint: reused_endpoint,
                        has_dedicated_socket: false, // Shared endpoint
                        tcp_conn_key: 0,
                    });
                }

                // Send subscribe messages
                let mut response_opt = Some(response);

                let mut eventgroups_to_subscribe = Vec::new();
                for &eventgroup_id in &eventgroup_ids {
                    let pending_key = PendingSubscriptionKey {
                        service_id: service_id.value(),
                        instance_id: instance_id.value(),
                        major_version,
                        eventgroup_id,
                    };
                    let pending_list = state.pending_subscriptions.entry(pending_key).or_default();
                    let is_first_waiter = pending_list.is_empty();
                    pending_list.push(PendingSubscription {
                        subscription_id,
                        response: response_opt.take(),
                    });

                    if is_first_waiter {
                        eventgroups_to_subscribe.push(eventgroup_id);
                    }
                }

                // Queue subscribe for time-based clustering
                if !eventgroups_to_subscribe.is_empty() {
                    tracing::debug!(
                        "Queueing subscribe to {:04x}:{:04x} v{} eventgroups {:?} via reused UDP endpoint {} for time-based clustering",
                        service_id.value(),
                        instance_id.value(),
                        major_version,
                        eventgroups_to_subscribe,
                        reused_endpoint
                    );

                    let msg = build_subscribe_message_multi(
                        service_id.value(),
                        instance_id.value(),
                        major_version,
                        &eventgroups_to_subscribe,
                        reused_endpoint,
                        reused_endpoint.port(),
                        state.sd_flags(true),
                        state.config.subscribe_ttl,
                        Transport::Udp,
                    );
                    state.queue_unicast_sd(msg, sd_endpoint);
                }

                // Return None since actions are queued for later flush
                return;
            }

            // No reusable endpoint found - create a new dedicated socket
            match spawn_udp_subscription_socket::<U>(
                endpoint_ip,
                events.clone(),
                service_id,
                instance_id,
                eventgroup_ids.first().copied().unwrap_or(0),
            )
            .await
            {
                Ok(dedicated_endpoint) => {
                    tracing::debug!(
                        "Created dedicated UDP socket {} for subscription to {:04x}:{:04x} eventgroups {:?} (no reusable endpoint)",
                        dedicated_endpoint,
                        service_id.value(),
                        instance_id.value(),
                        eventgroup_ids
                    );

                    // Register this new endpoint's usage
                    state.register_subscription_endpoint(
                        dedicated_endpoint.port(),
                        service_id.value(),
                        instance_id.value(),
                    );

                    // Store subscription with dedicated socket flag
                    // Track all eventgroups with a shared events channel
                    let subs = state.subscriptions.entry(key).or_default();
                    for &eventgroup_id in &eventgroup_ids {
                        subs.push(ClientSubscription {
                            subscription_id,
                            eventgroup_id,
                            events_tx: events.clone(),
                            local_endpoint: dedicated_endpoint,
                            has_dedicated_socket: true, // Events received on dedicated socket
                            tcp_conn_key: 0,            // Not used for UDP subscriptions
                        });
                    }

                    // Track pending subscriptions and send subscribe messages
                    let mut response_opt = Some(response);

                    let mut eventgroups_to_subscribe = Vec::new();
                    for &eventgroup_id in &eventgroup_ids {
                        let pending_key = PendingSubscriptionKey {
                            service_id: service_id.value(),
                            instance_id: instance_id.value(),
                            major_version,
                            eventgroup_id,
                        };
                        let pending_list =
                            state.pending_subscriptions.entry(pending_key).or_default();
                        let is_first_waiter = pending_list.is_empty();
                        pending_list.push(PendingSubscription {
                            subscription_id,
                            response: response_opt.take(),
                        });

                        if is_first_waiter {
                            eventgroups_to_subscribe.push(eventgroup_id);
                        }
                    }

                    // Queue subscribe for time-based clustering
                    if !eventgroups_to_subscribe.is_empty() {
                        tracing::debug!(
                            "Queueing subscribe to {:04x}:{:04x} v{} eventgroups {:?} via dedicated UDP endpoint {} for time-based clustering",
                            service_id.value(),
                            instance_id.value(),
                            major_version,
                            eventgroups_to_subscribe,
                            dedicated_endpoint
                        );

                        let msg = build_subscribe_message_multi(
                            service_id.value(),
                            instance_id.value(),
                            major_version,
                            &eventgroups_to_subscribe,
                            dedicated_endpoint,
                            dedicated_endpoint.port(),
                            state.sd_flags(true),
                            state.config.subscribe_ttl,
                            Transport::Udp,
                        );
                        state.queue_unicast_sd(msg, sd_endpoint);
                    }

                    // Return None since actions are queued for later flush
                    return;
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to create dedicated UDP socket for subscription: {}",
                        e
                    );
                    let _ = response.send(Err(crate::error::Error::Io(e)));
                    return;
                }
            }
        }
        // First subscription for this service - use the shared RPC endpoint
        // Register this service's usage of the client_rpc_endpoint
        state.register_subscription_endpoint(
            state.client_rpc_endpoint.port(),
            service_id.value(),
            instance_id.value(),
        );
        // tcp_conn_key=0 since this is UDP, not used for routing
        (
            SocketAddr::new(endpoint_ip, state.client_rpc_endpoint.port()),
            0,
        )
    };

    // Track all eventgroups with a shared events channel (cloned sender)
    let subs = state.subscriptions.entry(key).or_default();
    for &eventgroup_id in &eventgroup_ids {
        subs.push(ClientSubscription {
            subscription_id,
            eventgroup_id,
            events_tx: events.clone(),
            local_endpoint: endpoint_for_subscribe,
            has_dedicated_socket: false, // Both UDP and TCP use shared sockets/connections; events route via handle_incoming_notification
            tcp_conn_key, // Track which TCP connection this subscription uses for event routing
        });
    }

    // Track pending subscriptions for all eventgroups

    // For multi-eventgroup subscriptions, use "all or nothing" tracking
    // All eventgroups must ACK for success; any NACK fails the whole subscription
    let is_multi_eventgroup = eventgroup_ids.len() > 1;
    let mut response_opt = Some(response);

    if is_multi_eventgroup {
        // Register this as a multi-eventgroup subscription
        let multi_key = MultiEventgroupSubscriptionKey {
            service_id: service_id.value(),
            instance_id: instance_id.value(),
            major_version,
            subscription_id,
        };
        state.multi_eventgroup_subscriptions.insert(
            multi_key,
            MultiEventgroupSubscription {
                eventgroup_ids: eventgroup_ids.clone(),
                acked_eventgroups: std::collections::HashSet::new(),
                response: response_opt.take(), // Move response to multi-eventgroup tracker
            },
        );
    }

    // Collect eventgroups that need Subscribe messages (first waiter only)
    let mut eventgroups_to_subscribe = Vec::new();

    for &eventgroup_id in &eventgroup_ids {
        let pending_key = PendingSubscriptionKey {
            service_id: service_id.value(),
            instance_id: instance_id.value(),
            major_version,
            eventgroup_id,
        };
        let pending_list = state.pending_subscriptions.entry(pending_key).or_default();
        let is_first_waiter = pending_list.is_empty();

        // For single-eventgroup subscriptions, use the response channel directly
        // For multi-eventgroup subscriptions, the response is in multi_eventgroup_subscriptions
        pending_list.push(PendingSubscription {
            subscription_id,
            response: if is_multi_eventgroup {
                None // Response is tracked in multi_eventgroup_subscriptions
            } else {
                response_opt.take() // Single eventgroup gets the response directly
            },
        });

        // Collect eventgroups that need Subscribe messages
        if is_first_waiter {
            eventgroups_to_subscribe.push(eventgroup_id);
        }
    }

    // Send ONE SD message with all SubscribeEventgroup entries
    // This prevents duplicate session IDs and reboot detection issues
    if !eventgroups_to_subscribe.is_empty() {
        tracing::debug!(
            "Queueing subscribe to {:04x}:{:04x} v{} eventgroups {:?} via {:?} (endpoint: {}) for time-based clustering",
            service_id.value(),
            instance_id.value(),
            major_version,
            eventgroups_to_subscribe,
            transport,
            endpoint_for_subscribe
        );

        let msg = build_subscribe_message_multi(
            service_id.value(),
            instance_id.value(),
            major_version,
            &eventgroups_to_subscribe,
            endpoint_for_subscribe,
            endpoint_for_subscribe.port(),
            state.sd_flags(true),
            state.config.subscribe_ttl,
            transport,
        );

        // Queue action for time-based clustering instead of sending immediately
        state.queue_unicast_sd(msg, sd_endpoint);
    }
}

/// Handle `Command::Unsubscribe`
pub fn handle_unsubscribe(
    service_id: crate::ServiceId,
    instance_id: crate::InstanceId,
    major_version: u8,
    eventgroup_id: u16,
    subscription_id: u64,
    state: &mut RuntimeState,
) {
    let key = ServiceKey::new(service_id, instance_id, major_version);

    // First, gather info we need while holding an immutable borrow, then modify
    let (removed_endpoint, remaining_for_eventgroup, port_still_in_use) = {
        let Some(subs) = state.subscriptions.get_mut(&key) else {
            tracing::warn!(
                "Unsubscribe for unknown service sub_id {} on {:04x}:{:04x} v{} eventgroup {:04x}",
                subscription_id,
                service_id.value(),
                instance_id.value(),
                major_version,
                eventgroup_id
            );
            return;
        };

        // Capture the local_endpoint of the subscription being removed (for endpoint reuse tracking)
        // Note: A multi-eventgroup subscription has multiple entries (one per eventgroup),
        // all sharing the same subscription_id but with different eventgroup_ids.
        // We need to find and remove only the entry for this specific eventgroup.
        let removed_endpoint = subs
            .iter()
            .find(|s| s.subscription_id == subscription_id && s.eventgroup_id == eventgroup_id)
            .map(|s| s.local_endpoint);

        // Remove only the entry for this specific eventgroup (not all entries with this subscription_id)
        subs.retain(|s| {
            !(s.subscription_id == subscription_id && s.eventgroup_id == eventgroup_id)
        });

        // Check if this specific port is still used by any remaining subscription for this service
        let port_still_in_use = removed_endpoint.is_none_or(|endpoint| {
            subs.iter()
                .any(|s| s.local_endpoint.port() == endpoint.port())
        });

        // Count remaining subscriptions for this eventgroup
        let remaining_for_eventgroup = subs
            .iter()
            .filter(|s| s.eventgroup_id == eventgroup_id)
            .count();

        (
            removed_endpoint,
            remaining_for_eventgroup,
            port_still_in_use,
        )
    };

    // If this port is no longer used by any subscription for this service, unregister it
    // This allows the port to be reused by future subscriptions
    if !port_still_in_use {
        if let Some(endpoint) = removed_endpoint {
            state.unregister_subscription_endpoint(
                endpoint.port(),
                service_id.value(),
                instance_id.value(),
            );
            tracing::debug!(
                "Unregistered service {:04x}:{:04x} from endpoint port {} (no more subscriptions using this port)",
                service_id.value(),
                instance_id.value(),
                endpoint.port()
            );
        }
    }

    // Only send SD StopSubscribe if this was the last subscription for this eventgroup
    if remaining_for_eventgroup > 0 {
        tracing::debug!(
            "Subscription {} dropped for {:04x}:{:04x} v{} eventgroup {:04x}, but {} other(s) remain",
            subscription_id,
            service_id.value(),
            instance_id.value(),
            major_version,
            eventgroup_id,
            remaining_for_eventgroup
        );
        return;
    }

    // Must have the removed endpoint to build the StopSubscribe message
    let Some(subscription_endpoint) = removed_endpoint else {
        tracing::error!(
            "Cannot send StopSubscribe for {:04x}:{:04x} eventgroup {:04x}: \
             subscription endpoint was not captured",
            service_id.value(),
            instance_id.value(),
            eventgroup_id
        );
        return;
    };

    if let Some(discovered) = state.discovered.get(&key) {
        let prefer_tcp = state.config.preferred_transport == crate::config::Transport::Tcp;
        let Some(transport) = discovered.method_endpoint(prefer_tcp).map(|(_ep, t)| t) else {
            // Server offers nothing
            tracing::error!(
                "Trying to unsubscribe from eventgroup {:04x} on service {:04x} instance {:04x}, but server offers no transport endpoints",
                eventgroup_id,
                service_id.value(),
                instance_id.value()
            );
            return;
        };

        tracing::info!(
            "Unsubscribing from {:04x}:{:04x} v{} eventgroup {:04x}",
            service_id.value(),
            instance_id.value(),
            major_version,
            eventgroup_id
        );

        // Determine the actual local IP address to put in the endpoint option
        // Per feat_req_someipsd_814, we must provide a valid routable IP, not 0.0.0.0
        let Some(endpoint_ip) = get_endpoint_ip(state) else {
            tracing::error!(
                "Cannot unsubscribe from {:04x}:{:04x} eventgroup {:04x}: \
                 no valid IP address configured. Set RuntimeConfig::advertised_ip",
                service_id.value(),
                instance_id.value(),
                eventgroup_id
            );
            return;
        };

        // Use the same endpoint that was used for the subscription
        // Per feat_req_someipsd_1177: StopSubscribeEventgroup shall reference the same
        // options the SubscribeEventgroup Entry referenced
        let endpoint_for_unsubscribe = SocketAddr::new(endpoint_ip, subscription_endpoint.port());

        let msg = build_unsubscribe_message(
            service_id.value(),
            instance_id.value(),
            major_version,
            eventgroup_id,
            endpoint_for_unsubscribe,
            subscription_endpoint.port(),
            state.sd_flags(true),
            transport,
        );

        // Queue unsubscribe for time-based clustering with other SD messages
        state.queue_unicast_sd(msg, discovered.sd_endpoint);
    } else {
        tracing::warn!(
            "Trying to unsubscribe from eventgroup {:04x} on service {:04x} instance {:04x}, but service not discovered",
            eventgroup_id,
            service_id.value(),
            instance_id.value()
        );
    }
}

// ============================================================================
// INCOMING MESSAGE HANDLERS (CLIENT-SIDE)
// ============================================================================

/// Handle an incoming response (client-side)
pub fn handle_incoming_response(header: &Header, payload: Bytes, state: &mut RuntimeState) {
    let call_key = CallKey {
        client_id: header.client_id,
        session_id: header.session_id,
    };

    if let Some(pending) = state.pending_calls.remove(&call_key) {
        let return_code = match header.return_code {
            0x00 => ReturnCode::Ok,
            0x01 => ReturnCode::NotOk,
            0x02 => ReturnCode::UnknownService,
            0x03 => ReturnCode::UnknownMethod,
            0x04 => ReturnCode::NotReady,
            0x05 => ReturnCode::NotReachable,
            0x06 => ReturnCode::Timeout,
            0x07 => ReturnCode::WrongProtocolVersion,
            0x08 => ReturnCode::WrongInterfaceVersion,
            0x09 => ReturnCode::MalformedMessage,
            0x0A => ReturnCode::WrongMessageType,
            _ => ReturnCode::NotOk,
        };

        let response = Response {
            return_code,
            payload,
        };

        let _ = pending.response.send(Ok(response));
    } else {
        tracing::trace!(
            "Received response for unknown call {:04x}:{:04x}",
            header.client_id,
            header.session_id
        );
    }
}

/// Handle an incoming notification (event)
pub fn handle_incoming_notification(
    header: &Header,
    payload: Bytes,
    from: SocketAddr,
    state: &RuntimeState,
    subscription_id: u64,
) {
    // Method ID is the event ID for notifications
    let Some(event_id) = EventId::new(header.method_id) else {
        return; // Invalid event ID
    };

    let event = Event { event_id, payload };

    // Determine which service instance this event is from by looking up the 'from' address
    // in discovered services
    let Some(key): Option<ServiceKey> = state
        .discovered
        .iter()
        .find(|(key, disc)| {
            key.service_id == header.service_id
                && (disc.udp_endpoint == Some(from) || disc.tcp_endpoint == Some(from))
        })
        .map(|(key, _)| *key)
    else {
        tracing::warn!(
            "Received event {:04x} from unknown service {:04x} at {}",
            header.method_id,
            header.service_id,
            from
        );
        return;
    };

    // Find subscriptions for this service (dynamic via SD)
    // TCP connection sharing strategy:
    // - Different services CAN share connection (route by service_id in header)
    // - Same service different eventgroups use separate connections (slot-based conn_key)
    //
    // Routing logic uses tcp_conn_key to match events to subscriptions:
    // - Events arrive with a connection's subscription_id (the conn_key used when connecting)
    // - Route to subscriptions with matching tcp_conn_key
    if let Some(subs) = state.subscriptions.get(&key) {
        for sub in subs {
            // UDP subscriptions have dedicated sockets and skip this path entirely
            if sub.has_dedicated_socket {
                continue; // This subscription receives events on its own dedicated socket
            }

            // TCP subscriptions: Route based on connection key matching
            // The subscription_id in TcpMessage is the conn_key used when the connection was established.
            // Route events to subscriptions with matching tcp_conn_key.
            if sub.tcp_conn_key != subscription_id {
                continue;
            }

            let _ = sub.events_tx.try_send(event.clone());
        }
    }

    // Also check static listeners - iterate through all eventgroups
    // The event doesn't contain eventgroup info in the header, so we route to
    // all listeners for this service/instance combination
    for (key, events_tx) in &state.static_listeners {
        if key.service_id == header.service_id {
            let _ = events_tx.try_send(event.clone());
        }
    }
}

// ============================================================================
// MESSAGE BUILDING (CLIENT-SIDE)
// ============================================================================

use crate::wire::PROTOCOL_VERSION;
use bytes::BytesMut;

/// Build a SOME/IP request message
pub fn build_request(
    service_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    interface_version: u8,
    payload: &[u8],
) -> Bytes {
    let length = 8 + payload.len() as u32;

    let mut buf = BytesMut::with_capacity(Header::SIZE + payload.len());

    let header = Header {
        service_id,
        method_id,
        length,
        client_id,
        session_id,
        protocol_version: PROTOCOL_VERSION,
        interface_version,
        message_type: MessageType::Request,
        return_code: 0x00,
    };

    header.serialize(&mut buf);
    buf.extend_from_slice(payload);
    buf.freeze()
}

/// Build a SOME/IP fire-and-forget (`REQUEST_NO_RETURN`) message
pub fn build_fire_and_forget(
    service_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    interface_version: u8,
    payload: &[u8],
) -> Bytes {
    let length = 8 + payload.len() as u32;

    let mut buf = BytesMut::with_capacity(Header::SIZE + payload.len());

    let header = Header {
        service_id,
        method_id,
        length,
        client_id,
        session_id,
        protocol_version: PROTOCOL_VERSION,
        interface_version,
        message_type: MessageType::RequestNoReturn,
        return_code: 0x00,
    };

    header.serialize(&mut buf);
    buf.extend_from_slice(payload);
    buf.freeze()
}
