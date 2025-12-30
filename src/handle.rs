//! Handle types for interacting with the runtime.
//!
//! Handles are the user-facing API for finding services (client) and
//! offering services (server).

use std::marker::PhantomData;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use crate::error::{Error, Result};
use crate::runtime::{Command, RuntimeInner, ServiceAvailability, ServiceRequest};
use crate::{ClientInfo, Event, EventgroupId, EventId, InstanceId, MethodId, Response, ReturnCode, Service, ServiceId};

// ============================================================================
// PROXY HANDLE (CLIENT-SIDE)
// ============================================================================

/// Marker: service not yet available
#[derive(Clone)]
pub struct Unavailable;

/// Marker: service is available
#[derive(Clone)]
pub struct Available {
    endpoint: std::net::SocketAddr,
}

/// Proxy handle to a remote service.
///
/// Use `runtime.find::<MyService>(instance)` to create a proxy.
/// The proxy starts in `Unavailable` state and transitions to `Available`
/// when the service is discovered via Service Discovery.
///
/// `ProxyHandle` is `Clone`, allowing it to be shared across tasks for
/// concurrent requests.
pub struct ProxyHandle<S: Service, State> {
    inner: Arc<RuntimeInner>,
    service_id: ServiceId,
    instance_id: InstanceId,
    state: State,
    _phantom: PhantomData<S>,
}

impl<S: Service, State: Clone> Clone for ProxyHandle<S, State> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            service_id: self.service_id,
            instance_id: self.instance_id,
            state: self.state.clone(),
            _phantom: PhantomData,
        }
    }
}

impl<S: Service> ProxyHandle<S, Unavailable> {
    pub(crate) fn new(
        inner: Arc<RuntimeInner>,
        service_id: ServiceId,
        instance_id: InstanceId,
    ) -> Self {
        Self {
            inner,
            service_id,
            instance_id,
            state: Unavailable,
            _phantom: PhantomData,
        }
    }

    /// Wait until the service becomes available.
    ///
    /// This registers a find request with the runtime and waits for
    /// Service Discovery to locate the service.
    ///
    /// Returns an error if the runtime shuts down before the service is found.
    pub async fn available(self) -> Result<ProxyHandle<S, Available>> {
        let (notify_tx, mut notify_rx) = mpsc::channel(1);
        
        // Register find request
        self.inner.cmd_tx.send(Command::Find {
            service_id: self.service_id,
            instance_id: self.instance_id,
            notify: notify_tx,
        }).await.map_err(|_| Error::RuntimeShutdown)?;

        // Wait for availability notification
        let (endpoint, discovered_instance_id) = loop {
            match notify_rx.recv().await {
                Some(ServiceAvailability::Available { endpoint, instance_id }) => {
                    break (endpoint, instance_id)
                },
                Some(ServiceAvailability::Unavailable) => continue,
                None => {
                    // Runtime shut down before service was found
                    return Err(Error::RuntimeShutdown);
                }
            }
        };

        Ok(ProxyHandle {
            inner: Arc::clone(&self.inner),
            service_id: self.service_id,
            instance_id: InstanceId::Id(discovered_instance_id),
            state: Available { endpoint },
            _phantom: PhantomData,
        })
    }

    /// Check if the service is available without blocking.
    ///
    /// Returns `Ok(Available)` if available, `Err(self)` if not yet available.
    pub fn try_available(self) -> std::result::Result<ProxyHandle<S, Available>, Self> {
        // TODO: Check cached state from runtime
        Err(self)
    }

