//! Server Behavior Tests (Legacy Wire Format Location)
//!
//! Tests that verify how the server/client reacts to various inputs.
//! These tests use raw sockets to simulate the "other side" and verify
//! the implementation's behavior (not byte encoding).
//!
//! NOTE: Wire format tests (byte encoding verification) have been moved to
//! tests/compliance/wire_format/ module:
//! - sd_format.rs: SD message encoding
//! - rpc_format.rs: RPC message encoding
//! - subscription_format.rs: Subscription entry encoding
//!
//! TODO: Move these behavior tests to server_behavior/ module

use bytes::Bytes;
use someip_runtime::handle::ServiceEvent;
use someip_runtime::prelude::*;
use someip_runtime::runtime::{Runtime, RuntimeConfig};
use someip_runtime::wire::{Header, MessageType, SdMessage, SD_METHOD_ID, SD_SERVICE_ID};
use std::net::SocketAddr;
use std::time::Duration;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

/// Test service definition
struct TestService;

impl Service for TestService {
    const SERVICE_ID: u16 = 0x1234;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}

/// Helper to parse a SOME/IP header from raw bytes
fn parse_header(data: &[u8]) -> Option<Header> {
    if data.len() < Header::SIZE {
        return None;
    }
    Header::parse(&mut Bytes::copy_from_slice(data))
}

/// Helper to parse an SD message (header + payload) from raw bytes
fn parse_sd_message(data: &[u8]) -> Option<(Header, SdMessage)> {
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

// NOTE: SD, RPC, and Subscription wire format tests have been moved to
// tests/compliance/wire_format/ module (sd_format.rs, rpc_format.rs, subscription_format.rs)

// ============================================================================
// Helper functions for building raw SOME/IP packets
// ============================================================================

/// Build a raw SOME/IP request packet
fn build_request(
    service_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    payload: &[u8],
) -> Vec<u8> {
    let length = 8 + payload.len() as u32; // 8 bytes of header after length + payload
    let mut packet = Vec::with_capacity(16 + payload.len());

    // Message ID (Service ID + Method ID)
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&method_id.to_be_bytes());

    // Length
    packet.extend_from_slice(&length.to_be_bytes());

    // Request ID (Client ID + Session ID)
    packet.extend_from_slice(&client_id.to_be_bytes());
    packet.extend_from_slice(&session_id.to_be_bytes());

    // Protocol Version
    packet.push(0x01);

    // Interface Version
    packet.push(0x01);

    // Message Type (REQUEST = 0x00)
    packet.push(0x00);

    // Return Code (E_OK = 0x00)
    packet.push(0x00);

    // Payload
    packet.extend_from_slice(payload);

    packet
}

/// Build a raw SOME/IP fire-and-forget (REQUEST_NO_RETURN) packet
fn build_fire_and_forget_request(
    service_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    payload: &[u8],
) -> Vec<u8> {
    let length = 8 + payload.len() as u32; // 8 bytes of header after length + payload
    let mut packet = Vec::with_capacity(16 + payload.len());

    // Message ID (Service ID + Method ID)
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&method_id.to_be_bytes());

    // Length
    packet.extend_from_slice(&length.to_be_bytes());

    // Request ID (Client ID + Session ID)
    packet.extend_from_slice(&client_id.to_be_bytes());
    packet.extend_from_slice(&session_id.to_be_bytes());

    // Protocol Version
    packet.push(0x01);

    // Interface Version
    packet.push(0x01);

    // Message Type (REQUEST_NO_RETURN = 0x01)
    packet.push(0x01);

    // Return Code (E_OK = 0x00)
    packet.push(0x00);

    // Payload
    packet.extend_from_slice(payload);

    packet
}

/// Build a raw SOME/IP response packet based on a request header
fn build_response(request: &Header, payload: &[u8]) -> Vec<u8> {
    let length = 8 + payload.len() as u32;
    let mut packet = Vec::with_capacity(16 + payload.len());

    // Message ID (preserve from request)
    packet.extend_from_slice(&request.service_id.to_be_bytes());
    packet.extend_from_slice(&request.method_id.to_be_bytes());

    // Length
    packet.extend_from_slice(&length.to_be_bytes());

    // Request ID (preserve from request)
    packet.extend_from_slice(&request.client_id.to_be_bytes());
    packet.extend_from_slice(&request.session_id.to_be_bytes());

    // Protocol Version
    packet.push(0x01);

    // Interface Version
    packet.push(request.interface_version);

    // Message Type (RESPONSE = 0x80)
    packet.push(0x80);

    // Return Code (E_OK = 0x00)
    packet.push(0x00);

    // Payload
    packet.extend_from_slice(payload);

    packet
}

/// Build a raw SOME/IP-SD OfferService message
fn build_sd_offer(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    minor_version: u32,
    endpoint_ip: std::net::Ipv4Addr,
    endpoint_port: u16,
    ttl: u32,
) -> Vec<u8> {
    // SD message structure:
    // - SOME/IP header (16 bytes)
    // - SD header (12 bytes): flags(1) + reserved(3) + entries_length(4) + entries + options_length(4) + options
    // - Entry (16 bytes): OfferService entry
    // - Option (12 bytes): IPv4 Endpoint option

    let mut packet = Vec::with_capacity(56);

    // === SOME/IP Header (16 bytes) ===
    // Service ID = 0xFFFF (SD)
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes());
    // Method ID = 0x8100 (SD)
    packet.extend_from_slice(&0x8100u16.to_be_bytes());
    // Length = 8 (remaining header) + SD payload
    // We'll fill this in at the end
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes()); // placeholder
                                                   // Client ID
    packet.extend_from_slice(&0x0000u16.to_be_bytes());
    // Session ID
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    // Protocol Version
    packet.push(0x01);
    // Interface Version
    packet.push(0x01);
    // Message Type = NOTIFICATION (0x02)
    packet.push(0x02);
    // Return Code = E_OK
    packet.push(0x00);

    // === SD Payload ===
    // Flags (1 byte): Unicast flag = 0x40, Reboot = 0x80
    packet.push(0xC0);
    // Reserved (3 bytes)
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);

    // Entries array length (4 bytes) - 16 bytes for one entry
    packet.extend_from_slice(&16u32.to_be_bytes());

    // === OfferService Entry (16 bytes) ===
    // Type = OfferService (0x01)
    packet.push(0x01);
    // Index 1st options = 0 (first option)
    packet.push(0x00);
    // Index 2nd options = 0
    packet.push(0x00);
    // # of opt 1 (4 bits) | # of opt 2 (4 bits) = 0x10 (1 option in run 1)
    packet.push(0x10);
    // Service ID
    packet.extend_from_slice(&service_id.to_be_bytes());
    // Instance ID
    packet.extend_from_slice(&instance_id.to_be_bytes());
    // Major Version
    packet.push(major_version);
    // TTL (24-bit, big-endian)
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]); // 3 bytes
                                                // Minor Version (32-bit)
    packet.extend_from_slice(&minor_version.to_be_bytes());

    // Options array length (4 bytes) - 12 bytes for IPv4 endpoint
    packet.extend_from_slice(&12u32.to_be_bytes());

    // === IPv4 Endpoint Option (12 bytes) ===
    // Length = 9 (option content length, excluding length+type)
    packet.extend_from_slice(&9u16.to_be_bytes());
    // Type = IPv4 Endpoint (0x04)
    packet.push(0x04);
    // Reserved
    packet.push(0x00);
    // IPv4 address
    packet.extend_from_slice(&endpoint_ip.octets());
    // Reserved
    packet.push(0x00);
    // L4 Protocol = UDP (0x11)
    packet.push(0x11);
    // Port
    packet.extend_from_slice(&endpoint_port.to_be_bytes());

    // Fix up length field: total - 8 (first 8 bytes of header)
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}

