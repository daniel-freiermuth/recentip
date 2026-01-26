//! Builder for configuring and starting SOME/IP runtimes.
//!
//! ## Quick Start
//!
//! For most applications, the defaults work out of the box:
//!
//! ```no_run
//! # async fn example() -> recentip::Result<()> {
//! let runtime = recentip::configure().start().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Builder Pattern
//!
//! For custom configurations, use the builder:
//!
//! ```no_run
//! use recentip::prelude::*;
//! use std::net::{SocketAddr, SocketAddrV4, Ipv4Addr};
//!
//! # async fn example() -> recentip::Result<()> {
//! let someip = recentip::configure()
//!     .bind_addr(SocketAddr::V4(SocketAddrV4::new(
//!         Ipv4Addr::UNSPECIFIED,
//!         30490
//!     )))
//!     .advertised_ip(Ipv4Addr::new(192, 168, 1, 100).into())
//!     .preferred_transport(Transport::Tcp)  // Prefer TCP when service offers both
//!     .offer_ttl(1800)  // 30 minutes
//!     .cyclic_offer_delay(2000)  // 2 seconds
//!     .magic_cookies(true)  // Enable for debugging
//!     .start().await?;
//! # Ok(())
//! # }
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
//! Note: Multicast loopback only seems to work when binding to `0.0.0.0`.
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
//! If no valid IP is available, subscribe operations will fail with a configuration error

use crate::config::{clamp_ttl_to_24bit, RuntimeConfig, Transport};
use crate::error::Result;
use crate::handles::SomeIp;
use crate::net::{TcpListener, TcpStream, UdpSocket};
use std::net::{IpAddr, SocketAddr};

/// Builder for configuring and starting a SOME/IP runtime.
///
/// Created via [`configure()`](crate::configure).
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
///         .advertised_ip(Ipv4Addr::new(192, 168, 1, 100).into())
///         .preferred_transport(Transport::Tcp)
///         .start().await?;
///     Ok(())
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct SomeIpBuilder {
    config: RuntimeConfig,
}

impl SomeIpBuilder {
    /// Create a new builder with default configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// The Service Discovery (SD) bind address and port
    ///
    /// This will be used for SD multicast traffic. Thus the port needs to
    /// match the multicast port to work correctly.
    ///
    /// If no other address for unicast is specified with `sd_unicast`(not yet
    /// implemented), this address will also be used for unicast SD traffic.
    ///
    /// We recommend binding to `0.0.0.0` (`Ipv4Addr::UNSPECIFIED`) as this is
    /// the portable stable solution to receive multicast (in particular
    /// loopback multicast) on POSIX system. If you have a modern Linux system
    /// it might as well work to bind to the specific interface address.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    /// use std::net::{SocketAddr, Ipv4Addr, SocketAddrV4};
    ///
    /// # async fn example() -> Result<()> {
    /// let someip = recentip::configure()
    ///     .bind_addr(SocketAddr::V4(SocketAddrV4::new(
    ///         Ipv4Addr::UNSPECIFIED,
    ///         30490
    ///     )))
    ///     .start().await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Default: `0.0.0.0:30490`
    pub const fn bind_addr(mut self, addr: SocketAddr) -> Self {
        self.config.bind_addr = addr;
        self
    }

    /// Set the advertised IP address for endpoint options
    ///
    /// When binding the SD socket to `0.0.0.0`, recentIP doesn't know how
    /// other servers can reach it. Thus we need to explicitly set the
    /// advertised IP address. Must be a valid, non-unspecified address.
    ///
    /// Must be set if binding to `0.0.0.0` and no `sd_unicast` option
    /// supplied.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    /// use std::net::Ipv4Addr;
    ///
    /// # async fn example() -> Result<()> {
    /// let someip = recentip::configure()
    ///     .advertised_ip(Ipv4Addr::new(192, 168, 1, 100).into())
    ///     .start().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub const fn advertised_ip(mut self, ip: IpAddr) -> Self {
        self.config.advertised_ip = Some(ip);
        self
    }

    /// The multicast address and port to subscribe to for Service Discovery
    ///
    /// The port should match the port part of `bind_addr` to work correctly.
    ///
    /// Default: `239.255.0.1:30490`
    pub const fn sd_multicast(mut self, addr: SocketAddr) -> Self {
        self.config.sd_multicast = addr;
        self
    }

    /// Set the TTL for `OfferService` entries (in seconds)
    ///
    /// Values exceeding the 24-bit maximum (0xFFFFFF = 16,777,215) will be
    /// clamped to `SD_TTL_INFINITE` to prevent silent truncation during
    /// serialization.
    ///
    /// Default: 3600 seconds
    pub fn offer_ttl(mut self, ttl: u32) -> Self {
        self.config.offer_ttl = clamp_ttl_to_24bit(ttl, "offer_ttl");
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
        self.config.find_ttl = clamp_ttl_to_24bit(ttl, "find_ttl");
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
        self.config.subscribe_ttl = clamp_ttl_to_24bit(ttl, "subscribe_ttl");
        self
    }

    /// Set interval between cyclic offers (in milliseconds). This must be
    /// strictly less than `offer_ttl`.
    ///
    /// Default: 1000 ms
    pub const fn cyclic_offer_delay(mut self, delay_ms: u64) -> Self {
        self.config.cyclic_offer_delay = delay_ms;
        self
    }

    /// Set the preferred transport when a service advertises both TCP and UDP.
    ///
    /// This is only used for client-side endpoint selection when discovering
    /// services that offer both transports.
    ///
    /// Default: `Transport::Udp`
    pub const fn preferred_transport(mut self, transport: Transport) -> Self {
        self.config.preferred_transport = transport;
        self
    }

    /// Enable or disable Magic Cookies for TCP (default: false)
    ///
    /// Magic Cookies allow resynchronization in testing/debugging scenarios.
    pub const fn magic_cookies(mut self, enabled: bool) -> Self {
        self.config.magic_cookies = enabled;
        self
    }

    /// Start the SOME/IP runtime with tokio sockets (production use).
    ///
    /// This is the default method for production applications using real networking.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    /// use std::net::Ipv4Addr;
    ///
    /// # async fn example() -> recentip::Result<()> {
    /// let someip = recentip::configure()
    ///     .advertised_ip(Ipv4Addr::new(192, 168, 1, 100).into())
    ///     .start().await?;
    /// # Ok(())
    /// # }
    /// ```
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
    ///     .advertised_ip("192.168.1.100".parse().unwrap())
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
        SomeIp::new(self.config).await
    }
}

// Feature-gated turmoil helper
#[cfg(feature = "turmoil")]
impl SomeIpBuilder {
    /// Start with turmoil sockets (for network simulation testing).
    ///
    /// This is a convenience method for testing with the turmoil network simulator.
    /// Requires the `turmoil` feature flag.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    ///
    /// # async fn example() -> Result<()> {
    /// let someip = recentip::configure()
    ///     .advertised_ip(turmoil::lookup("server"))
    ///     .start_turmoil().await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if socket binding fails or configuration is invalid.
    pub async fn start_turmoil(
        self,
    ) -> Result<SomeIp<turmoil::net::UdpSocket, turmoil::net::TcpStream, turmoil::net::TcpListener>>
    {
        self.start_generic().await
    }
}
