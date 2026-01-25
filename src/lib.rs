//! # recentip
//!
//! [![Crate](https://img.shields.io/crates/v/recentip.svg)](https://crates.io/crates/recentip)
//! [![Docs](https://docs.rs/recentip/badge.svg)](https://docs.rs/recentip)
//! [![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](https://github.com/daniel-freiermuth/recentip/blob/main/LICENSE)
//!
//! A **type-safe, async SOME/IP protocol implementation** for [tokio](https://tokio.rs).
//!
//! SOME/IP (Scalable service-Oriented `MiddlewarE` over IP) is the standard middleware
//! protocol for automotive Ethernet communication, enabling service-oriented communication
//! between ECUs in modern vehicles.
//!
//! ## Features
//!
//! - **Type-safe API**: Compile-time guarantees via type-state patterns
//! - **Async/await**: Native tokio integration with zero-cost futures
//! - **Service Discovery**: Automatic discovery via multicast SD protocol
//! - **RPC**: Request/response and fire-and-forget method calls
//! - **Pub/Sub**: Event subscriptions with eventgroup management
//! - **Dual transport**: UDP (default) and TCP with Magic Cookie support
//! - **[Spec compliance](`compliance`)**: Extensive test suite with [traceability report](`compliance`) linking requirements → tests
//!
//! ## Documentation
//!
//! | Resource | Description |
//! |----------|-------------|
//! | [Quick Start](#quick-start) | Get up and running in 5 minutes |
//! | [`examples`] | In-depth guides: RPC, Pub/Sub, Transport, Monitoring |
//! | [`compliance`] | Spec traceability: requirements → tests |
//! | [Architecture Overview](#architecture-overview) | Internal design for contributors |
//! | [`prelude`] | Common imports for getting started |
//!
//! **Key types:**
//! - [`SomeIp`] — the runtime (start with [`configure()`])
//! - [`handle::OfferedService`] — client proxy for calling methods and subscribing
//! - [`handle::ServiceOffering`] — server handle for receiving requests and publishing events
//!
//! ## Quick Start
//!
//! Add to your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! recentip = "0.1"
//! tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
//! ```
//!
//! **📚 See the [`examples`] module for more complete, runnable code samples.**
//!
//! ### Minimal Client
//!
//! ```no_run
//! use recentip::prelude::*;
//!
//! const BRAKE_SERVICE_ID: u16 = 0x1234;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     // Create the runtime
//!     let runtime = recentip::configure().start().await?;
//!
//!     // Find a remote service (waits for SD announcement)
//!     let proxy = runtime.find(BRAKE_SERVICE_ID).await?;
//!
//!     // Call a method (RPC)
//!     let method_id = MethodId::new(0x0001).unwrap();
//!     let response = proxy.call(method_id, b"").await?;
//!     println!("Response: {:?}", response);
//!
//!     // Subscribe to events
//!     let eg = EventgroupId::new(0x0001).unwrap();
//!     let mut subscription = proxy
//!         .new_subscription()
//!         .eventgroup(eg)
//!         .subscribe()
//!         .await?;
//!     
//!     while let Some(event) = subscription.next().await {
//!         println!("Event: {:?}", event);
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! ### Minimal Server
//!
//! ```no_run
//! use recentip::prelude::*;
//! use recentip::handle::ServiceEvent;
//!
//! const BRAKE_SERVICE_ID: u16 = 0x1234;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let runtime = recentip::configure().start().await?;
//!
//!     // Offer a service (announces via SD)
//!     let mut offering = runtime.offer(BRAKE_SERVICE_ID, InstanceId::Id(0x0001))
//!         .version(1, 0)
//!         .udp()
//!         .start()
//!         .await?;
//!
//!     // Handle incoming requests
//!     while let Some(event) = offering.next().await {
//!         match event {
//!             ServiceEvent::Call { method, payload, responder, .. } => {
//!                 // Process request and send response
//!                 responder.reply(b"OK")?;
//!             }
//!             ServiceEvent::Subscribe { eventgroup, .. } => { }
//!             _ => {}
//!         }
//!     }
//!     Ok(())
//! }
//! ```
//!
//! ---
//!
//! # Architecture Overview
//!
//! This section explains the library's internal structure for contributors and
//! advanced users who need to understand how the pieces fit together.
//!
//! ## Conceptual Model
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                           User Application                              │
//! │  ┌──────────────┐    ┌───────────────┐    ┌────────────────────────┐   │
//! │  │ ProxyHandle  │    │ OfferingHandle│    │ ServiceInstance<State>│   │
//! │  │  (client)    │    │   (server)    │    │  (typestate server)   │   │
//! │  └──────┬───────┘    └───────┬───────┘    └───────────┬────────────┘   │
//! └─────────┼────────────────────┼────────────────────────┼────────────────┘
//!           │ Commands           │ Commands               │ Commands
//!           ▼                    ▼                        ▼
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                         SomeIp (Event Loop)                            │
//! │  ┌───────────────────────────────────────────────────────────────────┐  │
//! │  │                      RuntimeState                                 │  │
//! │  │  • offered: HashMap<ServiceKey, OfferedService>                   │  │
//! │  │  • discovered: HashMap<ServiceKey, DiscoveredService>             │  │
//! │  │  • pending_calls: HashMap<CallKey, PendingCall>                   │  │
//! │  │  • subscriptions: HashMap<SubscriberKey, Subscriber>              │  │
//! │  │  • session IDs (multicast + unicast)                              │  │
//! │  └───────────────────────────────────────────────────────────────────┘  │
//! │                                                                         │
//! │  Event Loop (select!):                                                  │
//! │    • Command channel (from handles)                                     │
//! │    • SD socket (multicast Service Discovery)                            │
//! │    • RPC socket (client UDP)                                            │
//! │    • TCP connections (client/server)                                    │
//! │    • Periodic timer (cyclic offers, TTL expiry)                         │
//! └─────────────────────────────────────────────────────────────────────────┘
//!                     │                           │
//!                     ▼                           ▼
//!           ┌─────────────────┐        ┌─────────────────┐
//!           │   SD Socket     │        │   RPC Socket    │
//!           │  (UDP 30490)    │        │  (UDP/TCP)      │
//!           └─────────────────┘        └─────────────────┘
//! ```
//!
//! ## Module Responsibilities
//!
//! The implementation is organized into modules with clear responsibilities:
//!
//! | Module | Visibility | Responsibility |
//! |--------|------------|----------------|
//! | `runtime` | Public | Event loop executor, socket management, [`SomeIp`] struct |
//! | [`handle`] | Public | User-facing API: [`OfferedService`], [`ServiceOffering`] |
//! | [`config`] | Public | Configuration: [`RuntimeConfig`], [`Transport`], [`MethodConfig`] |
//! | [`error`] | Public | Error types: [`Error`], [`Result`] |
//! | [`wire`] | Public | Wire format: [`Header`](wire::Header), [`SdMessage`](wire::SdMessage), parsing |
//! | [`tcp`] | Public | TCP framing, connection pooling, Magic Cookies |
//! | `command` | Internal | Command enum for handle→runtime communication |
//! | `state` | Internal | `RuntimeState`, `ServiceKey`, internal data structures |
//! | `sd` | Internal | Service Discovery message handlers and builders |
//! | `client` | Internal | Client-side handlers (find, call, subscribe) |
//! | `server` | Internal | Server-side handlers (offer, notify, respond) |
//!
//! ## Key Concepts
//!
//! ### The `SomeIp` as State Machine Executor
//!
//! The [`SomeIp`] is the **central coordinator**. It:
//!
//! 1. **Owns all state** in a single `RuntimeState` struct
//! 2. **Runs an event loop** via `tokio::select!` over multiple sources
//! 3. **Dispatches commands** from handles to appropriate handlers
//! 4. **Manages all I/O** through owned sockets
//!
//! Handles (like [`OfferedService`] and [`ServiceOffering`]) don't perform I/O themselves.
//! They send `Command` messages to the runtime, which processes them
//! atomically in the event loop. This design:
//!
//! - Eliminates data races (all state in one place)
//! - Enables efficient multiplexing (one socket for multiple services)
//! - Simplifies testing (deterministic message ordering)
//!
//! ### Type-State Pattern for Safety
//!
//! The library uses **type-state patterns** to enforce correct usage at compile time:
//!
//! - [`OfferedService`] → returned by `find()`, ready for `.call()`, `.subscribe()`, etc.
//!
//! This prevents runtime errors like "announcing a service that hasn't bound a socket".
//!
//! ### Service Discovery (SD)
//!
//! SD runs over UDP multicast (default: 239.255.0.1:30490). The runtime:
//!
//! - **Servers**: Periodically send `OfferService` entries
//! - **Clients**: Send `FindService` entries and listen for offers
//! - **Subscriptions**: Exchange `SubscribeEventgroup` / `SubscribeEventgroupAck`
//!
//! The `sd` module handles parsing and building SD messages.
//!
//! ### Session ID Management
//!
//! Per SOME/IP spec, session IDs:
//!
//! - Are 16-bit, wrapping from 0xFFFF → 0x0001 (never 0x0000)
//! - Have separate counters for multicast vs unicast SD
//! - Use a "reboot flag" to signal restart (first message after boot)
//!
//! This is tracked in `RuntimeState`.
//!
//! ---
//!
//! # How-To Guides
//!
//! ## Configure Transport (UDP vs TCP)
//!
//! ```
//! use recentip::{RuntimeConfig, Transport};
//!
//! // Default: UDP
//! let config = RuntimeConfig::default();
//!
//! // Use TCP for RPC
//! let config = RuntimeConfig::builder()
//!     .transport(Transport::Tcp)
//!     .magic_cookies(true)  // Enable Magic Cookies for debugging
//!     .build();
//! ```
//!
//! ## Subscribe to Events
//!
//! ```no_run
//! use recentip::prelude::*;
//! use recentip::handle::OfferedService;
//!
//! async fn subscribe_example(proxy: &OfferedService) -> Result<()> {
//!     let eg1 = EventgroupId::new(0x0001).unwrap();
//!     let eg2 = EventgroupId::new(0x0002).unwrap();
//!     
//!     let mut subscription = proxy
//!         .new_subscription()
//!         .eventgroup(eg1)
//!         .eventgroup(eg2)  // Optional: subscribe to multiple eventgroups
//!         .subscribe()
//!         .await?;
//!
//!     while let Some(event) = subscription.next().await {
//!         println!("Event {}: {} bytes", event.event_id.value(), event.payload.len());
//!     }
//!     Ok(())
//! }
//! # fn main() {}
//! ```
//!
//! ## Publish Events (Server-Side)
//!
//! ```no_run
//! use recentip::prelude::*;
//!
//! async fn publish_example(offering: &ServiceOffering) -> Result<()> {
//!     // Create an event handle that belongs to eventgroup 0x0001
//!     let temperature = offering
//!         .event(EventId::new(0x8001).unwrap())
//!         .eventgroup(EventgroupId::new(0x0001).unwrap())
//!         .create().await?;
//!
//!     // Send notification to all subscribers of this event's eventgroups
//!     temperature.notify(b"42.5").await?;
//!     Ok(())
//! }
//! # fn main() {}
//! ```
//!
//! ## Handle Errors Gracefully
//!
//! ```no_run
//! use recentip::prelude::*;
//! use recentip::handle::OfferedService;
//!
//! async fn error_handling_example(proxy: &OfferedService) -> Result<()> {
//!     let method_id = MethodId::new(0x0001).unwrap();
//!     let payload = b"request";
//!
//!     match proxy.call(method_id, payload).await {
//!         Ok(response) if response.return_code == ReturnCode::Ok => {
//!             // Success
//!         }
//!         Ok(response) => {
//!             // Server returned an error code
//!             eprintln!("Server error: {:?}", response.return_code);
//!         }
//!         Err(Error::ServiceUnavailable) => {
//!             // Service went offline
//!         }
//!         Err(Error::RuntimeShutdown) => {
//!             // SomeIp was dropped
//!         }
//!         Err(e) => {
//!             // Other error (I/O, protocol, etc.)
//!             eprintln!("Call failed: {}", e);
//!         }
//!     }
//!     Ok(())
//! }
//! # fn main() {}
//! ```
//!
//! ---
//!
//! # Reference
//!
//! ## Identifier Types
//!
//! | Type | Range | Reserved | Notes |
//! |------|-------|----------|-------|
//! | [`ServiceId`] | 0x0001-0xFFFE | 0x0000, 0xFFFF | Unique per service interface |
//! | [`InstanceId`] | 0x0001-0xFFFE | 0x0000 | 0xFFFF = wildcard ("any") |
//! | [`MethodId`] | 0x0000-0x7FFF | — | Bit 15 = 0 for methods |
//! | [`EventId`] | 0x8000-0xFFFE | 0xFFFF | Bit 15 = 1 for events |
//! | [`EventgroupId`] | 0x0001-0xFFFE | 0x0000, 0xFFFF | Groups related events |
//!
//! ## Wire Format
//!
//! See [`wire`] module for header structures. Key constants:
//!
//! - Protocol version: `0x01` (always, per spec)
//! - Header size: 16 bytes (fixed)
//! - SD port: 30490 (UDP only, per spec)
//!
//! ## Feature Flags
//!
//! | Feature | Default | Description |
//! |---------|---------|-------------|
//! | `turmoil` | Yes | Network simulation for deterministic testing |
//!
//! ---
//!
//! # Automotive Runtime Considerations
//!
//! SOME/IP is designed for automotive ECUs where runtime predictability can matter.
//! This section documents the library's behavior for systems with timing requirements.
//!
//! ## Safety & Certification
//!
//! ⚠️ **This library is NOT ASIL-qualified and NOT suitable for safety-critical functions.**
//!
//! - No formal verification or safety certification
//! - Not developed according to ISO 26262 processes
//! - Use only for QM (non-safety) applications
//! - For ASIL-rated functions, use a certified SOME/IP implementation
//!
//! ## Performance & Latency
//!
//! | Aspect | Status | Notes |
//! |--------|--------|-------|
//! | Throughput | Untested | No benchmarks yet; contributions welcome |
//! | Latency bounds | ⚠️ Theoretical | Tokio guarantees exist but `MAX_DELAY` is unspecified |
//! | Jitter | ❌ Variable | Work-stealing scheduler, GC-free but not deterministic |
//! | Zero-copy | ❌ Not yet | Message parsing allocates; zero-copy design possible |
//!
//! ## Tokio Runtime Behavior
//!
//! This library uses [Tokio](https://tokio.rs/) as its async runtime. Key characteristics
//! relevant to timing (see [Tokio runtime docs](https://docs.rs/tokio/latest/tokio/runtime/index.html)):
//!
//! **Bounded delay guarantee** (from Tokio docs):
//!
//! > Under the following two assumptions:
//! > - There is some number `MAX_TASKS` such that the total number of tasks never exceeds `MAX_TASKS`.
//! > - There is some number `MAX_SCHEDULE` such that calling `poll` on any task returns within `MAX_SCHEDULE` time units.
//! >
//! > Then, there is some number `MAX_DELAY` such that when a task is woken, it will be scheduled within `MAX_DELAY` time units.
//!
//! **What this means:** If you can prove bounded task count and bounded poll time, Tokio
//! guarantees bounded scheduling delay. However:
//!
//! | Aspect | Tokio Provides | ASIL C/D Requires |
//! |--------|----------------|-------------------|
//! | Bound exists? | ✅ Yes | ✅ Needed |
//! | Bound documented? | ❌ No | ✅ Must be specified |
//! | Bound is tight? | ❌ No | ✅ Must be practical |
//! | Preconditions provable? | ⚠️ Hard | ✅ Must be formal |
//!
//! **Additional runtime characteristics:**
//! - Tasks may be scheduled in any order; no priority support
//! - A task may be scheduled 5× before another ready task runs
//! - Work-stealing between threads adds non-deterministic delays
//! - IO/timer checks occur every ~61 scheduled tasks (`event_interval`)
//! - Global queue checked every ~31 local tasks (`global_queue_interval`)
//! - **Not NUMA-aware** — consider multiple runtimes on NUMA systems
//!
//! **What this means for SOME/IP:**
//! - Message latency depends on system load and task count
//! - No guarantee that high-priority responses arrive before low-priority work
//! - Cyclic SD announcements may jitter under load
//! - For ASIL A/B: empirical measurement with safety margin may suffice
//! - For ASIL C/D: Tokio is not certifiable (unknown `MAX_DELAY`, unproven preconditions)
//!
//! ## Current Limitations
//!
//! This library is **not suitable for hard real-time** applications:
//!
//! | Concern | Status | Notes |
//! |---------|--------|-------|
//! | Heap allocation | ⚠️ Dynamic | Allocates during operation (buffers, connections) |
//! | Async runtime | ⚠️ Tokio | Work-stealing scheduler, not deterministic |
//! | Blocking | ⚠️ Possible | Async mutex in TCP connection pool |
//! | Panic paths | ⚠️ Minimal | Few `unwrap()` calls remain (to be eliminated) |
//! | Unsafe code | ✅ Forbidden | `#![forbid(unsafe_code)]` enforced |
//! | Priority inversion | ⚠️ Possible | No priority-aware scheduling |
//!
//! ## Suitable Use Cases
//!
//! - **Prototyping and simulation** — Fast iteration on service interfaces
//! - **Test environments** — Deterministic network simulation via turmoil
//! - **Non-safety-critical ECUs** — Infotainment, logging, diagnostics
//! - **Development tooling** — Service monitors, traffic analyzers
//! - **Linux-based gateways** — Central compute platforms
//!
//! ## Future Directions
//!
//! For safety-critical or hard real-time deployments, consider:
//!
//! 1. **Alternative async runtimes** — The [`net`] module is abstracted; could support
//!    [embassy](https://embassy.dev/) for embedded targets
//! 2. **Pre-allocation** — Buffer pools and bounded collections could be added
//! 3. **`no_std` support** — Currently requires std; a `no_std` core is architecturally possible
//! 4. **Static configuration** — Compile-time service definitions to eliminate runtime allocation
//!
//! ## Relation to Real-Time Rust Ecosystems
//!
//! This library uses **Tokio**, designed for high-throughput servers, not real-time systems.
//! For hard real-time requirements, consider these Rust ecosystems:
//!
//! ### Pure Rust Runtimes
//!
//! | Project | Model | Notes |
//! |---------|-------|-------|
//! | [RTIC](https://rtic.rs/) | Interrupt-driven, priority-based | Ideal for bare-metal MCUs |
//! | [Embassy](https://embassy.dev/) | Async, `no_std`, embedded | Could backend our [`net`] abstraction |
//! | [Drone OS](https://www.drone-os.com/) | Async, preemptive threads | RTOS with async/await |
//! | [Hubris](https://hubris.oxide.computer/) | IPC-based microkernel | Oxide's secure embedded OS |
//! | [Tock](https://www.tockos.org/) | Process isolation, embedded | Security-focused embedded OS |
//!
//! ### Traditional RTOSes with Rust Bindings
//!
//! | Project | Rust Support | Notes |
//! |---------|--------------|-------|
//! | [FreeRTOS](https://www.freertos.org/) | [`freertos-rust`](https://crates.io/crates/freertos-rust) | Industry standard, 40+ architectures |
//! | [RIOT](https://riot-os.org/) | [`riot-wrappers`](https://crates.io/crates/riot-wrappers) | IoT-focused, threading, network stacks |
//!
//! ### Automotive-Focused Rust Frameworks
//!
//! | Project | Model | Notes |
//! |---------|-------|-------|
//! | [OxidOS](https://oxidos.io/) | Tock-based, sandboxed apps | Automotive ASIL targeting, RISC-V |
//! | [Veecle OS](https://github.com/veecle/veecle-os) | Actor-based, OSAL | std/Embassy/FreeRTOS backends; has SOME/IP |
//! | [veecle-pxros](https://github.com/veecle/veecle-pxros) | PXROS-HR runtime | AURIX, ASIL-D kernel |
//!
//! ### Robotics Middleware
//!
//! | Project | Rust Support | Notes |
//! |---------|--------------|-------|
//! | [ROS 2](https://docs.ros.org/) | [`ros2_rust`](https://github.com/ros2-rust/ros2_rust) | Pub/sub, services, zero-copy; ADAS/AD |
//!
//! ### Automotive Linux Distributions
//!
//! | Project | Rust Support | Notes |
//! |---------|--------------|-------|
//! | [Red Hat In-Vehicle OS](https://www.redhat.com/en/solutions/automotive) | Full Rust | RHEL-based, ASIL-B; VW, Audi |
//! | [Automotive Grade Linux](https://www.automotivelinux.org/) | Full Rust | Toyota, Honda, Mazda, Subaru, Suzuki, Mercedes |
//!
//! **Potential integration paths:**
//!
//! - **RTIC/Embassy**: Port [`wire`] parsing and state machine logic; replace Tokio I/O
//! - **Veecle OS**: Already has SOME/IP support; could potentially share wire-format code
//! - **FreeRTOS/RIOT**: Use Rust bindings for threading/networking, port state machine logic
//! - **ROS 2**: Bridge SOME/IP services to ROS 2 topics/services for ADAS integration
//! - **Hypervisor**: Run this library in Linux alongside an RTOS partition
//! - **Shared memory**: Use on Linux core, bridge to RTIC/Zephyr for safety-critical functions
//!
//! Currently, no Rust SOME/IP implementation targets `no_std` or real-time runtimes.
//!
//! Contributions toward these goals are welcome.
//!
//! ---
//!
//! # For Contributors
//!
//! ## Code Organization
//!
//! - **Public modules** are documented for users
//! - **Internal modules** (`pub(crate)`) are documented for contributors
//! - Tests are in `tests/compliance/` organized by specification area
//!
//! ## Testing
//!
//! ```bash
//! # Run all tests (includes turmoil simulation tests)
//! cargo nextest run --features turmoil
//!
//! # Run with coverage
//! cargo tarpaulin --features turmoil
//! ```
//!
//! ## Adding a New Feature
//!
//! 1. Add command variant to `runtime/command.rs` if handle→runtime communication needed
//! 2. Add state to `runtime/state.rs` if persistent tracking required
//! 3. Add handler to `runtime/client.rs` or `runtime/server.rs` depending on role
//! 4. Update `runtime.rs` event loop to dispatch the new command
//! 5. Add public API to `handles/`
//! 6. Write compliance tests in `tests/compliance/`

