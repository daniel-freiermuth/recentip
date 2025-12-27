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
use crate::handle::{OfferingHandle, ProxyHandle, Unavailable};
use crate::net::UdpSocket;
use crate::wire::{Header, L4Protocol, MessageType, SdEntry, SdEntryType, SdMessage, SdOption, PROTOCOL_VERSION, SD_METHOD_ID, SD_SERVICE_ID};
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
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            // Use the SD port for multicast group membership to work in turmoil
            local_addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_SD_PORT)),
            sd_multicast: SocketAddr::V4(SocketAddrV4::new(DEFAULT_SD_MULTICAST, DEFAULT_SD_PORT)),
            ttl: DEFAULT_TTL,
            cyclic_offer_delay: DEFAULT_CYCLIC_OFFER_DELAY,
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

    /// Build the configuration
    pub fn build(self) -> RuntimeConfig {
        self.config
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
    /// Shutdown the runtime
    #[allow(dead_code)]
    Shutdown,
}

/// Service availability notification
#[derive(Debug, Clone)]
pub(crate) enum ServiceAvailability {
    Available { endpoint: SocketAddr },
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
            && (self.instance_id == 0xFFFF || self.instance_id == instance_id || instance_id == 0xFFFF)
    }
}

/// Tracked offered service (our offerings)
struct OfferedService {
    major_version: u8,
    minor_version: u32,
    requests_tx: mpsc::Sender<ServiceRequest>,
    last_offer: Instant,
}

/// Tracked find request
struct FindRequest {
    notify: mpsc::Sender<ServiceAvailability>,
    repetitions_left: u32,
    last_find: Instant,
}

