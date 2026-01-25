//! SOME/IP Transport Protocol (SOME/IP-TP) Compliance Tests
//!
//! Tests the segmentation and reassembly of large SOME/IP messages over UDP.
//! SOME/IP-TP allows transporting messages larger than the ~1400 byte UDP limit.
//!
//! Key requirements tested:
//! - feat_req_someiptp_760: Use TP for payloads > 1400 bytes over UDP
//! - feat_req_someiptp_762: Session handling required for TP messages
//! - feat_req_someiptp_763: All segments share same Session ID
//! - feat_req_someiptp_765: TP-Flag must be set to 1 in Message Type
//! - feat_req_someiptp_766: TP header format (offset, reserved, more_segments)
//! - feat_req_someiptp_768: Offset is upper 28 bits (multiples of 16)
//! - feat_req_someiptp_770: More Segments Flag usage
//! - feat_req_someiptp_772: Segment length must be multiple of 16 (except last)
//! - feat_req_someiptp_773: Max segment size is 1392 bytes (87 x 16)
//!
//! NOTE: SOME/IP-TP is not yet implemented in the runtime. Integration tests
//! are marked as ignored until the feature is available.

#![allow(dead_code)]

use std::time::Duration;

use recentip::handle::ServiceEvent;
use recentip::prelude::*;

#[cfg(feature = "turmoil")]

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
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
// Test Service Definition
// ============================================================================

const TEST_SERVICE_ID: u16 = 0x1234;
const TEST_SERVICE_VERSION: (u8, u32) = (1, 0);

// ============================================================================
// TP Header Unit Tests (test infrastructure, not conformance)
// ============================================================================

#[cfg(test)]
mod tp_header_tests {
    use super::*;

    #[test_log::test]
    fn tp_header_size_is_4_bytes() {
        assert_eq!(TpHeader::SIZE, 4);
    }

    #[test_log::test]
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

    #[test_log::test]
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

    #[test_log::test]
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

    #[test_log::test]
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

    #[test_log::test]
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

    #[test_log::test]
    fn tp_header_reserved_ignored_on_receive() {
        // Reserved shall be ignored by receiver
        // Even if reserved bits are set, parsing should succeed
        let bytes = [0x00, 0x00, 0x05, 0x7F]; // offset=87*16, reserved=0x07, more=1
        let parsed = TpHeader::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.offset, 1392);
        assert!(parsed.more_segments);
        // reserved bits are captured but should be ignored for validation
    }

    #[test_log::test]
    fn tp_header_roundtrip() {
        let headers = [
            TpHeader {
                offset: 0,
                reserved: 0,
                more_segments: true,
            },
            TpHeader {
                offset: 1392,
                reserved: 0,
                more_segments: true,
            },
            TpHeader {
                offset: 2784,
                reserved: 0,
                more_segments: false,
            },
            TpHeader {
                offset: 0x10000,
                reserved: 0,
                more_segments: true,
            },
        ];

        for header in headers {
            let bytes = header.to_bytes();
            let parsed = TpHeader::from_bytes(&bytes).unwrap();
            assert_eq!(parsed, header);
        }
    }
}

// ============================================================================
// Integration Tests (require SOME/IP-TP implementation)
// ============================================================================

/// feat_req_someiptp_760: Large messages must use TP over UDP
///
/// When sending a SOME/IP message with payload > 1400 bytes over UDP,
/// the implementation must automatically segment it using SOME/IP-TP.
#[cfg(feature = "turmoil")]
#[test_log::test]
#[ignore = "SOME/IP-TP not yet implemented"]
fn large_udp_messages_use_tp() {
    covers!(feat_req_someiptp_760);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Echo back
        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::Call {
                    payload, responder, ..
                } => {
                    responder.reply(payload.as_ref()).unwrap();
                }
                _ => {}
            }
        }

        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        // Send a large message (2000 bytes payload - larger than 1400 byte limit)
        let large_payload = vec![0xABu8; 2000];
        let method = MethodId::new(0x0001).unwrap();
        let response =
            tokio::time::timeout(Duration::from_secs(5), proxy.call(method, &large_payload))
                .await
                .expect("RPC timeout")
                .expect("RPC should succeed");

        // Response should echo our payload
        assert_eq!(response.payload.len(), 2000);

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someiptp_762, feat_req_someiptp_763: Session ID consistency
///
/// All segments of a TP message must share the same Session ID.
#[cfg(feature = "turmoil")]
#[test_log::test]
#[ignore = "SOME/IP-TP not yet implemented"]
fn tp_segments_share_session_id() {
    covers!(feat_req_someiptp_762, feat_req_someiptp_763);
    // This test would verify wire capture to ensure all TP segments
    // of a large message share the same session ID
}

/// feat_req_someiptp_765: TP-Flag must be set
///
/// All TP segments must have the TP flag bit (0x20) set in message type.
#[cfg(feature = "turmoil")]
#[test_log::test]
#[ignore = "SOME/IP-TP not yet implemented"]
fn tp_flag_set_on_segments() {
    covers!(feat_req_someiptp_765);
    // Verify TP flag is set on all segments via wire capture
}

/// feat_req_someiptp_766: TP header format
///
/// TP header must follow the specified format: offset, reserved, more_segments.
#[cfg(feature = "turmoil")]
#[test_log::test]
#[ignore = "SOME/IP-TP not yet implemented"]
fn tp_header_format() {
    covers!(feat_req_someiptp_766);
    // Verify TP header format on wire
}

/// feat_req_someiptp_768: Offset is multiple of 16
///
/// The offset field in TP header must be in multiples of 16 bytes.
#[cfg(feature = "turmoil")]
#[test_log::test]
#[ignore = "SOME/IP-TP not yet implemented"]
fn tp_offset_is_16_aligned() {
    covers!(feat_req_someiptp_768);
    // Verify offset values on wire are 16-aligned
}

/// feat_req_someiptp_770: More Segments Flag
///
/// The More Segments flag must be set on all segments except the last.
#[cfg(feature = "turmoil")]
#[test_log::test]
#[ignore = "SOME/IP-TP not yet implemented"]
fn tp_more_segments_flag() {
    covers!(feat_req_someiptp_770);
    // Verify More Segments flag usage on wire
}

/// feat_req_someiptp_772: Segment length multiple of 16
///
/// All segment payloads except the last must be multiples of 16 bytes.
#[cfg(feature = "turmoil")]
#[test_log::test]
#[ignore = "SOME/IP-TP not yet implemented"]
fn tp_segment_length_alignment() {
    covers!(feat_req_someiptp_772);
    // Verify segment lengths on wire
}

/// feat_req_someiptp_773: Max segment size
///
/// Maximum segment size is 1392 bytes (87 x 16).
#[cfg(feature = "turmoil")]
#[test_log::test]
#[ignore = "SOME/IP-TP not yet implemented"]
fn tp_max_segment_size() {
    covers!(feat_req_someiptp_773);
    // Verify no segment exceeds max size
}

/// Large message reassembly on receiver
///
/// Receiver must correctly reassemble TP segments into original message.
#[cfg(feature = "turmoil")]
#[test_log::test]
#[ignore = "SOME/IP-TP not yet implemented"]
fn tp_reassembly() {
    covers!(feat_req_someiptp_760);
    // End-to-end test: send large message, verify correct reassembly
}
