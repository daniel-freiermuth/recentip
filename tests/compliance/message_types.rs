//! Message Type Compliance Tests
//!
//! Tests for SOME/IP message type field (byte 14) and valid type transitions.
//!
//! # Message Types (feat_req_recentip_103)
//! - 0x00: REQUEST - Client sends request expecting response
//! - 0x01: REQUEST_NO_RETURN - Fire-and-forget request
//! - 0x02: NOTIFICATION - Server sends event notification
//! - 0x80: RESPONSE - Response to a REQUEST
//! - 0x81: ERROR - Error response to a REQUEST
//!
//! # Valid Transitions (feat_req_recentip_282)
//! - REQUEST (0x00) → RESPONSE (0x80) or ERROR (0x81)
//! - REQUEST_NO_RETURN (0x01) → no response allowed
//! - NOTIFICATION (0x02) → no response allowed

#![allow(dead_code)]

// ============================================================================
// Message Type Constants
// ============================================================================

/// Message type byte values per spec
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    /// Request expecting response
    Request = 0x00,
    /// Fire-and-forget request
    RequestNoReturn = 0x01,
    /// Event notification from server
    Notification = 0x02,
    /// Response to request
    Response = 0x80,
    /// Error response
    Error = 0x81,
    /// TP-flagged request
    TpRequest = 0x20,
    /// TP-flagged request no return
    TpRequestNoReturn = 0x21,
    /// TP-flagged notification
    TpNotification = 0x22,
    /// TP-flagged response
    TpResponse = 0xA0,
    /// TP-flagged error
    TpError = 0xA1,
}

impl From<MessageType> for u8 {
    fn from(mt: MessageType) -> u8 {
        mt as u8
    }
}

impl TryFrom<u8> for MessageType {
    type Error = ();
    
    fn try_from(value: u8) -> std::result::Result<Self, ()> {
        match value {
            0x00 => Ok(MessageType::Request),
            0x01 => Ok(MessageType::RequestNoReturn),
            0x02 => Ok(MessageType::Notification),
            0x80 => Ok(MessageType::Response),
            0x81 => Ok(MessageType::Error),
            0x20 => Ok(MessageType::TpRequest),
            0x21 => Ok(MessageType::TpRequestNoReturn),
            0x22 => Ok(MessageType::TpNotification),
            0xA0 => Ok(MessageType::TpResponse),
            0xA1 => Ok(MessageType::TpError),
            _ => Err(()),
        }
    }
}

impl MessageType {
    /// Check if this is a TP-flagged message type
    pub fn is_tp(&self) -> bool {
        (*self as u8) & 0x20 != 0
    }
    
    /// Check if this is a request type (expects response)
    pub fn expects_response(&self) -> bool {
        matches!(self, MessageType::Request | MessageType::TpRequest)
    }
    
    /// Check if this is a fire-and-forget request
    pub fn is_fire_and_forget(&self) -> bool {
        matches!(self, MessageType::RequestNoReturn | MessageType::TpRequestNoReturn)
    }
    
    /// Check if this is a notification
    pub fn is_notification(&self) -> bool {
        matches!(self, MessageType::Notification | MessageType::TpNotification)
    }
    
    /// Check if this is a response type
    pub fn is_response(&self) -> bool {
        matches!(self, MessageType::Response | MessageType::TpResponse | MessageType::Error | MessageType::TpError)
    }
    
    /// Get the corresponding TP-flagged type
    pub fn with_tp_flag(&self) -> Option<MessageType> {
        match self {
            MessageType::Request => Some(MessageType::TpRequest),
            MessageType::RequestNoReturn => Some(MessageType::TpRequestNoReturn),
            MessageType::Notification => Some(MessageType::TpNotification),
            MessageType::Response => Some(MessageType::TpResponse),
            MessageType::Error => Some(MessageType::TpError),
            _ => None, // Already TP-flagged
        }
    }
    
    /// Get the base type without TP flag
    pub fn without_tp_flag(&self) -> MessageType {
        match self {
            MessageType::TpRequest => MessageType::Request,
            MessageType::TpRequestNoReturn => MessageType::RequestNoReturn,
            MessageType::TpNotification => MessageType::Notification,
            MessageType::TpResponse => MessageType::Response,
            MessageType::TpError => MessageType::Error,
            other => *other,
        }
    }
    
