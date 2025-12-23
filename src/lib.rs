//! # someip-runtime
//!
//! A type-safe, platform-agnostic SOME/IP protocol implementation.
//!
//! ## Quick Start
//!
//! ### Mixed Mode (Client + Server)
//!
//! ```rust,ignore
//! use someip_runtime::{Runtime, RuntimeConfig, ServiceId, InstanceId, EventgroupId};
//!
//! let mut runtime = Runtime::new(io_context, config)?;
//!
//! // Offer our service
//! let offering = runtime.offer(service_config)?;
//!
//! // Require a remote service
//! let proxy = runtime.require(ServiceId::new(0x5678)?, InstanceId::ANY);
//!
//! // Event loop
//! loop {
//!     match runtime.poll()? {
//!         RuntimeEvent::ServiceEvent { service, event } => {
//!             // Handle incoming requests to our offered services
//!             match event {
//!                 ServiceEvent::MethodCall { request } => {
//!                     request.responder.send_ok(&response)?;
//!                 }
//!                 ServiceEvent::Subscribe { ack, .. } => {
//!                     ack.accept()?;
//!                 }
//!                 _ => {}
//!             }
//!         }
//!         RuntimeEvent::ProxyAvailable { service } => {
//!             // A required service became available
//!         }
//!         RuntimeEvent::ProxyUnavailable { service } => {
//!             // A required service went away
//!         }
//!         RuntimeEvent::Timeout => {
//!             // No events pending
//!         }
//!     }
//! }
//! ```
//!
//! ### Client-Only Example
//!
//! ```rust,ignore
//! let mut runtime = Runtime::new(io_context, config)?;
//!
//! let proxy = runtime.require(ServiceId::new(0x1234)?, InstanceId::ANY);
//! let available = proxy.wait_available()?;
//!
//! let mut subscription = available.subscribe(EventgroupId::new(0x01)?)?;
//! while let Some(event) = subscription.next_event()? {
//!     println!("Received: {:?}", event);
//! }
//! ```
//!
//! ### Server-Only Example
//!
//! ```rust,ignore
//! let mut runtime = Runtime::new(io_context, config)?;
//!
//! let mut offering = runtime.offer(service_config)?;
//!
//! loop {
//!     match offering.next()? {
//!         ServiceEvent::MethodCall { request } => {
//!             request.responder.send_ok(&response)?;
//!         }
//!         _ => {}
//!     }
//! }
//! ```

// ============================================================================
// PROTOCOL IDENTIFIERS
// ============================================================================

/// Service identifier (0x0001-0xFFFE valid, 0x0000 and 0xFFFF reserved)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServiceId(u16);

impl ServiceId {
    /// Create a new ServiceId. Returns None for reserved values.
    pub fn new(id: u16) -> Option<Self> {
        match id {
            0x0000 | 0xFFFF => None,
            id => Some(Self(id)),
        }
    }

    /// Get the raw value
    pub fn value(&self) -> u16 {
        self.0
    }
}

/// Instance identifier for servers (0x0001-0xFFFE, no wildcards)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConcreteInstanceId(u16);

impl ConcreteInstanceId {
    /// Create a concrete instance ID. Returns None for reserved/wildcard values.
    pub fn new(id: u16) -> Option<Self> {
        match id {
            0x0000 | 0xFFFF => None,
            id => Some(Self(id)),
        }
    }

    pub fn value(&self) -> u16 {
        self.0
    }
}

/// Instance identifier for clients (allows 0xFFFF for ANY)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InstanceId(u16);

impl InstanceId {
    /// Match any instance
    pub const ANY: Self = Self(0xFFFF);

    /// Create a specific instance ID
    pub fn new(id: u16) -> Option<Self> {
        match id {
            0x0000 => None,
            id => Some(Self(id)),
        }
    }

    pub fn value(&self) -> u16 {
        self.0
    }
}

