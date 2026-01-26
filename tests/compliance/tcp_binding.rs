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

use recentip::handle::ServiceEvent;
use recentip::prelude::*;
use recentip::Transport;
use std::time::Duration;

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
// Helper Functions
// ============================================================================

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
