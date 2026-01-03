//! Service Discovery message handling.
//!
//! Contains SD message parsing, building, and action generation.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use bytes::Bytes;
use tokio::time::Instant;

use crate::command::ServiceAvailability;
use crate::config::Transport;
use crate::state::{
    DiscoveredService, OfferedService, PendingServerResponse, RuntimeState,
    ServiceKey, SubscriberKey,
};
use crate::wire::{L4Protocol, SdEntry, SdMessage, SdOption};

// ============================================================================
// ACTION TYPE
// ============================================================================

/// Action to execute after handling an event
pub(crate) enum Action {
    /// Send an SD message to a specific target
    SendSd {
        message: SdMessage,
        target: SocketAddr,
    },
    /// Notify find requests about a discovered service
    NotifyFound {
        key: ServiceKey,
        availability: ServiceAvailability,
    },
    /// Send a SOME/IP RPC message as a client (uses client RPC socket)
    SendClientMessage { data: Bytes, target: SocketAddr },
    /// Send a SOME/IP RPC message as a server (uses service's RPC socket)
    SendServerMessage {
        service_key: ServiceKey,
        data: Bytes,
        target: SocketAddr,
    },
    /// Track a pending server response
    TrackServerResponse {
        context: PendingServerResponse,
        receiver: tokio::sync::oneshot::Receiver<crate::error::Result<Bytes>>,
    },
    /// Reset TCP connections to a peer (due to reboot detection)
    ResetPeerTcpConnections { peer: std::net::IpAddr },
}

// ============================================================================
// SD MESSAGE HANDLERS
// ============================================================================

/// Handle an OfferService entry
pub(crate) fn handle_offer(
    entry: &SdEntry,
    sd_message: &SdMessage,
    from: SocketAddr,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    // Get UDP endpoint from SD option, falling back to source address
    // If the endpoint has unspecified IP (0.0.0.0), use the source IP with the option's port
    let udp_endpoint = match sd_message.get_udp_endpoint(entry) {
        Some(ep) if !ep.ip().is_unspecified() => Some(ep),
        Some(ep) => Some(SocketAddr::new(from.ip(), ep.port())),
        None => None,
    };

    // Get TCP endpoint from SD option if present
    let tcp_endpoint = match sd_message.get_tcp_endpoint(entry) {
        Some(ep) if !ep.ip().is_unspecified() => Some(ep),
        Some(ep) => Some(SocketAddr::new(from.ip(), ep.port())),
        None => None,
    };

    // Use UDP endpoint as fallback if neither is present
    let effective_endpoint = udp_endpoint.or(tcp_endpoint).unwrap_or(from);

    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
    };

    let ttl_duration = Duration::from_secs(entry.ttl as u64);

    tracing::debug!(
        "Discovered service {:04x}:{:04x} at {:?}/{:?} (TTL={})",
        entry.service_id,
        entry.instance_id,
        udp_endpoint,
        tcp_endpoint,
        entry.ttl
    );

    let is_new = !state.discovered.contains_key(&key);
    state.discovered.insert(
        key,
        DiscoveredService {
            udp_endpoint,
            tcp_endpoint,
            sd_endpoint: from, // Store the SD source for Subscribe messages
            major_version: entry.major_version,
            minor_version: entry.minor_version,
            ttl_expires: Instant::now() + ttl_duration,
        },
    );

    if is_new {
        actions.push(Action::NotifyFound {
            key,
            availability: ServiceAvailability::Available {
                endpoint: effective_endpoint,
                instance_id: entry.instance_id,
            },
        });
    }
}

/// Handle a StopOfferService entry
pub(crate) fn handle_stop_offer(
    entry: &SdEntry,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
    };

    if state.discovered.remove(&key).is_some() {
        tracing::debug!(
            "Service {:04x}:{:04x} stopped offering",
            entry.service_id,
            entry.instance_id
        );

        actions.push(Action::NotifyFound {
            key,
            availability: ServiceAvailability::Unavailable,
        });
    }
}

