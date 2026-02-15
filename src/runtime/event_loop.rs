//! # `SomeIp` Event Loop
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
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use tokio::sync::mpsc;

use crate::config::{RuntimeConfig, Transport};
use crate::error::{Error, Result};
use crate::net::{TcpListener, TcpStream, UdpSocket};
use crate::runtime::{
    client,
    client_concurrent::handle_subscribe_tcp,
    sd::{
        build_find_message, build_offer_message, handle_find_request, handle_offer,
        handle_stop_offer as handle_sd_stop_offer, handle_subscribe_ack, handle_subscribe_nack,
        handle_subscribe_request, handle_unsubscribe_request, Action,
    },
    server::{self, build_response},
    state::{
        PendingServerResponse, PendingSubscriptionKey, RpcMessage, RpcSendMessage, RuntimeState,
        SdChannel, ServiceKey,
    },
    Command,
};
use crate::tcp::{TcpCleanupRequest, TcpConnectionPool, TcpMessage};
use crate::wire::{
    validate_protocol_version, Header, L4Protocol, MessageType, SdEntry, SdEntryType, SdMessage,
    SdOption, SD_METHOD_ID, SD_SERVICE_ID,
};

// ============================================================================
// SUBSCRIBE STATE UPDATE TYPES
// ============================================================================

