//! # RecentIP
//!
//! [![Crate](https://img.shields.io/crates/v/recentip.svg)](https://crates.io/crates/recentip)
//! [![Docs](https://docs.rs/recentip/badge.svg)](https://docs.rs/recentip)
//! [![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](https://github.com/daniel-freiermuth/recentip/blob/main/LICENSE)
//!
//! An **async SOME/IP protocol implementation** for [tokio](https://tokio.rs).
//!
//! ## Quick Start
//!
//! ```no_run
//! use recentip::prelude::*;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let runtime = recentip::configure()
//!         .sd_unicast("192.168.1.100".parse().unwrap())
//!         .sd_multicast_group("239.255.255.250".parse().unwrap())
//!         .start().await?;
//!
//!     // Client: find and call a remote service
//!     let proxy = runtime.find(0x1234u16).await?;
//!     let response = proxy.call(MethodId::new(1).unwrap(), b"").await?;
//!
//!     // Server: offer a service
//!     let mut offering = runtime.offer(0x5678u16, InstanceId::Id(1))
//!         .udp().start().await?;
//!     
//!     while let Some(event) = offering.next().await {
//!         // Handle requests...
//!     }
//!     Ok(())
//! }
//! ```
//!
//! ## Configuration
//!
//! ```no_run
//! use recentip::prelude::*;
//!
//! # async fn example() -> Result<()> {
//! let someip = recentip::configure()
//!     .sd_unicast("192.168.1.100".parse().unwrap())
//!     .sd_multicast_group("239.255.255.250".parse().unwrap())
//!     .preferred_transport(Transport::Tcp)  // Prefer TCP when service offers both
//!     .magic_cookies(true)                  // Enable Magic Cookies for debugging
//!     .offer_ttl(3)                         // Service offer TTL in seconds
//!     .start().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## API Overview
//!
//! | Type | Role |
//! |------|------|
//! | [`SomeIp`] | Central coordinator, owns sockets |
//! | [`OfferedService`] | Client proxy to remote service |
//! | [`ServiceOffering`] | Server handle for offered service |
//! | [`SubscriptionBuilder`] | Build subscription to eventgroups |
//! | [`Subscription`] | Receive events from eventgroups |
//! | [`Responder`] | Reply to incoming RPC request |
//!
//! ## Learn More
//!
//! - **[`examples`]** — Complete guides: RPC, Pub/Sub, Transport, Monitoring
//! - **[`prelude`]** — Common imports
//! - **[`compliance`]** — Spec traceability report
//! - **[`internals`]** — Contributor documentation (architecture, internals)

use std::net::SocketAddrV4;

pub mod builder;
pub mod compliance;
pub mod examples;
pub mod internals;
pub mod net;

// Internal modules for runtime implementation (moved to runtime/)
pub(crate) mod runtime;

// Public modules
pub mod config;
pub mod error;
pub mod handles;
pub(crate) mod tcp;

/// Wire format parsing for SOME/IP headers and messages.
/// Exposed for testing and interoperability verification.
pub mod wire;

// Re-export SomeIp and builder
pub use builder::SomeIpBuilder;
pub use handles::{OfferBuilder, SomeIp};

pub use config::{
    MethodConfig, PortSpec, RuntimeConfig, TcpKeepaliveConfig, Transport, TransportPolicy,
    TransportPreference, TransportSelection,
};

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
/// Returns a builder that requires all three SD parameters before starting.
///
/// # Example
///
/// ```no_run
/// use recentip::prelude::*;
///
/// #[tokio::main]
/// async fn main() -> recentip::Result<()> {
///     let someip = recentip::configure()
///         .sd_port(30490)
///         .sd_multicast_group("239.255.255.250".parse().unwrap())
///         .sd_unicast("192.168.1.100".parse().unwrap())
///         .preferred_transport(Transport::Tcp)
///         .start().await?;
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
    pub const fn new(id: u16) -> Option<Self> {
        match id {
            0x0000 | 0xFFFF => None,
            id => Some(Self(id)),
        }
    }

    /// Get the raw value
    pub const fn value(&self) -> u16 {
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
    pub const fn new(id: u16) -> Option<Self> {
        match id {
            0x0000 => None,
            0xFFFF => Some(Self::Any),
            id => Some(Self::Id(id)),
        }
    }

    /// Get the raw value (0xFFFF for Any)
    pub const fn value(&self) -> u16 {
        match self {
            Self::Any => 0xFFFF,
            Self::Id(id) => *id,
        }
    }

    /// Check if this is a wildcard
    pub const fn is_any(&self) -> bool {
        matches!(self, Self::Any)
    }
}

/// Method identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MethodId(u16);

impl MethodId {
    /// Create a new `MethodId`. Valid range: 0x0000-0x7FFF (high bit reserved for events)
    pub const fn new(id: u16) -> Option<Self> {
        if id >= 0x8000 {
            None // High bit set = event, not method
        } else {
            Some(Self(id))
        }
    }

