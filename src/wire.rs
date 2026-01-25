//! # SOME/IP Wire Format
//!
//! This module handles encoding and decoding of SOME/IP messages according
//! to the specification. It provides low-level access to the wire format
//! for testing, debugging, and interoperability verification.
//!
//! ## SOME/IP Header Format (16 bytes)
//!
//! ```text
//! Offset  Size  Field
//! ──────────────────────────────────────────────────────
//!   0      2    Service ID
//!   2      2    Method ID (or Event ID if bit 15 set)
//!   4      4    Length (header from byte 8 + payload)
//!   8      2    Client ID
//!  10      2    Session ID
//!  12      1    Protocol Version (always 0x01)
//!  13      1    Interface Version
//!  14      1    Message Type
//!  15      1    Return Code
//! ──────────────────────────────────────────────────────
//! ```
//!
//! ## Message Types
//!
//! | Value | Name | Description |
//! |-------|------|-------------|
//! | 0x00 | REQUEST | RPC request expecting response |
//! | 0x01 | REQUEST_NO_RETURN | Fire-and-forget request |
//! | 0x02 | NOTIFICATION | Event/notification message |
//! | 0x80 | RESPONSE | RPC response (success or error) |
//! | 0x81 | ERROR | RPC error response (explicit error type) |
//! | 0x20 | TP_REQUEST | Segmented request (TP flag set) |
//! | 0x21 | TP_REQUEST_NO_RETURN | Segmented fire-and-forget |
//! | 0x22 | TP_NOTIFICATION | Segmented notification |
//! | 0xA0 | TP_RESPONSE | Segmented response |
//! | 0xA1 | TP_ERROR | Segmented error |
//!
//! ## Service Discovery (SD) Messages
//!
//! SD uses a special header format with:
//! - Service ID: 0xFFFF
//! - Method ID: 0x8100
//! - Interface Version: 0x01
//!
//! SD messages contain entries (Offer, Find, Subscribe, etc.) and options
//! (IPv4/IPv6 endpoints, configuration).
//!
//! ## Usage
//!
//! This module is primarily for internal use and testing. User code typically
//! doesn't need to interact with wire formats directly.
//!
//! ```
//! use recentip::wire::{Header, MessageType};
//! use bytes::{Buf, BytesMut, BufMut};
//!
//! // Build a header
//! let header = Header {
//!     service_id: 0x1234,
//!     method_id: 0x0001,
//!     length: 8, // header (8 bytes from offset 8) + payload
//!     client_id: 0x0001,
//!     session_id: 0x0001,
//!     protocol_version: 0x01,
//!     interface_version: 0x01,
//!     message_type: MessageType::Request,
//!     return_code: 0x00,
//! };
//!
//! // Serialize to bytes
//! let mut buf = BytesMut::with_capacity(16);
//! header.serialize(&mut buf);
//! assert_eq!(buf.len(), 16);
//!
//! // Parse a header from bytes
//! let parsed = Header::parse(&mut buf.freeze()).expect("valid header");
//! assert_eq!(parsed.message_type, MessageType::Request);
//! ```
//!
//! ## Specification Compliance
//!
//! This module implements wire formats per the SOME/IP specification.
//! Key compliance points:
//!
//! - Protocol version is always 0x01
//! - Session IDs wrap from 0xFFFF to 0x0001 (never 0x0000)
//! - Method IDs use bit 15 to distinguish methods (0) from events (1)
//! - SD always uses UDP port 30490

use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::fmt::Display;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

/// SOME/IP protocol version
pub const PROTOCOL_VERSION: u8 = 0x01;

/// Interface version for SD
pub const SD_INTERFACE_VERSION: u8 = 0x01;

/// Protocol version byte position in header (offset 12)
pub const PROTOCOL_VERSION_OFFSET: usize = 12;

/// Interface version byte position in header (offset 13)
pub const INTERFACE_VERSION_OFFSET: usize = 13;

// ============================================================================
// Magic Cookie (TCP Resynchronization)
// ============================================================================

/// Magic Cookie Service ID (0xFFFF)
pub const MAGIC_COOKIE_SERVICE_ID: u16 = 0xFFFF;

/// Magic Cookie Method ID for client requests (0x0000)
pub const MAGIC_COOKIE_CLIENT_METHOD_ID: u16 = 0x0000;

/// Magic Cookie Method ID for server responses (0x8000)
pub const MAGIC_COOKIE_SERVER_METHOD_ID: u16 = 0x8000;

/// Generate a Magic Cookie message (16 bytes).
///
/// Per `feat_req_recentip_591`: Each TCP segment shall start with a Magic Cookie.
/// Per `feat_req_recentip_592`: Only one Magic Cookie per segment.
///
/// Magic Cookie format:
/// - Service ID: 0xFFFF
/// - Method ID: 0x0000 (client) or 0x8000 (server)
/// - Length: 8 (minimal header, no payload)
/// - Client ID, Session ID: 0xDEAD, 0xBEEF (recognizable pattern)
/// - Protocol Version: 0x01
/// - Interface Version: 0x01
/// - Message Type: 0x01 (Request)
/// - Return Code: 0x00
pub fn magic_cookie_client() -> [u8; 16] {
    [
        0xFF, 0xFF, // Service ID: 0xFFFF
        0x00, 0x00, // Method ID: 0x0000 (client)
        0x00, 0x00, 0x00, 0x08, // Length: 8
        0xDE, 0xAD, // Client ID: 0xDEAD
        0xBE, 0xEF, // Session ID: 0xBEEF
        0x01, // Protocol Version
        0x01, // Interface Version
        0x01, // Message Type: Request
        0x00, // Return Code
    ]
}

/// Generate a server-side Magic Cookie message (16 bytes).
pub fn magic_cookie_server() -> [u8; 16] {
    [
        0xFF, 0xFF, // Service ID: 0xFFFF
        0x80, 0x00, // Method ID: 0x8000 (server)
        0x00, 0x00, 0x00, 0x08, // Length: 8
        0xDE, 0xAD, // Client ID: 0xDEAD
        0xBE, 0xEF, // Session ID: 0xBEEF
        0x01, // Protocol Version
        0x01, // Interface Version
        0x01, // Message Type: Request
        0x00, // Return Code
    ]
}

/// Check if bytes represent a Magic Cookie message.
///
/// Returns true if the first 4 bytes match Magic Cookie pattern
/// (Service ID 0xFFFF, Method ID 0x0000 or 0x8000).
pub fn is_magic_cookie(data: &[u8]) -> bool {
    if data.len() < 4 {
        return false;
    }
    // Check Service ID is 0xFFFF
    if data[0] != 0xFF || data[1] != 0xFF {
        return false;
    }
    // Check Method ID is 0x0000 (client) or 0x8000 (server)
    (data[2] == 0x00 && data[3] == 0x00) || (data[2] == 0x80 && data[3] == 0x00)
}

// ============================================================================
// Version Types
// ============================================================================

/// Represents a semantic version for interface compatibility.
///
/// In SOME/IP, only the major version is transmitted in the header.
/// The minor version is communicated via Service Discovery.
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
        Self { major, minor }
    }

    /// Check if this version is compatible with another version.
    ///
    /// Compatible means same major version, and our minor >= required minor.
    pub fn is_compatible_with(&self, required: &Self) -> bool {
        self.major == required.major && self.minor >= required.minor
    }

    /// Check if exact match (used for strict mode)
    pub fn matches_exactly(&self, other: &Self) -> bool {
        self.major == other.major && self.minor == other.minor
    }

    /// Get the major version byte for wire format
    pub fn wire_major(&self) -> u8 {
        self.major
    }
}

