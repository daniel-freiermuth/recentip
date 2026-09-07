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
use std::time::Duration;
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

/// A single entry in a [`TransportPolicy`]: which transport to try and which
/// local port to bind the client socket to.
///
/// Construct entries with the factory methods [`tcp()`](TransportPreference::tcp)
/// and [`udp()`](TransportPreference::udp), then chain
/// [`with_port`](TransportPreference::with_port) or
/// \[`with_port_range`\] to add a port constraint (the latter is planned, prepared, but commented out).
///
/// # Example
///
/// ```
/// use recentip::config::{TransportPreference, TransportPolicy};
///
/// let policy = TransportPolicy::new(vec![
///     TransportPreference::udp().with_port(30500),  // UDP, bind client to 30500
///     TransportPreference::tcp(),                    // TCP, any ephemeral port
/// ]);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransportPreference {
    /// Which server transport to accept.
    pub transport: Transport,
    /// Which local port to bind the client socket to for this transport.
    ///
    /// For UDP subscriptions this is the source port of the outbound datagram
    /// socket.  For TCP subscriptions, [`PortSpec::Fixed`] binds the TCP
    /// socket to the specified port before connecting (via
    /// `TcpSocket::bind` + `connect`), which is supported on real tokio
    /// sockets but silently ignored in turmoil simulation.
    pub local_port: PortSpec,
}

impl TransportPreference {
    /// Accept any TCP endpoint; client socket uses an ephemeral port.
    pub const fn tcp() -> Self {
        Self {
            transport: Transport::Tcp,
            local_port: PortSpec::Any,
        }
    }

    /// Accept any UDP endpoint; client socket uses an ephemeral port.
    pub const fn udp() -> Self {
        Self {
            transport: Transport::Udp,
            local_port: PortSpec::Any,
        }
    }

    /// Bind the client subscription socket to a fixed local port.
    #[must_use]
    pub const fn with_port(self, port: u16) -> Self {
        Self {
            local_port: PortSpec::Fixed(port),
            ..self
        }
    }

    /*
    /// Try local ports `start..=end` (inclusive) until binding succeeds.
    #[must_use]
    pub const fn with_port_range(self, start: u16, end: u16) -> Self {
        Self { local_port: PortSpec::Range(start, end), ..self }
    } */
}

/// Specifies which local port(s) a client UDP subscription socket may bind to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PortSpec {
    /// Let the OS pick an ephemeral port (default).
    #[default]
    Any,
    /// Bind to exactly this port; fails with `AddrInUse` if taken.
    Fixed(u16),
    /*
    /// Try each port in `start..=end` (inclusive) until one succeeds.
    Range(u16, u16),
    */
}

/// Result of applying a [`TransportPolicy`] to a set of offered endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportSelection {
    /// The server's endpoint to connect to.
    pub remote_endpoint: std::net::SocketAddrV4,
    /// Transport protocol to use.
    pub transport: Transport,
    /// Client-side port spec for the subscription socket (ignored for TCP).
    pub local_port: PortSpec,
}

/// Ordered list of transport preferences for endpoint selection.
///
/// The first entry in the list that matches the server's offered endpoints
/// determines the effective transport and client-side port binding.
/// If no entry matches, the connection attempt falls back to any available
/// endpoint with an ephemeral port.
///
/// # Common Policies
///
/// | Policy | Meaning |
/// |--------|---------|
/// | `[tcp(), udp()]` ← `preferred_transport(Tcp)` | prefer TCP, fall back to UDP |
/// | `[udp(), tcp()]` ← `preferred_transport(Udp)` | prefer UDP, fall back to TCP |
/// | `[tcp()]` | TCP only (fail-fast if server does not offer TCP at runtime) |
/// | `[udp().with_port(30500)]` | UDP, client binds to port 30500 |
/// | `[udp().with_port_range(30500, 30510), tcp()]` | UDP on range, fall back to TCP |
///
/// # Creating Policies
///
/// ```
/// use recentip::config::{TransportPolicy, TransportPreference};
///
/// // Prefer TCP, fall back to UDP
/// let policy = TransportPolicy::prefer_tcp();
///
/// // UDP, client socket on a fixed port
/// let fixed = TransportPolicy::new(vec![
///     TransportPreference::udp().with_port(30500),
/// ]);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportPolicy(Vec<TransportPreference>);

