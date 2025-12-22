# SOME/IP Rust Library Design Document

## Executive Summary

This document captures the design exploration for a Rust implementation of the SOME/IP protocol. The library aims to be protocol-compliant, type-safe, and usable across platforms from embedded systems to Linux services.

---

## Protocol Values

The SOME/IP specification explicitly states its design goals (from the introduction):

> The basic motivation to specify "yet another RPC-Mechanism" instead of using an existing infrastructure/technology is the goal to have a technology that:
>
> 1. **Fulfills the hard requirements regarding resource consumption in an embedded world.**
> 2. **Is compatible through as many use cases and communication partners as possible.**
> 3. **Is compatible with AUTOSAR** at least on the on-wire format level.
> 4. **Provides the features required by automotive use cases.**
> 5. **Is scalable from tiny to large platforms.**
> 6. **Can be implemented on different operating systems** (e.g. AUTOSAR, GENIVI, and OSEK) **and even embedded devices without operating system.**

These values directly inform our library design.

---

## Target Platforms

| Platform | Requirements |
|----------|--------------|
| **Linux** | Full async support, tokio integration, high performance |
| **QNX** | Sync-first, ionotify/pulse integration, no tokio dependency |
| **Embedded/RTOS** | Minimal dependencies, no_std where possible, explicit resource control |
| **C++ Integration** | Clean FFI, no callbacks across boundary, simple data types |

---

## Design Requirements

### From Protocol Analysis

1. **Endpoints**
   - An endpoint is: IP address + L4 protocol (UDP/TCP) + port number
   - Server can offer up to 1 UDP + 1 TCP endpoint per service instance
   - Different instances of the same service MUST use different ports
   - Different services CAN share the same port

2. **Service Discovery (SD)**
   - SD uses UDP only, typically port 30490
   - Port 30490 is reserved for SD — application traffic MUST NOT use it
   - "Single port pair" efficiency applies to application traffic, not SD

3. **Subscriptions**
   - Events SHALL NOT be sent without active subscriptions
   - SubscribeEventgroup happens AFTER OfferService (service must be available)
   - TCP connection must be established BEFORE subscribing (for reliable events)

4. **Configuration**
   - Port numbers come from configuration files (FIBEX, ARXML, FLYNC)
   - Library should be configuration-source agnostic
   - OEM/system integrator decides port allocation, not the library

### From Usage Analysis

1. **Roles**: Pure Server, Pure Client, Mixed (common in automotive)
2. **Patterns**: Request/Response, Fire & Forget, Publish/Subscribe, Fields
3. **Integration**: Standalone loop, integrated with existing event loop, async

---

## Design Decisions

### 1. Explicit/Pull over Callbacks

**Decision**: Use explicit polling (`next()`) rather than callbacks.

**Rationale**:
- Callbacks require hidden allocations (`Box<dyn Fn>`, `Arc`)
- Callbacks complicate thread safety and re-entrancy
- Callbacks across FFI are complex (context pointers, lifetime issues)
- Observed in other implementations: callbacks just set state, so users end up doing their own queuing anyway
- Explicit style matches Rust ecosystem patterns (`TcpListener::accept()`, channels)

```rust
// Chosen: Explicit
loop {
    match client.next()? {
        Message::Event { .. } => { ... }
        Message::Response { .. } => { ... }
    }
}

// Rejected: Callbacks
client.on_event(|e| { ... });  // Hidden complexity
```

### 2. Sync-First with Async Layer

**Decision**: Core library is synchronous; async is a separate layer on top.

**Rationale**:
- QNX has limited async ecosystem support
- Embedded systems may not have async runtimes
- Sync code is simpler to reason about and debug
- Async can always wrap sync; sync cannot easily wrap async
- Matches protocol's "tiny to large" scalability goal

### 3. Configuration via Structs + Builders

**Decision**: Accept configuration as structs, provide builder pattern, no opinion on source.

**Rationale**:
- Library should not depend on FIBEX/ARXML parsers
- Structs enable serde serialization from any format
- Builder pattern provides ergonomic construction
- Validation happens at `build()` time with clear errors

```rust
let config = ServerConfig::builder()
    .endpoint("192.168.1.10", |e| e.udp(30501))
    .service(0x1234, |s| s.instance(0x0001))
    .build()?;  // Validates here
```

### 4. Type-Safe State Machines

**Decision**: Use Rust's type system to make invalid states unrepresentable.

**Rationale**:
- Protocol has clear state machines (unavailable → available → subscribed)
- Type errors are caught at compile time, not runtime
- API becomes self-documenting
- Follows Rust idiom of "parse, don't validate"

### 5. Subscription as Event Source

**Decision**: Subscription is not just a guard — it IS the event source.

