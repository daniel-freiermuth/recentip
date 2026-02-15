//! # TCP Connection Management
//!
//! This module handles TCP connections for SOME/IP RPC communication.
//! It provides connection pooling, message framing, and Magic Cookie support.
//!
//! ## SOME/IP TCP Requirements
//!
//! Per specification:
//! - **`feat_req_someip_644`**: Single TCP connection per client-server pair
//! - **`feat_req_someip_646`**: Client opens connection on first request
//! - **`feat_req_someip_647`**: Client reestablishes after failure
//! - **`feat_req_someip_586`**: Optional Magic Cookies for resynchronization
//!
//! ## Connection Pool
//!
//! The [`TcpConnectionPool`] maintains one connection per remote peer:
//!
//! ```text
//! TcpConnectionPool
//!     │
//!     ├─── 192.168.1.10:30500 ───▶ [TCP Connection + Reader Task]
//!     ├─── 192.168.1.20:30500 ───▶ [TCP Connection + Reader Task]
//!     └─── 192.168.1.30:30500 ───▶ [TCP Connection + Reader Task]
//! ```
//!
//! ## Message Framing
//!
//! TCP doesn't have message boundaries. SOME/IP uses the length field in the
//! header to delimit messages:
//!
//! ```text
//! ┌────────────────────────────────────────────────┐
//! │ [Magic Cookie?] [Msg1] [Msg2] [Msg3] ...    │
//! │ ────────────── ───── ───── ─────           │
//! │      16 bytes   Header + Payload per msg    │
//! └────────────────────────────────────────────────┘
//! ```
//!
//! ## Magic Cookies
//!
//! When enabled, each TCP segment starts with a 16-byte Magic Cookie for
//! stream resynchronization (useful in testing/debugging):
//!
//! - Service ID: 0xFFFF
//! - Method ID: 0x0000 (client) or 0x8000 (server)
//! - Recognizable pattern allows recovery from stream corruption
//!
//! ## Server-Side TCP
//!
//! The [`TcpServer`] handles incoming connections for an offered service.
//! It spawns reader tasks for each accepted connection.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::{Buf, Bytes, BytesMut};
use dashmap::DashMap;
use tokio::sync::mpsc;

use crate::net::{TcpListener, TcpStream};
use crate::wire::{
    is_magic_cookie, magic_cookie_client, magic_cookie_server, parse_someip_length, Header,
};

/// Message received from a TCP connection (client-side response or server-side request)
#[derive(Debug)]
pub struct TcpMessage {
    /// The raw message data including SOME/IP header
    pub data: Bytes,
    /// The peer address this message came from
    pub from: SocketAddr,
    /// The subscription ID this message is for (0 for RPC)
    pub subscription_id: u64,
}

/// Request to clean up a TCP connection.
///
/// Sent by connection handler tasks when they exit. The event loop verifies
/// ownership via `connection_id` before removing — this prevents a race where
/// closing an old connection removes a newer connection's entry.
#[derive(Debug)]
pub struct TcpCleanupRequest {
    /// The connection key (peer address, subscription_id)
    pub key: (SocketAddr, u64),
    /// Unique ID assigned when the connection was created
    pub connection_id: u64,
}

/// TCP connection pool manages connections to remote peers.
///
/// Connections are keyed by (`remote_endpoint`, `subscription_id`) to allow
/// separate connections per subscription (like UDP's per-subscription sockets).
/// The pool handles connection establishment, reuse, and reconnection.
///
/// For client-side TCP: When a connection is established, a reader task is spawned
/// to receive responses and forward them to the runtime.
///
/// Uses a single DashMap with `OnceCell` for both coordination of concurrent
/// connection attempts AND storage of established connection state.
pub(crate) struct TcpConnectionPool<T: TcpStream> {
    /// Connections indexed by (peer address, `subscription_id`).
    /// OnceCell coordinates concurrent attempts: first caller connects, others wait.
    /// Once initialized, contains full connection state.
    connections: Arc<DashMap<(SocketAddr, u64), Arc<tokio::sync::OnceCell<TcpConnectionState>>>>,
    /// Channel to forward received messages to the runtime
    msg_tx: mpsc::Sender<TcpMessage>,
    /// Channel to send cleanup requests to the event loop
    cleanup_tx: mpsc::Sender<TcpCleanupRequest>,
    /// Counter for generating unique connection IDs (atomic for thread-safety)
    next_connection_id: Arc<std::sync::atomic::AtomicU64>,
    /// Enable Magic Cookies for TCP resync (`feat_req_someip_586`)
    magic_cookies: bool,
    /// Phantom data for the stream type - use `fn()` -> T for Send+Sync
    _phantom: std::marker::PhantomData<fn() -> T>,
}

