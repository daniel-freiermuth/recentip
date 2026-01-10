//! Subscription handles for receiving events from SOME/IP services

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::handles::runtime::RuntimeInner;
use crate::runtime::Command;
use crate::{Event, EventgroupId, InstanceId, ServiceId};

/// Active subscription to an eventgroup.
///
/// Events are received via the `next()` method. The subscription is
/// automatically stopped when dropped.
pub struct Subscription {
    inner: Arc<RuntimeInner>,
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    eventgroup: EventgroupId,
    subscription_id: u64,
    events: mpsc::Receiver<Event>,
}

impl Subscription {
    /// Create a new subscription (internal use only)
    pub(crate) fn new(
        inner: Arc<RuntimeInner>,
        service_id: ServiceId,
        instance_id: InstanceId,
        major_version: u8,
        eventgroup: EventgroupId,
        subscription_id: u64,
        events: mpsc::Receiver<Event>,
    ) -> Self {
        Self {
            inner,
            service_id,
            instance_id,
            major_version,
            eventgroup,
            subscription_id,
            events,
        }
    }

    /// Receive the next event.
    ///
    /// Returns `None` if the subscription has ended.
    pub async fn next(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    /// Get the eventgroup ID
    pub fn eventgroup(&self) -> EventgroupId {
        self.eventgroup
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let _ = self.inner.cmd_tx.try_send(Command::Unsubscribe {
            service_id: self.service_id,
            instance_id: self.instance_id,
            major_version: self.major_version,
            eventgroup_id: self.eventgroup.value(),
            subscription_id: self.subscription_id,
        });
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
