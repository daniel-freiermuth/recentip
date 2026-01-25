//! # `SomeIp` State (Internal)
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
//! | [`ServiceKey`] | Identifies a service+instance+version tuple |
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

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;

use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use super::command::ServiceRequest;
use super::sd::Action;
use crate::config::{MethodConfig, RuntimeConfig};
use crate::error::Result;
use crate::net::TcpStream;
use crate::runtime::event_loop::cluster_sd_actions;
use crate::tcp::{TcpConnectionPool, TcpSendMessage};
use crate::wire::SdMessage;
use crate::{InstanceId, ServiceId};

// ============================================================================
// SERVICE KEYS
// ============================================================================

/// Key for service identification
///
/// Per SOME/IP-SD spec, a service is uniquely identified by the tuple
/// `(service_id, instance_id, major_version)`. Different major versions
/// of the same service are considered different services.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServiceKey {
    pub(crate) service_id: u16,
    pub(crate) instance_id: u16,
    pub(crate) major_version: u8,
}

impl ServiceKey {
    pub(crate) fn new(service_id: ServiceId, instance_id: InstanceId, major_version: u8) -> Self {
        Self {
            service_id: service_id.value(),
            instance_id: instance_id.value(),
            major_version,
        }
    }

    /// Check if this key matches another key (supports wildcards on both sides)
    /// - `instance_id` 0xFFFF matches any instance
    /// - `major_version` 0xFF matches any version
    pub(crate) fn matches(self, other: Self) -> bool {
        self.service_id == other.service_id
            && (self.instance_id == 0xFFFF
                || self.instance_id == other.instance_id
                || other.instance_id == 0xFFFF)
            && (self.major_version == 0xFF
                || self.major_version == other.major_version
                || other.major_version == 0xFF)
    }
}

/// Key for tracking server-side subscribers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriberKey {
    pub(crate) service_id: u16,
    pub(crate) instance_id: u16,
    pub(crate) major_version: u8,
    pub(crate) eventgroup_id: u16,
}

/// Server-side subscription with TTL expiration tracking
///
/// Per SOME/IP-SD spec (`feat_req_recentipsd_445`), subscriptions have a TTL
/// and must be cleaned up when the TTL expires without renewal.
#[derive(Debug, Clone)]
pub struct ServerSubscription {
    /// Client's event endpoint (where to send events)
    pub(crate) endpoint: SocketAddr,
    /// Transport protocol for sending events to this subscriber
    pub(crate) transport: crate::config::Transport,
    /// When this subscription expires (based on client's TTL).
    /// `None` means infinite TTL (0xFFFFFF) - never expires per `feat_req_recentipsd_431`.
    pub(crate) expires_at: Option<tokio::time::Instant>,
}

/// Key for pending calls
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallKey {
    pub(crate) client_id: u16,
    pub(crate) session_id: u16,
}

// ============================================================================
// PEER SESSION TRACKING (Reboot Detection)
// ============================================================================

/// Tracks session state for a remote peer on a specific channel (multicast or unicast).
///
/// Per SOME/IP-SD spec (`feat_req_someipsd_765`), each peer has separate session counters
/// for multicast and unicast SD messages. Reboot detection must track these independently.
#[derive(Debug, Clone, Copy, Default)]
pub struct PeerChannelSession {
    /// Last observed session ID from this peer on this channel
    pub(crate) last_session_id: Option<u16>,
    /// Last observed reboot flag from this peer on this channel
    pub(crate) last_reboot_flag: bool,
}

