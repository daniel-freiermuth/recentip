//! Shared helpers for wire format tests
//!
//! This module contains common imports, test service definitions,
//! parser functions, and packet builders used across wire format tests.
//!
//! **Important**: Parser types in this module are intentionally independent
//! from `recentip::wire` to allow proper verification of wire format compliance.
//! The parsing logic here may mirror what the library does, but it's a separate
//! implementation to catch any bugs in the library's wire format handling.

pub use recentip::handle::ServiceEvent;
pub use recentip::prelude::*;
pub use std::net::{Ipv4Addr, SocketAddr};
pub use std::time::Duration;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}
pub(crate) use covers;

// Wire values for TestService
pub const TEST_SERVICE_ID: u16 = 0x1234;
pub const TEST_SERVICE_VERSION: (u8, u32) = (1, 0);

// ============================================================================
// INDEPENDENT WIRE FORMAT CONSTANTS (not from library)
// ============================================================================

/// SD Service ID (0xFFFF)
pub const SD_SERVICE_ID: u16 = 0xFFFF;
/// SD Method ID (0x8100)
pub const SD_METHOD_ID: u16 = 0x8100;
/// SOME/IP header size
pub const SOMEIP_HEADER_SIZE: usize = 16;

/// L4 Protocol: UDP
pub const L4_UDP: u8 = 0x11;
/// L4 Protocol: TCP
pub const L4_TCP: u8 = 0x06;

// ============================================================================
// INDEPENDENT PARSER TYPES (not using library code)
// ============================================================================

/// Parsed SOME/IP header - independent from library for test verification
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHeader {
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

impl ParsedHeader {
    /// Parse a SOME/IP header from raw bytes
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < SOMEIP_HEADER_SIZE {
            return None;
        }
        Some(Self {
            service_id: u16::from_be_bytes([data[0], data[1]]),
            method_id: u16::from_be_bytes([data[2], data[3]]),
            length: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
            client_id: u16::from_be_bytes([data[8], data[9]]),
            session_id: u16::from_be_bytes([data[10], data[11]]),
            protocol_version: data[12],
            interface_version: data[13],
            message_type: data[14],
            return_code: data[15],
        })
    }

    /// Get payload length (length field minus 8 bytes for header tail)
    pub fn payload_length(&self) -> usize {
        self.length.saturating_sub(8) as usize
    }

    /// Check if this is an SD message
    pub fn is_sd(&self) -> bool {
        self.service_id == SD_SERVICE_ID && self.method_id == SD_METHOD_ID
    }

    /// Check if this is a notification (event)
    pub fn is_notification(&self) -> bool {
        self.message_type == 0x02
    }

    /// Check if this is a request
    pub fn is_request(&self) -> bool {
        self.message_type == 0x00
    }

    /// Check if this is a response
    pub fn is_response(&self) -> bool {
        self.message_type == 0x80
    }
}

/// A complete parsed SOME/IP packet (header + payload)
#[derive(Debug, Clone)]
pub struct ParsedPacket {
    pub header: ParsedHeader,
    pub payload: Vec<u8>,
}

impl ParsedPacket {
    /// Parse a complete SOME/IP packet from raw bytes
    pub fn parse(data: &[u8]) -> Option<Self> {
        let header = ParsedHeader::parse(data)?;
        let payload_len = header.payload_length();
        let total_len = SOMEIP_HEADER_SIZE + payload_len;

        if data.len() < total_len {
            return None;
        }

        Some(Self {
            header,
            payload: data[SOMEIP_HEADER_SIZE..total_len].to_vec(),
        })
    }

    /// Parse as SD message if this is an SD packet
    pub fn as_sd(&self) -> Option<ParsedSdMessage> {
        if !self.header.is_sd() {
            return None;
        }
        ParsedSdMessage::parse(&self.payload)
    }
}

/// SD Entry type values
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SdEntryType {
    FindService = 0x00,
    OfferService = 0x01,
    SubscribeEventgroup = 0x06,
    SubscribeEventgroupAck = 0x07,
}

