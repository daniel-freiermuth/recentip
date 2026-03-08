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
//! The [`SomeIp`](crate::SomeIp) is generic over socket types:
//!
//! ```no_run
//! use recentip::prelude::*;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     // Production (default) - uses tokio sockets internally
//!     let runtime = recentip::configure()
//!         .sd_unicast("192.168.1.100".parse().unwrap())
//!         .sd_multicast_group("239.255.255.250".parse().unwrap())
//!         .start().await?;
//!
//!     // For testing with turmoil, see tests/compliance/ for examples
//!     // using SomeIp::<turmoil types>::with_socket_type()
//!     Ok(())
//! }
//! ```
//!
//! ## Feature Flags
//!
//! - `turmoil` (default): Enables turmoil implementations for testing

use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};

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
    fn bind(addr: SocketAddrV4) -> impl Future<Output = io::Result<Self>> + Send;

    /// Send data to the given address.
    fn send_to(
        &self,
        buf: &[u8],
        target: SocketAddrV4,
    ) -> impl Future<Output = io::Result<usize>> + Send;

    /// Receive data and the source address.
    fn recv_from(
        &self,
        buf: &mut [u8],
    ) -> impl Future<Output = io::Result<(usize, SocketAddrV4)>> + Send;

    /// Join a multicast group.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the multicast join fails.
    fn join_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()>;

    /// Leave a multicast group.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the multicast leave fails.
    fn leave_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()>;

    /// Get the local address this socket is bound to.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the address cannot be retrieved.
    fn local_addr(&self) -> io::Result<SocketAddrV4>;

    /// Set the IPv4 multicast outgoing interface to the given local address.
    ///
    /// This controls which source IP appears on outgoing multicast datagrams.
    /// Calling this on a socket bound to `0.0.0.0` with `addr = <advertised_ip>`
    /// causes multicast packets to carry `<advertised_ip>` as their source,
    /// so that recipients can address unicast replies back to the right endpoint.
    ///
    /// The default implementation is a no-op and returns `Ok(())`.  Simulated
    /// sockets (turmoil) inherit this default because their virtual routing
    /// already uses the correct per-host source addresses.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the option cannot be set.
    fn set_multicast_if_v4(&self, addr: Ipv4Addr) -> io::Result<()> {
        let _ = addr;
        Ok(())
    }
}

/// Async TCP stream abstraction.
///
/// Implemented by `tokio::net::TcpStream` and `turmoil::net::TcpStream`.
pub trait TcpStream: Send + Sized + 'static {
    /// The listener type that produces this stream.
    type Listener: TcpListener<Stream = Self>;

    /// Connect to the given address.
    fn connect(addr: SocketAddrV4) -> impl Future<Output = io::Result<Self>> + Send;

    /// Read data into the buffer.
    fn read(&mut self, buf: &mut [u8]) -> impl Future<Output = io::Result<usize>> + Send;

    /// Write data from the buffer.
    fn write(&mut self, buf: &[u8]) -> impl Future<Output = io::Result<usize>> + Send;

    /// Write all data from the buffer.
    fn write_all(&mut self, buf: &[u8]) -> impl Future<Output = io::Result<()>> + Send;

    /// Get the local address.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the address cannot be retrieved.
    fn local_addr(&self) -> io::Result<SocketAddrV4>;

    /// Get the peer address.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the peer address cannot be retrieved.
    fn peer_addr(&self) -> io::Result<SocketAddrV4>;
}

/// Async TCP listener abstraction.
///
/// Implemented by `tokio::net::TcpListener` and `turmoil::net::TcpListener`.
pub trait TcpListener: Send + Sync + Sized + 'static {
    /// The stream type produced when accepting connections.
    type Stream: TcpStream<Listener = Self>;

    /// Bind to the given address.
    fn bind(addr: SocketAddrV4) -> impl Future<Output = io::Result<Self>> + Send;

    /// Accept a new connection.
    fn accept(&self) -> impl Future<Output = io::Result<(Self::Stream, SocketAddrV4)>> + Send;

    /// Get the local address.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the address cannot be retrieved.
    fn local_addr(&self) -> io::Result<SocketAddrV4>;
}
