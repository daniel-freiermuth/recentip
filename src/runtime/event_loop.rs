//! # SomeIp Event Loop
//!
//! Internal event loop that processes commands and I/O events.
//!
//! This module contains the `runtime_task` function which is the core of the
//! SOME/IP runtime. It runs as a background tokio task and handles:
//!
//! - Service Discovery (SD) messages
//! - RPC request/response handling
//! - TCP and UDP message routing
//! - Periodic tasks (cyclic offers, TTL expiry)

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::time::interval;

use crate::config::{RuntimeConfig, Transport};
use crate::error::Result;
use crate::net::{TcpListener, TcpStream, UdpSocket};
use crate::runtime::{
    client,
    sd::{
        build_find_message, build_offer_message, handle_find_request, handle_offer,
        handle_stop_offer as handle_sd_stop_offer, handle_subscribe_ack, handle_subscribe_nack,
        handle_subscribe_request, handle_unsubscribe_request, Action,
    },
    server::{self, build_response},
    state::{PendingServerResponse, RpcMessage, RpcSendMessage, RuntimeState, ServiceKey},
    Command,
};
use crate::tcp::{TcpConnectionPool, TcpMessage};
use crate::wire::{
    validate_protocol_version, Header, MessageType, SdEntry, SdEntryType, SdMessage, SD_METHOD_ID,
    SD_SERVICE_ID,
};

// ============================================================================
// RUNTIME TASK
// ============================================================================

