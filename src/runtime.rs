//! SOME/IP runtime - manages network I/O and service discovery.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{interval, Instant};

use crate::error::{Error, Result};
use crate::handle::{Available, OfferingHandle, ProxyHandle, Unavailable};
use crate::net::{TcpListener, TcpStream, UdpSocket};
use crate::tcp::{TcpConnectionPool, TcpMessage, TcpSendMessage, TcpServer};
use crate::wire::{
    validate_protocol_version, Header, L4Protocol, MessageType, SdEntry, SdEntryType, SdMessage,
    SdOption, PROTOCOL_VERSION, SD_METHOD_ID, SD_SERVICE_ID,
};
use crate::{InstanceId, Response, ReturnCode, Service, ServiceId};

/// Default SD multicast address
pub const DEFAULT_SD_MULTICAST: Ipv4Addr = Ipv4Addr::new(239, 255, 0, 1);

/// Default SD port
pub const DEFAULT_SD_PORT: u16 = 30490;

/// Default TTL for SD entries (seconds)
pub const DEFAULT_TTL: u32 = 3600;

/// Default cyclic offer interval (ms)
pub const DEFAULT_CYCLIC_OFFER_DELAY: u64 = 1000;

/// Default find request repetitions
pub const DEFAULT_FIND_REPETITIONS: u32 = 3;

/// Transport protocol for RPC communication
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transport {
    /// UDP transport (default)
    #[default]
    Udp,
    /// TCP transport
    Tcp,
}

/// Runtime configuration
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Local address to bind to (default: 0.0.0.0:0)
    pub local_addr: SocketAddr,
    /// SD multicast group address (default: 239.255.0.1:30490)
    pub sd_multicast: SocketAddr,
    /// TTL for SD entries (default: 3600 seconds)
    pub ttl: u32,
    /// Cyclic offer delay in ms (default: 1000)
    pub cyclic_offer_delay: u64,
    /// Transport protocol for RPC communication (default: UDP)
    pub transport: Transport,
    /// Enable Magic Cookies for TCP resynchronization (default: false)
    ///
    /// When enabled (feat_req_recentip_586, feat_req_recentip_591, feat_req_recentip_592):
    /// - Each TCP segment starts with a Magic Cookie message
    /// - Only one Magic Cookie per segment
    /// - Allows resync in testing/debugging scenarios
    pub magic_cookies: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            // Use the SD port for multicast group membership to work in turmoil
            local_addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_SD_PORT)),
            sd_multicast: SocketAddr::V4(SocketAddrV4::new(DEFAULT_SD_MULTICAST, DEFAULT_SD_PORT)),
            ttl: DEFAULT_TTL,
            cyclic_offer_delay: DEFAULT_CYCLIC_OFFER_DELAY,
            transport: Transport::Udp,
            magic_cookies: false,
        }
    }
}

impl RuntimeConfig {
    /// Create a new builder
    pub fn builder() -> RuntimeConfigBuilder {
        RuntimeConfigBuilder::default()
    }
}

/// Builder for RuntimeConfig
#[derive(Default)]
pub struct RuntimeConfigBuilder {
    config: RuntimeConfig,
}

impl RuntimeConfigBuilder {
    /// Set the local address
    pub fn local_addr(mut self, addr: SocketAddr) -> Self {
        self.config.local_addr = addr;
        self
    }

    /// Set the SD multicast address
    pub fn sd_multicast(mut self, addr: SocketAddr) -> Self {
        self.config.sd_multicast = addr;
        self
    }

    /// Set the TTL for SD entries
    pub fn ttl(mut self, ttl: u32) -> Self {
        self.config.ttl = ttl;
        self
    }

    /// Set the cyclic offer delay
    pub fn cyclic_offer_delay(mut self, delay_ms: u64) -> Self {
        self.config.cyclic_offer_delay = delay_ms;
        self
    }

    /// Set the transport protocol (UDP or TCP)
    pub fn transport(mut self, transport: Transport) -> Self {
        self.config.transport = transport;
        self
    }

    /// Enable or disable Magic Cookies for TCP (default: false)
    ///
    /// Magic Cookies allow resynchronization in testing/debugging scenarios.
    /// See feat_req_recentip_586, feat_req_recentip_591, feat_req_recentip_592.
    pub fn magic_cookies(mut self, enabled: bool) -> Self {
        self.config.magic_cookies = enabled;
        self
    }

    /// Build the configuration
    pub fn build(self) -> RuntimeConfig {
        self.config
    }
}

// ============================================================================
// METHOD CONFIGURATION
// ============================================================================

use std::collections::HashSet;

/// Configuration for how a service handles error responses.
///
/// Per SOME/IP specification (feat_req_recentip_106, feat_req_recentip_726):
/// - By default, errors use RESPONSE (0x80) with non-OK return code
/// - EXCEPTION (0x81) is optional and must be explicitly configured per-method
///
/// This configuration is typically defined in the interface specification (IDL/FIDL)
/// at design time, not decided per-call at runtime.
#[derive(Debug, Clone, Default)]
pub struct MethodConfig {
    /// Set of method IDs that use EXCEPTION (0x81) message type for errors.
    /// Methods not in this set use RESPONSE (0x80) with error return code.
    exception_methods: HashSet<u16>,
}

impl MethodConfig {
    /// Create a new empty configuration (all methods use RESPONSE for errors)
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure a method to use EXCEPTION (0x81) message type for errors.
    ///
    /// Per spec, this should match the interface specification for the method.
    pub fn use_exception_for(mut self, method_id: u16) -> Self {
        self.exception_methods.insert(method_id);
        self
    }

    /// Check if a method uses EXCEPTION message type for errors.
    pub fn uses_exception(&self, method_id: u16) -> bool {
        self.exception_methods.contains(&method_id)
    }
}

// ============================================================================
// COMMANDS
// ============================================================================