impl SdEntryType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0x00 => Some(Self::FindService),
            0x01 => Some(Self::OfferService),
            0x06 => Some(Self::SubscribeEventgroup),
            0x07 => Some(Self::SubscribeEventgroupAck),
            _ => None,
        }
    }
}

/// Parsed SD entry - independent from library
#[derive(Debug, Clone)]
pub struct ParsedSdEntry {
    pub entry_type: SdEntryType,
    pub index_1st_option: u8,
    pub index_2nd_option: u8,
    pub num_options_1: u8,
    pub num_options_2: u8,
    pub service_id: u16,
    pub instance_id: u16,
    pub major_version: u8,
    pub ttl: u32,
    /// For service entries (Find/Offer)
    pub minor_version: u32,
    /// For eventgroup entries (Subscribe/SubscribeAck)
    pub eventgroup_id: u16,
    /// Counter for eventgroup entries
    pub counter: u8,
}

impl ParsedSdEntry {
    pub const SIZE: usize = 16;

    /// Parse an SD entry from raw bytes
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE {
            return None;
        }

        let entry_type = SdEntryType::from_u8(data[0])?;
        let index_1st_option = data[1];
        let index_2nd_option = data[2];
        let num_options = data[3];
        let num_options_1 = (num_options >> 4) & 0x0F;
        let num_options_2 = num_options & 0x0F;
        let service_id = u16::from_be_bytes([data[4], data[5]]);
        let instance_id = u16::from_be_bytes([data[6], data[7]]);
        let major_version = data[8];
        let ttl = u32::from_be_bytes([0, data[9], data[10], data[11]]);

        let (minor_version, eventgroup_id, counter) = match entry_type {
            SdEntryType::FindService | SdEntryType::OfferService => {
                let minor = u32::from_be_bytes([data[12], data[13], data[14], data[15]]);
                (minor, 0, 0)
            }
            SdEntryType::SubscribeEventgroup | SdEntryType::SubscribeEventgroupAck => {
                let counter = data[13];
                let eventgroup = u16::from_be_bytes([data[14], data[15]]);
                (0, eventgroup, counter)
            }
        };

        Some(Self {
            entry_type,
            index_1st_option,
            index_2nd_option,
            num_options_1,
            num_options_2,
            service_id,
            instance_id,
            major_version,
            ttl,
            minor_version,
            eventgroup_id,
            counter,
        })
    }

    /// Check if this is a stop entry (TTL = 0)
    pub fn is_stop(&self) -> bool {
        self.ttl == 0
    }

    /// Check if this is a Subscribe entry
    pub fn is_subscribe(&self) -> bool {
        matches!(self.entry_type, SdEntryType::SubscribeEventgroup)
    }

    /// Check if this is a SubscribeAck entry
    pub fn is_subscribe_ack(&self) -> bool {
        matches!(self.entry_type, SdEntryType::SubscribeEventgroupAck)
    }

    /// Check if this is an Offer entry
    pub fn is_offer(&self) -> bool {
        matches!(self.entry_type, SdEntryType::OfferService)
    }

    /// Check if this is a Find entry
    pub fn is_find(&self) -> bool {
        matches!(self.entry_type, SdEntryType::FindService)
    }
}

/// Parsed SD option - independent from library
#[derive(Debug, Clone)]
pub enum ParsedSdOption {
    Ipv4Endpoint {
        addr: Ipv4Addr,
        port: u16,
        protocol: u8,
    },
    Ipv4Multicast {
        addr: Ipv4Addr,
        port: u16,
    },
    Unknown {
        option_type: u8,
        data: Vec<u8>,
    },
}

impl ParsedSdOption {
    /// Parse an SD option from raw bytes, returns (option, bytes consumed)
    pub fn parse(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < 3 {
            return None;
        }

        let length = u16::from_be_bytes([data[0], data[1]]) as usize;
        let option_type = data[2];
        let total_size = 3 + length;

        if data.len() < total_size {
            return None;
        }

        let option = match option_type {
            0x04 => {
                // IPv4 Endpoint
                if length < 9 {
                    return None;
                }
                let addr = Ipv4Addr::new(data[4], data[5], data[6], data[7]);
                let protocol = data[9];
                let port = u16::from_be_bytes([data[10], data[11]]);
                Self::Ipv4Endpoint {
                    addr,
                    port,
                    protocol,
                }
            }
            0x14 => {
                // IPv4 Multicast
                if length < 9 {
                    return None;
                }
                let addr = Ipv4Addr::new(data[4], data[5], data[6], data[7]);
                let port = u16::from_be_bytes([data[10], data[11]]);
                Self::Ipv4Multicast { addr, port }
            }
            _ => Self::Unknown {
                option_type,
                data: data[3..total_size].to_vec(),
            },
        };

        Some((option, total_size))
    }