**Rationale**:
- Prevents accidental drop (you're actively using it)
- Makes subscription mandatory for receiving events (type enforced)
- Natural API: "I want events, I use my subscription"
- Drop semantics send StopSubscribeEventgroup automatically

### 6. Silently Drop Unsolicited Events

**Decision**: Events received without active subscription are silently dropped.

**Rationale**:
- Protocol says servers SHALL NOT send unsolicited events
- User explicitly didn't subscribe, so they don't want these
- Simpler API without "Unexpected" variants
- Debug logging available for troubleshooting

---

## Architecture: Layered Crates

```
┌─────────────────────────────────────────────────────────────┐
│ someip-types                                                │
│ no_std, zero dependencies                                   │
│                                                             │
│ • Protocol types: ServiceId, MethodId, EventgroupId, etc.  │
│ • Header parsing/serialization                              │
│ • SD message parsing/serialization                          │
│ • Newtypes with validation                                  │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│ someip-runtime                                              │
│ std, sync, trait-based I/O                                  │
│                                                             │
│ • Client, Server, ServiceProxy, Subscription                │
│ • SD state machines                                         │
│ • Unified poll API: next(), try_next()                     │
│ • I/O traits: UdpSocket, TcpStream                         │
│ • Configuration and validation                              │
└─────────────────────────────────────────────────────────────┘
          │                                    │
          ▼                                    ▼
┌─────────────────────┐          ┌─────────────────────────────┐
│ someip-tokio        │          │ someip-sys                  │
│                     │          │                             │
│ • AsyncClient       │          │ • C FFI bindings            │
│ • AsyncSubscription │          │ • someip_next()             │
│ • Tokio integration │          │ • Simple data types         │
│ • Channel-based     │          │ • No callbacks              │
│   distribution      │          │                             │
└─────────────────────┘          └─────────────────────────────┘
```

### Layer Responsibilities

| Layer | Responsibility | Dependencies | Audience |
|-------|---------------|--------------|----------|
| `someip-types` | Protocol data types | None (no_std) | Everyone |
| `someip-runtime` | Protocol logic, sync I/O | std | Embedded, QNX, FFI |
| `someip-tokio` | Async distribution | tokio | Linux services |
| `someip-sys` | C/C++ FFI | someip-runtime | C++ applications |

---

## I/O Abstraction Layer (IoContext)

The library is completely platform-agnostic. All platform-specific I/O (sockets, time, polling) is abstracted behind the `IoContext` trait. This enables:

- **Portability**: Same library code runs on Linux, QNX, embedded
- **Zero-cost**: Generic parameters enable monomorphization
- **Testing**: Simulated backend with time control, fault injection, introspection

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│ someip-runtime                                              │
│                                                             │
│ Generic over IoContext - knows nothing about:               │
│ • epoll vs kqueue vs ionotify                              │
│ • tokio vs std::net                                         │
│ • real vs simulated network                                 │
│                                                             │
│ Just calls: ctx.bind_udp(), socket.recv_from(), ctx.now()  │
└─────────────────────────────────────────────────────────────┘
                              │
          Uses trait ─────────┴─────────
                              │
    ┌─────────────────────────┼─────────────────────────────────┐
    │                         │                                 │
    ▼                         ▼                                 ▼
┌─────────────┐       ┌───────────────┐       ┌───────────────────────┐
│StdIoContext │       │ QnxIoContext  │       │ SimulatedIoContext    │
│             │       │               │       │                       │
│ std::net    │       │ ionotify      │       │ Virtual network       │
│ poll()      │       │ MsgReceive    │       │ Virtual time          │
│ Instant     │       │ pulses        │       │ Fault injection       │
└─────────────┘       └───────────────┘       └───────────────────────┘
     │                       │                         │
     ▼                       ▼                         ▼
   Linux                    QNX                     Testing
```

### Core Traits

```rust
/// The I/O abstraction - network + time
pub trait IoContext {
    type UdpSocket: UdpSocketOps;
    type TcpStream: TcpStreamOps;
    type TcpListener: TcpListenerOps<Stream = Self::TcpStream>;
    
    /// Bind a UDP socket to a local address
    fn bind_udp(&self, addr: SocketAddr) -> Result<Self::UdpSocket, IoError>;
    
    /// Connect a TCP stream to a remote address
    fn connect_tcp(&self, addr: SocketAddr) -> Result<Self::TcpStream, IoError>;
    
    /// Listen for TCP connections
    fn listen_tcp(&self, addr: SocketAddr) -> Result<Self::TcpListener, IoError>;
    
    /// Current time
    fn now(&self) -> Instant;
    
    /// Sleep/block for duration
    fn sleep(&self, duration: Duration);
}

pub trait UdpSocketOps {
    fn send_to(&self, buf: &[u8], addr: SocketAddr) -> Result<usize, IoError>;
    fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr), IoError>;
    fn local_addr(&self) -> Result<SocketAddr, IoError>;
    
    /// Non-blocking receive
    fn try_recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr), IoError>;
    
    /// Multicast group management
    fn join_multicast_v4(&self, group: Ipv4Addr, interface: Ipv4Addr) -> Result<(), IoError>;
    fn leave_multicast_v4(&self, group: Ipv4Addr, interface: Ipv4Addr) -> Result<(), IoError>;
    
    fn set_nonblocking(&self, nonblocking: bool) -> Result<(), IoError>;
}

