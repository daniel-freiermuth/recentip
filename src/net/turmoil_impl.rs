//! Turmoil socket implementations for testing.
//! Enabled automatically during tests or with the `turmoil` feature.

use super::{TcpListener, TcpStream, UdpSocket};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

impl UdpSocket for turmoil::net::UdpSocket {
    async fn bind(addr: SocketAddrV4) -> io::Result<Self> {
        // turmoil only accepts 0.0.0.0 / :: or loopback as the bind IP —
        // specific unicast addresses are rejected with AddrNotAvailable.
        // Each turmoil host has exactly one virtual IP managed by the simulation,
        // so binding to 0.0.0.0 is semantically equivalent to binding to the
        // host's specific IP; normalize here so callers don't need #[cfg(test)].
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, addr.port());
        Self::bind(addr).await
    }

    async fn send_to(&self, buf: &[u8], target: SocketAddrV4) -> io::Result<usize> {
        Self::send_to(self, buf, target).await
    }

    async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddrV4)> {
        Self::recv_from(self, buf)
            .await
            .map(|(size, addr)| match addr {
                SocketAddr::V4(v4) => (size, v4),
                SocketAddr::V6(_) => {
                    unreachable!("turmoil::net::UdpSocket should only produce IPv4 addresses")
                }
            })
    }

    fn join_multicast_v4(&self, multiaddr: Ipv4Addr, _interface: Ipv4Addr) -> io::Result<()> {
        Self::join_multicast_v4(self, multiaddr, Ipv4Addr::UNSPECIFIED)
    }

    fn leave_multicast_v4(&self, multiaddr: Ipv4Addr, _interface: Ipv4Addr) -> io::Result<()> {
        Self::leave_multicast_v4(self, multiaddr, Ipv4Addr::UNSPECIFIED)
    }

    fn local_addr(&self) -> io::Result<SocketAddrV4> {
        Self::local_addr(self).map(|addr| match addr {
            SocketAddr::V4(v4) => v4,
            SocketAddr::V6(_) => {
                unreachable!("turmoil::net::UdpSocket should only produce IPv4 addresses")
            }
        })
    }
}

impl TcpStream for turmoil::net::TcpStream {
    type Listener = turmoil::net::TcpListener;

    async fn connect(addr: SocketAddrV4) -> io::Result<Self> {
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

    fn local_addr(&self) -> io::Result<SocketAddrV4> {
        Self::local_addr(self).map(|addr| match addr {
            SocketAddr::V4(v4) => v4,
            SocketAddr::V6(_) => {
                unreachable!("turmoil::net::TcpStream should only produce IPv4 addresses")
            }
        })
    }

    fn peer_addr(&self) -> io::Result<SocketAddrV4> {
        Self::peer_addr(self).map(|addr| match addr {
            SocketAddr::V4(v4) => v4,
            SocketAddr::V6(_) => {
                unreachable!("turmoil::net::TcpStream should only produce IPv4 addresses")
            }
        })
    }
}

impl TcpListener for turmoil::net::TcpListener {
    type Stream = turmoil::net::TcpStream;

    async fn bind(addr: SocketAddrV4) -> io::Result<Self> {
        // Same normalization as UdpSocket: turmoil rejects binding to specific
        // unicast addresses; 0.0.0.0 is semantically equivalent per-host.
        let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, addr.port()));
        Self::bind(addr).await
    }

    async fn accept(&self) -> io::Result<(Self::Stream, SocketAddrV4)> {
        Self::accept(self).await.map(|(stream, addr)| match addr {
            SocketAddr::V4(v4) => (stream, v4),
            SocketAddr::V6(_) => {
                unreachable!("turmoil::net::TcpListener should only produce IPv4 addresses")
            }
        })
    }

    fn local_addr(&self) -> io::Result<SocketAddrV4> {
        Self::local_addr(self).map(|addr| match addr {
            SocketAddr::V4(v4) => v4,
            SocketAddr::V6(_) => {
                unreachable!("turmoil::net::TcpListener should only produce IPv4 addresses")
            }
        })
    }
}
