//! Event subscription types.
//!
//! - [`SubscriptionBuilder`]: Builder for subscribing to eventgroups
//! - [`Subscription`]: Active subscription receiving events

use std::sync::Arc;

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};

use crate::error::{Error, Result};
use crate::handles::runtime::RuntimeInner;
use crate::runtime::Command;
use crate::{Event, EventgroupId, InstanceId, ServiceId};

/// Builder for subscribing to eventgroups.
///
/// Created via [`OfferedService::new_subscription`](crate::handles::OfferedService::new_subscription).
///
/// # Example
///
/// ```no_run
/// use recentip::prelude::*;
///
/// # async fn example(proxy: recentip::handles::OfferedService) -> Result<()> {
/// let mut subscription = proxy
///     .new_subscription()
///     .eventgroup(EventgroupId::new(1).unwrap())
///     .eventgroup(EventgroupId::new(2).unwrap())  // multiple eventgroups OK
///     .subscribe()
///     .await?;
///
/// while let Some(event) = subscription.next().await {
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
    eventgroups: Vec<EventgroupId>,
}

impl SubscriptionBuilder {
    /// Create a new subscription builder (internal use only)
    pub(crate) fn new(
        inner: Arc<RuntimeInner>,
        service_id: ServiceId,
        instance_id: InstanceId,
        major_version: u8,
    ) -> Self {
        Self {
            inner,
            service_id,
            instance_id,
            major_version,
            eventgroups: Vec::new(),
        }
    }

    /// Add an eventgroup to this subscription.
    ///
    /// Can be called multiple times to subscribe to multiple eventgroups.
    /// All eventgroups will share the same network endpoint.
    pub fn eventgroup(mut self, eventgroup: EventgroupId) -> Self {
        self.eventgroups.push(eventgroup);
        self
    }

    /// Complete the subscription and wait for acknowledgment.
    ///
    /// Sends `SubscribeEventgroup` messages for all added eventgroups
    /// and waits for acknowledgments from the server.
    ///
    /// Returns a [`Subscription`] handle that can be used to receive events
    /// from any of the subscribed eventgroups.
    pub async fn subscribe(self) -> Result<Subscription> {
        if self.eventgroups.is_empty() {
            return Err(Error::Config(crate::error::ConfigError::new(
                "At least one eventgroup must be specified",
            )));
        }

        let (events_tx, events_rx) = mpsc::channel(64);
        let (response_tx, response_rx) = oneshot::channel();

        // Send subscribe command for all eventgroups
        self.inner
            .cmd_tx
            .send(Command::Subscribe {
                service_id: self.service_id,
                instance_id: self.instance_id,
                major_version: self.major_version,
                eventgroup_ids: self
                    .eventgroups
                    .iter()
                    .map(crate::EventgroupId::value)
                    .collect(),
                events: events_tx,
                response: response_tx,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        let subscription_id = response_rx.await.map_err(|_| Error::RuntimeShutdown)??;

        Ok(Subscription::new(
            self.inner,
            self.service_id,
            self.instance_id,
            self.major_version,
            self.eventgroups,
            subscription_id,
            events_rx,
        ))
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
    pub(crate) fn new(
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