use std::net::SocketAddr;

pub mod builder;
pub mod compliance;
pub mod examples;
pub mod net;

// Internal modules for runtime implementation (moved to runtime/)
pub(crate) mod runtime;

// Public modules
pub mod config;
pub mod error;
pub mod handles;
pub mod tcp;

/// Wire format parsing for SOME/IP headers and messages.
/// Exposed for testing and interoperability verification.
pub mod wire;

// Re-export SomeIp and builder
pub use builder::SomeIpBuilder;
pub use handles::{OfferBuilder, SomeIp};

pub use config::{MethodConfig, RuntimeConfig, Transport};

/// Deprecated: Use `SomeIp` instead.
///
/// This is a type alias for backward compatibility. New code should use [`SomeIp`] directly.
#[deprecated(since = "0.1.0", note = "Use `SomeIp` instead")]
pub type Runtime = SomeIp;

pub use error::*;

// Re-export handle types (explicit to avoid shadowing with internal runtime module)
pub use handles::{
    // Server-side handles
    EventBuilder,
    EventHandle,
    // Client-side handles
    FindBuilder,
    OfferedService,
    Responder,
    ServiceEvent,
    ServiceOffering,
    StaticEventListener,
    Subscription,
    SubscriptionBuilder,
};

// Re-export SD event types for monitoring API
pub use runtime::SdEvent;

