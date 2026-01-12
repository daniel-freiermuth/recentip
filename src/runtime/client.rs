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
//! Runtime receives command
//!        │
//!        ▼
//! handle_call() adds to pending_calls, returns Action::SendClientMessage
//!        │
//!        ▼
//! Runtime sends message via client RPC socket
//!        │
//!        ▼
//! Server processes and responds
//!        │
//!        ▼
//! Runtime receives on client RPC socket
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

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use tokio::time::Instant;

use super::command::ServiceAvailability;
use super::sd::{build_find_message, build_subscribe_message, build_unsubscribe_message, Action};
use super::state::{
    CallKey, ClientSubscription, FindRequest, PendingCall, PendingSubscription,
    PendingSubscriptionKey, RuntimeState, ServiceKey,
};
use crate::config::{Transport, DEFAULT_FIND_REPETITIONS};
use crate::net::TcpStream;
use crate::tcp::TcpConnectionPool;
use crate::wire::Header;
use crate::{Event, EventId, Response, ReturnCode};

// ============================================================================
// COMMAND HANDLERS (CLIENT-SIDE)
// ============================================================================

/// Determine the routable IP address to use in endpoint options
///
/// Per `feat_req_recentipsd_814`, endpoint options must contain valid routable IP addresses.
/// Returns the configured advertised IP, or falls back to `local_endpoint/client_rpc_endpoint`
/// if they have non-unspecified IPs, or None if no valid IP is available.
fn get_endpoint_ip(state: &RuntimeState) -> Option<std::net::IpAddr> {
    if let Some(advertised) = state.config.advertised_ip {
        Some(advertised)
    } else if !state.local_endpoint.ip().is_unspecified() {
        Some(state.local_endpoint.ip())
    } else {
        None
    }
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
        .find(|(k, _v)| k.matches(&key))
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
                last_find: Instant::now() - Duration::from_secs(10),
            },
        );

        let msg = build_find_message(
            service_id.value(),
            instance_id.value(),
            major_version.value(),
            state.sd_flags(true),
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
    payload: Bytes,
    response: tokio::sync::oneshot::Sender<crate::error::Result<Response>>,
    target_endpoint: SocketAddr,
    target_transport: Transport,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let session_id = state.next_session_id();
    let client_id = state.client_id;

    // Build request message
    let request_data = build_request(
        service_id.value(),
        method_id,
        client_id,
        session_id,
        1, // interface version
        &payload,
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
    payload: Bytes,
    target_endpoint: SocketAddr,
    target_transport: Transport,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let session_id = state.next_session_id();
    let client_id = state.client_id;

    // Build fire-and-forget message (no response tracking needed)
    let request_data = build_fire_and_forget(
        service_id.value(),
        method_id,
        client_id,
        session_id,
        1, // interface version
        &payload,
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
/// a TCP connection before subscribing, as required by `feat_req_recentipsd_767`:
///
/// > "The client shall open a TCP connection to the server and should be ready
/// > to receive message on that connection before sending the SubscribeEventgroup entry"
///
/// For UDP subscriptions, this behaves identically to the sync version.
/// For TCP subscriptions, it first establishes a TCP connection to the server,
/// then uses that connection's local address in the subscribe message.
pub async fn handle_subscribe_command<T: TcpStream>(
    service_id: crate::ServiceId,
    instance_id: crate::InstanceId,
    major_version: u8,
    eventgroup_id: u16,
    events: tokio::sync::mpsc::Sender<Event>,
    response: tokio::sync::oneshot::Sender<crate::error::Result<u64>>,
    state: &mut RuntimeState,
    tcp_pool: &TcpConnectionPool<T>,
) -> Option<Vec<Action>> {
    let key = ServiceKey::new(service_id, instance_id, major_version);

    // Generate a unique subscription ID for this subscription handle
    let subscription_id = state.next_subscription_id();

    let Some(discovered) = state.discovered.get(&key) else {
        tracing::debug!(
            "Cannot subscribe to {:04x}:{:04x} v{} eventgroup {:04x}: service not discovered",
            service_id.value(),
            instance_id.value(),
            major_version,
            eventgroup_id
        );
        let _ = response.send(Err(crate::error::Error::ServiceUnavailable));
        return None;
    };

    let prefer_tcp = state.config.preferred_transport == Transport::Tcp;
    let Some((_method_endpoint, transport)) = discovered.method_endpoint(prefer_tcp) else {
        tracing::debug!(
            "Trying to subscribe to eventgroup {:04x} on service {:04x} instance {:04x}, but server offers no compatible transport endpoints",
            eventgroup_id,
            service_id.value(),
            instance_id.value()
        );
        let _ = response.send(Err(crate::error::Error::Config(
            crate::error::ConfigError::new("Server offers no compatible transport endpoints"),
        )));
        return None;
    };

    // For TCP subscriptions, establish connection BEFORE subscribing (feat_req_recentipsd_767)
    // Use the actual TCP connection's local address in the subscribe message
    let endpoint_for_subscribe = if transport == Transport::Tcp {
        // Get the server's TCP RPC endpoint
        let Some(tcp_endpoint) = discovered.tcp_endpoint else {
            tracing::error!(
                "TCP transport selected but no TCP endpoint for {:04x}:{:04x}",
                service_id.value(),
                instance_id.value()
            );
            let _ = response.send(Err(crate::error::Error::Config(
                crate::error::ConfigError::new("TCP transport selected but no TCP endpoint"),
            )));
            return None;
        };

        // Establish TCP connection (or get existing connection's local address)
        match tcp_pool.ensure_connected(tcp_endpoint).await {
            Ok(local_addr) => {
                tracing::debug!(
                    "TCP connection established to {} (local addr: {}) for subscription to {:04x}:{:04x} eventgroup {:04x}",
                    tcp_endpoint,
                    local_addr,
                    service_id.value(),
                    instance_id.value(),
                    eventgroup_id
                );
                local_addr
            }
            Err(e) => {
                tracing::error!(
                    "Failed to establish TCP connection to {} for subscription: {}",
                    tcp_endpoint,
                    e
                );
                let _ = response.send(Err(crate::error::Error::Io(e)));
                return None;
            }
        }
    } else {
        // UDP: use our UDP RPC endpoint
        let Some(endpoint_ip) = get_endpoint_ip(state) else {
            tracing::error!(
                "Cannot subscribe to {:04x}:{:04x} eventgroup {:04x}: \
                    no valid IP address configured. Set RuntimeConfig::advertised_ip",
                service_id.value(),
                instance_id.value(),
                eventgroup_id
            );
            let _ = response.send(Err(crate::error::Error::Config(
                crate::error::ConfigError::new("No advertised IP configured for subscriptions"),
            )));
            return None;
        };
        SocketAddr::new(endpoint_ip, state.client_rpc_endpoint.port())
    };

    // Need to re-borrow discovered after the await
    let sd_endpoint = state.discovered.get(&key).unwrap().sd_endpoint;

    state
        .subscriptions
        .entry(key)
        .or_default()
        .push(ClientSubscription {
            subscription_id,
            eventgroup_id,
            events_tx: events,
        });

    // Store pending subscription - will be resolved when ACK/NACK is received
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
        response,
    });

    // Only send SD Subscribe message if this is the first waiter
    if is_first_waiter {
        tracing::debug!(
            "Subscribing to {:04x}:{:04x} v{} eventgroup {:04x} via {:?} (endpoint: {})",
            service_id.value(),
            instance_id.value(),
            major_version,
            eventgroup_id,
            transport,
            endpoint_for_subscribe
        );

        let msg = build_subscribe_message(
            service_id.value(),
            instance_id.value(),
            major_version,
            eventgroup_id,
            endpoint_for_subscribe,
            endpoint_for_subscribe.port(),
            state.sd_flags(true),
            state.config.subscribe_ttl,
            transport,
        );

        let mut actions = Vec::new();
        actions.push(Action::SendSd {
            message: msg,
            target: sd_endpoint,
        });
        Some(actions)
    } else {
        None
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
    actions: &mut Vec<Action>,
) {
    let key = ServiceKey::new(service_id, instance_id, major_version);

    // Check if we have subscriptions for this key
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

    // Remove the specific subscription by ID
    subs.retain(|s| s.subscription_id != subscription_id);

    // Count remaining subscriptions for this eventgroup
    let remaining_for_eventgroup = subs
        .iter()
        .filter(|s| s.eventgroup_id == eventgroup_id)
        .count();

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
        // Per feat_req_recentipsd_814, we must provide a valid routable IP, not 0.0.0.0
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

        let endpoint_for_unsubscribe =
            SocketAddr::new(endpoint_ip, state.client_rpc_endpoint.port());

        let msg = build_unsubscribe_message(
            service_id.value(),
            instance_id.value(),
            major_version,
            eventgroup_id,
            endpoint_for_unsubscribe,
            state.client_rpc_endpoint.port(),
            state.sd_flags(true),
            transport,
        );

        actions.push(Action::SendSd {
            message: msg,
            target: discovered.sd_endpoint, // Send to SD socket, not RPC socket
        });
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
    state: &mut RuntimeState,
) {
    // Method ID is the event ID for notifications
    let event_id = match EventId::new(header.method_id) {
        Some(id) => id,
        None => return, // Invalid event ID
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
    if let Some(subs) = state.subscriptions.get(&key) {
        for sub in subs {
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

use crate::wire::{MessageType, PROTOCOL_VERSION};
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
