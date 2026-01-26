//! # Server-Side Handlers (Internal)
//!
//! This module contains handlers for server-side SOME/IP operations.
//! It is `pub(crate)` — internal to the library.
//!
//! ## Responsibilities
//!
//! | Handler | Command | Purpose |
//! |---------|---------|----------|
//! | `handle_offer_command` | `Offer` | Create sockets, start offering service |
//! | `handle_bind_command` | `Bind` | Create sockets without SD announcement |
//! | `handle_notify` | `Notify` | Send event to all subscribers |
//! | `handle_start_announcing` | `StartAnnouncing` | Begin SD offers for bound service |
//! | `handle_stop_announcing` | `StopAnnouncing` | Stop SD offers, keep socket |
//! | `handle_incoming_request` | — | Process RPC request from client |
//!
//! ## Socket Management
//!
//! When a service is offered/bound:
//! 1. A dedicated RPC socket is created (UDP or TCP listener)
//! 2. The socket's sender is stored in `OfferedService`
//! 3. Incoming messages are forwarded to the runtime via channels
//!
//! ## Request/Response Flow
//!
//! ```text
//! Client sends RPC request
//!        │
//!        ▼
//! Server's RPC socket receives
//!        │
//!        ▼
//! SomeIp forwards to handle_incoming_request()
//!        │
//!        ▼
//! Creates ServiceRequest, sends to ServiceOffering's channel
//!        │
//!        ▼
//! User processes, calls responder.reply()
//!        │
//!        ▼
//! Responder sends result via oneshot to runtime
//!        │
//!        ▼
//! SomeIp sends response via OfferedService's RPC socket
//! ```
//!
//! ## Contributor Notes
//!
//! - `handle_offer_command` and `handle_bind_command` are async (create sockets)
//! - Other handlers are sync and return `Vec<Action>`
//! - Server responses must use the same transport as the request

use std::net::SocketAddr;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use super::command::ServiceRequest;
use super::sd::{build_stop_offer_message, Action};
use super::state::{
    OfferedService, PendingServerResponse, RpcMessage, RpcSendMessage, RpcTransportSender,
    RuntimeState, ServiceKey, SubscriberKey,
};
use crate::config::RuntimeConfig;
use crate::error::{Error, Result};
use crate::net::{TcpListener, TcpStream, UdpSocket};
use crate::tcp::{TcpMessage, TcpServer};
use crate::wire::{Header, MessageType, PROTOCOL_VERSION};
use crate::{InstanceId, ServiceId};

// ============================================================================
// ASYNC COMMAND HANDLERS (NEED SOCKET CREATION)
// ============================================================================