/// Handle a FindService request (we may need to respond with an offer)
pub(crate) fn handle_find_request(
    entry: &SdEntry,
    from: SocketAddr,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    // Determine protocol based on transport configuration
    let protocol = match state.config.transport {
        Transport::Tcp => L4Protocol::Tcp,
        Transport::Udp => L4Protocol::Udp,
    };

    for (key, offered) in &state.offered {
        // Only respond if the service is actively announcing
        if !offered.is_announcing {
            continue;
        }

        if entry.service_id == key.service_id
            && (entry.instance_id == 0xFFFF || entry.instance_id == key.instance_id)
        {
            let mut response = SdMessage::new(state.sd_flags(true));
            let opt_idx = response.add_option(SdOption::Ipv4Endpoint {
                addr: match offered.rpc_endpoint {
                    SocketAddr::V4(v4) => *v4.ip(),
                    _ => Ipv4Addr::LOCALHOST,
                },
                port: offered.rpc_endpoint.port(), // Use RPC endpoint port, not SD port!
                protocol,
            });
            response.add_entry(SdEntry::offer_service(
                key.service_id,
                key.instance_id,
                offered.major_version,
                offered.minor_version,
                state.config.ttl,
                opt_idx,
                1,
            ));

            actions.push(Action::SendSd {
                message: response,
                target: from,
            });
        }
    }
}

/// Handle a SubscribeEventgroup request
pub(crate) fn handle_subscribe_request(
    entry: &SdEntry,
    sd_message: &SdMessage,
    from: SocketAddr,
    state: &mut RuntimeState,
    actions: &mut Vec<Action>,
) {
    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
    };

    // Get client's event endpoint from SD option, falling back to source address
    // If the endpoint has unspecified IP (0.0.0.0), use the source IP with the option's port
    let client_endpoint = match sd_message.get_udp_endpoint(entry) {
        Some(ep) if !ep.ip().is_unspecified() => ep,
        Some(ep) => SocketAddr::new(from.ip(), ep.port()),
        None => from,
    };

    if let Some(offered) = state.offered.get(&key) {
        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        let _ = offered
            .requests_tx
            .try_send(crate::command::ServiceRequest::Subscribe {
                eventgroup_id: entry.eventgroup_id,
                client: client_endpoint,
                response: response_tx,
            });

        // Track the subscriber - use the endpoint from the option for event delivery
        let sub_key = SubscriberKey {
            service_id: entry.service_id,
            instance_id: entry.instance_id,
            eventgroup_id: entry.eventgroup_id,
        };
        state
            .server_subscribers
            .entry(sub_key)
            .or_insert_with(Vec::new)
            .push(client_endpoint);

        let mut ack = SdMessage::new(state.sd_flags(true));
        ack.add_entry(SdEntry::subscribe_eventgroup_ack(
            entry.service_id,
            entry.instance_id,
            entry.major_version,
            entry.eventgroup_id,
            state.config.ttl,
            entry.counter,
        ));

        // Send ACK to the SD source (not the event endpoint)
        actions.push(Action::SendSd {
            message: ack,
            target: from,
        });
    }
}

/// Handle a StopSubscribeEventgroup request
pub(crate) fn handle_unsubscribe_request(
    entry: &SdEntry,
    sd_message: &SdMessage,
    from: SocketAddr,
    state: &mut RuntimeState,
) {
    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
    };

    // Get client's event endpoint from SD option (same as subscribe)
    let client_endpoint = match sd_message.get_udp_endpoint(entry) {
        Some(ep) if !ep.ip().is_unspecified() => ep,
        Some(ep) => SocketAddr::new(from.ip(), ep.port()),
        None => from,
    };

    if let Some(offered) = state.offered.get(&key) {
        let _ = offered
            .requests_tx
            .try_send(crate::command::ServiceRequest::Unsubscribe {
                eventgroup_id: entry.eventgroup_id,
                client: client_endpoint,
            });

        // Remove the subscriber
        let sub_key = SubscriberKey {
            service_id: entry.service_id,
            instance_id: entry.instance_id,
            eventgroup_id: entry.eventgroup_id,
        };
        if let Some(subscribers) = state.server_subscribers.get_mut(&sub_key) {
            subscribers.retain(|addr| *addr != client_endpoint);
        }
    }
}

/// Handle a SubscribeEventgroupAck
pub(crate) fn handle_subscribe_ack(entry: &SdEntry, _state: &mut RuntimeState) {
    tracing::debug!(
        "Subscription acknowledged for {:04x}:{:04x} eventgroup {:04x}",
        entry.service_id,
        entry.instance_id,
        entry.eventgroup_id
    );
}

