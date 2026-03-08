//! Client-side proxy for calling methods and subscribing to events.
//!
//! See [`OfferedService`] for the main type.

use std::net::SocketAddrV4;
use std::sync::Arc;

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::oneshot;

use crate::config::{PortSpec, TransportPolicy, TransportSelection};
use crate::error::{Error, Result};
use crate::handles::runtime::RuntimeInner;
use crate::runtime::Command;
use crate::{
    EventgroupId, InstanceId, MajorVersion, MethodId, OfferedEndpoints, Response, ServiceId,
    Transport,
};

use super::SubscriptionBuilder;

/// Client-side proxy to a discovered SOME/IP service.
///
/// Obtained via [`SomeIp::find`](crate::SomeIp::find). Provides methods for:
/// - **RPC calls**: [`call`](Self::call), [`fire_and_forget`](Self::fire_and_forget)
/// - **Event subscriptions**: [`subscribe`](Self::subscribe)
///
/// # Example
///
/// ```no_run
/// use recentip::prelude::*;
///
/// # #[tokio::main]
/// # async fn main() -> Result<()> {
/// let runtime = recentip::configure()
///     .sd_unicast("192.168.1.100".parse().unwrap())
///     .sd_multicast_group("239.255.255.250".parse().unwrap())
///     .start().await?;
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
    remote_endpoints: OfferedEndpoints,
    remote_sd_endpoint: SocketAddrV4,
    /// Transport policy for this proxy (copied from global at creation, overridable).
    transport_policy: TransportPolicy,
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
            find_criteria: self.find_criteria,
            remote_endpoints: self.remote_endpoints.clone(),
            remote_sd_endpoint: self.remote_sd_endpoint,
            transport_policy: self.transport_policy.clone(),
        }
    }
}

impl OfferedService {
    /// Create a new `OfferedService` for a service at a known endpoint (static deployment).
    ///
    /// This bypasses service discovery and creates a proxy that directly communicates
    /// with the specified endpoint. Useful for:
    /// - Static/pre-configured deployments where endpoints are known at compile time
    /// - Testing without SD machinery
    /// - Scenarios where SD is disabled (`feat_req_someipsd_444`)
    ///
    /// # Arguments
    ///
    /// - `runtime`: Reference to the SOME/IP runtime
    /// - `service_id`: Service ID (required, e.g., 0x1234)
    /// - `instance_id`: Instance ID (required)
    /// - `major_version`: Major version of the service interface
    /// - `offered_endpoints`: Offered transport endpoints of the service
    /// - `sd_endpoint`: SD unicast endpoint of the peer (for subscribe messages)
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    /// use recentip::OfferedEndpoints;
    /// use std::net::{SocketAddrV4, Ipv4Addr};
    ///
    /// # async fn example() -> Result<()> {
    /// let runtime = recentip::configure()
    ///     .sd_unicast("192.168.1.100".parse().unwrap())
    ///     .sd_multicast_group("239.255.255.250".parse().unwrap())
    ///     .start().await?;
    ///
    /// // Connect to a service at a known TCP endpoint
    /// let tcp_endpoint = SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 10), 30501);
    /// let sd_endpoint  = SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 10), 30490);
    /// let proxy = OfferedService::new(
    ///     &runtime,
    ///     ServiceId::new(0x1234).unwrap(),
    ///     InstanceId::Id(1),
    ///     1,
    ///     OfferedEndpoints::TcpOnly(tcp_endpoint),
    ///     sd_endpoint,
    /// );
    ///
    /// // Use the proxy like any other
    /// let response = proxy.call(MethodId::new(0x0001).unwrap(), b"request").await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// - [`SomeIp::find`](crate::SomeIp::find) - For dynamic service discovery
    pub fn new<U, T, L>(
        runtime: &crate::SomeIp<U, T, L>,
        service_id: ServiceId,
        instance_id: InstanceId,
        major_version: u8,
        offered_endpoints: OfferedEndpoints,
        sd_endpoint: SocketAddrV4,
    ) -> Self
    where
        U: crate::net::UdpSocket,
        T: crate::net::TcpStream,
        L: crate::net::TcpListener<Stream = T>,
    {
        Self::from_inner(
            Arc::clone(runtime.inner()),
            service_id,
            instance_id,
            major_version,
            None, // No find_criteria for static deployments
            offered_endpoints,
            sd_endpoint,
        )
    }

    /// Override the transport policy for this proxy.
    ///
    /// By default the policy is copied from the global
    /// [`RuntimeConfig::transport_policy`](crate::config::RuntimeConfig::transport_policy)
    /// at creation time. Use this method to apply a per-service policy without
    /// changing the global setting.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    /// use recentip::config::{TransportPolicy, TransportPreference};
    ///
    /// # async fn example(proxy: OfferedService) -> OfferedService {
    /// // Force TCP for this specific service
    /// proxy.with_transport_policy(
    ///     TransportPolicy::new(vec![TransportPreference::tcp()])
    /// )
    /// # }
    /// ```
    #[must_use]
    pub fn with_transport_policy(mut self, policy: TransportPolicy) -> Self {
        self.transport_policy = policy;
        self
    }

