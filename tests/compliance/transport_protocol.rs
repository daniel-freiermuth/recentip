//! SOME/IP Transport Protocol (SOME/IP-TP) Compliance Tests
//!
//! Tests the segmentation and reassembly of large SOME/IP messages over UDP.
//! SOME/IP-TP allows transporting messages larger than the ~1400 byte UDP limit.
//!
//! Key requirements tested:
//! - feat_req_recentiptp_760: Use TP for payloads > 1400 bytes over UDP
//! - feat_req_recentiptp_762: Session handling required for TP messages
//! - feat_req_recentiptp_763: All segments share same Session ID
//! - feat_req_recentiptp_765: TP-Flag must be set to 1 in Message Type
//! - feat_req_recentiptp_766: TP header format (offset, reserved, more_segments)
//! - feat_req_recentiptp_768: Offset is upper 28 bits (multiples of 16)
//! - feat_req_recentiptp_770: More Segments Flag usage
//! - feat_req_recentiptp_772: Segment length must be multiple of 16 (except last)
//! - feat_req_recentiptp_773: Max segment size is 1392 bytes (87 x 16)

use someip_runtime::*;

// Re-use wire format parsing from the shared module
#[path = "../wire.rs"]
mod wire;

// Re-use simulated network
#[path = "../simulated.rs"]
mod simulated;

use simulated::{NetworkEvent, SimulatedNetwork};

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        // Documentation marker - these requirements are tested by this function
        let _ = ($(stringify!($req)),+);
    };
}

// ============================================================================
// TP Header Parsing Utilities (for test verification only)
// ============================================================================

/// TP Header structure (4 bytes after SOME/IP header)
/// - Offset [28 bits]: Upper 28 bits of byte offset (multiply by 16 for actual offset)
/// - Reserved [3 bits]: Must be 0
/// - More Segments Flag [1 bit]: 1 if more segments follow
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TpHeader {
    /// Offset in the original message (already multiplied by 16)
    pub offset: u32,
    /// Reserved bits (should be 0)
    pub reserved: u8,
    /// More segments follow this one
    pub more_segments: bool,
}

impl TpHeader {
    /// TP header is always 4 bytes
    pub const SIZE: usize = 4;

    /// Parse TP header from bytes (big-endian)
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE {
            return None;
        }

        // TP header: [offset_high:28][reserved:3][more_segments:1]
        let raw = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);

        // Upper 28 bits are offset / 16, lower 4 bits are reserved (3) + more_segments (1)
        let offset_field = raw >> 4; // Upper 28 bits
        let offset = offset_field << 4; // Actual byte offset (multiply by 16)
        let reserved = ((raw >> 1) & 0x07) as u8; // Bits 3..1
        let more_segments = (raw & 0x01) != 0; // Bit 0

        Some(Self {
            offset,
            reserved,
            more_segments,
        })
    }

    /// Convert to big-endian bytes
    pub fn to_bytes(&self) -> [u8; 4] {
        // Offset is stored as upper 28 bits (offset / 16)
        let offset_field = self.offset >> 4;
        let raw = (offset_field << 4)
            | ((self.reserved as u32 & 0x07) << 1)
            | (if self.more_segments { 1 } else { 0 });
        raw.to_be_bytes()
    }
}

// ============================================================================
// TP Constants
// ============================================================================

/// Maximum segment payload for UDP (87 x 16 = 1392 bytes)
pub const MAX_SEGMENT_SIZE: usize = 1392;

/// TP-Flag bit position in Message Type
pub const TP_FLAG_BIT: u8 = 0x20;

// ============================================================================
// TP Header Unit Tests (test infrastructure, not conformance)
// ============================================================================

#[cfg(test)]
mod tp_header_tests {
    use super::*;

    #[test]
    fn tp_header_size_is_4_bytes() {
        assert_eq!(TpHeader::SIZE, 4);
    }

    #[test]
    fn tp_header_first_segment() {
        // First segment: offset=0, more_segments=true
        let header = TpHeader {
            offset: 0,
            reserved: 0,
            more_segments: true,
        };
        let bytes = header.to_bytes();

        // Should be: 0x00 0x00 0x00 0x01 (only more_segments bit set)
        assert_eq!(bytes, [0x00, 0x00, 0x00, 0x01]);

        let parsed = TpHeader::from_bytes(&bytes).unwrap();
        assert_eq!(parsed, header);
    }

