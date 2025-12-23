//! Request/Response Communication Flow Compliance Tests
//!
//! Tests the RPC request/response patterns per SOME/IP specification.
//!
//! Key requirements tested:
//! - feat_req_recentip_328: Request/Response communication pattern
//! - feat_req_recentip_329: Request triggers response from server
//! - feat_req_recentip_338: Response contains same Request ID as request
//! - feat_req_recentip_345: Fire&Forget (REQUEST_NO_RETURN)
//! - feat_req_recentip_348: Fire&Forget shall not return error
//! - feat_req_recentip_141: Request (0x00) answered by Response (0x80)

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

/// Find all RPC messages (non-SD) in network history
fn find_rpc_messages(network: &SimulatedNetwork) -> Vec<(Header, Vec<u8>)> {
    network
        .history()
        .iter()
        .filter_map(|event| {
            if let NetworkEvent::UdpSent { data, to, .. } = event {
                // Exclude SD port
                if to.port() != 30490 && data.len() >= 16 {
                    if let Some(header) = Header::from_bytes(data) {
                        return Some((header, data.clone()));
                    }
                }
            }
            None
        })
        .collect()
}

/// Find messages by type
fn find_by_message_type(network: &SimulatedNetwork, msg_type: u8) -> Vec<Header> {
    find_rpc_messages(network)
        .into_iter()
        .filter(|(h, _)| h.message_type == msg_type)
        .map(|(h, _)| h)
        .collect()
}

// ============================================================================
// Request/Response Pattern Tests
// ============================================================================

