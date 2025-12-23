//! Advanced Error Handling Compliance Tests
//!
//! Tests error scenarios, return codes, and error message handling.
//!
//! Key requirements tested:
//! - feat_req_recentip_597: No error response for events/notifications
//! - feat_req_recentip_654: No error response for fire&forget methods
//! - feat_req_recentip_655: Error message copies request header fields
//! - feat_req_recentip_727: Error message has return code != 0x00
//! - feat_req_recentip_798: Messages with length < 8 shall be ignored
//! - feat_req_recentip_721: Message validation order
//! - feat_req_recentip_703: Use known protocol version

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
// Message Type Constants
// ============================================================================

mod message_type {
    pub const REQUEST: u8 = 0x00;
    pub const REQUEST_NO_RETURN: u8 = 0x01;
    pub const NOTIFICATION: u8 = 0x02;
    pub const RESPONSE: u8 = 0x80;
    pub const ERROR: u8 = 0x81;
}

// ============================================================================
// Helper Functions
// ============================================================================

fn find_messages_by_type(network: &SimulatedNetwork, msg_type: u8) -> Vec<(Header, Vec<u8>)> {
    network
        .history()
        .into_iter()
        .filter_map(|event| {
            if let NetworkEvent::UdpSent { data, to, .. } = event {
                if to.port() != 30490 && data.len() >= 16 {
                    if let Some(header) = Header::from_bytes(&data) {
                        if header.message_type == msg_type {
                            return Some((header, data));
                        }
                    }
                }
            }
            None
        })
        .collect()
}

// ============================================================================
// No Error Response for Events/Fire&Forget Tests
// ============================================================================

/// feat_req_recentip_597: No error response for events/notifications
///
/// The system shall not return an error message for events/notifications.
#[test]
#[ignore = "Runtime::new not implemented"]
fn no_error_response_for_events() {
    covers!(feat_req_recentip_597);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let eventgroup_id = EventgroupId::new(0x01).unwrap();
    let event_id = EventId::new(0x8001).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .eventgroup(eventgroup_id)
        .build()
        .unwrap();

    let mut offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let available = proxy.wait_available().unwrap();

    let _subscription = available.subscribe(eventgroup_id).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    network.clear_history();

    // Server sends event notification
    offering.notify(eventgroup_id, event_id, b"event_data").unwrap();
    network.advance(std::time::Duration::from_millis(50));

    // Even if client has an issue processing, no ERROR response should be sent
    let error_messages = find_messages_by_type(&network, message_type::ERROR);
    assert!(
        error_messages.is_empty(),
        "No ERROR response should be sent for events"
    );
}

/// feat_req_recentip_654: No error response for fire&forget methods
///
/// The system shall not return an error message for fire&forget methods.
#[test]
#[ignore = "Runtime::new not implemented"]
fn no_error_response_for_fire_and_forget() {
    covers!(feat_req_recentip_654);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let ff_method = MethodId::new(0x0010);

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let available = proxy.wait_available().unwrap();

    network.clear_history();

    // Client sends fire&forget
    available.fire_and_forget(ff_method, b"ff_payload").unwrap();
    network.advance(std::time::Duration::from_millis(50));

    // Server should NOT send any response (error or otherwise)
    let responses = find_messages_by_type(&network, message_type::RESPONSE);
    let errors = find_messages_by_type(&network, message_type::ERROR);

    assert!(responses.is_empty(), "No RESPONSE for fire&forget");
    assert!(errors.is_empty(), "No ERROR for fire&forget");
}

// ============================================================================
// Error Message Format Tests
// ============================================================================

