//! Builder for configuring and starting SOME/IP runtimes.
//!
//! ## Quick Start
//!
//! Two SD parameters are mandatory; the builder enforces this at compile time:
//!
//! ```no_run
//! use recentip::prelude::*;
//!
//! # async fn example() -> recentip::Result<()> {
//! // Dual-socket mode (default): mc_socket on multicast group, uc_socket on sd_unicast IP
//! let runtime = recentip::configure()
//!     .sd_multicast_group("239.255.255.250".parse().unwrap())
//!     .sd_unicast("192.168.1.10".parse().unwrap())
//!     .start().await?;
//!
//! // Override the default SD port (30490):
//! let runtime = recentip::configure()
//!     .sd_multicast_group("239.255.255.250".parse().unwrap())
//!     .sd_unicast("192.168.1.10".parse().unwrap())
//!     .sd_port(30491)
//!     .start().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Mandatory vs Optional Parameters
//!
//! | Parameter | Method | Mandatory? |
//! |-----------|--------|------------|
//! | SD multicast group | [`sd_multicast_group`](SomeIpBuilder::sd_multicast_group) | **Yes** |
//! | SD unicast IP | [`sd_unicast`](SomeIpBuilder::sd_unicast) | **Yes** |
//! | SD port | [`sd_port`](SomeIpBuilder::sd_port) | No — default 30490 |
//! | Offer TTL | [`offer_ttl`](SomeIpBuilder::offer_ttl) | No — default 3600 s |
//! | Find TTL | [`find_ttl`](SomeIpBuilder::find_ttl) | No — default 3600 s |
//! | Subscribe TTL | [`subscribe_ttl`](SomeIpBuilder::subscribe_ttl) | No — default 3600 s |
//! | Cyclic offer delay | [`cyclic_offer_delay`](SomeIpBuilder::cyclic_offer_delay) | No — default 1000 ms |
//! | Preferred transport | [`preferred_transport`](SomeIpBuilder::preferred_transport) | No — default UDP |
//! | Magic cookies | [`magic_cookies`](SomeIpBuilder::magic_cookies) | No — default false |
//! | Single-socket mode | [`single_socket`](SomeIpBuilder::single_socket) | No — default dual-socket |
//!
//! ## Socket Modes
//!
//! **Dual-socket** (default): the runtime binds `mc_socket` to `<sd_multicast_group>:<sd_port>`
//! and `uc_socket` to `<sd_unicast>:<sd_port>`.  The `sd_unicast` IP is embedded in SD
//! endpoint options.  This mode is isolated from other applications on the same port.
//!
//! **Single-socket** (opt-in via [`single_socket()`](SomeIpBuilder::single_socket)):
//! one socket bound to `0.0.0.0:<sd_port>`.  Use on platforms that do not support
//! multicast-address binds.  `sd_unicast` is still
//! required for SD endpoint option advertisement.

use crate::config::{
    clamp_ttl_to_24bit, MulticastAddress, RuntimeConfig, TcpKeepaliveConfig, Transport,
    TransportPolicy, UnicastAddress, DEFAULT_CYCLIC_OFFER_DELAY, DEFAULT_FIND_TTL,
    DEFAULT_OFFER_TTL, DEFAULT_SD_PORT, DEFAULT_SUBSCRIBE_TTL,
};
use crate::error::Result;
use crate::handles::SomeIp;
use crate::net::{TcpListener, TcpStream, UdpSocket};

// ============================================================================
// Builder struct
// ============================================================================

/// Builder for configuring and starting a SOME/IP runtime.
///
/// Created via [`configure()`](crate::configure).
///
/// The two type parameters track which mandatory fields have been set.
/// [`start`](SomeIpBuilder::start) / [`start_turmoil`](SomeIpBuilder::start_turmoil)
/// are only available once both are configured, enforced at compile time.
///
/// # Example
///
/// ```no_run
/// use recentip::prelude::*;
/// use std::net::Ipv4Addr;
///
/// #[tokio::main]
/// async fn main() -> recentip::Result<()> {
///     let someip = recentip::configure()
///         .sd_multicast_group("239.255.255.250".parse().unwrap())
///         .sd_unicast("192.168.1.100".parse().unwrap())
///         .preferred_transport(Transport::Tcp)
///         .start().await?;
///     Ok(())
/// }
/// ```
pub struct SomeIpBuilder<Addr = (), MC = ()> {
    sd_unicast: Addr,
    sd_multicast: MC,
    sd_port: u16,
    single_socket: bool,
    offer_ttl: u32,
    find_ttl: u32,
    subscribe_ttl: u32,
    cyclic_offer_delay: u64,
    transport_policy: TransportPolicy,
    magic_cookies: bool,
    tcp_keepalive_client: Option<TcpKeepaliveConfig>,
    tcp_keepalive_server: Option<TcpKeepaliveConfig>,
}

