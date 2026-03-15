//! Event subscription types.
//!
//! - [`SubscriptionBuilder`]: Builder for subscribing to eventgroups
//! - [`Subscription`]: Active subscription receiving events

use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};

use crate::config::{Transport, TransportPolicy};
use crate::error::{Error, Result};
use crate::handles::runtime::RuntimeInner;
use crate::runtime::Command;
use crate::{Event, EventgroupId, InstanceId, OfferedEndpoints, ServiceId};

/// Builder for subscribing to eventgroups.
///
/// Created via [`OfferedService::subscribe`](crate::handles::OfferedService::subscribe).
///
/// # Example
///
/// ```no_run
/// use recentip::prelude::*;
///
/// # async fn example(proxy: recentip::handles::OfferedService) -> Result<()> {
/// // Single eventgroup
/// let mut sub = proxy.subscribe(EventgroupId::new(1).unwrap()).await?;
///
/// // Multiple eventgroups
/// let mut sub = proxy
///     .subscribe(EventgroupId::new(1).unwrap())
///     .and(EventgroupId::new(2).unwrap())
///     .await?;
///
/// while let Some(event) = sub.next().await {
///     println!("Event 0x{:04X}", event.event_id.value());
/// }
/// # Ok(())
/// # }
/// ```
#[must_use]
pub struct SubscriptionBuilder {
    inner: Arc<RuntimeInner>,
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    eventgroups: vec1::Vec1<EventgroupId>,
    /// Full policy: each entry is tried in order.  On a port conflict the
    /// next entry is attempted, enabling exact preference-ordered fallback,
    /// e.g. `[udp(A), tcp(B), udp(C)]`:
    ///   sub1 → udp/A;  sub2 → udp/A conflict → tcp/B;  sub3 → tcp/B conflict → udp/C.
    transport_policy: TransportPolicy,
    /// Server's offered endpoints, used to match each preference entry.
    remote_endpoints: OfferedEndpoints,
    sd_endpoint: std::net::SocketAddrV4,
}

impl SubscriptionBuilder {
    /// Create a new subscription builder with the first eventgroup.
    pub(crate) fn new(
        inner: Arc<RuntimeInner>,
        service_id: ServiceId,
        instance_id: InstanceId,
        major_version: u8,
        first_eventgroup: EventgroupId,
        transport_policy: TransportPolicy,
        remote_endpoints: OfferedEndpoints,
        sd_endpoint: std::net::SocketAddrV4,
    ) -> Self {
        Self {
            inner,
            service_id,
            instance_id,
            major_version,
            eventgroups: vec1::vec1![first_eventgroup],
            transport_policy,
            remote_endpoints,
            sd_endpoint,
        }
    }

    /// Add another eventgroup to this subscription.
    ///
    /// All eventgroups share the same network endpoint.
    pub fn and(mut self, eventgroup: EventgroupId) -> Self {
        self.eventgroups.push(eventgroup);
        self
    }