/// Full state of an established TCP connection
struct TcpConnectionState {
    /// The local address of this connection (what the peer sees us as)
    local_addr: SocketAddr,
    /// Handle to abort the reader task
    task_handle: tokio::task::AbortHandle,
    /// Unique ID for this connection (for cleanup verification)
    connection_id: u64,
    /// Channel to send data on this connection
    sender: mpsc::Sender<Bytes>,
}

impl<T: TcpStream> TcpConnectionPool<T> {
    /// Create a new connection pool with a message receiver channel.
    ///
    /// The `cleanup_tx` channel is used to send cleanup requests to the event loop
    /// when connections close. This ensures proper synchronization and prevents
    /// race conditions where closing an old connection could remove a newer one.
    pub fn new(
        msg_tx: mpsc::Sender<TcpMessage>,
        cleanup_tx: mpsc::Sender<TcpCleanupRequest>,
        magic_cookies: bool,
    ) -> Self {
        Self {
            connections: Arc::new(DashMap::new()),
            msg_tx,
            cleanup_tx,
            next_connection_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            magic_cookies,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Send data to a peer, establishing connection if needed.
    ///
    /// This implements:
    /// - `feat_req_someip_646`: Opens connection on first request
    /// - `feat_req_someip_644`: Reuses existing connection
    ///
    /// When a new connection is established, a reader task is spawned to receive
    /// responses and forward them to the runtime via the `msg_tx` channel.
    /// Uses `subscription_id` 0 for RPC traffic (method calls/responses).
    ///
    /// # Errors
    ///
    /// Returns an I/O error if connection or send fails.
    pub async fn send(&self, target: SocketAddr, data: Bytes) -> io::Result<()> {
        // Use subscription_id 0 for RPC traffic
        let key = (target, 0);

        // Get or create connection using the same mechanism as ensure_connected
        let cell = self
            .connections
            .entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
            .clone();

        let state = cell.get_or_try_init(|| self.do_connect(target, 0)).await?;

        // Send the data
        state
            .sender
            .send(data)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Connection task closed"))?;

        Ok(())
    }

    /// Ensure a connection exists to a peer and return our local address on that connection.
    ///
    /// This is used for TCP pub/sub: the client must connect to the server BEFORE
    /// subscribing (per `feat_req_someipsd_767`), and must advertise the correct
    /// endpoint (our local address on the TCP connection) so the server can send
    /// events back on the same connection.
    ///
    /// The `subscription_id` allows multiple connections per server (one per subscription),
    /// mirroring UDP's per-subscription socket approach. Use 0 for RPC connections.
    ///
    /// This method coordinates concurrent connection attempts to the same endpoint:
    /// only one task actually establishes the connection while others wait on the result.
    /// This prevents multiple TCP connections being established to the same endpoint.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if connection establishment fails.
    pub async fn ensure_connected(
        &self,
        target: SocketAddr,
        subscription_id: u64,
    ) -> io::Result<SocketAddr> {
        let key = (target, subscription_id);

        // Get or create a OnceCell for this connection
        let cell = self
            .connections
            .entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
            .clone();

        // OnceCell::get_or_try_init ensures exactly one task runs the init closure.
        // If it fails, the cell stays uninitialized - next call will retry.
        let state = cell
            .get_or_try_init(|| self.do_connect(target, subscription_id))
            .await?;

        Ok(state.local_addr)
    }

    /// Perform the actual TCP connection and setup.
    ///
    /// Called by `OnceCell::get_or_try_init` - exactly one task executes this.
    async fn do_connect(
        &self,
        target: SocketAddr,
        subscription_id: u64,
    ) -> io::Result<TcpConnectionState> {
        tracing::debug!(
            "Establishing TCP connection to {} for subscription (feat_req_someipsd_767)",
            target
        );

        let stream = T::connect(target).await?;

        let local_addr = stream.local_addr()?;

        // Allocate unique connection ID for cleanup verification
        let connection_id = self
            .next_connection_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Create channel for sending data to this connection
        let (send_tx, send_rx) = mpsc::channel::<Bytes>(32);

        // Spawn connection handler task
        let msg_tx = self.msg_tx.clone();
        let cleanup_tx = self.cleanup_tx.clone();
        let magic_cookies = self.magic_cookies;

        let task = tokio::spawn(async move {
            handle_client_tcp_connection(
                stream,
                target,
                msg_tx,
                send_rx,
                cleanup_tx,
                connection_id,
                magic_cookies,
                subscription_id,
            )
            .await;
        });

        Ok(TcpConnectionState {
            local_addr,
            task_handle: task.abort_handle(),
            connection_id,
            sender: send_tx,
        })
    }

    /// Handle a cleanup request from a connection task.
    ///
    /// Only removes the connection if the `connection_id` matches the currently
    /// stored connection. This prevents a race where closing an old connection
    /// would incorrectly remove a newer connection that replaced it.
    pub fn handle_cleanup(&self, request: &TcpCleanupRequest) {
        let key = request.key;

        // Check if the connection still belongs to this task before removing
        let should_remove = self.connections.get(&key).is_some_and(|cell| {
            cell.get()
                .is_some_and(|state| state.connection_id == request.connection_id)
        });

        if should_remove {
            tracing::debug!(
                "Cleaning up TCP connection to {:?} (connection_id={})",
                key,
                request.connection_id
            );
            self.connections.remove(&key);
        } else {
            tracing::debug!(
                "Ignoring cleanup for {:?} (connection_id={}): connection was replaced",
                key,
                request.connection_id
            );
        }
    }

    /// Close ALL connections to a peer IP address (for reboot handling)
    ///
    /// This closes all connections to the given peer IP, regardless of port or
    /// `subscription_id`. Used when a peer reboot is detected per `feat_req_someipsd_872`.
    pub fn close_all_to_peer(&self, peer_ip: std::net::IpAddr) {
        // Collect keys to close (only established connections)
        let keys_to_close: Vec<_> = self
            .connections
            .iter()
            .filter(|entry| entry.key().0.ip() == peer_ip && entry.value().get().is_some())
            .map(|entry| *entry.key())
            .collect();

        if keys_to_close.is_empty() {
            return;
        }

        tracing::debug!(
            "Closing {} TCP connection(s) to peer {}",
            keys_to_close.len(),
            peer_ip
        );

        // Close each connection
        for key in keys_to_close {
            if let Some((_, cell)) = self.connections.remove(&key) {
                if let Some(state) = cell.get() {
                    state.task_handle.abort();
                }
            }
        }
    }

    /// Close connections to specific ports on a peer IP (for selective reboot handling)
    ///
    /// This closes only connections to the given peer IP on the specified ports.
    /// Used when some services are removed during reboot detection but others are kept.
    pub fn close_to_peer_ports(&self, peer_ip: std::net::IpAddr, ports: &[u16]) {
        if ports.is_empty() {
            return;
        }

        // Collect keys to close (only established connections to specified ports)
        let keys_to_close: Vec<_> = self
            .connections
            .iter()
            .filter(|entry| {
                entry.key().0.ip() == peer_ip
                    && ports.contains(&entry.key().0.port())
                    && entry.value().get().is_some()
            })
            .map(|entry| *entry.key())
            .collect();

        if keys_to_close.is_empty() {
            tracing::debug!(
                "No TCP connections to close for peer {} on ports {:?}",
                peer_ip,
                ports
            );
            return;
        }

        tracing::debug!(
            "Closing {} TCP connection(s) to peer {} on ports {:?}",
            keys_to_close.len(),
            peer_ip,
            ports
        );

        // Close each connection
        for key in keys_to_close {
            if let Some((_, cell)) = self.connections.remove(&key) {
                if let Some(state) = cell.get() {
                    state.task_handle.abort();
                }
            }
        }
    }
}

/// Handle a client-side TCP connection - both reading and writing
///
/// When `magic_cookies` is enabled:
/// - Each write is prepended with a Magic Cookie (`feat_req_someip_591`)
/// - Magic Cookies in received data are skipped (`feat_req_someip_586`)
///
/// On exit, sends a cleanup request to the event loop (via `cleanup_tx`) with the
/// `connection_id`. The event loop verifies ownership before removing the entry,
/// preventing a race where closing an old connection removes a newer one.
async fn handle_client_tcp_connection<T: TcpStream>(
    mut stream: T,
    peer_addr: SocketAddr,
    msg_tx: mpsc::Sender<TcpMessage>,
    mut send_rx: mpsc::Receiver<Bytes>,
    cleanup_tx: mpsc::Sender<TcpCleanupRequest>,
    connection_id: u64,
    magic_cookies: bool,
    subscription_id: u64,
) {
    let mut read_buffer = BytesMut::new();
    let mut read_buf = [0u8; 8192];

    loop {
        tokio::select! {
            // Read from the stream
            result = stream.read(&mut read_buf) => {
                match result {
                    Ok(0) => {
                        // Connection closed
                        tracing::debug!("TCP connection to {} closed", peer_addr);
                        break;
                    }
                    Ok(n) => {
                        if let Some(received) = read_buf.get(..n) {
                            read_buffer.extend_from_slice(received);
                        }

                        // Try to parse complete messages
                        while read_buffer.len() >= Header::SIZE {
                            // Skip Magic Cookies (feat_req_someip_586)
                            if is_magic_cookie(&read_buffer) {
                                let Some(length) = parse_someip_length(&read_buffer) else {
                                    break;
                                };
                                let total_size = 8 + length as usize;
                                if read_buffer.len() >= total_size {
                                    read_buffer.advance(total_size);
                                    continue;
                                }
                                break; // Need more data for magic cookie
                            }

                            // Parse length from header (offset 4-8, big-endian u32)
                            let Some(length) = parse_someip_length(&read_buffer) else {
                                break;
                            };

                            let total_size = 8 + length as usize;

                            if read_buffer.len() >= total_size {
                                // Extract complete message
                                let message_data = read_buffer.split_to(total_size);
                                let msg = TcpMessage {
                                    data: message_data.freeze(),
                                    from: peer_addr,
                                    subscription_id,
                                };
                                if msg_tx.send(msg).await.is_err() {
                                    tracing::debug!("SomeIp closed, stopping TCP client connection");
                                    break;
                                }
                            } else {
                                // Need more data
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("TCP read error from {}: {}", peer_addr, e);
                        break;
                    }
                }
            }

            // Write to the stream
            data = send_rx.recv() => {
                let Some(data) = data else {
                    // Channel closed - runtime is shutting down
                    tracing::debug!("Send channel closed, closing TCP connection to {}", peer_addr);
                    break;
                };
                // Prepend Magic Cookie if enabled (feat_req_someip_591)
                if magic_cookies {
                    let cookie = magic_cookie_client();
                    if let Err(e) = stream.write_all(&cookie).await {
                        tracing::error!("TCP write error (magic cookie) to {}: {}", peer_addr, e);
                        break;
                    }
                }
                if let Err(e) = stream.write_all(&data).await {
                    tracing::error!("TCP write error to {}: {}", peer_addr, e);
                    break;
                }
            }
        }
    }

    // Request cleanup via channel - event loop will verify ownership before removing
    let key = (peer_addr, subscription_id);
    let _ = cleanup_tx
        .send(TcpCleanupRequest { key, connection_id })
        .await;
}

/// Read SOME/IP messages from a TCP stream with framing.
///
/// SOME/IP uses the length field in the header for framing:
/// - Header is 16 bytes, includes length field at offset 4-8
/// - Length field = 8 + payload length (includes `client_id` through end)
/// - Total message size = 8 (first part of header) + length field value
///
/// # Errors
///
/// Returns an I/O error if reading fails or connection closes with partial data.
pub async fn read_framed_message<T: TcpStream>(
    stream: &mut T,
    buffer: &mut BytesMut,
) -> io::Result<Option<Bytes>> {
    loop {
        // Check if we have enough data for a complete message
        if buffer.len() >= Header::SIZE {
            // Parse length from header (offset 4-8, big-endian u32)
            let Some(length) = parse_someip_length(buffer) else {
                // Should not happen since we checked len >= Header::SIZE
                tracing::error!(
                    "BUG: parse_someip_length failed despite buffer.len()={} >= Header::SIZE={}",
                    buffer.len(),
                    Header::SIZE
                );
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Buffer too short for header",
                ));
            };

            // Total message size = 8 (service_id, method_id, length) + length
            let total_size = 8 + length as usize;

            if buffer.len() >= total_size {
                // We have a complete message
                let message = buffer.split_to(total_size).freeze();
                return Ok(Some(message));
            }
        }

        // Need more data - read from stream
        let mut read_buf = [0u8; 8192];
        let n = stream.read(&mut read_buf).await?;
        if n == 0 {
            // Connection closed
            return if buffer.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Connection closed with partial message",
                ))
            };
        }
        if let Some(received) = read_buf.get(..n) {
            buffer.extend_from_slice(received);
        }
    }
}