/// Handle a SubscribeEventgroupNack
pub(crate) fn handle_subscribe_nack(entry: &SdEntry, state: &mut RuntimeState) {
    let key = ServiceKey {
        service_id: entry.service_id,
        instance_id: entry.instance_id,
    };

    tracing::debug!(
        "Subscription rejected for {:04x}:{:04x} eventgroup {:04x}",
        entry.service_id,
        entry.instance_id,
        entry.eventgroup_id
    );

    state.subscriptions.remove(&key);
}

// ============================================================================
// SD MESSAGE BUILDING
// ============================================================================

/// Build an OfferService SD message for the given offered service
pub(crate) fn build_offer_message(
    key: &ServiceKey,
    offered: &OfferedService,
    sd_flags: u8,
    ttl: u32,
    transport: Transport,
) -> SdMessage {
    let protocol = match transport {
        Transport::Tcp => L4Protocol::Tcp,
        Transport::Udp => L4Protocol::Udp,
    };

    let mut msg = SdMessage::new(sd_flags);
    let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
        addr: match offered.rpc_endpoint {
            SocketAddr::V4(v4) => *v4.ip(),
            _ => Ipv4Addr::LOCALHOST,
        },
        port: offered.rpc_endpoint.port(),
        protocol,
    });
    msg.add_entry(SdEntry::offer_service(
        key.service_id,
        key.instance_id,
        offered.major_version,
        offered.minor_version,
        ttl,
        opt_idx,
        1,
    ));
    msg
}

/// Build a StopOfferService SD message
pub(crate) fn build_stop_offer_message(
    key: &ServiceKey,
    offered: &OfferedService,
    sd_flags: u8,
) -> SdMessage {
    let mut msg = SdMessage::new(sd_flags);
    msg.add_entry(SdEntry::stop_offer_service(
        key.service_id,
        key.instance_id,
        offered.major_version,
        offered.minor_version,
    ));
    msg
}

/// Build a FindService SD message
pub(crate) fn build_find_message(
    service_id: u16,
    instance_id: u16,
    sd_flags: u8,
    ttl: u32,
) -> SdMessage {
    let mut msg = SdMessage::new(sd_flags);
    msg.add_entry(SdEntry::find_service(
        service_id,
        instance_id,
        0xFF,
        0xFFFFFFFF,
        ttl,
    ));
    msg
}

/// Build a SubscribeEventgroup SD message
pub(crate) fn build_subscribe_message(
    service_id: u16,
    instance_id: u16,
    eventgroup_id: u16,
    local_endpoint: SocketAddr,
    client_rpc_port: u16,
    sd_flags: u8,
    ttl: u32,
    transport: Transport,
) -> SdMessage {
    let protocol = match transport {
        Transport::Tcp => L4Protocol::Tcp,
        Transport::Udp => L4Protocol::Udp,
    };

    let mut msg = SdMessage::new(sd_flags);
    let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
        addr: match local_endpoint {
            SocketAddr::V4(v4) => *v4.ip(),
            _ => Ipv4Addr::LOCALHOST,
        },
        port: client_rpc_port,
        protocol,
    });
    let mut entry = SdEntry::subscribe_eventgroup(
        service_id,
        instance_id,
        0xFF,
        eventgroup_id,
        ttl,
        0,
    );
    entry.index_1st_option = opt_idx;
    entry.num_options_1 = 1;
    msg.add_entry(entry);
    msg
}

/// Build an Unsubscribe (StopSubscribeEventgroup) SD message
pub(crate) fn build_unsubscribe_message(
    service_id: u16,
    instance_id: u16,
    eventgroup_id: u16,
    local_endpoint: SocketAddr,
    client_rpc_port: u16,
    sd_flags: u8,
    transport: Transport,
) -> SdMessage {
    let protocol = match transport {
        Transport::Tcp => L4Protocol::Tcp,
        Transport::Udp => L4Protocol::Udp,
    };

    let mut msg = SdMessage::new(sd_flags);
    // Include the same endpoint option as in Subscribe so server knows which subscriber to remove
    let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
        addr: match local_endpoint {
            SocketAddr::V4(v4) => *v4.ip(),
            _ => Ipv4Addr::LOCALHOST,
        },
        port: client_rpc_port,
        protocol,
    });
    let mut entry = SdEntry::subscribe_eventgroup(
        service_id,
        instance_id,
        0xFF,
        eventgroup_id,
        0, // TTL=0 indicates unsubscribe
        0,
    );
    entry.index_1st_option = opt_idx;
    entry.num_options_1 = 1;
    msg.add_entry(entry);
    msg
}