    /// Get the port if this is an endpoint option
    pub fn port(&self) -> Option<u16> {
        match self {
            Self::Ipv4Endpoint { port, .. } | Self::Ipv4Multicast { port, .. } => Some(*port),
            Self::Unknown { .. } => None,
        }
    }

    /// Get the address if this is an IPv4 option
    pub fn addr(&self) -> Option<Ipv4Addr> {
        match self {
            Self::Ipv4Endpoint { addr, .. } | Self::Ipv4Multicast { addr, .. } => Some(*addr),
            Self::Unknown { .. } => None,
        }
    }

    /// Check if this is a TCP endpoint
    pub fn is_tcp(&self) -> bool {
        matches!(self, Self::Ipv4Endpoint { protocol, .. } if *protocol == L4_TCP)
    }

    /// Check if this is a UDP endpoint
    pub fn is_udp(&self) -> bool {
        matches!(self, Self::Ipv4Endpoint { protocol, .. } if *protocol == L4_UDP)
    }
}

/// Parsed SD message - independent from library
#[derive(Debug, Clone)]
pub struct ParsedSdMessage {
    pub flags: u8,
    pub entries: Vec<ParsedSdEntry>,
    pub options: Vec<ParsedSdOption>,
}

impl ParsedSdMessage {
    /// Reboot flag
    pub const FLAG_REBOOT: u8 = 0x80;
    /// Unicast flag
    pub const FLAG_UNICAST: u8 = 0x40;

    /// Parse SD message from payload bytes (after SOME/IP header)
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 12 {
            return None;
        }

        let flags = data[0];
        // data[1..4] are reserved

        let entries_len = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let entries_start = 8;
        let entries_end = entries_start + entries_len;

        if data.len() < entries_end + 4 {
            return None;
        }

        // Parse entries
        let mut entries = Vec::new();
        let mut offset = entries_start;
        while offset + ParsedSdEntry::SIZE <= entries_end {
            if let Some(entry) = ParsedSdEntry::parse(&data[offset..]) {
                entries.push(entry);
                offset += ParsedSdEntry::SIZE;
            } else {
                break;
            }
        }

        // Parse options
        let options_len =
            u32::from_be_bytes([data[entries_end], data[entries_end + 1], data[entries_end + 2], data[entries_end + 3]])
                as usize;
        let options_start = entries_end + 4;
        let options_end = options_start + options_len;

        if data.len() < options_end {
            return None;
        }