/// Commands sent from handles to the runtime task
pub(crate) enum Command {
    /// Find a service
    Find {
        service_id: ServiceId,
        instance_id: InstanceId,
        notify: mpsc::Sender<ServiceAvailability>,
    },
    /// Stop finding a service
    StopFind {
        service_id: ServiceId,
        instance_id: InstanceId,
    },
    /// Offer a service
    Offer {
        service_id: ServiceId,
        instance_id: InstanceId,
        major_version: u8,
        minor_version: u32,
        method_config: MethodConfig,
        response: oneshot::Sender<Result<mpsc::Receiver<ServiceRequest>>>,
    },
    /// Stop offering a service
    StopOffer {
        service_id: ServiceId,
        instance_id: InstanceId,
    },
    /// Call a method
    Call {
        service_id: ServiceId,
        instance_id: InstanceId,
        method_id: u16,
        payload: bytes::Bytes,
        response: oneshot::Sender<Result<crate::Response>>,
        /// For static deployments: pre-configured endpoint
        target_endpoint: Option<SocketAddr>,
    },
    /// Fire-and-forget call (no response expected)
    FireAndForget {
        service_id: ServiceId,
        instance_id: InstanceId,
        method_id: u16,
        payload: bytes::Bytes,
        /// For static deployments: pre-configured endpoint
        target_endpoint: Option<SocketAddr>,
    },
    /// Subscribe to an eventgroup
    Subscribe {
        service_id: ServiceId,
        instance_id: InstanceId,
        eventgroup_id: u16,
        events: mpsc::Sender<crate::Event>,
        response: oneshot::Sender<Result<()>>,
    },
    /// Unsubscribe from an eventgroup
    Unsubscribe {
        service_id: ServiceId,
        instance_id: InstanceId,
        eventgroup_id: u16,
    },
    /// Send a notification event (server-side)
    Notify {
        service_id: ServiceId,
        instance_id: InstanceId,
        eventgroup_id: u16,
        event_id: u16,
        payload: bytes::Bytes,
    },
    /// Send a notification to static subscribers only (no SD)
    NotifyStatic {
        service_id: ServiceId,
        instance_id: InstanceId,
        eventgroup_id: u16,
        event_id: u16,
        payload: bytes::Bytes,
        targets: Vec<SocketAddr>,
    },
    /// Bind a service (listen on socket, no SD announcement)
    Bind {
        service_id: ServiceId,
        instance_id: InstanceId,
        major_version: u8,
        minor_version: u32,
        method_config: MethodConfig,
        response: oneshot::Sender<Result<mpsc::Receiver<ServiceRequest>>>,
    },
    /// Start announcing a bound service via SD
    StartAnnouncing {
        service_id: ServiceId,
        instance_id: InstanceId,
        response: oneshot::Sender<Result<()>>,
    },
    /// Stop announcing a service (keeps socket open)
    StopAnnouncing {
        service_id: ServiceId,
        instance_id: InstanceId,
        response: oneshot::Sender<Result<()>>,
    },
    /// Create a static proxy (pre-configured address, no SD)
    FindStatic {
        service_id: ServiceId,
        instance_id: InstanceId,
        endpoint: SocketAddr,
        notify: mpsc::Sender<ServiceAvailability>,
    },
    /// Query if there are subscribers for an eventgroup
    HasSubscribers {
        service_id: ServiceId,
        instance_id: InstanceId,
        eventgroup_id: u16,
        response: oneshot::Sender<bool>,
    },
    /// Listen for static events (pre-configured, no SD)
    ListenStatic {
        service_id: ServiceId,
        instance_id: InstanceId,
        eventgroup_id: u16,
        /// Port to bind for receiving events
        port: u16,
        /// Channel to send received events
        events: mpsc::Sender<crate::Event>,
        response: oneshot::Sender<Result<()>>,
    },
    /// Shutdown the runtime
    #[allow(dead_code)]
    Shutdown,
}

/// Service availability notification
#[derive(Debug, Clone)]
pub(crate) enum ServiceAvailability {
    Available {
        endpoint: SocketAddr,
        instance_id: u16,
    },
    Unavailable,
}

/// Service request (for offerings)
pub(crate) enum ServiceRequest {
    MethodCall {
        method_id: u16,
        payload: bytes::Bytes,
        client: SocketAddr,
        response: oneshot::Sender<Result<bytes::Bytes>>,
    },
    FireForget {
        method_id: u16,
        payload: bytes::Bytes,
        client: SocketAddr,
    },
    Subscribe {
        eventgroup_id: u16,
        client: SocketAddr,
        response: oneshot::Sender<Result<bool>>,
    },
    Unsubscribe {
        eventgroup_id: u16,
        client: SocketAddr,
    },
}

// ============================================================================
// RUNTIME STATE
// ============================================================================

/// Key for service identification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ServiceKey {
    service_id: u16,
    instance_id: u16,
}

impl ServiceKey {
    fn new(service_id: ServiceId, instance_id: InstanceId) -> Self {
        Self {
            service_id: service_id.value(),
            instance_id: instance_id.value(),
        }
    }

    fn matches(&self, service_id: u16, instance_id: u16) -> bool {
        self.service_id == service_id
            && (self.instance_id == 0xFFFF
                || self.instance_id == instance_id
                || instance_id == 0xFFFF)
    }
}

/// Message received from an RPC socket task
#[derive(Debug)]
struct RpcMessage {
    service_key: Option<ServiceKey>,
    data: Vec<u8>,
    from: SocketAddr,
}

/// Message to send via an RPC socket task  
#[derive(Debug)]
struct RpcSendMessage {
    data: Vec<u8>,
    to: SocketAddr,
}

/// Transport sender for offered services - either UDP or TCP
#[derive(Debug, Clone)]
enum RpcTransportSender {
    /// UDP socket sender
    Udp(mpsc::Sender<RpcSendMessage>),
    /// TCP server sender
    Tcp(mpsc::Sender<TcpSendMessage>),
}

impl RpcTransportSender {
    /// Send a message to the specified target
    async fn send(&self, data: Vec<u8>, to: SocketAddr) -> Result<()> {
        match self {
            RpcTransportSender::Udp(tx) => tx
                .send(RpcSendMessage { data, to })
                .await
                .map_err(|_| Error::RuntimeShutdown),
            RpcTransportSender::Tcp(tx) => tx
                .send(TcpSendMessage { data, to })
                .await
                .map_err(|_| Error::RuntimeShutdown),
        }
    }
}

/// Tracked offered service (our offerings)
struct OfferedService {
    major_version: u8,
    minor_version: u32,
    requests_tx: mpsc::Sender<ServiceRequest>,
    last_offer: Instant,
    /// Dedicated RPC endpoint for this service instance (separate from SD socket)
    rpc_endpoint: SocketAddr,
    /// Channel to send outgoing RPC messages to this service's socket/TCP task
    rpc_transport: RpcTransportSender,
    /// Configuration for which methods use EXCEPTION message type
    method_config: MethodConfig,
    /// Whether currently announcing via SD (false = bound but not announced)
    is_announcing: bool,
}

/// Tracked find request
struct FindRequest {
    notify: mpsc::Sender<ServiceAvailability>,
    repetitions_left: u32,
    last_find: Instant,
}

/// Discovered remote service
#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields used for version matching in future
struct DiscoveredService {
    /// UDP endpoint for sending SOME/IP RPC messages (if using UDP transport)
    udp_endpoint: Option<SocketAddr>,
    /// TCP endpoint for sending SOME/IP RPC messages (if using TCP transport)
    tcp_endpoint: Option<SocketAddr>,
    /// SD endpoint for sending SD messages (SubscribeEventgroup, etc.)
    sd_endpoint: SocketAddr,
    major_version: u8,
    minor_version: u32,
    ttl_expires: Instant,
}

impl DiscoveredService {
    /// Get the RPC endpoint based on preferred transport
    fn rpc_endpoint(&self, prefer_tcp: bool) -> Option<SocketAddr> {
        if prefer_tcp {
            self.tcp_endpoint.or(self.udp_endpoint)
        } else {
            self.udp_endpoint.or(self.tcp_endpoint)
        }
    }
}

