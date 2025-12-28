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

#[cfg(feature = "turmoil")]
#[path = "compliance/service_discovery.rs"]
mod service_discovery;

// transport_protocol contains unit tests for TP header parsing (always run)
// and integration tests marked #[ignore] (require TP implementation)
#[path = "compliance/transport_protocol.rs"]
mod transport_protocol;

// error_handling contains unit tests (always) + turmoil integration tests
#[cfg(feature = "turmoil")]
#[path = "compliance/error_handling.rs"]
mod error_handling;

#[cfg(feature = "turmoil")]
#[path = "compliance/subscription.rs"]
mod subscription;

#[cfg(feature = "turmoil")]
#[path = "compliance/session_handling.rs"]
mod session_handling;

// message_types contains unit tests (always) + turmoil integration tests + proptest
#[path = "compliance/message_types.rs"]
mod message_types;

// version_handling contains turmoil integration tests only
// (unit tests + proptests moved to src/wire.rs)
#[cfg(feature = "turmoil")]
#[path = "compliance/version_handling.rs"]
mod version_handling;

#[path = "compliance/tcp_binding.rs"]
mod tcp_binding;

#[path = "compliance/udp_binding.rs"]
mod udp_binding;

// rpc_flow contains turmoil integration tests for request/response patterns
#[cfg(feature = "turmoil")]
#[path = "compliance/rpc_flow.rs"]
mod rpc_flow;

// events contains turmoil integration tests for pub/sub event delivery
#[cfg(feature = "turmoil")]
#[path = "compliance/events.rs"]
mod events;

#[path = "compliance/fields.rs"]
mod fields;

#[path = "compliance/error_scenarios.rs"]
mod error_scenarios;

#[path = "compliance/session_edge_cases.rs"]
mod session_edge_cases;

#[path = "compliance/instances.rs"]
mod instances;

#[path = "compliance/multi_party.rs"]
mod multi_party;

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