impl PeerChannelSession {
    /// Check if the new message indicates a peer reboot.
    ///
    /// Per `feat_req_someipsd_764`, reboot is detected when:
    /// 1. Reboot flag transitions from 0 to 1 (explicit reboot), OR
    /// 2. Reboot flag is 1 AND session ID regresses (`session_id` < `last_session_id`)
    ///
    /// Returns true if reboot detected.
    pub(crate) fn check_and_update(&mut self, session_id: u16, reboot_flag: bool) -> bool {
        tracing::debug!(
            "check_and_update: incoming session_id={}, reboot_flag={}, stored last_session_id={:?}, last_reboot_flag={}",
            session_id,
            reboot_flag,
            self.last_session_id,
            self.last_reboot_flag
        );

        let reboot_detected = match (self.last_session_id, self.last_reboot_flag) {
            // First message from this peer on this channel - establish baseline
            (None, _) => {
                tracing::debug!("  => First contact - establishing baseline, no reboot");
                false
            }

            // Case 1: Reboot flag transition 0 → 1
            (Some(_), false) if reboot_flag => {
                tracing::warn!(
                    "  => REBOOT DETECTED: Case 1 - reboot flag transitioned false → true"
                );
                true
            }

            // Case 2: Reboot flag stays 1, but session ID regressed
            (Some(last_session), true) if reboot_flag && session_id <= last_session => {
                tracing::warn!(
                    "  => REBOOT DETECTED: Case 2 - session regression {} → {} with reboot flag still set",
                    last_session,
                    session_id
                );
                true
            }

            // Normal case: flag 1 → 0 is wraparound completion, not reboot
            // Normal case: session incrementing normally
            _ => {
                tracing::debug!("  => Normal case - no reboot");
                false
            }
        };

        // Update stored state
        self.last_session_id = Some(session_id);
        self.last_reboot_flag = reboot_flag;

        tracing::debug!(
            "  => Updated stored state: last_session_id={:?}, last_reboot_flag={}",
            self.last_session_id,
            self.last_reboot_flag
        );

        reboot_detected
    }
}

/// Tracks session state for both multicast and unicast channels from a peer.
///
/// Per `feat_req_someipsd_765`:
/// - Multicast channel: `FindService`, `OfferService`
/// - Unicast channel: `SubscribeEventgroup`, `SubscribeEventgroupAck`, `StopSubscribeEventgroup`
#[derive(Debug, Clone, Default)]
pub struct PeerSessionState {
    /// Session tracking for multicast SD messages
    pub(crate) multicast: PeerChannelSession,
    /// Session tracking for unicast SD messages
    pub(crate) unicast: PeerChannelSession,
}

/// Identifies whether an SD message is on the multicast or unicast channel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdChannel {
    Multicast,
    Unicast,
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
    /// UDP endpoint for this service instance (if offering via UDP)
    pub(crate) udp_endpoint: Option<SocketAddr>,
    /// UDP transport sender (if offering via UDP)
    pub(crate) udp_transport: Option<RpcTransportSender>,
    /// TCP endpoint for this service instance (if offering via TCP)
    pub(crate) tcp_endpoint: Option<SocketAddr>,
    /// TCP transport sender (if offering via TCP)
    pub(crate) tcp_transport: Option<RpcTransportSender>,
    /// Configuration for which methods use EXCEPTION message type
    pub(crate) method_config: MethodConfig,
    /// Whether currently announcing via SD (false = bound but not announced)
    pub(crate) is_announcing: bool,
}

// ============================================================================
// DISCOVERED SERVICES (CLIENT-SIDE)
// ============================================================================

/// Discovered remote service
///
/// Note: `major_version` is part of `DiscoveredServiceKey`, not stored here.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields used for version matching in future
pub struct DiscoveredService {
    /// UDP endpoint for sending SOME/IP RPC messages (if using UDP transport)
    pub(crate) udp_endpoint: Option<SocketAddr>,
    /// TCP endpoint for sending SOME/IP RPC messages (if using TCP transport)
    pub(crate) tcp_endpoint: Option<SocketAddr>,
    /// SD endpoint for sending SD messages (`SubscribeEventgroup`, etc.)
    pub(crate) sd_endpoint: SocketAddr,
    pub(crate) minor_version: u32,
    pub(crate) ttl_expires: Instant,
}

