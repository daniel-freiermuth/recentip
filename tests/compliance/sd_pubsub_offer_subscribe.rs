//! Offer → Subscribe Compliance Tests
//!
//! Tests for SOME/IP-SD requirements related to the relationship between
//! OfferService and SubscribeEventgroup messages.
//!
//! # Covered Requirements
//!
//! - `feat_req_someipsd_428`: OfferService shall be used as trigger for Subscriptions
//! - `feat_req_someipsd_431`: Client shall respond to OfferService with SubscribeEventgroup
//! - `feat_req_someipsd_631`: Subscriptions shall NOT be triggered cyclically but by OfferService
//!
//! # Test Summary
//!
//! | Test | Status | Requirement |
//! |------|--------|-------------|
//! | `no_subscribe_without_offer` | ✅ Pass | Implicit - subscribes only after offer |
//! | `available_returns_error_when_service_not_found` | ✅ Pass | Find request expiry behavior |
//! | `offer_triggers_subscribe_renewal` | ✅ Pass | `feat_req_someipsd_428/431` |
//! | `no_cyclic_subscribes_strict_631_compliance` | ✅ Pass | `feat_req_someipsd_631` |
//! | `max_ttl_subscription_no_renewal_needed` | ✅ Pass | Infinite TTL needs no renewal |
//!
//! # Test Strategy
//!
//! These tests use a "raw socket" server that sends OfferService messages directly
//! on the wire, while the client uses the library. This allows precise control over
//! offer timing and verification that the client responds correctly.
//!
//! Run with: cargo test --features turmoil --test compliance sd_pubsub_offer_subscribe

use bytes::Bytes;
use recentip::prelude::*;
use recentip::wire::{Header, SdEntryType, SdMessage, SD_METHOD_ID, SD_SERVICE_ID};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Coverage tracking macro
macro_rules! covers {
    ($req:ident) => {
        // Marker for requirement traceability - see scripts/generate_compliance.py
        let _ = stringify!($req);
    };
}

const SUBSCRIBE_TEST_SERVICE_ID: u16 = 0x2000;
const SUBSCRIBE_TEST_SERVICE_VERSION: (u8, u32) = (1, 0);

// ============================================================================
// HELPER FUNCTIONS: Build raw SD messages
// ============================================================================

/// Build a raw SOME/IP-SD OfferService message with configurable session ID and flags
fn build_sd_offer_with_session(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    minor_version: u32,
    endpoint_ip: Ipv4Addr,
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
    packet.extend_from_slice(&0u32.to_be_bytes()); // Length placeholder
    packet.extend_from_slice(&0x0000u16.to_be_bytes()); // Client ID
    packet.extend_from_slice(&session_id.to_be_bytes()); // Session ID
    packet.push(0x01); // Protocol Version
    packet.push(0x01); // Interface Version
    packet.push(0x02); // Message Type = NOTIFICATION
    packet.push(0x00); // Return Code = E_OK

    // === SD Payload ===
    let mut flags = if unicast_flag { 0x40u8 } else { 0x00u8 };
    if reboot_flag {
        flags |= 0x80;
    }
    packet.push(flags);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]); // Reserved

    // Entries array length (16 bytes for one entry)
    packet.extend_from_slice(&16u32.to_be_bytes());

    // === OfferService Entry (16 bytes) ===
    packet.push(0x01); // Type = OfferService
    packet.push(0x00); // Index 1st options
    packet.push(0x00); // Index 2nd options
    packet.push(0x10); // # of opt 1 | # of opt 2
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