pub trait TcpStreamOps {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, IoError>;
    fn write(&mut self, buf: &[u8]) -> Result<usize, IoError>;
    fn flush(&mut self) -> Result<(), IoError>;
    fn local_addr(&self) -> Result<SocketAddr, IoError>;
    fn peer_addr(&self) -> Result<SocketAddr, IoError>;
    fn shutdown(&self, how: Shutdown) -> Result<(), IoError>;
    fn set_nonblocking(&self, nonblocking: bool) -> Result<(), IoError>;
}

pub trait TcpListenerOps {
    type Stream: TcpStreamOps;
    
    fn accept(&self) -> Result<(Self::Stream, SocketAddr), IoError>;
    fn local_addr(&self) -> Result<SocketAddr, IoError>;
    fn set_nonblocking(&self, nonblocking: bool) -> Result<(), IoError>;
}
```

### Library Usage

The library code is generic over `IoContext`:

```rust
impl<Io: IoContext> Runtime<Io> {
    pub fn new(io: Io, config: RuntimeConfig) -> Result<Self> {
        let sd_socket = io.bind_udp(config.sd_addr)?;
        // ...
    }
    
    fn process_sd(&mut self) -> Result<()> {
        // Library doesn't know or care HOW recv works
        let (len, from) = self.sd_socket.recv_from(&mut self.buffer)?;
        // Protocol logic here - completely platform-agnostic
    }
}

// Usage with different backends:
let runtime = Runtime::new(StdIoContext::new(), config)?;           // Linux/std
let runtime = Runtime::new(QnxIoContext::new(), config)?;           // QNX
let runtime = Runtime::new(sim.create_node(addr), config)?;         // Testing
```

### Simulated I/O Backend

The simulated backend enables powerful testing capabilities:

```rust
pub struct SimulatedNetwork {
    time: Cell<SimulatedInstant>,
    nodes: RefCell<HashMap<IpAddr, NodeState>>,
    in_flight: RefCell<BinaryHeap<InFlightPacket>>,
    tcp_connections: RefCell<HashMap<TcpConnectionId, TcpConnectionState>>,
    
    // Fault injection configuration
    config: RefCell<SimConfig>,
    
    // Full history for introspection
    history: RefCell<Vec<NetworkEvent>>,
}

pub struct SimConfig {
    pub latency: Duration,
    pub jitter: Duration,
    pub drop_rate: f64,
    pub partitions: HashSet<(IpAddr, IpAddr)>,
}

impl SimulatedNetwork {
    pub fn new() -> Self { ... }
    
    /// Create a node (ECU) in the simulation
    pub fn create_node(&self, ip: IpAddr) -> SimulatedIoContext<'_>;
}
```

#### Time Control

```rust
impl SimulatedNetwork {
    /// Advance simulation time, delivering packets that become due
    pub fn advance(&self, duration: Duration) {
        let target = self.time.get() + duration;
        
        while let Some(packet) = self.next_due_packet(target) {
            self.time.set(packet.delivery_time);
            self.deliver_packet(packet);
        }
        
        self.time.set(target);
    }
    
    /// Run until no more pending events
    pub fn run_until_idle(&self) {
        while let Some(next_time) = self.next_event_time() {
            self.advance(next_time - self.time.get());
        }
    }
}
```

#### Fault Injection

```rust
impl SimulatedNetwork {
    /// Set packet drop probability (0.0 - 1.0)
    pub fn set_drop_rate(&self, rate: f64);
    
    /// Set network latency
    pub fn set_latency(&self, latency: Duration);
    
    /// Create network partition between two nodes
    pub fn partition(&self, a: IpAddr, b: IpAddr);
    
    /// Heal network partition
    pub fn heal(&self, a: IpAddr, b: IpAddr);
    
