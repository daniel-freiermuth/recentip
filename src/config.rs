//! # SOME/IP Configuration Types
//!
//! This module provides configuration types and constants for the SOME/IP runtime.
//!
//! For building and starting a runtime, see [`SomeIpBuilder`](crate::SomeIpBuilder) in
//! the [`builder`](crate::builder) module.
//!
//! ## Configuration Options Reference
//!
//! | Option | Default | Description |
//! |--------|---------|-------------|
//! | `bind_addr` | `0.0.0.0:30490` | Local address to bind SD socket |
//! | `advertised_ip` | None | Routable IP for endpoint options (required for subscriptions) |
//! | `sd_multicast` | `239.255.0.1:30490` | SD multicast group address |
//! | `offer_ttl` | 3600 | TTL for `OfferService` entries (seconds) |
//! | `find_ttl` | 3600 | TTL for `FindService` entries (seconds) |
//! | `subscribe_ttl` | 3600 | TTL for `SubscribeEventgroup` entries (seconds) |
//! | `cyclic_offer_delay` | 1000 | Interval between cyclic offers (ms) |
//! | `preferred_transport` | UDP | Preferred transport when service offers both |
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
use tracing::warn;

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

/// Infinite TTL value (0xFFFFFF = ~194 days).
///
/// Per SOME/IP-SD specification, TTL=0xFFFFFF means "until next reboot" -
/// the subscription or offer should never expire due to TTL timeout.
/// This is the maximum value that fits in the 24-bit TTL field.
///
/// See: `feat_req_someipsd_431`
pub const SD_TTL_INFINITE: u32 = 0xFF_FFFF;

/// Clamp a TTL value to the 24-bit maximum, logging a warning if truncation occurs.
///
/// SOME/IP-SD uses 24-bit TTL fields. Values exceeding 0xFFFFFF would be silently
/// truncated during serialization, potentially causing service announcements to
/// be misinterpreted as stop/nack messages (TTL=0).
pub fn clamp_ttl_to_24bit(ttl: u32, field_name: &str) -> u32 {
    if ttl > SD_TTL_INFINITE {
        warn!(
            "{} value {} exceeds 24-bit maximum ({}), clamping to SD_TTL_INFINITE",
            field_name, ttl, SD_TTL_INFINITE
        );
        SD_TTL_INFINITE
    } else {
        ttl
    }
}

/// Default TTL for `OfferService` entries in seconds (1 hour).
pub const DEFAULT_OFFER_TTL: u32 = 3600;

/// Default TTL for `FindService` entries in seconds (1 hour).
pub const DEFAULT_FIND_TTL: u32 = 3600;

/// Default TTL for `SubscribeEventgroup` entries in seconds (1 hour).
pub const DEFAULT_SUBSCRIBE_TTL: u32 = 3600;

/// Default cyclic offer interval in milliseconds (1 second).
///
/// Servers send periodic `OfferService` messages at this interval.
/// Lower values = faster discovery, higher network overhead.
pub const DEFAULT_CYCLIC_OFFER_DELAY: u64 = 1000;

/// Default number of `FindService` repetitions.
///
/// Clients repeat `FindService` messages this many times before giving up
/// on discovery and waiting for server offers.
pub const DEFAULT_FIND_REPETITIONS: u32 = 3;

/// Transport protocol for RPC communication.
///
/// Service Discovery always uses UDP multicast regardless of this setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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

/// `SomeIp` configuration
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Local address to bind to (default: 0.0.0.0:30490)
    pub bind_addr: SocketAddr,
    /// Advertised IP address for endpoint options (default: None)
    ///
    /// This is the routable IP address that will be included in SD endpoint options
    /// when subscribing to services. Must be a valid, non-unspecified address.
    ///
    /// - In turmoil tests: Use `turmoil::lookup(hostname)` to get the host's IP
    /// - On real networks: Use the actual interface IP address
    ///
    /// If not set, will attempt to use the IP from `bind_addr`, but `bind_addr`
    /// cannot be 0.0.0.0 (unspecified) if you need to subscribe to services.
    /// See module documentation for full fallback behavior.
    pub advertised_ip: Option<std::net::IpAddr>,
    /// SD multicast group address (default: 239.255.0.1:30490)
    pub sd_multicast: SocketAddr,
    /// TTL for `OfferService` entries (default: 3600 seconds)
    pub offer_ttl: u32,
    /// TTL for `FindService` entries (default: 3600 seconds)
    pub find_ttl: u32,
    /// TTL for `SubscribeEventgroup` entries (default: 3600 seconds)
    pub subscribe_ttl: u32,
    /// Cyclic offer delay in ms (default: 1000)
    pub cyclic_offer_delay: u64,
    /// Preferred transport protocol when a service advertises both (default: UDP)
    ///
    /// When a remote service offers both TCP and UDP endpoints, this setting
    /// determines which endpoint the client will use for RPC calls.
    pub preferred_transport: Transport,
    /// Enable Magic Cookies for TCP resynchronization (default: false)
    ///
    /// When enabled (`feat_req_someip_586`, `feat_req_someip_591`, `feat_req_someip_592)`:
    /// - Each TCP segment starts with a Magic Cookie message
    /// - Only one Magic Cookie per segment
    /// - Allows resync in testing/debugging scenarios
    pub magic_cookies: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            // Use the SD port for multicast group membership to work in turmoil
            bind_addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_SD_PORT)),
            advertised_ip: None,
            sd_multicast: SocketAddr::V4(SocketAddrV4::new(DEFAULT_SD_MULTICAST, DEFAULT_SD_PORT)),
            offer_ttl: DEFAULT_OFFER_TTL,
            find_ttl: DEFAULT_FIND_TTL,
            subscribe_ttl: DEFAULT_SUBSCRIBE_TTL,
            cyclic_offer_delay: DEFAULT_CYCLIC_OFFER_DELAY,
            preferred_transport: Transport::Udp,
            magic_cookies: false,
        }
    }
}