/// Message to send to a specific TCP connection
#[derive(Debug)]
pub struct TcpSendMessage {
    /// Data to send
    pub data: Bytes,
    /// Target peer address
    pub to: SocketAddr,
}

/// Manages a TCP server for an offered service.
///
/// This handles:
/// - Accepting incoming TCP connections
/// - Reading framed SOME/IP messages from clients
/// - Sending responses back to the correct client
/// - Closing connections from specific peers (feat_req_someipsd_872)
pub struct TcpServer<T: TcpStream> {
    /// Local address the server is listening on
    pub local_addr: SocketAddr,
    /// Channel to send responses to clients
    pub send_tx: mpsc::Sender<TcpSendMessage>,
    /// Channel to close specific connections from a peer IP (by port)
    pub close_peer_tx: mpsc::Sender<(std::net::IpAddr, Vec<u16>)>,
    /// Phantom for the stream type
    _phantom: std::marker::PhantomData<T>,
}

impl<T: TcpStream> TcpServer<T> {
    /// Spawn a new TCP server for an offered service.
    ///
    /// Returns the server handle and immediately starts accepting connections.
    /// Messages received from clients are forwarded via `msg_tx`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if retrieving the local address fails.
    pub fn spawn<L: TcpListener<Stream = T>>(
        listener: L,
        service_id: u16,
        instance_id: u16,
        msg_tx: mpsc::Sender<TcpMessage>,
        magic_cookies: bool,
    ) -> io::Result<Self> {
        use tokio::task::JoinHandle;

        let local_addr = listener.local_addr()?;

        // Channel for sending responses to clients
        let (send_tx, mut send_rx) = mpsc::channel::<TcpSendMessage>(100);

        // Channel for closing specific connections from a peer (feat_req_someipsd_872)
        // Tuple: (peer_ip, close_ports) - only connections matching these ports are closed
        let (close_peer_tx, mut close_peer_rx) = mpsc::channel::<(std::net::IpAddr, Vec<u16>)>(16);

        // Track active client connections - maps peer addr to a response sender
        let client_senders: Arc<DashMap<SocketAddr, mpsc::Sender<Bytes>>> =
            Arc::new(DashMap::new());
        let client_senders_for_responses = Arc::clone(&client_senders);
        let client_senders_for_close = Arc::clone(&client_senders);

        // Track connection task handles for abort
        let client_tasks: Arc<DashMap<SocketAddr, JoinHandle<()>>> = Arc::new(DashMap::new());
        let client_tasks_for_close = Arc::clone(&client_tasks);

        // Spawn the main server task
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Accept new connections
                    result = listener.accept() => {
                        match result {
                            Ok((stream, peer_addr)) => {
                                tracing::debug!(
                                    "TCP server: accepted connection from {} for service {:04x}:{:04x}",
                                    peer_addr, service_id, instance_id
                                );

                                // Create per-connection response channel
                                let (conn_send_tx, conn_send_rx) = mpsc::channel::<Bytes>(32);

                                // Register this connection
                                client_senders.insert(peer_addr, conn_send_tx);

                                // Spawn task to handle this client connection
                                let msg_tx = msg_tx.clone();
                                let senders = Arc::clone(&client_senders);
                                let tasks = Arc::clone(&client_tasks);
                                let handle = tokio::spawn(async move {
                                    handle_tcp_connection(
                                        stream,
                                        peer_addr,
                                        service_id,
                                        instance_id,
                                        msg_tx,
                                        conn_send_rx,
                                        senders,
                                        magic_cookies,
                                    ).await;
                                    // Clean up task handle when done
                                    tasks.remove(&peer_addr);
                                });
                                client_tasks.insert(peer_addr, handle);
                            }
                            Err(e) => {
                                tracing::error!(
                                    "TCP accept error for service {:04x}:{:04x}: {}",
                                    service_id, instance_id, e
                                );
                            }
                        }
                    }

                    // Close specific connections from a peer (reboot detection)
                    // close_ports contains ports from old subscriptions that should be closed
                    Some((peer_ip, close_ports)) = close_peer_rx.recv() => {
                        tracing::debug!(
                            "TCP server {:04x}:{:04x}: received close_peer request for peer {} ports {:?}",
                            service_id, instance_id, peer_ip, close_ports
                        );

                        // Log all current connections for debugging
                        let current_connections: Vec<SocketAddr> = client_senders_for_close.iter()
                            .map(|entry| *entry.key())
                            .collect();
                        tracing::debug!(
                            "TCP server {:04x}:{:04x}: current connections: {:?}",
                            service_id, instance_id, current_connections
                        );

                        let mut addrs_to_remove: Vec<SocketAddr> = Vec::new();
                        for entry in client_senders_for_close.iter() {
                            // Only close connections matching BOTH the peer IP AND a port in close_ports
                            if entry.key().ip() == peer_ip && close_ports.contains(&entry.key().port()) {
                                tracing::debug!(
                                    "TCP server {:04x}:{:04x}: MATCH - will close connection from {:?}",
                                    service_id, instance_id, entry.key()
                                );
                                addrs_to_remove.push(*entry.key());
                            }
                        }
                        if addrs_to_remove.is_empty() {
                            tracing::warn!(
                                "TCP server {:04x}:{:04x}: No matching connections found for peer {} ports {:?}",
                                service_id, instance_id, peer_ip, close_ports
                            );
                        } else {
                            tracing::debug!(
                                "TCP server {:04x}:{:04x}: closing {} connection(s) from peer {} (reboot detected, ports: {:?})",
                                service_id, instance_id, addrs_to_remove.len(), peer_ip, close_ports
                            );
                            for addr in addrs_to_remove {
                                // Remove sender (causes connection task to exit)
                                client_senders_for_close.remove(&addr);
                                // Abort the task to ensure it stops immediately
                                if let Some((_, handle)) = client_tasks_for_close.remove(&addr) {
                                    handle.abort();
                                }
                            }
                        }
                    }

                    // Route responses to the appropriate client connection - exit when channel closes
                    msg = send_rx.recv() => {
                        if let Some(send_msg) = msg {
                            if let Some(sender) = client_senders_for_responses.get(&send_msg.to) {
                                if sender.send(send_msg.data).await.is_err() {
                                    tracing::warn!("Failed to send response to {}: connection closed", send_msg.to);
                                }
                            } else {
                                tracing::warn!("No TCP connection for peer {}", send_msg.to);
                            }
                        } else {
                            // Sender was dropped (service stopped offering) - exit
                            tracing::debug!(
                                "TCP server for service {:04x}:{:04x} shutting down - sender dropped",
                                service_id, instance_id
                            );
                            break;
                        }
                    }
                }
            }
        });

        Ok(Self {
            local_addr,
            send_tx,
            close_peer_tx,
            _phantom: std::marker::PhantomData,
        })
    }
}

