//! # Examples
//!
//! Compile-checked examples for common `RecentIP` use cases.
//!
//! These examples are tested as part of `cargo test --doc` to ensure they stay
//! up-to-date with the API.
//!
//! ## Contents
//!
//! - [`quickstart`] - Minimal client, server, pub/sub examples
//! - [`rpc`] - Request/response and fire-and-forget patterns
//! - [`pubsub`] - Events, eventgroups, and subscriptions
//! - [`transport`] - UDP, TCP, magic cookies, and configuration
//! - [`monitoring`] - Service discovery monitoring and lifecycle

#[doc = include_str!("../docs/examples/quickstart.md")]
pub mod quickstart {}

#[doc = include_str!("../docs/examples/rpc.md")]
pub mod rpc {}

#[doc = include_str!("../docs/examples/pubsub.md")]
pub mod pubsub {}

#[doc = include_str!("../docs/examples/transport.md")]
pub mod transport {}

#[doc = include_str!("../docs/examples/monitoring.md")]
pub mod monitoring {}
