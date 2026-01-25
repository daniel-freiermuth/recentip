//! `FindBuilder` for discovering SOME/IP services

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::error::{Error, Result};
use crate::net::{TcpListener, TcpStream, UdpSocket};
use crate::runtime::{Command, ServiceAvailability};
use crate::{InstanceId, MajorVersion, ServiceId};

use super::OfferedService;

/// Builder for finding a remote SOME/IP service.
///
/// Created via [`SomeIp::find()`](crate::SomeIp::find). Configure the find
/// criteria, then `.await` to discover the service. Returns the **first**
/// matching `OfferService` announcement.
///
/// # Find Criteria
///
/// Per SOME/IP-SD spec, find requests can use wildcards:
/// - **Service ID**: Always exact (required)
/// - **Instance ID**: Exact or `Any` (default: `Any`)
/// - **Major Version**: Exact or `Any` (default: `Any`)
/// - **Minor Version**: Always `Any` on wire (per spec recommendation)
///
/// # Example
///
/// ```no_run
/// use recentip::prelude::*;
///
/// const BRAKE_SERVICE_ID: u16 = 0x1234;
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     let runtime = recentip::configure().start().await?;
///
///     // Find any instance of the service with any major version
///     let proxy = runtime.find(BRAKE_SERVICE_ID).await?;
///
///     // Or with specific criteria:
///     let proxy = runtime.find(BRAKE_SERVICE_ID)
///         .instance(InstanceId::Id(1))
///         .major_version(1)
///         .await?;
///
///     Ok(())
/// }
/// ```
#[must_use]
pub struct FindBuilder<'a, U, T, L>
where
    U: UdpSocket,
    T: TcpStream,
    L: TcpListener<Stream = T>,
{
    runtime: &'a crate::SomeIp<U, T, L>,
    service_id: ServiceId,
    instance_id: InstanceId,
    major_version: MajorVersion,
}

impl<'a, U, T, L> FindBuilder<'a, U, T, L>
where
    U: UdpSocket,
    T: TcpStream,
    L: TcpListener<Stream = T>,
{
    /// Create a new find builder with default criteria.
    ///
    /// Defaults:
    /// - Instance: `Any`
    /// - Major version: `Any`
    pub(crate) fn new(runtime: &'a crate::SomeIp<U, T, L>, service_id: ServiceId) -> Self {
        Self {
            runtime,
            service_id,
            instance_id: InstanceId::Any,
            major_version: MajorVersion::Any,
        }
    }

    /// Set the instance ID to find.
    ///
    /// Default: `InstanceId::Any` (matches any instance)
    pub fn instance(mut self, instance: impl Into<InstanceId>) -> Self {
        self.instance_id = instance.into();
        self
    }

    /// Set the major version requirement.
    ///
    /// Default: `MajorVersion::Any` (matches any version)
    pub fn major_version(mut self, version: impl Into<MajorVersion>) -> Self {
        self.major_version = version.into();
        self
    }

    /// Execute the find request and wait for the first matching service.
    ///
    /// Sends a `FindService` SD message and waits for a matching `OfferService`.
    /// Returns the first discovered service that matches the criteria.
    ///
    /// # Errors
    ///
    /// - [`Error::NotAvailable`] - No matching service found (all find repetitions exhausted)
    /// - [`Error::RuntimeShutdown`] - `SomeIp` was shut down during discovery
    pub async fn await_discovery(self) -> Result<OfferedService> {
        // TODO maybe oneshot channel would be better?
        let (notify_tx, mut notify_rx) = mpsc::channel(1);

        // Register find request with the runtime
        self.runtime
            .inner()
            .cmd_tx
            .send(Command::Find {
                service_id: self.service_id,
                instance_id: self.instance_id,
                major_version: self.major_version,
                notify: notify_tx,
            })
            .await
            .map_err(|_| Error::RuntimeShutdown)?;

        // Wait for availability notification
        let Some(ServiceAvailability::Available {
            endpoint,
            transport,
            instance_id: discovered_instance_id,
            major_version,
        }) = notify_rx.recv().await
        else {
            // Channel closed - either runtime shut down or find request expired
            return Err(Error::NotAvailable);
        };

        Ok(OfferedService::new(
            Arc::clone(self.runtime.inner()),
            self.service_id,
            InstanceId::Id(discovered_instance_id),
            major_version,
            endpoint,
            transport,
        ))
    }
}

/// Implement `IntoFuture` so the builder can be directly `.await`ed.
impl<'a, U, T, L> std::future::IntoFuture for FindBuilder<'a, U, T, L>
where
    U: UdpSocket + 'a,
    T: TcpStream + Sync + 'a,
    L: TcpListener<Stream = T> + 'a,
{
    type Output = Result<OfferedService>;
    type IntoFuture =
        std::pin::Pin<Box<dyn std::future::Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.await_discovery())
    }
}