/// Version validation errors
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionError {
    /// Protocol version mismatch
    ProtocolMismatch { expected: u8, found: u8 },
    /// Interface major version mismatch
    InterfaceMajorMismatch { expected: u8, found: u8 },
    /// Interface minor version too old
    InterfaceMinorTooOld { minimum: u32, found: u32 },
}

/// Validate protocol version byte from header.
///
/// Returns Ok if the version matches `PROTOCOL_VERSION` (0x01).
pub fn validate_protocol_version(version_byte: u8) -> Result<(), VersionError> {
    if version_byte == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(VersionError::ProtocolMismatch {
            expected: PROTOCOL_VERSION,
            found: version_byte,
        })
    }
}

/// Validate interface version from header against expected.
///
/// Only checks major version match (minor is SD-only).
pub fn validate_interface_version(
    header_major: u8,
    expected: &InterfaceVersion,
) -> Result<(), VersionError> {
    if header_major == expected.major {
        Ok(())
    } else {
        Err(VersionError::InterfaceMajorMismatch {
            expected: expected.major,
            found: header_major,
        })
    }
}

/// SOME/IP message types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    Request = 0x00,
    RequestNoReturn = 0x01,
    Notification = 0x02,
    Response = 0x80,
    Error = 0x81,
    TpRequest = 0x20,
    TpRequestNoReturn = 0x21,
    TpNotification = 0x22,
    TpResponse = 0xA0,
    TpError = 0xA1,
}

impl MessageType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0x00 => Some(Self::Request),
            0x01 => Some(Self::RequestNoReturn),
            0x02 => Some(Self::Notification),
            0x80 => Some(Self::Response),
            0x81 => Some(Self::Error),
            0x20 => Some(Self::TpRequest),
            0x21 => Some(Self::TpRequestNoReturn),
            0x22 => Some(Self::TpNotification),
            0xA0 => Some(Self::TpResponse),
            0xA1 => Some(Self::TpError),
            _ => None,
        }
    }

    /// Check if this is a TP-flagged message type
    pub fn is_tp(&self) -> bool {
        (*self as u8) & 0x20 != 0
    }

    /// Check if this is a request type (expects response)
    pub fn expects_response(&self) -> bool {
        matches!(self, Self::Request | Self::TpRequest)
    }

    /// Check if this is a fire-and-forget request
    pub fn is_fire_and_forget(&self) -> bool {
        matches!(self, Self::RequestNoReturn | Self::TpRequestNoReturn)
    }

    /// Check if this is a notification
    pub fn is_notification(&self) -> bool {
        matches!(self, Self::Notification | Self::TpNotification)
    }

    /// Check if this is a response type
    pub fn is_response(&self) -> bool {
        matches!(
            self,
            Self::Response | Self::TpResponse | Self::Error | Self::TpError
        )
    }

    /// Get the corresponding TP-flagged type
    pub fn with_tp_flag(&self) -> Option<Self> {
        match self {
            Self::Request => Some(Self::TpRequest),
            Self::RequestNoReturn => Some(Self::TpRequestNoReturn),
            Self::Notification => Some(Self::TpNotification),
            Self::Response => Some(Self::TpResponse),
            Self::Error => Some(Self::TpError),
            _ => None, // Already TP-flagged
        }
    }

    /// Get the base type without TP flag
    pub fn without_tp_flag(&self) -> Self {
        match self {
            Self::TpRequest => Self::Request,
            Self::TpRequestNoReturn => Self::RequestNoReturn,
            Self::TpNotification => Self::Notification,
            Self::TpResponse => Self::Response,
            Self::TpError => Self::Error,
            other => *other,
        }
    }

    /// Get the expected response type for this message type
    pub fn expected_response_type(&self) -> Option<Self> {
        match self {
            Self::Request => Some(Self::Response),
            Self::TpRequest => Some(Self::TpResponse),
            _ => None, // Other types don't expect responses
        }
    }

    /// Check if this type is valid as a response to the given request type
    pub fn is_valid_response_to(&self, request_type: Self) -> bool {
        match request_type {
            Self::Request => matches!(self, Self::Response | Self::Error),
            Self::TpRequest => {
                matches!(self, Self::TpResponse | Self::TpError)
            }
            _ => false, // Other types don't expect responses
        }
    }
}

/// SOME/IP header (16 bytes)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// Service ID
    pub service_id: u16,
    /// Method ID or Event ID
    pub method_id: u16,
    /// Length of payload + 8 bytes (`client_id`, `session_id`, `protocol_version`, `interface_version`, `message_type`, `return_code`)
    pub length: u32,
    /// Client ID
    pub client_id: u16,
    /// Session ID
    pub session_id: u16,
    /// Protocol version (always 0x01)
    pub protocol_version: u8,
    /// Interface version
    pub interface_version: u8,
    /// Message type
    pub message_type: MessageType,
    /// Return code
    pub return_code: u8,
}

impl Header {
    pub const SIZE: usize = 16;

    /// Parse a header from bytes
    pub fn parse(buf: &mut impl Buf) -> Option<Self> {
        if buf.remaining() < Self::SIZE {
            return None;
        }

        let service_id = buf.get_u16();
        let method_id = buf.get_u16();
        let length = buf.get_u32();
        let client_id = buf.get_u16();
        let session_id = buf.get_u16();
        let protocol_version = buf.get_u8();
        let interface_version = buf.get_u8();
        let message_type_raw = buf.get_u8();
        let return_code = buf.get_u8();

        let message_type = MessageType::from_u8(message_type_raw)?;

        // feat_req_recentip_798: Messages with Length < 8 shall be ignored
        // The length field represents payload size + 8 bytes for the header tail
        // (client_id, session_id, protocol_version, interface_version, message_type, return_code)
        // Therefore, the minimum valid length is 8 (no payload)
        if length < 8 {
            return None;
        }

        Some(Self {
            service_id,
            method_id,
            length,
            client_id,
            session_id,
            protocol_version,
            interface_version,
            message_type,
            return_code,
        })
    }

    /// Serialize the header to bytes
    pub fn serialize(&self, buf: &mut impl BufMut) {
        buf.put_u16(self.service_id);
        buf.put_u16(self.method_id);
        buf.put_u32(self.length);
        buf.put_u16(self.client_id);
        buf.put_u16(self.session_id);
        buf.put_u8(self.protocol_version);
        buf.put_u8(self.interface_version);
        buf.put_u8(self.message_type as u8);
        buf.put_u8(self.return_code);
    }

    /// Get the payload length (excluding the 8 bytes counted in length field)
    pub fn payload_length(&self) -> usize {
        self.length.saturating_sub(8) as usize
    }
}

/// A complete SOME/IP message (header + payload)
#[derive(Debug, Clone)]
#[allow(dead_code)] // Used for future TCP framing
pub struct Message {
    pub header: Header,
    pub payload: Bytes,
}

#[allow(dead_code)] // Used for future TCP framing
impl Message {
    /// Parse a message from bytes
    pub fn parse(buf: &mut impl Buf) -> Option<Self> {
        let header = Header::parse(buf)?;
        let payload_len = header.payload_length();

        if buf.remaining() < payload_len {
            return None;
        }

        let payload = buf.copy_to_bytes(payload_len);

        Some(Self { header, payload })
    }

    /// Serialize the message to bytes
    pub fn serialize(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(Header::SIZE + self.payload.len());
        self.header.serialize(&mut buf);
        buf.extend_from_slice(&self.payload);
        buf.freeze()
    }
}

// ============================================================================
// SERVICE DISCOVERY MESSAGES
// ============================================================================

