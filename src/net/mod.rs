//! # Network Abstraction Layer
//!
//! This module provides traits that abstract over async network I/O,
//! enabling the runtime to work with different socket implementations.
//!
//! ## Purpose
//!
//! The abstraction allows:
//! - **Production**: Real tokio sockets for actual network communication
//! - **Testing**: Simulated [turmoil](https://docs.rs/turmoil) sockets for
//!   deterministic, fast network simulation
//!
//! ## Trait Overview
//!
//! | Trait | Purpose | Production Impl | Testing Impl |
//! |-------|---------|-----------------|--------------|
//! | [`UdpSocket`] | UDP send/recv, multicast | `tokio::net::UdpSocket` | `turmoil::net::UdpSocket` |
//! | [`TcpStream`] | TCP connection, read/write | `tokio::net::TcpStream` | `turmoil::net::TcpStream` |
//! | [`TcpListener`] | Accept TCP connections | `tokio::net::TcpListener` | `turmoil::net::TcpListener` |
//!
//! ## Usage
//!
//! User code typically doesn't interact with these traits directly.
//! The [`Runtime`](crate::Runtime) is generic over socket types:
//!
//! ```no_run
//! use someip_runtime::prelude::*;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     // Production (default) - uses tokio sockets internally
//!     let runtime = Runtime::new(RuntimeConfig::default()).await?;
//!
//!     // For testing with turmoil, see tests/compliance/ for examples
//!     // using Runtime::<turmoil types>::with_socket_type()
//!     Ok(())
//! }
//! ```
//!
//! ## Feature Flags
//!
//! - `turmoil` (default): Enables turmoil implementations for testing

use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};

mod tokio_impl;

#[cfg(feature = "turmoil")]
mod turmoil_impl;

/// Async UDP socket abstraction.
///
/// This trait enables the runtime to work with different UDP implementations.
/// Users don't need to implement this directly.
///
/// ## Required Methods
///
/// - `bind`: Create a socket bound to an address
/// - `send_to` / `recv_from`: Send and receive datagrams
/// - `join_multicast_v4` / `leave_multicast_v4`: Multicast group management
/// - `local_addr`: Get the bound address
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
