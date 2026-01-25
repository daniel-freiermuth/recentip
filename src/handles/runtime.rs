//! # SOME/IP `SomeIp`
//!
//! The runtime is the **central coordinator** for all SOME/IP communication.
//! It manages network I/O, service discovery, and dispatches commands from handles.
//!
//! ## Role in the Architecture
//!
//! The runtime implements an **event loop pattern** where:
//!
//! 1. All state lives in a single `RuntimeState` struct
//! 2. Handles send `Command` messages via channels
//! 3. The event loop processes commands and I/O events in a `tokio::select!` loop
//! 4. Responses flow back through oneshot channels or notification channels
//!
//! This design ensures **thread-safety without locks** (single-threaded event loop)
//! and enables **efficient socket multiplexing** (one SD socket shared by all services).
//!
//! ## Lifetime and Shutdown
//!
//! The runtime task runs until:
//! - All handles are dropped (command channel closes)
//! - An unrecoverable I/O error occurs
//!
//! When the [`SomeIp`] struct is dropped, it signals shutdown and waits for
//! the background task to complete gracefully.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::config::{MethodConfig, RuntimeConfig, Transport};
use crate::error::{Error, Result};
use crate::handles::{FindBuilder, ServiceOffering};
use crate::net::{TcpListener, TcpStream, UdpSocket};
use crate::runtime::{
    event_loop::runtime_task,
    state::{RpcMessage, RpcSendMessage, RuntimeState},
    Command,
};
use crate::tcp::{TcpConnectionPool, TcpMessage};
use crate::{InstanceId, SdEvent, ServiceId};

// ============================================================================
// RUNTIME INNER
// ============================================================================

/// Shared state between the `SomeIp` handle and the runtime task.
///
/// This is an implementation detail. Users interact with [`SomeIp`] directly.
#[derive(Debug)]
pub(crate) struct RuntimeInner {
    /// Channel to send commands to the runtime task.
    ///
    /// All handle operations (find, offer, call, etc.) send commands through
    /// this channel. The runtime's event loop receives and processes them.
    pub(crate) cmd_tx: mpsc::Sender<Command>,
    /// `SomeIp` configuration (for static proxy creation)
    pub(crate) config: crate::config::RuntimeConfig,
}

// ============================================================================
// RUNTIME
// ============================================================================

/// SOME/IP `SomeIp` — the central coordinator for all SOME/IP communication.
///
/// Create one `SomeIp` per application. It manages:
///
/// - **Service Discovery**: Multicast offers, finds, and subscription exchanges
/// - **RPC Communication**: Request/response and fire-and-forget calls
/// - **Event Delivery**: Pub/sub notifications via eventgroups
/// - **Connection Management**: UDP sockets and TCP connection pooling
///
/// # Lifecycle
///
/// The runtime spawns a background task that runs until:
/// 1. All handles (`OfferedService`, `ServiceOffering`) are dropped
/// 2. An unrecoverable error occurs
///
/// When the `SomeIp` is dropped, it signals shutdown and waits for cleanup.
///
/// # Graceful Shutdown
///
/// For production code, prefer calling [`shutdown()`](Self::shutdown) explicitly
/// to ensure all pending RPC responses are sent:
///
/// ```no_run
/// # use recentip::prelude::*;
/// # async fn example(runtime: SomeIp) {
/// // Process requests...
/// runtime.shutdown().await;  // Ensures all pending responses are sent
/// # }
/// ```
///
/// # Socket Types
///
/// The runtime is generic over socket types to support testing with network
/// simulators like [turmoil](https://docs.rs/turmoil). For production use,
/// the default tokio types are used:
///
/// ```no_run
/// use recentip::prelude::*;
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     // Production (default)
///     let runtime = recentip::configure().start().await?;
///
///     // For turmoil testing, the runtime is parameterized:
///     // type TestRuntime = SomeIp<turmoil::net::UdpSocket, ...>;
///     // See tests/compliance/ for examples.
///     Ok(())
/// }
/// ```
///
/// # Thread Safety
///
/// The `SomeIp` can be shared across tasks via its handles. Internally,
/// all state is managed by a single-threaded event loop, avoiding locks.
pub struct SomeIp<
    U: UdpSocket = tokio::net::UdpSocket,
    T: TcpStream = tokio::net::TcpStream,
    L: TcpListener<Stream = T> = tokio::net::TcpListener,
> {
    inner: Arc<RuntimeInner>,
    /// Handle to the runtime task, used for graceful shutdown
    runtime_task: Option<tokio::task::JoinHandle<()>>,
    _phantom: std::marker::PhantomData<(U, T, L)>,
}

