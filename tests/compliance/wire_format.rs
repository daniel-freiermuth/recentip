//! Wire Format Compliance Tests
//!
//! Tests that verify wire-level protocol compliance using SimulatedNetwork.
//! These tests capture actual packet bytes and verify they match spec requirements.
//!
//! Tests are marked `#[ignore]` until Runtime is implemented.

#[path = "../simulated.rs"]
mod simulated;

#[path = "../wire.rs"]
mod wire;

use crate::covers;
use simulated::{NetworkEvent, SimulatedNetwork};
use someip_runtime::prelude::*;
use wire::{message_type, return_code, Header};

// ============================================================================
// MESSAGE TYPE COMPLIANCE
// ============================================================================

/// feat_req_recentip_103: Message Type REQUEST (0x00) and RESPONSE (0x80)
/// feat_req_recentip_60: Message ID = Service ID || Method ID
/// feat_req_recentip_83: Request ID = Client ID || Session ID
#[test]
#[ignore = "Runtime::new not implemented"]
fn request_response_roundtrip() {
    covers!(
        feat_req_recentip_103,
        feat_req_recentip_60,
        feat_req_recentip_83
    );

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let config = RuntimeConfig::builder()
        .local_addr(std::net::Ipv4Addr::new(192, 168, 1, 1))
        .build()
        .unwrap();

    let mut server_runtime = Runtime::new(io_server, config.clone()).unwrap();
    let mut client_runtime = Runtime::new(io_client, config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(0x0001).unwrap();
    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let mut offering = server_runtime.offer(service_config).unwrap();

    let proxy = client_runtime.require(service_id, InstanceId::ANY);
    let available = proxy.wait_available().unwrap();

    let method_id = MethodId::new(0x0001);
    let pending = available.call(method_id, b"hello").unwrap();

    match offering.next().unwrap() {
        ServiceEvent::MethodCall { request } => {
            assert_eq!(request.method, method_id);
            assert_eq!(request.payload, b"hello");
            request.responder.send_ok(b"world").unwrap();
        }
        _ => panic!("Expected MethodCall"),
    }

    let response = pending.wait().unwrap();
    assert_eq!(response.payload, b"world");

    // Verify wire format
    let history = network.history();
    let packets: Vec<_> = history
        .iter()
        .filter_map(|e| match e {
            NetworkEvent::UdpSent { data, .. } => Some(data.clone()),
            _ => None,
        })
        .collect();

    assert!(packets.len() >= 2, "Should have request and response");

    let request_header = Header::from_bytes(&packets[0]).unwrap();
    assert_eq!(request_header.service_id, 0x1234);
    assert_eq!(request_header.method_id, 0x0001);
    assert_eq!(request_header.message_type, message_type::REQUEST);
    assert_eq!(request_header.protocol_version, 0x01);

    let response_header = Header::from_bytes(&packets[1]).unwrap();
    assert_eq!(response_header.service_id, 0x1234);
    assert_eq!(response_header.method_id, 0x0001);
    assert_eq!(response_header.message_type, message_type::RESPONSE);
    assert_eq!(response_header.return_code, return_code::E_OK);
    assert_eq!(response_header.client_id, request_header.client_id);
    assert_eq!(response_header.session_id, request_header.session_id);
}

/// feat_req_recentip_103: Message Type REQUEST_NO_RETURN (0x01)
#[test]
#[ignore = "Runtime::new not implemented"]
fn fire_and_forget_message_type() {
    covers!(feat_req_recentip_103);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();
    let config = RuntimeConfig::default();

    let mut server = Runtime::new(io_server, config.clone()).unwrap();
    let mut client = Runtime::new(io_client, config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(0x0001).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();

    let proxy = client.require(service_id, InstanceId::ANY);
    let available = proxy.wait_available().unwrap();

    let method_id = MethodId::new(0x0002);
    available.fire_and_forget(method_id, b"fire!").unwrap();

    let history = network.history();
    let request_data = history
        .iter()
        .find_map(|e| match e {
            NetworkEvent::UdpSent { data, .. } => Some(data.clone()),
            _ => None,
        })
        .expect("Should have sent a packet");

    let header = Header::from_bytes(&request_data).unwrap();
    assert_eq!(header.message_type, message_type::REQUEST_NO_RETURN);
}

/// feat_req_recentip_103: Message Type NOTIFICATION (0x02)
/// feat_req_recentip_625: Event IDs have high bit set
#[test]
#[ignore = "Runtime::new not implemented"]
fn notification_message_type() {
    covers!(feat_req_recentip_103, feat_req_recentip_625);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();
    let config = RuntimeConfig::default();

    let mut server = Runtime::new(io_server, config.clone()).unwrap();
    let mut client = Runtime::new(io_client, config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(0x0001).unwrap();
    let eventgroup_id = EventgroupId::new(0x01).unwrap();
    let event_id = EventId::new(0x8001).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .eventgroup(eventgroup_id)
        .build()
        .unwrap();

    let offering = server.offer(service_config).unwrap();

    let proxy = client.require(service_id, InstanceId::ANY);
    let available = proxy.wait_available().unwrap();
    let _subscription = available.subscribe(eventgroup_id).unwrap();

    offering
        .notify(eventgroup_id, event_id, b"event data")
        .unwrap();

    let history = network.history();
    let notification_data = history
        .iter()
        .filter_map(|e| match e {
            NetworkEvent::UdpSent { data, .. } => Header::from_bytes(data),
            _ => None,
        })
        .find(|h| h.message_type == message_type::NOTIFICATION)
        .expect("Should have notification packet");

    assert_eq!(notification_data.service_id, 0x1234);
    assert_eq!(notification_data.method_id, 0x8001);
    assert!(notification_data.is_event());
}

// ============================================================================
// SESSION ID COMPLIANCE
// ============================================================================

/// feat_req_recentip_256: Session ID incremented per request
#[test]
#[ignore = "Runtime::new not implemented"]
fn session_id_increments() {
    covers!(feat_req_recentip_256);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();
    let config = RuntimeConfig::default();

    let mut server = Runtime::new(io_server, config.clone()).unwrap();
    let mut client = Runtime::new(io_client, config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(0x0001).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();

    let proxy = client.require(service_id, InstanceId::ANY);
    let available = proxy.wait_available().unwrap();
    let method_id = MethodId::new(0x0001);

    let _pending1 = available.call(method_id, b"req1").unwrap();
    let _pending2 = available.call(method_id, b"req2").unwrap();
    let _pending3 = available.call(method_id, b"req3").unwrap();

    let history = network.history();
    let headers: Vec<_> = history
        .iter()
        .filter_map(|e| match e {
            NetworkEvent::UdpSent { data, .. } => Header::from_bytes(data),
            _ => None,
        })
        .filter(|h| h.is_request())
        .collect();

    assert_eq!(headers.len(), 3);

    assert_eq!(headers[0].client_id, headers[1].client_id);
    assert_eq!(headers[1].client_id, headers[2].client_id);

    assert_eq!(headers[1].session_id, headers[0].session_id.wrapping_add(1));
    assert_eq!(headers[2].session_id, headers[1].session_id.wrapping_add(1));
}

// ============================================================================
// ERROR RESPONSE COMPLIANCE
// ============================================================================

/// feat_req_recentip_371: Return Code E_UNKNOWN_METHOD (0x03)
/// feat_req_recentip_103: Message Type ERROR (0x81)
#[test]
#[ignore = "Runtime::new not implemented"]
fn unknown_method_returns_error() {
    covers!(feat_req_recentip_371, feat_req_recentip_103);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();
    let config = RuntimeConfig::default();

    let mut server = Runtime::new(io_server, config.clone()).unwrap();
    let mut client = Runtime::new(io_client, config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(0x0001).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let mut offering = server.offer(service_config).unwrap();

    let proxy = client.require(service_id, InstanceId::ANY);
    let available = proxy.wait_available().unwrap();

    let unknown_method = MethodId::new(0x9999);
    let pending = available.call(unknown_method, b"data").unwrap();

    match offering.next().unwrap() {
        ServiceEvent::MethodCall { request } => {
            request
                .responder
                .send_error(ReturnCode::UnknownMethod)
                .unwrap();
        }
        _ => panic!("Expected MethodCall"),
    }

    let response = pending.wait().unwrap();
    assert_eq!(response.return_code, ReturnCode::UnknownMethod);

    let history = network.history();
    let error_header = history
        .iter()
        .filter_map(|e| match e {
            NetworkEvent::UdpSent { data, .. } => Header::from_bytes(data),
            _ => None,
        })
        .find(|h| h.is_error())
        .expect("Should have error response");

    assert_eq!(error_header.message_type, message_type::ERROR);
    assert_eq!(error_header.return_code, return_code::E_UNKNOWN_METHOD);
}

// ============================================================================
// PROTOCOL VERSION COMPLIANCE
// ============================================================================

/// feat_req_recentip_90: Protocol Version shall be 0x01
#[test]
#[ignore = "Runtime::new not implemented"]
fn protocol_version_in_wire_format() {
    covers!(feat_req_recentip_90);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();
    let config = RuntimeConfig::default();

    let mut server = Runtime::new(io_server, config.clone()).unwrap();
    let mut client = Runtime::new(io_client, config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(0x0001).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();

    let proxy = client.require(service_id, InstanceId::ANY);
    let available = proxy.wait_available().unwrap();
    let method_id = MethodId::new(0x0001);

    let _pending = available.call(method_id, b"test").unwrap();

    let history = network.history();
    for event in history.iter() {
        if let NetworkEvent::UdpSent { data, .. } = event {
            if let Some(header) = Header::from_bytes(data) {
                assert_eq!(
                    header.protocol_version, 0x01,
                    "All packets must have protocol version 0x01"
                );
            }
        }
    }
}

// ============================================================================
// LENGTH FIELD COMPLIANCE
// ============================================================================

/// feat_req_recentip_67: Length field interpretation
#[test]
#[ignore = "Runtime::new not implemented"]
fn length_field_includes_header_remainder() {
    covers!(feat_req_recentip_67);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();
    let config = RuntimeConfig::default();

    let mut server = Runtime::new(io_server, config.clone()).unwrap();
    let mut client = Runtime::new(io_client, config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(0x0001).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();

    let proxy = client.require(service_id, InstanceId::ANY);
    let available = proxy.wait_available().unwrap();
    let method_id = MethodId::new(0x0001);

    let payload = b"hello world"; // 11 bytes
    let _pending = available.call(method_id, payload).unwrap();

    let history = network.history();
    let request_data = history
        .iter()
        .find_map(|e| match e {
            NetworkEvent::UdpSent { data, .. } => Some(data.clone()),
            _ => None,
        })
        .expect("Should have request packet");

    let header = Header::from_bytes(&request_data).unwrap();

    let expected_length = 8 + payload.len() as u32;
    assert_eq!(header.length, expected_length);
    assert_eq!(header.payload_length(), payload.len() as u32);
}

// ============================================================================
// HEADER PARSING UNIT TESTS (test infrastructure, not conformance)
// ============================================================================

mod header_parsing {
    use super::*;

    #[test]
    fn header_size_is_16_bytes() {
        assert_eq!(Header::SIZE, 16);
    }

    #[test]
    fn protocol_version_constant() {
        assert_eq!(Header::PROTOCOL_VERSION, 0x01);
    }

    #[test]
    fn header_roundtrip() {
        let original = Header {
            service_id: 0x1234,
            method_id: 0x5678,
            length: 0x0000000C,
            client_id: 0xABCD,
            session_id: 0xEF01,
            protocol_version: 0x01,
            interface_version: 0x02,
            message_type: message_type::REQUEST,
            return_code: return_code::E_OK,
        };

        let bytes = original.to_bytes();
        let parsed = Header::from_bytes(&bytes).unwrap();

        assert_eq!(parsed, original);
    }

    #[test]
    fn header_big_endian() {
        let header = Header {
            service_id: 0x1234,
            method_id: 0x5678,
            length: 0x0000000C,
            client_id: 0xABCD,
            session_id: 0xEF01,
            protocol_version: 0x01,
            interface_version: 0x02,
            message_type: message_type::REQUEST,
            return_code: return_code::E_OK,
        };

        let bytes = header.to_bytes();

        assert_eq!(bytes[0], 0x12);
        assert_eq!(bytes[1], 0x34);
        assert_eq!(bytes[2], 0x56);
        assert_eq!(bytes[3], 0x78);
    }

    #[test]
    fn message_id_composition() {
        let header = Header {
            service_id: 0x1234,
            method_id: 0x5678,
            length: 8,
            client_id: 0,
            session_id: 0,
            protocol_version: 1,
            interface_version: 1,
            message_type: 0,
            return_code: 0,
        };

        assert_eq!(header.message_id(), 0x12345678);
    }

    #[test]
    fn request_id_composition() {
        let header = Header {
            service_id: 0,
            method_id: 0,
            length: 8,
            client_id: 0xABCD,
            session_id: 0x1234,
            protocol_version: 1,
            interface_version: 1,
            message_type: 0,
            return_code: 0,
        };

        assert_eq!(header.request_id(), 0xABCD1234);
    }

    #[test]
    fn message_type_constants() {
        assert_eq!(message_type::REQUEST, 0x00);
        assert_eq!(message_type::REQUEST_NO_RETURN, 0x01);
        assert_eq!(message_type::NOTIFICATION, 0x02);
        assert_eq!(message_type::RESPONSE, 0x80);
        assert_eq!(message_type::ERROR, 0x81);
        assert_eq!(message_type::TP_FLAG, 0x20);
    }

    #[test]
    fn return_code_constants() {
        assert_eq!(return_code::E_OK, 0x00);
        assert_eq!(return_code::E_NOT_OK, 0x01);
        assert_eq!(return_code::E_UNKNOWN_SERVICE, 0x02);
        assert_eq!(return_code::E_UNKNOWN_METHOD, 0x03);
        assert_eq!(return_code::E_WRONG_PROTOCOL_VERSION, 0x07);
        assert_eq!(return_code::E_MALFORMED_MESSAGE, 0x09);
    }
}
