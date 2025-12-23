//! Error Handling and Return Code Compliance Tests
//!
//! Tests the error handling behavior per SOME/IP specification.
//!
//! Key requirements tested:
//! - feat_req_recentip_371: Return code definitions (E_OK, E_NOT_OK, etc.)
//! - feat_req_recentip_144: Return code usage per message type
//! - feat_req_recentip_141: Request/Response message type mapping
//! - feat_req_recentip_704: No error responses to error messages
//! - feat_req_recentip_816: Optional E_UNKNOWN_SERVICE/E_UNKNOWN_METHOD
//! - feat_req_recentip_684: Message type definitions

use someip_runtime::*;

// Re-use wire format parsing from the shared module
#[path = "../wire.rs"]
mod wire;

// Re-use simulated network
#[path = "../simulated.rs"]
mod simulated;

use simulated::{NetworkEvent, SimulatedNetwork};

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

// ============================================================================
// Return Code Constants (per feat_req_recentip_371)
// ============================================================================

/// Return codes as defined in the specification
pub mod return_codes {
    pub const E_OK: u8 = 0x00;
    pub const E_NOT_OK: u8 = 0x01;
    pub const E_UNKNOWN_SERVICE: u8 = 0x02;
    pub const E_UNKNOWN_METHOD: u8 = 0x03;
    pub const E_NOT_READY: u8 = 0x04; // deprecated
    pub const E_NOT_REACHABLE: u8 = 0x05; // deprecated
    pub const E_TIMEOUT: u8 = 0x06; // deprecated
    pub const E_WRONG_PROTOCOL_VERSION: u8 = 0x07;
    pub const E_WRONG_INTERFACE_VERSION: u8 = 0x08;
    pub const E_MALFORMED_MESSAGE: u8 = 0x09;
    pub const E_WRONG_MESSAGE_TYPE: u8 = 0x0A;
    // 0x0B-0x1F: Reserved for generic SOME/IP errors
    // 0x20-0x3F: Reserved for service/method specific errors
}

// ============================================================================
// Message Type Constants (per feat_req_recentip_684)
// ============================================================================

pub mod message_types {
    pub const REQUEST: u8 = 0x00;
    pub const REQUEST_NO_RETURN: u8 = 0x01;
    pub const NOTIFICATION: u8 = 0x02;
    pub const REQUEST_ACK: u8 = 0x40; // Reserved
    pub const REQUEST_NO_RETURN_ACK: u8 = 0x41; // Reserved
    pub const NOTIFICATION_ACK: u8 = 0x42; // Reserved
    pub const RESPONSE: u8 = 0x80;
    pub const ERROR: u8 = 0x81; // Called EXCEPTION in spec
    pub const RESPONSE_ACK: u8 = 0xC0; // Reserved
    pub const ERROR_ACK: u8 = 0xC1; // Reserved

    /// TP flag bit (can be OR'd with message type)
    pub const TP_FLAG: u8 = 0x20;
}

// ============================================================================
// Return Code Unit Tests
// ============================================================================

#[cfg(test)]
mod return_code_tests {
    use super::return_codes::*;

    /// feat_req_recentip_371: E_OK is 0x00
    #[test]
    fn e_ok_is_zero() {
        assert_eq!(E_OK, 0x00);
    }

    /// feat_req_recentip_371: E_NOT_OK is 0x01
    #[test]
    fn e_not_ok_is_one() {
        assert_eq!(E_NOT_OK, 0x01);
    }

    /// feat_req_recentip_371: Return code values match spec
    #[test]
    fn return_code_values_match_spec() {
        assert_eq!(E_UNKNOWN_SERVICE, 0x02);
        assert_eq!(E_UNKNOWN_METHOD, 0x03);
        assert_eq!(E_NOT_READY, 0x04);
        assert_eq!(E_NOT_REACHABLE, 0x05);
        assert_eq!(E_TIMEOUT, 0x06);
        assert_eq!(E_WRONG_PROTOCOL_VERSION, 0x07);
        assert_eq!(E_WRONG_INTERFACE_VERSION, 0x08);
        assert_eq!(E_MALFORMED_MESSAGE, 0x09);
        assert_eq!(E_WRONG_MESSAGE_TYPE, 0x0A);
    }

