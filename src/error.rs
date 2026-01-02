//! Error types for someip-runtime.

use std::fmt;
use std::io;

/// Result type for someip-runtime operations
pub type Result<T> = std::result::Result<T, Error>;

/// Top-level error type
#[derive(Debug)]
pub enum Error {
    /// I/O error from network operations
    Io(io::Error),
    /// Configuration error
    Config(ConfigError),
    /// Protocol-level error
    Protocol(ProtocolError),
    /// Service is not available
    ServiceUnavailable,
    /// Not subscribed to the eventgroup
    NotSubscribed,
    /// The runtime has shut down
    RuntimeShutdown,
    /// Channel full, event was dropped
    ChannelFull,
    /// Service/instance is already offered or bound
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