    pub const fn value(&self) -> u16 {
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
    pub const fn new(id: u16) -> Option<Self> {
        if id < 0x8000 || id == 0xFFFF {
            None
        } else {
            Some(Self(id))
        }
    }

    pub const fn value(&self) -> u16 {
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
    pub const fn new(id: u16) -> Option<Self> {
        match id {
            0x0000 | 0xFFFF => None,
            id => Some(Self(id)),
        }
    }

    pub const fn value(&self) -> u16 {
        self.0
    }
}

/// Major version of a service interface - can be exact or wildcard (Any)
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MajorVersion {
    /// Match any major version (0xFF wildcard in SD `FindService`)
    #[default]
    Any,
    /// Specific major version (0x00-0xFE)
    Exact(u8),
}

impl MajorVersion {
    /// Create a specific major version
    pub const fn new(version: u8) -> Self {
        if version == 0xFF {
            Self::Any
        } else {
            Self::Exact(version)
        }
    }

    /// Get the raw value (0xFF for Any)
    pub const fn value(&self) -> u8 {
        match self {
            Self::Any => 0xFF,
            Self::Exact(v) => *v,
        }
    }

    /// Check if this is a wildcard
    pub const fn is_any(&self) -> bool {
        matches!(self, Self::Any)
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
    pub const fn new(version: u32) -> Self {
        Self(version)
    }

    pub const fn value(&self) -> u32 {
        self.0
    }
}

// ============================================================================
// RETURN CODES
// ============================================================================

/// SOME/IP return codes.
///
/// This enum represents every value the 8-bit return code field can carry.
/// Raw bytes are converted exactly once, at the wire boundary, via
/// [`ReturnCode::from_u8`]; everything above the wire layer works with this
/// type.
///
/// For **sending** error responses from a server, use [`ApplicationError`]
/// instead, which only exposes the codes that applications should generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnCode {
    /// No error occurred (0x00)
    Ok,
    /// An unspecified error occurred (0x01)
    NotOk,
    /// The requested Service ID is unknown (0x02) - optional
    UnknownService,
    /// The requested Method ID is unknown (0x03) - optional
    UnknownMethod,
    /// Application not running (0x04) - deprecated
    NotReady,
    /// System not reachable (0x05) - deprecated, internal only
    NotReachable,
    /// Timeout occurred (0x06) - deprecated, internal only
    Timeout,
    /// SOME/IP protocol version not supported (0x07) - obsolete
    WrongProtocolVersion,
    /// Interface version mismatch (0x08)
    WrongInterfaceVersion,
    /// Payload deserialization error (0x09)
    MalformedMessage,
    /// Unexpected message type received (0x0A)
    WrongMessageType,
    /// Service-specific error (0x20-0x3F)
    ServiceSpecific(ServiceErrorCode),
    /// Value reserved by the specification (0x0B-0x1F, 0x40-0xFF).
    ///
    /// Only produced when parsing a received message.
    Reserved(ReservedReturnCode),
}

impl ReturnCode {
    /// Classify a raw return code byte. Total: every byte maps to exactly one
    /// variant, and [`ReturnCode::as_u8`] round-trips it.
    ///
    /// ```
    /// use recentip::ReturnCode;
    ///
    /// assert_eq!(ReturnCode::from_u8(0x00), ReturnCode::Ok);
    /// assert!(matches!(ReturnCode::from_u8(0x21), ReturnCode::ServiceSpecific(c) if c.value() == 0x21));
    /// assert!(matches!(ReturnCode::from_u8(0x40), ReturnCode::Reserved(c) if c.value() == 0x40));
    /// ```
    #[must_use]
    pub const fn from_u8(value: u8) -> Self {
        match value {
            0x00 => Self::Ok,
            0x01 => Self::NotOk,
            0x02 => Self::UnknownService,
            0x03 => Self::UnknownMethod,
            0x04 => Self::NotReady,
            0x05 => Self::NotReachable,
            0x06 => Self::Timeout,
            0x07 => Self::WrongProtocolVersion,
            0x08 => Self::WrongInterfaceVersion,
            0x09 => Self::MalformedMessage,
            0x0A => Self::WrongMessageType,
            ServiceErrorCode::MIN..=ServiceErrorCode::MAX => {
                Self::ServiceSpecific(ServiceErrorCode(value))
            }
            _ => Self::Reserved(ReservedReturnCode(value)),
        }
    }

    /// Get the wire format value for this return code.
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Ok => 0x00,
            Self::NotOk => 0x01,
            Self::UnknownService => 0x02,
            Self::UnknownMethod => 0x03,
            Self::NotReady => 0x04,
            Self::NotReachable => 0x05,
            Self::Timeout => 0x06,
            Self::WrongProtocolVersion => 0x07,
            Self::WrongInterfaceVersion => 0x08,
            Self::MalformedMessage => 0x09,
            Self::WrongMessageType => 0x0A,
            Self::ServiceSpecific(code) => code.value(),
            Self::Reserved(code) => code.value(),
        }
    }
}