/// Build a raw SOME/IP-SD OfferService message (legacy, uses session_id=1 and reboot=true)
/*
fn build_sd_offer(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    minor_version: u32,
    endpoint_ip: Ipv4Addr,
    endpoint_port: u16,
    ttl: u32,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(56);

    // === SOME/IP Header (16 bytes) ===
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes()); // Service ID = SD
    packet.extend_from_slice(&0x8100u16.to_be_bytes()); // Method ID = SD
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes()); // Length placeholder
    packet.extend_from_slice(&0x0000u16.to_be_bytes()); // Client ID
    packet.extend_from_slice(&0x0001u16.to_be_bytes()); // Session ID
    packet.push(0x01); // Protocol Version
    packet.push(0x01); // Interface Version
    packet.push(0x02); // Message Type = NOTIFICATION
    packet.push(0x00); // Return Code = E_OK

    // === SD Payload ===
    packet.push(0xC0); // Flags: Unicast + Reboot
    packet.extend_from_slice(&[0x00, 0x00, 0x00]); // Reserved

    // Entries array length (16 bytes for one entry)
    packet.extend_from_slice(&16u32.to_be_bytes());

    // === OfferService Entry (16 bytes) ===
    packet.push(0x01); // Type = OfferService
    packet.push(0x00); // Index 1st options
    packet.push(0x00); // Index 2nd options
    packet.push(0x10); // # of opt 1 | # of opt 2
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

/// Build a raw SOME/IP-SD SubscribeEventgroupAck message with configurable session ID
fn build_sd_subscribe_ack_with_session(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    ttl: u32,
    session_id: u16,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(48);

    // === SOME/IP Header (16 bytes) ===
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes()); // Service ID = SD
    packet.extend_from_slice(&0x8100u16.to_be_bytes()); // Method ID = SD
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes()); // Length placeholder
    packet.extend_from_slice(&0x0000u16.to_be_bytes()); // Client ID
    packet.extend_from_slice(&session_id.to_be_bytes()); // Session ID
    packet.push(0x01); // Protocol Version
    packet.push(0x01); // Interface Version
    packet.push(0x02); // Message Type = NOTIFICATION
    packet.push(0x00); // Return Code = E_OK

    // === SD Payload ===
    packet.push(0xC0); // Flags: Unicast + Reboot
    packet.extend_from_slice(&[0x00, 0x00, 0x00]); // Reserved

    // Entries array length (16 bytes for one entry)
    packet.extend_from_slice(&16u32.to_be_bytes());

    // === SubscribeEventgroupAck Entry (16 bytes) ===
    packet.push(0x07); // Type = SubscribeEventgroupAck
    packet.push(0x00); // Index 1st options
    packet.push(0x00); // Index 2nd options
    packet.push(0x00); // # of options
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]); // TTL (24-bit)
    packet.push(0x00); // Reserved
    packet.push(0x00); // Reserved (counter high byte)
    packet.extend_from_slice(&eventgroup_id.to_be_bytes());

    // Options array length (0 - no options)
    packet.extend_from_slice(&0u32.to_be_bytes());

    // Fix up length field
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}

/// Build a raw SOME/IP-SD SubscribeEventgroupAck message (legacy, uses session_id=1)
/*
fn build_sd_subscribe_ack(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    ttl: u32,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(48);

    // === SOME/IP Header (16 bytes) ===
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes()); // Service ID = SD
    packet.extend_from_slice(&0x8100u16.to_be_bytes()); // Method ID = SD
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes()); // Length placeholder
    packet.extend_from_slice(&0x0000u16.to_be_bytes()); // Client ID
    packet.extend_from_slice(&0x0001u16.to_be_bytes()); // Session ID
    packet.push(0x01); // Protocol Version
    packet.push(0x01); // Interface Version
    packet.push(0x02); // Message Type = NOTIFICATION
    packet.push(0x00); // Return Code = E_OK

    // === SD Payload ===
    packet.push(0xC0); // Flags: Unicast + Reboot
    packet.extend_from_slice(&[0x00, 0x00, 0x00]); // Reserved

    // Entries array length (16 bytes for one entry)
    packet.extend_from_slice(&16u32.to_be_bytes());

    // === SubscribeEventgroupAck Entry (16 bytes) ===
    packet.push(0x07); // Type = SubscribeEventgroupAck
    packet.push(0x00); // Index 1st options
    packet.push(0x00); // Index 2nd options
    packet.push(0x00); // # of options
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]); // TTL (24-bit)
    packet.push(0x00); // Reserved
    packet.push(0x00); // Reserved (counter high byte)
    packet.extend_from_slice(&eventgroup_id.to_be_bytes());

    // Options array length (0 - no options)
    packet.extend_from_slice(&0u32.to_be_bytes());

    // Fix up length field
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
} */

/// Parse an SD message from raw bytes
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

/// Check if an SD message contains a SubscribeEventgroup entry and return its details
fn find_subscribe_entry(data: &[u8]) -> Option<(u16, u16, u16, u32)> {
    let (_header, sd_msg) = parse_sd_message(data)?;
    for entry in &sd_msg.entries {
        if entry.entry_type == SdEntryType::SubscribeEventgroup && entry.ttl > 0 {
            return Some((
                entry.service_id,
                entry.instance_id,
                entry.eventgroup_id,
                entry.ttl,
            ));
        }
    }
    None
}

// ============================================================================
// TESTS
// ============================================================================

