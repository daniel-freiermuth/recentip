//! Tokio socket implementations.

use super::{TcpListener, TcpStream, UdpSocket};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

impl UdpSocket for tokio::net::UdpSocket {
    async fn bind(addr: SocketAddr) -> io::Result<Self> {
        tokio::net::UdpSocket::bind(addr).await
    }

    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize> {
        tokio::net::UdpSocket::send_to(self, buf, target).await
    }

    async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        tokio::net::UdpSocket::recv_from(self, buf).await
    }

    fn join_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()> {
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
