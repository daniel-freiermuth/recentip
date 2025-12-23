//! Simulated network backend for testing
//!
//! Provides deterministic, controllable network behavior for tests.

use someip_runtime::{
    IoContext, IoError, IoErrorKind, IoResult, TcpListenerOps, TcpStreamOps, UdpSocketOps,
};
use std::collections::{HashMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ============================================================================
// NETWORK EVENT HISTORY
// ============================================================================

#[derive(Debug, Clone)]
pub enum NetworkEvent {
    UdpSent {
        from: SocketAddr,
        to: SocketAddr,
        dst_port: u16,
        size: usize,
        data: Vec<u8>, // Actual packet bytes for wire format verification
        timestamp: Duration,
    },
    UdpReceived {
        at: SocketAddr,
        from: SocketAddr,
        size: usize,
        data: Vec<u8>, // Actual packet bytes
        timestamp: Duration,
    },
    TcpConnect {
        from: SocketAddr,
        to: SocketAddr,
        timestamp: Duration,
    },
    TcpAccept {
        listener: SocketAddr,
        client: SocketAddr,
        timestamp: Duration,
    },
    TcpSent {
        from: SocketAddr,
        to: SocketAddr,
        size: usize,
        data: Vec<u8>, // Actual packet bytes
        timestamp: Duration,
    },
    TcpReceived {
        at: SocketAddr,
        from: SocketAddr,
        size: usize,
        data: Vec<u8>, // Actual packet bytes
        timestamp: Duration,
    },
    TcpReset {
        connection: (SocketAddr, SocketAddr),
        timestamp: Duration,
    },
    PacketDropped {
        from: SocketAddr,
        to: SocketAddr,
        reason: DropReason,
        timestamp: Duration,
    },
    MulticastJoin {
        socket: SocketAddr,
        group: Ipv4Addr,
        timestamp: Duration,
    },
    MulticastLeave {
        socket: SocketAddr,
        group: Ipv4Addr,
        timestamp: Duration,
    },
}

#[derive(Debug, Clone)]
pub enum DropReason {
    RandomLoss,
    Partitioned,
    BufferFull,
}

// ============================================================================
// SIMULATION CONFIGURATION
// ============================================================================

#[derive(Clone)]
pub struct SimConfig {
    /// Base latency for all packets
    pub latency: Duration,
    /// Random packet drop rate (0.0 - 1.0)
    pub drop_rate: f64,
    /// Jitter as a fraction of latency
    pub jitter: f64,
    /// Maximum pending packets per socket
    pub buffer_size: usize,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            latency: Duration::from_millis(1),
            drop_rate: 0.0,
            jitter: 0.0,
            buffer_size: 1024,
        }
    }
}

// ============================================================================
// SIMULATED NETWORK CORE
// ============================================================================

struct Packet {
    from: SocketAddr,
    to: SocketAddr,
    data: Vec<u8>,
    deliver_at: Duration,
}

struct TcpConnection {
    local: SocketAddr,
    remote: SocketAddr,
    tx_buffer: VecDeque<u8>,
    rx_buffer: VecDeque<u8>,
    connected: bool,
}

struct NetworkState {
    config: SimConfig,
    current_time: Duration,
    start_instant: Instant,

    // UDP
    udp_sockets: HashMap<SocketAddr, UdpSocketState>,
    pending_udp: VecDeque<Packet>,

    // TCP
    tcp_listeners: HashMap<SocketAddr, TcpListenerState>,
    tcp_connections: HashMap<(SocketAddr, SocketAddr), TcpConnection>,
    pending_tcp_connects: VecDeque<(SocketAddr, SocketAddr)>,

    // Multicast
    multicast_groups: HashMap<Ipv4Addr, Vec<SocketAddr>>,

    // Fault injection
    partitions: Vec<(String, String)>,
    broken_tcp: Vec<(SocketAddr, SocketAddr)>,

    // History
    history: Vec<NetworkEvent>,

    // Port allocation
    next_ephemeral_port: u16,
}

struct UdpSocketState {
    local_addr: SocketAddr,
    rx_buffer: VecDeque<(Vec<u8>, SocketAddr)>,
    nonblocking: bool,
}

