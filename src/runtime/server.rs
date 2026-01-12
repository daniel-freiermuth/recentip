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
//! Runtime forwards to handle_incoming_request()
//!        │
//!        ▼
//! Creates ServiceRequest, sends to OfferingHandle's channel
//!        │
//!        ▼
//! User processes, calls responder.reply()
//!        │
//!        ▼
//! Responder sends result via oneshot to runtime
//!        │
//!        ▼
//! Runtime sends response via OfferedService's RPC socket
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
use super::sd::{build_offer_message, build_stop_offer_message, Action};
use super::state::{
    OfferedService, PendingServerResponse, RpcMessage, RpcSendMessage, RpcTransportSender,
    RuntimeState, ServiceKey, SubscriberKey,
};
use crate::config::{MethodConfig, RuntimeConfig, Transport};
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
) -> Option<Vec<Action>> {
    let key = ServiceKey::new(service_id, instance_id, major_version);
    let mut actions = Vec::new();

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
                let (endpoint, rpc_send_tx) = spawn_rpc_socket_task(
                    rpc_socket,
                    service_id.value(),
                    instance_id.value(),
                    major_version,
                    rpc_tx.clone(),
                )
                .await;
                udp_endpoint = Some(endpoint);
                udp_transport = Some(RpcTransportSender::Udp(rpc_send_tx));
            }
            Err(e) => {
                tracing::error!("Failed to bind UDP RPC on {}: {}", rpc_addr, e);
                let _ = response.send(Err(Error::Io(e)));
                return None;
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
                )
                .await
                {
                    Ok(tcp_server) => {
                        tcp_endpoint = Some(tcp_server.local_addr);
                        tcp_transport = Some(RpcTransportSender::Tcp(tcp_server.send_tx));
                    }
                    Err(e) => {
                        tracing::error!("Failed to create TCP server on {}: {}", rpc_addr, e);
                        let _ = response.send(Err(Error::Io(e)));
                        return None;
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to bind TCP listener on {}: {}", rpc_addr, e);
                let _ = response.send(Err(Error::Io(e)));
                return None;
            }
        }
    }

    // Ensure at least one transport was created
    if udp_endpoint.is_none() && tcp_endpoint.is_none() {
        tracing::error!("No transport configured for offer");
        let _ = response.send(Err(Error::Config(crate::error::ConfigError::new(
            "No transport configured",
        ))));
        return None;
    }

    // Store the offered service
    state.offered.insert(
        key,
        OfferedService {
            major_version,
            minor_version,
            requests_tx,
            last_offer: Instant::now() - Duration::from_secs(10),
            udp_endpoint,
            udp_transport,
            tcp_endpoint,
            tcp_transport,
            method_config: offer_config.method_config,
            is_announcing: true, // Offer immediately announces
        },
    );

    // Build and send the initial offer message (reuse sd.rs helper)
    let offered = state.offered.get(&key).expect("just inserted");
    let msg = build_offer_message(
        &key,
        offered,
        state.sd_flags(true),
        config.offer_ttl,
        config.advertised_ip,
    );
    actions.push(Action::SendSd {
        message: msg,
        target: config.sd_multicast,
    });

    let _ = response.send(Ok(requests_rx));
    Some(actions)
}

