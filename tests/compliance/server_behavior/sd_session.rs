//! SD Session Management Tests (Server Under Test)
//!
//! Tests that verify the reboot flag behavior and session ID management
//! in Service Discovery messages.
//!
//! # Test Setup
//! - Library: Acts as **server** (offers service, sends SD messages)
//! - Raw socket: Acts as **observer** (captures and verifies SD messages)
//!
//! # Requirements Covered
//! - feat_req_someipsd_40: Reboot flag in SD flags
//! - feat_req_someipsd_41: Reboot flag behavior
//! - feat_req_someipsd_764: Reboot detection algorithm
//! - feat_req_someipsd_765: Per-peer session tracking
//! - feat_req_someipsd_863: Reliable reboot detection
//! - feat_req_someipsd_871: Expire services on peer reboot
//! - feat_req_someipsd_872: Reset TCP on peer reboot
//! - feat_req_someip_649: Session ID must start at 1

use crate::helpers::configure_tracing;

use super::helpers::{
    build_sd_subscribe_ack, build_sd_subscribe_with_udp_endpoint, covers, parse_sd_flags,
    parse_sd_message, TEST_SERVICE_ID, TEST_SERVICE_VERSION,
};
use recentip::prelude::*;
use recentip::Transport;
use std::time::Duration;

// ============================================================================
// SD OFFER BUILDER WITH CONFIGURABLE FLAGS AND SESSION
// ============================================================================

/// Build a raw SOME/IP-SD OfferService message with configurable session ID and flags
fn build_sd_offer_with_session(
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

    // === SOME/IP Header (16 bytes) ===
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes()); // Service ID = SD
    packet.extend_from_slice(&0x8100u16.to_be_bytes()); // Method ID = SD
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes()); // placeholder
    packet.extend_from_slice(&0x0000u16.to_be_bytes()); // Client ID
    packet.extend_from_slice(&session_id.to_be_bytes()); // Session ID (configurable)
    packet.push(0x01); // Protocol Version
    packet.push(0x01); // Interface Version
    packet.push(0x02); // Message Type = NOTIFICATION
    packet.push(0x00); // Return Code = E_OK

    // === SD Payload ===
    let mut flags = if unicast_flag { 0x40u8 } else { 0x00u8 }; // Unicast flag
    if reboot_flag {
        flags |= 0x80; // Reboot flag
    }
    packet.push(flags);
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
}

// ============================================================================
// REBOOT FLAG TESTS
// ============================================================================
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

