//! Wire format utilities for testing SOME/IP compliance.
//!
//! This module provides types for parsing and inspecting captured wire bytes.
//! It is intended for black-box testing only - parsing packets captured from
//! the SimulatedNetwork to verify protocol compliance.

// ============================================================================
// SOME/IP Header (for parsing/inspecting captured bytes)
// ============================================================================

/// SOME/IP protocol header (16 bytes)
///
/// This struct provides parsing of captured wire bytes for black-box testing
/// and interoperability verification.
///
/// Wire format (big-endian):
/// ```text
/// Byte 0-1:   Service ID
/// Byte 2-3:   Method ID (or Event ID if high bit set)
/// Byte 4-7:   Length (payload + 8 bytes for remaining header)
/// Byte 8-9:   Client ID
/// Byte 10-11: Session ID
/// Byte 12:    Protocol Version (currently 0x01)
/// Byte 13:    Interface Version
/// Byte 14:    Message Type
/// Byte 15:    Return Code
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub service_id: u16,
    pub method_id: u16,
    pub length: u32,
    pub client_id: u16,
    pub session_id: u16,
    pub protocol_version: u8,
    pub interface_version: u8,
    pub message_type: u8,
    pub return_code: u8,
}

impl Header {
    /// Header size in bytes
    pub const SIZE: usize = 16;

    /// Current SOME/IP protocol version
    pub const PROTOCOL_VERSION: u8 = 0x01;

    /// Parse a header from wire bytes (big-endian)
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::SIZE {
            return None;
        }
        Some(Self {
            service_id: u16::from_be_bytes([bytes[0], bytes[1]]),
            method_id: u16::from_be_bytes([bytes[2], bytes[3]]),
            length: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            client_id: u16::from_be_bytes([bytes[8], bytes[9]]),
            session_id: u16::from_be_bytes([bytes[10], bytes[11]]),
            protocol_version: bytes[12],
            interface_version: bytes[13],
            message_type: bytes[14],
            return_code: bytes[15],
        })
    }

    /// Serialize header to wire bytes (big-endian)
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        buf[0..2].copy_from_slice(&self.service_id.to_be_bytes());
        buf[2..4].copy_from_slice(&self.method_id.to_be_bytes());
        buf[4..8].copy_from_slice(&self.length.to_be_bytes());
        buf[8..10].copy_from_slice(&self.client_id.to_be_bytes());
        buf[10..12].copy_from_slice(&self.session_id.to_be_bytes());
        buf[12] = self.protocol_version;
        buf[13] = self.interface_version;
        buf[14] = self.message_type;
        buf[15] = self.return_code;
        buf
    }

    /// Check if this is an event/notification (method_id high bit set)
    pub fn is_event(&self) -> bool {
        self.method_id & 0x8000 != 0
    }

    /// Check if this is a request (message_type 0x00 or 0x01)
    pub fn is_request(&self) -> bool {
        self.message_type == message_type::REQUEST
            || self.message_type == message_type::REQUEST_NO_RETURN
    }

    /// Check if this is a response (message_type 0x80)
    pub fn is_response(&self) -> bool {
        self.message_type == message_type::RESPONSE
    }

    /// Check if this is a notification message
    pub fn is_notification(&self) -> bool {
        self.message_type == message_type::NOTIFICATION
    }

    /// Check if this is an error response
    pub fn is_error(&self) -> bool {
        self.message_type == message_type::ERROR
    }

    /// Check if this message uses TP (SOME/IP-TP segmentation)
    pub fn is_tp_message(&self) -> bool {
        self.message_type & message_type::TP_FLAG != 0
    }

    /// Get the Message ID (combined Service ID + Method ID as u32)
    pub fn message_id(&self) -> u32 {
        ((self.service_id as u32) << 16) | (self.method_id as u32)
    }

    /// Get the Request ID (combined Client ID + Session ID as u32)
    pub fn request_id(&self) -> u32 {
        ((self.client_id as u32) << 16) | (self.session_id as u32)
    }

    /// Payload length (total length minus 8 bytes for remaining header)
    pub fn payload_length(&self) -> u32 {
        self.length.saturating_sub(8)
    }
}

// ============================================================================
// Message Type Constants
// ============================================================================

/// Message type constants (SOME/IP header byte 14)
pub mod message_type {
    /// Request expecting response (0x00)
    pub const REQUEST: u8 = 0x00;
    /// Fire and forget - no response expected (0x01)
    pub const REQUEST_NO_RETURN: u8 = 0x01;
    /// Notification/event (0x02)
    pub const NOTIFICATION: u8 = 0x02;
    /// Response to request (0x80)
    pub const RESPONSE: u8 = 0x80;
    /// Error response (0x81)
    pub const ERROR: u8 = 0x81;
    /// TP flag - OR with message type for segmented messages (0x20)
    pub const TP_FLAG: u8 = 0x20;
}

// ============================================================================
// Return Code Constants
// ============================================================================

