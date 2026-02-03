//! Concurrent Subscribe command handling
//!
//! This module implements concurrent Subscribe command processing to avoid blocking
//! the event loop during TCP connection establishment.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

use crate::config::Transport;
use crate::error::{ConfigError, Error};
use crate::net::TcpStream;
use crate::runtime::event_loop::SubscribeStateUpdate;
use crate::runtime::sd::build_subscribe_message_multi;
use crate::runtime::state::{
    ClientSubscription, MultiEventgroupSubscription, MultiEventgroupSubscriptionKey,
    PendingSubscription, PendingSubscriptionKey, RuntimeState, ServiceKey,
};
use crate::tcp::TcpConnectionPool;
use crate::{Event, InstanceId, ServiceId};

/// Handle Subscribe command concurrently without blocking the event loop.
///
/// This function runs in a spawned task and performs TCP connection establishment
/// (which may take up to 2 seconds on timeout). Once the connection is established,
/// it sends a state update back to the event loop with a closure that applies all
/// the necessary state mutations.
#[allow(clippy::too_many_arguments)]
pub async fn handle_subscribe_command_concurrent<T: TcpStream>(
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    eventgroup_ids: Vec<u16>,
    events: mpsc::Sender<Event>,
    response: oneshot::Sender<crate::error::Result<u64>>,
    tcp_pool: Arc<TcpConnectionPool<T>>,
    update_tx: mpsc::Sender<SubscribeStateUpdate>,
    discovered_opt: Option<(
        SocketAddr,
        Option<SocketAddr>,
        Option<(SocketAddr, Transport)>,
    )>,
    subscription_id: u64,
    client_rpc_endpoint: SocketAddr,
    advertised_ip: Option<std::net::IpAddr>,
    sd_flags: u8,
    subscribe_ttl: u32,
    _preferred_transport: Transport,
    _service_already_has_subscription: bool,
    _reusable_endpoint_port: Option<u16>,
    used_conn_keys: HashSet<u64>,
) {
    // Early validation
    if eventgroup_ids.is_empty() {
        let _ = update_tx
            .send(SubscribeStateUpdate::Failed {
                response,
                error: Error::Config(ConfigError::new(
                    "At least one eventgroup must be specified",
                )),
            })
            .await;
        return;
    }

    let key = ServiceKey::new(service_id, instance_id, major_version);

    // Check if service is discovered
    let Some((sd_endpoint, tcp_endpoint_opt, method_endpoint_opt)) = discovered_opt else {
        tracing::debug!(
            "Cannot subscribe to {:04x}:{:04x} v{} eventgroups {:?}: service not discovered",
            service_id.value(),
            instance_id.value(),
            major_version,
            eventgroup_ids
        );
        let _ = update_tx
            .send(SubscribeStateUpdate::Failed {
                response,
                error: Error::ServiceUnavailable,
            })
            .await;
        return;
    };

    let Some((_method_endpoint, transport)) = method_endpoint_opt else {
        tracing::debug!(
            "Trying to subscribe to eventgroups {:?} on service {:04x} instance {:04x}, but server offers no compatible transport endpoints",
            eventgroup_ids,
            service_id.value(),
            instance_id.value()
        );
        let _ = update_tx
            .send(SubscribeStateUpdate::Failed {
                response,
                error: Error::Config(ConfigError::new(
                    "Server offers no compatible transport endpoints",
                )),
            })
            .await;
        return;
    };

    // For TCP subscriptions, establish connection BEFORE subscribing (feat_req_someipsd_767)
    let (endpoint_for_subscribe, tcp_conn_key) = if transport == Transport::Tcp {
        let Some(tcp_endpoint) = tcp_endpoint_opt else {
            tracing::error!(
                "TCP transport selected but no TCP endpoint for {:04x}:{:04x}",
                service_id.value(),
                instance_id.value()
            );
            let _ = update_tx
                .send(SubscribeStateUpdate::Failed {
                    response,
                    error: Error::Config(ConfigError::new(
                        "TCP transport selected but no TCP endpoint",
                    )),
                })
                .await;
            return;
        };

        // Find the smallest unused conn_key (slot) for this service
        let conn_key = {
            let mut slot = 0u64;
            while used_conn_keys.contains(&slot) {
                slot += 1;
            }
            slot
        };

        // Establish TCP connection (this is the potentially slow operation)
        // Now truly concurrent - no mutex held across multiple connections
        match tcp_pool.ensure_connected(tcp_endpoint, conn_key).await {
            Ok(local_addr) => {
                tracing::debug!(
                    "TCP connection established to {} (local addr: {}, conn_key: {}) for subscription to {:04x}:{:04x} eventgroups {:?}",
                    tcp_endpoint,
                    local_addr,
                    conn_key,
                    service_id.value(),
                    instance_id.value(),
                    eventgroup_ids
                );
                (local_addr, conn_key)
            }
            Err(e) => {
                tracing::error!(
                    "Failed to establish TCP connection to {} for subscription: {}",
                    tcp_endpoint,
                    e
                );
                let _ = update_tx
                    .send(SubscribeStateUpdate::Failed {
                        response,
                        error: Error::Io(e),
                    })
                    .await;
                return;
            }
        }
    } else {
        // UDP subscription - TODO: implement UDP endpoint management
        // For now, use simple shared endpoint approach
        let Some(endpoint_ip) = advertised_ip else {
            tracing::error!(
                "Cannot subscribe to {:04x}:{:04x} eventgroups {:?}: \
                    no valid IP address configured. Set RuntimeConfig::advertised_ip",
                service_id.value(),
                instance_id.value(),
                eventgroup_ids
            );
            let _ = update_tx
                .send(SubscribeStateUpdate::Failed {
                    response,
                    error: Error::Config(ConfigError::new(
                        "No advertised IP configured for subscriptions",
                    )),
                })
                .await;
            return;
        };

        let port = client_rpc_endpoint.port();
        (SocketAddr::new(endpoint_ip, port), 0)
    };

    // Build the Subscribe SD message
    let msg = build_subscribe_message_multi(
        service_id.value(),
        instance_id.value(),
        major_version,
        &eventgroup_ids,
        endpoint_for_subscribe,
        endpoint_for_subscribe.port(),
        sd_flags,
        subscribe_ttl,
        transport,
    );

    // Send state update with closure to apply changes
    let update = SubscribeStateUpdate::Success {
        apply_state: Box::new(move |state: &mut RuntimeState| {
            // Register subscription endpoint
            if transport == Transport::Tcp {
                state.register_subscription_endpoint(
                    endpoint_for_subscribe.port(),
                    service_id.value(),
                    instance_id.value(),
                );
            }

            // Track all eventgroups with a shared events channel
            let subs = state.subscriptions.entry(key).or_default();
            for &eventgroup_id in &eventgroup_ids {
                subs.push(ClientSubscription {
                    subscription_id,
                    eventgroup_id,
                    events_tx: events.clone(),
                    local_endpoint: endpoint_for_subscribe,
                    has_dedicated_socket: false,
                    tcp_conn_key,
                });
            }

            // Track pending subscriptions
            let is_multi_eventgroup = eventgroup_ids.len() > 1;
            let mut response_opt = Some(response);

            if is_multi_eventgroup {
                let multi_key = MultiEventgroupSubscriptionKey {
                    service_id: service_id.value(),
                    instance_id: instance_id.value(),
                    major_version,
                    subscription_id,
                };
                state.multi_eventgroup_subscriptions.insert(
                    multi_key,
                    MultiEventgroupSubscription {
                        eventgroup_ids: eventgroup_ids.clone(),
                        acked_eventgroups: HashSet::new(),
                        response: response_opt.take(),
                    },
                );
            }

            // Track pending subscriptions for each eventgroup
            let mut eventgroups_to_subscribe = Vec::new();
            for &eventgroup_id in &eventgroup_ids {
                let pending_key = PendingSubscriptionKey {
                    service_id: service_id.value(),
                    instance_id: instance_id.value(),
                    major_version,
                    eventgroup_id,
                };
                let pending_list = state.pending_subscriptions.entry(pending_key).or_default();
                let is_first_waiter = pending_list.is_empty();

                pending_list.push(PendingSubscription {
                    subscription_id,
                    response: response_opt.take(),
                });

                if is_first_waiter {
                    eventgroups_to_subscribe.push(eventgroup_id);
                }
            }

            // Queue the Subscribe SD message
            if !eventgroups_to_subscribe.is_empty() {
                tracing::debug!(
                    "Subscribing to {:04x}:{:04x} v{} eventgroups {:?} via {} (endpoint: {}, subscription_id: {})",
                    service_id.value(),
                    instance_id.value(),
                    major_version,
                    eventgroups_to_subscribe,
                    if transport == Transport::Tcp { "TCP" } else { "UDP" },
                    endpoint_for_subscribe,
                    subscription_id
                );
                state.queue_unicast_sd(msg, sd_endpoint);
            }
        }),
    };

    let _ = update_tx.send(update).await;
}
