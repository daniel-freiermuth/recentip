//! # SOME/IP Configuration Types
//!
//! This module provides configuration types and constants for the SOME/IP runtime.
//!
//! For building and starting a runtime, see [`SomeIpBuilder`](crate::SomeIpBuilder) in
//! the [`builder`](crate::builder) module.
//!
//! ## Configuration Options Reference
//!
//! | Option | Description |
//! |--------|-------------|
//! | `sd_port` | Service Discovery UDP port (mandatory) |
//! | `sd_multicast` | SD multicast group address (mandatory) |
//! | `sd_unicast` | Routable IP advertised in SD endpoint options (mandatory) |
//! | `offer_ttl` | TTL for `OfferService` entries (seconds) |
//! | `find_ttl` | TTL for `FindService` entries (seconds) |
//! | `subscribe_ttl` | TTL for `SubscribeEventgroup` entries (seconds) |
//! | `cyclic_offer_delay` | Interval between cyclic offers (ms) |
//! | `preferred_transport` | Preferred transport when service offers both |
//! | `magic_cookies` | Enable TCP Magic Cookies for debugging |
//!
//! ## Socket Modes
//!
//! **Dual-socket mode** (default): `mc_socket` binds to the multicast group address;
//! `uc_socket` binds to `<sd_unicast>:<sd_port>`.  Provides host-level non-interference.
//!
//! **Single-socket mode**: opt-in via [`single_socket`](crate::SomeIpBuilder::single_socket).
//! One socket binds to `0.0.0.0:<sd_port>`.  Use on platforms without multicast bind
//! support.  `sd_unicast` is still required for SD endpoint option advertisement.
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
use std::fmt;
use std::net::{Ipv4Addr, SocketAddrV4};
use tracing::warn;

// ============================================================================
// VALIDATED ADDRESS TYPES
// ============================================================================

/// A validated IPv4 multicast address (224.0.0.0/4).
///
/// Only constructible via [`TryFrom<Ipv4Addr>`], guaranteeing the address is
/// in the multicast range. Eliminates repeated `is_multicast()` checks in
/// runtime code.
///
/// # Example
///
/// ```
/// use std::net::Ipv4Addr;
/// use recentip::config::MulticastAddress;
///
/// let addr = MulticastAddress::try_from(Ipv4Addr::new(239, 255, 255, 250)).unwrap();
/// assert_eq!(addr.get(), Ipv4Addr::new(239, 255, 255, 250));
///
/// assert!(MulticastAddress::try_from(Ipv4Addr::LOCALHOST).is_err());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MulticastAddress(Ipv4Addr);

impl MulticastAddress {
    /// Returns the inner IPv4 address.
    #[inline]
    pub const fn get(self) -> Ipv4Addr {
        self.0
    }

    pub const fn static_try_from(ip: Ipv4Addr) -> Option<Self> {
        if ip.is_multicast() {
            Some(Self(ip))
        } else {
            None
        }
    }
}

impl TryFrom<Ipv4Addr> for MulticastAddress {
    type Error = InvalidAddressError;
    fn try_from(ip: Ipv4Addr) -> std::result::Result<Self, Self::Error> {
        if ip.is_multicast() {
            Ok(Self(ip))
        } else {
            Err(InvalidAddressError::NotMulticast(ip))
        }
    }
}

impl TryFrom<std::net::IpAddr> for MulticastAddress {
    type Error = InvalidAddressError;
    fn try_from(ip: std::net::IpAddr) -> std::result::Result<Self, Self::Error> {
        match ip {
            std::net::IpAddr::V4(v4) => Self::try_from(v4),
            std::net::IpAddr::V6(_) => Err(InvalidAddressError::ParseError),
        }
    }
}

impl std::str::FromStr for MulticastAddress {
    type Err = InvalidAddressError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        s.parse::<Ipv4Addr>()
            .map_err(|_| InvalidAddressError::ParseError)
            .and_then(Self::try_from)
    }
}

impl From<MulticastAddress> for Ipv4Addr {
    fn from(addr: MulticastAddress) -> Self {
        addr.get()
    }
}

impl From<MulticastAddress> for std::net::IpAddr {
    fn from(addr: MulticastAddress) -> Self {
        Self::V4(addr.get())
    }
}

impl fmt::Display for MulticastAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A validated IPv4 unicast address (not multicast, not unspecified).
///
/// Only constructible via [`TryFrom<Ipv4Addr>`]. Guarantees the address is
/// routable and suitable for SD endpoint advertisement. Eliminates
/// `is_unspecified()` / `is_multicast()` guard checks in runtime code.
///
/// # Example
///
/// ```
/// use std::net::Ipv4Addr;
/// use recentip::config::UnicastAddress;
///
/// let addr = UnicastAddress::try_from(Ipv4Addr::LOCALHOST).unwrap();
/// assert_eq!(addr.get(), Ipv4Addr::LOCALHOST);
///
/// assert!(UnicastAddress::try_from(Ipv4Addr::UNSPECIFIED).is_err());
/// assert!(UnicastAddress::try_from(Ipv4Addr::new(239, 255, 0, 1)).is_err());
/// assert!(UnicastAddress::try_from(Ipv4Addr::BROADCAST).is_err());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnicastAddress(Ipv4Addr);