// Backward compatibility: re-export handle module
#[doc(hidden)]
pub mod handle {
    //! Backward compatibility re-exports from handles module
    pub use crate::handles::*;
}

// ============================================================================
// ENTRY POINT
// ============================================================================

/// Configure and start a SOME/IP runtime.
///
/// This is the main entry point for creating a SOME/IP runtime instance.
/// Returns a builder that allows fluent configuration of the runtime parameters.
///
/// # Example
///
/// ```no_run
/// use recentip::prelude::*;
/// use std::net::Ipv4Addr;
///
/// #[tokio::main]
/// async fn main() -> recentip::Result<()> {
///     // Start with defaults
///     let someip = recentip::configure().start().await?;
///     
///     // Or configure before starting
///     let someip = recentip::configure()
///         .advertised_ip(Ipv4Addr::new(192, 168, 1, 100).into())
///         .preferred_transport(Transport::Tcp)
///         .start().await?;
///     
///     Ok(())
/// }
/// ```
pub fn configure() -> SomeIpBuilder {
    SomeIpBuilder::new()
}

// ============================================================================
// PROTOCOL IDENTIFIERS
// ============================================================================

/// Service identifier (0x0001-0xFFFE valid, 0x0000 and 0xFFFF reserved)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServiceId(u16);