/// Handle `Command::Bind` which creates socket but does NOT announce via SD
pub async fn handle_bind_command<U: UdpSocket, T: TcpStream, L: TcpListener<Stream = T>>(
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    minor_version: u32,
    transport: Transport,
    method_config: MethodConfig,
    response: oneshot::Sender<Result<mpsc::Receiver<ServiceRequest>>>,
    config: &RuntimeConfig,
    state: &mut RuntimeState,
    rpc_tx: &mpsc::Sender<RpcMessage>,
    tcp_rpc_tx: &mpsc::Sender<TcpMessage>,
) {
    let key = ServiceKey::new(service_id, instance_id, major_version);

    // Check if already bound
    if state.offered.contains_key(&key) {
        let _ = response.send(Err(Error::AlreadyOffered));
        return;
    }

    // Create channel for service requests
    let (requests_tx, requests_rx) = mpsc::channel(64);

    // Determine RPC port - use next available port starting from local_addr + 1
    let rpc_port = state.local_endpoint.port() + 1 + state.offered.len() as u16;
    let rpc_addr = SocketAddr::new(state.local_endpoint.ip(), rpc_port);

    // Create the appropriate transport listener/socket
    let result: std::io::Result<(SocketAddr, RpcTransportSender)> = match transport {
        Transport::Udp => {
            // Create and bind UDP RPC socket
            match U::bind(rpc_addr).await {
                Ok(rpc_socket) => {
                    let (rpc_endpoint, rpc_send_tx) = spawn_rpc_socket_task(
                        rpc_socket,
                        service_id.value(),
                        instance_id.value(),
                        major_version,
                        rpc_tx.clone(),
                    )
                    .await;
                    Ok((rpc_endpoint, RpcTransportSender::Udp(rpc_send_tx)))
                }
                Err(e) => Err(e),
            }
        }
        Transport::Tcp => {
            // Create and bind TCP listener
            match L::bind(rpc_addr).await {
                Ok(listener) => {
                    match TcpServer::<T>::spawn(
                        listener,
                        service_id.value(),
                        instance_id.value(),
                        tcp_rpc_tx.clone(),
                        config.magic_cookies,
                    )
                    .await
                    {
                        Ok(tcp_server) => Ok((
                            tcp_server.local_addr,
                            RpcTransportSender::Tcp(tcp_server.send_tx),
                        )),
                        Err(e) => Err(e),
                    }
                }
                Err(e) => Err(e),
            }
        }
    };

    match result {
        Ok((rpc_endpoint, rpc_transport)) => {
            // Store the offered service with its RPC endpoint (NOT announcing yet)
            // Bind uses only the specified transport (no dual-stack for static binding)
            let (udp_endpoint, udp_transport, tcp_endpoint, tcp_transport) = match transport {
                Transport::Udp => (Some(rpc_endpoint), Some(rpc_transport), None, None),
                Transport::Tcp => (None, None, Some(rpc_endpoint), Some(rpc_transport)),
            };
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
                    method_config,
                    is_announcing: false, // Bind does NOT announce
                },
            );

            let _ = response.send(Ok(requests_rx));
        }
        Err(e) => {
            tracing::error!("Failed to bind RPC {:?} on {}: {}", transport, rpc_addr, e);
            let _ = response.send(Err(Error::Io(e)));
        }
    }
}

