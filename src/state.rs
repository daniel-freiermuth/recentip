//! # Runtime State (Internal)
//!
//! This module contains the central state structure and supporting types
//! used by the runtime's event loop. It is `pub(crate)` — internal to the
//! library.
//!
//! ## Design Philosophy
//!
//! All mutable state lives in [`RuntimeState`]. The event loop owns this
//! structure exclusively, avoiding the need for locks or atomic operations.
//! Handler functions in other modules (client, server, sd) take `&mut RuntimeState`
//! and return [`Action`](crate::sd::Action) values for the event loop to execute.
//!
//! ## Key Data Structures
//!
//! | Type | Purpose |
//! |------|---------|
//! | [`RuntimeState`] | Top-level state container |
//! | [`ServiceKey`] | Identifies a service+instance pair |
//! | [`OfferedService`] | State for a service we're offering |
//! | [`DiscoveredService`] | State for a service we've discovered |
//! | [`PendingCall`] | Tracks an outstanding RPC call |
//! | [`ClientSubscription`] | Client-side subscription to an eventgroup |
//! | [`ServerSubscriber`] | Server's view of a subscribed client |
//!
//! ## Session ID Management
//!
//! Per SOME/IP spec:
//! - Session IDs are 16-bit, wrapping from 0xFFFF → 0x0001 (never 0x0000)
//! - Separate counters for multicast SD and unicast SD
//! - "Reboot flag" set on first message after startup
//!
//! The `next_session_id()` and `next_unicast_session_id()` methods handle this.
//!
//! ## Contributor Notes
//!
//! When adding new features:
//! 1. Add state fields to [`RuntimeState`]
//! 2. Add key types if needed (like [`ServiceKey`], [`CallKey`])
//! 3. Update `new()` to initialize new fields
//! 4. Add accessor methods if handlers in other modules need them

use std::collections::HashMap;
use std::net::SocketAddr;

use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::command::ServiceRequest;
use crate::config::{MethodConfig, RuntimeConfig};
use crate::error::Result;
use crate::tcp::TcpSendMessage;
use crate::wire::SdMessage;
use crate::{InstanceId, ServiceId};

// ============================================================================
// SERVICE KEYS
// ============================================================================

/// Key for service identification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServiceKey {
    pub(crate) service_id: u16,
    pub(crate) instance_id: u16,
}

impl ServiceKey {
    pub(crate) fn new(service_id: ServiceId, instance_id: InstanceId) -> Self {
        Self {
            service_id: service_id.value(),
            instance_id: instance_id.value(),
        }
    }

    pub(crate) fn matches(&self, service_id: u16, instance_id: u16) -> bool {
        self.service_id == service_id
            && (self.instance_id == 0xFFFF
                || self.instance_id == instance_id
                || instance_id == 0xFFFF)
    }
}

/// Key for tracking server-side subscribers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriberKey {
    pub(crate) service_id: u16,
    pub(crate) instance_id: u16,
    pub(crate) eventgroup_id: u16,
}

/// Server-side subscription with TTL expiration tracking
///
/// Per SOME/IP-SD spec (feat_req_recentipsd_445), subscriptions have a TTL
/// and must be cleaned up when the TTL expires without renewal.
#[derive(Debug, Clone)]
pub struct ServerSubscription {
    /// Client's event endpoint (where to send events)
    pub(crate) endpoint: SocketAddr,
    /// When this subscription expires (based on client's TTL).
    /// `None` means infinite TTL (0xFFFFFF) - never expires per feat_req_recentipsd_431.
    pub(crate) expires_at: Option<tokio::time::Instant>,
}

/// Key for pending calls
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallKey {
    pub(crate) client_id: u16,
    pub(crate) session_id: u16,
}

// ============================================================================
// RPC MESSAGE TYPES
// ============================================================================

/// Message received from an RPC socket task
#[derive(Debug)]
pub struct RpcMessage {
    pub(crate) service_key: Option<ServiceKey>,
    pub(crate) data: Vec<u8>,
    pub(crate) from: SocketAddr,
}

/// Message to send via an RPC socket task  
#[derive(Debug)]
pub struct RpcSendMessage {
    pub(crate) data: Vec<u8>,
    pub(crate) to: SocketAddr,
}