impl ServiceId {
    /// Create a new `ServiceId`. Returns None for reserved values.
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

/// Instance identifier - can be a specific ID or Any (wildcard)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstanceId {
    /// Match any instance (0xFFFF wildcard)
    Any,
    /// Specific instance ID (0x0001-0xFFFE valid)
    Id(u16),
}

impl InstanceId {
    /// Match any instance (0xFFFF wildcard) - alias for `Any`
    pub const ANY: Self = Self::Any;

    /// Create a specific instance ID. Returns None for reserved values.
    pub fn new(id: u16) -> Option<Self> {
        match id {
            0x0000 => None,
            0xFFFF => Some(Self::Any),
            id => Some(Self::Id(id)),
        }
    }

    /// Get the raw value (0xFFFF for Any)
    pub fn value(&self) -> u16 {
        match self {
            Self::Any => 0xFFFF,
            Self::Id(id) => *id,
        }
    }

    /// Check if this is a wildcard
    pub fn is_any(&self) -> bool {
        matches!(self, Self::Any)
    }
}

/// Method identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MethodId(u16);

impl MethodId {
    /// Create a new `MethodId`. Valid range: 0x0000-0x7FFF (high bit reserved for events)
    pub fn new(id: u16) -> Option<Self> {
        if id >= 0x8000 {
            None // High bit set = event, not method
        } else {
            Some(Self(id))
        }
    }