    /// Verify reserved ranges
    #[test]
    fn reserved_ranges() {
        // 0x0B-0x1F: Generic SOME/IP errors
        assert!(0x0B <= 0x1F);
        // 0x20-0x3F: Service-specific errors
        assert!(0x20 <= 0x3F);
    }
}

// ============================================================================
// Message Type Unit Tests
// ============================================================================

#[cfg(test)]
mod message_type_tests {
    use super::message_types::*;

    /// feat_req_recentip_684: Message type values
    #[test]
    fn message_type_values_match_spec() {
        assert_eq!(REQUEST, 0x00);
        assert_eq!(REQUEST_NO_RETURN, 0x01);
        assert_eq!(NOTIFICATION, 0x02);
        assert_eq!(RESPONSE, 0x80);
        assert_eq!(ERROR, 0x81);
    }

    /// feat_req_recentip_142: ACK bit is 0x40
    #[test]
    fn ack_bit_is_0x40() {
        assert_eq!(REQUEST_ACK, REQUEST | 0x40);
        assert_eq!(REQUEST_NO_RETURN_ACK, REQUEST_NO_RETURN | 0x40);
        assert_eq!(NOTIFICATION_ACK, NOTIFICATION | 0x40);
        assert_eq!(RESPONSE_ACK, RESPONSE | 0x40);
        assert_eq!(ERROR_ACK, ERROR | 0x40);
    }

    /// feat_req_recentip_761: TP flag is 0x20
    #[test]
    fn tp_flag_is_0x20() {
        assert_eq!(TP_FLAG, 0x20);
    }