/// feat_req_someipsd_428: OfferService shall be used as trigger for Subscriptions
/// feat_req_someipsd_431: Client shall respond to OfferService with SubscribeEventgroup
///
/// When a client has an active subscription and receives an OfferService,
/// it should send a SubscribeEventgroup in response.
///
/// Test setup:
/// 1. Raw server sends OfferService
/// 2. Client discovers service and subscribes
/// 3. Server sends ACK
/// 4. Server sends another OfferService
/// 5. Verify client sends SubscribeEventgroup in response
///
/// NOTE: This test is currently ignored because the library does not yet
/// implement offer-triggered re-subscription. This is a specification gap
/// that needs to be addressed. See PUBSUB_REQUIREMENTS.md for details.
#[test_log::test]
fn offer_triggers_subscribe_renewal() {
    covers!(feat_req_someipsd_428);
    covers!(feat_req_someipsd_431);

    let subscribe_count = Arc::new(AtomicUsize::new(0));
    let subscribe_count_clone = Arc::clone(&subscribe_count);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .max_message_latency(Duration::from_millis(10))
        .build();

    // Raw socket server - sends offers and counts subscribe responses
    let subscribe_count2 = Arc::clone(&subscribe_count_clone);
    sim.client("raw_server", async move {
        let my_ip: Ipv4Addr = turmoil::lookup("raw_server").to_string().parse().unwrap();

        // SD socket for offers/subscribes
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // RPC socket (for endpoint option)
        let _rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?;

        let mut buf = [0u8; 1500];

        // Session IDs for offers (multicast) and acks (unicast)
        let mut next_multicast_session_id: u16 = 1;
        let mut next_unicast_session_id: u16 = 1;

        // Phase 1: Send initial offer and wait for first subscribe
        eprintln!("[raw_server] Sending initial OfferService");

        for _ in 0..20 {
            let offer = build_sd_offer_with_session(
                SUBSCRIBE_TEST_SERVICE_ID,
                0x0001,
                SUBSCRIBE_TEST_SERVICE_VERSION.0,
                SUBSCRIBE_TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                3600,
                next_multicast_session_id,
                next_multicast_session_id == 1, // reboot_flag only on first
                false,
            );
            next_multicast_session_id = next_multicast_session_id.wrapping_add(1);
            sd_socket.send_to(&offer, sd_multicast).await?;

            // Check for subscribe responses
            while let Ok(Ok((len, from))) = tokio::time::timeout(
                Duration::from_millis(100),
                sd_socket.recv_from(&mut buf),
            ).await {
                if let Some((svc_id, inst_id, eg_id, ttl)) = find_subscribe_entry(&buf[..len]) {
                    eprintln!("[raw_server] Received SubscribeEventgroup: service=0x{:04X}, instance=0x{:04X}, eventgroup=0x{:04X}, TTL={}",
                        svc_id, inst_id, eg_id, ttl);

                    subscribe_count2.fetch_add(1, Ordering::SeqCst);

                    // Send ACK
                    let ack = build_sd_subscribe_ack_with_session(svc_id, inst_id, 1, eg_id, ttl, next_unicast_session_id);
                    next_unicast_session_id = next_unicast_session_id.wrapping_add(1);
                    sd_socket.send_to(&ack, from).await?;
                }
            }

            if subscribe_count2.load(Ordering::SeqCst) >= 1 {
                break;
            }
        }

        assert!(
            subscribe_count2.load(Ordering::SeqCst) >= 1,
            "Should receive initial SubscribeEventgroup"
        );

        eprintln!("[raw_server] Got initial subscribe, waiting before sending renewal offer...");
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Phase 2: Send another offer - client should re-subscribe
        // per feat_req_someipsd_428 and feat_req_someipsd_431
        let initial_count = subscribe_count2.load(Ordering::SeqCst);
        eprintln!("[raw_server] Sending renewal OfferService (subscribe count before: {})", initial_count);

        for _ in 0..10 {
            let offer = build_sd_offer_with_session(
                SUBSCRIBE_TEST_SERVICE_ID,
                0x0001,
                SUBSCRIBE_TEST_SERVICE_VERSION.0,
                SUBSCRIBE_TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                3600,
                next_multicast_session_id,
                false, // reboot_flag false for subsequent offers
                false,
            );
            next_multicast_session_id = next_multicast_session_id.wrapping_add(1);
            sd_socket.send_to(&offer, sd_multicast).await?;

            while let Ok(Ok((len, from))) = tokio::time::timeout(
                Duration::from_millis(100),
                sd_socket.recv_from(&mut buf),
            ).await {
                if let Some((svc_id, inst_id, eg_id, ttl)) = find_subscribe_entry(&buf[..len]) {
                    eprintln!("[raw_server] Received SubscribeEventgroup after renewal offer: service=0x{:04X}, eventgroup=0x{:04X}",
                        svc_id, eg_id);

                    subscribe_count2.fetch_add(1, Ordering::SeqCst);

                    // Send ACK
                    let ack = build_sd_subscribe_ack_with_session(svc_id, inst_id, 1, eg_id, ttl, next_unicast_session_id);
                    next_unicast_session_id = next_unicast_session_id.wrapping_add(1);
                    sd_socket.send_to(&ack, from).await?;
                }
            }

            if subscribe_count2.load(Ordering::SeqCst) > initial_count {
                break;
            }
        }

        let final_count = subscribe_count2.load(Ordering::SeqCst);
        eprintln!("[raw_server] Final subscribe count: {}", final_count);

        // NOTE: This assertion may fail if the library doesn't implement
        // offer-triggered re-subscription yet. This test documents the
        // expected behavior per the spec.
        assert!(
            final_count > initial_count,
            "Client should send SubscribeEventgroup in response to OfferService (feat_req_someipsd_428/431). \
                Got {} subscribes total, expected > {} after renewal offer.",
            final_count, initial_count
        );

        Ok(())
    });

    // Client using library - discovers and subscribes
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .subscribe_ttl(5)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Find and wait for service
        let proxy = runtime
            .find(SUBSCRIBE_TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Should discover service")
            .expect("Service available");

        eprintln!("[client] Service discovered, subscribing...");

        // Subscribe to eventgroup
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        eprintln!("[client] Subscription active, waiting for offer-triggered renewal...");

        // Keep subscription alive while server tests renewal
        tokio::time::sleep(Duration::from_secs(5)).await;

        Ok(())
    });

    sim.run().unwrap();

    let final_count = subscribe_count.load(Ordering::SeqCst);
    eprintln!(
        "Test complete: {} total SubscribeEventgroup messages received",
        final_count
    );
}

