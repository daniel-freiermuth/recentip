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

### 2. Async-First with Tokio

**Decision**: Core library is async using tokio; socket types are abstracted via traits.

**Rationale**:
- Modern async/await is ergonomic and well-supported in Rust
- Tokio provides excellent performance and ecosystem integration
- Network abstraction traits enable swapping socket implementations
- Testing uses turmoil for deterministic network simulation

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

### 7. Session Handling Always Enabled

**Decision**: Session handling is always on. No configuration option to disable.

**Rationale**:
- SOME/IP specification mandates session handling
- Session IDs are used for reboot detection
- Request/response matching requires session tracking
- Having a "disable" flag implies invalid protocol operation

### 8. Subscribe Takes Reference

**Decision**: `subscribe(&self, ...)` takes a reference, not ownership.

**Rationale**:
- Clients may need to subscribe to multiple eventgroups from the same service
- Taking ownership would force awkward workarounds
- The proxy remains available for additional subscriptions and RPC calls
- Event flow: `ServiceProxy<Available>` → `subscribe()` → `Subscription` (proxy still usable)

### 9. RPC Through Proxy, Not Subscription

**Decision**: Method calls go through `ServiceProxy<Available>`, not `Subscription`.

**Rationale**:
- RPC and events are orthogonal concerns
- A subscription is purely an event receiver
- User keeps the `Available` proxy and can call methods at any time
- Cleaner separation: subscription = events, proxy = methods

### 10. Separate Binding from Announcing

**Decision**: Server-side uses `ServiceInstance<State>` with distinct `Bound` and `Announced` states.

**Rationale**:
- The SOME/IP-SD spec clearly separates **network binding** (listening on endpoint) from **availability state** (up/down announced via SD)
- Per `feat_req_someipsd_184`: "service instance locations are commonly known; therefore, the state of the service instance is of primary concern"
- Per `feat_req_someipsd_444`: "implicit registration of a client to receive notifications from a server shall be supported" (pre-configured/static mode)
- Enables initialization before announcing availability
- Enables graceful shutdown (stop announcing before closing sockets)
- Enables static/pre-configured deployments without SD
- Enables testing without SD machinery

```rust
// Bind first, announce later
let mut service = runtime.bind::<MyService>(instance).build().await?;
do_initialization().await?;  // Service not yet discoverable
let mut service = service.announce().await?;  // Now sends OfferService

// Graceful shutdown
let service = service.stop_announcing().await?;  // Sends StopOfferService
tokio::time::sleep(Duration::from_secs(1)).await;  // Drain in-flight
drop(service);  // Close sockets
```

---

## Architecture: Single Crate with Modules

The library is a single crate `recentip` with modular organization:

```
┌─────────────────────────────────────────────────────────────┐
│ recentip                                                    │
│ async-first, tokio-based, trait-abstracted I/O              │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  wire/         Protocol types and parsing                   │
│  ├── ServiceId, MethodId, EventgroupId, etc.               │
│  ├── Header parsing/serialization                           │
│  └── SD message parsing/serialization                       │
│                                                             │
│  handles/      User-facing API handles                      │
│  ├── SomeIp (runtime)                                       │
│  ├── OfferedService, Subscription (client)                  │
│  └── ServiceOffering, Responder (server)                    │
│                                                             │
│  runtime/      Internal event loop and state                │
│  ├── Event loop and command processing                      │
│  ├── SD state machines                                      │
│  └── Client/server handlers                                 │
│                                                             │
│  net/          Network abstraction traits                   │
│  ├── UdpSocket, TcpStream, TcpListener traits               │
│  ├── tokio_impl (production)                                │
│  └── turmoil_impl (testing)                                 │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Module Responsibilities

| Module | Responsibility | Notes |
|--------|---------------|-------|
| `wire` | Protocol data types, parsing | Low-level wire format |
| `handles` | User-facing API | Client and server handles |
| `runtime` | Event loop, state machines | Internal implementation |
| `net` | Socket abstraction traits | Enables testing with turmoil |

---

## Network Abstraction Layer

The library abstracts network I/O via async traits, enabling different socket implementations:

- **Production**: Real tokio sockets for actual network communication
- **Testing**: Simulated [turmoil](https://docs.rs/turmoil) sockets for deterministic, fast network simulation

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│ Runtime<U: UdpSocket, T: TcpStream>                         │
│                                                             │
│ Generic over socket traits - works with any implementation  │
│ Just calls: U::bind(), socket.recv_from(), T::connect()    │
└─────────────────────────────────────────────────────────────┘
                              │
          Uses trait ─────────┴─────────
                              │
    ┌─────────────────────────┼─────────────────────────┐
    │                         │                         │
    ▼                         ▼                         ▼
┌─────────────┐       ┌─────────────┐       ┌─────────────┐
│ tokio_impl  │       │turmoil_impl│       │ (future)    │
│             │       │             │       │             │
│ tokio::net  │       │ turmoil::   │       │ std::net    │
│ UdpSocket   │       │ net sockets │       │ for sync    │
│ TcpStream   │       │ Simulated   │       │ embedded    │
└─────────────┘       └─────────────┘       └─────────────┘
     │                     │                     │
     ▼                     ▼                     ▼
  Production            Testing              Future
```