/// The main runtime task
pub async fn runtime_task<U: UdpSocket, T: TcpStream, L: TcpListener<Stream = T>>(
    sd_socket: U,
    config: RuntimeConfig,
    mut cmd_rx: mpsc::Receiver<Command>,
    mut method_rx: mpsc::Receiver<RpcMessage>,
    rpc_tx: mpsc::Sender<RpcMessage>,
    mut tcp_rpc_rx: mpsc::Receiver<TcpMessage>,
    tcp_rpc_tx: mpsc::Sender<TcpMessage>,
    mut tcp_client_rx: mpsc::Receiver<TcpMessage>,
    mut state: RuntimeState,
    tcp_pool: TcpConnectionPool<T>,
) {
    let mut buf = [0u8; 65535];
    let mut ticker = interval(Duration::from_millis(config.cyclic_offer_delay));

    // Track pending server responses
    let mut pending_responses: FuturesUnordered<
        std::pin::Pin<
            Box<dyn std::future::Future<Output = (PendingServerResponse, Result<Bytes>)> + Send>,
        >,
    > = FuturesUnordered::new();

    loop {
        tokio::select! {
            // Handle incoming SD packets from SD socket
            result = sd_socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, from)) => {
                        let mut data = &buf[..len];

                        let Some(header) = Header::parse(&mut data) else {
                            tracing::warn!("Received invalid SOME/IP header on SD socket from {}", from);
                            continue;
                        };

                        if header.service_id != SD_SERVICE_ID || header.method_id != SD_METHOD_ID {
                            tracing::warn!("Received non-SD message on SD socket from {}", from);
                            continue;
                        }

                        if let Some(actions) = handle_sd_message(&header, &mut data, from, &mut state) {
                            for action in actions {
                                execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error receiving SD packet: {}", e);
                    }
                }
            }

            // Handle incoming messages from UDP data socket tasks
            Some(method_msg) = method_rx.recv() => {
                let mut cursor = method_msg.data.as_slice();
                let Some(header) = Header::parse(&mut cursor) else {
                    tracing::warn!("Received invalid SOME/IP header on method socket from {}", method_msg.from);
                    continue;
                };

                if header.service_id == SD_SERVICE_ID {
                    tracing::warn!("Received SD message on method socket from {}", method_msg.from);
                    continue;
                }

                if let Some(actions) = handle_method_message(&header, &mut cursor, method_msg.from, &mut state, method_msg.service_key, Transport::Udp) {
                    for action in actions {
                        execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
                    }
                }
            }

            // Handle incoming RPC messages from TCP server connections
            Some(tcp_msg) = tcp_rpc_rx.recv() => {
                // For TCP messages, we need to find the service key by looking up which service
                // this message is for (based on the service_id in the header)
                let mut cursor = tcp_msg.data.as_slice();
                let Some(header) = Header::parse(&mut cursor) else {
                    tracing::warn!("Received invalid SOME/IP header on TCP server socket from {}", tcp_msg.from);
                    continue;
                };

                if header.service_id == SD_SERVICE_ID {
                    tracing::warn!("Received SD message on method socket from {}", tcp_msg.from);
                    continue;
                }

                // Find which offered service this belongs to based on service_id
                // TODO This is not matching on on instance id. Is that a problem?
                let service_key = state
                    .offered
                    .iter()
                    .find(|(key, _)| key.service_id == header.service_id)
                    .map(|(key, _)| *key);

                if let Some(actions) = handle_method_message(&header, &mut cursor, tcp_msg.from, &mut state, service_key, Transport::Tcp) {
                    for action in actions {
                        execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
                    }
                }
            }

            // Handle responses received on client TCP connections
            Some(tcp_msg) = tcp_client_rx.recv() => {
                // These are responses to RPC calls we made as a client
                // Process them like any other incoming packet
                let mut cursor = tcp_msg.data.as_slice();
                let Some(header) = Header::parse(&mut cursor) else {
                    tracing::warn!("Received invalid SOME/IP header on TCP client socket from {}", tcp_msg.from);
                    continue;
                };

                if header.service_id == SD_SERVICE_ID {
                    tracing::warn!("Received SD message on TCP client socket from {}", tcp_msg.from);
                    continue;
                }

                // Protocol is weird. But it works anyway?
                if let Some(actions) = handle_method_message(&header, &mut cursor, tcp_msg.from, &mut state, None, Transport::Udp) {
                    for action in actions {
                        execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
                    }
                }
            }

            // Handle commands from handles
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(Command::Shutdown) | None => {
                        tracing::info!("SomeIp shutting down, draining {} pending responses", pending_responses.len());
                        // Send StopOffer for all offered services
                        send_stop_offers(&sd_socket, &config, &mut state).await;

                        // Drain all pending responses before exiting
                        // This ensures responses in flight are sent even after offerings are dropped
                        while let Some((context, result)) = pending_responses.next().await {
                            let response_data = match result {
                                Ok(payload) => build_response(
                                    context.service_id,
                                    context.method_id,
                                    context.client_id,
                                    context.session_id,
                                    context.interface_version,
                                    0x00, // OK
                                    &payload,
                                    false,
                                ),
                                Err(_) => build_response(
                                    context.service_id,
                                    context.method_id,
                                    context.client_id,
                                    context.session_id,
                                    context.interface_version,
                                    0x01, // NOT_OK
                                    &[],
                                    context.uses_exception,
                                ),
                            };

                            // Send response via the captured RPC transport
                            if let Err(e) = context.rpc_transport.send(response_data.to_vec(), context.client_addr).await {
                                tracing::error!("Failed to send response during shutdown: {}", e);
                            }
                        }
                        tracing::info!("SomeIp shutdown complete");
                        break;
                    }
                    // Special handling for Offer - needs async socket/listener creation
                    Some(Command::Offer { service_id, instance_id, major_version, minor_version, offer_config, response }) => {
                        if let Some(actions) = server::handle_offer_command::<U, T, L>(
                            service_id, instance_id, major_version, minor_version, offer_config, response,
                            &config, &mut state, &rpc_tx, &tcp_rpc_tx
                        ).await {
                            for action in actions {
                                execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
                            }
                        }
                    }
                    // Special handling for Subscribe - handles multiple eventgroups with shared endpoint
                    Some(Command::Subscribe { service_id, instance_id, major_version, eventgroup_ids, events, response }) => {
                        if let Some(actions) = client::handle_subscribe_command::<U, T>(
                            service_id, instance_id, major_version, eventgroup_ids, events, response,
                            &mut state, &tcp_pool
                        ).await {
                            for action in actions {
                                execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
                            }
                        }
                    }
                    Some(cmd) => {
                        if let Some(actions) = handle_command(cmd, &mut state) {
                            for action in actions {
                                execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
                            }
                        }
                    }
                }
            }

            // Periodic tasks (cyclic offers, find retries)
            _ = ticker.tick() => {
                if let Some(actions) = handle_periodic(&mut state) {
                    for action in actions {
                        execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
                    }
                }
            }

            // Handle completed server responses
            Some((context, result)) = pending_responses.next() => {
                let response_data = match result {
                    Ok(payload) => build_response(
                        context.service_id,
                        context.method_id,
                        context.client_id,
                        context.session_id,
                        context.interface_version,
                        0x00, // OK
                        &payload,
                        false, // uses_exception doesn't matter for OK responses
                    ),
                    Err(_) => build_response(
                        context.service_id,
                        context.method_id,
                        context.client_id,
                        context.session_id,
                        context.interface_version,
                        0x01, // NOT_OK
                        &[],
                        context.uses_exception,
                    ),
                };

                // Send response via the captured RPC transport
                // This works even if the service offering has been dropped
                if let Err(e) = context.rpc_transport.send(response_data.to_vec(), context.client_addr).await {
                    tracing::error!("Failed to send response via RPC transport: {}", e);
                }
            }
        }
    }
}