/// SD message type identifiers
pub const SD_SERVICE_ID: u16 = 0xFFFF;
pub const SD_METHOD_ID: u16 = 0x8100;
pub const SD_CLIENT_ID: u16 = 0x0000;

/// L4 Protocol types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum L4Protocol {
    Tcp = 0x06,
    Udp = 0x11,
}

impl L4Protocol {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x06 => Some(Self::Tcp),
            0x11 => Some(Self::Udp),
            _ => None,
        }
    }
}

/// SD entry types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SdEntryType {
    FindService = 0x00,
    OfferService = 0x01,
    SubscribeEventgroup = 0x06,
    SubscribeEventgroupAck = 0x07,
}

impl SdEntryType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x00 => Some(Self::FindService),
            0x01 => Some(Self::OfferService),
            0x06 => Some(Self::SubscribeEventgroup),
            0x07 => Some(Self::SubscribeEventgroupAck),
            _ => None,
        }
    }
}

/// A parsed SD entry
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdEntry {
    pub entry_type: SdEntryType,
    pub service_id: u16,
    pub instance_id: u16,
    pub major_version: u8,
    pub ttl: u32,           // 24-bit, 0 = stop/nack
    pub minor_version: u32, // For Type 1 (service entries)
    pub eventgroup_id: u16, // For Type 2 (eventgroup entries)
    pub counter: u8,        // For eventgroup entries
    /// Index of first option in run 1
    pub index_1st_option: u8,
    /// Index of first option in run 2
    pub index_2nd_option: u8,
    /// Number of options in run 1
    pub num_options_1: u8,
    /// Number of options in run 2
    pub num_options_2: u8,
}

impl Display for SdEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.entry_type {
            SdEntryType::FindService | SdEntryType::OfferService => {
                write!(
                    f,
                    "{:?} Service {}:{}, v{}.{} TTL={}",
                    self.entry_type,
                    self.service_id,
                    self.instance_id,
                    self.major_version,
                    self.minor_version,
                    self.ttl
                )
            }
            SdEntryType::SubscribeEventgroup | SdEntryType::SubscribeEventgroupAck => {
                write!(
                    f,
                    "{:?} Evtgrp {} Service {}:{}, v{}. TTL={} #{}",
                    self.entry_type,
                    self.eventgroup_id,
                    self.service_id,
                    self.instance_id,
                    self.major_version,
                    self.ttl,
                    self.counter
                )
            }
        }
    }
}

impl SdEntry {
    pub const SIZE: usize = 16;

    /// Check if this is a stop/nack entry (TTL = 0)
    pub fn is_stop(&self) -> bool {
        self.ttl == 0
    }

    pub fn parse(buf: &mut impl Buf) -> Option<Self> {
        if buf.remaining() < Self::SIZE {
            return None;
        }

        let entry_type_raw = buf.get_u8();
        let index_1st_option = buf.get_u8();
        let index_2nd_option = buf.get_u8();
        let num_options = buf.get_u8();
        let num_options_1 = (num_options >> 4) & 0x0F;
        let num_options_2 = num_options & 0x0F;
        let service_id = buf.get_u16();
        let instance_id = buf.get_u16();
        let major_version = buf.get_u8();
        let ttl_bytes = [0, buf.get_u8(), buf.get_u8(), buf.get_u8()];
        let ttl = u32::from_be_bytes(ttl_bytes);

        let entry_type = SdEntryType::from_u8(entry_type_raw)?;

        // Bytes 12-15 depend on entry type
        let (minor_version, eventgroup_id, counter) = match entry_type {
            SdEntryType::FindService | SdEntryType::OfferService => {
                let minor = buf.get_u32();
                (minor, 0, 0)
            }
            SdEntryType::SubscribeEventgroup | SdEntryType::SubscribeEventgroupAck => {
                let _reserved = buf.get_u8();
                let counter = buf.get_u8();
                let eventgroup = buf.get_u16();
                (0, eventgroup, counter)
            }
        };

        Some(Self {
            entry_type,
            service_id,
            instance_id,
            major_version,
            ttl,
            minor_version,
            eventgroup_id,
            counter,
            index_1st_option,
            index_2nd_option,
            num_options_1,
            num_options_2,
        })
    }

    pub fn serialize(&self, buf: &mut impl BufMut) {
        buf.put_u8(self.entry_type as u8);
        buf.put_u8(self.index_1st_option);
        buf.put_u8(self.index_2nd_option);
        buf.put_u8((self.num_options_1 << 4) | (self.num_options_2 & 0x0F));
        buf.put_u16(self.service_id);
        buf.put_u16(self.instance_id);
        buf.put_u8(self.major_version);
        // TTL is 24-bit
        buf.put_u8(((self.ttl >> 16) & 0xFF) as u8);
        buf.put_u8(((self.ttl >> 8) & 0xFF) as u8);
        buf.put_u8((self.ttl & 0xFF) as u8);

        match self.entry_type {
            SdEntryType::FindService | SdEntryType::OfferService => {
                buf.put_u32(self.minor_version);
            }
            SdEntryType::SubscribeEventgroup | SdEntryType::SubscribeEventgroupAck => {
                buf.put_u8(0); // reserved
                buf.put_u8(self.counter);
                buf.put_u16(self.eventgroup_id);
            }
        }
    }

    /// Create a `FindService` entry
    pub fn find_service(
        service_id: u16,
        instance_id: u16,
        major_version: u8,
        minor_version: u32,
        ttl: u32,
    ) -> Self {
        Self {
            entry_type: SdEntryType::FindService,
            service_id,
            instance_id,
            major_version,
            ttl,
            minor_version,
            eventgroup_id: 0,
            counter: 0,
            index_1st_option: 0,
            index_2nd_option: 0,
            num_options_1: 0,
            num_options_2: 0,
        }
    }

    /// Create an `OfferService` entry
    pub fn offer_service(
        service_id: u16,
        instance_id: u16,
        major_version: u8,
        minor_version: u32,
        ttl: u32,
        option_index: u8,
        num_options: u8,
    ) -> Self {
        Self {
            entry_type: SdEntryType::OfferService,
            service_id,
            instance_id,
            major_version,
            ttl,
            minor_version,
            eventgroup_id: 0,
            counter: 0,
            index_1st_option: option_index,
            index_2nd_option: 0,
            num_options_1: num_options,
            num_options_2: 0,
        }
    }

    /// Create a `StopOfferService` entry (`OfferService` with TTL=0)
    pub fn stop_offer_service(
        service_id: u16,
        instance_id: u16,
        major_version: u8,
        minor_version: u32,
    ) -> Self {
        Self::offer_service(
            service_id,
            instance_id,
            major_version,
            minor_version,
            0,
            0,
            0,
        )
    }

    /// Create a `SubscribeEventgroup` entry
    pub fn subscribe_eventgroup(
        service_id: u16,
        instance_id: u16,
        major_version: u8,
        eventgroup_id: u16,
        ttl: u32,
        counter: u8,
    ) -> Self {
        Self {
            entry_type: SdEntryType::SubscribeEventgroup,
            service_id,
            instance_id,
            major_version,
            ttl,
            minor_version: 0,
            eventgroup_id,
            counter,
            index_1st_option: 0,
            index_2nd_option: 0,
            num_options_1: 0,
            num_options_2: 0,
        }
    }

    /// Create a `SubscribeEventgroupAck` entry
    pub fn subscribe_eventgroup_ack(
        service_id: u16,
        instance_id: u16,
        major_version: u8,
        eventgroup_id: u16,
        ttl: u32,
        counter: u8,
    ) -> Self {
        Self {
            entry_type: SdEntryType::SubscribeEventgroupAck,
            service_id,
            instance_id,
            major_version,
            ttl,
            minor_version: 0,
            eventgroup_id,
            counter,
            index_1st_option: 0,
            index_2nd_option: 0,
            num_options_1: 0,
            num_options_2: 0,
        }
    }