impl<U: UdpSocket, T: TcpStream, L: TcpListener<Stream = T>> SomeIp<U, T, L> {
    /// Create a new runtime with a specific socket type.
    ///
    /// This is mainly useful for testing with turmoil.
    pub(crate) async fn new(config: RuntimeConfig) -> Result<Self> {
        // Bind the SD socket (always UDP, per SOME/IP spec)
        let sd_socket = U::bind(config.bind_addr).await?;
        let local_addr = sd_socket.local_addr()?;

        // Join multicast group
        if let SocketAddr::V4(addr) = config.sd_multicast {
            sd_socket.join_multicast_v4(*addr.ip(), Ipv4Addr::UNSPECIFIED)?;
        }

        // Create command channel
        let (cmd_tx, cmd_rx) = mpsc::channel(64);

        // Create RPC message channel (for messages from UDP RPC socket tasks to runtime)
        let (method_tx, method_rx) = mpsc::channel::<RpcMessage>(100);

        // Create TCP RPC message channel (for messages from TCP server connections to runtime)
        let (tcp_method_tx, tcp_method_rx) = mpsc::channel::<TcpMessage>(100);

        // Create TCP client message channel (for responses received on client TCP connections)
        let (tcp_client_tx, tcp_client_rx) = mpsc::channel::<TcpMessage>(100);

        // Create TCP connection pool for client-side TCP connections
        let tcp_pool: TcpConnectionPool<T> =
            TcpConnectionPool::new(tcp_client_tx, config.magic_cookies);

        // Create dedicated client RPC socket (ephemeral port)
        // Per feat_req_recentip_676: Port 30490 is only for SD, not for RPC
        // Clients need their own socket for sending RPC requests, separate from SD
        let client_method_socket = U::bind(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            0, // ephemeral port
        )))
        .await?;
        let client_method_addr = client_method_socket.local_addr()?;

        // Spawn task to handle client RPC socket (receives responses to our requests)
        let (client_method_send_tx, mut client_method_send_rx) =
            mpsc::channel::<RpcSendMessage>(100);
        let client_method_tx_clone = method_tx.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 65535];
            loop {
                tokio::select! {
                    // Receive incoming method responses
                    result = client_method_socket.recv_from(&mut buf) => {
                        match result {
                            Ok((len, from)) => {
                                let data = buf[..len].to_vec();
                                // Forward to runtime task for processing
                                let _ = client_method_tx_clone.send(RpcMessage {
                                    service_key: None,
                                    data,
                                    from,
                                }).await;
                            }
                            Err(e) => {
                                tracing::error!("Error receiving on client method socket: {}", e);
                            }
                        }
                    }

                    // Send outgoing method requests
                    Some(send_msg) = client_method_send_rx.recv() => {
                        if let Err(e) = client_method_socket.send_to(&send_msg.data, send_msg.to).await {
                            tracing::error!("Error sending on client RPC socket: {}", e);
                        }
                    }
                }
            }
        });

        // Spawn the runtime task
        let inner = Arc::new(RuntimeInner {
            cmd_tx,
            config: config.clone(),
        });

        let state = RuntimeState::new(
            local_addr,
            client_method_addr,
            client_method_send_tx,
            config.clone(),
        );

        let runtime_task = tokio::spawn(async move {
            runtime_task::<U, T, L>(
                sd_socket,
                config,
                cmd_rx,
                method_rx,
                method_tx,
                tcp_method_rx,
                tcp_method_tx,
                tcp_client_rx,
                state,
                tcp_pool,
            )
            .await;
        });

        Ok(Self {
            inner,
            runtime_task: Some(runtime_task),
            _phantom: std::marker::PhantomData,
        })
    }

    /// Create a new runtime with a specific socket type.
    ///
    /// This is for backward compatibility and testing with turmoil.
    /// New code should use [`configure()`](crate::configure) instead.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    ///
    /// # async fn example() -> Result<()> {
    /// let config = RuntimeConfig::builder()
    ///     .advertised_ip("192.168.1.100".parse().unwrap())
    ///     .build();
    /// let runtime = recentip::configure().start().await?;
    /// # Ok(())
    /// # }
    /// ```
    #[deprecated(since = "0.1.0", note = "Use `configure()` instead")]
    pub async fn with_socket_type(config: RuntimeConfig) -> Result<Self> {
        Self::new(config).await
    }

    /// Find a remote SOME/IP service.
    ///
    /// Returns a [`FindBuilder`] to configure the find criteria, then `.await`
    /// to discover the first matching service.
    ///
    /// # Arguments
    ///
    /// - `service_id`: The service ID to find (required)
    ///
    /// # Find Criteria (via builder)
    ///
    /// - **Instance ID**: Default `Any`, configurable via `.instance()`
    /// - **Major Version**: Default `Any`, configurable via `.major_version()`
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    ///
    /// const MY_SERVICE_ID: u16 = 0x1234;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let runtime = recentip::configure().start().await?;
    ///
    ///     // Simple: find any instance
    ///     let proxy = runtime.find(MY_SERVICE_ID).await?;
    ///
    ///     // With criteria: specific instance and version
    ///     let proxy = runtime.find(MY_SERVICE_ID)
    ///         .instance(InstanceId::Id(1))
    ///         .major_version(1)
    ///         .await?;
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn find(&self, service_id: u16) -> FindBuilder<'_, U, T, L> {
        let service_id = ServiceId::new(service_id).expect("Invalid service ID");
        FindBuilder::new(self, service_id)
    }

    /// Offer a service with configurable transport endpoints.
    ///
    /// Returns an `OfferBuilder` that allows configuring which transports (TCP/UDP)
    /// the service should be offered on, and with which ports.
    ///
    /// # Arguments
    ///
    /// - `service_id`: The service ID to offer (required)
    /// - `instance`: The instance ID (required)
    ///
    /// # Example
    /// ```no_run
    /// use recentip::prelude::*;
    ///
    /// const MY_SERVICE_ID: u16 = 0x1234;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let runtime = recentip::configure().start().await?;
    ///
    ///     // Offer on both TCP and UDP with custom ports
    ///     let offering = runtime.offer(MY_SERVICE_ID, InstanceId::Id(1))
    ///         .version(1, 0)  // major, minor
    ///         .tcp_port(30501)
    ///         .udp_port(30502)
    ///         .start()
    ///         .await?;
    ///
    ///     // Or use defaults from runtime config
    ///     let offering2 = runtime.offer(MY_SERVICE_ID, InstanceId::Id(2))
    ///         .version(1, 0)
    ///         .udp()  // Use default UDP port
    ///         .start()
    ///         .await?;
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn offer(&self, service_id: u16, instance: InstanceId) -> OfferBuilder<'_, U, T, L> {
        let service_id = ServiceId::new(service_id).expect("Invalid service ID");
        OfferBuilder::new(self, service_id, instance)
    }

    /// Gracefully shutdown the runtime.
    ///
    /// This sends a shutdown command to the runtime task and waits for it to
    /// complete all pending operations (including sending any pending RPC responses).
    ///
    /// Use this when you need to ensure all in-flight responses are sent before
    /// the runtime exits. If you just drop the runtime, pending responses may
    /// not be sent.
    ///
    /// # Example
    /// ```no_run
    /// use recentip::prelude::*;
    /// use recentip::handle::ServiceEvent;
    ///
    /// const MY_SERVICE_ID: u16 = 0x1234;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let runtime = recentip::configure().start().await?;
    ///     let mut offering = runtime.offer(MY_SERVICE_ID, InstanceId::Id(1))
    ///         .version(1, 0)
    ///         .start()
    ///         .await?;
    ///
    ///     // Process one request then shutdown
    ///     if let Some(ServiceEvent::Call { responder, .. }) = offering.next().await {
    ///         responder.reply(b"response")?;
    ///     }
    ///
    ///     // Ensure the response is actually sent before exiting
    ///     runtime.shutdown().await;
    ///     Ok(())
    /// }
    /// ```
    pub async fn shutdown(mut self) {
        // Send shutdown command to the runtime task
        let _ = self.inner.cmd_tx.send(Command::Shutdown).await;

        // Wait for the runtime task to complete
        if let Some(handle) = self.runtime_task.take() {
            let _ = handle.await;
        }
    }

    /// Monitor all Service Discovery (SD) events.
    ///
    /// Returns a channel that receives [`SdEvent`] notifications for all SD-related
    /// activity: service announcements, service unavailability, and service expiration.
    ///
    /// This allows applications to monitor the dynamic service landscape without
    /// knowing specific service IDs in advance.
    ///
    /// # Returns
    ///
    /// A `Result` containing a receiver channel for [`SdEvent`]s, or an error if
    /// the command could not be sent.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use recentip::{SomeIp, SdEvent};
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let runtime = recentip::configure().start().await?;
    /// let mut sd_events = runtime.monitor_sd().await?;
    ///
    /// while let Some(event) = sd_events.recv().await {
    ///     match event {
    ///         SdEvent::ServiceAvailable { service_id, instance_id, .. } => {
    ///             println!("Service available: {}:{}", service_id, instance_id);
    ///         }
    ///         SdEvent::ServiceUnavailable { service_id, instance_id } => {
    ///             println!("Service unavailable: {}:{}", service_id, instance_id);
    ///         }
    ///         SdEvent::ServiceExpired { service_id, instance_id } => {
    ///             println!("Service expired: {}:{}", service_id, instance_id);
    ///         }
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn monitor_sd(&self) -> Result<mpsc::Receiver<SdEvent>> {
        let (tx, rx) = mpsc::channel(100);
        self.inner
            .cmd_tx
            .send(Command::MonitorSd { events: tx })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;
        Ok(rx)
    }

    /// Get a reference to the runtime inner for use by builders.
    #[doc(hidden)]
    pub(crate) fn inner(&self) -> &Arc<RuntimeInner> {
        &self.inner
    }
}