/// Execute an action
async fn execute_action<U: UdpSocket, T: TcpStream>(
    sd_socket: &U,
    _config: &RuntimeConfig,
    state: &mut RuntimeState,
    action: Action,
    pending_responses: &mut FuturesUnordered<
        std::pin::Pin<
            Box<dyn std::future::Future<Output = (PendingServerResponse, Result<Bytes>)> + Send>,
        >,
    >,
    tcp_pool: &TcpConnectionPool<T>,
) {
    match action {
        Action::SendSd { message, target } => {
            let session_id = state.next_session_id();
            let data = message.serialize(session_id);
            if let Err(e) = sd_socket.send_to(&data, target).await {
                tracing::error!("Failed to send SD message: {}", e);
            }
        }
        Action::NotifyFound { key, availability } => {
            // Find all matching find requests and notify them
            for (req_key, request) in &state.find_requests {
                if req_key.matches(&key) {
                    let _ = request.notify.try_send(availability.clone());
                }
            }
        }
        Action::SendClientMessage {
            data,
            target,
            transport,
        } => {
            // Use TCP or UDP based on transport determined by the discovered service endpoint
            tracing::debug!(
                "SendClientMessage: target={}, transport={:?}, data_len={}",
                target,
                transport,
                data.len()
            );
            if transport == Transport::Tcp {
                // Send via TCP connection pool (establishes connection if needed)
                tracing::debug!("Sending via TCP pool to {}", target);
                if let Err(e) = tcp_pool.send(target, &data).await {
                    tracing::error!("Failed to send client SOME/IP message via TCP: {}", e);
                }
            } else {
                // Client messages use dedicated client RPC socket (NOT SD socket)
                // Per feat_req_recentip_676: Port 30490 is only for SD, not for RPC
                if let Err(e) = state
                    .client_rpc_tx
                    .send(RpcSendMessage {
                        data: data.to_vec(),
                        to: target,
                    })
                    .await
                {
                    tracing::error!("Failed to send client SOME/IP message via UDP: {}", e);
                }
            }
        }
        Action::SendServerMessage {
            service_key,
            data,
            target,
            transport,
        } => {
            // Server messages use the service's method transport (UDP or TCP)
            if let Some(offered) = state.offered.get(&service_key) {
                let method_transport = match transport {
                    crate::config::Transport::Tcp => offered.tcp_transport.as_ref(),
                    crate::config::Transport::Udp => offered.udp_transport.as_ref(),
                };
                if let Some(method_transport) = method_transport {
                    if let Err(e) = method_transport.send(data.to_vec(), target).await {
                        tracing::error!("Failed to send server message via {:?}: {}", transport, e);
                    }
                } else {
                    tracing::error!(
                        "Service {:04x}:{:04x} does not have {:?} transport configured",
                        service_key.service_id,
                        service_key.instance_id,
                        transport
                    );
                }
            } else {
                tracing::error!(
                    "Attempted to send server message for unknown service {:04x}:{:04x}",
                    service_key.service_id,
                    service_key.instance_id
                );
            }
        }
        Action::TrackServerResponse { context, receiver } => {
            let fut = Box::pin(async move {
                let result = receiver
                    .await
                    .unwrap_or_else(|_| Err(crate::error::Error::RuntimeShutdown));
                (context, result)
            });
            pending_responses.push(fut);
        }
        Action::ResetPeerTcpConnections { peer } => {
            // Close all TCP connections to the peer that rebooted
            // This handles both client-side (via pool) and triggers reconnection on next request
            tracing::info!(
                "Detected reboot of peer {}, resetting TCP connections",
                peer
            );

            // Find all socket addresses for this peer and close them
            // Client-side connections are managed by the TCP pool
            let addresses_to_close: Vec<SocketAddr> = state
                .discovered
                .values()
                .filter_map(|svc| svc.tcp_endpoint)
                .filter(|addr| addr.ip() == peer)
                .collect();

            for addr in addresses_to_close {
                tcp_pool.close(&addr).await;
            }
        }
        Action::EmitSdEvent { event } => {
            // Send event to all SD monitors
            // Remove monitors that have closed their receivers
            state
                .sd_monitors
                .retain(|monitor| monitor.try_send(event.clone()).is_ok());
        }
    }
}