### Core Traits

```rust
/// Async UDP socket abstraction
pub trait UdpSocket: Send + Sync + Sized + 'static {
    fn bind(addr: SocketAddr) -> impl Future<Output = io::Result<Self>> + Send;
    fn send_to(&self, buf: &[u8], target: SocketAddr) -> impl Future<Output = io::Result<usize>> + Send;
    fn recv_from(&self, buf: &mut [u8]) -> impl Future<Output = io::Result<(usize, SocketAddr)>> + Send;
    fn join_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()>;
    fn leave_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()>;
    fn local_addr(&self) -> io::Result<SocketAddr>;
}

/// Async TCP stream abstraction
pub trait TcpStream: Send + Sized + 'static {
    type Listener: TcpListener<Stream = Self>;
    fn connect(addr: SocketAddr) -> impl Future<Output = io::Result<Self>> + Send;
    fn read(&mut self, buf: &mut [u8]) -> impl Future<Output = io::Result<usize>> + Send;
    fn write_all(&mut self, buf: &[u8]) -> impl Future<Output = io::Result<()>> + Send;
    fn local_addr(&self) -> io::Result<SocketAddr>;
    fn peer_addr(&self) -> io::Result<SocketAddr>;
}

/// Async TCP listener abstraction
pub trait TcpListener: Send + Sync + Sized + 'static {
    type Stream: TcpStream<Listener = Self>;
    fn bind(addr: SocketAddr) -> impl Future<Output = io::Result<Self>> + Send;
    fn accept(&self) -> impl Future<Output = io::Result<(Self::Stream, SocketAddr)>> + Send;
    fn local_addr(&self) -> io::Result<SocketAddr>;
}
```

### Turmoil Testing