struct TcpListenerState {
    local_addr: SocketAddr,
    pending_connections: VecDeque<(SocketAddr, SocketAddr)>,
    nonblocking: bool,
}

impl NetworkState {
    fn new(config: SimConfig) -> Self {
        Self {
            config,
            current_time: Duration::ZERO,
            start_instant: Instant::now(),
            udp_sockets: HashMap::new(),
            pending_udp: VecDeque::new(),
            tcp_listeners: HashMap::new(),
            tcp_connections: HashMap::new(),
            pending_tcp_connects: VecDeque::new(),
            multicast_groups: HashMap::new(),
            partitions: Vec::new(),
            broken_tcp: Vec::new(),
            history: Vec::new(),
            next_ephemeral_port: 49152,
        }
    }

    fn allocate_port(&mut self) -> u16 {
        let port = self.next_ephemeral_port;
        self.next_ephemeral_port += 1;
        port
    }

    fn is_partitioned(&self, a: SocketAddr, b: SocketAddr) -> bool {
        for (net_a, net_b) in &self.partitions {
            let in_a_first = self.addr_matches_cidr(a, net_a);
            let in_b_second = self.addr_matches_cidr(b, net_b);
            let in_b_first = self.addr_matches_cidr(b, net_a);
            let in_a_second = self.addr_matches_cidr(a, net_b);

            if (in_a_first && in_b_second) || (in_b_first && in_a_second) {
                return true;
            }
        }
        false
    }

    fn addr_matches_cidr(&self, addr: SocketAddr, cidr: &str) -> bool {
        // Simple CIDR matching (e.g., "192.168.1.0/24")
        let parts: Vec<&str> = cidr.split('/').collect();
        if parts.len() != 2 {
            return false;
        }

        let network: std::net::IpAddr = match parts[0].parse() {
            Ok(ip) => ip,
            Err(_) => return false,
        };

        let prefix: u8 = match parts[1].parse() {
            Ok(p) => p,
            Err(_) => return false,
        };

        match (addr.ip(), network) {
            (std::net::IpAddr::V4(a), std::net::IpAddr::V4(n)) => {
                let mask = !0u32 << (32 - prefix);
                (u32::from(a) & mask) == (u32::from(n) & mask)
            }
            _ => false,
        }
    }

    fn should_drop(&self) -> bool {
        if self.config.drop_rate <= 0.0 {
            return false;
        }
        rand::random::<f64>() < self.config.drop_rate
    }

    fn process_pending_packets(&mut self) {
        let now = self.current_time;

        // Process UDP packets
        let mut delivered = Vec::new();
        for (idx, packet) in self.pending_udp.iter().enumerate() {
            if packet.deliver_at <= now {
                delivered.push(idx);
            }
        }

        for idx in delivered.into_iter().rev() {
            let packet = self.pending_udp.remove(idx).unwrap();
            self.deliver_udp_packet(packet);
        }
    }

    fn deliver_udp_packet(&mut self, packet: Packet) {
        // Check for multicast
        if let SocketAddr::V4(addr) = packet.to {
            if addr.ip().is_multicast() {
                self.deliver_multicast(packet);
                return;
            }
        }

        // Unicast delivery
        if let Some(socket) = self.udp_sockets.get_mut(&packet.to) {
            if socket.rx_buffer.len() < self.config.buffer_size {
                socket
                    .rx_buffer
                    .push_back((packet.data.clone(), packet.from));
                self.history.push(NetworkEvent::UdpReceived {
                    at: packet.to,
                    from: packet.from,
                    size: packet.data.len(),
                    data: packet.data.clone(),
                    timestamp: self.current_time,
                });
            } else {
                self.history.push(NetworkEvent::PacketDropped {
                    from: packet.from,
                    to: packet.to,
                    reason: DropReason::BufferFull,
                    timestamp: self.current_time,
                });
            }
        }
    }

    fn deliver_multicast(&mut self, packet: Packet) {
        if let SocketAddr::V4(addr) = packet.to {
            let group = *addr.ip();
            if let Some(members) = self.multicast_groups.get(&group).cloned() {
                for member in members {
                    if let Some(socket) = self.udp_sockets.get_mut(&member) {
                        if socket.rx_buffer.len() < self.config.buffer_size {
                            socket
                                .rx_buffer
                                .push_back((packet.data.clone(), packet.from));
                            self.history.push(NetworkEvent::UdpReceived {
                                at: member,
                                from: packet.from,
                                size: packet.data.len(),
                                data: packet.data.clone(),
                                timestamp: self.current_time,
                            });
                        }
                    }
                }
            }
        }
    }
}