    /// Complete the subscription and wait for acknowledgment.
    ///
    /// Sends `SubscribeEventgroup` messages for all added eventgroups
    /// and waits for acknowledgments from the server.
    ///
    /// If subcribtions to all eventgroups are acknowledged successfully,
    /// returns a [`Subscription`] handle that can be used to receive events
    /// from any of the subscribed eventgroups.
    ///
    /// Fails if at least one subscription could not be created.
    ///
    /// You can also just `.await` the builder directly.
    ///
    /// # Errors
    ///
    /// - [`Error::TransportMismatch`] if the transport policy has no match for the service.
    /// - [`Error::SubscriptionRejected`] if the server sends a NACK.
    /// - [`Error::RuntimeShutdown`] if the runtime has been dropped.
    pub async fn subscribe(self) -> Result<Subscription> {
        let Self {
            inner,
            service_id,
            instance_id,
            major_version,
            eventgroups,
            transport_policy,
            remote_endpoints,
            sd_endpoint,
        } = self;

        // Pre-compute the eventgroup ID list (doesn't change across retries).
        let eventgroup_ids: vec1::Vec1<u16> = eventgroups.clone().mapped(|id| id.value());
        let eventgroups_for_subscription = eventgroups.into_vec();

        // True when an IO error indicates a port or 4-tuple conflict.
        let is_port_conflict = |e: &Error| match e {
            Error::Io(ie) => matches!(
                ie.kind(),
                std::io::ErrorKind::AddrInUse | std::io::ErrorKind::AddrNotAvailable
            ),
            _ => false,
        };

        // Iterate policy preferences in order, one entry at a time.
        //
        // Trying each entry individually (rather than grouping all same-transport
        // entries together) gives exact preference-ordered backtracking:
        //   policy [udp(A), tcp(B), udp(C)]:
        //     sub1 → udp/A;  sub2 → conflict on udp/A → tcp/B;  sub3 → tcp/B conflict → udp/C
        let mut last_conflict_err: Option<Error> = None;

        for pref in transport_policy.preferences() {
            // Skip entries whose transport the server doesn't offer.
            let remote_ep = match (pref.transport, &remote_endpoints) {
                (
                    Transport::Tcp,
                    OfferedEndpoints::TcpOnly(a) | OfferedEndpoints::Both { tcp: a, .. },
                ) => *a,
                (
                    Transport::Udp,
                    OfferedEndpoints::UdpOnly(a) | OfferedEndpoints::Both { udp: a, .. },
                ) => *a,
                _ => continue,
            };

            let (events_tx, events_rx) = mpsc::channel(64);
            let (response_tx, response_rx) = oneshot::channel();

            inner
                .cmd_tx
                .send(Command::Subscribe {
                    service_id,
                    instance_id,
                    major_version,
                    eventgroup_ids: eventgroup_ids.clone(),
                    events: events_tx,
                    response: response_tx,
                    transport: pref.transport,
                    remote_endpoint: remote_ep,
                    sd_endpoint,
                    local_port: pref.local_port,
                })
                .await
                .map_err(|_| Error::RuntimeShutdown)?;

            match response_rx.await.map_err(|_| Error::RuntimeShutdown)? {
                Ok(subscription_id) => {
                    return Ok(Subscription::new(
                        inner,
                        service_id,
                        instance_id,
                        major_version,
                        eventgroups_for_subscription,
                        subscription_id,
                        events_rx,
                    ));
                }
                Err(e) if is_port_conflict(&e) => {
                    last_conflict_err = Some(e);
                    // continue to next preference entry
                }
                Err(e) => return Err(e),
            }
        }

        // No preference entry matched or all conflicted.
        Err(last_conflict_err.unwrap_or(Error::TransportMismatch))
    }
}

impl IntoFuture for SubscriptionBuilder {
    type Output = Result<Subscription>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.subscribe())
    }
}

/// Active subscription receiving events from one or more eventgroups.
///
/// Created via [`SubscriptionBuilder::subscribe`]. Call [`next`](Self::next)
/// to receive events.
///
/// # Lifecycle
///
/// When dropped, sends `StopSubscribeEventgroup` (TTL=0) for all eventgroups.
/// If the runtime is already shut down, cleanup relies on server-side TTL expiry.
pub struct Subscription {
    inner: Arc<RuntimeInner>,
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    eventgroups: Vec<EventgroupId>,
    id: u64,
    events: mpsc::Receiver<Event>,
}

impl Subscription {
    /// Create a new subscription (internal use only)
    pub(crate) const fn new(
        inner: Arc<RuntimeInner>,
        service_id: ServiceId,
        instance_id: InstanceId,
        major_version: u8,
        eventgroups: Vec<EventgroupId>,
        subscription_id: u64,
        events: mpsc::Receiver<Event>,
    ) -> Self {
        Self {
            inner,
            service_id,
            instance_id,
            major_version,
            eventgroups,
            id: subscription_id,
            events,
        }
    }

    /// Receive the next event from any subscribed eventgroup.
    ///
    /// Returns `None` if the subscription has ended.
    pub async fn next(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    /// Get the list of eventgroup IDs in this subscription.
    pub fn eventgroups(&self) -> &[EventgroupId] {
        &self.eventgroups
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        // Unsubscribe from all eventgroups (best-effort).
        //
        // Note: We use try_send because Drop cannot be async. If the command channel
        // is full (very unlikely - 64 item capacity), the unsubscribe will fail.
        // In that case:
        // - We log a warning for observability
        // - The server will eventually clean up the subscription via TTL expiry
        // - The specification's TTL mechanism serves as a backstop for exactly this case
        for eventgroup in &self.eventgroups {
            let cmd = Command::Unsubscribe {
                service_id: self.service_id,
                instance_id: self.instance_id,
                major_version: self.major_version,
                eventgroup_id: eventgroup.value(),
                subscription_id: self.id,
            };
            if let Err(e) = self.inner.cmd_tx.try_send(cmd) {
                match e {
                    TrySendError::Full(_) => {
                        tracing::warn!(
                            "Failed to send unsubscribe for eventgroup {} (service {:04x}:{:04x}): \
                             command channel full. Server will clean up via TTL.",
                            eventgroup.value(),
                            self.service_id.value(),
                            self.instance_id.value()
                        );
                    }
                    TrySendError::Closed(_) => {
                        // Runtime already shut down - this is expected during shutdown
                        tracing::debug!(
                            "Unsubscribe skipped: runtime already shut down (service {:04x}:{:04x})",
                            self.service_id.value(),
                            self.instance_id.value()
                        );
                    }
                }
            }
        }
    }
}