/// Discovered remote service
#[derive(Debug, Clone)]
#[allow(dead_code)]  // Fields used for version matching in future
struct DiscoveredService {
    endpoint: SocketAddr,
    major_version: u8,
    minor_version: u32,
    ttl_expires: Instant,
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
#[derive(Debug)]
struct PendingServerResponse {
    service_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    interface_version: u8,
    client_addr: SocketAddr,
}

/// Runtime state managed by the runtime task
struct RuntimeState {
    /// Our local endpoint for receiving RPC
    local_endpoint: SocketAddr,
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
}

impl RuntimeState {
    fn new(local_endpoint: SocketAddr, config: RuntimeConfig) -> Self {
        // Use port as part of client_id to help with uniqueness
        let client_id = (local_endpoint.port() % 0xFFFE) + 1;
        Self {
            local_endpoint,
            offered: HashMap::new(),
            find_requests: HashMap::new(),
            discovered: HashMap::new(),
            subscriptions: HashMap::new(),
            server_subscribers: HashMap::new(),
            pending_calls: HashMap::new(),
            client_id,
            session_id: 1,
            reboot_flag: true,
            has_wrapped_once: false,
            config,
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
pub struct Runtime<U: UdpSocket = tokio::net::UdpSocket> {
    inner: Arc<RuntimeInner>,
    _phantom: std::marker::PhantomData<U>,
}

impl Runtime<tokio::net::UdpSocket> {
    /// Create a new runtime with the given configuration.
    ///
    /// This binds to the configured local address and joins the SD multicast group.
    pub async fn new(config: RuntimeConfig) -> Result<Self> {
        Self::with_socket_type(config).await
    }
}

impl<U: UdpSocket> Runtime<U> {
    /// Create a new runtime with a specific socket type.
    ///
    /// This is mainly useful for testing with turmoil.
    pub async fn with_socket_type(config: RuntimeConfig) -> Result<Self> {
        // Bind the SD socket
        let sd_socket = U::bind(config.local_addr).await?;
        let local_addr = sd_socket.local_addr()?;

        // Join multicast group
        if let SocketAddr::V4(addr) = config.sd_multicast {
            sd_socket.join_multicast_v4(*addr.ip(), Ipv4Addr::UNSPECIFIED)?;
        }

        // Create command channel
        let (cmd_tx, cmd_rx) = mpsc::channel(64);

        // Spawn the runtime task
        let inner = Arc::new(RuntimeInner { cmd_tx });

        let state = RuntimeState::new(local_addr, config.clone());

        tokio::spawn(async move {
            runtime_task(sd_socket, config, cmd_rx, state).await;
        });

        Ok(Self {
            inner,
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
    pub async fn offer<S: Service>(&self, instance: InstanceId) -> Result<OfferingHandle<S>> {
        let service_id = ServiceId::new(S::SERVICE_ID).expect("Invalid service ID");

        let (response_tx, response_rx) = oneshot::channel();

        self.inner
            .cmd_tx
            .send(Command::Offer {
                service_id,
                instance_id: instance,
                major_version: S::MAJOR_VERSION,
                minor_version: S::MINOR_VERSION,
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
}

impl<U: UdpSocket> Clone for Runtime<U> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            _phantom: std::marker::PhantomData,
        }
    }
}

// ============================================================================
// RUNTIME TASK
// ============================================================================

/// The main runtime task
async fn runtime_task<U: UdpSocket>(
    sd_socket: U,
    config: RuntimeConfig,
    mut cmd_rx: mpsc::Receiver<Command>,
    mut state: RuntimeState,
) {
    let mut buf = [0u8; 65535];
    let mut ticker = interval(Duration::from_millis(config.cyclic_offer_delay));
    
    // Track pending server responses
    let mut pending_responses: FuturesUnordered<std::pin::Pin<Box<dyn std::future::Future<Output = (PendingServerResponse, Result<Bytes>)> + Send>>> = FuturesUnordered::new();

    loop {
        tokio::select! {
            // Handle incoming packets (SD or RPC)
            result = sd_socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, from)) => {
                        let data = &buf[..len];
                        if let Some(actions) = handle_packet(data, from, &mut state) {
                            for action in actions {
                                execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses).await;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error receiving packet: {}", e);
                    }
                }
            }

            // Handle commands from handles
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(Command::Shutdown) | None => {
                        tracing::info!("Runtime shutting down");
                        // Send StopOffer for all offered services
                        send_stop_offers(&sd_socket, &config, &mut state).await;
                        break;
                    }
                    Some(cmd) => {
                        if let Some(actions) = handle_command(cmd, &mut state) {
                            for action in actions {
                                execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses).await;
                            }
                        }
                    }
                }
            }

            // Periodic tasks (cyclic offers, find retries)
            _ = ticker.tick() => {
                if let Some(actions) = handle_periodic(&mut state) {
                    for action in actions {
                        execute_action(&sd_socket, &config, &mut state, action, &mut pending_responses).await;
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
                    ),
                    Err(_) => build_response(
                        context.service_id,
                        context.method_id,
                        context.client_id,
                        context.session_id,
                        context.interface_version,
                        0x01, // NOT_OK
                        &[],
                    ),
                };
                if let Err(e) = sd_socket.send_to(&response_data, context.client_addr).await {
                    tracing::error!("Failed to send response: {}", e);
                }
            }
        }
    }
}

/// Action to execute after handling an event
enum Action {
    /// Send an SD message to a specific target
    SendSd { message: SdMessage, target: SocketAddr },
    /// Notify find requests about a discovered service
    NotifyFound { key: ServiceKey, availability: ServiceAvailability },
    /// Send a SOME/IP message
    SendMessage { data: Bytes, target: SocketAddr },
    /// Track a pending server response
    TrackServerResponse {
        context: PendingServerResponse,
        receiver: oneshot::Receiver<Result<Bytes>>,
    },
}

/// Execute an action
async fn execute_action<U: UdpSocket>(
    sd_socket: &U,
    _config: &RuntimeConfig,
    state: &mut RuntimeState,
    action: Action,
    pending_responses: &mut FuturesUnordered<std::pin::Pin<Box<dyn std::future::Future<Output = (PendingServerResponse, Result<Bytes>)> + Send>>>,
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
        Action::SendMessage { data, target } => {
            if let Err(e) = sd_socket.send_to(&data, target).await {
                tracing::error!("Failed to send SOME/IP message: {}", e);
            }
        }
        Action::TrackServerResponse { context, receiver } => {
            let fut = Box::pin(async move {
                let result = receiver.await
                    .unwrap_or_else(|_| Err(Error::RuntimeShutdown));
                (context, result)
            });
            pending_responses.push(fut);
        }
    }
}