impl SomeIpBuilder<(), ()> {
    /// Create a new builder with no mandatory fields set.
    pub fn new() -> Self {
        Self {
            sd_unicast: (),
            sd_multicast: (),
            sd_port: DEFAULT_SD_PORT,
            single_socket: false,
            offer_ttl: DEFAULT_OFFER_TTL,
            find_ttl: DEFAULT_FIND_TTL,
            subscribe_ttl: DEFAULT_SUBSCRIBE_TTL,
            cyclic_offer_delay: DEFAULT_CYCLIC_OFFER_DELAY,
            transport_policy: TransportPolicy::default(),
            magic_cookies: false,
            tcp_keepalive_client: None,
            tcp_keepalive_server: None,
        }
    }
}

impl Default for SomeIpBuilder<(), ()> {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Mandatory field transitions
// ============================================================================

impl<MC> SomeIpBuilder<(), MC> {
    /// Set the routable unicast IP advertised in SD endpoint options.
    ///
    /// The address is validated as a non-unspecified, non-multicast, non-broadcast
    /// IPv4 address.  For address construction, you can use:
    ///
    /// - `"192.168.1.100".parse::<UnicastAddress>().unwrap()`
    /// - `UnicastAddress::try_from(some_ipv4_addr)?`
    ///
    /// `ip` must be reachable by remote peers.
    ///
    /// **Choose this when** you have a specific routable IP address (real network
    /// or separate loopback aliases like `127.0.0.2`).  This mode provides
    /// host-level non-interference: the multicast socket does not share the
    /// `SO_REUSEPORT` pool with wildcard-bound applications.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    ///
    /// # async fn example() -> recentip::Result<()> {
    /// let someip = recentip::configure()
    ///     .sd_multicast_group("239.255.255.250".parse().unwrap())
    ///     .sd_unicast("192.168.1.100".parse().unwrap())
    ///     .start().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn sd_unicast(self, addr: UnicastAddress) -> SomeIpBuilder<UnicastAddress, MC> {
        SomeIpBuilder {
            sd_unicast: addr,
            sd_multicast: self.sd_multicast,
            sd_port: self.sd_port,
            single_socket: self.single_socket,
            offer_ttl: self.offer_ttl,
            find_ttl: self.find_ttl,
            subscribe_ttl: self.subscribe_ttl,
            cyclic_offer_delay: self.cyclic_offer_delay,
            transport_policy: self.transport_policy,
            magic_cookies: self.magic_cookies,
            tcp_keepalive_client: self.tcp_keepalive_client,
            tcp_keepalive_server: self.tcp_keepalive_server,
        }
    }
}

impl<A> SomeIpBuilder<A, ()> {
    /// Set the SD multicast group address.
    ///
    /// SOME/IP-SD traffic is sent to and received from this multicast address.
    /// The address must be in the IPv4 multicast range `224.0.0.0/4`.  For
    /// address construction, you can use:
    ///
    /// - `"239.255.255.250".parse::<MulticastAddress>().unwrap()`
    /// - `MulticastAddress::try_from(some_ipv4_addr)?`
    ///
    /// There is no spec-mandated default.  Common values:
    /// - `239.255.255.250` — automotive/vehicle deployments (common convention)
    /// - `239.255.0.1` — single-host / test environments
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    ///
    /// # async fn example() -> recentip::Result<()> {
    /// let someip = recentip::configure()
    ///     .sd_multicast_group("239.255.255.250".parse().unwrap())
    ///     .sd_unicast("192.168.1.100".parse().unwrap())
    ///     .start().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn sd_multicast_group(self, addr: MulticastAddress) -> SomeIpBuilder<A, MulticastAddress> {
        SomeIpBuilder {
            sd_unicast: self.sd_unicast,
            sd_multicast: addr,
            sd_port: self.sd_port,
            single_socket: self.single_socket,
            offer_ttl: self.offer_ttl,
            find_ttl: self.find_ttl,
            subscribe_ttl: self.subscribe_ttl,
            cyclic_offer_delay: self.cyclic_offer_delay,
            transport_policy: self.transport_policy,
            magic_cookies: self.magic_cookies,
            tcp_keepalive_client: self.tcp_keepalive_client,
            tcp_keepalive_server: self.tcp_keepalive_server,
        }
    }
}