    /// Force-break a TCP connection (simulate RST)
    pub fn break_tcp(&self, conn: TcpConnectionId);
}
```

#### Introspection

```rust
/// Events recorded by the simulation
#[derive(Debug, Clone)]
pub enum NetworkEvent {
    PacketSent { time: Instant, from: IpAddr, to: IpAddr, packet: PacketRecord },
    PacketDelivered { time: Instant, from: IpAddr, to: IpAddr },
    PacketDropped { time: Instant, from: IpAddr, to: IpAddr, reason: DropReason },
    TcpConnected { time: Instant, client: SocketAddr, server: SocketAddr },
    TcpClosed { time: Instant, client: SocketAddr, server: SocketAddr },
    TcpReset { time: Instant, client: SocketAddr, server: SocketAddr },
    MulticastJoin { time: Instant, node: IpAddr, group: Ipv4Addr },
    MulticastLeave { time: Instant, node: IpAddr, group: Ipv4Addr },
}

impl SimulatedNetwork {
    /// Get full event history
    pub fn history(&self) -> Vec<NetworkEvent>;
    
    /// Get packets sent by a specific node
    pub fn packets_sent_by(&self, ip: IpAddr) -> Vec<PacketRecord>;
    
    /// Get packets between two nodes
    pub fn packets_between(&self, from: IpAddr, to: IpAddr) -> Vec<PacketRecord>;
    
    /// Get current multicast group members
    pub fn multicast_members(&self, group: Ipv4Addr) -> Vec<IpAddr>;
}
```

#### Simulation Testing Examples

```rust
#[test]
fn test_sd_retries_on_packet_loss() {
    let sim = SimulatedNetwork::new();
    
    let server_io = sim.create_node("192.168.1.10".parse().unwrap());
    let client_io = sim.create_node("192.168.1.20".parse().unwrap());
    
    let mut server = Runtime::new(server_io, server_config())?;
    let mut client = Runtime::new(client_io, client_config())?;
    
    let _offering = server.offer(service_config())?;
    
    // Drop first few SD packets
    sim.set_drop_rate(1.0);
    sim.advance(Duration::from_millis(500));
    
    // Verify service not yet discovered
    let proxy = client.require(ServiceId::new(0x1234).unwrap(), InstanceId::ANY);
    assert!(proxy.try_available().is_err());
    
    // Allow packets through
    sim.set_drop_rate(0.0);
    sim.run_until_idle();
    
    // Now service should be discovered (SD retries worked)
    assert!(proxy.try_available().is_ok());
    
    // Verify SD retry behavior from history
    let sd_packets = sim.packets_sent_by("192.168.1.10".parse().unwrap());
    assert!(sd_packets.len() > 1, "Expected SD retries");
}

#[test]
fn test_subscription_survives_temporary_partition() {
    let sim = SimulatedNetwork::new();
    
    // ... setup server and client ...
    
    let mut subscription = available.subscribe(EventgroupId(0x01))?;
    sim.run_until_idle();
    
    // Create network partition
    sim.partition(server_ip, client_ip);
    
    // Server sends events (will be dropped)
    offering.notify(EventgroupId(0x01), EventId(0x8001), &[1, 2, 3])?;
    sim.advance(Duration::from_secs(1));
    
    // Heal partition
    sim.heal(server_ip, client_ip);
    
    // Next event should work
    offering.notify(EventgroupId(0x01), EventId(0x8001), &[4, 5, 6])?;
    sim.run_until_idle();
    
    let event = subscription.try_next_event()?.expect("should receive event");
    assert_eq!(event.payload, &[4, 5, 6]);
}

#[test]
fn test_tcp_connection_reset_handling() {
    let sim = SimulatedNetwork::new();
    
    // ... setup with TCP subscription ...
    
    // Find the TCP connection from history
    let tcp_conn = sim.history()
        .iter()
        .find_map(|e| match e {
            NetworkEvent::TcpConnected { client, server, .. } => Some((*client, *server)),
            _ => None
        })
        .expect("TCP connection should exist");
    
    // Force connection reset
    sim.break_tcp(tcp_conn);
    sim.run_until_idle();
    
    // Verify client handles it gracefully
    match subscription.next_event() {
        Ok(None) => { /* connection closed cleanly */ }
        Err(e) if e.kind() == ErrorKind::ConnectionReset => { /* expected */ }
        other => panic!("Unexpected result: {:?}", other),
    }
    
    // Verify reset was logged
    assert!(sim.history().iter().any(|e| matches!(e, NetworkEvent::TcpReset { .. })));
}