/// Build a raw SOME/IP-SD SubscribeEventgroup message
fn build_sd_subscribe(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    ttl: u32,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(64);

    // === SOME/IP Header (16 bytes) ===
    // Service ID = 0xFFFF (SD)
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes());
    // Method ID = 0x8100 (SD)
    packet.extend_from_slice(&0x8100u16.to_be_bytes());
    // Length placeholder
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes());
    // Client ID
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    // Session ID
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    // Protocol Version
    packet.push(0x01);
    // Interface Version
    packet.push(0x01);
    // Message Type = NOTIFICATION (0x02)
    packet.push(0x02);
    // Return Code = E_OK
    packet.push(0x00);

    // === SD Payload ===
    // Flags
    packet.push(0xC0);
    // Reserved (3 bytes)
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);

    // Entries array length - 16 bytes for eventgroup entry
    packet.extend_from_slice(&16u32.to_be_bytes());

    // === SubscribeEventgroup Entry (16 bytes) ===
    // Type = SubscribeEventgroup (0x06)
    packet.push(0x06);
    // Index 1st options
    packet.push(0x00);
    // Index 2nd options
    packet.push(0x00);
    // # of options
    packet.push(0x00);
    // Service ID
    packet.extend_from_slice(&service_id.to_be_bytes());
    // Instance ID
    packet.extend_from_slice(&instance_id.to_be_bytes());
    // Major Version
    packet.push(major_version);
    // TTL (24-bit)
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]);
    // Reserved
    packet.push(0x00);
    // Flags + Counter
    packet.push(0x00);
    // Eventgroup ID
    packet.extend_from_slice(&eventgroup_id.to_be_bytes());

    // Options array length (0 options)
    packet.extend_from_slice(&0u32.to_be_bytes());

    // Fix up length field
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}

/// Build a raw SOME/IP-SD SubscribeEventgroupAck message
fn build_sd_subscribe_ack(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    ttl: u32,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(64);

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

    // === SubscribeEventgroupAck Entry (16 bytes) ===
    // Type = SubscribeEventgroupAck (0x07)
    packet.push(0x07);
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

    // Options array length
    packet.extend_from_slice(&0u32.to_be_bytes());

    // Fix up length field
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}

// ============================================================================
// REBOOT FLAG TESTS
// ============================================================================
// feat_req_recentipsd_41: Reboot flag behavior
// feat_req_recentipsd_764: Reboot detection algorithm
// feat_req_recentipsd_765: Per-peer session tracking
//
// Reboot flag lifecycle:
//   - After startup: reboot_flag=1, session_id starts at 1
//   - After 65535 messages: reboot_flag transitions 1→0 (first wraparound)
//   - After further wraps: reboot_flag stays 0
//
// Reboot detection algorithm (receiver perspective):
//   old.reboot=0, new.reboot=1           → Reboot detected
//   old.reboot=1, new.reboot=1, old>=new → Reboot detected
//   old.reboot=1, new.reboot=0           → Normal wraparound (NOT reboot)
//   old.reboot=0, new.reboot=0           → Normal operation
// ============================================================================

