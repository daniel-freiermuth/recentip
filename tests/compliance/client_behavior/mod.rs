//! Client Behavior Tests
//!
//! Tests where the library acts as the **client** discovering and consuming services,
//! and raw sockets simulate server behavior.
//!
//! # Test Setup
//! - Library: Discovers services, subscribes, makes RPC calls
//! - Raw sockets: Simulate servers sending offers and ACKs
//!
//! # Modules
//! - `subscribe_format` - Subscribe message format verification
//! - `subscribe_nack` - Subscribe NACK handling
//! - `rpc_port` - SD vs RPC port separation, TCP connection timing

pub mod helpers;
pub mod rpc_port;
pub mod subscribe_format;
pub mod subscribe_nack;
pub mod subscription_handle;
