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
//!     .bind_addr(SocketAddr::V4(SocketAddrV4::new(
//!         Ipv4Addr::new(192, 168, 1, 100),
//!         30490
//!     )))
//!     .advertised_ip(Ipv4Addr::new(192, 168, 1, 100).into())
//!     .preferred_transport(Transport::Tcp)  // Prefer TCP when service offers both
//!     .ttl(1800)  // 30 minutes
//!     .cyclic_offer_delay(2000)  // 2 seconds
//!     .magic_cookies(true)  // Enable for debugging
//!     .build();
//! ```
//!
//! ## IP Address Configuration
//!
//! Understanding the different address settings is important for correct operation:
//!
//! ### `bind_addr` - Local Socket Binding
//!
//! The address to bind local sockets to. Default: `0.0.0.0:30490`.
//!
//! - **IP part**: Usually `0.0.0.0` (listen on all interfaces) or a specific interface IP
//! - **Port part**: The SD port (30490 by default)
//!
//! This controls where sockets listen for incoming packets. Using `0.0.0.0` allows
//! receiving on any interface.
//!
//! Note: Multicast loopback only seems to works when binding to `0.0.0.0`.
//!
//! ### `advertised_ip` - Endpoint Option Address
//!
//! The routable IP address for endpoint options in SD messages. Default: `None`.
//!
//! **Required for both offering and subscribing.** This IP is embedded in endpoint
//! options of `OfferService` and `SubscribeEventgroup` messages, telling remote
//! peers where to send traffic.
//!
//! - Must be a valid, routable IP address (not `0.0.0.0`)
//! - Should be reachable by remote peers
//!
//! ```no_run
//! use someip_runtime::RuntimeConfig;
//! use std::net::Ipv4Addr;
//!
//! // Real network example
//! let config = RuntimeConfig::builder()
//!     .advertised_ip(Ipv4Addr::new(192, 168, 1, 100).into())
//!     .build();
//! ```
//!
//! ### Why Both?
//!
//! - `bind_addr: 0.0.0.0` lets you listen on all interfaces
//! - `advertised_ip` tells remote peers your specific routable address
//!
//! You cannot use `0.0.0.0` in endpoint options because remote peers wouldn't know
//! where to send traffic. The SOME/IP-SD specification requires valid addresses.
//!
//! ### Fallback Behavior
//!
//! If `advertised_ip` is not set, the runtime attempts to use:
//! 1. The IP from `bind_addr` (if not `0.0.0.0`)
//! 2. The IP of the client method socket (if not `0.0.0.0`)
//!
//! If no valid IP is available, subscribe operations will fail with a configuration error.
//!
//! ## Configuration Options Reference
//!
//! | Option | Default | Description |
//! |--------|---------|-------------|
//! | `bind_addr` | `0.0.0.0:30490` | Local address to bind SD socket |
//! | `advertised_ip` | None | Routable IP for endpoint options (required for subscriptions) |
//! | `sd_multicast` | `239.255.0.1:30490` | SD multicast group address |
//! | `offer_ttl` | 3600 | TTL for OfferService entries (seconds) |
//! | `find_ttl` | 3600 | TTL for FindService entries (seconds) |
//! | `subscribe_ttl` | 3600 | TTL for SubscribeEventgroup entries (seconds) |
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
/// See: feat_req_recentipsd_431
pub const SD_TTL_INFINITE: u32 = 0xFFFFFF;

/// Default TTL for OfferService entries in seconds (1 hour).
pub const DEFAULT_OFFER_TTL: u32 = 3600;

/// Default TTL for FindService entries in seconds (1 hour).
pub const DEFAULT_FIND_TTL: u32 = 3600;

/// Default TTL for SubscribeEventgroup entries in seconds (1 hour).
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
    /// TTL for OfferService entries (default: 3600 seconds)
    pub offer_ttl: u32,
    /// TTL for FindService entries (default: 3600 seconds)
    pub find_ttl: u32,
    /// TTL for SubscribeEventgroup entries (default: 3600 seconds)
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
    /// When enabled (`feat_req_recentip_586`, `feat_req_recentip_591`, `feat_req_recentip_592)`:
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

impl RuntimeConfig {
    /// Create a new builder
    pub fn builder() -> RuntimeConfigBuilder {
        RuntimeConfigBuilder::default()
    }
}

