//! # Handle Types for SOME/IP Communication
//!
//! This module provides the **user-facing API** for interacting with the runtime.
//! All SOME/IP operations go through handles, which internally send commands to
//! the runtime's event loop.
//!
//! ## Handle Types Overview
//!
//! | Handle | Role | State Pattern |
//! |--------|------|---------------|
//! | [`ProxyHandle`] | Client: call methods, subscribe to events | `Unavailable` → `Available` |
//! | [`OfferingHandle`] | Server: receive requests, send responses | — |
//! | [`ServiceInstance`] | Server (advanced): typestate for bind/announce | `Bound` → `Announced` |
//! | [`Subscription`] | Client: receive events from a subscribed eventgroup | — |
//! | [`SubscriptionAck`] | Server: accept/reject incoming subscriptions | — |
//! | [`Responder`] | Server: reply to a specific RPC request | Consumed on reply |
//!
//! ## Client-Side Pattern
//!
//! ```no_run
//! use someip_runtime::prelude::*;
//! use someip_runtime::handle::Available;
//!
//! struct MyService;
//! impl Service for MyService {
//!     const SERVICE_ID: u16 = 0x1234;
//!     const MAJOR_VERSION: u8 = 1;
//!     const MINOR_VERSION: u32 = 0;
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let runtime = Runtime::new(RuntimeConfig::default()).await?;
//!
//!     // 1. Create a proxy (starts Unavailable)
//!     let proxy = runtime.find::<MyService>(InstanceId::Any);
//!
//!     // 2. Wait for discovery (transitions to Available)
//!     let proxy: someip_runtime::handle::ProxyHandle<MyService, Available> = proxy.available().await?;
//!
//!     // 3. Call methods (only possible when Available)
//!     let method_id = MethodId::new(0x0001).unwrap();
//!     let response = proxy.call(method_id, b"payload").await?;
//!
//!     // 4. Subscribe to events
//!     let eventgroup = EventgroupId::new(0x0001).unwrap();
//!     let mut subscription = proxy.subscribe(eventgroup).await?;
//!     while let Some(event) = subscription.next().await {
//!         // Process event
//!     }
//!     Ok(())
//! }
//! ```
//!
//! ## Server-Side Pattern (Simple)
//!
//! ```no_run
//! use someip_runtime::prelude::*;
//! use someip_runtime::handle::ServiceEvent;
//!
//! struct MyService;
//! impl Service for MyService {
//!     const SERVICE_ID: u16 = 0x1234;
//!     const MAJOR_VERSION: u8 = 1;
//!     const MINOR_VERSION: u32 = 0;
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let runtime = Runtime::new(RuntimeConfig::default()).await?;
//!
//!     // 1. Offer a service
//!     let mut offering = runtime.offer::<MyService>(InstanceId::Id(1)).await?;
//!
//!     // 2. Handle incoming events
//!     while let Some(event) = offering.next().await {
//!         match event {
//!             ServiceEvent::Call { method, payload, responder, .. } => {
//!                 responder.reply(b"response").await?;
//!             }
//!             ServiceEvent::Subscribe { ack, .. } => {
//!                 ack.accept().await?;
//!             }
//!             _ => {}
//!         }
//!     }
//!     Ok(())
//! }
//! ```
//!
//! ## Server-Side Pattern (Advanced Typestate)
//!
//! For finer control over the bind/announce lifecycle:
//!
//! ```no_run
//! use someip_runtime::prelude::*;
//! use someip_runtime::handle::{ServiceInstance, Bound, Announced};
//!
//! struct MyService;
//! impl Service for MyService {
//!     const SERVICE_ID: u16 = 0x1234;
//!     const MAJOR_VERSION: u8 = 1;
//!     const MINOR_VERSION: u32 = 0;
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let runtime = Runtime::new(RuntimeConfig::default()).await?;
//!
//!     // 1. Bind (opens socket, but no SD announcement)
//!     let instance: ServiceInstance<_, Bound> = runtime.bind::<MyService>(
//!         InstanceId::Id(1),
//!     ).await?;
//!
//!     // 2. Start announcing (transitions to Announced)
//!     let instance: ServiceInstance<_, Announced> = instance.announce().await?;
//!
//!     // 3. Now handle requests...
//!
//!     // 4. Stop announcing (transitions back to Bound)
//!     let instance: ServiceInstance<_, Bound> = instance.stop_announcing().await?;
//!
//!     // Socket stays open, can re-announce later
//!     Ok(())
//! }
//! ```
//!
//! ## Type-State Pattern
//!
//! This module uses **type-state patterns** to enforce correct API usage at compile time:
//!
//! - [`ProxyHandle<S, Unavailable>`] can only call `.available()` or `.is_available()`
//! - [`ProxyHandle<S, Available>`] can call `.call()`, `.subscribe()`, etc.
//! - [`ServiceInstance<S, Bound>`] can only call `.announce()` or handle static requests
//! - [`ServiceInstance<S, Announced>`] can call `.stop_announcing()` or handle SD requests
//!
//! This prevents common bugs like:
//! - Calling a method before the service is discovered
//! - Announcing a service that hasn't bound to a socket
//!
//! ## Thread Safety
//!
//! All handles are `Clone` and can be shared across tokio tasks. They internally
//! hold an `Arc<RuntimeInner>` and communicate via channels.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use crate::error::{Error, Result};
use crate::runtime::{Command, RuntimeInner, ServiceAvailability, ServiceRequest};
use crate::{
    ClientInfo, Event, EventId, EventgroupId, InstanceId, MethodId, Response, ReturnCode, Service,
    ServiceId,
};

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
// PROXY HANDLE (CLIENT-SIDE)
// ============================================================================