impl TransportPolicy {
    /// Create a policy from an explicit ordered list of preferences.
    ///
    /// An empty list means no transport will ever be selected (falls back to
    /// any available endpoint with an ephemeral port).
    pub const fn new(preferences: Vec<TransportPreference>) -> Self {
        Self(preferences)
    }

    /// Prefer TCP; fall back to UDP if TCP is not available.
    ///
    /// Shorthand for `.preferred_transport(Transport::Tcp)`.
    #[must_use]
    pub fn prefer_tcp() -> Self {
        Self(vec![TransportPreference::tcp(), TransportPreference::udp()])
    }

    /// Prefer UDP; fall back to TCP if UDP is not available.
    ///
    /// Shorthand for `.preferred_transport(Transport::Udp)`.
    #[must_use]
    pub fn prefer_udp() -> Self {
        Self(vec![TransportPreference::udp(), TransportPreference::tcp()])
    }

    /// Select the best transport given the server's offered endpoints.
    ///
    /// Iterates the preference list in order and returns a [`TransportSelection`]
    /// for the first entry whose transport matches an offered endpoint.
    /// Returns `None` if no preference matches.
    #[must_use]
    pub fn select(&self, endpoints: &crate::OfferedEndpoints) -> Option<TransportSelection> {
        use crate::OfferedEndpoints;
        for pref in &self.0 {
            let remote = match (pref.transport, endpoints) {
                (
                    Transport::Tcp,
                    OfferedEndpoints::TcpOnly(addr) | OfferedEndpoints::Both { tcp: addr, .. },
                ) => Some(*addr),
                (
                    Transport::Udp,
                    OfferedEndpoints::UdpOnly(addr) | OfferedEndpoints::Both { udp: addr, .. },
                ) => Some(*addr),
                _ => None,
            };
            if let Some(remote_endpoint) = remote {
                return Some(TransportSelection {
                    remote_endpoint,
                    transport: pref.transport,
                    local_port: pref.local_port,
                });
            }
        }
        None
    }

    /// Returns the ordered list of transport preferences in this policy.
    ///
    /// Used by the subscription path to iterate through all preferences when
    /// performing cross-transport fallback (e.g. TCP → UDP).
    #[must_use]
    pub fn preferences(&self) -> &[TransportPreference] {
        &self.0
    }

    /// Returns the primary (first) transport in the policy, or `Transport::Udp` if empty.
    ///
    /// Used as the default transport for server-side offer configuration.
    #[must_use]
    pub fn primary(&self) -> Transport {
        self.0.first().map_or(Transport::Udp, |p| p.transport)
    }
}

impl Default for TransportPolicy {
    /// Default policy: prefer UDP, fall back to TCP.
    fn default() -> Self {
        Self::prefer_udp()
    }
}

impl From<Transport> for TransportPolicy {
    fn from(t: Transport) -> Self {
        match t {
            Transport::Tcp => Self::prefer_tcp(),
            Transport::Udp => Self::prefer_udp(),
        }
    }
}

// ============================================================================
// TCP KEEPALIVE CONFIGURATION
// ============================================================================

