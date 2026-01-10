//! ProxyHandle for calling methods on remote SOME/IP services

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use crate::error::{Error, Result};
use crate::runtime::Command;
use crate::handles::runtime::RuntimeInner;
use crate::{EventgroupId, InstanceId, MajorVersion, MethodId, Response, ServiceId};

use super::Subscription;

/// Client-side proxy to a remote SOME/IP service.
///
/// # Creating a Proxy
///
/// Use [`Runtime::find`](crate::Runtime::find) to find and connect to a service:
///
/// ```no_run
/// use someip_runtime::prelude::*;
///
/// const MY_SERVICE_ID: u16 = 0x1234;
///
/// # #[tokio::main]
/// # async fn main() -> Result<()> {
/// let runtime = Runtime::new(RuntimeConfig::default()).await?;
/// let proxy = runtime.find(MY_SERVICE_ID).await?;
/// # Ok(())
/// # }
/// ```
///
/// # Calling Methods
///
/// Call methods with `.call()`:
///
/// ```no_run
/// use someip_runtime::prelude::*;
/// use someip_runtime::handles::ProxyHandle;
///
/// async fn call_method(proxy: &ProxyHandle) -> Result<()> {
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
/// use someip_runtime::handles::ProxyHandle;
///
/// async fn subscribe_events(proxy: &ProxyHandle) -> Result<()> {
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
/// use someip_runtime::handles::ProxyHandle;
///
/// async fn clone_example(proxy: ProxyHandle) {
///     let method = MethodId::new(0x0001).unwrap();
///     let proxy2 = proxy.clone();
///     tokio::spawn(async move {
///         let _ = proxy2.call(method, b"hello").await;
///     });
/// }
/// # fn main() {}
/// ```
pub struct ProxyHandle {
    inner: Arc<RuntimeInner>,
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    endpoint: SocketAddr,
    transport: crate::config::Transport,
}

impl Clone for ProxyHandle {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            service_id: self.service_id,
            instance_id: self.instance_id,
            major_version: self.major_version,
            endpoint: self.endpoint,
            transport: self.transport,
        }
    }
}

impl ProxyHandle {
    /// Create a new `ProxyHandle` (for static deployments or after discovery).
    pub(crate) fn new(
        inner: Arc<RuntimeInner>,
        service_id: ServiceId,
        instance_id: InstanceId,
        major_version: u8,
        endpoint: SocketAddr,
        transport: crate::config::Transport,
    ) -> Self {
        Self {
            inner,
            service_id,
            instance_id,
            major_version,
            endpoint,
            transport,
        }
    }

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
        self.transport
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
    /// use someip_runtime::handles::ProxyHandle;
    ///
    /// async fn concurrent_calls(proxy: ProxyHandle) {
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
                target_endpoint: self.endpoint,
                target_transport: self.transport,
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
                target_endpoint: self.endpoint,
                target_transport: self.transport,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        Ok(())
    }

    /// Subscribe to an eventgroup.
    ///
    /// Returns a subscription that can be used to receive events.
    pub async fn subscribe(&self, eventgroup: EventgroupId) -> Result<Subscription> {
        let (events_tx, events_rx) = mpsc::channel(64);
        let (response_tx, response_rx) = oneshot::channel();

        self.inner
            .cmd_tx
            .send(Command::Subscribe {
                service_id: self.service_id,
                instance_id: self.instance_id,
                major_version: self.major_version,
                eventgroup_id: eventgroup.value(),
                events: events_tx,
                response: response_tx,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        let subscription_id = response_rx.await.map_err(|_| Error::RuntimeShutdown)??;

        Ok(Subscription::new(
            Arc::clone(&self.inner),
            self.service_id,
            self.instance_id,
            self.major_version,
            eventgroup,
            subscription_id,
            events_rx,
        ))
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
        self.endpoint
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        // Notify runtime to stop finding (best effort)
        let _ = self.inner.cmd_tx.try_send(Command::StopFind {
            service_id: self.service_id,
            instance_id: self.instance_id,
            major_version: MajorVersion::new(self.major_version),
        });
    }
}
