//! # SOME/IP Runtime
//!
//! The runtime is the **central coordinator** for all SOME/IP communication.
//! It manages network I/O, service discovery, and dispatches commands from handles.
//!
//! ## Role in the Architecture
//!
//! The runtime implements an **event loop pattern** where:
//!
//! 1. All state lives in a single [`RuntimeState`](crate::state::RuntimeState) struct
//! 2. Handles send [`Command`](crate::command::Command) messages via channels
//! 3. The event loop processes commands and I/O events in a `tokio::select!` loop
//! 4. Responses flow back through oneshot channels or notification channels
//!
//! This design ensures **thread-safety without locks** (single-threaded event loop)
//! and enables **efficient socket multiplexing** (one SD socket shared by all services).
//!
//! ## Event Loop Sources
//!
//! The runtime's `select!` loop handles:
//!
//! | Source | Description |
//! |--------|-------------|
//! | Command channel | Commands from handles (find, offer, call, etc.) |
//! | SD socket (UDP) | Service Discovery multicast messages |
//! | Client RPC socket | Responses to outgoing RPC calls |
//! | TCP client responses | Responses on TCP client connections |
//! | TCP server requests | Incoming requests on TCP server sockets |
//! | Periodic timer | Cyclic offers, TTL expiry, find repetitions |
//!
//! ## Module Organization
//!
//! The runtime implementation is split across several internal modules:
//!
//! - [`config`](crate::config): Configuration types ([`RuntimeConfig`], [`Transport`])
//! - [`command`](crate::command): Command enum for handle→runtime communication
//! - [`state`](crate::state): `RuntimeState` and internal data structures
//! - [`sd`](crate::sd): Service Discovery message handlers and builders
//! - [`client`](crate::client): Client-side handlers (find, call, subscribe)
//! - [`server`](crate::server): Server-side handlers (offer, notify, respond)
//!
//! Each module contains pure functions that operate on `RuntimeState` and return
//! [`Action`](crate::sd::Action) values. The runtime executes these actions after
//! each event is processed.
//!
//! ## Lifetime and Shutdown
//!
//! The runtime task runs until:
//! - All handles are dropped (command channel closes)
//! - An unrecoverable I/O error occurs
//!
//! When the [`Runtime`] struct is dropped, it signals shutdown and waits for
//! the background task to complete gracefully.
//!
//! ## Example: Custom Socket Types (Testing)
//!
//! The runtime is generic over socket types for testing with network simulators:
//!
//! ```no_run
//! use someip_runtime::prelude::*;
//!
//! // Production runtime (default socket types)
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let config = RuntimeConfig::default();
//!     let runtime = Runtime::new(config).await?;
//!     // Use runtime...
//!     Ok(())
//! }
//!
//! // For turmoil testing, see tests/compliance/ for examples
//! // using Runtime::<turmoil types>::with_socket_type()
//! ```

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::time::interval;

use crate::SdEvent;

use crate::client;
use crate::error::{Error, Result};
use crate::handle::{Available, OfferingHandle, ProxyHandle, Unavailable};
use crate::net::{TcpListener, TcpStream, UdpSocket};
use crate::sd::{
    build_find_message, build_offer_message, handle_find_request, handle_offer,
    handle_stop_offer as handle_sd_stop_offer, handle_subscribe_ack, handle_subscribe_nack,
    handle_subscribe_request, handle_unsubscribe_request, Action,
};
use crate::server::{self, build_response};
use crate::state::{PendingServerResponse, RpcMessage, RpcSendMessage, RuntimeState, ServiceKey};
use crate::tcp::{TcpConnectionPool, TcpMessage};
use crate::wire::{
    validate_protocol_version, Header, MessageType, SdEntry, SdEntryType, SdMessage, SD_METHOD_ID,
    SD_SERVICE_ID,
};
use crate::{InstanceId, Service, ServiceId};

// Re-export configuration types for backward compatibility
pub use crate::config::{
    MethodConfig, RuntimeConfig, RuntimeConfigBuilder, Transport, DEFAULT_CYCLIC_OFFER_DELAY,
    DEFAULT_FIND_REPETITIONS, DEFAULT_SD_MULTICAST, DEFAULT_SD_PORT, DEFAULT_TTL,
};

