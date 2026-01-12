//! # Server-Side Handles
//!
//! Handles for server-side SOME/IP operations:
//! - [`ServiceOffering`]: Simple API for offering a service
//! - [`OfferBuilder`]: Builder for configuring service offerings (in runtime module)
//! - [`ServiceInstance`]: Advanced typestate API for bind/announce lifecycle
//! - [`EventHandle`]: Handle to send notification events
//! - [`EventBuilder`]: Builder for creating events with eventgroup membership
//! - [`Responder`]: Handle to reply to RPC requests
//! - [`ServiceEvent`]: Events received by an offered service

mod event;
mod offering;
mod responder;

pub use event::{EventBuilder, EventHandle};
pub use offering::ServiceOffering;
pub use responder::{Responder, ServiceEvent};
