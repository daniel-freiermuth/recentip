//! # Client-Side Handles
//!
//! Handles for client-side SOME/IP operations:
//! - [`FindBuilder`]: Builder for discovering services
//! - [`OfferedService`]: Proxy to call methods on a remote service
//! - [`SubscriptionBuilder`]: Builder for creating subscriptions
//! - [`Subscription`]: Handle to receive events from subscribed eventgroups
//! - [`StaticEventListener`]: Listen for events without Service Discovery

mod find;
mod proxy;
mod subscription;

pub use find::FindBuilder;
pub use proxy::OfferedService;
pub use subscription::{StaticEventListener, Subscription, SubscriptionBuilder};