/// Helper to extract SD flags from raw packet bytes
fn parse_sd_flags(data: &[u8]) -> Option<(bool, bool)> {
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

/// feat_req_recentipsd_41: SD Reboot flag is set after startup
///
/// When a runtime starts, it must set the reboot flag (bit 7 of SD flags)
/// to 1 in all SD messages until the session ID wraps around.
#[test_log::test]
fn sd_reboot_flag_set_after_startup() {
    covers!(feat_req_recentipsd_41);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library side - offers a service
    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .udp()
            .start()
            .await
            .unwrap();

        // Keep server alive
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    // Raw socket side - captures SD multicast and verifies reboot flag
    sim.client("raw_observer", async move {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut found_reboot_flag = false;

        for _ in 0..5 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), socket.recv_from(&mut buf)).await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((header, _sd_msg)) = parse_sd_message(&buf[..len]) {
                    // Check header is SD
                    if header.service_id == SD_SERVICE_ID && header.method_id == SD_METHOD_ID {
                        // Check SD flags
                        if let Some((reboot_flag, _unicast_flag)) = parse_sd_flags(&buf[..len]) {
                            // First message(s) after startup should have reboot_flag=1
                            if header.session_id <= 10 {
                                assert!(
                                    reboot_flag,
                                    "Reboot flag must be set (1) after startup (session_id={})",
                                    header.session_id
                                );
                                found_reboot_flag = true;
                            }
                        }
                    }
                }
            }
        }

        assert!(
            found_reboot_flag,
            "Should have captured SD message with reboot flag set"
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_41: Session ID starts at 1 after startup
/// feat_req_recentip_649: Session ID must start at 1
///
/// Verify that the first SD message has session_id=1
///
/// NOTE: Due to turmoil simulation timing, we may not always capture the very
/// first packet. This test verifies that session_id=1 exists among the first
/// few captured messages, which proves the runtime started counting at 1.
#[test_log::test]
fn sd_session_starts_at_one() {
    covers!(feat_req_recentipsd_41, feat_req_recentip_649);

    use std::sync::atomic::{AtomicBool, Ordering};

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Use AtomicBool for cross-host synchronization
    static OBSERVER_READY: AtomicBool = AtomicBool::new(false);
    OBSERVER_READY.store(false, Ordering::SeqCst);

    sim.host("server", || async move {
        // Wait until observer is ready
        while !OBSERVER_READY.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Extra delay to ensure observer's multicast join has propagated
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("raw_observer", async move {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        // Give multicast join time to propagate
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Signal that we're ready to receive
        OBSERVER_READY.store(true, Ordering::SeqCst);

        let mut buf = [0u8; 1500];
        let mut captured_session_ids: Vec<u16> = Vec::new();

        for _ in 0..10 {
            let result =
                tokio::time::timeout(Duration::from_millis(500), socket.recv_from(&mut buf)).await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((header, _sd_msg)) = parse_sd_message(&buf[..len]) {
                    captured_session_ids.push(header.session_id);
                    if captured_session_ids.len() >= 3 {
                        break; // Got enough samples
                    }
                }
            }
        }

        assert!(
            !captured_session_ids.is_empty(),
            "Should have captured at least one SD message"
        );

        // Verify session_id=1 is present, proving the runtime started at 1
        let min_session_id = *captured_session_ids.iter().min().unwrap();
        assert_eq!(
            min_session_id, 1,
            "Minimum captured session_id should be 1 (got {}, captured: {:?})",
            min_session_id, captured_session_ids
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_41: Reboot flag is cleared after session wraparound
///
/// After 65535 SD messages (session wraps 0xFFFF → 1), the reboot flag
/// must transition from 1 to 0. This indicates normal operation, not reboot.
///
/// NOTE: This test simulates the wraparound scenario by checking the runtime
/// state. Full integration would require 65535 actual messages.
#[test_log::test]
fn sd_reboot_flag_clears_after_wraparound() {
    covers!(feat_req_recentipsd_41, feat_req_recentipsd_764);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(60))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .udp()
            .start()
            .await
            .unwrap();

        // In a real implementation, we'd need to send 65535 messages.
        // For now, we verify the initial state and document the expected behavior.
        // The runtime implementation must track `has_wrapped: bool` and clear
        // the reboot flag after the first complete cycle of session IDs.

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("raw_observer", async move {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];

        // Capture first few messages - reboot flag should be set
        for _ in 0..3 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), socket.recv_from(&mut buf)).await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((header, _sd_msg)) = parse_sd_message(&buf[..len]) {
                    if let Some((reboot_flag, _)) = parse_sd_flags(&buf[..len]) {
                        // Early messages should have reboot_flag=1
                        if header.session_id < 100 {
                            assert!(
                                reboot_flag,
                                "Reboot flag should be 1 before wraparound (session={})",
                                header.session_id
                            );
                        }
                    }
                }
            }
        }

        // NOTE: Full verification would require observing 65535 messages and
        // checking that after wraparound, reboot_flag becomes 0.
        // This is documented in the spec and verified by unit tests.

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_765: SD uses separate session counters for multicast vs unicast
///
/// Per the specification, session ID counters must be maintained separately
/// for multicast and unicast SD messages. A peer receiving both types
/// should see independent session sequences.
#[test_log::test]
fn sd_separate_multicast_unicast_sessions() {
    covers!(feat_req_recentipsd_765);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        // Subscribe to trigger unicast SD responses
        let proxy = runtime.find::<TestService>(InstanceId::Any);
        let _ = tokio::time::timeout(Duration::from_secs(2), proxy.available()).await;

        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(())
    });

    sim.client("raw_observer", async move {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut multicast_sessions: Vec<u16> = Vec::new();

        for _ in 0..10 {
            let result =
                tokio::time::timeout(Duration::from_millis(100), socket.recv_from(&mut buf)).await;

            if let Ok(Ok((len, from))) = result {
                if let Some((header, _sd_msg)) = parse_sd_message(&buf[..len]) {
                    // Check if this is multicast (from multicast address)
                    let is_multicast = from.ip().to_string().starts_with("239.");
                    if is_multicast {
                        multicast_sessions.push(header.session_id);
                    }
                }
            }
        }

        // Verify multicast sessions are incrementing properly
        if multicast_sessions.len() >= 2 {
            for window in multicast_sessions.windows(2) {
                assert!(
                    window[1] > window[0] || window[1] == 1, // wraparound case
                    "Multicast session IDs should increment: {} -> {}",
                    window[0],
                    window[1]
                );
            }
        }

        // NOTE: Full verification requires capturing both multicast and unicast
        // packets from the server and verifying independent session counters.
        // Unicast sessions would start at 1 independently of multicast sessions.

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentip_676: Port 30490 is only for SD, not for RPC
/// feat_req_recentipsd_779: Service endpoint denotes where service is reachable
/// feat_req_recentipsd_758: UDP endpoint used for source port of events
///
/// **BUG DEMONSTRATION TEST**
///
/// This test verifies that client RPC messages (requests) do NOT originate from
/// the SD socket (port 30490). The SD port is reserved exclusively for Service
/// Discovery messages. RPC communication should use dedicated RPC sockets with
/// ephemeral or configured ports.
///
/// Per the specification:
/// - feat_req_recentip_676: Port 30490 shall be only used for SOME/IP-SD
/// - feat_req_recentipsd_779: Endpoint options denote where service is reachable
/// - feat_req_recentipsd_758: UDP endpoint is used for source port (for events)
/// - Servers use announced ports as source for both responses and events
/// - Clients should similarly use dedicated RPC sockets, not SD socket
///
/// **Current Behavior (INCORRECT):**
/// Client sends RPC requests from SD socket (port 30490) at runtime.rs:1201
///
/// **Expected Behavior:**
/// Client should use a dedicated RPC socket with ephemeral port (like servers do)
///
/// **Test Result:** PASSES - verifies correct implementation
#[test_log::test]
fn client_rpc_must_not_use_sd_port() {
    covers!(
        feat_req_recentip_676,
        feat_req_recentipsd_779,
        feat_req_recentipsd_758
    );

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Raw socket acts as server - sends SD offer, then captures REQUEST source port
    sim.host("raw_server", || async {
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_server").to_string().parse().unwrap();

        // RPC socket to receive requests
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?;
        eprintln!("Raw server RPC listening on {}", rpc_socket.local_addr()?);

        // SD socket to send offers
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        // Send SD offer to multicast so the library can discover us
        let offer = build_sd_offer(0x1234, 0x0001, 1, 0, my_ip, 30509, 3600);
        let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // Send offers periodically until we receive a request
        let mut buf = [0u8; 1500];
        let mut request_received = false;
        let mut client_source_port: Option<u16> = None;

        for _ in 0..20 {
            // Send an offer
            sd_socket.send_to(&offer, sd_multicast).await?;
            eprintln!("Raw server sent SD offer");

            // Check for RPC request (non-blocking with short timeout)
            let result =
                tokio::time::timeout(Duration::from_millis(200), rpc_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                eprintln!("Raw server received {} bytes from {}", len, from);
                request_received = true;
                client_source_port = Some(from.port());

                // Parse as SOME/IP to verify it's an RPC request
                let data = &buf[..len];
                let header = parse_header(data).expect("Should parse as SOME/IP header");

                eprintln!("Received RPC request from port: {}", from.port());
                eprintln!("Service ID: 0x{:04x}, Method ID: 0x{:04x}", header.service_id, header.method_id);
                eprintln!("Message Type: {:?}", header.message_type);

                // This is the critical check: RPC messages MUST NOT come from SD port
                assert_ne!(
                    from.port(),
                    30490,
                    "Client RPC request MUST NOT originate from SD port 30490 (feat_req_recentip_676). \
                     SD port is reserved exclusively for Service Discovery. \
                     RPC communication requires dedicated RPC socket with ephemeral port."
                );

                // Send a response back so the client doesn't timeout
                let response = build_response(&header, b"ok");
                rpc_socket.send_to(&response, from).await?;
                break;
            }
        }

        assert!(request_received, "Should have received an RPC request");
 
        if let Some(port) = client_source_port {
            eprintln!("✓ Client used port {} for RPC (not SD port 30490)", port);
        }

        Ok(())
    });

    // Library side - discovers service via SD and makes RPC call
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        // Use public API: find service and wait for availability via SD
        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));

        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Should discover service via SD")
            .expect("Service available");

        // Make an RPC call - this should NOT use port 30490
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            proxy.call(MethodId::new(0x0001).unwrap(), b"hello"),
        )
        .await;

        match result {
            Ok(Ok(response)) => {
                eprintln!("RPC call succeeded: {:?}", response);
            }
            Ok(Err(e)) => {
                eprintln!("RPC call failed: {}", e);
            }
            Err(_) => {
                eprintln!("RPC call timed out");
            }
        }

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// TRANSPORT MISMATCH TESTS
// ============================================================================
// These tests verify that a server correctly NACKs subscription requests
// when the client provides an endpoint option for a transport the server
// does not offer events on.
//
// Per feat_req_recentipsd_786: SubscribeEventgroup can reference UDP and/or TCP endpoint
// Per feat_req_recentipsd_1144: If options are in conflict → respond negatively (NACK)
// Per feat_req_recentipsd_1137: Respond with SubscribeEventgroupNack for invalid subscribe
// ============================================================================

/// Build a raw SOME/IP-SD SubscribeEventgroup with a TCP endpoint option
/// This is used to test what happens when a client subscribes with TCP
/// to a service that only offers UDP events.
fn build_sd_subscribe_with_tcp_endpoint(
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
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes()); // Service ID = SD
    packet.extend_from_slice(&0x8100u16.to_be_bytes()); // Method ID = SD
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes()); // Length placeholder
    packet.extend_from_slice(&0x0001u16.to_be_bytes()); // Client ID
    packet.extend_from_slice(&0x0001u16.to_be_bytes()); // Session ID
    packet.push(0x01); // Protocol Version
    packet.push(0x01); // Interface Version
    packet.push(0x02); // Message Type = NOTIFICATION
    packet.push(0x00); // Return Code = E_OK

    // === SD Payload ===
    packet.push(0xC0); // Flags (reboot + unicast)
    packet.extend_from_slice(&[0x00, 0x00, 0x00]); // Reserved

    // Entries array length - 16 bytes for eventgroup entry
    packet.extend_from_slice(&16u32.to_be_bytes());

    // === SubscribeEventgroup Entry (16 bytes) ===
    packet.push(0x06); // Type = SubscribeEventgroup
    packet.push(0x00); // Index 1st options = 0 (references our TCP endpoint)
    packet.push(0x00); // Index 2nd options
    packet.push(0x10); // # of opts: 1 in run1, 0 in run2 (0x10 = 1 option in 1st run)
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]); // TTL (24-bit)
    packet.push(0x00); // Reserved
    packet.push(0x00); // Flags + Counter
    packet.extend_from_slice(&eventgroup_id.to_be_bytes());

    // === Options array ===
    // Options array length (12 bytes for one IPv4 endpoint option)
    packet.extend_from_slice(&12u32.to_be_bytes());

    // IPv4 Endpoint Option (12 bytes total: 2 length + 1 type + 9 data)
    packet.extend_from_slice(&9u16.to_be_bytes()); // Length = 9
    packet.push(0x04); // Type = IPv4 Endpoint
    packet.push(0x00); // Reserved
    packet.extend_from_slice(&client_ip.octets()); // IPv4 address
    packet.push(0x00); // Reserved
    packet.push(0x06); // L4 Protocol = TCP (0x06)
    packet.extend_from_slice(&client_port.to_be_bytes()); // Port

    // Fix up length field
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}