/// Handle `Command::ListenStatic` which creates a socket to receive static events
pub async fn handle_listen_static_command<U: UdpSocket>(
    service_id: ServiceId,
    instance_id: InstanceId,
    eventgroup_id: u16,
    port: u16,
    events: mpsc::Sender<crate::Event>,
    response: oneshot::Sender<Result<()>>,
    state: &mut RuntimeState,
) {
    let key = SubscriberKey {
        service_id: service_id.value(),
        instance_id: instance_id.value(),
        eventgroup_id,
    };

    // Bind a UDP socket to receive static notifications
    let bind_addr = SocketAddr::new(state.local_endpoint.ip(), port);

    match U::bind(bind_addr).await {
        Ok(socket) => {
            // Store the event channel for this listener
            state.static_listeners.insert(key, events.clone());

            // Spawn a task to receive events on this socket
            tokio::spawn(async move {
                let mut buf = [0u8; 65535];
                loop {
                    match socket.recv_from(&mut buf).await {
                        Ok((len, _from)) => {
                            // Parse the SOME/IP header
                            if len >= Header::SIZE {
                                let mut data = &buf[..len];
                                if let Some(header) = Header::parse(&mut data) {
                                    // Check if this is a notification
                                    if header.message_type == MessageType::Notification {
                                        if let Some(event_id) =
                                            crate::EventId::new(header.method_id)
                                        {
                                            let payload_start = Header::SIZE;
                                            let payload_end = (header.length as usize + 8).min(len);
                                            let payload = Bytes::copy_from_slice(
                                                &buf[payload_start..payload_end],
                                            );

                                            let event = crate::Event { event_id, payload };

                                            if events.send(event).await.is_err() {
                                                // Receiver dropped, exit the task
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("Error receiving on static listener socket: {}", e);
                            break;
                        }
                    }
                }
            });

            let _ = response.send(Ok(()));
        }
        Err(e) => {
            tracing::error!(
                "Failed to bind static listener socket on port {}: {}",
                port,
                e
            );
            let _ = response.send(Err(Error::Io(e)));
        }
    }
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
    let events = state
        .registered_events
        .entry(key)
        .or_insert_with(std::collections::HashSet::new);

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
        let msg = build_stop_offer_message(&key, &offered, state.sd_flags(false));

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
    payload: Bytes,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let service_key = ServiceKey::new(service_id, instance_id, major_version);

    // Collect all unique subscribers across all eventgroups
    // Use a set to avoid sending duplicates if a client is subscribed to multiple eventgroups
    let mut seen_subscribers = std::collections::HashSet::new();
    let mut subscribers: Vec<(SocketAddr, crate::config::Transport)> = Vec::new();

    for &eventgroup_id in eventgroup_ids {
        let sub_key = SubscriberKey {
            service_id: service_id.value(),
            instance_id: instance_id.value(),
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
            state.next_session_id(),
            1, // interface version
            &payload,
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

/// Handle `Command::NotifyStatic`
pub fn handle_notify_static(
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    event_id: u16,
    payload: Bytes,
    targets: Vec<SocketAddr>,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let service_key = ServiceKey::new(service_id, instance_id, major_version);

    if state.offered.contains_key(&service_key) {
        let notification_data = build_notification(
            service_id.value(),
            event_id,
            state.client_id,
            state.next_session_id(),
            1, // interface version
            &payload,
        );

        for target in targets {
            actions.push(Action::SendServerMessage {
                service_key,
                data: notification_data.clone(),
                target,
                // TODO: Accept transport in API
                transport: crate::config::Transport::Udp,
            });
        }
    }
}

/// Handle `Command::StartAnnouncing`
pub fn handle_start_announcing(
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    response: oneshot::Sender<Result<()>>,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let key = ServiceKey::new(service_id, instance_id, major_version);

    // Capture values before mutable borrow
    let sd_flags = state.sd_flags(true);
    let ttl = state.config.offer_ttl;
    let sd_multicast = state.config.sd_multicast;

    if let Some(offered) = state.offered.get_mut(&key) {
        // Mark as announcing
        offered.is_announcing = true;
        offered.last_offer = Instant::now() - Duration::from_secs(10); // Force immediate offer

        let msg = build_offer_message(&key, offered, sd_flags, ttl, state.config.advertised_ip);

        actions.push(Action::SendSd {
            message: msg,
            target: sd_multicast,
        });

        let _ = response.send(Ok(()));
    } else {
        let _ = response.send(Err(Error::ServiceUnavailable));
    }
}

/// Handle `Command::StopAnnouncing`
/// Announcing a service that was only bound (not offered) before
pub fn handle_stop_announcing(
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    response: oneshot::Sender<Result<()>>,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let key = ServiceKey::new(service_id, instance_id, major_version);

    // Capture values before mutable borrow
    let sd_flags = state.sd_flags(false);
    let sd_multicast = state.config.sd_multicast;

    if let Some(offered) = state.offered.get_mut(&key) {
        // Mark as not announcing
        offered.is_announcing = false;

        let msg = build_stop_offer_message(&key, offered, sd_flags);

        actions.push(Action::SendSd {
            message: msg,
            target: sd_multicast,
        });

        let _ = response.send(Ok(()));
    } else {
        let _ = response.send(Err(Error::ServiceUnavailable));
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
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
    service_key: Option<ServiceKey>,
    transport: crate::config::Transport,
) {
    // Find the offering:
    // - If service_key is provided (from server RPC socket), use exact matching
    // - Otherwise fall back to service_id-only matching
    let offering = if let Some(key) = service_key {
        state.offered.get_key_value(&key)
    } else {
        state
            .offered
            .iter()
            .find(|(k, _)| k.service_id == header.service_id)
    };

    if let Some((service_key, offered)) = offering {
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
    state: &mut RuntimeState,
) {
    // Find matching offering
    // NOTE: Same limitation as handle_incoming_request - see comment there
    let offering = state
        .offered
        .iter()
        .find(|(k, _)| k.service_id == header.service_id);

    if let Some((_, offered)) = offering {
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
/// - `feat_req_recentip_726`: If EXCEPTION is not configured, errors use RESPONSE (0x80)
/// - `feat_req_recentip_106`: EXCEPTION (0x81) is optional and must be configured per-method
/// - `feat_req_recentip_107`: Receiving errors on both 0x80 and 0x81 must be supported
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

    // feat_req_recentip_655: Error message must copy request header fields
    // feat_req_recentip_727: Error messages have return code != 0x00
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
pub async fn spawn_rpc_socket_task<U: UdpSocket>(
    rpc_socket: U,
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    rpc_tx_to_runtime: mpsc::Sender<RpcMessage>,
) -> (SocketAddr, mpsc::Sender<RpcSendMessage>) {
    let local_endpoint = rpc_socket
        .local_addr()
        .expect("Failed to get local address");
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

    (local_endpoint, send_tx)
}
