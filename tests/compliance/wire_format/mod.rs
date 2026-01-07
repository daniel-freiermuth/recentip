//! Wire Format Tests
//!
//! These tests verify that SOME/IP and SOME/IP-SD messages have the correct
//! byte-level encoding as specified by the protocol specifications.
//!
//! The tests are organized into submodules:
//! - `sd_format`: SD message encoding (header constants, entry types, TTL fields)
//! - `rpc_format`: RPC message encoding (REQUEST, RESPONSE, REQUEST_NO_RETURN types)
//! - `subscription_format`: Subscription entry encoding (Subscribe, SubscribeAck, StopSubscribe)
//!
//! ## What belongs here
//! Tests that verify **byte-level encoding** of messages, such as:
//! - Service ID, Method ID positions and values
//! - Message type byte values (0x00 = REQUEST, 0x80 = RESPONSE, etc.)
//! - Entry type encoding (0x01 = OfferService, 0x06 = SubscribeEventgroup, etc.)
//! - TTL field encoding (24-bit big-endian)
//! - Big-endian encoding of multi-byte fields
//!
//! ## What does NOT belong here
//! Tests that verify **behavior** (how the implementation reacts to inputs)
//! belong in `server_behavior` or `client_behavior`:
//! - NACK for unknown service/instance/eventgroup
//! - Reboot flag handling
//! - Transport mismatch handling
//! - TCP connection timing requirements

pub mod helpers;
pub mod rpc_format;
pub mod sd_format;
pub mod subscription_format;