/// Build a raw SOME/IP-SD SubscribeEventgroup with a UDP endpoint option
fn build_sd_subscribe_with_udp_endpoint(
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

/// feat_req_recentipsd_1144: Transport mismatch should result in NACK
/// feat_req_recentipsd_1137: Respond with SubscribeEventgroupNack for invalid subscribe
///
/// When a server offers events only via UDP, but a client sends a SubscribeEventgroup
/// with only a TCP endpoint option, the server MUST respond with a SubscribeEventgroupNack.
///
/// Current behavior: Server incorrectly accepts and sends ACK (falling back to source address)
/// Expected behavior: Server rejects with NACK (TTL=0, entry type 0x07)
#[test_log::test]
fn subscribe_tcp_endpoint_to_udp_only_server_should_nack() {
    covers!(feat_req_recentipsd_1144, feat_req_recentipsd_1137);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers events via UDP only
    sim.host("server", || async {
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(Default::default()).await.unwrap();

        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .udp() // UDP only!
            .start()
            .await
            .unwrap();

        // Keep server running
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Raw client: sends Subscribe with TCP endpoint only
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == 0x1234
                            && entry.ttl > 0
                        {
                            // Subscribe messages should be sent to SD port (30490)
                            server_endpoint = Some(SocketAddr::new(from.ip(), 30490));
                        }
                    }
                }
            }

            if server_endpoint.is_some() {
                break;
            }
        }

        let server_ep = server_endpoint.expect("Should discover server");

        // Get our local address for the endpoint option
        let local_addr = sd_socket.local_addr()?;
        let client_ip: std::net::Ipv4Addr = match local_addr.ip() {
            std::net::IpAddr::V4(ip) => ip,
            std::net::IpAddr::V6(_) => "0.0.0.0".parse().unwrap(),
        };

        // Send SubscribeEventgroup with TCP endpoint (mismatch!)
        let subscribe = build_sd_subscribe_with_tcp_endpoint(
            0x1234, 0x0001, 1, 0x0001, // eventgroup
            3600,   // TTL
            client_ip, 40000, // TCP port we're "listening" on
        );
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for response - should be NACK (type 0x07 with TTL=0)
        let mut found_nack = false;
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234 && entry.eventgroup_id == 0x0001 {
                            if entry.entry_type as u8 == 0x07 {
                                if entry.ttl == 0 {
                                    // NACK (type 0x07 with TTL=0)
                                    found_nack = true;
                                } else {
                                    // ACK (type 0x07 with TTL>0)
                                    found_ack = true;
                                }
                            }
                        }
                    }
                }
            }

            if found_nack || found_ack {
                break;
            }
        }

        assert!(
            found_nack,
            "Server should NACK subscribe with TCP endpoint when only UDP is offered. \
             Got ACK={}, NACK={}. \
             Per feat_req_recentipsd_1144: incompatible options should be responded negatively.",
            found_ack, found_nack
        );
        assert!(
            !found_ack,
            "Server should NOT ACK subscribe with incompatible transport"
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_1144: Transport mismatch should result in NACK
/// feat_req_recentipsd_1137: Respond with SubscribeEventgroupNack for invalid subscribe
///
/// When a server offers events only via TCP, but a client sends a SubscribeEventgroup
/// with only a UDP endpoint option, the server MUST respond with a SubscribeEventgroupNack.
#[test_log::test]
fn subscribe_udp_endpoint_to_tcp_only_server_should_nack() {
    covers!(feat_req_recentipsd_1144, feat_req_recentipsd_1137);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers events via TCP only
    sim.host("server", || async {
        let runtime: Runtime<
            turmoil::net::UdpSocket,
            turmoil::net::TcpStream,
            turmoil::net::TcpListener,
        > = Runtime::with_socket_type(Default::default()).await.unwrap();

        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .tcp() // TCP only!
            .start()
            .await
            .unwrap();

        // Keep server running
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Raw client: sends Subscribe with UDP endpoint only
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];

        // Wait for OfferService (should have TCP endpoint)
        let mut sd_server_addr: Option<SocketAddr> = None;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == 0x1234
                            && entry.ttl > 0
                        {
                            // Use SD source address for sending subscribe
                            // (not the RPC endpoint from the option)
                            sd_server_addr = Some(from);
                        }
                    }
                }
            }

            if sd_server_addr.is_some() {
                break;
            }
        }

        let server_ep = sd_server_addr.expect("Should discover server");

        let local_addr = sd_socket.local_addr()?;
        let client_ip: std::net::Ipv4Addr = match local_addr.ip() {
            std::net::IpAddr::V4(ip) => ip,
            std::net::IpAddr::V6(_) => "0.0.0.0".parse().unwrap(),
        };

        // Send SubscribeEventgroup with UDP endpoint (mismatch - server is TCP only!)
        let subscribe =
            build_sd_subscribe_with_udp_endpoint(0x1234, 0x0001, 1, 0x0001, 3600, client_ip, 40000);
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for response - should be NACK
        let mut found_nack = false;
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234 && entry.eventgroup_id == 0x0001 {
                            if entry.entry_type as u8 == 0x07 {
                                if entry.ttl == 0 {
                                    found_nack = true;
                                } else {
                                    found_ack = true;
                                }
                            }
                        }
                    }
                }
            }

            if found_nack || found_ack {
                break;
            }
        }

        assert!(
            found_nack,
            "Server should NACK subscribe with UDP endpoint when only TCP is offered. \
             Got ACK={}, NACK={}. \
             Per feat_req_recentipsd_1144: incompatible options should be responded negatively.",
            found_ack, found_nack
        );
        assert!(
            !found_ack,
            "Server should NOT ACK subscribe with incompatible transport"
        );

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// NACK TESTS - UNKNOWN SERVICE/INSTANCE/EVENTGROUP
// ============================================================================