// ============================================================================
// OFFER BUILDER
// ============================================================================

/// Builder for configuring and starting a service offering.
///
/// Created by [`SomeIp::offer`]. Configure transport endpoints using the builder
/// methods, then call [`start`](OfferBuilder::start) to begin offering the service.
///
/// # Example
/// ```no_run
/// use recentip::prelude::*;
///
/// const MY_SERVICE_ID: u16 = 0x1234;
///
/// # async fn example(runtime: SomeIp) -> Result<()> {
/// // Offer on both TCP and UDP
/// let offering = runtime.offer(MY_SERVICE_ID, InstanceId::Id(1))
///     .version(1, 0)
///     .tcp_port(30501)
///     .udp_port(30502)
///     .start()
///     .await?;
///
/// // Offer on TCP only with default port
/// let tcp_offering = runtime.offer(MY_SERVICE_ID, InstanceId::Id(2))
///     .version(1, 0)
///     .tcp()
///     .start()
///     .await?;
/// # Ok(())
/// # }
/// ```
#[must_use]
pub struct OfferBuilder<'a, U: UdpSocket, T: TcpStream, L: TcpListener<Stream = T>> {
    runtime: &'a SomeIp<U, T, L>,
    service_id: ServiceId,
    instance: InstanceId,
    major_version: u8,
    minor_version: u32,
    config: crate::config::OfferConfig,
}