/// State update from a Subscribe command task back to the event loop.
///
/// Subscribe commands are processed concurrently in spawned tasks to avoid
/// blocking the event loop during TCP connection establishment. The task
/// sends state updates back to the event loop via a channel, and the event
/// loop applies these updates to RuntimeState.
pub enum SubscribeStateUpdate {
    /// Subscribe operation completed successfully - apply state changes
    Success {
        /// Closure that applies all state changes (subscriptions, pending, etc.)
        apply_state: Box<dyn FnOnce(&mut RuntimeState) + Send>,
    },
    /// Subscribe operation failed - send error to response channel
    Failed {
        /// The response channel to send the error to
        response: tokio::sync::oneshot::Sender<crate::error::Result<u64>>,
        /// The error that occurred
        error: crate::error::Error,
    },
}

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
    mut tcp_cleanup_rx: mpsc::Receiver<TcpCleanupRequest>,
    mut state: RuntimeState,
    tcp_pool: TcpConnectionPool<T>,
) {
    // TcpConnectionPool now uses DashMap internally for lock-free concurrent access
    let tcp_pool = Arc::new(tcp_pool);

    // Channel for Subscribe state updates from spawned tasks
    let (subscribe_update_tx, mut subscribe_update_rx) = mpsc::channel::<SubscribeStateUpdate>(32);

    let mut buf = [0u8; 65535];
    let cycle_interval = Duration::from_millis(config.cyclic_offer_delay);
    let mut next_periodic_cycle_at = tokio::time::Instant::now() + cycle_interval;
    state.last_periodic_cycle = Some(next_periodic_cycle_at);

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
                        let Some(received) = buf.get(..len) else {
                            continue;
                        };
                        let mut data: &[u8] = received;

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
                // Convert to Bytes for unified handling (this is the one copy for UDP)
                let data = Bytes::from(method_msg.data);
                let mut cursor = &data[..];
                let Some(header) = Header::parse(&mut cursor) else {
                    tracing::warn!("Received invalid SOME/IP header on method socket from {}", method_msg.from);
                    continue;
                };

                if header.service_id == SD_SERVICE_ID {
                    tracing::warn!("Received SD message on method socket from {}", method_msg.from);
                    continue;
                }

                if let Some(actions) = handle_method_message(&header, &data, method_msg.from, &mut state, method_msg.service_key, Transport::Udp, 0) {
                    for action in actions {
                        execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
                    }
                }
            }

            // Handle incoming RPC messages from TCP server connections
            Some(tcp_msg) = tcp_rpc_rx.recv() => {
                // For TCP messages, we need to find the service key by looking up which service
                // this message is for (based on the service_id in the header)
                // TCP data is already Bytes - zero-copy path
                let mut cursor = &tcp_msg.data[..];
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

                if let Some(actions) = handle_method_message(&header, &tcp_msg.data, tcp_msg.from, &mut state, service_key, Transport::Tcp, tcp_msg.subscription_id) {
                    for action in actions {
                        execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
                    }
                }
            }

            // Handle responses received on client TCP connections
            Some(tcp_msg) = tcp_client_rx.recv() => {
                // These are responses to RPC calls we made as a client
                // Process them like any other incoming packet
                // TCP data is already Bytes - zero-copy path
                let mut cursor = &tcp_msg.data[..];
                let Some(header) = Header::parse(&mut cursor) else {
                    tracing::warn!("Received invalid SOME/IP header on TCP client socket from {}", tcp_msg.from);
                    continue;
                };

                if header.service_id == SD_SERVICE_ID {
                    tracing::warn!("Received SD message on TCP client socket from {}", tcp_msg.from);
                    continue;
                }

                if let Some(actions) = handle_method_message(&header, &tcp_msg.data, tcp_msg.from, &mut state, None, Transport::Tcp, tcp_msg.subscription_id) {
                    for action in actions {
                        execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
                    }
                }
            }

            // Handle TCP connection cleanup requests
            Some(cleanup_request) = tcp_cleanup_rx.recv() => {
                tcp_pool.handle_cleanup(&cleanup_request);
            }

            // Flush pending initial offers if deadline has passed AND we're not too close to periodic cycle
            () = async {
                if let Some(deadline) = state.pending_offers_deadline {
                    tokio::time::sleep_until(deadline).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                if let Some(actions) = flush_pending_initial_offers(&config, &mut state) {
                    for action in actions {
                        execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
                    }
                }
            }

            // Flush pending unicast SD actions when deadline is reached
            () = state.await_pending_unicast_sd_flush_deadline() => {
                for action in state.flush_pending_unicast_sd() {
                    execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
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
                            if let Err(e) = context.rpc_transport.send(response_data, context.client_addr).await {
                                tracing::error!("Failed to send response during shutdown: {}", e);
                            }
                        }
                        tracing::info!("SomeIp shutdown complete");
                        break;
                    }
                    // Special handling for Offer - needs async socket/listener creation
                    Some(Command::Offer { service_id, instance_id, major_version, minor_version, offer_config, response }) => {
                        server::handle_offer_command::<U, T, L>(
                            service_id, instance_id, major_version, minor_version, offer_config, response,
                            &config, &mut state, &rpc_tx, &tcp_rpc_tx
                        ).await;                    }
                    // Special handling for Subscribe
                    // - TCP subscriptions: spawn as concurrent task to avoid blocking on connection establishment (feat_req_someipsd_767)
                    // - UDP subscriptions: handle inline since binding is instant and doesn't block
                    Some(Command::Subscribe { service_id, instance_id, major_version, eventgroup_ids, events, response }) => {
                        let service_key = ServiceKey::new(service_id, instance_id, major_version);

                        // Determine transport and extract endpoints (if discovered)
                        let prefer_tcp = config.preferred_transport == Transport::Tcp;
                        let (sd_endpoint, endpoint, transport) = {
                            let Some(discovered) = state.discovered.get(&service_key) else {
                                tracing::error!(
                                    "Cannot subscribe to {:04x}:{:04x} v{} eventgroups {:?}: service not discovered (discovered services: {:?})",
                                    service_id.value(),
                                    instance_id.value(),
                                    major_version,
                                    eventgroup_ids,
                                    state.discovered.iter().map(|e| (*e.key(), e.value().udp_endpoint, e.value().tcp_endpoint)).collect::<Vec<_>>()
                                );
                                let _ = response.send(Err(Error::ServiceUnavailable));
                                continue;
                            };
                            let Some((endpoint, transport)) = discovered.method_endpoint(prefer_tcp) else {
                                tracing::error!(
                                    "Cannot subscribe to {:04x}:{:04x} v{} eventgroups {:?}: no valid endpoint",
                                    service_id.value(),
                                    instance_id.value(),
                                    major_version,
                                    eventgroup_ids,
                                );
                                let _ = response.send(Err(Error::ServiceUnavailable));
                                continue;
                            };
                            (discovered.sd_endpoint, endpoint, transport)
                        };

                        if transport == Transport::Tcp {
                            let tcp_pool_clone = Arc::clone(&tcp_pool);
                            let update_tx = subscribe_update_tx.clone();
                            let subscription_id = state.next_subscription_id();
                            let sd_flags = state.sd_flags(true);
                            let subscribe_ttl = state.config.subscribe_ttl;

                            let used_conn_keys: std::collections::HashSet<u64> = state.subscriptions.get(&service_key)
                                .map_or_else(std::collections::HashSet::default, |subs| {
                                    subs.iter().map(|sub| sub.tcp_conn_key).collect()
                                });

                            tokio::spawn(async move {
                                handle_subscribe_tcp::<T>(
                                    service_id,
                                    instance_id,
                                    major_version,
                                    eventgroup_ids,
                                    events,
                                    response,
                                    tcp_pool_clone,
                                    update_tx,
                                    sd_endpoint,
                                    endpoint,
                                    subscription_id,
                                    sd_flags,
                                    subscribe_ttl,
                                    used_conn_keys,
                                ).await;
                            });
                        } else {
                            // UDP path: Handle inline (binding is instant)
                            client::handle_subscribe_udp::<U>(
                                service_id, instance_id, major_version, eventgroup_ids,
                                events, response, sd_endpoint, &mut state
                            ).await;
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

            // Handle Subscribe state updates from spawned tasks
            Some(update) = subscribe_update_rx.recv() => {
                match update {
                    SubscribeStateUpdate::Success { apply_state } => {
                        // Apply state changes from the Subscribe task
                        apply_state(&mut state);
                    }
                    SubscribeStateUpdate::Failed { response, error } => {
                        // Send error back to caller
                        let _ = response.send(Err(error));
                    }
                }
            }

            // Periodic tasks (cyclic offers, find retries)
            () = tokio::time::sleep_until(next_periodic_cycle_at) => {
                let now = tokio::time::Instant::now();
                // Track when periodic cycle runs (use the scheduled time, not now)
                state.last_periodic_cycle = Some(next_periodic_cycle_at);

                // Schedule next cycle
                next_periodic_cycle_at = now + cycle_interval;

                // Also flush any pending initial offers at this time
                if let Some(actions) = flush_pending_initial_offers(&config, &mut state) {
                    for action in actions {
                        execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
                    }
                }

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
                    Err(ref e) => {
                        let return_code = match e {
                            Error::Protocol(proto_err) => proto_err.return_code.unwrap_or(0x01),
                            _ => 0x01, // NOT_OK for other errors
                        };
                        build_response(
                            context.service_id,
                            context.method_id,
                            context.client_id,
                            context.session_id,
                            context.interface_version,
                            return_code,
                            &[],
                            context.uses_exception,
                        )
                    }
                };

                // Send response via the captured RPC transport
                // This works even if the service offering has been dropped
                if let Err(e) = context.rpc_transport.send(response_data, context.client_addr).await {
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
    tcp_pool: &Arc<TcpConnectionPool<T>>,
) {
    match action {
        Action::SendSd { message, target } => {
            // Determine if this is multicast or unicast based on the FLAG_UNICAST bit in the message
            // Per feat_req_someipsd_41, use separate session counters
            let is_unicast = (message.flags & SdMessage::FLAG_UNICAST) != 0;
            let session_id = if is_unicast {
                state.next_unicast_session_id()
            } else {
                state.next_multicast_session_id()
            };
            let entry_types: Vec<String> = message
                .entries
                .iter()
                .map(|e| {
                    format!(
                        "{:?} {}:{:04X} v{} EG:{:04X}",
                        e.entry_type, e.service_id, e.instance_id, e.major_version, e.eventgroup_id
                    )
                })
                .collect();
            tracing::debug!(
                "[{:?}] Sending SD to {} ({}): session_id={}, {} entries: {:?}",
                tokio::time::Instant::now(),
                target,
                if is_unicast { "unicast" } else { "multicast" },
                session_id,
                message.entries.len(),
                entry_types
            );
            let data = message.serialize(session_id);
            if let Err(e) = sd_socket.send_to(&data, target).await {
                tracing::error!("Failed to send SD message: {}", e);
            }
        }
        Action::NotifyFound { key, availability } => {
            // Find all matching find requests and notify them
            for (req_key, request) in &state.find_requests {
                if req_key.matches(key) {
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
                if let Err(e) = tcp_pool.send(target, data).await {
                    tracing::error!("Failed to send client SOME/IP message via TCP: {}", e);
                }
            } else {
                // Client messages use dedicated client RPC socket (NOT SD socket)
                // Per feat_req_someip_676: Port 30490 is only for SD, not for RPC
                if let Err(e) = state
                    .client_rpc_tx
                    .send(RpcSendMessage { data, to: target })
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
                    if let Err(e) = method_transport.send(data, target).await {
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
        Action::ResetPeerTcpConnections {
            peer,
            server_ports,
            client_ports,
            close_client_pool,
        } => {
            // Close TCP connections related to the rebooted peer
            tracing::info!(
                "Detected reboot of peer {}, closing TCP connections: server_ports={:?}, client_ports={:?}, close_client_pool={}",
                peer, server_ports, client_ports, close_client_pool
            );

            // Close client-side TCP connections to this peer (outgoing connections)
            // Per feat_req_someipsd_872: reset TCP state on peer reboot
            if close_client_pool {
                // Close ALL client connections to this peer
                tcp_pool.close_all_to_peer(peer);
            } else if !client_ports.is_empty() {
                // Close only specific client connections (to removed services at these ports)
                // This preserves connections for kept subscriptions
                tcp_pool.close_to_peer_ports(peer, &client_ports);
            }

            // Close server-side TCP connections from this peer (only specific ports)
            // These are connections FROM the peer TO our services
            // For each offered service with a TCP server, tell it to close specific connections
            if !server_ports.is_empty() {
                for offered in state.offered.values() {
                    if let Some(ref close_tx) = offered.tcp_close_peer_tx {
                        // Non-blocking send - channel might be full or closed
                        // Send (peer_ip, server_ports) tuple
                        let _ = close_tx.try_send((peer, server_ports.clone()));
                    }
                }
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
    header: &Header,
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

    // Extract session info and check for peer reboot
    let peer_ip = from.ip();
    if let Some(reboot_actions) = detect_and_handle_peer_reboot(header, &sd_message, peer_ip, state)
    {
        actions.extend(reboot_actions);
    }

    // Process SD entries - new Offers will re-add services after reboot clear
    process_sd_entries(&sd_message, from, state, &mut actions);

    // Cluster multiple SendSd actions with SD messages going to the same target
    // This prevents duplicate session IDs and reboot detection issues
    actions = cluster_sd_actions(actions);

    if actions.is_empty() {
        None
    } else {
        Some(actions)
    }
}

/// Detect peer reboot and handle all consequences
///
/// Returns actions to execute (TCP connection resets, etc.)
fn detect_and_handle_peer_reboot(
    header: &Header,
    sd_message: &SdMessage,
    peer_ip: std::net::IpAddr,
    state: &mut RuntimeState,
) -> Option<Vec<Action>> {
    // Extract session info for reboot detection (feat_req_someipsd_764, feat_req_someipsd_765)
    let peer_reboot_flag = (sd_message.flags & SdMessage::FLAG_REBOOT) != 0;
    let peer_unicast_flag = (sd_message.flags & SdMessage::FLAG_UNICAST) != 0;
    let session_id = header.session_id;

    // Determine the channel type based on the UNICAST flag in the SD message
    // Per feat_req_someipsd_765, each peer has separate session counters for:
    // - Multicast channel (FLAG_UNICAST = 0)
    // - Unicast channel (FLAG_UNICAST = 1)
    let channel = if peer_unicast_flag {
        SdChannel::Unicast
    } else {
        SdChannel::Multicast
    };

    tracing::debug!(
        "[{:?}] SD from {} on {:?} channel: session_id={}, reboot_flag={}, entries: {:}",
        tokio::time::Instant::now(),
        peer_ip,
        channel,
        session_id,
        peer_reboot_flag,
        sd_message
            .entries
            .iter()
            .map(|e| format!("{e}"))
            .collect::<Vec<_>>()
            .join(", "),
    );

    // Check for peer reboot using proper session tracking
    tracing::debug!(
        "Checking reboot for peer_ip={} on {:?} channel",
        peer_ip,
        channel
    );
    let peer_state = state.peer_sessions.entry(peer_ip).or_default();
    let reboot_detected = peer_state.check_reboot_and_reset(channel, session_id, peer_reboot_flag);

    if reboot_detected {
        tracing::info!("Detected reboot of peer {}", peer_ip);
        Some(handle_peer_reboot(peer_ip, state))
    } else {
        None
    }
}

/// Handle all consequences of a peer reboot
///
/// Per feat_req_someipsd_871: Services shall be expired if not updated after reboot.
/// Per feat_req_someipsd_872: TCP connections to rebooted peer shall be reset.
///
/// Returns actions to execute (TCP connection resets)
fn handle_peer_reboot(peer_ip: std::net::IpAddr, state: &mut RuntimeState) -> Vec<Action> {
    // Expire services and collect TCP ports to close
    let (has_kept_subscriptions, removed_service_tcp_ports) =
        expire_services_from_rebooted_peer(peer_ip, state);

    // Expire server-side subscriptions and collect TCP ports to close
    let expired_tcp_ports = expire_server_subscriptions_from_rebooted_peer(peer_ip, state);

    // Expire client-side subscriptions
    expire_client_subscriptions_to_rebooted_peer(peer_ip, state);

    // Queue TCP connection reset action
    let mut actions = Vec::new();
    queue_tcp_reset_for_rebooted_peer(
        peer_ip,
        has_kept_subscriptions,
        expired_tcp_ports,
        removed_service_tcp_ports,
        &mut actions,
    );

    actions
}

/// Expire services from a rebooted peer
///
/// Returns (has_kept_subscriptions, removed_service_tcp_ports)
fn expire_services_from_rebooted_peer(
    peer_ip: std::net::IpAddr,
    state: &mut RuntimeState,
) -> (bool, Vec<u16>) {
    tracing::debug!("Expiring services from rebooted peer {}", peer_ip);

    // Find all services from this peer
    let mut services_to_remove = Vec::new();
    for entry in state.discovered.iter() {
        let is_from_peer = entry
            .value()
            .udp_endpoint
            .is_some_and(|addr| addr.ip() == peer_ip)
            || entry
                .value()
                .tcp_endpoint
                .is_some_and(|addr| addr.ip() == peer_ip);
        if is_from_peer {
            services_to_remove.push(*entry.key());
        }
    }

    let mut has_kept_subscriptions = false;
    let mut removed_service_tcp_ports: Vec<u16> = Vec::new();

    for key in services_to_remove {
        // Check if we should keep this service (has subscriptions with pending ACKs)
        let should_keep_service = state.subscriptions.get(&key).is_some_and(|subs| {
            subs.iter().any(|sub| {
                let pending_key = PendingSubscriptionKey {
                    service_id: key.service_id,
                    instance_id: key.instance_id,
                    major_version: key.major_version,
                    eventgroup_id: sub.eventgroup_id,
                };
                state.pending_subscriptions.contains_key(&pending_key)
            })
        });

        if should_keep_service {
            // Keep the service AND subscription (server knows about both)
            has_kept_subscriptions = true;
            tracing::debug!(
                "Keeping service {:04X}:{:04X} and subscription (has pending ACK, server knows about it)",
                key.service_id,
                key.instance_id
            );
        } else {
            // Remove service and subscription
            if let Some((_, svc)) = state.discovered.remove(&key) {
                tracing::debug!(
                    "Removed service {:04X}:{:04X} from peer {} (not yet re-offered after reboot)",
                    key.service_id,
                    key.instance_id,
                    peer_ip
                );

                // Collect TCP port of this service for connection cleanup
                if let Some(tcp_ep) = svc.tcp_endpoint {
                    if tcp_ep.ip() == peer_ip {
                        removed_service_tcp_ports.push(tcp_ep.port());
                        tracing::debug!(
                            "Marking TCP port {} for closure (service {:04X}:{:04X} removed)",
                            tcp_ep.port(),
                            key.service_id,
                            key.instance_id
                        );
                    }
                }

                state.find_requests.remove(&key);
                state.subscriptions.remove(&key);
            }
        }
    }

    (has_kept_subscriptions, removed_service_tcp_ports)
}

/// Expire server-side subscriptions from a rebooted client
///
/// Returns TCP ports of expired subscriptions
fn expire_server_subscriptions_from_rebooted_peer(
    peer_ip: std::net::IpAddr,
    state: &mut RuntimeState,
) -> Vec<u16> {
    tracing::debug!("Expiring subscriptions from rebooted peer {}", peer_ip);

    let mut expired_tcp_ports: Vec<u16> = Vec::new();
    let subscriber_keys: Vec<_> = state.server_subscribers.keys().copied().collect();

    for key in subscriber_keys {
        if let Some(subs) = state.server_subscribers.get_mut(&key) {
            let before_count = subs.len();
            // Collect ports of TCP subscriptions from this peer before removing
            for sub in subs.iter() {
                if sub.endpoint.ip() == peer_ip && sub.transport == Transport::Tcp {
                    expired_tcp_ports.push(sub.endpoint.port());
                }
            }
            subs.retain(|sub| sub.endpoint.ip() != peer_ip);
            let removed = before_count - subs.len();
            if removed > 0 {
                tracing::debug!(
                    "Removed {} subscription(s) from peer {} for service {:04X}:{:04X}",
                    removed,
                    peer_ip,
                    key.service_id,
                    key.instance_id
                );
            }
        }
    }
    state.server_subscribers.retain(|_, subs| !subs.is_empty());

    expired_tcp_ports
}

/// Expire client-side subscriptions to a rebooted server
///
/// Subscriptions with pending ACKs are NOT expired - the server knows about them.
fn expire_client_subscriptions_to_rebooted_peer(
    peer_ip: std::net::IpAddr,
    state: &mut RuntimeState,
) {
    tracing::debug!(
        "Expiring client-side subscriptions for rebooted peer {}",
        peer_ip
    );

    let subscriptions_to_remove: Vec<_> = state
        .subscriptions
        .iter()
        .filter_map(|(key, subs)| {
            // Check if this subscription is to the rebooted peer
            if let Some(svc) = state.discovered.get(key) {
                let is_from_peer = svc.udp_endpoint.is_some_and(|addr| addr.ip() == peer_ip)
                    || svc.tcp_endpoint.is_some_and(|addr| addr.ip() == peer_ip);
                if is_from_peer {
                    // Check if ANY eventgroup in this subscription is pending ACK
                    // If so, don't expire - the server knows about this subscription
                    let has_pending = subs.iter().any(|sub| {
                        let pending_key = PendingSubscriptionKey {
                            service_id: key.service_id,
                            instance_id: key.instance_id,
                            major_version: key.major_version,
                            eventgroup_id: sub.eventgroup_id,
                        };
                        state.pending_subscriptions.contains_key(&pending_key)
                    });

                    if has_pending {
                        tracing::debug!(
                            "Not expiring subscription {:04X}:{:04X} - has pending ACK (server knows about it)",
                            key.service_id,
                            key.instance_id
                        );
                    } else {
                        return Some(*key);
                    }
                }
            }
            None
        })
        .collect();

    for key in subscriptions_to_remove {
        if let Some(subs) = state.subscriptions.remove(&key) {
            let count = subs.len();
            tracing::debug!(
                "Expired {} subscription(s) to {:04X}:{:04X} (peer {} rebooted)",
                count,
                key.service_id,
                key.instance_id,
                peer_ip
            );
        }
    }
}

/// Queue TCP connection reset action for a rebooted peer
fn queue_tcp_reset_for_rebooted_peer(
    peer_ip: std::net::IpAddr,
    has_kept_subscriptions: bool,
    expired_tcp_ports: Vec<u16>,
    removed_service_tcp_ports: Vec<u16>,
    actions: &mut Vec<Action>,
) {
    if has_kept_subscriptions {
        tracing::debug!(
            "Keeping TCP pool open for peer {} - have subscriptions with pending ACKs, but closing: server_ports={:?}, client_ports={:?}",
            peer_ip,
            expired_tcp_ports,
            removed_service_tcp_ports
        );
        // Close specific connections:
        // - Server-side: expired_tcp_ports (connections FROM peer on these local ports)
        // - Client-side: removed_service_tcp_ports (connections TO peer on these remote ports)
        // Do NOT close entire client pool - new connections exist for kept subscriptions
        if !expired_tcp_ports.is_empty() || !removed_service_tcp_ports.is_empty() {
            tracing::debug!(
                "Closing expired TCP connections from peer {}: server_ports={:?}, client_ports={:?}",
                peer_ip,
                expired_tcp_ports,
                removed_service_tcp_ports
            );
            actions.push(Action::ResetPeerTcpConnections {
                peer: peer_ip,
                server_ports: expired_tcp_ports,
                client_ports: removed_service_tcp_ports,
                close_client_pool: false, // Keep other client connections alive
            });
        }
        tracing::debug!("Keeping TCP pool open - have subscriptions with pending ACKs");
    } else {
        tracing::debug!(
            "Closing ALL TCP connections from peer {} (no subscriptions with pending ACKs, safe to reset entire pool)",
            peer_ip
        );
        // No subscriptions kept - safe to reset entire TCP pool
        actions.push(Action::ResetPeerTcpConnections {
            peer: peer_ip,
            server_ports: expired_tcp_ports,
            client_ports: removed_service_tcp_ports,
            close_client_pool: true, // Close all client connections
        });
    }
}

/// Process SD entries in a message
fn process_sd_entries(
    sd_message: &SdMessage,
    from: SocketAddr,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    for entry in &sd_message.entries {
        match entry.entry_type {
            SdEntryType::OfferService => {
                if entry.is_stop() {
                    handle_sd_stop_offer(entry, state, actions);
                } else {
                    handle_offer(entry, sd_message, from, state, actions);
                }
            }
            SdEntryType::FindService => {
                handle_find_request(entry, from, state);
            }
            SdEntryType::SubscribeEventgroup => {
                if entry.is_stop() {
                    handle_unsubscribe_request(entry, sd_message, from, state);
                } else {
                    handle_subscribe_request(entry, sd_message, from, state);
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
}

/// Cluster multiple `SendSd` actions going to the same target into a single SD message
///
/// This prevents duplicate session IDs when responding to multi-entry SD messages
/// (e.g., client sends Subscribe for multiple eventgroups, server responds with
/// multiple Acks in ONE SD message instead of separate messages with different session IDs)
///
/// **Important:** When merging messages, option indices in entries are recalculated to point
/// to the correct positions in the merged option array.
pub fn cluster_sd_actions(actions: Vec<Action>) -> Vec<Action> {
    use std::collections::HashMap;

    let mut sd_by_target: HashMap<SocketAddr, SdMessage> = HashMap::new();
    let mut other_actions = Vec::new();

    for action in actions {
        match action {
            Action::SendSd { message, target } => {
                // Merge this SD message into the existing message for this target
                let clustered = sd_by_target.entry(target).or_insert_with(|| {
                    // Create new message with same flags as the first message to this target
                    SdMessage::new(message.flags)
                });

                // Calculate the option offset for this message's entries
                // All options added so far determine the new base index
                let option_offset = clustered.options.len() as u8;

                // First, add all options from this message to the clustered message
                for opt in &message.options {
                    clustered.add_option(opt.clone());
                }

                // Then add all entries, adjusting their option indices to account for the offset
                for mut sd_entry in message.entries.clone() {
                    // Adjust option indices to account for previously added options
                    if sd_entry.num_options_1 > 0 {
                        sd_entry.index_1st_option =
                            sd_entry.index_1st_option.saturating_add(option_offset);
                    }
                    if sd_entry.num_options_2 > 0 {
                        sd_entry.index_2nd_option =
                            sd_entry.index_2nd_option.saturating_add(option_offset);
                    }
                    clustered.add_entry(sd_entry);
                }
            }
            other => other_actions.push(other),
        }
    }

    // Convert clustered SD messages back to actions
    other_actions.extend(
        sd_by_target
            .into_iter()
            .map(|(target, message)| Action::SendSd { message, target }),
    );

    other_actions
}

/// Handle an RPC message (Request, Response, etc.)
///
/// Takes a reference to message data as `Bytes` to enable zero-copy payload extraction
/// via `Bytes::slice()`. For TCP messages this is truly zero-copy; for UDP messages
/// the data is copied once when converting from the receive buffer to `Bytes`.
fn handle_method_message(
    header: &Header,
    data: &Bytes,
    from: SocketAddr,
    state: &mut RuntimeState,
    service_key: Option<ServiceKey>,
    transport: Transport,
    subscription_id: u64,
) -> Option<Vec<Action>> {
    // Validate protocol version - silently drop messages with wrong version
    if validate_protocol_version(header.protocol_version).is_err() {
        tracing::trace!(
            "Dropping message with invalid protocol version 0x{:02X} from {}",
            header.protocol_version,
            from
        );
        return None;
    }

    let payload_len = header.payload_length();
    let payload_start = Header::SIZE;
    let payload_end = payload_start + payload_len;

    if data.len() < payload_end {
        return None;
    }

    // Zero-copy slice of the payload from the message buffer
    let payload = data.slice(payload_start..payload_end);

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
            // Incoming fire-and-forget - route to offering with service_key for validation
            server::handle_incoming_fire_forget(header, payload, from, state, service_key);
        }
        MessageType::Response | MessageType::Error => {
            // Response to our request - route to pending call
            client::handle_incoming_response(header, payload, state);
        }
        MessageType::Notification => {
            // Event notification - route to subscription
            client::handle_incoming_notification(header, payload, from, state, subscription_id);
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
                &payload,
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
                &payload,
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
                &payload,
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
    let sd_flags = state.sd_flags(false); // Multicast periodic messages, so FLAG_UNICAST=0
    let offer_ttl = state.config.offer_ttl;
    let find_ttl = state.config.find_ttl;
    let sd_multicast = state.config.sd_multicast;

    // Cyclic offers (only for services that are announcing)
    for (key, offered) in &mut state.offered {
        // Skip services that are bound but not announcing
        if !offered.is_announcing {
            continue;
        }

        let elapsed = now.duration_since(offered.last_offer);
        tracing::debug!(
            "[{:?}] [OFFER] Checking cyclic offer for {}:{:04X} v{}: elapsed={:?}ms, interval={:?}ms",
            now,
            key.service_id,
            key.instance_id,
            offered.major_version,
            elapsed.as_millis(),
            offer_interval.as_millis()
        );

        if elapsed >= offer_interval / 2 {
            offered.last_offer = now;
            tracing::debug!(
                "[OFFER] [{:?}] Sending cyclic offer for {}:{:04X} v{}",
                now,
                key.service_id,
                key.instance_id,
                offered.major_version
            );

            let msg = build_offer_message(
                *key,
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
    for entry in state.discovered.iter() {
        if !entry.is_alive() {
            expired_discovered.push(*entry.key());
        }
    }

    for key in expired_discovered {
        if state.discovered.remove(&key).is_some() {
            // Also clear any cached subscriptions to this service since it's no longer available
            state.subscriptions.remove(&key);

            // Emit SD event to monitors
            actions.push(Action::EmitSdEvent {
                event: crate::SdEvent::ServiceExpired {
                    service_id: key.service_id,
                    instance_id: key.instance_id,
                },
            });
        }
    }

    // Expire stale server-side subscriptions (feat_req_someipsd_445)
    // Remove subscriptions whose TTL has elapsed without renewal
    // Note: expires_at=None means infinite TTL (0xFFFFFF) per feat_req_someipsd_431
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

    // Cluster SD actions going to the same target to prevent duplicate session IDs
    // This is especially important for cyclic offers when multiple services are offered
    actions = cluster_sd_actions(actions);

    if actions.is_empty() {
        None
    } else {
        Some(actions)
    }
}

/// Flush pending initial offers - sends all queued offers in one clustered message
///
/// This is called when the flush deadline is reached (~50ms after first offer).
/// Clustering prevents multiple session IDs when starting multiple services quickly.
fn flush_pending_initial_offers(
    config: &RuntimeConfig,
    state: &mut RuntimeState,
) -> Option<Vec<Action>> {
    if state.pending_initial_offers.is_empty() {
        state.pending_offers_deadline = None;
        return None;
    }

    let mut msg = SdMessage::new(state.sd_flags(false)); // Multicast

    // Helper to get the IP address - use advertised_ip if set, otherwise use endpoint IP
    let get_ip = |ep: SocketAddr| -> std::net::Ipv4Addr {
        if let Some(std::net::IpAddr::V4(ip)) = config.advertised_ip {
            ip
        } else {
            match ep {
                SocketAddr::V4(v4) => *v4.ip(),
                _ => std::net::Ipv4Addr::LOCALHOST,
            }
        }
    };

    // Build clustered offer message for all pending offers
    for key in &state.pending_initial_offers {
        if let Some(offered) = state.offered.get_mut(key) {
            offered.last_offer = tokio::time::Instant::now();
            let mut option_indices = Vec::new();

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

            let entry = SdEntry::offer_service(
                key.service_id,
                key.instance_id,
                offered.major_version,
                offered.minor_version,
                config.offer_ttl,
                first_opt_idx,
                num_options,
            );
            msg.add_entry(entry);
        }
    }

    tracing::debug!(
        "[OFFER] Flushing {} pending initial offers (clustered into one SD message)",
        state.pending_initial_offers.len()
    );

    state.pending_initial_offers.clear();
    state.pending_offers_deadline = None;

    Some(vec![Action::SendSd {
        message: msg,
        target: config.sd_multicast,
    }])
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

    let session_id = state.next_multicast_session_id();
    let data = msg.serialize(session_id);
    let _ = sd_socket.send_to(&data, config.sd_multicast).await;
}