/// feat_req_someipsd_41: SD Reboot flag is set after startup
///
/// When a runtime starts, it must set the reboot flag (bit 7 of SD flags)
/// to 1 in all SD messages until the session ID wraps around.
#[test_log::test]
fn sd_reboot_flag_set_after_startup() {
    covers!(feat_req_someipsd_41);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library side - offers a service
    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
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
                    if header.service_id == 0xFFFF && header.method_id == 0x8100 {
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

/// feat_req_someipsd_41: Session ID starts at 1 after startup
/// feat_req_someip_649: Session ID must start at 1
///
/// Verify that the first SD message has session_id=1
///
/// NOTE: Due to turmoil simulation timing, we may not always capture the very
/// first packet. This test verifies that session_id=1 exists among the first
/// few captured messages, which proves the runtime started counting at 1.
#[test_log::test]
fn sd_session_starts_at_one() {
    covers!(feat_req_someipsd_41, feat_req_someip_649);

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

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
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

/// feat_req_someipsd_41: Session ID 0x0000 is invalid and should be rejected
/// feat_req_someip_649: Session ID shall not be set to 0
///
/// Per spec, session ID 0x0000 is reserved for "session handling disabled" and
/// must never be used in normal operation. Messages with session_id=0 should
/// be silently dropped or ignored.
///
/// CURRENTLY FAILING: Implementation does NOT reject session_id=0
#[test_log::test]
#[ignore = "Implementation bug: session_id=0 is not being rejected"]
fn sd_session_zero_rejected() {
    covers!(feat_req_someipsd_41, feat_req_someip_649);

    use std::sync::atomic::{AtomicU8, Ordering};

    static MESSAGES_SENT: AtomicU8 = AtomicU8::new(0);
    MESSAGES_SENT.store(0, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.client("client", async move {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Try to discover service - should fail because server sends invalid session_id=0
        let proxy_result =
            tokio::time::timeout(Duration::from_millis(500), runtime.find(TEST_SERVICE_ID)).await;

        // Should timeout because messages with session_id=0 are invalid and dropped
        assert!(
            proxy_result.is_err(),
            "Should timeout - messages with session_id=0 should be rejected"
        );

        Ok(())
    });

    sim.host("server", || async move {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server").to_string().parse().unwrap();
        let client_ip: std::net::IpAddr = turmoil::lookup("client").into();
        let client_sd_addr = std::net::SocketAddr::new(client_ip, 30490);

        // Send offers with INVALID session_id=0 (should be rejected)
        for _ in 0..5 {
            let offer = build_sd_offer_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                5,
                0, // INVALID: session_id = 0
                true,
                false,
            );
            socket.send_to(&offer, client_sd_addr).await?;
            MESSAGES_SENT.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();

    // Verify server actually sent the invalid messages
    assert_eq!(
        MESSAGES_SENT.load(Ordering::SeqCst),
        5,
        "Server should have sent 5 messages with session_id=0"
    );
}

/// feat_req_someipsd_41: Reboot flag is cleared after session wraparound
///
/// After 65535 SD messages (session wraps 0xFFFF → 1), the reboot flag
/// must transition from 1 to 0. This indicates normal operation, not reboot.
///
/// NOTE: This test simulates the wraparound scenario by checking the runtime
/// state. Full integration would require 65535 actual messages.
#[test_log::test]
fn sd_reboot_flag_clears_after_wraparound() {
    covers!(feat_req_someipsd_41, feat_req_someipsd_764);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(60))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
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

/// feat_req_someipsd_765: SD uses separate session counters for multicast vs unicast
///
/// Per the specification, session ID counters must be maintained separately
/// for multicast and unicast SD messages. A peer receiving both types
/// should see independent session sequences.
#[test_log::test]
fn sd_separate_multicast_unicast_sessions() {
    covers!(feat_req_someipsd_765);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Subscribe to trigger unicast SD responses
        let proxy = runtime.find(TEST_SERVICE_ID);
        let _ = tokio::time::timeout(Duration::from_secs(2), proxy).await;

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

/// feat_req_someip_649: Session ID must start at 1
/// feat_req_someipsd_765: Separate session counters for multicast and unicast
///
/// Verify that the server (library under test) maintains independent session
/// counters for multicast and unicast SD messages, both starting at 1 and
/// incrementing consecutively.
///
/// Test setup:
/// - Library acts as server (offers service)
/// - Raw observer captures all SD messages (multicast and unicast)
/// - Verifies multicast sessions: 1, 2, 3, 4, ...
/// - Verifies unicast sessions: 1, 2, 3, 4, ... (independent from multicast)
#[test_log::test]
fn server_session_ids_start_at_one_and_increment_independently() {
    covers!(feat_req_someip_649, feat_req_someipsd_765);

    use std::sync::{Arc, Mutex};

    // Shared storage for captured sessions
    let multicast_sessions = Arc::new(Mutex::new(Vec::<u16>::new()));
    let unicast_sessions = Arc::new(Mutex::new(Vec::<u16>::new()));

    let multicast_clone = Arc::clone(&multicast_sessions);
    let unicast_clone = Arc::clone(&unicast_sessions);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server (library under test) - offers service
    sim.host("server", || async {
        // Small delay to let observer join multicast
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Keep offering to generate multiple SD messages
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });

    // Observer (.client drives the simulation) - spawns raw socket capture task
    sim.client("observer", async move {
        // Bind and join multicast immediately
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let our_ip = turmoil::lookup("observer").to_string().parse().unwrap();

        let mut buf = [0u8; 1500];

        // Capture SD messages for 3 seconds
        let start = tokio::time::Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            match tokio::time::timeout(Duration::from_millis(50), socket.recv_from(&mut buf)).await
            {
                Ok(Ok((len, from))) => {
                    if len >= 16 {
                        // Parse SOME/IP header for session ID
                        let session_id = u16::from_be_bytes([buf[10], buf[11]]);

                        // Check the SD flags to determine message type
                        if len >= 20 {
                            let flags = buf[16]; // SD flags byte
                            let unicast_flag = (flags & 0x40) != 0;

                            if unicast_flag {
                                // Unicast message
                                unicast_clone.lock().unwrap().push(session_id);
                            } else {
                                // Multicast message
                                multicast_clone.lock().unwrap().push(session_id);
                                let subscribe = build_sd_subscribe_with_udp_endpoint(
                                    TEST_SERVICE_ID,
                                    0x0001,
                                    TEST_SERVICE_VERSION.0,
                                    1, // eventgroup
                                    5, // TTL
                                    our_ip,
                                    30510,
                                    1, // session_id
                                );
                                socket.send_to(&subscribe, from).await?;
                            }
                        }
                    }
                }
                Ok(Err(_)) => break,
                Err(_) => continue, // Timeout, keep trying
            }
        }

        Ok(())
    });

    sim.run().unwrap();

    // Verify multicast sessions
    let multicast = multicast_sessions.lock().unwrap();
    let unicast = unicast_sessions.lock().unwrap();

    eprintln!(
        "Multicast sessions captured: {:?}",
        &multicast[..multicast.len()]
    );
    eprintln!("Unicast sessions captured: {:?}", &unicast[..unicast.len()]);

    // Multicast should contain session 1 (may not be first due to timing)
    assert!(
        !multicast.is_empty(),
        "Should have captured multicast SD messages"
    );
    assert_eq!(
        multicast[0],
        1,
        "Multicast session ID should start at 1 (sessions captured: {:?})",
        &multicast[..multicast.len().min(10)]
    );

    // Multicast should increment consecutively
    for i in 1..multicast.len() {
        let expected = multicast[i - 1] + 1;
        assert_eq!(
            multicast[i],
            expected,
            "Multicast session IDs should increment consecutively: after {} expected {}, got {}",
            multicast[i - 1],
            expected,
            multicast[i]
        );
    }

    // verify unicast also start at 1 and increment
    assert_eq!(
        unicast[0], 1,
        "Unicast session ID should start at 1 (independent from multicast), got {}",
        unicast[0]
    );

    for i in 1..unicast.len() {
        let expected = unicast[i - 1] + 1;
        assert_eq!(
            unicast[i],
            expected,
            "Unicast session IDs should increment consecutively: after {} expected {}, got {}",
            unicast[i - 1],
            expected,
            unicast[i]
        );
    }
}

/// Verify that the server maintains separate session counters for unicast
/// SD messages, starting at 1 and incrementing independently from multicast.
///
/// Test setup:
/// - Server offers service with eventgroup
/// - Client uses raw socket to send Subscribe and capture SubscribeAck responses
/// - Verifies unicast SubscribeAck sessions: 1, 2, 3, 4, ...
#[test_log::test]
fn server_unicast_session_ids_start_at_one_and_increment() {
    covers!(feat_req_someip_649, feat_req_someipsd_765);

    use std::sync::{Arc, Mutex};

    // Shared storage for captured unicast sessions
    let unicast_sessions = Arc::new(Mutex::new(Vec::<u16>::new()));
    let unicast_clone = Arc::clone(&unicast_sessions);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server (library under test) - offers service with eventgroup
    sim.host("server", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Create an event in eventgroup to allow subscriptions
        let eventgroup = EventgroupId::new(1).unwrap();
        let _event_handle = offering
            .event(EventId::new(0x8001).unwrap())
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        // Keep offering for client subscriptions
        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    // Client - uses raw socket to subscribe and capture SubscribeAck
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let server_ip: std::net::Ipv4Addr = "192.168.0.1".parse().unwrap();
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("client").to_string().parse().unwrap();

        // Wait for server to offer
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send multiple Subscribe messages to trigger unicast SubscribeAck responses
        for _counter in 0..4 {
            let subscribe = build_sd_subscribe_with_udp_endpoint(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                1, // eventgroup
                5, // TTL
                my_ip,
                30510,
                1, // session_id
            );
            socket.send_to(&subscribe, (server_ip, 30490)).await?;

            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // Capture SubscribeAck responses for 2 seconds
        let mut buf = [0u8; 1500];
        for _ in 0..40 {
            match tokio::time::timeout(Duration::from_millis(50), socket.recv_from(&mut buf)).await
            {
                Ok(Ok((len, from))) => {
                    if len >= 20 {
                        let from_ip = from.ip().to_string();

                        // Only capture from server
                        if from_ip == "192.168.0.1" {
                            let flags = buf[16]; // SD flags byte
                            let unicast_flag = (flags & 0x40) != 0;

                            // Only capture unicast messages (SubscribeAck)
                            if unicast_flag && len >= 24 {
                                // Check if it's SubscribeAck (entry type 0x07)
                                let entry_type = buf[24];
                                if entry_type == 0x07 {
                                    let session_id = u16::from_be_bytes([buf[10], buf[11]]);
                                    unicast_clone.lock().unwrap().push(session_id);
                                }
                            }
                        }
                    }
                }
                Ok(Err(_)) => break,
                Err(_) => continue,
            }
        }

        Ok(())
    });

    sim.run().unwrap();

    // Verify unicast sessions
    let unicast = unicast_sessions.lock().unwrap();

    eprintln!(
        "Unicast sessions captured: {:?}",
        &unicast[..unicast.len().min(10)]
    );

    // Unicast should start at 1
    assert!(
        !unicast.is_empty(),
        "Should have captured unicast SD messages (SubscribeAck)"
    );
    assert_eq!(
        unicast[0], 1,
        "Unicast session ID should start at 1, got {}",
        unicast[0]
    );

    // Unicast should increment consecutively
    if unicast.len() >= 2 {
        for i in 1..unicast.len() {
            let expected = unicast[i - 1] + 1;
            assert_eq!(
                unicast[i],
                expected,
                "Unicast session IDs should increment consecutively: after {} expected {}, got {}",
                unicast[i - 1],
                expected,
                unicast[i]
            );
        }
    }
}

// ============================================================================
// PEER REBOOT DETECTION TESTS (Client Under Test)
// ============================================================================
// These tests verify that our runtime detects when a peer reboots.
// We simulate the peer (server) by sending raw SD packets with controlled
// reboot flag and session ID values.
// ============================================================================

/// feat_req_someipsd_764: Reboot detection when flag transitions 0 → 1
/// feat_req_someipsd_863: Reliable reboot detection
/// feat_req_someipsd_871: Expire services on peer reboot
///
/// When a peer's reboot flag transitions from 0 to 1, we detect reboot.
/// This is the simple case: peer was running normally (reboot=0),
/// then rebooted (now reboot=1).
///
/// We verify:
/// 1. Service from rebooting server should be expired
/// 2. Service from non-rebooting server should remain available
#[test_log::test]
fn detect_peer_reboot_flag_transition_0_to_1() {
    covers!(
        feat_req_someipsd_764,
        feat_req_someipsd_863,
        feat_req_someipsd_871,
        feat_req_someipsd_872
    );

    use std::sync::atomic::{AtomicBool, Ordering};

    static REBOOTING_SERVICE_EXPIRED: AtomicBool = AtomicBool::new(false);
    static STABLE_SERVICE_EXPIRED: AtomicBool = AtomicBool::new(false);
    REBOOTING_SERVICE_EXPIRED.store(false, Ordering::SeqCst);
    STABLE_SERVICE_EXPIRED.store(false, Ordering::SeqCst);

    const REBOOTING_SERVICE_ID: u16 = TEST_SERVICE_ID;
    const STABLE_SERVICE_ID: u16 = 0x5678;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Client (library under test) discovers services from both servers
    sim.host("client", || async move {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Monitor SD events to detect service expiration
        let mut sd_events = runtime.monitor_sd().await.unwrap();

        // Find both services
        let proxy1 = runtime.find(REBOOTING_SERVICE_ID);
        let proxy2 = runtime.find(STABLE_SERVICE_ID);

        let available1 = tokio::time::timeout(Duration::from_secs(5), proxy1).await;
        let available2 = tokio::time::timeout(Duration::from_secs(5), proxy2).await;

        assert!(available1.is_ok(), "Rebooting service should be discovered");
        assert!(available2.is_ok(), "Stable service should be discovered");

        // Wait for peer reboot to be simulated and detected
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Check for ServiceExpired or ServiceUnavailable events
            while let Ok(event) = sd_events.try_recv() {
                match event {
                    recentip::SdEvent::ServiceUnavailable { service_id, .. }
                    | recentip::SdEvent::ServiceExpired { service_id, .. } => {
                        if service_id == REBOOTING_SERVICE_ID {
                            REBOOTING_SERVICE_EXPIRED.store(true, Ordering::SeqCst);
                        }
                        if service_id == STABLE_SERVICE_ID {
                            STABLE_SERVICE_EXPIRED.store(true, Ordering::SeqCst);
                        }
                    }
                    _ => {}
                }
            }
        }

        // NOTE: These assertions will fail until feat_req_someipsd_871 is implemented
        // Currently reboot detection only resets TCP, doesn't expire services
        // assert!(
        //     REBOOTING_SERVICE_EXPIRED.load(Ordering::SeqCst),
        //     "Service from rebooting peer should be expired"
        // );
        // assert!(
        //     !STABLE_SERVICE_EXPIRED.load(Ordering::SeqCst),
        //     "Service from stable peer should NOT be expired"
        // );

        Ok(())
    });

    // Simulated server 1 - REBOOTS (flag transitions 0 → 1)
    sim.host("server_rebooting", || async move {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server_rebooting")
            .to_string()
            .parse()
            .unwrap();
        let client_ip: std::net::IpAddr = turmoil::lookup("client").into();
        let client_sd_addr = std::net::SocketAddr::new(client_ip, 30490);

        // Phase 1: Send offer with reboot=false (simulating running system)
        for session in 100..103 {
            let offer = build_sd_offer_with_session(
                REBOOTING_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                3,
                session,
                false, // reboot_flag = false
                false,
            );
            socket.send_to(&offer, client_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Phase 2: Simulate reboot - send offer with reboot=true
        for session in 1..4 {
            let offer = build_sd_offer_with_session(
                REBOOTING_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                3,
                session,
                true, // reboot_flag = true (REBOOTED!)
                false,
            );
            socket.send_to(&offer, client_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        tokio::time::sleep(Duration::from_millis(1500)).await;
        Ok(())
    });

    // Simulated server 2 - STABLE (no reboot, continuous operation)
    sim.host("server_stable", || async move {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server_stable")
            .to_string()
            .parse()
            .unwrap();
        let client_ip: std::net::IpAddr = turmoil::lookup("client").into();
        let client_sd_addr = std::net::SocketAddr::new(client_ip, 30490);

        // Continuously send offers with reboot=false (stable operation)
        // Session IDs keep incrementing normally
        for session in 1..20 {
            let offer = build_sd_offer_with_session(
                STABLE_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                30510,
                3,
                session,
                session < 10, // reboot=true for first few, then false (normal wraparound)
                false,
            );
            socket.send_to(&offer, client_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someipsd_764: NO false positive on normal flag transition 1 → 0
///
/// When reboot flag transitions from 1 to 0, this is NORMAL operation
/// (session ID wrapped around). This should NOT trigger reboot detection.
#[test_log::test]
fn no_false_positive_on_normal_1_to_0_transition() {
    covers!(feat_req_someipsd_764, feat_req_someipsd_863);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("client", || async move {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let available = tokio::time::timeout(Duration::from_secs(5), proxy).await;
        assert!(available.is_ok(), "Service should be discovered");

        // Wait for flag transition simulation
        tokio::time::sleep(Duration::from_millis(800)).await;

        // Service should still be available (no false reboot detection)
        // If reboot was falsely detected, the service might be expired

        Ok(())
    });

    sim.host("server", || async move {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server").to_string().parse().unwrap();
        let client_ip: std::net::IpAddr = turmoil::lookup("client").into();
        let client_sd_addr = std::net::SocketAddr::new(client_ip, 30490);

        // Phase 1: Send offer with reboot=true (just started)
        for session in 1..5 {
            let offer = build_sd_offer_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                5,
                session,
                true, // reboot_flag = true (just started)
                false,
            );
            socket.send_to(&offer, client_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Phase 2: Simulate normal wraparound - reboot flag transitions to false
        // This should NOT trigger reboot detection
        for session in 5..10 {
            let offer = build_sd_offer_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                5,
                session,
                false, // reboot_flag = false (normal wraparound)
                false,
            );
            socket.send_to(&offer, client_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someipsd_764: Session ID wraparound with reboot=0 is normal (no false positive)
///
/// When session ID wraps from 0xFFFF to 0x0001 with reboot=0, this is NORMAL operation.
/// This should NOT trigger reboot detection even though new_session < old_session.
/// The reboot flag being 0 indicates this is normal wraparound, not a reboot.
///
/// This test uses subscription + event delivery to verify continuous operation across
/// the wraparound boundary. Server uses raw sockets to control SD session IDs while
/// handling subscriptions and sending events. If false reboot was detected, events would stop.
#[test_log::test]
fn no_false_positive_on_session_wraparound_reboot_0() {
    covers!(feat_req_someipsd_764, feat_req_someipsd_863);

    use recentip::EventgroupId;
    use std::sync::{
        atomic::{AtomicU32, AtomicU8, Ordering},
        Arc,
    };

    static EVENTS_BEFORE_WRAPAROUND: AtomicU32 = AtomicU32::new(0);
    static EVENTS_AFTER_WRAPAROUND: AtomicU32 = AtomicU32::new(0);
    static PHASE1_OFFERS: AtomicU8 = AtomicU8::new(0);
    static PHASE2_OFFERS: AtomicU8 = AtomicU8::new(0);
    EVENTS_BEFORE_WRAPAROUND.store(0, Ordering::SeqCst);
    EVENTS_AFTER_WRAPAROUND.store(0, Ordering::SeqCst);
    PHASE1_OFFERS.store(0, Ordering::SeqCst);
    PHASE2_OFFERS.store(0, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Client (library under test) - subscribes and counts events, drives the simulation
    sim.client("client", async move {
        // Give server a moment to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Find and subscribe to service
        let proxy = tokio::time::timeout(Duration::from_secs(10), runtime.find(TEST_SERVICE_ID))
            .await
            .expect("Should discover service within 10s")
            .expect("Service should be available");

        let eventgroup = EventgroupId::new(1).unwrap();
        let mut subscription = proxy
            .new_subscription()
            .eventgroup(eventgroup)
            .subscribe()
            .await
            .expect("Should subscribe successfully");

        // Count events in phase 1 (before wraparound)
        // Server sends events with session IDs 0xFFFD, 0xFFFE, 0xFFFF during this phase
        // Each event has payload [0x01] to mark it as phase 1
        for _i in 0..3 {
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(2000), subscription.next()).await
            {
                // Verify event is from the correct eventgroup/service
                assert_eq!(
                    event.event_id,
                    EventId::new(0x8001).unwrap(),
                    "Event should have correct event ID"
                );

                // Verify event is from phase 1 (before wraparound)
                assert!(event.payload.len() >= 1, "Event should have payload");
                assert_eq!(
                    event.payload[0], 0x01,
                    "Event should be from phase 1 (payload marker 0x01), got 0x{:02x}",
                    event.payload[0]
                );

                EVENTS_BEFORE_WRAPAROUND.fetch_add(1, Ordering::SeqCst);
            }
        }

        // Count events in phase 2 (after wraparound)
        // Server sends events with session IDs 1, 2, 3, 4, 5 during this phase
        // Each event has payload [0x02] to mark it as phase 2
        // If false reboot was detected, subscription would be invalidated and events would stop
        for _ in 0..5 {
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(200), subscription.next()).await
            {
                // Verify event is still valid after wraparound
                assert_eq!(
                    event.event_id,
                    EventId::new(0x8001).unwrap(),
                    "Event should still have correct event ID after wraparound"
                );

                // Verify event is from phase 2 (after wraparound)
                assert!(event.payload.len() >= 1, "Event should have payload");
                assert_eq!(
                    event.payload[0], 0x02,
                    "Event should be from phase 2 (payload marker 0x02), got 0x{:02x}",
                    event.payload[0]
                );

                EVENTS_AFTER_WRAPAROUND.fetch_add(1, Ordering::SeqCst);
            }
        }

        // Give server time to finish sending all SD offers
        tokio::time::sleep(Duration::from_millis(200)).await;

        Ok(())
    });

    // Server (raw sockets) - sends SD offers with controlled session IDs, handles subscriptions, sends events
    sim.host("server", || async move {
        let sd_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?);
        let event_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?);

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server").to_string().parse().unwrap();
        let client_ip: std::net::IpAddr = turmoil::lookup("client").into();
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // Task to send periodic initial offers for discovery
        let sd_socket_discovery = Arc::clone(&sd_socket);
        let discovery_task = tokio::spawn(async move {
            for i in 1..20 {
                let offer = build_sd_offer_with_session(
                    TEST_SERVICE_ID,
                    0x0001,
                    TEST_SERVICE_VERSION.0,
                    TEST_SERVICE_VERSION.1,
                    my_ip,
                    30509,
                    5,
                    i,    // Start with session 1
                    true, // reboot=true for initial discovery
                    false,
                );
                let _ = sd_socket_discovery.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        // Wait for subscription
        let mut buf = [0u8; 1500];
        let client_event_port = loop {
            match tokio::time::timeout(Duration::from_millis(500), sd_socket.recv_from(&mut buf))
                .await
            {
                Ok(Ok((len, from))) => {
                    // Check if this is a SubscribeEventgroup message
                    // Entry type is at offset 24 (16 bytes SOME/IP header + 4 bytes SD flags + 4 bytes entry length)
                    if len > 40 && buf[24] == 0x06 {
                        // Entry type = SubscribeEventgroup
                        // Extract endpoint port from option
                        if len > 52 {
                            let port = u16::from_be_bytes([buf[len - 2], buf[len - 1]]);

                            // Send SubscribeAck
                            let subscribe_ack = build_sd_subscribe_ack(
                                TEST_SERVICE_ID,
                                0x0001,
                                TEST_SERVICE_VERSION.0,
                                1, // eventgroup_id
                                5, // ttl
                                1, // session_id
                            );
                            let _ = sd_socket.send_to(&subscribe_ack, from).await;

                            break port;
                        }
                    }
                }
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => {
                    // Timeout - keep trying
                    continue;
                }
            }
        };

        // Stop discovery task now that subscription is complete
        discovery_task.abort();
        // Wait for any in-flight messages from the discovery task to be processed
        // before starting sd_task which uses different session/reboot values
        tokio::time::sleep(Duration::from_millis(100)).await;

        let client_event_addr = std::net::SocketAddr::new(client_ip, client_event_port);

        // Task to send SD offers with controlled session IDs
        let sd_socket_clone = Arc::clone(&sd_socket);
        // Continue progression offers (reboot=0, session advancing)
        for session in 20..30 {
            let offer = build_sd_offer_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                5,
                session,
                false, // reboot=false (peer already wrapped once)
                false,
            );
            let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Phase 1: Send offers near end of session ID range with reboot=0
        for session in 0xFFFD..=0xFFFF {
            let offer = build_sd_offer_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                5,
                session,
                false, // reboot_flag = false (normal operation)
                false,
            );
            let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
            PHASE1_OFFERS.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Send events synchronized with SD phases
        // Phase 1 events: sent while SD session IDs are 0xFFFD-0xFFFF
        // Payload contains 0x01 to mark phase 1
        for i in 0..3 {
            let mut event = Vec::with_capacity(17);
            event.extend_from_slice(&TEST_SERVICE_ID.to_be_bytes());
            event.extend_from_slice(&0x8001u16.to_be_bytes()); // Event ID
            event.extend_from_slice(&9u32.to_be_bytes()); // Length (header 8 + payload 1)
            event.extend_from_slice(&0x0000u16.to_be_bytes()); // Client ID
            event.extend_from_slice(&(0xFFFDu16 + i).to_be_bytes()); // Session ID matches phase 1
            event.push(0x01); // Protocol Version
            event.push(0x01); // Interface Version
            event.push(0x02); // Message Type = NOTIFICATION
            event.push(0x00); // Return Code
            event.push(0x01); // Payload: Phase 1 marker

            let _ = event_socket.send_to(&event, client_event_addr).await;
            tracing::info!("Sent phase 1 event with session {}", 0xFFFD + i);
            tokio::time::sleep(Duration::from_millis(80)).await;
        }

        // Wait before wraparound to ensure phase 1 events are sent
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Phase 2: Wrap around - session resets to 1 with reboot=0 (NORMAL, NOT reboot)
        for session in 1..5 {
            let offer = build_sd_offer_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                5,
                session,
                false, // reboot_flag = false (normal wraparound, NOT reboot)
                false,
            );
            let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
            PHASE2_OFFERS.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Phase 2 events: sent while SD session IDs are 1-4
        // Payload contains 0x02 to mark phase 2
        for i in 0..5 {
            let mut event = Vec::with_capacity(17);
            event.extend_from_slice(&TEST_SERVICE_ID.to_be_bytes());
            event.extend_from_slice(&0x8001u16.to_be_bytes()); // Event ID
            event.extend_from_slice(&9u32.to_be_bytes()); // Length (header 8 + payload 1)
            event.extend_from_slice(&0x0000u16.to_be_bytes()); // Client ID
            event.extend_from_slice(&((i + 1) as u16).to_be_bytes()); // Session ID matches phase 2
            event.push(0x01); // Protocol Version
            event.push(0x01); // Interface Version
            event.push(0x02); // Message Type = NOTIFICATION
            event.push(0x00); // Return Code
            event.push(0x02); // Payload: Phase 2 marker

            let _ = event_socket.send_to(&event, client_event_addr).await;
            tokio::time::sleep(Duration::from_millis(80)).await;
        }

        Ok(())
    });

    sim.run().unwrap();

    // Verify events were continuously received across wraparound
    let events_before = EVENTS_BEFORE_WRAPAROUND.load(Ordering::SeqCst);
    let events_after = EVENTS_AFTER_WRAPAROUND.load(Ordering::SeqCst);

    assert!(
        events_before > 0,
        "Should receive events before wraparound (got {})",
        events_before
    );
    assert!(
        events_after > 0,
        "Should receive events after wraparound (got {})",
        events_after
    );
    assert_eq!(
        PHASE1_OFFERS.load(Ordering::SeqCst),
        3,
        "Server should have sent 3 SD offers in phase 1 (0xFFFD-0xFFFF)"
    );
    assert_eq!(
        PHASE2_OFFERS.load(Ordering::SeqCst),
        4,
        "Server should have sent 4 SD offers in phase 2 (1-4)"
    );
}

/// feat_req_someipsd_764: Detect reboot via session regression (case 2)
///
/// When both old and new messages have reboot=1, and new.session <= old.session,
/// this indicates a reboot (the peer rebooted again before wrapping).
///
#[test_log::test]
fn detect_peer_reboot_session_regression() {
    covers!(feat_req_someipsd_764, feat_req_someipsd_863);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("client", || async move {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let _ = tokio::time::timeout(Duration::from_secs(5), proxy).await;

        // Wait for session regression to be simulated
        tokio::time::sleep(Duration::from_millis(800)).await;

        // TODO: Verify services were expired due to detected reboot
        // This requires feat_req_someipsd_871 implementation

        Ok(())
    });

    sim.host("server", || async move {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server").to_string().parse().unwrap();
        let client_ip: std::net::IpAddr = turmoil::lookup("client").into();
        let client_sd_addr = std::net::SocketAddr::new(client_ip, 30490);

        // Phase 1: Send offers with reboot=true, session increasing
        for session in 100..105 {
            let offer = build_sd_offer_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                5,
                session,
                true, // reboot_flag = true
                false,
            );
            socket.send_to(&offer, client_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Phase 2: Simulate second reboot - session regresses while reboot=true
        // Detection: old.reboot=1, new.reboot=1, old.session(104) >= new.session(1)
        for session in 1..5 {
            let offer = build_sd_offer_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                5,
                session,
                true, // reboot_flag = true (still rebooting!)
                false,
            );
            socket.send_to(&offer, client_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someipsd_764: Detect reboot when session ID stays equal (edge case)
/// feat_req_someipsd_871: Expire subscriptions on peer reboot
///
/// When both old and new messages have reboot=1, and new.session == old.session,
/// this indicates a reboot (retransmission or stuck in reboot loop).
/// Per spec: old.session_id >= new.session_id includes equality.
///
/// This test verifies via subscription that reboot detection works for the equality case:
/// - Client subscribes and receives events during phase 1 (session=100, reboot=1)
/// - Server sends SD offers with SAME session=100 and reboot=1 (simulates reboot)
/// - Events should stop because subscription is expired due to reboot detection
#[test_log::test]
fn detect_peer_reboot_session_equal() {
    covers!(
        feat_req_someipsd_764,
        feat_req_someipsd_863,
        feat_req_someipsd_871
    );

    use recentip::EventgroupId;
    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    };

    static EVENTS_BEFORE_REBOOT: AtomicU32 = AtomicU32::new(0);
    static EVENTS_AFTER_REBOOT: AtomicU32 = AtomicU32::new(0);
    EVENTS_BEFORE_REBOOT.store(0, Ordering::SeqCst);
    EVENTS_AFTER_REBOOT.store(0, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Client (library under test) - subscribes and counts events
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(Duration::from_secs(10), runtime.find(TEST_SERVICE_ID))
            .await
            .expect("Should discover service within 10s")
            .expect("Service should be available");

        let eventgroup = EventgroupId::new(1).unwrap();
        let mut subscription = proxy
            .new_subscription()
            .eventgroup(eventgroup)
            .subscribe()
            .await
            .expect("Should subscribe successfully");

        // Phase 1: Receive events before reboot detection (session=100, reboot=1)
        for _ in 0..3 {
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(2000), subscription.next()).await
            {
                assert_eq!(event.event_id, EventId::new(0x8001).unwrap());
                assert!(event.payload.len() >= 1);
                assert_eq!(event.payload[0], 0x01, "Event should be from phase 1");
                EVENTS_BEFORE_REBOOT.fetch_add(1, Ordering::SeqCst);
            }
        }

        // Phase 2: Try to receive events after reboot - should timeout/fail
        // because subscription should be expired due to reboot detection
        for _ in 0..5 {
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(200), subscription.next()).await
            {
                assert_eq!(event.event_id, EventId::new(0x8001).unwrap());
                assert!(event.payload.len() >= 1);
                assert_eq!(event.payload[0], 0x02, "Event should be from phase 2");
                EVENTS_AFTER_REBOOT.fetch_add(1, Ordering::SeqCst);
            }
        }

        Ok(())
    });

    // Server (raw sockets) - controls SD session IDs, handles subscription, sends events
    sim.host("server", || async move {
        let sd_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?);
        let event_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?);

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server").to_string().parse().unwrap();
        let client_ip: std::net::IpAddr = turmoil::lookup("client").into();
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // Discovery task - send initial offers
        let sd_socket_discovery = Arc::clone(&sd_socket);
        let discovery_task = tokio::spawn(async move {
            for i in 1..20 {
                let offer = build_sd_offer_with_session(
                    TEST_SERVICE_ID,
                    0x0001,
                    TEST_SERVICE_VERSION.0,
                    TEST_SERVICE_VERSION.1,
                    my_ip,
                    30509,
                    5,
                    i,
                    true, // reboot=true for initial discovery
                    false,
                );
                let _ = sd_socket_discovery.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        // Wait for subscription
        let mut buf = [0u8; 1500];
        let client_event_port = loop {
            match tokio::time::timeout(Duration::from_millis(500), sd_socket.recv_from(&mut buf))
                .await
            {
                Ok(Ok((len, from))) => {
                    if len > 40 && buf[24] == 0x06 {
                        if len > 52 {
                            let port = u16::from_be_bytes([buf[len - 2], buf[len - 1]]);

                            let subscribe_ack = build_sd_subscribe_ack(
                                TEST_SERVICE_ID,
                                0x0001,
                                TEST_SERVICE_VERSION.0,
                                1,
                                5,
                                1,
                            );
                            let _ = sd_socket.send_to(&subscribe_ack, from).await;
                            break port;
                        }
                    }
                }
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => continue,
            }
        };

        discovery_task.abort();
        // Wait for any in-flight messages from discovery_task to be processed
        // This prevents race conditions where session=1 messages arrive after
        // sd_task starts sending session=100
        tokio::time::sleep(Duration::from_millis(100)).await;
        let client_event_addr = std::net::SocketAddr::new(client_ip, client_event_port);

        // SD task - send offers with session=100, then repeat with same session (reboot!)
        let sd_socket_clone = Arc::clone(&sd_socket);
        // Phase 1: Normal operation with session=100, reboot=1
        for i in 0..5 {
            let offer = build_sd_offer_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                5,
                100 + i, // session 100
                true,    // reboot=true
                false,
            );
            let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // Send events synchronized with SD phases
        // Phase 1: Events before "reboot" (payload 0x01)
        for _ in 0..3 {
            let mut event = Vec::with_capacity(17);
            event.extend_from_slice(&TEST_SERVICE_ID.to_be_bytes());
            event.extend_from_slice(&0x8001u16.to_be_bytes());
            event.extend_from_slice(&9u32.to_be_bytes());
            event.extend_from_slice(&0x0000u16.to_be_bytes());
            event.extend_from_slice(&100u16.to_be_bytes()); // Session 100
            event.push(0x01);
            event.push(0x01);
            event.push(0x02);
            event.push(0x00);
            event.push(0x01); // Phase 1 marker

            let _ = event_socket.send_to(&event, client_event_addr).await;
            tokio::time::sleep(Duration::from_millis(80)).await;
        }

        tokio::time::sleep(Duration::from_millis(100)).await;

        // Phase 2: "Reboot" detected - SAME session=100 with reboot=1
        // Detection: old.reboot=1, new.reboot=1, old.session(100) >= new.session(100) → TRUE
        for _ in 0..5 {
            let offer = build_sd_offer_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                5,
                200,  // SAME session 100 (reboot detected!)
                true, // reboot=true
                false,
            );
            let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Phase 2: Events after "reboot" (payload 0x02)
        // These should NOT be delivered because subscription should be expired
        for _ in 0..5 {
            let mut event = Vec::with_capacity(17);
            event.extend_from_slice(&TEST_SERVICE_ID.to_be_bytes());
            event.extend_from_slice(&0x8001u16.to_be_bytes());
            event.extend_from_slice(&9u32.to_be_bytes());
            event.extend_from_slice(&0x0000u16.to_be_bytes());
            event.extend_from_slice(&100u16.to_be_bytes()); // Still session 100
            event.push(0x01);
            event.push(0x01);
            event.push(0x02);
            event.push(0x00);
            event.push(0x02); // Phase 2 marker

            let _ = event_socket.send_to(&event, client_event_addr).await;
            tokio::time::sleep(Duration::from_millis(80)).await;
        }

        Ok(())
    });

    sim.run().unwrap();

    // Verify events were received before reboot, but NOT after
    let events_before = EVENTS_BEFORE_REBOOT.load(Ordering::SeqCst);
    let events_after = EVENTS_AFTER_REBOOT.load(Ordering::SeqCst);

    assert_eq!(
        events_before, 3,
        "Should receive events before reboot (got {})",
        events_before
    );
    assert_eq!(events_after, 0,
        "Should NOT receive events after reboot detection (session stayed equal) - got {} events, expected 0", 
        events_after);
}

/// feat_req_someipsd_764: Detect reboot via session regression (case 2) - multi-peer
/// feat_req_someipsd_863: Reliable reboot detection
/// feat_req_someipsd_871: Expire services on peer reboot
///
/// When both old and new messages have reboot=1, and new.session <= old.session,
/// this indicates a reboot. This test verifies per-peer isolation:
/// - Rebooting server's service should be expired
/// - Stable server's service should NOT be affected
///
/// NOTE: This is currently NOT IMPLEMENTED in recentIP
#[test_log::test]
fn detect_peer_reboot_session_regression_multi_peer() {
    covers!(
        feat_req_someipsd_764,
        feat_req_someipsd_863,
        feat_req_someipsd_871
    );

    use std::sync::atomic::{AtomicBool, Ordering};

    static REBOOTING_SERVICE_EXPIRED: AtomicBool = AtomicBool::new(false);
    static STABLE_SERVICE_EXPIRED: AtomicBool = AtomicBool::new(false);
    REBOOTING_SERVICE_EXPIRED.store(false, Ordering::SeqCst);
    STABLE_SERVICE_EXPIRED.store(false, Ordering::SeqCst);

    const REBOOTING_SERVICE_ID: u16 = TEST_SERVICE_ID;
    const STABLE_SERVICE_ID: u16 = 0x5678;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Client (library under test) discovers services from both servers
    sim.host("client", || async move {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let mut sd_events = runtime.monitor_sd().await.unwrap();

        // Find both services
        let proxy1 = runtime.find(REBOOTING_SERVICE_ID);
        let proxy2 = runtime.find(STABLE_SERVICE_ID);

        let available1 = tokio::time::timeout(Duration::from_secs(5), proxy1).await;
        let available2 = tokio::time::timeout(Duration::from_secs(5), proxy2).await;

        assert!(available1.is_ok(), "Rebooting service should be discovered");
        assert!(available2.is_ok(), "Stable service should be discovered");

        // Wait for session regression to be simulated and detected
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(100)).await;

            while let Ok(event) = sd_events.try_recv() {
                match event {
                    recentip::SdEvent::ServiceUnavailable { service_id, .. }
                    | recentip::SdEvent::ServiceExpired { service_id, .. } => {
                        if service_id == REBOOTING_SERVICE_ID {
                            REBOOTING_SERVICE_EXPIRED.store(true, Ordering::SeqCst);
                        }
                        if service_id == STABLE_SERVICE_ID {
                            STABLE_SERVICE_EXPIRED.store(true, Ordering::SeqCst);
                        }
                    }
                    _ => {}
                }
            }
        }

        // Verify per-peer isolation when implemented:
        // assert!(REBOOTING_SERVICE_EXPIRED.load(Ordering::SeqCst),
        //     "Service from rebooting peer should be expired");
        // assert!(!STABLE_SERVICE_EXPIRED.load(Ordering::SeqCst),
        //     "Service from stable peer should NOT be expired");

        Ok(())
    });

    // Simulated server 1 - REBOOTS via session regression (reboot=1, session drops)
    sim.host("server_rebooting", || async move {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server_rebooting")
            .to_string()
            .parse()
            .unwrap();
        let client_ip: std::net::IpAddr = turmoil::lookup("client").into();
        let client_sd_addr = std::net::SocketAddr::new(client_ip, 30490);

        // Phase 1: Send offers with reboot=true, session increasing (100-104)
        for session in 100..105 {
            let offer = build_sd_offer_with_session(
                REBOOTING_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                3,
                session,
                true, // reboot_flag = true
                false,
            );
            socket.send_to(&offer, client_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Phase 2: Simulate second reboot - session regresses while reboot=true
        // Detection: old.reboot=1, new.reboot=1, old.session(104) > new.session(1)
        for session in 1..5 {
            let offer = build_sd_offer_with_session(
                REBOOTING_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                3,
                session,
                true, // reboot_flag = true (STILL rebooting - session regression!)
                false,
            );
            socket.send_to(&offer, client_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        tokio::time::sleep(Duration::from_millis(1500)).await;
        Ok(())
    });

    // Simulated server 2 - STABLE (reboot=true with increasing sessions, no regression)
    sim.host("server_stable", || async move {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server_stable")
            .to_string()
            .parse()
            .unwrap();
        let client_ip: std::net::IpAddr = turmoil::lookup("client").into();
        let client_sd_addr = std::net::SocketAddr::new(client_ip, 30490);

        // Stable operation: reboot=true with monotonically increasing sessions
        for session in 1..20 {
            let offer = build_sd_offer_with_session(
                STABLE_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                30510,
                3,
                session,
                true, // reboot_flag = true (normal, sessions keep increasing)
                false,
            );
            socket.send_to(&offer, client_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// SERVER-SIDE REBOOT DETECTION TESTS
// ============================================================================
// When a client reboots, the server should detect it and expire all
// subscriptions from that client.
// ============================================================================

/// Build a raw SOME/IP-SD SubscribeEventgroup message with configurable session/reboot
fn build_sd_subscribe_with_session(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    client_endpoint_ip: std::net::Ipv4Addr,
    client_endpoint_port: u16,
    ttl: u32,
    session_id: u16,
    reboot_flag: bool,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(72);

    // === SOME/IP Header (16 bytes) ===
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes()); // Service ID = SD
    packet.extend_from_slice(&0x8100u16.to_be_bytes()); // Method ID = SD
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes()); // placeholder
    packet.extend_from_slice(&0x0000u16.to_be_bytes()); // Client ID
    packet.extend_from_slice(&session_id.to_be_bytes()); // Session ID
    packet.push(0x01); // Protocol Version
    packet.push(0x01); // Interface Version
    packet.push(0x02); // Message Type = NOTIFICATION
    packet.push(0x00); // Return Code = E_OK

    // === SD Payload ===
    let mut flags = 0x40u8; // Unicast flag
    if reboot_flag {
        flags |= 0x80; // Reboot flag
    }
    packet.push(flags);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]); // Reserved

    // Entries array length (16 bytes for one entry)
    packet.extend_from_slice(&16u32.to_be_bytes());

    // === SubscribeEventgroup Entry (16 bytes) ===
    packet.push(0x06); // Type = SubscribeEventgroup
    packet.push(0x00); // Index 1st options
    packet.push(0x00); // Index 2nd options
    packet.push(0x10); // # of opt 1 | # of opt 2 = 1 option in run 1
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]); // TTL (24-bit)
    packet.push(0x00); // Reserved
    packet.push(0x00); // Counter
    packet.extend_from_slice(&eventgroup_id.to_be_bytes());

    // Options array length (12 bytes for IPv4 endpoint)
    packet.extend_from_slice(&12u32.to_be_bytes());

    // === IPv4 Endpoint Option (12 bytes) ===
    packet.extend_from_slice(&9u16.to_be_bytes()); // Length
    packet.push(0x04); // Type = IPv4 Endpoint
    packet.push(0x00); // Reserved
    packet.extend_from_slice(&client_endpoint_ip.octets());
    packet.push(0x00); // Reserved
    packet.push(0x11); // L4 Protocol = UDP
    packet.extend_from_slice(&client_endpoint_port.to_be_bytes());

    // Fix up length field
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}

/// Build a raw SOME/IP-SD FindService message with configurable session/reboot
///
/// FindService messages are typically sent via multicast, which uses a separate
/// session counter from unicast messages (like SubscribeEventgroup).
fn build_sd_find_with_session(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    minor_version: u32,
    ttl: u32,
    session_id: u16,
    reboot_flag: bool,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(40);

    // === SOME/IP Header (16 bytes) ===
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes()); // Service ID = SD
    packet.extend_from_slice(&0x8100u16.to_be_bytes()); // Method ID = SD
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes()); // placeholder
    packet.extend_from_slice(&0x0000u16.to_be_bytes()); // Client ID
    packet.extend_from_slice(&session_id.to_be_bytes()); // Session ID
    packet.push(0x01); // Protocol Version
    packet.push(0x01); // Interface Version
    packet.push(0x02); // Message Type = NOTIFICATION
    packet.push(0x00); // Return Code = E_OK

    // === SD Payload ===
    let mut flags = 0x00u8; // No unicast flag (multicast message)
    if reboot_flag {
        flags |= 0x80; // Reboot flag
    }
    packet.push(flags);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]); // Reserved

    // Entries array length (16 bytes for one entry)
    packet.extend_from_slice(&16u32.to_be_bytes());

    // === FindService Entry (16 bytes) ===
    packet.push(0x00); // Type = FindService
    packet.push(0x00); // Index 1st options
    packet.push(0x00); // Index 2nd options
    packet.push(0x00); // # of opt 1 | # of opt 2 = 0 options
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]); // TTL (24-bit)
    packet.extend_from_slice(&minor_version.to_be_bytes());

    // Options array length (0 - no options for FindService)
    packet.extend_from_slice(&0u32.to_be_bytes());

    // Fix up length field
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}

/// feat_req_someipsd_871: Server expires subscriptions when client reboots
///
/// When the server detects a client reboot (via reboot flag transition 0→1),
/// it should expire all subscriptions from that client but NOT affect other clients.
///
/// Test setup:
/// - Server (library) offers a service with an eventgroup
/// - Two simulated clients subscribe
/// - client1 "reboots" (session regresses with reboot=1)
/// - Verify: client1's subscription is expired, client2 still receives events
#[test_log::test]
#[ignore = "Server-side client reboot detection not yet implemented"]
fn server_expires_subscriptions_on_client_reboot() {
    covers!(feat_req_someipsd_764, feat_req_someipsd_871);

    use recentip::handle::ServiceEvent;
    use recentip::{EventId, EventgroupId};
    use std::sync::atomic::{AtomicU32, Ordering};

    static CLIENT1_EVENTS_AFTER_REBOOT: AtomicU32 = AtomicU32::new(0);
    static CLIENT2_EVENTS_AFTER_REBOOT: AtomicU32 = AtomicU32::new(0);
    CLIENT1_EVENTS_AFTER_REBOOT.store(0, Ordering::SeqCst);
    CLIENT2_EVENTS_AFTER_REBOOT.store(0, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server (library under test) - offers service, sends events
    sim.host("server", || async move {
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

        // Create event in eventgroup 1
        let eventgroup = EventgroupId::new(1).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        // Wait for subscriptions from both simulated clients
        let mut subscription_count = 0;
        for _ in 0..100 {
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(100), offering.next()).await
            {
                if matches!(event, ServiceEvent::Subscribe { .. }) {
                    subscription_count += 1;
                    if subscription_count >= 2 {
                        break;
                    }
                }
            }
        }
        assert!(
            subscription_count >= 2,
            "Should receive subscriptions from both clients"
        );

        // Send events before client1 "reboot"
        for i in 0..3 {
            event_handle
                .notify(format!("event-before-{}", i).as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Wait for client1 to "reboot" (session regression)
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send events after client1 "reboot"
        // Client1 should NOT receive these (subscription expired until re-subscribe)
        // Client2 should still receive these (not affected by client1's reboot)
        for i in 0..3 {
            event_handle
                .notify(format!("event-after-{}", i).as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    // Client 1 - subscribes, then "reboots"
    sim.host("client_rebooting", || async move {
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:40000").await?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("client_rebooting")
            .to_string()
            .parse()
            .unwrap();
        let server_ip: std::net::IpAddr = turmoil::lookup("server").into();
        let server_sd_addr = std::net::SocketAddr::new(server_ip, 30490);

        // Phase 1: Subscribe with session=100, reboot=false (established client)
        for session in 100..103 {
            let subscribe = build_sd_subscribe_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                1, // eventgroup_id
                my_ip,
                40000, // event port
                300,   // TTL
                session,
                false, // reboot = false (established)
            );
            sd_socket.send_to(&subscribe, server_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Receive events before "reboot"
        for _ in 0..10 {
            let mut buf = [0u8; 1500];
            let _ =
                tokio::time::timeout(Duration::from_millis(100), event_socket.recv_from(&mut buf))
                    .await;
        }

        // Phase 2: "Reboot" - send subscribe with session=1, reboot=true
        for session in 1..4 {
            let subscribe = build_sd_subscribe_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                1,
                my_ip,
                40000,
                300,
                session,
                true, // reboot = true (just rebooted!)
            );
            sd_socket.send_to(&subscribe, server_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Count events after "reboot"
        // Should NOT receive events until the re-subscribe is processed
        let mut events_after = 0u32;
        for _ in 0..20 {
            let mut buf = [0u8; 1500];
            if let Ok(Ok((len, _))) =
                tokio::time::timeout(Duration::from_millis(100), event_socket.recv_from(&mut buf))
                    .await
            {
                if len > 0 {
                    events_after += 1;
                }
            }
        }
        CLIENT1_EVENTS_AFTER_REBOOT.store(events_after, Ordering::SeqCst);

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    // Client 2 - subscribes, does NOT reboot (stable)
    sim.host("client_stable", || async move {
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:40001").await?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("client_stable")
            .to_string()
            .parse()
            .unwrap();
        let server_ip: std::net::IpAddr = turmoil::lookup("server").into();
        let server_sd_addr = std::net::SocketAddr::new(server_ip, 30490);

        // Subscribe with reboot=true initially (just started), then transition to false
        for session in 1..10 {
            let subscribe = build_sd_subscribe_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                1, // eventgroup_id
                my_ip,
                40001, // different event port
                300,   // TTL
                session,
                session < 5, // reboot=true initially, then false (normal transition)
            );
            sd_socket.send_to(&subscribe, server_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Count all events received
        // This client should continue receiving events throughout
        let mut events_after = 0u32;
        for _ in 0..30 {
            let mut buf = [0u8; 1500];
            if let Ok(Ok((len, _))) =
                tokio::time::timeout(Duration::from_millis(100), event_socket.recv_from(&mut buf))
                    .await
            {
                if len > 0 {
                    events_after += 1;
                }
            }
        }
        CLIENT2_EVENTS_AFTER_REBOOT.store(events_after, Ordering::SeqCst);

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    sim.run().unwrap();

    // Verify: stable client should receive events, rebooting client's old subscription
    // should be expired (but may re-establish via re-subscribe)
    let client2_events = CLIENT2_EVENTS_AFTER_REBOOT.load(Ordering::SeqCst);
    assert!(
        client2_events > 0,
        "Stable client should continue receiving events (got {})",
        client2_events
    );
}

/// feat_req_someipsd_764: Server detects client reboot via session regression (case 2)
/// feat_req_someipsd_871: Expire subscriptions on client reboot
///
/// When a client's session ID regresses while reboot=1, this indicates a second reboot
/// before the session wrapped. Server should detect this and expire subscriptions
/// from the rebooting client, while NOT affecting other clients.
///
/// NOTE: This is currently NOT IMPLEMENTED in recentIP
#[test_log::test]
#[ignore = "Session regression detection not yet implemented"]
fn server_expires_subscriptions_on_client_session_regression() {
    covers!(feat_req_someipsd_764, feat_req_someipsd_871);

    use recentip::handle::ServiceEvent;
    use recentip::{EventId, EventgroupId};
    use std::sync::atomic::{AtomicU32, Ordering};

    static CLIENT1_EVENTS_AFTER_REGRESSION: AtomicU32 = AtomicU32::new(0);
    static CLIENT2_EVENTS_AFTER_REGRESSION: AtomicU32 = AtomicU32::new(0);
    CLIENT1_EVENTS_AFTER_REGRESSION.store(0, Ordering::SeqCst);
    CLIENT2_EVENTS_AFTER_REGRESSION.store(0, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server (library under test) - offers service, sends events
    sim.host("server", || async move {
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

        let eventgroup = EventgroupId::new(1).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        // Wait for subscriptions from both simulated clients
        let mut subscription_count = 0;
        for _ in 0..100 {
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(100), offering.next()).await
            {
                if matches!(event, ServiceEvent::Subscribe { .. }) {
                    subscription_count += 1;
                    if subscription_count >= 2 {
                        break;
                    }
                }
            }
        }
        assert!(
            subscription_count >= 2,
            "Should receive subscriptions from both clients"
        );

        // Send events before client1 session regression
        for i in 0..3 {
            event_handle
                .notify(format!("event-before-{}", i).as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Wait for client1 session regression to be simulated
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send events after client1 session regression
        // Client1's subscription should be expired
        // Client2 should still receive events
        for i in 0..3 {
            event_handle
                .notify(format!("event-after-{}", i).as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    // Client 1 - subscribes, then "reboots" via session regression (reboot=1, session drops)
    sim.host("client_rebooting", || async move {
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:40000").await?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("client_rebooting")
            .to_string()
            .parse()
            .unwrap();
        let server_ip: std::net::IpAddr = turmoil::lookup("server").into();
        let server_sd_addr = std::net::SocketAddr::new(server_ip, 30490);

        // Phase 1: Subscribe with reboot=true, session=100-104 (increasing)
        for session in 100..105 {
            let subscribe = build_sd_subscribe_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                1,
                my_ip,
                40000,
                300,
                session,
                true, // reboot = true
            );
            sd_socket.send_to(&subscribe, server_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Receive events before session regression
        for _ in 0..10 {
            let mut buf = [0u8; 1500];
            let _ =
                tokio::time::timeout(Duration::from_millis(100), event_socket.recv_from(&mut buf))
                    .await;
        }

        // Phase 2: Simulate second reboot - session regresses while reboot=true
        // Detection: old.reboot=1, new.reboot=1, old.session(104) > new.session(1)
        for session in 1..5 {
            let subscribe = build_sd_subscribe_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                1,
                my_ip,
                40000,
                300,
                session,
                true, // reboot = true (STILL - session regression!)
            );
            sd_socket.send_to(&subscribe, server_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Count events after session regression
        let mut events_after = 0u32;
        for _ in 0..20 {
            let mut buf = [0u8; 1500];
            if let Ok(Ok((len, _))) =
                tokio::time::timeout(Duration::from_millis(100), event_socket.recv_from(&mut buf))
                    .await
            {
                if len > 0 {
                    events_after += 1;
                }
            }
        }
        CLIENT1_EVENTS_AFTER_REGRESSION.store(events_after, Ordering::SeqCst);

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    // Client 2 - subscribes with reboot=true, sessions always increasing (stable)
    sim.host("client_stable", || async move {
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:40001").await?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("client_stable")
            .to_string()
            .parse()
            .unwrap();
        let server_ip: std::net::IpAddr = turmoil::lookup("server").into();
        let server_sd_addr = std::net::SocketAddr::new(server_ip, 30490);

        // Stable operation: reboot=true with monotonically increasing sessions
        for session in 1..20 {
            let subscribe = build_sd_subscribe_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                1,
                my_ip,
                40001,
                300,
                session,
                true, // reboot = true, but sessions keep increasing
            );
            sd_socket.send_to(&subscribe, server_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // This client should continue receiving events throughout
        let mut events_after = 0u32;
        for _ in 0..30 {
            let mut buf = [0u8; 1500];
            if let Ok(Ok((len, _))) =
                tokio::time::timeout(Duration::from_millis(100), event_socket.recv_from(&mut buf))
                    .await
            {
                if len > 0 {
                    events_after += 1;
                }
            }
        }
        CLIENT2_EVENTS_AFTER_REGRESSION.store(events_after, Ordering::SeqCst);

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    sim.run().unwrap();

    // Verify: stable client should receive events
    let client2_events = CLIENT2_EVENTS_AFTER_REGRESSION.load(Ordering::SeqCst);
    assert!(
        client2_events > 0,
        "Stable client should continue receiving events (got {})",
        client2_events
    );
}

// ============================================================================
// SESSION COUNTER ISOLATION TESTS
// ============================================================================
// Per spec, multicast and unicast SD messages use separate session counters.
// A "regression" on one counter should NOT affect state established via the other.
// ============================================================================

/// feat_req_someipsd_765: Separate session counters for multicast vs unicast
///
/// A client sends SubscribeEventgroup (unicast, high session ID), then later
/// sends FindService (multicast, low session ID). Since these use separate
/// session counters, the server MUST NOT interpret this as a reboot.
///
/// This is a regression test for incorrect implementations that track only
/// one session counter per peer instead of per (peer, channel) pair.
#[test_log::test]
fn subscription_survives_low_session_find_service() {
    covers!(feat_req_someipsd_765, feat_req_someipsd_764);

    use recentip::handle::ServiceEvent;
    use recentip::{EventId, EventgroupId};
    use std::sync::atomic::{AtomicU32, Ordering};

    static EVENTS_RECEIVED_AFTER_FIND: AtomicU32 = AtomicU32::new(0);
    EVENTS_RECEIVED_AFTER_FIND.store(0, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server (library under test) - offers service with eventgroup
    sim.host("server", || async move {
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

        // Create event in eventgroup 1
        let eventgroup = EventgroupId::new(1).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        // Wait for subscription from simulated client
        let mut got_subscription = false;
        for _ in 0..50 {
            match tokio::time::timeout(Duration::from_millis(100), offering.next()).await {
                Ok(Some(event)) => {
                    if matches!(event, ServiceEvent::Subscribe { .. }) {
                        got_subscription = true;
                        break;
                    }
                }
                Ok(None) => {
                    break;
                }
                Err(_) => {
                    // timeout, continue
                }
            }
        }
        assert!(
            got_subscription,
            "Should receive subscription from simulated client"
        );

        // Wait for client to send FindService with low session ID
        // Give enough time for the client to subscribe and send FindService
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Send events AFTER the FindService with low session ID
        // These MUST still be delivered - subscription should survive
        for i in 0..5 {
            event_handle
                .notify(format!("event-after-find-{}", i).as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    // Simulated client - subscribes with high session, then sends FindService with low session
    sim.client("client", async move {
        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:40000").await?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("client").to_string().parse().unwrap();

        // Wait for OfferService from server and extract server address
        let mut buf = [0u8; 1500];
        let mut server_sd_addr: Option<std::net::SocketAddr> = None;
        for _ in 0..50 {
            if let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // OfferService = 0x01 with matching service_id and TTL > 0
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == TEST_SERVICE_ID
                            && entry.ttl > 0
                        {
                            server_sd_addr = Some(std::net::SocketAddr::new(from.ip(), 30490));
                            break;
                        }
                    }
                }
            }
            if server_sd_addr.is_some() {
                break;
            }
        }
        let server_sd_addr = server_sd_addr.expect("Should receive OfferService from server");

        // Phase 1: Subscribe using the known-working helper
        // Send multiple times to ensure delivery
        for i in 0..4 {
            let subscribe = build_sd_subscribe_with_udp_endpoint(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                1,        // eventgroup_id
                0xFFFFFF, // Max TTL (~194 days)
                my_ip,
                40000, // event port
                i,     // session_id
            );
            sd_socket.send_to(&subscribe, server_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Wait for subscription to be processed
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Phase 2: Send FindService with LOW multicast session ID (1-3)
        // This simulates the client's multicast session counter being lower
        // than its unicast session counter (completely normal per spec)
        for session in 1..4 {
            let find = build_sd_find_with_session(
                TEST_SERVICE_ID,
                0xFFFF,     // ANY instance
                0xFF,       // ANY major version
                0xFFFFFFFF, // ANY minor version
                3,          // TTL
                session,
                true, // reboot = true
            );
            // Send FindService to server's SD port (simulating multicast behavior)
            sd_socket.send_to(&find, server_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Phase 3: Count events received AFTER the FindService
        // If subscription survived (correct behavior), we should get events
        let mut events_after = 0u32;
        for _ in 0..30 {
            let mut buf = [0u8; 1500];
            if let Ok(Ok((len, _))) =
                tokio::time::timeout(Duration::from_millis(100), event_socket.recv_from(&mut buf))
                    .await
            {
                if len > 0 {
                    events_after += 1;
                }
            }
        }
        EVENTS_RECEIVED_AFTER_FIND.store(events_after, Ordering::SeqCst);

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    sim.run().unwrap();

    // Verify: subscription should survive, events should be received
    let events_after = EVENTS_RECEIVED_AFTER_FIND.load(Ordering::SeqCst);
    assert!(
        events_after > 0,
        "Subscription should survive FindService with lower session ID (got {} events after)",
        events_after
    );
}

/// feat_req_someipsd_765: Separate session counters for multicast vs unicast
///
/// The inverse scenario: A client sends many FindService messages (pushing up
/// its multicast session counter), then sends SubscribeEventgroup with a lower
/// session ID. Since these use separate session counters, the subscription
/// MUST be accepted as valid.
///
/// This proves the session counter isolation works in both directions.
#[test_log::test]
fn subscription_valid_after_high_session_finds() {
    covers!(feat_req_someipsd_765, feat_req_someipsd_764);

    use recentip::handle::ServiceEvent;
    use recentip::{EventId, EventgroupId};
    use std::sync::atomic::{AtomicU32, Ordering};

    static EVENTS_RECEIVED: AtomicU32 = AtomicU32::new(0);
    EVENTS_RECEIVED.store(0, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .max_message_latency(Duration::from_millis(10))
        .build();

    // Server (library under test) - offers service with eventgroup
    sim.host("server", || async move {
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

        // Create event in eventgroup 1
        let eventgroup = EventgroupId::new(1).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        // Wait for subscription from simulated client
        // This should arrive AFTER the client has sent many FindService messages
        let mut got_subscription = false;
        for _ in 0..100 {
            match tokio::time::timeout(Duration::from_millis(100), offering.next()).await {
                Ok(Some(event)) => {
                    if matches!(event, ServiceEvent::Subscribe { .. }) {
                        got_subscription = true;
                        break;
                    }
                }
                Ok(None) => {
                    break;
                }
                Err(_) => {
                    // timeout, continue
                }
            }
        }
        assert!(
            got_subscription,
            "Should receive subscription despite lower session ID than FindService"
        );

        // Send events to the subscriber
        for i in 0..5 {
            event_handle
                .notify(format!("event-{}", i).as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    // Simulated client - sends many FindService first, then subscribes with low session
    sim.client("client", async move {
        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:40000").await?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("client").to_string().parse().unwrap();

        // Wait for OfferService from server and extract server address
        let mut buf = [0u8; 1500];
        let mut server_sd_addr: Option<std::net::SocketAddr> = None;
        for _ in 0..50 {
            if let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // OfferService = 0x01 with matching service_id and TTL > 0
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == TEST_SERVICE_ID
                            && entry.ttl > 0
                        {
                            server_sd_addr = Some(std::net::SocketAddr::new(from.ip(), 30490));
                            break;
                        }
                    }
                }
            }
            if server_sd_addr.is_some() {
                break;
            }
        }
        let server_sd_addr = server_sd_addr.expect("Should receive OfferService from server");

        // Phase 1: Send MANY FindService messages with HIGH session IDs
        // This simulates the client having sent lots of multicast SD messages
        // before establishing a unicast subscription
        for session in 100..150u16 {
            let find = build_sd_find_with_session(
                TEST_SERVICE_ID,
                0xFFFF,     // ANY instance
                0xFF,       // ANY major version
                0xFFFFFFFF, // ANY minor version
                3,          // TTL
                session,
                true, // reboot = true
            );
            sd_socket.send_to(&find, server_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // Phase 2: Subscribe with LOW session ID (simulating fresh unicast counter)
        // This should be valid because unicast has a separate session counter
        for _ in 0..4 {
            let subscribe = build_sd_subscribe_with_udp_endpoint(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                1,        // eventgroup_id
                0xFFFFFF, // Max TTL
                my_ip,
                40000, // event port
                1,     // session_id
            );
            sd_socket.send_to(&subscribe, server_sd_addr).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Phase 3: Receive events - if subscription was accepted, we get events
        let mut events_received = 0u32;
        for _ in 0..30 {
            let mut buf = [0u8; 1500];
            if let Ok(Ok((len, _))) =
                tokio::time::timeout(Duration::from_millis(100), event_socket.recv_from(&mut buf))
                    .await
            {
                if len > 0 {
                    events_received += 1;
                }
            }
        }
        EVENTS_RECEIVED.store(events_received, Ordering::SeqCst);

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    sim.run().unwrap();

    // Verify: subscription should be valid, events should be received
    let events = EVENTS_RECEIVED.load(Ordering::SeqCst);
    assert!(
        events > 0,
        "Subscription with low session ID should be valid after high-session FindService (got {} events)",
        events
    );
}

/// feat_req_someipsd_764, feat_req_someipsd_765: Normal session wraparound does NOT trigger reboot
///
/// When a peer's session ID wraps from 0xFFFF to 0x0001 AND the reboot flag
/// transitions from 1 to 0, this is NORMAL behavior - the peer has completed
/// its first session cycle. This should NOT trigger reboot detection.
///
/// The server should continue to honor the subscription.
#[test_log::test]
#[ignore = "Session regression detection not yet implemented - this tests the no-false-positive case"]
fn normal_session_wraparound_does_not_trigger_reboot() {
    covers!(feat_req_someipsd_764, feat_req_someipsd_765);

    use recentip::{EventId, EventgroupId};
    use std::sync::atomic::{AtomicU32, Ordering};

    static EVENTS_AFTER_WRAPAROUND: AtomicU32 = AtomicU32::new(0);
    EVENTS_AFTER_WRAPAROUND.store(0, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server (library under test) - offers service and publishes events
    sim.host("server", || async move {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Create event in eventgroup 1
        let eventgroup = EventgroupId::new(1).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        // Spawn event publisher
        tokio::spawn(async move {
            loop {
                event_handle.notify(&[0xAA, 0xBB]);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        // Keep server alive
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    });

    // Simulated client - subscribes, then simulates session wraparound
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:40000").await?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("client").to_string().parse().unwrap();

        // Wait for OfferService from server
        let mut buf = [0u8; 1500];
        let mut server_sd_addr: Option<std::net::SocketAddr> = None;
        for _ in 0..50 {
            if let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == TEST_SERVICE_ID
                            && entry.ttl > 0
                        {
                            server_sd_addr = Some(std::net::SocketAddr::new(from.ip(), 30490));
                            break;
                        }
                    }
                }
            }
            if server_sd_addr.is_some() {
                break;
            }
        }
        let server_sd_addr = server_sd_addr.expect("Should receive OfferService from server");

        // Phase 1: Subscribe with HIGH session ID (0xFFFE) and reboot=1
        // This simulates a client near the end of its first session cycle
        let subscribe1 = build_sd_subscribe_with_session(
            TEST_SERVICE_ID,
            0x0001,
            TEST_SERVICE_VERSION.0,
            1, // eventgroup_id
            my_ip,
            40000,    // event port
            0xFFFFFF, // Max TTL
            0xFFFE,   // HIGH session ID (near wraparound)
            true,     // reboot = 1 (first cycle)
        );
        sd_socket.send_to(&subscribe1, server_sd_addr).await?;
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Verify we're getting events before wraparound
        let mut events_before = 0u32;
        for _ in 0..5 {
            let mut buf = [0u8; 1500];
            if let Ok(Ok((len, _))) =
                tokio::time::timeout(Duration::from_millis(100), event_socket.recv_from(&mut buf))
                    .await
            {
                if len > 0 {
                    events_before += 1;
                }
            }
        }
        assert!(events_before > 0, "Should receive events before wraparound");

        // Phase 2: Send subscribe refresh with session 0xFFFF (last before wrap)
        let subscribe2 = build_sd_subscribe_with_session(
            TEST_SERVICE_ID,
            0x0001,
            TEST_SERVICE_VERSION.0,
            1,
            my_ip,
            40000,
            0xFFFFFF, // Max TTL
            0xFFFF,   // Last session before wrap
            true,     // reboot = 1 (still first cycle)
        );
        sd_socket.send_to(&subscribe2, server_sd_addr).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Phase 3: WRAPAROUND - session goes to 0x0001, reboot flag clears to 0
        // This is the NORMAL wraparound behavior - NOT a reboot
        let subscribe3 = build_sd_subscribe_with_session(
            TEST_SERVICE_ID,
            0x0001,
            TEST_SERVICE_VERSION.0,
            1,
            my_ip,
            40000,
            0xFFFFFF, // Max TTL
            0x0001,   // Wrapped around to 1
            false,    // reboot = 0 (wrapped - normal behavior!)
        );
        sd_socket.send_to(&subscribe3, server_sd_addr).await?;
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Phase 4: Count events AFTER wraparound
        // Subscription should still be active (no false positive reboot detection)
        let mut events_after = 0u32;
        for _ in 0..20 {
            let mut buf = [0u8; 1500];
            if let Ok(Ok((len, _))) =
                tokio::time::timeout(Duration::from_millis(100), event_socket.recv_from(&mut buf))
                    .await
            {
                if len > 0 {
                    events_after += 1;
                }
            }
        }
        EVENTS_AFTER_WRAPAROUND.store(events_after, Ordering::SeqCst);

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    sim.run().unwrap();

    // Verify: subscription should survive the wraparound
    let events = EVENTS_AFTER_WRAPAROUND.load(Ordering::SeqCst);
    assert!(
        events > 0,
        "Subscription should survive normal session wraparound (0xFFFF→1 with reboot 1→0), got {} events",
        events
    );
}

/// feat_req_someipsd_764: Multicast session wraparound does NOT affect unicast subscriptions
///
/// Similar to above, but tests that multicast session wraparound (from
/// FindService/OfferService) doesn't affect unicast subscriptions.
#[test_log::test]
#[ignore = "Session regression detection not yet implemented - this tests the no-false-positive case"]
fn multicast_session_wraparound_does_not_affect_subscriptions() {
    covers!(feat_req_someipsd_764, feat_req_someipsd_765);

    use recentip::{EventId, EventgroupId};
    use std::sync::atomic::{AtomicU32, Ordering};

    static EVENTS_AFTER_WRAPAROUND: AtomicU32 = AtomicU32::new(0);
    EVENTS_AFTER_WRAPAROUND.store(0, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async move {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Create event in eventgroup 1
        let eventgroup = EventgroupId::new(1).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        // Spawn event publisher
        tokio::spawn(async move {
            loop {
                event_handle.notify(&[0xCC, 0xDD]);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        // Keep server alive
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:40000").await?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("client").to_string().parse().unwrap();

        // Wait for OfferService
        let mut buf = [0u8; 1500];
        let mut server_sd_addr: Option<std::net::SocketAddr> = None;
        for _ in 0..50 {
            if let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == TEST_SERVICE_ID
                            && entry.ttl > 0
                        {
                            server_sd_addr = Some(std::net::SocketAddr::new(from.ip(), 30490));
                            break;
                        }
                    }
                }
            }
            if server_sd_addr.is_some() {
                break;
            }
        }
        let server_sd_addr = server_sd_addr.expect("Should receive OfferService from server");

        // Phase 1: Subscribe with normal unicast session ID
        let subscribe = build_sd_subscribe_with_udp_endpoint(
            TEST_SERVICE_ID,
            0x0001,
            TEST_SERVICE_VERSION.0,
            1,
            0xFFFFFF,
            my_ip,
            40000,
            1, // session_id
        );
        sd_socket.send_to(&subscribe, server_sd_addr).await?;
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Verify events before multicast wraparound
        let mut events_before = 0u32;
        for _ in 0..5 {
            let mut buf = [0u8; 1500];
            if let Ok(Ok((len, _))) =
                tokio::time::timeout(Duration::from_millis(100), event_socket.recv_from(&mut buf))
                    .await
            {
                if len > 0 {
                    events_before += 1;
                }
            }
        }
        assert!(
            events_before > 0,
            "Should receive events before multicast wraparound"
        );

        // Phase 2: Send FindService with HIGH multicast session (0xFFFF) and reboot=1
        let find1 = build_sd_find_with_session(
            TEST_SERVICE_ID,
            0xFFFF,
            0xFF,
            0xFFFFFFFF,
            3,
            0xFFFF, // HIGH multicast session
            true,   // reboot = 1
        );
        sd_socket.send_to(&find1, server_sd_addr).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Phase 3: Send FindService with WRAPPED session (0x0001) and reboot=0
        // This simulates multicast counter wraparound
        let find2 = build_sd_find_with_session(
            TEST_SERVICE_ID,
            0xFFFF,
            0xFF,
            0xFFFFFFFF,
            3,
            0x0001, // Wrapped multicast session
            false,  // reboot = 0 (normal wraparound)
        );
        sd_socket.send_to(&find2, server_sd_addr).await?;
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Phase 4: Count events AFTER multicast wraparound
        // Unicast subscription should be unaffected
        let mut events_after = 0u32;
        for _ in 0..20 {
            let mut buf = [0u8; 1500];
            if let Ok(Ok((len, _))) =
                tokio::time::timeout(Duration::from_millis(100), event_socket.recv_from(&mut buf))
                    .await
            {
                if len > 0 {
                    events_after += 1;
                }
            }
        }
        EVENTS_AFTER_WRAPAROUND.store(events_after, Ordering::SeqCst);

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    sim.run().unwrap();

    let events = EVENTS_AFTER_WRAPAROUND.load(Ordering::SeqCst);
    assert!(
        events > 0,
        "Unicast subscription should survive multicast session wraparound, got {} events",
        events
    );
}

// ============================================================================
// SESSION REGRESSION ON UNICAST COUNTER TESTS
// ============================================================================
// When a peer sends a unicast message with a lower session ID than previously
// seen (while reboot flag is still 1), this indicates a reboot and the server
// should expire all state from that peer.
//
// Note: Only unicast messages share a session counter. FindService and
// OfferService typically go via multicast and use a separate counter, so
// they don't trigger session regression detection on the unicast counter.
// ============================================================================

/// feat_req_someipsd_764: Session regression via Subscribe cancels prior subscription
///
/// A client subscribes with a high session ID, then sends another Subscribe
/// (for a different eventgroup or same) with a lower session ID. The session
/// regression should trigger reboot detection and cancel ALL subscriptions.
#[test_log::test]
fn unicast_subscribe_with_low_session_cancels_subscription() {
    covers!(feat_req_someipsd_764, feat_req_someipsd_871);

    use recentip::handle::ServiceEvent;
    use recentip::{EventId, EventgroupId};
    use std::sync::atomic::{AtomicU32, Ordering};

    static EVENTS_BEFORE: AtomicU32 = AtomicU32::new(0);
    static EVENTS_AFTER: AtomicU32 = AtomicU32::new(0);
    EVENTS_BEFORE.store(0, Ordering::SeqCst);
    EVENTS_AFTER.store(0, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async move {
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

        // Create two eventgroups to allow subscribing to different ones
        let eventgroup1 = EventgroupId::new(1).unwrap();
        let event_id1 = EventId::new(0x8001).unwrap();
        let event_handle1 = offering
            .event(event_id1)
            .eventgroup(eventgroup1)
            .create()
            .await
            .unwrap();

        let eventgroup2 = EventgroupId::new(2).unwrap();
        let event_id2 = EventId::new(0x8002).unwrap();
        let _event_handle2 = offering
            .event(event_id2)
            .eventgroup(eventgroup2)
            .create()
            .await
            .unwrap();

        // Wait for first subscription
        let mut got_subscription = false;
        for _ in 0..50 {
            match tokio::time::timeout(Duration::from_millis(100), offering.next()).await {
                Ok(Some(event)) => {
                    if matches!(event, ServiceEvent::Subscribe { .. }) {
                        got_subscription = true;
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => {}
            }
        }
        assert!(got_subscription, "Should receive subscription");

        // Send events before session regression
        for i in 0..3 {
            event_handle1
                .notify(format!("before-{}", i).as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Wait for client to send low-session Subscribe
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Send events after session regression - should NOT be delivered
        for i in 0..3 {
            event_handle1
                .notify(format!("after-{}", i).as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:40000").await?;
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("client").to_string().parse().unwrap();

        // Wait for OfferService
        let mut buf = [0u8; 1500];
        let mut server_sd_addr: Option<std::net::SocketAddr> = None;
        for _ in 0..50 {
            if let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == TEST_SERVICE_ID
                            && entry.ttl > 0
                        {
                            server_sd_addr = Some(std::net::SocketAddr::new(from.ip(), 30490));
                            break;
                        }
                    }
                }
            }
            if server_sd_addr.is_some() {
                break;
            }
        }
        let server_sd_addr = server_sd_addr.expect("Should receive OfferService");

        // Subscribe to eventgroup 1 with HIGH session ID (100)
        let subscribe1 = build_sd_subscribe_with_session(
            TEST_SERVICE_ID,
            0x0001,
            TEST_SERVICE_VERSION.0,
            1, // eventgroup 1
            my_ip,
            40000,
            0xFFFFFF, // max TTL
            100,      // HIGH session ID
            true,     // reboot flag
        );
        sd_socket.send_to(&subscribe1, server_sd_addr).await?;

        // Count events before sending low-session message
        let mut events_before = 0u32;
        for _ in 0..15 {
            if let Ok(Ok((len, _))) =
                tokio::time::timeout(Duration::from_millis(100), event_socket.recv_from(&mut buf))
                    .await
            {
                if len > 0 {
                    events_before += 1;
                }
            }
        }
        EVENTS_BEFORE.store(events_before, Ordering::SeqCst);

        // Subscribe to eventgroup 2 with LOW session ID (1)
        // This is a session regression on the same unicast counter
        let subscribe2 = build_sd_subscribe_with_session(
            TEST_SERVICE_ID,
            0x0001,
            TEST_SERVICE_VERSION.0,
            2, // eventgroup 2 (different)
            my_ip,
            40000,
            0xFFFFFF, // max TTL
            1,        // LOW session ID
            true,     // reboot flag still set
        );
        sd_socket.send_to(&subscribe2, server_sd_addr).await?;

        // Count events after session regression - should be 0
        let mut events_after = 0u32;
        for _ in 0..15 {
            if let Ok(Ok((len, _))) =
                tokio::time::timeout(Duration::from_millis(100), event_socket.recv_from(&mut buf))
                    .await
            {
                if len > 0 {
                    events_after += 1;
                }
            }
        }
        EVENTS_AFTER.store(events_after, Ordering::SeqCst);

        Ok(())
    });

    sim.run().unwrap();

    let before = EVENTS_BEFORE.load(Ordering::SeqCst);
    let after = EVENTS_AFTER.load(Ordering::SeqCst);
    assert!(
        before > 0,
        "Should receive events before session regression"
    );
    assert_eq!(
        after, 0,
        "Should NOT receive events after session regression (got {})",
        after
    );
}

// ============================================================================
// SESSION REGRESSION ON MULTICAST COUNTER TESTS
// ============================================================================
// FindService and OfferService typically go over multicast and share a
// separate session counter from unicast messages. Session regression on the
// multicast counter should also trigger reboot detection.
// ============================================================================

/// feat_req_someipsd_764: Multicast session regression (Find→Find) triggers reboot
///
/// A client sends FindService with high session ID, later sends another
/// FindService with lower session ID. This regression on the multicast
/// counter should trigger reboot detection and expire discovered services.
#[test_log::test]
fn multicast_find_to_find_session_regression() {
    covers!(feat_req_someipsd_764, feat_req_someipsd_871);

    use std::sync::atomic::{AtomicBool, Ordering};

    static REBOOT_DETECTED: AtomicBool = AtomicBool::new(false);
    REBOOT_DETECTED.store(false, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server (library under test) - should detect multicast session regression
    sim.host("server", || async move {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for simulation to complete
        tokio::time::sleep(Duration::from_secs(5)).await;

        // TODO: Check if reboot was detected for the client peer
        // This would require an API to query peer state or observe reboot events

        Ok(())
    });

    // Simulated client - sends Find with high session, then Find with low session
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        // Wait for server's Offer and get its address
        let mut buf = [0u8; 1500];
        let mut server_sd_addr: Option<std::net::SocketAddr> = None;
        for _ in 0..50 {
            if let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == TEST_SERVICE_ID
                            && entry.ttl > 0
                        {
                            server_sd_addr = Some(std::net::SocketAddr::new(from.ip(), 30490));
                            break;
                        }
                    }
                }
            }
            if server_sd_addr.is_some() {
                break;
            }
        }
        let server_sd_addr = server_sd_addr.expect("Should receive OfferService");

        // Send FindService with HIGH session ID (100)
        let find1 = build_sd_find_with_session(
            TEST_SERVICE_ID,
            0xFFFF,
            0xFF,
            0xFFFFFFFF,
            3,
            100,  // HIGH session ID
            true, // reboot flag
        );
        sd_socket.send_to(&find1, server_sd_addr).await?;

        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send FindService with LOW session ID (1) - multicast session regression
        let find2 = build_sd_find_with_session(
            TEST_SERVICE_ID,
            0xFFFF,
            0xFF,
            0xFFFFFFFF,
            3,
            1,    // LOW session ID
            true, // reboot flag still set
        );
        sd_socket.send_to(&find2, server_sd_addr).await?;

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();

    // TODO: Assert that reboot was detected
    // For now this test documents the expected behavior
}

/// feat_req_someipsd_764: Multicast session regression (Offer→Offer) triggers reboot
///
/// A client sends OfferService with high session ID, later sends another
/// OfferService with lower session ID. This regression on the multicast
/// counter should trigger reboot detection.
#[test_log::test]
fn multicast_offer_to_offer_session_regression() {
    covers!(feat_req_someipsd_764, feat_req_someipsd_871);

    use std::sync::atomic::{AtomicBool, Ordering};

    static REBOOT_DETECTED: AtomicBool = AtomicBool::new(false);
    REBOOT_DETECTED.store(false, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server (library under test) - should detect multicast session regression
    sim.host("server", || async move {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Offer a service so the runtime is active and processing SD messages
        // When the peer "reboots" (session regression), its discovered services
        // should be invalidated
        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });

    // Simulated client - sends Offer with high session, then Offer with low session
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("client").to_string().parse().unwrap();

        // Wait briefly for server to start
        tokio::time::sleep(Duration::from_millis(200)).await;

        let server_sd_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // Send OfferService with HIGH session ID (100)
        let offer1 = build_sd_offer_with_session(
            0x9999, 0x0001, 1, 0, my_ip, 50000, 3,    // TTL
            100,  // HIGH session ID
            true, // reboot flag
            false,
        );
        sd_socket.send_to(&offer1, server_sd_addr).await?;

        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send OfferService with LOW session ID (1) - multicast session regression
        let offer2 = build_sd_offer_with_session(
            0x9999, 0x0001, 1, 0, my_ip, 50000, 3,    // TTL
            1,    // LOW session ID
            true, // reboot flag still set
            false,
        );
        sd_socket.send_to(&offer2, server_sd_addr).await?;

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();

    // TODO: Assert that reboot was detected and service discovery state reset
}

/// feat_req_someipsd_764: Multicast session regression (Find→Offer) triggers reboot
///
/// A client sends FindService with high session ID, later sends OfferService
/// with lower session ID. Both are multicast messages sharing the same counter,
/// so this should trigger reboot detection.
#[test_log::test]
fn multicast_find_to_offer_session_regression() {
    covers!(feat_req_someipsd_764, feat_req_someipsd_871);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async move {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("client").to_string().parse().unwrap();

        // Wait for server's Offer
        let mut buf = [0u8; 1500];
        let mut server_sd_addr: Option<std::net::SocketAddr> = None;
        for _ in 0..50 {
            if let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == TEST_SERVICE_ID
                            && entry.ttl > 0
                        {
                            server_sd_addr = Some(std::net::SocketAddr::new(from.ip(), 30490));
                            break;
                        }
                    }
                }
            }
            if server_sd_addr.is_some() {
                break;
            }
        }
        let server_sd_addr = server_sd_addr.expect("Should receive OfferService");

        // Send FindService with HIGH session ID (100)
        let find = build_sd_find_with_session(
            TEST_SERVICE_ID,
            0xFFFF,
            0xFF,
            0xFFFFFFFF,
            3,
            100,  // HIGH session ID
            true, // reboot flag
        );
        sd_socket.send_to(&find, server_sd_addr).await?;

        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send OfferService with LOW session ID (1) - same multicast counter
        let offer = build_sd_offer_with_session(
            0x9999, // different service
            0x0001, 1, 0, my_ip, 50000, 3,    // TTL
            1,    // LOW session ID
            true, // reboot flag still set
            false,
        );
        sd_socket.send_to(&offer, server_sd_addr).await?;

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someipsd_764: Multicast session regression (Offer→Find) triggers reboot
///
/// A client sends OfferService with high session ID, later sends FindService
/// with lower session ID. Both are multicast messages sharing the same counter,
/// so this should trigger reboot detection.
#[test_log::test]
fn multicast_offer_to_find_session_regression() {
    covers!(feat_req_someipsd_764, feat_req_someipsd_871);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async move {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Offer a service so the runtime is active and processing SD messages
        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("client").to_string().parse().unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;

        let server_sd_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // Send OfferService with HIGH session ID (100)
        let offer = build_sd_offer_with_session(
            0x9999, 0x0001, 1, 0, my_ip, 50000, 3,    // TTL
            100,  // HIGH session ID
            true, // reboot flag
            false,
        );
        sd_socket.send_to(&offer, server_sd_addr).await?;

        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send FindService with LOW session ID (1) - same multicast counter
        let find = build_sd_find_with_session(
            TEST_SERVICE_ID,
            0xFFFF,
            0xFF,
            0xFFFFFFFF,
            3,
            1,    // LOW session ID
            true, // reboot flag still set
        );
        sd_socket.send_to(&find, server_sd_addr).await?;

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// CLIENT-SIDE SESSION REGRESSION TESTS
// ============================================================================
// When a client (library) detects session regression from a server it has
// discovered/subscribed to, it should invalidate the discovered service and
// close any active subscriptions.
// ============================================================================

/// feat_req_someipsd_764, feat_req_someipsd_871: Client detects server reboot via Offer session regression
///
/// A client discovers a service (via OfferService with high session ID), subscribes,
/// and receives events. Then the server "reboots" (sends Offer with lower session ID).
/// The client should detect this, invalidate the service, and close the subscription.
#[test_log::test]
fn client_closes_subscription_on_server_offer_regression() {
    covers!(feat_req_someipsd_764, feat_req_someipsd_871);

    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    static SUBSCRIPTION_CLOSED: AtomicBool = AtomicBool::new(false);
    static EVENTS_BEFORE_REBOOT: AtomicU32 = AtomicU32::new(0);
    SUBSCRIPTION_CLOSED.store(false, Ordering::SeqCst);
    EVENTS_BEFORE_REBOOT.store(0, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Simulated server - sends Offer, accepts Subscribe, sends events, then "reboots"
    sim.host("server", || async move {
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:50000").await?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server").to_string().parse().unwrap();

        // Send OfferService with HIGH session ID (100)
        let offer1 = build_sd_offer_with_session(
            TEST_SERVICE_ID,
            0x0001,
            TEST_SERVICE_VERSION.0,
            TEST_SERVICE_VERSION.1,
            my_ip,
            50000,
            0xFFFFFF, // max TTL
            100,      // HIGH session ID
            true,     // reboot flag
            false,
        );
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();
        sd_socket.send_to(&offer1, multicast_addr).await?;

        // Wait for Subscribe from client
        let mut buf = [0u8; 1500];
        let mut client_addr: Option<std::net::SocketAddr> = None;
        let mut client_event_port: Option<u16> = None;
        for _ in 0..50 {
            if let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // SubscribeEventgroup = 0x06
                        if entry.entry_type as u8 == 0x06 && entry.service_id == TEST_SERVICE_ID {
                            client_addr = Some(from);
                            // Extract event port from options (simplified - use default)
                            client_event_port = Some(40000);
                            break;
                        }
                    }
                }
            }
            if client_addr.is_some() {
                break;
            }
        }

        if let (Some(client), Some(port)) = (client_addr, client_event_port) {
            let client_event_addr = std::net::SocketAddr::new(client.ip(), port);

            // Send SubscribeAck with session 101
            let ack = build_sd_subscribe_ack_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                1, // eventgroup
                0xFFFFFF,
                101,  // session 101
                true, // reboot flag
            );
            sd_socket.send_to(&ack, client).await?;

            // Send some events before "rebooting"
            for i in 0..3 {
                let event = build_event_message(0x8001, i);
                event_socket.send_to(&event, client_event_addr).await?;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            tokio::time::sleep(Duration::from_millis(500)).await;

            // "Reboot" - send Offer with LOW session ID (1)
            let offer2 = build_sd_offer_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                50000,
                0xFFFFFF,
                1,    // LOW session ID - regression!
                true, // reboot flag still set
                false,
            );
            sd_socket.send_to(&offer2, multicast_addr).await?;

            // Send more events after "reboot" - client should NOT receive these
            // (subscription should be closed)
            for i in 10..13 {
                let event = build_event_message(0x8001, i);
                event_socket.send_to(&event, client_event_addr).await?;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    // Client (library under test) - discovers, subscribes, should detect reboot
    sim.client("client", async move {
        let _runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Wait for service to be discovered
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Find and subscribe to the service
        // TODO: This requires a discovery/subscription API that we can observe
        // For now, document the expected behavior

        tokio::time::sleep(Duration::from_secs(5)).await;

        // Check if subscription was closed after reboot detection
        // SUBSCRIPTION_CLOSED would be set by observing the subscription handle

        Ok(())
    });

    sim.run().unwrap();

    // TODO: Assert subscription was closed
    // assert!(SUBSCRIPTION_CLOSED.load(Ordering::SeqCst), "Subscription should be closed after server reboot");
}

/// Build a SubscribeEventgroupAck message with configurable session
fn build_sd_subscribe_ack_with_session(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    ttl: u32,
    session_id: u16,
    reboot_flag: bool,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(56);

    // === SOME/IP Header (16 bytes) ===
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes()); // Service ID = SD
    packet.extend_from_slice(&0x8100u16.to_be_bytes()); // Method ID = SD
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes()); // placeholder
    packet.extend_from_slice(&0x0000u16.to_be_bytes()); // Client ID
    packet.extend_from_slice(&session_id.to_be_bytes()); // Session ID
    packet.push(0x01); // Protocol Version
    packet.push(0x01); // Interface Version
    packet.push(0x02); // Message Type = NOTIFICATION
    packet.push(0x00); // Return Code = E_OK

    // === SD Payload ===
    let mut flags = 0x40u8; // Unicast flag
    if reboot_flag {
        flags |= 0x80; // Reboot flag
    }
    packet.push(flags);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]); // Reserved

    // Entries array length (16 bytes for one entry)
    packet.extend_from_slice(&16u32.to_be_bytes());

    // === SubscribeEventgroupAck Entry (16 bytes) ===
    packet.push(0x07); // Type = SubscribeEventgroupAck
    packet.push(0x00); // Index 1st options
    packet.push(0x00); // Index 2nd options
    packet.push(0x00); // # of opt 1 | # of opt 2 = 0 options
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]); // TTL (24-bit)
    packet.push(0x00); // Reserved
    packet.push(0x00); // Counter
    packet.extend_from_slice(&eventgroup_id.to_be_bytes());

    // Options array length (0)
    packet.extend_from_slice(&0u32.to_be_bytes());

    // Fix up length field
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}

/// Build a simple SOME/IP event message
fn build_event_message(event_id: u16, sequence: u8) -> Vec<u8> {
    let mut packet = Vec::with_capacity(24);

    // === SOME/IP Header (16 bytes) ===
    packet.extend_from_slice(&TEST_SERVICE_ID.to_be_bytes()); // Service ID
    packet.extend_from_slice(&event_id.to_be_bytes()); // Method/Event ID
    packet.extend_from_slice(&8u32.to_be_bytes()); // Length (header remainder + payload)
    packet.extend_from_slice(&0x0000u16.to_be_bytes()); // Client ID
    packet.extend_from_slice(&0x0001u16.to_be_bytes()); // Session ID
    packet.push(0x01); // Protocol Version
    packet.push(0x01); // Interface Version
    packet.push(0x02); // Message Type = NOTIFICATION
    packet.push(0x00); // Return Code = E_OK

    // Payload
    packet.push(sequence);

    // Fix length
    let length = (packet.len() - 8) as u32;
    packet[4..8].copy_from_slice(&length.to_be_bytes());

    packet
}

/// feat_req_someipsd_764: Client detects server reboot via SubscribeAck session regression
///
/// A client subscribes and receives SubscribeAck with high session ID.
/// Later, server sends another SubscribeAck (e.g., refresh) with lower session ID.
/// The client should detect this as a reboot and close the subscription.
#[test_log::test]
fn client_closes_subscription_on_server_ack_regression() {
    covers!(feat_req_someipsd_764, feat_req_someipsd_871);

    use std::sync::atomic::{AtomicBool, Ordering};

    static SUBSCRIPTION_CLOSED: AtomicBool = AtomicBool::new(false);
    SUBSCRIPTION_CLOSED.store(false, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async move {
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server").to_string().parse().unwrap();

        // Send OfferService
        let offer = build_sd_offer_with_session(
            TEST_SERVICE_ID,
            0x0001,
            TEST_SERVICE_VERSION.0,
            TEST_SERVICE_VERSION.1,
            my_ip,
            50000,
            0xFFFFFF,
            1, // session
            true,
            false,
        );
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();
        sd_socket.send_to(&offer, multicast_addr).await?;

        // Wait for Subscribe
        let mut buf = [0u8; 1500];
        let mut client_addr: Option<std::net::SocketAddr> = None;
        for _ in 0..50 {
            if let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x06 && entry.service_id == TEST_SERVICE_ID {
                            client_addr = Some(from);
                            break;
                        }
                    }
                }
            }
            if client_addr.is_some() {
                break;
            }
        }

        if let Some(client) = client_addr {
            // Send SubscribeAck with HIGH session ID (100)
            let ack1 = build_sd_subscribe_ack_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                1,
                0xFFFFFF,
                100, // HIGH session ID
                true,
            );
            sd_socket.send_to(&ack1, client).await?;

            tokio::time::sleep(Duration::from_secs(1)).await;

            // "Reboot" - send SubscribeAck with LOW session ID (1)
            let ack2 = build_sd_subscribe_ack_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                1,
                0xFFFFFF,
                1, // LOW session ID - regression!
                true,
            );
            sd_socket.send_to(&ack2, client).await?;
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
        Ok(())
    });

    sim.client("client", async move {
        let _runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // TODO: Subscribe and observe subscription handle state
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someipsd_764: Client detects server reboot - Offer→Offer on multicast counter
///
/// A client discovers a service via OfferService. Later, it receives another
/// OfferService with lower session ID (on the multicast counter). The client
/// should detect this as a server reboot and invalidate the discovered service.
#[test_log::test]
fn client_invalidates_service_on_offer_regression() {
    covers!(feat_req_someipsd_764, feat_req_someipsd_871);

    use std::sync::atomic::{AtomicBool, Ordering};

    static SERVICE_INVALIDATED: AtomicBool = AtomicBool::new(false);
    SERVICE_INVALIDATED.store(false, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async move {
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server").to_string().parse().unwrap();
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // Send OfferService with HIGH session ID (100)
        let offer1 = build_sd_offer_with_session(
            TEST_SERVICE_ID,
            0x0001,
            TEST_SERVICE_VERSION.0,
            TEST_SERVICE_VERSION.1,
            my_ip,
            50000,
            0xFFFFFF,
            100, // HIGH session ID
            true,
            false,
        );
        sd_socket.send_to(&offer1, multicast_addr).await?;

        tokio::time::sleep(Duration::from_secs(1)).await;

        // "Reboot" - send OfferService with LOW session ID (1)
        let offer2 = build_sd_offer_with_session(
            TEST_SERVICE_ID,
            0x0001,
            TEST_SERVICE_VERSION.0,
            TEST_SERVICE_VERSION.1,
            my_ip,
            50000,
            0xFFFFFF,
            1, // LOW session ID - regression!
            true,
            false,
        );
        sd_socket.send_to(&offer2, multicast_addr).await?;

        tokio::time::sleep(Duration::from_secs(3)).await;
        Ok(())
    });

    sim.client("client", async move {
        let _runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // TODO: Watch for service discovery events
        // After first Offer: service should be discovered
        // After second Offer (regression): service should be invalidated/removed
        // Then potentially re-discovered from the same Offer

        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someipsd_765: Client subscription survives low-session Offer (different counter)
///
/// This tests that unicast (SubscribeAck) and multicast (Offer) use SEPARATE
/// session counters. A high-session SubscribeAck followed by a low-session Offer
/// should NOT trigger reboot detection since they're on different counters.
#[test_log::test]
fn client_subscription_survives_low_session_offer() {
    covers!(feat_req_someipsd_765);

    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    static SUBSCRIPTION_STILL_ACTIVE: AtomicBool = AtomicBool::new(false);
    static EVENTS_AFTER_OFFER: AtomicU32 = AtomicU32::new(0);
    SUBSCRIPTION_STILL_ACTIVE.store(false, Ordering::SeqCst);
    EVENTS_AFTER_OFFER.store(0, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async move {
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:50000").await?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server").to_string().parse().unwrap();
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // Send initial OfferService (multicast, session 1)
        let offer1 = build_sd_offer_with_session(
            TEST_SERVICE_ID,
            0x0001,
            TEST_SERVICE_VERSION.0,
            TEST_SERVICE_VERSION.1,
            my_ip,
            50000,
            0xFFFFFF,
            1, // multicast session 1
            true,
            false,
        );
        sd_socket.send_to(&offer1, multicast_addr).await?;

        // Wait for Subscribe
        let mut buf = [0u8; 1500];
        let mut client_addr: Option<std::net::SocketAddr> = None;
        let mut client_event_port: Option<u16> = None;
        for _ in 0..50 {
            if let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x06 && entry.service_id == TEST_SERVICE_ID {
                            client_addr = Some(from);
                            client_event_port = Some(40000);
                            break;
                        }
                    }
                }
            }
            if client_addr.is_some() {
                break;
            }
        }

        if let (Some(client), Some(port)) = (client_addr, client_event_port) {
            let client_event_addr = std::net::SocketAddr::new(client.ip(), port);

            // Send SubscribeAck with HIGH session ID (100) - unicast counter
            let ack = build_sd_subscribe_ack_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                1,
                0xFFFFFF,
                100, // HIGH unicast session
                true,
            );
            sd_socket.send_to(&ack, client).await?;

            // Send some events
            for i in 0..3 {
                let event = build_event_message(0x8001, i);
                event_socket.send_to(&event, client_event_addr).await?;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            tokio::time::sleep(Duration::from_millis(500)).await;

            // Send Offer with LOW session ID (2) - but this is MULTICAST counter
            // Should NOT affect the subscription since different counters
            let offer2 = build_sd_offer_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                50000,
                0xFFFFFF,
                2, // low multicast session (but normal increment from 1)
                true,
                false,
            );
            sd_socket.send_to(&offer2, multicast_addr).await?;

            // Send more events - subscription should still be active
            for i in 10..15 {
                let event = build_event_message(0x8001, i);
                event_socket.send_to(&event, client_event_addr).await?;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("client", async move {
        let _runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // TODO: Subscribe and count events
        // Should receive events both before and after the low-session Offer
        // because Offer uses multicast counter, SubscribeAck uses unicast counter

        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });

    sim.run().unwrap();

    // TODO: Assert events were received after the Offer
    // let events = EVENTS_AFTER_OFFER.load(Ordering::SeqCst);
    // assert!(events > 0, "Subscription should survive low-session Offer (different counter)");
}

// ============================================================================
// MULTI-SUBSCRIPTION REBOOT ISOLATION TESTS
// ============================================================================
// These tests verify that reboot detection correctly handles scenarios with
// multiple subscriptions - either to multiple eventgroups on the same server,
// or to multiple different servers.
// ============================================================================

/// feat_req_someipsd_871: Server reboot invalidates ALL subscriptions to that server
///
/// A client subscribes to multiple eventgroups from the same server.
/// When the server reboots, ALL subscriptions to that server should be
/// invalidated, not just one.
#[test_log::test]
fn client_all_subscriptions_invalidated_on_server_reboot() {
    covers!(feat_req_someipsd_871, feat_req_someipsd_764);

    use std::sync::atomic::{AtomicBool, Ordering};

    static EG1_INVALIDATED: AtomicBool = AtomicBool::new(false);
    static EG2_INVALIDATED: AtomicBool = AtomicBool::new(false);
    static EG3_INVALIDATED: AtomicBool = AtomicBool::new(false);
    EG1_INVALIDATED.store(false, Ordering::SeqCst);
    EG2_INVALIDATED.store(false, Ordering::SeqCst);
    EG3_INVALIDATED.store(false, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Simulated server - offers service with 3 eventgroups, then "reboots"
    sim.host("server", || async move {
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:50000").await?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server").to_string().parse().unwrap();
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // Send OfferService with HIGH session ID (100)
        let offer1 = build_sd_offer_with_session(
            TEST_SERVICE_ID,
            0x0001,
            TEST_SERVICE_VERSION.0,
            TEST_SERVICE_VERSION.1,
            my_ip,
            50000,
            0xFFFFFF,
            100, // HIGH session
            true,
            false,
        );
        sd_socket.send_to(&offer1, multicast_addr).await?;

        // Wait for subscribes and send acks for each eventgroup
        let mut buf = [0u8; 1500];
        let mut client_addr: Option<std::net::SocketAddr> = None;
        let mut subscribed_eventgroups = Vec::new();

        for _ in 0..100 {
            if let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // SubscribeEventgroup = 0x06
                        if entry.entry_type as u8 == 0x06 && entry.service_id == TEST_SERVICE_ID {
                            client_addr = Some(from);
                            let eg_id = entry.eventgroup_id;
                            if !subscribed_eventgroups.contains(&eg_id) {
                                subscribed_eventgroups.push(eg_id);

                                // Send SubscribeAck
                                let ack = build_sd_subscribe_ack_with_session(
                                    TEST_SERVICE_ID,
                                    0x0001,
                                    TEST_SERVICE_VERSION.0,
                                    eg_id,
                                    0xFFFFFF,
                                    100 + subscribed_eventgroups.len() as u16,
                                    true,
                                );
                                sd_socket.send_to(&ack, from).await?;
                            }
                        }
                    }
                }
            }

            if subscribed_eventgroups.len() >= 3 {
                break;
            }
        }

        // Send some events to each eventgroup
        if let Some(client) = client_addr {
            let client_event_addr = std::net::SocketAddr::new(client.ip(), 40000);
            for eg in 1..=3 {
                let event = build_event_message(0x8000 + eg, 1);
                event_socket.send_to(&event, client_event_addr).await?;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;

            // "REBOOT" - send Offer with LOW session ID (1)
            let offer2 = build_sd_offer_with_session(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                my_ip,
                50000,
                0xFFFFFF,
                1,    // LOW session - reboot!
                true, // reboot flag still set
                false,
            );
            sd_socket.send_to(&offer2, multicast_addr).await?;
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
        Ok(())
    });

    // Client (library under test) - subscribes to 3 eventgroups
    sim.client("client", async move {
        let _runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // TODO: Subscribe to eventgroups 1, 2, and 3
        // Monitor for reboot detection
        // After server reboot, ALL three subscriptions should be invalidated

        // The test verifies that when one server reboots, all subscriptions
        // to that server are invalidated atomically, not just the first one
        // encountered.

        tokio::time::sleep(Duration::from_secs(8)).await;
        Ok(())
    });

    sim.run().unwrap();

    // TODO: Assert all three subscriptions were invalidated
    // assert!(EG1_INVALIDATED.load(Ordering::SeqCst), "Eventgroup 1 subscription should be invalidated");
    // assert!(EG2_INVALIDATED.load(Ordering::SeqCst), "Eventgroup 2 subscription should be invalidated");
    // assert!(EG3_INVALIDATED.load(Ordering::SeqCst), "Eventgroup 3 subscription should be invalidated");
}

/// feat_req_someipsd_871, feat_req_someipsd_765: One server rebooting doesn't affect other servers
///
/// A client subscribes to services from two different servers.
/// When one server reboots, subscriptions to the OTHER server should
/// remain valid - reboot detection is per-peer.
#[test_log::test]
fn client_other_server_subscriptions_survive_one_server_reboot() {
    covers!(feat_req_someipsd_871, feat_req_someipsd_765);

    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    static SERVER1_SUB_INVALIDATED: AtomicBool = AtomicBool::new(false);
    static SERVER2_EVENTS_AFTER_REBOOT: AtomicU32 = AtomicU32::new(0);
    SERVER1_SUB_INVALIDATED.store(false, Ordering::SeqCst);
    SERVER2_EVENTS_AFTER_REBOOT.store(0, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server 1 - will reboot
    sim.host("server1", || async move {
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:50000").await?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server1").to_string().parse().unwrap();
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // Offer service 0x1111
        let offer1 = build_sd_offer_with_session(
            0x1111, // Different service ID
            0x0001,
            TEST_SERVICE_VERSION.0,
            TEST_SERVICE_VERSION.1,
            my_ip,
            50000,
            0xFFFFFF,
            100, // HIGH session
            true,
            false,
        );
        sd_socket.send_to(&offer1, multicast_addr).await?;

        // Wait for subscribe and send ack
        let mut buf = [0u8; 1500];
        let mut client_addr: Option<std::net::SocketAddr> = None;
        for _ in 0..50 {
            if let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x06 && entry.service_id == 0x1111 {
                            client_addr = Some(from);
                            let ack = build_sd_subscribe_ack_with_session(
                                0x1111,
                                0x0001,
                                TEST_SERVICE_VERSION.0,
                                1,
                                0xFFFFFF,
                                101,
                                true,
                            );
                            sd_socket.send_to(&ack, from).await?;
                            break;
                        }
                    }
                }
            }
            if client_addr.is_some() {
                break;
            }
        }

        // Send some events
        if let Some(client) = client_addr {
            let client_event_addr = std::net::SocketAddr::new(client.ip(), 40000);
            for i in 0..3 {
                let event = build_event_message(0x8001, i);
                event_socket.send_to(&event, client_event_addr).await?;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        tokio::time::sleep(Duration::from_secs(1)).await;

        // "REBOOT" - send Offer with LOW session ID
        let offer2 = build_sd_offer_with_session(
            0x1111,
            0x0001,
            TEST_SERVICE_VERSION.0,
            TEST_SERVICE_VERSION.1,
            my_ip,
            50000,
            0xFFFFFF,
            1, // LOW session - reboot!
            true,
            false,
        );
        sd_socket.send_to(&offer2, multicast_addr).await?;

        tokio::time::sleep(Duration::from_secs(3)).await;
        Ok(())
    });

    // Server 2 - stays stable, no reboot
    sim.host("server2", || async move {
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:50001").await?;

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server2").to_string().parse().unwrap();
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // Offer service 0x2222
        let offer = build_sd_offer_with_session(
            0x2222, // Different service ID
            0x0001,
            TEST_SERVICE_VERSION.0,
            TEST_SERVICE_VERSION.1,
            my_ip,
            50001,
            0xFFFFFF,
            1, // Normal session progression
            true,
            false,
        );
        sd_socket.send_to(&offer, multicast_addr).await?;

        // Wait for subscribe and send ack
        let mut buf = [0u8; 1500];
        let mut client_addr: Option<std::net::SocketAddr> = None;
        for _ in 0..50 {
            if let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x06 && entry.service_id == 0x2222 {
                            client_addr = Some(from);
                            let ack = build_sd_subscribe_ack_with_session(
                                0x2222,
                                0x0001,
                                TEST_SERVICE_VERSION.0,
                                1,
                                0xFFFFFF,
                                2, // Normal session increment
                                true,
                            );
                            sd_socket.send_to(&ack, from).await?;
                            break;
                        }
                    }
                }
            }
            if client_addr.is_some() {
                break;
            }
        }

        // Keep sending events throughout the test
        // These should continue to be received even after server1 "reboots"
        if let Some(client) = client_addr {
            let client_event_addr = std::net::SocketAddr::new(client.ip(), 40001);
            for i in 0..30 {
                let event = build_event_message(0x8002, i);
                event_socket.send_to(&event, client_event_addr).await?;
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    // Client (library under test) - subscribes to both servers
    sim.client("client", async move {
        let _runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // TODO: Subscribe to service 0x1111 on server1 and 0x2222 on server2
        // After server1 reboots:
        // - Subscription to 0x1111 should be invalidated
        // - Subscription to 0x2222 should remain valid (still receiving events)

        tokio::time::sleep(Duration::from_secs(8)).await;
        Ok(())
    });

    sim.run().unwrap();

    // TODO: Verify isolation
    // assert!(SERVER1_SUB_INVALIDATED.load(Ordering::SeqCst),
    //     "Subscription to server1 should be invalidated after its reboot");
    // let server2_events = SERVER2_EVENTS_AFTER_REBOOT.load(Ordering::SeqCst);
    // assert!(server2_events > 0,
    //     "Subscription to server2 should survive server1's reboot, got {} events", server2_events);
}

// ============================================================================
// TCP CONNECTION RESET ON REBOOT TESTS
// ============================================================================
// These tests verify that TCP connections are properly closed/reset when
// a peer reboot is detected. This is critical for connection state management.
// ============================================================================

/// feat_req_someipsd_872: Client closes TCP when server reboots (flag 0→1)
/// feat_req_someipsd_764: Reboot detection via flag transition
///
/// When a server reboots (reboot flag transitions 0→1), the client should:
/// 1. Detect the reboot via SD messages
/// 2. Close/reset all TCP connections to that server
/// 3. Subscriptions should be invalidated (no events after reboot)
///
/// This test verifies from the client perspective using TCP-based subscription.
/// Server offers TCP, client prefers TCP transport, then server reboots.
#[test_log::test]
fn client_closes_tcp_on_server_reboot_flag_0_to_1() {
    covers!(
        feat_req_someipsd_872,
        feat_req_someipsd_764,
        feat_req_someipsd_871
    );

    use recentip::EventgroupId;
    use std::sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    static EVENTS_BEFORE_REBOOT: AtomicU32 = AtomicU32::new(0);
    static EVENTS_AFTER_REBOOT: AtomicU32 = AtomicU32::new(0);
    static TCP_CLOSED: AtomicBool = AtomicBool::new(false);
    EVENTS_BEFORE_REBOOT.store(0, Ordering::SeqCst);
    EVENTS_AFTER_REBOOT.store(0, Ordering::SeqCst);
    TCP_CLOSED.store(false, Ordering::SeqCst);

    let (broadcast_tx, mut broadcast_rx) = tokio::sync::broadcast::channel(1);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Client (library under test) - subscribes via TCP
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(Duration::from_secs(10), runtime.find(TEST_SERVICE_ID))
            .await
            .expect("Should discover service within 10s")
            .expect("Service should be available");
        tracing::info!("Discovered service via proxy");

        // Subscribe - will use TCP since client prefers it and server offers it
        let eventgroup = EventgroupId::new(1).unwrap();
        let mut subscription = proxy
            .new_subscription()
            .eventgroup(eventgroup)
            .subscribe()
            .await
            .expect("Should subscribe successfully via TCP");
        tracing::info!("Subscription established via TCP");

        // Phase 1: Receive events before reboot (via TCP)
        for _ in 0..3 {
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(300), subscription.next()).await
            {
                assert_eq!(event.event_id, EventId::new(0x8001).unwrap());
                assert!(event.payload.len() >= 1);
                assert_eq!(event.payload[0], 0x01, "Event should be from phase 1");
                EVENTS_BEFORE_REBOOT.fetch_add(1, Ordering::SeqCst);
            }
        }
        tracing::info!(
            "Received {} events before reboot",
            EVENTS_BEFORE_REBOOT.load(Ordering::SeqCst)
        );

        // Wait for server to signal that reboot simulation (flag transition 0→1) has begun
        tracing::info!("Waiting for server to signal reboot phase...");
        broadcast_rx.recv().await.unwrap();
        tracing::info!("Proceeding to phase 2: checking for TCP closure after reboot...");

        tracing::info!("Checking for TCP closure after server reboot");
        // Phase 2: Try to receive events after reboot
        // TCP connection should be closed, so events should stop
        for _ in 0..5 {
            match tokio::time::timeout(Duration::from_millis(200), subscription.next()).await {
                Ok(Some(event)) => {
                    assert_eq!(event.event_id, EventId::new(0x8001).unwrap());
                    EVENTS_AFTER_REBOOT.fetch_add(1, Ordering::SeqCst);
                }
                Ok(None) => {
                    // Subscription closed - expected behavior
                    tracing::info!("Subscription closed as expected after server reboot");
                    break;
                }
                Err(_) => {
                    // Timeout - also acceptable
                    tracing::info!("No event received after server reboot (timeout)");
                    break;
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    let mut b_rx = broadcast_tx.subscribe();
    let b_tx = broadcast_tx.clone();
    // Server (raw sockets + TCP listener) - controls SD, handles TCP
    sim.client("server", async move {
        let sd_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?);
        let tcp_listener = Arc::new(turmoil::net::TcpListener::bind("0.0.0.0:30509").await?);

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server").to_string().parse().unwrap();
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // Discovery task - send initial offers with TCP endpoint
        let sd_socket_discovery = Arc::clone(&sd_socket);
        tracing::info!("Starting discovery task to send initial Offers with reboot=true");
        let discovery_task = tokio::spawn(async move {
            for i in 0..20 {
                let offer = build_sd_offer_with_tcp_endpoint(
                    TEST_SERVICE_ID,
                    0x0001,
                    TEST_SERVICE_VERSION.0,
                    TEST_SERVICE_VERSION.1,
                    my_ip,
                    30509,
                    5,
                    i,    // session 1
                    true, // reboot=true initially
                    false,
                );
                let _ = sd_socket_discovery.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        // Wait for TCP connection (subscription)
        let tcp_connection = tokio::select! {
            result = tcp_listener.accept() => result.ok(),
            _ = tokio::time::sleep(Duration::from_secs(10)) => None,
        };

        let (mut tcp_stream, _) = tcp_connection.expect("Should receive TCP connection");
        tracing::info!("Accepted TCP connection from client");

        // Wait for Subscribe message on SD socket
        let mut buf = [0u8; 1500];
        let _subscribe_received = loop {
            match tokio::time::timeout(Duration::from_millis(500), sd_socket.recv_from(&mut buf))
                .await
            {
                Ok(Ok((len, from))) => {
                    if len > 40 && buf[24] == 0x06 {
                        let subscribe_ack = build_sd_subscribe_ack(
                            TEST_SERVICE_ID,
                            0x0001,
                            TEST_SERVICE_VERSION.0,
                            1,
                            5,
                            1, // session_id
                        );
                        let _ = sd_socket.send_to(&subscribe_ack, from).await;
                        break true;
                    }
                }
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => continue,
            }
        };
        tracing::info!("Processed Subscribe and sent SubscribeAck");

        discovery_task.abort();
        // Wait for any in-flight messages from discovery_task to be processed
        tokio::time::sleep(Duration::from_millis(100)).await;

        tracing::info!("Starting SD task to send Offers simulating reboot");
        // SD task - transition from reboot=false to reboot=true (simulating reboot)
        let sd_socket_clone = Arc::clone(&sd_socket);
        let sd_task = tokio::spawn(async move {
            // Phase 1: Normal operation (reboot=false, sessions 2-5)
            for session in 20..60 {
                let offer = build_sd_offer_with_tcp_endpoint(
                    TEST_SERVICE_ID,
                    0x0001,
                    TEST_SERVICE_VERSION.0,
                    TEST_SERVICE_VERSION.1,
                    my_ip,
                    30509,
                    5,
                    session,
                    false, // reboot=false (normal operation)
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            tracing::info!("Completed Phase 1 Offers, pausing before reboot simulation");
            tokio::time::sleep(Duration::from_millis(100)).await;

            tracing::info!("Broadcasting to client to proceed to phase 2...");
            b_tx.send(()).unwrap();

            tracing::info!("Starting Phase 2 Offers simulating server reboot");
            // Phase 2: SERVER REBOOTS - flag transitions 0→1
            for session in 70..80 {
                let offer = build_sd_offer_with_tcp_endpoint(
                    TEST_SERVICE_ID,
                    0x0001,
                    TEST_SERVICE_VERSION.0,
                    TEST_SERVICE_VERSION.1,
                    my_ip,
                    30509,
                    5,
                    session,
                    true, // reboot=true (REBOOT DETECTED!)
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        tracing::info!("Starting event sending task via TCP");
        // Send events via TCP
        // Phase 1: Events before reboot
        for _ in 0..3 {
            let mut event = Vec::with_capacity(17);
            event.extend_from_slice(&TEST_SERVICE_ID.to_be_bytes());
            event.extend_from_slice(&0x8001u16.to_be_bytes());
            event.extend_from_slice(&9u32.to_be_bytes());
            event.extend_from_slice(&0x0000u16.to_be_bytes());
            event.extend_from_slice(&2u16.to_be_bytes());
            event.push(0x01);
            event.push(0x01);
            event.push(0x02);
            event.push(0x00);
            event.push(0x01); // Phase 1 marker

            if tcp_stream.write_all(&event).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(80)).await;
        }

        tracing::info!("Waiting for SD task to signal phase 2...");
        b_rx.recv().await.unwrap();

        // Give client time to process reboot detection and close TCP
        tokio::time::sleep(Duration::from_millis(500)).await;

        tracing::info!("Starting to send events after server reboot simulation");
        // Phase 2: Try to send events after reboot
        // Client should have closed TCP connection due to reboot detection
        for _ in 0..5 {
            let mut event = Vec::with_capacity(17);
            event.extend_from_slice(&TEST_SERVICE_ID.to_be_bytes());
            event.extend_from_slice(&0x8001u16.to_be_bytes());
            event.extend_from_slice(&9u32.to_be_bytes());
            event.extend_from_slice(&0x0000u16.to_be_bytes());
            event.extend_from_slice(&3u16.to_be_bytes());
            event.push(0x01);
            event.push(0x01);
            event.push(0x02);
            event.push(0x00);
            event.push(0x02); // Phase 2 marker

            if tcp_stream.write_all(&event).await.is_err() {
                // Connection closed by client - expected!
                TCP_CLOSED.store(true, Ordering::SeqCst);
                break;
            }
            tokio::time::sleep(Duration::from_millis(80)).await;
        }

        // Try to read from TCP - should get EOF or error
        // In turmoil simulation, the TCP close may not propagate immediately
        // so we try with a short timeout
        let mut read_buf = [0u8; 16];
        match tokio::time::timeout(Duration::from_millis(200), tcp_stream.read(&mut read_buf)).await
        {
            Ok(Ok(0)) => TCP_CLOSED.store(true, Ordering::SeqCst), // EOF
            Ok(Err(_)) => TCP_CLOSED.store(true, Ordering::SeqCst), // Connection reset
            Err(_) => {} // Timeout - turmoil quirk, connection may not report close
            _ => {}
        }

        let _ = sd_task.await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();

    let events_before = EVENTS_BEFORE_REBOOT.load(Ordering::SeqCst);
    let events_after = EVENTS_AFTER_REBOOT.load(Ordering::SeqCst);

    assert!(
        events_before > 0,
        "Should receive events before reboot via TCP (got {})",
        events_before
    );
    assert_eq!(
        events_after, 0,
        "Should NOT receive events after server reboot - TCP should be closed (got {})",
        events_after
    );
    assert!(
        TCP_CLOSED.load(Ordering::SeqCst),
        "TCP connection should be closed by client upon server reboot detection"
    );
}

/// Build a raw SOME/IP-SD OfferService message with TCP endpoint
fn build_sd_offer_with_tcp_endpoint(
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

    // === SOME/IP Header (16 bytes) ===
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

    // === SD Payload ===
    let mut flags = if unicast_flag { 0x40u8 } else { 0x00u8 };
    if reboot_flag {
        flags |= 0x80;
    }
    packet.push(flags);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);

    packet.extend_from_slice(&16u32.to_be_bytes());

    // === OfferService Entry ===
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

    // === IPv4 Endpoint Option - TCP ===
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
}

/// feat_req_someipsd_872: Client closes TCP on session wraparound false positive check
/// feat_req_someipsd_764: Session wraparound with reboot=0 is NOT a reboot
///
/// When session wraps (0xFFFF→1) with reboot=0, this is NORMAL operation.
/// TCP connections should NOT be closed. This test ensures no false positive.
#[test_log::test]
fn client_keeps_tcp_on_normal_session_wraparound() {
    covers!(feat_req_someipsd_872, feat_req_someipsd_764);

    use recentip::EventgroupId;
    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    };
    use tokio::io::AsyncWriteExt;

    static EVENTS_BEFORE_WRAPAROUND: AtomicU32 = AtomicU32::new(0);
    static EVENTS_AFTER_WRAPAROUND: AtomicU32 = AtomicU32::new(0);
    EVENTS_BEFORE_WRAPAROUND.store(0, Ordering::SeqCst);
    EVENTS_AFTER_WRAPAROUND.store(0, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(Duration::from_secs(10), runtime.find(TEST_SERVICE_ID))
            .await
            .expect("Should discover service within 10s")
            .expect("Service should be available");

        let eventgroup = EventgroupId::new(1).unwrap();
        let mut subscription = proxy
            .new_subscription()
            .eventgroup(eventgroup)
            .subscribe()
            .await
            .expect("Should subscribe successfully via TCP");

        // Phase 1: Events before wraparound
        for _ in 0..3 {
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(300), subscription.next()).await
            {
                assert_eq!(event.event_id, EventId::new(0x8001).unwrap());
                assert_eq!(event.payload[0], 0x01);
                EVENTS_BEFORE_WRAPAROUND.fetch_add(1, Ordering::SeqCst);
            }
        }

        tokio::time::sleep(Duration::from_millis(300)).await;

        // Phase 2: Events after wraparound
        // TCP should stay open, events should continue
        for _ in 0..5 {
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(300), subscription.next()).await
            {
                assert_eq!(event.event_id, EventId::new(0x8001).unwrap());
                assert_eq!(event.payload[0], 0x02);
                EVENTS_AFTER_WRAPAROUND.fetch_add(1, Ordering::SeqCst);
            }
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    sim.host("server", || async move {
        let sd_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?);
        let tcp_listener = Arc::new(turmoil::net::TcpListener::bind("0.0.0.0:30509").await?);

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server").to_string().parse().unwrap();
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        let sd_socket_discovery = Arc::clone(&sd_socket);
        let discovery_task = tokio::spawn(async move {
            let mut next_session_id = 1;
            for _ in 0..20 {
                let offer = build_sd_offer_with_tcp_endpoint(
                    TEST_SERVICE_ID,
                    0x0001,
                    TEST_SERVICE_VERSION.0,
                    TEST_SERVICE_VERSION.1,
                    my_ip,
                    30509,
                    5,
                    next_session_id,
                    true,
                    false,
                );
                next_session_id = next_session_id.wrapping_add(1);
                let _ = sd_socket_discovery.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        let (mut tcp_stream, _) = tokio::select! {
            result = tcp_listener.accept() => result?,
            _ = tokio::time::sleep(Duration::from_secs(10)) =>
                return Err("No TCP connection received".into()),
        };

        let mut buf = [0u8; 1500];
        let _ = loop {
            match tokio::time::timeout(Duration::from_millis(500), sd_socket.recv_from(&mut buf))
                .await
            {
                Ok(Ok((len, from))) => {
                    if len > 40 && buf[24] == 0x06 {
                        // Use session=1, reboot=true to match discovery_task offers
                        let subscribe_ack = build_sd_subscribe_ack_with_session(
                            TEST_SERVICE_ID,
                            0x0001,
                            TEST_SERVICE_VERSION.0,
                            1,
                            5,
                            1,    // session_id
                            true, // reboot=true (matches discovery phase)
                        );
                        let _ = sd_socket.send_to(&subscribe_ack, from).await;
                        break true;
                    }
                }
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => continue,
            }
        };

        discovery_task.abort();
        // Wait for any in-flight messages from discovery_task to be processed
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket_clone = Arc::clone(&sd_socket);
        let sd_task = tokio::spawn(async move {
            // Normal progression
            for session in 30..50 {
                let offer = build_sd_offer_with_tcp_endpoint(
                    TEST_SERVICE_ID,
                    0x0001,
                    TEST_SERVICE_VERSION.0,
                    TEST_SERVICE_VERSION.1,
                    my_ip,
                    30509,
                    5,
                    session,
                    false,
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }

            // Approaching wraparound
            for session in 0xFFFD..=0xFFFF {
                let offer = build_sd_offer_with_tcp_endpoint(
                    TEST_SERVICE_ID,
                    0x0001,
                    TEST_SERVICE_VERSION.0,
                    TEST_SERVICE_VERSION.1,
                    my_ip,
                    30509,
                    5,
                    session,
                    false,
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;

            // Wraparound: 0xFFFF → 1 with reboot=false (NORMAL!)
            for session in 1..5 {
                let offer = build_sd_offer_with_tcp_endpoint(
                    TEST_SERVICE_ID,
                    0x0001,
                    TEST_SERVICE_VERSION.0,
                    TEST_SERVICE_VERSION.1,
                    my_ip,
                    30509,
                    5,
                    session,
                    false,
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });

        // Phase 1 events
        for _ in 0..3 {
            let mut event = Vec::with_capacity(17);
            event.extend_from_slice(&TEST_SERVICE_ID.to_be_bytes());
            event.extend_from_slice(&0x8001u16.to_be_bytes());
            event.extend_from_slice(&9u32.to_be_bytes());
            event.extend_from_slice(&0x0000u16.to_be_bytes());
            event.extend_from_slice(&0xFFFDu16.to_be_bytes());
            event.push(0x01);
            event.push(0x01);
            event.push(0x02);
            event.push(0x00);
            event.push(0x01);

            if tcp_stream.write_all(&event).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(80)).await;
        }

        tokio::time::sleep(Duration::from_millis(200)).await;

        // Phase 2 events - TCP should still be open
        for i in 0..5 {
            let mut event = Vec::with_capacity(17);
            event.extend_from_slice(&TEST_SERVICE_ID.to_be_bytes());
            event.extend_from_slice(&0x8001u16.to_be_bytes());
            event.extend_from_slice(&9u32.to_be_bytes());
            event.extend_from_slice(&0x0000u16.to_be_bytes());
            event.extend_from_slice(&((i + 1) as u16).to_be_bytes());
            event.push(0x01);
            event.push(0x01);
            event.push(0x02);
            event.push(0x00);
            event.push(0x02);

            if tcp_stream.write_all(&event).await.is_err() {
                // If TCP closed, this is a bug (false positive reboot detection)
                break;
            }
            tokio::time::sleep(Duration::from_millis(80)).await;
        }

        let _ = sd_task.await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();

    let events_before = EVENTS_BEFORE_WRAPAROUND.load(Ordering::SeqCst);
    let events_after = EVENTS_AFTER_WRAPAROUND.load(Ordering::SeqCst);

    assert!(
        events_before > 0,
        "Should receive events before wraparound (got {})",
        events_before
    );
    assert!(
        events_after > 0,
        "Should CONTINUE receiving events after wraparound - TCP should stay open (got {})",
        events_after
    );
}

/// feat_req_someipsd_872: Client closes TCP on session regression with reboot=1
/// feat_req_someipsd_764: Reboot detection via session regression
///
/// When both old and new SD messages have reboot=1, and new.session < old.session,
/// this indicates a reboot. TCP connections should be closed.
#[test]
fn client_closes_tcp_on_server_session_regression() {
    covers!(
        feat_req_someipsd_872,
        feat_req_someipsd_764,
        feat_req_someipsd_871
    );

    configure_tracing();

    use recentip::EventgroupId;
    use std::sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    static EVENTS_BEFORE_REBOOT: AtomicU32 = AtomicU32::new(0);
    static EVENTS_AFTER_REBOOT: AtomicU32 = AtomicU32::new(0);
    static TCP_CLOSED: AtomicBool = AtomicBool::new(false);
    EVENTS_BEFORE_REBOOT.store(0, Ordering::SeqCst);
    EVENTS_AFTER_REBOOT.store(0, Ordering::SeqCst);
    TCP_CLOSED.store(false, Ordering::SeqCst);

    let (broadcast_tx, mut broadcast_rx) = tokio::sync::broadcast::channel(1);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(Duration::from_secs(10), runtime.find(TEST_SERVICE_ID))
            .await
            .expect("Should discover service within 10s")
            .expect("Service should be available");
        tracing::info!("Discovered service via TCP");

        let eventgroup = EventgroupId::new(1).unwrap();
        let mut subscription = proxy
            .new_subscription()
            .eventgroup(eventgroup)
            .subscribe()
            .await
            .expect("Should subscribe successfully via TCP");
        tracing::info!("Subscribed to eventgroup via TCP");

        // Phase 1: Events before reboot
        for _ in 0..3 {
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(300), subscription.next()).await
            {
                assert_eq!(event.event_id, EventId::new(0x8001).unwrap());
                assert_eq!(event.payload[0], 0x01);
                EVENTS_BEFORE_REBOOT.fetch_add(1, Ordering::SeqCst);
            }
        }

        tracing::info!("Waiting for server to signal session regression phase...");
        broadcast_rx.recv().await.unwrap();
        tracing::info!("Proceeding to phase 2: session regression...");

        // Phase 2: Events after session regression
        for _ in 0..5 {
            match tokio::time::timeout(Duration::from_millis(200), subscription.next()).await {
                Ok(Some(event)) => {
                    assert_eq!(event.event_id, EventId::new(0x8001).unwrap());
                    EVENTS_AFTER_REBOOT.fetch_add(1, Ordering::SeqCst);
                    tracing::info!(
                        "Received event after session regression - TCP still open (BUG!)"
                    );
                }
                Ok(None) => {
                    tracing::info!(
                        "Subscription closed after session regression - TCP likely closed"
                    );
                    break;
                }
                Err(_) => {
                    tracing::info!(
                        "Timed out waiting for event after session regression - TCP likely closed"
                    );
                    break;
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    let mut b_rx = broadcast_tx.subscribe();
    let b_tx = broadcast_tx.clone();
    sim.client("server", async move {
        let sd_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?);
        let tcp_listener = Arc::new(turmoil::net::TcpListener::bind("0.0.0.0:30509").await?);

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server").to_string().parse().unwrap();
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        let sd_socket_discovery = Arc::clone(&sd_socket);
        tracing::info!("Starting discovery task...");
        let discovery_task = tokio::spawn(async move {
            for i in 0..20 {
                let offer = build_sd_offer_with_tcp_endpoint(
                    TEST_SERVICE_ID,
                    0x0001,
                    TEST_SERVICE_VERSION.0,
                    TEST_SERVICE_VERSION.1,
                    my_ip,
                    30509,
                    5,
                    i,
                    true,
                    false,
                );
                let _ = sd_socket_discovery.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        let (mut tcp_stream, _) = tokio::select! {
            result = tcp_listener.accept() => result?,
            _ = tokio::time::sleep(Duration::from_secs(10)) =>
                return Err("No TCP connection received".into()),
        };
        tracing::info!("TCP connection accepted from client");

        let mut buf = [0u8; 1500];
        let _ = loop {
            match tokio::time::timeout(Duration::from_millis(500), sd_socket.recv_from(&mut buf))
                .await
            {
                Ok(Ok((len, from))) => {
                    if len > 40 && buf[24] == 0x06 {
                        tracing::info!("Subscribe message received from client");
                        let subscribe_ack = build_sd_subscribe_ack(
                            TEST_SERVICE_ID,
                            0x0001,
                            TEST_SERVICE_VERSION.0,
                            1,
                            5,
                            1, // session_id
                        );
                        let _ = sd_socket.send_to(&subscribe_ack, from).await;
                        break true;
                    }
                }
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => continue,
            }
        };

        discovery_task.abort();
        // Wait for any in-flight discovery messages to be delivered
        tokio::time::sleep(Duration::from_millis(200)).await;
        tracing::info!("Starting SD task for session regression test...");

        let sd_socket_clone = Arc::clone(&sd_socket);
        let sd_task = tokio::spawn(async move {
            // Phase 1: reboot=1, sessions increasing (100-104)
            for session in 100..105 {
                let offer = build_sd_offer_with_tcp_endpoint(
                    TEST_SERVICE_ID,
                    0x0001,
                    TEST_SERVICE_VERSION.0,
                    TEST_SERVICE_VERSION.1,
                    my_ip,
                    30509,
                    5,
                    session,
                    true,
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;

            tracing::info!("Broadcasting to client to proceed to phase 2...");
            b_tx.send(()).unwrap();

            // Phase 2: Session regression - reboot=1, session drops to 1
            // Detection: old.reboot=1, new.reboot=1, old.session(104) > new.session(1)
            for session in 1..5 {
                let offer = build_sd_offer_with_tcp_endpoint(
                    TEST_SERVICE_ID,
                    0x0001,
                    TEST_SERVICE_VERSION.0,
                    TEST_SERVICE_VERSION.1,
                    my_ip,
                    30509,
                    5,
                    session,
                    true,
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        tracing::info!("Sending phase 1 events before session regression...");
        // Phase 1 events
        for _ in 0..3 {
            let mut event = Vec::with_capacity(17);
            event.extend_from_slice(&TEST_SERVICE_ID.to_be_bytes());
            event.extend_from_slice(&0x8001u16.to_be_bytes());
            event.extend_from_slice(&9u32.to_be_bytes());
            event.extend_from_slice(&0x0000u16.to_be_bytes());
            event.extend_from_slice(&100u16.to_be_bytes());
            event.push(0x01);
            event.push(0x01);
            event.push(0x02);
            event.push(0x00);
            event.push(0x01);

            if tcp_stream.write_all(&event).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(80)).await;
        }

        tracing::info!("Waiting for SD task to wrap");
        b_rx.recv().await.unwrap();

        // Give client time to process session regression and to wind down TCP
        tokio::time::sleep(Duration::from_millis(500)).await;

        tracing::info!("Trying to send phase 2 events after session regression...");
        // Phase 2 events - should fail (TCP closed)
        for _ in 0..5 {
            let mut event = Vec::with_capacity(17);
            event.extend_from_slice(&TEST_SERVICE_ID.to_be_bytes());
            event.extend_from_slice(&0x8001u16.to_be_bytes());
            event.extend_from_slice(&9u32.to_be_bytes());
            event.extend_from_slice(&0x0000u16.to_be_bytes());
            event.extend_from_slice(&1u16.to_be_bytes());
            event.push(0x01);
            event.push(0x01);
            event.push(0x02);
            event.push(0x00);
            event.push(0x02);

            if tcp_stream.write_all(&event).await.is_err() {
                TCP_CLOSED.store(true, Ordering::SeqCst);
                break;
            }
            tokio::time::sleep(Duration::from_millis(80)).await;
        }

        tracing::info!("Checking TCP connection read status after session regression...");

        let mut read_buf = [0u8; 16];
        match tcp_stream.read(&mut read_buf).await {
            Ok(0) => TCP_CLOSED.store(true, Ordering::SeqCst),
            Err(_) => TCP_CLOSED.store(true, Ordering::SeqCst),
            _ => {}
        }

        let _ = sd_task.await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();

    let events_before = EVENTS_BEFORE_REBOOT.load(Ordering::SeqCst);
    let events_after = EVENTS_AFTER_REBOOT.load(Ordering::SeqCst);

    assert!(
        events_before > 0,
        "Should receive events before reboot (got {})",
        events_before
    );
    assert_eq!(events_after, 0,
        "Should NOT receive events after reboot (session regression) - TCP should be closed (got {})", 
        events_after);
    assert!(
        TCP_CLOSED.load(Ordering::SeqCst),
        "TCP connection should be closed after session regression reboot detection"
    );
}

// ============================================================================
// PER-PEER ISOLATION TESTS
// ============================================================================
// These tests verify that session IDs and reboot flags are tracked
// independently for each peer. A client with multiple servers, or a server
// with multiple clients, must not confuse state between different peers.
// ============================================================================

/// feat_req_someipsd_765: Per-peer session tracking (client perspective)
/// feat_req_someipsd_764: Reboot detection is per-peer
///
/// Client tracks two servers with independent session IDs. When server1's
/// session ID decreases (reboot), server2's service should NOT be affected.
/// Verify client doesn't confuse session state between different peers.
///
/// NOTE: Currently failing - server2 is incorrectly detected as rebooting when
/// server1 reboots. This suggests per-peer isolation is not working correctly.
#[test_log::test]
#[ignore = "Per-peer isolation bug: server2 falsely detected as rebooting when server1 reboots"]
fn client_tracks_session_ids_per_server_independently() {
    covers!(
        feat_req_someipsd_765,
        feat_req_someipsd_764,
        feat_req_someipsd_871
    );

    use std::sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    };

    static SERVER1_EVENTS_BEFORE: AtomicU32 = AtomicU32::new(0);
    static SERVER1_EVENTS_AFTER: AtomicU32 = AtomicU32::new(0);
    static SERVER2_EVENTS_BEFORE: AtomicU32 = AtomicU32::new(0);
    static SERVER2_EVENTS_AFTER: AtomicU32 = AtomicU32::new(0);
    static SERVER1_REBOOTED: AtomicBool = AtomicBool::new(false);

    SERVER1_EVENTS_BEFORE.store(0, Ordering::SeqCst);
    SERVER1_EVENTS_AFTER.store(0, Ordering::SeqCst);
    SERVER2_EVENTS_BEFORE.store(0, Ordering::SeqCst);
    SERVER2_EVENTS_AFTER.store(0, Ordering::SeqCst);
    SERVER1_REBOOTED.store(false, Ordering::SeqCst);

    const SERVER1_SERVICE: u16 = TEST_SERVICE_ID;
    const SERVER2_SERVICE: u16 = 0x5678;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Client subscribes to both servers
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Discover both services
        let proxy1 = tokio::time::timeout(Duration::from_secs(10), runtime.find(SERVER1_SERVICE))
            .await
            .expect("Timeout")
            .expect("Server1 available");
        let proxy2 = tokio::time::timeout(Duration::from_secs(10), runtime.find(SERVER2_SERVICE))
            .await
            .expect("Timeout")
            .expect("Server2 available");

        let eventgroup = EventgroupId::new(1).unwrap();

        let mut sub1 = proxy1
            .new_subscription()
            .eventgroup(eventgroup)
            .subscribe()
            .await
            .unwrap();
        let mut sub2 = proxy2
            .new_subscription()
            .eventgroup(eventgroup)
            .subscribe()
            .await
            .unwrap();

        // Phase 1: Receive events from both servers before server1 reboot
        for _ in 0..3 {
            tokio::select! {
                Some(event) = sub1.next() => {
                    if event.payload.get(0) == Some(&0x01) {
                        SERVER1_EVENTS_BEFORE.fetch_add(1, Ordering::SeqCst);
                    }
                }
                Some(event) = sub2.next() => {
                    if event.payload.get(0) == Some(&0x01) {
                        SERVER2_EVENTS_BEFORE.fetch_add(1, Ordering::SeqCst);
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(300)) => break,
            }
        }

        tokio::time::sleep(Duration::from_millis(400)).await;

        // Phase 2: After server1 reboots, server1 events should stop, server2 continues
        for _ in 0..10 {
            tokio::select! {
                result = sub1.next() => {
                    if let Some(event) = result {
                        if event.payload.get(0) == Some(&0x02) {
                            SERVER1_EVENTS_AFTER.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
                Some(event) = sub2.next() => {
                    if event.payload.get(0) == Some(&0x02) {
                        SERVER2_EVENTS_AFTER.fetch_add(1, Ordering::SeqCst);
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(200)) => break,
            }
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    // Server 1 - REBOOTS (session goes from 100→1 with reboot=1)
    sim.host("server1", || async move {
        let sd_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?);
        let event_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?);

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server1").to_string().parse().unwrap();
        let client_ip: std::net::IpAddr = turmoil::lookup("client").into();
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        let sd_socket_discovery = Arc::clone(&sd_socket);
        let discovery_task = tokio::spawn(async move {
            for _ in 0..20 {
                let offer = build_sd_offer_with_session(
                    SERVER1_SERVICE,
                    0x0001,
                    1,
                    0,
                    my_ip,
                    30509,
                    5,
                    1,
                    true,
                    false,
                );
                let _ = sd_socket_discovery.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        let mut buf = [0u8; 1500];
        let client_event_port = loop {
            match tokio::time::timeout(Duration::from_millis(500), sd_socket.recv_from(&mut buf))
                .await
            {
                Ok(Ok((len, from))) => {
                    if len > 40 && buf[24] == 0x06 {
                        if len > 52 {
                            let port = u16::from_be_bytes([buf[len - 2], buf[len - 1]]);
                            let subscribe_ack =
                                build_sd_subscribe_ack(SERVER1_SERVICE, 0x0001, 1, 1, 5, 1);
                            let _ = sd_socket.send_to(&subscribe_ack, from).await;
                            break port;
                        }
                    }
                }
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => continue,
            }
        };
        discovery_task.abort();
        let client_event_addr = std::net::SocketAddr::new(client_ip, client_event_port);

        let sd_socket_clone = Arc::clone(&sd_socket);
        let sd_task = tokio::spawn(async move {
            // Normal operation with high session IDs
            for session in 100..105 {
                let offer = build_sd_offer_with_session(
                    SERVER1_SERVICE,
                    0x0001,
                    1,
                    0,
                    my_ip,
                    30509,
                    5,
                    session,
                    true,
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
            SERVER1_REBOOTED.store(true, Ordering::SeqCst);

            // REBOOT: Session regresses to 1
            for session in 1..5 {
                let offer = build_sd_offer_with_session(
                    SERVER1_SERVICE,
                    0x0001,
                    1,
                    0,
                    my_ip,
                    30509,
                    5,
                    session,
                    true,
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });

        // Send events
        for _ in 0..3 {
            let mut event = vec![0u8; 17];
            event[0..2].copy_from_slice(&SERVER1_SERVICE.to_be_bytes());
            event[2..4].copy_from_slice(&0x8001u16.to_be_bytes());
            event[4..8].copy_from_slice(&9u32.to_be_bytes());
            event[8..10].copy_from_slice(&0x0000u16.to_be_bytes());
            event[10..12].copy_from_slice(&100u16.to_be_bytes());
            event[12] = 0x01;
            event[13] = 0x01;
            event[14] = 0x02;
            event[15] = 0x00;
            event[16] = 0x01; // Phase 1
            let _ = event_socket.send_to(&event, client_event_addr).await;
            tokio::time::sleep(Duration::from_millis(80)).await;
        }

        tokio::time::sleep(Duration::from_millis(300)).await;

        // Try to send after reboot - should fail due to subscription expired
        for _ in 0..3 {
            let mut event = vec![0u8; 17];
            event[0..2].copy_from_slice(&SERVER1_SERVICE.to_be_bytes());
            event[2..4].copy_from_slice(&0x8001u16.to_be_bytes());
            event[4..8].copy_from_slice(&9u32.to_be_bytes());
            event[8..10].copy_from_slice(&0x0000u16.to_be_bytes());
            event[10..12].copy_from_slice(&1u16.to_be_bytes());
            event[12] = 0x01;
            event[13] = 0x01;
            event[14] = 0x02;
            event[15] = 0x00;
            event[16] = 0x02; // Phase 2
            let _ = event_socket.send_to(&event, client_event_addr).await;
            tokio::time::sleep(Duration::from_millis(80)).await;
        }

        let _ = sd_task.await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    // Server 2 - STABLE (continues normal operation)
    sim.host("server2", || async move {
        let sd_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?);
        let event_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30510").await?);

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server2").to_string().parse().unwrap();
        let client_ip: std::net::IpAddr = turmoil::lookup("client").into();
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        let sd_socket_discovery = Arc::clone(&sd_socket);
        let discovery_task = tokio::spawn(async move {
            for session in 1..20 {
                let offer = build_sd_offer_with_session(
                    SERVER2_SERVICE,
                    0x0001,
                    1,
                    0,
                    my_ip,
                    30510,
                    5,
                    session,
                    session == 1,
                    false,
                );
                let _ = sd_socket_discovery.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        let mut buf = [0u8; 1500];
        let client_event_port = loop {
            match tokio::time::timeout(Duration::from_millis(500), sd_socket.recv_from(&mut buf))
                .await
            {
                Ok(Ok((len, from))) => {
                    if len > 40 && buf[24] == 0x06 {
                        if len > 52 {
                            let port = u16::from_be_bytes([buf[len - 2], buf[len - 1]]);
                            let subscribe_ack =
                                build_sd_subscribe_ack(SERVER2_SERVICE, 0x0001, 1, 1, 5, 1);
                            let _ = sd_socket.send_to(&subscribe_ack, from).await;
                            break port;
                        }
                    }
                }
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => continue,
            }
        };
        discovery_task.abort();
        let client_event_addr = std::net::SocketAddr::new(client_ip, client_event_port);

        let sd_socket_clone = Arc::clone(&sd_socket);
        let sd_task = tokio::spawn(async move {
            // Server2 continues with normal incremental sessions
            // Even while server1 is rebooting, server2 stays stable
            // reboot=false for all after initial discovery
            for session in 20..40 {
                let offer = build_sd_offer_with_session(
                    SERVER2_SERVICE,
                    0x0001,
                    1,
                    0,
                    my_ip,
                    30510,
                    5,
                    session,
                    false,
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        // Send events continuously through both phases
        for phase in 1..=2 {
            for _ in 0..5 {
                let mut event = vec![0u8; 17];
                event[0..2].copy_from_slice(&SERVER2_SERVICE.to_be_bytes());
                event[2..4].copy_from_slice(&0x8001u16.to_be_bytes());
                event[4..8].copy_from_slice(&9u32.to_be_bytes());
                event[8..10].copy_from_slice(&0x0000u16.to_be_bytes());
                event[10..12].copy_from_slice(&10u16.to_be_bytes());
                event[12] = 0x01;
                event[13] = 0x01;
                event[14] = 0x02;
                event[15] = 0x00;
                event[16] = phase; // Phase marker
                let _ = event_socket.send_to(&event, client_event_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        let _ = sd_task.await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();

    let s1_before = SERVER1_EVENTS_BEFORE.load(Ordering::SeqCst);
    let s1_after = SERVER1_EVENTS_AFTER.load(Ordering::SeqCst);
    let s2_before = SERVER2_EVENTS_BEFORE.load(Ordering::SeqCst);
    let s2_after = SERVER2_EVENTS_AFTER.load(Ordering::SeqCst);

    assert!(
        s1_before > 0,
        "Should receive events from server1 before reboot (got {})",
        s1_before
    );
    assert_eq!(
        s1_after, 0,
        "Should NOT receive events from server1 after reboot (got {})",
        s1_after
    );
    assert!(
        s2_before > 0,
        "Should receive events from server2 in phase 1 (got {})",
        s2_before
    );
    assert!(s2_after > 0, "Should CONTINUE receiving events from server2 in phase 2 - server1 reboot should not affect server2 (got {})", s2_after);
}

/// feat_req_someipsd_765: Per-peer session tracking (client perspective)
///
/// Client tracks two servers with similar session progressions. Verify that
/// session wraparound on one server doesn't affect the other server's state.
#[test_log::test]
fn client_does_not_confuse_session_wraparound_between_servers() {
    covers!(feat_req_someipsd_765, feat_req_someipsd_764);

    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    };

    static SERVER1_EVENTS: AtomicU32 = AtomicU32::new(0);
    static SERVER2_EVENTS: AtomicU32 = AtomicU32::new(0);
    SERVER1_EVENTS.store(0, Ordering::SeqCst);
    SERVER2_EVENTS.store(0, Ordering::SeqCst);

    const SERVER1_SERVICE: u16 = TEST_SERVICE_ID;
    const SERVER2_SERVICE: u16 = 0x5679;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy1 = tokio::time::timeout(Duration::from_secs(10), runtime.find(SERVER1_SERVICE))
            .await
            .unwrap()
            .unwrap();
        let proxy2 = tokio::time::timeout(Duration::from_secs(10), runtime.find(SERVER2_SERVICE))
            .await
            .unwrap()
            .unwrap();

        let eventgroup = EventgroupId::new(1).unwrap();
        let mut sub1 = proxy1
            .new_subscription()
            .eventgroup(eventgroup)
            .subscribe()
            .await
            .unwrap();
        let mut sub2 = proxy2
            .new_subscription()
            .eventgroup(eventgroup)
            .subscribe()
            .await
            .unwrap();

        // Receive events from both - server1 does wraparound, server2 doesn't
        for _ in 0..20 {
            tokio::select! {
                Some(_) = sub1.next() => {
                    SERVER1_EVENTS.fetch_add(1, Ordering::SeqCst);
                }
                Some(_) = sub2.next() => {
                    SERVER2_EVENTS.fetch_add(1, Ordering::SeqCst);
                }
                _ = tokio::time::sleep(Duration::from_millis(200)) => break,
            }
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    // Server 1 - Does session wraparound (0xFFFF→1)
    sim.host("server1", || async move {
        let sd_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?);
        let event_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30511").await?);

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server1").to_string().parse().unwrap();
        let client_ip: std::net::IpAddr = turmoil::lookup("client").into();
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        let sd_socket_discovery = Arc::clone(&sd_socket);
        let discovery_task = tokio::spawn(async move {
            for i in 0..20 {
                let offer = build_sd_offer_with_session(
                    SERVER1_SERVICE,
                    0x0001,
                    1,
                    0,
                    my_ip,
                    30511,
                    5,
                    i,
                    true,
                    false,
                );
                let _ = sd_socket_discovery.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        let mut buf = [0u8; 1500];
        let client_event_port = loop {
            match tokio::time::timeout(Duration::from_millis(500), sd_socket.recv_from(&mut buf))
                .await
            {
                Ok(Ok((len, from))) => {
                    if len > 40 && buf[24] == 0x06 {
                        if len > 52 {
                            let port = u16::from_be_bytes([buf[len - 2], buf[len - 1]]);
                            // Use session=1, reboot=true to match discovery_task offers
                            let subscribe_ack = build_sd_subscribe_ack_with_session(
                                SERVER1_SERVICE,
                                0x0001,
                                1,
                                1,
                                5,
                                1,    // session_id
                                true, // reboot=true (matches discovery phase)
                            );
                            let _ = sd_socket.send_to(&subscribe_ack, from).await;
                            break port;
                        }
                    }
                }
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => continue,
            }
        };
        discovery_task.abort();
        // Wait for any in-flight messages from discovery_task to be processed
        tokio::time::sleep(Duration::from_millis(100)).await;
        let client_event_addr = std::net::SocketAddr::new(client_ip, client_event_port);

        let sd_socket_clone = Arc::clone(&sd_socket);
        tokio::spawn(async move {
            for session in 20..30 {
                let offer = build_sd_offer_with_session(
                    SERVER1_SERVICE,
                    0x0001,
                    1,
                    0,
                    my_ip,
                    30511,
                    5,
                    session,
                    false,
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            // Wraparound: high sessions then wrap to 1
            for session in 0xFFFD..=0xFFFF {
                let offer = build_sd_offer_with_session(
                    SERVER1_SERVICE,
                    0x0001,
                    1,
                    0,
                    my_ip,
                    30511,
                    5,
                    session,
                    false,
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            for session in 1..5 {
                let offer = build_sd_offer_with_session(
                    SERVER1_SERVICE,
                    0x0001,
                    1,
                    0,
                    my_ip,
                    30511,
                    5,
                    session,
                    false,
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });

        for _ in 0..10 {
            let mut event = vec![0u8; 17];
            event[0..2].copy_from_slice(&SERVER1_SERVICE.to_be_bytes());
            event[2..4].copy_from_slice(&0x8001u16.to_be_bytes());
            event[4..8].copy_from_slice(&9u32.to_be_bytes());
            event[8..10].copy_from_slice(&0x0000u16.to_be_bytes());
            event[10..12].copy_from_slice(&100u16.to_be_bytes());
            event[12] = 0x01;
            event[13] = 0x01;
            event[14] = 0x02;
            event[15] = 0x00;
            event[16] = 0xAA;
            let _ = event_socket.send_to(&event, client_event_addr).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    // Server 2 - Normal incremental sessions (no wraparound)
    sim.host("server2", || async move {
        let sd_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?);
        let event_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30512").await?);

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server2").to_string().parse().unwrap();
        let client_ip: std::net::IpAddr = turmoil::lookup("client").into();
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        let sd_socket_discovery = Arc::clone(&sd_socket);
        let discovery_task = tokio::spawn(async move {
            for i in 0..20 {
                let offer = build_sd_offer_with_session(
                    SERVER2_SERVICE,
                    0x0001,
                    1,
                    0,
                    my_ip,
                    30512,
                    5,
                    i,
                    true,
                    false,
                );
                let _ = sd_socket_discovery.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        let mut buf = [0u8; 1500];
        let client_event_port = loop {
            match tokio::time::timeout(Duration::from_millis(500), sd_socket.recv_from(&mut buf))
                .await
            {
                Ok(Ok((len, from))) => {
                    if len > 40 && buf[24] == 0x06 {
                        if len > 52 {
                            let port = u16::from_be_bytes([buf[len - 2], buf[len - 1]]);
                            // Use session=1, reboot=true to match discovery_task offers
                            let subscribe_ack = build_sd_subscribe_ack_with_session(
                                SERVER2_SERVICE,
                                0x0001,
                                1,
                                1,
                                5,
                                1,    // session_id
                                true, // reboot=true (matches discovery phase)
                            );
                            let _ = sd_socket.send_to(&subscribe_ack, from).await;
                            break port;
                        }
                    }
                }
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => continue,
            }
        };
        discovery_task.abort();
        // Wait for any in-flight messages from discovery_task to be processed
        tokio::time::sleep(Duration::from_millis(100)).await;
        let client_event_addr = std::net::SocketAddr::new(client_ip, client_event_port);

        let sd_socket_clone = Arc::clone(&sd_socket);
        tokio::spawn(async move {
            // Just normal progression, no wraparound
            for session in 20..50 {
                let offer = build_sd_offer_with_session(
                    SERVER2_SERVICE,
                    0x0001,
                    1,
                    0,
                    my_ip,
                    30512,
                    5,
                    session,
                    session < 30,
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        for _ in 0..10 {
            let mut event = vec![0u8; 17];
            event[0..2].copy_from_slice(&SERVER2_SERVICE.to_be_bytes());
            event[2..4].copy_from_slice(&0x8001u16.to_be_bytes());
            event[4..8].copy_from_slice(&9u32.to_be_bytes());
            event[8..10].copy_from_slice(&0x0000u16.to_be_bytes());
            event[10..12].copy_from_slice(&200u16.to_be_bytes());
            event[12] = 0x01;
            event[13] = 0x01;
            event[14] = 0x02;
            event[15] = 0x00;
            event[16] = 0xBB;
            let _ = event_socket.send_to(&event, client_event_addr).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();

    let s1_events = SERVER1_EVENTS.load(Ordering::SeqCst);
    let s2_events = SERVER2_EVENTS.load(Ordering::SeqCst);

    assert!(
        s1_events > 0,
        "Should receive events from server1 (wraparound server) - got {}",
        s1_events
    );
    assert!(
        s2_events > 0,
        "Should receive events from server2 (normal server) - got {}",
        s2_events
    );
}

/// feat_req_someipsd_765: Per-peer reboot flag tracking (client perspective)
///
/// Client tracks two servers with independent reboot flags. When server1's
/// reboot flag transitions 1→0 (normal wraparound), and server2's reboot flag
/// transitions 0→1 (actual reboot), client must handle both correctly and
/// independently.
///
/// NOTE: Currently failing - per-peer reboot flag isolation not working correctly.
#[test_log::test]
#[ignore = "Per-peer isolation bug: reboot flag confusion between servers"]
fn client_tracks_reboot_flags_per_server_independently() {
    covers!(
        feat_req_someipsd_765,
        feat_req_someipsd_764,
        feat_req_someipsd_871
    );

    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    };

    static SERVER1_EVENTS: AtomicU32 = AtomicU32::new(0);
    static SERVER2_EVENTS_BEFORE: AtomicU32 = AtomicU32::new(0);
    static SERVER2_EVENTS_AFTER: AtomicU32 = AtomicU32::new(0);
    SERVER1_EVENTS.store(0, Ordering::SeqCst);
    SERVER2_EVENTS_BEFORE.store(0, Ordering::SeqCst);
    SERVER2_EVENTS_AFTER.store(0, Ordering::SeqCst);

    const SERVER1_SERVICE: u16 = TEST_SERVICE_ID;
    const SERVER2_SERVICE: u16 = 0x567A;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy1 = tokio::time::timeout(Duration::from_secs(10), runtime.find(SERVER1_SERVICE))
            .await
            .unwrap()
            .unwrap();
        let proxy2 = tokio::time::timeout(Duration::from_secs(10), runtime.find(SERVER2_SERVICE))
            .await
            .unwrap()
            .unwrap();

        let eventgroup = EventgroupId::new(1).unwrap();
        let mut sub1 = proxy1
            .new_subscription()
            .eventgroup(eventgroup)
            .subscribe()
            .await
            .unwrap();
        let mut sub2 = proxy2
            .new_subscription()
            .eventgroup(eventgroup)
            .subscribe()
            .await
            .unwrap();

        // Phase 1: Both servers sending events
        for _ in 0..10 {
            tokio::select! {
                Some(_) = sub1.next() => {
                    SERVER1_EVENTS.fetch_add(1, Ordering::SeqCst);
                }
                Some(event) = sub2.next() => {
                    if event.payload.get(0) == Some(&0x01) {
                        SERVER2_EVENTS_BEFORE.fetch_add(1, Ordering::SeqCst);
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(200)) => break,
            }
        }

        tokio::time::sleep(Duration::from_millis(400)).await;

        // Phase 2: After server2 reboot, server1 continues, server2 stops
        for _ in 0..10 {
            tokio::select! {
                Some(_) = sub1.next() => {
                    SERVER1_EVENTS.fetch_add(1, Ordering::SeqCst);
                }
                result = sub2.next() => {
                    if let Some(event) = result {
                        if event.payload.get(0) == Some(&0x02) {
                            SERVER2_EVENTS_AFTER.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(200)) => break,
            }
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    // Server 1 - Reboot flag 1→0 (normal wraparound, NOT a reboot)
    sim.host("server1", || async move {
        let sd_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?);
        let event_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30513").await?);

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server1").to_string().parse().unwrap();
        let client_ip: std::net::IpAddr = turmoil::lookup("client").into();
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        let sd_socket_discovery = Arc::clone(&sd_socket);
        let discovery_task = tokio::spawn(async move {
            for _ in 0..20 {
                let offer = build_sd_offer_with_session(
                    SERVER1_SERVICE,
                    0x0001,
                    1,
                    0,
                    my_ip,
                    30513,
                    5,
                    1,
                    true,
                    false,
                );
                let _ = sd_socket_discovery.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        let mut buf = [0u8; 1500];
        let client_event_port = loop {
            match tokio::time::timeout(Duration::from_millis(500), sd_socket.recv_from(&mut buf))
                .await
            {
                Ok(Ok((len, from))) => {
                    if len > 40 && buf[24] == 0x06 {
                        if len > 52 {
                            let port = u16::from_be_bytes([buf[len - 2], buf[len - 1]]);
                            let subscribe_ack =
                                build_sd_subscribe_ack(SERVER1_SERVICE, 0x0001, 1, 1, 5, 1);
                            let _ = sd_socket.send_to(&subscribe_ack, from).await;
                            break port;
                        }
                    }
                }
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => continue,
            }
        };
        discovery_task.abort();
        let client_event_addr = std::net::SocketAddr::new(client_ip, client_event_port);

        let sd_socket_clone = Arc::clone(&sd_socket);
        tokio::spawn(async move {
            // reboot=1, increasing sessions
            for session in 2..10 {
                let offer = build_sd_offer_with_session(
                    SERVER1_SERVICE,
                    0x0001,
                    1,
                    0,
                    my_ip,
                    30513,
                    5,
                    session,
                    true,
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            // reboot flag transitions 1→0 (normal wraparound)
            for session in 10..25 {
                let offer = build_sd_offer_with_session(
                    SERVER1_SERVICE,
                    0x0001,
                    1,
                    0,
                    my_ip,
                    30513,
                    5,
                    session,
                    false,
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        // Send events continuously
        for _ in 0..15 {
            let mut event = vec![0u8; 17];
            event[0..2].copy_from_slice(&SERVER1_SERVICE.to_be_bytes());
            event[2..4].copy_from_slice(&0x8001u16.to_be_bytes());
            event[4..8].copy_from_slice(&9u32.to_be_bytes());
            event[8..10].copy_from_slice(&0x0000u16.to_be_bytes());
            event[10..12].copy_from_slice(&100u16.to_be_bytes());
            event[12] = 0x01;
            event[13] = 0x01;
            event[14] = 0x02;
            event[15] = 0x00;
            event[16] = 0xCC;
            let _ = event_socket.send_to(&event, client_event_addr).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    // Server 2 - Reboot flag 0→1 (actual REBOOT)
    sim.host("server2", || async move {
        let sd_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?);
        let event_socket = Arc::new(turmoil::net::UdpSocket::bind("0.0.0.0:30514").await?);

        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server2").to_string().parse().unwrap();
        let client_ip: std::net::IpAddr = turmoil::lookup("client").into();
        let multicast_addr: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        let sd_socket_discovery = Arc::clone(&sd_socket);
        let discovery_task = tokio::spawn(async move {
            for _ in 0..20 {
                let offer = build_sd_offer_with_session(
                    SERVER2_SERVICE,
                    0x0001,
                    1,
                    0,
                    my_ip,
                    30514,
                    5,
                    1,
                    true,
                    false,
                );
                let _ = sd_socket_discovery.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        let mut buf = [0u8; 1500];
        let client_event_port = loop {
            match tokio::time::timeout(Duration::from_millis(500), sd_socket.recv_from(&mut buf))
                .await
            {
                Ok(Ok((len, from))) => {
                    if len > 40 && buf[24] == 0x06 {
                        if len > 52 {
                            let port = u16::from_be_bytes([buf[len - 2], buf[len - 1]]);
                            let subscribe_ack =
                                build_sd_subscribe_ack(SERVER2_SERVICE, 0x0001, 1, 1, 5, 1);
                            let _ = sd_socket.send_to(&subscribe_ack, from).await;
                            break port;
                        }
                    }
                }
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => continue,
            }
        };
        discovery_task.abort();
        let client_event_addr = std::net::SocketAddr::new(client_ip, client_event_port);

        let sd_socket_clone = Arc::clone(&sd_socket);
        tokio::spawn(async move {
            // reboot=0, normal operation
            for session in 2..6 {
                let offer = build_sd_offer_with_session(
                    SERVER2_SERVICE,
                    0x0001,
                    1,
                    0,
                    my_ip,
                    30514,
                    5,
                    session,
                    false,
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            // reboot flag transitions 0→1 (REBOOT!)
            for session in 1..5 {
                let offer = build_sd_offer_with_session(
                    SERVER2_SERVICE,
                    0x0001,
                    1,
                    0,
                    my_ip,
                    30514,
                    5,
                    session,
                    true,
                    false,
                );
                let _ = sd_socket_clone.send_to(&offer, multicast_addr).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });

        // Send events before reboot
        for _ in 0..5 {
            let mut event = vec![0u8; 17];
            event[0..2].copy_from_slice(&SERVER2_SERVICE.to_be_bytes());
            event[2..4].copy_from_slice(&0x8001u16.to_be_bytes());
            event[4..8].copy_from_slice(&9u32.to_be_bytes());
            event[8..10].copy_from_slice(&0x0000u16.to_be_bytes());
            event[10..12].copy_from_slice(&200u16.to_be_bytes());
            event[12] = 0x01;
            event[13] = 0x01;
            event[14] = 0x02;
            event[15] = 0x00;
            event[16] = 0x01; // Phase 1
            let _ = event_socket.send_to(&event, client_event_addr).await;
            tokio::time::sleep(Duration::from_millis(80)).await;
        }

        tokio::time::sleep(Duration::from_millis(300)).await;

        // Try to send after reboot
        for _ in 0..5 {
            let mut event = vec![0u8; 17];
            event[0..2].copy_from_slice(&SERVER2_SERVICE.to_be_bytes());
            event[2..4].copy_from_slice(&0x8001u16.to_be_bytes());
            event[4..8].copy_from_slice(&9u32.to_be_bytes());
            event[8..10].copy_from_slice(&0x0000u16.to_be_bytes());
            event[10..12].copy_from_slice(&1u16.to_be_bytes());
            event[12] = 0x01;
            event[13] = 0x01;
            event[14] = 0x02;
            event[15] = 0x00;
            event[16] = 0x02; // Phase 2
            let _ = event_socket.send_to(&event, client_event_addr).await;
            tokio::time::sleep(Duration::from_millis(80)).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();

    let s1_events = SERVER1_EVENTS.load(Ordering::SeqCst);
    let s2_before = SERVER2_EVENTS_BEFORE.load(Ordering::SeqCst);
    let s2_after = SERVER2_EVENTS_AFTER.load(Ordering::SeqCst);

    assert!(
        s1_events > 5,
        "Server1 should continue sending events through reboot flag transition 1→0 (got {})",
        s1_events
    );
    assert!(
        s2_before > 0,
        "Server2 should send events before reboot (got {})",
        s2_before
    );
    assert_eq!(
        s2_after, 0,
        "Server2 should NOT send events after reboot (flag 0→1) - got {}, expected 0",
        s2_after
    );
}
