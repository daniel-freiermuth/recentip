//! `ServiceOffering` for server-side service offerings

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

use crate::handles::runtime::RuntimeInner;
use crate::handles::server::event::EventBuilder;
use crate::runtime::{Command, ServiceRequest};
use crate::{ClientInfo, EventId, EventgroupId, InstanceId, MethodId, ServiceId};

use super::{Responder, ServiceEvent};

/// Handle for an offered service.
///
/// Use `runtime.offer(service_id, instance)` to create an offering.
/// Receive requests via `next()` and send events via `notify()`.
pub struct ServiceOffering {
    inner: Arc<RuntimeInner>,
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    requests: mpsc::Receiver<ServiceRequest>,
}

impl ServiceOffering {
    pub(crate) fn new(
        inner: Arc<RuntimeInner>,
        service_id: ServiceId,
        instance_id: InstanceId,
        major_version: u8,
        requests: mpsc::Receiver<ServiceRequest>,
    ) -> Self {
        Self {
            inner,
            service_id,
            instance_id,
            major_version,
            requests,
        }
    }

    /// Receive the next service event.
    ///
    /// Returns `None` if the runtime has shut down.
    pub async fn next(&mut self) -> Option<ServiceEvent> {
        loop {
            match self.requests.recv().await? {
                ServiceRequest::MethodCall {
                    method_id,
                    payload,
                    client,
                    transport,
                    response,
                } => {
                    let Some(method) = MethodId::new(method_id) else {
                        // Defense-in-depth: should be caught earlier in server.rs
                        // If we get here, runtime already sent an error response
                        tracing::debug!(
                            "Dropping method call with invalid method_id 0x{:04x} (high bit set = event, not method)",
                            method_id
                        );
                        // Response channel will be dropped, runtime handles cleanup
                        continue;
                    };
                    return Some(ServiceEvent::Call {
                        method,
                        payload,
                        client: ClientInfo {
                            address: client,
                            transport,
                        },
                        responder: Responder {
                            response: Some(response),
                        },
                    });
                }
                ServiceRequest::FireForget {
                    method_id,
                    payload,
                    client,
                    transport,
                } => {
                    let Some(method) = MethodId::new(method_id) else {
                        // Defense-in-depth: should be caught earlier in server.rs
                        tracing::debug!(
                            "Dropping fire-forget with invalid method_id 0x{:04x} (high bit set = event, not method)",
                            method_id
                        );
                        continue;
                    };
                    return Some(ServiceEvent::FireForget {
                        method,
                        payload,
                        client: ClientInfo {
                            address: client,
                            transport,
                        },
                    });
                }
                ServiceRequest::Subscribe {
                    eventgroup_id,
                    client,
                    transport,
                } => {
                    let Some(eventgroup) = EventgroupId::new(eventgroup_id) else {
                        tracing::error!(
                            "BUG: runtime sent invalid eventgroup_id 0x{:04x} (reserved value)",
                            eventgroup_id
                        );
                        continue;
                    };
                    return Some(ServiceEvent::Subscribe {
                        eventgroup,
                        client: ClientInfo {
                            address: client,
                            transport,
                        },
                    });
                }
                ServiceRequest::Unsubscribe {
                    eventgroup_id,
                    client,
                    transport,
                } => {
                    let Some(eventgroup) = EventgroupId::new(eventgroup_id) else {
                        tracing::error!(
                            "BUG: runtime sent invalid eventgroup_id 0x{:04x} (reserved value)",
                            eventgroup_id
                        );
                        continue;
                    };
                    return Some(ServiceEvent::Unsubscribe {
                        eventgroup,
                        client: ClientInfo {
                            address: client,
                            transport,
                        },
                    });
                }
            }
        }
    }

    /// Create an event that can send notifications to subscribers.
    ///
    /// Events declare which eventgroups they belong to via the builder.
    /// When `notify()` is called on the returned [`EventHandle`], the
    /// notification is sent to subscribers of all configured eventgroups.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use recentip::prelude::*;
    /// # async fn example(offering: ServiceOffering) -> Result<()> {
    /// let temperature = offering
    ///     .event(EventId::new(0x8001).unwrap())
    ///     .eventgroup(EventgroupId::new(0x0001).unwrap())
    ///     .create().await?;
    ///
    /// temperature.notify(b"42.5").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn event(&self, event_id: EventId) -> EventBuilder {
        EventBuilder::new(
            self.inner.clone(),
            self.service_id,
            self.instance_id,
            self.major_version,
            event_id,
        )
    }

    /// Get the service ID
    pub fn service_id(&self) -> ServiceId {
        self.service_id
    }

    /// Get the instance ID
    pub fn instance_id(&self) -> InstanceId {
        self.instance_id
    }
}

impl Drop for ServiceOffering {
    fn drop(&mut self) {
        // Stop offering (best-effort).
        // If the channel is full or closed, the runtime will clean up on shutdown.
        let cmd = Command::StopOffer {
            service_id: self.service_id,
            instance_id: self.instance_id,
            major_version: self.major_version,
        };
        if let Err(e) = self.inner.cmd_tx.try_send(cmd) {
            match e {
                TrySendError::Full(_) => {
                    tracing::warn!(
                        "Failed to send StopOffer for service {:04x}:{:04x}: \
                         command channel full. Service will stop on runtime shutdown.",
                        self.service_id.value(),
                        self.instance_id.value()
                    );
                }
                TrySendError::Closed(_) => {
                    // Runtime already shut down - this is expected during shutdown
                    tracing::debug!(
                        "StopOffer skipped: runtime already shut down (service {:04x}:{:04x})",
                        self.service_id.value(),
                        self.instance_id.value()
                    );
                }
            }
        }
    }
}