    /// Get the expected response type for this message type
    pub fn expected_response_type(&self) -> Option<MessageType> {
        match self {
            MessageType::Request => Some(MessageType::Response),
            MessageType::TpRequest => Some(MessageType::TpResponse),
            _ => None, // Other types don't expect responses
        }
    }
    
    /// Check if this type is valid as a response to the given request type
    pub fn is_valid_response_to(&self, request_type: MessageType) -> bool {
        match request_type {
            MessageType::Request => {
                matches!(self, MessageType::Response | MessageType::Error)
            }
            MessageType::TpRequest => {
                matches!(self, MessageType::TpResponse | MessageType::TpError)
            }
            _ => false, // Other types don't expect responses
        }
    }
}

// ============================================================================
// Unit Tests - MessageType Parsing
// ============================================================================

/// [feat_req_recentip_103] All valid message types can be parsed
#[test]
fn message_type_parse_all_valid_values() {
    // Non-TP types
    assert_eq!(MessageType::try_from(0x00).unwrap(), MessageType::Request);
    assert_eq!(MessageType::try_from(0x01).unwrap(), MessageType::RequestNoReturn);
    assert_eq!(MessageType::try_from(0x02).unwrap(), MessageType::Notification);
    assert_eq!(MessageType::try_from(0x80).unwrap(), MessageType::Response);
    assert_eq!(MessageType::try_from(0x81).unwrap(), MessageType::Error);
    
    // TP-flagged types
    assert_eq!(MessageType::try_from(0x20).unwrap(), MessageType::TpRequest);
    assert_eq!(MessageType::try_from(0x21).unwrap(), MessageType::TpRequestNoReturn);
    assert_eq!(MessageType::try_from(0x22).unwrap(), MessageType::TpNotification);
    assert_eq!(MessageType::try_from(0xA0).unwrap(), MessageType::TpResponse);
    assert_eq!(MessageType::try_from(0xA1).unwrap(), MessageType::TpError);
}

/// [feat_req_recentip_103] Invalid message type values are rejected
#[test]
fn message_type_rejects_invalid_values() {
    // Invalid values should fail to parse
    assert!(MessageType::try_from(0x03).is_err());
    assert!(MessageType::try_from(0x23).is_err());
    assert!(MessageType::try_from(0x82).is_err());
    assert!(MessageType::try_from(0xFF).is_err());
}

/// [feat_req_recentip_103] TP flag bit is 0x20
#[test]
fn message_type_tp_flag_is_bit_5() {
    assert!(!MessageType::Request.is_tp());
    assert!(!MessageType::RequestNoReturn.is_tp());
    assert!(!MessageType::Notification.is_tp());
    assert!(!MessageType::Response.is_tp());
    assert!(!MessageType::Error.is_tp());
    
    assert!(MessageType::TpRequest.is_tp());
    assert!(MessageType::TpRequestNoReturn.is_tp());
    assert!(MessageType::TpNotification.is_tp());
    assert!(MessageType::TpResponse.is_tp());
    assert!(MessageType::TpError.is_tp());
}

/// [feat_req_recentip_282] REQUEST expects RESPONSE or ERROR
#[test]
fn message_type_request_expects_response() {
    assert!(MessageType::Request.expects_response());
    assert!(MessageType::TpRequest.expects_response());
    
    // These don't expect responses
    assert!(!MessageType::RequestNoReturn.expects_response());
    assert!(!MessageType::Notification.expects_response());
    assert!(!MessageType::Response.expects_response());
}

/// [feat_req_recentip_284] REQUEST_NO_RETURN is fire-and-forget
#[test]
fn message_type_request_no_return_is_fire_and_forget() {
    assert!(MessageType::RequestNoReturn.is_fire_and_forget());
    assert!(MessageType::TpRequestNoReturn.is_fire_and_forget());
    
    // Regular request is NOT fire-and-forget
    assert!(!MessageType::Request.is_fire_and_forget());
}