/// Active subscription (client-side)
struct ClientSubscription {
    eventgroup_id: u16,
    events_tx: mpsc::Sender<crate::Event>,
}

/// Key for tracking server-side subscribers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SubscriberKey {
    service_id: u16,
    instance_id: u16,
    eventgroup_id: u16,
}

/// Pending RPC call (client-side)
struct PendingCall {
    response: oneshot::Sender<Result<Response>>,
}

/// Key for pending calls
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CallKey {
    client_id: u16,
    session_id: u16,
}

/// Pending server response (server-side) - holds context to send response back
#[derive(Debug, Clone)]
struct PendingServerResponse {
    service_id: u16,
    instance_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    interface_version: u8,
    client_addr: SocketAddr,
    /// Whether this method uses EXCEPTION (0x81) for errors instead of RESPONSE (0x80)
    uses_exception: bool,
    /// Transport to use for sending the response - captured when request received
    /// This allows responses to be sent even after the service offering is dropped
    rpc_transport: RpcTransportSender,
}

/// Runtime state managed by the runtime task
struct RuntimeState {
    /// SD endpoint (port 30490) - only for Service Discovery
    local_endpoint: SocketAddr,
    /// Client RPC endpoint (ephemeral port) - for sending client RPC requests
    /// Per feat_req_recentip_676: Port 30490 is only for SD, not for RPC
    client_rpc_endpoint: SocketAddr,
    /// Sender for client RPC messages (sends to client_rpc_socket task)
    client_rpc_tx: mpsc::Sender<RpcSendMessage>,
    /// Services we're offering
    offered: HashMap<ServiceKey, OfferedService>,
    /// Services we're looking for
    find_requests: HashMap<ServiceKey, FindRequest>,
    /// Discovered remote services
    discovered: HashMap<ServiceKey, DiscoveredService>,
    /// Active subscriptions (client-side)
    subscriptions: HashMap<ServiceKey, Vec<ClientSubscription>>,
    /// Server-side subscribers (clients subscribed to our offered services)
    server_subscribers: HashMap<SubscriberKey, Vec<SocketAddr>>,
    /// Static event listeners (client-side, no SD)
    static_listeners: HashMap<SubscriberKey, mpsc::Sender<crate::Event>>,
    /// Pending RPC calls waiting for responses
    pending_calls: HashMap<CallKey, PendingCall>,
    /// Client ID for outgoing requests
    client_id: u16,
    /// SD session ID counter
    session_id: u16,
    /// Reboot flag - set to true after startup, cleared after first session wraparound
    reboot_flag: bool,
    /// Whether the session ID has wrapped around at least once
    has_wrapped_once: bool,
    /// Configuration
    config: RuntimeConfig,
    /// Last known reboot flag state for each peer (by IP) for reboot detection
    peer_reboot_flags: HashMap<std::net::IpAddr, bool>,
}

impl RuntimeState {
    fn new(
        local_endpoint: SocketAddr,
        client_rpc_endpoint: SocketAddr,
        client_rpc_tx: mpsc::Sender<RpcSendMessage>,
        config: RuntimeConfig,
    ) -> Self {
        // Use port as part of client_id to help with uniqueness
        let client_id = (local_endpoint.port() % 0xFFFE) + 1;
        Self {
            local_endpoint,
            client_rpc_endpoint,
            client_rpc_tx,
            offered: HashMap::new(),
            find_requests: HashMap::new(),
            discovered: HashMap::new(),
            subscriptions: HashMap::new(),
            server_subscribers: HashMap::new(),
            static_listeners: HashMap::new(),
            pending_calls: HashMap::new(),
            client_id,
            session_id: 1,
            reboot_flag: true,
            has_wrapped_once: false,
            config,
            peer_reboot_flags: HashMap::new(),
        }
    }

    fn next_session_id(&mut self) -> u16 {
        let id = self.session_id;
        self.session_id = self.session_id.wrapping_add(1);
        if self.session_id == 0 {
            self.session_id = 1;
            // After first wraparound, clear the reboot flag
            if !self.has_wrapped_once {
                self.has_wrapped_once = true;
                self.reboot_flag = false;
            }
        }
        id
    }

    /// Get the SD flags byte with reboot flag based on current state
    fn sd_flags(&self, unicast: bool) -> u8 {
        let mut flags = 0u8;
        if self.reboot_flag {
            flags |= SdMessage::FLAG_REBOOT;
        }
        if unicast {
            flags |= SdMessage::FLAG_UNICAST;
        }
        flags
    }
}

// ============================================================================
// RUNTIME
// ============================================================================

/// Shared runtime state
pub(crate) struct RuntimeInner {
    /// Channel to send commands to the runtime task
    pub(crate) cmd_tx: mpsc::Sender<Command>,
}

/// SOME/IP runtime
///
/// The runtime manages all SOME/IP communication. Create one per application.
/// When all handles (proxies, offerings) are dropped, the runtime shuts down.
///
/// The runtime is generic over the UDP socket type for testing purposes.
/// TCP support is enabled via the `transport` configuration option.
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

        let state = RuntimeState::new(local_addr, client_rpc_addr, client_rpc_send_tx, config.clone());

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
    /// Per SOME/IP specification (feat_req_recentip_106, feat_req_recentip_726):
    /// - By default, errors use RESPONSE (0x80) with non-OK return code
    /// - EXCEPTION (0x81) is optional and must be explicitly configured per-method
    ///
    /// # Example
    /// ```ignore
    /// let config = MethodConfig::new()
    ///     .use_exception_for(0x0001)  // Method 0x0001 uses EXCEPTION for errors
    ///     .use_exception_for(0x0002); // Method 0x0002 also uses EXCEPTION
    ///
    /// let offering = runtime.offer_with_config::<MyService>(
    ///     InstanceId::Id(1),
    ///     config,
    /// ).await?;
    /// ```
    pub async fn offer_with_config<S: Service>(
        &self,
        instance: InstanceId,
        method_config: MethodConfig,
    ) -> Result<OfferingHandle<S>> {
        let service_id = ServiceId::new(S::SERVICE_ID).expect("Invalid service ID");

        let (response_tx, response_rx) = oneshot::channel();

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
    /// ```ignore
    /// let service = runtime.bind::<MyService>(InstanceId::Id(1)).await?;
    /// // Service is listening, but not announced via SD
    /// // Can handle static deployments where clients know the address
    ///
    /// let announced = service.announce().await?;
    /// // Now announced via SD, clients can discover
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

        let (response_tx, response_rx) = oneshot::channel();

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
    /// ```ignore
    /// let proxy = runtime.find_static::<MyService>(
    ///     InstanceId::Id(1),
    ///     "192.168.1.10:30509".parse().unwrap(),
    /// );
    /// // Can immediately make RPC calls
    /// let response = proxy.call(method, payload).await?;
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
    /// ```ignore
    /// // Client listens on port 30502
    /// let listener = runtime.listen_static(
    ///     ServiceId::new(0x1234).unwrap(),
    ///     InstanceId::Id(1),
    ///     EventgroupId::new(0x0001).unwrap(),
    ///     30502,
    /// ).await?;
    /// 
    /// // Server adds this client as static subscriber
    /// // service.add_static_subscriber("client:30502", &[eventgroup]);
    /// 
    /// while let Some(event) = listener.next().await {
    ///     println!("Received event: {:?}", event);
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
        let (response_tx, response_rx) = oneshot::channel();

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
    /// ```ignore
    /// // Process requests...
    /// responder.reply(payload).await?;
    /// 
    /// // Ensure the response is actually sent before exiting
    /// runtime.shutdown().await;
    /// ```
    pub async fn shutdown(mut self) {
        // Send shutdown command to the runtime task
        let _ = self.inner.cmd_tx.send(Command::Shutdown).await;
        
        // Wait for the runtime task to complete
        if let Some(handle) = self.runtime_task.take() {
            let _ = handle.await;
        }
    }
}