Most tests use [turmoil](https://docs.rs/turmoil) for deterministic network simulation:

```rust
#[test]
fn test_service_discovery() {
    let mut sim = turmoil::Builder::new().build();
    
    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await?;
        
        let mut offering = runtime.offer(0x1234u16, InstanceId::Id(1))
            .udp().start().await?;
        // ...
        Ok(())
    });
    
    sim.host("client", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await?;
        
        let proxy = runtime.find(0x1234u16).await?;
        // ...
        Ok(())
    });
    
    sim.run().unwrap();
}
```

### Benefits of Turmoil

| Capability | Benefit |
|------------|--------|
| **Deterministic execution** | Same test, same result every time |
| **Fast** | No real network delays, tests run in milliseconds |
| **Time control** | Fast-forward through delays, timeouts |
| **Network partitions** | Test split-brain scenarios |
| **Packet inspection** | Debug network issues |

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
    → "I can subscribe and call methods (multiple times)"

"I have a Subscription"
    → "I can receive events"

"I have a MethodRequest"
    → "I must respond (responder must be consumed)"
```

### Client-Side Type Flow

```
SomeIp (Runtime)
   │
   └── find() ──────────────────► OfferedService (after discovery)
                                        │
                    ┌───────────────────┼────────────────────┐
                    │                   │                    │
              subscribe()          subscribe()           call()
                    │                   │                    │
                    ▼                   ▼                    ▼
              Subscription        Subscription          Response
              (eventgroup 1)     (eventgroup 2)
                    │
              next() ──► Event

Note: subscribe() takes &self, so OfferedService remains usable 
      for additional subscriptions and RPC calls.
```

### Server-Side Type Flow (Current Implementation)

The current implementation uses a simpler approach with `ServiceOffering`:

```
SomeIp (Runtime)
   │
   └── offer() ──► OfferBuilder
                       │
                       └── start() ──► ServiceOffering
                                           │
                    ┌──────────────────────┼───────────────────────┐
                    │                      │                       │
                next()               send_event()            (drop = close)
                    │                      │
                    ▼                      ▼
              ServiceEvent          Sends to subscribers
                    │
              ┌─────┼─────────┐
              │     │         │
            Call  Subscribe  (other)
              │       │
         .responder   │
              │       │
          reply()     │
         (must use)   │

// Drop behavior: sends StopOfferService, closes sockets
```

**Note**: The typestate `ServiceInstance<Bound>` → `ServiceInstance<Announced>` pattern
described in Design Decision #10 is a future enhancement. The current `ServiceOffering`
automatically announces on creation.

### Compile-Time Enforcement

| Invalid State/Behavior | How Rust Prevents It |
|------------------------|----------------------|
| Subscribe before available | `subscribe()` only on `OfferedService` (after find) |
| Get events without subscribing | `next()` only on `Subscription` |
| Ignore method request | `Responder` must be consumed (panics on drop) |
| Respond twice | `Responder` moved on first use |
| Use reserved Instance ID 0x0000 | `InstanceId::new(0)` returns `None` |
| Use reserved Service ID 0x0000/0xFFFF | `ServiceId::new()` returns `None` |

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
let proxy = runtime.find(0x1234u16).await?;
let response = proxy.call(MethodId::new(1).unwrap(), b"request").await?;
// response contains the reply payload

// Server side
while let Some(event) = offering.next().await {
    if let ServiceEvent::Call { method, payload, responder, .. } = event {
        let result = process(&payload);
        responder.reply(&result)?;  // Must respond
    }
}
```

**Type Safety**: `Responder` must be consumed - panics if dropped without replying.

---

### Pattern 2: Fire & Forget

**Protocol**: Client sends message, no response expected. Server processes silently.

**Use when**: Notifications, logging, commands where confirmation isn't needed.

```rust
// Client side
let proxy = runtime.find(0x1234u16).await?;
proxy.fire(MethodId::new(2).unwrap(), b"payload").await?;
// No response - returns immediately after sending

// Server side
while let Some(event) = offering.next().await {
    if let ServiceEvent::FireAndForget { method, payload, .. } = event {
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
let proxy = runtime.find(0x1234u16).await?;
let mut subscription = proxy.subscribe(EventgroupId::new(1).unwrap()).await?;

while let Some(event) = subscription.next().await {
    process_sensor_data(&event.payload);
}
// subscription drops -> StopSubscribeEventgroup sent automatically

// Server side - handle subscription requests
while let Some(event) = offering.next().await {
    if let ServiceEvent::Subscribe { eventgroup, client, .. } = event {
        // Subscription is auto-acknowledged
    }
}

// Server side - send events
let event_handle = offering.event(EventgroupId::new(1).unwrap()).build().await?;
event_handle.send(EventId::new(0x8001).unwrap(), b"sensor data").await?;
```

**Type Safety**: 
- Can't receive events without `Subscription` (type enforced)
- Subscription drop auto-unsubscribes (RAII)

---

### Pattern 4: Service Discovery

**Protocol**: Clients find services, servers announce availability. Handles service coming/going dynamically.

**Use when**: Always - this is how clients and servers find each other.

```rust
// Client side: Find and wait for a service
let proxy = runtime.find(0x1234u16).await?;  // Waits until discovered
// proxy is now ready for use

// Server side: Offer a service
let mut offering = runtime.offer(0x1234u16, InstanceId::Id(1))
    .udp()
    .start()
    .await?;
// Now discoverable via SD

// Handle requests...
while let Some(event) = offering.next().await {
    // ...
}
// Drop sends StopOfferService
```

**Type Safety**: 
- `find()` only returns after service is discovered
- Dropping `ServiceOffering` automatically sends StopOfferService

---

### Pattern Summary

| Pattern | Client API | Server API | Key Type Safety |
|---------|-----------|-----------|-----------------|
| **Request/Response** | `call()` → response | `Call` + `Responder` | Must reply |
| **Fire & Forget** | `fire()` | `FireAndForget` (no responder) | No accidental response |
| **Publish/Subscribe** | `subscribe()` → `Subscription` | `event().send()` | Must subscribe first |
| **Discovery** | `find()` (async) | `offer().start()` | Waits for discovery |

---

## Deployment Use Cases

### Use Case 1: Linux Service (Tokio)

**Scenario**: Service providing sensor data, handling method calls, publishing events.

```rust
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let runtime = recentip::configure().start().await?;
    
    // Offer a service
    let mut offering = runtime.offer(0x1234u16, InstanceId::Id(1))
        .udp()
        .start()
        .await?;
    
    // Create event handle for publishing
    let event_handle = offering.event(EventgroupId::new(1).unwrap())
        .build().await?;
    
    loop {
        tokio::select! {
            Some(event) = offering.next() => {
                match event {
                    ServiceEvent::Call { method, payload, responder, .. } => {
                        responder.reply(b"response")?;
                    }
                    ServiceEvent::Subscribe { .. } => { }
                    _ => {}
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                // Periodic event publishing
                event_handle.send(
                    EventId::new(0x8001).unwrap(), 
                    b"sensor data"
                ).await?;
            }
        }
    }
}
```

### Use Case 2: Client Consuming Events

**Scenario**: Service consuming sensor data, subscribing to events, calling methods.

```rust
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let runtime = recentip::configure().start().await?;
    
    // Find and wait for service
    let proxy = runtime.find(0x1234u16).await?;
    
    // Subscribe - this is our event source
    let mut subscription = proxy.subscribe(EventgroupId::new(1).unwrap()).await?;
    
    // Handle events in a task
    tokio::spawn(async move {
        while let Some(event) = subscription.next().await {
            process_sensor_reading(&event.payload);
        }
    });
    
    // Call methods
    let response = proxy.call(MethodId::new(2).unwrap(), b"config").await?;
    
    Ok(())
}
```

---

## Platform Considerations

### Linux (Current Focus)

- Full tokio async support
- Uses turmoil for deterministic testing
- Channel-based communication between handles and event loop

### Future: QNX / Embedded

- Network traits could be implemented for platform-specific sockets
- Not yet implemented

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
2. **Async-First with Tokio**: Ergonomic async/await with trait-based socket abstraction
3. **Explicit over Magic**: User controls service lifecycle, no hidden state
4. **Configuration Agnostic**: Library accepts config via builders, not file formats
5. **Protocol Compliant**: Invalid protocol states are unrepresentable where possible

### Trade-offs Made

| Choice | Benefit | Cost |
|--------|---------|------|
| Async-first | Ergonomic, efficient | Requires tokio runtime |
| Explicit polling | Full user control | More code than callbacks |
| Strict types | Compile-time safety | More types to understand |
| Turmoil for testing | Deterministic, fast tests | Adds dev dependency |

### Success Criteria

- [x] Type-safe API where invalid states are unrepresentable
- [x] Compiles on Linux
- [x] Protocol-compliant message handling
- [x] Comprehensive test suite with turmoil
- [ ] Full SOME/IP-TP support
- [ ] QNX / embedded support

---

## Implementation Status

### API Surface

**Entry Point**:
- `recentip::configure()` → `SomeIpBuilder` → `SomeIp` (runtime handle)

**Protocol Identifiers** (validated newtypes in `src/lib.rs`):
- `ServiceId` - validates against reserved values 0x0000, 0xFFFF
- `InstanceId` - enum with `Any` (wildcard) or `Id(u16)`
- `EventId` - validates high bit set (0x8000-0xFFFE)
- `EventgroupId` - validates non-zero
- `MethodId` - method identifier (0x0000-0x7FFF)

**Client API** (`src/handles/client.rs`):
- `FindBuilder` - configure service discovery
- `OfferedService` - discovered service proxy for RPC and subscriptions
- `Subscription` - active event subscription, IS the event source

**Server API** (`src/handles/server.rs`):
- `OfferBuilder` - configure service offering
- `ServiceOffering` - handle for incoming requests and sending events
- `Responder` - must-use response sender (panics if dropped)
- `ServiceEvent` - method calls, subscriptions, etc.

**Network Abstraction** (`src/net/`):
- `UdpSocket`, `TcpStream`, `TcpListener` traits
- `tokio_impl` - production implementation
- `turmoil_impl` - testing implementation

### Test Suite

**Current Status**: 365 tests pass, 19 ignored (see TODO.md for details)

| Category | Notes |
|----------|-------|
| Wire format | Header parsing, SD messages |
| Service discovery | Offer, find, subscribe flows |
| RPC | Request/response, fire-and-forget |
| Events | Pub/sub, subscriptions |
| Session handling | Reboot detection, session IDs |
| Transport Protocol | 9 stub tests, implementation pending |

**Testing with Turmoil**:
```bash
cargo test                    # All tests (uses turmoil by default)
cargo test --test compliance  # Compliance suite
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