/// [feat_req_recentip_285] NOTIFICATION is server-to-client event
#[test]
fn message_type_notification_classification() {
    assert!(MessageType::Notification.is_notification());
    assert!(MessageType::TpNotification.is_notification());
    
    assert!(!MessageType::Request.is_notification());
    assert!(!MessageType::Response.is_notification());
}

/// [feat_req_recentip_103] TP flag can be added to base types
#[test]
fn message_type_tp_flag_conversion() {
    assert_eq!(MessageType::Request.with_tp_flag(), Some(MessageType::TpRequest));
    assert_eq!(MessageType::RequestNoReturn.with_tp_flag(), Some(MessageType::TpRequestNoReturn));
    assert_eq!(MessageType::Notification.with_tp_flag(), Some(MessageType::TpNotification));
    assert_eq!(MessageType::Response.with_tp_flag(), Some(MessageType::TpResponse));
    assert_eq!(MessageType::Error.with_tp_flag(), Some(MessageType::TpError));
    
    // Already TP-flagged types return None
    assert_eq!(MessageType::TpRequest.with_tp_flag(), None);
}

/// [feat_req_recentip_103] TP flag can be removed from TP types
#[test]
fn message_type_tp_flag_removal() {
    assert_eq!(MessageType::TpRequest.without_tp_flag(), MessageType::Request);
    assert_eq!(MessageType::TpRequestNoReturn.without_tp_flag(), MessageType::RequestNoReturn);
    assert_eq!(MessageType::TpNotification.without_tp_flag(), MessageType::Notification);
    assert_eq!(MessageType::TpResponse.without_tp_flag(), MessageType::Response);
    assert_eq!(MessageType::TpError.without_tp_flag(), MessageType::Error);
    
    // Base types stay the same
    assert_eq!(MessageType::Request.without_tp_flag(), MessageType::Request);
}

/// [feat_req_recentip_103] Response bit is 0x80
#[test]
fn message_type_response_bit_pattern() {
    // The response bit is bit 7 (0x80)
    // REQUEST (0x00) | 0x80 = RESPONSE (0x80)
    assert_eq!(0x00u8 | 0x80, 0x80); // Request -> Response
    
    // ERROR is 0x81 = Response bit + error indicator
    assert_eq!(0x01u8 | 0x80, 0x81);
    
    // TP versions maintain the pattern
    assert_eq!(0x20u8 | 0x80, 0xA0); // TpRequest -> TpResponse
    assert_eq!(0x21u8 | 0x80, 0xA1); // TpRequestNoReturn -> TpError
}

/// [feat_req_recentip_103] TP bit is 0x20
#[test]
fn message_type_tp_bit_pattern() {
    // TP bit is bit 5 (0x20)
    assert_eq!(0x00u8 | 0x20, 0x20); // Request -> TpRequest
    assert_eq!(0x01u8 | 0x20, 0x21); // RequestNoReturn -> TpRequestNoReturn
    assert_eq!(0x02u8 | 0x20, 0x22); // Notification -> TpNotification
    assert_eq!(0x80u8 | 0x20, 0xA0); // Response -> TpResponse
    assert_eq!(0x81u8 | 0x20, 0xA1); // Error -> TpError
}

/// [feat_req_recentip_282] Valid response types for REQUEST
#[test]
fn message_type_valid_responses_to_request() {
    // REQUEST can receive RESPONSE or ERROR
    assert!(MessageType::Response.is_valid_response_to(MessageType::Request));
    assert!(MessageType::Error.is_valid_response_to(MessageType::Request));
    
    // TP_REQUEST can receive TP_RESPONSE or TP_ERROR
    assert!(MessageType::TpResponse.is_valid_response_to(MessageType::TpRequest));
    assert!(MessageType::TpError.is_valid_response_to(MessageType::TpRequest));
    
    // Cross-type responses are invalid (non-TP response to TP request)
    assert!(!MessageType::Response.is_valid_response_to(MessageType::TpRequest));
    assert!(!MessageType::TpResponse.is_valid_response_to(MessageType::Request));
}