/// Return code constants (SOME/IP header byte 15)
pub mod return_code {
    /// No error (0x00)
    pub const E_OK: u8 = 0x00;
    /// Unspecified error (0x01)
    pub const E_NOT_OK: u8 = 0x01;
    /// Service ID unknown (0x02)
    pub const E_UNKNOWN_SERVICE: u8 = 0x02;
    /// Method ID unknown (0x03)
    pub const E_UNKNOWN_METHOD: u8 = 0x03;
    /// Service not ready (0x04)
    pub const E_NOT_READY: u8 = 0x04;
    /// Service not reachable (0x05)
    pub const E_NOT_REACHABLE: u8 = 0x05;
    /// Timeout (0x06)
    pub const E_TIMEOUT: u8 = 0x06;
    /// Wrong protocol version (0x07)
    pub const E_WRONG_PROTOCOL_VERSION: u8 = 0x07;
    /// Wrong interface version (0x08)
    pub const E_WRONG_INTERFACE_VERSION: u8 = 0x08;
    /// Malformed message (0x09)
    pub const E_MALFORMED_MESSAGE: u8 = 0x09;
    /// Wrong message type (0x0A)
    pub const E_WRONG_MESSAGE_TYPE: u8 = 0x0A;
    /// E2E repeated (0x0B)
    pub const E_E2E_REPEATED: u8 = 0x0B;
    /// E2E wrong sequence (0x0C)
    pub const E_E2E_WRONG_SEQUENCE: u8 = 0x0C;
    /// E2E error (0x0D)
    pub const E_E2E: u8 = 0x0D;
    /// E2E not available (0x0E)
    pub const E_E2E_NOT_AVAILABLE: u8 = 0x0E;
    /// E2E no new data (0x0F)
    pub const E_E2E_NO_NEW_DATA: u8 = 0x0F;
}

// ============================================================================
// Helper functions for parsing captured packets
// ============================================================================

/// Parse all SOME/IP headers from a list of captured packet data
#[allow(dead_code)]
pub fn parse_headers(packets: &[&[u8]]) -> Vec<Header> {
    packets
        .iter()
        .filter_map(|data| Header::from_bytes(data))
        .collect()
}

/// Find a request message by service and method ID
#[allow(dead_code)]
pub fn find_request(headers: &[Header], service_id: u16, method_id: u16) -> Option<&Header> {
    headers
        .iter()
        .find(|h| h.service_id == service_id && h.method_id == method_id && h.is_request())
}