impl<U: UdpSocket, T: TcpStream, L: TcpListener<Stream = T>> Clone for Runtime<U, T, L> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            runtime_task: None, // Clones don't own the runtime task
            _phantom: std::marker::PhantomData,
        }
    }
}

// ============================================================================
// RUNTIME TASK
// ============================================================================

/// Spawns a task to handle an RPC socket for a specific service instance
/// Returns the endpoint and a sender to send outgoing messages
async fn spawn_rpc_socket_task<U: UdpSocket>(
    rpc_socket: U,
    service_id: u16,
    instance_id: u16,
    rpc_tx_to_runtime: mpsc::Sender<RpcMessage>,
) -> (SocketAddr, mpsc::Sender<RpcSendMessage>) {
    let local_endpoint = rpc_socket
        .local_addr()
        .expect("Failed to get local address");
    let (send_tx, mut send_rx) = mpsc::channel::<RpcSendMessage>(100);

    let service_key = ServiceKey {
        service_id,
        instance_id,
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

                // Send outgoing RPC messages
                Some(send_msg) = send_rx.recv() => {
                    if let Err(e) = rpc_socket.send_to(&send_msg.data, send_msg.to).await {
                        tracing::error!("Error sending on RPC socket for service {}/{}: {}",
                            service_id, instance_id, e);
                    }
                }
            }
        }
    });

    (local_endpoint, send_tx)
}

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
                        if let Some(actions) = handle_offer_command::<U, T, L>(
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
                        handle_bind_command::<U, T, L>(
                            service_id, instance_id, major_version, minor_version, method_config, response,
                            &config, &mut state, &rpc_tx, &tcp_rpc_tx
                        ).await;
                    }
                    // Special handling for ListenStatic - needs async socket creation
                    Some(Command::ListenStatic { service_id, instance_id, eventgroup_id, port, events, response }) => {
                        handle_listen_static_command::<U>(
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

/// Action to execute after handling an event
enum Action {
    /// Send an SD message to a specific target
    SendSd {
        message: SdMessage,
        target: SocketAddr,
    },
    /// Notify find requests about a discovered service
    NotifyFound {
        key: ServiceKey,
        availability: ServiceAvailability,
    },
    /// Send a SOME/IP RPC message as a client (uses SD socket)
    SendClientMessage { data: Bytes, target: SocketAddr },
    /// Send a SOME/IP RPC message as a server (uses service's RPC socket)
    SendServerMessage {
        service_key: ServiceKey,
        data: Bytes,
        target: SocketAddr,
    },
    /// Track a pending server response
    TrackServerResponse {
        context: PendingServerResponse,
        receiver: oneshot::Receiver<Result<Bytes>>,
    },
    /// Reset TCP connections to a peer (due to reboot detection)
    ResetPeerTcpConnections { peer: std::net::IpAddr },
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
/// TCP messages from clients don't have an explicit service_key since they come from
/// a shared channel. We need to determine the service_key from the SOME/IP header's
/// service_id and find the matching offered service.
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
                    handle_stop_offer(entry, state, &mut actions);
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
            handle_incoming_request(header, payload, from, state, &mut actions, service_key);
        }
        MessageType::RequestNoReturn => {
            // Incoming fire-and-forget - route to offering without response tracking
            handle_incoming_fire_forget(header, payload, from, state);
        }
        MessageType::Response | MessageType::Error => {
            // Response to our request - route to pending call
            handle_incoming_response(header, payload, state);
        }
        MessageType::Notification => {
            // Event notification - route to subscription
            handle_incoming_notification(header, payload, from, state);
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

/// Handle an incoming request (server-side)
fn handle_incoming_request(
    header: &Header,
    payload: Bytes,
    from: SocketAddr,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
    service_key: Option<ServiceKey>,
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
                response: response_tx,
            })
            .is_ok()
        {
            // Track this pending response - will be polled in the main loop
            let context = PendingServerResponse {
                service_id: header.service_id,
                instance_id: service_key.instance_id,
                method_id: header.method_id,
                client_id: header.client_id,
                session_id: header.session_id,
                interface_version: header.interface_version,
                client_addr: from,
                uses_exception,
                rpc_transport: offered.rpc_transport.clone(),
            };
            actions.push(Action::TrackServerResponse {
                context,
                receiver: response_rx,
            });
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
        actions.push(Action::SendClientMessage {
            data: response_data,
            target: from,
        });
    }
}

/// Handle an incoming fire-and-forget request (server-side, no response)
fn handle_incoming_fire_forget(
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
        });
        // No response tracking needed - fire and forget
    }
    // If unknown service, silently ignore (no error response for fire-and-forget)
}