// ============================================================================
// SIMULATED NETWORK PUBLIC API
// ============================================================================

#[derive(Clone)]
pub struct SimulatedNetwork {
    state: Arc<Mutex<NetworkState>>,
}

impl SimulatedNetwork {
    /// Create a new simulated network with default config
    pub fn new() -> Self {
        Self::with_config(SimConfig::default())
    }

    /// Create a new simulated network with custom config
    pub fn with_config(config: SimConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(NetworkState::new(config))),
        }
    }

    /// Create a network and two IoContexts for typical client/server testing
    pub fn new_pair() -> (Self, SimulatedIoContext, SimulatedIoContext) {
        let network = Self::new();
        let io_a = network.io_context("192.168.1.10".parse().unwrap());
        let io_b = network.io_context("192.168.1.20".parse().unwrap());
        (network, io_a, io_b)
    }

    /// Create an IoContext bound to a specific IP address
    pub fn io_context(&self, local_ip: Ipv4Addr) -> SimulatedIoContext {
        SimulatedIoContext {
            network: self.clone(),
            local_ip,
        }
    }

    /// Advance simulated time and process pending events
    pub fn advance(&self, duration: Duration) {
        let mut state = self.state.lock().unwrap();
        let target = state.current_time + duration;

        // Process in small steps to ensure proper packet ordering
        while state.current_time < target {
            state.current_time += Duration::from_micros(100);
            state.process_pending_packets();
        }

        state.current_time = target;
    }

    /// Get current simulated time
    pub fn now(&self) -> Duration {
        self.state.lock().unwrap().current_time
    }

    /// Set random packet drop rate (0.0 - 1.0)
    pub fn set_drop_rate(&self, rate: f64) {
        self.state.lock().unwrap().config.drop_rate = rate.clamp(0.0, 1.0);
    }

    /// Create a network partition between two CIDR ranges
    pub fn partition(&self, net_a: &str, net_b: &str) {
        self.state
            .lock()
            .unwrap()
            .partitions
            .push((net_a.to_string(), net_b.to_string()));
    }

    /// Remove all partitions
    pub fn heal(&self) {
        self.state.lock().unwrap().partitions.clear();
    }

    /// Break a specific TCP connection
    pub fn break_tcp(&self, a: SocketAddr, b: SocketAddr) {
        let mut state = self.state.lock().unwrap();
        state.broken_tcp.push((a, b));
        state.tcp_connections.remove(&(a, b));
        state.tcp_connections.remove(&(b, a));
        let timestamp = state.current_time;
        state.history.push(NetworkEvent::TcpReset {
            connection: (a, b),
            timestamp,
        });
    }

    /// Get history of network events
    pub fn history(&self) -> Vec<NetworkEvent> {
        self.state.lock().unwrap().history.clone()
    }

    /// Clear history
    pub fn clear_history(&self) {
        self.state.lock().unwrap().history.clear();
    }
}

// ============================================================================
// SIMULATED IO CONTEXT
// ============================================================================

pub struct SimulatedIoContext {
    network: SimulatedNetwork,
    local_ip: Ipv4Addr,
}

impl IoContext for SimulatedIoContext {
    type UdpSocket = SimulatedUdpSocket;
    type TcpStream = SimulatedTcpStream;
    type TcpListener = SimulatedTcpListener;

    fn bind_udp(&self, addr: SocketAddr) -> IoResult<Self::UdpSocket> {
        let mut state = self.network.state.lock().unwrap();

        let local_addr = if addr.port() == 0 {
            SocketAddr::V4(SocketAddrV4::new(self.local_ip, state.allocate_port()))
        } else {
            if state.udp_sockets.contains_key(&addr) {
                return Err(IoError::new(IoErrorKind::AddrInUse, "address in use"));
            }
            addr
        };

        state.udp_sockets.insert(
            local_addr,
            UdpSocketState {
                local_addr,
                rx_buffer: VecDeque::new(),
                nonblocking: false,
            },
        );

        Ok(SimulatedUdpSocket {
            network: self.network.clone(),
            local_addr,
        })
    }

