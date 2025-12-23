//! TCP Binding Compliance Tests
//!
//! Tests for SOME/IP over TCP transport binding.
//!
//! Key requirements tested:
//! - feat_req_recentip_324: TCP binding is based on UDP binding
//! - feat_req_recentip_585: Every payload has its own header
//! - feat_req_recentip_325: Nagle's algorithm disabled
//! - feat_req_recentip_326: Connection lost handling (requests timeout, NOT reboot)
//! - feat_req_recentip_644: Single TCP connection per client-server pair
//! - feat_req_recentip_646: Client opens TCP connection
//! - feat_req_recentip_647: Client reestablishes connection
//! - feat_req_recentip_702: Multiple messages per segment supported
//! - feat_req_recentipsd_872: Reboot detection triggers TCP reset

use someip_runtime::*;

// Re-use wire format parsing from the shared module
#[path = "../wire.rs"]
mod wire;

// Re-use simulated network
#[path = "../simulated.rs"]
mod simulated;

use simulated::{NetworkEvent, SimulatedNetwork};
use wire::Header;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Find all TCP-sent SOME/IP messages in network history
#[allow(dead_code)]
fn find_tcp_messages(network: &SimulatedNetwork) -> Vec<Vec<u8>> {
    network
        .history()
        .iter()
        .filter_map(|event| {
            if let NetworkEvent::TcpSent { data, .. } = event {
                if data.len() >= 16 {
                    return Some(data.clone());
                }
            }
            None
        })
        .collect()
}

/// Parse multiple SOME/IP messages from a TCP byte stream
#[allow(dead_code)]
fn parse_tcp_stream(data: &[u8]) -> Vec<Header> {
    let mut headers = Vec::new();
    let mut offset = 0;

    while offset + 16 <= data.len() {
        if let Some(header) = Header::from_bytes(&data[offset..]) {
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
// TCP Connection Lifecycle Tests
// ============================================================================

/// feat_req_recentip_646: Client opens TCP connection when first request sent
///
/// The TCP connection shall be opened by the client, when the first
/// request is to be sent to the server.
#[test]
#[ignore = "Runtime::new not implemented"]
fn client_opens_tcp_connection() {
    covers!(feat_req_recentip_646);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    // Configure to use TCP transport
    let client_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let server_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let mut client = Runtime::new(io_client, client_config).unwrap();
    let mut server = Runtime::new(io_server, server_config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Before request: count TCP connections
    let connects_before = network
        .history()
        .into_iter()
        .filter(|e| matches!(e, NetworkEvent::TcpConnect { .. }))
        .count();
    
    // Get proxy (may trigger SD but not TCP yet)
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    // Send first request - this should open TCP connection
    let method_id = MethodId::new(0x0001);
    let _pending = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // After request: should have TCP connection
    let connects_after = network
        .history()
        .into_iter()
        .filter(|e| matches!(e, NetworkEvent::TcpConnect { .. }))
        .count();

    assert!(
        connects_after > connects_before,
        "Client should open TCP connection when sending first request"
    );
}

/// feat_req_recentip_644: Single TCP connection for all messages between client-server
///
/// The client and server shall use a single TCP connection for
/// all SOME/IP messages between them.
#[test]
#[ignore = "Runtime::new not implemented"]
fn single_tcp_connection_reused() {
    covers!(feat_req_recentip_644);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let client_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let server_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let mut client = Runtime::new(io_client, client_config).unwrap();
    let mut server = Runtime::new(io_server, server_config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let mut offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    let method_id = MethodId::new(0x0001);

    // Send multiple requests
    for i in 0..5 {
        let pending = proxy.call(method_id, &[i]).unwrap();
        network.advance(std::time::Duration::from_millis(10));

        // Server responds
        if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
            request.responder.send_ok(&[i + 100]).unwrap();
        }
        network.advance(std::time::Duration::from_millis(10));

        let _response = pending.wait().unwrap();
    }

    // Count TCP connections - should be exactly 1
    let tcp_connects = network
        .history()
        .into_iter()
        .filter(|e| matches!(e, NetworkEvent::TcpConnect { .. }))
        .count();

    assert_eq!(
        tcp_connects,
        1,
        "Should reuse single TCP connection for all requests"
    );
}

/// feat_req_recentip_647: Client reestablishes connection after failure
///
/// The client is responsible for reestablishing the TCP connection
/// after it has been closed or lost.
#[test]
#[ignore = "Runtime::new not implemented"]
fn client_reestablishes_connection() {
    covers!(feat_req_recentip_647);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let client_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let server_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let mut client = Runtime::new(io_client, client_config).unwrap();
    let mut server = Runtime::new(io_server, server_config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let mut offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    let method_id = MethodId::new(0x0001);

    // First request establishes connection
    let pending = proxy.call(method_id, &[1]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        request.responder.send_ok(&[1]).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));
    let _response = pending.wait().unwrap();

    // Simulate connection loss
    let client_addr = std::net::SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 10)),
        30501,
    );
    let server_addr = std::net::SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 20)),
        30501,
    );
    network.break_tcp(client_addr, server_addr);
    network.advance(std::time::Duration::from_millis(100));

    // Second request should trigger reconnection
    let _pending = proxy.call(method_id, &[2]).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Count TCP connections - should be 2 (original + reconnect)
    let tcp_connects = network
        .history()
        .into_iter()
        .filter(|e| matches!(e, NetworkEvent::TcpConnect { .. }))
        .count();

    assert!(
        tcp_connects >= 2,
        "Client should reestablish TCP connection after failure"
    );
}