/// feat_req_someipsd_631: Subscriptions shall NOT be triggered cyclically but SHALL
/// be triggered by OfferService entries.
///
/// This is the strict 631-compliant test: the client must NEVER send a subscribe
/// on its own. It should ONLY send subscribes in direct response to offers.
///
/// Test scenario with deliberate gaps to expose any internal timers:
/// - Offer at t=0s → expect subscribe
/// - Gap of 8 seconds (longer than typical TTL/2 renewal) → expect NO subscribes  
/// - Offer at t=8s → expect subscribe
/// - Gap of 15 seconds (way past any reasonable TTL) → expect NO subscribes
/// - Offer at t=23s → expect subscribe
/// - Gap of 30 seconds (extreme) → expect NO subscribes
/// - Offer at t=53s → expect subscribe
///
/// If the client has ANY internal cyclic timer, it will fire during the gaps
/// and this test will catch it.
#[test_log::test]
#[cfg(feature = "slow-tests")]
fn no_cyclic_subscribes_strict_631_compliance() {
    covers!(feat_req_someipsd_631);

    // Track: (timestamp_ms, was_offer_sent_recently)
    let subscribe_events = Arc::new(std::sync::Mutex::new(Vec::<(u64, bool)>::new()));
    let subscribe_events_clone = Arc::clone(&subscribe_events);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(120))
        .build();

    sim.client("raw_server", async move {
        let my_ip: Ipv4Addr = turmoil::lookup("raw_server").to_string().parse().unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let start = tokio::time::Instant::now();

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // Session IDs for offers (multicast) and acks (unicast)
        let mut next_multicast_session_id: u16 = 1;
        let mut next_unicast_session_id: u16 = 1;

        let mut buf = [0u8; 1500];

        // Offer schedule with deliberate gaps to catch cyclic timers
        // Format: (time_ms, is_offer_window_start)
        // The gaps are designed to be longer than any reasonable TTL/2 renewal interval
        let schedule = [
            (0, true),     // t=0s: send offer, expect subscribe
            (500, false),  // t=0.5s: still in response window
            (1000, false), // t=1s: end of valid response window
            // GAP: 1s to 8s (7 seconds) - way past TTL/2=2.5s, should see NO subscribes
            (8000, true),  // t=8s: send offer, expect subscribe
            (8500, false), // t=8.5s: still in response window
            (9000, false), // t=9s: end of valid response window
            // GAP: 9s to 23s (14 seconds) - way past TTL=5s, should see NO subscribes
            (23000, true),  // t=23s: send offer, expect subscribe
            (23500, false), // t=23.5s: still in response window
            (24000, false), // t=24s: end of valid response window
            // GAP: 24s to 53s (29 seconds) - extreme gap, should see NO subscribes
            (53000, true),  // t=53s: send offer, expect subscribe
            (53500, false), // t=53.5s: still in response window
            (54000, false), // t=54s: end of valid response window
            // GAP: 54s to 90s (36 seconds) - final extreme gap
            (90000, true), // t=90s: final offer
            (90500, false),
            (91000, false),
        ];

        let mut schedule_idx = 0;
        let mut last_offer_time: Option<u64> = None;
        let mut offer_response_window = false;

        let test_end = tokio::time::Instant::now() + Duration::from_secs(95);

        while tokio::time::Instant::now() < test_end {
            let elapsed = start.elapsed().as_millis() as u64;

            // Process schedule
            while schedule_idx < schedule.len() {
                let (sched_time, is_offer) = schedule[schedule_idx];
                if elapsed >= sched_time {
                    if is_offer {
                        eprintln!("[raw_server] t={}ms: Sending offer", elapsed);
                        let offer = build_sd_offer_with_session(
                            SUBSCRIBE_TEST_SERVICE_ID,
                            0x0001,
                            SUBSCRIBE_TEST_SERVICE_VERSION.0,
                            SUBSCRIBE_TEST_SERVICE_VERSION.1,
                            my_ip,
                            30509,
                            5000,
                            next_multicast_session_id,
                            next_multicast_session_id == 1, // reboot_flag only on first
                            false,
                        );
                        next_multicast_session_id = next_multicast_session_id.wrapping_add(1);
                        sd_socket.send_to(&offer, sd_multicast).await?;
                        last_offer_time = Some(elapsed);
                        offer_response_window = true;
                    } else if offer_response_window && elapsed > last_offer_time.unwrap_or(0) + 1000
                    {
                        // Response window closed after 1 second
                        offer_response_window = false;
                        eprintln!(
                            "[raw_server] t={}ms: Response window closed, entering gap",
                            elapsed
                        );
                    }
                    schedule_idx += 1;
                } else {
                    break;
                }
            }

            // Check for response window expiry
            if let Some(lot) = last_offer_time {
                if elapsed > lot + 1000 {
                    offer_response_window = false;
                }
            }

            // Receive subscribes
            while let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(50), sd_socket.recv_from(&mut buf)).await
            {
                if let Some((svc_id, inst_id, eg_id, ttl)) = find_subscribe_entry(&buf[..len]) {
                    let now_ms = start.elapsed().as_millis() as u64;
                    let in_window = offer_response_window;

                    eprintln!(
                        "[raw_server] t={}ms: Received Subscribe (in_offer_window={})",
                        now_ms, in_window
                    );

                    subscribe_events_clone
                        .lock()
                        .unwrap()
                        .push((now_ms, in_window));

                    // Always ACK to keep the test going
                    let ack = build_sd_subscribe_ack_with_session(
                        svc_id,
                        inst_id,
                        1,
                        eg_id,
                        ttl,
                        next_unicast_session_id,
                    );
                    next_unicast_session_id = next_unicast_session_id.wrapping_add(1);
                    sd_socket.send_to(&ack, from).await?;
                }
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        Ok(())
    });

    sim.client("client", async move {
        // Use short TTL to tempt renewal
        let runtime = recentip::configure()
            .subscribe_ttl(5) // 5 second TTL
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        eprintln!("[client] Runtime initialized, looking for service...");

        let proxy = runtime
            .find(SUBSCRIBE_TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(10), proxy)
            .await
            .expect("Should discover service")
            .expect("Service available");

        eprintln!("[client] Service discovered, subscribing...");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        eprintln!("[client] Subscription active, keeping alive for test duration...");

        tokio::time::sleep(Duration::from_secs(100)).await;
        runtime.shutdown().await;
        Ok(())
    });

    sim.run().unwrap();

    // Analyze results
    let events = subscribe_events.lock().unwrap();
    eprintln!("\n=== SUBSCRIBE EVENT ANALYSIS ===");
    eprintln!("Total subscribes received: {}", events.len());

    let mut subscribes_in_window = 0;
    let mut subscribes_outside_window = 0;

    for (time_ms, in_window) in events.iter() {
        let status = if *in_window {
            "✓ IN WINDOW"
        } else {
            "✗ OUTSIDE WINDOW (631 VIOLATION!)"
        };
        eprintln!("  t={}ms: {}", time_ms, status);
        if *in_window {
            subscribes_in_window += 1;
        } else {
            subscribes_outside_window += 1;
        }
    }

    eprintln!("\nSummary:");
    eprintln!(
        "  Subscribes in offer response window: {}",
        subscribes_in_window
    );
    eprintln!(
        "  Subscribes outside window (violations): {}",
        subscribes_outside_window
    );

    // We expect exactly 5 offers, so up to 5 subscribes in response
    // ANY subscribes outside the window are 631 violations
    assert_eq!(
        subscribes_outside_window, 0,
        "feat_req_someipsd_631 VIOLATION: Client sent {} SubscribeEventgroup messages \
         without a preceding OfferService. Subscriptions shall NOT be triggered cyclically \
         but SHALL be triggered by OfferService entries.",
        subscribes_outside_window
    );

    // We should have received at least some subscribes (proves test worked)
    assert!(
        subscribes_in_window >= 1,
        "Test infrastructure issue: no subscribes received in response to offers"
    );

    eprintln!("\n✓ 631 COMPLIANCE VERIFIED: All subscribes were triggered by offers");
}

