//! Version Handling Compliance Tests
//!
//! Tests for SOME/IP protocol version and interface version fields.
//!
//! # Protocol Version (feat_req_recentip_300)
//! - Byte offset 12 in header
//! - Current version is 0x01
//! - Mismatch should be rejected
//!
//! # Interface Version (feat_req_recentip_278)
//! - Byte offset 13 in header  
//! - Configured per service
//! - Client/server must match major version

#![allow(dead_code)]

// ============================================================================
// Protocol Version Constants
// ============================================================================

/// Current SOME/IP protocol version
pub const PROTOCOL_VERSION: u8 = 0x01;

/// Protocol version byte position in header
pub const PROTOCOL_VERSION_OFFSET: usize = 12;

/// Interface version byte position in header
pub const INTERFACE_VERSION_OFFSET: usize = 13;

// ============================================================================
// Version Validation Types
// ============================================================================

/// Represents a semantic version for interface compatibility
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceVersion {
    /// Major version - incompatible changes
    pub major: u8,
    /// Minor version - backwards compatible changes
    pub minor: u32,
}

impl InterfaceVersion {
    /// Create a new interface version
    pub fn new(major: u8, minor: u32) -> Self {
        InterfaceVersion { major, minor }
    }
    
    /// Check if this version is compatible with another version
    /// 
    /// Compatible means same major version, and our minor >= required minor
    pub fn is_compatible_with(&self, required: &InterfaceVersion) -> bool {
        self.major == required.major && self.minor >= required.minor
    }
    
    /// Check if exact match (used for strict mode)
    pub fn matches_exactly(&self, other: &InterfaceVersion) -> bool {
        self.major == other.major && self.minor == other.minor
    }
    
    /// Get the major version byte for wire format
    pub fn wire_major(&self) -> u8 {
        self.major
    }
}

/// Protocol version validation result
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionError {
    /// Protocol version mismatch
    ProtocolMismatch { expected: u8, found: u8 },
    /// Interface major version mismatch
    InterfaceMajorMismatch { expected: u8, found: u8 },
    /// Interface minor version too old
    InterfaceMinorTooOld { minimum: u32, found: u32 },
}

/// Validate protocol version byte from header
pub fn validate_protocol_version(version_byte: u8) -> std::result::Result<(), VersionError> {
    if version_byte == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(VersionError::ProtocolMismatch {
            expected: PROTOCOL_VERSION,
            found: version_byte,
        })
    }
}

/// Validate interface version from header against expected
pub fn validate_interface_version(
    header_major: u8,
    expected: &InterfaceVersion,
) -> std::result::Result<(), VersionError> {
    if header_major == expected.major {
        Ok(())
    } else {
        Err(VersionError::InterfaceMajorMismatch {
            expected: expected.major,
            found: header_major,
        })
    }
}

// ============================================================================
// Unit Tests - Protocol Version
// ============================================================================

/// [feat_req_recentip_300] Protocol version is 0x01
#[test]
fn protocol_version_is_0x01() {
    assert_eq!(PROTOCOL_VERSION, 0x01);
}

/// [feat_req_recentip_300] Protocol version is at byte offset 12
#[test]
fn protocol_version_offset_is_12() {
    assert_eq!(PROTOCOL_VERSION_OFFSET, 12);
}

/// [feat_req_recentip_300] Valid protocol version is accepted
#[test]
fn protocol_version_valid_accepted() {
    assert!(validate_protocol_version(0x01).is_ok());
}

/// [feat_req_recentip_300] Invalid protocol version is rejected
#[test]
fn protocol_version_invalid_rejected() {
    // Version 0x00 is invalid
    let result = validate_protocol_version(0x00);
    assert!(matches!(
        result,
        Err(VersionError::ProtocolMismatch { expected: 0x01, found: 0x00 })
    ));
    
    // Version 0x02 is invalid (future version)
    let result = validate_protocol_version(0x02);
    assert!(matches!(
        result,
        Err(VersionError::ProtocolMismatch { expected: 0x01, found: 0x02 })
    ));
    
    // Version 0xFF is invalid
    let result = validate_protocol_version(0xFF);
    assert!(matches!(
        result,
        Err(VersionError::ProtocolMismatch { expected: 0x01, found: 0xFF })
    ));
}

// ============================================================================
// Unit Tests - Interface Version
// ============================================================================

/// [feat_req_recentip_278] Interface version is at byte offset 13
#[test]
fn interface_version_offset_is_13() {
    assert_eq!(INTERFACE_VERSION_OFFSET, 13);
}