#[test]
fn test_multicast_delivery() {
    let sim = SimulatedNetwork::new();
    
    let server_io = sim.create_node("192.168.1.10".parse().unwrap());
    let client1_io = sim.create_node("192.168.1.20".parse().unwrap());
    let client2_io = sim.create_node("192.168.1.30".parse().unwrap());
    
    // ... setup subscriptions ...
    
    sim.run_until_idle();
    
    // Verify multicast group membership
    let members = sim.multicast_members("239.192.1.1".parse().unwrap());
    assert!(members.contains(&"192.168.1.20".parse().unwrap()));
    assert!(members.contains(&"192.168.1.30".parse().unwrap()));
    
    // Send multicast event
    offering.notify(EventgroupId(0x01), EventId(0x8001), &[42])?;
    sim.run_until_idle();
    
    // Both clients should receive
    assert!(sub1.try_next_event()?.is_some());
    assert!(sub2.try_next_event()?.is_some());
}
```

### Simulation Capabilities Summary

| Capability | Use Case |
|------------|----------|
| **Time control** | Deterministic tests, fast-forward through delays |
| **Packet loss** | Test retry logic, resilience |
| **Latency** | Test timeout handling |
| **Partitions** | Test split-brain scenarios, reconnection |
| **TCP reset** | Test connection error handling |
| **Multicast tracking** | Verify group membership, delivery |
| **Full history** | Debug test failures, verify timing |

---

## Type-Safe API Design

### Core Principle: Types as Rails

The type system guides users through valid protocol states:

```
"I have a Client"
    → "I can require a service"

"I have a ServiceProxy<Unavailable>"
    → "I must wait for availability"

"I have a ServiceProxy<Available>"
    → "I can subscribe or call methods"

"I have a Subscription"
    → "I can receive events and call methods"

"I have a MethodRequest"
    → "I must respond (responder must be consumed)"
```

### Client-Side Type Flow

```
Client
   │
   └── require() ──► ServiceProxy<Unavailable>
                          │
                          └── wait_available() ──► ServiceProxy<Available>
                                                        │
                                ┌───────────────────────┴────────────────────┐
                                │                                            │
                          subscribe()                                  call()
                                │                                            │
                                ▼                                            ▼
                          Subscription ◄─── events come from here    PendingResponse
                                │
                          next_event() ──► Event
                          call() ──► PendingResponse
```

### Server-Side Type Flow

```
Server
   │
   └── configure() ──► ServiceConfig
                           │
                           └── offer() ──► ServiceOffering
                                               │
                                          next() ──► ServiceEvent
                                               │
                        ┌──────────────────────┼──────────────────────┐
                        │                      │                      │
                  MethodCall             Subscribe              (other)
                        │                      │
                  .responder                 .ack
                        │                      │
                  send_ok()/send_error()   accept()/reject()
                  (must consume)           (must consume)
```

### Compile-Time Enforcement

| Invalid State/Behavior | How Rust Prevents It |
|------------------------|----------------------|
| Subscribe before available | `subscribe()` only on `ServiceProxy<Available>` |
| Get events without subscribing | `next_event()` only on `Subscription` |
| Ignore method request | `Responder` must be consumed (panics on drop) |
| Respond twice | `Responder` moved on first use |
| Ignore subscription request | `SubscribeAck` must be consumed |
| Use reserved Instance ID 0x0000 | `ConcreteInstanceId::new()` returns `None` |
| App traffic on SD port 30490 | `AppPort::new(30490)` returns `None` |
| TCP multicast | Type doesn't exist |

### Validated Newtypes

```rust
pub struct ServiceId(u16);
impl ServiceId {
    pub fn new(id: u16) -> Option<Self> {
        match id {
            0x0000 | 0xFFFF => None,  // Reserved
            id => Some(Self(id))
        }
    }
}

pub struct AppPort(u16);
impl AppPort {
    pub fn new(port: u16) -> Option<Self> {
        if port == 30490 { None } else { Some(Self(port)) }
    }
}

pub struct ConcreteInstanceId(u16);  // For servers: 0x0001-0xFFFE only
pub struct InstanceId(u16);          // For clients: allows 0xFFFF (ANY)
```

---

## Protocol Communication Patterns

The SOME/IP protocol defines specific communication patterns. This section shows how each pattern maps to our library API.

### Pattern 1: Request/Response (RPC)

**Protocol**: Client sends request, server processes, server sends response. Both sides know the call succeeded/failed.

**Use when**: Reading data, performing actions that need confirmation, any operation where the client needs a result.

```rust
// Client side
let available = proxy.wait_available()?;
let pending = available.call(MethodId(0x0001), &request_payload)?;
let response = pending.wait()?;  // Blocks until response
match response {
    Ok(data) => { /* success */ }
    Err(code) => { /* method returned error */ }
}

// Server side
match offering.next()? {
    ServiceEvent::MethodCall { request } => {
        let result = process(&request.payload);
        request.responder.send_ok(&result)?;  // Must respond
    }
}
```

**Type Safety**: `PendingResponse` ensures you don't forget the response. `Responder` must be consumed.

---

### Pattern 2: Fire & Forget

**Protocol**: Client sends message, no response expected. Server processes silently.

**Use when**: Notifications, logging, commands where confirmation isn't needed.

```rust
// Client side
let available = proxy.wait_available()?;
available.fire_and_forget(MethodId(0x0002), &payload)?;
// No PendingResponse - nothing to wait for