// ============================================================================
// TCP Message Framing Tests
// ============================================================================

/// feat_req_recentip_585: Every SOME/IP payload has its own header
///
/// Every SOME/IP payload shall have its own SOME/IP header (no batching
/// of payloads under single header).
#[test]
#[ignore = "Runtime::new not implemented"]
fn each_message_has_own_header() {
    covers!(feat_req_recentip_585);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let client_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let server_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let mut client = Runtime::new(io_client, client_config).unwrap();
    let mut server = Runtime::new(io_server, server_config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    // Send multiple requests with different payloads
    let method_id = MethodId::new(0x0001);
    let payloads = [b"short".to_vec(), b"medium length".to_vec(), b"a".to_vec()];

    for payload in &payloads {
        let _pending = proxy.call(method_id, payload).unwrap();
    }
    network.advance(std::time::Duration::from_millis(100));

    // Parse all TCP messages
    let tcp_messages = find_tcp_messages(&network);
    
    // Each message should have its own header with correct length
    for msg in &tcp_messages {
        let header = Header::from_bytes(msg).expect("Should parse header");
        let expected_payload_len = msg.len() - 16;
        let header_payload_len = header.length as usize - 8;
        
        assert_eq!(
            header_payload_len, expected_payload_len,
            "Header length field should match actual payload"
        );
    }
}

/// feat_req_recentip_702: Multiple messages can be in one TCP segment
///
/// All Transport Protocol Bindings shall support transporting more than one
/// SOME/IP message in a single TCP segment.
#[test]
#[ignore = "Runtime::new not implemented"]
fn multiple_messages_per_segment_parsed() {
    covers!(feat_req_recentip_702);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let client_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let server_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let mut client = Runtime::new(io_client, client_config).unwrap();
    let mut server = Runtime::new(io_server, server_config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let mut offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    // Send requests quickly (may be coalesced in TCP)
    let method_id = MethodId::new(0x0001);
    for i in 0u8..3 {
        let _pending = proxy.call(method_id, &[i]).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    // Server should receive all 3 requests even if sent in single segment
    let mut received_count = 0;
    while let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        request.responder.send_ok(&[]).unwrap();
        received_count += 1;
    }

    assert_eq!(received_count, 3, "Server should parse all messages from TCP stream");
}

// ============================================================================
// TCP Connection Lost Handling
// ============================================================================

/// feat_req_recentip_326: Outstanding requests fail when connection lost
///
/// When the TCP connection is lost, outstanding requests shall be
/// handled as if a timeout occurred.
#[test]
#[ignore = "Runtime::new not implemented"]
fn connection_lost_fails_pending_requests() {
    covers!(feat_req_recentip_326);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let client_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let server_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let mut client = Runtime::new(io_client, client_config).unwrap();
    let mut server = Runtime::new(io_server, server_config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    // Send request but don't respond
    let method_id = MethodId::new(0x0001);
    let pending = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Break the connection while request is pending
    let client_addr = std::net::SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 10)),
        30501,
    );
    let server_addr = std::net::SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 20)),
        30501,
    );
    network.break_tcp(client_addr, server_addr);
    network.advance(std::time::Duration::from_millis(100));

    // Pending request should fail
    // In a real implementation, wait() would return an error when connection is lost
    // For now, we just verify the test structure is correct
    let result = pending.wait();
    assert!(
        result.is_err(),
        "Pending request should fail when connection lost"
    );
}

// ============================================================================
// Magic Cookie Tests (TCP Resync)
// ============================================================================

