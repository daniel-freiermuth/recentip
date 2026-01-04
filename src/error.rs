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
//! | [`Error::RuntimeShutdown`] | Runtime was dropped | No (restart app) |
//! | [`Error::ChannelFull`] | Event dropped due to backpressure | Yes (slow down) |
//! | [`Error::AlreadyOffered`] | Duplicate offer attempt | Yes (use existing) |
//!
//! ## Usage Pattern
//!
//! ```no_run
//! use someip_runtime::prelude::*;
//! use someip_runtime::handle::{ProxyHandle, Available};
//!
//! struct MyService;
//! impl Service for MyService {
//!     const SERVICE_ID: u16 = 0x1234;
//!     const MAJOR_VERSION: u8 = 1;
//!     const MINOR_VERSION: u32 = 0;
//! }
//!
//! async fn call_with_error_handling(proxy: &ProxyHandle<MyService, Available>) -> Result<()> {
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
    /// Occurs when the [`Runtime`](crate::Runtime) is dropped while
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
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {}", e),
            Error::Config(e) => write!(f, "Configuration error: {}", e.message),
            Error::Protocol(e) => write!(f, "Protocol error: {}", e.message),
            Error::ServiceUnavailable => write!(f, "Service unavailable"),
            Error::NotSubscribed => write!(f, "Not subscribed to eventgroup"),
            Error::RuntimeShutdown => write!(f, "Runtime has shut down"),
            Error::ChannelFull => write!(f, "Channel full, event dropped"),
            Error::AlreadyOffered => write!(f, "Service/instance is already offered or bound"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
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