    /// Create a `SubscribeEventgroupNack` entry (TTL=0 indicates rejection)
    ///
    /// Per `feat_req_recentipsd_1137`: Respond with `SubscribeEventgroupNack` for invalid subscribe
    pub fn subscribe_eventgroup_nack(
        service_id: u16,
        instance_id: u16,
        major_version: u8,
        eventgroup_id: u16,
        counter: u8,
    ) -> Self {
        Self {
            entry_type: SdEntryType::SubscribeEventgroupAck, // Same type, TTL=0 indicates NACK
            service_id,
            instance_id,
            major_version,
            ttl: 0, // TTL=0 indicates NACK
            minor_version: 0,
            eventgroup_id,
            counter,
            index_1st_option: 0,
            index_2nd_option: 0,
            num_options_1: 0,
            num_options_2: 0,
        }
    }
}

/// SD Option (IPv4 endpoint, multicast, etc.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdOption {
    Ipv4Endpoint {
        addr: Ipv4Addr,
        port: u16,
        protocol: L4Protocol,
    },
    Ipv4Multicast {
        addr: Ipv4Addr,
        port: u16,
    },
    Unknown {
        option_type: u8,
        data: Bytes,
    },
}

impl SdOption {
    /// Parse an option from bytes, returns (option, bytes consumed)
    pub fn parse(buf: &mut impl Buf) -> Option<Self> {
        if buf.remaining() < 3 {
            return None;
        }

        let length = buf.get_u16() as usize;
        let option_type = buf.get_u8();

        if buf.remaining() < length {
            return None;
        }

        match option_type {
            0x04 => {
                // IPv4 Endpoint
                if length < 9 {
                    return None;
                }
                let _reserved = buf.get_u8();
                let a = buf.get_u8();
                let b = buf.get_u8();
                let c = buf.get_u8();
                let d = buf.get_u8();
                let _reserved2 = buf.get_u8();
                let protocol = L4Protocol::from_u8(buf.get_u8())?;
                let port = buf.get_u16();
                Some(Self::Ipv4Endpoint {
                    addr: Ipv4Addr::new(a, b, c, d),
                    port,
                    protocol,
                })
            }
            0x14 => {
                // IPv4 Multicast
                if length < 9 {
                    return None;
                }
                let _reserved = buf.get_u8();
                let a = buf.get_u8();
                let b = buf.get_u8();
                let c = buf.get_u8();
                let d = buf.get_u8();
                let _reserved2 = buf.get_u8();
                let _protocol = buf.get_u8();
                let port = buf.get_u16();
                Some(Self::Ipv4Multicast {
                    addr: Ipv4Addr::new(a, b, c, d),
                    port,
                })
            }
            _ => {
                // Unknown option - skip
                let data = buf.copy_to_bytes(length);
                Some(Self::Unknown { option_type, data })
            }
        }
    }

    pub fn serialize(&self, buf: &mut impl BufMut) {
        match self {
            Self::Ipv4Endpoint {
                addr,
                port,
                protocol,
            } => {
                buf.put_u16(9); // length
                buf.put_u8(0x04); // type
                buf.put_u8(0); // reserved
                buf.put_slice(&addr.octets());
                buf.put_u8(0); // reserved
                buf.put_u8(*protocol as u8);
                buf.put_u16(*port);
            }
            Self::Ipv4Multicast { addr, port } => {
                buf.put_u16(9); // length
                buf.put_u8(0x14); // type
                buf.put_u8(0); // reserved
                buf.put_slice(&addr.octets());
                buf.put_u8(0); // reserved
                buf.put_u8(L4Protocol::Udp as u8);
                buf.put_u16(*port);
            }
            Self::Unknown { option_type, data } => {
                buf.put_u16(data.len() as u16);
                buf.put_u8(*option_type);
                buf.put_slice(data);
            }
        }
    }

    /// Size in bytes when serialized
    pub fn size(&self) -> usize {
        match self {
            Self::Ipv4Endpoint { .. } | Self::Ipv4Multicast { .. } => 12, // 2 + 1 + 9
            Self::Unknown { data, .. } => 3 + data.len(),
        }
    }
}

/// Complete SD message (parsed payload)
#[derive(Debug, Clone)]
pub struct SdMessage {
    pub flags: u8,
    pub entries: Vec<SdEntry>,
    pub options: Vec<SdOption>,
}

impl SdMessage {
    /// Reboot flag
    pub const FLAG_REBOOT: u8 = 0x80;
    /// Unicast flag
    pub const FLAG_UNICAST: u8 = 0x40;

    /// Create a new SD message
    pub fn new(flags: u8) -> Self {
        Self {
            flags,
            entries: Vec::new(),
            options: Vec::new(),
        }
    }

    /// Create an SD message for initial announcement (reboot + unicast flags)
    pub fn initial() -> Self {
        Self::new(Self::FLAG_REBOOT | Self::FLAG_UNICAST)
    }

    /// Parse from bytes (the payload after SOME/IP header)
    pub fn parse(buf: &mut impl Buf) -> Option<Self> {
        if buf.remaining() < 12 {
            return None;
        }

        let flags = buf.get_u8();
        let _reserved = [buf.get_u8(), buf.get_u8(), buf.get_u8()];

        let entries_len = buf.get_u32() as usize;
        if buf.remaining() < entries_len {
            return None;
        }

        let mut entries = Vec::new();
        let mut entries_consumed = 0;
        while entries_consumed + SdEntry::SIZE <= entries_len {
            if let Some(entry) = SdEntry::parse(buf) {
                entries.push(entry);
                entries_consumed += SdEntry::SIZE;
            } else {
                return None;
            }
        }
        // Skip any remaining bytes in entries array
        if entries_consumed < entries_len {
            buf.advance(entries_len - entries_consumed);
        }

        if buf.remaining() < 4 {
            return None;
        }
        let options_len = buf.get_u32() as usize;
        if buf.remaining() < options_len {
            return None;
        }

        let mut options = Vec::new();
        let options_start = buf.remaining();
        while options_start - buf.remaining() < options_len && buf.remaining() >= 3 {
            if let Some(option) = SdOption::parse(buf) {
                options.push(option);
            } else {
                break;
            }
        }

        Some(Self {
            flags,
            entries,
            options,
        })
    }

    /// Serialize to bytes (just the SD payload, without SOME/IP header)
    pub fn serialize_payload(&self) -> Bytes {
        let entries_len = self.entries.len() * SdEntry::SIZE;
        let options_len: usize = self.options.iter().map(SdOption::size).sum();
        let total_len = 4 + 4 + entries_len + 4 + options_len;

        let mut buf = BytesMut::with_capacity(total_len);

        // Flags + reserved
        buf.put_u8(self.flags);
        buf.put_u8(0);
        buf.put_u8(0);
        buf.put_u8(0);

        // Entries array
        buf.put_u32(entries_len as u32);
        for entry in &self.entries {
            entry.serialize(&mut buf);
        }

        // Options array
        buf.put_u32(options_len as u32);
        for option in &self.options {
            option.serialize(&mut buf);
        }

        buf.freeze()
    }