/// Verify that no SubscribeEventgroup is sent before an OfferService is received.
///
/// The spec implies subscriptions are triggered by offers. A client should not
/// send SubscribeEventgroup for a service it hasn't discovered yet.
#[test_log::test]
fn no_subscribe_without_offer() {
    let subscribe_before_offer = Arc::new(AtomicUsize::new(0));
    let subscribe_after_offer = Arc::new(AtomicUsize::new(0));
    let subscribe_before_clone = Arc::clone(&subscribe_before_offer);
    let subscribe_after_clone = Arc::clone(&subscribe_after_offer);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let subscribe_before = Arc::clone(&subscribe_before_clone);
    let subscribe_after = Arc::clone(&subscribe_after_clone);
    // Raw server - first listens for subscribes, then sends offer
    sim.client("raw_server", async move {
        let my_ip: Ipv4Addr = turmoil::lookup("raw_server").to_string().parse().unwrap();

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

        let _rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?;

        let mut buf = [0u8; 1500];

        // Phase 1: Listen for 2 seconds WITHOUT sending offer
        eprintln!("[raw_server] Phase 1: Listening for subscribes before sending offer...");
        let listen_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < listen_deadline {
            if let Ok(Ok((len, _from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                if find_subscribe_entry(&buf[..len]).is_some() {
                    eprintln!("[raw_server] ERROR: Received subscribe BEFORE offer!");
                    subscribe_before.fetch_add(1, Ordering::SeqCst);
                }
            }
        }

        // Phase 2: Send offer and expect subscribe
        eprintln!("[raw_server] Phase 2: Sending OfferService...");

        // Session IDs for offers (multicast) and acks (unicast)
        let mut next_multicast_session_id: u16 = 1;
        let mut next_unicast_session_id: u16 = 1;

        for _ in 0..20 {
            let offer = build_sd_offer_with_session(
                SUBSCRIBE_TEST_SERVICE_ID,
                0x0001,
                SUBSCRIBE_TEST_SERVICE_VERSION.0,
                SUBSCRIBE_TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                3600,
                next_multicast_session_id,
                next_multicast_session_id == 1, // reboot_flag only on first
                false,
            );
            next_multicast_session_id = next_multicast_session_id.wrapping_add(1);
            sd_socket.send_to(&offer, sd_multicast).await?;

            while let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                if let Some((svc_id, inst_id, eg_id, ttl)) = find_subscribe_entry(&buf[..len]) {
                    eprintln!("[raw_server] Received subscribe AFTER offer from {}", from);
                    subscribe_after.fetch_add(1, Ordering::SeqCst);

                    // ACK
                    let ack = build_sd_subscribe_ack_with_session(
                        svc_id,
                        inst_id,
                        1,
                        eg_id,
                        ttl,
                        next_unicast_session_id,
                    );
                    next_unicast_session_id = next_unicast_session_id.wrapping_add(1);
                    eprintln!(
                        "[raw_server] Sending SubscribeAck to {} ({} bytes)",
                        from,
                        ack.len()
                    );
                    sd_socket.send_to(&ack, from).await?;
                    eprintln!("[raw_server] SubscribeAck sent");
                }
            }

            if subscribe_after.load(Ordering::SeqCst) >= 1 {
                break;
            }
        }
        Ok(())
    });

    // Client attempts to subscribe immediately
    sim.client("client", async move {
        // Start immediately - try to find and subscribe
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(SUBSCRIBE_TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));

        // Wait for service to become available (should only happen after offer)
        let proxy = tokio::time::timeout(Duration::from_secs(10), proxy)
            .await
            .expect("Should eventually discover service")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");
        Ok(())
    });

    sim.run().unwrap();

    let before = subscribe_before_offer.load(Ordering::SeqCst);
    let after = subscribe_after_offer.load(Ordering::SeqCst);

    eprintln!(
        "Subscribes before offer: {}, after offer: {}",
        before, after
    );

    assert_eq!(
        before, 0,
        "Client should NOT send SubscribeEventgroup before receiving OfferService"
    );
    assert!(
        after >= 1,
        "Client should send SubscribeEventgroup after receiving OfferService"
    );
}