/// Type-state marker: service **not yet discovered**.
///
/// The proxy must transition to [`Available`] via `.available()` before
/// methods can be called.
#[derive(Clone)]
pub struct Unavailable;

/// Type-state marker: service **discovered and ready**.
///
/// Contains the service's endpoint address and transport for RPC communication.
#[derive(Clone)]
pub struct Available {
    endpoint: std::net::SocketAddr,
    transport: crate::config::Transport,
}

/// Client-side proxy to a remote SOME/IP service.
///
/// # Creating a Proxy
///
/// Use [`Runtime::find`](crate::Runtime::find) to create a proxy:
///
/// ```no_run
/// use someip_runtime::prelude::*;
///
/// struct MyService;
/// impl Service for MyService {
///     const SERVICE_ID: u16 = 0x1234;
///     const MAJOR_VERSION: u8 = 1;
///     const MINOR_VERSION: u32 = 0;
/// }
///
/// # #[tokio::main]
/// # async fn main() -> Result<()> {
/// let runtime = Runtime::new(RuntimeConfig::default()).await?;
/// let proxy = runtime.find::<MyService>(InstanceId::Any);
/// # Ok(())
/// # }
/// ```
///
/// # Discovery (Unavailable → Available)
///
/// The proxy starts in [`Unavailable`] state. Call `.available()` to wait
/// for Service Discovery:
///
/// ```no_run
/// use someip_runtime::prelude::*;
/// use someip_runtime::handle::{ProxyHandle, Unavailable};
///
/// struct MyService;
/// impl Service for MyService {
///     const SERVICE_ID: u16 = 0x1234;
///     const MAJOR_VERSION: u8 = 1;
///     const MINOR_VERSION: u32 = 0;
/// }
///
/// async fn discover(proxy: ProxyHandle<MyService, Unavailable>) -> Result<()> {
///     let proxy = proxy.available().await?;
///     // Now `proxy` is `ProxyHandle<MyService, Available>`
///     Ok(())
/// }
/// # fn main() {}
/// ```
///
/// # Calling Methods
///
/// Once available, call methods with `.call()`:
///
/// ```no_run
/// use someip_runtime::prelude::*;
/// use someip_runtime::handle::{ProxyHandle, Available};
///
/// struct MyService;
/// impl Service for MyService {
///     const SERVICE_ID: u16 = 0x1234;
///     const MAJOR_VERSION: u8 = 1;
///     const MINOR_VERSION: u32 = 0;
/// }
///
/// async fn call_method(proxy: &ProxyHandle<MyService, Available>) -> Result<()> {
///     let method_id = MethodId::new(0x0001).unwrap();
///     let response = proxy.call(method_id, b"request payload").await?;
///     if response.return_code == ReturnCode::Ok {
///         // Success
///     }
///     Ok(())
/// }
/// # fn main() {}
/// ```
///
/// # Subscribing to Events
///
/// Subscribe to eventgroups to receive events:
///
/// ```no_run
/// use someip_runtime::prelude::*;
/// use someip_runtime::handle::{ProxyHandle, Available};
///
/// struct MyService;
/// impl Service for MyService {
///     const SERVICE_ID: u16 = 0x1234;
///     const MAJOR_VERSION: u8 = 1;
///     const MINOR_VERSION: u32 = 0;
/// }
///
/// async fn subscribe_events(proxy: &ProxyHandle<MyService, Available>) -> Result<()> {
///     let eventgroup = EventgroupId::new(0x0001).unwrap();
///     let mut sub = proxy.subscribe(eventgroup).await?;
///     while let Some(event) = sub.next().await {
///         println!("Event: {:?}", event);
///     }
///     Ok(())
/// }
/// # fn main() {}
/// ```
///
/// # Cloning
///
/// `ProxyHandle` is `Clone`. Clone it to share across tasks:
///
/// ```no_run
/// use someip_runtime::prelude::*;
/// use someip_runtime::handle::{ProxyHandle, Available};
///
/// struct MyService;
/// impl Service for MyService {
///     const SERVICE_ID: u16 = 0x1234;
///     const MAJOR_VERSION: u8 = 1;
///     const MINOR_VERSION: u32 = 0;
/// }
///
/// async fn clone_example(proxy: ProxyHandle<MyService, Available>) {
///     let method = MethodId::new(0x0001).unwrap();
///     let proxy2 = proxy.clone();
///     tokio::spawn(async move {
///         let _ = proxy2.call(method, b"hello").await;
///     });
/// }
/// # fn main() {}
/// ```
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
}