impl From<ConcreteInstanceId> for InstanceId {
    fn from(id: ConcreteInstanceId) -> Self {
        Self(id.0)
    }
}

/// Method identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MethodId(u16);

impl MethodId {
    pub fn new(id: u16) -> Self {
        Self(id)
    }

    pub fn value(&self) -> u16 {
        self.0
    }
}

/// Event identifier (high bit set for events: 0x8000-0xFFFE)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventId(u16);

impl EventId {
    pub fn new(id: u16) -> Option<Self> {
        if id >= 0x8000 && id <= 0xFFFE {
            Some(Self(id))
        } else {
            None
        }
    }

    pub fn value(&self) -> u16 {
        self.0
    }
}

/// Eventgroup identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventgroupId(u16);

impl EventgroupId {
    pub fn new(id: u16) -> Option<Self> {
        match id {
            0x0000 => None,
            id => Some(Self(id)),
        }
    }

    pub fn value(&self) -> u16 {
        self.0
    }
}

/// Application port (cannot be 30490 - reserved for SD)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppPort(u16);

impl AppPort {
    /// Create an application port. Returns None for SD reserved port.
    pub fn new(port: u16) -> Option<Self> {
        if port == 30490 {
            None
        } else {
            Some(Self(port))
        }
    }

    pub fn value(&self) -> u16 {
        self.0
    }
}

// ============================================================================
// RETURN CODES
// ============================================================================

/// SOME/IP return codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ReturnCode {
    Ok = 0x00,
    NotOk = 0x01,
    UnknownService = 0x02,
    UnknownMethod = 0x03,
    NotReady = 0x04,
    NotReachable = 0x05,
    Timeout = 0x06,
    WrongProtocolVersion = 0x07,
    WrongInterfaceVersion = 0x08,
    MalformedMessage = 0x09,
    WrongMessageType = 0x0A,
}

// ============================================================================
// I/O CONTEXT TRAITS
// ============================================================================

/// Result type for I/O operations
pub type IoResult<T> = std::result::Result<T, IoError>;

/// Platform abstraction for network I/O and time
pub trait IoContext {
    type UdpSocket: UdpSocketOps;
    type TcpStream: TcpStreamOps;
    type TcpListener: TcpListenerOps<Stream = Self::TcpStream>;

    fn bind_udp(&self, addr: std::net::SocketAddr) -> IoResult<Self::UdpSocket>;
    fn connect_tcp(&self, addr: std::net::SocketAddr) -> IoResult<Self::TcpStream>;
    fn listen_tcp(&self, addr: std::net::SocketAddr) -> IoResult<Self::TcpListener>;

    fn now(&self) -> std::time::Instant;
    fn sleep(&self, duration: std::time::Duration);
}

pub trait UdpSocketOps {
    fn send_to(&self, buf: &[u8], addr: std::net::SocketAddr) -> IoResult<usize>;
    fn recv_from(&self, buf: &mut [u8]) -> IoResult<(usize, std::net::SocketAddr)>;
    fn try_recv_from(&self, buf: &mut [u8]) -> IoResult<(usize, std::net::SocketAddr)>;
    fn local_addr(&self) -> IoResult<std::net::SocketAddr>;
    fn join_multicast_v4(
        &self,
        group: std::net::Ipv4Addr,
        interface: std::net::Ipv4Addr,
    ) -> IoResult<()>;
    fn leave_multicast_v4(
        &self,
        group: std::net::Ipv4Addr,
        interface: std::net::Ipv4Addr,
    ) -> IoResult<()>;
    fn set_nonblocking(&self, nonblocking: bool) -> IoResult<()>;
}

pub trait TcpStreamOps {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize>;
    fn write(&mut self, buf: &[u8]) -> IoResult<usize>;
    fn flush(&mut self) -> IoResult<()>;
    fn local_addr(&self) -> IoResult<std::net::SocketAddr>;
    fn peer_addr(&self) -> IoResult<std::net::SocketAddr>;
    fn shutdown(&self, how: std::net::Shutdown) -> IoResult<()>;
    fn set_nonblocking(&self, nonblocking: bool) -> IoResult<()>;
}

