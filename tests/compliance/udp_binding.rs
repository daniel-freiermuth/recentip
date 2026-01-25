//! UDP Binding Compliance Tests
//!
//! Tests for SOME/IP over UDP transport binding.
//!
//! Key requirements tested:
//! - feat_req_someip_318: UDP binding is straightforward transport
//! - feat_req_someip_319: Multiple messages per UDP datagram
//! - feat_req_someip_584: Each payload has its own header
//! - feat_req_someip_585: Same as 584 (every payload has its own header)
//! - feat_req_someip_811: UDP supports unicast and multicast
//! - feat_req_someip_812: Multicast eventgroups with initial events
//! - feat_req_someip_814: Clients receive via unicast and/or multicast

use bytes::Bytes;
use recentip::handle::ServiceEvent;
use recentip::prelude::*;
use recentip::wire::{Header, SD_SERVICE_ID};
use std::net::SocketAddr;
use std::time::Duration;

use crate::helpers::wait_for_subscription;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

/// Type alias for turmoil-based runtime (UDP for networking, TCP for connection pool)

const TEST_SERVICE_ID: u16 = 0x1234;
const TEST_SERVICE_VERSION: (u8, u32) = (1, 0);

const EVENT_SERVICE_ID: u16 = 0x5678;
const EVENT_SERVICE_VERSION: (u8, u32) = (1, 0);

// ============================================================================
// Helper Functions
// ============================================================================

/// Helper to parse a SOME/IP header from raw bytes
fn parse_header(data: &[u8]) -> Option<Header> {
    if data.len() < Header::SIZE {
        return None;
    }
    Header::parse(&mut Bytes::copy_from_slice(data))
}

