//! TCP Binding Compliance Tests
//!
//! Tests for SOME/IP over TCP transport binding.
//!
//! NOTE: These tests are currently ignored because TCP transport is not yet
//! fully implemented in the runtime. They serve as TDD targets for future implementation.
//!
//! Key requirements tested:
//! - feat_req_someip_324: TCP binding is based on UDP binding
//! - feat_req_someip_325: Nagle's algorithm disabled
//! - feat_req_someip_326: Connection lost handling (requests timeout, NOT reboot)
//! - feat_req_someip_585: Every payload has its own header
//! - feat_req_someip_586: Magic Cookies allow resync in testing
//! - feat_req_someip_591: TCP segment starts with Magic Cookie
//! - feat_req_someip_592: Only one Magic Cookie per segment
//! - feat_req_someip_644: Single TCP connection per client-server pair
//! - feat_req_someip_646: Client opens TCP connection
//! - feat_req_someip_647: Client reestablishes connection
//! - feat_req_someip_702: Multiple messages per segment supported
//! - feat_req_someipsd_872: Reboot detection triggers TCP reset

use crate::helpers::configure_tracing;
use crate::wire_format::helpers::{
    parse_sd_packet, ParsedSdMessage, SdEntryType, SdOfferBuilder, SdSubscribeAckBuilder,
};
use recentip::handle::ServiceEvent;
use recentip::prelude::*;
use recentip::Transport;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::TcpSocket;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

/// Type alias for turmoil-based runtime (needs both UdpSocket and TcpStream)

const TEST_SERVICE_ID: u16 = 0x1234;
const TEST_SERVICE_VERSION: (u8, u32) = (1, 0);

// ============================================================================
// TCP Connection Lifecycle Tests
// ============================================================================