pub trait TcpListenerOps {
    type Stream: TcpStreamOps;

    fn accept(&self) -> IoResult<(Self::Stream, std::net::SocketAddr)>;
    fn local_addr(&self) -> IoResult<std::net::SocketAddr>;
    fn set_nonblocking(&self, nonblocking: bool) -> IoResult<()>;
}

// ============================================================================
// ERRORS
// ============================================================================

#[derive(Debug)]
pub struct IoError {
    kind: IoErrorKind,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoErrorKind {
    WouldBlock,
    ConnectionReset,
    ConnectionRefused,
    AddrInUse,
    NotConnected,
    TimedOut,
    Other,
}

impl IoError {
    pub fn new(kind: IoErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> IoErrorKind {
        self.kind
    }
}

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for IoError {}

#[derive(Debug)]
pub enum Error {
    Io(IoError),
    Config(ConfigError),
    Protocol(ProtocolError),
    ServiceUnavailable,
    NotSubscribed,
}

#[derive(Debug)]
pub struct ConfigError {
    pub message: String,
}

#[derive(Debug)]
pub struct ProtocolError {
    pub message: String,
}

pub type Result<T> = std::result::Result<T, Error>;

// ============================================================================
// RUNTIME (UNIFIED CLIENT + SERVER)
// ============================================================================

/// Unified SOME/IP runtime - handles both client and server functionality
///
/// One Runtime per application/ECU. It manages:
/// - Service Discovery (SD) participation
/// - Offering services (server role)
/// - Requiring services (client role)
/// - Shared endpoints and network resources
pub struct Runtime<Io: IoContext> {
    _io: Io,
    // ... internal state: SD, endpoints, offerings, proxies
}

impl<Io: IoContext> Runtime<Io> {
    /// Create a new runtime with the given I/O context and configuration
    pub fn new(_io: Io, _config: RuntimeConfig) -> Result<Self> {
        todo!()
    }

    /// Offer a service. Returns a handle to manage the offering.
    pub fn offer(&mut self, _config: ServiceConfig) -> Result<ServiceOffering<Io>> {
        todo!()
    }

    /// Request a service. Returns a proxy in Unavailable state.
    pub fn require(
        &mut self,
        _service: ServiceId,
        _instance: InstanceId,
    ) -> ServiceProxy<Io, Unavailable> {
        todo!()
    }

    /// Poll for runtime events. Non-blocking.
    pub fn poll(&mut self) -> Result<RuntimeEvent> {
        todo!()
    }

    /// Poll with timeout. Blocks up to the given duration.
    pub fn poll_timeout(&mut self, _timeout: std::time::Duration) -> Result<RuntimeEvent> {
        todo!()
    }

    /// Process internal timers and maintenance (SD announcements, TTL, etc.)
    pub fn tick(&mut self) -> Result<()> {
        todo!()
    }
}

/// Runtime configuration
#[derive(Debug, Clone, Default)]
pub struct RuntimeConfig {
    /// Local IP address for this runtime
    pub local_addr: Option<std::net::Ipv4Addr>,
    /// SD multicast group (default: 224.224.224.245)
    pub sd_multicast: Option<std::net::Ipv4Addr>,
    /// SD port (default: 30490)
    pub sd_port: Option<u16>,
    // ... other SD timing parameters
}

impl RuntimeConfig {
    pub fn builder() -> RuntimeConfigBuilder {
        RuntimeConfigBuilder::default()
    }
}

#[derive(Default)]
pub struct RuntimeConfigBuilder {
    config: RuntimeConfig,
}

impl RuntimeConfigBuilder {
    pub fn local_addr(mut self, addr: std::net::Ipv4Addr) -> Self {
        self.config.local_addr = Some(addr);
        self
    }

