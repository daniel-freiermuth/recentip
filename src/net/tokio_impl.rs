//! Tokio socket implementations.

use super::{TcpListener, TcpStream, UdpSocket};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing;

/// Extract an IPv4 address from a [`SocketAddr`], logging an error and falling
/// back to `0.0.0.0:0` when an IPv6 address is encountered unexpectedly.
///
/// All tokio socket wrappers in this module bind to IPv4 only, so receiving an
/// IPv6 address indicates a bug.
fn expect_v4(addr: SocketAddr, context: &str) -> SocketAddrV4 {
    match addr {
        SocketAddr::V4(v4) => v4,
        SocketAddr::V6(v6) => {
            tracing::error!(
                "BUG: {context} produced IPv6 address: {v6}. \
                 Returning fallback IPv4 address."
            );
            SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)
        }
    }
}

impl UdpSocket for tokio::net::UdpSocket {
    #[allow(clippy::unused_async_trait_impl)] // trait requires Future; this impl is sync
    async fn bind(addr: SocketAddrV4) -> io::Result<Self> {
        // Use socket2 to set SO_REUSEPORT before binding.
        // This allows multiple processes/runtimes to share the same port,
        // which is required for SOME/IP SD multicast to work properly.
        use socket2::{Domain, Protocol, Socket, Type};

        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;

        // Set SO_REUSEADDR (allows reuse of local addresses)
        socket.set_reuse_address(true)?;

        // Set non-blocking before converting to tokio socket
        socket.set_nonblocking(true)?;

        // Bind to the address
        socket.bind(&addr.into())?;

        // Convert to tokio socket
        let std_socket: std::net::UdpSocket = socket.into();
        Self::from_std(std_socket)
    }

    async fn send_to(&self, buf: &[u8], target: SocketAddrV4) -> io::Result<usize> {
        Self::send_to(self, buf, target).await
    }

    async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddrV4)> {
        Self::recv_from(self, buf)
            .await
            .map(|(size, addr)| (size, expect_v4(addr, "tokio::net::UdpSocket::recv_from")))
    }

    fn join_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()> {
        // Enable multicast loopback so packets sent from this host are delivered back
        // to other sockets on the same host. This is required for testing on a single machine.
        Self::set_multicast_loop_v4(self, true)?;

        Self::join_multicast_v4(self, multiaddr, interface)
    }

    fn leave_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()> {
        Self::leave_multicast_v4(self, multiaddr, interface)
    }

    fn local_addr(&self) -> io::Result<SocketAddrV4> {
        Self::local_addr(self).map(|addr| expect_v4(addr, "tokio::net::UdpSocket::local_addr"))
    }

    fn set_multicast_if_v4(&self, addr: Ipv4Addr) -> io::Result<()> {
        // Use socket2::SockRef to borrow the underlying socket (safe, no ownership taken)
        // and set IP_MULTICAST_IF.  This controls the source IP on outgoing multicast
        // datagrams, which is essential when the socket is bound to 0.0.0.0 but we want
        // the multicast Offer to appear to come from a specific IP.
        use socket2::SockRef;
        SockRef::from(self).set_multicast_if_v4(&addr)
    }
}

impl TcpStream for tokio::net::TcpStream {
    type Listener = tokio::net::TcpListener;

    async fn connect(addr: SocketAddrV4) -> io::Result<Self> {
        Self::connect(addr).await
    }

    async fn connect_from(local: SocketAddrV4, target: SocketAddrV4) -> io::Result<Self> {
        let socket = tokio::net::TcpSocket::new_v4()?;
        // SO_REUSEADDR lets the client rebind a fixed local port even when a
        // previous connection on that port is still in TIME_WAIT (important for
        // deterministic source ports mandated by firewall / SOME/IP config).
        socket.set_reuseaddr(true)?;
        socket.bind(SocketAddr::V4(local))?;
        socket.connect(SocketAddr::V4(target)).await
    }

    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        AsyncReadExt::read(self, buf).await
    }

    async fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        AsyncWriteExt::write(self, buf).await
    }

    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        AsyncWriteExt::write_all(self, buf).await
    }

    fn local_addr(&self) -> io::Result<SocketAddrV4> {
        Self::local_addr(self).map(|addr| expect_v4(addr, "tokio::net::TcpStream::local_addr"))
    }

    fn peer_addr(&self) -> io::Result<SocketAddrV4> {
        Self::peer_addr(self).map(|addr| expect_v4(addr, "tokio::net::TcpStream::peer_addr"))
    }

    fn set_keepalive(&self, config: &crate::config::TcpKeepaliveConfig) -> io::Result<()> {
        use socket2::{SockRef, TcpKeepalive};
        let keepalive = TcpKeepalive::new()
            .with_time(config.time)
            .with_interval(config.interval);
        // TCP_KEEPCNT is not available on all platforms; skip where unsupported.
        #[cfg(any(
            target_os = "android",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "fuchsia",
            target_os = "illumos",
            target_os = "ios",
            target_os = "linux",
            target_os = "macos",
            target_os = "netbsd",
            target_os = "tvos",
            target_os = "watchos",
        ))]
        let keepalive = keepalive.with_retries(config.retries);
        SockRef::from(self).set_tcp_keepalive(&keepalive)
    }
}

impl TcpListener for tokio::net::TcpListener {
    type Stream = tokio::net::TcpStream;

    async fn bind(addr: SocketAddrV4) -> io::Result<Self> {
        Self::bind(addr).await
    }

    async fn accept(&self) -> io::Result<(Self::Stream, SocketAddrV4)> {
        Self::accept(self)
            .await
            .map(|(stream, addr)| (stream, expect_v4(addr, "tokio::net::TcpListener::accept")))
    }

    fn local_addr(&self) -> io::Result<SocketAddrV4> {
        Self::local_addr(self).map(|addr| expect_v4(addr, "tokio::net::TcpListener::local_addr"))
    }
}
