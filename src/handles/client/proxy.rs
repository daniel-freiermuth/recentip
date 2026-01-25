//! `OfferedService` for calling methods on remote SOME/IP services

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::oneshot;

use crate::error::{Error, Result};
use crate::handles::runtime::RuntimeInner;
use crate::runtime::Command;
use crate::{InstanceId, MajorVersion, MethodId, Response, ServiceId};

use super::SubscriptionBuilder;

/// Client-side proxy to a remote SOME/IP service.
///
/// # Creating a Proxy
///
/// Use [`SomeIp::find`](crate::SomeIp::find) to find and connect to a service:
///
/// ```no_run
/// use recentip::prelude::*;
///
/// const MY_SERVICE_ID: u16 = 0x1234;
///
/// # #[tokio::main]
/// # async fn main() -> Result<()> {
/// let runtime = recentip::configure().start().await?;
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
/// use recentip::prelude::*;
/// use recentip::handles::OfferedService;
///
/// async fn call_method(proxy: &OfferedService) -> Result<()> {
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
/// use recentip::prelude::*;
/// use recentip::handles::OfferedService;
///
/// async fn subscribe_events(proxy: &OfferedService) -> Result<()> {
///     let eg1 = EventgroupId::new(0x0001).unwrap();
///     let eg2 = EventgroupId::new(0x0002).unwrap();
///     
///     let mut subscription = proxy
///         .new_subscription()
///         .eventgroup(eg1)
///         .eventgroup(eg2)
///         .subscribe()
///         .await?;
///     
///     while let Some(event) = subscription.next().await {
///         println!("Event: {:?}", event);
///     }
///     Ok(())
/// }
/// # fn main() {}
/// ```
///
/// # Cloning
///
/// `OfferedService` is `Clone`. Clone it to share across tasks:
///
/// ```no_run
/// use recentip::prelude::*;
/// use recentip::handles::OfferedService;
///
/// async fn clone_example(proxy: OfferedService) {
///     let method = MethodId::new(0x0001).unwrap();
///     let proxy2 = proxy.clone();
///     tokio::spawn(async move {
///         let _ = proxy2.call(method, b"hello").await;
///     });
/// }
/// # fn main() {}
/// ```
pub struct OfferedService {
    inner: Arc<RuntimeInner>,
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    endpoint: SocketAddr,
    transport: crate::config::Transport,
    /// Original find criteria - used for StopFind on drop
    /// If None, this proxy was created without discovery (static deployment)
    find_criteria: Option<(InstanceId, MajorVersion)>,
}

impl Clone for OfferedService {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            service_id: self.service_id,
            instance_id: self.instance_id,
            major_version: self.major_version,
            endpoint: self.endpoint,
            transport: self.transport,
            find_criteria: self.find_criteria,
        }
    }
}

impl OfferedService {
    /// Create a new `OfferedService` (for static deployments or after discovery).
    ///
    /// # Parameters
    /// - `find_criteria`: Original (instance_id, major_version) used in the find request.
    ///   Used for `StopFind` on drop. Pass `None` for static deployments.
    pub(crate) fn new(
        inner: Arc<RuntimeInner>,
        service_id: ServiceId,
        instance_id: InstanceId,
        major_version: u8,
        endpoint: SocketAddr,
        transport: crate::config::Transport,
        find_criteria: Option<(InstanceId, MajorVersion)>,
    ) -> Self {
        Self {
            inner,
            service_id,
            instance_id,
            major_version,
            endpoint,
            transport,
            find_criteria,
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
    /// use recentip::prelude::*;
    /// use recentip::handles::OfferedService;
    ///
    /// async fn concurrent_calls(proxy: OfferedService) {
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

    /// Create a new subscription builder.
    ///
    /// Use the builder to subscribe to one or more eventgroups that share
    /// the same network endpoint.
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
    /// while let Some(event) = subscription.next().await {
    ///     println!("Event: {:?}", event);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn new_subscription(&self) -> SubscriptionBuilder {
        SubscriptionBuilder::new(
            Arc::clone(&self.inner),
            self.service_id,
            self.instance_id,
            self.major_version,
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

    /// Get the endpoint address
    pub fn endpoint(&self) -> std::net::SocketAddr {
        self.endpoint
    }
}

impl Drop for OfferedService {
    fn drop(&mut self) {
        // Only send StopFind if this proxy was created via discovery
        // (static deployments don't have find_criteria)
        let Some((original_instance, original_version)) = self.find_criteria else {
            return;
        };

        // Use the original find criteria to match the key in find_requests.
        // This is critical: the Find command was registered with potentially wildcard
        // instance/version, so StopFind must use the same key.
        let cmd = Command::StopFind {
            service_id: self.service_id,
            instance_id: original_instance,
            major_version: original_version,
        };

        if let Err(e) = self.inner.cmd_tx.try_send(cmd) {
            match e {
                TrySendError::Full(_) => {
                    tracing::warn!(
                        "Failed to send StopFind for service {:04x}:{:04x}: \
                         command channel full. Find will stop on runtime shutdown.",
                        self.service_id.value(),
                        original_instance.value()
                    );
                }
                TrySendError::Closed(_) => {
                    // Runtime already shut down - this is expected during shutdown
                    tracing::debug!(
                        "StopFind skipped: runtime already shut down (service {:04x}:{:04x})",
                        self.service_id.value(),
                        original_instance.value()
                    );
                }
            }
        }
    }
}