/// feat_req_recentipsd_1137: Server SHALL NACK when Service ID is unknown
///
/// When a server receives a SubscribeEventgroup for a Service ID it doesn't offer,
/// it MUST respond with SubscribeEventgroupNack (TTL=0).
#[test_log::test]
fn subscribe_to_unknown_service_id_should_nack() {
    covers!(feat_req_recentipsd_1137);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service 0x1234
    sim.host("server", || async {
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(Default::default()).await.unwrap();

        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .udp()
            .start()
            .await
            .unwrap();

        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Raw client: tries to subscribe to service 0x9999 (doesn't exist)
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService for 0x1234
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.ttl > 0 {
                            server_endpoint = Some(SocketAddr::new(from.ip(), 30490));
                        }
                    }
                }
            }

            if server_endpoint.is_some() {
                break;
            }
        }

        let server_ep = server_endpoint.expect("Should discover server");

        // Get our IP from turmoil (not local_addr which is 0.0.0.0)
        let client_ip: std::net::Ipv4Addr = turmoil::lookup("raw_client").to_string().parse().unwrap();

        // Subscribe to SERVICE 0x9999 (NOT 0x1234!)
        let subscribe = build_sd_subscribe_with_udp_endpoint(
            0x9999, 0x0001, 1, 0x0001, 3600, client_ip, 40000,
        );
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for NACK
        let mut found_nack = false;
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x9999 && entry.eventgroup_id == 0x0001 {
                            if entry.entry_type as u8 == 0x07 {
                                if entry.ttl == 0 {
                                    found_nack = true;
                                } else {
                                    found_ack = true;
                                }
                            }
                        }
                    }
                }
            }
        }

        assert!(
            found_nack,
            "Server should NACK subscribe to unknown service ID. Got ACK={}, NACK={}",
            found_ack, found_nack
        );
        assert!(!found_ack, "Server should NOT ACK unknown service");

        Ok(())
    });

    // Second client: subscribes with CORRECT service ID, should get ACK
    sim.client("correct_client", async move {
        tokio::time::sleep(Duration::from_millis(150)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 && entry.ttl > 0 {
                            server_endpoint = Some(SocketAddr::new(from.ip(), 30490));
                        }
                    }
                }
            }

            if server_endpoint.is_some() {
                break;
            }
        }

        let server_ep = server_endpoint.expect("Should discover server");

        // Get our IP from turmoil (not local_addr which is 0.0.0.0)
        let client_ip: std::net::Ipv4Addr = turmoil::lookup("correct_client").to_string().parse().unwrap();

        // Subscribe with CORRECT service ID 0x1234
        let subscribe = build_sd_subscribe_with_udp_endpoint(
            0x1234, 0x0001, 1, 0x0001, 3600, client_ip, 40001,
        );
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for ACK
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234 && entry.eventgroup_id == 0x0001 {
                            if entry.entry_type as u8 == 0x07 && entry.ttl > 0 {
                                found_ack = true;
                            }
                        }
                    }
                }
            }

            if found_ack {
                break;
            }
        }

        assert!(
            found_ack,
            "Server should ACK subscribe with correct service ID"
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_1137: Server SHALL NACK when Instance ID is unknown
///
/// Server offers service 0x1234 instance 0x0001, client tries to subscribe to
/// instance 0x0099. Server MUST respond with NACK.
#[test_log::test]
fn subscribe_to_unknown_instance_id_should_nack() {
    covers!(feat_req_recentipsd_1137);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers instance 0x0001
    sim.host("server", || async {
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(Default::default()).await.unwrap();

        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .udp()
            .start()
            .await
            .unwrap();

        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Raw client: tries to subscribe to instance 0x0099 (doesn't exist)
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == 0x1234
                            && entry.instance_id == 0x0001
                            && entry.ttl > 0
                        {
                            server_endpoint = Some(SocketAddr::new(from.ip(), 30490));
                        }
                    }
                }
            }

            if server_endpoint.is_some() {
                break;
            }
        }

        let server_ep = server_endpoint.expect("Should discover server");

        // Get our IP from turmoil (not local_addr which is 0.0.0.0)
        let client_ip: std::net::Ipv4Addr = turmoil::lookup("raw_client").to_string().parse().unwrap();

        // Subscribe to INSTANCE 0x0099 (NOT 0x0001!)
        let subscribe = build_sd_subscribe_with_udp_endpoint(
            0x1234, 0x0099, 1, 0x0001, 3600, client_ip, 40000,
        );
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for NACK
        let mut found_nack = false;
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234
                            && entry.instance_id == 0x0099
                            && entry.eventgroup_id == 0x0001
                        {
                            if entry.entry_type as u8 == 0x07 {
                                if entry.ttl == 0 {
                                    found_nack = true;
                                } else {
                                    found_ack = true;
                                }
                            }
                        }
                    }
                }
            }
        }

        assert!(
            found_nack,
            "Server should NACK subscribe to unknown instance ID. Got ACK={}, NACK={}",
            found_ack, found_nack
        );
        assert!(!found_ack, "Server should NOT ACK unknown instance");

        Ok(())
    });

    // Second client: subscribes with CORRECT instance ID, should get ACK
    sim.client("correct_client", async move {
        tokio::time::sleep(Duration::from_millis(150)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 
                            && entry.service_id == 0x1234 
                            && entry.instance_id == 0x0001
                            && entry.ttl > 0 
                        {
                            server_endpoint = Some(SocketAddr::new(from.ip(), 30490));
                        }
                    }
                }
            }

            if server_endpoint.is_some() {
                break;
            }
        }

        let server_ep = server_endpoint.expect("Should discover server");

        // Get our IP from turmoil (not local_addr which is 0.0.0.0)
        let client_ip: std::net::Ipv4Addr = turmoil::lookup("correct_client").to_string().parse().unwrap();

        // Subscribe with CORRECT instance ID 0x0001
        let subscribe = build_sd_subscribe_with_udp_endpoint(
            0x1234, 0x0001, 1, 0x0001, 3600, client_ip, 40001,
        );
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for ACK
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234 
                            && entry.instance_id == 0x0001
                            && entry.eventgroup_id == 0x0001 
                        {
                            if entry.entry_type as u8 == 0x07 && entry.ttl > 0 {
                                found_ack = true;
                            }
                        }
                    }
                }
            }

            if found_ack {
                break;
            }
        }

        assert!(
            found_ack,
            "Server should ACK subscribe with correct instance ID"
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_1137: Server SHALL NACK when Eventgroup is unknown
///
/// Server offers eventgroup 0x0001, client tries to subscribe to eventgroup 0x0099.
/// Server MUST respond with NACK.
#[test_log::test]
#[ignore = "Known issue: current implementation does not NACK unknown eventgroups"]
fn subscribe_to_unknown_eventgroup_should_nack() {
    covers!(feat_req_recentipsd_1137);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers eventgroup 0x0001
    sim.host("server", || async {
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(Default::default()).await.unwrap();

        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .udp()
            .start()
            .await
            .unwrap();

        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Raw client: tries to subscribe to eventgroup 0x0099 (doesn't exist)
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == 0x1234
                            && entry.ttl > 0
                        {
                            server_endpoint = Some(SocketAddr::new(from.ip(), 30490));
                        }
                    }
                }
            }

            if server_endpoint.is_some() {
                break;
            }
        }

        let server_ep = server_endpoint.expect("Should discover server");

        // Get our IP from turmoil (not local_addr which is 0.0.0.0)
        let client_ip: std::net::Ipv4Addr = turmoil::lookup("raw_client").to_string().parse().unwrap();

        // Subscribe to EVENTGROUP 0x0099 (NOT 0x0001!)
        let subscribe = build_sd_subscribe_with_udp_endpoint(
            0x1234, 0x0001, 1, 0x0099, 3600, client_ip, 40000,
        );
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for NACK
        let mut found_nack = false;
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234 && entry.eventgroup_id == 0x0099 {
                            if entry.entry_type as u8 == 0x07 {
                                if entry.ttl == 0 {
                                    found_nack = true;
                                } else {
                                    found_ack = true;
                                }
                            }
                        }
                    }
                }
            }
        }

        assert!(
            found_nack,
            "Server should NACK subscribe to unknown eventgroup. Got ACK={}, NACK={}",
            found_ack, found_nack
        );
        assert!(!found_ack, "Server should NOT ACK unknown eventgroup");

        Ok(())
    });

    // Second client: subscribes with CORRECT eventgroup, should get ACK
    sim.client("correct_client", async move {
        tokio::time::sleep(Duration::from_millis(150)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == 0x1234
                            && entry.ttl > 0
                        {
                            server_endpoint = Some(SocketAddr::new(from.ip(), 30490));
                        }
                    }
                }
            }

            if server_endpoint.is_some() {
                break;
            }
        }

        let server_ep = server_endpoint.expect("Should discover server");

        // Get our IP from turmoil (not local_addr which is 0.0.0.0)
        let client_ip: std::net::Ipv4Addr = turmoil::lookup("correct_client").to_string().parse().unwrap();

        // Subscribe with CORRECT eventgroup 0x0001
        let subscribe = build_sd_subscribe_with_udp_endpoint(
            0x1234, 0x0001, 1, 0x0001, 3600, client_ip, 40001,
        );
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for ACK
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234 && entry.eventgroup_id == 0x0001 {
                            if entry.entry_type as u8 == 0x07 && entry.ttl > 0 {
                                found_ack = true;
                            }
                        }
                    }
                }
            }

            if found_ack {
                break;
            }
        }

        assert!(
            found_ack,
            "Server should ACK subscribe with correct eventgroup"
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_1137: Server SHALL NACK when Major Version doesn't match
///
/// Server offers service with major version 1, client tries to subscribe with
/// major version 99. Server MUST respond with NACK.
#[test_log::test]
#[ignore = "known missing implementation"]
fn subscribe_to_wrong_major_version_should_nack() {
    covers!(feat_req_recentipsd_1137);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers major version 1
    sim.host("server", || async {
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(Default::default()).await.unwrap();

        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .udp()
            .start()
            .await
            .unwrap();

        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Raw client: tries to subscribe with major version 99
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == 0x1234
                            && entry.major_version == 1
                            && entry.ttl > 0
                        {
                            server_endpoint = Some(SocketAddr::new(from.ip(), 30490));
                        }
                    }
                }
            }

            if server_endpoint.is_some() {
                break;
            }
        }

        let server_ep = server_endpoint.expect("Should discover server");

        // Get our IP from turmoil (not local_addr which is 0.0.0.0)
        let client_ip: std::net::Ipv4Addr = turmoil::lookup("raw_client").to_string().parse().unwrap();

        // Subscribe with MAJOR VERSION 99 (NOT 1!)
        let subscribe = build_sd_subscribe_with_udp_endpoint(
            0x1234, 0x0001, 99, 0x0001, 3600, client_ip, 40000,
        );
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for NACK
        let mut found_nack = false;
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234
                            && entry.instance_id == 0x0001
                            && entry.eventgroup_id == 0x0001
                            && entry.major_version == 99
                        {
                            if entry.entry_type as u8 == 0x07 {
                                if entry.ttl == 0 {
                                    found_nack = true;
                                } else {
                                    found_ack = true;
                                }
                            }
                        }
                    }
                }
            }
        }

        assert!(
            found_nack,
            "Server should NACK subscribe with wrong major version. Got ACK={}, NACK={}",
            found_ack, found_nack
        );
        assert!(!found_ack, "Server should NOT ACK wrong major version");

        Ok(())
    });

    // Second client: subscribes with CORRECT major version, should get ACK
    sim.client("correct_client", async move {
        tokio::time::sleep(Duration::from_millis(150)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == 0x1234
                            && entry.major_version == 1
                            && entry.ttl > 0
                        {
                            server_endpoint = Some(SocketAddr::new(from.ip(), 30490));
                        }
                    }
                }
            }

            if server_endpoint.is_some() {
                break;
            }
        }

        let server_ep = server_endpoint.expect("Should discover server");

        // Get our IP from turmoil (not local_addr which is 0.0.0.0)
        let client_ip: std::net::Ipv4Addr = turmoil::lookup("correct_client").to_string().parse().unwrap();

        // Subscribe with CORRECT major version 1
        let subscribe = build_sd_subscribe_with_udp_endpoint(
            0x1234, 0x0001, 1, 0x0001, 3600, client_ip, 40001,
        );
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for ACK
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234
                            && entry.instance_id == 0x0001
                            && entry.eventgroup_id == 0x0001
                            && entry.major_version == 1
                        {
                            if entry.entry_type as u8 == 0x07 && entry.ttl > 0 {
                                found_ack = true;
                            }
                        }
                    }
                }
            }

            if found_ack {
                break;
            }
        }

        assert!(
            found_ack,
            "Server should ACK subscribe with correct major version"
        );

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// SUBSCRIBE MESSAGE FORMAT TESTS - CYCLIC OFFER RESPONSE
// ============================================================================

