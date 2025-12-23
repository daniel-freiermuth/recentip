//! Spec Compliance Tests
//!
//! This module contains tests that verify compliance with the RECENT/IP specification.
//! Each test function documents which requirement ID(s) it covers.
//!
//! Run with: cargo test --test compliance
//!
//! # Test Organization
//!
//! - `api_types`: Type system validation (ServiceId, MethodId, etc.)
//! - `wire_format`: Wire protocol compliance via SimulatedNetwork
//! - `service_discovery`: SOME/IP-SD message format, entries, reboot detection
//! - `transport_protocol`: SOME/IP-TP segmentation and reassembly
//! - `error_handling`: Return codes and message type handling
//! - `subscription`: Pub/Sub eventgroup entries and lifecycle
//! - `session_handling`: Request ID, Client ID, Session ID management
//! - `message_types`: Message type field values and transitions
//! - `version_handling`: Protocol and interface version validation
//!
//! # Test Summary (as of last update)
//!
//! | Module             | Passing | Ignored |
//! |--------------------|---------|---------|
//! | api_types          | 16      | 0       |
//! | wire_format        | 22      | 6       |
//! | service_discovery  | 18      | 14      |
//! | transport_protocol | 28      | 10      |
//! | error_handling     | 24      | 10      |
//! | subscription       | 28      | 6       |
//! | session_handling   | 25      | 6       |
//! | message_types      | 20      | 7       |
//! | version_handling   | 21      | 4       |
//! | **Total**          | **202** | **63**  |
//!
//! # Requirement Traceability
//!
//! Each test documents which requirement(s) it covers using the `covers!()` macro.
//! The compliance matrix in `COMPLIANCE.md` maps tests to requirements.

#[path = "compliance/api_types.rs"]
mod api_types;

#[path = "compliance/wire_format.rs"]
mod wire_format;

#[path = "compliance/service_discovery.rs"]
mod service_discovery;

#[path = "compliance/transport_protocol.rs"]
mod transport_protocol;

#[path = "compliance/error_handling.rs"]
mod error_handling;

#[path = "compliance/subscription.rs"]
mod subscription;

#[path = "compliance/session_handling.rs"]
mod session_handling;

#[path = "compliance/message_types.rs"]
mod message_types;

#[path = "compliance/version_handling.rs"]
mod version_handling;

/// Macro to document which requirements a test covers.
/// This is a no-op at runtime; used for traceability documentation.
#[macro_export]
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {};
}

/// Macro to document why a requirement is NOT tested.
#[allow(unused_macros)]
macro_rules! not_tested {
    ($req:ident, $reason:literal) => {};
}
