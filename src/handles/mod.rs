//! # Handle Types for SOME/IP Communication
//!
//! This module provides the **user-facing API** for interacting with the runtime.
//! All SOME/IP operations go through handles, which internally send commands to
//! the runtime's event loop.
//!
//! ## Handle Types Overview
//!
//! | Handle | Role | Notes |
//! |--------|------|-------|
//! | [`OfferedService`] | Client: call methods, subscribe to events | Created via `SomeIp::find()` |
//! | [`ServiceOffering`] | Server: receive requests, send responses | — |
//! | `ServiceInstance` | Server (advanced): typestate for bind/announce | `Bound` → `Announced` |
//! | [`Subscription`] | Client: receive events from a subscribed eventgroup | — |
//! | [`Responder`] | Server: reply to a specific RPC request | Consumed on reply |
//!
//! ## Client-Side Pattern
//!
//! ```no_run
//! use recentip::prelude::*;
//!
//! const MY_SERVICE_ID: u16 = 0x1234;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let runtime = recentip::configure().start().await?;
//!
//!     // 1. Find the service (waits for discovery)
//!     let proxy = runtime.find(MY_SERVICE_ID).await?;
//!
//!     // 2. Call methods
//!     let method_id = MethodId::new(0x0001).unwrap();
//!     let response = proxy.call(method_id, b"payload").await?;
//!
//!     // 3. Subscribe to events
//!     let eventgroup = EventgroupId::new(0x0001).unwrap();
//!     let mut subscription = proxy
//!         .new_subscription()
//!         .eventgroup(eventgroup)
//!         .subscribe()
//!         .await?;
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
//! use recentip::prelude::*;
//! use recentip::handles::ServiceEvent;
//!
//! const MY_SERVICE_ID: u16 = 0x1234;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let runtime = recentip::configure().start().await?;
//!
//!     // 1. Offer a service
//!     let mut offering = runtime.offer(MY_SERVICE_ID, InstanceId::Id(1))
//!         .version(1, 0)
//!         .udp()
//!         .start()
//!         .await?;
//!
//!     // 2. Handle incoming events
//!     while let Some(event) = offering.next().await {
//!         match event {
//!             ServiceEvent::Call { method, payload, responder, .. } => {
//!                 responder.reply(b"response")?;
//!             }
//!             ServiceEvent::Subscribe { .. } => {  }
//!             _ => {}
//!         }
//!     }
//!     Ok(())
//! }
//! ```
//!
//! ## Server-Side Pattern (Advanced Typestate)
//!
//! For finer control over the bind/announce lifecycle (TODO: `ServiceInstance` API):
//!
//! ```ignore
//! use recentip::prelude::*;
//! use recentip::handles::{ServiceInstance, Bound, Announced};
//! use recentip::Transport;
//!
//! const MY_SERVICE_ID: u16 = 0x1234;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let runtime = recentip::configure().start().await?;
//!
//!     // 1. Bind (opens socket, but no SD announcement)
//!     let instance: ServiceInstance<Bound> = runtime.bind(
//!         MY_SERVICE_ID,
//!         InstanceId::Id(1),
//!         (1, 0),  // version
//!         Transport::Udp,
//!     ).await?;
//!
//!     // 2. Start announcing (transitions to Announced)
//!     let instance: ServiceInstance<Announced> = instance.announce().await?;
//!
//!     // 3. Now handle requests...
//!
//!     // 4. Stop announcing (transitions back to Bound)
//!     let instance: ServiceInstance<Bound> = instance.stop_announcing().await?;
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
//! - `ServiceInstance<Bound>` can only call `.announce()` or handle static requests
//! - `ServiceInstance<Announced>` can call `.stop_announcing()` or handle SD requests
//!
//! This prevents common bugs like:
//! - Announcing a service that hasn't bound to a socket
//!
//! ## Thread Safety
//!
//! All handles are `Clone` and can be shared across tokio tasks. They internally
//! hold an `Arc<RuntimeInner>` and communicate via channels.

pub mod client;
pub mod runtime;
pub mod server;

// Re-export all public types for convenient access
pub use client::{
    FindBuilder, OfferedService, StaticEventListener, Subscription, SubscriptionBuilder,
};
pub use runtime::{OfferBuilder, SomeIp};
pub use server::{EventBuilder, EventHandle, Responder, ServiceEvent, ServiceOffering};
