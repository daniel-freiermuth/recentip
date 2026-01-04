//! Turmoil socket implementations for testing.
//! Enabled automatically during tests or with the `turmoil` feature.

use super::{TcpListener, TcpStream, UdpSocket};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

impl UdpSocket for turmoil::net::UdpSocket {
    async fn bind(addr: SocketAddr) -> io::Result<Self> {
        Self::bind(addr).await
    }

    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize> {
        Self::send_to(self, buf, target).await
    }

    async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        Self::recv_from(self, buf).await
    }

    fn join_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()> {
        Self::join_multicast_v4(self, multiaddr, interface)
    }

    fn leave_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()> {
        Self::leave_multicast_v4(self, multiaddr, interface)
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Self::local_addr(self)
    }
}

impl TcpStream for turmoil::net::TcpStream {
    type Listener = turmoil::net::TcpListener;

    async fn connect(addr: SocketAddr) -> io::Result<Self> {
        Self::connect(addr).await
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
        Self::local_addr(self)
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Self::peer_addr(self)
    }
}

impl TcpListener for turmoil::net::TcpListener {
    type Stream = turmoil::net::TcpStream;

    async fn bind(addr: SocketAddr) -> io::Result<Self> {
        Self::bind(addr).await
    }

    async fn accept(&self) -> io::Result<(Self::Stream, SocketAddr)> {
        Self::accept(self).await
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Self::local_addr(self)
    }
}
