//! Server behavior tests (Library as Server)
//!
//! Tests where the library acts as the **server** offering services,
//! and raw sockets simulate client behavior.
//!
//! # Test Setup
//! - Library: Offers services, responds to subscribes
//! - Raw sockets: Simulate clients sending subscribe messages

pub mod event_dedup;
mod helpers;
pub mod offer_timing;
pub mod sd_session;
pub mod subscription_nack;
pub mod transport_mismatch;