    pub fn build(self) -> std::result::Result<RuntimeConfig, ConfigError> {
        Ok(self.config)
    }
}

/// Events from the runtime's poll loop
#[derive(Debug)]
pub enum RuntimeEvent {
    /// An event for an offered service
    ServiceEvent {
        /// Which service this event is for
        service: ServiceId,
        /// The actual event
        event: ServiceEvent,
    },
    /// A required service became available
    ProxyAvailable {
        service: ServiceId,
        instance: InstanceId,
    },
    /// A required service is no longer available  
    ProxyUnavailable {
        service: ServiceId,
        instance: InstanceId,
    },
    /// An event was received on a subscription
    SubscriptionEvent {
        eventgroup: EventgroupId,
        event: Event,
    },
    /// A pending response arrived
    ResponseReady {
        // ... response identifier
    },
    /// No events pending (timeout or would block)
    Timeout,
}

// ============================================================================
// SERVICE PROXY (CLIENT-SIDE)
// ============================================================================

/// Marker: service not yet available
pub struct Unavailable;

/// Marker: service is available
pub struct Available;

/// Proxy to a remote service
pub struct ServiceProxy<Io: IoContext, State> {
    _io: std::marker::PhantomData<Io>,
    _state: std::marker::PhantomData<State>,
    service: ServiceId,
    instance: InstanceId,
}

impl<Io: IoContext> ServiceProxy<Io, Unavailable> {
    /// Block until the service becomes available
    pub fn wait_available(self) -> Result<ServiceProxy<Io, Available>> {
        todo!()
    }

    /// Non-blocking check for availability
    pub fn try_available(self) -> std::result::Result<ServiceProxy<Io, Available>, Self> {
        todo!()
    }

    /// Check if available without consuming
    pub fn is_available(&self) -> bool {
        todo!()
    }
}

impl<Io: IoContext> ServiceProxy<Io, Available> {
    /// Subscribe to an eventgroup. Returns the subscription which is the event source.
    pub fn subscribe(self, _eventgroup: EventgroupId) -> Result<Subscription<Io>> {
        todo!()
    }

    /// Call a method (request/response)
    pub fn call(&self, _method: MethodId, _payload: &[u8]) -> Result<PendingResponse<Io>> {
        todo!()
    }

    /// Fire and forget - no response expected
    pub fn fire_and_forget(&self, _method: MethodId, _payload: &[u8]) -> Result<()> {
        todo!()
    }

    /// Get a field value
    pub fn get_field(&self, _field: MethodId) -> Result<PendingResponse<Io>> {
        todo!()
    }

    /// Set a field value
    pub fn set_field(&self, _field: MethodId, _value: &[u8]) -> Result<PendingResponse<Io>> {
        todo!()
    }
}

// ============================================================================
// SUBSCRIPTION (CLIENT-SIDE EVENT SOURCE)
// ============================================================================

/// Active subscription to an eventgroup - this IS the event source
pub struct Subscription<Io: IoContext> {
    _io: std::marker::PhantomData<Io>,
    eventgroup: EventgroupId,
    // ... internal state
}

impl<Io: IoContext> Subscription<Io> {
    /// Get the next event. Blocks until event received or subscription ends.
    pub fn next_event(&mut self) -> Result<Option<Event>> {
        todo!()
    }

    /// Non-blocking event check
    pub fn try_next_event(&mut self) -> Result<Option<Event>> {
        todo!()
    }

    /// Check if subscription is still active
    pub fn is_active(&self) -> bool {
        todo!()
    }

    /// Get the eventgroup this subscription is for
    pub fn eventgroup(&self) -> EventgroupId {
        self.eventgroup
    }