// Server side
match offering.next()? {
    ServiceEvent::FireAndForget { method, payload } => {
        process(&payload);
        // No responder - nothing to send back
    }
}
```

**Type Safety**: No `Responder` on fire-and-forget events - impossible to accidentally try to respond.

---

### Pattern 3: Publish/Subscribe (Events)

**Protocol**: Client subscribes to eventgroup. Server sends events to all subscribers when data changes or cyclically.

**Use when**: Sensor data streams, status updates, any data that changes over time and multiple clients need.

```rust
// Client side
let available = proxy.wait_available()?;
let mut subscription = available.subscribe(EventgroupId(0x01))?;

loop {
    match subscription.next_event()? {
        Some(Event { event_id, payload }) => {
            process_sensor_data(&payload);
        }
        None => break,  // Service went away or unsubscribed
    }
}
// subscription drops -> StopSubscribeEventgroup sent automatically

// Server side
match offering.next()? {
    ServiceEvent::Subscribe { eventgroup, ack, .. } => {
        ack.accept()?;  // Must accept or reject
    }
}

// Later, when data changes:
if offering.has_subscribers(EventgroupId(0x01)) {
    offering.notify(EventgroupId(0x01), EventId(0x8001), &sensor_data)?;
}
```

**Type Safety**: 
- Can't receive events without `Subscription` (type enforced)
- Can't subscribe without service available (type enforced)
- Subscription drop auto-unsubscribes (RAII)
- `SubscribeAck` must be consumed

---

### Pattern 4: Fields (Getter/Setter/Notifier)

**Protocol**: A field represents remote state. Can be read (getter), written (setter), and/or watched for changes (notifier). Initial value sent on subscription.

**Use when**: Configuration parameters, status values, any stateful property.

```rust
// Client side: Get current value
let available = proxy.wait_available()?;
let pending = available.get_field(FieldId(0x01))?;
let current_value = pending.wait()?;

// Client side: Set value
let pending = available.set_field(FieldId(0x01), &new_value)?;
pending.wait()?;  // Confirms write succeeded

// Client side: Watch for changes (subscribe to notifier)
let mut subscription = available.subscribe(EventgroupId(0x01))?;
// First event is the initial value
let initial = subscription.next_event()?;

loop {
    match subscription.next_event()? {
        Some(event) => {
            // Field value changed
            update_local_cache(event.payload);
        }
        None => break,
    }
}

// Server side
match offering.next()? {
    ServiceEvent::GetField { field_id, responder } => {
        responder.send_ok(&current_value)?;
    }
    ServiceEvent::SetField { field_id, value, responder } => {
        update_field(field_id, &value)?;
        responder.send_ok_empty()?;
        // Notify subscribers of change
        offering.notify_field(field_id, &value)?;
    }
}
```

**Type Safety**: Same as request/response - responders must be consumed.

---

### Pattern 5: Service Discovery

**Protocol**: Clients find services, servers announce availability. Handles service coming/going dynamically.

**Use when**: Always - this is how clients and servers find each other.

```rust
// Client side: Waiting for a specific service
let proxy = client.require(ServiceId::new(0x1234).unwrap(), InstanceId::ANY);

// Blocking wait
let available = proxy.wait_available()?;

// Or non-blocking poll
loop {
    match proxy.try_available()? {
        Either::Right(available) => {
            // Service found!
            break;
        }
        Either::Left(still_waiting) => {
            // Do other work
            proxy = still_waiting;
        }
    }
}

// Or integrate with poll/select
let fd = proxy.as_raw_fd();
// ... use in poll()

// Server side: Offering a service
let mut offering = runtime.offer(service_config)?;
// SD automatically sends OfferService

// offering drops -> StopOfferService sent automatically
```

**Type Safety**: 
- `ServiceProxy<Unavailable>` → must wait → `ServiceProxy<Available>`
- Can't call methods or subscribe without `Available` (type enforced)

---

### Pattern Summary

| Pattern | Client API | Server API | Key Type Safety |
|---------|-----------|-----------|-----------------|
| **Request/Response** | `call()` → `PendingResponse` | `MethodCall` + `Responder` | Must handle response |
| **Fire & Forget** | `fire_and_forget()` | `FireAndForget` (no responder) | No accidental response |
| **Publish/Subscribe** | `subscribe()` → `Subscription` | `Subscribe` + `SubscribeAck`, `notify()` | Must subscribe first |
| **Fields** | `get_field()`, `set_field()`, `subscribe()` | `GetField`, `SetField` + `Responder` | Must handle response |
| **Discovery** | `require()` → `wait_available()` | `offer()` | Must wait for available |

---

## Deployment Use Cases

### Use Case 1: Embedded Sensor Server (QNX)

**Scenario**: ECU providing temperature sensor data, handling method calls, publishing events on change.

**Requirements**: No async, minimal overhead, explicit control.

```rust
use someip_runtime::{Server, ServiceConfig, ServiceEvent};