/// Transport sender for offered services - either UDP or TCP
#[derive(Debug, Clone)]
pub enum RpcTransportSender {
    /// UDP socket sender
    Udp(mpsc::Sender<RpcSendMessage>),
    /// TCP server sender
    Tcp(mpsc::Sender<TcpSendMessage>),
}

impl RpcTransportSender {
    /// Send a message to the specified target
    pub(crate) async fn send(&self, data: Vec<u8>, to: SocketAddr) -> Result<()> {
        use crate::error::Error;
        match self {
            Self::Udp(tx) => tx
                .send(RpcSendMessage { data, to })
                .await
                .map_err(|_| Error::RuntimeShutdown),
            Self::Tcp(tx) => tx
                .send(TcpSendMessage { data, to })
                .await
                .map_err(|_| Error::RuntimeShutdown),
        }
    }
}

// ============================================================================
// OFFERED SERVICES (SERVER-SIDE)
// ============================================================================

/// Tracked offered service (our offerings)
pub struct OfferedService {
    pub(crate) major_version: u8,
    pub(crate) minor_version: u32,
    pub(crate) requests_tx: mpsc::Sender<ServiceRequest>,
    pub(crate) last_offer: Instant,
    /// Dedicated RPC endpoint for this service instance (separate from SD socket)
    pub(crate) rpc_endpoint: SocketAddr,
    /// Channel to send outgoing RPC messages to this service's socket/TCP task
    pub(crate) rpc_transport: RpcTransportSender,
    /// Configuration for which methods use EXCEPTION message type
    pub(crate) method_config: MethodConfig,
    /// Whether currently announcing via SD (false = bound but not announced)
    pub(crate) is_announcing: bool,
}

// ============================================================================
// DISCOVERED SERVICES (CLIENT-SIDE)
// ============================================================================

/// Discovered remote service
#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields used for version matching in future
pub struct DiscoveredService {
    /// UDP endpoint for sending SOME/IP RPC messages (if using UDP transport)
    pub(crate) udp_endpoint: Option<SocketAddr>,
    /// TCP endpoint for sending SOME/IP RPC messages (if using TCP transport)
    pub(crate) tcp_endpoint: Option<SocketAddr>,
    /// SD endpoint for sending SD messages (`SubscribeEventgroup`, etc.)
    pub(crate) sd_endpoint: SocketAddr,
    pub(crate) major_version: u8,
    pub(crate) minor_version: u32,
    pub(crate) ttl_expires: Instant,
}

impl DiscoveredService {
    /// Get the RPC endpoint based on preferred transport
    pub(crate) fn rpc_endpoint(&self, prefer_tcp: bool) -> Option<SocketAddr> {
        if prefer_tcp {
            self.tcp_endpoint.or(self.udp_endpoint)
        } else {
            self.udp_endpoint.or(self.tcp_endpoint)
        }
    }
}

// ============================================================================
// CLIENT-SIDE SUBSCRIPTIONS
// ============================================================================

/// Tracked find request
pub struct FindRequest {
    pub(crate) notify: mpsc::Sender<crate::command::ServiceAvailability>,
    pub(crate) repetitions_left: u32,
    pub(crate) last_find: Instant,
}

/// Active subscription (client-side)
pub struct ClientSubscription {
    pub(crate) eventgroup_id: u16,
    pub(crate) events_tx: mpsc::Sender<crate::Event>,
}

/// Pending RPC call (client-side)
pub struct PendingCall {
    pub(crate) response: oneshot::Sender<Result<crate::Response>>,
}

/// Key for pending subscriptions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PendingSubscriptionKey {
    pub(crate) service_id: u16,
    pub(crate) instance_id: u16,
    pub(crate) eventgroup_id: u16,
}

/// Pending subscription waiting for ACK/NACK from server
pub struct PendingSubscription {
    pub(crate) response: oneshot::Sender<crate::error::Result<()>>,
}

// ============================================================================
// SERVER-SIDE RESPONSE TRACKING
// ============================================================================

