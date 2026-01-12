//! `ServiceInstance` with typestate-based lifecycle management

use std::collections::HashMap;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use crate::error::{Error, Result};
use crate::handles::runtime::RuntimeInner;
use crate::runtime::{Command, ServiceRequest};
use crate::{ClientInfo, EventId, EventgroupId, InstanceId, MethodId, ServiceId};

use super::{Responder, ServiceEvent};

// ============================================================================
// TYPESTATE MARKERS
// ============================================================================

/// Type-state marker: service instance is **bound** (listening on socket)
/// but **not announced** via Service Discovery.
///
/// In this state, the service can:
/// - Handle requests from statically-configured clients
/// - Transition to [`Announced`] via `.announce()`
///
/// The service cannot be discovered dynamically until announced.
#[derive(Debug, Clone)]
pub struct Bound;

/// Type-state marker: service instance is **announced** via Service Discovery.
///
/// In this state, the service can:
/// - Be discovered by clients via SD
/// - Handle requests from any client
/// - Transition back to [`Bound`] via `.stop_announcing()`
#[derive(Debug, Clone)]
pub struct Announced;

// ============================================================================
// SERVICE INSTANCE
// ============================================================================

/// A service instance with typestate-based lifecycle management.
///
/// The service instance can be in one of two states:
/// - `Bound`: Listening on an endpoint but NOT announced via Service Discovery
/// - `Announced`: Listening AND announced via SD (`OfferService` sent)
///
/// # Lifecycle
///
/// ```text
/// Runtime::bind() → ServiceInstance<Bound>
///                         │
///                    announce()
///                         ↓
///                   ServiceInstance<Announced>
///                         │
///                   stop_announcing()
///                         ↓
///                   ServiceInstance<Bound>
/// ```
///
/// # State-Specific Operations
///
/// - **Bound state**: Can receive RPC (for static deployments), add static subscribers,
///   and notify static subscribers
/// - **Announced state**: Can receive RPC, receive dynamic subscriptions, and notify
///   all subscribers (static + dynamic)
pub struct ServiceInstance<State> {
    /// Inner runtime handle - wrapped in Option for state transitions
    inner: Option<Arc<RuntimeInner>>,
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    /// Requests channel - wrapped in Option for state transitions
    requests: Option<mpsc::Receiver<ServiceRequest>>,
    /// Static subscribers (pre-configured, independent of SD)
    static_subscribers: HashMap<SocketAddr, Vec<EventgroupId>>,
    _phantom: PhantomData<State>,
}

impl ServiceInstance<Bound> {
    pub(crate) fn new(
        inner: Arc<RuntimeInner>,
        service_id: ServiceId,
        instance_id: InstanceId,
        major_version: u8,
        requests: mpsc::Receiver<ServiceRequest>,
    ) -> Self {
        Self {
            inner: Some(inner),
            service_id,
            instance_id,
            major_version,
            requests: Some(requests),
            static_subscribers: HashMap::new(),
            _phantom: PhantomData,
        }
    }

