//! Runtime configuration for SOME/IP runtime.
//!
//! Contains `RuntimeConfig`, `RuntimeConfigBuilder`, `Transport`, and `MethodConfig`.

use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

/// Default SD multicast address
pub const DEFAULT_SD_MULTICAST: Ipv4Addr = Ipv4Addr::new(239, 255, 0, 1);

/// Default SD port
pub const DEFAULT_SD_PORT: u16 = 30490;

/// Default TTL for SD entries (seconds)
pub const DEFAULT_TTL: u32 = 3600;

/// Default cyclic offer interval (ms)
pub const DEFAULT_CYCLIC_OFFER_DELAY: u64 = 1000;

/// Default find request repetitions
pub const DEFAULT_FIND_REPETITIONS: u32 = 3;

/// Transport protocol for RPC communication
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transport {
    /// UDP transport (default)
    #[default]
    Udp,
    /// TCP transport
    Tcp,
}

/// Runtime configuration
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Local address to bind to (default: 0.0.0.0:0)
    pub local_addr: SocketAddr,
    /// SD multicast group address (default: 239.255.0.1:30490)
    pub sd_multicast: SocketAddr,
    /// TTL for SD entries (default: 3600 seconds)
    pub ttl: u32,
    /// Cyclic offer delay in ms (default: 1000)
    pub cyclic_offer_delay: u64,
    /// Transport protocol for RPC communication (default: UDP)
    pub transport: Transport,
    /// Enable Magic Cookies for TCP resynchronization (default: false)
    ///
    /// When enabled (feat_req_recentip_586, feat_req_recentip_591, feat_req_recentip_592):
    /// - Each TCP segment starts with a Magic Cookie message
    /// - Only one Magic Cookie per segment
    /// - Allows resync in testing/debugging scenarios
    pub magic_cookies: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            // Use the SD port for multicast group membership to work in turmoil
            local_addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_SD_PORT)),
            sd_multicast: SocketAddr::V4(SocketAddrV4::new(DEFAULT_SD_MULTICAST, DEFAULT_SD_PORT)),
            ttl: DEFAULT_TTL,
            cyclic_offer_delay: DEFAULT_CYCLIC_OFFER_DELAY,
            transport: Transport::Udp,
            magic_cookies: false,
        }
    }
}

impl RuntimeConfig {
    /// Create a new builder
    pub fn builder() -> RuntimeConfigBuilder {
        RuntimeConfigBuilder::default()
    }
}

/// Builder for RuntimeConfig
#[derive(Default)]
pub struct RuntimeConfigBuilder {
    config: RuntimeConfig,
}

impl RuntimeConfigBuilder {
    /// Set the local address
    pub fn local_addr(mut self, addr: SocketAddr) -> Self {
        self.config.local_addr = addr;
        self
    }

    /// Set the SD multicast address
    pub fn sd_multicast(mut self, addr: SocketAddr) -> Self {
        self.config.sd_multicast = addr;
        self
    }

    /// Set the TTL for SD entries
    pub fn ttl(mut self, ttl: u32) -> Self {
        self.config.ttl = ttl;
        self
    }

    /// Set the cyclic offer delay
    pub fn cyclic_offer_delay(mut self, delay_ms: u64) -> Self {
        self.config.cyclic_offer_delay = delay_ms;
        self
    }

    /// Set the transport protocol (UDP or TCP)
    pub fn transport(mut self, transport: Transport) -> Self {
        self.config.transport = transport;
        self
    }

    /// Enable or disable Magic Cookies for TCP (default: false)
    ///
    /// Magic Cookies allow resynchronization in testing/debugging scenarios.
    /// See feat_req_recentip_586, feat_req_recentip_591, feat_req_recentip_592.
    pub fn magic_cookies(mut self, enabled: bool) -> Self {
        self.config.magic_cookies = enabled;
        self
    }

    /// Build the configuration
    pub fn build(self) -> RuntimeConfig {
        self.config
    }
}

// ============================================================================
// METHOD CONFIGURATION
// ============================================================================

/// Configuration for how a service handles error responses.
///
/// Per SOME/IP specification (feat_req_recentip_106, feat_req_recentip_726):
/// - By default, errors use RESPONSE (0x80) with non-OK return code
/// - EXCEPTION (0x81) is optional and must be explicitly configured per-method
///
/// This configuration is typically defined in the interface specification (IDL/FIDL)
/// at design time, not decided per-call at runtime.
#[derive(Debug, Clone, Default)]
pub struct MethodConfig {
    /// Set of method IDs that use EXCEPTION (0x81) message type for errors.
    /// Methods not in this set use RESPONSE (0x80) with error return code.
    exception_methods: HashSet<u16>,
}

impl MethodConfig {
    /// Create a new empty configuration (all methods use RESPONSE for errors)
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure a method to use EXCEPTION (0x81) message type for errors.
    ///
    /// Per spec, this should match the interface specification for the method.
    pub fn use_exception_for(mut self, method_id: u16) -> Self {
        self.exception_methods.insert(method_id);
        self
    }

    /// Check if a method uses EXCEPTION message type for errors.
    pub fn uses_exception(&self, method_id: u16) -> bool {
        self.exception_methods.contains(&method_id)
    }
}
