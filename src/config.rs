//! # Runtime Configuration
//!
//! This module provides configuration types for the SOME/IP runtime.
//!
//! ## Quick Start
//!
//! For most applications, the defaults work out of the box:
//!
//! ```no_run
//! use someip_runtime::{Runtime, RuntimeConfig};
//!
//! # async fn example() -> someip_runtime::Result<()> {
//! let config = RuntimeConfig::default();
//! let runtime = Runtime::new(config).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Builder Pattern
//!
//! For custom configurations, use the builder:
//!
//! ```
//! use someip_runtime::{RuntimeConfig, Transport};
//! use std::net::{SocketAddr, SocketAddrV4, Ipv4Addr};
//!
//! let config = RuntimeConfig::builder()
//!     .local_addr(SocketAddr::V4(SocketAddrV4::new(
//!         Ipv4Addr::new(192, 168, 1, 100),
//!         30490
//!     )))
//!     .transport(Transport::Tcp)
//!     .ttl(1800)  // 30 minutes
//!     .cyclic_offer_delay(2000)  // 2 seconds
//!     .magic_cookies(true)  // Enable for debugging
//!     .build();
//! ```
//!
//! ## Configuration Options
//!
//! | Option | Default | Description |
//! |--------|---------|-------------|
//! | `local_addr` | `0.0.0.0:30490` | Local address to bind SD socket |
//! | `sd_multicast` | `239.255.0.1:30490` | SD multicast group address |
//! | `ttl` | 3600 | TTL for SD entries (seconds) |
//! | `cyclic_offer_delay` | 1000 | Interval between cyclic offers (ms) |
//! | `transport` | UDP | Transport protocol for RPC (UDP/TCP) |
//! | `magic_cookies` | false | Enable TCP Magic Cookies for debugging |
//!
//! ## Transport Selection
//!
//! SOME/IP supports both UDP and TCP for RPC communication:
//!
//! - **UDP** (default): Lower latency, no connection overhead. Payload limited
//!   to ~1400 bytes without SOME/IP-TP segmentation.
//! - **TCP**: Reliable delivery, supports large payloads, connection reuse.
//!   Higher latency due to connection setup.
//!
//! Service Discovery always uses UDP multicast.

use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

/// Default SD multicast address (239.255.0.1) per SOME/IP specification.
pub const DEFAULT_SD_MULTICAST: Ipv4Addr = Ipv4Addr::new(239, 255, 0, 1);

/// Default SD port (30490) per SOME/IP specification.
///
/// Note: This port is **only for Service Discovery**, not RPC traffic.
/// RPC uses ephemeral ports for clients or configured ports for servers.
pub const DEFAULT_SD_PORT: u16 = 30490;

/// Default TTL for SD entries in seconds (1 hour).
///
/// Services re-announce before TTL expiry to maintain presence.
/// Clients remove services from cache when TTL expires without renewal.
pub const DEFAULT_TTL: u32 = 3600;

/// Default cyclic offer interval in milliseconds (1 second).
///
/// Servers send periodic OfferService messages at this interval.
/// Lower values = faster discovery, higher network overhead.
pub const DEFAULT_CYCLIC_OFFER_DELAY: u64 = 1000;

/// Default number of FindService repetitions.
///
/// Clients repeat FindService messages this many times before giving up
/// on discovery and waiting for server offers.
pub const DEFAULT_FIND_REPETITIONS: u32 = 3;

/// Transport protocol for RPC communication.
///
/// Service Discovery always uses UDP multicast regardless of this setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transport {
    /// UDP transport (default).
    ///
    /// Lower latency, connectionless. Best for small, frequent messages.
    /// Payload size limited to ~1400 bytes without SOME/IP-TP.
    #[default]
    Udp,

    /// TCP transport.
    ///
    /// Reliable, connection-oriented. Best for large payloads or when
    /// guaranteed delivery is required. Connections are pooled and reused.
    Tcp,
}

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
    /// Transport protocol for RPC communication (default: UDP)
    pub transport: Transport,
    /// Enable Magic Cookies for TCP resynchronization (default: false)
    ///
    /// When enabled (feat_req_recentip_586, feat_req_recentip_591, feat_req_recentip_592):
    /// - Each TCP segment starts with a Magic Cookie message
    /// - Only one Magic Cookie per segment
    /// - Allows resync in testing/debugging scenarios
    pub magic_cookies: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            // Use the SD port for multicast group membership to work in turmoil
            local_addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_SD_PORT)),
            sd_multicast: SocketAddr::V4(SocketAddrV4::new(DEFAULT_SD_MULTICAST, DEFAULT_SD_PORT)),
            ttl: DEFAULT_TTL,
            cyclic_offer_delay: DEFAULT_CYCLIC_OFFER_DELAY,
            transport: Transport::Udp,
            magic_cookies: false,
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

    /// Set the transport protocol (UDP or TCP)
    pub fn transport(mut self, transport: Transport) -> Self {
        self.config.transport = transport;
        self
    }

    /// Enable or disable Magic Cookies for TCP (default: false)
    ///
    /// Magic Cookies allow resynchronization in testing/debugging scenarios.
    /// See feat_req_recentip_586, feat_req_recentip_591, feat_req_recentip_592.
    pub fn magic_cookies(mut self, enabled: bool) -> Self {
        self.config.magic_cookies = enabled;
        self
    }

    /// Build the configuration
    pub fn build(self) -> RuntimeConfig {
        self.config
    }
}

// ============================================================================
// METHOD CONFIGURATION
// ============================================================================

/// Configuration for how a service handles error responses.
///
/// Per SOME/IP specification (feat_req_recentip_106, feat_req_recentip_726):
/// - By default, errors use RESPONSE (0x80) with non-OK return code
/// - EXCEPTION (0x81) is optional and must be explicitly configured per-method
///
/// This configuration is typically defined in the interface specification (IDL/FIDL)
/// at design time, not decided per-call at runtime.
#[derive(Debug, Clone, Default)]
pub struct MethodConfig {
    /// Set of method IDs that use EXCEPTION (0x81) message type for errors.
    /// Methods not in this set use RESPONSE (0x80) with error return code.
    exception_methods: HashSet<u16>,
}

impl MethodConfig {
    /// Create a new empty configuration (all methods use RESPONSE for errors)
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure a method to use EXCEPTION (0x81) message type for errors.
    ///
    /// Per spec, this should match the interface specification for the method.
    pub fn use_exception_for(mut self, method_id: u16) -> Self {
        self.exception_methods.insert(method_id);
        self
    }

    /// Check if a method uses EXCEPTION message type for errors.
    pub fn uses_exception(&self, method_id: u16) -> bool {
        self.exception_methods.contains(&method_id)
    }
}
