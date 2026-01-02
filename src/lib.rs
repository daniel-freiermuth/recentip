//! # someip-runtime
//!
//! A type-safe async SOME/IP protocol implementation for tokio.
//!
//! ## Quick Start
//!
//! ### Client Example
//!
//! ```rust,ignore
//! use someip_runtime::{Runtime, RuntimeConfig, ServiceId, InstanceId, EventgroupId};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let runtime = Runtime::new(RuntimeConfig::default()).await?;
//!
//!     // Find a remote service
//!     let proxy = runtime.find::<BrakeService>(InstanceId::Any);
//!     let proxy = proxy.available().await;
//!
//!     // Call a method
//!     let response = proxy.call(GetStatus { wheel: 0 }).await?;
//!
//!     // Subscribe to events
//!     let mut events = proxy.subscribe(eventgroup).await?;
//!     while let Some(event) = events.next().await {
//!         println!("Received: {:?}", event);
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! ### Server Example
//!
//! ```rust,ignore
//! use someip_runtime::{Runtime, RuntimeConfig, ServiceId, InstanceId};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let runtime = Runtime::new(RuntimeConfig::default()).await?;
//!
//!     let mut offering = runtime.offer::<BrakeService>(InstanceId::Id(0x0001)).await?;
//!
//!     while let Some(event) = offering.next().await {
//!         match event {
//!             ServiceEvent::Call { request, responder } => {
//!                 responder.reply(response).await?;
//!             }
//!             ServiceEvent::Subscribe { ack, .. } => {
//!                 ack.accept().await?;
//!             }
//!             _ => {}
//!         }
//!     }
//!
//!     Ok(())
//! }
//! ```

use std::net::SocketAddr;

pub mod net;

pub mod error;
pub mod handle;
pub mod runtime;
pub mod tcp;

/// Wire format parsing for SOME/IP headers and messages.
/// Exposed for testing and interoperability verification.
pub mod wire;

pub use error::*;
pub use handle::*;
pub use runtime::{MethodConfig, Runtime, RuntimeConfig, Transport};

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
    /// Create a new MethodId. Valid range: 0x0000-0x7FFF (high bit reserved for events)
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventId(u16);

impl EventId {
    /// Create a new EventId. Valid range: 0x8000-0xFFFE
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventgroupId(u16);

impl EventgroupId {
    /// Create a new EventgroupId. Valid range: 0x0001-0xFFFE
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

/// Major version of a service interface
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct MajorVersion(u8);

impl MajorVersion {
    pub fn new(version: u8) -> Self {
        Self(version)
    }

    pub fn value(&self) -> u8 {
        self.0
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
// SERVICE TRAIT
// ============================================================================

/// Trait for service definitions (implemented by generated code)
pub trait Service {
    /// The service ID
    const SERVICE_ID: u16;
    /// Major version
    const MAJOR_VERSION: u8;
    /// Minor version  
    const MINOR_VERSION: u32;
}

// ============================================================================
// EVENTS AND RESPONSES
// ============================================================================

/// Event received from a subscription
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
}

// ============================================================================
// RE-EXPORTS
// ============================================================================

pub mod prelude {
    pub use crate::{
        Error, Event, EventId, EventgroupId, InstanceId, MajorVersion, MethodConfig, MethodId,
        MinorVersion, Response, Result, ReturnCode, Runtime, RuntimeConfig, Service, ServiceId,
    };
}