impl From<u8> for ReturnCode {
    fn from(value: u8) -> Self {
        Self::from_u8(value)
    }
}

impl From<ReturnCode> for u8 {
    fn from(code: ReturnCode) -> Self {
        code.as_u8()
    }
}

/// A service-specific SOME/IP return code, guaranteed to be in 0x20-0x3F.
///
/// The only ways to obtain one are [`ServiceErrorCode::new`] (validating) or
/// parsing a received message, so an out-of-range value cannot be sent as a
/// service-specific error.
///
/// ```compile_fail
/// use recentip::ServiceErrorCode;
///
/// // The field is private: unvalidated construction does not compile.
/// let _ = ServiceErrorCode(0x00);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServiceErrorCode(u8);

impl ServiceErrorCode {
    /// Lowest service-specific return code (0x20).
    pub const MIN: u8 = 0x20;
    /// Highest service-specific return code (0x3F).
    pub const MAX: u8 = 0x3F;

    /// Validate a service-specific return code.
    ///
    /// Returns `None` if `code` is outside 0x20-0x3F.
    ///
    /// ```
    /// use recentip::ServiceErrorCode;
    ///
    /// assert!(ServiceErrorCode::new(0x20).is_some());
    /// assert!(ServiceErrorCode::new(0x3F).is_some());
    /// assert!(ServiceErrorCode::new(0x1F).is_none());
    /// assert!(ServiceErrorCode::new(0x40).is_none());
    /// ```
    #[must_use]
    pub const fn new(code: u8) -> Option<Self> {
        if code >= Self::MIN && code <= Self::MAX {
            Some(Self(code))
        } else {
            None
        }
    }

    /// Get the wire format value.
    #[must_use]
    pub const fn value(self) -> u8 {
        self.0
    }
}

/// A return code value reserved by the SOME/IP specification
/// (0x0B-0x1F, 0x40-0xFF).
///
/// Only obtainable by parsing a received message via [`ReturnCode::from_u8`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReservedReturnCode(u8);

impl ReservedReturnCode {
    /// Get the wire format value.
    #[must_use]
    pub const fn value(self) -> u8 {
        self.0
    }
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

    /// Payload deserialization error (0x09).
    ///
    /// Use this when you cannot deserialize the request payload.
    /// For example, if the payload doesn't match the expected schema.
    MalformedMessage,

    /// Service-specific error (0x20-0x3F).
    ///
    /// Use this for errors defined by your service's interface specification.
    /// The payload is a [`ServiceErrorCode`], which guarantees the 0x20-0x3F range.
    ///
    /// Create with [`ApplicationError::service_specific()`].
    ServiceSpecific(ServiceErrorCode),
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
    /// Returns `Some(ApplicationError::ServiceSpecific(_))` if the code
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
        match ServiceErrorCode::new(code) {
            Some(code) => Some(Self::ServiceSpecific(code)),
            None => None,
        }
    }

    /// Get the return code carried on the wire for this error.
    #[must_use]
    pub const fn return_code(self) -> ReturnCode {
        match self {
            Self::NotOk => ReturnCode::NotOk,
            Self::UnknownMethod => ReturnCode::UnknownMethod,
            Self::MalformedMessage => ReturnCode::MalformedMessage,
            Self::ServiceSpecific(code) => ReturnCode::ServiceSpecific(code),
        }
    }

    /// Get the wire format value for this error.
    #[must_use]
    pub const fn as_u8(&self) -> u8 {
        self.return_code().as_u8()
    }
}

impl From<ApplicationError> for ReturnCode {
    fn from(error: ApplicationError) -> Self {
        error.return_code()
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
    pub address: SocketAddrV4,
    /// Transport used by the client
    pub transport: crate::config::Transport,
}

#[derive(Debug, Clone)]
pub enum OfferedEndpoints {
    UdpOnly(SocketAddrV4),
    TcpOnly(SocketAddrV4),
    Both {
        udp: SocketAddrV4,
        tcp: SocketAddrV4,
    },
}

// ============================================================================
// RE-EXPORTS
// ============================================================================

pub mod prelude {
    pub use crate::config::{
        DEFAULT_SD_PORT, InvalidAddressError, MulticastAddress, UnicastAddress,
    };
    pub use crate::{
        ApplicationError, Error, Event, EventBuilder, EventHandle, EventId, EventgroupId,
        InstanceId, MajorVersion, MethodConfig, MethodId, MinorVersion, OfferedService, PortSpec,
        Response, Result, ReturnCode, RuntimeConfig, ServiceErrorCode, ServiceId, ServiceOffering,
        SomeIp, SomeIpBuilder, Subscription, SubscriptionBuilder, Transport, TransportPolicy,
        TransportPreference, TransportSelection, configure,
    };
}