    /// Serialize as a complete SOME/IP message
    pub fn serialize(&self, session_id: u16) -> Bytes {
        let payload = self.serialize_payload();

        let header = Header {
            service_id: SD_SERVICE_ID,
            method_id: SD_METHOD_ID,
            length: 8 + payload.len() as u32,
            client_id: SD_CLIENT_ID,
            session_id,
            protocol_version: PROTOCOL_VERSION,
            interface_version: SD_INTERFACE_VERSION,
            message_type: MessageType::Notification,
            return_code: 0x00,
        };

        let mut buf = BytesMut::with_capacity(Header::SIZE + payload.len());
        header.serialize(&mut buf);
        buf.extend_from_slice(&payload);
        buf.freeze()
    }

    /// Add an entry and return its index
    pub fn add_entry(&mut self, entry: SdEntry) -> usize {
        let idx = self.entries.len();
        self.entries.push(entry);
        idx
    }

    /// Add an option and return its index
    pub fn add_option(&mut self, option: SdOption) -> u8 {
        let idx = self.options.len() as u8;
        self.options.push(option);
        idx
    }

    /// Get the UDP endpoint from options for an entry.
    ///
    /// Searches both option runs (first and second) as per SOME/IP-SD specification.
    pub fn get_udp_endpoint(&self, entry: &SdEntry) -> Option<SocketAddr> {
        self.get_endpoint_with_protocol(entry, L4Protocol::Udp)
    }

    /// Get the TCP endpoint from options for an entry.
    ///
    /// Searches both option runs (first and second) as per SOME/IP-SD specification.
    #[allow(dead_code)] // Will be used for TCP transport
    pub fn get_tcp_endpoint(&self, entry: &SdEntry) -> Option<SocketAddr> {
        self.get_endpoint_with_protocol(entry, L4Protocol::Tcp)
    }

    /// Search for an endpoint option with the specified protocol in both option runs.
    fn get_endpoint_with_protocol(
        &self,
        entry: &SdEntry,
        target_protocol: L4Protocol,
    ) -> Option<SocketAddr> {
        // First option run
        let start1 = entry.index_1st_option as usize;
        let count1 = entry.num_options_1 as usize;
        for i in start1..(start1 + count1) {
            if let Some(SdOption::Ipv4Endpoint {
                addr,
                port,
                protocol,
            }) = self.options.get(i)
            {
                if *protocol == target_protocol {
                    return Some(SocketAddr::V4(SocketAddrV4::new(*addr, *port)));
                }
            }
        }

        // Second option run
        let start2 = entry.index_2nd_option as usize;
        let count2 = entry.num_options_2 as usize;
        for i in start2..(start2 + count2) {
            if let Some(SdOption::Ipv4Endpoint {
                addr,
                port,
                protocol,
            }) = self.options.get(i)
            {
                if *protocol == target_protocol {
                    return Some(SocketAddr::V4(SocketAddrV4::new(*addr, *port)));
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_log::test]
    fn test_header_roundtrip() {
        let header = Header {
            service_id: 0x1234,
            method_id: 0x5678,
            length: 16,
            client_id: 0x0001,
            session_id: 0x0001,
            protocol_version: PROTOCOL_VERSION,
            interface_version: 0x01,
            message_type: MessageType::Request,
            return_code: 0x00,
        };

        let mut buf = BytesMut::new();
        header.serialize(&mut buf);

        let mut cursor = buf.freeze();
        let parsed = Header::parse(&mut cursor).unwrap();

        assert_eq!(header, parsed);
    }

    #[test_log::test]
    fn test_sd_entry_roundtrip() {
        let entry = SdEntry::offer_service(0x1234, 0x0001, 1, 0, 3600, 0, 1);

        let mut buf = BytesMut::new();
        entry.serialize(&mut buf);

        let mut cursor = buf.freeze();
        let parsed = SdEntry::parse(&mut cursor).unwrap();

        assert_eq!(entry.entry_type, parsed.entry_type);
        assert_eq!(entry.service_id, parsed.service_id);
        assert_eq!(entry.instance_id, parsed.instance_id);
        assert_eq!(entry.major_version, parsed.major_version);
        assert_eq!(entry.ttl, parsed.ttl);
        assert_eq!(entry.minor_version, parsed.minor_version);
    }

    #[test_log::test]
    fn test_sd_option_roundtrip() {
        let option = SdOption::Ipv4Endpoint {
            addr: Ipv4Addr::new(192, 168, 1, 100),
            port: 30490,
            protocol: L4Protocol::Udp,
        };

        let mut buf = BytesMut::new();
        option.serialize(&mut buf);

        let mut cursor = buf.freeze();
        let parsed = SdOption::parse(&mut cursor).unwrap();

        assert_eq!(option, parsed);
    }

    #[test_log::test]
    fn test_sd_message_roundtrip() {
        let mut msg = SdMessage::initial();
        let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
            addr: Ipv4Addr::new(192, 168, 1, 100),
            port: 30501,
            protocol: L4Protocol::Udp,
        });
        msg.add_entry(SdEntry::offer_service(
            0x1234, 0x0001, 1, 0, 3600, opt_idx, 1,
        ));

        let bytes = msg.serialize(1);

        // Skip SOME/IP header
        let mut cursor = bytes.slice(Header::SIZE..);
        let parsed = SdMessage::parse(&mut cursor).unwrap();

        assert_eq!(msg.flags, parsed.flags);
        assert_eq!(msg.entries.len(), parsed.entries.len());
        assert_eq!(msg.options.len(), parsed.options.len());
    }

    #[test_log::test]
    fn test_get_udp_endpoint_from_first_option_run() {
        let mut msg = SdMessage::initial();
        let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
            addr: Ipv4Addr::new(192, 168, 1, 100),
            port: 30501,
            protocol: L4Protocol::Udp,
        });
        let entry = SdEntry::offer_service(0x1234, 0x0001, 1, 0, 3600, opt_idx, 1);