/// Find a response message matching a request
#[allow(dead_code)]
pub fn find_response<'a>(headers: &'a [Header], request: &Header) -> Option<&'a Header> {
    headers.iter().find(|h| {
        h.service_id == request.service_id
            && h.method_id == request.method_id
            && h.client_id == request.client_id
            && h.session_id == request.session_id
            && (h.is_response() || h.is_error())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_size_is_16_bytes() {
        assert_eq!(Header::SIZE, 16);
    }

    #[test]
    fn header_to_bytes_is_big_endian() {
        let header = Header {
            service_id: 0x1234,
            method_id: 0x5678,
            length: 0x0000000C,
            client_id: 0xABCD,
            session_id: 0xEF01,
            protocol_version: 0x01,
            interface_version: 0x02,
            message_type: message_type::REQUEST,
            return_code: return_code::E_OK,
        };

        let bytes = header.to_bytes();

        // Verify big-endian encoding byte by byte
        assert_eq!(bytes[0], 0x12); // Service ID high
        assert_eq!(bytes[1], 0x34); // Service ID low
        assert_eq!(bytes[2], 0x56); // Method ID high
        assert_eq!(bytes[3], 0x78); // Method ID low
        assert_eq!(bytes[4], 0x00); // Length byte 0 (MSB)
        assert_eq!(bytes[5], 0x00); // Length byte 1
        assert_eq!(bytes[6], 0x00); // Length byte 2
        assert_eq!(bytes[7], 0x0C); // Length byte 3 (LSB)
        assert_eq!(bytes[8], 0xAB); // Client ID high
        assert_eq!(bytes[9], 0xCD); // Client ID low
        assert_eq!(bytes[10], 0xEF); // Session ID high
        assert_eq!(bytes[11], 0x01); // Session ID low
        assert_eq!(bytes[12], 0x01); // Protocol Version
        assert_eq!(bytes[13], 0x02); // Interface Version
        assert_eq!(bytes[14], message_type::REQUEST);
        assert_eq!(bytes[15], return_code::E_OK);
    }

    #[test]
    fn header_from_bytes_parses_correctly() {
        let bytes: [u8; 16] = [
            0x12, 0x34, // Service ID
            0x56, 0x78, // Method ID
            0x00, 0x00, 0x00, 0x0C, // Length
            0xAB, 0xCD, // Client ID
            0xEF, 0x01, // Session ID
            0x01, // Protocol Version
            0x02, // Interface Version
            0x00, // Message Type (REQUEST)
            0x00, // Return Code (E_OK)
        ];

        let header = Header::from_bytes(&bytes).expect("valid header");

        assert_eq!(header.service_id, 0x1234);
        assert_eq!(header.method_id, 0x5678);
        assert_eq!(header.length, 0x0000000C);
        assert_eq!(header.client_id, 0xABCD);
        assert_eq!(header.session_id, 0xEF01);
        assert_eq!(header.protocol_version, 0x01);
        assert_eq!(header.interface_version, 0x02);
        assert_eq!(header.message_type, message_type::REQUEST);
        assert_eq!(header.return_code, return_code::E_OK);
    }

    #[test]
    fn header_roundtrip() {
        let original = Header {
            service_id: 0xFFFF,
            method_id: 0x8001,
            length: 0xDEADBEEF,
            client_id: 0x1234,
            session_id: 0x5678,
            protocol_version: 0x01,
            interface_version: 0xFF,
            message_type: message_type::NOTIFICATION,
            return_code: return_code::E_OK,
        };

        let bytes = original.to_bytes();
        let parsed = Header::from_bytes(&bytes).expect("roundtrip");

        assert_eq!(parsed, original);
    }

    #[test]
    fn header_rejects_short_input() {
        let bytes: [u8; 15] = [0; 15];
        assert!(Header::from_bytes(&bytes).is_none());
    }

    #[test]
    fn message_id_composition() {
        let header = Header {
            service_id: 0x1234,
            method_id: 0x5678,
            length: 8,
            client_id: 0,
            session_id: 0,
            protocol_version: 1,
            interface_version: 1,
            message_type: 0,
            return_code: 0,
        };

        assert_eq!(header.message_id(), 0x12345678);
    }

    #[test]
    fn request_id_composition() {
        let header = Header {
            service_id: 0,
            method_id: 0,
            length: 8,
            client_id: 0xABCD,
            session_id: 0x1234,
            protocol_version: 1,
            interface_version: 1,
            message_type: 0,
            return_code: 0,
        };

        assert_eq!(header.request_id(), 0xABCD1234);
    }

    #[test]
    fn protocol_version_constant() {
        assert_eq!(Header::PROTOCOL_VERSION, 0x01);
    }

    #[test]
    fn message_type_constants() {
        assert_eq!(message_type::REQUEST, 0x00);
        assert_eq!(message_type::REQUEST_NO_RETURN, 0x01);
        assert_eq!(message_type::NOTIFICATION, 0x02);
        assert_eq!(message_type::RESPONSE, 0x80);
        assert_eq!(message_type::ERROR, 0x81);
        assert_eq!(message_type::TP_FLAG, 0x20);
    }

    #[test]
    fn return_code_constants() {
        assert_eq!(return_code::E_OK, 0x00);
        assert_eq!(return_code::E_NOT_OK, 0x01);
        assert_eq!(return_code::E_UNKNOWN_SERVICE, 0x02);
        assert_eq!(return_code::E_UNKNOWN_METHOD, 0x03);
        assert_eq!(return_code::E_NOT_READY, 0x04);
        assert_eq!(return_code::E_NOT_REACHABLE, 0x05);
        assert_eq!(return_code::E_TIMEOUT, 0x06);
        assert_eq!(return_code::E_WRONG_PROTOCOL_VERSION, 0x07);
        assert_eq!(return_code::E_WRONG_INTERFACE_VERSION, 0x08);
        assert_eq!(return_code::E_MALFORMED_MESSAGE, 0x09);
        assert_eq!(return_code::E_WRONG_MESSAGE_TYPE, 0x0A);
        assert_eq!(return_code::E_E2E_REPEATED, 0x0B);
        assert_eq!(return_code::E_E2E_WRONG_SEQUENCE, 0x0C);
        assert_eq!(return_code::E_E2E, 0x0D);
        assert_eq!(return_code::E_E2E_NOT_AVAILABLE, 0x0E);
        assert_eq!(return_code::E_E2E_NO_NEW_DATA, 0x0F);
    }

    #[test]
    fn message_type_detection() {
        let mut header = Header {
            service_id: 0x1234,
            method_id: 0x0001,
            length: 8,
            client_id: 0,
            session_id: 0,
            protocol_version: 1,
            interface_version: 1,
            message_type: message_type::REQUEST,
            return_code: 0,
        };

        assert!(header.is_request());
        assert!(!header.is_response());
        assert!(!header.is_notification());
        assert!(!header.is_error());

        header.message_type = message_type::RESPONSE;
        assert!(!header.is_request());
        assert!(header.is_response());

        header.message_type = message_type::NOTIFICATION;
        assert!(header.is_notification());

        header.message_type = message_type::ERROR;
        assert!(header.is_error());
    }

    #[test]
    fn tp_flag_detection() {
        let mut header = Header {
            service_id: 0x1234,
            method_id: 0x0001,
            length: 8,
            client_id: 0,
            session_id: 0,
            protocol_version: 1,
            interface_version: 1,
            message_type: message_type::REQUEST,
            return_code: 0,
        };

        assert!(!header.is_tp_message());

        header.message_type = message_type::REQUEST | message_type::TP_FLAG;
        assert!(header.is_tp_message());

        header.message_type = message_type::RESPONSE | message_type::TP_FLAG;
        assert!(header.is_tp_message());
    }
}