// ============================================================================
// Optional configuration (available in any state)
// ============================================================================

impl<A, MC> SomeIpBuilder<A, MC> {
    /// Set the Service Discovery UDP port.
    ///
    /// Per the SOME/IP specification, `30490` (`DEFAULT_SD_PORT`) is the
    /// conventional SD port.  Override only when your system configuration
    /// mandates a different value.
    ///
    /// Default: `30490`
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    ///
    /// # async fn example() -> recentip::Result<()> {
    /// let someip = recentip::configure()
    ///     .sd_multicast_group("239.255.255.250".parse().unwrap())
    ///     .sd_unicast("192.168.1.100".parse().unwrap())
    ///     .sd_port(30490)
    ///     .start().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub const fn sd_port(mut self, port: u16) -> Self {
        self.sd_port = port;
        self
    }
    /// Set the TTL for `OfferService` entries (in seconds).
    ///
    /// Values exceeding the 24-bit maximum (0xFFFFFF = 16,777,215) will be
    /// clamped to `SD_TTL_INFINITE` to prevent silent truncation during
    /// serialization.
    ///
    /// Default: 3600 seconds
    pub fn offer_ttl(mut self, ttl: u32) -> Self {
        self.offer_ttl = clamp_ttl_to_24bit(ttl, "offer_ttl");
        self
    }

    /// Set the TTL for `FindService` entries (in seconds)
    ///
    /// So far, this option is pretty irrelevant as clients don't, but the
    /// behavior might change in the future.
    ///
    /// Values exceeding the 24-bit maximum (0xFFFFFF = 16,777,215) will be
    /// clamped to `SD_TTL_INFINITE` to prevent silent truncation during
    /// serialization.
    ///
    /// Default: 3600 seconds
    pub fn find_ttl(mut self, ttl: u32) -> Self {
        self.find_ttl = clamp_ttl_to_24bit(ttl, "find_ttl");
        self
    }

    /// Set the minimal TTL for `SubscribeEventgroup` entries (in seconds)
    ///
    /// This is a peculiar option. Intuitively, it sounds reasonable to set
    /// this to the desired subscription duration. However, it turns out that
    /// this duration is hardly known in reality. Typical values range thus
    /// rather low (slightly higher than the other side's cyclic offer delay)
    /// to very high (carrying the notion of a "long-lived" subscription).
    ///
    /// The key insight here is that the value does not really matter as
    /// subscriptions need to be renewed upon every incomming offer anyway.
    /// Thus it is important for uninterrupted subscriptions that this value is
    /// set striclty higher than the remote side's cyclic offer delay. A value
    /// that is unfortunately unknown to the client. `RecentIP` thus subscribes
    /// with `TTL=max(configured_min_sub_ttl`, `remote_offer_ttl`) and hopes that
    /// the other implementations do the same.
    ///
    /// So the subscription TTL rather carries beliefs about the
    /// • expected network latency fluctuations
    /// • expected offer -> resubscribe latency fluctuations
    /// • expected packet loss (which again depends on the server's cyclic
    ///   offer delay)
    /// Then the subscription TTL offer a means to auto-cleanup stale
    /// subscriptions.
    ///
    /// **Special Value**: Setting this to `SD_TTL_INFINITE` (0xFFFFFF) means
    /// that the subscription should never expire. The protocol then also skips
    /// offer renewals and relies on unsubscribe message to clean up
    /// subscriptions.
    ///
    /// **In the end**, this setting is a question of detecting stale
    /// subscriptions and thus carries beliefs about the network and SOME/IP
    /// participant reliability. For events delivered via TCP, there is no
    /// point in setting a finite subscription TTL.
    ///
    /// Default: 3600 seconds
    pub fn subscribe_ttl(mut self, ttl: u32) -> Self {
        self.subscribe_ttl = clamp_ttl_to_24bit(ttl, "subscribe_ttl");
        self
    }

    /// Set interval between cyclic offers (in milliseconds).
    ///
    /// This must be strictly less than `offer_ttl`.
    ///
    /// Default: 1000 ms
    pub const fn cyclic_offer_delay(mut self, delay_ms: u64) -> Self {
        self.cyclic_offer_delay = delay_ms;
        self
    }

    /// Set the preferred transport when a service advertises both TCP and UDP.
    ///
    /// This is a shorthand for [`transport_policy`](Self::transport_policy):
    /// - `Transport::Tcp` → policy `[tcp(), udp()]` (prefer TCP, fall back to UDP)
    /// - `Transport::Udp` → policy `[udp(), tcp()]` (prefer UDP, fall back to TCP)
    ///
    /// For finer control (e.g. TCP-only, no fallback), use
    /// [`transport_policy`](Self::transport_policy) directly.
    ///
    /// Default: `Transport::Udp`
    pub fn preferred_transport(mut self, transport: Transport) -> Self {
        self.transport_policy = TransportPolicy::from(transport);
        self
    }

    /// Set the global transport policy for endpoint selection.
    ///
    /// The policy is an ordered list of [`crate::config::TransportPreference`] entries.
    /// The first entry that matches a server's offered endpoints determines
    /// the effective transport.  [`crate::handles::OfferedService`] copies this policy at
    /// creation time and may override it via
    /// [`with_transport_policy`](crate::handles::OfferedService::with_transport_policy).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    /// use recentip::config::{TransportPolicy, TransportPreference};
    ///
    /// # async fn example() -> recentip::Result<()> {
    /// // TCP only — fail if service does not offer TCP
    /// let someip = recentip::configure()
    ///     .sd_multicast_group("239.255.255.250".parse().unwrap())
    ///     .sd_unicast("192.168.1.100".parse().unwrap())
    ///     .transport_policy(TransportPolicy::new(vec![TransportPreference::tcp()]))
    ///     .start().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn transport_policy(mut self, policy: TransportPolicy) -> Self {
        self.transport_policy = policy;
        self
    }

    /// Enable or disable Magic Cookies for TCP (default: false)
    ///
    /// Magic Cookies allow resynchronization in testing/debugging scenarios.
    pub const fn magic_cookies(mut self, enabled: bool) -> Self {
        self.magic_cookies = enabled;
        self
    }

    /// Set TCP keepalive parameters for outgoing (client-side) connections.
    ///
    /// When set, the OS will send keepalive probes on idle client TCP connections.
    /// This detects dead peers and triggers reconnection via SOME/IP error handling.
    ///
    /// Default: `None` (OS default — keepalive not explicitly enabled).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    /// use recentip::config::TcpKeepaliveConfig;
    /// use std::time::Duration;
    ///
    /// # async fn example() -> recentip::Result<()> {
    /// let someip = recentip::configure()
    ///     .sd_multicast_group("239.255.255.250".parse().unwrap())
    ///     .sd_unicast("192.168.1.100".parse().unwrap())
    ///     .tcp_keepalive_client(Some(TcpKeepaliveConfig {
    ///         time: Duration::from_secs(60),
    ///         interval: Duration::from_secs(10),
    ///         retries: 5,
    ///     }))
    ///     .start().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub const fn tcp_keepalive_client(mut self, config: Option<TcpKeepaliveConfig>) -> Self {
        self.tcp_keepalive_client = config;
        self
    }

    /// Set TCP keepalive parameters for incoming (server-side) connections.
    ///
    /// When set, the OS will send keepalive probes on idle server TCP connections.
    /// This allows the server to detect and clean up dead clients.
    ///
    /// Default: `None` (OS default — keepalive not explicitly enabled).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    /// use recentip::config::TcpKeepaliveConfig;
    /// use std::time::Duration;
    ///
    /// # async fn example() -> recentip::Result<()> {
    /// let someip = recentip::configure()
    ///     .sd_multicast_group("239.255.255.250".parse().unwrap())
    ///     .sd_unicast("192.168.1.100".parse().unwrap())
    ///     .tcp_keepalive_server(Some(TcpKeepaliveConfig {
    ///         time: Duration::from_secs(60),
    ///         interval: Duration::from_secs(10),
    ///         retries: 5,
    ///     }))
    ///     .start().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub const fn tcp_keepalive_server(mut self, config: Option<TcpKeepaliveConfig>) -> Self {
        self.tcp_keepalive_server = config;
        self
    }

    /// Enable single-socket mode: bind one socket to `0.0.0.0:<sd_port>` instead
    /// of the dual-socket layout.
    ///
    /// In single-socket mode `sd_unicast` is not used for socket binding, but it
    /// is still required and still advertised in SD endpoint options so remote
    /// peers know how to reach this runtime.
    ///
    /// Use this on platforms that do not support binding a UDP socket to a
    /// multicast group address (e.g. some QNX configurations).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    ///
    /// # async fn example() -> recentip::Result<()> {
    /// let someip = recentip::configure()
    ///     .sd_port(30490)
    ///     .sd_multicast_group("239.255.255.250".parse().unwrap())
    ///     .sd_unicast("192.168.1.100".parse().unwrap())
    ///     .single_socket()
    ///     .start().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub const fn single_socket(mut self) -> Self {
        self.single_socket = true;
        self
    }
}