/// [feat_req_recentip_284] No valid responses to REQUEST_NO_RETURN
#[test]
fn message_type_no_responses_to_fire_and_forget() {
    // No response type is valid for fire-and-forget
    assert!(!MessageType::Response.is_valid_response_to(MessageType::RequestNoReturn));
    assert!(!MessageType::Error.is_valid_response_to(MessageType::RequestNoReturn));
    assert!(!MessageType::TpResponse.is_valid_response_to(MessageType::TpRequestNoReturn));
    assert!(!MessageType::TpError.is_valid_response_to(MessageType::TpRequestNoReturn));
}

/// [feat_req_recentip_285] No valid responses to NOTIFICATION
#[test]
fn message_type_no_responses_to_notification() {
    // No response type is valid for notification
    assert!(!MessageType::Response.is_valid_response_to(MessageType::Notification));
    assert!(!MessageType::Error.is_valid_response_to(MessageType::Notification));
}

/// [feat_req_recentip_282] Expected response type mapping
#[test]
fn message_type_expected_response() {
    assert_eq!(MessageType::Request.expected_response_type(), Some(MessageType::Response));
    assert_eq!(MessageType::TpRequest.expected_response_type(), Some(MessageType::TpResponse));
    
    // Fire-and-forget and notification have no expected response
    assert_eq!(MessageType::RequestNoReturn.expected_response_type(), None);
    assert_eq!(MessageType::Notification.expected_response_type(), None);
    
    // Response types themselves have no expected response
    assert_eq!(MessageType::Response.expected_response_type(), None);
    assert_eq!(MessageType::Error.expected_response_type(), None);
}

/// [feat_req_recentip_103] MessageType is_response classification
#[test]
fn message_type_response_classification() {
    // Response types
    assert!(MessageType::Response.is_response());
    assert!(MessageType::Error.is_response());
    assert!(MessageType::TpResponse.is_response());
    assert!(MessageType::TpError.is_response());
    
    // Non-response types
    assert!(!MessageType::Request.is_response());
    assert!(!MessageType::RequestNoReturn.is_response());
    assert!(!MessageType::Notification.is_response());
    assert!(!MessageType::TpRequest.is_response());
}

// ============================================================================
// Integration Tests (require Runtime implementation)
// ============================================================================

use someip_runtime::*;
use std::time::Duration;

#[path = "../simulated.rs"]
mod simulated;
use simulated::{NetworkEvent, SimulatedNetwork};

/// Helper: Find all SOME/IP messages with a specific message type (byte 14)
fn find_messages_with_type(network: &SimulatedNetwork, msg_type: u8) -> Vec<Vec<u8>> {
    network
        .history()
        .into_iter()
        .filter_map(|event| match event {
            NetworkEvent::UdpSent { data, .. } => Some(data),
            NetworkEvent::TcpSent { data, .. } => Some(data),
            _ => None,
        })
        .filter(|data| data.len() >= 16 && data[14] == msg_type)
        .collect()
}

/// [feat_req_recentip_282] REQUEST gets RESPONSE with matching type
#[test]
#[ignore = "Runtime::new not implemented"]
fn request_receives_response_type() {
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
    network.advance(Duration::from_millis(100));
    
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();
    
    // Client sends request
    let method_id = MethodId::new(0x0001);
    let _pending = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(Duration::from_millis(100));
    
    // Server handles and responds
    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        request.responder.send_ok(&[4, 5, 6]).unwrap();
    }
    network.advance(Duration::from_millis(100));
    
    // Verify REQUEST (0x00) was sent
    let requests = find_messages_with_type(&network, 0x00);
    assert!(!requests.is_empty(), "Should have sent REQUEST");
    
    // Verify RESPONSE (0x80) was sent
    let responses = find_messages_with_type(&network, 0x80);
    assert!(!responses.is_empty(), "Should have sent RESPONSE");
}

/// [feat_req_recentip_282] REQUEST can get ERROR type response
#[test]
#[ignore = "Runtime::new not implemented"]
fn request_can_receive_error_type() {
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
    network.advance(Duration::from_millis(100));
    
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();
    
    // Client sends request
    let method_id = MethodId::new(0x0001);
    let _pending = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(Duration::from_millis(100));
    
    // Server handles and sends ERROR response
    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        request.responder.send_error(ReturnCode::NotOk).unwrap();
    }
    network.advance(Duration::from_millis(100));
    
    // Verify REQUEST (0x00) was sent
    let requests = find_messages_with_type(&network, 0x00);
    assert!(!requests.is_empty(), "Should have sent REQUEST");
    
    // Verify ERROR (0x81) was sent
    let errors = find_messages_with_type(&network, 0x81);
    assert!(!errors.is_empty(), "Should have sent ERROR");
}