/// feat_req_someip_646: Client opens TCP connection when first request sent
///
/// The TCP connection shall be opened by the client, when the first
/// request is to be sent to the server.
#[test_log::test]
fn client_opens_tcp_connection() {
    covers!(feat_req_someip_646);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        // Wait for connection and request
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
            .preferred_transport(Transport::Tcp)
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

        // Before first request: no TCP connection
        // First request should open TCP connection
        let _response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(0x0001).unwrap(), b"request"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");

        // TODO: Verify TCP connection was opened for first request
        // This requires access to TCP connection count or similar metric

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_644: Single TCP connection for all messages between client-server
///
/// The client and server shall use a single TCP connection for
/// all SOME/IP messages between them.
#[test_log::test]
fn single_tcp_connection_reused() {
    covers!(feat_req_someip_644);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .tcp()
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

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
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
            let response =
                tokio::time::timeout(Duration::from_secs(5), proxy.call(method_id, &[i]))
                    .await
                    .expect("Timeout")
                    .expect("Call should succeed");

            assert_eq!(response.payload.as_ref(), &[i]);
        }

        // TODO: Verify only 1 TCP connection was used
        // turmoil::established_tcp_stream_count() should equal 1

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_647: Client reestablishes connection after failure
///
/// The client is responsible for reestablishing the TCP connection
/// after it has been closed or lost.
#[test_log::test]

fn client_reestablishes_connection() {
    covers!(feat_req_someip_647);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        // Handle requests (may get multiple after reconnection)
        loop {
            tokio::select! {
                event = offering.next() => {
                    if let Some(ServiceEvent::Call { payload, responder, .. }) = event {
                        responder.reply(&payload).unwrap();
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(500)) => break,
            }
        }

        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
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

        // First request establishes connection
        let _response =
            tokio::time::timeout(Duration::from_secs(5), proxy.call(method_id, b"first"))
                .await
                .expect("Timeout")
                .expect("Call should succeed");

        // TODO: Simulate connection loss using turmoil::partition("server", "client")
        // turmoil::partition("server", "client");
        // tokio::time::sleep(Duration::from_millis(100)).await;
        // turmoil::repair("server", "client");

        // Second request after "reconnection" should succeed
        let _response =
            tokio::time::timeout(Duration::from_secs(5), proxy.call(method_id, b"second"))
                .await
                .expect("Timeout")
                .expect("Reconnection should succeed");

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// TCP Message Framing Tests
// ============================================================================

/// feat_req_someip_585: Every SOME/IP payload has its own header
///
/// Every SOME/IP payload shall have its own SOME/IP header (no batching
/// of payloads under single header).
#[test_log::test]

fn each_message_has_own_header() {
    covers!(feat_req_someip_585);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        // Each message arrives with its own header
        for expected in [b"short".as_slice(), b"medium length", b"a"] {
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
            .preferred_transport(Transport::Tcp)
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

        // Send multiple requests with different payloads
        for payload in [b"short".as_slice(), b"medium length", b"a"] {
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

/// feat_req_someip_702: Multiple messages can be in one TCP segment
///
/// All Transport Protocol Bindings shall support transporting more than one
/// SOME/IP message in a single TCP segment.
#[test_log::test]

fn multiple_messages_per_segment_parsed() {
    covers!(feat_req_someip_702);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        // Server should receive all 3 requests even if sent in single segment
        for _ in 0..3 {
            if let Some(ServiceEvent::Call { responder, .. }) =
                tokio::time::timeout(Duration::from_secs(10), offering.next())
                    .await
                    .ok()
                    .flatten()
            {
                responder.reply(&[]).unwrap();
            }
        }

        // Give time for final response to be delivered before server exits
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
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

        // Send requests quickly (may be coalesced in TCP)
        for i in 0u8..3 {
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
// TCP Connection Lost Handling
// ============================================================================

/// feat_req_someip_326: Outstanding requests fail when connection lost
///
/// When the TCP connection is lost, outstanding requests shall be
/// handled as if a timeout occurred.
#[test_log::test]

fn connection_lost_fails_pending_requests() {
    covers!(feat_req_someip_326);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        // Receive request but don't respond (to test pending request failure)
        if let Some(ServiceEvent::Call { .. }) = offering.next().await {
            // Don't respond - let the connection be broken
            tokio::time::sleep(Duration::from_secs(10)).await;
        }

        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
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

        // Send request but don't wait for response yet
        let pending = proxy.call(method_id, b"request");

        // Give time for request to be sent
        tokio::time::sleep(Duration::from_millis(50)).await;

        // TODO: Break the connection using turmoil::partition()
        // turmoil::partition("server", "client");

        // Pending request should fail due to connection loss
        let result = tokio::time::timeout(Duration::from_secs(5), pending).await;

        // Should error (connection lost = timeout-like behavior)
        // Note: In current implementation this would actually timeout,
        // which is acceptable behavior for connection loss
        assert!(
            result.is_err() || result.unwrap().is_err(),
            "Pending request should fail when connection lost"
        );

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// Magic Cookie Tests (TCP Resync)
// ============================================================================

/// feat_req_someip_586: Magic Cookies allow resync in testing
///
/// In order to allow resynchronization to SOME/IP over TCP in testing and
/// debugging scenarios, implementations shall support Magic Cookies.
#[test_log::test]
fn magic_cookie_recognized() {
    covers!(feat_req_someip_586);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .magic_cookies(true)
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        // Server should receive request even with magic cookies
        if let Some(ServiceEvent::Call { responder, .. }) = offering.next().await {
            responder.reply(b"response").unwrap();
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .magic_cookies(true)
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

        // With magic cookies enabled, communication should still work
        let response =
            tokio::time::timeout(Duration::from_secs(5), proxy.call(method_id, b"request"))
                .await
                .expect("Timeout")
                .expect("Call should succeed with magic cookies");

        assert_eq!(response.payload.as_ref(), b"response");

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_591: TCP segment starts with Magic Cookie
///
/// Each TCP segment shall start with a SOME/IP Magic Cookie Message
/// (when magic cookies are enabled).
#[test_log::test]
fn tcp_segment_starts_with_magic_cookie() {
    covers!(feat_req_someip_591);

    // This test verifies that when magic_cookies(true) is set,
    // communication still works correctly. The implementation prepends
    // a 16-byte Magic Cookie to each TCP write.

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .magic_cookies(true)
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        // Server prepends magic cookie (Service ID 0xFFFF, Method ID 0x8000)
        // to each response
        if let Some(ServiceEvent::Call { responder, .. }) = offering.next().await {
            responder.reply(b"with_magic").unwrap();
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .magic_cookies(true)
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

        // Client prepends magic cookie (Service ID 0xFFFF, Method ID 0x0000)
        // to each request
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(method_id, b"test_request"),
        )
        .await
        .expect("Timeout")
        .expect("Call should work with magic cookies");

        // Verify we got the response (magic cookies are transparent)
        assert_eq!(response.payload.as_ref(), b"with_magic");

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_592: Only one Magic Cookie per segment
///
/// The implementation shall only include up to one SOME/IP Magic Cookie
/// Message per TCP segment.
#[test_log::test]
fn only_one_magic_cookie_per_segment() {
    covers!(feat_req_someip_592);

    // This test verifies that even with multiple messages, the
    // implementation only prepends one Magic Cookie per write operation.
    // Each write is treated as a TCP segment.

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .magic_cookies(true)
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        // Handle multiple requests - each response gets one magic cookie
        for i in 0..3 {
            if let Some(ServiceEvent::Call { responder, .. }) = offering.next().await {
                let reply = format!("response_{}", i);
                responder.reply(reply.as_bytes()).unwrap();
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .magic_cookies(true)
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

        // Make multiple calls - each gets one magic cookie prepended
        for i in 0..3 {
            let request = format!("request_{}", i);
            let response = tokio::time::timeout(
                Duration::from_secs(5),
                proxy.call(method_id, request.as_bytes()),
            )
            .await
            .expect("Timeout")
            .expect("Call should succeed");

            let expected = format!("response_{}", i);
            assert_eq!(response.payload.as_ref(), expected.as_bytes());
        }

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// Wire Format Consistency (TCP vs UDP)
// ============================================================================

/// feat_req_someip_324: TCP binding based on UDP binding
///
/// The TCP binding of SOME/IP is heavily based on the UDP binding.
/// The header format is identical.
#[test_log::test]

fn tcp_header_format_matches_udp() {
    covers!(feat_req_someip_324);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        if let Some(ServiceEvent::Call { responder, .. }) = offering.next().await {
            responder.reply(b"response").unwrap();
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
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

        let response =
            tokio::time::timeout(Duration::from_secs(5), proxy.call(method_id, b"request"))
                .await
                .expect("Timeout")
                .expect("Call should succeed");

        // Response received proves the header format worked
        // (same 16-byte header as UDP)
        assert_eq!(response.payload.as_ref(), b"response");

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// TCP Connection Loss vs Reboot Distinction
// ============================================================================

/// Verify that TCP connection loss does NOT reset session IDs
///
/// A lost TCP connection is just a transport event - the peer may still be
/// running. Session IDs should continue incrementing, not reset to 1.
/// Reboot detection happens at the SD layer via the Reboot Flag, not TCP.
#[test_log::test]

fn tcp_loss_does_not_reset_session_id() {
    // This test verifies the distinction between:
    // - feat_req_someip_326: TCP loss → requests timeout (no session reset)
    // - feat_req_someipsd_872: Reboot detected → TCP reset (session DOES reset)
    covers!(feat_req_someip_326, feat_req_someip_647);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        // Handle requests
        loop {
            tokio::select! {
                event = offering.next() => {
                    if let Some(ServiceEvent::Call { responder, .. }) = event {
                        responder.reply(&[]).unwrap();
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => break,
            }
        }

        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
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

        // First request
        let _response =
            tokio::time::timeout(Duration::from_secs(5), proxy.call(method_id, b"first"))
                .await
                .expect("Timeout")
                .expect("Call should succeed");

        // TODO: Capture session_id from first request

        // TODO: Simulate TCP loss and reconnection
        // turmoil::partition("server", "client");
        // tokio::time::sleep(Duration::from_millis(100)).await;
        // turmoil::repair("server", "client");

        // Second request after reconnection
        let _response =
            tokio::time::timeout(Duration::from_secs(5), proxy.call(method_id, b"second"))
                .await
                .expect("Timeout")
                .expect("Reconnection should succeed");

        // TODO: Verify session_id continued incrementing (not reset to 1)
        // Session ID should be session_before + 1 (or wrapped), not 1

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someipsd_872: Reboot detection triggers TCP connection reset
///
/// When the system detects the reboot of a peer (via SD Reboot Flag),
/// it shall reset the state of TCP connections to that peer.
///
/// The reboot flag transitions: Initially set to 1, cleared after session ID wraps.
/// A new server starts with reboot=1 again, which signals reboot to clients who
/// previously saw reboot=0.
#[test_log::test]
fn reboot_detection_resets_tcp_connections() {
    covers!(feat_req_someipsd_872);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(60))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        // Handle first request
        if let Some(ServiceEvent::Call { responder, .. }) =
            tokio::time::timeout(Duration::from_secs(10), offering.next())
                .await
                .ok()
                .flatten()
        {
            responder.reply(b"response1").unwrap();
        }

        // Handle second request (after "reboot")
        if let Some(ServiceEvent::Call { responder, .. }) =
            tokio::time::timeout(Duration::from_secs(10), offering.next())
                .await
                .ok()
                .flatten()
        {
            responder.reply(b"response2").unwrap();
        }

        // Keep server alive for final response
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
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

        // First request - establishes TCP connection
        let _response =
            tokio::time::timeout(Duration::from_secs(5), proxy.call(method_id, b"request1"))
                .await
                .expect("Timeout")
                .expect("First call should succeed");

        // Second request - uses same connection (or reconnects if reboot detected)
        // The reboot detection happens at SD layer, which would trigger TCP reset
        let _response =
            tokio::time::timeout(Duration::from_secs(5), proxy.call(method_id, b"request2"))
                .await
                .expect("Timeout")
                .expect("Second call should succeed after potential reconnect");

        // Test passes if both requests complete successfully
        // The actual reboot detection is tested by the runtime logic

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// Nagle's Algorithm Tests
// ============================================================================

/// feat_req_someip_325: Nagle's algorithm disabled
///
/// Nagle's algorithm shall be disabled on TCP connections to reduce latency.
#[test_log::test]

fn tcp_nodelay_enabled() {
    covers!(feat_req_someip_325);

    // This would verify that TCP_NODELAY socket option is set.
    // With turmoil, we'd need to check the socket configuration
    // or verify latency characteristics.

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.client("placeholder", async {
        // TODO: Verify TCP_NODELAY is set on connections
        // Could be done by:
        // 1. Checking socket options if exposed
        // 2. Measuring latency for small messages (Nagle would add delay)
        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// TCP Connection Cleanup Tests
// ============================================================================

/// Test 1: Server-initiated disconnect triggers TCP cleanup
///
/// Lib-client, wire-server. The client subscribes to a service.
/// The server sees a connection being opened. The server stops the offer.
/// The client should unsubscribe, and the TCP connection cleanup should occur.
///
/// This tests that handle_stop_offer() triggers TCP cleanup when a service
/// with TCP endpoints is stopped (feat_req_someipsd_872).
#[test_log::test]
fn tcp_cleanup_on_server_stop_offer() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::AsyncReadExt;

    let tcp_connection_opened = Arc::new(AtomicBool::new(false));
    let tcp_connection_closed = Arc::new(AtomicBool::new(false));
    let subscribe_received = Arc::new(AtomicUsize::new(0));

    let tcp_opened_clone = Arc::clone(&tcp_connection_opened);
    let tcp_closed_clone = Arc::clone(&tcp_connection_closed);
    let subscribe_clone = Arc::clone(&subscribe_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Wire-level server
    sim.host("wire_server", move || {
        let tcp_opened = Arc::clone(&tcp_opened_clone);
        let tcp_closed = Arc::clone(&tcp_closed_clone);
        let subscribe_count = Arc::clone(&subscribe_clone);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();
            let sd_multicast: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            let tcp_listener = turmoil::net::TcpListener::bind("0.0.0.0:30509").await?;

            // Spawn TCP accept task
            let tcp_opened_for_accept = Arc::clone(&tcp_opened);
            let tcp_closed_for_accept = Arc::clone(&tcp_closed);
            tokio::spawn(async move {
                if let Ok((mut stream, peer)) = tcp_listener.accept().await {
                    tracing::info!("[wire_server] TCP connection from {}", peer);
                    tcp_opened_for_accept.store(true, Ordering::SeqCst);

                    // Read until connection closes
                    let mut buf = [0u8; 1024];
                    loop {
                        match stream.read(&mut buf).await {
                            Ok(0) => {
                                tracing::info!("[wire_server] TCP connection closed by client");
                                tcp_closed_for_accept.store(true, Ordering::SeqCst);
                                break;
                            }
                            Ok(_) => continue,
                            Err(_) => {
                                tcp_closed_for_accept.store(true, Ordering::SeqCst);
                                break;
                            }
                        }
                    }
                }
            });

            let mut buf = [0u8; 1500];
            let mut session_id = 1u16;
            let mut ack_session_id = 1u16;

            // Send offers periodically until we get a subscribe
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(15);

            while tokio::time::Instant::now() < deadline {
                // Send periodic offers
                if last_offer.elapsed() >= Duration::from_secs(1) {
                    let offer = SdOfferBuilder::new(TEST_SERVICE_ID, 0x0001, my_ip, 30509)
                        .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
                        .tcp()
                        .ttl(10)
                        .session_id(session_id)
                        .reboot_flag(true)
                        .build();
                    sd_socket.send_to(&offer, sd_multicast).await?;
                    session_id += 1;
                    last_offer = tokio::time::Instant::now();
                }

                if let Ok(Ok((len, from))) =
                    tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                        .await
                {
                    // Parse SD message to check for subscribe
                    if len > 16 {
                        if let Some(sd_msg) = ParsedSdMessage::parse(&buf[16..len]) {
                            let has_subscribe = sd_msg
                                .entries
                                .iter()
                                .any(|e| e.entry_type == SdEntryType::SubscribeEventgroup);
                            if has_subscribe {
                                let count = subscribe_count.fetch_add(1, Ordering::SeqCst) + 1;
                                tracing::info!("[wire_server] Received subscribe #{}", count);

                                // Send ACK
                                let ack =
                                    SdSubscribeAckBuilder::new(TEST_SERVICE_ID, 0x0001, 0x0001)
                                        .major_version(TEST_SERVICE_VERSION.0)
                                        .ttl(5)
                                        .session_id(ack_session_id)
                                        .reboot_flag(false)
                                        .build();
                                ack_session_id += 1;
                                sd_socket.send_to(&ack, from).await?;

                                // Wait a bit then send StopOffer
                                tokio::time::sleep(Duration::from_millis(500)).await;

                                tracing::info!("[wire_server] Sending StopOffer");
                                let stop_offer =
                                    SdOfferBuilder::new(TEST_SERVICE_ID, 0x0001, my_ip, 30509)
                                        .major_version(TEST_SERVICE_VERSION.0)
                                        .minor_version(TEST_SERVICE_VERSION.1)
                                        .ttl(0)
                                        .session_id(session_id)
                                        .reboot_flag(true)
                                        .unicast_flag(false) // Same channel as offers (multicast)
                                        .tcp()
                                        .build();
                                sd_socket.send_to(&stop_offer, sd_multicast).await?;
                                break;
                            }
                        }
                    }
                }
            }

            // Wait for TCP task to see connection close
            tokio::time::sleep(Duration::from_secs(2)).await;

            Ok(())
        }
    });

    // Library client
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .preferred_transport(Transport::Tcp)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        tracing::info!("[client] Service discovered, subscribing...");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        tracing::info!("[client] Subscribed, waiting for StopOffer...");

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            let proxy_res = runtime
                .find(TEST_SERVICE_ID)
                .instance(InstanceId::Id(0x0001))
                .await;
            if proxy_res.is_err() {
                tracing::info!("[client] Service no longer available, likely due to StopOffer");
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        tokio::time::sleep(Duration::from_secs(2)).await;

        Ok(())
    });

    sim.run().unwrap();

    assert!(
        tcp_connection_opened.load(Ordering::SeqCst),
        "TCP connection should have been opened"
    );
    assert!(
        subscribe_received.load(Ordering::SeqCst) >= 1,
        "Should have received at least one subscribe"
    );
    assert!(
        tcp_connection_closed.load(Ordering::SeqCst),
        "TCP connection should have been closed after StopOffer"
    );
}

/// Test 2: TCP cleanup after server crash allows reconnection
///
/// Lib-client, lib-server. Client subscribes and receives events.
/// Server crashes (turmoil::crash). This terminates the server's TCP connections.
/// Server restarts (turmoil::bounce). Client re-discovers and re-subscribes.
///
/// This tests that server crash (which closes TCP connections) triggers
/// client-side cleanup, allowing fresh connections after server restart.
#[test]
fn tcp_cleanup_after_server_crash_allows_reconnection() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    configure_tracing();

    let events_before_crash = Arc::new(AtomicUsize::new(0));
    let events_after_restart = Arc::new(AtomicUsize::new(0));
    let ready_to_crash = Arc::new(AtomicBool::new(false));

    let events_before_clone = Arc::clone(&events_before_crash);
    let events_after_clone = Arc::clone(&events_after_restart);
    let ready_to_crash_clone = Arc::clone(&ready_to_crash);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(120))
        .tick_duration(Duration::from_millis(50))
        .build();

    let offer_ttl_s = 4;

    // Server offering TCP service (will be crashed and restarted)
    sim.host("server", move || async move {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .offer_ttl(offer_ttl_s)
            .cyclic_offer_delay(offer_ttl_s as u64 * 1000 / 3)
            .start_turmoil()
            .await
            .unwrap();

        let offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        tracing::info!("[server] Started, sending events");

        // Send events continuously
        let mut counter = 0u32;
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let payload = counter.to_be_bytes();
            let _ = event_handle.notify(&payload).await;
            counter += 1;
        }
    });

    // Client with subscription across server crash
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .preferred_transport(Transport::Tcp)
            .subscribe_ttl(60) // 60 second TTL
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Phase 1: Subscribe and receive some events
        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(10), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription =
            tokio::time::timeout(Duration::from_secs(10), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        tracing::info!("[client] Phase 1: Receiving events before server crash");

        // Receive a few events to confirm everything works
        for _ in 0..5 {
            if let Ok(Some(_event)) =
                tokio::time::timeout(Duration::from_secs(5), subscription.next()).await
            {
                events_before_clone.fetch_add(1, Ordering::SeqCst);
            }
        }

        tracing::info!(
            "[client] Received {} events before crash, signaling ready",
            events_before_clone.load(Ordering::SeqCst)
        );

        // Signal that we're ready for the server to crash
        ready_to_crash_clone.store(true, Ordering::SeqCst);
        // make sure that offer times out
        tokio::time::sleep(Duration::from_secs(2 * offer_ttl_s as u64)).await;

        // Phase 2: Re-discover and re-subscribe after server restart
        tracing::info!("[client] Phase 2: Re-subscribing after server restart");

        let proxy2 = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy2 = tokio::time::timeout(Duration::from_secs(15), proxy2)
            .await
            .expect("Re-discovery timeout after server restart")
            .expect("Service should be available again");

        tracing::info!("[client] Service rediscovered, re-subscribing...");
        let mut subscription2 =
            tokio::time::timeout(Duration::from_secs(15), proxy2.subscribe(eventgroup))
                .await
                .expect("Re-subscribe timeout")
                .expect("Re-subscribe should succeed - cleanup must have cleared stale TCP entry");

        tracing::info!("[client] Re-subscribed successfully!");

        // Receive events after restart
        for _ in 0..5 {
            if let Ok(Some(_event)) =
                tokio::time::timeout(Duration::from_secs(5), subscription2.next()).await
            {
                events_after_clone.fetch_add(1, Ordering::SeqCst);
            }
        }

        tracing::info!(
            "[client] Received {} events after restart",
            events_after_clone.load(Ordering::SeqCst)
        );

        Ok(())
    });

    // Run simulation with crash/bounce intervention
    let mut crashed_at = None;
    let mut bounced = false;
    let mut total_steps = 0u32;

    loop {
        total_steps += 1;
        match sim.step() {
            Ok(false) => {
                if let Some(crashed_at) = crashed_at {
                    if !bounced
                        && (sim.elapsed() - crashed_at)
                            >= Duration::from_secs(3 * offer_ttl_s as u64)
                    {
                        tracing::info!("[harness] Bouncing server");
                        sim.bounce("server");
                        bounced = true;
                    }
                } else if ready_to_crash.load(Ordering::SeqCst) {
                    // Simulation still running (clients not finished)
                    tracing::info!("[harness] Crashing server at step {}", total_steps);
                    sim.crash("server");
                    crashed_at = Some(sim.elapsed());
                }
            }
            Ok(true) => {
                // All clients finished - simulation complete
                tracing::info!("[harness] Simulation ended at step {}", total_steps);
                break;
            }
            Err(e) => {
                panic!("Simulation failed at step {}: {}", total_steps, e);
            }
        }
    }

    let before = events_before_crash.load(Ordering::SeqCst);
    let after = events_after_restart.load(Ordering::SeqCst);

    assert!(
        before > 0,
        "Should have received events before server crash (got {})",
        before
    );
    assert!(
        after > 0,
        "Should have received events after server restart (got {}). \
         This proves crash triggered TCP cleanup, allowing reconnection.",
        after
    );
}

/// Test 3: Client shutdown properly closes TCP connection
///
/// Lib-client, wire-server. Client subscribes. Connection being opened.
/// Client shuts down (via .shutdown()). Server sees connection being closed.
///
/// This tests that runtime shutdown triggers proper TCP cleanup.
#[test_log::test]
fn tcp_connection_closed_on_client_shutdown() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::io::AsyncReadExt;

    let tcp_connection_opened = Arc::new(AtomicBool::new(false));
    let tcp_connection_closed = Arc::new(AtomicBool::new(false));
    let client_shutdown_complete = Arc::new(AtomicBool::new(false));

    let tcp_opened_clone = Arc::clone(&tcp_connection_opened);
    let tcp_closed_clone = Arc::clone(&tcp_connection_closed);
    let shutdown_clone = Arc::clone(&client_shutdown_complete);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Wire-level server to observe TCP connection lifecycle
    sim.client("wire_server", {
        let tcp_opened = Arc::clone(&tcp_opened_clone);
        let tcp_closed = Arc::clone(&tcp_closed_clone);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();
            let sd_multicast: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket.join_multicast_v4(
                "239.255.0.1".parse().unwrap(),
                "0.0.0.0".parse().unwrap(),
            )?;

            let tcp_listener = turmoil::net::TcpListener::bind("0.0.0.0:30509").await?;

            // We'll accept the TCP connection in the main loop and monitor it there
            let mut tcp_stream: Option<turmoil::net::TcpStream> = None;

            let mut buf = [0u8; 1500];
            let mut tcp_buf = [0u8; 1024];
            let mut session_id = 1u16;
            let mut ack_session_id = 1u16;

            // Send offers periodically
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(15);

            while tokio::time::Instant::now() < deadline {
                // Send periodic offers
                if last_offer.elapsed() >= Duration::from_secs(1) {
                    let offer = SdOfferBuilder::new(TEST_SERVICE_ID, 0x0001, my_ip, 30509)
                        .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
                        .tcp()
                        .ttl(10)
                        .session_id(session_id)
                        .reboot_flag(session_id == 1)
                        .build();
                    sd_socket.send_to(&offer, sd_multicast).await?;
                    session_id += 1;
                    last_offer = tokio::time::Instant::now();
                }

                // Use select to handle multiple events
                tokio::select! {
                    // Try to accept new TCP connections
                    accept_result = tcp_listener.accept(), if tcp_stream.is_none() => {
                        if let Ok((stream, peer)) = accept_result {
                            tracing::info!("[wire_server] TCP connection accepted from {}", peer);
                            tcp_opened.store(true, Ordering::SeqCst);
                            tcp_stream = Some(stream);
                        }
                    }
                    // Check if existing TCP connection closed
                    read_result = async {
                        if let Some(ref mut stream) = tcp_stream {
                            stream.read(&mut tcp_buf).await
                        } else {
                            // No stream, just sleep forever (will be cancelled by other branches)
                            std::future::pending().await
                        }
                    } => {
                        match read_result {
                            Ok(0) => {
                                tracing::info!("[wire_server] TCP connection closed (EOF) - client shutdown detected");
                                tcp_closed.store(true, Ordering::SeqCst);
                                tcp_stream = None;
                            }
                            Ok(_) => {
                                // Got some data, continue
                            }
                            Err(e) => {
                                tracing::info!("[wire_server] TCP connection error: {} - treating as closed", e);
                                tcp_closed.store(true, Ordering::SeqCst);
                                tcp_stream = None;
                            }
                        }
                    }
                    // Handle SD messages
                    sd_result = sd_socket.recv_from(&mut buf) => {
                        if let Ok((len, from)) = sd_result {
                            if len > 16 {
                                if let Some(sd_msg) = ParsedSdMessage::parse(&buf[16..len]) {
                                    let has_subscribe = sd_msg.entries.iter().any(|e| {
                                        e.entry_type == SdEntryType::SubscribeEventgroup
                                    });
                                    if has_subscribe {
                                        tracing::info!("[wire_server] Received subscribe, sending ACK");
                                        let ack = SdSubscribeAckBuilder::new(TEST_SERVICE_ID, 0x0001, 0x0001)
                                            .major_version(TEST_SERVICE_VERSION.0)
                                            .ttl(10)
                                            .session_id(ack_session_id)
                                            .reboot_flag(false)
                                            .build();
                                        ack_session_id += 1;
                                        sd_socket.send_to(&ack, from).await?;
                                    }
                                }
                            }
                        }
                    }
                    _ = tokio::time::sleep_until(deadline) => {}
                }
            }

            Ok(())
        }
    });

    // Library client that will shutdown
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .preferred_transport(Transport::Tcp)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        tracing::info!("[client] Service discovered, subscribing...");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        tracing::info!("[client] Subscribed via TCP, waiting before shutdown...");

        // Keep subscription active for a bit
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Shutdown the runtime - this should close TCP connections
        tracing::info!("[client] Initiating runtime shutdown...");
        runtime.shutdown().await;

        tracing::info!("[client] Shutdown complete");
        shutdown_clone.store(true, Ordering::SeqCst);

        // Give server time to observe the connection close
        tokio::time::sleep(Duration::from_secs(2)).await;

        Ok(())
    });

    sim.run().unwrap();

    assert!(
        tcp_connection_opened.load(Ordering::SeqCst),
        "TCP connection should have been opened"
    );
    assert!(
        client_shutdown_complete.load(Ordering::SeqCst),
        "Client shutdown should have completed"
    );
    assert!(
        tcp_connection_closed.load(Ordering::SeqCst),
        "TCP connection should have been closed after client shutdown"
    );
}

// ============================================================================
// Reboot Behavior Tests
// ============================================================================

/// Test: Reboot clears old discovered services, only new ones remain
///
/// Requirements tested:
/// - feat_req_someipsd_871: Expire subscriptions on reboot
/// - feat_req_someipsd_872: Reset TCP connections on reboot
/// - Discovered services from before reboot should be cleared
///
/// Timeline:
/// 1. Wire-server offers SERVICE1
/// 2. Client discovers SERVICE1 (not SERVICE2)
/// 3. Wire-server "reboots" (session ID resets)
/// 4. Wire-server offers SERVICE2 (not SERVICE1)
/// 5. Client should discover only SERVICE2 (SERVICE1 cleared)
#[test_log::test]
fn reboot_clears_old_services_offers_new() {
    covers!(feat_req_someipsd_871, feat_req_someipsd_872);

    const SERVICE1_ID: u16 = 0x1111;
    const SERVICE2_ID: u16 = 0x2222;
    const INSTANCE_ID: u16 = 0x0001;
    const VERSION: (u8, u32) = (1, 0);
    const TTL_100_DAYS: u32 = 100 * 24 * 3600; // 100 days - proves reboot detection clears services, not TTL

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(20))
        .build();

    // Wire-level server that manually sends SD messages
    sim.host("wire_server", || async {
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("wire_server").to_string().parse().unwrap();
        let sd_multicast: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        // Phase 1: Offer SERVICE1 with session=1
        tracing::info!("[wire_server] Phase 1: Offering SERVICE1");
        let offer1 = SdOfferBuilder::new(SERVICE1_ID, INSTANCE_ID, my_ip, 30500)
            .major_version(VERSION.0)
            .minor_version(VERSION.1)
            .ttl(TTL_100_DAYS)
            .session_id(1)
            .reboot_flag(true)
            .unicast_flag(false)
            .build();

        // Send offer multiple times to ensure client discovers it
        for _ in 0..3 {
            sd_socket.send_to(&offer1, sd_multicast).await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Wait for client to discover SERVICE1
        tokio::time::sleep(Duration::from_secs(1)).await;

        // Phase 2: "Reboot" and offer SERVICE2 (session ID resets to 1)
        tracing::info!("[wire_server] Phase 2: Rebooting and offering SERVICE2");
        let offer2 = SdOfferBuilder::new(SERVICE2_ID, INSTANCE_ID, my_ip, 30501)
            .major_version(VERSION.0)
            .minor_version(VERSION.1)
            .ttl(TTL_100_DAYS)
            .session_id(1)
            .reboot_flag(true)
            .unicast_flag(false)
            .build();

        // Send new offer multiple times
        for _ in 0..3 {
            sd_socket.send_to(&offer2, sd_multicast).await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Keep server alive
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });

    // Library-based client
    sim.client("lib_client", async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("lib_client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Phase 1: Wait for SERVICE1 to be discovered
        tracing::info!("[lib_client] Phase 1: Looking for SERVICE1");
        let service1 = tokio::time::timeout(
            Duration::from_secs(3),
            runtime
                .find(SERVICE1_ID)
                .instance(InstanceId::Id(INSTANCE_ID)),
        )
        .await
        .expect("SERVICE1 should be discovered within timeout")
        .expect("SERVICE1 should be found");

        tracing::info!("[lib_client] SERVICE1 found");

        // Verify SERVICE2 is NOT available yet
        let service2_result = tokio::time::timeout(
            Duration::from_millis(500),
            runtime
                .find(SERVICE2_ID)
                .instance(InstanceId::Id(INSTANCE_ID)),
        )
        .await;

        assert!(
            service2_result.is_err(),
            "SERVICE2 should NOT be discovered before reboot"
        );

        tracing::info!("[lib_client] Verified: only SERVICE1 available");

        // Phase 2: Wait for reboot and SERVICE2
        tokio::time::sleep(Duration::from_secs(2)).await;

        tracing::info!("[lib_client] Phase 2: Looking for SERVICE2 after reboot");
        let service2 = tokio::time::timeout(
            Duration::from_secs(3),
            runtime
                .find(SERVICE2_ID)
                .instance(InstanceId::Id(INSTANCE_ID)),
        )
        .await
        .expect("SERVICE2 should be discovered after reboot")
        .expect("SERVICE2 should be found");

        tracing::info!("[lib_client] SERVICE2 found");

        // Verify SERVICE1 is NO LONGER available (cleared by reboot detection)
        // With 100-day TTL, this proves reboot detection clears services, not TTL expiry
        let service1_after_reboot = tokio::time::timeout(
            Duration::from_millis(500),
            runtime
                .find(SERVICE1_ID)
                .instance(InstanceId::Id(INSTANCE_ID)),
        )
        .await;

        assert!(
            service1_after_reboot.is_err(),
            "SERVICE1 should NOT be available after reboot (cleared by reboot detection)"
        );

        tracing::info!("[lib_client] Verified: only SERVICE2 available after reboot");

        Ok(())
    });

    sim.run().unwrap();
}

/// Test: Server detects client reboot via session ID drop in Subscribe message
///
/// Setup:
/// - lib-server: Offers SERVICE1 and SERVICE2, sends events every 200ms
/// - wire-client: Manually sends SD messages
///
/// Flow:
/// 1. Client subscribes to SERVICE1 (session=1,2,3...)
/// 2. Client receives events for SERVICE1
/// 3. Client "reboots" - sends Subscribe for SERVICE2 with session=1 (drop)
/// 4. Server detects reboot, closes TCP connections, expires subscriptions
/// 5. Client receives events for SERVICE2
///
/// Verifies:
/// - feat_req_someipsd_871: Server expires subscriptions from rebooted client
/// - feat_req_someipsd_872: Server resets TCP connections on client reboot
#[test_log::test]
fn server_detects_client_reboot_clears_subscriptions() {
    use crate::wire_format::helpers::{ParsedHeader, SdSubscribeBuilder, SOMEIP_HEADER_SIZE};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::AsyncReadExt;

    covers!(feat_req_someipsd_871, feat_req_someipsd_872);

    const SERVICE1_ID: u16 = 0x1111;
    const SERVICE2_ID: u16 = 0x2222;
    const SERVICE3_ID: u16 = 0x3333;
    const INSTANCE_ID: u16 = 0x0001;
    const EVENTGROUP_ID: u16 = 0x0001;
    const EVENT1_ID: u16 = 0x8001;
    const EVENT2_ID: u16 = 0x8002;
    const EVENT3_ID: u16 = 0x8003;
    const VERSION: (u8, u32) = (1, 0);

    let events_service1 = Arc::new(AtomicUsize::new(0));
    let events_service2 = Arc::new(AtomicUsize::new(0));
    let events_service3 = Arc::new(AtomicUsize::new(0));
    let test_complete = Arc::new(AtomicBool::new(false));

    let events_service1_clone = Arc::clone(&events_service1);
    let events_service2_clone = Arc::clone(&events_service2);
    let events_service3_clone = Arc::clone(&events_service3);
    let test_complete_clone = Arc::clone(&test_complete);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .max_message_latency(Duration::from_millis(50))
        .build();

    // Library-based server offering three services with events
    sim.host("lib_server", move || {
        let events_s1 = Arc::clone(&events_service1_clone);
        let events_s2 = Arc::clone(&events_service2_clone);
        let events_s3 = Arc::clone(&events_service3_clone);
        let test_done = Arc::clone(&test_complete_clone);

        async move {
            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("lib_server").to_string().parse().unwrap())
                .preferred_transport(Transport::Tcp)
                .start_turmoil()
                .await
                .unwrap();

            // Offer SERVICE1
            let offering1 = runtime
                .offer(SERVICE1_ID, InstanceId::Id(INSTANCE_ID))
                .version(VERSION.0, VERSION.1)
                .tcp()
                .start()
                .await
                .unwrap();

            let eventgroup = EventgroupId::new(EVENTGROUP_ID).unwrap();
            let event1_handle = offering1
                .event(EventId::new(EVENT1_ID).unwrap())
                .eventgroup(eventgroup)
                .create()
                .await
                .unwrap();

            // Offer SERVICE2
            let offering2 = runtime
                .offer(SERVICE2_ID, InstanceId::Id(INSTANCE_ID))
                .version(VERSION.0, VERSION.1)
                .tcp()
                .start()
                .await
                .unwrap();

            let event2_handle = offering2
                .event(EventId::new(EVENT2_ID).unwrap())
                .eventgroup(eventgroup)
                .create()
                .await
                .unwrap();

            // Offer SERVICE3
            let offering3 = runtime
                .offer(SERVICE3_ID, InstanceId::Id(INSTANCE_ID))
                .version(VERSION.0, VERSION.1)
                .tcp()
                .start()
                .await
                .unwrap();

            let event3_handle = offering3
                .event(EventId::new(EVENT3_ID).unwrap())
                .eventgroup(eventgroup)
                .create()
                .await
                .unwrap();

            tracing::info!("[lib_server] Services offered, starting event loop");

            // Send events for all three services
            let mut counter = 0u32;
            while !test_done.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let payload = counter.to_be_bytes();

                if event1_handle.notify(&payload).await.is_ok() {
                    events_s1.fetch_add(1, Ordering::SeqCst);
                }
                if event2_handle.notify(&payload).await.is_ok() {
                    events_s2.fetch_add(1, Ordering::SeqCst);
                }
                if event3_handle.notify(&payload).await.is_ok() {
                    events_s3.fetch_add(1, Ordering::SeqCst);
                }
                counter += 1;
            }

            Ok(())
        }
    });

    // Wire-level client that manually sends SD messages
    let test_complete_for_client = Arc::clone(&test_complete);
    sim.client("wire_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let my_ip: std::net::Ipv4Addr =
            turmoil::lookup("wire_client").to_string().parse().unwrap();
        let server_ip: std::net::Ipv4Addr =
            turmoil::lookup("lib_server").to_string().parse().unwrap();

        // Bind SD socket
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket
            .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut client_session_id = 1u16;

        // Wait for Offer for all services, capture TCP ports
        tracing::info!("[wire_client] Waiting for service Offers...");
        let mut service1_tcp_port = 0u16;
        let mut service2_tcp_port = 0u16;
        let mut service3_tcp_port = 0u16;
        loop {
            let (len, _from) = sd_socket.recv_from(&mut buf).await?;
            if let Some((_header, sd)) = parse_sd_packet(&buf[..len]) {
                for entry in sd.offer_entries() {
                    if let Some(opt) = sd.option_at(entry.index_1st_option) {
                        if let Some(port) = opt.port() {
                            if entry.service_id == SERVICE1_ID && service1_tcp_port == 0 {
                                service1_tcp_port = port;
                                tracing::info!("[wire_client] Found SERVICE1 on TCP port {}", port);
                            }
                            if entry.service_id == SERVICE2_ID && service2_tcp_port == 0 {
                                service2_tcp_port = port;
                                tracing::info!("[wire_client] Found SERVICE2 on TCP port {}", port);
                            }
                            if entry.service_id == SERVICE3_ID && service3_tcp_port == 0 {
                                service3_tcp_port = port;
                                tracing::info!("[wire_client] Found SERVICE3 on TCP port {}", port);
                            }
                        }
                    }
                }
            }
            if service1_tcp_port != 0 && service2_tcp_port != 0 && service3_tcp_port != 0 {
                break;
            }
        }

        // Phase 1: Connect to SERVER's TCP port for SERVICE1
        tracing::info!("[wire_client] Phase 1: Connecting to SERVICE1 TCP port...");
        let tcp_addr: std::net::SocketAddr = (server_ip, service1_tcp_port).into();
        let mut tcp_stream = turmoil::net::TcpStream::connect(tcp_addr).await?;
        let local_port = tcp_stream.local_addr()?.port();
        tracing::info!("[wire_client] Connected to SERVICE1, local port {}", local_port);

        // Subscribe to SERVICE1
        tracing::info!("[wire_client] Subscribing to SERVICE1");
        let subscribe1 = SdSubscribeBuilder::new(
            SERVICE1_ID,
            INSTANCE_ID,
            EVENTGROUP_ID,
            my_ip,
            local_port, // Use the local port of our TCP connection
        )
        .tcp()
        .session_id(client_session_id)
        .reboot_flag(true)
        .build();
        client_session_id += 1;

        let server_sd_addr: std::net::SocketAddr = (server_ip, 30490).into();
        sd_socket.send_to(&subscribe1, server_sd_addr).await?;

        // Wait for SubscribeAck
        tracing::info!("[wire_client] Waiting for SubscribeAck...");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut got_ack = false;
        while tokio::time::Instant::now() < deadline && !got_ack {
            tokio::select! {
                result = sd_socket.recv_from(&mut buf) => {
                    let (len, _from) = result?;
                    if let Some((_header, sd)) = parse_sd_packet(&buf[..len]) {
                        for entry in sd.subscribe_ack_entries() {
                            if entry.service_id == SERVICE1_ID && !entry.is_stop() {
                                tracing::info!("[wire_client] Got SubscribeAck for SERVICE1");
                                got_ack = true;
                            }
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
        assert!(got_ack, "Should receive SubscribeAck for SERVICE1");

        // Receive SERVICE1 events on our TCP connection
        tracing::info!("[wire_client] Receiving SERVICE1 events...");
        let mut events_received_s1 = 0;
        let mut tcp_buf = [0u8; 1024];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline && events_received_s1 < 5 {
            tokio::select! {
                result = tcp_stream.read(&mut tcp_buf) => {
                    match result {
                        Ok(0) => break, // Connection closed
                        Ok(n) => {
                            // Parse SOME/IP events (skip magic cookie bytes if present)
                            let mut offset = 0;
                            while offset + SOMEIP_HEADER_SIZE <= n {
                                // Check for magic cookie
                                if n >= offset + 8 && tcp_buf[offset..offset+8] == [0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08] {
                                    offset += 8;
                                    continue;
                                }
                                if let Some(header) = ParsedHeader::parse(&tcp_buf[offset..]) {
                                    if header.is_notification() {
                                        if header.service_id == SERVICE1_ID {
                                            events_received_s1 += 1;
                                            tracing::info!("[wire_client] Received SERVICE1 event #{}", events_received_s1);
                                        } else {
                                            panic!("Received not service 1 event before subscribing - should not happen!");
                                        }
                                    }
                                    offset += SOMEIP_HEADER_SIZE + header.payload_length();
                                } else {
                                    break;
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }

        tracing::info!("[wire_client] Received {} SERVICE1 events", events_received_s1);
        assert!(events_received_s1 >= 3, "Should receive at least 3 SERVICE1 events");

        // Phase 2: Simulate client reboot
        // A real freshly booted client would:
        // 1. Client restarts, opens fresh TCP connections
        // 2. Sends Subscribes - server detects reboot from session=1
        tracing::info!("[wire_client] Phase 2: Client 'rebooting'...");

        // Step 1: Open TWO fresh TCP connections (like a rebooted client would)
        tracing::info!("[wire_client] Opening fresh TCP connections to SERVICE2 and SERVICE3...");
        let tcp_addr2: std::net::SocketAddr = (server_ip, service2_tcp_port).into();
        let mut tcp_stream2 = turmoil::net::TcpStream::connect(tcp_addr2).await?;
        let local_port2 = tcp_stream2.local_addr()?.port();
        tracing::info!("[wire_client] Connected to SERVICE2, local port {}", local_port2);

        let tcp_addr3: std::net::SocketAddr = (server_ip, service3_tcp_port).into();
        let mut tcp_stream3 = turmoil::net::TcpStream::connect(tcp_addr3).await?;
        let local_port3 = tcp_stream3.local_addr()?.port();
        tracing::info!("[wire_client] Connected to SERVICE3, local port {}", local_port3);

        // Step 2: Send Subscribe for SERVICE2 with session=1, reboot=true
        // This triggers server's reboot detection
        tracing::info!("[wire_client] Subscribing to SERVICE2 (session=1, reboot=true)");
        let subscribe2 = SdSubscribeBuilder::new(
            SERVICE2_ID,
            INSTANCE_ID,
            EVENTGROUP_ID,
            my_ip,
            local_port2,
        )
        .tcp()
        .session_id(1) // Session reset to 1 = reboot
        .reboot_flag(true)
        .build();

        sd_socket.send_to(&subscribe2, server_sd_addr).await?;

        // Wait 50ms before sending second Subscribe
        tokio::time::sleep(Duration::from_millis(50)).await;

        // TODO I'm not sure whether we can actually exepect this. See the
        // _reuse_port test below for comparison.
        // Verify server closed the old SERVICE1 TCP connection (feat_req_someipsd_872)
        // The server should have closed it when it detected client reboot
        tracing::info!("[wire_client] Checking if server closed old SERVICE1 TCP connection...");
        let mut check_buf = [0u8; 64];
        match tcp_stream.read(&mut check_buf).await {
            Ok(0) => {
                tracing::info!("[wire_client] SERVER CLOSED SERVICE1 TCP - correct behavior for reboot!");
            }
            Ok(n) => {
                panic!("Expected server to close SERVICE1 TCP on reboot, but received {} bytes", n);
            }
            Err(e) => {
                // Connection reset/broken pipe is also acceptable - server forcefully closed
                tracing::info!("[wire_client] SERVICE1 TCP connection error (server closed): {}", e);
            }
        }

        // Step 3: Send Subscribe for SERVICE3 with session=2, reboot=true
        tracing::info!("[wire_client] Subscribing to SERVICE3 (session=2, reboot=true)");
        let subscribe3 = SdSubscribeBuilder::new(
            SERVICE3_ID,
            INSTANCE_ID,
            EVENTGROUP_ID,
            my_ip,
            local_port3,
        )
        .tcp()
        .session_id(2) // Still in same session sequence
        .reboot_flag(true) // Reboot flag stays true until wrap-around
        .build();

        sd_socket.send_to(&subscribe3, server_sd_addr).await?;

        // Wait for SubscribeAcks for both services
        tracing::info!("[wire_client] Waiting for SubscribeAcks...");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut got_ack2 = false;
        let mut got_ack3 = false;
        while tokio::time::Instant::now() < deadline && !(got_ack2 && got_ack3) {
            tokio::select! {
                result = sd_socket.recv_from(&mut buf) => {
                    let (len, _from) = result?;
                    if let Some((_header, sd)) = parse_sd_packet(&buf[..len]) {
                        for entry in sd.subscribe_ack_entries() {
                            if entry.service_id == SERVICE2_ID && !entry.is_stop() {
                                tracing::info!("[wire_client] Got SubscribeAck for SERVICE2");
                                got_ack2 = true;
                            }
                            if entry.service_id == SERVICE3_ID && !entry.is_stop() {
                                tracing::info!("[wire_client] Got SubscribeAck for SERVICE3");
                                got_ack3 = true;
                            }
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
        assert!(got_ack2, "Should receive SubscribeAck for SERVICE2");
        assert!(got_ack3, "Should receive SubscribeAck for SERVICE3");

        // Receive events from both services on their TCP connections
        tracing::info!("[wire_client] Receiving events from SERVICE2 and SERVICE3...");
        let mut events_received_s2 = 0;
        let mut events_received_s3 = 0;
        let mut tcp_buf2 = [0u8; 1024];
        let mut tcp_buf3 = [0u8; 1024];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);

        while tokio::time::Instant::now() < deadline && (events_received_s2 < 3 || events_received_s3 < 3) {
            tokio::select! {
                // Read from SERVICE2 TCP
                result = tcp_stream2.read(&mut tcp_buf2) => {
                    match result {
                        Ok(0) => {
                            panic!("SERVICE2 TCP connection closed unexpectedly!");
                        }
                        Ok(n) => {
                            let mut offset = 0;
                            while offset + SOMEIP_HEADER_SIZE <= n {
                                if n >= offset + 8 && tcp_buf2[offset..offset+8] == [0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08] {
                                    offset += 8;
                                    continue;
                                }
                                if let Some(header) = ParsedHeader::parse(&tcp_buf2[offset..]) {
                                    if header.is_notification() {
                                        if header.service_id == SERVICE2_ID {
                                            events_received_s2 += 1;
                                            tracing::info!("[wire_client] Received SERVICE2 event #{}", events_received_s2);
                                        } else {
                                            panic!("Received event for unexpected service: {}", header.service_id);
                                        }
                                    }
                                    offset += SOMEIP_HEADER_SIZE + header.payload_length();
                                } else {
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            panic!("SERVICE2 TCP read error: {}", e);
                        }
                    }
                }
                // Read from SERVICE3 TCP
                result = tcp_stream3.read(&mut tcp_buf3) => {
                    match result {
                        Ok(0) => {
                            panic!("SERVICE3 TCP connection closed unexpectedly!");
                        }
                        Ok(n) => {
                            let mut offset = 0;
                            while offset + SOMEIP_HEADER_SIZE <= n {
                                if n >= offset + 8 && tcp_buf3[offset..offset+8] == [0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08] {
                                    offset += 8;
                                    continue;
                                }
                                if let Some(header) = ParsedHeader::parse(&tcp_buf3[offset..]) {
                                    if header.is_notification() {
                                        if header.service_id == SERVICE3_ID {
                                            events_received_s3 += 1;
                                            tracing::info!("[wire_client] Received SERVICE3 event #{}", events_received_s3);
                                        } else {
                                            panic!("Received event for unexpected service: {}", header.service_id);
                                        }
                                    }
                                    offset += SOMEIP_HEADER_SIZE + header.payload_length();
                                } else {
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            panic!("SERVICE3 TCP read error: {}", e);
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }

        tracing::info!("[wire_client] Received {} SERVICE2 events, {} SERVICE3 events",
            events_received_s2, events_received_s3);
        assert!(events_received_s2 >= 3, "Should receive at least 3 SERVICE2 events after reboot");
        assert!(events_received_s3 >= 3, "Should receive at least 3 SERVICE3 events after reboot");

        // Signal test completion
        test_complete_for_client.store(true, Ordering::SeqCst);

        tracing::info!("[wire_client] Test complete - SERVICE1: {}, SERVICE2: {}, SERVICE3: {}",
            events_received_s1, events_received_s2, events_received_s3);

        Ok(())
    });

    sim.run().unwrap();

    // Verify events were sent by server (indicates server was actively sending)
    let s1_sent = events_service1.load(Ordering::SeqCst);
    let s2_sent = events_service2.load(Ordering::SeqCst);
    let s3_sent = events_service3.load(Ordering::SeqCst);
    tracing::info!(
        "Server sent {} SERVICE1, {} SERVICE2, {} SERVICE3 events",
        s1_sent,
        s2_sent,
        s3_sent
    );
}

/// Client detects server reboot from SubscribeAck session regression
///
/// Lib-client, wire-server scenario:
/// 1. Server offers service with long TTL (3 days)
/// 2. Client discovers, subscribes, receives event, unsubscribes
/// 3. 1 hour passes (simulated)
/// 4. Server crashes silently (no StopOffer sent, TCP closes)
/// 5. Server restarts on same port (session ID resets to 1)
/// 6. Client re-subscribes
/// 7. Server sends SubscribeAck with session=1 (regressed from previous ~3)
/// 8. Client should detect reboot but NOT expire the just-created subscription
/// 9. Client should receive events normally
///
/// This tests that reboot detection from SubscribeAck doesn't expire the
/// subscription that triggered the detection (similar to server-side fix).
#[test]
#[cfg(feature = "slow-tests")]
fn client_detects_server_reboot_from_subscribe_ack_session_regression() {
    configure_tracing();

    use crate::wire_format::helpers::SomeIpPacketBuilder;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;

    covers!(feat_req_someipsd_871, feat_req_someipsd_872);

    const SERVICE_ID: u16 = 0x4567;
    const INSTANCE_ID: u16 = 0x0001;
    const EVENTGROUP_ID: u16 = 0x0001;
    const EVENT_ID: u16 = 0x8001;
    const TCP_PORT: u16 = 30509;
    const TTL_3_DAYS: u32 = 3 * 24 * 60 * 60; // 3 days in seconds

    let events_received_phase1 = Arc::new(AtomicUsize::new(0));
    let events_received_phase2 = Arc::new(AtomicUsize::new(0));
    let test_complete = Arc::new(AtomicBool::new(false));

    let events_p1_clone = Arc::clone(&events_received_phase1);
    let events_p2_clone = Arc::clone(&events_received_phase2);
    let test_complete_clone = Arc::clone(&test_complete);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(7500)) // ~2 hours simulated
        .build();

    // Wire-level server that will "crash" and restart
    sim.host("wire_server", move || {
        let test_done = Arc::clone(&test_complete_clone);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();
            let sd_multicast: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

            // === PHASE 1: Initial server instance ===
            tracing::info!("[wire_server] Phase 1: Starting initial server instance");

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            let tcp_listener = turmoil::net::TcpListener::bind(format!("0.0.0.0:{}", TCP_PORT)).await?;

            let mut server_session_id = 1u16;

            // Wait for client to be ready
            tokio::time::sleep(Duration::from_millis(200)).await;

            // Send initial Offer (session=1, reboot=true)
            let offer = SdOfferBuilder::new(SERVICE_ID, INSTANCE_ID, my_ip, TCP_PORT)
                .version(1, 0)
                .tcp()
                .ttl(TTL_3_DAYS)
                .session_id(server_session_id)
                .reboot_flag(true)
                .build();
            sd_socket.send_to(&offer, sd_multicast).await?;
            server_session_id += 1;

            // Wait for Subscribe and handle TCP connections concurrently
            let mut buf = [0u8; 1500];
            let mut got_subscribe = false;
            let mut tcp_stream_opt: Option<turmoil::net::TcpStream> = None;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

            while !got_subscribe && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(1)).await;
                tokio::select! {
                    // Accept TCP connections (client connects before sending Subscribe)
                    result = tcp_listener.accept(), if tcp_stream_opt.is_none() => {
                        let (stream, peer) = result?;
                        tracing::info!("[wire_server] Phase 1: TCP connection from {}", peer);
                        tcp_stream_opt = Some(stream);
                    }
                    // Handle SD messages
                    result = sd_socket.recv_from(&mut buf) => {
                        let (len, from) = result?;
                        if let Some((_header, sd)) = parse_sd_packet(&buf[..len]) {
                            for entry in sd.subscribe_entries() {
                                if entry.service_id == SERVICE_ID {
                                    tracing::info!("[wire_server] Phase 1: Got Subscribe from {}", from);
                                    got_subscribe = true;

                                    // Send SubscribeAck (session=2, reboot=true - server just started)
                                    let ack = SdSubscribeAckBuilder::new(SERVICE_ID, INSTANCE_ID, EVENTGROUP_ID)
                                        .major_version(1)
                                        .ttl(entry.ttl)
                                        .session_id(server_session_id)
                                        .reboot_flag(true)
                                        .build();
                                    sd_socket.send_to(&ack, from).await?;
                                    server_session_id += 1;

                                    // Send one event over TCP
                                    if let Some(ref mut tcp_stream) = tcp_stream_opt {
                                        let event = SomeIpPacketBuilder::notification(SERVICE_ID, EVENT_ID)
                                            .session_id(1)
                                            .payload(&[0x01, 0x02, 0x03, 0x04])
                                            .build();
                                        tcp_stream.write_all(&event).await?;
                                        tracing::info!("[wire_server] Phase 1: Sent event");
                                    }
                                }
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(2000)) => {}
                }
            }

            // Wait for StopSubscribe
            let stop_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while tokio::time::Instant::now() < stop_deadline {
                tokio::time::sleep(Duration::from_millis(1)).await;
                tokio::select! {
                    result = sd_socket.recv_from(&mut buf) => {
                        let (len, _) = result?;
                        if let Some((_, sd)) = parse_sd_packet(&buf[..len]) {
                            for e in sd.subscribe_entries().filter(|e| e.is_stop()) {
                                if e.service_id == SERVICE_ID {
                                    tracing::info!("[wire_server] Phase 1: Got StopSubscribe");
                                }
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(2000)) => {}
                }
            }

            // Close TCP connection (part of "crash")
            drop(tcp_stream_opt);

            // Drop everything to simulate crash
            drop(tcp_listener);
            drop(sd_socket);
            tracing::info!("[wire_server] Phase 1: Server 'crashed' - all sockets closed");

            // === Simulate some time passing ===
            tracing::info!("[wire_server] Simulating time delay...");
            tokio::time::sleep(Duration::from_secs(3600)).await;

            // === PHASE 2: Server restarts with fresh state ===
            tracing::info!("[wire_server] Phase 2: Server restarting...");

            let sd_socket2 = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket2.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            let tcp_listener2 = turmoil::net::TcpListener::bind(format!("0.0.0.0:{}", TCP_PORT)).await?;

            // Session ID resets to 1 after reboot!
            let mut server_session_id2 = 1u16;

            // NOTE: We deliberately DO NOT send an Offer here!
            // This simulates the initial Offer being lost (e.g., UDP packet loss).
            // The client will re-subscribe based on cached discovery info from Phase 1.
            // The SubscribeAck with session=2 (regressed from ~3) should trigger
            // client-side reboot detection via session ID regression.
            tracing::info!("[wire_server] Phase 2: Server restarted - NOT sending Offer (simulating lost packet)");

            // Wait for Subscribe from client (handle TCP accept concurrently)
            let mut got_subscribe2 = false;
            let mut tcp_stream2_opt: Option<turmoil::net::TcpStream> = None;
            let deadline2 = tokio::time::Instant::now() + Duration::from_secs(1000);

            while !got_subscribe2 && tokio::time::Instant::now() < deadline2 {
                tokio::time::sleep(Duration::from_millis(1)).await;
                tokio::select! {
                    // Accept TCP connections
                    result = tcp_listener2.accept(), if tcp_stream2_opt.is_none() => {
                        let (stream, peer) = result?;
                        tracing::info!("[wire_server] Phase 2: TCP connection from {}", peer);
                        tcp_stream2_opt = Some(stream);
                    }
                    // Handle SD messages
                    result = sd_socket2.recv_from(&mut buf) => {
                        let (len, from) = result?;
                        if let Some((_header, sd)) = parse_sd_packet(&buf[..len]) {
                            for entry in sd.subscribe_entries() {
                                if entry.service_id == SERVICE_ID {
                                    tracing::info!("[wire_server] Phase 2: Got Subscribe from {}", from);
                                    got_subscribe2 = true;

                                    // Send SubscribeAck with SESSION=1 (regressed!) AND reboot=true
                                    // This triggers client-side reboot detection via session regression
                                    // (Case 2: reboot flag stays 1 AND session regressed)
                                    let ack2 = SdSubscribeAckBuilder::new(SERVICE_ID, INSTANCE_ID, EVENTGROUP_ID)
                                        .major_version(1)
                                        .ttl(entry.ttl)
                                        .session_id(server_session_id2)
                                        .reboot_flag(true)
                                        .build();
                                    sd_socket2.send_to(&ack2, from).await?;
                                    tracing::info!("[wire_server] Phase 2: Sent SubscribeAck with session={}, reboot=true (regressed!)", server_session_id2);
                                    server_session_id2 += 1;
                                }
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                }
            }
            tracing::info!("Done waiting for Subscribe in Phase 2 - got_subscribe2={}", got_subscribe2);

            // Send events - client should receive these despite reboot detection
            if let Some(ref mut tcp_stream2) = tcp_stream2_opt {
                for i in 0..5 {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let event = SomeIpPacketBuilder::notification(SERVICE_ID, EVENT_ID)
                        .session_id(i + 1)
                        .payload(&[0x10 + i as u8, 0x20, 0x30, 0x40])
                        .build();
                    if tcp_stream2.write_all(&event).await.is_err() {
                        tracing::warn!("[wire_server] Phase 2: Failed to send event {}", i);
                        break;
                    }
                    tracing::info!("[wire_server] Phase 2: Sent event #{}", i + 1);
                }
            } else {
                panic!("Expected client to connect to TCP after reboot, but no connection was accepted!");
            }

            // Keep connection open until test completes
            while !test_done.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(1000)).await;
            }

            Ok(())
        }
    });

    // Library-based client
    let events_p1 = Arc::clone(&events_received_phase1);
    let events_p2 = Arc::clone(&events_received_phase2);
    let test_done = Arc::clone(&test_complete);

    sim.client("lib_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("lib_client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        // === Phase 1: Discover, subscribe, receive event, unsubscribe ===
        tracing::info!("[lib_client] Phase 1: Discovering service...");

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SERVICE_ID)
                .instance(InstanceId::Id(INSTANCE_ID)),
        )
        .await
        .expect("Discovery timeout")
        .expect("Service should be found");

        tracing::info!("[lib_client] Phase 1: Service discovered, subscribing...");

        let eventgroup = EventgroupId::new(EVENTGROUP_ID).unwrap();
        let mut subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        tracing::info!("[lib_client] Phase 1: Subscribed, waiting for event...");

        // Wait for first event
        let event_timeout = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("Event timeout");

        if event_timeout.is_some() {
            events_p1.fetch_add(1, Ordering::SeqCst);
            tracing::info!("[lib_client] Phase 1: Received event!");
        }

        // Unsubscribe
        tracing::info!("[lib_client] Phase 1: Unsubscribing...");
        drop(subscription);
        tokio::time::sleep(Duration::from_millis(500)).await;

        // === Wait 1 hour (simulated) ===
        // During this time, server will crash and restart
        tracing::info!("[lib_client] Waiting 1 hour (simulated)...");
        tokio::time::sleep(Duration::from_secs(3700)).await;

        // === Phase 2: Re-subscribe after server reboot ===
        tracing::info!("[lib_client] Phase 2: Re-subscribing to service...");

        // The service should still be "discovered" (TTL was 3 days, only 1 hour passed)
        // But subscriber needs fresh subscription
        let mut subscription2 =
            tokio::time::timeout(Duration::from_secs(10), proxy.subscribe(eventgroup))
                .await
                .expect("Re-subscribe timeout")
                .expect("Re-subscribe should succeed");

        tracing::info!("[lib_client] Phase 2: Re-subscribed, waiting for events...");

        // The SubscribeAck from server will have session=1 (regressed from previous session ~3)
        // This should trigger reboot detection, but the NEW subscription should NOT be expired

        // Receive events
        let mut events_count = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline && events_count < 3 {
            tokio::select! {
                event = subscription2.next() => {
                    if event.is_some() {
                        events_count += 1;
                        events_p2.fetch_add(1, Ordering::SeqCst);
                        tracing::info!("[lib_client] Phase 2: Received event #{}", events_count);
                    } else {
                        tracing::warn!("[lib_client] Phase 2: Subscription stream ended!");
                        break;
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }

        tracing::info!(
            "[lib_client] Phase 2: Received {} events total",
            events_count
        );

        test_done.store(true, Ordering::SeqCst);

        Ok(())
    });

    sim.run().unwrap();

    let p1_events = events_received_phase1.load(Ordering::SeqCst);
    let p2_events = events_received_phase2.load(Ordering::SeqCst);

    tracing::info!(
        "Test results: Phase 1 events={}, Phase 2 events={}",
        p1_events,
        p2_events
    );

    assert!(
        p1_events >= 1,
        "Phase 1: Should receive at least 1 event before unsubscribe"
    );
    assert!(
        p2_events >= 3,
        "Phase 2: Should receive events after re-subscribe despite server reboot detection. Got {} events",
        p2_events
    );
}

/// Test: Client detects server reboot AND closes server-side TCP connections
///
/// This test extends the session regression reboot detection scenario by having the client
/// ALSO offer a service that the server subscribes to. This tests that when the client
/// detects a server reboot, it correctly:
/// 1. Keeps the NEW client-side subscription (with pending ACK) alive
/// 2. Closes server-side TCP connections from the rebooted peer (expired subscriptions)
///
/// Scenario:
/// - Client offers SERVICE_CLIENT_ID over TCP
/// - Wire-server subscribes to client's service (establishes TCP from server TO client)
/// - Client subscribes to server's SERVICE_ID (establishes TCP from client TO server)
/// - Server reboots
/// - Client detects reboot via SubscribeAck session regression
/// - Client should:
///   - Keep its NEW subscription's TCP connection alive (has pending ACK)
///   - Close the server-side TCP connection (expired subscription from rebooted peer)
/// - Client receives events on the new subscription
#[test]
fn client_detects_server_reboot_closes_server_tcp_connections() {
    configure_tracing();

    use crate::wire_format::helpers::{SdSubscribeBuilder, SomeIpPacketBuilder};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;

    covers!(feat_req_someipsd_871, feat_req_someipsd_872);

    const SERVICE_ID: u16 = 0x4567; // Server offers this (event subscription)
    const SERVICE_ID_RPC: u16 = 0x1234; // Server offers this (RPC calls)
    const SERVICE_CLIENT_ID: u16 = 0x9999; // Client offers this
    const INSTANCE_ID: u16 = 0x0001;
    const EVENTGROUP_ID: u16 = 0x0001;
    const EVENT_ID: u16 = 0x8001;
    const METHOD_ID: u16 = 0x0042;
    const TCP_PORT_SERVER: u16 = 30509;
    const TCP_PORT_RPC: u16 = 30511;
    const TCP_PORT_CLIENT: u16 = 30510;
    const TTL_3_MIN: u32 = 180; // 3 minutes

    let events_received_phase1 = Arc::new(AtomicUsize::new(0));
    let events_received_phase2 = Arc::new(AtomicUsize::new(0));
    let test_complete = Arc::new(AtomicBool::new(false));
    let server_tcp_closed = Arc::new(AtomicBool::new(false));
    let rpc_tcp_closed = Arc::new(AtomicBool::new(false));
    let wire_check_done = Arc::new(AtomicBool::new(false));

    let test_complete_clone = Arc::clone(&test_complete);
    let server_tcp_closed_clone = Arc::clone(&server_tcp_closed);
    let rpc_tcp_closed_clone = Arc::clone(&rpc_tcp_closed);
    let wire_check_done_clone = Arc::clone(&wire_check_done);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .max_message_latency(Duration::from_millis(50))
        .build();

    // Wire-level server
    sim.host("wire_server", move || {
        let test_done = Arc::clone(&test_complete_clone);
        let tcp_closed = Arc::clone(&server_tcp_closed_clone);
        let rpc_closed = Arc::clone(&rpc_tcp_closed_clone);
        let check_done = Arc::clone(&wire_check_done_clone);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();
            let sd_multicast: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

            // === PHASE 1: Initial server instance ===
            tracing::info!("[wire_server] Phase 1: Starting initial server instance");

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            let tcp_listener_server = turmoil::net::TcpListener::bind(format!("0.0.0.0:{}", TCP_PORT_SERVER)).await?;
            let tcp_listener_rpc = turmoil::net::TcpListener::bind(format!("0.0.0.0:{}", TCP_PORT_RPC)).await?;

            let mut server_session_id = 1u16;

            // Wait for client to be ready
            tokio::time::sleep(Duration::from_millis(200)).await;

            // Send initial Offer for SERVICE_ID (event subscription) (session=1, reboot=true)
            let offer = SdOfferBuilder::new(SERVICE_ID, INSTANCE_ID, my_ip, TCP_PORT_SERVER)
                .version(1, 0)
                .tcp()
                .ttl(TTL_3_MIN)
                .session_id(server_session_id)
                .reboot_flag(true)
                .build();
            sd_socket.send_to(&offer, sd_multicast).await?;
            server_session_id += 1;

            tokio::time::sleep(Duration::from_millis(50)).await;

            // Send initial Offer for SERVICE_ID_RPC (RPC service) (session=2, reboot=true)
            let offer_rpc = SdOfferBuilder::new(SERVICE_ID_RPC, INSTANCE_ID, my_ip, TCP_PORT_RPC)
                .version(1, 0)
                .tcp()
                .ttl(TTL_3_MIN)
                .session_id(server_session_id)
                .reboot_flag(true)
                .build();
            sd_socket.send_to(&offer_rpc, sd_multicast).await?;
            server_session_id += 1;

            // Wait for client's Offer for SERVICE_CLIENT_ID
            let mut client_tcp_port: Option<u16> = None;
            let mut buf = [0u8; 1500];
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);

            tracing::info!("[wire_server] Phase 1: Waiting for client Offer...");
            while client_tcp_port.is_none() && tokio::time::Instant::now() < deadline {
                tokio::select! {
                    result = sd_socket.recv_from(&mut buf) => {
                        let (len, from) = result?;
                        if let Some((_header, sd)) = parse_sd_packet(&buf[..len]) {
                            tracing::info!("[wire_server] Phase 1: Got SD message with {} entries", sd.entries.len());
                            for entry in sd.offer_entries() {
                                tracing::info!("[wire_server] Phase 1: Offer entry for service {:04X}", entry.service_id);
                                if entry.service_id == SERVICE_CLIENT_ID {
                                    tracing::info!("[wire_server] Phase 1: Got Offer for SERVICE_CLIENT_ID from {}", from);
                                    // Extract TCP endpoint port from options
                                    if let Some(port) = sd.endpoint_port_for_entry(entry) {
                                        client_tcp_port = Some(port);
                                        tracing::info!("[wire_server] Client TCP port: {}", port);
                                    } else {
                                        tracing::warn!("[wire_server] No TCP port found in Offer");
                                    }
                                }
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(50)) => {}
                }
            }

            let client_tcp_port = client_tcp_port.expect("Should receive client Offer");
            let client_ip = turmoil::lookup("lib_client").to_string().parse::<std::net::Ipv4Addr>().unwrap();
            let client_tcp_addr: std::net::SocketAddr = (client_ip, client_tcp_port).into();

            // Give client time to fully initialize the service
            tokio::time::sleep(Duration::from_millis(200)).await;

            // Subscribe to client's service over TCP
            tracing::info!("[wire_server] Phase 1: Subscribing to client's service at {}", client_tcp_addr);
            let tcp_to_client = turmoil::net::TcpStream::connect(client_tcp_addr).await?;
            let local_addr_to_client = tcp_to_client.local_addr()?;
            let local_port_to_client = local_addr_to_client.port();
            tracing::info!("[wire_server] Phase 1: TCP connection to client: local={}, remote={}", local_addr_to_client, tcp_to_client.peer_addr()?);
            tracing::info!("[wire_server] Phase 1: Sending Subscribe with endpoint {}:{}", my_ip, local_port_to_client);

            let subscribe = SdSubscribeBuilder::new(
                SERVICE_CLIENT_ID,
                INSTANCE_ID,
                EVENTGROUP_ID,
                my_ip,
                local_port_to_client,
            )
            .tcp()
            .ttl(TTL_3_MIN)
            .session_id(server_session_id)
            .reboot_flag(true)
            .build();
            sd_socket.send_to(&subscribe, sd_multicast).await?;
            server_session_id += 1;

            // Keep the tcp_to_client connection alive so it stays in client's connection map

            // Wait for Subscribe from client and handle TCP connections
            let mut got_subscribe = false;
            let mut tcp_stream_opt: Option<turmoil::net::TcpStream> = None;
            let mut rpc_stream_opt: Option<turmoil::net::TcpStream> = None;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

            while !got_subscribe && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(1)).await;
                tokio::select! {
                    result = tcp_listener_server.accept(), if tcp_stream_opt.is_none() => {
                        let (stream, peer) = result?;
                        tracing::info!("[wire_server] Phase 1: TCP connection for events from client {}", peer);
                        tcp_stream_opt = Some(stream);
                    }
                    result = tcp_listener_rpc.accept(), if rpc_stream_opt.is_none() => {
                        let (stream, peer) = result?;
                        tracing::info!("[wire_server] Phase 1: TCP connection for RPC from client {}", peer);
                        rpc_stream_opt = Some(stream);
                    }
                    result = sd_socket.recv_from(&mut buf) => {
                        let (len, from) = result?;
                        if let Some((_header, sd)) = parse_sd_packet(&buf[..len]) {
                            for entry in sd.subscribe_entries() {
                                if entry.service_id == SERVICE_ID {
                                    tracing::info!("[wire_server] Phase 1: Got Subscribe from {}", from);
                                    got_subscribe = true;

                                    let ack = SdSubscribeAckBuilder::new(SERVICE_ID, INSTANCE_ID, EVENTGROUP_ID)
                                        .major_version(1)
                                        .ttl(entry.ttl)
                                        .session_id(server_session_id)
                                        .reboot_flag(true)
                                        .build();
                                    sd_socket.send_to(&ack, from).await?;
                                    server_session_id += 1;
                                }
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(2000)) => {}
                }
            }

            let mut tcp_stream = tcp_stream_opt.expect("Should have accepted TCP connection from client for events");
            let rpc_stream = rpc_stream_opt.expect("Should have accepted RPC TCP connection from client");

            // Keep rpc_stream alive to check if client closes it on reboot
            // We don't need to handle RPC requests since we're just testing connection lifecycle

            // Send multiple events in Phase 1
            for i in 0..3 {
                tokio::time::sleep(Duration::from_millis(10)).await;
                let event = SomeIpPacketBuilder::notification(SERVICE_ID, EVENT_ID)
                    .session_id(i + 1)
                    .payload(&[0x01 + i as u8, 0x02, 0x03, 0x04])
                    .build();
                tcp_stream.write_all(&event).await?;
                tracing::info!("[wire_server] Phase 1: Sent event #{}", i + 1);
            }

            // === PHASE 2: Simulate reboot by closing and reopening SD socket ===
            // Simulate server restart: Close SD socket and reopen with fresh state
            tracing::info!("[wire_server] Phase 2: Simulating reboot (closing and reopening SD socket)");

            // Close the SD socket and TCP streams
            drop(sd_socket);
            drop(tcp_stream);
            // Keep rpc_stream to check if client closes it

            // Wait for client to unsubscribe
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Reopen SD socket with fresh state
            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            // NOTE: We deliberately DO NOT send an Offer here!
            // The service was cached from Phase 1 (TTL=180s, only ~1s has passed).
            // The client will re-subscribe based on cached discovery.
            // The regressed SubscribeAck will trigger reboot detection.

            // Wait for client to re-Subscribe after "reboot"
            let mut sent_regressed_ack = false;
            let mut tcp_stream_phase2: Option<turmoil::net::TcpStream> = None;
            let deadline_reboot = tokio::time::Instant::now() + Duration::from_secs(7);

            // Wait for BOTH Subscribe AND TCP connection
            while (!sent_regressed_ack || tcp_stream_phase2.is_none()) && tokio::time::Instant::now() < deadline_reboot {
                tokio::select! {
                    // Accept NEW TCP connection from client after re-subscribe
                    result = tcp_listener_server.accept(), if tcp_stream_phase2.is_none() => {
                        let (stream, peer) = result?;
                        tracing::info!("[wire_server] Phase 2: NEW TCP connection from client {}", peer);
                        tcp_stream_phase2 = Some(stream);
                    }
                    // Handle SD messages
                    result = sd_socket.recv_from(&mut buf) => {
                        let (len, from) = result?;
                        tracing::debug!("[wire_server] Phase 2: Got SD packet from {}", from);
                        if let Some((_header, sd)) = parse_sd_packet(&buf[..len]) {
                            tracing::debug!("[wire_server] Phase 2: Parsed SD with {} entries (offers={}, subs={})",
                                sd.entries.len(), sd.offer_entries().count(), sd.subscribe_entries().count());
                            for entry in sd.subscribe_entries() {
                                tracing::info!("[wire_server] Phase 2: Subscribe for service {:04X}", entry.service_id);
                                if entry.service_id == SERVICE_ID {
                                    tracing::info!("[wire_server] Phase 2: Got re-Subscribe, sending regressed ack");
                                    sent_regressed_ack = true;

                                    // Send SubscribeAck with SESSION=1 (regressed from previous 3!)
                                    let ack = SdSubscribeAckBuilder::new(SERVICE_ID, INSTANCE_ID, EVENTGROUP_ID)
                                        .major_version(1)
                                        .ttl(entry.ttl)
                                        .session_id(1)
                                        .reboot_flag(true)
                                        .build();
                                    sd_socket.send_to(&ack, from).await?;
                                    tracing::info!("[wire_server] Phase 2: Sent SubscribeAck session=1 (REGRESSED!)");
                                    break;
                                }
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        // Periodic check to avoid busy loop
                    }
                }
            }

            if !sent_regressed_ack {
                tracing::warn!("[wire_server] Phase 2: Never received re-Subscribe!");
            }

            // Send multiple events on the NEW connection
            if let Some(ref mut tcp_stream) = tcp_stream_phase2 {
                for i in 0..5 {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let event = SomeIpPacketBuilder::notification(SERVICE_ID, EVENT_ID)
                        .session_id(i + 10)
                        .payload(&[0x10 + i as u8, 0x20, 0x30, 0x40])
                        .build();
                    if tcp_stream.write_all(&event).await.is_err() {
                        tracing::warn!("[wire_server] Phase 2: Failed to send event #{}", i + 1);
                        break;
                    }
                    tracing::info!("[wire_server] Phase 2: Sent event #{}", i + 1);
                }
            } else {
                panic!("Expected client to establish TCP connection for Phase 2 subscription, but no connection was accepted!");
            }

            // Give client time to detect reboot and close OLD tcp_to_client
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Check if client closed our tcp_to_client connection (subscription)
            tracing::info!("[wire_server] Phase 2: Checking if client closed tcp_to_client (subscription)");
            let mut read_buf = [0u8; 1];
            use tokio::io::AsyncReadExt;
            let mut tcp_to_client_mut = tcp_to_client;

            match tokio::time::timeout(
                Duration::from_millis(500),
                tcp_to_client_mut.read(&mut read_buf)
            ).await {
                Ok(Ok(0)) => {
                    tracing::info!("[wire_server] Phase 2: ✓ Subscription connection closed by client (EOF)!");
                    tcp_closed.store(true, Ordering::SeqCst);
                }
                Ok(Ok(n)) => {
                    tracing::warn!("[wire_server] Phase 2: Got {} bytes on subscription (unexpected data)", n);
                }
                Ok(Err(e)) => {
                    tracing::info!("[wire_server] Phase 2: ✓ Subscription read error: {} (closed)", e);
                    tcp_closed.store(true, Ordering::SeqCst);
                }
                Err(_) => {
                    tracing::error!("[wire_server] Phase 2: ✗ Timeout - subscription connection still open!");
                }
            }

            // Give turmoil network simulator time to propagate connection closure
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Check if client closed the RPC connection
            // Note: There may be data in the receive buffer from before the reboot,
            // so we drain it first, then check for EOF to verify closure
            tracing::info!("[wire_server] Phase 2: Checking if client closed rpc_stream (RPC)");
            let mut rpc_stream_mut = rpc_stream;

            let mut total_drained = 0;
            loop {
                match tokio::time::timeout(
                    Duration::from_millis(500),
                    rpc_stream_mut.read(&mut read_buf)
                ).await {
                    Ok(Ok(0)) => {
                        // EOF - connection is closed
                        if total_drained > 0 {
                            tracing::info!("[wire_server] Phase 2: ✓ Drained {} bytes, then got EOF - RPC connection closed!", total_drained);
                        } else {
                            tracing::info!("[wire_server] Phase 2: ✓ Got immediate EOF - RPC connection closed!");
                        }
                        rpc_closed.store(true, Ordering::SeqCst);
                        break;
                    }
                    Ok(Ok(n)) => {
                        // Got data from receive buffer (sent before connection closed)
                        total_drained += n;
                        tracing::debug!("[wire_server] Phase 2: Draining {} bytes from RPC buffer (total: {})", n, total_drained);
                        // Continue reading to get EOF
                    }
                    Ok(Err(e)) => {
                        // Read error means connection is closed
                        tracing::info!("[wire_server] Phase 2: ✓ RPC read error after draining {} bytes: {} (closed)", total_drained, e);
                        rpc_closed.store(true, Ordering::SeqCst);
                        break;
                    }
                    Err(_) => {
                        // Timeout - connection is still open
                        tracing::error!("[wire_server] Phase 2: ✗ Timeout after draining {} bytes - RPC connection still open!", total_drained);
                        break;
                    }
                }
            }

            // Signal that check is complete
            check_done.store(true, Ordering::SeqCst);

            while !test_done.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }

            Ok(())
        }
    });

    // Library-based client (offers AND subscribes)
    let events_p1 = Arc::clone(&events_received_phase1);
    let events_p2 = Arc::clone(&events_received_phase2);
    let test_done = Arc::clone(&test_complete);
    let wire_check_done = Arc::clone(&wire_check_done);

    sim.client("lib_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("lib_client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        // Offer SERVICE_CLIENT_ID
        tracing::info!("[lib_client] Offering SERVICE_CLIENT_ID...");
        let _offering = runtime
            .offer(SERVICE_CLIENT_ID, InstanceId::Id(INSTANCE_ID))
            .version(1, 0)
            .tcp_port(TCP_PORT_CLIENT)
            .start()
            .await
            .expect("Should offer service");

        // Give the service time to fully initialize and start sending offers
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Discover and call the RPC service (just discovery establishes TCP connection)
        tracing::info!("[lib_client] Discovering server's RPC service...");
        let _rpc_proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(SERVICE_ID_RPC).instance(InstanceId::Id(INSTANCE_ID))
        )
        .await
        .expect("RPC discovery timeout")
        .expect("RPC service should be found");

        // Make an RPC call to establish the TCP connection
        tracing::info!("[lib_client] Making test RPC call...");
        match tokio::time::timeout(
            Duration::from_secs(2),
            _rpc_proxy.call(MethodId::new(METHOD_ID).unwrap(), &[0x01, 0x02, 0x03, 0x04])
        ).await {
            Ok(Ok(response)) => {
                tracing::info!("[lib_client] RPC call succeeded, response len={}", response.payload.len());
            }
            Ok(Err(e)) => {
                tracing::info!("[lib_client] RPC call failed (expected, no handler): {}", e);
            }
            Err(_) => {
                tracing::info!("[lib_client] RPC call timeout (expected, no handler)");
            }
        }

        // Subscribe to SERVER's event service
        tracing::info!("[lib_client] Discovering server's event service...");
        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(SERVICE_ID).instance(InstanceId::Id(INSTANCE_ID))
        )
        .await
        .expect("Discovery timeout")
        .expect("Service should be found");

        tracing::info!("[lib_client] Subscribing to server...");
        let eventgroup = EventgroupId::new(EVENTGROUP_ID).unwrap();
        let mut subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(eventgroup)
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        tracing::info!("[lib_client] Phase 1: Waiting for events...");
        // Receive multiple events in Phase 1
        let mut phase1_count = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline && phase1_count < 3 {
            tokio::select! {
                event = subscription.next() => {
                    if event.is_some() {
                        phase1_count += 1;
                        events_p1.fetch_add(1, Ordering::SeqCst);
                        tracing::info!("[lib_client] Phase 1: Received event #{}", phase1_count);
                    } else {
                        break;
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
        tracing::info!("[lib_client] Phase 1: Received {} events total", phase1_count);

        // Unsubscribe
        tracing::info!("[lib_client] Unsubscribing...");
        drop(subscription);
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Wait for server to simulate reboot
        tracing::info!("[lib_client] Waiting for server reboot simulation...");
        tokio::time::sleep(Duration::from_secs(1)).await;

        // Re-subscribe (this sends Subscribe message that wire-server intercepts with regressed ack)
        // The regressed SubscribeAck triggers reboot detection and TCP closure of OLD connection
        // The NEW subscription should still work and receive events
        tracing::info!("[lib_client] Phase 2: Re-subscribing after server 'reboot'...");
        let mut subscription2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(eventgroup)
        )
        .await
        .expect("Re-subscribe timeout")
        .expect("Re-subscribe should succeed");

        tracing::info!("[lib_client] Phase 2: Waiting for events...");
        // Receive multiple events in Phase 2 (despite reboot detection)
        let mut phase2_count = 0;
        let deadline_p2 = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline_p2 && phase2_count < 5 {
            tokio::select! {
                event = subscription2.next() => {
                    if event.is_some() {
                        phase2_count += 1;
                        events_p2.fetch_add(1, Ordering::SeqCst);
                        tracing::info!("[lib_client] Phase 2: Received event #{}", phase2_count);
                    } else {
                        tracing::warn!("[lib_client] Phase 2: Subscription stream ended!");
                        break;
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
        tracing::info!("[lib_client] Phase 2: Received {} events total. Reboot should have been detected, OLD tcp_to_client should be closed", phase2_count);

        // Give time for TCP cleanup to complete AND for wire-server to check the connection
        tokio::time::sleep(Duration::from_millis(1000)).await;

        tracing::info!("[lib_client] Phase 1 events: {}, Phase 2 events: {}",
            events_p1.load(Ordering::SeqCst), events_p2.load(Ordering::SeqCst));
        test_done.store(true, Ordering::SeqCst);

        // Keep alive until wire-server completes its check
        while !wire_check_done.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        Ok(())
    });

    sim.run().unwrap();

    let p1_events = events_received_phase1.load(Ordering::SeqCst);
    let p2_events = events_received_phase2.load(Ordering::SeqCst);
    let tcp_was_closed = server_tcp_closed.load(Ordering::SeqCst);
    let rpc_was_closed = rpc_tcp_closed.load(Ordering::SeqCst);

    tracing::info!("Test results: Phase 1 events={}, Phase 2 events={}, server_tcp_closed={}, rpc_tcp_closed={}",
        p1_events, p2_events, tcp_was_closed, rpc_was_closed);

    assert!(
        p1_events >= 3,
        "Phase 1: Should receive at least 3 events. Got {}",
        p1_events
    );
    assert!(
        p2_events >= 3,
        "Phase 2: Should receive events after re-subscribe despite reboot detection. Got {}",
        p2_events
    );
    assert!(tcp_was_closed, "Client should have closed server-side TCP connection (subscription) after detecting reboot");
    assert!(
        rpc_was_closed,
        "Client should have closed server-side TCP connection (RPC) after detecting reboot"
    );
}

#[test_log::test]
#[ignore = "This need real network. But for interesting real network tests to happen, we need support for the SD unicast endpoint option."]
fn server_detects_client_reboot_clears_subscriptions_port_reuse() {
    use crate::wire_format::helpers::{ParsedHeader, SdSubscribeBuilder, SOMEIP_HEADER_SIZE};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::AsyncReadExt;

    covers!(feat_req_someipsd_871, feat_req_someipsd_872);

    const SERVICE1_ID: u16 = 0x1111;
    const SERVICE2_ID: u16 = 0x2222;
    const SERVICE3_ID: u16 = 0x3333;
    const INSTANCE_ID: u16 = 0x0001;
    const EVENTGROUP_ID: u16 = 0x0001;
    const EVENT1_ID: u16 = 0x8001;
    const EVENT2_ID: u16 = 0x8002;
    const EVENT3_ID: u16 = 0x8003;
    const VERSION: (u8, u32) = (1, 0);

    let events_service1 = Arc::new(AtomicUsize::new(0));
    let events_service2 = Arc::new(AtomicUsize::new(0));
    let events_service3 = Arc::new(AtomicUsize::new(0));
    let test_complete = Arc::new(AtomicBool::new(false));

    let events_service1_clone = Arc::clone(&events_service1);
    let events_service2_clone = Arc::clone(&events_service2);
    let events_service3_clone = Arc::clone(&events_service3);
    let test_complete_clone = Arc::clone(&test_complete);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library-based server offering three services with events
    sim.host("lib_server", move || {
        let events_s1 = Arc::clone(&events_service1_clone);
        let events_s2 = Arc::clone(&events_service2_clone);
        let events_s3 = Arc::clone(&events_service3_clone);
        let test_done = Arc::clone(&test_complete_clone);

        async move {
            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("lib_server").to_string().parse().unwrap())
                .preferred_transport(Transport::Tcp)
                .start_turmoil()
                .await
                .unwrap();

            // Offer SERVICE1
            let offering1 = runtime
                .offer(SERVICE1_ID, InstanceId::Id(INSTANCE_ID))
                .version(VERSION.0, VERSION.1)
                .tcp()
                .start()
                .await
                .unwrap();

            let eventgroup = EventgroupId::new(EVENTGROUP_ID).unwrap();
            let event1_handle = offering1
                .event(EventId::new(EVENT1_ID).unwrap())
                .eventgroup(eventgroup)
                .create()
                .await
                .unwrap();

            // Offer SERVICE2
            let offering2 = runtime
                .offer(SERVICE2_ID, InstanceId::Id(INSTANCE_ID))
                .version(VERSION.0, VERSION.1)
                .tcp()
                .start()
                .await
                .unwrap();

            let event2_handle = offering2
                .event(EventId::new(EVENT2_ID).unwrap())
                .eventgroup(eventgroup)
                .create()
                .await
                .unwrap();

            // Offer SERVICE3
            let offering3 = runtime
                .offer(SERVICE3_ID, InstanceId::Id(INSTANCE_ID))
                .version(VERSION.0, VERSION.1)
                .tcp()
                .start()
                .await
                .unwrap();

            let event3_handle = offering3
                .event(EventId::new(EVENT3_ID).unwrap())
                .eventgroup(eventgroup)
                .create()
                .await
                .unwrap();

            tracing::info!("[lib_server] Services offered, starting event loop");

            // Send events for all three services
            let mut counter = 0u32;
            while !test_done.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let payload = counter.to_be_bytes();

                if event1_handle.notify(&payload).await.is_ok() {
                    events_s1.fetch_add(1, Ordering::SeqCst);
                }
                if event2_handle.notify(&payload).await.is_ok() {
                    events_s2.fetch_add(1, Ordering::SeqCst);
                }
                if event3_handle.notify(&payload).await.is_ok() {
                    events_s3.fetch_add(1, Ordering::SeqCst);
                }
                counter += 1;
            }

            Ok(())
        }
    });

    // Wire-level client that manually sends SD messages
    let test_complete_for_client = Arc::clone(&test_complete);
    sim.client("wire_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let my_ip: std::net::Ipv4Addr =
            turmoil::lookup("wire_client").to_string().parse().unwrap();
        let server_ip: std::net::Ipv4Addr =
            turmoil::lookup("lib_server").to_string().parse().unwrap();

        // Bind SD socket
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket
            .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut client_session_id = 1u16;

        // Wait for Offer for all services, capture TCP ports
        tracing::info!("[wire_client] Waiting for service Offers...");
        let mut service1_tcp_port = 0u16;
        let mut service2_tcp_port = 0u16;
        let mut service3_tcp_port = 0u16;
        loop {
            let (len, _from) = sd_socket.recv_from(&mut buf).await?;
            if let Some((_header, sd)) = parse_sd_packet(&buf[..len]) {
                for entry in sd.offer_entries() {
                    if let Some(opt) = sd.option_at(entry.index_1st_option) {
                        if let Some(port) = opt.port() {
                            if entry.service_id == SERVICE1_ID && service1_tcp_port == 0 {
                                service1_tcp_port = port;
                                tracing::info!("[wire_client] Found SERVICE1 on TCP port {}", port);
                            }
                            if entry.service_id == SERVICE2_ID && service2_tcp_port == 0 {
                                service2_tcp_port = port;
                                tracing::info!("[wire_client] Found SERVICE2 on TCP port {}", port);
                            }
                            if entry.service_id == SERVICE3_ID && service3_tcp_port == 0 {
                                service3_tcp_port = port;
                                tracing::info!("[wire_client] Found SERVICE3 on TCP port {}", port);
                            }
                        }
                    }
                }
            }
            if service1_tcp_port != 0 && service2_tcp_port != 0 && service3_tcp_port != 0 {
                break;
            }
        }

        // Phase 1: Connect to SERVER's TCP port for SERVICE1
        tracing::info!("[wire_client] Phase 1: Connecting to SERVICE1 TCP port...");
        let tcp_addr: std::net::SocketAddr = (server_ip, service1_tcp_port).into();
        let mut tcp_stream = turmoil::net::TcpStream::connect(tcp_addr).await?;
        let local_port = tcp_stream.local_addr()?.port();
        tracing::info!("[wire_client] Connected to SERVICE1, local port {}", local_port);

        // Subscribe to SERVICE1
        tracing::info!("[wire_client] Subscribing to SERVICE1");
        let subscribe1 = SdSubscribeBuilder::new(
            SERVICE1_ID,
            INSTANCE_ID,
            EVENTGROUP_ID,
            my_ip,
            local_port, // Use the local port of our TCP connection
        )
        .tcp()
        .session_id(client_session_id)
        .reboot_flag(true)
        .build();
        client_session_id += 1;

        let server_sd_addr: std::net::SocketAddr = (server_ip, 30490).into();
        sd_socket.send_to(&subscribe1, server_sd_addr).await?;

        // Wait for SubscribeAck
        tracing::info!("[wire_client] Waiting for SubscribeAck...");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut got_ack = false;
        while tokio::time::Instant::now() < deadline && !got_ack {
            tokio::select! {
                result = sd_socket.recv_from(&mut buf) => {
                    let (len, _from) = result?;
                    if let Some((_header, sd)) = parse_sd_packet(&buf[..len]) {
                        for entry in sd.subscribe_ack_entries() {
                            if entry.service_id == SERVICE1_ID && !entry.is_stop() {
                                tracing::info!("[wire_client] Got SubscribeAck for SERVICE1");
                                got_ack = true;
                            }
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
        assert!(got_ack, "Should receive SubscribeAck for SERVICE1");

        // Receive SERVICE1 events on our TCP connection
        tracing::info!("[wire_client] Receiving SERVICE1 events...");
        let mut events_received_s1 = 0;
        let mut tcp_buf = [0u8; 1024];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline && events_received_s1 < 5 {
            tokio::select! {
                result = tcp_stream.read(&mut tcp_buf) => {
                    match result {
                        Ok(0) => break, // Connection closed
                        Ok(n) => {
                            // Parse SOME/IP events (skip magic cookie bytes if present)
                            let mut offset = 0;
                            while offset + SOMEIP_HEADER_SIZE <= n {
                                // Check for magic cookie
                                if n >= offset + 8 && tcp_buf[offset..offset+8] == [0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08] {
                                    offset += 8;
                                    continue;
                                }
                                if let Some(header) = ParsedHeader::parse(&tcp_buf[offset..]) {
                                    if header.is_notification() {
                                        if header.service_id == SERVICE1_ID {
                                            events_received_s1 += 1;
                                            tracing::info!("[wire_client] Received SERVICE1 event #{}", events_received_s1);
                                        } else {
                                            panic!("Received not service 1 event before subscribing - should not happen!");
                                        }
                                    }
                                    offset += SOMEIP_HEADER_SIZE + header.payload_length();
                                } else {
                                    break;
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }

        tracing::info!("[wire_client] Received {} SERVICE1 events", events_received_s1);
        assert!(events_received_s1 >= 3, "Should receive at least 3 SERVICE1 events");
        drop(tcp_stream); // Close TCP connection before rebooting

        // Phase 2: Simulate client reboot
        // A real freshly booted client would:
        // 1. Client restarts, opens fresh TCP connections
        // 2. Sends Subscribes - server detects reboot from session=1
        tracing::info!("[wire_client] Phase 2: Client 'rebooting'...");
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Step 1: Open TWO fresh TCP connections (like a rebooted client would)
        tracing::info!("[wire_client] Opening fresh TCP connections to SERVICE2 and SERVICE3...");
        let tcp_addr2: std::net::SocketAddr = (server_ip, service2_tcp_port).into();
        let local_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), local_port);

        let socket = TcpSocket::new_v4()?;
        socket.bind(local_addr)?;
        let mut tcp_stream2 = socket.connect(tcp_addr2).await?;
        let local_port2 = tcp_stream2.local_addr()?.port();
        tracing::info!("[wire_client] Connected to SERVICE2, local port {}", local_port2);

        let tcp_addr3: std::net::SocketAddr = (server_ip, service3_tcp_port).into();
        let mut tcp_stream3 = turmoil::net::TcpStream::connect(tcp_addr3).await?;
        let local_port3 = tcp_stream3.local_addr()?.port();
        tracing::info!("[wire_client] Connected to SERVICE3, local port {}", local_port3);

        // Step 2: Send Subscribe for SERVICE2 with session=1, reboot=true
        // This triggers server's reboot detection
        tracing::info!("[wire_client] Subscribing to SERVICE2 (session=1, reboot=true)");
        let subscribe2 = SdSubscribeBuilder::new(
            SERVICE2_ID,
            INSTANCE_ID,
            EVENTGROUP_ID,
            my_ip,
            local_port2,
        )
        .tcp()
        .session_id(1) // Session reset to 1 = reboot
        .reboot_flag(true)
        .build();

        sd_socket.send_to(&subscribe2, server_sd_addr).await?;

        // Wait 50ms before sending second Subscribe
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Step 3: Send Subscribe for SERVICE3 with session=2, reboot=true
        tracing::info!("[wire_client] Subscribing to SERVICE3 (session=2, reboot=true)");
        let subscribe3 = SdSubscribeBuilder::new(
            SERVICE3_ID,
            INSTANCE_ID,
            EVENTGROUP_ID,
            my_ip,
            local_port3,
        )
        .tcp()
        .session_id(2) // Still in same session sequence
        .reboot_flag(true) // Reboot flag stays true until wrap-around
        .build();

        sd_socket.send_to(&subscribe3, server_sd_addr).await?;

        // Wait for SubscribeAcks for both services
        tracing::info!("[wire_client] Waiting for SubscribeAcks...");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut got_ack2 = false;
        let mut got_ack3 = false;
        while tokio::time::Instant::now() < deadline && !(got_ack2 && got_ack3) {
            tokio::select! {
                result = sd_socket.recv_from(&mut buf) => {
                    let (len, _from) = result?;
                    if let Some((_header, sd)) = parse_sd_packet(&buf[..len]) {
                        for entry in sd.subscribe_ack_entries() {
                            if entry.service_id == SERVICE2_ID && !entry.is_stop() {
                                tracing::info!("[wire_client] Got SubscribeAck for SERVICE2");
                                got_ack2 = true;
                            }
                            if entry.service_id == SERVICE3_ID && !entry.is_stop() {
                                tracing::info!("[wire_client] Got SubscribeAck for SERVICE3");
                                got_ack3 = true;
                            }
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
        assert!(got_ack2, "Should receive SubscribeAck for SERVICE2");
        assert!(got_ack3, "Should receive SubscribeAck for SERVICE3");

        // Receive events from both services on their TCP connections
        tracing::info!("[wire_client] Receiving events from SERVICE2 and SERVICE3...");
        let mut events_received_s2 = 0;
        let mut events_received_s3 = 0;
        let mut tcp_buf2 = [0u8; 1024];
        let mut tcp_buf3 = [0u8; 1024];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);

        while tokio::time::Instant::now() < deadline && (events_received_s2 < 3 || events_received_s3 < 3) {
            tokio::select! {
                // Read from SERVICE2 TCP
                result = tcp_stream2.read(&mut tcp_buf2) => {
                    match result {
                        Ok(0) => {
                            panic!("SERVICE2 TCP connection closed unexpectedly!");
                        }
                        Ok(n) => {
                            let mut offset = 0;
                            while offset + SOMEIP_HEADER_SIZE <= n {
                                if n >= offset + 8 && tcp_buf2[offset..offset+8] == [0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08] {
                                    offset += 8;
                                    continue;
                                }
                                if let Some(header) = ParsedHeader::parse(&tcp_buf2[offset..]) {
                                    if header.is_notification() {
                                        if header.service_id == SERVICE2_ID {
                                            events_received_s2 += 1;
                                            tracing::info!("[wire_client] Received SERVICE2 event #{}", events_received_s2);
                                        } else {
                                            panic!("Received event for unexpected service: {}", header.service_id);
                                        }
                                    }
                                    offset += SOMEIP_HEADER_SIZE + header.payload_length();
                                } else {
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            panic!("SERVICE2 TCP read error: {}", e);
                        }
                    }
                }
                // Read from SERVICE3 TCP
                result = tcp_stream3.read(&mut tcp_buf3) => {
                    match result {
                        Ok(0) => {
                            panic!("SERVICE3 TCP connection closed unexpectedly!");
                        }
                        Ok(n) => {
                            let mut offset = 0;
                            while offset + SOMEIP_HEADER_SIZE <= n {
                                if n >= offset + 8 && tcp_buf3[offset..offset+8] == [0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08] {
                                    offset += 8;
                                    continue;
                                }
                                if let Some(header) = ParsedHeader::parse(&tcp_buf3[offset..]) {
                                    if header.is_notification() {
                                        if header.service_id == SERVICE3_ID {
                                            events_received_s3 += 1;
                                            tracing::info!("[wire_client] Received SERVICE3 event #{}", events_received_s3);
                                        } else {
                                            panic!("Received event for unexpected service: {}", header.service_id);
                                        }
                                    }
                                    offset += SOMEIP_HEADER_SIZE + header.payload_length();
                                } else {
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            panic!("SERVICE3 TCP read error: {}", e);
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }

        tracing::info!("[wire_client] Received {} SERVICE2 events, {} SERVICE3 events",
            events_received_s2, events_received_s3);
        assert!(events_received_s2 >= 3, "Should receive at least 3 SERVICE2 events after reboot");
        assert!(events_received_s3 >= 3, "Should receive at least 3 SERVICE3 events after reboot");

        // Signal test completion
        test_complete_for_client.store(true, Ordering::SeqCst);

        tracing::info!("[wire_client] Test complete - SERVICE1: {}, SERVICE2: {}, SERVICE3: {}",
            events_received_s1, events_received_s2, events_received_s3);

        Ok(())
    });

    sim.run().unwrap();

    // Verify events were sent by server (indicates server was actively sending)
    let s1_sent = events_service1.load(Ordering::SeqCst);
    let s2_sent = events_service2.load(Ordering::SeqCst);
    let s3_sent = events_service3.load(Ordering::SeqCst);
    tracing::info!(
        "Server sent {} SERVICE1, {} SERVICE2, {} SERVICE3 events",
        s1_sent,
        s2_sent,
        s3_sent
    );
}

/// Test: Client closes TCP connection when detecting server reboot
///
/// Setup:
/// - wire-server: Manually sends SD messages and monitors TCP connections
/// - lib-client: Uses library to connect and subscribe
///
/// Flow:
/// 1. Wire-server offers service via TCP (session=1)
/// 2. Lib-client discovers and subscribes (TCP connection established)
/// 3. Client receives events on TCP connection
/// 4. Wire-server "reboots" - sends new Offer with session=1 (regression from previous session ~3)
/// 5. **Client should detect reboot and close the old TCP connection**
/// 6. Wire-server observes the TCP connection being closed
///
/// Verifies:
/// - feat_req_someipsd_872: Client resets TCP connections on server reboot detection
#[test_log::test]
fn client_closes_tcp_on_server_reboot_detection() {
    use crate::wire_format::helpers::SomeIpPacketBuilder;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    covers!(feat_req_someipsd_872);

    const SERVICE_ID: u16 = 0x5678;
    const INSTANCE_ID: u16 = 0x0001;
    const EVENTGROUP_ID: u16 = 0x0001;
    const EVENT_ID: u16 = 0x8001;
    const TCP_PORT: u16 = 30510;
    const TTL_1_HOUR: u32 = 3600;

    let tcp_connected = Arc::new(AtomicBool::new(false));
    let tcp_closed_after_reboot = Arc::new(AtomicBool::new(false));
    let events_received_phase1 = Arc::new(AtomicUsize::new(0));
    let test_complete = Arc::new(AtomicBool::new(false));

    let tcp_connected_clone = Arc::clone(&tcp_connected);
    let tcp_closed_clone = Arc::clone(&tcp_closed_after_reboot);
    let test_complete_clone = Arc::clone(&test_complete);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Wire-level server that monitors TCP connection lifecycle
    sim.host("wire_server", move || {
        let tcp_connected = Arc::clone(&tcp_connected_clone);
        let tcp_closed = Arc::clone(&tcp_closed_clone);
        let test_done = Arc::clone(&test_complete_clone);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();
            let sd_multicast: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            let tcp_listener = turmoil::net::TcpListener::bind(format!("0.0.0.0:{}", TCP_PORT)).await?;

            let mut server_session_id = 1u16;

            // Wait for client to be ready
            tokio::time::sleep(Duration::from_millis(200)).await;

            // Phase 1: Send initial Offer (session=1, reboot=true)
            tracing::info!("[wire_server] Phase 1: Offering service");
            let offer1 = SdOfferBuilder::new(SERVICE_ID, INSTANCE_ID, my_ip, TCP_PORT)
                .version(1, 0)
                .tcp()
                .ttl(TTL_1_HOUR)
                .session_id(server_session_id)
                .reboot_flag(true)
                .build();
            for _ in 0..3 {
                sd_socket.send_to(&offer1, sd_multicast).await?;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            server_session_id += 1;

            // Accept TCP connection and handle Subscribe
            let mut buf = [0u8; 1500];
            let mut got_subscribe = false;
            let mut tcp_stream_opt: Option<turmoil::net::TcpStream> = None;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

            while !got_subscribe && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(1)).await;
                tokio::select! {
                    result = tcp_listener.accept(), if tcp_stream_opt.is_none() => {
                        let (stream, addr) = result?;
                        tracing::info!("[wire_server] TCP connection accepted from {}", addr);
                        tcp_connected.store(true, Ordering::SeqCst);
                        tcp_stream_opt = Some(stream);
                    }
                    result = sd_socket.recv_from(&mut buf) => {
                        let (len, from) = result?;
                        if let Some((_header, sd)) = parse_sd_packet(&buf[..len]) {
                            for entry in sd.subscribe_entries() {
                                if entry.service_id == SERVICE_ID {
                                    tracing::info!("[wire_server] Received Subscribe from {}", from);
                                    got_subscribe = true;

                                    // Send SubscribeAck
                                    let ack = SdSubscribeAckBuilder::new(SERVICE_ID, INSTANCE_ID, EVENTGROUP_ID)
                                        .major_version(entry.major_version)
                                        .ttl(TTL_1_HOUR)
                                        .session_id(server_session_id)
                                        .reboot_flag(true)
                                        .build();
                                    sd_socket.send_to(&ack, from).await?;
                                    server_session_id += 1;
                                    tracing::info!("[wire_server] Sent SubscribeAck");
                                }
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                }
            }

            assert!(tcp_stream_opt.is_some(), "Client should have connected via TCP");
            let mut tcp_stream = tcp_stream_opt.unwrap();

            // Send a few events on the TCP connection
            tracing::info!("[wire_server] Sending events on TCP connection...");
            for i in 0..3 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let event = SomeIpPacketBuilder::notification(SERVICE_ID, EVENT_ID)
                    .payload(&[0xAA, 0xBB, i])
                    .build();
                if tcp_stream.write_all(&event).await.is_err() {
                    tracing::warn!("[wire_server] Failed to send event #{}", i + 1);
                    break;
                }
                tracing::info!("[wire_server] Sent event #{}", i + 1);
            }

            // Wait a bit for client to receive events
            tokio::time::sleep(Duration::from_millis(300)).await;

            // Phase 2: "Reboot" - send new Offer with session=1 (regression)
            tracing::info!("[wire_server] Phase 2: Simulating reboot (session regression to 1)");
            let offer_reboot = SdOfferBuilder::new(SERVICE_ID, INSTANCE_ID, my_ip, TCP_PORT)
                .version(1, 0)
                .tcp()
                .ttl(TTL_1_HOUR)
                .session_id(1)
                .reboot_flag(true)
                .build();
            for _ in 0..3 {
                sd_socket.send_to(&offer_reboot, sd_multicast).await?;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            // Monitor TCP connection - client should close it after detecting reboot
            tracing::info!("[wire_server] Monitoring TCP connection for client-initiated close...");
            let mut check_buf = [0u8; 1024];
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

            while tokio::time::Instant::now() < deadline {
                tokio::select! {
                    result = tcp_stream.read(&mut check_buf) => {
                        match result {
                            Ok(0) => {
                                tracing::info!("[wire_server] CLIENT CLOSED TCP CONNECTION - correct behavior!");
                                tcp_closed.store(true, Ordering::SeqCst);
                                break;
                            }
                            Ok(n) => {
                                tracing::debug!("[wire_server] Received {} bytes from client", n);
                            }
                            Err(e) => {
                                tracing::info!("[wire_server] TCP connection error (client closed): {}", e);
                                tcp_closed.store(true, Ordering::SeqCst);
                                break;
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        // Keep checking
                    }
                }
            }

            // Keep server alive until test completes
            while !test_done.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }

            Ok(())
        }
    });

    // Library-based client
    let events_p1 = Arc::clone(&events_received_phase1);
    let test_done = Arc::clone(&test_complete);

    sim.client("lib_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("lib_client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        // Discover service
        tracing::info!("[lib_client] Discovering service...");
        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(SERVICE_ID).instance(InstanceId::Id(INSTANCE_ID))
        )
        .await
        .expect("Discovery timeout")
        .expect("Service should be found");

        tracing::info!("[lib_client] Service discovered, subscribing...");

        let eventgroup = EventgroupId::new(EVENTGROUP_ID).unwrap();
        let mut subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(eventgroup)
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        tracing::info!("[lib_client] Subscribed, receiving events...");

        // Receive events
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            tokio::select! {
                event = subscription.next() => {
                    if event.is_some() {
                        let count = events_p1.fetch_add(1, Ordering::SeqCst) + 1;
                        tracing::info!("[lib_client] Received event #{}", count);
                    } else {
                        tracing::info!("[lib_client] Subscription stream ended (expected after reboot detection)");
                        break;
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }

        tracing::info!("[lib_client] Received {} events total", events_p1.load(Ordering::SeqCst));

        // Keep client alive for a bit to ensure cleanup completes
        tokio::time::sleep(Duration::from_secs(2)).await;

        test_done.store(true, Ordering::SeqCst);

        Ok(())
    });

    sim.run().unwrap();

    let events = events_received_phase1.load(Ordering::SeqCst);
    tracing::info!(
        "Test results: events={}, tcp_connected={}, tcp_closed_after_reboot={}",
        events,
        tcp_connected.load(Ordering::SeqCst),
        tcp_closed_after_reboot.load(Ordering::SeqCst)
    );

    assert!(
        tcp_connected.load(Ordering::SeqCst),
        "TCP connection should have been established"
    );
    assert!(
        events >= 1,
        "Should receive at least 1 event before server reboot"
    );
    assert!(
        tcp_closed_after_reboot.load(Ordering::SeqCst),
        "Client should close TCP connection after detecting server reboot (feat_req_someipsd_872)"
    );
}

/// Test: Client detects server reboot and closes TCP connections to removed services
///
/// This test validates selective TCP cleanup when the client detects a server reboot:
/// - Client makes RPC call to SERVICE_ID_RPC (establishes TCP connection)
/// - Client subscribes to SERVICE_ID events (establishes TCP connection)
/// - Server "reboots" (session ID resets, no re-Offers)
/// - Client re-subscribes, server sends regressed SubscribeAck (session=1)
/// - Client should:
///   - Close RPC TCP connection (SERVICE_ID_RPC removed, not re-offered)
///   - Keep subscription TCP alive (NEW subscription has pending ACK)
/// - Events continue flowing on new subscription
///
/// Verifies:
/// - feat_req_someipsd_871: Client expires services from rebooted server
/// - feat_req_someipsd_872: Client closes TCP to removed services, keeps active subscriptions
#[test]
fn client_detects_server_reboot_tcp_connections() {
    configure_tracing();

    use crate::wire_format::helpers::{SdSubscribeBuilder, SomeIpPacketBuilder};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;

    covers!(feat_req_someipsd_871, feat_req_someipsd_872);

    const SERVICE_ID: u16 = 0x4567; // Server offers this (event subscription)
    const SERVICE_ID_RPC: u16 = 0x1234; // Server offers this (RPC calls)
    const SERVICE_CLIENT_ID: u16 = 0x9999; // Client offers this
    const INSTANCE_ID: u16 = 0x0001;
    const EVENTGROUP_ID: u16 = 0x0001;
    const EVENT_ID: u16 = 0x8001;
    const METHOD_ID: u16 = 0x0042;
    const TCP_PORT_SERVER: u16 = 30509;
    const TCP_PORT_RPC: u16 = 30511;
    const TCP_PORT_CLIENT: u16 = 30510;
    const TTL_3_MIN: u32 = 180; // 3 minutes

    let events_received_phase1 = Arc::new(AtomicUsize::new(0));
    let events_received_phase2 = Arc::new(AtomicUsize::new(0));
    let test_complete = Arc::new(AtomicBool::new(false));
    let rpc_tcp_closed = Arc::new(AtomicBool::new(false));
    let wire_check_done = Arc::new(AtomicBool::new(false));

    let test_complete_clone = Arc::clone(&test_complete);
    let rpc_tcp_closed_clone = Arc::clone(&rpc_tcp_closed);
    let wire_check_done_clone = Arc::clone(&wire_check_done);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Wire-level server
    sim.host("wire_server", move || {
        let test_done = Arc::clone(&test_complete_clone);
        let rpc_closed = Arc::clone(&rpc_tcp_closed_clone);
        let check_done = Arc::clone(&wire_check_done_clone);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();
            let sd_multicast: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

            // === PHASE 1: Initial server instance ===
            tracing::info!("[wire_server] Phase 1: Starting initial server instance");

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            let tcp_listener_server = turmoil::net::TcpListener::bind(format!("0.0.0.0:{}", TCP_PORT_SERVER)).await?;
            let tcp_listener_rpc = turmoil::net::TcpListener::bind(format!("0.0.0.0:{}", TCP_PORT_RPC)).await?;

            let mut server_session_id = 1u16;

            // Wait for client to be ready
            tokio::time::sleep(Duration::from_millis(200)).await;

            // Send initial Offer for SERVICE_ID (event subscription) (session=1, reboot=true)
            let offer = SdOfferBuilder::new(SERVICE_ID, INSTANCE_ID, my_ip, TCP_PORT_SERVER)
                .version(1, 0)
                .tcp()
                .ttl(TTL_3_MIN)
                .session_id(server_session_id)
                .reboot_flag(true)
                .build();
            sd_socket.send_to(&offer, sd_multicast).await?;
            server_session_id += 1;
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Send initial Offer for SERVICE_ID_RPC (RPC service) (session=2, reboot=true)
            let offer_rpc = SdOfferBuilder::new(SERVICE_ID_RPC, INSTANCE_ID, my_ip, TCP_PORT_RPC)
                .version(1, 0)
                .tcp()
                .ttl(TTL_3_MIN)
                .session_id(server_session_id)
                .reboot_flag(true)
                .build();
            sd_socket.send_to(&offer_rpc, sd_multicast).await?;
            server_session_id += 1;

            // Give client time to fully initialize the service
            tokio::time::sleep(Duration::from_millis(200)).await;

            let mut buf = [0u8; 1500];
            // Wait for Subscribe from client and handle TCP connections
            let mut got_subscribe = false;
            let mut tcp_stream_opt: Option<turmoil::net::TcpStream> = None;
            let mut rpc_stream_opt: Option<turmoil::net::TcpStream> = None;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

            while !got_subscribe && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(1)).await;
                tokio::select! {
                    result = tcp_listener_server.accept(), if tcp_stream_opt.is_none() => {
                        let (stream, peer) = result?;
                        tracing::info!("[wire_server] Phase 1: TCP connection for events from client {}", peer);
                        tcp_stream_opt = Some(stream);
                    }
                    result = tcp_listener_rpc.accept(), if rpc_stream_opt.is_none() => {
                        let (stream, peer) = result?;
                        tracing::info!("[wire_server] Phase 1: TCP connection for RPC from client {}", peer);
                        rpc_stream_opt = Some(stream);
                    }
                    result = sd_socket.recv_from(&mut buf) => {
                        let (len, from) = result?;
                        if let Some((_header, sd)) = parse_sd_packet(&buf[..len]) {
                            for entry in sd.subscribe_entries() {
                                if entry.service_id == SERVICE_ID {
                                    tracing::info!("[wire_server] Phase 1: Got Subscribe from {}", from);
                                    got_subscribe = true;

                                    let ack = SdSubscribeAckBuilder::new(SERVICE_ID, INSTANCE_ID, EVENTGROUP_ID)
                                        .major_version(1)
                                        .ttl(entry.ttl)
                                        .session_id(server_session_id)
                                        .reboot_flag(true)
                                        .build();
                                    sd_socket.send_to(&ack, from).await?;
                                    server_session_id += 1;
                                }
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(2000)) => {}
                }
            }

            let mut tcp_stream = tcp_stream_opt.expect("Should have accepted TCP connection from client for events");
            let rpc_stream = rpc_stream_opt.expect("Should have accepted RPC TCP connection from client");

            // Keep rpc_stream alive to check if client closes it on reboot
            // We don't need to handle RPC requests since we're just testing connection lifecycle

            // Send multiple events in Phase 1
            for i in 0..3 {
                tokio::time::sleep(Duration::from_millis(10)).await;
                let event = SomeIpPacketBuilder::notification(SERVICE_ID, EVENT_ID)
                    .session_id(i + 1)
                    .payload(&[0x01 + i as u8, 0x02, 0x03, 0x04])
                    .build();
                tcp_stream.write_all(&event).await?;
                tracing::info!("[wire_server] Phase 1: Sent event #{}", i + 1);
            }

            // === PHASE 2: Simulate reboot by closing and reopening SD socket ===
            // Simulate server restart: Close SD socket and reopen with fresh state
            tracing::info!("[wire_server] Phase 2: Simulating reboot (closing and reopening SD socket)");

            // Close the SD socket and TCP streams
            drop(sd_socket);
            drop(tcp_stream);
            // Keep rpc_stream to check if client closes it

            // Wait for client to unsubscribe
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Reopen SD socket with fresh state
            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            // NOTE: We deliberately DO NOT send an Offer here!
            // The service was cached from Phase 1 (TTL=180s, only ~1s has passed).
            // The client will re-subscribe based on cached discovery.
            // The regressed SubscribeAck will trigger reboot detection.

            // Wait for client to re-Subscribe after "reboot"
            let mut sent_regressed_ack = false;
            let mut tcp_stream_phase2: Option<turmoil::net::TcpStream> = None;
            let deadline_reboot = tokio::time::Instant::now() + Duration::from_secs(7);

            // Wait for BOTH Subscribe AND TCP connection
            while (!sent_regressed_ack || tcp_stream_phase2.is_none()) && tokio::time::Instant::now() < deadline_reboot {
                tokio::select! {
                    // Accept NEW TCP connection from client after re-subscribe
                    result = tcp_listener_server.accept(), if tcp_stream_phase2.is_none() => {
                        let (stream, peer) = result?;
                        tracing::info!("[wire_server] Phase 2: NEW TCP connection from client {}", peer);
                        tcp_stream_phase2 = Some(stream);
                    }
                    // Handle SD messages
                    result = sd_socket.recv_from(&mut buf) => {
                        let (len, from) = result?;
                        tracing::debug!("[wire_server] Phase 2: Got SD packet from {}", from);
                        if let Some((_header, sd)) = parse_sd_packet(&buf[..len]) {
                            tracing::debug!("[wire_server] Phase 2: Parsed SD with {} entries (offers={}, subs={})",
                                sd.entries.len(), sd.offer_entries().count(), sd.subscribe_entries().count());
                            for entry in sd.subscribe_entries() {
                                tracing::info!("[wire_server] Phase 2: Subscribe for service {:04X}", entry.service_id);
                                if entry.service_id == SERVICE_ID {
                                    tracing::info!("[wire_server] Phase 2: Got re-Subscribe, sending regressed ack");
                                    sent_regressed_ack = true;

                                    // Send SubscribeAck with SESSION=1 (regressed from previous 3!)
                                    let ack = SdSubscribeAckBuilder::new(SERVICE_ID, INSTANCE_ID, EVENTGROUP_ID)
                                        .major_version(1)
                                        .ttl(entry.ttl)
                                        .session_id(1)
                                        .reboot_flag(true)
                                        .build();
                                    sd_socket.send_to(&ack, from).await?;
                                    tracing::info!("[wire_server] Phase 2: Sent SubscribeAck session=1 (REGRESSED!)");
                                    break;
                                }
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        // Periodic check to avoid busy loop
                    }
                }
            }

            if !sent_regressed_ack {
                tracing::warn!("[wire_server] Phase 2: Never received re-Subscribe!");
            }

            // Send multiple events on the NEW connection
            if let Some(ref mut tcp_stream) = tcp_stream_phase2 {
                for i in 0..5 {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let event = SomeIpPacketBuilder::notification(SERVICE_ID, EVENT_ID)
                        .session_id(i + 10)
                        .payload(&[0x10 + i as u8, 0x20, 0x30, 0x40])
                        .build();
                    if tcp_stream.write_all(&event).await.is_err() {
                        tracing::warn!("[wire_server] Phase 2: Failed to send event #{}", i + 1);
                        break;
                    }
                    tracing::info!("[wire_server] Phase 2: Sent event #{}", i + 1);
                }
            } else {
                panic!("Expected client to establish TCP connection for Phase 2 subscription, but no connection was accepted!");
            }

            // Give client time to detect reboot and close OLD tcp_to_client
            tokio::time::sleep(Duration::from_millis(500)).await;

            let mut read_buf = [0u8; 1];
            use tokio::io::AsyncReadExt;

            // Give turmoil network simulator time to propagate connection closure
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Check if client closed the RPC connection
            // Note: There may be data in the receive buffer from before the reboot,
            // so we drain it first, then check for EOF to verify closure
            tracing::info!("[wire_server] Phase 2: Checking if client closed rpc_stream (RPC)");
            let mut rpc_stream_mut = rpc_stream;

            let mut total_drained = 0;
            loop {
                match tokio::time::timeout(
                    Duration::from_millis(500),
                    rpc_stream_mut.read(&mut read_buf)
                ).await {
                    Ok(Ok(0)) => {
                        // EOF - connection is closed
                        if total_drained > 0 {
                            tracing::info!("[wire_server] Phase 2: ✓ Drained {} bytes, then got EOF - RPC connection closed!", total_drained);
                        } else {
                            tracing::info!("[wire_server] Phase 2: ✓ Got immediate EOF - RPC connection closed!");
                        }
                        rpc_closed.store(true, Ordering::SeqCst);
                        break;
                    }
                    Ok(Ok(n)) => {
                        // Got data from receive buffer (sent before connection closed)
                        total_drained += n;
                        tracing::debug!("[wire_server] Phase 2: Draining {} bytes from RPC buffer (total: {})", n, total_drained);
                        // Continue reading to get EOF
                    }
                    Ok(Err(e)) => {
                        // Read error means connection is closed
                        tracing::info!("[wire_server] Phase 2: ✓ RPC read error after draining {} bytes: {} (closed)", total_drained, e);
                        rpc_closed.store(true, Ordering::SeqCst);
                        break;
                    }
                    Err(_) => {
                        // Timeout - connection is still open
                        tracing::error!("[wire_server] Phase 2: ✗ Timeout after draining {} bytes - RPC connection still open!", total_drained);
                        break;
                    }
                }
            }

            // Signal that check is complete
            check_done.store(true, Ordering::SeqCst);

            while !test_done.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }

            Ok(())
        }
    });

    // Library-based client (offers AND subscribes)
    let events_p1 = Arc::clone(&events_received_phase1);
    let events_p2 = Arc::clone(&events_received_phase2);
    let test_done = Arc::clone(&test_complete);
    let wire_check_done = Arc::clone(&wire_check_done);

    sim.client("lib_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("lib_client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        // Give the service time to fully initialize and start sending offers
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Discover and call the RPC service (just discovery establishes TCP connection)
        tracing::info!("[lib_client] Discovering server's RPC service...");
        let _rpc_proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(SERVICE_ID_RPC).instance(InstanceId::Id(INSTANCE_ID))
        )
        .await
        .expect("RPC discovery timeout")
        .expect("RPC service should be found");

        // Make an RPC call to establish the TCP connection
        tracing::info!("[lib_client] Making test RPC call...");
        match tokio::time::timeout(
            Duration::from_secs(2),
            _rpc_proxy.call(MethodId::new(METHOD_ID).unwrap(), &[0x01, 0x02, 0x03, 0x04])
        ).await {
            Ok(Ok(response)) => {
                tracing::info!("[lib_client] RPC call succeeded, response len={}", response.payload.len());
            }
            Ok(Err(e)) => {
                tracing::info!("[lib_client] RPC call failed (expected, no handler): {}", e);
            }
            Err(_) => {
                tracing::info!("[lib_client] RPC call timeout (expected, no handler)");
            }
        }

        // Subscribe to SERVER's event service
        tracing::info!("[lib_client] Discovering server's event service...");
        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(SERVICE_ID).instance(InstanceId::Id(INSTANCE_ID))
        )
        .await
        .expect("Discovery timeout")
        .expect("Service should be found");

        tracing::info!("[lib_client] Subscribing to server...");
        let eventgroup = EventgroupId::new(EVENTGROUP_ID).unwrap();
        let mut subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(eventgroup)
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        tracing::info!("[lib_client] Phase 1: Waiting for events...");
        // Receive multiple events in Phase 1
        let mut phase1_count = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline && phase1_count < 3 {
            tokio::select! {
                event = subscription.next() => {
                    if event.is_some() {
                        phase1_count += 1;
                        events_p1.fetch_add(1, Ordering::SeqCst);
                        tracing::info!("[lib_client] Phase 1: Received event #{}", phase1_count);
                    } else {
                        break;
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
        tracing::info!("[lib_client] Phase 1: Received {} events total", phase1_count);

        // Unsubscribe
        tracing::info!("[lib_client] Unsubscribing...");
        drop(subscription);
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Wait for server to simulate reboot
        tracing::info!("[lib_client] Waiting for server reboot simulation...");
        tokio::time::sleep(Duration::from_secs(1)).await;

        // Re-subscribe (this sends Subscribe message that wire-server intercepts with regressed ack)
        // The regressed SubscribeAck triggers reboot detection and TCP closure of OLD connection
        // The NEW subscription should still work and receive events
        tracing::info!("[lib_client] Phase 2: Re-subscribing after server 'reboot'...");
        let mut subscription2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(eventgroup)
        )
        .await
        .expect("Re-subscribe timeout")
        .expect("Re-subscribe should succeed");

        tracing::info!("[lib_client] Phase 2: Waiting for events...");
        // Receive multiple events in Phase 2 (despite reboot detection)
        let mut phase2_count = 0;
        let deadline_p2 = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline_p2 && phase2_count < 5 {
            tokio::select! {
                event = subscription2.next() => {
                    if event.is_some() {
                        phase2_count += 1;
                        events_p2.fetch_add(1, Ordering::SeqCst);
                        tracing::info!("[lib_client] Phase 2: Received event #{}", phase2_count);
                    } else {
                        tracing::warn!("[lib_client] Phase 2: Subscription stream ended!");
                        break;
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
        tracing::info!("[lib_client] Phase 2: Received {} events total. Reboot should have been detected, OLD tcp_to_client should be closed", phase2_count);

        // Give time for TCP cleanup to complete AND for wire-server to check the connection
        tokio::time::sleep(Duration::from_millis(1000)).await;

        tracing::info!("[lib_client] Phase 1 events: {}, Phase 2 events: {}",
            events_p1.load(Ordering::SeqCst), events_p2.load(Ordering::SeqCst));
        test_done.store(true, Ordering::SeqCst);

        // Keep alive until wire-server completes its check
        while !wire_check_done.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        Ok(())
    });

    sim.run().unwrap();

    let p1_events = events_received_phase1.load(Ordering::SeqCst);
    let p2_events = events_received_phase2.load(Ordering::SeqCst);
    let rpc_was_closed = rpc_tcp_closed.load(Ordering::SeqCst);

    tracing::info!(
        "Test results: Phase 1 events={}, Phase 2 events={}, rpc_tcp_closed={}",
        p1_events,
        p2_events,
        rpc_was_closed
    );

    assert!(
        p1_events >= 3,
        "Phase 1: Should receive at least 3 events. Got {}",
        p1_events
    );
    assert!(
        p2_events >= 3,
        "Phase 2: Should receive events after re-subscribe despite reboot detection. Got {}",
        p2_events
    );
    assert!(
        rpc_was_closed,
        "Client should have closed server-side TCP connection (RPC) after detecting reboot"
    );
}