    #[test]
    fn tp_header_middle_segment() {
        // Middle segment: offset=1392 (0x570), more_segments=true
        // offset_field = 1392 / 16 = 87 = 0x57
        let header = TpHeader {
            offset: 1392,
            reserved: 0,
            more_segments: true,
        };
        let bytes = header.to_bytes();

        // offset_field = 87 = 0x57, stored in upper 28 bits
        // Raw = (0x57 << 4) | 0x01 = 0x571
        assert_eq!(bytes, [0x00, 0x00, 0x05, 0x71]);

        let parsed = TpHeader::from_bytes(&bytes).unwrap();
        assert_eq!(parsed, header);
    }

    #[test]
    fn tp_header_last_segment() {
        // Last segment: offset=2784 (0xAE0), more_segments=false
        let header = TpHeader {
            offset: 2784,
            reserved: 0,
            more_segments: false,
        };
        let bytes = header.to_bytes();

        // offset_field = 2784 / 16 = 174 = 0xAE
        // Raw = (0xAE << 4) | 0x00 = 0xAE0
        assert_eq!(bytes, [0x00, 0x00, 0x0A, 0xE0]);

        let parsed = TpHeader::from_bytes(&bytes).unwrap();
        assert_eq!(parsed, header);
    }

    #[test]
    fn tp_header_large_offset() {
        // Large offset near max: offset = 0x0FFFFFF0 (268,435,440 bytes)
        let header = TpHeader {
            offset: 0x0FFF_FFF0,
            reserved: 0,
            more_segments: false,
        };
        let bytes = header.to_bytes();

        let parsed = TpHeader::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.offset, 0x0FFF_FFF0);
        assert!(!parsed.more_segments);
    }

    #[test]
    fn tp_header_offset_must_be_16_aligned() {
        // Offsets must be multiples of 16 (lower 4 bits always 0)
        for offset in [0, 16, 32, 1392, 2784, 0x1000] {
            let header = TpHeader {
                offset,
                reserved: 0,
                more_segments: true,
            };
            let bytes = header.to_bytes();
            let parsed = TpHeader::from_bytes(&bytes).unwrap();
            assert_eq!(parsed.offset, offset, "Offset {} should roundtrip", offset);
            assert_eq!(parsed.offset % 16, 0, "Offset must be 16-aligned");
        }
    }

    #[test]
    fn tp_header_reserved_ignored_on_receive() {
        // Reserved shall be ignored by receiver
        // Even if reserved bits are set, parsing should succeed
        let bytes = [0x00, 0x00, 0x05, 0x7F]; // offset=87*16, reserved=0x07, more=1
        let parsed = TpHeader::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.offset, 1392);
        assert!(parsed.more_segments);
        // reserved bits are captured but should be ignored for validation
    }

    #[test]
    fn tp_header_roundtrip() {
        let headers = [
            TpHeader { offset: 0, reserved: 0, more_segments: true },
            TpHeader { offset: 1392, reserved: 0, more_segments: true },
            TpHeader { offset: 2784, reserved: 0, more_segments: false },
            TpHeader { offset: 0x10000, reserved: 0, more_segments: true },
        ];

        for header in headers {
            let bytes = header.to_bytes();
            let parsed = TpHeader::from_bytes(&bytes).unwrap();
            assert_eq!(parsed, header);
        }
    }
}

// ============================================================================
// Integration Tests (require Runtime implementation with RPC support)
// ============================================================================

/// Helper to find TP segments in captured network traffic
#[allow(dead_code)]
fn find_tp_segments(network: &SimulatedNetwork) -> Vec<Vec<u8>> {
    network
        .history()
        .iter()
        .filter_map(|event| {
            if let NetworkEvent::UdpSent { data, .. } = event {
                // Check if this is a SOME/IP message with TP flag
                if data.len() >= 16 {
                    let message_type = data[14];
                    if message_type & TP_FLAG_BIT != 0 {
                        return Some(data.clone());
                    }
                }
            }
            None
        })
        .collect()
}

/// Helper to find regular (non-TP) UDP messages
#[allow(dead_code)]
fn find_regular_messages(network: &SimulatedNetwork) -> Vec<Vec<u8>> {
    network
        .history()
        .iter()
        .filter_map(|event| {
            if let NetworkEvent::UdpSent { data, .. } = event {
                if data.len() >= 16 {
                    let message_type = data[14];
                    // Non-TP message (TP flag not set)
                    if message_type & TP_FLAG_BIT == 0 {
                        return Some(data.clone());
                    }
                }
            }
            None
        })
        .collect()
}

