//! Network abstraction traits for async socket operations.
//!
//! This module defines traits that abstract over async network I/O,
//! allowing the runtime to work with both real tokio sockets (production)
//! and simulated turmoil sockets (testing).

use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};

mod tokio_impl;

#[cfg(feature = "turmoil")]
mod turmoil_impl;

/// Async UDP socket abstraction.
///
/// Implemented by `tokio::net::UdpSocket` and `turmoil::net::UdpSocket`.
pub trait UdpSocket: Send + Sync + Sized + 'static {
    /// Bind to the given address.
    fn bind(addr: SocketAddr) -> impl Future<Output = io::Result<Self>> + Send;

    /// Send data to the given address.
    fn send_to(
        &self,
        buf: &[u8],
        target: SocketAddr,
    ) -> impl Future<Output = io::Result<usize>> + Send;

    /// Receive data and the source address.
    fn recv_from(
        &self,
        buf: &mut [u8],
    ) -> impl Future<Output = io::Result<(usize, SocketAddr)>> + Send;

    /// Join a multicast group.
    fn join_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()>;

    /// Leave a multicast group.
    fn leave_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()>;

    /// Get the local address this socket is bound to.
    fn local_addr(&self) -> io::Result<SocketAddr>;
}

/// Async TCP stream abstraction.
///
/// Implemented by `tokio::net::TcpStream` and `turmoil::net::TcpStream`.
pub trait TcpStream: Send + Sized + 'static {
    /// The listener type that produces this stream.
    type Listener: TcpListener<Stream = Self>;

    /// Connect to the given address.
    fn connect(addr: SocketAddr) -> impl Future<Output = io::Result<Self>> + Send;

    /// Read data into the buffer.
    fn read(&mut self, buf: &mut [u8]) -> impl Future<Output = io::Result<usize>> + Send;

    /// Write data from the buffer.
    fn write(&mut self, buf: &[u8]) -> impl Future<Output = io::Result<usize>> + Send;

    /// Write all data from the buffer.
    fn write_all(&mut self, buf: &[u8]) -> impl Future<Output = io::Result<()>> + Send;

    /// Get the local address.
    fn local_addr(&self) -> io::Result<SocketAddr>;

    /// Get the peer address.
    fn peer_addr(&self) -> io::Result<SocketAddr>;
}

/// Async TCP listener abstraction.
///
/// Implemented by `tokio::net::TcpListener` and `turmoil::net::TcpListener`.
pub trait TcpListener: Send + Sync + Sized + 'static {
    /// The stream type produced when accepting connections.
    type Stream: TcpStream<Listener = Self>;

    /// Bind to the given address.
    fn bind(addr: SocketAddr) -> impl Future<Output = io::Result<Self>> + Send;

    /// Accept a new connection.
    fn accept(&self) -> impl Future<Output = io::Result<(Self::Stream, SocketAddr)>> + Send;

    /// Get the local address.
    fn local_addr(&self) -> io::Result<SocketAddr>;
}
