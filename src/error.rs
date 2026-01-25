//! # Error Types
//!
//! This module defines all error types used throughout the library.
//!
//! ## Error Hierarchy
//!
//! The main [`Error`] enum covers all possible failure modes:
//!
//! | Variant | Cause | Recoverable? |
//! |---------|-------|--------------|
//! | [`Error::Io`] | Network I/O failure | Maybe (retry) |
//! | [`Error::Config`] | Invalid configuration | No (fix config) |
//! | [`Error::Protocol`] | Wire protocol violation | No (bug/incompatibility) |
//! | [`Error::ServiceUnavailable`] | Service not found/offline | Maybe (retry later) |
//! | [`Error::NotSubscribed`] | Eventgroup not subscribed | Yes (subscribe first) |
//! | [`Error::RuntimeShutdown`] | `SomeIp` was dropped | No (restart app) |
//! | [`Error::ChannelFull`] | Event dropped due to backpressure | Yes (slow down) |
//! | [`Error::AlreadyOffered`] | Duplicate offer attempt | Yes (use existing) |
//! | [`Error::SubscriptionRejected`] | Server rejected subscription | Maybe (check eventgroup) |
//! | [`Error::NotAvailable`] | Service discovery timed out | Yes (retry later) |
//!
//! ## Usage Pattern
//!
//! ```no_run
//! use recentip::prelude::*;
//! use recentip::handle::OfferedService;
//!
//! async fn call_with_error_handling(proxy: &OfferedService) -> Result<()> {
//!     let method = MethodId::new(0x0001).unwrap();
//!     let payload = b"request";
//!
//!     match proxy.call(method, payload).await {
//!         Ok(response) => { /* success */ }
//!         Err(Error::ServiceUnavailable) => {
//!             // Service went offline, maybe retry
//!         }
//!         Err(Error::RuntimeShutdown) => {
//!             // Application is shutting down
//!             return Err(Error::RuntimeShutdown);
//!         }
//!         Err(e) => {
//!             // Log and propagate
//!             eprintln!("Call failed: {}", e);
//!             return Err(e);
//!         }
//!     }
//!     Ok(())
//! }
//! # fn main() {}
//! ```

use std::fmt;
use std::io;

/// Result type alias using the library's [`Error`] type.
pub type Result<T> = std::result::Result<T, Error>;

/// Top-level error type for all library operations.
///
/// This enum covers all possible failure modes. Use pattern matching
/// to handle specific error cases.
#[derive(Debug)]
pub enum Error {
    /// Network I/O error (socket operations, connection failures).
    ///
    /// Wraps a [`std::io::Error`]. May be transient (connection refused)
    /// or permanent (address already in use).
    Io(io::Error),

    /// Configuration error (invalid addresses, ports, etc.).
    ///
    /// Indicates a problem with the provided [`RuntimeConfig`](crate::RuntimeConfig).
    /// Fix the configuration and restart.
    Config(ConfigError),

    /// Protocol-level error (malformed messages, version mismatch).
    ///
    /// Indicates incompatibility with the remote peer or a bug.
    /// Check protocol versions and message formats.
    Protocol(ProtocolError),

    /// The requested service is not available.
    ///
    /// Causes:
    /// - Service hasn't been discovered yet
    /// - Service went offline (TTL expired)
    /// - No server is offering this service
    ServiceUnavailable,

    /// Not subscribed to the eventgroup.
    ///
    /// Call `subscribe()` before trying to receive events.
    NotSubscribed,

    /// The runtime has shut down.
    ///
    /// Occurs when the [`SomeIp`](crate::SomeIp) is dropped while
    /// operations are pending. Application is terminating.
    RuntimeShutdown,

    /// Channel full, event was dropped due to backpressure.
    ///
    /// The receiver isn't consuming events fast enough. Consider:
    /// - Processing events faster
    /// - Increasing channel capacity
    /// - Dropping old events
    ChannelFull,

    /// Service/instance is already offered or bound.
    ///
    /// A service can only be offered once per instance ID. Use the
    /// existing [`OfferingHandle`](crate::handle::OfferingHandle).
    AlreadyOffered,

    /// Subscription was rejected by the server.
    ///
    /// The server sent a `SubscribeEventgroupNack` (TTL=0) in response
    /// to our subscription request.
    SubscriptionRejected,

    /// Service discovery failed - the service could not be found.
    ///
    /// Returned by [`ProxyHandle::available()`](crate::handle::ProxyHandle::available)
    /// when the find request expires without locating the service.
    ///
    /// Causes:
    /// - No server is offering the requested service/instance
    /// - Network issues preventing SD messages from being received
    /// - Find request exhausted all repetitions (default: 3)
    ///
    /// This is recoverable - you can retry the discovery later.
    NotAvailable,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Config(e) => write!(f, "Configuration error: {}", e.message),
            Self::Protocol(e) => write!(f, "Protocol error: {}", e.message),
            Self::ServiceUnavailable => write!(f, "Service unavailable"),
            Self::NotSubscribed => write!(f, "Not subscribed to eventgroup"),
            Self::RuntimeShutdown => write!(f, "SomeIp has shut down"),
            Self::ChannelFull => write!(f, "Channel full, event dropped"),
            Self::AlreadyOffered => write!(f, "Service/instance is already offered or bound"),
            Self::SubscriptionRejected => write!(f, "Subscription rejected by server"),
            Self::NotAvailable => write!(f, "Service could not be found"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Configuration error
#[derive(Debug)]
pub struct ConfigError {
    pub message: String,
}

impl ConfigError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Protocol-level error
#[derive(Debug)]
pub struct ProtocolError {
    pub message: String,
}

impl ProtocolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
