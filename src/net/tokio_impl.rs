//! Tokio socket implementations.

use super::{TcpListener, TcpStream, UdpSocket};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

impl UdpSocket for tokio::net::UdpSocket {
    async fn bind(addr: SocketAddr) -> io::Result<Self> {
        // Use socket2 to set SO_REUSEPORT before binding.
        // This allows multiple processes/runtimes to share the same port,
        // which is required for SOME/IP SD multicast to work properly.
        use socket2::{Domain, Protocol, Socket, Type};

        let domain = match addr {
            SocketAddr::V4(_) => Domain::IPV4,
            SocketAddr::V6(_) => Domain::IPV6,
        };

        let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;

        // Set SO_REUSEADDR (allows reuse of local addresses)
        socket.set_reuse_address(true)?;

        // Set SO_REUSEPORT (allows multiple sockets to bind to same port)
        // This is available on Linux and recent macOS
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
        socket.set_reuse_port(true)?;

        // Set non-blocking before converting to tokio socket
        socket.set_nonblocking(true)?;

        // Bind to the address
        socket.bind(&addr.into())?;

        // Convert to tokio socket
        let std_socket: std::net::UdpSocket = socket.into();
        tokio::net::UdpSocket::from_std(std_socket)
    }

    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize> {
        tokio::net::UdpSocket::send_to(self, buf, target).await
    }

    async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        tokio::net::UdpSocket::recv_from(self, buf).await
    }

    fn join_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()> {
        // Enable multicast loopback so packets sent from this host are delivered back
        // to other sockets on the same host. This is required for testing on a single machine.
        tokio::net::UdpSocket::set_multicast_loop_v4(self, true)?;

        tokio::net::UdpSocket::join_multicast_v4(self, multiaddr, interface)
    }

    fn leave_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()> {
        tokio::net::UdpSocket::leave_multicast_v4(self, multiaddr, interface)
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        tokio::net::UdpSocket::local_addr(self)
    }
}

impl TcpStream for tokio::net::TcpStream {
    type Listener = tokio::net::TcpListener;

    async fn connect(addr: SocketAddr) -> io::Result<Self> {
        tokio::net::TcpStream::connect(addr).await
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

    fn local_addr(&self) -> io::Result<SocketAddr> {
        tokio::net::TcpStream::local_addr(self)
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        tokio::net::TcpStream::peer_addr(self)
    }
}

impl TcpListener for tokio::net::TcpListener {
    type Stream = tokio::net::TcpStream;

    async fn bind(addr: SocketAddr) -> io::Result<Self> {
        tokio::net::TcpListener::bind(addr).await
    }

    async fn accept(&self) -> io::Result<(Self::Stream, SocketAddr)> {
        tokio::net::TcpListener::accept(self).await
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        tokio::net::TcpListener::local_addr(self)
    }
}