    fn connect_tcp(&self, addr: SocketAddr) -> IoResult<Self::TcpStream> {
        let mut state = self.network.state.lock().unwrap();

        // Check for listener
        if !state.tcp_listeners.contains_key(&addr) {
            return Err(IoError::new(
                IoErrorKind::ConnectionRefused,
                "connection refused",
            ));
        }

        let local_addr = SocketAddr::V4(SocketAddrV4::new(self.local_ip, state.allocate_port()));

        // Check partition
        if state.is_partitioned(local_addr, addr) {
            return Err(IoError::new(IoErrorKind::TimedOut, "partitioned"));
        }

        // Create connection
        let conn = TcpConnection {
            local: local_addr,
            remote: addr,
            tx_buffer: VecDeque::new(),
            rx_buffer: VecDeque::new(),
            connected: true,
        };
        state.tcp_connections.insert((local_addr, addr), conn);

        // Notify listener
        if let Some(listener) = state.tcp_listeners.get_mut(&addr) {
            listener.pending_connections.push_back((local_addr, addr));
        }

        let timestamp = state.current_time;
        state.history.push(NetworkEvent::TcpConnect {
            from: local_addr,
            to: addr,
            timestamp,
        });

        Ok(SimulatedTcpStream {
            network: self.network.clone(),
            local_addr,
            remote_addr: addr,
        })
    }

    fn listen_tcp(&self, addr: SocketAddr) -> IoResult<Self::TcpListener> {
        let mut state = self.network.state.lock().unwrap();

        let local_addr = if addr.port() == 0 {
            SocketAddr::V4(SocketAddrV4::new(self.local_ip, state.allocate_port()))
        } else {
            if state.tcp_listeners.contains_key(&addr) {
                return Err(IoError::new(IoErrorKind::AddrInUse, "address in use"));
            }
            addr
        };

        state.tcp_listeners.insert(
            local_addr,
            TcpListenerState {
                local_addr,
                pending_connections: VecDeque::new(),
                nonblocking: false,
            },
        );

        Ok(SimulatedTcpListener {
            network: self.network.clone(),
            local_addr,
        })
    }

    fn now(&self) -> Instant {
        let state = self.network.state.lock().unwrap();
        state.start_instant + state.current_time
    }

    fn sleep(&self, duration: Duration) {
        // In simulation, we just advance time
        self.network.advance(duration);
    }
}

// ============================================================================
// SIMULATED SOCKETS
// ============================================================================

pub struct SimulatedUdpSocket {
    network: SimulatedNetwork,
    local_addr: SocketAddr,
}

impl UdpSocketOps for SimulatedUdpSocket {
    fn send_to(&self, buf: &[u8], addr: SocketAddr) -> IoResult<usize> {
        let mut state = self.network.state.lock().unwrap();
        let timestamp = state.current_time;

        // Check partition
        if state.is_partitioned(self.local_addr, addr) {
            state.history.push(NetworkEvent::PacketDropped {
                from: self.local_addr,
                to: addr,
                reason: DropReason::Partitioned,
                timestamp,
            });
            return Ok(buf.len()); // Appears to succeed
        }

        // Check random drop
        if state.should_drop() {
            state.history.push(NetworkEvent::PacketDropped {
                from: self.local_addr,
                to: addr,
                reason: DropReason::RandomLoss,
                timestamp,
            });
            return Ok(buf.len());
        }

        // Record send
        state.history.push(NetworkEvent::UdpSent {
            from: self.local_addr,
            to: addr,
            dst_port: addr.port(),
            size: buf.len(),
            data: buf.to_vec(),
            timestamp,
        });

        // Queue for delivery
        let deliver_at = state.current_time + state.config.latency;
        state.pending_udp.push_back(Packet {
            from: self.local_addr,
            to: addr,
            data: buf.to_vec(),
            deliver_at,
        });

        Ok(buf.len())
    }

    fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr), IoError> {
        loop {
            match self.try_recv_from(buf) {
                Ok(result) => return Ok(result),
                Err(e) if e.kind() == IoErrorKind::WouldBlock => {
                    // In blocking mode, advance time and retry
                    self.network.advance(Duration::from_millis(1));
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn try_recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr), IoError> {
        let mut state = self.network.state.lock().unwrap();

        let socket = state
            .udp_sockets
            .get_mut(&self.local_addr)
            .ok_or_else(|| IoError::new(IoErrorKind::Other, "socket closed"))?;

        match socket.rx_buffer.pop_front() {
            Some((data, from)) => {
                let len = data.len().min(buf.len());
                buf[..len].copy_from_slice(&data[..len]);
                Ok((len, from))
            }
            None => Err(IoError::new(IoErrorKind::WouldBlock, "would block")),
        }
    }

    fn local_addr(&self) -> IoResult<SocketAddr> {
        Ok(self.local_addr)
    }

    fn join_multicast_v4(&self, group: Ipv4Addr, _interface: Ipv4Addr) -> IoResult<()> {
        let mut state = self.network.state.lock().unwrap();
        let timestamp = state.current_time;

        state
            .multicast_groups
            .entry(group)
            .or_insert_with(Vec::new)
            .push(self.local_addr);

        state.history.push(NetworkEvent::MulticastJoin {
            socket: self.local_addr,
            group,
            timestamp,
        });

        Ok(())
    }

    fn leave_multicast_v4(&self, group: Ipv4Addr, _interface: Ipv4Addr) -> IoResult<()> {
        let mut state = self.network.state.lock().unwrap();
        let timestamp = state.current_time;

        if let Some(members) = state.multicast_groups.get_mut(&group) {
            members.retain(|&addr| addr != self.local_addr);
        }

        state.history.push(NetworkEvent::MulticastLeave {
            socket: self.local_addr,
            group,
            timestamp,
        });

        Ok(())
    }

    fn set_nonblocking(&self, nonblocking: bool) -> IoResult<()> {
        let mut state = self.network.state.lock().unwrap();
        if let Some(socket) = state.udp_sockets.get_mut(&self.local_addr) {
            socket.nonblocking = nonblocking;
        }
        Ok(())
    }
}

impl Drop for SimulatedUdpSocket {
    fn drop(&mut self) {
        let mut state = self.network.state.lock().unwrap();
        state.udp_sockets.remove(&self.local_addr);

        // Remove from all multicast groups
        for members in state.multicast_groups.values_mut() {
            members.retain(|&addr| addr != self.local_addr);
        }
    }
}

pub struct SimulatedTcpStream {
    network: SimulatedNetwork,
    local_addr: SocketAddr,
    remote_addr: SocketAddr,
}

impl TcpStreamOps for SimulatedTcpStream {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        let mut state = self.network.state.lock().unwrap();
        let timestamp = state.current_time;

        let key = (self.local_addr, self.remote_addr);
        let conn = state
            .tcp_connections
            .get_mut(&key)
            .ok_or_else(|| IoError::new(IoErrorKind::NotConnected, "not connected"))?;

        if !conn.connected {
            return Err(IoError::new(
                IoErrorKind::ConnectionReset,
                "connection reset",
            ));
        }

        if conn.rx_buffer.is_empty() {
            return Err(IoError::new(IoErrorKind::WouldBlock, "would block"));
        }

        let len = buf.len().min(conn.rx_buffer.len());
        let mut received_data = Vec::with_capacity(len);
        for i in 0..len {
            let byte = conn.rx_buffer.pop_front().unwrap();
            buf[i] = byte;
            received_data.push(byte);
        }

        state.history.push(NetworkEvent::TcpReceived {
            at: self.local_addr,
            from: self.remote_addr,
            size: len,
            data: received_data,
            timestamp,
        });

        Ok(len)
    }

    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        let mut state = self.network.state.lock().unwrap();
        let timestamp = state.current_time;

        // Check for broken connection
        if state
            .broken_tcp
            .contains(&(self.local_addr, self.remote_addr))
            || state
                .broken_tcp
                .contains(&(self.remote_addr, self.local_addr))
        {
            return Err(IoError::new(
                IoErrorKind::ConnectionReset,
                "connection reset",
            ));
        }

        let key = (self.local_addr, self.remote_addr);
        let conn = state
            .tcp_connections
            .get(&key)
            .ok_or_else(|| IoError::new(IoErrorKind::NotConnected, "not connected"))?;

        if !conn.connected {
            return Err(IoError::new(
                IoErrorKind::ConnectionReset,
                "connection reset",
            ));
        }

        // Write to remote's rx buffer
        let remote_key = (self.remote_addr, self.local_addr);
        if let Some(remote_conn) = state.tcp_connections.get_mut(&remote_key) {
            remote_conn.rx_buffer.extend(buf);
        }

        state.history.push(NetworkEvent::TcpSent {
            from: self.local_addr,
            to: self.remote_addr,
            size: buf.len(),
            data: buf.to_vec(),
            timestamp,
        });

        Ok(buf.len())
    }

    fn flush(&mut self) -> IoResult<()> {
        Ok(())
    }

    fn local_addr(&self) -> IoResult<SocketAddr> {
        Ok(self.local_addr)
    }

    fn peer_addr(&self) -> IoResult<SocketAddr> {
        Ok(self.remote_addr)
    }

    fn shutdown(&self, _how: std::net::Shutdown) -> IoResult<()> {
        let mut state = self.network.state.lock().unwrap();
        state
            .tcp_connections
            .remove(&(self.local_addr, self.remote_addr));
        Ok(())
    }

    fn set_nonblocking(&self, _nonblocking: bool) -> IoResult<()> {
        Ok(())
    }
}