/// feat_req_recentiptp_760: Large messages must use TP over UDP
///
/// When sending a SOME/IP message with payload > 1400 bytes over UDP,
/// the implementation must automatically segment it using SOME/IP-TP.
#[test]
#[ignore = "Runtime::new not implemented"]
fn large_udp_messages_use_tp() {
    covers!(feat_req_recentiptp_760);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Client requires the service and waits for availability
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    // Send a large message (2000 bytes payload - larger than 1400 byte limit)
    let large_payload = vec![0xABu8; 2000];
    let method_id = MethodId::new(0x0001);
    let _response = proxy.call(method_id, &large_payload).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Should have been segmented into multiple TP segments
    let segments = find_tp_segments(&network);
    assert!(segments.len() >= 2, "Large message should be segmented");

    // Each segment should have TP flag set
    for segment in &segments {
        let message_type = segment[14];
        assert!(
            message_type & TP_FLAG_BIT != 0,
            "TP flag must be set on all segments"
        );
    }
}

/// feat_req_recentiptp_762, feat_req_recentiptp_763: Session ID consistency
///
/// All segments of a TP message must share the same Session ID.
/// Session handling must be active for TP messages.
#[test]
#[ignore = "Runtime::new not implemented"]
fn tp_segments_share_session_id() {
    covers!(feat_req_recentiptp_762, feat_req_recentiptp_763);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Client requires and waits for service
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    // Send large message that will be segmented
    let large_payload = vec![0xCDu8; 5000];
    let method_id = MethodId::new(0x0001);
    let _response = proxy.call(method_id, &large_payload).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let segments = find_tp_segments(&network);
    assert!(segments.len() >= 2, "Should have multiple segments");

    // Extract session IDs from all segments
    let session_ids: Vec<u16> = segments
        .iter()
        .map(|seg| {
            let header = wire::Header::from_bytes(seg).unwrap();
            header.session_id
        })
        .collect();

    // All session IDs must be the same
    let first_session = session_ids[0];
    assert!(first_session != 0, "Session ID must not be 0 for TP messages");
    for (i, &session) in session_ids.iter().enumerate() {
        assert_eq!(
            session, first_session,
            "Segment {} has different session ID", i
        );
    }
}

/// feat_req_recentiptp_765: TP-Flag in Message Type
///
/// All SOME/IP-TP segments must have the TP-Flag (bit 5) set to 1.
#[test]
#[ignore = "Runtime::new not implemented"]
fn tp_flag_is_set_on_segments() {
    covers!(feat_req_recentiptp_765);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    let large_payload = vec![0xEFu8; 3000];
    let method_id = MethodId::new(0x0001);
    let _response = proxy.call(method_id, &large_payload).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let segments = find_tp_segments(&network);
    for (i, segment) in segments.iter().enumerate() {
        let header = wire::Header::from_bytes(segment).unwrap();
        assert!(
            header.is_tp_message(),
            "Segment {} must have TP flag set", i
        );
        assert_eq!(
            header.message_type & TP_FLAG_BIT, TP_FLAG_BIT,
            "TP flag bit must be set"
        );
    }
}

/// feat_req_recentiptp_766, feat_req_recentiptp_768: TP Header format
///
/// TP header follows SOME/IP header: [offset:28][reserved:3][more:1]
/// Offset represents multiples of 16 bytes.
#[test]
#[ignore = "Runtime::new not implemented"]
fn tp_header_format_correct() {
    covers!(feat_req_recentiptp_766, feat_req_recentiptp_768);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    let large_payload = vec![0x12u8; 4000];
    let method_id = MethodId::new(0x0001);
    let _response = proxy.call(method_id, &large_payload).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let segments = find_tp_segments(&network);
    assert!(segments.len() >= 3, "Large payload should produce at least 3 segments");

    let mut expected_offset: u32 = 0;
    for (i, segment) in segments.iter().enumerate() {
        // TP header starts at byte 16 (after SOME/IP header)
        let tp_header = TpHeader::from_bytes(&segment[16..]).unwrap();

        // Offset must match expected value
        assert_eq!(
            tp_header.offset, expected_offset,
            "Segment {} offset mismatch", i
        );

        // Offset must be 16-aligned
        assert_eq!(
            tp_header.offset % 16, 0,
            "Offset must be multiple of 16"
        );

        // Reserved bits should be 0
        assert_eq!(
            tp_header.reserved, 0,
            "Reserved bits must be 0"
        );

        // Calculate segment payload size (message length - 8 - 4 for TP header)
        let header = wire::Header::from_bytes(segment).unwrap();
        let segment_payload = header.length as usize - 8 - 4;

        expected_offset += segment_payload as u32;
    }
}