    pub fn value(&self) -> u16 {
        self.0
    }
}

/// Event identifier (high bit must be set: 0x8000-0xFFFE)
///
/// also see: [`Event`] and [`EventgroupId`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventId(u16);

impl EventId {
    /// Create a new `EventId`. Valid range: 0x8000-0xFFFE
    pub fn new(id: u16) -> Option<Self> {
        if id < 0x8000 || id == 0xFFFF {
            None
        } else {
            Some(Self(id))
        }
    }

    pub fn value(&self) -> u16 {
        self.0
    }
}

/// Eventgroup identifier
///
/// also see: [`Event`] and [`EventId`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventgroupId(u16);

impl EventgroupId {
    /// Create a new `EventgroupId`. Valid range: 0x0001-0xFFFE
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

/// Major version of a service interface - can be exact or wildcard (Any)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MajorVersion {
    /// Match any major version (0xFF wildcard in SD `FindService`)
    Any,
    /// Specific major version (0x00-0xFE)
    Exact(u8),
}

impl MajorVersion {
    /// Create a specific major version
    pub fn new(version: u8) -> Self {
        if version == 0xFF {
            Self::Any
        } else {
            Self::Exact(version)
        }
    }

    /// Get the raw value (0xFF for Any)
    pub fn value(&self) -> u8 {
        match self {
            Self::Any => 0xFF,
            Self::Exact(v) => *v,
        }
    }