impl Drop for SimulatedTcpStream {
    fn drop(&mut self) {
        let mut state = self.network.state.lock().unwrap();
        state
            .tcp_connections
            .remove(&(self.local_addr, self.remote_addr));
    }
}

pub struct SimulatedTcpListener {
    network: SimulatedNetwork,
    local_addr: SocketAddr,
}

impl TcpListenerOps for SimulatedTcpListener {
    type Stream = SimulatedTcpStream;

    fn accept(&self) -> Result<(Self::Stream, SocketAddr), IoError> {
        let mut state = self.network.state.lock().unwrap();
        let timestamp = state.current_time;

        let listener = state
            .tcp_listeners
            .get_mut(&self.local_addr)
            .ok_or_else(|| IoError::new(IoErrorKind::Other, "listener closed"))?;

        match listener.pending_connections.pop_front() {
            Some((client_addr, _server_addr)) => {
                // Create server-side connection
                let conn = TcpConnection {
                    local: self.local_addr,
                    remote: client_addr,
                    tx_buffer: VecDeque::new(),
                    rx_buffer: VecDeque::new(),
                    connected: true,
                };
                state
                    .tcp_connections
                    .insert((self.local_addr, client_addr), conn);

                state.history.push(NetworkEvent::TcpAccept {
                    listener: self.local_addr,
                    client: client_addr,
                    timestamp,
                });

                Ok((
                    SimulatedTcpStream {
                        network: self.network.clone(),
                        local_addr: self.local_addr,
                        remote_addr: client_addr,
                    },
                    client_addr,
                ))
            }
            None => Err(IoError::new(IoErrorKind::WouldBlock, "would block")),
        }
    }

    fn local_addr(&self) -> IoResult<SocketAddr> {
        Ok(self.local_addr)
    }

    fn set_nonblocking(&self, nonblocking: bool) -> IoResult<()> {
        let mut state = self.network.state.lock().unwrap();
        if let Some(listener) = state.tcp_listeners.get_mut(&self.local_addr) {
            listener.nonblocking = nonblocking;
        }
        Ok(())
    }
}

impl Drop for SimulatedTcpListener {
    fn drop(&mut self) {
        let mut state = self.network.state.lock().unwrap();
        state.tcp_listeners.remove(&self.local_addr);
    }
}