    /// Check if the service is available without consuming self.
    pub fn is_available(&self) -> bool {
        // TODO: Check cached state from runtime
        false
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

impl<S: Service> ProxyHandle<S, Available> {
    /// Call a method and wait for the response.
    ///
    /// Accepts any type that implements `AsRef<[u8]>`, including:
    /// - `&[u8]`, `&[u8; N]` (will be copied)
    /// - `Vec<u8>` (will be copied, but you can use `Bytes` for zero-copy)
    /// - `b"string literals"`
    ///
    /// The payload is copied internally to ensure it lives long enough for
    /// the async operation. For large payloads where zero-copy is important,
    /// consider using `call_owned` with a `Bytes` value directly.
    ///
    /// For concurrent requests, clone the proxy handle:
    /// ```ignore
    /// let proxy = proxy.clone();
    /// tokio::spawn(async move {
    ///     proxy.call(method, b"hello").await
    /// });
    /// ```
    pub async fn call(&self, method: MethodId, payload: impl AsRef<[u8]>) -> Result<Response> {
        let payload_bytes = bytes::Bytes::copy_from_slice(payload.as_ref());
        let (response_tx, response_rx) = oneshot::channel();
        
        self.inner
            .cmd_tx
            .send(Command::Call {
                service_id: self.service_id,
                instance_id: self.instance_id,
                method_id: method.value(),
                payload: payload_bytes,
                response: response_tx,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        response_rx.await.map_err(|_| Error::RuntimeShutdown)?
    }

    /// Fire and forget - send a request without expecting a response.
    pub async fn fire_and_forget(&self, method: MethodId, payload: &[u8]) -> Result<()> {
        let payload_bytes = bytes::Bytes::copy_from_slice(payload);
        
        self.inner
            .cmd_tx
            .send(Command::FireAndForget {
                service_id: self.service_id,
                instance_id: self.instance_id,
                method_id: method.value(),
                payload: payload_bytes,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        Ok(())
    }

    /// Subscribe to an eventgroup.
    ///
    /// Returns a subscription that can be used to receive events.
    pub async fn subscribe(&self, eventgroup: EventgroupId) -> Result<Subscription<S>> {
        let (events_tx, events_rx) = mpsc::channel(64);
        let (response_tx, response_rx) = oneshot::channel();
        
        self.inner
            .cmd_tx
            .send(Command::Subscribe {
                service_id: self.service_id,
                instance_id: self.instance_id,
                eventgroup_id: eventgroup.value(),
                events: events_tx,
                response: response_tx,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        response_rx.await.map_err(|_| Error::RuntimeShutdown)??;

        Ok(Subscription {
            inner: Arc::clone(&self.inner),
            service_id: self.service_id,
            instance_id: self.instance_id,
            eventgroup,
            events: events_rx,
            _phantom: PhantomData,
        })
    }

    /// Get the service ID
    pub fn service_id(&self) -> ServiceId {
        self.service_id
    }

    /// Get the instance ID
    pub fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    /// Get the endpoint address
    pub fn endpoint(&self) -> std::net::SocketAddr {
        self.state.endpoint
    }
}

impl<S: Service, State> Drop for ProxyHandle<S, State> {
    fn drop(&mut self) {
        // Notify runtime to stop finding (best effort)
        let _ = self.inner.cmd_tx.try_send(Command::StopFind {
            service_id: self.service_id,
            instance_id: self.instance_id,
        });
    }
}

// ============================================================================
// SUBSCRIPTION (CLIENT-SIDE)
// ============================================================================

/// Active subscription to an eventgroup.
///
/// Events are received via the `next()` method. The subscription is
/// automatically stopped when dropped.
pub struct Subscription<S: Service> {
    inner: Arc<RuntimeInner>,
    service_id: ServiceId,
    instance_id: InstanceId,
    eventgroup: EventgroupId,
    events: mpsc::Receiver<Event>,
    _phantom: PhantomData<S>,
}

impl<S: Service> Subscription<S> {
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

impl<S: Service> Drop for Subscription<S> {
    fn drop(&mut self) {
        let _ = self.inner.cmd_tx.try_send(Command::Unsubscribe {
            service_id: self.service_id,
            instance_id: self.instance_id,
            eventgroup_id: self.eventgroup.value(),
        });
    }
}

// ============================================================================
// OFFERING HANDLE (SERVER-SIDE)
// ============================================================================

/// Handle for an offered service.
///
/// Use `runtime.offer::<MyService>(instance)` to create an offering.
/// Receive requests via `next()` and send events via `notify()`.
pub struct OfferingHandle<S: Service> {
    inner: Arc<RuntimeInner>,
    service_id: ServiceId,
    instance_id: InstanceId,
    requests: mpsc::Receiver<ServiceRequest>,
    _phantom: PhantomData<S>,
}

impl<S: Service> OfferingHandle<S> {
    pub(crate) fn new(
        inner: Arc<RuntimeInner>,
        service_id: ServiceId,
        instance_id: InstanceId,
        requests: mpsc::Receiver<ServiceRequest>,
    ) -> Self {
        Self {
            inner,
            service_id,
            instance_id,
            requests,
            _phantom: PhantomData,
        }
    }

    /// Receive the next service event.
    ///
    /// Returns `None` if the runtime has shut down.
    pub async fn next(&mut self) -> Option<ServiceEvent> {
        match self.requests.recv().await? {
            ServiceRequest::MethodCall { method_id, payload, client, response } => {
                Some(ServiceEvent::Call {
                    method: MethodId::new(method_id).unwrap_or(MethodId::new(1).unwrap()),
                    payload,
                    client: ClientInfo { address: client },
                    responder: Responder { response: Some(response) },
                })
            }
            ServiceRequest::FireForget { method_id, payload, client } => {
                Some(ServiceEvent::FireForget {
                    method: MethodId::new(method_id),
                    payload,
                    client: ClientInfo { address: client },
                })
            }
            ServiceRequest::Subscribe { eventgroup_id, client, response } => {
                Some(ServiceEvent::Subscribe {
                    eventgroup: EventgroupId::new(eventgroup_id).unwrap_or(EventgroupId::new(1).unwrap()),
                    client: ClientInfo { address: client },
                    ack: SubscribeAck { response: Some(response) },
                })
            }
            ServiceRequest::Unsubscribe { eventgroup_id, client } => {
                Some(ServiceEvent::Unsubscribe {
                    eventgroup: EventgroupId::new(eventgroup_id).unwrap_or(EventgroupId::new(1).unwrap()),
                    client: ClientInfo { address: client },
                })
            }
        }
    }

    /// Send a notification event to all subscribers of an eventgroup.
    pub async fn notify(&self, eventgroup: EventgroupId, event_id: EventId, payload: &[u8]) -> Result<()> {
        self.inner
            .cmd_tx
            .send(Command::Notify {
                service_id: self.service_id,
                instance_id: self.instance_id,
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

impl<S: Service> Drop for OfferingHandle<S> {
    fn drop(&mut self) {
        let _ = self.inner.cmd_tx.try_send(Command::StopOffer {
            service_id: self.service_id,
            instance_id: self.instance_id,
        });
    }
}

// ============================================================================
// SERVICE EVENTS
// ============================================================================

/// Events received by an offered service
#[derive(Debug)]
pub enum ServiceEvent {
    /// A method was called
    Call {
        method: MethodId,
        payload: bytes::Bytes,
        client: ClientInfo,
        responder: Responder,
    },
    /// A fire-and-forget method was called (no response expected)
    FireForget {
        method: MethodId,
        payload: bytes::Bytes,
        client: ClientInfo,
    },
    /// A client wants to subscribe
    Subscribe {
        eventgroup: EventgroupId,
        client: ClientInfo,
        ack: SubscribeAck,
    },
    /// A client unsubscribed
    Unsubscribe {
        eventgroup: EventgroupId,
        client: ClientInfo,
    },
}

/// Responder for method calls - must be used to send a response.
#[derive(Debug)]
pub struct Responder {
    response: Option<oneshot::Sender<Result<bytes::Bytes>>>,
}

impl Responder {
    /// Send a successful response.
    pub async fn reply(mut self, payload: &[u8]) -> Result<()> {
        if let Some(tx) = self.response.take() {
            let _ = tx.send(Ok(bytes::Bytes::copy_from_slice(payload)));
        }
        Ok(())
    }

    /// Send an error response.
    pub async fn reply_error(mut self, code: ReturnCode) -> Result<()> {
        if let Some(tx) = self.response.take() {
            let _ = tx.send(Err(Error::Protocol(crate::error::ProtocolError {
                message: format!("Error: {:?}", code),
            })));
        }
        Ok(())
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        if self.response.is_some() {
            tracing::warn!("Responder dropped without sending response");
            // In debug mode we could panic, but we chose zero-panic
        }
    }
}

/// Acknowledgment for subscribe requests - must be used to accept or reject.
#[derive(Debug)]
pub struct SubscribeAck {
    response: Option<oneshot::Sender<Result<bool>>>,
}

impl SubscribeAck {
    /// Accept the subscription.
    pub async fn accept(mut self) -> Result<()> {
        if let Some(tx) = self.response.take() {
            let _ = tx.send(Ok(true));
        }
        Ok(())
    }

    /// Reject the subscription.
    pub async fn reject(mut self) -> Result<()> {
        if let Some(tx) = self.response.take() {
            let _ = tx.send(Ok(false));
        }
        Ok(())
    }
}

impl Drop for SubscribeAck {
    fn drop(&mut self) {
        if self.response.is_some() {
            tracing::warn!("SubscribeAck dropped without accepting or rejecting");
        }
    }
}