    /// Check if this is a wildcard
    pub fn is_any(&self) -> bool {
        matches!(self, Self::Any)
    }
}

impl Default for MajorVersion {
    fn default() -> Self {
        Self::Any
    }
}

impl From<u8> for MajorVersion {
    fn from(v: u8) -> Self {
        Self::new(v)
    }
}

/// Minor version of a service interface
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct MinorVersion(u32);

impl MinorVersion {
    pub fn new(version: u32) -> Self {
        Self(version)
    }

    pub fn value(&self) -> u32 {
        self.0
    }
}

// ============================================================================
// RETURN CODES
// ============================================================================

/// SOME/IP return codes (for parsing received responses).
///
/// This enum represents all possible SOME/IP return codes as defined in the
/// specification. It is used when **receiving** responses from servers.
///
/// For **sending** error responses from a server, use [`ApplicationError`]
/// instead, which only exposes the codes that applications should generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ReturnCode {
    /// No error occurred (0x00)
    Ok = 0x00,
    /// An unspecified error occurred (0x01)
    NotOk = 0x01,
    /// The requested Service ID is unknown (0x02) - optional
    UnknownService = 0x02,
    /// The requested Method ID is unknown (0x03) - optional
    UnknownMethod = 0x03,
    /// Application not running (0x04) - deprecated
    NotReady = 0x04,
    /// System not reachable (0x05) - deprecated, internal only
    NotReachable = 0x05,
    /// Timeout occurred (0x06) - deprecated, internal only
    Timeout = 0x06,
    /// SOME/IP protocol version not supported (0x07) - obsolete
    WrongProtocolVersion = 0x07,
    /// Interface version mismatch (0x08)
    WrongInterfaceVersion = 0x08,
    /// Payload deserialization error (0x09)
    MalformedMessage = 0x09,
    /// Unexpected message type received (0x0A)
    WrongMessageType = 0x0A,
}