// ============================================================================
// TESTS FOR THE SIMULATION ITSELF
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn udp_roundtrip() {
        let (network, io_a, io_b) = SimulatedNetwork::new_pair();

        let addr_a = SocketAddr::V4(SocketAddrV4::new("192.168.1.10".parse().unwrap(), 5000));
        let addr_b = SocketAddr::V4(SocketAddrV4::new("192.168.1.20".parse().unwrap(), 6000));

        let sock_a = io_a.bind_udp(addr_a).unwrap();
        let sock_b = io_b.bind_udp(addr_b).unwrap();

        sock_a.send_to(b"hello", addr_b).unwrap();
        network.advance(Duration::from_millis(10));

        let mut buf = [0u8; 1024];
        let (len, from) = sock_b.try_recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"hello");
        assert_eq!(from, addr_a);
    }

    #[test]
    fn multicast_delivery() {
        let network = SimulatedNetwork::new();
        let io_a = network.io_context("192.168.1.10".parse().unwrap());
        let io_b = network.io_context("192.168.1.20".parse().unwrap());
        let io_c = network.io_context("192.168.1.30".parse().unwrap());

        let mcast_group: Ipv4Addr = "239.255.1.1".parse().unwrap();
        let mcast_addr = SocketAddr::V4(SocketAddrV4::new(mcast_group, 30490));

        let sender = io_a
            .bind_udp(SocketAddr::V4(SocketAddrV4::new(
                "192.168.1.10".parse().unwrap(),
                5000,
            )))
            .unwrap();

        let recv_b = io_b
            .bind_udp(SocketAddr::V4(SocketAddrV4::new(
                "192.168.1.20".parse().unwrap(),
                30490,
            )))
            .unwrap();
        let recv_c = io_c
            .bind_udp(SocketAddr::V4(SocketAddrV4::new(
                "192.168.1.30".parse().unwrap(),
                30490,
            )))
            .unwrap();

        recv_b
            .join_multicast_v4(mcast_group, "192.168.1.20".parse().unwrap())
            .unwrap();
        recv_c
            .join_multicast_v4(mcast_group, "192.168.1.30".parse().unwrap())
            .unwrap();

        sender.send_to(b"multicast!", mcast_addr).unwrap();
        network.advance(Duration::from_millis(10));

        let mut buf = [0u8; 1024];

        let (len, _) = recv_b.try_recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"multicast!");

        let (len, _) = recv_c.try_recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"multicast!");
    }

    #[test]
    fn partition_blocks_traffic() {
        let (network, io_a, io_b) = SimulatedNetwork::new_pair();

        let addr_a = SocketAddr::V4(SocketAddrV4::new("192.168.1.10".parse().unwrap(), 5000));
        let addr_b = SocketAddr::V4(SocketAddrV4::new("192.168.2.20".parse().unwrap(), 6000));

        let io_b = network.io_context("192.168.2.20".parse().unwrap());

        let sock_a = io_a.bind_udp(addr_a).unwrap();
        let sock_b = io_b.bind_udp(addr_b).unwrap();

        // Before partition
        sock_a.send_to(b"hello", addr_b).unwrap();
        network.advance(Duration::from_millis(10));

        let mut buf = [0u8; 1024];
        assert!(sock_b.try_recv_from(&mut buf).is_ok());

        // Create partition
        network.partition("192.168.1.0/24", "192.168.2.0/24");

        sock_a.send_to(b"blocked", addr_b).unwrap();
        network.advance(Duration::from_millis(10));

        // Should not receive
        assert!(sock_b.try_recv_from(&mut buf).is_err());

        // Check history shows dropped packet
        let history = network.history();
        assert!(history.iter().any(|e| matches!(
            e,
            NetworkEvent::PacketDropped {
                reason: DropReason::Partitioned,
                ..
            }
        )));
    }

    #[test]
    fn tcp_connect_and_transfer() {
        let (network, io_client, io_server) = SimulatedNetwork::new_pair();

        let server_addr = SocketAddr::V4(SocketAddrV4::new("192.168.1.20".parse().unwrap(), 8080));

        let listener = io_server.listen_tcp(server_addr).unwrap();
        listener.set_nonblocking(true).unwrap();

        let mut client = io_client.connect_tcp(server_addr).unwrap();

        network.advance(Duration::from_millis(10));

        let (mut server_conn, client_addr) = listener.accept().unwrap();

        client.write(b"ping").unwrap();
        network.advance(Duration::from_millis(10));

        let mut buf = [0u8; 1024];
        let len = server_conn.read(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"ping");

        server_conn.write(b"pong").unwrap();
        network.advance(Duration::from_millis(10));

        let len = client.read(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"pong");
    }
}
