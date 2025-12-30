//! TCP Binding Compliance Tests
//!
//! Tests for SOME/IP over TCP transport binding.
//!
//! NOTE: These tests are currently ignored because TCP transport is not yet
//! implemented in the runtime. They serve as TDD targets for future implementation.
//!
//! Key requirements tested:
//! - feat_req_recentip_324: TCP binding is based on UDP binding
//! - feat_req_recentip_325: Nagle's algorithm disabled
//! - feat_req_recentip_326: Connection lost handling (requests timeout, NOT reboot)
//! - feat_req_recentip_585: Every payload has its own header
//! - feat_req_recentip_586: Magic Cookies allow resync in testing
//! - feat_req_recentip_591: TCP segment starts with Magic Cookie
//! - feat_req_recentip_592: Only one Magic Cookie per segment
//! - feat_req_recentip_644: Single TCP connection per client-server pair
//! - feat_req_recentip_646: Client opens TCP connection
//! - feat_req_recentip_647: Client reestablishes connection
//! - feat_req_recentip_702: Multiple messages per segment supported
//! - feat_req_recentipsd_872: Reboot detection triggers TCP reset

use bytes::Bytes;
use someip_runtime::prelude::*;
use someip_runtime::runtime::{Runtime, RuntimeConfig};
use someip_runtime::wire::Header;
use someip_runtime::handle::ServiceEvent;
use std::time::Duration;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

/// Type alias for turmoil-based runtime (using UdpSocket for now, will need TcpStream)
type TurmoilRuntime = Runtime<turmoil::net::UdpSocket>;

/// Test service definition
struct TestService;

impl Service for TestService {
    const SERVICE_ID: u16 = 0x1234;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Helper to parse a SOME/IP header from raw bytes
#[allow(dead_code)]
fn parse_header(data: &[u8]) -> Option<Header> {
    if data.len() < Header::SIZE {
        return None;
    }
    Header::parse(&mut Bytes::copy_from_slice(data))
}

/// Parse multiple SOME/IP messages from a TCP byte stream
#[allow(dead_code)]
fn parse_tcp_stream(data: &[u8]) -> Vec<Header> {
    let mut headers = Vec::new();
    let mut offset = 0;

    while offset + 16 <= data.len() {
        if let Some(header) = parse_header(&data[offset..]) {
            let msg_len = 8 + header.length as usize; // First 8 bytes + length field
            if offset + msg_len <= data.len() {
                headers.push(header);
                offset += msg_len;
            } else {
                break; // Incomplete message
            }
        } else {
            break;
        }
    }

    headers
}

/// Check if a byte sequence looks like a Magic Cookie
#[allow(dead_code)]
fn is_magic_cookie(data: &[u8]) -> bool {
    // Magic Cookie: Service ID 0xFFFF, Method ID 0x0000 or specific pattern
    if data.len() < 16 {
        return false;
    }
    // Check for Magic Cookie pattern per spec
    data[0] == 0xFF && data[1] == 0xFF && data[2] == 0x00 && data[3] == 0x00
}

// ============================================================================
// TCP Transport Config Stubs
// ============================================================================

/// Transport type for runtime configuration (stub for TCP support)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Transport {
    Udp,
    Tcp,
}

/// Extended runtime config builder with TCP support (stub)
#[allow(dead_code)]
trait TcpRuntimeConfigExt {
    fn transport(self, transport: Transport) -> Self;
    fn magic_cookies(self, enabled: bool) -> Self;
}

// ============================================================================
// TCP Connection Lifecycle Tests
// ============================================================================