/// Handle an incoming response (client-side)
fn handle_incoming_response(header: &Header, payload: Bytes, state: &mut RuntimeState) {
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
fn handle_incoming_notification(
    header: &Header,
    payload: Bytes,
    from: SocketAddr,
    state: &mut RuntimeState,
) {
    // Method ID is the event ID for notifications
    let event_id = match crate::EventId::new(header.method_id) {
        Some(id) => id,
        None => return, // Invalid event ID
    };

    let event = crate::Event {
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

/// Build a SOME/IP response message
///
/// Per SOME/IP specification:
/// - feat_req_recentip_726: If EXCEPTION is not configured, errors use RESPONSE (0x80)
/// - feat_req_recentip_106: EXCEPTION (0x81) is optional and must be configured per-method
/// - feat_req_recentip_107: Receiving errors on both 0x80 and 0x81 must be supported
fn build_response(
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

/// Build a SOME/IP request message
fn build_request(
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
fn build_fire_and_forget(
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

/// Build a SOME/IP notification (event) message
fn build_notification(
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

/// Handle an OfferService entry
fn handle_offer(
    entry: &SdEntry,
    sd_message: &SdMessage,
    from: SocketAddr,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    // Get UDP endpoint from SD option, falling back to source address
    // If the endpoint has unspecified IP (0.0.0.0), use the source IP with the option's port
    let udp_endpoint = match sd_message.get_udp_endpoint(entry) {
        Some(ep) if !ep.ip().is_unspecified() => Some(ep),
        Some(ep) => Some(SocketAddr::new(from.ip(), ep.port())),
        None => None,
    };

    // Get TCP endpoint from SD option if present
    let tcp_endpoint = match sd_message.get_tcp_endpoint(entry) {
        Some(ep) if !ep.ip().is_unspecified() => Some(ep),
        Some(ep) => Some(SocketAddr::new(from.ip(), ep.port())),
        None => None,
    };

    // Use UDP endpoint as fallback if neither is present
    let effective_endpoint = udp_endpoint.or(tcp_endpoint).unwrap_or(from);

    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
    };

    let ttl_duration = Duration::from_secs(entry.ttl as u64);

    tracing::debug!(
        "Discovered service {:04x}:{:04x} at {:?}/{:?} (TTL={})",
        entry.service_id,
        entry.instance_id,
        udp_endpoint,
        tcp_endpoint,
        entry.ttl
    );

    let is_new = !state.discovered.contains_key(&key);
    state.discovered.insert(
        key,
        DiscoveredService {
            udp_endpoint,
            tcp_endpoint,
            sd_endpoint: from, // Store the SD source for Subscribe messages
            major_version: entry.major_version,
            minor_version: entry.minor_version,
            ttl_expires: Instant::now() + ttl_duration,
        },
    );

    if is_new {
        actions.push(Action::NotifyFound {
            key,
            availability: ServiceAvailability::Available {
                endpoint: effective_endpoint,
                instance_id: entry.instance_id,
            },
        });
    }
}

/// Handle a StopOfferService entry
fn handle_stop_offer(entry: &SdEntry, state: &mut RuntimeState, actions: &mut Vec<Action>) {
    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
    };

    if state.discovered.remove(&key).is_some() {
        tracing::debug!(
            "Service {:04x}:{:04x} stopped offering",
            entry.service_id,
            entry.instance_id
        );

        actions.push(Action::NotifyFound {
            key,
            availability: ServiceAvailability::Unavailable,
        });
    }
}

/// Handle a FindService request (we may need to respond with an offer)
fn handle_find_request(
    entry: &SdEntry,
    from: SocketAddr,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    // Determine protocol based on transport configuration
    let protocol = match state.config.transport {
        Transport::Tcp => L4Protocol::Tcp,
        Transport::Udp => L4Protocol::Udp,
    };

    for (key, offered) in &state.offered {
        // Only respond if the service is actively announcing
        if !offered.is_announcing {
            continue;
        }

        if entry.service_id == key.service_id
            && (entry.instance_id == 0xFFFF || entry.instance_id == key.instance_id)
        {
            let mut response = SdMessage::new(state.sd_flags(true));
            let opt_idx = response.add_option(SdOption::Ipv4Endpoint {
                addr: match offered.rpc_endpoint {
                    SocketAddr::V4(v4) => *v4.ip(),
                    _ => Ipv4Addr::LOCALHOST,
                },
                port: offered.rpc_endpoint.port(), // Use RPC endpoint port, not SD port!
                protocol,
            });
            response.add_entry(SdEntry::offer_service(
                key.service_id,
                key.instance_id,
                offered.major_version,
                offered.minor_version,
                state.config.ttl,
                opt_idx,
                1,
            ));

            actions.push(Action::SendSd {
                message: response,
                target: from,
            });
        }
    }
}

/// Handle a SubscribeEventgroup request
fn handle_subscribe_request(
    entry: &SdEntry,
    sd_message: &SdMessage,
    from: SocketAddr,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
    };

    // Get client's event endpoint from SD option, falling back to source address
    // If the endpoint has unspecified IP (0.0.0.0), use the source IP with the option's port
    let client_endpoint = match sd_message.get_udp_endpoint(entry) {
        Some(ep) if !ep.ip().is_unspecified() => ep,
        Some(ep) => SocketAddr::new(from.ip(), ep.port()),
        None => from,
    };

    if let Some(offered) = state.offered.get(&key) {
        let (response_tx, _response_rx) = oneshot::channel();
        let _ = offered.requests_tx.try_send(ServiceRequest::Subscribe {
            eventgroup_id: entry.eventgroup_id,
            client: client_endpoint,
            response: response_tx,
        });

        // Track the subscriber - use the endpoint from the option for event delivery
        let sub_key = SubscriberKey {
            service_id: entry.service_id,
            instance_id: entry.instance_id,
            eventgroup_id: entry.eventgroup_id,
        };
        state
            .server_subscribers
            .entry(sub_key)
            .or_insert_with(Vec::new)
            .push(client_endpoint);

        let mut ack = SdMessage::new(state.sd_flags(true));
        ack.add_entry(SdEntry::subscribe_eventgroup_ack(
            entry.service_id,
            entry.instance_id,
            entry.major_version,
            entry.eventgroup_id,
            state.config.ttl,
            entry.counter,
        ));

        // Send ACK to the SD source (not the event endpoint)
        actions.push(Action::SendSd {
            message: ack,
            target: from,
        });
    }
}

/// Handle a StopSubscribeEventgroup request
fn handle_unsubscribe_request(
    entry: &SdEntry,
    sd_message: &SdMessage,
    from: SocketAddr,
    state: &mut RuntimeState,
) {
    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
    };

    // Get client's event endpoint from SD option (same as subscribe)
    let client_endpoint = match sd_message.get_udp_endpoint(entry) {
        Some(ep) if !ep.ip().is_unspecified() => ep,
        Some(ep) => SocketAddr::new(from.ip(), ep.port()),
        None => from,
    };

    if let Some(offered) = state.offered.get(&key) {
        let _ = offered.requests_tx.try_send(ServiceRequest::Unsubscribe {
            eventgroup_id: entry.eventgroup_id,
            client: client_endpoint,
        });

        // Remove the subscriber
        let sub_key = SubscriberKey {
            service_id: entry.service_id,
            instance_id: entry.instance_id,
            eventgroup_id: entry.eventgroup_id,
        };
        if let Some(subscribers) = state.server_subscribers.get_mut(&sub_key) {
            subscribers.retain(|addr| *addr != client_endpoint);
        }
    }
}

/// Handle a SubscribeEventgroupAck
fn handle_subscribe_ack(entry: &SdEntry, _state: &mut RuntimeState) {
    tracing::debug!(
        "Subscription acknowledged for {:04x}:{:04x} eventgroup {:04x}",
        entry.service_id,
        entry.instance_id,
        entry.eventgroup_id
    );
}

/// Handle a SubscribeEventgroupNack
fn handle_subscribe_nack(entry: &SdEntry, state: &mut RuntimeState) {
    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
    };

    tracing::debug!(
        "Subscription rejected for {:04x}:{:04x} eventgroup {:04x}",
        entry.service_id,
        entry.instance_id,
        entry.eventgroup_id
    );

    state.subscriptions.remove(&key);
}

/// Handle Command::Offer which requires async socket creation
async fn handle_offer_command<U: UdpSocket, T: TcpStream, L: TcpListener<Stream = T>>(
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    minor_version: u32,
    method_config: MethodConfig,
    response: oneshot::Sender<Result<mpsc::Receiver<ServiceRequest>>>,
    config: &RuntimeConfig,
    state: &mut RuntimeState,
    rpc_tx: &mpsc::Sender<RpcMessage>,
    tcp_rpc_tx: &mpsc::Sender<TcpMessage>,
) -> Option<Vec<Action>> {
    let key = ServiceKey::new(service_id, instance_id);
    let mut actions = Vec::new();

    // Create channel for service requests
    let (requests_tx, requests_rx) = mpsc::channel(64);

    // Determine RPC port - use next available port starting from local_addr + 1
    // For test environments, this gives us predictable ports
    let rpc_port = state.local_endpoint.port() + 1 + state.offered.len() as u16;
    let rpc_addr = SocketAddr::new(state.local_endpoint.ip(), rpc_port);

    // Determine protocol based on transport configuration
    let protocol = match config.transport {
        Transport::Tcp => L4Protocol::Tcp,
        Transport::Udp => L4Protocol::Udp,
    };

    // Create the appropriate transport listener/socket
    let result: std::io::Result<(SocketAddr, RpcTransportSender)> = match config.transport {
        Transport::Udp => {
            // Create and bind UDP RPC socket
            match U::bind(rpc_addr).await {
                Ok(rpc_socket) => {
                    let (rpc_endpoint, rpc_send_tx) = spawn_rpc_socket_task(
                        rpc_socket,
                        service_id.value(),
                        instance_id.value(),
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
            // Store the offered service with its RPC endpoint
            state.offered.insert(
                key,
                OfferedService {
                    major_version,
                    minor_version,
                    requests_tx,
                    last_offer: Instant::now() - Duration::from_secs(10),
                    rpc_endpoint,
                    rpc_transport,
                    method_config,
                    is_announcing: true, // Offer immediately announces
                },
            );

            // Create SD offer message advertising the RPC endpoint (not SD endpoint!)
            let mut msg = SdMessage::new(state.sd_flags(true));
            let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
                addr: match rpc_endpoint {
                    SocketAddr::V4(v4) => *v4.ip(),
                    _ => Ipv4Addr::LOCALHOST,
                },
                port: rpc_endpoint.port(),
                protocol,
            });
            msg.add_entry(SdEntry::offer_service(
                service_id.value(),
                instance_id.value(),
                major_version,
                minor_version,
                config.ttl,
                opt_idx,
                1,
            ));

            actions.push(Action::SendSd {
                message: msg,
                target: config.sd_multicast,
            });

            let _ = response.send(Ok(requests_rx));
        }
        Err(e) => {
            tracing::error!(
                "Failed to bind RPC {:?} on {}: {}",
                config.transport,
                rpc_addr,
                e
            );
            let _ = response.send(Err(Error::Io(e)));
        }
    }

    Some(actions)
}

/// Handle Command::Bind which creates socket but does NOT announce via SD
async fn handle_bind_command<U: UdpSocket, T: TcpStream, L: TcpListener<Stream = T>>(
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    minor_version: u32,
    method_config: MethodConfig,
    response: oneshot::Sender<Result<mpsc::Receiver<ServiceRequest>>>,
    config: &RuntimeConfig,
    state: &mut RuntimeState,
    rpc_tx: &mpsc::Sender<RpcMessage>,
    tcp_rpc_tx: &mpsc::Sender<TcpMessage>,
) {
    let key = ServiceKey::new(service_id, instance_id);

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
    let result: std::io::Result<(SocketAddr, RpcTransportSender)> = match config.transport {
        Transport::Udp => {
            // Create and bind UDP RPC socket
            match U::bind(rpc_addr).await {
                Ok(rpc_socket) => {
                    let (rpc_endpoint, rpc_send_tx) = spawn_rpc_socket_task(
                        rpc_socket,
                        service_id.value(),
                        instance_id.value(),
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
            state.offered.insert(
                key,
                OfferedService {
                    major_version,
                    minor_version,
                    requests_tx,
                    last_offer: Instant::now(),
                    rpc_endpoint,
                    rpc_transport,
                    method_config,
                    is_announcing: false, // Bind does NOT announce
                },
            );

            let _ = response.send(Ok(requests_rx));
        }
        Err(e) => {
            tracing::error!(
                "Failed to bind RPC {:?} on {}: {}",
                config.transport,
                rpc_addr,
                e
            );
            let _ = response.send(Err(Error::Io(e)));
        }
    }
}

/// Handle Command::ListenStatic which creates a socket to receive static events
async fn handle_listen_static_command<U: UdpSocket>(
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
                                        if let Some(event_id) = crate::EventId::new(header.method_id) {
                                            let payload_start = Header::SIZE;
                                            let payload_end = (header.length as usize + 8).min(len);
                                            let payload = Bytes::copy_from_slice(&buf[payload_start..payload_end]);
                                            
                                            let event = crate::Event {
                                                event_id,
                                                payload,
                                            };
                                            
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
            tracing::error!("Failed to bind static listener socket on port {}: {}", port, e);
            let _ = response.send(Err(Error::Io(e)));
        }
    }
}

/// Handle a command from a handle
fn handle_command(cmd: Command, state: &mut RuntimeState) -> Option<Vec<Action>> {
    let mut actions = Vec::new();
    let prefer_tcp = state.config.transport == Transport::Tcp;

    match cmd {
        Command::Find {
            service_id,
            instance_id,
            notify,
        } => {
            let key = ServiceKey::new(service_id, instance_id);

            // Check if we already have a matching discovered service
            // Use wildcard-aware matching: if instance_id is Any (0xFFFF), match any instance
            let found = if instance_id == InstanceId::Any {
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

                let mut msg = SdMessage::new(state.sd_flags(true));
                msg.add_entry(SdEntry::find_service(
                    service_id.value(),
                    instance_id.value(),
                    0xFF,
                    0xFFFFFFFF,
                    state.config.ttl,
                ));

                actions.push(Action::SendSd {
                    message: msg,
                    target: state.config.sd_multicast,
                });
            }
        }

        Command::StopFind {
            service_id,
            instance_id,
        } => {
            let key = ServiceKey::new(service_id, instance_id);
            state.find_requests.remove(&key);
        }

        // Command::Offer is handled separately in runtime_task for async socket creation
        Command::StopOffer {
            service_id,
            instance_id,
        } => {
            let key = ServiceKey::new(service_id, instance_id);

            if let Some(offered) = state.offered.remove(&key) {
                let mut msg = SdMessage::new(state.sd_flags(false));
                msg.add_entry(SdEntry::stop_offer_service(
                    service_id.value(),
                    instance_id.value(),
                    offered.major_version,
                    offered.minor_version,
                ));

                actions.push(Action::SendSd {
                    message: msg,
                    target: state.config.sd_multicast,
                });
            }
        }

        Command::Call {
            service_id,
            instance_id,
            method_id,
            payload,
            response,
            target_endpoint,
        } => {
            let key = ServiceKey::new(service_id, instance_id);

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
                let _ = response.send(Err(Error::ServiceUnavailable));
            }
        }

        Command::FireAndForget {
            service_id,
            instance_id,
            method_id,
            payload,
            target_endpoint,
        } => {
            let key = ServiceKey::new(service_id, instance_id);

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

        Command::Subscribe {
            service_id,
            instance_id,
            eventgroup_id,
            events,
            response,
        } => {
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

                // Determine protocol for subscription endpoint
                let protocol = match state.config.transport {
                    Transport::Tcp => L4Protocol::Tcp,
                    Transport::Udp => L4Protocol::Udp,
                };

                let mut msg = SdMessage::new(state.sd_flags(true));
                let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
                    addr: match state.local_endpoint {
                        SocketAddr::V4(v4) => *v4.ip(),
                        _ => Ipv4Addr::LOCALHOST,
                    },
                    port: state.client_rpc_endpoint.port(),
                    protocol,
                });
                let mut entry = SdEntry::subscribe_eventgroup(
                    service_id.value(),
                    instance_id.value(),
                    0xFF,
                    eventgroup_id,
                    state.config.ttl,
                    0,
                );
                entry.index_1st_option = opt_idx;
                entry.num_options_1 = 1;
                msg.add_entry(entry);

                actions.push(Action::SendSd {
                    message: msg,
                    target: discovered.sd_endpoint, // Send to SD socket, not RPC socket
                });

                let _ = response.send(Ok(()));
            } else {
                let _ = response.send(Err(Error::ServiceUnavailable));
            }
        }

        Command::Unsubscribe {
            service_id,
            instance_id,
            eventgroup_id,
        } => {
            let key = ServiceKey::new(service_id, instance_id);

            if let Some(subs) = state.subscriptions.get_mut(&key) {
                subs.retain(|s| s.eventgroup_id != eventgroup_id);
            }

            if let Some(discovered) = state.discovered.get(&key) {
                // Determine protocol for subscription endpoint
                let protocol = match state.config.transport {
                    Transport::Tcp => L4Protocol::Tcp,
                    Transport::Udp => L4Protocol::Udp,
                };

                let mut msg = SdMessage::new(state.sd_flags(true));
                // Include the same endpoint option as in Subscribe so server knows which subscriber to remove
                let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
                    addr: match state.local_endpoint {
                        SocketAddr::V4(v4) => *v4.ip(),
                        _ => Ipv4Addr::LOCALHOST,
                    },
                    port: state.client_rpc_endpoint.port(),
                    protocol,
                });
                let mut entry = SdEntry::subscribe_eventgroup(
                    service_id.value(),
                    instance_id.value(),
                    0xFF,
                    eventgroup_id,
                    0, // TTL=0 indicates unsubscribe
                    0,
                );
                entry.index_1st_option = opt_idx;
                entry.num_options_1 = 1;
                msg.add_entry(entry);

                actions.push(Action::SendSd {
                    message: msg,
                    target: discovered.sd_endpoint, // Send to SD socket, not RPC socket
                });
            }
        }

        Command::Notify {
            service_id,
            instance_id,
            eventgroup_id,
            event_id,
            payload,
        } => {
            let service_key = ServiceKey::new(service_id, instance_id);

            // Find all subscribers for this eventgroup
            let sub_key = SubscriberKey {
                service_id: service_id.value(),
                instance_id: instance_id.value(),
                eventgroup_id,
            };

            // Clone subscribers to avoid borrow conflict with next_session_id
            let subscribers: Vec<SocketAddr> = state
                .server_subscribers
                .get(&sub_key)
                .map(|s| s.clone())
                .unwrap_or_default();

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

                for subscriber in subscribers {
                    actions.push(Action::SendServerMessage {
                        service_key,
                        data: notification_data.clone(),
                        target: subscriber,
                    });
                }
            }
        }

        Command::Shutdown => {}

        // Command::Offer is handled separately in runtime_task for async socket creation
        // TODO. Is this fine?
        Command::Offer { .. } => {}

        // Command::Bind is handled separately in runtime_task for async socket creation
        Command::Bind { .. } => {}

        // Command::ListenStatic is handled separately in runtime_task
        Command::ListenStatic { .. } => {}

        // NotifyStatic: Send notification to specific static subscribers
        Command::NotifyStatic {
            service_id,
            instance_id,
            eventgroup_id: _,
            event_id,
            payload,
            targets,
        } => {
            let service_key = ServiceKey::new(service_id, instance_id);

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
                    });
                }
            }
        }

        // StartAnnouncing: Begin SD announcements for an already-bound service
        Command::StartAnnouncing {
            service_id,
            instance_id,
            response,
        } => {
            let key = ServiceKey::new(service_id, instance_id);

            // First, read-only access to extract what we need
            let sd_flags = state.sd_flags(true);
            let protocol = match state.config.transport {
                Transport::Tcp => L4Protocol::Tcp,
                Transport::Udp => L4Protocol::Udp,
            };
            let ttl = state.config.ttl;
            let sd_multicast = state.config.sd_multicast;

            if let Some(offered) = state.offered.get_mut(&key) {
                // Mark as announcing
                offered.is_announcing = true;
                offered.last_offer = Instant::now() - Duration::from_secs(10); // Force immediate offer

                // Extract what we need from offered before creating msg
                let rpc_endpoint = offered.rpc_endpoint;
                let major_version = offered.major_version;
                let minor_version = offered.minor_version;

                let _ = response.send(Ok(()));

                // Send initial offer
                let mut msg = SdMessage::new(sd_flags);
                let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
                    addr: match rpc_endpoint {
                        SocketAddr::V4(v4) => *v4.ip(),
                        _ => Ipv4Addr::LOCALHOST,
                    },
                    port: rpc_endpoint.port(),
                    protocol,
                });
                msg.add_entry(SdEntry::offer_service(
                    key.service_id,
                    key.instance_id,
                    major_version,
                    minor_version,
                    ttl,
                    opt_idx,
                    1,
                ));

                actions.push(Action::SendSd {
                    message: msg,
                    target: sd_multicast,
                });
            } else {
                let _ = response.send(Err(Error::ServiceUnavailable));
            }
        }

        // StopAnnouncing: Stop SD announcements but keep socket open
        Command::StopAnnouncing {
            service_id,
            instance_id,
            response,
        } => {
            let key = ServiceKey::new(service_id, instance_id);

            // First, read-only access to extract what we need
            let sd_flags = state.sd_flags(false);
            let sd_multicast = state.config.sd_multicast;

            if let Some(offered) = state.offered.get_mut(&key) {
                // Mark as not announcing
                offered.is_announcing = false;

                // Extract what we need
                let major_version = offered.major_version;
                let minor_version = offered.minor_version;

                // Send StopOfferService
                let mut msg = SdMessage::new(sd_flags);
                msg.add_entry(SdEntry::stop_offer_service(
                    service_id.value(),
                    instance_id.value(),
                    major_version,
                    minor_version,
                ));

                actions.push(Action::SendSd {
                    message: msg,
                    target: sd_multicast,
                });

                let _ = response.send(Ok(()));
            } else {
                let _ = response.send(Err(Error::ServiceUnavailable));
            }
        }

        // FindStatic: Immediately mark service as available at given endpoint
        Command::FindStatic {
            service_id: _,
            instance_id,
            endpoint,
            notify,
        } => {
            // Immediately notify that service is available at the given endpoint
            let _ = notify.try_send(ServiceAvailability::Available {
                endpoint,
                instance_id: instance_id.value(),
            });
        }

        // HasSubscribers: Query if there are subscribers for an eventgroup
        Command::HasSubscribers {
            service_id,
            instance_id,
            eventgroup_id,
            response,
        } => {
            let sub_key = SubscriberKey {
                service_id: service_id.value(),
                instance_id: instance_id.value(),
                eventgroup_id,
            };
            let has_subscribers = state
                .server_subscribers
                .get(&sub_key)
                .map(|v| !v.is_empty())
                .unwrap_or(false);
            let _ = response.send(has_subscribers);
        }
    }

    if actions.is_empty() {
        None
    } else {
        Some(actions)
    }
}

