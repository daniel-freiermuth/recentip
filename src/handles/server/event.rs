//! Event handle for server-side notifications
//!
//! Events are created from a [`ServiceOffering`] and know which eventgroups
//! they belong to. This enables a simpler `notify()` API that automatically
//! routes to the correct subscribers.
//!
//! # Example
//!
//! ```no_run
//! # use recentip::prelude::*;
//! # async fn example(offering: ServiceOffering) -> Result<()> {
//! // Create an event that belongs to eventgroup 0x0001
//! let temperature = offering
//!     .event(EventId::new(0x8001).unwrap())
//!     .eventgroup(EventgroupId::new(0x0001).unwrap())
//!     .create().await?;
//!
//! // Send to all subscribers of eventgroup 0x0001
//! temperature.notify(b"42.5").await?;
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;

use crate::error::{Error, Result};
use crate::handles::runtime::RuntimeInner;
use crate::runtime::Command;
use crate::{EventId, EventgroupId, InstanceId, ServiceId};

/// Builder for creating an [`EventHandle`].
///
/// Obtained via [`ServiceOffering::event()`](super::ServiceOffering::event).
pub struct EventBuilder {
    inner: Arc<RuntimeInner>,
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    event_id: EventId,
    eventgroups: Vec<EventgroupId>,
}

impl EventBuilder {
    pub(crate) fn new(
        inner: Arc<RuntimeInner>,
        service_id: ServiceId,
        instance_id: InstanceId,
        major_version: u8,
        event_id: EventId,
    ) -> Self {
        Self {
            inner,
            service_id,
            instance_id,
            major_version,
            event_id,
            eventgroups: Vec::new(),
        }
    }

    /// Add an eventgroup that this event belongs to.
    ///
    /// An event can belong to multiple eventgroups. When `notify()` is called,
    /// the event is sent to subscribers of ALL configured eventgroups.
    ///
    /// At least one eventgroup must be specified before calling `create()`.
    pub fn eventgroup(mut self, eventgroup: EventgroupId) -> Self {
        if !self.eventgroups.contains(&eventgroup) {
            self.eventgroups.push(eventgroup);
        }
        self
    }

    /// Create the [`EventHandle`].
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No eventgroups were specified
    /// - The event ID is already registered for this service instance
    pub async fn create(self) -> Result<EventHandle> {
        if self.eventgroups.is_empty() {
            return Err(Error::Config(crate::error::ConfigError::new(
                "Event must belong to at least one eventgroup",
            )));
        }

        // Register the event with the runtime to validate uniqueness
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.inner
            .cmd_tx
            .send(crate::runtime::Command::RegisterEvent {
                service_id: self.service_id,
                instance_id: self.instance_id,
                major_version: self.major_version,
                event_id: self.event_id.value(),
                response: tx,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        // Wait for registration result
        rx.await.map_err(|_| Error::RuntimeShutdown)??;

        Ok(EventHandle {
            inner: self.inner,
            service_id: self.service_id,
            instance_id: self.instance_id,
            major_version: self.major_version,
            event_id: self.event_id,
            eventgroups: self.eventgroups,
        })
    }
}

/// Handle to a specific event within an offered service.
///
/// Created via [`EventBuilder::create()`]. The handle knows which eventgroups
/// the event belongs to and routes notifications automatically.
///
/// # Example
///
/// ```no_run
/// # use recentip::prelude::*;
/// # async fn example(event: EventHandle) -> Result<()> {
/// // Send notification to all subscribers
/// event.notify(b"payload").await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct EventHandle {
    inner: Arc<RuntimeInner>,
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    event_id: EventId,
    eventgroups: Vec<EventgroupId>,
}

impl EventHandle {
    /// Send a notification to all subscribers of this event's eventgroups.
    ///
    /// The notification is sent to subscribers of ALL eventgroups this event
    /// belongs to. Each subscriber receives the notification at most once,
    /// even if subscribed to multiple eventgroups containing this event.
    pub async fn notify(&self, payload: &[u8]) -> Result<()> {
        self.inner
            .cmd_tx
            .send(Command::Notify {
                service_id: self.service_id,
                instance_id: self.instance_id,
                major_version: self.major_version,
                eventgroup_ids: self.eventgroups.iter().map(|eg| eg.value()).collect(),
                event_id: self.event_id.value(),
                payload: bytes::Bytes::copy_from_slice(payload),
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;
        Ok(())
    }

    /// Get the event ID.
    pub fn id(&self) -> EventId {
        self.event_id
    }

    /// Get the eventgroups this event belongs to.
    pub fn eventgroups(&self) -> &[EventgroupId] {
        &self.eventgroups
    }
}