/// feat_req_recentip_655: Error response copies request header fields
///
/// For request/response methods the error message shall copy over the
/// fields of the header from the request.
#[test]
#[ignore = "Runtime::new not implemented"]
fn error_response_copies_request_header() {
    covers!(feat_req_recentip_655);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let method_id = MethodId::new(0x0001);

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let mut offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let available = proxy.wait_available().unwrap();

    network.clear_history();

    // Send request
    let _pending = available.call(method_id, b"request_data").unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Capture request header
    let requests = find_messages_by_type(&network, message_type::REQUEST);
    assert!(!requests.is_empty());
    let (request_header, _) = &requests[0];

    network.clear_history();

    // Server responds with error
    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        request.responder.send_error(ReturnCode::UnknownMethod).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    // Check error response copies header fields
    let errors = find_messages_by_type(&network, message_type::ERROR);
    assert!(!errors.is_empty(), "Should receive ERROR response");

    let (error_header, _) = &errors[0];

    // Service ID and Method ID should match
    assert_eq!(
        error_header.service_id, request_header.service_id,
        "Error should copy Service ID from request"
    );
    assert_eq!(
        error_header.method_id, request_header.method_id,
        "Error should copy Method ID from request"
    );
    // Client ID and Session ID should match (Request ID)
    assert_eq!(
        error_header.client_id, request_header.client_id,
        "Error should copy Client ID from request"
    );
    assert_eq!(
        error_header.session_id, request_header.session_id,
        "Error should copy Session ID from request"
    );
}

/// feat_req_recentip_727: Error message has return code != 0x00
#[test]
#[ignore = "Runtime::new not implemented"]
fn error_message_has_nonzero_return_code() {
    covers!(feat_req_recentip_727);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let method_id = MethodId::new(0x0001);

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let mut offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let available = proxy.wait_available().unwrap();

    let _pending = available.call(method_id, b"data").unwrap();
    network.advance(std::time::Duration::from_millis(10));

    network.clear_history();

    // Server sends error
    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        request.responder.send_error(ReturnCode::NotOk).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    let errors = find_messages_by_type(&network, message_type::ERROR);
    assert!(!errors.is_empty());

    let (error_header, _) = &errors[0];
    assert_ne!(
        error_header.return_code, 0x00,
        "Error message must have return code != 0x00"
    );
}

// ============================================================================
// Message Validation Tests
// ============================================================================

/// feat_req_recentip_798: Messages with length < 8 shall be ignored
///
/// SOME/IP messages with a length value < 8 bytes shall be ignored.
#[test]
#[ignore = "Runtime::new not implemented"]
fn messages_with_short_length_ignored() {
    covers!(feat_req_recentip_798);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

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
    let _available = proxy.wait_available().unwrap();

    // Inject malformed message with Length < 8
    // Normal header is 16 bytes, Length field indicates payload + 8
    // If Length < 8, message is malformed
    let mut malformed = vec![0u8; 16];
    malformed[0..2].copy_from_slice(&0x1234u16.to_be_bytes()); // Service ID
    malformed[2..4].copy_from_slice(&0x0001u16.to_be_bytes()); // Method ID
    malformed[4..8].copy_from_slice(&0x00000004u32.to_be_bytes()); // Length = 4 (< 8!)
    // Rest is garbage

    // Inject from external address to server's service port
    let external_addr: std::net::SocketAddr = "192.168.1.100:50000".parse().unwrap();
    let server_addr: std::net::SocketAddr = "192.168.1.20:30501".parse().unwrap();
    network.inject_udp(external_addr, server_addr, &malformed);
    network.advance(std::time::Duration::from_millis(10));

    // Server should not process this as a valid request
    let event = offering.try_next().ok().flatten();
    assert!(
        event.is_none(),
        "Malformed message with Length < 8 should be ignored"
    );
}

/// feat_req_recentip_703: Implementation shall not use unknown protocol version
#[test]
#[ignore = "Runtime::new not implemented"]
fn uses_known_protocol_version() {
    covers!(feat_req_recentip_703);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let method_id = MethodId::new(0x0001);

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let available = proxy.wait_available().unwrap();

    network.clear_history();

    // Send request
    let _pending = available.call(method_id, b"data").unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // All messages should use protocol version 0x01
    let messages: Vec<_> = network
        .history()
        .into_iter()
        .filter_map(|e| {
            if let NetworkEvent::UdpSent { data, to, .. } = e {
                if to.port() != 30490 && data.len() >= 16 {
                    return Header::from_bytes(&data);
                }
            }
            None
        })
        .collect();

    for header in &messages {
        assert_eq!(
            header.protocol_version, 0x01,
            "Should use protocol version 0x01"
        );
    }
}

