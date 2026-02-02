//! Shared helpers for wire format tests
//!
//! This module contains common imports, test service definitions,
//! parser functions, and packet builders used across wire format tests.

use bytes::Bytes;
pub use recentip::handle::ServiceEvent;
pub use recentip::prelude::*;
pub use recentip::wire::{Header, MessageType, SdMessage, SD_METHOD_ID, SD_SERVICE_ID};
pub use std::net::SocketAddr;
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
// PARSER FUNCTIONS
// ============================================================================

/// Helper to parse a SOME/IP header from raw bytes
pub fn parse_header(data: &[u8]) -> Option<Header> {
    if data.len() < Header::SIZE {
        return None;
    }
    Header::parse(&mut Bytes::copy_from_slice(data))
}

/// Helper to parse an SD message (header + payload) from raw bytes
pub fn parse_sd_message(data: &[u8]) -> Option<(Header, SdMessage)> {
    if data.len() < Header::SIZE {
        return None;
    }
    let mut bytes = Bytes::copy_from_slice(data);
    let header = Header::parse(&mut bytes)?;
    if header.service_id == SD_SERVICE_ID && header.method_id == SD_METHOD_ID {
        let sd_msg = SdMessage::parse(&mut bytes)?;
        Some((header, sd_msg))
    } else {
        None
    }
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
pub fn build_response(request: &Header, payload: &[u8]) -> Vec<u8> {
    SomeIpPacketBuilder::response(request.service_id, request.method_id)
        .client_id(request.client_id)
        .session_id(request.session_id)
        .interface_version(request.interface_version)
        .payload(payload)
        .build()
}

/// Build a raw SOME/IP-SD OfferService message
pub fn build_sd_offer(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    minor_version: u32,
    endpoint_ip: std::net::Ipv4Addr,
    endpoint_port: u16,
    ttl: u32,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(56);

    // SOME/IP Header
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes());
    packet.extend_from_slice(&0x8100u16.to_be_bytes());
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes()); // placeholder
    packet.extend_from_slice(&0x0000u16.to_be_bytes()); // Client ID
    packet.extend_from_slice(&0x0001u16.to_be_bytes()); // Session ID
    packet.push(0x01); // Protocol Version
    packet.push(0x01); // Interface Version
    packet.push(0x02); // Message Type (NOTIFICATION)
    packet.push(0x00); // Return Code

    // SD Payload
    packet.push(0xC0); // Flags (Unicast + Reboot)
    packet.extend_from_slice(&[0x00, 0x00, 0x00]); // Reserved

    // Entries array
    packet.extend_from_slice(&16u32.to_be_bytes()); // Length

    // OfferService Entry
    packet.push(0x01); // Type
    packet.push(0x00); // Index 1st options
    packet.push(0x00); // Index 2nd options
    packet.push(0x10); // # of options (1 in run 1)
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]);
    packet.extend_from_slice(&minor_version.to_be_bytes());

    // Options array
    packet.extend_from_slice(&12u32.to_be_bytes()); // Length

    // IPv4 Endpoint Option
    packet.extend_from_slice(&9u16.to_be_bytes()); // Length
    packet.push(0x04); // Type
    packet.push(0x00); // Reserved
    packet.extend_from_slice(&endpoint_ip.octets());
    packet.push(0x00); // Reserved
    packet.push(0x11); // L4 Protocol (UDP)
    packet.extend_from_slice(&endpoint_port.to_be_bytes());

    // Fix up length field
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}

/// Build a raw SOME/IP-SD SubscribeEventgroupAck message
pub fn build_sd_subscribe_ack(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    ttl: u32,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(64);

    // SOME/IP Header
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes());
    packet.extend_from_slice(&0x8100u16.to_be_bytes());
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes());
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    packet.push(0x01);
    packet.push(0x01);
    packet.push(0x02);
    packet.push(0x00);

    // SD Payload
    packet.push(0xC0);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);

    // Entries array
    packet.extend_from_slice(&16u32.to_be_bytes());

    // SubscribeEventgroupAck Entry
    packet.push(0x07); // Type
    packet.push(0x00);
    packet.push(0x00);
    packet.push(0x00);
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]);
    packet.push(0x00);
    packet.push(0x00);
    packet.extend_from_slice(&eventgroup_id.to_be_bytes());

    // Options array (empty)
    packet.extend_from_slice(&0u32.to_be_bytes());

    // Fix up length field
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
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
    let mut packet = Vec::with_capacity(80);

    // === SOME/IP Header (16 bytes) ===
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes());
    packet.extend_from_slice(&0x8100u16.to_be_bytes());
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes());
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    packet.push(0x01);
    packet.push(0x01);
    packet.push(0x02);
    packet.push(0x00);

    // === SD Payload ===
    packet.push(0xC0);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);

    // Entries array length
    packet.extend_from_slice(&16u32.to_be_bytes());

    // === SubscribeEventgroup Entry ===
    packet.push(0x06); // Type = SubscribeEventgroup
    packet.push(0x00); // Index 1st options = 0
    packet.push(0x00); // Index 2nd options
    packet.push(0x10); // # of opts: 1 in run1, 0 in run2
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]);
    packet.push(0x00);
    packet.push(0x00);
    packet.extend_from_slice(&eventgroup_id.to_be_bytes());

    // === Options array ===
    packet.extend_from_slice(&12u32.to_be_bytes());

    // IPv4 Endpoint Option with UDP
    packet.extend_from_slice(&9u16.to_be_bytes()); // Length = 9
    packet.push(0x04); // Type = IPv4 Endpoint
    packet.push(0x00); // Reserved
    packet.extend_from_slice(&client_ip.octets());
    packet.push(0x00); // Reserved
    packet.push(0x11); // L4 Protocol = UDP (0x11)
    packet.extend_from_slice(&client_port.to_be_bytes());

    // Fix up length field
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}