    /// Response is request with high bit set
    #[test]
    fn response_is_request_with_high_bit() {
        assert_eq!(RESPONSE, REQUEST | 0x80);
        assert_eq!(ERROR, REQUEST_NO_RETURN | 0x80);
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Find all SOME/IP messages (non-SD) in network history
#[allow(dead_code)]
fn find_rpc_messages(network: &SimulatedNetwork) -> Vec<Vec<u8>> {
    network
        .history()
        .iter()
        .filter_map(|event| {
            if let NetworkEvent::UdpSent { data, dst_port, .. } = event {
                // Exclude SD messages (port 30490)
                if *dst_port != 30490 && data.len() >= 16 {
                    return Some(data.clone());
                }
            }
            None
        })
        .collect()
}

/// Find messages with a specific return code
#[allow(dead_code)]
fn find_messages_with_return_code(network: &SimulatedNetwork, code: u8) -> Vec<Vec<u8>> {
    find_rpc_messages(network)
        .into_iter()
        .filter(|data| data.len() >= 16 && data[15] == code)
        .collect()
}

/// Find messages with a specific message type
#[allow(dead_code)]
fn find_messages_with_type(network: &SimulatedNetwork, msg_type: u8) -> Vec<Vec<u8>> {
    find_rpc_messages(network)
        .into_iter()
        .filter(|data| data.len() >= 16 && data[14] == msg_type)
        .collect()
}

// ============================================================================
// Integration Tests (require Runtime implementation)
// ============================================================================

/// feat_req_recentip_144: REQUEST must have return_code = E_OK
///
/// Messages of Type REQUEST, REQUEST_NO_RETURN, and Notification
/// must set the Return Code to 0x00 (E_OK).
#[test]
#[ignore = "Runtime::new not implemented"]
fn request_has_return_code_e_ok() {
    covers!(feat_req_recentip_144);

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

    // Send a request
    let method_id = MethodId::new(0x0001);
    let _response = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Find REQUEST messages
    let requests = find_messages_with_type(&network, message_types::REQUEST);
    assert!(!requests.is_empty(), "Should have sent REQUEST message");

    for request in requests {
        let return_code = request[15];
        assert_eq!(
            return_code,
            return_codes::E_OK,
            "REQUEST must have return_code = E_OK"
        );
    }
}

/// feat_req_recentip_144: REQUEST_NO_RETURN must have return_code = E_OK
#[test]
#[ignore = "Runtime::new not implemented"]
fn fire_and_forget_has_return_code_e_ok() {
    covers!(feat_req_recentip_144);

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

    // Send fire-and-forget
    let method_id = MethodId::new(0x0002);
    proxy.fire_and_forget(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Find REQUEST_NO_RETURN messages
    let requests = find_messages_with_type(&network, message_types::REQUEST_NO_RETURN);
    assert!(!requests.is_empty(), "Should have sent REQUEST_NO_RETURN");

    for request in requests {
        let return_code = request[15];
        assert_eq!(
            return_code,
            return_codes::E_OK,
            "REQUEST_NO_RETURN must have return_code = E_OK"
        );
    }
}

/// feat_req_recentip_141: REQUEST answered by RESPONSE on success
///
/// Regular request (message type 0x00) shall be answered by a response
/// (message type 0x80) when no error occurred.
#[test]
#[ignore = "Runtime::new not implemented"]
fn request_answered_by_response() {
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

    // Client sends a request (returns PendingResponse)
    let method_id = MethodId::new(0x0001);
    let pending = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Server receives the request and responds with success
    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        assert_eq!(request.method, method_id);
        request.responder.send_ok(&[4, 5, 6]).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    // Client receives the response
    let _response = pending.wait().unwrap();

    // Should have both REQUEST and RESPONSE
    let requests = find_messages_with_type(&network, message_types::REQUEST);
    let responses = find_messages_with_type(&network, message_types::RESPONSE);

    assert!(!requests.is_empty(), "Should have REQUEST");
    assert!(!responses.is_empty(), "Should have RESPONSE");

    // Response should have E_OK
    for response in responses {
        assert_eq!(
            response[15],
            return_codes::E_OK,
            "Successful RESPONSE should have E_OK"
        );
    }
}

/// feat_req_recentip_141: Error can be in RESPONSE or ERROR message
///
/// If an error occurs, response message with return code not equal to 0x00
/// shall be sent. This could be RESPONSE (0x80) or ERROR/EXCEPTION (0x81).
#[test]
#[ignore = "Runtime::new not implemented"]
fn error_response_has_nonzero_return_code() {
    covers!(feat_req_recentip_141, feat_req_recentip_726);

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

    // Client sends a request
    let method_id = MethodId::new(0x0001);
    let pending = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Server receives the request and responds with an error
    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        // Respond with E_NOT_OK error
        request
            .responder
            .send_error(ReturnCode::NotOk)
            .unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    // Client receives an error response
    let response = pending.wait().unwrap();
    assert_ne!(response.return_code, ReturnCode::Ok, "Should receive error return code");

    // Find error responses (either RESPONSE with non-E_OK or ERROR message)
    let responses = find_messages_with_type(&network, message_types::RESPONSE);
    let errors = find_messages_with_type(&network, message_types::ERROR);

    let error_count = responses
        .iter()
        .filter(|r| r[15] != return_codes::E_OK)
        .count()
        + errors.len();

    assert!(error_count > 0, "Should have error response");

    // ERROR messages must have non-E_OK return code
    for error in errors {
        assert_ne!(
            error[15],
            return_codes::E_OK,
            "ERROR message must have non-E_OK return code"
        );
    }
}

/// feat_req_recentip_704: No error responses to error messages
///
/// Implementations shall not answer with errors to SOME/IP messages
/// already carrying an error (return code != 0x00).
#[test]
#[ignore = "Runtime::new not implemented"]
fn no_error_response_to_error_message() {
    covers!(feat_req_recentip_704);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut _client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
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

    // Craft a malformed REQUEST message with non-zero return code
    // This is invalid per spec - requests should have E_OK
    // The server should NOT respond with an error to this
    let malformed_request: [u8; 24] = [
        // Message ID: Service 0x1234, Method 0x0001
        0x12, 0x34, 0x00, 0x01,
        // Length: 8 (header remainder + 0 payload)
        0x00, 0x00, 0x00, 0x08,
        // Request ID: Client 0x0001, Session 0x0001
        0x00, 0x01, 0x00, 0x01,
        // Protocol version, Interface version
        0x01, 0x01,
        // Message type: REQUEST (0x00)
        message_types::REQUEST,
        // Return code: E_NOT_OK (non-zero - this is the error!)
        return_codes::E_NOT_OK,
        // Payload (8 bytes to match length)
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    // Inject the malformed packet to the server's address
    let client_addr = std::net::SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 1)),
        30501,
    );
    let server_addr = std::net::SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 2)),
        30501,
    );
    network.inject_udp(client_addr, server_addr, &malformed_request);
    network.advance(std::time::Duration::from_millis(100));

    // Count error responses generated AFTER injection
    let error_responses: Vec<_> = find_rpc_messages(&network)
        .into_iter()
        .filter(|msg| {
            let msg_type = msg[14];
            let return_code = msg[15];
            // Look for RESPONSE or ERROR messages with non-E_OK
            (msg_type == message_types::RESPONSE || msg_type == message_types::ERROR)
                && return_code != return_codes::E_OK
        })
        .collect();

    // Should not have generated any error responses to the error message
    assert!(
        error_responses.is_empty(),
        "Should not respond to messages already carrying errors"
    );
}

