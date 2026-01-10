//! `OfferingHandle` for server-side service offerings

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::error::{Error, Result};
use crate::handles::runtime::RuntimeInner;
use crate::runtime::{Command, ServiceRequest};
use crate::{ClientInfo, EventId, EventgroupId, InstanceId, MethodId, ServiceId};

use super::{Responder, ServiceEvent};

/// Handle for an offered service.
///
/// Use `runtime.offer(service_id, instance)` to create an offering.
/// Receive requests via `next()` and send events via `notify()`.
pub struct OfferingHandle {
    inner: Arc<RuntimeInner>,
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    requests: mpsc::Receiver<ServiceRequest>,
}

impl OfferingHandle {
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
                        tracing::error!(
                            "BUG: runtime sent invalid method_id 0x{:04x} (high bit set = event, not method)",
                            method_id
                        );
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
                        tracing::error!(
                            "BUG: runtime sent invalid method_id 0x{:04x} (high bit set = event, not method)",
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

    /// Send a notification event to all subscribers of an eventgroup.
    pub async fn notify(
        &self,
        eventgroup: EventgroupId,
        event_id: EventId,
        payload: &[u8],
    ) -> Result<()> {
        self.inner
            .cmd_tx
            .send(Command::Notify {
                service_id: self.service_id,
                instance_id: self.instance_id,
                major_version: self.major_version,
                eventgroup_id: eventgroup.value(),
                event_id: event_id.value(),
                payload: bytes::Bytes::copy_from_slice(payload),
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;
        Ok(())
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

impl Drop for OfferingHandle {
    fn drop(&mut self) {
        let _ = self.inner.cmd_tx.try_send(Command::StopOffer {
            service_id: self.service_id,
            instance_id: self.instance_id,
            major_version: self.major_version,
        });
    }
}