/// Handle a single TCP client connection (server-side).
///
/// Reads messages and forwards them to the runtime, while also
/// handling outgoing responses.
///
/// When `magic_cookies` is enabled:
/// - Each write is prepended with a Magic Cookie (`feat_req_someip_591`)
/// - Magic Cookies in received data are skipped (`feat_req_someip_586`)
async fn handle_tcp_connection<T: TcpStream>(
    mut stream: T,
    peer_addr: SocketAddr,
    service_id: u16,
    instance_id: u16,
    msg_tx: mpsc::Sender<TcpMessage>,
    mut response_rx: mpsc::Receiver<Bytes>,
    client_senders: Arc<DashMap<SocketAddr, mpsc::Sender<Bytes>>>,
    magic_cookies: bool,
) {
    let mut read_buffer = BytesMut::new();
    let mut read_buf = [0u8; 8192];

    loop {
        tokio::select! {
            // Read from client
            result = stream.read(&mut read_buf) => {
                match result {
                    Ok(0) => {
                        // Connection closed
                        tracing::debug!("TCP client {} disconnected from service {:04x}:{:04x}",
                            peer_addr, service_id, instance_id);
                        break;
                    }
                    Ok(n) => {
                        if let Some(received) = read_buf.get(..n) {
                            read_buffer.extend_from_slice(received);
                        }

                        // Try to parse complete messages
                        while read_buffer.len() >= Header::SIZE {
                            // Skip Magic Cookies (feat_req_someip_586)
                            if is_magic_cookie(&read_buffer) {
                                let Some(length) = parse_someip_length(&read_buffer) else {
                                    break;
                                };
                                let total_size = 8 + length as usize;
                                if read_buffer.len() >= total_size {
                                    read_buffer.advance(total_size);
                                    continue;
                                }
                                break; // Need more data for magic cookie
                            }

                            // Parse length from header (offset 4-8, big-endian u32)
                            let Some(length) = parse_someip_length(&read_buffer) else {
                                break;
                            };

                            let total_size = 8 + length as usize;

                            if read_buffer.len() >= total_size {
                                // Extract complete message
                                let message_data = read_buffer.split_to(total_size);
                                let msg = TcpMessage {
                                    data: message_data.freeze(),
                                    from: peer_addr,
                                    subscription_id: 0, // Server-side, no subscription
                                };
                                if msg_tx.send(msg).await.is_err() {
                                    tracing::debug!("SomeIp closed, stopping TCP connection handler");
                                    break;
                                }
                            } else {
                                // Need more data
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("TCP read error from {} for service {:04x}:{:04x}: {}",
                            peer_addr, service_id, instance_id, e);
                        break;
                    }
                }
            }

            // Write responses to client
            data = response_rx.recv() => {
                let Some(data) = data else {
                    // Channel closed - runtime is shutting down
                    tracing::debug!("Response channel closed, closing TCP connection to {}", peer_addr);
                    break;
                };
                // Prepend Magic Cookie if enabled (feat_req_someip_591)
                if magic_cookies {
                    let cookie = magic_cookie_server();
                    if let Err(e) = stream.write_all(&cookie).await {
                        tracing::error!("TCP write error (magic cookie) to {} for service {:04x}:{:04x}: {}",
                            peer_addr, service_id, instance_id, e);
                        break;
                    }
                }
                if let Err(e) = stream.write_all(&data).await {
                    tracing::error!("TCP write error to {} for service {:04x}:{:04x}: {}",
                        peer_addr, service_id, instance_id, e);
                    break;
                }
            }
        }
    }

    // Clean up: remove from senders map
    client_senders.remove(&peer_addr);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BufMut;

    #[test_log::test]
    fn test_frame_parsing_logic() {
        // Simulate a SOME/IP message: header (16 bytes) + 4 bytes payload
        let mut msg = BytesMut::new();
        // Service ID: 0x1234
        msg.put_u16(0x1234);
        // Method ID: 0x0001
        msg.put_u16(0x0001);
        // Length: 8 (rest of header) + 4 (payload) = 12
        msg.put_u32(12);
        // Client ID, Session ID
        msg.put_u16(0x0001);
        msg.put_u16(0x0001);
        // Protocol version, interface version, message type, return code
        msg.put_u8(0x01);
        msg.put_u8(0x01);
        msg.put_u8(0x00);
        msg.put_u8(0x00);
        // Payload
        msg.put_slice(b"test");

        // The total size should be 8 + 12 = 20 bytes
        assert_eq!(msg.len(), 20);

        // Check length field parsing
        let length = u32::from_be_bytes([msg[4], msg[5], msg[6], msg[7]]) as usize;
        assert_eq!(length, 12);
        assert_eq!(8 + length, 20);
    }
}