/// Builder for `RuntimeConfig`
#[derive(Default)]
pub struct RuntimeConfigBuilder {
    config: RuntimeConfig,
}

impl RuntimeConfigBuilder {
    /// Set the local address
    pub fn bind_addr(mut self, addr: SocketAddr) -> Self {
        self.config.bind_addr = addr;
        self
    }

    /// Set the advertised IP address for endpoint options
    /// 
    /// This is the routable IP address that will be included in SD endpoint options.
    /// Must be a valid, non-unspecified address. Required for subscribing to services.
    /// 
    /// # Example
    /// ```no_run
    /// use someip_runtime::prelude::*;
    /// use std::net::Ipv4Addr;
    /// 
    /// let config = RuntimeConfig::builder()
    ///     .advertised_ip(Ipv4Addr::new(192, 168, 1, 100).into())
    ///     .build();
    /// ```
    pub fn advertised_ip(mut self, ip: std::net::IpAddr) -> Self {
        self.config.advertised_ip = Some(ip);
        self
    }

    /// Set the SD multicast address
    pub fn sd_multicast(mut self, addr: SocketAddr) -> Self {
        self.config.sd_multicast = addr;
        self
    }

    /// Set the TTL for OfferService entries (in seconds)
    pub fn offer_ttl(mut self, ttl: u32) -> Self {
        self.config.offer_ttl = ttl;
        self
    }

    /// Set the TTL for FindService entries (in seconds)
    pub fn find_ttl(mut self, ttl: u32) -> Self {
        self.config.find_ttl = ttl;
        self
    }

    /// Set the TTL for SubscribeEventgroup entries (in seconds)
    pub fn subscribe_ttl(mut self, ttl: u32) -> Self {
        self.config.subscribe_ttl = ttl;
        self
    }

    /// Set all TTLs (offer, find, subscribe) to the same value (convenience method)
    pub fn ttl(self, ttl: u32) -> Self {
        self.offer_ttl(ttl).find_ttl(ttl).subscribe_ttl(ttl)
    }

    /// Set the cyclic offer delay
    pub fn cyclic_offer_delay(mut self, delay_ms: u64) -> Self {
        self.config.cyclic_offer_delay = delay_ms;
        self
    }

    /// Set the preferred transport when a service advertises both TCP and UDP.
    ///
    /// This is only used for client-side endpoint selection when discovering
    /// services that offer both transports. For explicit transport selection,
    /// use `bind()` or `offer()` with the specific transport.
    pub fn preferred_transport(mut self, transport: Transport) -> Self {
        self.config.preferred_transport = transport;
        self
    }

    /// Deprecated: Use `preferred_transport` instead.
    #[deprecated(since = "0.2.0", note = "Use `preferred_transport()` instead")]
    pub fn transport(self, transport: Transport) -> Self {
        self.preferred_transport(transport)
    }

    /// Enable or disable Magic Cookies for TCP (default: false)
    ///
    /// Magic Cookies allow resynchronization in testing/debugging scenarios.
    /// See `feat_req_recentip_586`, `feat_req_recentip_591`, `feat_req_recentip_592`.
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
/// Per SOME/IP specification (`feat_req_recentip_106`, `feat_req_recentip_726)`:
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
/// use someip_runtime::config::OfferConfig;
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
    pub fn tcp(mut self) -> Self {
        self.tcp_port = Some(0); // 0 means use default
        self
    }

    /// Enable TCP transport with a specific port.
    pub fn tcp_port(mut self, port: u16) -> Self {
        self.tcp_port = Some(port);
        self
    }

    /// Enable UDP transport with the default RPC port (30491).
    pub fn udp(mut self) -> Self {
        self.udp_port = Some(0); // 0 means use default
        self
    }

    /// Enable UDP transport with a specific port.
    pub fn udp_port(mut self, port: u16) -> Self {
        self.udp_port = Some(port);
        self
    }

    /// Configure method-specific behavior (e.g., exception handling).
    pub fn method_config(mut self, config: MethodConfig) -> Self {
        self.method_config = config;
        self
    }

    /// Check if any transport is configured.
    pub fn has_transport(&self) -> bool {
        self.tcp_port.is_some() || self.udp_port.is_some()
    }

    /// Check if TCP is enabled.
    pub fn has_tcp(&self) -> bool {
        self.tcp_port.is_some()
    }

    /// Check if UDP is enabled.
    pub fn has_udp(&self) -> bool {
        self.udp_port.is_some()
    }
}
