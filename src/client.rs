//! Client-side handlers for SOME/IP runtime.
//!
//! Handles service discovery (find), RPC calls, subscriptions, and incoming responses/events.

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use tokio::time::Instant;

use crate::command::ServiceAvailability;
use crate::config::{Transport, DEFAULT_FIND_REPETITIONS};
use crate::sd::{build_find_message, build_subscribe_message, build_unsubscribe_message, Action};
use crate::state::{
    CallKey, ClientSubscription, FindRequest, PendingCall,
    RuntimeState, ServiceKey,
};
use crate::wire::Header;
use crate::{Event, EventId, Response, ReturnCode};

// ============================================================================
// COMMAND HANDLERS (CLIENT-SIDE)
// ============================================================================

/// Handle Command::Find
pub(crate) fn handle_find(
    service_id: crate::ServiceId,
    instance_id: crate::InstanceId,
    notify: tokio::sync::mpsc::Sender<ServiceAvailability>,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let key = ServiceKey::new(service_id, instance_id);
    let prefer_tcp = state.config.transport == Transport::Tcp;

    // Check if we already have a matching discovered service
    // Use wildcard-aware matching: if instance_id is Any (0xFFFF), match any instance
    let found = if instance_id == crate::InstanceId::Any {
        // Find any service with matching service_id
        state
            .discovered
            .iter()
            .find(|(k, _)| k.service_id == service_id.value())
            .and_then(|(k, v)| v.rpc_endpoint(prefer_tcp).map(|ep| (k.instance_id, ep)))
    } else {
        // Exact match
        state
            .discovered
            .get(&key)
            .and_then(|v| v.rpc_endpoint(prefer_tcp).map(|ep| (key.instance_id, ep)))
    };

    if let Some((discovered_instance_id, endpoint)) = found {
        let _ = notify.try_send(ServiceAvailability::Available {
            endpoint,
            instance_id: discovered_instance_id,
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
            state.sd_flags(true),
            state.config.ttl,
        );

        actions.push(Action::SendSd {
            message: msg,
            target: state.config.sd_multicast,
        });
    }
}

/// Handle Command::StopFind
pub(crate) fn handle_stop_find(
    service_id: crate::ServiceId,
    instance_id: crate::InstanceId,
    state: &mut RuntimeState,
) {
    let key = ServiceKey::new(service_id, instance_id);
    state.find_requests.remove(&key);
}

/// Handle Command::Call
pub(crate) fn handle_call(
    service_id: crate::ServiceId,
    instance_id: crate::InstanceId,
    method_id: u16,
    payload: Bytes,
    response: tokio::sync::oneshot::Sender<crate::error::Result<Response>>,
    target_endpoint: Option<SocketAddr>,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let key = ServiceKey::new(service_id, instance_id);
    let prefer_tcp = state.config.transport == Transport::Tcp;

    // Use static endpoint if provided, otherwise look up from discovered services
    let endpoint = target_endpoint.or_else(|| {
        state
            .discovered
            .get(&key)
            .and_then(|d| d.rpc_endpoint(prefer_tcp))
            .or_else(|| {
                // If searching for Any, find any instance of this service
                if instance_id.is_any() {
                    state
                        .discovered
                        .iter()
                        .find(|(k, _)| k.service_id == service_id.value())
                        .and_then(|(_, v)| v.rpc_endpoint(prefer_tcp))
                } else {
                    None
                }
            })
    });

    if let Some(endpoint) = endpoint {
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
            target: endpoint,
        });
    } else {
        let _ = response.send(Err(crate::error::Error::ServiceUnavailable));
    }
}

/// Handle Command::FireAndForget
pub(crate) fn handle_fire_and_forget(
    service_id: crate::ServiceId,
    instance_id: crate::InstanceId,
    method_id: u16,
    payload: Bytes,
    target_endpoint: Option<SocketAddr>,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let key = ServiceKey::new(service_id, instance_id);
    let prefer_tcp = state.config.transport == Transport::Tcp;

    // Use static endpoint if provided, otherwise look up from discovered services
    let endpoint = target_endpoint.or_else(|| {
        state
            .discovered
            .get(&key)
            .and_then(|d| d.rpc_endpoint(prefer_tcp))
            .or_else(|| {
                if instance_id.is_any() {
                    state
                        .discovered
                        .iter()
                        .find(|(k, _)| k.service_id == service_id.value())
                        .and_then(|(_, v)| v.rpc_endpoint(prefer_tcp))
                } else {
                    None
                }
            })
    });

    if let Some(endpoint) = endpoint {
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
            target: endpoint,
        });
    }
    // Note: No error response for fire-and-forget - it's best effort
}