fn main() -> Result<()> {
    let config = ServiceConfig::builder()
        .service(ServiceId::new(0x1234).unwrap())
        .instance(ConcreteInstanceId::new(0x0001).unwrap())
        .endpoint("192.168.1.10", |e| e.udp(AppPort::new(30501).unwrap()))
        .eventgroup(EventgroupId(0x01))
        .build()?;

    let mut runtime = Runtime::new(QnxSockets::new()?, RuntimeConfig::default())?;
    let mut offering = runtime.offer(config)?;

    loop {
        match offering.next()? {
            ServiceEvent::MethodCall { request } => {
                match request.method.0 {
                    0x0001 => {
                        let temp = read_sensor();
                        request.responder.send_ok(&temp.to_bytes())?;
                    }
                    _ => request.responder.send_error(ReturnCode::UnknownMethod)?,
                }
            }
            ServiceEvent::Subscribe { ack, .. } => {
                ack.accept()?;
            }
            ServiceEvent::Tick => {
                if temperature_changed() {
                    offering.notify(EventgroupId(0x01), EventId(0x8001), &new_temp)?;
                }
            }
            _ => {}
        }
    }
}
```

### Use Case 2: Linux Service Client (Tokio)

**Scenario**: Service consuming sensor data, subscribing to events, calling methods.

**Requirements**: Async integration, multiple concurrent operations.

```rust
use someip_tokio::{AsyncRuntime, AsyncSubscription};

#[tokio::main]
async fn main() -> Result<()> {
    let runtime = AsyncRuntime::new(config).await?;
    
    // Find and wait for service
    let proxy = runtime.require(ServiceId::new(0x1234).unwrap(), InstanceId::ANY);
    let available = proxy.wait_available().await?;
    
    // Subscribe - this is our event source
    let mut subscription = available.subscribe(EventgroupId(0x01)).await?;
    
    // Handle events
    tokio::spawn(async move {
        while let Some(event) = subscription.next_event().await {
            process_sensor_reading(event.payload);
        }
    });
    
    // Call methods concurrently
    let response = available.call(MethodId(0x0002), &config_data).await?;
    
    Ok(())
}
```

### Use Case 3: Mixed Client/Server (Automotive Gateway)

**Scenario**: ECU that both provides services and consumes others.

```rust
use someip_runtime::{Runtime, RuntimeEvent};

fn main() -> Result<()> {
    let mut runtime = Runtime::new(sockets, config)?;
    
    // Offer our service
    let mut my_service = runtime.offer(my_service_config)?;
    
    // Require remote service
    let remote_proxy = runtime.require(remote_service_id, InstanceId::ANY);
    let remote = remote_proxy.wait_available()?;
    let mut remote_events = remote.subscribe(EventgroupId(0x01))?;
    
    loop {
        match runtime.next()? {
            RuntimeEvent::Service(event) => {
                handle_incoming_request(&mut my_service, event)?;
            }
            RuntimeEvent::Subscription(event) => {
                let data = event.payload;
                // Forward to our subscribers
                my_service.notify(EventgroupId(0x02), EventId(0x8002), &data)?;
            }
        }
    }
}
```

### Use Case 4: C++ Integration

**Scenario**: Existing C++ application needs SOME/IP support.

```cpp
#include <someip.h>

int main() {
    someip_config_t config = { /* ... */ };
    someip_client_t* client = someip_client_new(&config);
    
    // Wait for service
    while (someip_wait_available(client, 1000) != SOMEIP_OK) {
        // Retry
    }
    
    // Subscribe
    someip_subscribe(client, 0x01);
    
    // Main loop
    someip_message_t msg;
    while (someip_next(client, &msg, -1) == SOMEIP_OK) {
        switch (msg.type) {
            case SOMEIP_MSG_EVENT:
                handle_event(&msg);
                break;
            case SOMEIP_MSG_RESPONSE:
                handle_response(&msg);
                break;
        }
    }
    
    someip_client_free(client);
    return 0;
}
```

---

## Platform-Specific Considerations

### Linux

- Full tokio async support via `someip-tokio`
- epoll integration via `as_raw_fd()`
- Channel-based distribution to worker threads

### QNX

- Sync API only (`someip-runtime`)
- ionotify/pulse integration via `as_raw_fd()`
- No async runtime dependency
- Explicit resource control

### Embedded/RTOS

- `someip-types` is no_std compatible
- User provides socket abstraction via traits
- No hidden allocations
- Explicit polling, no background threads

### C++ Bindings

- Clean C API via `someip-sys`
- No callbacks across FFI boundary
- Simple data types (structs, enums)
- User manages threading

---

## Configuration Validation

### Error Categories

| Category | Example | Handling |
|----------|---------|----------|
| **Hard Errors** | Port 0, Instance ID 0x0000 | Reject at `build()` |
| **Spec Violations** | App traffic on port 30490 | Reject at `build()` |
| **Logical Errors** | TCP multicast endpoint | Reject at `build()` (or unrepresentable) |
| **Interop Risks** | Non-standard SD port | Warn, but allow |
| **Suboptimal Config** | Multiple ports to same ECU | Warn, but allow |

### Validation Result

```rust
pub fn build(self) -> Result<(Config, ValidationResult), ConfigError>;