/// Pending server response (server-side) - holds context to send response back
#[derive(Debug, Clone)]
pub struct PendingServerResponse {
    pub(crate) service_id: u16,
    #[allow(dead_code)]
    pub(crate) instance_id: u16,
    pub(crate) method_id: u16,
    pub(crate) client_id: u16,
    pub(crate) session_id: u16,
    pub(crate) interface_version: u8,
    pub(crate) client_addr: SocketAddr,
    /// Whether this method uses EXCEPTION (0x81) for errors instead of RESPONSE (0x80)
    pub(crate) uses_exception: bool,
    /// Transport to use for sending the response - captured when request received
    /// This allows responses to be sent even after the service offering is dropped
    pub(crate) rpc_transport: RpcTransportSender,
}

// ============================================================================
// RUNTIME STATE
// ============================================================================

/// Runtime state managed by the runtime task
pub struct RuntimeState {
    /// SD endpoint (port 30490) - only for Service Discovery
    pub(crate) local_endpoint: SocketAddr,
    /// Client RPC endpoint (ephemeral port) - for sending client RPC requests
    /// Per `feat_req_recentip_676`: Port 30490 is only for SD, not for RPC
    pub(crate) client_rpc_endpoint: SocketAddr,
    /// Sender for client RPC messages (sends to `client_rpc_socket` task)
    pub(crate) client_rpc_tx: mpsc::Sender<RpcSendMessage>,
    /// Services we're offering
    pub(crate) offered: HashMap<ServiceKey, OfferedService>,
    /// Services we're looking for
    pub(crate) find_requests: HashMap<ServiceKey, FindRequest>,
    /// Discovered remote services
    pub(crate) discovered: HashMap<ServiceKey, DiscoveredService>,
    /// Active subscriptions (client-side)
    pub(crate) subscriptions: HashMap<ServiceKey, Vec<ClientSubscription>>,
    /// Server-side subscribers (clients subscribed to our offered services)
    /// Each subscription includes TTL expiration for cleanup per feat_req_recentipsd_445
    pub(crate) server_subscribers: HashMap<SubscriberKey, Vec<ServerSubscription>>,
    /// Static event listeners (client-side, no SD)
    pub(crate) static_listeners: HashMap<SubscriberKey, mpsc::Sender<crate::Event>>,
    /// Pending RPC calls waiting for responses
    pub(crate) pending_calls: HashMap<CallKey, PendingCall>,
    /// Pending subscriptions waiting for ACK/NACK
    pub(crate) pending_subscriptions: HashMap<PendingSubscriptionKey, PendingSubscription>,
    /// Client ID for outgoing requests
    pub(crate) client_id: u16,
    /// SD session ID counter
    session_id: u16,
    /// Reboot flag - set to true after startup, cleared after first session wraparound
    reboot_flag: bool,
    /// Whether the session ID has wrapped around at least once
    has_wrapped_once: bool,
    /// Configuration
    pub(crate) config: RuntimeConfig,
    /// Last known reboot flag state for each peer (by IP) for reboot detection
    pub(crate) peer_reboot_flags: HashMap<std::net::IpAddr, bool>,
    /// SD event monitors - channels to send all SD events to
    pub(crate) sd_monitors: Vec<mpsc::Sender<crate::SdEvent>>,
}

impl RuntimeState {
    pub(crate) fn new(
        local_endpoint: SocketAddr,
        client_rpc_endpoint: SocketAddr,
        client_rpc_tx: mpsc::Sender<RpcSendMessage>,
        config: RuntimeConfig,
    ) -> Self {
        // Use port as part of client_id to help with uniqueness
        let client_id = (local_endpoint.port() % 0xFFFE) + 1;
        Self {
            local_endpoint,
            client_rpc_endpoint,
            client_rpc_tx,
            offered: HashMap::new(),
            find_requests: HashMap::new(),
            discovered: HashMap::new(),
            subscriptions: HashMap::new(),
            server_subscribers: HashMap::new(),
            static_listeners: HashMap::new(),
            pending_calls: HashMap::new(),
            pending_subscriptions: HashMap::new(),
            client_id,
            session_id: 1,
            reboot_flag: true,
            has_wrapped_once: false,
            config,
            peer_reboot_flags: HashMap::new(),
            sd_monitors: Vec::new(),
        }
    }