        let mut options = Vec::new();
        let mut offset = options_start;
        while offset < options_end {
            if let Some((option, consumed)) = ParsedSdOption::parse(&data[offset..]) {
                options.push(option);
                offset += consumed;
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

    /// Check if reboot flag is set
    pub fn has_reboot_flag(&self) -> bool {
        self.flags & Self::FLAG_REBOOT != 0
    }

    /// Check if unicast flag is set
    pub fn has_unicast_flag(&self) -> bool {
        self.flags & Self::FLAG_UNICAST != 0
    }

    /// Get the option at index (used with entry's index_1st_option/index_2nd_option)
    pub fn option_at(&self, index: u8) -> Option<&ParsedSdOption> {
        self.options.get(index as usize)
    }

    /// Get the endpoint port from an entry's first option
    pub fn endpoint_port_for_entry(&self, entry: &ParsedSdEntry) -> Option<u16> {
        if entry.num_options_1 > 0 {
            self.option_at(entry.index_1st_option)?.port()
        } else {
            None
        }
    }

    /// Get all subscribe entries
    pub fn subscribe_entries(&self) -> impl Iterator<Item = &ParsedSdEntry> {
        self.entries.iter().filter(|e| e.is_subscribe())
    }

    /// Get all offer entries
    pub fn offer_entries(&self) -> impl Iterator<Item = &ParsedSdEntry> {
        self.entries.iter().filter(|e| e.is_offer())
    }

    /// Get all subscribe ack entries
    pub fn subscribe_ack_entries(&self) -> impl Iterator<Item = &ParsedSdEntry> {
        self.entries.iter().filter(|e| e.is_subscribe_ack())
    }
}

// ============================================================================
// CONVENIENCE PARSER FUNCTIONS
// ============================================================================

/// Parse a SOME/IP header from raw bytes
pub fn parse_header(data: &[u8]) -> Option<ParsedHeader> {
    ParsedHeader::parse(data)
}

/// Parse a complete SOME/IP packet from raw bytes
pub fn parse_packet(data: &[u8]) -> Option<ParsedPacket> {
    ParsedPacket::parse(data)
}

/// Parse a raw SD packet (SOME/IP header + SD payload)
pub fn parse_sd_packet(data: &[u8]) -> Option<(ParsedHeader, ParsedSdMessage)> {
    let packet = ParsedPacket::parse(data)?;
    let sd = packet.as_sd()?;
    Some((packet.header, sd))
}

/// Parse an SD message (header + payload) from raw bytes
/// Returns the header and the parsed SD message
pub fn parse_sd_message(data: &[u8]) -> Option<(ParsedHeader, ParsedSdMessage)> {
    parse_sd_packet(data)
}

// ============================================================================
// PACKET BUILDER FUNCTIONS
// ============================================================================

/// Builder for constructing raw SOME/IP packets for wire-level testing.
///
/// Provides a fluent API for creating test packets with customizable fields.
/// Defaults to protocol version 0x01 and interface version 0x01.
///
/// # Example
/// ```ignore
/// let packet = SomeIpPacketBuilder::request(0x1234, 0x0001)
///     .client_id(0xABCD)
///     .session_id(0x1234)
///     .interface_version(0x02)
///     .payload(b"test")
///     .build();
/// ```
pub struct SomeIpPacketBuilder {
    service_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    protocol_version: u8,
    interface_version: u8,
    message_type: u8,
    return_code: u8,
    payload: Vec<u8>,
}

impl SomeIpPacketBuilder {
    /// Create a new REQUEST packet builder with default protocol/interface version
    pub fn request(service_id: u16, method_id: u16) -> Self {
        Self {
            service_id,
            method_id,
            client_id: 0x0000,
            session_id: 0x0001,
            protocol_version: 0x01,
            interface_version: 0x01,
            message_type: 0x00, // REQUEST
            return_code: 0x00,
            payload: Vec::new(),
        }
    }

    /// Create a REQUEST_NO_RETURN (fire-and-forget) packet builder
    pub fn fire_and_forget(service_id: u16, method_id: u16) -> Self {
        Self {
            message_type: 0x01, // REQUEST_NO_RETURN
            ..Self::request(service_id, method_id)
        }
    }

    /// Create a RESPONSE packet builder
    pub fn response(service_id: u16, method_id: u16) -> Self {
        Self {
            message_type: 0x80, // RESPONSE
            ..Self::request(service_id, method_id)
        }
    }

    /// Create a NOTIFICATION packet builder (typically for SD messages)
    pub fn notification(service_id: u16, method_id: u16) -> Self {
        Self {
            message_type: 0x02, // NOTIFICATION
            ..Self::request(service_id, method_id)
        }
    }

    pub fn client_id(mut self, client_id: u16) -> Self {
        self.client_id = client_id;
        self
    }

    pub fn session_id(mut self, session_id: u16) -> Self {
        self.session_id = session_id;
        self
    }

    pub fn protocol_version(mut self, version: u8) -> Self {
        self.protocol_version = version;
        self
    }

    pub fn interface_version(mut self, version: u8) -> Self {
        self.interface_version = version;
        self
    }

    pub fn payload(mut self, payload: &[u8]) -> Self {
        self.payload = payload.to_vec();
        self
    }

    pub fn build(self) -> Vec<u8> {
        let length = 8 + self.payload.len() as u32;
        let mut packet = Vec::with_capacity(16 + self.payload.len());

        packet.extend_from_slice(&self.service_id.to_be_bytes());
        packet.extend_from_slice(&self.method_id.to_be_bytes());
        packet.extend_from_slice(&length.to_be_bytes());
        packet.extend_from_slice(&self.client_id.to_be_bytes());
        packet.extend_from_slice(&self.session_id.to_be_bytes());
        packet.push(self.protocol_version);
        packet.push(self.interface_version);
        packet.push(self.message_type);
        packet.push(self.return_code);
        packet.extend_from_slice(&self.payload);

        packet
    }
}

/// Build a raw SOME/IP request packet
pub fn build_request(
    service_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    payload: &[u8],
) -> Vec<u8> {
    SomeIpPacketBuilder::request(service_id, method_id)
        .client_id(client_id)
        .session_id(session_id)
        .payload(payload)
        .build()
}

/// Build a raw SOME/IP fire-and-forget (REQUEST_NO_RETURN) packet
pub fn build_fire_and_forget_request(
    service_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    payload: &[u8],
) -> Vec<u8> {
    SomeIpPacketBuilder::fire_and_forget(service_id, method_id)
        .client_id(client_id)
        .session_id(session_id)
        .payload(payload)
        .build()
}

/// Build a raw SOME/IP response packet based on a request header
pub fn build_response(request: &ParsedHeader, payload: &[u8]) -> Vec<u8> {
    SomeIpPacketBuilder::response(request.service_id, request.method_id)
        .client_id(request.client_id)
        .session_id(request.session_id)
        .interface_version(request.interface_version)
        .payload(payload)
        .build()
}

/// L4 Protocol constants for SD endpoint options
pub const L4_PROTOCOL_UDP: u8 = 0x11;
pub const L4_PROTOCOL_TCP: u8 = 0x06;

/// Builder for constructing raw SOME/IP-SD OfferService messages.
///
/// Provides a fluent API for creating test packets with customizable fields.
/// Defaults to UDP protocol, TTL=0xFFFFFF, session_id=1, reboot_flag=true.
///
/// # Example
/// ```ignore
/// let offer = SdOfferBuilder::new(0x1234, 0x0001, "192.168.1.1".parse().unwrap(), 30501)
///     .major_version(2)
///     .tcp()
///     .session_id(5)
///     .build();
/// ```
pub struct SdOfferBuilder {
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    minor_version: u32,
    endpoint_ip: std::net::Ipv4Addr,
    endpoint_port: u16,
    protocol: u8,
    ttl: u32,
    session_id: u16,
    reboot_flag: bool,
    unicast_flag: bool,
}

impl SdOfferBuilder {
    /// Create a new OfferService builder with required parameters
    ///
    /// Defaults:
    /// - protocol: UDP
    /// - TTL: 0xFFFFFF (max)
    /// - session_id: 1
    /// - reboot_flag: true
    /// - unicast_flag: false (offers are typically multicast)
    pub fn new(
        service_id: u16,
        instance_id: u16,
        endpoint_ip: std::net::Ipv4Addr,
        endpoint_port: u16,
    ) -> Self {
        Self {
            service_id,
            instance_id,
            major_version: 1,
            minor_version: 0,
            endpoint_ip,
            endpoint_port,
            protocol: L4_PROTOCOL_UDP,
            ttl: 0xFFFFFF,
            session_id: 1,
            reboot_flag: true,
            unicast_flag: false, // Offers typically don't have unicast flag
        }
    }

    pub fn major_version(mut self, version: u8) -> Self {
        self.major_version = version;
        self
    }

    pub fn minor_version(mut self, version: u32) -> Self {
        self.minor_version = version;
        self
    }

    pub fn version(mut self, major: u8, minor: u32) -> Self {
        self.major_version = major;
        self.minor_version = minor;
        self
    }

    /// Set protocol to TCP (0x06)
    pub fn tcp(mut self) -> Self {
        self.protocol = L4_PROTOCOL_TCP;
        self
    }

    /// Set protocol to UDP (0x11) - this is the default
    pub fn udp(mut self) -> Self {
        self.protocol = L4_PROTOCOL_UDP;
        self
    }

    /// Set raw protocol byte
    pub fn protocol(mut self, protocol: u8) -> Self {
        self.protocol = protocol;
        self
    }

    pub fn ttl(mut self, ttl: u32) -> Self {
        self.ttl = ttl;
        self
    }

    pub fn session_id(mut self, session_id: u16) -> Self {
        self.session_id = session_id;
        self
    }

    pub fn reboot_flag(mut self, flag: bool) -> Self {
        self.reboot_flag = flag;
        self
    }

    pub fn unicast_flag(mut self, flag: bool) -> Self {
        self.unicast_flag = flag;
        self
    }

    pub fn build(self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(40);

        // SD Payload - Flags
        let mut flags = 0u8;
        if self.reboot_flag {
            flags |= 0x80;
        }
        if self.unicast_flag {
            flags |= 0x40;
        }
        payload.push(flags);
        payload.extend_from_slice(&[0x00, 0x00, 0x00]); // Reserved

        // Entries array
        payload.extend_from_slice(&16u32.to_be_bytes()); // Length

        // OfferService Entry
        payload.push(0x01); // Type
        payload.push(0x00); // Index 1st options
        payload.push(0x00); // Index 2nd options
        payload.push(0x10); // # of options (1 in run 1)
        payload.extend_from_slice(&self.service_id.to_be_bytes());
        payload.extend_from_slice(&self.instance_id.to_be_bytes());
        payload.push(self.major_version);
        let ttl_bytes = self.ttl.to_be_bytes();
        payload.extend_from_slice(&ttl_bytes[1..4]);
        payload.extend_from_slice(&self.minor_version.to_be_bytes());

        // Options array
        payload.extend_from_slice(&12u32.to_be_bytes()); // Length

        // IPv4 Endpoint Option
        payload.extend_from_slice(&9u16.to_be_bytes()); // Length
        payload.push(0x04); // Type
        payload.push(0x00); // Reserved
        payload.extend_from_slice(&self.endpoint_ip.octets());
        payload.push(0x00); // Reserved
        payload.push(self.protocol);
        payload.extend_from_slice(&self.endpoint_port.to_be_bytes());

        SomeIpPacketBuilder::notification(SD_SERVICE_ID, SD_METHOD_ID)
            .client_id(0x0000)
            .session_id(self.session_id)
            .payload(payload.as_slice())
            .build()
    }
}

/// Build a raw SOME/IP-SD OfferService message (convenience function)
pub fn build_sd_offer(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    minor_version: u32,
    endpoint_ip: std::net::Ipv4Addr,
    endpoint_port: u16,
    ttl: u32,
) -> Vec<u8> {
    SdOfferBuilder::new(service_id, instance_id, endpoint_ip, endpoint_port)
        .major_version(major_version)
        .minor_version(minor_version)
        .ttl(ttl)
        .build()
}

/// Builder for constructing raw SOME/IP-SD SubscribeEventgroupAck messages.
///
/// Provides a fluent API for creating test packets with customizable fields.
/// Defaults to TTL=0xFFFFFF, session_id=1, reboot_flag=true, unicast_flag=true.
///
/// # Example
/// ```ignore
/// let ack = SdSubscribeAckBuilder::new(0x1234, 0x0001, 0x0001)
///     .major_version(2)
///     .session_id(5)
///     .build();
/// ```
pub struct SdSubscribeAckBuilder {
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    ttl: u32,
    session_id: u16,
    reboot_flag: bool,
    unicast_flag: bool,
    counter: u8,
}

impl SdSubscribeAckBuilder {
    /// Create a new SubscribeEventgroupAck builder with required parameters
    pub fn new(service_id: u16, instance_id: u16, eventgroup_id: u16) -> Self {
        Self {
            service_id,
            instance_id,
            major_version: 1,
            eventgroup_id,
            ttl: 0xFFFFFF,
            session_id: 1,
            reboot_flag: true,
            unicast_flag: true,
            counter: 0,
        }
    }

    pub fn major_version(mut self, version: u8) -> Self {
        self.major_version = version;
        self
    }

    pub fn ttl(mut self, ttl: u32) -> Self {
        self.ttl = ttl;
        self
    }

    pub fn session_id(mut self, session_id: u16) -> Self {
        self.session_id = session_id;
        self
    }

    pub fn reboot_flag(mut self, flag: bool) -> Self {
        self.reboot_flag = flag;
        self
    }

    pub fn unicast_flag(mut self, flag: bool) -> Self {
        self.unicast_flag = flag;
        self
    }

    pub fn counter(mut self, counter: u8) -> Self {
        self.counter = counter;
        self
    }

    pub fn build(self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(48);

        // SD Payload - Flags
        let mut flags = 0u8;
        if self.reboot_flag {
            flags |= 0x80;
        }
        if self.unicast_flag {
            flags |= 0x40;
        }
        payload.push(flags);
        payload.extend_from_slice(&[0x00, 0x00, 0x00]); // Reserved

        // Entries array
        payload.extend_from_slice(&16u32.to_be_bytes());

        // SubscribeEventgroupAck Entry
        payload.push(0x07); // Type
        payload.push(0x00);
        payload.push(0x00);
        payload.push(0x00);
        payload.extend_from_slice(&self.service_id.to_be_bytes());
        payload.extend_from_slice(&self.instance_id.to_be_bytes());
        payload.push(self.major_version);
        let ttl_bytes = self.ttl.to_be_bytes();
        payload.extend_from_slice(&ttl_bytes[1..4]);
        payload.push(0x00); // Reserved
        payload.push(self.counter);
        payload.extend_from_slice(&self.eventgroup_id.to_be_bytes());

        // Options array (empty)
        payload.extend_from_slice(&0u32.to_be_bytes());

        SomeIpPacketBuilder::notification(SD_SERVICE_ID, SD_METHOD_ID)
            .client_id(0x0000)
            .session_id(self.session_id)
            .payload(payload.as_slice())
            .build()
    }
}

/// Build a raw SOME/IP-SD SubscribeEventgroupAck message (convenience function)
pub fn build_sd_subscribe_ack(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    ttl: u32,
) -> Vec<u8> {
    SdSubscribeAckBuilder::new(service_id, instance_id, eventgroup_id)
        .major_version(major_version)
        .ttl(ttl)
        .build()
}

/// Build a raw SOME/IP-SD SubscribeEventgroup with a UDP endpoint option
pub fn build_sd_subscribe_with_udp_endpoint(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    ttl: u32,
    client_ip: std::net::Ipv4Addr,
    client_port: u16,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(64);

    // SD Payload
    payload.push(0xC0);
    payload.extend_from_slice(&[0x00, 0x00, 0x00]);

    // Entries array length
    payload.extend_from_slice(&16u32.to_be_bytes());

    // SubscribeEventgroup Entry
    payload.push(0x06); // Type = SubscribeEventgroup
    payload.push(0x00); // Index 1st options = 0
    payload.push(0x00); // Index 2nd options
    payload.push(0x10); // # of opts: 1 in run1, 0 in run2
    payload.extend_from_slice(&service_id.to_be_bytes());
    payload.extend_from_slice(&instance_id.to_be_bytes());
    payload.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    payload.extend_from_slice(&ttl_bytes[1..4]);
    payload.push(0x00);
    payload.push(0x00);
    payload.extend_from_slice(&eventgroup_id.to_be_bytes());

    // Options array
    payload.extend_from_slice(&12u32.to_be_bytes());

    // IPv4 Endpoint Option with UDP
    payload.extend_from_slice(&9u16.to_be_bytes()); // Length = 9
    payload.push(0x04); // Type = IPv4 Endpoint
    payload.push(0x00); // Reserved
    payload.extend_from_slice(&client_ip.octets());
    payload.push(0x00); // Reserved
    payload.push(0x11); // L4 Protocol = UDP (0x11)
    payload.extend_from_slice(&client_port.to_be_bytes());

    // Build SOME/IP-SD packet using builder
    SomeIpPacketBuilder::notification(SD_SERVICE_ID, SD_METHOD_ID)
        .client_id(0x0001)
        .session_id(0x0001)
        .payload(payload.as_slice())
        .build()
}