        let endpoint = msg.get_udp_endpoint(&entry);
        assert_eq!(
            endpoint,
            Some(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(192, 168, 1, 100),
                30501
            )))
        );
    }

    #[test_log::test]
    fn test_get_udp_endpoint_from_second_option_run() {
        // Scenario: First option run has multicast, second has UDP endpoint
        let mut msg = SdMessage::initial();

        // Option 0: Multicast (in first run)
        msg.add_option(SdOption::Ipv4Multicast {
            addr: Ipv4Addr::new(239, 255, 0, 1),
            port: 30490,
        });

        // Option 1: UDP endpoint (in second run)
        msg.add_option(SdOption::Ipv4Endpoint {
            addr: Ipv4Addr::new(192, 168, 1, 200),
            port: 30502,
            protocol: L4Protocol::Udp,
        });

        // Entry with two option runs:
        // - First run: option 0 (multicast)
        // - Second run: option 1 (UDP endpoint)
        let mut entry = SdEntry::offer_service(0x1234, 0x0001, 1, 0, 3600, 0, 1);
        entry.index_2nd_option = 1;
        entry.num_options_2 = 1;

        let endpoint = msg.get_udp_endpoint(&entry);
        assert_eq!(
            endpoint,
            Some(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(192, 168, 1, 200),
                30502
            )))
        );
    }

    #[test_log::test]
    fn test_get_tcp_endpoint_from_second_option_run() {
        let mut msg = SdMessage::initial();

        // Option 0: UDP endpoint (not what we're looking for)
        msg.add_option(SdOption::Ipv4Endpoint {
            addr: Ipv4Addr::new(192, 168, 1, 100),
            port: 30501,
            protocol: L4Protocol::Udp,
        });

        // Option 1: TCP endpoint (in second run)
        msg.add_option(SdOption::Ipv4Endpoint {
            addr: Ipv4Addr::new(192, 168, 1, 100),
            port: 30502,
            protocol: L4Protocol::Tcp,
        });

        // First run has UDP, second run has TCP
        let mut entry = SdEntry::offer_service(0x1234, 0x0001, 1, 0, 3600, 0, 1);
        entry.index_2nd_option = 1;
        entry.num_options_2 = 1;

        let tcp_endpoint = msg.get_tcp_endpoint(&entry);
        assert_eq!(
            tcp_endpoint,
            Some(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(192, 168, 1, 100),
                30502
            )))
        );

        // UDP should come from first run
        let udp_endpoint = msg.get_udp_endpoint(&entry);
        assert_eq!(
            udp_endpoint,
            Some(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(192, 168, 1, 100),
                30501
            )))
        );
    }

    #[test_log::test]
    fn test_get_endpoint_returns_none_when_not_found() {
        let mut msg = SdMessage::initial();

        // Only add a multicast option
        msg.add_option(SdOption::Ipv4Multicast {
            addr: Ipv4Addr::new(239, 255, 0, 1),
            port: 30490,
        });

        let entry = SdEntry::offer_service(0x1234, 0x0001, 1, 0, 3600, 0, 1);

        assert!(msg.get_udp_endpoint(&entry).is_none());
        assert!(msg.get_tcp_endpoint(&entry).is_none());
    }

    #[test_log::test]
    fn test_header_rejects_short_input() {
        use bytes::Bytes;

        // Empty buffer
        let mut empty = Bytes::new();
        assert!(
            Header::parse(&mut empty).is_none(),
            "Empty buffer should return None"
        );

        // 1 byte - way too short
        let mut one_byte = Bytes::from_static(&[0x12]);
        assert!(
            Header::parse(&mut one_byte).is_none(),
            "1 byte should return None"
        );

        // 15 bytes - just one byte short
        let mut almost = Bytes::from_static(&[
            0x12, 0x34, // service_id
            0x00, 0x01, // method_id
            0x00, 0x00, 0x00, 0x08, // length
            0x00, 0x01, // client_id
            0x00, 0x01, // session_id
            0x01, // protocol_version
            0x01, // interface_version
            0x00, // message_type - only 15 bytes, missing return_code
        ]);
        assert!(
            Header::parse(&mut almost).is_none(),
            "15 bytes should return None"
        );

        // 16 bytes - exactly right
        let mut exact = Bytes::from_static(&[
            0x12, 0x34, // service_id
            0x00, 0x01, // method_id
            0x00, 0x00, 0x00, 0x08, // length
            0x00, 0x01, // client_id
            0x00, 0x01, // session_id
            0x01, // protocol_version
            0x01, // interface_version
            0x00, // message_type (Request)
            0x00, // return_code
        ]);
        assert!(
            Header::parse(&mut exact).is_some(),
            "16 bytes should parse successfully"
        );
    }

    #[test_log::test]
    fn test_header_rejects_invalid_length() {
        use bytes::Bytes;

        // feat_req_recentip_798: Messages with Length < 8 shall be ignored
        // Test with Length = 0
        let mut zero_length = Bytes::from_static(&[
            0x12, 0x34, // service_id
            0x00, 0x01, // method_id
            0x00, 0x00, 0x00, 0x00, // length = 0 (INVALID)
            0x00, 0x01, // client_id
            0x00, 0x01, // session_id
            0x01, // protocol_version
            0x01, // interface_version
            0x00, // message_type (Request)
            0x00, // return_code
        ]);
        assert!(
            Header::parse(&mut zero_length).is_none(),
            "Length=0 should be rejected"
        );

        // Test with Length = 4
        let mut length_four = Bytes::from_static(&[
            0x12, 0x34, // service_id
            0x00, 0x01, // method_id
            0x00, 0x00, 0x00, 0x04, // length = 4 (INVALID, must be >= 8)
            0x00, 0x01, // client_id
            0x00, 0x01, // session_id
            0x01, // protocol_version
            0x01, // interface_version
            0x00, // message_type (Request)
            0x00, // return_code
        ]);
        assert!(
            Header::parse(&mut length_four).is_none(),
            "Length=4 should be rejected"
        );

        // Test with Length = 7 (boundary)
        let mut length_seven = Bytes::from_static(&[
            0x12, 0x34, // service_id
            0x00, 0x01, // method_id
            0x00, 0x00, 0x00, 0x07, // length = 7 (INVALID, must be >= 8)
            0x00, 0x01, // client_id
            0x00, 0x01, // session_id
            0x01, // protocol_version
            0x01, // interface_version
            0x00, // message_type (Request)
            0x00, // return_code
        ]);
        assert!(
            Header::parse(&mut length_seven).is_none(),
            "Length=7 should be rejected"
        );

        // Test with Length = 8 (minimum valid)
        let mut length_eight = Bytes::from_static(&[
            0x12, 0x34, // service_id
            0x00, 0x01, // method_id
            0x00, 0x00, 0x00, 0x08, // length = 8 (VALID, minimum)
            0x00, 0x01, // client_id
            0x00, 0x01, // session_id
            0x01, // protocol_version
            0x01, // interface_version
            0x00, // message_type (Request)
            0x00, // return_code
        ]);
        assert!(
            Header::parse(&mut length_eight).is_some(),
            "Length=8 should be accepted"
        );
    }

    // ========================================================================
    // MessageType Unit Tests
    // ========================================================================

    #[test_log::test]
    fn message_type_parse_all_valid_values() {
        // Non-TP types
        assert_eq!(MessageType::from_u8(0x00), Some(MessageType::Request));
        assert_eq!(
            MessageType::from_u8(0x01),
            Some(MessageType::RequestNoReturn)
        );
        assert_eq!(MessageType::from_u8(0x02), Some(MessageType::Notification));
        assert_eq!(MessageType::from_u8(0x80), Some(MessageType::Response));
        assert_eq!(MessageType::from_u8(0x81), Some(MessageType::Error));

        // TP-flagged types
        assert_eq!(MessageType::from_u8(0x20), Some(MessageType::TpRequest));
        assert_eq!(
            MessageType::from_u8(0x21),
            Some(MessageType::TpRequestNoReturn)
        );
        assert_eq!(
            MessageType::from_u8(0x22),
            Some(MessageType::TpNotification)
        );
        assert_eq!(MessageType::from_u8(0xA0), Some(MessageType::TpResponse));
        assert_eq!(MessageType::from_u8(0xA1), Some(MessageType::TpError));
    }

    #[test_log::test]
    fn message_type_rejects_invalid_values() {
        assert!(MessageType::from_u8(0x03).is_none());
        assert!(MessageType::from_u8(0x23).is_none());
        assert!(MessageType::from_u8(0x82).is_none());
        assert!(MessageType::from_u8(0xFF).is_none());
    }

    #[test_log::test]
    fn message_type_tp_flag_is_bit_5() {
        assert!(!MessageType::Request.is_tp());
        assert!(!MessageType::RequestNoReturn.is_tp());
        assert!(!MessageType::Notification.is_tp());
        assert!(!MessageType::Response.is_tp());
        assert!(!MessageType::Error.is_tp());

        assert!(MessageType::TpRequest.is_tp());
        assert!(MessageType::TpRequestNoReturn.is_tp());
        assert!(MessageType::TpNotification.is_tp());
        assert!(MessageType::TpResponse.is_tp());
        assert!(MessageType::TpError.is_tp());
    }

    #[test_log::test]
    fn message_type_request_expects_response() {
        assert!(MessageType::Request.expects_response());
        assert!(MessageType::TpRequest.expects_response());

        assert!(!MessageType::RequestNoReturn.expects_response());
        assert!(!MessageType::Notification.expects_response());
        assert!(!MessageType::Response.expects_response());
    }

    #[test_log::test]
    fn message_type_request_no_return_is_fire_and_forget() {
        assert!(MessageType::RequestNoReturn.is_fire_and_forget());
        assert!(MessageType::TpRequestNoReturn.is_fire_and_forget());
        assert!(!MessageType::Request.is_fire_and_forget());
    }

    #[test_log::test]
    fn message_type_notification_classification() {
        assert!(MessageType::Notification.is_notification());
        assert!(MessageType::TpNotification.is_notification());
        assert!(!MessageType::Request.is_notification());
        assert!(!MessageType::Response.is_notification());
    }

    #[test_log::test]
    fn message_type_tp_flag_conversion() {
        assert_eq!(
            MessageType::Request.with_tp_flag(),
            Some(MessageType::TpRequest)
        );
        assert_eq!(
            MessageType::RequestNoReturn.with_tp_flag(),
            Some(MessageType::TpRequestNoReturn)
        );
        assert_eq!(
            MessageType::Notification.with_tp_flag(),
            Some(MessageType::TpNotification)
        );
        assert_eq!(
            MessageType::Response.with_tp_flag(),
            Some(MessageType::TpResponse)
        );
        assert_eq!(
            MessageType::Error.with_tp_flag(),
            Some(MessageType::TpError)
        );
        assert_eq!(MessageType::TpRequest.with_tp_flag(), None);
    }

    #[test_log::test]
    fn message_type_tp_flag_removal() {
        assert_eq!(
            MessageType::TpRequest.without_tp_flag(),
            MessageType::Request
        );
        assert_eq!(
            MessageType::TpRequestNoReturn.without_tp_flag(),
            MessageType::RequestNoReturn
        );
        assert_eq!(
            MessageType::TpNotification.without_tp_flag(),
            MessageType::Notification
        );
        assert_eq!(
            MessageType::TpResponse.without_tp_flag(),
            MessageType::Response
        );
        assert_eq!(MessageType::TpError.without_tp_flag(), MessageType::Error);
        assert_eq!(MessageType::Request.without_tp_flag(), MessageType::Request);
    }

    #[test_log::test]
    fn message_type_response_bit_pattern() {
        assert_eq!(0x00u8 | 0x80, 0x80); // Request -> Response
        assert_eq!(0x01u8 | 0x80, 0x81); // RequestNoReturn -> Error
        assert_eq!(0x20u8 | 0x80, 0xA0); // TpRequest -> TpResponse
        assert_eq!(0x21u8 | 0x80, 0xA1); // TpRequestNoReturn -> TpError
    }

    #[test_log::test]
    fn message_type_tp_bit_pattern() {
        assert_eq!(0x00u8 | 0x20, 0x20); // Request -> TpRequest
        assert_eq!(0x01u8 | 0x20, 0x21); // RequestNoReturn -> TpRequestNoReturn
        assert_eq!(0x02u8 | 0x20, 0x22); // Notification -> TpNotification
        assert_eq!(0x80u8 | 0x20, 0xA0); // Response -> TpResponse
        assert_eq!(0x81u8 | 0x20, 0xA1); // Error -> TpError
    }

    #[test_log::test]
    fn message_type_valid_responses_to_request() {
        assert!(MessageType::Response.is_valid_response_to(MessageType::Request));
        assert!(MessageType::Error.is_valid_response_to(MessageType::Request));
        assert!(MessageType::TpResponse.is_valid_response_to(MessageType::TpRequest));
        assert!(MessageType::TpError.is_valid_response_to(MessageType::TpRequest));

        // Cross-type responses are invalid
        assert!(!MessageType::Response.is_valid_response_to(MessageType::TpRequest));
        assert!(!MessageType::TpResponse.is_valid_response_to(MessageType::Request));
    }

    #[test_log::test]
    fn message_type_no_responses_to_fire_and_forget() {
        assert!(!MessageType::Response.is_valid_response_to(MessageType::RequestNoReturn));
        assert!(!MessageType::Error.is_valid_response_to(MessageType::RequestNoReturn));
        assert!(!MessageType::TpResponse.is_valid_response_to(MessageType::TpRequestNoReturn));
        assert!(!MessageType::TpError.is_valid_response_to(MessageType::TpRequestNoReturn));
    }

    #[test_log::test]
    fn message_type_no_responses_to_notification() {
        assert!(!MessageType::Response.is_valid_response_to(MessageType::Notification));
        assert!(!MessageType::Error.is_valid_response_to(MessageType::Notification));
    }

    #[test_log::test]
    fn message_type_expected_response() {
        assert_eq!(
            MessageType::Request.expected_response_type(),
            Some(MessageType::Response)
        );
        assert_eq!(
            MessageType::TpRequest.expected_response_type(),
            Some(MessageType::TpResponse)
        );
        assert_eq!(MessageType::RequestNoReturn.expected_response_type(), None);
        assert_eq!(MessageType::Notification.expected_response_type(), None);
        assert_eq!(MessageType::Response.expected_response_type(), None);
        assert_eq!(MessageType::Error.expected_response_type(), None);
    }

    #[test_log::test]
    fn message_type_response_classification() {
        assert!(MessageType::Response.is_response());
        assert!(MessageType::Error.is_response());
        assert!(MessageType::TpResponse.is_response());
        assert!(MessageType::TpError.is_response());

        assert!(!MessageType::Request.is_response());
        assert!(!MessageType::RequestNoReturn.is_response());
        assert!(!MessageType::Notification.is_response());
        assert!(!MessageType::TpRequest.is_response());
    }

    // ========================================================================
    // Protocol Version Tests
    // ========================================================================

    /// [feat_req_recentip_300] Protocol version is 0x01
    #[test_log::test]
    fn protocol_version_is_0x01() {
        assert_eq!(PROTOCOL_VERSION, 0x01);
    }

    /// [feat_req_recentip_300] Protocol version is at byte offset 12
    #[test_log::test]
    fn protocol_version_offset_is_12() {
        assert_eq!(PROTOCOL_VERSION_OFFSET, 12);
    }

    /// [feat_req_recentip_300] Valid protocol version is accepted
    #[test_log::test]
    fn protocol_version_valid_accepted() {
        assert!(validate_protocol_version(0x01).is_ok());
    }

    /// [feat_req_recentip_300] Invalid protocol version is rejected
    #[test_log::test]
    fn protocol_version_invalid_rejected() {
        // Version 0x00 is invalid
        let result = validate_protocol_version(0x00);
        assert!(matches!(
            result,
            Err(VersionError::ProtocolMismatch {
                expected: 0x01,
                found: 0x00
            })
        ));

        // Version 0x02 is invalid (future version)
        let result = validate_protocol_version(0x02);
        assert!(matches!(
            result,
            Err(VersionError::ProtocolMismatch {
                expected: 0x01,
                found: 0x02
            })
        ));

        // Version 0xFF is invalid
        let result = validate_protocol_version(0xFF);
        assert!(matches!(
            result,
            Err(VersionError::ProtocolMismatch {
                expected: 0x01,
                found: 0xFF
            })
        ));
    }

    // ========================================================================
    // Interface Version Tests
    // ========================================================================

    /// [feat_req_recentip_278] Interface version is at byte offset 13
    #[test_log::test]
    fn interface_version_offset_is_13() {
        assert_eq!(INTERFACE_VERSION_OFFSET, 13);
    }

    /// [feat_req_recentip_278] Interface version major can be any value 0-255
    #[test_log::test]
    fn interface_version_major_range() {
        // All major versions 0-255 are valid
        for major in 0u8..=255u8 {
            let version = InterfaceVersion::new(major, 0);
            assert_eq!(version.wire_major(), major);
        }
    }

    /// [feat_req_recentip_278] Interface version minor is 32-bit
    #[test_log::test]
    fn interface_version_minor_is_32bit() {
        let version = InterfaceVersion::new(1, u32::MAX);
        assert_eq!(version.minor, u32::MAX);

        let version = InterfaceVersion::new(1, 0);
        assert_eq!(version.minor, 0);
    }

    /// [feat_req_recentip_278] Same major version is compatible
    #[test_log::test]
    fn interface_version_same_major_compatible() {
        let v1 = InterfaceVersion::new(1, 0);
        let v2 = InterfaceVersion::new(1, 5);

        // Both should be compatible with v1.0
        assert!(v1.is_compatible_with(&InterfaceVersion::new(1, 0)));
        assert!(v2.is_compatible_with(&InterfaceVersion::new(1, 0)));
    }

    /// [feat_req_recentip_278] Different major version is incompatible
    #[test_log::test]
    fn interface_version_different_major_incompatible() {
        let v1 = InterfaceVersion::new(1, 0);
        let v2 = InterfaceVersion::new(2, 0);

        assert!(!v1.is_compatible_with(&v2));
        assert!(!v2.is_compatible_with(&v1));
    }

    /// [feat_req_recentip_278] Higher minor version is compatible with lower
    #[test_log::test]
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
    #[test_log::test]
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
    #[test_log::test]
    fn interface_version_header_validation() {
        let expected = InterfaceVersion::new(1, 0);

        // Matching major is OK
        assert!(validate_interface_version(1, &expected).is_ok());

        // Mismatched major is error
        let result = validate_interface_version(2, &expected);
        match result {
            Err(VersionError::InterfaceMajorMismatch {
                expected: 1,
                found: 2,
            }) => {}
            other => panic!("Expected InterfaceMajorMismatch, got {:?}", other),
        }
    }

    // ========================================================================
    // Wire Format Version Tests
    // ========================================================================

    /// [feat_req_recentip_300] Protocol version in header at correct offset
    #[test_log::test]
    fn wire_format_protocol_version_position() {
        // Minimal valid SOME/IP header
        let header = [
            0x12, 0x34, // Service ID
            0x00, 0x01, // Method ID
            0x00, 0x00, 0x00, 0x08, // Length
            0x00, 0x01, // Client ID
            0x00, 0x01, // Session ID
            0x01, // Protocol version (offset 12)
            0x01, // Interface version (offset 13)
            0x00, // Message type
            0x00, // Return code
        ];

        assert_eq!(header[PROTOCOL_VERSION_OFFSET], PROTOCOL_VERSION);
    }

    /// [feat_req_recentip_278] Interface version in header at correct offset
    #[test_log::test]
    fn wire_format_interface_version_position() {
        let interface_v = 0x05u8; // Major version 5

        let header = [
            0x12,
            0x34, // Service ID
            0x00,
            0x01, // Method ID
            0x00,
            0x00,
            0x00,
            0x08, // Length
            0x00,
            0x01, // Client ID
            0x00,
            0x01,        // Session ID
            0x01,        // Protocol version
            interface_v, // Interface version (offset 13)
            0x00,        // Message type
            0x00,        // Return code
        ];

        assert_eq!(header[INTERFACE_VERSION_OFFSET], interface_v);
    }

    /// [feat_req_recentip_300] Parse protocol version from raw bytes
    #[test_log::test]
    fn parse_protocol_version_from_bytes() {
        let header_bytes = [
            0x12, 0x34, 0x00, 0x01, 0x00, 0x00, 0x00, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01, 0x05,
            0x00, 0x00,
        ];

        let protocol_v = header_bytes[PROTOCOL_VERSION_OFFSET];
        let interface_v = header_bytes[INTERFACE_VERSION_OFFSET];

        assert_eq!(protocol_v, 0x01);
        assert_eq!(interface_v, 0x05);
    }
}