// ============================================================================
// APPLICATION ERRORS
// ============================================================================

/// Error codes that applications can send in response to method calls.
///
/// This type enforces the SOME/IP specification's separation of concerns:
/// - **Protocol-level errors** (like wrong protocol version) are generated
///   automatically by the library
/// - **Application-level errors** are generated by your service implementation
///
/// Per SOME/IP specification (`feat_req_someip_371)`:
/// - Codes 0x00-0x0A are protocol-defined
/// - Codes 0x20-0x3F are reserved for service-specific errors
///
/// # Example
///
/// ```no_run
/// use recentip::prelude::*;
///
/// # async fn handle_request(responder: recentip::handles::Responder, method: MethodId) -> Result<()> {
/// // Unknown method - application doesn't implement this
/// if method.value() != 0x0001 {
///     return responder.reply_error(ApplicationError::UnknownMethod);
/// }
///
/// // Payload parsing failed
/// // responder.reply_error(ApplicationError::MalformedMessage)?;
///
/// // Service-specific error (0x20-0x3F range)
/// // responder.reply_error(ApplicationError::service_specific(0x21)?)?;
///
/// // General error
/// // responder.reply_error(ApplicationError::NotOk)?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplicationError {
    /// An unspecified error occurred (0x01).
    ///
    /// Use this for general errors that don't fit other categories.
    NotOk,

    /// The requested Method ID is unknown (0x03).
    ///
    /// Use this when your service receives a method ID it doesn't implement.
    /// Since this library delivers all method calls to your application
    /// (it doesn't maintain a method registry), you must check method IDs
    /// and return this error for unknown ones.
    UnknownMethod,

    /// Interface version mismatch (0x08).
    ///
    /// Use this when the client's interface version doesn't match what
    /// your service expects. Check the version in the request header.
    WrongInterfaceVersion,

    /// Payload deserialization error (0x09).
    ///
    /// Use this when you cannot deserialize the request payload.
    /// For example, if the payload doesn't match the expected schema.
    MalformedMessage,

    /// Service-specific error (0x20-0x3F).
    ///
    /// Use this for errors defined by your service's interface specification.
    /// The value must be in the range 0x20-0x3F.
    ///
    /// Create with [`ApplicationError::service_specific()`].
    ServiceSpecific(u8),
}