/// [feat_req_recentip_278] Interface version major can be any value 0-255
#[test]
fn interface_version_major_range() {
    // All major versions 0-255 are valid
    for major in 0u8..=255u8 {
        let version = InterfaceVersion::new(major, 0);
        assert_eq!(version.wire_major(), major);
    }
}

/// [feat_req_recentip_278] Interface version minor is 32-bit
#[test]
fn interface_version_minor_is_32bit() {
    let version = InterfaceVersion::new(1, u32::MAX);
    assert_eq!(version.minor, u32::MAX);
    
    let version = InterfaceVersion::new(1, 0);
    assert_eq!(version.minor, 0);
}

/// [feat_req_recentip_278] Same major version is compatible
#[test]
fn interface_version_same_major_compatible() {
    let v1 = InterfaceVersion::new(1, 0);
    let v2 = InterfaceVersion::new(1, 5);
    
    // Both should be compatible with v1.0
    assert!(v1.is_compatible_with(&InterfaceVersion::new(1, 0)));
    assert!(v2.is_compatible_with(&InterfaceVersion::new(1, 0)));
}

/// [feat_req_recentip_278] Different major version is incompatible
#[test]
fn interface_version_different_major_incompatible() {
    let v1 = InterfaceVersion::new(1, 0);
    let v2 = InterfaceVersion::new(2, 0);
    
    assert!(!v1.is_compatible_with(&v2));
    assert!(!v2.is_compatible_with(&v1));
}

/// [feat_req_recentip_278] Higher minor version is compatible with lower
#[test]
fn interface_version_higher_minor_compatible() {
    let server_v = InterfaceVersion::new(1, 5);
    let required_v = InterfaceVersion::new(1, 3);
    
    // Server with 1.5 is compatible with client needing 1.3
    assert!(server_v.is_compatible_with(&required_v));
    
    // But not the other way around
    let old_server = InterfaceVersion::new(1, 2);
    assert!(!old_server.is_compatible_with(&required_v));
}

/// [feat_req_recentip_278] Exact version match check
#[test]
fn interface_version_exact_match() {
    let v1 = InterfaceVersion::new(1, 5);
    let v2 = InterfaceVersion::new(1, 5);
    let v3 = InterfaceVersion::new(1, 6);
    let v4 = InterfaceVersion::new(2, 5);
    
    assert!(v1.matches_exactly(&v2));
    assert!(!v1.matches_exactly(&v3)); // Different minor
    assert!(!v1.matches_exactly(&v4)); // Different major
}

/// [feat_req_recentip_278] Validate interface version header byte
#[test]
fn interface_version_header_validation() {
    let expected = InterfaceVersion::new(1, 0);
    
    // Matching major is OK
    assert!(validate_interface_version(1, &expected).is_ok());
    
    // Mismatched major is error
    let result = validate_interface_version(2, &expected);
    match result {
        Err(VersionError::InterfaceMajorMismatch { expected: 1, found: 2 }) => {}
        other => panic!("Expected InterfaceMajorMismatch, got {:?}", other),
    }
}

// ============================================================================
// Wire Format Tests
// ============================================================================

/// [feat_req_recentip_300] Protocol version in header at correct offset
#[test]
fn wire_format_protocol_version_position() {
    // Minimal valid SOME/IP header
    let header = [
        0x12, 0x34,             // Service ID
        0x00, 0x01,             // Method ID
        0x00, 0x00, 0x00, 0x08, // Length
        0x00, 0x01,             // Client ID
        0x00, 0x01,             // Session ID
        0x01,                   // Protocol version (offset 12)
        0x01,                   // Interface version (offset 13)
        0x00,                   // Message type
        0x00,                   // Return code
    ];
    
    assert_eq!(header[PROTOCOL_VERSION_OFFSET], PROTOCOL_VERSION);
}

/// [feat_req_recentip_278] Interface version in header at correct offset
#[test]
fn wire_format_interface_version_position() {
    let interface_v = 0x05u8; // Major version 5
    
    let header = [
        0x12, 0x34,             // Service ID
        0x00, 0x01,             // Method ID
        0x00, 0x00, 0x00, 0x08, // Length
        0x00, 0x01,             // Client ID
        0x00, 0x01,             // Session ID
        0x01,                   // Protocol version
        interface_v,            // Interface version (offset 13)
        0x00,                   // Message type
        0x00,                   // Return code
    ];
    
    assert_eq!(header[INTERFACE_VERSION_OFFSET], interface_v);
}