/// Handle an SD message
fn handle_sd_message(
    _header: &Header,
    cursor: &mut &[u8],
    from: SocketAddr,
    state: &mut RuntimeState,
) -> Option<Vec<Action>> {
    // Parse SD payload
    let sd_message = SdMessage::parse(cursor)?;

    tracing::trace!(
        "Received SD message from {} with {} entries",
        from,
        sd_message.entries.len()
    );

    let mut actions = Vec::new();

    // Check for peer reboot detection (feat_req_recentipsd_872)
    // If we've seen this peer before with reboot_flag=false, and now it's true, they rebooted
    let peer_reboot_flag = (sd_message.flags & SdMessage::FLAG_REBOOT) != 0;
    let peer_ip = from.ip();

    if let Some(&last_reboot_flag) = state.peer_reboot_flags.get(&peer_ip) {
        if !last_reboot_flag && peer_reboot_flag {
            // Peer rebooted: they went from reboot_flag=false to reboot_flag=true
            tracing::debug!(
                "Detected reboot of peer {} (reboot flag transitioned false → true)",
                peer_ip
            );
            actions.push(Action::ResetPeerTcpConnections { peer: peer_ip });
        }
    }
    // Update the stored reboot flag for this peer
    state.peer_reboot_flags.insert(peer_ip, peer_reboot_flag);

    for entry in &sd_message.entries {
        match entry.entry_type {
            SdEntryType::OfferService => {
                if entry.is_stop() {
                    handle_sd_stop_offer(entry, state, &mut actions);
                } else {
                    handle_offer(entry, &sd_message, from, state, &mut actions);
                }
            }
            SdEntryType::FindService => {
                handle_find_request(entry, from, state, &mut actions);
            }
            SdEntryType::SubscribeEventgroup => {
                if entry.is_stop() {
                    handle_unsubscribe_request(entry, &sd_message, from, state);
                } else {
                    handle_subscribe_request(entry, &sd_message, from, state, &mut actions);
                }
            }
            SdEntryType::SubscribeEventgroupAck => {
                if entry.is_stop() {
                    handle_subscribe_nack(entry, state);
                } else {
                    handle_subscribe_ack(entry, state);
                }
            }
        }
    }

    if actions.is_empty() {
        None
    } else {
        Some(actions)
    }
}