impl ApplicationError {
    /// Create a service-specific error code.
    ///
    /// # Arguments
    ///
    /// * `code` - Error code in the range 0x20-0x3F
    ///
    /// # Returns
    ///
    /// Returns `Some(ApplicationError::ServiceSpecific(code))` if the code
    /// is in the valid range, `None` otherwise.
    ///
    /// # Example
    ///
    /// ```
    /// use recentip::ApplicationError;
    ///
    /// // Valid service-specific codes
    /// assert!(ApplicationError::service_specific(0x20).is_some());
    /// assert!(ApplicationError::service_specific(0x3F).is_some());
    ///
    /// // Invalid codes
    /// assert!(ApplicationError::service_specific(0x1F).is_none());
    /// assert!(ApplicationError::service_specific(0x40).is_none());
    /// ```
    #[must_use]
    pub const fn service_specific(code: u8) -> Option<Self> {
        if code >= 0x20 && code <= 0x3F {
            Some(Self::ServiceSpecific(code))
        } else {
            None
        }
    }

    /// Get the wire format value for this error.
    #[must_use]
    pub const fn as_u8(&self) -> u8 {
        match self {
            Self::NotOk => 0x01,
            Self::UnknownMethod => 0x03,
            Self::WrongInterfaceVersion => 0x08,
            Self::MalformedMessage => 0x09,
            Self::ServiceSpecific(code) => *code,
        }
    }
}

// ============================================================================
// EVENTS AND RESPONSES
// ============================================================================

/// Specific event received from a subscription (aka Notify message or
/// Notification)
///
/// An `Event` contains the type of event (`event_id`) and payload data.
///
/// Events are organized into eventgroups identified by [`EventgroupId`].
/// [`EventId`]s and [`EventgroupId`]s are in N:M relationship - an eventgroup
/// is a collection of [`EventId`]s subscribed to together and [`EventId`]s
/// can belong to multiple [`EventgroupId`]s.
#[derive(Debug, Clone)]
pub struct Event {
    /// The event ID
    pub event_id: EventId,
    /// The payload data
    pub payload: bytes::Bytes,
}

/// Response from a method call
#[derive(Debug, Clone)]
pub struct Response {
    /// The return code
    pub return_code: ReturnCode,
    /// The payload data
    pub payload: bytes::Bytes,
}

impl Response {
    pub fn is_ok(&self) -> bool {
        self.return_code == ReturnCode::Ok
    }

    pub fn is_err(&self) -> bool {
        self.return_code != ReturnCode::Ok
    }
}

/// Information about a client (for server-side)
#[derive(Debug, Clone)]
pub struct ClientInfo {
    /// Client's address
    pub address: SocketAddr,
    /// Transport used by the client
    pub transport: crate::config::Transport,
}

// ============================================================================
// RE-EXPORTS
// ============================================================================

pub mod prelude {
    pub use crate::{
        configure, ApplicationError, Error, Event, EventBuilder, EventHandle, EventId,
        EventgroupId, InstanceId, MajorVersion, MethodConfig, MethodId, MinorVersion,
        OfferedService, Response, Result, ReturnCode, RuntimeConfig, ServiceId, ServiceOffering,
        SomeIp, SomeIpBuilder, Subscription, SubscriptionBuilder, Transport,
    };
}