/// [feat_req_recentip_300] Parse protocol version from raw bytes
#[test]
fn parse_protocol_version_from_bytes() {
    let header_bytes = [
        0x12, 0x34, 0x00, 0x01, 0x00, 0x00, 0x00, 0x08,
        0x00, 0x01, 0x00, 0x01, 0x01, 0x05, 0x00, 0x00,
    ];
    
    let protocol_v = header_bytes[PROTOCOL_VERSION_OFFSET];
    let interface_v = header_bytes[INTERFACE_VERSION_OFFSET];
    
    assert_eq!(protocol_v, 0x01);
    assert_eq!(interface_v, 0x05);
}

// ============================================================================
// Integration Tests (require Runtime implementation)
// ============================================================================

use someip_runtime::*;
use std::time::Duration;

#[path = "../simulated.rs"]
mod simulated;
use simulated::{NetworkEvent, SimulatedNetwork};

/// Helper: Find all SOME/IP RPC messages (non-SD, i.e., service ID != 0xFFFF)
fn find_rpc_messages(network: &SimulatedNetwork) -> Vec<Vec<u8>> {
    network
        .history()
        .into_iter()
        .filter_map(|event| match event {
            NetworkEvent::UdpSent { data, .. } => Some(data),
            NetworkEvent::TcpSent { data, .. } => Some(data),
            _ => None,
        })
        .filter(|data| {
            if data.len() < 16 {
                return false;
            }
            let service_id = u16::from_be_bytes([data[0], data[1]]);
            service_id != 0xFFFF // Exclude SD messages
        })
        .collect()
}

/// [feat_req_recentip_300] Server rejects wrong protocol version
#[test]
#[ignore = "Runtime::new not implemented"]
fn server_rejects_wrong_protocol_version() {
    let (network, _io_client, io_server) = SimulatedNetwork::new_pair();
    
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();
    
    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    
    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();
    
    let mut offering = server.offer(service_config).unwrap();
    network.advance(Duration::from_millis(100));
    
    // Inject packet with WRONG protocol version (0x02 instead of 0x01)
    let bad_protocol_packet = [
        0x12, 0x34,             // Service ID
        0x00, 0x01,             // Method ID
        0x00, 0x00, 0x00, 0x08, // Length
        0x00, 0x01,             // Client ID
        0x00, 0x01,             // Session ID
        0x02,                   // Protocol version: WRONG (should be 0x01)
        0x01,                   // Interface version
        0x00,                   // Message type: REQUEST
        0x00,                   // Return code
    ];
    
    let from_addr = "192.168.1.100:50000".parse().unwrap();
    let to_addr = "192.168.1.20:30490".parse().unwrap();
    network.inject_udp(from_addr, to_addr, &bad_protocol_packet);
    network.advance(Duration::from_millis(100));
    
    // Server should NOT process this request
    let event = offering.try_next().unwrap();
    assert!(event.is_none(), "Wrong protocol version should be rejected");
}

/// [feat_req_recentip_278] Server rejects wrong interface version
#[test]
#[ignore = "Runtime::new not implemented"]
fn server_rejects_wrong_interface_version() {
    let (network, _io_client, io_server) = SimulatedNetwork::new_pair();
    
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();
    
    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    
    // Server offers service with major version 2
    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .major_version(0x02) // Major version 2
        .build()
        .unwrap();
    
    let mut offering = server.offer(service_config).unwrap();
    network.advance(Duration::from_millis(100));
    
    // Inject request with interface version 1 (wrong)
    let wrong_version_packet = [
        0x12, 0x34,             // Service ID
        0x00, 0x01,             // Method ID
        0x00, 0x00, 0x00, 0x08, // Length
        0x00, 0x01,             // Client ID
        0x00, 0x01,             // Session ID
        0x01,                   // Protocol version
        0x01,                   // Interface version: WRONG (should be 0x02)
        0x00,                   // Message type: REQUEST
        0x00,                   // Return code
    ];
    
    let from_addr = "192.168.1.100:50000".parse().unwrap();
    let to_addr = "192.168.1.20:30490".parse().unwrap();
    network.inject_udp(from_addr, to_addr, &wrong_version_packet);
    network.advance(Duration::from_millis(100));
    
    // Server should reject with WRONG_INTERFACE_VERSION or drop the message
    // Check for error response with return code 0x08
    let messages = find_rpc_messages(&network);
    let error_responses: Vec<_> = messages
        .iter()
        .filter(|m| m.len() >= 16 && m[14] == 0x81 && m[15] == 0x08)
        .collect();
    
    // Either server drops the message OR sends E_WRONG_INTERFACE_VERSION
    let event = offering.try_next().unwrap();
    let rejected = event.is_none() || !error_responses.is_empty();
    assert!(rejected, "Wrong interface version should be rejected");
}