    /// Announce this service instance via Service Discovery.
    ///
    /// Sends an `OfferService` message and starts cyclic offer announcements.
    /// After this, clients can discover the service via SD.
    ///
    /// Consumes `self` and returns `ServiceInstance<Announced>`.
    pub async fn announce(mut self) -> Result<ServiceInstance<Announced>> {
        let inner = self.inner.take().expect("inner should be Some");
        let requests = self.requests.take().expect("requests should be Some");

        let (response_tx, response_rx) = oneshot::channel();

        inner
            .cmd_tx
            .send(Command::StartAnnouncing {
                service_id: self.service_id,
                instance_id: self.instance_id,
                major_version: self.major_version,
                response: response_tx,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        response_rx.await.map_err(|_| Error::RuntimeShutdown)??;

        // self.inner and self.requests are now None, so Drop won't send StopOffer
        Ok(ServiceInstance {
            inner: Some(inner),
            service_id: self.service_id,
            instance_id: self.instance_id,
            major_version: self.major_version,
            requests: Some(requests),
            static_subscribers: std::mem::take(&mut self.static_subscribers),
            _phantom: PhantomData,
        })
    }

    /// Add a static subscriber (pre-configured, no SD required).
    ///
    /// Static subscribers receive notifications via `notify_static()` even
    /// when the service is not announced. This supports deployments where
    /// endpoints are pre-configured (implicit subscriptions per SD spec).
    pub fn add_static_subscriber(&mut self, address: SocketAddr, eventgroups: &[EventgroupId]) {
        self.static_subscribers
            .insert(address, eventgroups.to_vec());
    }

    /// Send a notification to static subscribers only.
    ///
    /// This works even when the service is not announced via SD.
    /// Only subscribers registered via `add_static_subscriber()` will receive
    /// the notification.
    pub async fn notify_static(
        &self,
        eventgroup: EventgroupId,
        event_id: EventId,
        payload: &[u8],
    ) -> Result<()> {
        let inner = self.inner.as_ref().expect("inner should be Some");

        // Find static subscribers for this eventgroup
        let targets: Vec<SocketAddr> = self
            .static_subscribers
            .iter()
            .filter(|(_, groups)| groups.contains(&eventgroup))
            .map(|(addr, _)| *addr)
            .collect();

        if targets.is_empty() {
            return Ok(()); // No static subscribers for this eventgroup
        }

        inner
            .cmd_tx
            .send(Command::NotifyStatic {
                service_id: self.service_id,
                instance_id: self.instance_id,
                major_version: self.major_version,
                eventgroup_id: eventgroup.value(),
                event_id: event_id.value(),
                payload: bytes::Bytes::copy_from_slice(payload),
                targets,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        Ok(())
    }

    /// Receive the next service event.
    ///
    /// In Bound state, this receives RPC requests from clients that have
    /// pre-configured the service address (static deployment).
    ///
    /// Returns `None` if the runtime has shut down.
    pub async fn next(&mut self) -> Option<ServiceEvent> {
        let requests = self.requests.as_mut()?;
        receive_service_event(requests).await
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

impl ServiceInstance<Announced> {
    /// Stop announcing this service instance via Service Discovery.
    ///
    /// Sends a `StopOfferService` message. The socket remains open for
    /// draining existing connections.
    ///
    /// Consumes `self` and returns `ServiceInstance<Bound>`.
    pub async fn stop_announcing(mut self) -> Result<ServiceInstance<Bound>> {
        let inner = self.inner.take().expect("inner should be Some");
        let requests = self.requests.take().expect("requests should be Some");

        let (response_tx, response_rx) = oneshot::channel();

        inner
            .cmd_tx
            .send(Command::StopAnnouncing {
                service_id: self.service_id,
                instance_id: self.instance_id,
                major_version: self.major_version,
                response: response_tx,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        response_rx.await.map_err(|_| Error::RuntimeShutdown)??;

        // self.inner and self.requests are now None, so Drop won't send StopOffer
        Ok(ServiceInstance {
            inner: Some(inner),
            service_id: self.service_id,
            instance_id: self.instance_id,
            major_version: self.major_version,
            requests: Some(requests),
            static_subscribers: std::mem::take(&mut self.static_subscribers),
            _phantom: PhantomData,
        })
    }

    /// Send a notification to all subscribers (static + dynamic).
    ///
    /// This sends to both:
    /// - Static subscribers registered via `add_static_subscriber()`
    /// - Dynamic subscribers that subscribed via Service Discovery
    pub async fn notify(
        &self,
        eventgroup: EventgroupId,
        event_id: EventId,
        payload: &[u8],
    ) -> Result<()> {
        let inner = self.inner.as_ref().expect("inner should be Some");

        inner
            .cmd_tx
            .send(Command::Notify {
                service_id: self.service_id,
                instance_id: self.instance_id,
                major_version: self.major_version,
                eventgroup_ids: vec![eventgroup.value()],
                event_id: event_id.value(),
                payload: bytes::Bytes::copy_from_slice(payload),
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        Ok(())
    }

    /// Send a notification ONLY to static subscribers.
    ///
    /// Useful when you want to notify pre-configured subscribers
    /// independently from Service Discovery-based subscribers.
    ///
    /// This is the same as `notify_static()` on `Bound` state.
    pub async fn notify_static(
        &self,
        eventgroup: EventgroupId,
        event_id: EventId,
        payload: &[u8],
    ) -> Result<()> {
        let inner = self.inner.as_ref().expect("inner should be Some");

        // Find static subscribers for this eventgroup
        let targets: Vec<std::net::SocketAddr> = self
            .static_subscribers
            .iter()
            .filter(|(_, groups)| groups.contains(&eventgroup))
            .map(|(addr, _)| *addr)
            .collect();

        if targets.is_empty() {
            return Ok(()); // No static subscribers for this eventgroup
        }

        inner
            .cmd_tx
            .send(Command::NotifyStatic {
                service_id: self.service_id,
                instance_id: self.instance_id,
                major_version: self.major_version,
                eventgroup_id: eventgroup.value(),
                event_id: event_id.value(),
                payload: bytes::Bytes::copy_from_slice(payload),
                targets,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        Ok(())
    }

    /// Receive the next service event.
    ///
    /// In Announced state, this receives:
    /// - RPC requests (Call, `FireForget`)
    /// - Subscription events (Subscribe, Unsubscribe)
    ///
    /// Returns `None` if the runtime has shut down.
    pub async fn next(&mut self) -> Option<ServiceEvent> {
        let requests = self.requests.as_mut()?;
        receive_service_event(requests).await
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

impl<State> Drop for ServiceInstance<State> {
    fn drop(&mut self) {
        // Only send StopOffer if inner is still Some (not taken during state transition)
        if let Some(inner) = &self.inner {
            let _ = inner.cmd_tx.try_send(Command::StopOffer {
                service_id: self.service_id,
                instance_id: self.instance_id,
                major_version: self.major_version,
            });
        }
    }
}

/// Helper function to receive service events from the request channel
async fn receive_service_event(
    requests: &mut mpsc::Receiver<ServiceRequest>,
) -> Option<ServiceEvent> {
    loop {
        match requests.recv().await? {
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