/// Handle periodic tasks
fn handle_periodic(state: &mut RuntimeState) -> Option<Vec<Action>> {
    let mut actions = Vec::new();
    let now = Instant::now();
    let offer_interval = Duration::from_millis(state.config.cyclic_offer_delay);
    let sd_flags_unicast = state.sd_flags(true);
    let ttl = state.config.ttl;
    let sd_multicast = state.config.sd_multicast;

    // Determine protocol based on transport configuration
    let protocol = match state.config.transport {
        Transport::Tcp => L4Protocol::Tcp,
        Transport::Udp => L4Protocol::Udp,
    };

    // Cyclic offers (only for services that are announcing)
    for (key, offered) in &mut state.offered {
        // Skip services that are bound but not announcing
        if !offered.is_announcing {
            continue;
        }

        if now.duration_since(offered.last_offer) >= offer_interval {
            offered.last_offer = now;

            let mut msg = SdMessage::new(sd_flags_unicast);
            let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
                addr: match offered.rpc_endpoint {
                    SocketAddr::V4(v4) => *v4.ip(),
                    _ => Ipv4Addr::LOCALHOST,
                },
                port: offered.rpc_endpoint.port(), // Use RPC endpoint port, not SD port!
                protocol,
            });
            msg.add_entry(SdEntry::offer_service(
                key.service_id,
                key.instance_id,
                offered.major_version,
                offered.minor_version,
                ttl,
                opt_idx,
                1,
            ));

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

            let mut msg = SdMessage::new(sd_flags_unicast);
            msg.add_entry(SdEntry::find_service(
                key.service_id,
                key.instance_id,
                0xFF,
                0xFFFFFFFF,
                ttl,
            ));

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
        }
    }

    if actions.is_empty() {
        None
    } else {
        Some(actions)
    }
}