/// TCP keepalive parameters.
///
/// Controls how the OS probes idle TCP connections to detect dead peers.
/// Applied independently to outgoing (client) and incoming (server) connections
/// via [`SomeIpBuilder::tcp_keepalive_client`](crate::SomeIpBuilder::tcp_keepalive_client)
/// and [`SomeIpBuilder::tcp_keepalive_server`](crate::SomeIpBuilder::tcp_keepalive_server).
///
/// # Platform notes
///
/// `retries` maps to `TCP_KEEPCNT` and is ignored on platforms that do not
/// expose that socket option (e.g. some QNX configurations).
///
/// # Example
///
/// ```
/// use std::time::Duration;
/// use recentip::config::TcpKeepaliveConfig;
///
/// let cfg = TcpKeepaliveConfig {
///     time: Duration::from_secs(60),
///     interval: Duration::from_secs(10),
///     retries: 5,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct TcpKeepaliveConfig {
    /// Idle time before the first keepalive probe is sent (`TCP_KEEPIDLE`).
    pub time: Duration,
    /// Interval between consecutive probes when there is no response (`TCP_KEEPINTVL`)
    /// and after the last probe.
    pub interval: Duration,
    /// Number of unacknowledged probes before the connection is considered dead
    /// (`TCP_KEEPCNT`).  Not supported on all platforms.
    pub retries: u32,
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
    /// Transport policy for endpoint selection (default: prefer UDP, fall back to TCP).
    ///
    /// When a remote service offers endpoints, the first entry in this policy that
    /// matches the offered endpoints determines the effective transport.
    /// Overridable per-service via [`OfferedService::with_transport_policy`](crate::OfferedService::with_transport_policy).
    ///
    /// Set via [`SomeIpBuilder::preferred_transport`](crate::SomeIpBuilder::preferred_transport)
    /// (shorthand) or [`SomeIpBuilder::transport_policy`](crate::SomeIpBuilder::transport_policy)
    /// (full control).
    pub transport_policy: TransportPolicy,
    /// Enable Magic Cookies for TCP resynchronization (default: false)
    ///
    /// When enabled (`feat_req_someip_586`, `feat_req_someip_591`, `feat_req_someip_592)`:
    /// - Each TCP segment starts with a Magic Cookie message
    /// - Only one Magic Cookie per segment
    /// - Allows resync in testing/debugging scenarios
    pub magic_cookies: bool,
    /// TCP keepalive settings applied to outgoing (client-side) connections.
    ///
    /// `None` means OS defaults — keepalive is not explicitly enabled.
    pub tcp_keepalive_client: Option<TcpKeepaliveConfig>,
    /// TCP keepalive settings applied to incoming (server-side) connections.
    ///
    /// `None` means OS defaults — keepalive is not explicitly enabled.
    pub tcp_keepalive_server: Option<TcpKeepaliveConfig>,
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
    use crate::OfferedEndpoints;

    // ========================================================================
    // Helpers
    // ========================================================================

    fn any_ip() -> std::net::Ipv4Addr {
        std::net::Ipv4Addr::new(10, 0, 0, 1)
    }

    fn udp_ep(port: u16) -> std::net::SocketAddrV4 {
        std::net::SocketAddrV4::new(any_ip(), port)
    }

    fn tcp_ep(port: u16) -> std::net::SocketAddrV4 {
        std::net::SocketAddrV4::new(any_ip(), port + 1000)
    }

    // ========================================================================
    // TTL tests
    // ========================================================================

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

    // ========================================================================
    // TransportPolicy selection tests
    // ========================================================================

    /// Default policy prefers UDP over TCP when both endpoints are available.
    #[test]
    fn policy_default_is_prefer_udp() {
        let policy = TransportPolicy::default();
        let udp = udp_ep(30500);
        let tcp = tcp_ep(30500);

        let sel = policy
            .select(&OfferedEndpoints::Both { udp, tcp })
            .expect("default policy must select an endpoint");

        assert_eq!(sel.transport, Transport::Udp);
        assert_eq!(sel.remote_endpoint, udp);
    }

    /// `prefer_tcp()` selects the TCP endpoint from a `TcpOnly` offer.
    #[test]
    fn policy_prefer_tcp_from_tcp_only() {
        let policy = TransportPolicy::prefer_tcp();
        let tcp = tcp_ep(30501);

        let sel = policy
            .select(&OfferedEndpoints::TcpOnly(tcp))
            .expect("prefer_tcp must match TcpOnly offer");

        assert_eq!(sel.transport, Transport::Tcp);
        assert_eq!(sel.remote_endpoint, tcp);
    }

    /// `prefer_tcp()` selects TCP when the server offers both transports.
    #[test]
    fn policy_prefer_tcp_from_both() {
        let policy = TransportPolicy::prefer_tcp();
        let udp = udp_ep(30502);
        let tcp = tcp_ep(30502);

        let sel = policy
            .select(&OfferedEndpoints::Both { udp, tcp })
            .expect("prefer_tcp must match Both offer");

        assert_eq!(sel.transport, Transport::Tcp);
        assert_eq!(sel.remote_endpoint, tcp);
    }

    /// `prefer_tcp()` falls back to UDP when only UDP is offered.
    #[test]
    fn policy_prefer_tcp_fallback_to_udp() {
        let policy = TransportPolicy::prefer_tcp();
        let udp = udp_ep(30503);

        let sel = policy
            .select(&OfferedEndpoints::UdpOnly(udp))
            .expect("prefer_tcp must fall back to UDP when TCP unavailable");

        assert_eq!(sel.transport, Transport::Udp);
        assert_eq!(sel.remote_endpoint, udp);
    }

    /// `prefer_udp()` selects the UDP endpoint from a `UdpOnly` offer.
    #[test]
    fn policy_prefer_udp_from_udp_only() {
        let policy = TransportPolicy::prefer_udp();
        let udp = udp_ep(30504);

        let sel = policy
            .select(&OfferedEndpoints::UdpOnly(udp))
            .expect("prefer_udp must match UdpOnly offer");

        assert_eq!(sel.transport, Transport::Udp);
        assert_eq!(sel.remote_endpoint, udp);
    }

    /// `prefer_udp()` selects UDP when the server offers both transports.
    #[test]
    fn policy_prefer_udp_from_both() {
        let policy = TransportPolicy::prefer_udp();
        let udp = udp_ep(30505);
        let tcp = tcp_ep(30505);

        let sel = policy
            .select(&OfferedEndpoints::Both { udp, tcp })
            .expect("prefer_udp must match Both offer");

        assert_eq!(sel.transport, Transport::Udp);
        assert_eq!(sel.remote_endpoint, udp);
    }

    /// `prefer_udp()` falls back to TCP when only TCP is offered.
    #[test]
    fn policy_prefer_udp_fallback_to_tcp() {
        let policy = TransportPolicy::prefer_udp();
        let tcp = tcp_ep(30506);

        let sel = policy
            .select(&OfferedEndpoints::TcpOnly(tcp))
            .expect("prefer_udp must fall back to TCP when UDP unavailable");

        assert_eq!(sel.transport, Transport::Tcp);
        assert_eq!(sel.remote_endpoint, tcp);
    }

    /// A TCP-only policy (no fallback) returns `None` for a UDP-only server.
    #[test]
    fn policy_tcp_only_no_match_for_udp_server() {
        let policy = TransportPolicy::new(vec![TransportPreference::tcp()]);
        let udp = udp_ep(30507);

        let sel = policy.select(&OfferedEndpoints::UdpOnly(udp));

        assert!(
            sel.is_none(),
            "TCP-only policy should not match a UDP-only server"
        );
    }

    /// A UDP-only policy (no fallback) returns `None` for a TCP-only server.
    #[test]
    fn policy_udp_only_no_match_for_tcp_server() {
        let policy = TransportPolicy::new(vec![TransportPreference::udp()]);
        let tcp = tcp_ep(30508);

        let sel = policy.select(&OfferedEndpoints::TcpOnly(tcp));

        assert!(
            sel.is_none(),
            "UDP-only policy should not match a TCP-only server"
        );
    }

    /// An empty policy returns `None` for every endpoint type.
    #[test]
    fn policy_empty_returns_none() {
        let policy = TransportPolicy::new(vec![]);
        let udp = udp_ep(30509);
        let tcp = tcp_ep(30509);

        assert!(policy.select(&OfferedEndpoints::UdpOnly(udp)).is_none());
        assert!(policy.select(&OfferedEndpoints::TcpOnly(tcp)).is_none());
        assert!(
            policy
                .select(&OfferedEndpoints::Both { udp, tcp })
                .is_none()
        );
    }

    /// `with_port()` stores a `PortSpec::Fixed` in the returned `TransportSelection`.
    #[test]
    fn policy_with_fixed_port_sets_port_spec() {
        let policy = TransportPolicy::new(vec![TransportPreference::udp().with_port(30600)]);
        let udp = udp_ep(30510);

        let sel = policy
            .select(&OfferedEndpoints::UdpOnly(udp))
            .expect("policy must match");

        assert_eq!(sel.local_port, PortSpec::Fixed(30600));
        assert_eq!(sel.transport, Transport::Udp);
    }

    /*
    /// `with_port_range()` stores a `PortSpec::Range` in the returned `TransportSelection`.
    #[test]
    fn policy_with_port_range_sets_port_spec() {
        let policy =
            TransportPolicy::new(vec![TransportPreference::udp().with_port_range(30700, 30710)]);
        let udp = udp_ep(30511);

        let sel = policy
            .select(&OfferedEndpoints::UdpOnly(udp))
            .expect("policy must match");

        assert_eq!(sel.local_port, PortSpec::Range(30700, 30710));
        assert_eq!(sel.transport, Transport::Udp);
    } */

    /// `From<Transport::Tcp>` creates a prefer-TCP policy (TCP first, UDP fallback).
    #[test]
    fn policy_from_transport_tcp() {
        let policy = TransportPolicy::from(Transport::Tcp);
        let udp = udp_ep(30512);
        let tcp = tcp_ep(30512);

        // Should select TCP when both are offered
        let sel = policy
            .select(&OfferedEndpoints::Both { udp, tcp })
            .expect("must match");
        assert_eq!(sel.transport, Transport::Tcp);

        // Should fall back to UDP when only UDP is offered
        let sel = policy
            .select(&OfferedEndpoints::UdpOnly(udp))
            .expect("must fall back to UDP");
        assert_eq!(sel.transport, Transport::Udp);
    }

    /// `From<Transport::Udp>` creates a prefer-UDP policy (UDP first, TCP fallback).
    #[test]
    fn policy_from_transport_udp() {
        let policy = TransportPolicy::from(Transport::Udp);
        let udp = udp_ep(30513);
        let tcp = tcp_ep(30513);

        // Should select UDP when both are offered
        let sel = policy
            .select(&OfferedEndpoints::Both { udp, tcp })
            .expect("must match");
        assert_eq!(sel.transport, Transport::Udp);

        // Should fall back to TCP when only TCP is offered
        let sel = policy
            .select(&OfferedEndpoints::TcpOnly(tcp))
            .expect("must fall back to TCP");
        assert_eq!(sel.transport, Transport::Tcp);
    }

    /// `primary()` returns the transport of the first entry in the policy list.
    #[test]
    fn policy_primary_returns_first_transport() {
        let tcp_first = TransportPolicy::prefer_tcp();
        assert_eq!(tcp_first.primary(), Transport::Tcp);

        let udp_first = TransportPolicy::prefer_udp();
        assert_eq!(udp_first.primary(), Transport::Udp);

        let tcp_only = TransportPolicy::new(vec![TransportPreference::tcp()]);
        assert_eq!(tcp_only.primary(), Transport::Tcp);
    }

    /// `primary()` returns `Transport::Udp` for an empty policy.
    #[test]
    fn policy_primary_empty_defaults_to_udp() {
        let empty = TransportPolicy::new(vec![]);
        assert_eq!(empty.primary(), Transport::Udp);
    }

    /// The selected `remote_endpoint` always exactly matches the address advertised
    /// by the server for the chosen transport.
    #[test]
    fn selection_endpoint_matches_offered() {
        let udp = std::net::SocketAddrV4::new(std::net::Ipv4Addr::new(192, 168, 5, 10), 31000);
        let tcp = std::net::SocketAddrV4::new(std::net::Ipv4Addr::new(192, 168, 5, 10), 31001);

        let tcp_sel = TransportPolicy::prefer_tcp()
            .select(&OfferedEndpoints::Both { udp, tcp })
            .unwrap();
        assert_eq!(tcp_sel.remote_endpoint, tcp);

        let udp_sel = TransportPolicy::prefer_udp()
            .select(&OfferedEndpoints::Both { udp, tcp })
            .unwrap();
        assert_eq!(udp_sel.remote_endpoint, udp);
    }

    /// When multiple preferences are listed, the **first** one that matches wins,
    /// even if later entries would also match.
    #[test]
    fn preference_ordering_first_match_wins() {
        let policy = TransportPolicy::new(vec![
            TransportPreference::udp().with_port(30800),
            TransportPreference::tcp(),
        ]);
        let udp = udp_ep(30514);
        let tcp = tcp_ep(30514);

        let sel = policy
            .select(&OfferedEndpoints::Both { udp, tcp })
            .expect("must match");

        // First preference (UDP with port 30800) should be chosen
        assert_eq!(sel.transport, Transport::Udp);
        assert_eq!(sel.local_port, PortSpec::Fixed(30800));
        assert_eq!(sel.remote_endpoint, udp);
    }
}
