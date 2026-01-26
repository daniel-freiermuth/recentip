//! Shared test helpers for server behavior tests.
//!
//! These helpers are used across multiple test modules to build raw SOME/IP packets,
//! parse responses, and set up test infrastructure.

use bytes::Bytes;
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