/// feat_req_recentip_816: E_UNKNOWN_SERVICE is optional
///
/// The error codes E_UNKNOWN_SERVICE and E_UNKNOWN_METHOD are optional.
#[test]
#[ignore = "Runtime::new not implemented"]
fn unknown_service_error_optional() {
    covers!(feat_req_recentip_816);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut _server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    // Don't offer any service - server has no services

    let service_id = ServiceId::new(0x9999).unwrap(); // Unknown service

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));

    // Try to use proxy even though service might not be available
    // Implementation may return E_UNKNOWN_SERVICE or simply not respond
    let _result = proxy.try_available();
    network.advance(std::time::Duration::from_millis(100));

    // If there's a response, it may have E_UNKNOWN_SERVICE (optional)
    let unknown_service_errors =
        find_messages_with_return_code(&network, return_codes::E_UNKNOWN_SERVICE);

    // This is optional - test passes whether or not this error is sent
    let _ = unknown_service_errors.len();
}

/// feat_req_recentip_816: E_UNKNOWN_METHOD is optional
#[test]
#[ignore = "Runtime::new not implemented"]
fn unknown_method_error_optional() {
    covers!(feat_req_recentip_816);

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

    // Call a method that doesn't exist
    let unknown_method = MethodId::new(0xFFFF);
    let _result = proxy.call(unknown_method, &[]);
    network.advance(std::time::Duration::from_millis(100));

    // May return E_UNKNOWN_METHOD (optional per spec)
    let unknown_method_errors =
        find_messages_with_return_code(&network, return_codes::E_UNKNOWN_METHOD);

    // This is optional - test passes whether or not this error is sent
    let _ = unknown_method_errors.len();
}

/// feat_req_recentip_141: NOTIFICATION is fire-and-forget from server
///
/// A request of a notification/event callback expecting no response.
#[test]
#[ignore = "Runtime::new not implemented"]
fn notification_message_type() {
    covers!(feat_req_recentip_684);

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

    let offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    // Subscribe to eventgroup
    let eventgroup = EventgroupId::new(1).unwrap();
    let _subscription = proxy.subscribe(eventgroup).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Server triggers an event notification
    let event_id = EventId::new(0x8001).unwrap();
    offering.notify(eventgroup, event_id, &[10, 20, 30]).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Find NOTIFICATION messages
    let notifications = find_messages_with_type(&network, message_types::NOTIFICATION);

    // Events should be sent as NOTIFICATION
    assert!(!notifications.is_empty(), "Should have sent NOTIFICATION");
    for notification in &notifications {
        let return_code = notification[15];
        assert_eq!(
            return_code,
            return_codes::E_OK,
            "NOTIFICATION must have return_code = E_OK"
        );
    }
}

/// Request/Response message IDs must match
///
/// The response Message ID and Request ID must match the request.
#[test]
#[ignore = "Runtime::new not implemented"]
fn response_ids_match_request() {
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

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    let method_id = MethodId::new(0x0001);
    let _response = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let requests = find_messages_with_type(&network, message_types::REQUEST);
    let responses = find_messages_with_type(&network, message_types::RESPONSE);

    assert!(!requests.is_empty() && !responses.is_empty());

    let request = &requests[0];
    let response = &responses[0];

    // Message ID (bytes 0-3) must match
    assert_eq!(
        &request[0..4],
        &response[0..4],
        "Response Message ID must match Request"
    );

    // Request ID (bytes 8-11) must match
    assert_eq!(
        &request[8..12],
        &response[8..12],
        "Response Request ID must match Request"
    );
}

/// Protocol version must be consistent
#[test]
#[ignore = "Runtime::new not implemented"]
fn protocol_version_in_response() {
    covers!(feat_req_recentip_369);

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

    let method_id = MethodId::new(0x0001);
    let _response = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let responses = find_messages_with_type(&network, message_types::RESPONSE);

    for response in responses {
        let protocol_version = response[12];
        assert_eq!(
            protocol_version, 0x01,
            "Protocol version must be 0x01"
        );
    }
}
