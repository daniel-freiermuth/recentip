//! Spec Compliance Tests
//!
//! This module contains tests that verify compliance with the RECENT/IP specification.
//! Each test function documents which requirement ID(s) it covers.
//!
//! Run with: cargo test --features turmoil --test compliance
//!
//! # Test Organization
//!
//! - `api_types`: Type system validation (ServiceId, MethodId, etc.)
//! - `integration`: Full client-server integration tests via turmoil
//! - `wire_capture`: Wire protocol compliance via raw socket capture
//! - `sd_async`: Service Discovery message format, offers, discovery
//! - `subscription_async`: Pub/Sub eventgroup subscription lifecycle
//! - `session_async`: Request ID, Session ID management
//!
//! # Requirement Traceability
//!
//! Each test documents which requirement(s) it covers using the `covers!()` macro.

#[path = "compliance/api_types.rs"]
mod api_types;

// Async compliance tests using turmoil
#[cfg(feature = "turmoil")]
#[path = "compliance/integration.rs"]
mod integration;

// Wire format tests - new modular organization
#[cfg(feature = "turmoil")]
#[path = "compliance/wire_format/mod.rs"]
mod wire_format;

// Server behavior tests - reactions to various inputs
#[cfg(feature = "turmoil")]
#[path = "compliance/server_behavior/mod.rs"]
mod server_behavior;

// Client behavior tests - client discovery, subscribe, RPC
#[cfg(feature = "turmoil")]
#[path = "compliance/client_behavior/mod.rs"]
mod client_behavior;

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
#[path = "compliance/subscription_multi_instance.rs"]
mod subscription_multi_instance;

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

// TCP binding tests (ignored until TCP transport implemented)
#[cfg(feature = "turmoil")]
#[path = "compliance/tcp_binding.rs"]
mod tcp_binding;

// UDP binding tests
#[cfg(feature = "turmoil")]
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

// fields contains turmoil integration tests for field getter/setter/notifier patterns
#[cfg(feature = "turmoil")]
#[path = "compliance/fields.rs"]
mod fields;

// error_scenarios contains turmoil integration tests for error handling behavior
#[cfg(feature = "turmoil")]
#[path = "compliance/error_scenarios.rs"]
mod error_scenarios;

// instances contains turmoil integration tests for service instance management
#[cfg(feature = "turmoil")]
#[path = "compliance/instances.rs"]
mod instances;

// multi_party contains turmoil integration tests for multi-client/multi-server scenarios
#[cfg(feature = "turmoil")]
#[path = "compliance/multi_party.rs"]
mod multi_party;

// service_instance_api contains turmoil integration tests for ServiceInstance typestate API
#[cfg(feature = "turmoil")]
#[path = "compliance/service_instance_api.rs"]
mod service_instance_api;

// SD pub/sub compliance tests for subscription lifecycle
#[cfg(feature = "turmoil")]
#[path = "compliance/sd_pubsub.rs"]
mod sd_pubsub;

// SD pub/sub offer→subscribe relationship tests
#[cfg(feature = "turmoil")]
#[path = "compliance/sd_pubsub_offer_subscribe.rs"]
mod sd_pubsub_offer_subscribe;

// Multi-protocol transport tests (TCP + UDP in same scenario)
#[cfg(feature = "turmoil")]
#[path = "compliance/multi_protocol.rs"]
mod multi_protocol;

// TCP pub/sub tests for TCP-specific event delivery
#[cfg(feature = "turmoil")]
#[path = "compliance/tcp_pubsub.rs"]
mod tcp_pubsub;

// Real network tests using tokio sockets (no turmoil simulation)
#[path = "compliance/real_network.rs"]
mod real_network;

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