/// [feat_req_recentip_284] REQUEST_NO_RETURN gets no response
#[test]
#[ignore = "Runtime::new not implemented"]
fn request_no_return_receives_no_response() {
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
    network.advance(Duration::from_millis(100));
    
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();
    
    // Client sends fire-and-forget
    let method_id = MethodId::new(0x0001);
    proxy.fire_and_forget(method_id, &[1, 2, 3]).unwrap();
    network.advance(Duration::from_millis(500));
    
    // Verify REQUEST_NO_RETURN (0x01) was sent
    let requests = find_messages_with_type(&network, 0x01);
    assert!(!requests.is_empty(), "Should have sent REQUEST_NO_RETURN");
    
    // Verify NO response was sent
    let responses = find_messages_with_type(&network, 0x80);
    let errors = find_messages_with_type(&network, 0x81);
    assert!(responses.is_empty(), "Should NOT have sent RESPONSE");
    assert!(errors.is_empty(), "Should NOT have sent ERROR");
}

/// [feat_req_recentip_285] Server can send NOTIFICATION messages
#[test]
#[ignore = "Runtime::new not implemented"]
fn server_sends_notification_type() {
    let (network, io_client, io_server) = SimulatedNetwork::new_pair();
    
    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();
    
    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let eventgroup_id = EventgroupId::new(1).unwrap();
    
    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .eventgroup(eventgroup_id)
        .build()
        .unwrap();
    
    let offering = server.offer(service_config).unwrap();
    network.advance(Duration::from_millis(100));
    
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();
    
    // Client subscribes
    let _subscription = proxy.subscribe(eventgroup_id).unwrap();
    network.advance(Duration::from_millis(100));
    
    // Server sends notification
    let event_id = EventId::new(0x8001).unwrap();
    offering.notify(eventgroup_id, event_id, &[0xAA, 0xBB]).unwrap();
    network.advance(Duration::from_millis(100));
    
    // Verify NOTIFICATION (0x02) was sent
    let notifications = find_messages_with_type(&network, 0x02);
    assert!(!notifications.is_empty(), "Should have sent NOTIFICATION");
    
    // Verify event ID in method field (bytes 2-3)
    let notif = &notifications[0];
    let method_field = u16::from_be_bytes([notif[2], notif[3]]);
    assert_eq!(method_field, 0x8001, "Event ID should be in method field");
}

/// [feat_req_recentip_103] Malformed message type is rejected
#[test]
#[ignore = "Runtime::new not implemented"]
fn malformed_message_type_rejected() {
    let (network, _io_client, io_server) = SimulatedNetwork::new_pair();
    
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();
    
    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    
    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();
    
    let mut offering = server.offer(service_config).unwrap();
    network.advance(Duration::from_millis(100));
    
    // Inject malformed packet with invalid message type 0x03
    let malformed_packet = [
        0x12, 0x34,             // Service ID
        0x00, 0x01,             // Method ID
        0x00, 0x00, 0x00, 0x08, // Length (header only)
        0x00, 0x01,             // Client ID
        0x00, 0x01,             // Session ID
        0x01,                   // Protocol version
        0x01,                   // Interface version
        0x03,                   // Message type: INVALID
        0x00,                   // Return code
    ];
    
    let from_addr = "192.168.1.100:50000".parse().unwrap();
    let to_addr = "192.168.1.20:30490".parse().unwrap();
    network.inject_udp(from_addr, to_addr, &malformed_packet);
    network.advance(Duration::from_millis(100));
    
    // Server should NOT process this as a valid request
    let event = offering.try_next().unwrap();
    assert!(event.is_none(), "Malformed message should be dropped");
}