/// Handle an RPC message (Request, Response, etc.)
fn handle_method_message(
    header: &Header,
    cursor: &mut &[u8],
    from: SocketAddr,
    state: &mut RuntimeState,
    service_key: Option<ServiceKey>,
    transport: Transport,
) -> Option<Vec<Action>> {
    use bytes::Buf;

    // Validate protocol version - silently drop messages with wrong version
    if validate_protocol_version(header.protocol_version).is_err() {
        tracing::trace!(
            "Dropping message with invalid protocol version 0x{:02X} from {}",
            header.protocol_version,
            from
        );
        // Still consume the payload bytes from the cursor
        let payload_len = header.payload_length();
        if cursor.remaining() >= payload_len {
            cursor.advance(payload_len);
        }
        return None;
    }

    let payload_len = header.payload_length();
    if cursor.remaining() < payload_len {
        return None;
    }
    let payload = cursor.copy_to_bytes(payload_len);

    let mut actions = Vec::new();

    match header.message_type {
        MessageType::Request => {
            // Incoming request - route to offering using service_key from RPC socket
            server::handle_incoming_request(
                header,
                payload,
                from,
                state,
                &mut actions,
                service_key,
                transport,
            );
        }
        MessageType::RequestNoReturn => {
            // Incoming fire-and-forget - route to offering without response tracking
            server::handle_incoming_fire_forget(header, payload, from, state);
        }
        MessageType::Response | MessageType::Error => {
            // Response to our request - route to pending call
            client::handle_incoming_response(header, payload, state);
        }
        MessageType::Notification => {
            // Event notification - route to subscription
            client::handle_incoming_notification(header, payload, from, state);
        }
        _ => {
            tracing::trace!(
                "Ignoring message type {:?} from {}",
                header.message_type,
                from
            );
        }
    }

    if actions.is_empty() {
        None
    } else {
        Some(actions)
    }
}

/// Handle a command from a handle
fn handle_command(cmd: Command, state: &mut RuntimeState) -> Option<Vec<Action>> {
    let mut actions = Vec::new();

    match cmd {
        Command::Find {
            service_id,
            instance_id,
            major_version,
            notify,
        } => {
            client::handle_find(
                service_id,
                instance_id,
                major_version,
                notify,
                state,
                &mut actions,
            );
        }

        Command::StopFind {
            service_id,
            instance_id,
            major_version,
        } => {
            client::handle_stop_find(service_id, instance_id, major_version, state);
        }

        Command::StopOffer {
            service_id,
            instance_id,
            major_version,
        } => {
            server::handle_stop_offer(service_id, instance_id, major_version, state, &mut actions);
        }

        Command::Call {
            service_id,
            method_id,
            payload,
            response,
            target_endpoint,
            target_transport,
        } => {
            client::handle_call(
                service_id,
                method_id,
                payload,
                response,
                target_endpoint,
                target_transport,
                state,
                &mut actions,
            );
        }

        Command::FireAndForget {
            service_id,
            method_id,
            payload,
            target_endpoint,
            target_transport,
        } => {
            client::handle_fire_and_forget(
                service_id,
                method_id,
                payload,
                target_endpoint,
                target_transport,
                state,
                &mut actions,
            );
        }

        // Subscribe is handled separately in runtime_task for async TCP connection
        Command::Subscribe { .. } => {}

        Command::Unsubscribe {
            service_id,
            instance_id,
            major_version,
            eventgroup_id,
            subscription_id,
        } => {
            client::handle_unsubscribe(
                service_id,
                instance_id,
                major_version,
                eventgroup_id,
                subscription_id,
                state,
                &mut actions,
            );
        }

        Command::RegisterEvent {
            service_id,
            instance_id,
            major_version,
            event_id,
            response,
        } => {
            let result = server::handle_register_event(
                service_id,
                instance_id,
                major_version,
                event_id,
                state,
            );
            let _ = response.send(result);
        }

        Command::Notify {
            service_id,
            instance_id,
            major_version,
            eventgroup_ids,
            event_id,
            payload,
        } => {
            server::handle_notify(
                service_id,
                instance_id,
                major_version,
                &eventgroup_ids,
                event_id,
                payload,
                state,
                &mut actions,
            );
        }

        Command::MonitorSd { events } => {
            state.sd_monitors.push(events);
        }

        // These are handled separately in runtime_task for async socket creation
        Command::Shutdown | Command::Offer { .. } => {}
    }

    if actions.is_empty() {
        None
    } else {
        Some(actions)
    }
}