/// Verify that `` returns an error when the service is never offered.
///
/// When a client calls `find()` and no OfferService is ever received,
/// the find request will eventually expire after exhausting its repetitions.
/// The client should receive an error rather than hanging forever.
///
/// This tests the find request cleanup mechanism:
/// 1. Client calls `proxy` - registers find request with N repetitions
/// 2. Runtime sends FindService messages, decrementing repetitions each cycle
/// 3. When repetitions reach 0, the find request is removed
/// 4. Removing the find request drops the notification channel sender
/// 5. `available()` receives channel close and returns `RuntimeShutdown` error
#[test_log::test]
fn available_returns_error_when_service_not_found() {
    let find_messages_received = Arc::new(AtomicUsize::new(0));
    let find_messages_clone = Arc::clone(&find_messages_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Raw server that never sends Offerjust counts FindService messages
    sim.client("raw_server", async move {
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];

        // Listen for FindService messages (but never respond with Offer)
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok((len, _from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                // Check if this is a FindService message
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type == SdEntryType::FindService {
                            eprintln!(
                                "[raw_server] Received FindService for {:04X}:{:04X}",
                                entry.service_id, entry.instance_id
                            );
                            find_messages_clone.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
            }
        }

        Ok(())
    });

    // Client tries to find a service that doesn't exist
    sim.client("client", async move {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        eprintln!("[client] Looking for service that doesn't exist...");

        let proxy = runtime
            .find(SUBSCRIBE_TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));

        // available() should return an error when find request expires
        let result = proxy.await;

        match &result {
            Ok(_) => panic!("available() should have returned an error for non-existent service"),
            Err(Error::NotAvailable) => {
                eprintln!("[client] Got expected error");
            }
            Err(e) => {
                panic!("available() returned unexpected error: {:?}", e);
            }
        }

        assert!(
            result.is_err(),
            "available() should return error when service not found"
        );

        Ok(())
    });

    sim.run().unwrap();

    let find_count = find_messages_received.load(Ordering::SeqCst);
    eprintln!("FindService messages received: {}", find_count);

    // Should have received some FindService messages before giving up
    assert!(
        find_count >= 1,
        "Should have sent at least one FindService message"
    );
}