pub struct ValidationResult {
    pub warnings: Vec<ConfigWarning>,
}

pub enum ConfigWarning {
    NonStandardSdPort { port: u16 },
    MultiplePortsToSameEcu { ecu: IpAddr },
    // ...
}
```

---

## Summary

### Core Design Principles

1. **Types as Rails**: The type system guides users through valid protocol states
2. **Sync-First**: Core library works everywhere; async is an optional layer
3. **Explicit over Magic**: User controls the event loop, threading, distribution
4. **Configuration Agnostic**: Library accepts config structs, not file formats
5. **Protocol Compliant**: Invalid protocol states are unrepresentable where possible

### Trade-offs Made

| Choice | Benefit | Cost |
|--------|---------|------|
| Sync-first | Works on QNX, embedded | Async requires wrapper layer |
| Explicit polling | Full user control | More code than callbacks |
| Strict types | Compile-time safety | More types to understand |
| No config parsing | No parser dependencies | User must provide parsers |

### Success Criteria

- [x] Type-safe API where invalid states are unrepresentable
- [ ] Compiles on Linux and QNX
- [ ] Zero undefined behavior
- [ ] Protocol-compliant message handling
- [ ] Clean C FFI for C++ integration
- [ ] No hidden allocations or spawned threads in core

---

## Implementation Status

### API Surface (src/lib.rs)

The top-level API has been defined with:

**Protocol Identifiers** (validated newtypes):
- `ServiceId` - validates against reserved values 0x0000, 0xFFFF
- `ConcreteInstanceId` - for servers, no wildcards (0xFFFF rejected)
- `InstanceId` - for clients, allows `ANY` (0xFFFF)
- `EventId` - validates high bit set (0x8000-0xFFFE)
- `EventgroupId` - validates non-zero
- `MethodId` - method/field identifier
- `AppPort` - validates not 30490 (SD reserved)

**Client API**:
- `Client<Io>` - client runtime
- `ServiceProxy<Io, State>` - typestate proxy (Unavailable → Available)
- `Subscription<Io>` - active subscription, IS the event source
- `PendingResponse<Io>` - awaiting method response

**Server API**:
- `Server<Io>` - server runtime
- `ServiceOffering<Io>` - active service offer, handles events
- `ServiceEvent` - method calls, subscriptions, etc.
- `Responder` - must-use response sender (panics if dropped)
- `SubscribeAck` - must-use subscription handler

**I/O Abstraction**:
- `IoContext` - platform abstraction trait
- `UdpSocketOps`, `TcpStreamOps`, `TcpListenerOps` - socket traits
- `IoResult<T>` - I/O result type

### Test Suite

**Type Safety Tests** (tests/api_usage.rs) - 14 passing:
- Reserved ID rejection (ServiceId, InstanceId, EventId, etc.)
- Port 30490 reservation
- Instance ID conversion
- Event ID range validation

**Simulated Network** (tests/simulated.rs) - 4 passing:
- UDP roundtrip
- Multicast delivery
- Network partitioning
- TCP connect and transfer

**Integration Tests** (pending implementation):
- Service discovery
- Subscription flow
- Method calls
- Error handling

### Running Tests

```bash
cd rust-api-design
cargo test --test api_usage   # Type safety tests
cargo test --test simulated   # Simulated network tests
```

---

## Appendix: Protocol Quick Reference

### Endpoint Rules

- Endpoint = IP + Protocol (UDP/TCP) + Port
- Server: up to 1 UDP + 1 TCP per service instance
- Same service, multiple instances → MUST use different ports
- Different services → CAN share ports

### Port Rules

- SD port 30490: Reserved for Service Discovery only
- Application ports: Configured, must not be 30490
- Ephemeral: 49152-65535 for dynamic allocation

### Subscription Rules

- SubscribeEventgroup only after OfferService (service must be available)
- TCP connection before SubscribeEventgroup (for reliable events)
- Events only sent to subscribed clients
- StopSubscribeEventgroup on unsubscribe or drop

### SD Rules

- SD over UDP only
- Session ID per communication relation
- Reboot detection via Session ID