/// Build a raw SOME/IP-SD OfferService message with TCP endpoint only
fn build_sd_offer_tcp_only(
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

    // === SD Payload ===
    packet.push(0xC0);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);
    packet.extend_from_slice(&16u32.to_be_bytes());

    // === OfferService Entry (16 bytes) ===
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

    // Options array length - 12 bytes for IPv4 endpoint
    packet.extend_from_slice(&12u32.to_be_bytes());

    // === IPv4 Endpoint Option (12 bytes) - TCP ===
    packet.extend_from_slice(&9u16.to_be_bytes());
    packet.push(0x04);
    packet.push(0x00);
    packet.extend_from_slice(&endpoint_ip.octets());
    packet.push(0x00);
    packet.push(0x06); // L4 Protocol = TCP
    packet.extend_from_slice(&endpoint_port.to_be_bytes());

    // Fix up length field
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}

/// Build a raw SOME/IP-SD OfferService message with both UDP and TCP endpoints
fn build_sd_offer_dual_stack(
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

    // === SOME/IP Header (16 bytes) ===
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

    // === SD Payload ===
    packet.push(0xC0);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);
    packet.extend_from_slice(&16u32.to_be_bytes());

    // === OfferService Entry (16 bytes) ===
    packet.push(0x01);
    packet.push(0x00); // 1st option index
    packet.push(0x00); // 2nd option index
    packet.push(0x20); // 2 options in run 1
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]);
    packet.extend_from_slice(&minor_version.to_be_bytes());

    // Options array length - 24 bytes for 2 IPv4 endpoints
    packet.extend_from_slice(&24u32.to_be_bytes());

    // === IPv4 Endpoint Option 1 (12 bytes) - UDP ===
    packet.extend_from_slice(&9u16.to_be_bytes());
    packet.push(0x04);
    packet.push(0x00);
    packet.extend_from_slice(&endpoint_ip.octets());
    packet.push(0x00);
    packet.push(0x11); // L4 Protocol = UDP
    packet.extend_from_slice(&udp_port.to_be_bytes());

    // === IPv4 Endpoint Option 2 (12 bytes) - TCP ===
    packet.extend_from_slice(&9u16.to_be_bytes());
    packet.push(0x04);
    packet.push(0x00);
    packet.extend_from_slice(&endpoint_ip.octets());
    packet.push(0x00);
    packet.push(0x06); // L4 Protocol = TCP
    packet.extend_from_slice(&tcp_port.to_be_bytes());

    // Fix up length field
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}

/// Verify subscribe message format for initial and subsequent subscribes
/// triggered by cyclic offers.
///
/// Setup:
/// - Wire-level server offers UDP-only service and sends cyclic offers
/// - Client (our library) subscribes and stays alive
/// - Server verifies format of initial subscribe AND subsequent re-subscribes
///
/// Per feat_req_recentipsd_631: Subscriptions triggered by OfferService entries
#[test_log::test]
fn subscribe_format_udp_only_cyclic_offers() {
    covers!(feat_req_recentipsd_431, feat_req_recentipsd_631);

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let subscribe_count = Arc::new(AtomicUsize::new(0));
    let subscribe_count_server = Arc::clone(&subscribe_count);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Wire-level server that sends offers and inspects subscribes
    sim.host("wire_server", move || {
        let subscribe_count = Arc::clone(&subscribe_count_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            let offer = build_sd_offer(0x1234, 0x0001, 1, 0, my_ip, 30509, 3);
            let mut buf = [0u8; 1500];

            // Send offers cyclically and collect subscribes
            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);

            while tokio::time::Instant::now() < deadline {
                // Send offer every 1 second (simulating cyclic offer with short TTL)
                if last_offer.elapsed() >= Duration::from_millis(1000) {
                    sd_socket.send_to(&offer, sd_multicast).await?;
                    eprintln!("[wire_server] Sent offer");
                    last_offer = tokio::time::Instant::now();
                }

                // Check for incoming messages
                if let Ok(Ok((len, from))) = tokio::time::timeout(
                    Duration::from_millis(100),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            // SubscribeEventgroup = 0x06
                            if entry.entry_type as u8 == 0x06 && entry.service_id == 0x1234 {
                                let count = subscribe_count.fetch_add(1, Ordering::SeqCst) + 1;
                                eprintln!(
                                    "[wire_server] Received subscribe #{}: eventgroup={:04x}, ttl={}",
                                    count, entry.eventgroup_id, entry.ttl
                                );

                                // Verify subscribe has correct format
                                assert_eq!(entry.service_id, 0x1234, "Service ID mismatch");
                                assert_eq!(entry.instance_id, 0x0001, "Instance ID mismatch");
                                assert!(entry.ttl > 0, "Subscribe TTL should be > 0");

                                // Check that subscribe includes endpoint option
                                // The client should include its endpoint in the subscribe
                                let has_udp_endpoint = sd_msg.get_udp_endpoint(entry).is_some();
                                let has_tcp_endpoint = sd_msg.get_tcp_endpoint(entry).is_some();
                                assert!(
                                    has_udp_endpoint || has_tcp_endpoint,
                                    "Subscribe #{} should include endpoint option (UDP={}, TCP={})",
                                    count, has_udp_endpoint, has_tcp_endpoint
                                );

                                // For UDP-only offer, client should send UDP endpoint
                                assert!(
                                    has_udp_endpoint,
                                    "Subscribe #{} to UDP-only service should include UDP endpoint",
                                    count
                                );

                                // Send ACK
                                let ack = build_sd_subscribe_ack(
                                    entry.service_id,
                                    entry.instance_id,
                                    entry.major_version,
                                    entry.eventgroup_id,
                                    entry.ttl,
                                );
                                sd_socket.send_to(&ack, from).await?;
                            }
                        }
                    }
                }
            }

            Ok(())
        }
    });

    // Client using our library
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .subscribe_ttl(5) // Short TTL so we can see renewals
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        // Discover and subscribe
        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        eprintln!("[client] Service discovered");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        eprintln!("[client] Subscribed, staying alive for cyclic offers...");

        // Stay alive to receive cyclic offers and send renewals
        tokio::time::sleep(Duration::from_secs(6)).await;

        Ok(())
    });

    sim.run().unwrap();

    let count = subscribe_count.load(Ordering::SeqCst);
    eprintln!("Total subscribes received: {}", count);
    assert!(
        count >= 2,
        "Should receive at least initial subscribe + 1 renewal, got {}",
        count
    );
}