/// Send StopOffer for all offered services (on shutdown)
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

// ============================================================================
// UNIT TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// feat_req_recentip_677: Session ID wraps from 0xFFFF to 0x0001
    /// feat_req_recentip_649: Session ID starts at 0x0001
    ///
    /// Session ID 0x0000 is reserved for "session handling disabled" and must never be used.
    #[test]
    fn session_id_wraps_to_0001_not_0000() {
        let addr = "127.0.0.1:30490".parse().unwrap();
        let client_rpc_addr = "127.0.0.1:49152".parse().unwrap();
        let (client_rpc_tx, _) = mpsc::channel(1);
        let mut state = RuntimeState::new(addr, client_rpc_addr, client_rpc_tx, RuntimeConfig::default());

        // First session should be 1
        assert_eq!(state.next_session_id(), 1, "Session ID should start at 1");

        // Iterate through all possible session IDs
        for expected in 2..=0xFFFFu16 {
            let id = state.next_session_id();
            assert_eq!(id, expected, "Session ID should be {}", expected);
            assert_ne!(id, 0, "Session ID should never be 0");
        }

        // After 0xFFFF, should wrap to 1 (not 0)
        let wrapped = state.next_session_id();
        assert_eq!(wrapped, 1, "Session ID should wrap to 1 after 0xFFFF");

        // Continue a few more to verify
        assert_eq!(state.next_session_id(), 2);
        assert_eq!(state.next_session_id(), 3);
    }

    /// feat_req_recentipsd_41: Reboot flag is cleared after first wraparound
    #[test]
    fn reboot_flag_clears_after_wraparound() {
        let addr = "127.0.0.1:30490".parse().unwrap();
        let client_rpc_addr = "127.0.0.1:49152".parse().unwrap();
        let (client_rpc_tx, _) = mpsc::channel(1);
        let mut state = RuntimeState::new(addr, client_rpc_addr, client_rpc_tx, RuntimeConfig::default());

        // Initially reboot flag should be set
        assert!(state.reboot_flag, "Reboot flag should be true initially");
        assert!(
            !state.has_wrapped_once,
            "has_wrapped_once should be false initially"
        );

        // Iterate through all session IDs until wraparound
        for _ in 1..=0xFFFFu16 {
            state.next_session_id();
        }

        // After wraparound, reboot flag should be cleared
        assert!(
            !state.reboot_flag,
            "Reboot flag should be false after wraparound"
        );
        assert!(
            state.has_wrapped_once,
            "has_wrapped_once should be true after wraparound"
        );

        // Second wraparound should not change anything
        for _ in 1..=0xFFFFu16 {
            state.next_session_id();
        }
        assert!(
            !state.reboot_flag,
            "Reboot flag should stay false after second wraparound"
        );
    }

    /// Session ID never returns 0
    #[test]
    fn session_id_never_zero() {
        let addr = "127.0.0.1:30490".parse().unwrap();
        let client_rpc_addr = "127.0.0.1:49152".parse().unwrap();
        let (client_rpc_tx, _) = mpsc::channel(1);
        let mut state = RuntimeState::new(addr, client_rpc_addr, client_rpc_tx, RuntimeConfig::default());

        // Iterate through 2 full cycles + some extra
        for _ in 0..(0xFFFF * 2 + 1000) {
            let id = state.next_session_id();
            assert_ne!(id, 0, "Session ID must never be 0");
        }
    }
}
