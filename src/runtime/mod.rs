//! # `SomeIp` Implementation (Internal)
//!
//! This module contains the internal implementation of the SOME/IP runtime.
//! It is `pub(crate)` — internal to the library.
//!
//! ## Module Structure
//!
//! - [`command`]: Command enum for handle→runtime communication
//! - [`state`]: `RuntimeState` and internal data structures
//! - [`client`]: Client-side handlers (find, call, subscribe UDP)
//! - [`client_concurrent`]: Concurrent TCP Subscribe command handling
//! - [`server`]: Server-side handlers (offer, notify, respond)
//! - [`sd`]: Service Discovery message handlers and builders
//! - [`event_loop`]: The main runtime task and event loop

pub mod client;
pub mod client_concurrent;
pub mod command;
pub mod event_loop;
pub mod sd;
pub mod server;
pub mod state;

// Re-export commonly used types for internal use
pub use command::SdEvent;
pub use command::{Command, ServiceAvailability, ServiceRequest};