// ============================================================================
// METHOD CONFIGURATION
// ============================================================================

/// Configuration for how a service handles error responses.
///
/// Per SOME/IP specification (`feat_req_someip_106`, `feat_req_someip_726)`:
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

// ============================================================================
// OFFER CONFIGURATION
// ============================================================================

/// Configuration for service transport endpoints.
///
/// Specifies which transports (TCP and/or UDP) a service should be offered on,
/// and optionally custom ports for each transport.
///
/// # Example
/// ```
/// use recentip::config::OfferConfig;
///
/// // Offer on both TCP and UDP with custom ports
/// let config = OfferConfig::new()
///     .tcp_port(30501)
///     .udp_port(30502);
///
/// // Offer on TCP only with default port
/// let tcp_only = OfferConfig::new().tcp();
///
/// // Offer on UDP only (default behavior)
/// let udp_only = OfferConfig::new().udp();
/// ```
#[derive(Debug, Clone, Default)]
#[must_use]
pub struct OfferConfig {
    /// TCP port to offer on (None = not offered via TCP, Some(0) = use default)
    pub tcp_port: Option<u16>,
    /// UDP port to offer on (None = not offered via UDP, Some(0) = use default)
    pub udp_port: Option<u16>,
    /// Method-specific configuration (exception handling, etc.)
    pub method_config: MethodConfig,
}

impl OfferConfig {
    /// Create a new empty offer configuration.
    ///
    /// By default, no transports are configured. You must call at least one of
    /// `.tcp()`, `.udp()`, `.tcp_port()`, or `.udp_port()` before starting.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable TCP transport with the default RPC port (30491).
    pub const fn tcp(mut self) -> Self {
        self.tcp_port = Some(0); // 0 means use default
        self
    }

    /// Enable TCP transport with a specific port.
    pub const fn tcp_port(mut self, port: u16) -> Self {
        self.tcp_port = Some(port);
        self
    }

    /// Enable UDP transport with the default RPC port (30491).
    pub const fn udp(mut self) -> Self {
        self.udp_port = Some(0); // 0 means use default
        self
    }

    /// Enable UDP transport with a specific port.
    pub const fn udp_port(mut self, port: u16) -> Self {
        self.udp_port = Some(port);
        self
    }

    /// Configure method-specific behavior (e.g., exception handling).
    pub fn method_config(mut self, config: MethodConfig) -> Self {
        self.method_config = config;
        self
    }

    /// Check if any transport is configured.
    pub const fn has_transport(&self) -> bool {
        self.tcp_port.is_some() || self.udp_port.is_some()
    }

    /// Check if TCP is enabled.
    pub const fn has_tcp(&self) -> bool {
        self.tcp_port.is_some()
    }

    /// Check if UDP is enabled.
    pub const fn has_udp(&self) -> bool {
        self.udp_port.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ttl_clamping_to_24bit_max() {
        // Values within 24-bit range should pass through unchanged
        assert_eq!(clamp_ttl_to_24bit(0, "test"), 0);
        assert_eq!(clamp_ttl_to_24bit(3600, "test"), 3600);
        assert_eq!(clamp_ttl_to_24bit(SD_TTL_INFINITE, "test"), SD_TTL_INFINITE);

        // Values exceeding 24-bit max should be clamped to SD_TTL_INFINITE
        assert_eq!(clamp_ttl_to_24bit(0x01_00_00_00, "test"), SD_TTL_INFINITE);
        assert_eq!(clamp_ttl_to_24bit(u32::MAX, "test"), SD_TTL_INFINITE);
    }
}