/// [feat_req_recentip_278] Client discovers service version via SD
#[test]
#[ignore = "Runtime::new not implemented"]
fn client_discovers_interface_version() {
    let (network, io_client, io_server) = SimulatedNetwork::new_pair();
    
    let mut _client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();
    
    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    
    // Server offers service with specific version
    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .major_version(0x03)
        .minor_version(0x00000005)
        .build()
        .unwrap();
    
    let _offering = server.offer(service_config).unwrap();
    
    // Run SD to exchange offers
    network.advance(Duration::from_millis(1000));
    
    // TODO: When proxy becomes available, verify it reports correct version
    // let proxy = client.require(service_id, InstanceId::ANY);
    // network.advance(Duration::from_millis(500));
    // let available = proxy.wait_available().unwrap();
    // assert_eq!(available.major_version(), 0x03);
}

/// [feat_req_recentip_278] Wire format carries only major version
#[test]
#[ignore = "Runtime::new not implemented"]
fn version_negotiation_uses_major_only() {
    let (network, io_client, io_server) = SimulatedNetwork::new_pair();
    
    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();
    
    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    
    // Server with major=2, minor=12345
    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .major_version(0x02)
        .minor_version(12345)
        .build()
        .unwrap();
    
    let _offering = server.offer(service_config).unwrap();
    network.advance(Duration::from_millis(100));
    
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();
    
    // Send request
    let method_id = MethodId::new(0x0001);
    let _pending = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(Duration::from_millis(100));
    
    // Check wire format: interface version byte should be 0x02
    let messages = find_rpc_messages(&network);
    let request = messages.iter().find(|m| m.len() >= 16 && m[14] == 0x00);
    assert!(request.is_some(), "Should have sent REQUEST");
    
    let request = request.unwrap();
    assert_eq!(request[INTERFACE_VERSION_OFFSET], 0x02, 
        "Wire format should carry major version");
    // Note: minor version (12345) is NOT in the SOME/IP header - it's SD-only
}

// ============================================================================
// Property-Based Tests
// ============================================================================

#[cfg(test)]
mod proptest_suite {
    use super::*;
    use proptest::prelude::*;
    
    proptest! {
        /// [feat_req_recentip_300] Only protocol version 0x01 is valid
        #[test]
        fn only_protocol_v1_valid(version in 0u8..=255u8) {
            let result = validate_protocol_version(version);
            if version == 0x01 {
                prop_assert!(result.is_ok());
            } else {
                prop_assert!(result.is_err());
            }
        }
        
        /// [feat_req_recentip_278] Interface version major matches header byte
        #[test]
        fn interface_version_wire_major(major in 0u8..=255u8, minor in 0u32..=u32::MAX) {
            let version = InterfaceVersion::new(major, minor);
            prop_assert_eq!(version.wire_major(), major);
        }
        
        /// [feat_req_recentip_278] Compatibility is reflexive
        #[test]
        fn interface_version_reflexive_compat(major in 0u8..=255u8, minor in 0u32..=u32::MAX) {
            let version = InterfaceVersion::new(major, minor);
            prop_assert!(version.is_compatible_with(&version));
        }
        
        /// [feat_req_recentip_278] Exact match is reflexive
        #[test]
        fn interface_version_reflexive_exact(major in 0u8..=255u8, minor in 0u32..=u32::MAX) {
            let version = InterfaceVersion::new(major, minor);
            prop_assert!(version.matches_exactly(&version));
        }
        
        /// [feat_req_recentip_278] Different major always incompatible
        #[test]
        fn interface_version_major_mismatch(
            major1 in 0u8..=127u8,
            major2 in 128u8..=255u8,
            minor1 in 0u32..=u32::MAX,
            minor2 in 0u32..=u32::MAX,
        ) {
            let v1 = InterfaceVersion::new(major1, minor1);
            let v2 = InterfaceVersion::new(major2, minor2);
            
            // Different major versions are never compatible
            prop_assert!(!v1.is_compatible_with(&v2));
            prop_assert!(!v2.is_compatible_with(&v1));
        }
        
        /// [feat_req_recentip_278] Higher minor is compatible with lower
        #[test]
        fn interface_version_minor_forward_compat(
            major in 0u8..=255u8,
            minor_low in 0u32..100u32,
            minor_delta in 1u32..100u32,
        ) {
            let minor_high = minor_low.saturating_add(minor_delta);
            
            let low = InterfaceVersion::new(major, minor_low);
            let high = InterfaceVersion::new(major, minor_high);
            
            // Higher minor can serve lower minor
            prop_assert!(high.is_compatible_with(&low));
            
            // But not vice versa (unless equal)
            if minor_low < minor_high {
                prop_assert!(!low.is_compatible_with(&high));
            }
        }
    }
}