// Re-export command types for handle.rs
pub(crate) use crate::command::{Command, ServiceAvailability, ServiceRequest};

// ============================================================================
// RUNTIME
// ============================================================================

/// Shared state between the Runtime handle and the runtime task.
///
/// This is an implementation detail. Users interact with [`Runtime`] directly.
pub(crate) struct RuntimeInner {
    /// Channel to send commands to the runtime task.
    ///
    /// All handle operations (find, offer, call, etc.) send commands through
    /// this channel. The runtime's event loop receives and processes them.
    pub(crate) cmd_tx: mpsc::Sender<Command>,
}

/// SOME/IP Runtime — the central coordinator for all SOME/IP communication.
///
/// Create one `Runtime` per application. It manages:
///
/// - **Service Discovery**: Multicast offers, finds, and subscription exchanges
/// - **RPC Communication**: Request/response and fire-and-forget calls
/// - **Event Delivery**: Pub/sub notifications via eventgroups
/// - **Connection Management**: UDP sockets and TCP connection pooling
///
/// # Lifecycle
///
/// The runtime spawns a background task that runs until:
/// 1. All handles ([`ProxyHandle`](crate::handle::ProxyHandle),
///    [`OfferingHandle`](crate::handle::OfferingHandle)) are dropped
/// 2. An unrecoverable error occurs
///
/// When the `Runtime` is dropped, it signals shutdown and waits for cleanup.
///
/// # Graceful Shutdown
///
/// For production code, prefer calling [`shutdown()`](Self::shutdown) explicitly
/// to ensure all pending RPC responses are sent:
///
/// ```no_run
/// # use someip_runtime::prelude::*;
/// # async fn example(runtime: Runtime) {
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
/// use someip_runtime::prelude::*;
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     // Production (default)
///     let runtime = Runtime::new(RuntimeConfig::default()).await?;
///
///     // For turmoil testing, the runtime is parameterized:
///     // type TestRuntime = Runtime<turmoil::net::UdpSocket, ...>;
///     // See tests/compliance/ for examples.
///     Ok(())
/// }
/// ```
///
/// # Thread Safety
///
/// The `Runtime` can be shared across tasks via its handles. Internally,
/// all state is managed by a single-threaded event loop, avoiding locks.
pub struct Runtime<
    U: UdpSocket = tokio::net::UdpSocket,
    T: TcpStream = tokio::net::TcpStream,
    L: TcpListener<Stream = T> = tokio::net::TcpListener,
> {
    inner: Arc<RuntimeInner>,
    /// Handle to the runtime task, used for graceful shutdown
    runtime_task: Option<tokio::task::JoinHandle<()>>,
    _phantom: std::marker::PhantomData<(U, T, L)>,
}

impl Runtime<tokio::net::UdpSocket, tokio::net::TcpStream, tokio::net::TcpListener> {
    /// Create a new runtime with the given configuration.
    ///
    /// This binds to the configured local address and joins the SD multicast group.
    pub async fn new(config: RuntimeConfig) -> Result<Self> {
        Self::with_socket_type(config).await
    }
}