    /// Internal constructor used by the find operation.
    ///
    /// # Parameters
    /// - `find_criteria`: Original (`instance_id`, `major_version`) used in the find request.
    ///   Used for `StopFind` on drop. Pass `None` for static deployments.
    pub(crate) fn from_inner(
        inner: Arc<RuntimeInner>,
        service_id: ServiceId,
        instance_id: InstanceId,
        major_version: u8,
        find_criteria: Option<(InstanceId, MajorVersion)>,
        offered_endpoints: OfferedEndpoints,
        sd_endpoint: SocketAddrV4,
    ) -> Self {
        let transport_policy = inner.config.transport_policy.clone();
        Self {
            inner,
            service_id,
            instance_id,
            major_version,
            find_criteria,
            remote_endpoints: offered_endpoints,
            remote_sd_endpoint: sd_endpoint,
            transport_policy,
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
        self.effective_transport().transport
    }

    /// Call a method and wait for the response.
    ///
    /// The payload is copied internally. For large payloads where zero-copy
    /// matters, use `bytes::Bytes` directly.
    ///
    /// # Errors
    ///
    /// Returns an error if the runtime is shut down or the call fails.
    pub async fn call(&self, method: MethodId, payload: impl AsRef<[u8]>) -> Result<Response> {
        let payload_bytes = bytes::Bytes::copy_from_slice(payload.as_ref());
        let (response_tx, response_rx) = oneshot::channel();

        let sel = self.effective_transport();

        self.inner
            .cmd_tx
            .send(Command::Call {
                service_id: self.service_id,
                method_id: method.value(),
                payload: payload_bytes,
                response: response_tx,
                target_endpoint: sel.remote_endpoint,
                target_transport: sel.transport,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        response_rx.await.map_err(|_| Error::RuntimeShutdown)?
    }

    /// Fire and forget - send a request without expecting a response.
    ///
    /// # Errors
    ///
    /// Returns [`Error::RuntimeShutdown`] if the runtime has been dropped.
    pub async fn fire_and_forget(&self, method: MethodId, payload: &[u8]) -> Result<()> {
        let payload_bytes = bytes::Bytes::copy_from_slice(payload);

        let sel = self.effective_transport();

        self.inner
            .cmd_tx
            .send(Command::FireAndForget {
                service_id: self.service_id,
                method_id: method.value(),
                payload: payload_bytes,
                target_endpoint: sel.remote_endpoint,
                target_transport: sel.transport,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        Ok(())
    }

    fn effective_transport(&self) -> TransportSelection {
        // Try the transport policy first; fall back to any available endpoint.
        if let Some(sel) = self.transport_policy.select(&self.remote_endpoints) {
            return sel;
        }
        // Policy found no match — use whatever the server offers with an ephemeral port.
        let (remote_endpoint, transport) = match &self.remote_endpoints {
            crate::OfferedEndpoints::UdpOnly(addr) => (*addr, Transport::Udp),
            crate::OfferedEndpoints::TcpOnly(addr) => (*addr, Transport::Tcp),
            crate::OfferedEndpoints::Both { udp, .. } => (*udp, Transport::Udp),
        };
        TransportSelection {
            remote_endpoint,
            transport,
            local_port: PortSpec::Any,
        }
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
        let sel = self.effective_transport();
        SubscriptionBuilder::new(
            Arc::clone(&self.inner),
            self.service_id,
            self.instance_id,
            self.major_version,
            eventgroup,
            sel.transport,
            sel.remote_endpoint,
            self.remote_sd_endpoint,
            sel.local_port,
        )
    }

    /// Get the service ID
    pub const fn service_id(&self) -> ServiceId {
        self.service_id
    }

    /// Get the instance ID
    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    /// Get the major version
    pub const fn major_version(&self) -> u8 {
        self.major_version
    }

    /// Get the endpoint address
    pub fn endpoint(&self) -> std::net::SocketAddrV4 {
        self.effective_transport().remote_endpoint
    }

    /// Check if the service offer is currently alive (TTL not expired).
    ///
    /// Returns `true` if the service was recently discovered or is still being offered
    /// (TTL has not expired). Returns `false` if:
    /// - The service has never been discovered via SD
    /// - The service's TTL has expired without renewal
    ///
    /// This works for both discovered proxies (via `find()`) and static proxies
    /// (via `new()`) - it checks if the actual service at the endpoint is currently
    /// offering itself via SD.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use recentip::prelude::*;
    ///
    /// # async fn example(proxy: OfferedService) -> Result<()> {
    /// if proxy.is_offer_alive() {
    ///     // Service is still being offered
    ///     let response = proxy.call(MethodId::new(0x01).unwrap(), b"request").await?;
    /// } else {
    ///     // Service TTL expired or never discovered
    ///     println!("Service is no longer available");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn is_offer_alive(&self) -> bool {
        let key = crate::runtime::state::ServiceKey::new(
            self.service_id,
            self.instance_id,
            self.major_version,
        );
        self.inner
            .discovered
            .get(&key)
            .is_some_and(|svc| svc.is_alive())
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