/// Handle Command::Subscribe
pub(crate) fn handle_subscribe(
    service_id: crate::ServiceId,
    instance_id: crate::InstanceId,
    eventgroup_id: u16,
    events: tokio::sync::mpsc::Sender<Event>,
    response: tokio::sync::oneshot::Sender<crate::error::Result<()>>,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let key = ServiceKey::new(service_id, instance_id);

    if let Some(discovered) = state.discovered.get(&key) {
        state
            .subscriptions
            .entry(key)
            .or_insert_with(Vec::new)
            .push(ClientSubscription {
                eventgroup_id,
                events_tx: events,
            });

        let msg = build_subscribe_message(
            service_id.value(),
            instance_id.value(),
            eventgroup_id,
            state.local_endpoint,
            state.client_rpc_endpoint.port(),
            state.sd_flags(true),
            state.config.ttl,
            state.config.transport,
        );

        actions.push(Action::SendSd {
            message: msg,
            target: discovered.sd_endpoint, // Send to SD socket, not RPC socket
        });

        let _ = response.send(Ok(()));
    } else {
        let _ = response.send(Err(crate::error::Error::ServiceUnavailable));
    }
}

/// Handle Command::Unsubscribe
pub(crate) fn handle_unsubscribe(
    service_id: crate::ServiceId,
    instance_id: crate::InstanceId,
    eventgroup_id: u16,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let key = ServiceKey::new(service_id, instance_id);

    if let Some(subs) = state.subscriptions.get_mut(&key) {
        subs.retain(|s| s.eventgroup_id != eventgroup_id);
    }

    if let Some(discovered) = state.discovered.get(&key) {
        let msg = build_unsubscribe_message(
            service_id.value(),
            instance_id.value(),
            eventgroup_id,
            state.local_endpoint,
            state.client_rpc_endpoint.port(),
            state.sd_flags(true),
            state.config.transport,
        );

        actions.push(Action::SendSd {
            message: msg,
            target: discovered.sd_endpoint, // Send to SD socket, not RPC socket
        });
    }
}

/// Handle Command::FindStatic
pub(crate) fn handle_find_static(
    instance_id: crate::InstanceId,
    endpoint: SocketAddr,
    notify: tokio::sync::mpsc::Sender<ServiceAvailability>,
) {
    // Immediately notify that service is available at the given endpoint
    let _ = notify.try_send(ServiceAvailability::Available {
        endpoint,
        instance_id: instance_id.value(),
    });
}

// ============================================================================
// INCOMING MESSAGE HANDLERS (CLIENT-SIDE)
// ============================================================================

/// Handle an incoming response (client-side)
pub(crate) fn handle_incoming_response(header: &Header, payload: Bytes, state: &mut RuntimeState) {
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
pub(crate) fn handle_incoming_notification(
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

    let event = Event {
        event_id,
        payload: payload.clone(),
    };

    // Determine which instance this event is from by looking up the 'from' address
    // in discovered services
    let instance_id_filter: Option<u16> = state
        .discovered
        .iter()
        .find(|(key, disc)| {
            key.service_id == header.service_id
                && (disc.udp_endpoint == Some(from) || disc.tcp_endpoint == Some(from))
        })
        .map(|(key, _)| key.instance_id);

    // Find subscriptions for this service/eventgroup (dynamic via SD)
    for (key, subs) in &state.subscriptions {
        // Match service_id from header
        if key.service_id != header.service_id {
            continue;
        }

        // If we determined an instance_id, filter by it to prevent cross-instance delivery
        // If we couldn't determine it, deliver to all subscriptions (backward compatible)
        if let Some(inst_id) = instance_id_filter {
            if key.instance_id != inst_id {
                continue;
            }
        }

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

use bytes::BytesMut;
use crate::wire::{MessageType, PROTOCOL_VERSION};

/// Build a SOME/IP request message
pub(crate) fn build_request(
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

/// Build a SOME/IP fire-and-forget (REQUEST_NO_RETURN) message
pub(crate) fn build_fire_and_forget(
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