#[cfg(test)]
mod proptest_suite {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// MessageType round-trips through u8
        #[test_log::test]
        fn message_type_roundtrip(byte in prop::sample::select(vec![
            0x00u8, 0x01, 0x02, 0x80, 0x81, 0x20, 0x21, 0x22, 0xA0, 0xA1
        ])) {
            let mt = MessageType::from_u8(byte).unwrap();
            let back = mt as u8;
            prop_assert_eq!(byte, back);
        }

        /// Only valid message types parse successfully
        #[test_log::test]
        fn invalid_message_types_fail(byte in 0u8..=255u8) {
            let valid = [0x00, 0x01, 0x02, 0x80, 0x81, 0x20, 0x21, 0x22, 0xA0, 0xA1];
            let result = MessageType::from_u8(byte);

            if valid.contains(&byte) {
                prop_assert!(result.is_some());
            } else {
                prop_assert!(result.is_none());
            }
        }

        /// TP flag only affects bit 5
        #[test_log::test]
        fn tp_flag_only_bit_5(base in prop::sample::select(vec![
            0x00u8, 0x01, 0x02, 0x80, 0x81
        ])) {
            let with_tp = base | 0x20;

            let base_mt = MessageType::from_u8(base).unwrap();
            prop_assert!(!base_mt.is_tp());

            let tp_mt = MessageType::from_u8(with_tp).unwrap();
            prop_assert!(tp_mt.is_tp());
        }