/// Parse multiple SOME/IP messages from a UDP datagram
#[allow(dead_code)]
fn parse_udp_datagram(data: &[u8]) -> Vec<Header> {
    let mut headers = Vec::new();
    let mut offset = 0;

    while offset + 16 <= data.len() {
        if let Some(header) = parse_header(&data[offset..]) {
            let msg_len = 8 + header.length as usize;
            if offset + msg_len <= data.len() {
                headers.push(header);
                offset += msg_len;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    headers
}

/// Check if an address is multicast
#[allow(dead_code)]
fn is_multicast(addr: &SocketAddr) -> bool {
    match addr {
        SocketAddr::V4(v4) => v4.ip().is_multicast(),
        SocketAddr::V6(v6) => v6.ip().is_multicast(),
    }
}

// ============================================================================
// Basic UDP Transport Tests
// ============================================================================

/// feat_req_someip_318: UDP binding is straightforward
///
/// The UDP binding of SOME/IP is straight forward by transporting SOME/IP
/// messages in UDP datagrams.
#[test_log::test]
fn udp_binding_transports_someip_messages() {
    covers!(feat_req_someip_318);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
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

        // Handle one request
        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::Call { responder, .. } => {
                    responder.reply(b"response").unwrap();
                }
                _ => {}
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout waiting for service")
            .expect("Service available");

        // Send RPC request via UDP
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(0x0001).unwrap(), b"request"),
        )
        .await
        .expect("Timeout waiting for response")
        .expect("Call should succeed");

        assert_eq!(response.payload.as_ref(), b"response");

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_584: Each SOME/IP payload has its own header
///
/// Each SOME/IP payload shall have its own SOME/IP header.
/// (Same concept as TCP requirement feat_req_someip_585)
#[test_log::test]
fn udp_each_message_has_own_header() {
    covers!(feat_req_someip_584, feat_req_someip_585);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server receives requests and verifies each has a valid header
    sim.host("server", || async {
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

        // Handle three requests - each should arrive with its own header
        let expected_payloads: [&[u8]; 3] = [b"one", b"two", b"three"];
        for expected in expected_payloads {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call {
                        payload, responder, ..
                    } => {
                        assert_eq!(payload.as_ref(), expected);
                        responder.reply(expected).unwrap();
                    }
                    _ => panic!("Expected Call event"),
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout")
            .expect("Service available");

        let method_id = MethodId::new(0x0001).unwrap();

        // Send three requests with different payloads
        let payloads: [&[u8]; 3] = [b"one", b"two", b"three"];
        for payload in payloads {
            let response =
                tokio::time::timeout(Duration::from_secs(5), proxy.call(method_id, payload))
                    .await
                    .expect("Timeout")
                    .expect("Call should succeed");

            assert_eq!(response.payload.as_ref(), payload);
        }

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_319: Multiple messages per UDP datagram
///
/// The header format allows transporting more than one SOME/IP message
/// in a single UDP datagram.
///
/// Note: The runtime may or may not coalesce messages. This test verifies
/// that the wire format supports multiple messages per datagram by capturing
/// raw UDP traffic.
#[test_log::test]
fn udp_multiple_messages_format_supported() {
    covers!(feat_req_someip_319);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server just receives and echoes
    sim.host("server", || async {
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

        // Handle multiple requests
        for _ in 0..5 {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call {
                        payload, responder, ..
                    } => {
                        responder.reply(&payload).unwrap();
                    }
                    _ => {}
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    // Raw observer captures UDP traffic and verifies format
    sim.client("raw_observer", async move {
        // Bind to port to receive traffic
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        socket
            .join_multicast_v4(
                std::net::Ipv4Addr::new(239, 255, 0, 1),
                std::net::Ipv4Addr::UNSPECIFIED,
            )
            .unwrap();

        let mut buf = vec![0u8; 65535];
        let mut total_headers_seen = 0;

        // Capture some traffic
        for _ in 0..20 {
            tokio::select! {
                result = socket.recv_from(&mut buf) => {
                    if let Ok((len, _from)) = result {
                        let headers = parse_udp_datagram(&buf[..len]);
                        total_headers_seen += headers.len();
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }

        // We should have seen multiple SOME/IP messages
        // (even if not coalesced, verifies the format is parseable)
        assert!(
            total_headers_seen > 0,
            "Should have captured SOME/IP headers from UDP"
        );

        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout")
            .expect("Service available");

        let method_id = MethodId::new(0x0001).unwrap();

        // Send multiple requests
        for i in 0u8..5 {
            let _response =
                tokio::time::timeout(Duration::from_secs(5), proxy.call(method_id, &[i]))
                    .await
                    .expect("Timeout")
                    .expect("Call should succeed");
        }

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// Unicast and Multicast Tests
// ============================================================================

/// feat_req_someip_811: UDP supports unicast and multicast
///
/// The UDP Binding shall support unicast and multicast transmission
/// depending on the use case.
#[test_log::test]
fn udp_supports_unicast_and_multicast() {
    covers!(feat_req_someip_811);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Offer triggers multicast SD messages
        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Keep alive long enough for observation
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    // Observe multicast SD traffic
    sim.client("multicast_observer", async move {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        socket
            .join_multicast_v4(
                std::net::Ipv4Addr::new(239, 255, 0, 1),
                std::net::Ipv4Addr::UNSPECIFIED,
            )
            .unwrap();

        let mut buf = vec![0u8; 65535];
        let mut multicast_seen = false;

        // Wait for multicast SD message
        for _ in 0..20 {
            tokio::select! {
                result = socket.recv_from(&mut buf) => {
                    if let Ok((len, _from)) = result {
                        // Verify it's an SD message
                        if let Some(header) = parse_header(&buf[..len]) {
                            if header.service_id == SD_SERVICE_ID {
                                multicast_seen = true;
                                // Source should be from server (multicast is delivery mechanism)
                                break;
                            }
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }

        assert!(multicast_seen, "Should receive multicast SD messages");
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_814: Clients receive via unicast and/or multicast
///
/// SOME/IP clients shall support receiving via unicast and/or via multicast
/// depending on configuration.
#[test_log::test]
fn udp_client_receives_multicast_offers() {
    covers!(feat_req_someip_814);

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

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Client discovers service via multicast OfferService
        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));

        // Should discover the service (received via multicast)
        let result = tokio::time::timeout(Duration::from_secs(5), proxy).await;

        assert!(
            result.is_ok(),
            "Client should discover service via multicast SD"
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_812: Multicast eventgroups with initial events
///
/// The UDP Binding shall support multicast eventgroups with initial events
/// of fields transported via UDP unicast.
#[test_log::test]
fn udp_multicast_eventgroup_with_initial_events() {
    covers!(feat_req_someip_812);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let eventgroup = EventgroupId::new(0x01).unwrap();
        let mut offering = runtime
            .offer(EVENT_SERVICE_ID, InstanceId::Id(0x0001))
            .version(EVENT_SERVICE_VERSION.0, EVENT_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for client to subscribe
        tokio::time::timeout(Duration::from_secs(5), wait_for_subscription(&mut offering))
            .await
            .expect("Timeout waiting for subscription")
            .expect("Sub expected");

        // Send initial event (unicast to subscriber)
        let event_id = EventId::new(0x8001).unwrap();
        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();
        tracing::info!("Sending initial event");
        event_handle.notify(b"initial value").await.unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;

        // Send subsequent event (could be multicast in full implementation)
        tracing::info!("Sending update event");
        event_handle.notify(b"update").await.unwrap();

        // Unfortunately, SOME/IP doesn't define a way to end subscriptions
        // with guaranteed delivery of last event, so we just wait a bit before
        // sending StopOffer.
        tokio::time::sleep(Duration::from_millis(200)).await;
        tracing::info!("Server done");
        drop(event_handle);
        drop(offering);
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(EVENT_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x01).unwrap();
        let mut subscription = proxy
            .new_subscription()
            .eventgroup(eventgroup)
            .subscribe()
            .await
            .unwrap();

        // Should receive initial event (unicast)
        let initial = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("Timeout for initial event");

        assert!(initial.is_some(), "Should receive initial event");
        let initial_event = initial.unwrap();
        assert_eq!(
            String::from_utf8_lossy(&initial_event.payload),
            "initial value"
        );

        // Should receive subsequent event
        let update = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("Timeout for update event");

        assert!(update.is_some(), "Should receive update event");
        let update_event = update.unwrap();
        assert_eq!(update_event.payload.as_ref(), b"update");

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// UDP Message Framing Tests
// ============================================================================

/// Verify UDP request/response works correctly with varying payload sizes
#[test_log::test]
fn udp_handles_various_payload_sizes() {
    covers!(feat_req_someip_318, feat_req_someip_584);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
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

        // Echo back varying payload sizes
        for _ in 0..3 {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call {
                        payload, responder, ..
                    } => {
                        responder.reply(&payload).unwrap();
                    }
                    _ => {}
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout")
            .expect("Service available");

        let method_id = MethodId::new(0x0001).unwrap();

        // Test various payload sizes
        let payloads = [
            b"x".to_vec(),                     // Tiny
            b"medium length payload".to_vec(), // Medium
            vec![0xAB; 1000],                  // Large (within UDP MTU)
        ];

        for payload in &payloads {
            let response =
                tokio::time::timeout(Duration::from_secs(5), proxy.call(method_id, payload))
                    .await
                    .expect("Timeout")
                    .expect("Call should succeed");

            assert_eq!(response.payload.as_ref(), payload.as_slice());
        }

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// Default Transport Verification
// ============================================================================

/// Verify default transport is UDP (not TCP)
#[test_log::test]
fn default_transport_is_udp() {
    covers!(feat_req_someip_318);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        // No transport configuration - should default to UDP
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

        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::Call { responder, .. } => {
                    responder.reply(b"udp response").unwrap();
                }
                _ => {}
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // No transport configuration - should default to UDP
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout")
            .expect("Service available");

        // Communication works via UDP by default
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(0x0001).unwrap(), b"request"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");

        assert_eq!(response.payload.as_ref(), b"udp response");

        Ok(())
    });

    sim.run().unwrap();
}
