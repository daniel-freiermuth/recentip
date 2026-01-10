//! # Server-Side Handles
//!
//! Handles for server-side SOME/IP operations:
//! - [`OfferingHandle`]: Simple API for offering a service
//! - [`OfferBuilder`]: Builder for configuring service offerings (in runtime module)
//! - [`ServiceInstance`]: Advanced typestate API for bind/announce lifecycle
//! - [`Responder`]: Handle to reply to RPC requests
//! - [`ServiceEvent`]: Events received by an offered service

mod instance;
mod offering;
mod responder;

pub use instance::{Announced, Bound, ServiceInstance};
pub use offering::OfferingHandle;
pub use responder::{Responder, ServiceEvent};
