//! Client-side proxy for calling methods and subscribing to events.
//!
//! See [`OfferedService`] for the main type.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::oneshot;

use crate::error::{Error, Result};
use crate::handles::runtime::RuntimeInner;
use crate::runtime::Command;
use crate::{EventgroupId, InstanceId, MajorVersion, MethodId, Response, ServiceId};

use super::SubscriptionBuilder;

/// Client-side proxy to a discovered SOME/IP service.
///
/// Obtained via [`SomeIp::find`](crate::SomeIp::find). Provides methods for:
/// - **RPC calls**: [`call`](Self::call), [`fire_and_forget`](Self::fire_and_forget)
/// - **Event subscriptions**: [`new_subscription`](Self::new_subscription)
///
/// # Example
///
/// ```no_run
/// use recentip::prelude::*;
///
/// # #[tokio::main]
/// # async fn main() -> Result<()> {
/// let runtime = recentip::configure().start().await?;
///
/// // Discover a service
/// let proxy = runtime.find(0x1234).await?;
///
/// // Call a method
/// let response = proxy.call(MethodId::new(0x01).unwrap(), b"request").await?;
///
/// // Subscribe to events
/// let mut sub = proxy.subscribe(EventgroupId::new(1).unwrap()).await?;
///
/// while let Some(event) = sub.next().await {
///     println!("Event 0x{:04X}: {:?}", event.event_id.value(), event.payload);
/// }
/// # Ok(())
/// # }
/// ```
///
/// # Lifecycle
///
/// When dropped, sends `StopFind` to the runtime (for proxies created via discovery).
/// This allows the runtime to stop listening for offers of this service.
pub struct OfferedService {
    inner: Arc<RuntimeInner>,
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: u8,
    endpoint: SocketAddr,
    transport: crate::config::Transport,
    /// Original find criteria - used for `StopFind` on drop
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
    /// - `find_criteria`: Original (`instance_id`, `major_version`) used in the find request.
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
    /// The payload is copied internally. For large payloads where zero-copy
    /// matters, use `bytes::Bytes` directly.
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

    /// Subscribe to an eventgroup (or multiple with `.and()`).
    ///
    /// Returns a [`SubscriptionBuilder`] that can be awaited directly,
    /// or chained with `.and()` to add more eventgroups.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    ///
    /// # async fn example(proxy: recentip::handles::OfferedService) -> Result<()> {
    /// // Single eventgroup
    /// let mut sub = proxy.subscribe(EventgroupId::new(1).unwrap()).await?;
    ///
    /// // Multiple eventgroups
    /// let eg1 = EventgroupId::new(1).unwrap();
    /// let eg2 = EventgroupId::new(2).unwrap();
    /// let mut sub = proxy.subscribe(eg1).and(eg2).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn subscribe(&self, eventgroup: EventgroupId) -> SubscriptionBuilder {
        SubscriptionBuilder::new(
            Arc::clone(&self.inner),
            self.service_id,
            self.instance_id,
            self.major_version,
            eventgroup,
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