impl UnicastAddress {
    /// Returns the inner IPv4 address.
    #[inline]
    pub const fn get(self) -> Ipv4Addr {
        self.0
    }
}

impl TryFrom<Ipv4Addr> for UnicastAddress {
    type Error = InvalidAddressError;
    fn try_from(ip: Ipv4Addr) -> std::result::Result<Self, Self::Error> {
        if ip.is_unspecified() {
            Err(InvalidAddressError::Unspecified)
        } else if ip.is_multicast() {
            Err(InvalidAddressError::IsMulticast(ip))
        } else if ip.is_broadcast() {
            Err(InvalidAddressError::IsBroadcast)
        } else {
            Ok(Self(ip))
        }
    }
}

impl TryFrom<std::net::IpAddr> for UnicastAddress {
    type Error = InvalidAddressError;
    fn try_from(ip: std::net::IpAddr) -> std::result::Result<Self, Self::Error> {
        match ip {
            std::net::IpAddr::V4(v4) => Self::try_from(v4),
            std::net::IpAddr::V6(_) => Err(InvalidAddressError::ParseError),
        }
    }
}

impl std::str::FromStr for UnicastAddress {
    type Err = InvalidAddressError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        s.parse::<Ipv4Addr>()
            .map_err(|_| InvalidAddressError::ParseError)
            .and_then(Self::try_from)
    }
}

impl From<UnicastAddress> for Ipv4Addr {
    fn from(addr: UnicastAddress) -> Self {
        addr.get()
    }
}

impl fmt::Display for UnicastAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Error returned when an address fails validation for a specific role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidAddressError {
    /// The address must be in the multicast range `224.0.0.0/4`.
    NotMulticast(Ipv4Addr),
    /// The address must not be a multicast address.
    IsMulticast(Ipv4Addr),
    /// The address must not be unspecified (`0.0.0.0`).
    Unspecified,
    /// The address must not be the limited broadcast address (`255.255.255.255`).
    IsBroadcast,
    /// The string could not be parsed as a valid IPv4 address.
    ParseError,
}

impl fmt::Display for InvalidAddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotMulticast(ip) => write!(
                f,
                "{ip} is not a multicast address (must be in 224.0.0.0/4)"
            ),
            Self::IsMulticast(ip) => {
                write!(
                    f,
                    "{ip} is a multicast address; a unicast address is required"
                )
            }
            Self::Unspecified => write!(
                f,
                "0.0.0.0 is not a valid unicast address; specify a routable IP"
            ),
            Self::IsBroadcast => write!(f, "255.255.255.255 is not a valid unicast address"),
            Self::ParseError => write!(f, "not a valid IPv4 address"),
        }
    }
}

impl std::error::Error for InvalidAddressError {}

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
///
/// Constructed via [`SomeIpBuilder`](crate::SomeIpBuilder); not intended for
/// direct construction outside the builder.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Service Discovery UDP port (e.g. `30490`)
    pub sd_port: u16,
    /// SD multicast group address (e.g. `239.255.255.250`)
    pub sd_multicast: MulticastAddress,
    /// Routable unicast IP advertised in SD endpoint options.
    ///
    /// This IP is embedded in `OfferService` and `SubscribeEventgroup` endpoint
    /// options so remote peers know where to send unicast SD messages and RPC
    /// traffic.  In dual-socket mode (default) this address is also used as the
    /// bind address for the dedicated unicast SD socket.
    pub sd_unicast: UnicastAddress,
    /// Use single-socket mode: bind one socket to `0.0.0.0:<sd_port>` instead
    /// of the dual-socket layout.  `sd_unicast` is still used for endpoint
    /// advertisement.  Use on platforms that do not support multicast-address
    /// binds.
    pub single_socket: bool,
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

impl RuntimeConfig {
    /// Full SD socket address = multicast group IP + SD port.
    pub(crate) const fn sd_multicast_addr(&self) -> SocketAddrV4 {
        SocketAddrV4::new(self.sd_multicast.get(), self.sd_port)
    }

    /// Unicast SD address.
    ///
    /// Always valid — `sd_unicast` is a [`UnicastAddress`], guaranteeing
    /// the address is non-unspecified and routable.
    pub(crate) const fn unicast_ip(&self) -> Ipv4Addr {
        self.sd_unicast.get()
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