// ============================================================================
// Return Code Tests
// ============================================================================

/// All defined return codes are valid
#[test]
#[ignore = "Runtime::new not implemented"]
fn all_return_codes_are_valid() {
    covers!(feat_req_recentip_683);

    // Test that all defined return codes can be used
    let codes = [
        ReturnCode::Ok,
        ReturnCode::NotOk,
        ReturnCode::UnknownService,
        ReturnCode::UnknownMethod,
        ReturnCode::NotReady,
        ReturnCode::NotReachable,
        ReturnCode::Timeout,
        ReturnCode::WrongProtocolVersion,
        ReturnCode::WrongInterfaceVersion,
        ReturnCode::MalformedMessage,
        ReturnCode::WrongMessageType,
    ];

    for code in codes {
        // Each code should have a distinct value
        let value = code as u8;
        assert!(value <= 0x0A, "Return code {:?} should be in valid range", code);
    }

    // Verify E_OK is 0x00
    assert_eq!(ReturnCode::Ok as u8, 0x00);
}

/// Server can return any valid return code
#[test]
#[ignore = "Runtime::new not implemented"]
fn server_returns_various_error_codes() {
    covers!(feat_req_recentip_683, feat_req_recentip_649);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let method_id = MethodId::new(0x0001);

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let mut offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let available = proxy.wait_available().unwrap();

    // Test returning E_NOT_READY
    let pending = available.call(method_id, b"data").unwrap();
    network.advance(std::time::Duration::from_millis(10));

    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        request.responder.send_error(ReturnCode::NotReady).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    let response = pending.wait().unwrap();
    assert_eq!(response.return_code, ReturnCode::NotReady);
}

// ============================================================================
// Protocol Version Mismatch Tests
// ============================================================================

/// Receiving wrong protocol version returns E_WRONG_PROTOCOL_VERSION
#[test]
#[ignore = "Runtime::new not implemented"]
fn wrong_protocol_version_returns_error() {
    covers!(feat_req_recentip_703, feat_req_recentip_649);

    let (network, _io_client, io_server) = SimulatedNetwork::new_pair();

    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Inject request with wrong protocol version
    let mut bad_request = vec![0u8; 24]; // 16 header + 8 payload
    bad_request[0..2].copy_from_slice(&0x1234u16.to_be_bytes()); // Service ID
    bad_request[2..4].copy_from_slice(&0x0001u16.to_be_bytes()); // Method ID
    bad_request[4..8].copy_from_slice(&0x00000010u32.to_be_bytes()); // Length = 16
    bad_request[8..10].copy_from_slice(&0x0001u16.to_be_bytes()); // Client ID
    bad_request[10..12].copy_from_slice(&0x0001u16.to_be_bytes()); // Session ID
    bad_request[12] = 0x99; // Wrong Protocol Version (should be 0x01)
    bad_request[13] = 0x01; // Interface Version
    bad_request[14] = 0x00; // Message Type = REQUEST
    bad_request[15] = 0x00; // Return Code

    network.clear_history();
    
    // Inject from external address to server's service port
    let external_addr: std::net::SocketAddr = "192.168.1.100:50000".parse().unwrap();
    let server_addr: std::net::SocketAddr = "192.168.1.20:30501".parse().unwrap();
    network.inject_udp(external_addr, server_addr, &bad_request);
    network.advance(std::time::Duration::from_millis(10));

    // Server should respond with E_WRONG_PROTOCOL_VERSION or ignore
    let errors = find_messages_by_type(&network, message_type::ERROR);
    if !errors.is_empty() {
        let (header, _) = &errors[0];
        assert_eq!(
            header.return_code,
            ReturnCode::WrongProtocolVersion as u8,
            "Should return E_WRONG_PROTOCOL_VERSION"
        );
    }
    // Note: Ignoring is also valid per feat_req_recentip_818
}
