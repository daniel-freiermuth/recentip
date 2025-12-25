//! Turmoil socket implementations for testing.
//! Enabled automatically during tests or with the `turmoil` feature.

use super::{TcpListener, TcpStream, UdpSocket};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

impl UdpSocket for turmoil::net::UdpSocket {
    async fn bind(addr: SocketAddr) -> io::Result<Self> {
        turmoil::net::UdpSocket::bind(addr).await
    }

    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize> {
        turmoil::net::UdpSocket::send_to(self, buf, target).await
    }

    async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        turmoil::net::UdpSocket::recv_from(self, buf).await
    }

    fn join_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()> {
        turmoil::net::UdpSocket::join_multicast_v4(self, multiaddr, interface)
    }

    fn leave_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()> {
        turmoil::net::UdpSocket::leave_multicast_v4(self, multiaddr, interface)
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        turmoil::net::UdpSocket::local_addr(self)
    }
}

impl TcpStream for turmoil::net::TcpStream {
    type Listener = turmoil::net::TcpListener;

    async fn connect(addr: SocketAddr) -> io::Result<Self> {
        turmoil::net::TcpStream::connect(addr).await
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
        turmoil::net::TcpStream::local_addr(self)
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        turmoil::net::TcpStream::peer_addr(self)
    }
}

impl TcpListener for turmoil::net::TcpListener {
    type Stream = turmoil::net::TcpStream;

    async fn bind(addr: SocketAddr) -> io::Result<Self> {
        turmoil::net::TcpListener::bind(addr).await
    }

    async fn accept(&self) -> io::Result<(Self::Stream, SocketAddr)> {
        turmoil::net::TcpListener::accept(self).await
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        turmoil::net::TcpListener::local_addr(self)
    }
}
