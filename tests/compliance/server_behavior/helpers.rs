//! Shared test helpers for server behavior tests.
//!
//! These helpers are used across multiple test modules to build raw SOME/IP packets,
//! parse responses, and set up test infrastructure.

use bytes::Bytes;
use recentip::prelude::*;
use recentip::wire::{Header, SdMessage, SD_METHOD_ID, SD_SERVICE_ID};

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

/// Helper to extract SD flags from raw packet bytes
pub fn parse_sd_flags(data: &[u8]) -> Option<(bool, bool)> {
    // SD flags are in byte 16 (first byte after SOME/IP header)
    // Bit 7: Reboot flag
    // Bit 6: Unicast flag
    if data.len() < 17 {
        return None;
    }
    let reboot_flag = (data[16] & 0x80) != 0;
    let unicast_flag = (data[16] & 0x40) != 0;
    Some((reboot_flag, unicast_flag))
}

// ============================================================================
// Packet builders
// ============================================================================

/// Build a raw SOME/IP request packet
#[allow(dead_code)]
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
#[allow(dead_code)]
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

/*
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
} */

/*
/// Build a raw SOME/IP-SD OfferService message (UDP endpoint)
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

    // === SOME/IP Header (16 bytes) ===
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes()); // Service ID = SD
    packet.extend_from_slice(&0x8100u16.to_be_bytes()); // Method ID = SD
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes()); // placeholder
    packet.extend_from_slice(&0x0000u16.to_be_bytes()); // Client ID
    packet.extend_from_slice(&0x0001u16.to_be_bytes()); // Session ID
    packet.push(0x01); // Protocol Version
    packet.push(0x01); // Interface Version
    packet.push(0x02); // Message Type = NOTIFICATION
    packet.push(0x00); // Return Code = E_OK

    // === SD Payload ===
    packet.push(0xC0); // Flags (Unicast + Reboot)
    packet.extend_from_slice(&[0x00, 0x00, 0x00]); // Reserved

    // Entries array length (16 bytes for one entry)
    packet.extend_from_slice(&16u32.to_be_bytes());

    // === OfferService Entry (16 bytes) ===
    packet.push(0x01); // Type = OfferService
    packet.push(0x00); // Index 1st options
    packet.push(0x00); // Index 2nd options
    packet.push(0x10); // # of opt 1 | # of opt 2 = 1 option in run 1
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]); // TTL (24-bit)
    packet.extend_from_slice(&minor_version.to_be_bytes());

    // Options array length (12 bytes for IPv4 endpoint)
    packet.extend_from_slice(&12u32.to_be_bytes());

    // === IPv4 Endpoint Option (12 bytes) ===
    packet.extend_from_slice(&9u16.to_be_bytes()); // Length
    packet.push(0x04); // Type = IPv4 Endpoint
    packet.push(0x00); // Reserved
    packet.extend_from_slice(&endpoint_ip.octets());
    packet.push(0x00); // Reserved
    packet.push(0x11); // L4 Protocol = UDP
    packet.extend_from_slice(&endpoint_port.to_be_bytes());

    // Fix up length field
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
} */

/*
/// Build a raw SOME/IP-SD OfferService message with TCP endpoint only
pub fn build_sd_offer_tcp_only(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    minor_version: u32,
    endpoint_ip: std::net::Ipv4Addr,
    endpoint_port: u16,
    ttl: u32,
    session_id: u16,
    reboot_flag: bool,
    unicast_flag: bool,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(56);

    packet.extend_from_slice(&0xFFFFu16.to_be_bytes());
    packet.extend_from_slice(&0x8100u16.to_be_bytes());
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes());
    packet.extend_from_slice(&0x0000u16.to_be_bytes());
    packet.extend_from_slice(&session_id.to_be_bytes());
    packet.push(0x01);
    packet.push(0x01);
    packet.push(0x02);
    packet.push(0x00);

    let mut flags = if unicast_flag { 0x40u8 } else { 0x00u8 };
    if reboot_flag {
        flags |= 0x80;
    }
    packet.push(flags);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);
    packet.extend_from_slice(&16u32.to_be_bytes());

    packet.push(0x01);
    packet.push(0x00);
    packet.push(0x00);
    packet.push(0x10);
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]);
    packet.extend_from_slice(&minor_version.to_be_bytes());

    packet.extend_from_slice(&12u32.to_be_bytes());

    packet.extend_from_slice(&9u16.to_be_bytes());
    packet.push(0x04);
    packet.push(0x00);
    packet.extend_from_slice(&endpoint_ip.octets());
    packet.push(0x00);
    packet.push(0x06); // L4 Protocol = TCP
    packet.extend_from_slice(&endpoint_port.to_be_bytes());

    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
} */