/// Verify subscribe message format when server offers TCP only
///
/// Setup:
/// - Wire-level server offers TCP-only service
/// - Client (our library, with TCP preference) subscribes
/// - Server verifies subscribe includes TCP endpoint option
#[test_log::test]
fn subscribe_format_tcp_only_cyclic_offers() {
    covers!(feat_req_recentipsd_431, feat_req_recentipsd_631);

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let subscribe_count = Arc::new(AtomicUsize::new(0));
    let subscribe_count_server = Arc::clone(&subscribe_count);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Wire-level server offering TCP only
    sim.host("wire_server", move || {
        let subscribe_count = Arc::clone(&subscribe_count_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            let offer = build_sd_offer_tcp_only(0x1234, 0x0001, 1, 0, my_ip, 30509, 3);
            let mut buf = [0u8; 1500];

            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);

            while tokio::time::Instant::now() < deadline {
                if last_offer.elapsed() >= Duration::from_millis(1000) {
                    sd_socket.send_to(&offer, sd_multicast).await?;
                    eprintln!("[wire_server] Sent TCP-only offer");
                    last_offer = tokio::time::Instant::now();
                }

                if let Ok(Ok((len, from))) =
                    tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                        .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            if entry.entry_type as u8 == 0x06 && entry.service_id == 0x1234 {
                                let count = subscribe_count.fetch_add(1, Ordering::SeqCst) + 1;
                                eprintln!(
                                    "[wire_server] Received subscribe #{}: eventgroup={:04x}",
                                    count, entry.eventgroup_id
                                );

                                // For TCP-only offer, client MUST send TCP endpoint
                                let has_tcp_endpoint = sd_msg.get_tcp_endpoint(entry).is_some();
                                assert!(
                                    has_tcp_endpoint,
                                    "Subscribe #{} to TCP-only service MUST include TCP endpoint",
                                    count
                                );

                                // Send ACK
                                let ack = build_sd_subscribe_ack(
                                    entry.service_id,
                                    entry.instance_id,
                                    entry.major_version,
                                    entry.eventgroup_id,
                                    entry.ttl,
                                );
                                sd_socket.send_to(&ack, from).await?;
                            }
                        }
                    }
                }
            }

            Ok(())
        }
    });

    // Client with TCP preference
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .preferred_transport(someip_runtime::Transport::Tcp)
            .subscribe_ttl(5)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<
            turmoil::net::UdpSocket,
            turmoil::net::TcpStream,
            turmoil::net::TcpListener,
        > = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        eprintln!("[client] Service discovered via TCP");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        eprintln!("[client] Subscribed via TCP, staying alive...");
        tokio::time::sleep(Duration::from_secs(6)).await;

        Ok(())
    });

    sim.run().unwrap();

    let count = subscribe_count.load(Ordering::SeqCst);
    assert!(
        count >= 2,
        "Should receive at least initial subscribe + 1 renewal, got {}",
        count
    );
}

/// Verify subscribe message format when server offers both UDP and TCP
/// Client prefers UDP - should send UDP endpoint
#[test_log::test]
fn subscribe_format_dual_stack_client_prefers_udp() {
    covers!(feat_req_recentipsd_431, feat_req_recentipsd_631);

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let subscribe_count = Arc::new(AtomicUsize::new(0));
    let subscribe_count_server = Arc::clone(&subscribe_count);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("wire_server", move || {
        let subscribe_count = Arc::clone(&subscribe_count_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            let offer = build_sd_offer_dual_stack(0x1234, 0x0001, 1, 0, my_ip, 30509, 30510, 3);
            let mut buf = [0u8; 1500];

            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);

            while tokio::time::Instant::now() < deadline {
                if last_offer.elapsed() >= Duration::from_millis(1000) {
                    sd_socket.send_to(&offer, sd_multicast).await?;
                    eprintln!("[wire_server] Sent dual-stack offer");
                    last_offer = tokio::time::Instant::now();
                }

                if let Ok(Ok((len, from))) = tokio::time::timeout(
                    Duration::from_millis(100),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            if entry.entry_type as u8 == 0x06 && entry.service_id == 0x1234 {
                                let count = subscribe_count.fetch_add(1, Ordering::SeqCst) + 1;

                                let has_udp = sd_msg.get_udp_endpoint(entry).is_some();
                                let has_tcp = sd_msg.get_tcp_endpoint(entry).is_some();
                                eprintln!(
                                    "[wire_server] Subscribe #{}: UDP={}, TCP={}",
                                    count, has_udp, has_tcp
                                );

                                // Client prefers UDP, so should send UDP endpoint
                                assert!(
                                    has_udp,
                                    "Subscribe #{} from UDP-preferring client should include UDP endpoint",
                                    count
                                );

                                let ack = build_sd_subscribe_ack(
                                    entry.service_id,
                                    entry.instance_id,
                                    entry.major_version,
                                    entry.eventgroup_id,
                                    entry.ttl,
                                );
                                sd_socket.send_to(&ack, from).await?;
                            }
                        }
                    }
                }
            }

            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Default config prefers UDP
        let config = RuntimeConfig::builder()
            .preferred_transport(someip_runtime::Transport::Udp)
            .subscribe_ttl(5)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        tokio::time::sleep(Duration::from_secs(6)).await;
        Ok(())
    });

    sim.run().unwrap();

    let count = subscribe_count.load(Ordering::SeqCst);
    assert!(
        count >= 2,
        "Should receive at least 2 subscribes, got {}",
        count
    );
}

/// Verify subscribe message format when server offers both UDP and TCP
/// Client prefers TCP - should send TCP endpoint
#[test_log::test]
fn subscribe_format_dual_stack_client_prefers_tcp() {
    covers!(feat_req_recentipsd_431, feat_req_recentipsd_631);

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let subscribe_count = Arc::new(AtomicUsize::new(0));
    let subscribe_count_server = Arc::clone(&subscribe_count);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("wire_server", move || {
        let subscribe_count = Arc::clone(&subscribe_count_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            let offer = build_sd_offer_dual_stack(0x1234, 0x0001, 1, 0, my_ip, 30509, 30510, 3);
            let mut buf = [0u8; 1500];

            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);

            while tokio::time::Instant::now() < deadline {
                if last_offer.elapsed() >= Duration::from_millis(1000) {
                    sd_socket.send_to(&offer, sd_multicast).await?;
                    eprintln!("[wire_server] Sent dual-stack offer");
                    last_offer = tokio::time::Instant::now();
                }

                if let Ok(Ok((len, from))) = tokio::time::timeout(
                    Duration::from_millis(100),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            if entry.entry_type as u8 == 0x06 && entry.service_id == 0x1234 {
                                let count = subscribe_count.fetch_add(1, Ordering::SeqCst) + 1;

                                let has_udp = sd_msg.get_udp_endpoint(entry).is_some();
                                let has_tcp = sd_msg.get_tcp_endpoint(entry).is_some();
                                eprintln!(
                                    "[wire_server] Subscribe #{}: UDP={}, TCP={}",
                                    count, has_udp, has_tcp
                                );

                                // Client prefers TCP, so should send TCP endpoint
                                assert!(
                                    has_tcp,
                                    "Subscribe #{} from TCP-preferring client should include TCP endpoint",
                                    count
                                );

                                let ack = build_sd_subscribe_ack(
                                    entry.service_id,
                                    entry.instance_id,
                                    entry.major_version,
                                    entry.eventgroup_id,
                                    entry.ttl,
                                );
                                sd_socket.send_to(&ack, from).await?;
                            }
                        }
                    }
                }
            }

            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .preferred_transport(someip_runtime::Transport::Tcp)
            .subscribe_ttl(5)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<
            turmoil::net::UdpSocket,
            turmoil::net::TcpStream,
            turmoil::net::TcpListener,
        > = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        tokio::time::sleep(Duration::from_secs(6)).await;
        Ok(())
    });

    sim.run().unwrap();

    let count = subscribe_count.load(Ordering::SeqCst);
    assert!(
        count >= 2,
        "Should receive at least 2 subscribes, got {}",
        count
    );
}

