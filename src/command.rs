//! Commands sent from handles to the runtime task.
//!
//! Contains the `Command` enum, `ServiceAvailability`, and `ServiceRequest`.

use std::net::SocketAddr;

use bytes::Bytes;
use tokio::sync::{mpsc, oneshot};

use crate::config::MethodConfig;
use crate::error::Result;
use crate::{InstanceId, ServiceId};

/// Commands sent from handles to the runtime task
pub(crate) enum Command {
    /// Find a service
    Find {
        service_id: ServiceId,
        instance_id: InstanceId,
        notify: mpsc::Sender<ServiceAvailability>,
    },
    /// Stop finding a service
    StopFind {
        service_id: ServiceId,
        instance_id: InstanceId,
    },
    /// Offer a service
    Offer {
        service_id: ServiceId,
        instance_id: InstanceId,
        major_version: u8,
        minor_version: u32,
        method_config: MethodConfig,
        response: oneshot::Sender<Result<mpsc::Receiver<ServiceRequest>>>,
    },
    /// Stop offering a service
    StopOffer {
        service_id: ServiceId,
        instance_id: InstanceId,
    },
    /// Call a method
    Call {
        service_id: ServiceId,
        instance_id: InstanceId,
        method_id: u16,
        payload: Bytes,
        response: oneshot::Sender<Result<crate::Response>>,
        /// For static deployments: pre-configured endpoint
        target_endpoint: Option<SocketAddr>,
    },
    /// Fire-and-forget call (no response expected)
    FireAndForget {
        service_id: ServiceId,
        instance_id: InstanceId,
        method_id: u16,
        payload: Bytes,
        /// For static deployments: pre-configured endpoint
        target_endpoint: Option<SocketAddr>,
    },
    /// Subscribe to an eventgroup
    Subscribe {
        service_id: ServiceId,
        instance_id: InstanceId,
        eventgroup_id: u16,
        events: mpsc::Sender<crate::Event>,
        response: oneshot::Sender<Result<()>>,
    },
    /// Unsubscribe from an eventgroup
    Unsubscribe {
        service_id: ServiceId,
        instance_id: InstanceId,
        eventgroup_id: u16,
    },
    /// Send a notification event (server-side)
    Notify {
        service_id: ServiceId,
        instance_id: InstanceId,
        eventgroup_id: u16,
        event_id: u16,
        payload: Bytes,
    },
    /// Send a notification to static subscribers only (no SD)
    #[allow(dead_code)]
    NotifyStatic {
        service_id: ServiceId,
        instance_id: InstanceId,
        #[allow(dead_code)]
        eventgroup_id: u16,
        event_id: u16,
        payload: Bytes,
        targets: Vec<SocketAddr>,
    },
    /// Bind a service (listen on socket, no SD announcement)
    Bind {
        service_id: ServiceId,
        instance_id: InstanceId,
        major_version: u8,
        minor_version: u32,
        method_config: MethodConfig,
        response: oneshot::Sender<Result<mpsc::Receiver<ServiceRequest>>>,
    },
    /// Start announcing a bound service via SD
    StartAnnouncing {
        service_id: ServiceId,
        instance_id: InstanceId,
        response: oneshot::Sender<Result<()>>,
    },
    /// Stop announcing a service (keeps socket open)
    StopAnnouncing {
        service_id: ServiceId,
        instance_id: InstanceId,
        response: oneshot::Sender<Result<()>>,
    },
    /// Create a static proxy (pre-configured address, no SD)
    #[allow(dead_code)]
    FindStatic {
        service_id: ServiceId,
        instance_id: InstanceId,
        endpoint: SocketAddr,
        notify: mpsc::Sender<ServiceAvailability>,
    },
    /// Query if there are subscribers for an eventgroup
    HasSubscribers {
        service_id: ServiceId,
        instance_id: InstanceId,
        eventgroup_id: u16,
        response: oneshot::Sender<bool>,
    },
    /// Listen for static events (pre-configured, no SD)
    ListenStatic {
        service_id: ServiceId,
        instance_id: InstanceId,
        eventgroup_id: u16,
        /// Port to bind for receiving events
        port: u16,
        /// Channel to send received events
        events: mpsc::Sender<crate::Event>,
        response: oneshot::Sender<Result<()>>,
    },
    /// Shutdown the runtime
    #[allow(dead_code)]
    Shutdown,
}

/// Service availability notification
#[derive(Debug, Clone)]
pub(crate) enum ServiceAvailability {
    Available {
        endpoint: SocketAddr,
        instance_id: u16,
    },
    Unavailable,
}

/// Service request (for offerings)
pub(crate) enum ServiceRequest {
    MethodCall {
        method_id: u16,
        payload: Bytes,
        client: SocketAddr,
        response: oneshot::Sender<Result<Bytes>>,
    },
    FireForget {
        method_id: u16,
        payload: Bytes,
        client: SocketAddr,
    },
    Subscribe {
        eventgroup_id: u16,
        client: SocketAddr,
        response: oneshot::Sender<Result<bool>>,
    },
    Unsubscribe {
        eventgroup_id: u16,
        client: SocketAddr,
    },
}