    /// Call a method through this subscription's service
    pub fn call(&self, _method: MethodId, _payload: &[u8]) -> Result<PendingResponse<Io>> {
        todo!()
    }
}

impl<Io: IoContext> Drop for Subscription<Io> {
    fn drop(&mut self) {
        // Sends StopSubscribeEventgroup
    }
}

/// Event received from a subscription
#[derive(Debug, Clone)]
pub struct Event {
    pub event_id: EventId,
    pub payload: Vec<u8>,
}

// ============================================================================
// PENDING RESPONSE (CLIENT-SIDE)
// ============================================================================

/// A pending response to a method call
pub struct PendingResponse<Io: IoContext> {
    _io: std::marker::PhantomData<Io>,
    // ... internal state
}

impl<Io: IoContext> PendingResponse<Io> {
    /// Block until response received
    pub fn wait(self) -> Result<Response> {
        todo!()
    }

    /// Non-blocking check for response
    pub fn try_get(&mut self) -> Result<Option<Response>> {
        todo!()
    }
}

/// Response from a method call
#[derive(Debug, Clone)]
pub struct Response {
    pub return_code: ReturnCode,
    pub payload: Vec<u8>,
}

// ============================================================================
// SERVER API
// ============================================================================

/// Service configuration builder
#[derive(Debug, Clone)]
pub struct ServiceConfig {
    service: ServiceId,
    instance: ConcreteInstanceId,
    major_version: u8,
    minor_version: u32,
    // ... endpoints, eventgroups, etc.
}

impl ServiceConfig {
    pub fn builder() -> ServiceConfigBuilder {
        ServiceConfigBuilder::default()
    }
}

#[derive(Default)]
pub struct ServiceConfigBuilder {
    service: Option<ServiceId>,
    instance: Option<ConcreteInstanceId>,
    major_version: Option<u8>,
    minor_version: Option<u32>,
    // ...
}

impl ServiceConfigBuilder {
    pub fn service(mut self, service: ServiceId) -> Self {
        self.service = Some(service);
        self
    }

    pub fn instance(mut self, instance: ConcreteInstanceId) -> Self {
        self.instance = Some(instance);
        self
    }

    pub fn major_version(mut self, version: u8) -> Self {
        self.major_version = Some(version);
        self
    }

    pub fn minor_version(mut self, version: u32) -> Self {
        self.minor_version = Some(version);
        self
    }

    pub fn endpoint<F>(self, _addr: &str, _configure: F) -> Self
    where
        F: FnOnce(EndpointBuilder) -> EndpointBuilder,
    {
        // TODO: configure endpoint
        self
    }

    pub fn eventgroup(self, _eventgroup: EventgroupId) -> Self {
        // TODO: add eventgroup
        self
    }

    pub fn build(self) -> std::result::Result<ServiceConfig, ConfigError> {
        let service = self.service.ok_or_else(|| ConfigError {
            message: "service is required".into(),
        })?;
        let instance = self.instance.ok_or_else(|| ConfigError {
            message: "instance is required".into(),
        })?;

        Ok(ServiceConfig {
            service,
            instance,
            major_version: self.major_version.unwrap_or(0x01),
            minor_version: self.minor_version.unwrap_or(0x00000000),
        })
    }
}

pub struct EndpointBuilder {
    // ...
}

impl EndpointBuilder {
    pub fn udp(self, _port: AppPort) -> Self {
        self
    }

    pub fn tcp(self, _port: AppPort) -> Self {
        self
    }
}

// ============================================================================
// SERVICE OFFERING
// ============================================================================

/// An active service offering
pub struct ServiceOffering<Io: IoContext> {
    _io: std::marker::PhantomData<Io>,
    service: ServiceId,
    instance: ConcreteInstanceId,
}

impl<Io: IoContext> ServiceOffering<Io> {
    /// Get the next service event. Blocks until event received.
    pub fn next(&mut self) -> Result<ServiceEvent> {
        todo!()
    }

    /// Non-blocking event check
    pub fn try_next(&mut self) -> Result<Option<ServiceEvent>> {
        todo!()
    }