/// [feat_req_recentip_103] TP-flagged request gets TP-flagged response
#[test]
#[ignore = "TP not implemented"]
fn tp_request_gets_tp_response() {
    // TP (Transport Protocol) is used for large payloads that exceed UDP MTU.
    // When a request uses TP (message type 0x20), the response must also use TP (0xA0).
    // This test requires the TP implementation to be complete.
}

/// [feat_req_recentip_282] Unexpected response is ignored
#[test]
#[ignore = "Runtime::new not implemented"]
fn unexpected_response_ignored() {
    let (network, io_client, _io_server) = SimulatedNetwork::new_pair();
    
    let _client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    network.advance(Duration::from_millis(100));
    
    // Inject a response packet with no matching pending request
    let orphan_response = [
        0x12, 0x34,             // Service ID
        0x00, 0x01,             // Method ID
        0x00, 0x00, 0x00, 0x08, // Length
        0x00, 0x01,             // Client ID
        0x00, 0x99,             // Session ID (no matching request)
        0x01,                   // Protocol version
        0x01,                   // Interface version
        0x80,                   // Message type: RESPONSE
        0x00,                   // Return code: OK
    ];
    
    let from_addr = "192.168.1.100:30490".parse().unwrap();
    let to_addr = "192.168.1.10:50000".parse().unwrap();
    network.inject_udp(from_addr, to_addr, &orphan_response);
    network.advance(Duration::from_millis(100));
    
    // Runtime should silently discard the orphan response
    // No crash, no error - this test passes if we get here without panicking
}

// ============================================================================
// Property-Based Tests
// ============================================================================

#[cfg(test)]
mod proptest_suite {
    use super::*;
    use proptest::prelude::*;
    
    proptest! {
        /// [feat_req_recentip_103] MessageType round-trips through u8
        #[test]
        fn message_type_roundtrip(byte in prop::sample::select(vec![
            0x00u8, 0x01, 0x02, 0x80, 0x81, 0x20, 0x21, 0x22, 0xA0, 0xA1
        ])) {
            let mt = MessageType::try_from(byte).unwrap();
            let back: u8 = mt.into();
            prop_assert_eq!(byte, back);
        }
        
        /// [feat_req_recentip_103] Only valid message types parse successfully
        #[test]
        fn invalid_message_types_fail(byte in 0u8..=255u8) {
            let valid = [0x00, 0x01, 0x02, 0x80, 0x81, 0x20, 0x21, 0x22, 0xA0, 0xA1];
            let result = MessageType::try_from(byte);
            
            if valid.contains(&byte) {
                prop_assert!(result.is_ok());
            } else {
                prop_assert!(result.is_err());
            }
        }
        
        /// [feat_req_recentip_103] TP flag only affects bit 5
        #[test]
        fn tp_flag_only_bit_5(base in prop::sample::select(vec![
            0x00u8, 0x01, 0x02, 0x80, 0x81
        ])) {
            let with_tp = base | 0x20;
            
            // Base type without TP flag
            let base_mt = MessageType::try_from(base).unwrap();
            prop_assert!(!base_mt.is_tp());
            
            // With TP flag
            let tp_mt = MessageType::try_from(with_tp).unwrap();
            prop_assert!(tp_mt.is_tp());
        }
        
        /// [feat_req_recentip_103] without_tp_flag is inverse of with_tp_flag
        #[test]
        fn tp_flag_inverse_operations(base in prop::sample::select(vec![
            0x00u8, 0x01, 0x02, 0x80, 0x81
        ])) {
            let base_mt = MessageType::try_from(base).unwrap();
            
            if let Some(tp_mt) = base_mt.with_tp_flag() {
                prop_assert_eq!(tp_mt.without_tp_flag(), base_mt);
            }
        }
        
        /// [feat_req_recentip_282] Only REQUEST types expect responses
        #[test]
        fn only_requests_expect_responses(byte in prop::sample::select(vec![
            0x00u8, 0x01, 0x02, 0x80, 0x81, 0x20, 0x21, 0x22, 0xA0, 0xA1
        ])) {
            let mt = MessageType::try_from(byte).unwrap();
            let expects = mt.expects_response();
            
            // Only REQUEST (0x00) and TP_REQUEST (0x20) expect responses
            prop_assert_eq!(expects, byte == 0x00 || byte == 0x20);
        }
    }
}