/// Handle `Command::Offer` which requires async socket creation
pub async fn handle_offer_command<U: UdpSocket, T: TcpStream, L: TcpListener<Stream = T>>(
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    minor_version: u32,
    offer_config: crate::config::OfferConfig,
    response: oneshot::Sender<Result<mpsc::Receiver<ServiceRequest>>>,
    config: &RuntimeConfig,
    state: &mut RuntimeState,
    rpc_tx: &mpsc::Sender<RpcMessage>,
    tcp_rpc_tx: &mpsc::Sender<TcpMessage>,
) {
    let key = ServiceKey::new(service_id, instance_id, major_version);

    // Create channel for service requests
    let (requests_tx, requests_rx) = mpsc::channel(64);

    // Track endpoints and transports
    let mut udp_endpoint: Option<SocketAddr> = None;
    let mut udp_transport: Option<RpcTransportSender> = None;
    let mut tcp_endpoint: Option<SocketAddr> = None;
    let mut tcp_transport: Option<RpcTransportSender> = None;

    // Calculate base port
    // TODO: add test and fix math
    let base_port = state.local_endpoint.port() + 1 + (state.offered.len() as u16 * 2);

    // Create UDP transport if configured
    if let Some(port) = offer_config.udp_port {
        let rpc_port = if port == 0 { base_port } else { port };
        let rpc_addr = SocketAddr::new(state.local_endpoint.ip(), rpc_port);

        match U::bind(rpc_addr).await {
            Ok(rpc_socket) => {
                match spawn_rpc_socket_task(
                    rpc_socket,
                    service_id.value(),
                    instance_id.value(),
                    major_version,
                    rpc_tx.clone(),
                ) {
                    Ok((endpoint, rpc_send_tx)) => {
                        udp_endpoint = Some(endpoint);
                        udp_transport = Some(RpcTransportSender::Udp(rpc_send_tx));
                    }
                    Err(e) => {
                        tracing::error!("Failed to get local address for UDP RPC: {}", e);
                        let _ = response.send(Err(Error::Io(e)));
                        return;
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to bind UDP RPC on {}: {}", rpc_addr, e);
                let _ = response.send(Err(Error::Io(e)));
                return;
            }
        }
    }

    // Create TCP transport if configured
    if let Some(port) = offer_config.tcp_port {
        let rpc_port = if port == 0 { base_port + 1 } else { port };
        let rpc_addr = SocketAddr::new(state.local_endpoint.ip(), rpc_port);

        match L::bind(rpc_addr).await {
            Ok(listener) => {
                match TcpServer::<T>::spawn(
                    listener,
                    service_id.value(),
                    instance_id.value(),
                    tcp_rpc_tx.clone(),
                    config.magic_cookies,
                ) {
                    Ok(tcp_server) => {
                        tcp_endpoint = Some(tcp_server.local_addr);
                        tcp_transport = Some(RpcTransportSender::Tcp(tcp_server.send_tx));
                    }
                    Err(e) => {
                        tracing::error!("Failed to create TCP server on {}: {}", rpc_addr, e);
                        let _ = response.send(Err(Error::Io(e)));
                        return;
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to bind TCP listener on {}: {}", rpc_addr, e);
                let _ = response.send(Err(Error::Io(e)));
                return;
            }
        }
    }

    // Ensure at least one transport was created
    if udp_endpoint.is_none() && tcp_endpoint.is_none() {
        tracing::error!("No transport configured for offer");
        let _ = response.send(Err(Error::Config(crate::error::ConfigError::new(
            "No transport configured",
        ))));
        return;
    }

    // Store the offered service
    state.offered.insert(
        key,
        OfferedService {
            major_version,
            minor_version,
            requests_tx,
            last_offer: Instant::now(),
            udp_endpoint,
            udp_transport,
            tcp_endpoint,
            tcp_transport,
            method_config: offer_config.method_config,
            is_announcing: true, // Offer immediately announces
        },
    );

    // Queue this offer for time-based clustered sending (within ~50ms)
    // This prevents multiple session IDs when starting multiple services quickly
    state.pending_initial_offers.push(key);
    if state.pending_offers_deadline.is_none() {
        // Set deadline for first pending offer - but only if it won't be too close to periodic cycle
        const MIN_SD_MSG_DISTANCE: Duration = Duration::from_millis(150);

        let proposed_deadline = Instant::now() + MIN_SD_MSG_DISTANCE;

        // Check if proposed deadline would be too close to the next (or last) periodic cycle
        let should_set_deadline = state.last_periodic_cycle.is_some_and(|last_cycle| {
            let cycle_interval = Duration::from_millis(state.config.cyclic_offer_delay);
            let next_cycle = last_cycle + cycle_interval;

            proposed_deadline + MIN_SD_MSG_DISTANCE < next_cycle
        });

        if should_set_deadline {
            state.pending_offers_deadline = Some(proposed_deadline);
            tracing::debug!(
                "[OFFER] {:#?} Queued initial offer for {}:{:04X} v{} (will flush clustered within {:#?})",
                tokio::time::Instant::now(),
                key.service_id,
                key.instance_id,
                major_version,
                MIN_SD_MSG_DISTANCE
            );
        } else {
            tracing::debug!(
                "[{:?}] [OFFER] Queued initial offer for {}:{:04X} v{} (will flush at next periodic cycle)",
                tokio::time::Instant::now(),
                key.service_id,
                key.instance_id,
                major_version
            );
        }
    } else {
        tracing::debug!(
            "[OFFER] Queued initial offer for {}:{:04X} v{} (already scheduled for flush)",
            key.service_id,
            key.instance_id,
            major_version
        );
    }

    let _ = response.send(Ok(requests_rx));
}

// ============================================================================
// SYNC COMMAND HANDLERS (SERVER-SIDE)
// ============================================================================

/// Handle `Command::RegisterEvent` - validate event ID uniqueness
pub fn handle_register_event(
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    event_id: u16,
    state: &mut RuntimeState,
) -> Result<()> {
    let key = ServiceKey::new(service_id, instance_id, major_version);

    // Check if the service is actually being offered
    if !state.offered.contains_key(&key) {
        return Err(Error::Config(crate::error::ConfigError::new(format!(
            "Cannot register event for service {}/{}/{} that is not being offered",
            service_id.value(),
            instance_id.value(),
            major_version
        ))));
    }

    // Get or create the event set for this service
    let events = state.registered_events.entry(key).or_default();

    // Check if event_id is already registered
    if events.contains(&event_id) {
        return Err(Error::Config(crate::error::ConfigError::new(format!(
            "Event ID 0x{:04x} already registered for service {}/{}/{}",
            event_id,
            service_id.value(),
            instance_id.value(),
            major_version
        ))));
    }

    // Register the event
    events.insert(event_id);
    Ok(())
}

/// Handle `Command::StopOffer`
pub fn handle_stop_offer(
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let key = ServiceKey::new(service_id, instance_id, major_version);

    if let Some(offered) = state.offered.remove(&key) {
        let msg = build_stop_offer_message(key, &offered, state.sd_flags(false));

        actions.push(Action::SendSd {
            message: msg,
            target: state.config.sd_multicast,
        });
    }

    // Clean up registered events for this service
    state.registered_events.remove(&key);
}

/// Handle `Command::Notify`
pub fn handle_notify(
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    eventgroup_ids: &[u16],
    event_id: u16,
    payload: &Bytes,
    state: &RuntimeState,
    actions: &mut Vec<Action>,
) {
    let service_key = ServiceKey::new(service_id, instance_id, major_version);

    // Collect all unique subscribers across all eventgroups
    // Deduplicate by endpoint to avoid sending the same event multiple times to the same client.
    // For UDP: Each eventgroup subscription has its own socket (unique port), so dedup is per-EG.
    // For TCP: Multiple eventgroup subscriptions share one connection, so we send once and let
    //          the client route internally based on which EGs it subscribed to.
    let mut seen_subscribers = std::collections::HashSet::new();
    let mut subscribers: Vec<(SocketAddr, crate::config::Transport)> = Vec::new();

    for &eventgroup_id in eventgroup_ids {
        let sub_key = SubscriberKey {
            service_id: service_id.value(),
            instance_id: instance_id.value(),
            major_version,
            eventgroup_id,
        };

        if let Some(subs) = state.server_subscribers.get(&sub_key) {
            for s in subs {
                if seen_subscribers.insert((s.endpoint, s.transport)) {
                    subscribers.push((s.endpoint, s.transport));
                }
            }
        }
    }

    if !subscribers.is_empty() {
        // Build notification message
        let notification_data = build_notification(
            service_id.value(),
            event_id,
            state.client_id,
            0,
            1, // interface version
            payload,
        );

        for (subscriber, transport) in subscribers {
            actions.push(Action::SendServerMessage {
                service_key,
                data: notification_data.clone(),
                target: subscriber,
                transport,
            });
        }
    }
}

// ============================================================================
// INCOMING MESSAGE HANDLERS (SERVER-SIDE)
// ============================================================================

/// Handle an incoming request (server-side)
/// i.e. process RPC request from client
pub fn handle_incoming_request(
    header: &Header,
    payload: Bytes,
    from: SocketAddr,
    state: &RuntimeState,
    actions: &mut Vec<Action>,
    service_key: Option<ServiceKey>,
    transport: crate::config::Transport,
) {
    // Find the offering:
    // - If service_key is provided (from server RPC socket), validate service_id matches
    //   and use exact matching
    // - Otherwise fall back to service_id-only matching
    let offering = if let Some(key) = service_key {
        // Validate that the service_id in the header matches the socket's service
        // This prevents routing requests to the wrong service when a client sends
        // a request with a mismatched service_id to a service-specific socket
        if key.service_id != header.service_id {
            tracing::warn!(
                "Service ID mismatch: socket belongs to service 0x{:04x} but header has 0x{:04x} from {}",
                key.service_id,
                header.service_id,
                from
            );

            // Send E_UNKNOWN_SERVICE (0x02) response
            // Per feat_req_someip_816: E_UNKNOWN_SERVICE is optional and may be sent
            // when the Service ID is wrong
            let response_data = build_response(
                header.service_id,
                header.method_id,
                header.client_id,
                header.session_id,
                header.interface_version,
                0x02, // E_UNKNOWN_SERVICE
                &[],
                false, // No exception config for misrouted services
            );
            actions.push(Action::SendClientMessage {
                data: response_data,
                target: from,
                transport,
            });
            return;
        }
        state.offered.get_key_value(&key)
    } else {
        state
            .offered
            .iter()
            .find(|(k, _)| k.service_id == header.service_id)
    };

    if let Some((service_key, offered)) = offering {
        // Validate method_id early: high bit set means event ID, not method ID
        // Per SOME/IP spec: method IDs are 0x0000-0x7FFF, event IDs are 0x8000-0xFFFE
        if header.method_id >= 0x8000 {
            tracing::warn!(
                "Received request with invalid method_id 0x{:04x} (high bit set = event ID, not method ID) from {}",
                header.method_id,
                from
            );

            // Determine transport for response
            let rpc_transport = match transport {
                crate::config::Transport::Tcp => offered.tcp_transport.clone(),
                crate::config::Transport::Udp => offered.udp_transport.clone(),
            };

            // Send E_MALFORMED_MESSAGE (0x09) response
            let response_data = build_response(
                header.service_id,
                header.method_id,
                header.client_id,
                header.session_id,
                header.interface_version,
                0x09, // E_MALFORMED_MESSAGE - method_id field contains an event ID
                &[],
                offered.method_config.uses_exception(header.method_id),
            );

            if rpc_transport.is_some() {
                actions.push(Action::SendServerMessage {
                    service_key: *service_key,
                    data: response_data,
                    target: from,
                    transport,
                });
            }
            return;
        }

        // Create a response channel
        let (response_tx, response_rx) = oneshot::channel();

        // Check if this method uses EXCEPTION for errors
        let uses_exception = offered.method_config.uses_exception(header.method_id);

        // Send request to the offering handle
        if offered
            .requests_tx
            .try_send(ServiceRequest::MethodCall {
                method_id: header.method_id,
                payload,
                client: from,
                transport,
                response: response_tx,
            })
            .is_ok()
        {
            // Track this pending response - will be polled in the main loop
            // Use the transport that the request came in on
            let rpc_transport = match transport {
                crate::config::Transport::Tcp => offered.tcp_transport.clone(),
                crate::config::Transport::Udp => offered.udp_transport.clone(),
            };

            if let Some(rpc_transport) = rpc_transport {
                let context = PendingServerResponse {
                    service_id: header.service_id,
                    instance_id: service_key.instance_id,
                    method_id: header.method_id,
                    client_id: header.client_id,
                    session_id: header.session_id,
                    interface_version: header.interface_version,
                    client_addr: from,
                    uses_exception,
                    rpc_transport,
                };
                actions.push(Action::TrackServerResponse {
                    context,
                    receiver: response_rx,
                });
            } else {
                tracing::error!(
                    "We seemingly got a request via {:?}, which we don't offer",
                    transport
                );
            }
        }
    } else {
        // Unknown service - send error response via SD socket
        // Use RESPONSE (0x80) since we don't have method config for unknown services
        let response_data = build_response(
            header.service_id,
            header.method_id,
            header.client_id,
            header.session_id,
            header.interface_version,
            0x02, // UNKNOWN_SERVICE
            &[],
            false, // No exception config for unknown services
        );
        // TODO This loos weird or suspicous
        actions.push(Action::SendClientMessage {
            data: response_data,
            target: from,
            transport,
        });
    }
}

/// Handle an incoming fire-and-forget request (server-side, no response)
pub fn handle_incoming_fire_forget(
    header: &Header,
    payload: Bytes,
    from: SocketAddr,
    state: &RuntimeState,
    service_key: Option<ServiceKey>,
) {
    // Find matching offering with service_id validation
    // If service_key is provided (from server RPC socket), validate service_id matches
    let offering = if let Some(key) = service_key {
        // Validate that the service_id in the header matches the socket's service
        if key.service_id != header.service_id {
            tracing::warn!(
                "Fire-and-forget service ID mismatch: socket belongs to service 0x{:04x} but header has 0x{:04x} from {}",
                key.service_id,
                header.service_id,
                from
            );
            // Silently ignore - fire-and-forget doesn't get responses
            return;
        }
        state.offered.get(&key)
    } else {
        state
            .offered
            .iter()
            .find(|(k, _)| k.service_id == header.service_id)
            .map(|(_, v)| v)
    };

    if let Some(offered) = offering {
        // Send fire-and-forget request to the offering handle (no response channel)
        let _ = offered.requests_tx.try_send(ServiceRequest::FireForget {
            method_id: header.method_id,
            payload,
            client: from,
            transport: crate::config::Transport::Udp, // Fire-and-forget is UDP-only for now
        });
        // No response tracking needed - fire and forget
    }
    // If unknown service, silently ignore (no error response for fire-and-forget)
}

// ============================================================================
// MESSAGE BUILDING (SERVER-SIDE)
// ============================================================================

/// Build a SOME/IP response message
///
/// Per SOME/IP specification:
/// - `feat_req_someip_726`: If EXCEPTION is not configured, errors use RESPONSE (0x80)
/// - `feat_req_someip_106`: EXCEPTION (0x81) is optional and must be configured per-method
/// - `feat_req_someip_107`: Receiving errors on both 0x80 and 0x81 must be supported
pub fn build_response(
    service_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    interface_version: u8,
    return_code: u8,
    payload: &[u8],
    uses_exception: bool,
) -> Bytes {
    let length = 8 + payload.len() as u32;

    let mut buf = BytesMut::with_capacity(Header::SIZE + payload.len());

    // feat_req_someip_655: Error message must copy request header fields
    // feat_req_someip_727: Error messages have return code != 0x00
    // Only use EXCEPTION (0x81) if configured for this method AND it's an error
    let message_type = if return_code != 0x00 && uses_exception {
        MessageType::Error
    } else {
        MessageType::Response
    };

    let header = Header {
        service_id,
        method_id,
        length,
        client_id,
        session_id,
        protocol_version: PROTOCOL_VERSION,
        interface_version,
        message_type,
        return_code,
    };

    header.serialize(&mut buf);
    buf.extend_from_slice(payload);
    buf.freeze()
}

/// Build a SOME/IP notification (event) message
pub fn build_notification(
    service_id: u16,
    event_id: u16,
    client_id: u16,
    session_id: u16,
    interface_version: u8,
    payload: &[u8],
) -> Bytes {
    let length = 8 + payload.len() as u32;

    let mut buf = BytesMut::with_capacity(Header::SIZE + payload.len());

    let header = Header {
        service_id,
        method_id: event_id, // Event ID goes in method_id field
        length,
        client_id,
        session_id,
        protocol_version: PROTOCOL_VERSION,
        interface_version,
        message_type: MessageType::Notification,
        return_code: 0x00,
    };

    header.serialize(&mut buf);
    buf.extend_from_slice(payload);
    buf.freeze()
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Spawns a task to handle an RPC socket for a specific service instance
/// Returns the endpoint and a sender to send outgoing messages
pub fn spawn_rpc_socket_task<U: UdpSocket>(
    rpc_socket: U,
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    rpc_tx_to_runtime: mpsc::Sender<RpcMessage>,
) -> std::io::Result<(SocketAddr, mpsc::Sender<RpcSendMessage>)> {
    let local_endpoint = rpc_socket.local_addr()?;
    let (send_tx, mut send_rx) = mpsc::channel::<RpcSendMessage>(100);

    let service_key = ServiceKey {
        service_id,
        instance_id,
        major_version,
    };

    tokio::spawn(async move {
        let mut buf = [0u8; 65535];

        loop {
            tokio::select! {
                // Receive incoming RPC messages and forward to runtime task
                result = rpc_socket.recv_from(&mut buf) => {
                    match result {
                        Ok((len, from)) => {
                            let data = buf[..len].to_vec();
                            let msg = RpcMessage { service_key: Some(service_key), data, from };
                            if rpc_tx_to_runtime.send(msg).await.is_err() {
                                tracing::debug!("RPC socket task for service {}/{} shutting down - runtime closed",
                                    service_id, instance_id);
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::error!("Error receiving on RPC socket for service {}/{}: {}",
                                service_id, instance_id, e);
                        }
                    }
                }

                // Send outgoing RPC messages - exit when channel closes
                msg = send_rx.recv() => {
                    if let Some(send_msg) = msg {
                        if let Err(e) = rpc_socket.send_to(&send_msg.data, send_msg.to).await {
                            tracing::error!("Error sending on RPC socket for service {}/{}: {}",
                                service_id, instance_id, e);
                        }
                    } else {
                        // Sender was dropped (service stopped offering) - exit
                        tracing::debug!("RPC socket task for service {}/{} shutting down - sender dropped",
                            service_id, instance_id);
                        break;
                    }
                }
            }
        }
    });

    Ok((local_endpoint, send_tx))
}