/// Handle an incoming packet (SD or RPC)
fn handle_packet(data: &[u8], from: SocketAddr, state: &mut RuntimeState) -> Option<Vec<Action>> {
    let mut cursor = data;

    // Parse SOME/IP header
    let header = Header::parse(&mut cursor)?;

    // Check if it's an SD message
    if header.service_id == SD_SERVICE_ID && header.method_id == SD_METHOD_ID {
        return handle_sd_message(&header, &mut cursor, from, state);
    }

    // Handle RPC messages
    handle_rpc_message(&header, &mut cursor, from, state)
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

    tracing::trace!("Received SD message from {} with {} entries", from, sd_message.entries.len());

    let mut actions = Vec::new();

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
                    handle_unsubscribe_request(entry, from, state);
                } else {
                    handle_subscribe_request(entry, from, state, &mut actions);
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
) -> Option<Vec<Action>> {
    let payload_len = header.payload_length();
    if cursor.remaining() < payload_len {
        return None;
    }
    let payload = cursor.copy_to_bytes(payload_len);

    let mut actions = Vec::new();

    match header.message_type {
        MessageType::Request => {
            // Incoming request - route to offering
            handle_incoming_request(header, payload, from, state, &mut actions);
        }
        MessageType::Response | MessageType::Error => {
            // Response to our request - route to pending call
            handle_incoming_response(header, payload, state);
        }
        MessageType::Notification => {
            // Event notification - route to subscription
            handle_incoming_notification(header, payload, state);
        }
        _ => {
            tracing::trace!("Ignoring message type {:?} from {}", header.message_type, from);
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
) {
    // Find matching offering
    let offering = state.offered.iter().find(|(k, _)| {
        k.service_id == header.service_id
    });

    if let Some((_, offered)) = offering {
        // Create a response channel
        let (response_tx, response_rx) = oneshot::channel();
        
        // Send request to the offering handle
        if offered.requests_tx.try_send(ServiceRequest::MethodCall {
            method_id: header.method_id,
            payload,
            client: from,
            response: response_tx,
        }).is_ok() {
            // Track this pending response - will be polled in the main loop
            let context = PendingServerResponse {
                service_id: header.service_id,
                method_id: header.method_id,
                client_id: header.client_id,
                session_id: header.session_id,
                interface_version: header.interface_version,
                client_addr: from,
            };
            actions.push(Action::TrackServerResponse { context, receiver: response_rx });
        }
    } else {
        // Unknown service - send error response
        let response_data = build_response(
            header.service_id,
            header.method_id,
            header.client_id,
            header.session_id,
            header.interface_version,
            0x02, // UNKNOWN_SERVICE
            &[],
        );
        actions.push(Action::SendMessage {
            data: response_data,
            target: from,
        });
    }
}

/// Handle an incoming response (client-side)
fn handle_incoming_response(
    header: &Header,
    payload: Bytes,
    state: &mut RuntimeState,
) {
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
    state: &mut RuntimeState,
) {
    // Find subscriptions for this service/eventgroup
    for (key, subs) in &state.subscriptions {
        if key.service_id == header.service_id {
            // Method ID is the event ID for notifications
            let event_id = crate::EventId::new(header.method_id);
            
            if let Some(event_id) = event_id {
                let event = crate::Event {
                    event_id,
                    payload: payload.clone(),
                };

                for sub in subs {
                    let _ = sub.events_tx.try_send(event.clone());
                }
            }
        }
    }
}

/// Build a SOME/IP response message
fn build_response(
    service_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    interface_version: u8,
    return_code: u8,
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
        message_type: MessageType::Response,
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
    // Get endpoint from SD option, falling back to source address
    // If the endpoint has unspecified IP (0.0.0.0), use the source IP with the option's port
    let endpoint = match sd_message.get_udp_endpoint(entry) {
        Some(ep) if !ep.ip().is_unspecified() => ep,
        Some(ep) => SocketAddr::new(from.ip(), ep.port()),
        None => from,
    };

    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
    };

    let ttl_duration = Duration::from_secs(entry.ttl as u64);

    tracing::debug!(
        "Discovered service {:04x}:{:04x} at {} (TTL={})",
        entry.service_id,
        entry.instance_id,
        endpoint,
        entry.ttl
    );

    let is_new = !state.discovered.contains_key(&key);
    state.discovered.insert(
        key,
        DiscoveredService {
            endpoint,
            major_version: entry.major_version,
            minor_version: entry.minor_version,
            ttl_expires: Instant::now() + ttl_duration,
        },
    );

    if is_new {
        actions.push(Action::NotifyFound {
            key,
            availability: ServiceAvailability::Available { endpoint },
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
    for (key, offered) in &state.offered {
        if entry.service_id == key.service_id
            && (entry.instance_id == 0xFFFF || entry.instance_id == key.instance_id)
        {
            let mut response = SdMessage::new(state.sd_flags(true));
            let opt_idx = response.add_option(SdOption::Ipv4Endpoint {
                addr: match state.local_endpoint {
                    SocketAddr::V4(v4) => *v4.ip(),
                    _ => Ipv4Addr::LOCALHOST,
                },
                port: state.local_endpoint.port(),
                protocol: L4Protocol::Udp,
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
    from: SocketAddr,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
    };

    if let Some(offered) = state.offered.get(&key) {
        let (response_tx, _response_rx) = oneshot::channel();
        let _ = offered.requests_tx.try_send(ServiceRequest::Subscribe {
            eventgroup_id: entry.eventgroup_id,
            client: from,
            response: response_tx,
        });

        // Track the subscriber
        let sub_key = SubscriberKey {
            service_id: entry.service_id,
            instance_id: entry.instance_id,
            eventgroup_id: entry.eventgroup_id,
        };
        state.server_subscribers
            .entry(sub_key)
            .or_insert_with(Vec::new)
            .push(from);

        let mut ack = SdMessage::new(state.sd_flags(true));
        ack.add_entry(SdEntry::subscribe_eventgroup_ack(
            entry.service_id,
            entry.instance_id,
            entry.major_version,
            entry.eventgroup_id,
            state.config.ttl,
            entry.counter,
        ));

        actions.push(Action::SendSd {
            message: ack,
            target: from,
        });
    }
}

/// Handle a StopSubscribeEventgroup request
fn handle_unsubscribe_request(entry: &SdEntry, from: SocketAddr, state: &mut RuntimeState) {
    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
    };

    if let Some(offered) = state.offered.get(&key) {
        let _ = offered.requests_tx.try_send(ServiceRequest::Unsubscribe {
            eventgroup_id: entry.eventgroup_id,
            client: from,
        });

        // Remove the subscriber
        let sub_key = SubscriberKey {
            service_id: entry.service_id,
            instance_id: entry.instance_id,
            eventgroup_id: entry.eventgroup_id,
        };
        if let Some(subscribers) = state.server_subscribers.get_mut(&sub_key) {
            subscribers.retain(|addr| *addr != from);
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

/// Handle a command from a handle
fn handle_command(cmd: Command, state: &mut RuntimeState) -> Option<Vec<Action>> {
    let mut actions = Vec::new();

    match cmd {
        Command::Find { service_id, instance_id, notify } => {
            let key = ServiceKey::new(service_id, instance_id);

            if let Some(discovered) = state.discovered.get(&key) {
                let _ = notify.try_send(ServiceAvailability::Available {
                    endpoint: discovered.endpoint,
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

        Command::StopFind { service_id, instance_id } => {
            let key = ServiceKey::new(service_id, instance_id);
            state.find_requests.remove(&key);
        }

        Command::Offer { service_id, instance_id, major_version, minor_version, response } => {
            let key = ServiceKey::new(service_id, instance_id);

            let (requests_tx, requests_rx) = mpsc::channel(64);

            state.offered.insert(
                key,
                OfferedService {
                    major_version,
                    minor_version,
                    requests_tx,
                    last_offer: Instant::now() - Duration::from_secs(10),
                },
            );

            let mut msg = SdMessage::new(state.sd_flags(true));
            let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
                addr: match state.local_endpoint {
                    SocketAddr::V4(v4) => *v4.ip(),
                    _ => Ipv4Addr::LOCALHOST,
                },
                port: state.local_endpoint.port(),
                protocol: L4Protocol::Udp,
            });
            msg.add_entry(SdEntry::offer_service(
                service_id.value(),
                instance_id.value(),
                major_version,
                minor_version,
                state.config.ttl,
                opt_idx,
                1,
            ));

            actions.push(Action::SendSd {
                message: msg,
                target: state.config.sd_multicast,
            });

            let _ = response.send(Ok(requests_rx));
        }

        Command::StopOffer { service_id, instance_id } => {
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

        Command::Call { service_id, instance_id, method_id, payload, response } => {
            let key = ServiceKey::new(service_id, instance_id);

                // Find the discovered service (try exact match first, then any instance)
            let endpoint = state.discovered.get(&key).map(|d| d.endpoint).or_else(|| {
                    // If searching for Any, find any instance of this service
                    if instance_id.is_any() {
                        state.discovered.iter()
                            .find(|(k, _)| k.service_id == service_id.value())
                            .map(|(_, v)| v.endpoint)
                    } else {
                        None
                    }
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
                let call_key = CallKey { client_id, session_id };
                state.pending_calls.insert(call_key, PendingCall { response });

                actions.push(Action::SendMessage {
                    data: request_data,
                    target: endpoint,
                });
            } else {
                let _ = response.send(Err(Error::ServiceUnavailable));
            }
        }

        Command::Subscribe { service_id, instance_id, eventgroup_id, events, response } => {
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

                let mut msg = SdMessage::new(state.sd_flags(true));
                let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
                    addr: match state.local_endpoint {
                        SocketAddr::V4(v4) => *v4.ip(),
                        _ => Ipv4Addr::LOCALHOST,
                    },
                    port: state.local_endpoint.port(),
                    protocol: L4Protocol::Udp,
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
                    target: discovered.endpoint,
                });

                let _ = response.send(Ok(()));
            } else {
                let _ = response.send(Err(Error::ServiceUnavailable));
            }
        }

        Command::Unsubscribe { service_id, instance_id, eventgroup_id } => {
            let key = ServiceKey::new(service_id, instance_id);

            if let Some(subs) = state.subscriptions.get_mut(&key) {
                subs.retain(|s| s.eventgroup_id != eventgroup_id);
            }

            if let Some(discovered) = state.discovered.get(&key) {
                let mut msg = SdMessage::new(state.sd_flags(true));
                msg.add_entry(SdEntry::subscribe_eventgroup(
                    service_id.value(),
                    instance_id.value(),
                    0xFF,
                    eventgroup_id,
                    0,
                    0,
                ));

                actions.push(Action::SendSd {
                    message: msg,
                    target: discovered.endpoint,
                });
            }
        }

        Command::Notify { service_id, instance_id, eventgroup_id, event_id, payload } => {
            // Find all subscribers for this eventgroup
            let sub_key = SubscriberKey {
                service_id: service_id.value(),
                instance_id: instance_id.value(),
                eventgroup_id,
            };

            // Clone subscribers to avoid borrow conflict with next_session_id
            let subscribers: Vec<SocketAddr> = state.server_subscribers
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
                    actions.push(Action::SendMessage {
                        data: notification_data.clone(),
                        target: subscriber,
                    });
                }
            }
        }

        Command::Shutdown => {}
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
    let local_endpoint = state.local_endpoint;
    let ttl = state.config.ttl;
    let sd_multicast = state.config.sd_multicast;

    // Cyclic offers
    for (key, offered) in &mut state.offered {
        if now.duration_since(offered.last_offer) >= offer_interval {
            offered.last_offer = now;

            let mut msg = SdMessage::new(sd_flags_unicast);
            let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
                addr: match local_endpoint {
                    SocketAddr::V4(v4) => *v4.ip(),
                    _ => Ipv4Addr::LOCALHOST,
                },
                port: local_endpoint.port(),
                protocol: L4Protocol::Udp,
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
        if find_req.repetitions_left > 0 && now.duration_since(find_req.last_find) >= find_interval {
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
        let mut state = RuntimeState::new(addr, RuntimeConfig::default());

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
        let mut state = RuntimeState::new(addr, RuntimeConfig::default());

        // Initially reboot flag should be set
        assert!(state.reboot_flag, "Reboot flag should be true initially");
        assert!(!state.has_wrapped_once, "has_wrapped_once should be false initially");

        // Iterate through all session IDs until wraparound
        for _ in 1..=0xFFFFu16 {
            state.next_session_id();
        }

        // After wraparound, reboot flag should be cleared
        assert!(!state.reboot_flag, "Reboot flag should be false after wraparound");
        assert!(state.has_wrapped_once, "has_wrapped_once should be true after wraparound");

        // Second wraparound should not change anything
        for _ in 1..=0xFFFFu16 {
            state.next_session_id();
        }
        assert!(!state.reboot_flag, "Reboot flag should stay false after second wraparound");
    }

    /// Session ID never returns 0
    #[test]
    fn session_id_never_zero() {
        let addr = "127.0.0.1:30490".parse().unwrap();
        let mut state = RuntimeState::new(addr, RuntimeConfig::default());

        // Iterate through 2 full cycles + some extra
        for _ in 0..(0xFFFF * 2 + 1000) {
            let id = state.next_session_id();
            assert_ne!(id, 0, "Session ID must never be 0");
        }
    }
}