/*
/// Build a raw SOME/IP-SD OfferService message with both UDP and TCP endpoints
pub fn build_sd_offer_dual_stack(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    minor_version: u32,
    endpoint_ip: std::net::Ipv4Addr,
    udp_port: u16,
    tcp_port: u16,
    ttl: u32,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(68);

    packet.extend_from_slice(&0xFFFFu16.to_be_bytes());
    packet.extend_from_slice(&0x8100u16.to_be_bytes());
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes());
    packet.extend_from_slice(&0x0000u16.to_be_bytes());
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    packet.push(0x01);
    packet.push(0x01);
    packet.push(0x02);
    packet.push(0x00);

    packet.push(0xC0);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);
    packet.extend_from_slice(&16u32.to_be_bytes());

    packet.push(0x01);
    packet.push(0x00);
    packet.push(0x00);
    packet.push(0x20); // 2 options in run 1
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]);
    packet.extend_from_slice(&minor_version.to_be_bytes());

    // 24 bytes for 2 endpoints
    packet.extend_from_slice(&24u32.to_be_bytes());

    // UDP endpoint
    packet.extend_from_slice(&9u16.to_be_bytes());
    packet.push(0x04);
    packet.push(0x00);
    packet.extend_from_slice(&endpoint_ip.octets());
    packet.push(0x00);
    packet.push(0x11); // UDP
    packet.extend_from_slice(&udp_port.to_be_bytes());

    // TCP endpoint
    packet.extend_from_slice(&9u16.to_be_bytes());
    packet.push(0x04);
    packet.push(0x00);
    packet.extend_from_slice(&endpoint_ip.octets());
    packet.push(0x00);
    packet.push(0x06); // TCP
    packet.extend_from_slice(&tcp_port.to_be_bytes());

    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
} */

/// Build a raw SOME/IP-SD SubscribeEventgroup message (no endpoint option)
/*
pub fn build_sd_subscribe(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    ttl: u32,
    session_id: u16,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(64);

    packet.extend_from_slice(&0xFFFFu16.to_be_bytes());
    packet.extend_from_slice(&0x8100u16.to_be_bytes());
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes());
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    packet.extend_from_slice(&session_id.to_be_bytes());
    packet.push(0x01);
    packet.push(0x01);
    packet.push(0x02);
    packet.push(0x00);

    packet.push(0xC0);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);
    packet.extend_from_slice(&16u32.to_be_bytes());

    packet.push(0x06); // SubscribeEventgroup
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

    packet.extend_from_slice(&0u32.to_be_bytes());

    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
} */

/// Build a raw SOME/IP-SD SubscribeEventgroup with a TCP endpoint option
pub fn build_sd_subscribe_with_tcp_endpoint(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    ttl: u32,
    client_ip: std::net::Ipv4Addr,
    client_port: u16,
    session_id: u16,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(80);

    packet.extend_from_slice(&0xFFFFu16.to_be_bytes());
    packet.extend_from_slice(&0x8100u16.to_be_bytes());
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes());
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    packet.extend_from_slice(&session_id.to_be_bytes());
    packet.push(0x01);
    packet.push(0x01);
    packet.push(0x02);
    packet.push(0x00);

    packet.push(0xC0);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);
    packet.extend_from_slice(&16u32.to_be_bytes());

    packet.push(0x06); // SubscribeEventgroup
    packet.push(0x00);
    packet.push(0x00);
    packet.push(0x10); // 1 option in run 1
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]);
    packet.push(0x00);
    packet.push(0x00);
    packet.extend_from_slice(&eventgroup_id.to_be_bytes());

    packet.extend_from_slice(&12u32.to_be_bytes());

    packet.extend_from_slice(&9u16.to_be_bytes());
    packet.push(0x04);
    packet.push(0x00);
    packet.extend_from_slice(&client_ip.octets());
    packet.push(0x00);
    packet.push(0x06); // TCP
    packet.extend_from_slice(&client_port.to_be_bytes());

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
    session_id: u16,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(80);

    packet.extend_from_slice(&0xFFFFu16.to_be_bytes());
    packet.extend_from_slice(&0x8100u16.to_be_bytes());
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes());
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    packet.extend_from_slice(&session_id.to_be_bytes());
    packet.push(0x01);
    packet.push(0x01);
    packet.push(0x02);
    packet.push(0x00);

    packet.push(0xC0);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);
    packet.extend_from_slice(&16u32.to_be_bytes());

    packet.push(0x06); // SubscribeEventgroup
    packet.push(0x00);
    packet.push(0x00);
    packet.push(0x10); // 1 option in run 1
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]);
    packet.push(0x00);
    packet.push(0x00);
    packet.extend_from_slice(&eventgroup_id.to_be_bytes());

    packet.extend_from_slice(&12u32.to_be_bytes());

    packet.extend_from_slice(&9u16.to_be_bytes());
    packet.push(0x04);
    packet.push(0x00);
    packet.extend_from_slice(&client_ip.octets());
    packet.push(0x00);
    packet.push(0x11); // UDP
    packet.extend_from_slice(&client_port.to_be_bytes());

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
    session_id: u16,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(64);

    packet.extend_from_slice(&0xFFFFu16.to_be_bytes());
    packet.extend_from_slice(&0x8100u16.to_be_bytes());
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes());
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    packet.extend_from_slice(&session_id.to_be_bytes());
    packet.push(0x01);
    packet.push(0x01);
    packet.push(0x02);
    packet.push(0x00);

    packet.push(0xC0);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);
    packet.extend_from_slice(&16u32.to_be_bytes());

    packet.push(0x07); // SubscribeEventgroupAck
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

    packet.extend_from_slice(&0u32.to_be_bytes());

    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}