// ============================================================================
// start() — only available when all mandatory fields are set
// ============================================================================

impl SomeIpBuilder<UnicastAddress, MulticastAddress> {
    /// Start the SOME/IP runtime with tokio sockets (production use).
    ///
    /// # Errors
    ///
    /// Returns an error if socket binding fails or configuration is invalid.
    pub async fn start(
        self,
    ) -> Result<SomeIp<tokio::net::UdpSocket, tokio::net::TcpStream, tokio::net::TcpListener>> {
        self.start_generic().await
    }

    /// Start with custom socket types (for custom network implementations).
    ///
    /// Use this method when you need custom socket implementations.
    /// For most use cases, prefer [`start`](Self::start) (tokio) or
    /// [`start_turmoil`](Self::start_turmoil) (testing).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    ///
    /// # async fn example() -> Result<()> {
    /// let someip = recentip::configure()
    ///     .sd_multicast_group("239.255.255.250".parse().unwrap())
    ///     .sd_unicast("192.168.1.100".parse().unwrap())
    ///     .start_generic::<tokio::net::UdpSocket, tokio::net::TcpStream, tokio::net::TcpListener>()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if socket binding fails or configuration is invalid.
    pub async fn start_generic<U, T, L>(self) -> Result<SomeIp<U, T, L>>
    where
        U: UdpSocket,
        T: TcpStream,
        L: TcpListener<Stream = T>,
    {
        let config = RuntimeConfig {
            sd_multicast: self.sd_multicast,
            sd_unicast: self.sd_unicast,
            sd_port: self.sd_port,
            single_socket: self.single_socket,
            offer_ttl: self.offer_ttl,
            find_ttl: self.find_ttl,
            subscribe_ttl: self.subscribe_ttl,
            cyclic_offer_delay: self.cyclic_offer_delay,
            transport_policy: self.transport_policy,
            magic_cookies: self.magic_cookies,
            tcp_keepalive_client: self.tcp_keepalive_client,
            tcp_keepalive_server: self.tcp_keepalive_server,
        };
        SomeIp::new(config).await
    }
}

// Feature-gated turmoil helper
#[cfg(feature = "turmoil")]
impl SomeIpBuilder<UnicastAddress, MulticastAddress> {
    /// Start with turmoil sockets (for network simulation testing).
    ///
    /// Requires the `turmoil` feature flag.
    ///
    /// Single-socket mode is forced automatically: turmoil does not support
    /// binding to multicast addresses, so the dual-socket path is not available.
    ///
    /// # Errors
    ///
    /// Returns an error if socket binding fails or configuration is invalid.
    pub async fn start_turmoil(
        mut self,
    ) -> Result<SomeIp<turmoil::net::UdpSocket, turmoil::net::TcpStream, turmoil::net::TcpListener>>
    {
        self.single_socket = true;
        self.start_generic().await
    }
}