impl DiscoveredService {
    /// Get the RPC endpoint and transport based on preferred transport
    pub(crate) fn method_endpoint(
        &self,
        prefer_tcp: bool,
    ) -> Option<(SocketAddr, crate::config::Transport)> {
        if prefer_tcp {
            self.tcp_endpoint
                .map(|ep| (ep, crate::config::Transport::Tcp))
                .or_else(|| {
                    self.udp_endpoint
                        .map(|ep| (ep, crate::config::Transport::Udp))
                })
        } else {
            self.udp_endpoint
                .map(|ep| (ep, crate::config::Transport::Udp))
                .or_else(|| {
                    self.tcp_endpoint
                        .map(|ep| (ep, crate::config::Transport::Tcp))
                })
        }
    }
}

// ============================================================================
// CLIENT-SIDE SUBSCRIPTIONS
// ============================================================================

/// Tracked find request
pub struct FindRequest {
    pub(crate) notify: mpsc::Sender<super::command::ServiceAvailability>,
    pub(crate) repetitions_left: u32,
    pub(crate) last_find: Instant,
}

/// Active subscription (client-side)
pub struct ClientSubscription {
    pub(crate) subscription_id: u64,
    pub(crate) eventgroup_id: u16,
    pub(crate) events_tx: mpsc::Sender<crate::Event>,
    /// Local endpoint for this subscription (events are received on this port)
    pub(crate) local_endpoint: SocketAddr,
    /// True if this subscription has a dedicated socket task (UDP subscriptions only)
    /// TCP subscriptions receive events via the shared TCP connection handler
    pub(crate) has_dedicated_socket: bool,
    /// The connection key used for TCP subscriptions (0 = shared connection, >0 = dedicated)
    /// Used for routing: events from `conn_key=0` only go to subscriptions with `conn_key=0`
    pub(crate) tcp_conn_key: u64,
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
    pub(crate) major_version: u8,
    pub(crate) eventgroup_id: u16,
}

/// Pending subscription waiting for ACK/NACK from server
pub struct PendingSubscription {
    pub(crate) subscription_id: u64,
    pub(crate) response: Option<oneshot::Sender<crate::error::Result<u64>>>,
}

/// Key for tracking multi-eventgroup subscriptions
/// A multi-eventgroup subscription uses the same `subscription_id` for all eventgroups
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MultiEventgroupSubscriptionKey {
    pub(crate) service_id: u16,
    pub(crate) instance_id: u16,
    pub(crate) major_version: u8,
    pub(crate) subscription_id: u64,
}