impl<'a, U: UdpSocket, T: TcpStream, L: TcpListener<Stream = T>> OfferBuilder<'a, U, T, L> {
    pub(crate) fn new(
        runtime: &'a SomeIp<U, T, L>,
        service_id: ServiceId,
        instance: InstanceId,
    ) -> Self {
        Self {
            runtime,
            service_id,
            instance,
            major_version: 0,
            minor_version: 0,
            config: crate::config::OfferConfig::new(),
        }
    }

    /// Set the major and minor version for the service offering.
    ///
    /// This is required for proper SD announcements.
    pub fn version(mut self, major: u8, minor: u32) -> Self {
        self.major_version = major;
        self.minor_version = minor;
        self
    }

    /// Enable TCP transport with the default RPC port (30491).
    pub fn tcp(mut self) -> Self {
        self.config = self.config.tcp();
        self
    }

    /// Enable TCP transport with a specific port.
    pub fn tcp_port(mut self, port: u16) -> Self {
        self.config = self.config.tcp_port(port);
        self
    }

    /// Enable UDP transport with the default RPC port (30491).
    pub fn udp(mut self) -> Self {
        self.config = self.config.udp();
        self
    }

    /// Enable UDP transport with a specific port.
    pub fn udp_port(mut self, port: u16) -> Self {
        self.config = self.config.udp_port(port);
        self
    }

    /// Configure method-specific behavior (e.g., exception handling).
    pub fn method_config(mut self, config: MethodConfig) -> Self {
        self.config = self.config.method_config(config);
        self
    }

    /// Start offering the service with the configured transports.
    ///
    /// If no transports are configured, falls back to the runtime's default
    /// transport configuration for backward compatibility.
    pub async fn start(mut self) -> Result<ServiceOffering> {
        // Fallback to runtime config if no transport specified
        if !self.config.has_transport() {
            match self.runtime.inner.config.preferred_transport {
                Transport::Tcp => self.config = self.config.tcp(),
                Transport::Udp => self.config = self.config.udp(),
            }
        }

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        self.runtime
            .inner
            .cmd_tx
            .send(Command::Offer {
                service_id: self.service_id,
                instance_id: self.instance,
                major_version: self.major_version,
                minor_version: self.minor_version,
                offer_config: self.config,
                response: response_tx,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        let requests_rx = response_rx.await.map_err(|_| Error::RuntimeShutdown)??;

        Ok(ServiceOffering::new(
            Arc::clone(&self.runtime.inner),
            self.service_id,
            self.instance,
            self.major_version,
            requests_rx,
        ))
    }
}