/// feat_req_recentip_328: Request/Response communication pattern
///
/// One of the most common communication patterns is the request/response pattern.
/// One communication partner sends a request to another and gets a response.
#[test]
#[ignore = "Runtime::new not implemented"]
fn request_response_pattern_works() {
    covers!(feat_req_recentip_328, feat_req_recentip_329);

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
    let proxy = proxy.wait_available().unwrap();

    // Client sends request
    let method_id = MethodId::new(0x0001);
    let pending = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Server receives request
    let event = offering.try_next().ok().flatten();
    assert!(
        matches!(event, Some(ServiceEvent::MethodCall { .. })),
        "Server should receive method call"
    );

    // Server sends response
    if let Some(ServiceEvent::MethodCall { request }) = event {
        request.responder.send_ok(&[4, 5, 6]).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    // Client receives response
    let response = pending.wait().unwrap();
    assert_eq!(response.payload, &[4, 5, 6]);
}

/// feat_req_recentip_338: Response contains same Request ID
///
/// The Response shall contain the same Request ID as the Request.
#[test]
#[ignore = "Runtime::new not implemented"]
fn response_has_matching_request_id() {
    covers!(feat_req_recentip_338, feat_req_recentip_83);

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
    let proxy = proxy.wait_available().unwrap();

    // Clear history to capture just the RPC
    network.clear_history();

    // Client sends request
    let method_id = MethodId::new(0x0001);
    let pending = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Capture request's Request ID
    let requests = find_by_message_type(&network, message_type::REQUEST);
    assert!(!requests.is_empty(), "Should have request message");
    let request_header = &requests[0];
    let request_client_id = request_header.client_id;
    let request_session_id = request_header.session_id;

    // Server responds
    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        request.responder.send_ok(&[4, 5, 6]).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    // Capture response's Request ID
    let responses = find_by_message_type(&network, message_type::RESPONSE);
    assert!(!responses.is_empty(), "Should have response message");
    let response_header = &responses[0];

    // Request ID must match
    assert_eq!(
        response_header.client_id, request_client_id,
        "Response Client ID must match request"
    );
    assert_eq!(
        response_header.session_id, request_session_id,
        "Response Session ID must match request"
    );
}

/// feat_req_recentip_141: Request (0x00) answered by Response (0x80)
///
/// Regular request (message type 0x00) shall be answered by a response
/// (message type 0x80) or error (message type 0x81).
#[test]
#[ignore = "Runtime::new not implemented"]
fn request_message_type_answered_by_response() {
    covers!(feat_req_recentip_141, feat_req_recentip_684);

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
    let proxy = proxy.wait_available().unwrap();

    network.clear_history();

    // Send request
    let method_id = MethodId::new(0x0001);
    let pending = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Verify request message type is 0x00
    let requests = find_by_message_type(&network, message_type::REQUEST);
    assert!(!requests.is_empty(), "Should have REQUEST (0x00) message");
    assert_eq!(requests[0].message_type, 0x00);

    // Server responds with success
    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        request.responder.send_ok(&[]).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    // Verify response message type is 0x80
    let responses = find_by_message_type(&network, message_type::RESPONSE);
    assert!(!responses.is_empty(), "Should have RESPONSE (0x80) message");
    assert_eq!(responses[0].message_type, 0x80);

    // Client receives response
    let _result = pending.wait();
}

// ============================================================================
// Fire & Forget (REQUEST_NO_RETURN) Tests
// ============================================================================

/// feat_req_recentip_345: Fire&Forget uses REQUEST_NO_RETURN (0x01)
///
/// Fire&Forget messages use Message Type REQUEST_NO_RETURN (0x01).
#[test]
#[ignore = "Runtime::new not implemented"]
fn fire_and_forget_uses_request_no_return() {
    covers!(feat_req_recentip_345);

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

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    network.clear_history();

    // Send fire-and-forget request
    let method_id = MethodId::new(0x0001);
    proxy.fire_and_forget(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Verify message type is REQUEST_NO_RETURN (0x01)
    let messages = find_by_message_type(&network, message_type::REQUEST_NO_RETURN);
    assert!(
        !messages.is_empty(),
        "Fire&Forget should use REQUEST_NO_RETURN (0x01)"
    );
    assert_eq!(messages[0].message_type, 0x01);
}

/// feat_req_recentip_348: Fire&Forget shall not return error
///
/// Fire&Forget messages shall not return an error. Error handling shall
/// be implemented in the application if needed.
#[test]
#[ignore = "Runtime::new not implemented"]
fn fire_and_forget_no_response() {
    covers!(feat_req_recentip_348);

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
    let proxy = proxy.wait_available().unwrap();

    network.clear_history();

    // Send fire-and-forget request
    let method_id = MethodId::new(0x0001);
    proxy.fire_and_forget(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Server receives but should NOT send response
    let _event = offering.try_next();
    network.advance(std::time::Duration::from_millis(100));

    // No response or error should be sent
    let responses = find_by_message_type(&network, message_type::RESPONSE);
    let errors = find_by_message_type(&network, message_type::ERROR);

    assert!(
        responses.is_empty(),
        "Fire&Forget should not have RESPONSE"
    );
    assert!(
        errors.is_empty(),
        "Fire&Forget should not have ERROR"
    );
}

// ============================================================================
// Error Response Tests
// ============================================================================

/// feat_req_recentip_141: Request can be answered by Error (0x81)
///
/// Regular request can be answered by error (message type 0x81) instead
/// of response (0x80) when an error occurs.
#[test]
#[ignore = "Runtime::new not implemented"]
fn request_can_receive_error_response() {
    covers!(feat_req_recentip_141);

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
    let proxy = proxy.wait_available().unwrap();

    network.clear_history();

    // Send request
    let method_id = MethodId::new(0x0001);
    let pending = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Server responds with error
    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        request.responder.send_error(ReturnCode::NotOk).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    // Verify error message type is 0x81
    let errors = find_by_message_type(&network, message_type::ERROR);
    assert!(!errors.is_empty(), "Should have ERROR (0x81) message");
    assert_eq!(errors[0].message_type, 0x81);

    // Client should receive error
    let result = pending.wait();
    assert!(result.is_err(), "Client should receive error result");
}

// ============================================================================
// Multiple Concurrent Requests Tests
// ============================================================================

/// Multiple concurrent requests are correctly matched with responses
#[test]
#[ignore = "Runtime::new not implemented"]
fn concurrent_requests_correctly_matched() {
    covers!(feat_req_recentip_338, feat_req_recentip_88);

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
    let proxy = proxy.wait_available().unwrap();

    // Send multiple requests
    let method_id = MethodId::new(0x0001);
    let pending1 = proxy.call(method_id, &[1]).unwrap();
    let pending2 = proxy.call(method_id, &[2]).unwrap();
    let pending3 = proxy.call(method_id, &[3]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Server responds in different order
    let mut requests = Vec::new();
    while let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        requests.push(request);
    }

    // Respond in reverse order
    for request in requests.into_iter().rev() {
        let value = request.payload[0];
        request.responder.send_ok(&[value + 100]).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    // Each pending should get its correct response
    let r1 = pending1.wait().unwrap();
    let r2 = pending2.wait().unwrap();
    let r3 = pending3.wait().unwrap();

    assert_eq!(r1.payload, &[101], "Request 1 should get response 101");
    assert_eq!(r2.payload, &[102], "Request 2 should get response 102");
    assert_eq!(r3.payload, &[103], "Request 3 should get response 103");
}

// ============================================================================
// Session ID Behavior in RPC
// ============================================================================

/// Session ID increments for each request
#[test]
#[ignore = "Runtime::new not implemented"]
fn session_id_increments_per_request() {
    covers!(feat_req_recentip_88, feat_req_recentip_649);

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
    let proxy = proxy.wait_available().unwrap();

    network.clear_history();

    // Send multiple requests
    let method_id = MethodId::new(0x0001);
    for i in 0..5 {
        let pending = proxy.call(method_id, &[i]).unwrap();
        network.advance(std::time::Duration::from_millis(10));

        if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
            request.responder.send_ok(&[]).unwrap();
        }
        network.advance(std::time::Duration::from_millis(10));
        let _result = pending.wait();
    }

    // Check session IDs are incrementing
    let requests = find_by_message_type(&network, message_type::REQUEST);
    assert_eq!(requests.len(), 5);

    let session_ids: Vec<u16> = requests.iter().map(|h| h.session_id).collect();
    for i in 1..session_ids.len() {
        assert!(
            session_ids[i] > session_ids[i - 1]
                || (session_ids[i] == 1 && session_ids[i - 1] == 0xFFFF),
            "Session IDs should increment (or wrap)"
        );
    }
}