    /// Send an event to all subscribers of an eventgroup
    pub fn notify(
        &self,
        _eventgroup: EventgroupId,
        _event: EventId,
        _payload: &[u8],
    ) -> Result<()> {
        todo!()
    }

    /// Check if anyone is subscribed to an eventgroup
    pub fn has_subscribers(&self, _eventgroup: EventgroupId) -> bool {
        todo!()
    }

    /// Get service ID
    pub fn service(&self) -> ServiceId {
        self.service
    }

    /// Get instance ID
    pub fn instance(&self) -> ConcreteInstanceId {
        self.instance
    }
}

impl<Io: IoContext> Drop for ServiceOffering<Io> {
    fn drop(&mut self) {
        // Sends StopOfferService
    }
}

// ============================================================================
// SERVICE EVENTS (SERVER-SIDE)
// ============================================================================

/// Events received by a service offering
#[derive(Debug)]
pub enum ServiceEvent {
    /// A method was called
    MethodCall { request: MethodRequest },

    /// A fire-and-forget message was received
    FireAndForget { method: MethodId, payload: Vec<u8> },

    /// A client wants to subscribe
    Subscribe {
        eventgroup: EventgroupId,
        client: ClientInfo,
        ack: SubscribeAck,
    },

    /// A client unsubscribed
    Unsubscribe {
        eventgroup: EventgroupId,
        client: ClientInfo,
    },

    /// Get field request
    GetField {
        field: MethodId,
        responder: Responder,
    },

    /// Set field request
    SetField {
        field: MethodId,
        value: Vec<u8>,
        responder: Responder,
    },
}

/// Information about a client
#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub address: std::net::SocketAddr,
}

/// A method request that must be responded to
#[derive(Debug)]
pub struct MethodRequest {
    pub method: MethodId,
    pub payload: Vec<u8>,
    pub responder: Responder,
}

/// Response sender - must be consumed (exactly one response)
#[derive(Debug)]
pub struct Responder {
    sent: bool,
}

impl Responder {
    /// Send a successful response
    pub fn send_ok(mut self, _payload: &[u8]) -> Result<()> {
        self.sent = true;
        todo!()
    }

    /// Send an empty successful response
    pub fn send_ok_empty(mut self) -> Result<()> {
        self.sent = true;
        todo!()
    }

    /// Send an error response
    pub fn send_error(mut self, _code: ReturnCode) -> Result<()> {
        self.sent = true;
        todo!()
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        if !self.sent {
            #[cfg(debug_assertions)]
            panic!("Responder dropped without sending response");

            // In release, could send an error response
        }
    }
}

/// Subscription acknowledgment - must be consumed
#[derive(Debug)]
pub struct SubscribeAck {
    handled: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum RejectReason {
    NotAuthorized,
    ResourceLimit,
    InvalidEventgroup,
    Other,
}

impl SubscribeAck {
    /// Accept the subscription
    pub fn accept(mut self) -> Result<()> {
        self.handled = true;
        todo!()
    }

    /// Reject the subscription
    pub fn reject(mut self, _reason: RejectReason) -> Result<()> {
        self.handled = true;
        todo!()
    }
}

impl Drop for SubscribeAck {
    fn drop(&mut self) {
        if !self.handled {
            #[cfg(debug_assertions)]
            panic!("SubscribeAck dropped without accepting or rejecting");
        }
    }
}

// ============================================================================
// RE-EXPORTS
// ============================================================================

pub mod prelude {
    pub use crate::{
        AppPort, ConcreteInstanceId, Error, Event, EventId, EventgroupId, InstanceId, IoContext,
        IoResult, MethodId, MethodRequest, PendingResponse, RejectReason, Responder, Response,
        Result, ReturnCode, Runtime, RuntimeConfig, RuntimeEvent, ServiceConfig, ServiceEvent,
        ServiceId, ServiceOffering, ServiceProxy, SubscribeAck, Subscription,
    };
}