/// feat_req_recentiptp_770: More Segments Flag
///
/// More Segments = 1 for all segments except the last.
/// More Segments = 0 for the final segment.
#[test]
#[ignore = "Runtime::new not implemented"]
fn more_segments_flag_correct() {
    covers!(feat_req_recentiptp_770);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    let large_payload = vec![0x34u8; 3500];
    let method_id = MethodId::new(0x0001);
    let _response = proxy.call(method_id, &large_payload).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let segments = find_tp_segments(&network);
    let num_segments = segments.len();
    assert!(num_segments >= 3, "Should have at least 3 segments");

    for (i, segment) in segments.iter().enumerate() {
        let tp_header = TpHeader::from_bytes(&segment[16..]).unwrap();
        let is_last = i == num_segments - 1;

        if is_last {
            assert!(
                !tp_header.more_segments,
                "Last segment must have more_segments=0"
            );
        } else {
            assert!(
                tp_header.more_segments,
                "Segment {} must have more_segments=1", i
            );
        }
    }
}

/// feat_req_recentiptp_772: Segment alignment
///
/// All segments except the last must have length that is a multiple of 16 bytes.
#[test]
#[ignore = "Runtime::new not implemented"]
fn segment_length_16_aligned() {
    covers!(feat_req_recentiptp_772);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    // Use a payload that doesn't divide evenly by 16
    let large_payload = vec![0x56u8; 3333];
    let method_id = MethodId::new(0x0001);
    let _response = proxy.call(method_id, &large_payload).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let segments = find_tp_segments(&network);
    let num_segments = segments.len();

    for (i, segment) in segments.iter().enumerate() {
        let header = wire::Header::from_bytes(segment).unwrap();
        // Segment payload = length - 8 (rest of SOME/IP header) - 4 (TP header)
        let segment_payload = header.length as usize - 8 - 4;
        let is_last = i == num_segments - 1;

        if !is_last {
            assert_eq!(
                segment_payload % 16, 0,
                "Non-last segment {} payload ({}) must be 16-aligned",
                i, segment_payload
            );
        }
        // Last segment can be any size
    }
}

/// feat_req_recentiptp_773: Maximum segment size
///
/// Maximum segment payload is 1392 bytes (87 x 16) to fit in UDP.
#[test]
#[ignore = "Runtime::new not implemented"]
fn max_segment_size_respected() {
    covers!(feat_req_recentiptp_773);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    // Send a very large message
    let large_payload = vec![0x78u8; 50000];
    let method_id = MethodId::new(0x0001);
    let _response = proxy.call(method_id, &large_payload).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let segments = find_tp_segments(&network);

    for segment in &segments {
        let header = wire::Header::from_bytes(segment).unwrap();
        let segment_payload = header.length as usize - 8 - 4;

        assert!(
            segment_payload <= MAX_SEGMENT_SIZE,
            "Segment payload {} exceeds max {}",
            segment_payload, MAX_SEGMENT_SIZE
        );
    }
}

/// feat_req_recentiptp_764: Terminology validation
///
/// Validate that segmented messages can be reassembled to recover
/// the original payload exactly.
#[test]
#[ignore = "Runtime::new not implemented"]
fn tp_reassembly_produces_original_payload() {
    covers!(feat_req_recentiptp_764);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    // Create a recognizable pattern for the payload
    let mut original_payload = Vec::with_capacity(5000);
    for i in 0..5000 {
        original_payload.push((i % 256) as u8);
    }

    let method_id = MethodId::new(0x0001);
    let _response = proxy.call(method_id, &original_payload).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Manually reassemble from captured segments
    let segments = find_tp_segments(&network);
    let mut reassembled = vec![0u8; original_payload.len()];

    for segment in &segments {
        let tp_header = TpHeader::from_bytes(&segment[16..]).unwrap();
        let offset = tp_header.offset as usize;
        // Segment data starts at byte 20 (16 SOME/IP + 4 TP header)
        let segment_data = &segment[20..];

        let end = (offset + segment_data.len()).min(reassembled.len());
        let copy_len = end - offset;
        reassembled[offset..end].copy_from_slice(&segment_data[..copy_len]);
    }

    assert_eq!(
        reassembled, original_payload,
        "Reassembled payload must match original"
    );
}

/// Small messages should NOT use TP
///
/// Messages that fit within UDP payload limit should be sent directly
/// without segmentation.
#[test]
#[ignore = "Runtime::new not implemented"]
fn small_messages_not_segmented() {
    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    // Small payload that fits in single UDP packet
    let small_payload = vec![0xABu8; 100];
    let method_id = MethodId::new(0x0001);
    let _response = proxy.call(method_id, &small_payload).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Should NOT have any TP segments
    let tp_segments = find_tp_segments(&network);
    assert!(
        tp_segments.is_empty(),
        "Small messages should not use TP segmentation"
    );

    // Should have a regular (non-TP) message
    let regular_messages = find_regular_messages(&network);
    assert!(
        !regular_messages.is_empty(),
        "Small message should be sent as regular SOME/IP message"
    );
}
