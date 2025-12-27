//! SOME/IP wire format serialization and parsing.
//!
//! This module handles encoding and decoding of SOME/IP messages
//! according to the specification.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

/// SOME/IP protocol version
pub const PROTOCOL_VERSION: u8 = 0x01;

/// Interface version for SD
pub const SD_INTERFACE_VERSION: u8 = 0x01;

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
}

/// SOME/IP header (16 bytes)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// Service ID
    pub service_id: u16,
    /// Method ID or Event ID
    pub method_id: u16,
    /// Length of payload + 8 bytes (client_id, session_id, protocol_version, interface_version, message_type, return_code)
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
#[allow(dead_code)]  // Used for future TCP framing
pub struct Message {
    pub header: Header,
    pub payload: Bytes,
}

#[allow(dead_code)]  // Used for future TCP framing
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
    pub ttl: u32, // 24-bit, 0 = stop/nack
    pub minor_version: u32, // For Type 1 (service entries)
    pub eventgroup_id: u16, // For Type 2 (eventgroup entries)
    pub counter: u8, // For eventgroup entries
    /// Index of first option in run 1
    pub index_1st_option: u8,
    /// Index of first option in run 2
    pub index_2nd_option: u8,
    /// Number of options in run 1
    pub num_options_1: u8,
    /// Number of options in run 2
    pub num_options_2: u8,
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

    /// Create a FindService entry
    pub fn find_service(service_id: u16, instance_id: u16, major_version: u8, minor_version: u32, ttl: u32) -> Self {
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

    /// Create an OfferService entry
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

    /// Create a StopOfferService entry (OfferService with TTL=0)
    pub fn stop_offer_service(service_id: u16, instance_id: u16, major_version: u8, minor_version: u32) -> Self {
        Self::offer_service(service_id, instance_id, major_version, minor_version, 0, 0, 0)
    }

    /// Create a SubscribeEventgroup entry
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

    /// Create a SubscribeEventgroupAck entry
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
                Some(SdOption::Ipv4Endpoint {
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
                Some(SdOption::Ipv4Multicast {
                    addr: Ipv4Addr::new(a, b, c, d),
                    port,
                })
            }
            _ => {
                // Unknown option - skip
                let data = buf.copy_to_bytes(length);
                Some(SdOption::Unknown { option_type, data })
            }
        }
    }

    pub fn serialize(&self, buf: &mut impl BufMut) {
        match self {
            SdOption::Ipv4Endpoint { addr, port, protocol } => {
                buf.put_u16(9); // length
                buf.put_u8(0x04); // type
                buf.put_u8(0); // reserved
                buf.put_slice(&addr.octets());
                buf.put_u8(0); // reserved
                buf.put_u8(*protocol as u8);
                buf.put_u16(*port);
            }
            SdOption::Ipv4Multicast { addr, port } => {
                buf.put_u16(9); // length
                buf.put_u8(0x14); // type
                buf.put_u8(0); // reserved
                buf.put_slice(&addr.octets());
                buf.put_u8(0); // reserved
                buf.put_u8(L4Protocol::Udp as u8);
                buf.put_u16(*port);
            }
            SdOption::Unknown { option_type, data } => {
                buf.put_u16(data.len() as u16);
                buf.put_u8(*option_type);
                buf.put_slice(data);
            }
        }
    }

    /// Size in bytes when serialized
    pub fn size(&self) -> usize {
        match self {
            SdOption::Ipv4Endpoint { .. } | SdOption::Ipv4Multicast { .. } => 12, // 2 + 1 + 9
            SdOption::Unknown { data, .. } => 3 + data.len(),
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

        Some(Self { flags, entries, options })
    }

    /// Serialize to bytes (just the SD payload, without SOME/IP header)
    pub fn serialize_payload(&self) -> Bytes {
        let entries_len = self.entries.len() * SdEntry::SIZE;
        let options_len: usize = self.options.iter().map(|o| o.size()).sum();
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

    /// Get the UDP endpoint from options for an entry
    pub fn get_udp_endpoint(&self, entry: &SdEntry) -> Option<SocketAddr> {
        let start = entry.index_1st_option as usize;
        let count = entry.num_options_1 as usize;
        
        for i in start..(start + count) {
            if let Some(SdOption::Ipv4Endpoint { addr, port, protocol: L4Protocol::Udp }) = self.options.get(i) {
                return Some(SocketAddr::V4(SocketAddrV4::new(*addr, *port)));
            }
        }
        None
    }

    /// Get the TCP endpoint from options for an entry
    #[allow(dead_code)]  // Will be used for TCP transport
    pub fn get_tcp_endpoint(&self, entry: &SdEntry) -> Option<SocketAddr> {
        let start = entry.index_1st_option as usize;
        let count = entry.num_options_1 as usize;
        
        for i in start..(start + count) {
            if let Some(SdOption::Ipv4Endpoint { addr, port, protocol: L4Protocol::Tcp }) = self.options.get(i) {
                return Some(SocketAddr::V4(SocketAddrV4::new(*addr, *port)));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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

    #[test]
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

    #[test]
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

    #[test]
    fn test_sd_message_roundtrip() {
        let mut msg = SdMessage::initial();
        let opt_idx = msg.add_option(SdOption::Ipv4Endpoint {
            addr: Ipv4Addr::new(192, 168, 1, 100),
            port: 30501,
            protocol: L4Protocol::Udp,
        });
        msg.add_entry(SdEntry::offer_service(0x1234, 0x0001, 1, 0, 3600, opt_idx, 1));
        
        let bytes = msg.serialize(1);
        
        // Skip SOME/IP header
        let mut cursor = bytes.slice(Header::SIZE..);
        let parsed = SdMessage::parse(&mut cursor).unwrap();
        
        assert_eq!(msg.flags, parsed.flags);
        assert_eq!(msg.entries.len(), parsed.entries.len());
        assert_eq!(msg.options.len(), parsed.options.len());
    }

    #[test]
    fn test_header_rejects_short_input() {
        use bytes::Bytes;
        
        // Empty buffer
        let mut empty = Bytes::new();
        assert!(Header::parse(&mut empty).is_none(), "Empty buffer should return None");
        
        // 1 byte - way too short
        let mut one_byte = Bytes::from_static(&[0x12]);
        assert!(Header::parse(&mut one_byte).is_none(), "1 byte should return None");
        
        // 15 bytes - just one byte short
        let mut almost = Bytes::from_static(&[
            0x12, 0x34, // service_id
            0x00, 0x01, // method_id
            0x00, 0x00, 0x00, 0x08, // length
            0x00, 0x01, // client_id
            0x00, 0x01, // session_id
            0x01,       // protocol_version
            0x01,       // interface_version
            0x00,       // message_type - only 15 bytes, missing return_code
        ]);
        assert!(Header::parse(&mut almost).is_none(), "15 bytes should return None");
        
        // 16 bytes - exactly right
        let mut exact = Bytes::from_static(&[
            0x12, 0x34, // service_id
            0x00, 0x01, // method_id
            0x00, 0x00, 0x00, 0x08, // length
            0x00, 0x01, // client_id
            0x00, 0x01, // session_id
            0x01,       // protocol_version
            0x01,       // interface_version
            0x00,       // message_type (Request)
            0x00,       // return_code
        ]);
        assert!(Header::parse(&mut exact).is_some(), "16 bytes should parse successfully");
    }
}