        /// without_tp_flag is inverse of with_tp_flag
        #[test_log::test]
        fn tp_flag_inverse_operations(base in prop::sample::select(vec![
            0x00u8, 0x01, 0x02, 0x80, 0x81
        ])) {
            let base_mt = MessageType::from_u8(base).unwrap();

            if let Some(tp_mt) = base_mt.with_tp_flag() {
                prop_assert_eq!(tp_mt.without_tp_flag(), base_mt);
            }
        }

        /// Only REQUEST types expect responses
        #[test_log::test]
        fn only_requests_expect_responses(byte in prop::sample::select(vec![
            0x00u8, 0x01, 0x02, 0x80, 0x81, 0x20, 0x21, 0x22, 0xA0, 0xA1
        ])) {
            let mt = MessageType::from_u8(byte).unwrap();
            let expects = mt.expects_response();

            prop_assert_eq!(expects, byte == 0x00 || byte == 0x20);
        }

        // ====================================================================
        // Version Property Tests
        // ====================================================================

        /// [feat_req_recentip_300] Only protocol version 0x01 is valid
        #[test_log::test]
        fn only_protocol_v1_valid(version in 0u8..=255u8) {
            let result = validate_protocol_version(version);
            if version == 0x01 {
                prop_assert!(result.is_ok());
            } else {
                prop_assert!(result.is_err());
            }
        }

        /// [feat_req_recentip_278] Interface version major matches header byte
        #[test_log::test]
        fn interface_version_wire_major(major in 0u8..=255u8, minor in 0u32..=u32::MAX) {
            let version = InterfaceVersion::new(major, minor);
            prop_assert_eq!(version.wire_major(), major);
        }

        /// [feat_req_recentip_278] Compatibility is reflexive
        #[test_log::test]
        fn interface_version_reflexive_compat(major in 0u8..=255u8, minor in 0u32..=u32::MAX) {
            let version = InterfaceVersion::new(major, minor);
            prop_assert!(version.is_compatible_with(&version));
        }

        /// [feat_req_recentip_278] Exact match is reflexive
        #[test_log::test]
        fn interface_version_reflexive_exact(major in 0u8..=255u8, minor in 0u32..=u32::MAX) {
            let version = InterfaceVersion::new(major, minor);
            prop_assert!(version.matches_exactly(&version));
        }

        /// [feat_req_recentip_278] Different major always incompatible
        #[test_log::test]
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
        #[test_log::test]
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
