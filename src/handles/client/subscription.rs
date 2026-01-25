//! Subscription handles for receiving events from SOME/IP services

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use crate::error::{Error, Result};
use crate::handles::runtime::RuntimeInner;
use crate::runtime::Command;
use crate::{Event, EventgroupId, InstanceId, ServiceId};

/// Builder for creating a subscription to one or more eventgroups.
///
/// Created by calling [`OfferedService::new_subscription()`](crate::handles::OfferedService::new_subscription).
///
/// All eventgroups added to this builder will share the same network endpoint,
/// ensuring proper event deduplication per the SOME/IP specification.
///
/// # Example
///
/// ```no_run
/// use recentip::prelude::*;
///
/// # async fn example(proxy: recentip::handles::OfferedService) -> Result<()> {
/// let eg1 = EventgroupId::new(0x0001).unwrap();
/// let eg2 = EventgroupId::new(0x0002).unwrap();
///
/// let mut subscription = proxy
///     .new_subscription()
///     .eventgroup(eg1)
///     .eventgroup(eg2)
///     .subscribe()
///     .await?;
///
/// // Receive events from both eventgroups
/// while let Some(event) = subscription.next().await {
///     println!("Event: {:?}", event);
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

/// Active subscription to one or more eventgroups.
///
/// Events from any subscribed eventgroup are received via the `next()` method.
/// The subscription is automatically stopped (`StopSubscribeEventgroup` sent for
/// all eventgroups) when dropped.
///
/// All eventgroups in a subscription share the same network endpoint, ensuring
/// proper event deduplication per the SOME/IP specification.
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
        // Unsubscribe from all eventgroups
        for eventgroup in &self.eventgroups {
            let _ = self.inner.cmd_tx.try_send(Command::Unsubscribe {
                service_id: self.service_id,
                instance_id: self.instance_id,
                major_version: self.major_version,
                eventgroup_id: eventgroup.value(),
                subscription_id: self.id,
            });
        }
    }
}

/// Listener for events from a statically configured service.
///
/// Use `runtime.listen_static()` to create a listener.
/// This is for static deployments where the service address is
/// pre-configured and events are sent directly to this client.
pub struct StaticEventListener {
    pub(crate) eventgroup: EventgroupId,
    pub(crate) events: mpsc::Receiver<Event>,
}

impl StaticEventListener {
    /// Receive the next event.
    ///
    /// Returns `None` if the listener has been closed.
    pub async fn next(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    /// Get the eventgroup ID this listener is subscribed to.
    pub fn eventgroup(&self) -> EventgroupId {
        self.eventgroup
    }
}