/// Handle periodic tasks
fn handle_periodic(state: &mut RuntimeState) -> Option<Vec<Action>> {
    use tokio::time::Instant;

    let mut actions = Vec::new();
    let now = Instant::now();
    let offer_interval = Duration::from_millis(state.config.cyclic_offer_delay);

    // Capture values before mutable borrow
    let sd_flags = state.sd_flags(true);
    let offer_ttl = state.config.offer_ttl;
    let find_ttl = state.config.find_ttl;
    let sd_multicast = state.config.sd_multicast;

    // Cyclic offers (only for services that are announcing)
    for (key, offered) in &mut state.offered {
        // Skip services that are bound but not announcing
        if !offered.is_announcing {
            continue;
        }

        if now.duration_since(offered.last_offer) >= offer_interval {
            offered.last_offer = now;

            let msg = build_offer_message(
                key,
                offered,
                sd_flags,
                offer_ttl,
                state.config.advertised_ip,
            );

            actions.push(Action::SendSd {
                message: msg,
                target: sd_multicast,
            });
        }
    }

    // Find request repetitions
    let find_interval = Duration::from_millis(state.config.cyclic_offer_delay);
    let mut expired_finds = Vec::new();

    for (key, find_req) in &mut state.find_requests {
        if find_req.repetitions_left > 0 && now.duration_since(find_req.last_find) >= find_interval
        {
            find_req.last_find = now;
            find_req.repetitions_left -= 1;

            let msg = build_find_message(
                key.service_id,
                key.instance_id,
                key.major_version,
                sd_flags,
                find_ttl,
            );

            actions.push(Action::SendSd {
                message: msg,
                target: sd_multicast,
            });
        } else if find_req.repetitions_left == 0 {
            expired_finds.push(*key);
        }
    }

    for key in expired_finds {
        state.find_requests.remove(&key);
    }

    // Check for expired discovered services
    let mut expired_discovered = Vec::new();
    for (key, discovered) in &state.discovered {
        if now >= discovered.ttl_expires {
            expired_discovered.push(*key);
        }
    }

    for key in expired_discovered {
        if state.discovered.remove(&key).is_some() {
            // Emit SD event to monitors
            actions.push(Action::EmitSdEvent {
                event: crate::SdEvent::ServiceExpired {
                    service_id: key.service_id,
                    instance_id: key.instance_id,
                },
            });
        }
    }

    // Expire stale server-side subscriptions (feat_req_recentipsd_445)
    // Remove subscriptions whose TTL has elapsed without renewal
    // Note: expires_at=None means infinite TTL (0xFFFFFF) per feat_req_recentipsd_431
    for subscribers in state.server_subscribers.values_mut() {
        let before_count = subscribers.len();
        subscribers.retain(|sub| match sub.expires_at {
            None => true, // Infinite TTL - never expires
            Some(expires_at) if now >= expires_at => {
                tracing::debug!("Subscription from {} expired (TTL elapsed)", sub.endpoint);
                false
            }
            Some(_) => true,
        });
        let expired_count = before_count - subscribers.len();
        if expired_count > 0 {
            tracing::debug!("{} subscription(s) expired", expired_count);
        }
    }

    // Clean up empty subscription entries
    state
        .server_subscribers
        .retain(|_, subscribers| !subscribers.is_empty());

    if actions.is_empty() {
        None
    } else {
        Some(actions)
    }
}

/// Send `StopOffer` for all offered services (on shutdown)
async fn send_stop_offers<U: UdpSocket>(
    sd_socket: &U,
    config: &RuntimeConfig,
    state: &mut RuntimeState,
) {
    if state.offered.is_empty() {
        return;
    }

    let mut msg = SdMessage::new(state.sd_flags(false));
    for (key, offered) in &state.offered {
        msg.add_entry(SdEntry::stop_offer_service(
            key.service_id,
            key.instance_id,
            offered.major_version,
            offered.minor_version,
        ));
    }

    let session_id = state.next_session_id();
    let data = msg.serialize(session_id);
    let _ = sd_socket.send_to(&data, config.sd_multicast).await;
}
