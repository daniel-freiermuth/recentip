//! Concurrent TCP Subscribe command handling
//!
//! This module implements concurrent TCP Subscribe command processing to avoid blocking
//! the event loop during TCP connection establishment.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

use crate::config::Transport;
use crate::error::Error;
use crate::net::TcpStream;
use crate::runtime::event_loop::SubscribeStateUpdate;
use crate::runtime::sd::build_subscribe_message_multi;
use crate::runtime::state::{
    ClientSubscription, MultiEventgroupSubscription, MultiEventgroupSubscriptionKey,
    PendingSubscription, PendingSubscriptionKey, RuntimeState, ServiceKey,
};
use crate::tcp::TcpConnectionPool;
use crate::{Event, InstanceId, ServiceId};

/// Handle Subscribe command for TCP transport concurrently.
///
/// This function runs in a spawned task and performs TCP connection establishment
/// (which may take up to 2 seconds on timeout). Once the connection is established,
/// it sends a state update back to the event loop with a closure that applies all
/// the necessary state mutations.
///
/// Per `feat_req_someipsd_767`:
/// > "The client shall open a TCP connection to the server and should be ready
/// > to receive message on that connection before sending the SubscribeEventgroup entry"
#[allow(clippy::too_many_arguments)]
pub async fn handle_subscribe_tcp<T: TcpStream>(
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    eventgroup_ids: vec1::Vec1<u16>,
    events: mpsc::Sender<Event>,
    response: oneshot::Sender<crate::error::Result<u64>>,
    tcp_pool: Arc<TcpConnectionPool<T>>,
    update_tx: mpsc::Sender<SubscribeStateUpdate>,
    sd_endpoint: SocketAddr,
    tcp_endpoint: SocketAddr,
    subscription_id: u64,
    sd_flags: u8,
    subscribe_ttl: u32,
    used_conn_keys: HashSet<u64>,
) {
    let key = ServiceKey::new(service_id, instance_id, major_version);

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
    let endpoint_for_subscribe = match tcp_pool.ensure_connected(tcp_endpoint, conn_key).await {
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
            local_addr
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
        Transport::Tcp,
    );

    // Send state update with closure to apply changes
    let update = SubscribeStateUpdate::Success {
        apply_state: Box::new(move |state: &mut RuntimeState| {
            // Register subscription endpoint
            state.register_subscription_endpoint(
                endpoint_for_subscribe.port(),
                service_id.value(),
                instance_id.value(),
            );

            // Track all eventgroups with a shared events channel
            let subs = state.subscriptions.entry(key).or_default();
            for &eventgroup_id in &eventgroup_ids {
                subs.push(ClientSubscription {
                    subscription_id,
                    eventgroup_id,
                    events_tx: events.clone(),
                    local_endpoint: endpoint_for_subscribe,
                    has_dedicated_socket: false,
                    tcp_conn_key: conn_key,
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
                        eventgroup_ids: eventgroup_ids.to_vec(),
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
                    "Subscribing to {:04x}:{:04x} v{} eventgroups {:?} via TCP (endpoint: {}, subscription_id: {})",
                    service_id.value(),
                    instance_id.value(),
                    major_version,
                    eventgroups_to_subscribe,
                    endpoint_for_subscribe,
                    subscription_id
                );
                state.queue_unicast_sd(msg, sd_endpoint);
            }
        }),
    };

    let _ = update_tx.send(update).await;
}