impl<S: Service> ProxyHandle<S, Available> {
    /// Create a new `ProxyHandle` that is immediately available (for static deployments).
    pub(crate) fn new_available(
        inner: Arc<RuntimeInner>,
        service_id: ServiceId,
        instance_id: InstanceId,
        endpoint: SocketAddr,
        transport: crate::config::Transport,
    ) -> Self {
        Self {
            inner,
            service_id,
            instance_id,
            state: Available { endpoint, transport },
            _phantom: PhantomData,
        }
    }
}

impl<S: Service> ProxyHandle<S, Unavailable> {
    /// Wait until the service becomes available.
    ///
    /// This registers a find request with the runtime and waits for
    /// Service Discovery to locate the service.
    ///
    /// Returns an error if the runtime shuts down before the service is found.
    pub async fn available(self) -> Result<ProxyHandle<S, Available>> {
        let (notify_tx, mut notify_rx) = mpsc::channel(1);

        // Register find request
        self.inner
            .cmd_tx
            .send(Command::Find {
                service_id: self.service_id,
                instance_id: self.instance_id,
                notify: notify_tx,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        // Wait for availability notification
        let (endpoint, transport, discovered_instance_id) = loop {
            match notify_rx.recv().await {
                Some(ServiceAvailability::Available {
                    endpoint,
                    transport,
                    instance_id,
                }) => break (endpoint, transport, instance_id),
                Some(ServiceAvailability::Unavailable) => continue,
                None => {
                    // Channel closed - either runtime shut down or find request expired
                    // (all repetitions exhausted without finding the service)
                    return Err(Error::NotAvailable);
                }
            }
        };

        Ok(ProxyHandle {
            inner: Arc::clone(&self.inner),
            service_id: self.service_id,
            instance_id: InstanceId::Id(discovered_instance_id),
            state: Available { endpoint, transport },
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
}

impl<S: Service> ProxyHandle<S, Available> {
    /// Get the transport being used for this proxy.
    ///
    /// Returns the transport (TCP or UDP) that was selected during service discovery.
    /// This reflects the `preferred_transport` configuration at the time of discovery,
    /// or whichever transport was available if only one was offered.
    ///
    /// # Stability
    ///
    /// **This API is unstable and intended for testing/diagnostics only.**
    /// It may be removed or changed in future versions without notice.
    #[doc(hidden)]
    pub fn transport(&self) -> crate::config::Transport {
        self.state.transport
    }

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
    /// ```no_run
    /// use someip_runtime::prelude::*;
    /// use someip_runtime::handle::{ProxyHandle, Available};
    ///
    /// struct MyService;
    /// impl Service for MyService {
    ///     const SERVICE_ID: u16 = 0x1234;
    ///     const MAJOR_VERSION: u8 = 1;
    ///     const MINOR_VERSION: u32 = 0;
    /// }
    ///
    /// async fn concurrent_calls(proxy: ProxyHandle<MyService, Available>) {
    ///     let method = MethodId::new(0x0001).unwrap();
    ///     let proxy2 = proxy.clone();
    ///     tokio::spawn(async move {
    ///         let _ = proxy2.call(method, b"hello").await;
    ///     });
    /// }
    /// # fn main() {}
    /// ```
    pub async fn call(&self, method: MethodId, payload: impl AsRef<[u8]>) -> Result<Response> {
        let payload_bytes = bytes::Bytes::copy_from_slice(payload.as_ref());
        let (response_tx, response_rx) = oneshot::channel();

        self.inner
            .cmd_tx
            .send(Command::Call {
                service_id: self.service_id,
                method_id: method.value(),
                payload: payload_bytes,
                response: response_tx,
                target_endpoint: self.state.endpoint,
                target_transport: self.state.transport,
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
                method_id: method.value(),
                payload: payload_bytes,
                target_endpoint: self.state.endpoint,
                target_transport: self.state.transport,
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
// STATIC EVENT LISTENER (CLIENT-SIDE, NO SD)
// ============================================================================

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
                    response,
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
                        ack: SubscribeAck {
                            response: Some(response),
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
                message: format!("Error: {code:?}"),
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

// ============================================================================
// SERVICE INSTANCE - TYPESTATE API
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
/// Runtime::bind() → ServiceInstance<S, Bound>
///                         │
///                    announce()
///                         ↓
///                   ServiceInstance<S, Announced>
///                         │
///                   stop_announcing()
///                         ↓
///                   ServiceInstance<S, Bound>
/// ```
///
/// # State-Specific Operations
///
/// - **Bound state**: Can receive RPC (for static deployments), add static subscribers,
///   and notify static subscribers
/// - **Announced state**: Can receive RPC, receive dynamic subscriptions, and notify
///   all subscribers (static + dynamic)
pub struct ServiceInstance<S: Service, State> {
    /// Inner runtime handle - wrapped in Option for state transitions
    inner: Option<Arc<RuntimeInner>>,
    service_id: ServiceId,
    instance_id: InstanceId,
    /// Requests channel - wrapped in Option for state transitions  
    requests: Option<mpsc::Receiver<ServiceRequest>>,
    /// Static subscribers (pre-configured, independent of SD)
    static_subscribers: HashMap<SocketAddr, Vec<EventgroupId>>,
    _phantom: PhantomData<(S, State)>,
}

impl<S: Service> ServiceInstance<S, Bound> {
    pub(crate) fn new(
        inner: Arc<RuntimeInner>,
        service_id: ServiceId,
        instance_id: InstanceId,
        requests: mpsc::Receiver<ServiceRequest>,
    ) -> Self {
        Self {
            inner: Some(inner),
            service_id,
            instance_id,
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
    /// Consumes `self` and returns `ServiceInstance<S, Announced>`.
    pub async fn announce(mut self) -> Result<ServiceInstance<S, Announced>> {
        let inner = self.inner.take().expect("inner should be Some");
        let requests = self.requests.take().expect("requests should be Some");

        let (response_tx, response_rx) = oneshot::channel();

        inner
            .cmd_tx
            .send(Command::StartAnnouncing {
                service_id: self.service_id,
                instance_id: self.instance_id,
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

impl<S: Service> ServiceInstance<S, Announced> {
    /// Stop announcing this service instance via Service Discovery.
    ///
    /// Sends a `StopOfferService` message. The socket remains open for
    /// draining existing connections.
    ///
    /// Consumes `self` and returns `ServiceInstance<S, Bound>`.
    pub async fn stop_announcing(mut self) -> Result<ServiceInstance<S, Bound>> {
        let inner = self.inner.take().expect("inner should be Some");
        let requests = self.requests.take().expect("requests should be Some");

        let (response_tx, response_rx) = oneshot::channel();

        inner
            .cmd_tx
            .send(Command::StopAnnouncing {
                service_id: self.service_id,
                instance_id: self.instance_id,
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
                eventgroup_id: eventgroup.value(),
                event_id: event_id.value(),
                payload: bytes::Bytes::copy_from_slice(payload),
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        Ok(())
    }

    /// Check if there are any subscribers for an eventgroup.
    ///
    /// Returns `true` if there are dynamic subscribers (from SD) or
    /// static subscribers for the given eventgroup.
    pub async fn has_subscribers(&self, eventgroup: EventgroupId) -> bool {
        // Check static subscribers first
        let has_static = self
            .static_subscribers
            .values()
            .any(|groups| groups.contains(&eventgroup));

        if has_static {
            return true;
        }

        // Query runtime for dynamic subscribers
        let inner = match self.inner.as_ref() {
            Some(inner) => inner,
            None => return false,
        };

        let (response_tx, response_rx) = oneshot::channel();

        if inner
            .cmd_tx
            .send(Command::HasSubscribers {
                service_id: self.service_id,
                instance_id: self.instance_id,
                eventgroup_id: eventgroup.value(),
                response: response_tx,
            })
            .await
            .is_err()
        {
            return false;
        }

        response_rx.await.unwrap_or(false)
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

impl<S: Service, State> Drop for ServiceInstance<S, State> {
    fn drop(&mut self) {
        // Only send StopOffer if inner is still Some (not taken during state transition)
        if let Some(inner) = &self.inner {
            let _ = inner.cmd_tx.try_send(Command::StopOffer {
                service_id: self.service_id,
                instance_id: self.instance_id,
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
                response,
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
                    ack: SubscribeAck {
                        response: Some(response),
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
