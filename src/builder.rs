//! Builder for configuring and starting SOME/IP runtimes

use crate::config::{RuntimeConfig, Transport};
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
#[derive(Debug, Clone)]
pub struct SomeIpBuilder {
    config: RuntimeConfig,
}

impl Default for SomeIpBuilder {
    fn default() -> Self {
        Self {
            config: RuntimeConfig::default(),
        }
    }
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
    pub fn bind_addr(mut self, addr: SocketAddr) -> Self {
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
    pub fn advertised_ip(mut self, ip: IpAddr) -> Self {
        self.config.advertised_ip = Some(ip);
        self
    }

    /// The multicast address and port to subscribe to for Service Discovery
    ///
    /// The port should match the port part of `bind_addr` to work correctly.
    ///
    /// Default: `239.255.0.1:30490`
    pub fn sd_multicast(mut self, addr: SocketAddr) -> Self {
        self.config.sd_multicast = addr;
        self
    }

    /// Set the TTL for `OfferService` entries (in seconds)
    ///
    /// Default: 3600 seconds
    pub fn offer_ttl(mut self, ttl: u32) -> Self {
        self.config.offer_ttl = ttl;
        self
    }

    /// Set the TTL for `FindService` entries (in seconds)
    ///
    /// So far, this option is pretty irrelevant as clients don't, but the
    /// behavior might change in the future.
    ///
    /// Default: 3600 seconds
    pub fn find_ttl(mut self, ttl: u32) -> Self {
        self.config.find_ttl = ttl;
        self
    }

    /// Set the minimal TTL for `SubscribeEventgroup` entries (in seconds)
    ///
    /// See [config module documentation](crate::config) for details on subscription TTL semantics.
    ///
    /// Default: 3600 seconds
    pub fn subscribe_ttl(mut self, ttl: u32) -> Self {
        self.config.subscribe_ttl = ttl;
        self
    }

    /// Set interval between cyclic offers (in milliseconds). This must be
    /// strictly less than `offer_ttl`.
    ///
    /// Default: 1000 ms
    pub fn cyclic_offer_delay(mut self, delay_ms: u64) -> Self {
        self.config.cyclic_offer_delay = delay_ms;
        self
    }

    /// Set the preferred transport when a service advertises both TCP and UDP.
    ///
    /// This is only used for client-side endpoint selection when discovering
    /// services that offer both transports.
    ///
    /// Default: `Transport::Udp`
    pub fn preferred_transport(mut self, transport: Transport) -> Self {
        self.config.preferred_transport = transport;
        self
    }

    /// Enable or disable Magic Cookies for TCP (default: false)
    ///
    /// Magic Cookies allow resynchronization in testing/debugging scenarios.
    pub fn magic_cookies(mut self, enabled: bool) -> Self {
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
    pub async fn start(self) -> Result<SomeIp> {
        SomeIp::new(self.config).await
    }

    /// Start with custom socket types (for testing with network simulators).
    ///
    /// This is primarily used for testing with turmoil or other network simulators.
    /// Most users should use [`start()`](Self::start) instead.
    pub async fn start_with_sockets<U, T, L>(self) -> Result<SomeIp<U, T, L>>
    where
        U: UdpSocket,
        T: TcpStream,
        L: TcpListener<Stream = T>,
    {
        SomeIp::with_socket_type(self.config).await
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
    pub async fn start_turmoil(
        self,
    ) -> Result<SomeIp<turmoil::net::UdpSocket, turmoil::net::TcpStream, turmoil::net::TcpListener>>
    {
        self.start_with_sockets().await
    }
}