/// feat_req_recentip_646: Client opens TCP connection when first request sent
///
/// The TCP connection shall be opened by the client, when the first
/// request is to be sent to the server.
#[test]
#[ignore = "TCP transport not implemented"]
fn client_opens_tcp_connection() {
    covers!(feat_req_recentip_646);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::default();
        // TODO: config.transport(Transport::Tcp)
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Wait for connection and request
        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::Call { responder, .. } => {
                    responder.reply(b"response").await.unwrap();
                }
                _ => {}
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::default();
        // TODO: config.transport(Transport::Tcp)
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
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

/// feat_req_recentip_644: Single TCP connection for all messages between client-server
///
/// The client and server shall use a single TCP connection for
/// all SOME/IP messages between them.
#[test]
#[ignore = "TCP transport not implemented"]
fn single_tcp_connection_reused() {
    covers!(feat_req_recentip_644);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Handle multiple requests
        for _ in 0..5 {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call { payload, responder, .. } => {
                        responder.reply(&payload).await.unwrap();
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

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Timeout")
            .expect("Service available");

        let method_id = MethodId::new(0x0001).unwrap();

        // Send multiple requests
        for i in 0u8..5 {
            let response = tokio::time::timeout(
                Duration::from_secs(5),
                proxy.call(method_id, &[i]),
            )
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

/// feat_req_recentip_647: Client reestablishes connection after failure
///
/// The client is responsible for reestablishing the TCP connection
/// after it has been closed or lost.
#[test]
#[ignore = "TCP transport not implemented"]
fn client_reestablishes_connection() {
    covers!(feat_req_recentip_647);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Handle requests (may get multiple after reconnection)
        loop {
            tokio::select! {
                event = offering.next() => {
                    if let Some(ServiceEvent::Call { payload, responder, .. }) = event {
                        responder.reply(&payload).await.unwrap();
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(500)) => break,
            }
        }

        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Timeout")
            .expect("Service available");

        let method_id = MethodId::new(0x0001).unwrap();

        // First request establishes connection
        let _response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(method_id, b"first"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");

        // TODO: Simulate connection loss using turmoil::partition("server", "client")
        // turmoil::partition("server", "client");
        // tokio::time::sleep(Duration::from_millis(100)).await;
        // turmoil::repair("server", "client");

        // Second request after "reconnection" should succeed
        let _response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(method_id, b"second"),
        )
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

/// feat_req_recentip_585: Every SOME/IP payload has its own header
///
/// Every SOME/IP payload shall have its own SOME/IP header (no batching
/// of payloads under single header).
#[test]
#[ignore = "TCP transport not implemented"]
fn each_message_has_own_header() {
    covers!(feat_req_recentip_585);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Each message arrives with its own header
        for expected in [b"short".as_slice(), b"medium length", b"a"] {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call { payload, responder, .. } => {
                        assert_eq!(payload.as_ref(), expected);
                        responder.reply(expected).await.unwrap();
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

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Timeout")
            .expect("Service available");

        let method_id = MethodId::new(0x0001).unwrap();

        // Send multiple requests with different payloads
        for payload in [b"short".as_slice(), b"medium length", b"a"] {
            let response = tokio::time::timeout(
                Duration::from_secs(5),
                proxy.call(method_id, payload),
            )
            .await
            .expect("Timeout")
            .expect("Call should succeed");

            assert_eq!(response.payload.as_ref(), payload);
        }

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentip_702: Multiple messages can be in one TCP segment
///
/// All Transport Protocol Bindings shall support transporting more than one
/// SOME/IP message in a single TCP segment.
#[test]
#[ignore = "TCP transport not implemented"]
fn multiple_messages_per_segment_parsed() {
    covers!(feat_req_recentip_702);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Server should receive all 3 requests even if sent in single segment
        let mut received_count = 0;
        loop {
            tokio::select! {
                event = offering.next() => {
                    if let Some(ServiceEvent::Call { responder, .. }) = event {
                        responder.reply(&[]).await.unwrap();
                        received_count += 1;
                        if received_count >= 3 {
                            break;
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(5)) => break,
            }
        }

        assert_eq!(received_count, 3, "Server should parse all messages from TCP stream");
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Timeout")
            .expect("Service available");

        let method_id = MethodId::new(0x0001).unwrap();

        // Send requests quickly (may be coalesced in TCP)
        for i in 0u8..3 {
            let _response = tokio::time::timeout(
                Duration::from_secs(5),
                proxy.call(method_id, &[i]),
            )
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

/// feat_req_recentip_326: Outstanding requests fail when connection lost
///
/// When the TCP connection is lost, outstanding requests shall be
/// handled as if a timeout occurred.
#[test]
#[ignore = "TCP transport not implemented"]
fn connection_lost_fails_pending_requests() {
    covers!(feat_req_recentip_326);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
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

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
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

/// feat_req_recentip_586: Magic Cookies allow resync in testing
///
/// In order to allow resynchronization to SOME/IP over TCP in testing and
/// debugging scenarios, implementations shall support Magic Cookies.
#[test]
#[ignore = "TCP transport and Magic Cookies not implemented"]
fn magic_cookie_recognized() {
    covers!(feat_req_recentip_586);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::default();
        // TODO: config.magic_cookies(true)
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Server should receive request even with magic cookies
        if let Some(ServiceEvent::Call { responder, .. }) = offering.next().await {
            responder.reply(b"response").await.unwrap();
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::default();
        // TODO: config.magic_cookies(true)
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Timeout")
            .expect("Service available");

        let method_id = MethodId::new(0x0001).unwrap();

        // With magic cookies enabled, communication should still work
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(method_id, b"request"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed with magic cookies");

        assert_eq!(response.payload.as_ref(), b"response");

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentip_591: TCP segment starts with Magic Cookie
///
/// Each TCP segment shall start with a SOME/IP Magic Cookie Message
/// (when magic cookies are enabled).
#[test]
#[ignore = "TCP transport and Magic Cookies not implemented"]
fn tcp_segment_starts_with_magic_cookie() {
    covers!(feat_req_recentip_591);

    // This test would require capturing raw TCP data to verify
    // the first bytes of each segment are a Magic Cookie.
    // With turmoil, we'd use a TcpListener to capture traffic.

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // TODO: Implement with raw TCP capture
    // The test should verify that when magic_cookies(true) is set,
    // the first 16 bytes of each TCP write are the Magic Cookie pattern:
    // Service ID: 0xFFFF, Method ID: 0x0000, Length: 8, etc.

    sim.client("placeholder", async {
        // Placeholder until TCP is implemented
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentip_592: Only one Magic Cookie per segment
///
/// The implementation shall only include up to one SOME/IP Magic Cookie
/// Message per TCP segment.
#[test]
#[ignore = "TCP transport and Magic Cookies not implemented"]
fn only_one_magic_cookie_per_segment() {
    covers!(feat_req_recentip_592);

    // This test would capture TCP data and verify that each segment
    // contains at most one Magic Cookie (at the beginning).

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.client("placeholder", async {
        // Placeholder until TCP is implemented
        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// Wire Format Consistency (TCP vs UDP)
// ============================================================================

/// feat_req_recentip_324: TCP binding based on UDP binding
///
/// The TCP binding of SOME/IP is heavily based on the UDP binding.
/// The header format is identical.
#[test]
#[ignore = "TCP transport not implemented"]
fn tcp_header_format_matches_udp() {
    covers!(feat_req_recentip_324);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        if let Some(ServiceEvent::Call { responder, .. }) = offering.next().await {
            responder.reply(b"response").await.unwrap();
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Timeout")
            .expect("Service available");

        let method_id = MethodId::new(0x0001).unwrap();

        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(method_id, b"request"),
        )
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
#[test]
#[ignore = "TCP transport not implemented"]
fn tcp_loss_does_not_reset_session_id() {
    // This test verifies the distinction between:
    // - feat_req_recentip_326: TCP loss → requests timeout (no session reset)
    // - feat_req_recentipsd_872: Reboot detected → TCP reset (session DOES reset)
    covers!(feat_req_recentip_326, feat_req_recentip_647);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Handle requests
        loop {
            tokio::select! {
                event = offering.next() => {
                    if let Some(ServiceEvent::Call { responder, .. }) = event {
                        responder.reply(&[]).await.unwrap();
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => break,
            }
        }

        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Timeout")
            .expect("Service available");

        let method_id = MethodId::new(0x0001).unwrap();

        // First request
        let _response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(method_id, b"first"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");

        // TODO: Capture session_id from first request

        // TODO: Simulate TCP loss and reconnection
        // turmoil::partition("server", "client");
        // tokio::time::sleep(Duration::from_millis(100)).await;
        // turmoil::repair("server", "client");

        // Second request after reconnection
        let _response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(method_id, b"second"),
        )
        .await
        .expect("Timeout")
        .expect("Reconnection should succeed");

        // TODO: Verify session_id continued incrementing (not reset to 1)
        // Session ID should be session_before + 1 (or wrapped), not 1

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_872: Reboot detection triggers TCP connection reset
///
/// When the system detects the reboot of a peer (via SD Reboot Flag),
/// it shall reset the state of TCP connections to that peer.
#[test]
#[ignore = "TCP transport and reboot detection not implemented"]
fn reboot_detection_resets_tcp_connections() {
    covers!(feat_req_recentipsd_872);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // This test requires:
    // 1. Establish TCP connection
    // 2. Simulate server reboot (new server process with Reboot Flag = 1 in SD)
    // 3. Client detects Reboot Flag in OfferService
    // 4. Client resets TCP connection state and reconnects

    sim.client("placeholder", async {
        // TODO: Full implementation when TCP and reboot detection are available
        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// Nagle's Algorithm Tests
// ============================================================================

/// feat_req_recentip_325: Nagle's algorithm disabled
///
/// Nagle's algorithm shall be disabled on TCP connections to reduce latency.
#[test]
#[ignore = "TCP transport not implemented"]
fn tcp_nodelay_enabled() {
    covers!(feat_req_recentip_325);

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