/// State for a multi-eventgroup subscription waiting for all ACKs
pub struct MultiEventgroupSubscription {
    /// All eventgroup IDs in this subscription
    pub(crate) eventgroup_ids: Vec<u16>,
    /// Eventgroups that have been `ACKed` (set)
    pub(crate) acked_eventgroups: HashSet<u16>,
    /// Response channel to send result when all `ACKed` or any `NACKed`
    pub(crate) response: Option<oneshot::Sender<crate::error::Result<u64>>>,
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

/// `SomeIp` state managed by the runtime task
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
    /// Registered event IDs per service (for uniqueness validation)
    pub(crate) registered_events: HashMap<ServiceKey, HashSet<u16>>,
    /// Services we're looking for
    pub(crate) find_requests: HashMap<ServiceKey, FindRequest>,
    /// Discovered remote services
    pub(crate) discovered: HashMap<ServiceKey, DiscoveredService>,
    /// Active subscriptions (client-side)
    pub(crate) subscriptions: HashMap<ServiceKey, Vec<ClientSubscription>>,
    /// Server-side subscribers (clients subscribed to our offered services)
    /// Each subscription includes TTL expiration for cleanup per `feat_req_recentipsd_445`
    pub(crate) server_subscribers: HashMap<SubscriberKey, Vec<ServerSubscription>>,
    /// Static event listeners (client-side, no SD)
    pub(crate) static_listeners: HashMap<SubscriberKey, mpsc::Sender<crate::Event>>,
    /// Pending RPC calls waiting for responses
    pub(crate) pending_calls: HashMap<CallKey, PendingCall>,
    /// Pending subscriptions waiting for ACK/NACK (single eventgroup subscriptions)
    pub(crate) pending_subscriptions: HashMap<PendingSubscriptionKey, Vec<PendingSubscription>>,
    /// Multi-eventgroup subscriptions waiting for ALL ACKs (or any NACK)
    /// "All or nothing" semantics: subscription only succeeds if ALL eventgroups ACK
    pub(crate) multi_eventgroup_subscriptions:
        HashMap<MultiEventgroupSubscriptionKey, MultiEventgroupSubscription>,
    /// Client ID for outgoing requests
    pub(crate) client_id: u16,
    /// SD session ID counter for multicast messages (per `feat_req_someipsd_41`)
    multicast_session_id: u16,
    /// SD session ID counter for unicast messages (per `feat_req_someipsd_41`)
    unicast_session_id: u16,
    /// Whether the multicast session ID has wrapped around at least once
    multicast_has_wrapped_once: bool,
    /// Whether the unicast session ID has wrapped around at least once
    unicast_has_wrapped_once: bool,
    /// Pending initial offers waiting to be clustered and sent (time-based batching)
    pub(crate) pending_initial_offers: Vec<ServiceKey>,
    /// Deadline for flushing pending initial offers (None if no pending offers)
    pub(crate) pending_offers_deadline: Option<Instant>,
    /// Last time the periodic cycle ran (for coordination with pending offer flush)
    pub(crate) last_periodic_cycle: Option<Instant>,
    /// Pending outward unicast SD actions waiting to be clustered and sent (time-based batching)
    /// This includes: initial subscribes, offer-triggered subscribes, find-triggered offers, subscribe ACKs/NACKs
    pending_unicast_sd: Vec<Action>,
    /// Deadline for flushing pending unicast SD actions (None if no pending actions)
    pending_unicast_sd_deadline: Option<Instant>,
    /// Configuration
    pub(crate) config: RuntimeConfig,
    /// Session tracking per peer for reboot detection (`feat_req_someipsd_764`, `feat_req_someipsd_765`)
    /// Tracks both multicast and unicast session counters independently per peer
    pub(crate) peer_sessions: HashMap<std::net::IpAddr, PeerSessionState>,
    /// SD event monitors - channels to send all SD events to
    pub(crate) sd_monitors: Vec<mpsc::Sender<crate::SdEvent>>,
    /// Next subscription ID for client-side subscriptions (unique per handle)
    next_subscription_id: u64,
    /// Tracks which (`service_id`, `instance_id`) pairs are using each subscription endpoint (port).
    /// This enables endpoint reuse across DIFFERENT services while maintaining isolation
    /// within the SAME service (events can be routed by `service_id` in header, but not by eventgroup).
    /// Key: port number, Value: set of (`service_id`, `instance_id`) using that port
    pub(crate) subscription_endpoint_usage: HashMap<u16, HashSet<(u16, u16)>>,
    payload_session_id: u16,
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
            registered_events: HashMap::new(),
            find_requests: HashMap::new(),
            discovered: HashMap::new(),
            subscriptions: HashMap::new(),
            server_subscribers: HashMap::new(),
            static_listeners: HashMap::new(),
            pending_calls: HashMap::new(),
            pending_subscriptions: HashMap::new(),
            multi_eventgroup_subscriptions: HashMap::new(),
            client_id,
            multicast_session_id: 1,
            unicast_session_id: 1,
            multicast_has_wrapped_once: false,
            unicast_has_wrapped_once: false,
            payload_session_id: 1,
            pending_initial_offers: Vec::new(),
            pending_offers_deadline: None,
            last_periodic_cycle: None,
            pending_unicast_sd: Vec::new(),
            pending_unicast_sd_deadline: None,
            config,
            peer_sessions: HashMap::new(),
            sd_monitors: Vec::new(),
            next_subscription_id: 1,
            subscription_endpoint_usage: HashMap::new(),
        }
    }

    /// Queue an SD message for time-based clustering before sending.
    ///
    /// This batches outward unicast SD traffic (subscribes, ACKs, NACKs, find-triggered offers)
    /// to prevent session ID collisions when multiple messages need to be sent close together.
    pub(crate) fn queue_unicast_sd(&mut self, message: SdMessage, target: SocketAddr) {
        self.pending_unicast_sd
            .push(Action::SendSd { message, target });

        // Set deadline if not already set (50ms from now)
        if self.pending_unicast_sd_deadline.is_none() {
            // Experimentally determined interval to ensure sequential packet delivery
            // and prevent reordering in our test environment.
            // good: 100ms
            // bad: 93ms
            let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(100);
            self.pending_unicast_sd_deadline = Some(deadline);
            tracing::debug!("Set unicast SD flush deadline to {:?}", deadline);
        }
    }

    /// Flush pending unicast SD actions (time-based clustering)
    /// Clusters all pending SD actions by target and sends them together
    /// This handles: initial subscribes, offer-triggered subscribes, find-triggered offers, subscribe ACKs/NACKs
    pub(crate) fn flush_pending_unicast_sd<T: TcpStream>(
        &mut self,
        _tcp_pool: &TcpConnectionPool<T>,
    ) -> Vec<Action> {
        self.pending_unicast_sd_deadline = None;

        tracing::debug!(
            "Flushing {} pending unicast SD actions (clustering by target)",
            self.pending_unicast_sd.len()
        );

        // Cluster actions by target using existing cluster_sd_actions logic
        let actions = std::mem::take(&mut self.pending_unicast_sd);
        cluster_sd_actions(actions)
    }

    pub(crate) async fn await_pending_unicast_sd_flush_deadline(&self) {
        if let Some(deadline) = self.pending_unicast_sd_deadline {
            tokio::time::sleep_until(deadline).await;
        } else {
            std::future::pending::<()>().await;
        }
    }

    /// Allocate a unique subscription ID for client-side subscriptions
    pub(crate) fn next_subscription_id(&mut self) -> u64 {
        let id = self.next_subscription_id;
        // The wrapping is a problem in it's own
        self.next_subscription_id = self.next_subscription_id.wrapping_add(1);
        id
    }

    /// Get next session ID for multicast SD messages
    pub(crate) fn next_multicast_session_id(&mut self) -> u16 {
        let id = self.multicast_session_id;
        self.multicast_session_id = self.multicast_session_id.wrapping_add(1);
        if self.multicast_session_id == 0 {
            self.multicast_session_id = 1;
            self.multicast_has_wrapped_once = true;
        }
        id
    }

    /// Get next session ID for unicast SD messages
    pub(crate) fn next_unicast_session_id(&mut self) -> u16 {
        let id = self.unicast_session_id;
        self.unicast_session_id = self.unicast_session_id.wrapping_add(1);
        if self.unicast_session_id == 0 {
            self.unicast_session_id = 1;
            self.unicast_has_wrapped_once = true;
        }
        id
    }

    pub(crate) fn next_payload_session_id(&mut self) -> u16 {
        let id = self.payload_session_id;
        self.payload_session_id = self.payload_session_id.wrapping_add(1);
        if self.payload_session_id == 0 {
            self.payload_session_id = 1;
        }
        id
    }

    /// Get the SD flags byte with reboot flag based on current state
    pub(crate) fn sd_flags(&self, unicast: bool) -> u8 {
        let mut flags = 0u8;
        if unicast {
            flags |= SdMessage::FLAG_UNICAST;
            if !self.unicast_has_wrapped_once {
                flags |= SdMessage::FLAG_REBOOT;
            }
        } else if !self.multicast_has_wrapped_once {
            flags |= SdMessage::FLAG_REBOOT;
        }
        flags
    }

    /// Find an existing subscription endpoint that can be reused for a new subscription.
    ///
    /// Returns Some(port) if there's an endpoint that doesn't already have a subscription
    /// from the same (`service_id`, `instance_id`). This enables endpoint sharing across
    /// different services while maintaining isolation within the same service.
    ///
    /// This includes the `client_rpc_endpoint` if this service isn't currently using it,
    /// allowing port reuse after unsubscribe/resubscribe cycles.
    ///
    /// Returns None if no suitable endpoint exists (need to create a new one).
    pub(crate) fn find_reusable_subscription_endpoint(
        &self,
        service_id: u16,
        instance_id: u16,
    ) -> Option<u16> {
        // First, check if the client_rpc_endpoint is available for this service
        // (i.e., this service has no subscriptions currently using it)
        let rpc_port = self.client_rpc_endpoint.port();
        let service_uses_rpc_port = self
            .subscription_endpoint_usage
            .get(&rpc_port)
            .is_some_and(|services| services.contains(&(service_id, instance_id)));

        if !service_uses_rpc_port {
            // The shared RPC endpoint is available for this service - prefer it
            return Some(rpc_port);
        }

        // Otherwise, look for other dedicated endpoints not used by this service
        for (&port, services) in &self.subscription_endpoint_usage {
            if port == rpc_port {
                continue; // Already checked above
            }
            // Can reuse if this service isn't already using this port
            if !services.contains(&(service_id, instance_id)) {
                return Some(port);
            }
        }
        None
    }

    /// Register that a (`service_id`, `instance_id`) is using a subscription endpoint (port).
    pub(crate) fn register_subscription_endpoint(
        &mut self,
        port: u16,
        service_id: u16,
        instance_id: u16,
    ) {
        self.subscription_endpoint_usage
            .entry(port)
            .or_default()
            .insert((service_id, instance_id));
    }

    /// Unregister a (`service_id`, `instance_id`) from a subscription endpoint.
    /// Called when a subscription is dropped.
    pub(crate) fn unregister_subscription_endpoint(
        &mut self,
        port: u16,
        service_id: u16,
        instance_id: u16,
    ) {
        if let Some(services) = self.subscription_endpoint_usage.get_mut(&port) {
            services.remove(&(service_id, instance_id));
            // Clean up empty entries to avoid memory leak
            if services.is_empty() {
                self.subscription_endpoint_usage.remove(&port);
            }
        }
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
    #[test_log::test]
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
        assert_eq!(
            state.next_multicast_session_id(),
            1,
            "Session ID should start at 1"
        );

        // Iterate through all possible session IDs
        for expected in 2..=0xFFFFu16 {
            let id = state.next_multicast_session_id();
            assert_eq!(id, expected, "Session ID should be {}", expected);
            assert_ne!(id, 0, "Session ID should never be 0");
        }

        // After 0xFFFF, should wrap to 1 (not 0)
        let wrapped = state.next_multicast_session_id();
        assert_eq!(wrapped, 1, "Session ID should wrap to 1 after 0xFFFF");

        // Continue a few more to verify
        assert_eq!(state.next_multicast_session_id(), 2);
        assert_eq!(state.next_multicast_session_id(), 3);
    }

    /// feat_req_recentipsd_41: Reboot flag is cleared after first wraparound
    #[test_log::test]
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
        assert!(
            !state.multicast_has_wrapped_once,
            "multicast_has_wrapped_once should be false initially"
        );

        // Iterate through all session IDs until wraparound
        for _ in 1..=0xFFFFu16 {
            state.next_multicast_session_id();
        }

        // After wraparound, reboot flag should be cleared
        assert!(
            state.multicast_has_wrapped_once,
            "multicast_has_wrapped_once should be true after wraparound"
        );

        // Second wraparound should not change anything
        for _ in 1..=0xFFFFu16 {
            state.next_multicast_session_id();
        }
        assert!(
            state.multicast_has_wrapped_once,
            "multicast_has_wrapped_once should be true after second wraparound"
        );
    }

    /// Session ID never returns 0
    #[test_log::test]
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
            let id = state.next_unicast_session_id();
            assert_ne!(id, 0, "Session ID must never be 0");
        }
    }
}
