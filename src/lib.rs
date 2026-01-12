//! # someip-runtime
//!
//! [![Crate](https://img.shields.io/crates/v/someip-runtime.svg)](https://crates.io/crates/someip-runtime)
//! [![Docs](https://docs.rs/someip-runtime/badge.svg)](https://docs.rs/someip-runtime)
//! [![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](LICENSE)
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
//! - **Spec compliance**: Extensive test coverage against SOME/IP specification
//!
//! ## Quick Start
//!
//! Add to your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! someip-runtime = "0.1"
//! tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
//! ```
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
//!     let runtime = Runtime::new(RuntimeConfig::default()).await?;
//!
//!     // Find a remote service (waits for SD announcement)
//!     let proxy = runtime.find(BRAKE_SERVICE_ID).await?;
//!
//!     // Call a method (RPC)
//!     let method_id = MethodId::new(0x0001).unwrap();
//!     let response = proxy.call(method_id, b"").await?;
//!     println!("Response: {:?}", response);
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
//!     let runtime = Runtime::new(RuntimeConfig::default()).await?;
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
//!                 responder.reply(b"OK").await?;
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
//! │                         Runtime (Event Loop)                            │
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
//! | [`runtime`] | Public | Event loop executor, socket management, [`Runtime`] struct |
//! | [`handle`] | Public | User-facing API: [`ProxyHandle`], [`OfferingHandle`], [`ServiceInstance`] |
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
//! ### The Runtime as State Machine Executor
//!
//! The [`Runtime`] is the **central coordinator**. It:
//!
//! 1. **Owns all state** in a single [`RuntimeState`](state) struct
//! 2. **Runs an event loop** via `tokio::select!` over multiple sources
//! 3. **Dispatches commands** from handles to appropriate handlers
//! 4. **Manages all I/O** through owned sockets
//!
//! Handles (like [`ProxyHandle`] and [`OfferingHandle`]) don't perform I/O themselves.
//! They send [`Command`](command) messages to the runtime, which processes them
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
//! - [`ProxyHandle`] → returned by `find()`, ready for `.call()`, `.subscribe()`, etc.
//! - [`ServiceInstance<Bound>`] → socket is open, but not announced via SD
//! - [`ServiceInstance<Announced>`] → actively announced, accepting requests
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
//! The [`sd`](sd) module handles parsing and building SD messages.
//!
//! ### Session ID Management
//!
//! Per SOME/IP spec, session IDs:
//!
//! - Are 16-bit, wrapping from 0xFFFF → 0x0001 (never 0x0000)
//! - Have separate counters for multicast vs unicast SD
//! - Use a "reboot flag" to signal restart (first message after boot)
//!
//! This is tracked in [`RuntimeState`](state).
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
//! use recentip::handle::ProxyHandle;
//!
//! async fn subscribe_example(proxy: &ProxyHandle) -> Result<()> {
//!     let eventgroup = EventgroupId::new(0x0001).unwrap();
//!     let mut events = proxy.subscribe(eventgroup).await?;
//!
//!     while let Some(event) = events.next().await {
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
//! use recentip::handle::OfferingHandle;
//!
//! async fn publish_example(offering: &OfferingHandle) -> Result<()> {
//!     let eventgroup = EventgroupId::new(0x0001).unwrap();
//!     let event_id = EventId::new(0x8001).unwrap();
//!
//!     // Send notification to all subscribers
//!     offering.notify(eventgroup, event_id, b"payload").await?;
//!     Ok(())
//! }
//! # fn main() {}
//! ```
//!
//! ## Handle Errors Gracefully
//!
//! ```no_run
//! use recentip::prelude::*;
//! use recentip::handle::ProxyHandle;
//!
//! async fn error_handling_example(proxy: &ProxyHandle) -> Result<()> {
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
//!             // Runtime was dropped
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

// Re-export Runtime and config types from handles module
pub use handles::{OfferBuilder, Runtime, RuntimeConfig};

pub use config::{MethodConfig, RuntimeConfigBuilder, Transport};
pub use error::*;

// Re-export handle types (explicit to avoid shadowing with internal runtime module)
pub use handles::{
    Announced,
    Bound,
    // Client-side handles
    FindBuilder,
    // Server-side handles
    OfferingHandle,
    ProxyHandle,
    Responder,
    ServiceEvent,
    ServiceInstance,
    StaticEventListener,
    Subscription,
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
// EVENTS AND RESPONSES
// ============================================================================

/// Specific event received from a subscription (aka Notify message or
/// Notification)
///
/// An `Event` contains the type of event (`event_id`) and payload data.
///
/// Events are organized into eventgroups identified by [`EventgroupId`].
/// [`EventId`]s and [`EventgroupId`]s are in N:M relationship - an eventgroup
/// is a collection of [`EventId`]s subscribed to together and [`EvendtId`]s
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
        Error, Event, EventId, EventgroupId, InstanceId, MajorVersion, MethodConfig, MethodId,
        MinorVersion, Response, Result, ReturnCode, Runtime, RuntimeConfig, ServiceId,
    };
}