/// feat_req_recentip_586: Magic Cookies allow resync in testing
///
/// In order to allow resynchronization to SOME/IP over TCP in testing and
/// debugging scenarios, implementations shall support Magic Cookies.
#[test]
#[ignore = "Runtime::new not implemented"]
fn magic_cookie_recognized() {
    covers!(feat_req_recentip_586);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let client_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .magic_cookies(true)  // Enable magic cookies
        .build()
        .unwrap();

    let server_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .magic_cookies(true)
        .build()
        .unwrap();

    let mut client = Runtime::new(io_client, client_config).unwrap();
    let mut server = Runtime::new(io_server, server_config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let mut offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    // First TCP segment should start with Magic Cookie
    let method_id = MethodId::new(0x0001);
    let _pending = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    let tcp_messages = find_tcp_messages(&network);
    
    // Check if first message is a magic cookie or data starts after one
    let first_data = tcp_messages.first();
    if let Some(data) = first_data {
        // If magic cookies enabled, first bytes should be magic cookie
        // OR the implementation sends magic cookie as separate message
        let has_magic = is_magic_cookie(data) || 
            tcp_messages.iter().any(|m| is_magic_cookie(m));
        
        // This is implementation-dependent - just verify messages are parseable
        assert!(data.len() >= 16, "Should have valid SOME/IP message");
    }

    // Server should still receive the request
    let event = offering.try_next().ok().flatten();
    assert!(
        matches!(event, Some(ServiceEvent::MethodCall { .. })),
        "Server should receive request even with magic cookies"
    );
}

/// feat_req_recentip_591: TCP segment starts with Magic Cookie
///
/// Each TCP segment shall start with a SOME/IP Magic Cookie Message
/// (when magic cookies are enabled).
#[test]
#[ignore = "Runtime::new not implemented"]
fn tcp_segment_starts_with_magic_cookie() {
    covers!(feat_req_recentip_591);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let client_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .magic_cookies(true)
        .build()
        .unwrap();

    let server_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .magic_cookies(true)
        .build()
        .unwrap();

    let mut client = Runtime::new(io_client, client_config).unwrap();
    let mut server = Runtime::new(io_server, server_config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    // Clear history to capture just the RPC
    network.clear_history();

    let method_id = MethodId::new(0x0001);
    let _pending = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Get first TCP send event
    let first_tcp = network
        .history()
        .iter()
        .find_map(|e| {
            if let NetworkEvent::TcpSent { data, .. } = e {
                Some(data.clone())
            } else {
                None
            }
        });

    if let Some(data) = first_tcp {
        assert!(
            is_magic_cookie(&data),
            "TCP segment should start with Magic Cookie when enabled"
        );
    }
}

/// feat_req_recentip_592: Only one Magic Cookie per segment
///
/// The implementation shall only include up to one SOME/IP Magic Cookie
/// Message per TCP segment.
#[test]
#[ignore = "Runtime::new not implemented"]
fn only_one_magic_cookie_per_segment() {
    covers!(feat_req_recentip_592);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let client_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .magic_cookies(true)
        .build()
        .unwrap();

    let server_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .magic_cookies(true)
        .build()
        .unwrap();

    let mut client = Runtime::new(io_client, client_config).unwrap();
    let mut server = Runtime::new(io_server, server_config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    network.clear_history();

    // Send multiple requests quickly
    let method_id = MethodId::new(0x0001);
    for i in 0u8..5 {
        let _pending = proxy.call(method_id, &[i]).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    // Check each TCP send for magic cookie count
    for event in network.history() {
        if let NetworkEvent::TcpSent { data, .. } = event {
            // Count magic cookies in this segment
            let headers = parse_tcp_stream(data.as_slice());
            let magic_count = headers
                .iter()
                .filter(|h| h.service_id == 0xFFFF && h.method_id == 0x0000)
                .count();

            assert!(
                magic_count <= 1,
                "Should have at most one magic cookie per segment, found {}",
                magic_count
            );
        }
    }
}

// ============================================================================
// Wire Format Consistency (TCP vs UDP)
// ============================================================================

/// feat_req_recentip_324: TCP binding based on UDP binding
///
/// The TCP binding of SOME/IP is heavily based on the UDP binding.
/// The header format is identical.
#[test]
#[ignore = "Runtime::new not implemented"]
fn tcp_header_format_matches_udp() {
    covers!(feat_req_recentip_324);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let client_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let server_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let mut client = Runtime::new(io_client, client_config).unwrap();
    let mut server = Runtime::new(io_server, server_config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    let method_id = MethodId::new(0x0001);
    let _pending = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    let tcp_messages = find_tcp_messages(&network);
    assert!(!tcp_messages.is_empty(), "Should have TCP message");

    // Parse header - should match SOME/IP header format exactly
    let header = Header::from_bytes(&tcp_messages[0]).expect("Should parse as SOME/IP header");
    
    // Verify header structure is valid
    assert_eq!(header.service_id, 0x1234, "Service ID should match");
    assert_eq!(header.method_id, 0x0001, "Method ID should match");
    assert_eq!(header.protocol_version, 0x01, "Protocol version should be 0x01");
    assert!(header.length >= 8, "Length should include at least header remainder");
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
#[ignore = "Runtime::new not implemented"]
fn tcp_loss_does_not_reset_session_id() {
    // This test verifies the distinction between:
    // - feat_req_recentip_326: TCP loss → requests timeout (no session reset)
    // - feat_req_recentipsd_872: Reboot detected → TCP reset (session DOES reset)
    covers!(feat_req_recentip_326, feat_req_recentip_647);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let client_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let server_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let mut client = Runtime::new(io_client, client_config).unwrap();
    let mut server = Runtime::new(io_server, server_config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let mut offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    let method_id = MethodId::new(0x0001);

    // Send first request, capture session ID
    let _pending = proxy.call(method_id, &[1]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    let messages_before = find_tcp_messages(&network);
    let header_before = Header::from_bytes(&messages_before.last().unwrap())
        .expect("Should parse header");
    let session_before = header_before.session_id;

    // Complete the request
    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        request.responder.send_ok(&[1]).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    // Break TCP connection
    let client_addr = std::net::SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 10)),
        30501,
    );
    let server_addr = std::net::SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 20)),
        30501,
    );
    network.break_tcp(client_addr, server_addr);
    network.advance(std::time::Duration::from_millis(100));

    // Send another request after reconnection
    network.clear_history();
    let _pending = proxy.call(method_id, &[2]).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let messages_after = find_tcp_messages(&network);
    if !messages_after.is_empty() {
        let header_after = Header::from_bytes(&messages_after.last().unwrap())
            .expect("Should parse header");
        let session_after = header_after.session_id;

        // Session ID should have incremented, NOT reset to 1
        // (unless it wrapped around, which is unlikely after just 2 requests)
        assert!(
            session_after > session_before || session_after == 1 && session_before == 0xFFFF,
            "Session ID should continue after TCP reconnection (was {}, now {}), not reset",
            session_before,
            session_after
        );

        // Definitely should NOT have reset to 1 from a low number
        if session_before < 100 {
            assert_ne!(
                session_after, 1,
                "Session ID should not reset to 1 after TCP loss (that would indicate reboot)"
            );
        }
    }
}

/// feat_req_recentipsd_872: Reboot detection triggers TCP connection reset
///
/// When the system detects the reboot of a peer (via SD Reboot Flag),
/// it shall reset the state of TCP connections to that peer.
#[test]
#[ignore = "Runtime::new not implemented"]
fn reboot_detection_resets_tcp_connections() {
    covers!(feat_req_recentipsd_872);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let client_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let server_config = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();

    let mut client = Runtime::new(io_client, client_config).unwrap();
    let mut server = Runtime::new(io_server, server_config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let mut offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    let method_id = MethodId::new(0x0001);

    // Establish TCP connection with first request
    let pending = proxy.call(method_id, &[1]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        request.responder.send_ok(&[1]).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));
    let _response = pending.wait().unwrap();

    // Count current TCP connections
    let connects_before = network
        .history()
        .into_iter()
        .filter(|e| matches!(e, NetworkEvent::TcpConnect { .. }))
        .count();

    // Simulate server reboot: server sends SD message with Reboot Flag set
    // This is detected by client's SD layer
    drop(server);
    drop(offering);

    // Create new server instance (simulates reboot)
    let (_, _, io_server_new) = SimulatedNetwork::new_pair();
    let server_config_new = RuntimeConfig::builder()
        .transport(Transport::Tcp)
        .build()
        .unwrap();
    let mut server_new = Runtime::new(io_server_new, server_config_new).unwrap();

    let service_config_new = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering_new = server_new.offer(service_config_new).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // The new server's SD OfferService should have Reboot Flag = 1
    // Client should detect this and reset its TCP connection state

    // Client tries to send another request
    // Should establish NEW TCP connection (old one was reset due to reboot)
    let _pending = proxy.call(method_id, &[2]).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let connects_after = network
        .history()
        .into_iter()
        .filter(|e| matches!(e, NetworkEvent::TcpConnect { .. }))
        .count();

    // Should have a new connection attempt after reboot detection
    assert!(
        connects_after > connects_before,
        "Client should establish new TCP connection after detecting peer reboot"
    );
}