    pub(crate) fn next_session_id(&mut self) -> u16 {
        let id = self.session_id;
        self.session_id = self.session_id.wrapping_add(1);
        if self.session_id == 0 {
            self.session_id = 1;
            // After first wraparound, clear the reboot flag
            if !self.has_wrapped_once {
                self.has_wrapped_once = true;
                self.reboot_flag = false;
            }
        }
        id
    }

    /// Get the SD flags byte with reboot flag based on current state
    pub(crate) fn sd_flags(&self, unicast: bool) -> u8 {
        let mut flags = 0u8;
        if self.reboot_flag {
            flags |= SdMessage::FLAG_REBOOT;
        }
        if unicast {
            flags |= SdMessage::FLAG_UNICAST;
        }
        flags
    }
}

// ============================================================================
// UNIT TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// feat_req_recentip_677: Session ID wraps from 0xFFFF to 0x0001
    /// feat_req_recentip_649: Session ID starts at 0x0001
    ///
    /// Session ID 0x0000 is reserved for "session handling disabled" and must never be used.
    #[test]
    fn session_id_wraps_to_0001_not_0000() {
        let addr = "127.0.0.1:30490".parse().unwrap();
        let client_rpc_addr = "127.0.0.1:49152".parse().unwrap();
        let (client_rpc_tx, _) = mpsc::channel(1);
        let mut state = RuntimeState::new(
            addr,
            client_rpc_addr,
            client_rpc_tx,
            RuntimeConfig::default(),
        );

        // First session should be 1
        assert_eq!(state.next_session_id(), 1, "Session ID should start at 1");

        // Iterate through all possible session IDs
        for expected in 2..=0xFFFFu16 {
            let id = state.next_session_id();
            assert_eq!(id, expected, "Session ID should be {}", expected);
            assert_ne!(id, 0, "Session ID should never be 0");
        }

        // After 0xFFFF, should wrap to 1 (not 0)
        let wrapped = state.next_session_id();
        assert_eq!(wrapped, 1, "Session ID should wrap to 1 after 0xFFFF");

        // Continue a few more to verify
        assert_eq!(state.next_session_id(), 2);
        assert_eq!(state.next_session_id(), 3);
    }

    /// feat_req_recentipsd_41: Reboot flag is cleared after first wraparound
    #[test]
    fn reboot_flag_clears_after_wraparound() {
        let addr = "127.0.0.1:30490".parse().unwrap();
        let client_rpc_addr = "127.0.0.1:49152".parse().unwrap();
        let (client_rpc_tx, _) = mpsc::channel(1);
        let mut state = RuntimeState::new(
            addr,
            client_rpc_addr,
            client_rpc_tx,
            RuntimeConfig::default(),
        );

        // Initially reboot flag should be set
        assert!(state.reboot_flag, "Reboot flag should be true initially");
        assert!(
            !state.has_wrapped_once,
            "has_wrapped_once should be false initially"
        );

        // Iterate through all session IDs until wraparound
        for _ in 1..=0xFFFFu16 {
            state.next_session_id();
        }

        // After wraparound, reboot flag should be cleared
        assert!(
            !state.reboot_flag,
            "Reboot flag should be false after wraparound"
        );
        assert!(
            state.has_wrapped_once,
            "has_wrapped_once should be true after wraparound"
        );

        // Second wraparound should not change anything
        for _ in 1..=0xFFFFu16 {
            state.next_session_id();
        }
        assert!(
            !state.reboot_flag,
            "Reboot flag should stay false after second wraparound"
        );
    }

    /// Session ID never returns 0
    #[test]
    fn session_id_never_zero() {
        let addr = "127.0.0.1:30490".parse().unwrap();
        let client_rpc_addr = "127.0.0.1:49152".parse().unwrap();
        let (client_rpc_tx, _) = mpsc::channel(1);
        let mut state = RuntimeState::new(
            addr,
            client_rpc_addr,
            client_rpc_tx,
            RuntimeConfig::default(),
        );

        // Iterate through 2 full cycles + some extra
        for _ in 0..(0xFFFF * 2 + 1000) {
            let id = state.next_session_id();
            assert_ne!(id, 0, "Session ID must never be 0");
        }
    }
}