impl<U: UdpSocket, T: TcpStream, L: TcpListener<Stream = T>> Runtime<U, T, L> {
    /// Create a new runtime with a specific socket type.
    ///
    /// This is mainly useful for testing with turmoil.
    pub async fn with_socket_type(config: RuntimeConfig) -> Result<Self> {
        // Bind the SD socket (always UDP, per SOME/IP spec)
        let sd_socket = U::bind(config.local_addr).await?;
        let local_addr = sd_socket.local_addr()?;

        // Join multicast group
        if let SocketAddr::V4(addr) = config.sd_multicast {
            sd_socket.join_multicast_v4(*addr.ip(), Ipv4Addr::UNSPECIFIED)?;
        }

        // Create command channel
        let (cmd_tx, cmd_rx) = mpsc::channel(64);

        // Create RPC message channel (for messages from UDP RPC socket tasks to runtime)
        let (rpc_tx, rpc_rx) = mpsc::channel::<RpcMessage>(100);

        // Create TCP RPC message channel (for messages from TCP server connections to runtime)
        let (tcp_rpc_tx, tcp_rpc_rx) = mpsc::channel::<TcpMessage>(100);

        // Create TCP client message channel (for responses received on client TCP connections)
        let (tcp_client_tx, tcp_client_rx) = mpsc::channel::<TcpMessage>(100);

        // Create TCP connection pool for client-side TCP connections
        let tcp_pool: TcpConnectionPool<T> =
            TcpConnectionPool::new(tcp_client_tx, config.magic_cookies);

        // Create dedicated client RPC socket (ephemeral port)
        // Per feat_req_recentip_676: Port 30490 is only for SD, not for RPC
        // Clients need their own socket for sending RPC requests, separate from SD
        let client_rpc_socket = U::bind(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            0, // ephemeral port
        )))
        .await?;
        let client_rpc_addr = client_rpc_socket.local_addr()?;

        // Spawn task to handle client RPC socket (receives responses to our requests)
        let (client_rpc_send_tx, mut client_rpc_send_rx) = mpsc::channel::<RpcSendMessage>(100);
        let client_rpc_tx_clone = rpc_tx.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 65535];
            loop {
                tokio::select! {
                    // Receive incoming RPC responses
                    result = client_rpc_socket.recv_from(&mut buf) => {
                        match result {
                            Ok((len, from)) => {
                                let data = buf[..len].to_vec();
                                // Forward to runtime task for processing
                                let _ = client_rpc_tx_clone.send(RpcMessage {
                                    service_key: None,
                                    data,
                                    from,
                                }).await;
                            }
                            Err(e) => {
                                tracing::error!("Error receiving on client RPC socket: {}", e);
                            }
                        }
                    }

                    // Send outgoing RPC requests
                    Some(send_msg) = client_rpc_send_rx.recv() => {
                        if let Err(e) = client_rpc_socket.send_to(&send_msg.data, send_msg.to).await {
                            tracing::error!("Error sending on client RPC socket: {}", e);
                        }
                    }
                }
            }
        });

        // Spawn the runtime task
        let inner = Arc::new(RuntimeInner { cmd_tx });

        let state = RuntimeState::new(
            local_addr,
            client_rpc_addr,
            client_rpc_send_tx,
            config.clone(),
        );

        let runtime_task = tokio::spawn(async move {
            runtime_task::<U, T, L>(
                sd_socket,
                config,
                cmd_rx,
                rpc_rx,
                rpc_tx,
                tcp_rpc_rx,
                tcp_rpc_tx,
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

    /// Find a service by ID and instance.
    ///
    /// Returns a proxy handle that will become available when the service is discovered.
    pub fn find<S: Service>(&self, instance: InstanceId) -> ProxyHandle<S, Unavailable> {
        let service_id = ServiceId::new(S::SERVICE_ID).expect("Invalid service ID");
        ProxyHandle::new(Arc::clone(&self.inner), service_id, instance)
    }

    /// Offer a service.
    ///
    /// Returns an offering handle to receive requests and send events.
    /// By default, all methods use RESPONSE (0x80) for errors.
    /// Use `offer_with_config` to configure specific methods to use EXCEPTION (0x81).
    pub async fn offer<S: Service>(&self, instance: InstanceId) -> Result<OfferingHandle<S>> {
        self.offer_with_config::<S>(instance, MethodConfig::default())
            .await
    }

    /// Offer a service with custom method configuration.
    ///
    /// Use this to configure which methods use EXCEPTION (0x81) message type for errors
    /// instead of the default RESPONSE (0x80) with error return code.
    ///
    /// Per SOME/IP specification (`feat_req_recentip_106`, `feat_req_recentip_726)`:
    /// - By default, errors use RESPONSE (0x80) with non-OK return code
    /// - EXCEPTION (0x81) is optional and must be explicitly configured per-method
    ///
    /// # Example
    /// ```no_run
    /// use someip_runtime::prelude::*;
    ///
    /// struct MyService;
    /// impl Service for MyService {
    ///     const SERVICE_ID: u16 = 0x1234;
    ///     const MAJOR_VERSION: u8 = 1;
    ///     const MINOR_VERSION: u32 = 0;
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let runtime = Runtime::new(RuntimeConfig::default()).await?;
    ///
    ///     let config = MethodConfig::new()
    ///         .use_exception_for(0x0001)  // Method 0x0001 uses EXCEPTION for errors
    ///         .use_exception_for(0x0002); // Method 0x0002 also uses EXCEPTION
    ///
    ///     let offering = runtime.offer_with_config::<MyService>(
    ///         InstanceId::Id(1),
    ///         config,
    ///     ).await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn offer_with_config<S: Service>(
        &self,
        instance: InstanceId,
        method_config: MethodConfig,
    ) -> Result<OfferingHandle<S>> {
        let service_id = ServiceId::new(S::SERVICE_ID).expect("Invalid service ID");

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        self.inner
            .cmd_tx
            .send(Command::Offer {
                service_id,
                instance_id: instance,
                major_version: S::MAJOR_VERSION,
                minor_version: S::MINOR_VERSION,
                method_config,
                response: response_tx,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        let requests_rx = response_rx.await.map_err(|_| Error::RuntimeShutdown)??;

        Ok(OfferingHandle::new(
            Arc::clone(&self.inner),
            service_id,
            instance,
            requests_rx,
        ))
    }

    /// Bind a service instance to a local endpoint without announcing via SD.
    ///
    /// Returns a `ServiceInstance<S, Bound>` that can:
    /// - Accept RPC requests from clients with pre-configured addresses
    /// - Add static subscribers for notifications without SD
    /// - Later transition to `Announced` state via `announce()`
    ///
    /// This implements the bind/announce separation pattern per RECENT/IP spec:
    /// - `feat_req_recentipsd_184`: SD is about service state, not location
    /// - `feat_req_recentipsd_444`: Implicit subscriptions are supported
    ///
    /// # Example
    /// ```no_run
    /// use someip_runtime::prelude::*;
    ///
    /// struct MyService;
    /// impl Service for MyService {
    ///     const SERVICE_ID: u16 = 0x1234;
    ///     const MAJOR_VERSION: u8 = 1;
    ///     const MINOR_VERSION: u32 = 0;
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let runtime = Runtime::new(RuntimeConfig::default()).await?;
    ///
    ///     let service = runtime.bind::<MyService>(InstanceId::Id(1)).await?;
    ///     // Service is listening, but not announced via SD
    ///     // Can handle static deployments where clients know the address
    ///
    ///     let announced = service.announce().await?;
    ///     // Now announced via SD, clients can discover
    ///     Ok(())
    /// }
    /// ```
    pub async fn bind<S: Service>(
        &self,
        instance: InstanceId,
    ) -> Result<crate::handle::ServiceInstance<S, crate::handle::Bound>> {
        self.bind_with_config::<S>(instance, MethodConfig::default())
            .await
    }

    /// Bind a service instance with custom method configuration.
    ///
    /// See `bind()` for details. This variant allows configuring which methods
    /// use EXCEPTION (0x81) message type for errors.
    pub async fn bind_with_config<S: Service>(
        &self,
        instance: InstanceId,
        method_config: MethodConfig,
    ) -> Result<crate::handle::ServiceInstance<S, crate::handle::Bound>> {
        let service_id = ServiceId::new(S::SERVICE_ID).expect("Invalid service ID");

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        self.inner
            .cmd_tx
            .send(Command::Bind {
                service_id,
                instance_id: instance,
                major_version: S::MAJOR_VERSION,
                minor_version: S::MINOR_VERSION,
                method_config,
                response: response_tx,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        let requests_rx = response_rx.await.map_err(|_| Error::RuntimeShutdown)??;

        Ok(crate::handle::ServiceInstance::new(
            Arc::clone(&self.inner),
            service_id,
            instance,
            requests_rx,
        ))
    }

    /// Find a service by pre-configured address (no Service Discovery).
    ///
    /// Returns a `ProxyHandle<S, Available>` that is immediately usable
    /// without waiting for SD discovery. Use this for static deployments
    /// where service locations are pre-configured.
    ///
    /// # Example
    /// ```no_run
    /// use someip_runtime::prelude::*;
    ///
    /// struct MyService;
    /// impl Service for MyService {
    ///     const SERVICE_ID: u16 = 0x1234;
    ///     const MAJOR_VERSION: u8 = 1;
    ///     const MINOR_VERSION: u32 = 0;
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let runtime = Runtime::new(RuntimeConfig::default()).await?;
    ///
    ///     let proxy = runtime.find_static::<MyService>(
    ///         InstanceId::Id(1),
    ///         "192.168.1.10:30509".parse().unwrap(),
    ///     );
    ///     // Can immediately make RPC calls
    ///     let method = MethodId::new(0x0001).unwrap();
    ///     let response = proxy.call(method, b"payload").await?;
    ///     Ok(())
    /// }
    /// ```
    pub fn find_static<S: Service>(
        &self,
        instance: InstanceId,
        endpoint: SocketAddr,
    ) -> ProxyHandle<S, Available> {
        let service_id = ServiceId::new(S::SERVICE_ID).expect("Invalid service ID");
        ProxyHandle::new_available(Arc::clone(&self.inner), service_id, instance, endpoint)
    }

    /// Listen for events from a static (pre-configured) service endpoint.
    ///
    /// Returns a `StaticEventListener` that receives events without SD.
    /// Use this for static deployments where the service address is
    /// pre-configured and events are sent directly to this client.
    ///
    /// The `port` parameter specifies which UDP port to listen on for events.
    /// The server should be configured to send events to this client's
    /// IP address and this port.
    ///
    /// # Example
    /// ```no_run
    /// use someip_runtime::prelude::*;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let runtime = Runtime::new(RuntimeConfig::default()).await?;
    ///
    ///     // Client listens on port 30502
    ///     let mut listener = runtime.listen_static(
    ///         ServiceId::new(0x1234).unwrap(),
    ///         InstanceId::Id(1),
    ///         EventgroupId::new(0x0001).unwrap(),
    ///         30502,
    ///     ).await?;
    ///
    ///     // Server adds this client as static subscriber
    ///     // service.add_static_subscriber("client:30502", &[eventgroup]);
    ///
    ///     while let Some(event) = listener.next().await {
    ///         println!("Received event: {:?}", event);
    ///     }
    ///     Ok(())
    /// }
    /// ```
    pub async fn listen_static(
        &self,
        service_id: ServiceId,
        instance_id: InstanceId,
        eventgroup: crate::EventgroupId,
        port: u16,
    ) -> Result<crate::handle::StaticEventListener> {
        let (events_tx, events_rx) = mpsc::channel(64);
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        self.inner
            .cmd_tx
            .send(Command::ListenStatic {
                service_id,
                instance_id,
                eventgroup_id: eventgroup.value(),
                port,
                events: events_tx,
                response: response_tx,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        response_rx.await.map_err(|_| Error::RuntimeShutdown)??;

        Ok(crate::handle::StaticEventListener {
            eventgroup,
            events: events_rx,
        })
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
    /// use someip_runtime::prelude::*;
    /// use someip_runtime::handle::ServiceEvent;
    ///
    /// struct MyService;
    /// impl Service for MyService {
    ///     const SERVICE_ID: u16 = 0x1234;
    ///     const MAJOR_VERSION: u8 = 1;
    ///     const MINOR_VERSION: u32 = 0;
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let runtime = Runtime::new(RuntimeConfig::default()).await?;
    ///     let mut offering = runtime.offer::<MyService>(InstanceId::Id(1)).await?;
    ///
    ///     // Process one request then shutdown
    ///     if let Some(ServiceEvent::Call { responder, .. }) = offering.next().await {
    ///         responder.reply(b"response").await?;
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
    /// # use someip_runtime::{Runtime, SdEvent};
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let runtime = Runtime::create(Default::default()).await?;
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
}

// ============================================================================
// RUNTIME TASK
// ============================================================================

/// The main runtime task
async fn runtime_task<U: UdpSocket, T: TcpStream, L: TcpListener<Stream = T>>(
    sd_socket: U,
    config: RuntimeConfig,
    mut cmd_rx: mpsc::Receiver<Command>,
    mut rpc_rx: mpsc::Receiver<RpcMessage>,
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
                        let data = &buf[..len];
                        if let Some(actions) = handle_packet(data, from, &mut state, None) {
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

            // Handle incoming RPC messages from UDP RPC socket tasks
            Some(rpc_msg) = rpc_rx.recv() => {
                if let Some(actions) = handle_packet(&rpc_msg.data, rpc_msg.from, &mut state, rpc_msg.service_key) {
                    for action in actions {
                        execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
                    }
                }
            }

            // Handle incoming RPC messages from TCP server connections
            Some(tcp_msg) = tcp_rpc_rx.recv() => {
                // For TCP messages, we need to find the service key by looking up which service
                // this message is for (based on the service_id in the header)
                if let Some(actions) = handle_tcp_rpc_message(&tcp_msg, &mut state) {
                    for action in actions {
                        execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
                    }
                }
            }

            // Handle responses received on client TCP connections
            Some(tcp_msg) = tcp_client_rx.recv() => {
                // These are responses to RPC calls we made as a client
                // Process them like any other incoming packet
                if let Some(actions) = handle_packet(&tcp_msg.data, tcp_msg.from, &mut state, None) {
                    for action in actions {
                        execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
                    }
                }
            }

            // Handle commands from handles
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(Command::Shutdown) | None => {
                        tracing::info!("Runtime shutting down, draining {} pending responses", pending_responses.len());
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
                        tracing::info!("Runtime shutdown complete");
                        break;
                    }
                    // Special handling for Offer - needs async socket/listener creation
                    Some(Command::Offer { service_id, instance_id, major_version, minor_version, method_config, response }) => {
                        if let Some(actions) = server::handle_offer_command::<U, T, L>(
                            service_id, instance_id, major_version, minor_version, method_config, response,
                            &config, &mut state, &rpc_tx, &tcp_rpc_tx
                        ).await {
                            for action in actions {
                                execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses, &tcp_pool).await;
                            }
                        }
                    }
                    // Special handling for Bind - needs async socket/listener creation (no SD announcement)
                    Some(Command::Bind { service_id, instance_id, major_version, minor_version, method_config, response }) => {
                        server::handle_bind_command::<U, T, L>(
                            service_id, instance_id, major_version, minor_version, method_config, response,
                            &config, &mut state, &rpc_tx, &tcp_rpc_tx
                        ).await;
                    }
                    // Special handling for ListenStatic - needs async socket creation
                    Some(Command::ListenStatic { service_id, instance_id, eventgroup_id, port, events, response }) => {
                        server::handle_listen_static_command::<U>(
                            service_id, instance_id, eventgroup_id, port, events, response,
                            &mut state
                        ).await;
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
    config: &RuntimeConfig,
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
                if req_key.matches(key.service_id, key.instance_id) {
                    let _ = request.notify.try_send(availability.clone());
                }
            }
        }
        Action::SendClientMessage { data, target } => {
            // Use TCP or UDP based on configuration
            if config.transport == Transport::Tcp {
                // Send via TCP connection pool (establishes connection if needed)
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
        } => {
            // Server messages use the service's RPC transport (UDP or TCP)
            if let Some(offered) = state.offered.get(&service_key) {
                if let Err(e) = offered.rpc_transport.send(data.to_vec(), target).await {
                    tracing::error!("Failed to send server message via RPC transport: {}", e);
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
                    .unwrap_or_else(|_| Err(Error::RuntimeShutdown));
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
            state.sd_monitors.retain(|monitor| {
                monitor.try_send(event.clone()).is_ok()
            });
        }
    }
}

/// Handle an incoming packet (SD or RPC)
fn handle_packet(
    data: &[u8],
    from: SocketAddr,
    state: &mut RuntimeState,
    service_key: Option<ServiceKey>,
) -> Option<Vec<Action>> {
    let mut cursor = data;

    // Parse SOME/IP header
    let header = Header::parse(&mut cursor)?;

    // Check if it's an SD message
    if header.service_id == SD_SERVICE_ID && header.method_id == SD_METHOD_ID {
        return handle_sd_message(&header, &mut cursor, from, state);
    }

    // Handle RPC messages
    handle_rpc_message(&header, &mut cursor, from, state, service_key)
}

/// Handle an RPC message received via TCP server
///
/// TCP messages from clients don't have an explicit `service_key` since they come from
/// a shared channel. We need to determine the `service_key` from the SOME/IP header's
/// `service_id` and find the matching offered service.
fn handle_tcp_rpc_message(tcp_msg: &TcpMessage, state: &mut RuntimeState) -> Option<Vec<Action>> {
    let mut cursor = &tcp_msg.data[..];

    // Parse SOME/IP header to get service_id
    let header = Header::parse(&mut cursor)?;

    // Find which offered service this belongs to based on service_id
    let service_key = state
        .offered
        .iter()
        .find(|(key, _)| key.service_id == header.service_id)
        .map(|(key, _)| *key);

    // Process as RPC message
    handle_rpc_message(&header, &mut cursor, tcp_msg.from, state, service_key)
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
fn handle_rpc_message(
    header: &Header,
    cursor: &mut &[u8],
    from: SocketAddr,
    state: &mut RuntimeState,
    service_key: Option<ServiceKey>,
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
            notify,
        } => {
            client::handle_find(service_id, instance_id, notify, state, &mut actions);
        }

        Command::StopFind {
            service_id,
            instance_id,
        } => {
            client::handle_stop_find(service_id, instance_id, state);
        }

        Command::StopOffer {
            service_id,
            instance_id,
        } => {
            server::handle_stop_offer(service_id, instance_id, state, &mut actions);
        }

        Command::Call {
            service_id,
            instance_id,
            method_id,
            payload,
            response,
            target_endpoint,
        } => {
            client::handle_call(
                service_id,
                instance_id,
                method_id,
                payload,
                response,
                target_endpoint,
                state,
                &mut actions,
            );
        }

        Command::FireAndForget {
            service_id,
            instance_id,
            method_id,
            payload,
            target_endpoint,
        } => {
            client::handle_fire_and_forget(
                service_id,
                instance_id,
                method_id,
                payload,
                target_endpoint,
                state,
                &mut actions,
            );
        }

        Command::Subscribe {
            service_id,
            instance_id,
            eventgroup_id,
            events,
            response,
        } => {
            client::handle_subscribe(
                service_id,
                instance_id,
                eventgroup_id,
                events,
                response,
                state,
                &mut actions,
            );
        }

        Command::Unsubscribe {
            service_id,
            instance_id,
            eventgroup_id,
        } => {
            client::handle_unsubscribe(service_id, instance_id, eventgroup_id, state, &mut actions);
        }

        Command::Notify {
            service_id,
            instance_id,
            eventgroup_id,
            event_id,
            payload,
        } => {
            server::handle_notify(
                service_id,
                instance_id,
                eventgroup_id,
                event_id,
                payload,
                state,
                &mut actions,
            );
        }

        Command::NotifyStatic {
            service_id,
            instance_id,
            eventgroup_id: _,
            event_id,
            payload,
            targets,
        } => {
            server::handle_notify_static(
                service_id,
                instance_id,
                event_id,
                payload,
                targets,
                state,
                &mut actions,
            );
        }

        Command::StartAnnouncing {
            service_id,
            instance_id,
            response,
        } => {
            server::handle_start_announcing(service_id, instance_id, response, state, &mut actions);
        }

        Command::StopAnnouncing {
            service_id,
            instance_id,
            response,
        } => {
            server::handle_stop_announcing(service_id, instance_id, response, state, &mut actions);
        }

        Command::FindStatic {
            service_id: _,
            instance_id,
            endpoint,
            notify,
        } => {
            client::handle_find_static(instance_id, endpoint, notify);
        }

        Command::HasSubscribers {
            service_id,
            instance_id,
            eventgroup_id,
            response,
        } => {
            server::handle_has_subscribers(service_id, instance_id, eventgroup_id, response, state);
        }

        Command::MonitorSd { events } => {
            state.sd_monitors.push(events);
        }

        // These are handled separately in runtime_task for async socket creation
        Command::Shutdown
        | Command::Offer { .. }
        | Command::Bind { .. }
        | Command::ListenStatic { .. } => {}
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
    let ttl = state.config.ttl;
    let transport = state.config.transport;
    let sd_multicast = state.config.sd_multicast;

    // Cyclic offers (only for services that are announcing)
    for (key, offered) in &mut state.offered {
        // Skip services that are bound but not announcing
        if !offered.is_announcing {
            continue;
        }

        if now.duration_since(offered.last_offer) >= offer_interval {
            offered.last_offer = now;

            let msg = build_offer_message(key, offered, sd_flags, ttl, transport);

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

            let msg = build_find_message(key.service_id, key.instance_id, sd_flags, ttl);

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
            actions.push(Action::NotifyFound {
                key,
                availability: ServiceAvailability::Unavailable,
            });

            // Emit SD event to monitors
            actions.push(Action::EmitSdEvent {
                event: crate::SdEvent::ServiceExpired {
                    service_id: key.service_id,
                    instance_id: key.instance_id,
                },
            });
        }
    }

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