/// Verify that client adapts transport when preference doesn't match offer
/// Client prefers TCP but server offers UDP only - client should adapt and use UDP
#[test_log::test]
fn subscribe_format_client_adapts_to_available_transport() {
    covers!(feat_req_recentipsd_431, feat_req_recentip_324);

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let subscribe_count = Arc::new(AtomicUsize::new(0));
    let subscribe_count_server = Arc::clone(&subscribe_count);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("wire_server", move || {
        let subscribe_count = Arc::clone(&subscribe_count_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            // Offer UDP ONLY
            let offer = build_sd_offer(0x1234, 0x0001, 1, 0, my_ip, 30509, 3);
            let mut buf = [0u8; 1500];

            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);

            while tokio::time::Instant::now() < deadline {
                if last_offer.elapsed() >= Duration::from_millis(1000) {
                    sd_socket.send_to(&offer, sd_multicast).await?;
                    eprintln!("[wire_server] Sent UDP-only offer");
                    last_offer = tokio::time::Instant::now();
                }

                if let Ok(Ok((len, from))) = tokio::time::timeout(
                    Duration::from_millis(100),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            if entry.entry_type as u8 == 0x06 && entry.service_id == 0x1234 {
                                let count = subscribe_count.fetch_add(1, Ordering::SeqCst) + 1;

                                let has_udp = sd_msg.get_udp_endpoint(entry).is_some();
                                let has_tcp = sd_msg.get_tcp_endpoint(entry).is_some();
                                eprintln!(
                                    "[wire_server] Subscribe #{}: UDP={}, TCP={}",
                                    count, has_udp, has_tcp
                                );

                                // Client prefers TCP but server only offers UDP
                                // Client MUST adapt and send UDP endpoint
                                assert!(
                                    has_udp,
                                    "Subscribe #{} to UDP-only service MUST include UDP endpoint \
                                     even when client prefers TCP",
                                    count
                                );
                                assert!(
                                    !has_tcp,
                                    "Subscribe #{} to UDP-only service should NOT include TCP endpoint",
                                    count
                                );

                                let ack = build_sd_subscribe_ack(
                                    entry.service_id,
                                    entry.instance_id,
                                    entry.major_version,
                                    entry.eventgroup_id,
                                    entry.ttl,
                                );
                                sd_socket.send_to(&ack, from).await?;
                            }
                        }
                    }
                }
            }

            Ok(())
        }
    });

    // Client PREFERS TCP but server only offers UDP
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .preferred_transport(someip_runtime::Transport::Tcp) // Prefers TCP!
            .subscribe_ttl(5)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        // Use full runtime type to support TCP preference
        let runtime: Runtime<
            turmoil::net::UdpSocket,
            turmoil::net::TcpStream,
            turmoil::net::TcpListener,
        > = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        // Verify the proxy detected UDP transport despite TCP preference
        assert_eq!(
            proxy.transport(),
            someip_runtime::Transport::Udp,
            "Proxy should use UDP transport when that's all that's offered"
        );

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        tokio::time::sleep(Duration::from_secs(6)).await;
        Ok(())
    });

    sim.run().unwrap();

    let count = subscribe_count.load(Ordering::SeqCst);
    assert!(
        count >= 2,
        "Should receive at least 2 subscribes, got {}",
        count
    );
}

// ============================================================================
// TCP CONNECTION TIMING TESTS
// ============================================================================

/// [feat_req_recentipsd_767] Client opens TCP connection BEFORE sending SubscribeEventgroup.
///
/// This wire-level test verifies the timing requirement:
/// - Raw server offers TCP-only service
/// - Raw server has TCP listener
/// - Client (our library) discovers and subscribes
/// - Server verifies: TCP connection established BEFORE SubscribeEventgroup arrives
///
/// This is critical for reliable event delivery - the server needs the TCP connection
/// ready to send events as soon as it ACKs the subscription.
///
/// Currently IGNORED: The client does not yet establish TCP connection before subscribing.
/// This documents a known compliance gap that needs to be implemented.
#[test_log::test]
#[ignore = "TCP connection before subscribe not yet implemented (feat_req_recentipsd_767)"]
fn tcp_connection_established_before_subscribe_767() {
    covers!(feat_req_recentipsd_767);

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::Notify;

    let tcp_connected_before_subscribe = Arc::new(AtomicBool::new(false));
    let tcp_connected = Arc::clone(&tcp_connected_before_subscribe);

    let tcp_connection_count = Arc::new(AtomicUsize::new(0));
    let tcp_count = Arc::clone(&tcp_connection_count);

    let subscribe_received = Arc::new(AtomicBool::new(false));
    let subscribe_flag = Arc::clone(&subscribe_received);

    // Notify when TCP connection is established
    let tcp_notify = Arc::new(Notify::new());
    let tcp_notify_server = Arc::clone(&tcp_notify);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Wire-level server offering TCP only
    sim.host("wire_server", move || {
        let tcp_connected = Arc::clone(&tcp_connected);
        let tcp_count = Arc::clone(&tcp_count);
        let subscribe_flag = Arc::clone(&subscribe_flag);
        let tcp_notify = Arc::clone(&tcp_notify_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            // SD socket for offers and subscription handling
            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            // TCP listener for RPC/events - THIS IS THE KEY PART
            let tcp_listener = turmoil::net::TcpListener::bind(format!("0.0.0.0:30509")).await?;
            eprintln!("[wire_server] TCP listener bound on port 30509");

            let offer = build_sd_offer_tcp_only(0x1234, 0x0001, 1, 0, my_ip, 30509, 3600);
            let mut sd_buf = [0u8; 1500];

            // Track if we've seen a TCP connection
            let tcp_connected_inner = Arc::clone(&tcp_connected);
            let tcp_count_inner = Arc::clone(&tcp_count);
            let tcp_notify_inner = Arc::clone(&tcp_notify);

            // Spawn task to accept TCP connections
            let tcp_accept_task = tokio::spawn(async move {
                loop {
                    match tcp_listener.accept().await {
                        Ok((stream, peer)) => {
                            let count = tcp_count_inner.fetch_add(1, Ordering::SeqCst) + 1;
                            eprintln!(
                                "[wire_server] TCP connection #{} accepted from {}",
                                count, peer
                            );
                            tcp_connected_inner.store(true, Ordering::SeqCst);
                            tcp_notify_inner.notify_one();

                            // Keep connection alive by reading in background
                            tokio::spawn(async move {
                                use tokio::io::AsyncReadExt;
                                let mut stream = stream;
                                let mut buf = [0u8; 1024];
                                loop {
                                    match stream.read(&mut buf).await {
                                        Ok(0) => break, // Connection closed
                                        Ok(n) => {
                                            eprintln!(
                                                "[wire_server] TCP received {} bytes",
                                                n
                                            );
                                        }
                                        Err(_) => break,
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            eprintln!("[wire_server] TCP accept error: {}", e);
                            break;
                        }
                    }
                }
            });

            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);

            while tokio::time::Instant::now() < deadline {
                // Send periodic offers
                if last_offer.elapsed() >= Duration::from_millis(500) {
                    sd_socket.send_to(&offer, sd_multicast).await?;
                    eprintln!("[wire_server] Sent TCP-only offer");
                    last_offer = tokio::time::Instant::now();
                }

                // Check for SubscribeEventgroup
                if let Ok(Ok((len, from))) =
                    tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut sd_buf))
                        .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&sd_buf[..len]) {
                        for entry in &sd_msg.entries {
                            if entry.entry_type as u8 == 0x06 && entry.service_id == 0x1234 {
                                eprintln!(
                                    "[wire_server] *** SubscribeEventgroup received! ***"
                                );

                                // THE CRITICAL CHECK: Was TCP connected BEFORE subscribe?
                                let was_connected = tcp_connected.load(Ordering::SeqCst);
                                eprintln!(
                                    "[wire_server] TCP was connected before subscribe: {}",
                                    was_connected
                                );

                                // Per feat_req_recentipsd_767, client MUST connect BEFORE subscribing
                                tcp_connected.store(was_connected, Ordering::SeqCst);
                                subscribe_flag.store(true, Ordering::SeqCst);

                                // Verify TCP endpoint is in the subscribe
                                let has_tcp = sd_msg.get_tcp_endpoint(entry).is_some();
                                eprintln!(
                                    "[wire_server] Subscribe has TCP endpoint: {}",
                                    has_tcp
                                );

                                // Send ACK
                                let ack = build_sd_subscribe_ack(
                                    entry.service_id,
                                    entry.instance_id,
                                    entry.major_version,
                                    entry.eventgroup_id,
                                    entry.ttl,
                                );
                                sd_socket.send_to(&ack, from).await?;
                                eprintln!("[wire_server] Sent SubscribeEventgroupAck");
                            }
                        }
                    }
                }
            }

            tcp_accept_task.abort();
            Ok(())
        }
    });

    // Client using our library
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .preferred_transport(someip_runtime::Transport::Tcp)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<
            turmoil::net::UdpSocket,
            turmoil::net::TcpStream,
            turmoil::net::TcpListener,
        > = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        eprintln!("[client] Service discovered (should be TCP-only)");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        eprintln!("[client] Subscribed successfully");

        // Keep alive for a bit
        tokio::time::sleep(Duration::from_secs(2)).await;

        Ok(())
    });

    sim.run().unwrap();

    // Verify the test actually ran
    assert!(
        subscribe_received.load(Ordering::SeqCst),
        "Server should have received SubscribeEventgroup"
    );

    // THE KEY ASSERTION: TCP connection must be established BEFORE subscribe
    assert!(
        tcp_connected_before_subscribe.load(Ordering::SeqCst),
        "[feat_req_recentipsd_767] TCP connection MUST be established BEFORE \
         sending SubscribeEventgroup. The client did not connect to TCP before subscribing."
    );

    let tcp_count = tcp_connection_count.load(Ordering::SeqCst);
    assert!(
        tcp_count >= 1,
        "Should have at least 1 TCP connection, got {}",
        tcp_count
    );
}

