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

/// Build a raw SOME/IP request packet
pub fn build_request(
    service_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    payload: &[u8],
) -> Vec<u8> {
    let length = 8 + payload.len() as u32;
    let mut packet = Vec::with_capacity(16 + payload.len());

    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&method_id.to_be_bytes());
    packet.extend_from_slice(&length.to_be_bytes());
    packet.extend_from_slice(&client_id.to_be_bytes());
    packet.extend_from_slice(&session_id.to_be_bytes());
    packet.push(0x01); // Protocol Version
    packet.push(0x01); // Interface Version
    packet.push(0x00); // Message Type (REQUEST)
    packet.push(0x00); // Return Code (E_OK)
    packet.extend_from_slice(payload);

    packet
}

/// Build a raw SOME/IP fire-and-forget (REQUEST_NO_RETURN) packet
pub fn build_fire_and_forget_request(
    service_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    payload: &[u8],
) -> Vec<u8> {
    let length = 8 + payload.len() as u32;
    let mut packet = Vec::with_capacity(16 + payload.len());

    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&method_id.to_be_bytes());
    packet.extend_from_slice(&length.to_be_bytes());
    packet.extend_from_slice(&client_id.to_be_bytes());
    packet.extend_from_slice(&session_id.to_be_bytes());
    packet.push(0x01); // Protocol Version
    packet.push(0x01); // Interface Version
    packet.push(0x01); // Message Type (REQUEST_NO_RETURN)
    packet.push(0x00); // Return Code (E_OK)
    packet.extend_from_slice(payload);

    packet
}

/// Build a raw SOME/IP response packet based on a request header
pub fn build_response(request: &Header, payload: &[u8]) -> Vec<u8> {
    let length = 8 + payload.len() as u32;
    let mut packet = Vec::with_capacity(16 + payload.len());

    packet.extend_from_slice(&request.service_id.to_be_bytes());
    packet.extend_from_slice(&request.method_id.to_be_bytes());
    packet.extend_from_slice(&length.to_be_bytes());
    packet.extend_from_slice(&request.client_id.to_be_bytes());
    packet.extend_from_slice(&request.session_id.to_be_bytes());
    packet.push(0x01); // Protocol Version
    packet.push(request.interface_version);
    packet.push(0x80); // Message Type (RESPONSE)
    packet.push(0x00); // Return Code (E_OK)
    packet.extend_from_slice(payload);

    packet
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