/// When a subscription uses TTL=0xFFFFFF (infinite/until reboot), subsequent
/// OfferService messages should NOT trigger re-subscription because the
/// subscription never expires.
///
/// Per the spec:
/// - TTL=0xFFFFFF means "valid until next reboot"
/// - No renewal is needed since subscription doesn't expire
/// - Only the initial subscribe (on first offer) is required
///
/// Test scenario:
/// 1. Server sends offer
/// 2. Client subscribes with TTL=0xFFFFFF (max)
/// 3. Server ACKs
/// 4. Server sends many more offers over 30+ seconds
/// 5. Verify: client sends exactly ONE subscribe (the initial one)
#[test_log::test]
#[cfg(feature = "slow-tests")]
fn max_ttl_subscription_no_renewal_needed() {
    let subscribe_count = Arc::new(AtomicUsize::new(0));
    let subscribe_count_clone = Arc::clone(&subscribe_count);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(60))
        .build();

    sim.client("raw_server", async move {
        let my_ip: Ipv4Addr = turmoil::lookup("raw_server").to_string().parse().unwrap();

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

        let _rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?;

        // Session IDs for offers (multicast) and acks (unicast)
        let mut next_multicast_session_id: u16 = 1;
        let mut next_unicast_session_id: u16 = 1;

        let mut buf = [0u8; 1500];
        let start = tokio::time::Instant::now();

        // Phase 1: Get initial subscription
        eprintln!("[raw_server] Phase 1: Sending initial offer, waiting for subscribe...");
        for _ in 0..20 {
            let offer = build_sd_offer_with_session(
                SUBSCRIBE_TEST_SERVICE_ID,
                0x0001,
                SUBSCRIBE_TEST_SERVICE_VERSION.0,
                SUBSCRIBE_TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                3, // Short server TTL to send frequent offers
                next_multicast_session_id,
                next_multicast_session_id == 1, // reboot_flag only on first
                false,
            );
            next_multicast_session_id = next_multicast_session_id.wrapping_add(1);
            sd_socket.send_to(&offer, sd_multicast).await?;

            while let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                if let Some((svc_id, inst_id, eg_id, ttl)) = find_subscribe_entry(&buf[..len]) {
                    eprintln!(
                        "[raw_server] t={}ms: Subscribe received (TTL=0x{:06X})",
                        start.elapsed().as_millis(),
                        ttl
                    );
                    subscribe_count_clone.fetch_add(1, Ordering::SeqCst);

                    // ACK with max TTL to match client's request
                    let ack = build_sd_subscribe_ack_with_session(
                        svc_id,
                        inst_id,
                        1,
                        eg_id,
                        ttl,
                        next_unicast_session_id,
                    );
                    next_unicast_session_id = next_unicast_session_id.wrapping_add(1);
                    sd_socket.send_to(&ack, from).await?;
                }
            }

            if subscribe_count_clone.load(Ordering::SeqCst) >= 1 {
                break;
            }
        }

        assert!(
            subscribe_count_clone.load(Ordering::SeqCst) >= 1,
            "Should receive initial subscribe"
        );

        let initial_count = subscribe_count_clone.load(Ordering::SeqCst);
        eprintln!("[raw_server] Got {} initial subscribe(s)", initial_count);

        // Phase 2: Hammer with offers for 30 seconds
        // With TTL=max subscription, none of these should trigger re-subscribes
        eprintln!("[raw_server] Phase 2: Sending offers every 1s for 30s...");
        let phase2_end = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut offer_count = 0;

        while tokio::time::Instant::now() < phase2_end {
            let offer = build_sd_offer_with_session(
                SUBSCRIBE_TEST_SERVICE_ID,
                0x0001,
                SUBSCRIBE_TEST_SERVICE_VERSION.0,
                SUBSCRIBE_TEST_SERVICE_VERSION.1,
                my_ip,
                30509,
                3,
                next_multicast_session_id,
                false, // reboot_flag false for subsequent offers
                false,
            );
            next_multicast_session_id = next_multicast_session_id.wrapping_add(1);
            sd_socket.send_to(&offer, sd_multicast).await?;
            offer_count += 1;
            eprintln!(
                "[raw_server] t={}ms: Sent offer #{}",
                start.elapsed().as_millis(),
                offer_count
            );

            // Check for any subscribes (we don't expect any)
            while let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await
            {
                if let Some((svc_id, inst_id, eg_id, ttl)) = find_subscribe_entry(&buf[..len]) {
                    eprintln!(
                        "[raw_server] t={}ms: UNEXPECTED re-subscribe! (TTL=0x{:06X})",
                        start.elapsed().as_millis(),
                        ttl
                    );
                    subscribe_count_clone.fetch_add(1, Ordering::SeqCst);

                    // Still ACK it
                    let ack = build_sd_subscribe_ack_with_session(
                        svc_id,
                        inst_id,
                        1,
                        eg_id,
                        ttl,
                        next_unicast_session_id,
                    );
                    next_unicast_session_id = next_unicast_session_id.wrapping_add(1);
                    sd_socket.send_to(&ack, from).await?;
                }
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        let final_count = subscribe_count_clone.load(Ordering::SeqCst);
        eprintln!("\n=== RESULTS ===");
        eprintln!("Offers sent in phase 2: {}", offer_count);
        eprintln!("Total subscribes received: {}", final_count);
        eprintln!("Expected: exactly {} (initial only)", initial_count);

        // With TTL=max, we should only see the initial subscribe
        // Any additional subscribes are unnecessary and wasteful
        assert_eq!(
            final_count, initial_count,
            "With TTL=0xFFFFFF (infinite) subscription, client should NOT re-subscribe \
             in response to offers. Got {} subscribes total, expected only {} (initial). \
             Sent {} offers that should NOT have triggered renewal.",
            final_count, initial_count, offer_count
        );

        eprintln!("\n✓ MAX TTL TEST PASSED: No unnecessary re-subscribes");
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Configure with MAX TTL (infinite/until reboot)
        let runtime = recentip::configure()
            .subscribe_ttl(0xFFFFFF) // Max TTL = valid until reboot
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(SUBSCRIBE_TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(10), proxy)
            .await
            .expect("Should discover service")
            .expect("Service available");

        eprintln!("[client] Service discovered, subscribing with TTL=max...");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        eprintln!("[client] Subscription active (TTL=max), waiting...");

        // Keep subscription alive for entire test
        tokio::time::sleep(Duration::from_secs(45)).await;
        Ok(())
    });

    sim.run().unwrap();

    let final_count = subscribe_count.load(Ordering::SeqCst);
    eprintln!("Test complete: {} total subscribes", final_count);
}
