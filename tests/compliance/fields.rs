//! Field Operations Compliance Tests
//!
//! Fields are combinations of getters, setters, and notifiers per SOME/IP spec.
//!
//! Key requirements tested:
//! - feat_req_recentip_631: Field is combination of getter/setter/notifier
//! - feat_req_recentip_632: Field without getter/setter/notifier shall not exist
//! - feat_req_recentip_633: Getter is request/response with empty request payload
//! - feat_req_recentip_634: Setter is request/response with value as request payload
//! - feat_req_recentip_635: Notifier sends notification event with updated value

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
    pub const RESPONSE: u8 = 0x80;
    pub const NOTIFICATION: u8 = 0x02;
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Find RPC messages (non-SD) in network history
fn find_rpc_messages(network: &SimulatedNetwork) -> Vec<(Header, Vec<u8>)> {
    network
        .history()
        .into_iter()
        .filter_map(|event| {
            if let NetworkEvent::UdpSent { data, to, .. } = event {
                if to.port() != 30490 && data.len() >= 16 {
                    if let Some(header) = Header::from_bytes(&data) {
                        return Some((header, data));
                    }
                }
            }
            None
        })
        .collect()
}

// ============================================================================
// Field Getter Tests
// ============================================================================

/// feat_req_recentip_633: Getter has empty request payload, value in response
///
/// The getter of a field shall be a request/response call that has an empty
/// payload for the request and the current value as payload of the response.
#[test]
#[ignore = "Runtime::new not implemented"]
fn field_getter_empty_request_payload() {
    covers!(feat_req_recentip_633);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    // Getter method ID (by convention, could be any valid method ID)
    let getter_method = MethodId::new(0x0001);

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

    // Call getter with empty payload
    let pending = available.get_field(getter_method).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Verify request has empty payload
    let messages = find_rpc_messages(&network);
    let request = messages.iter().find(|(h, _)| h.message_type == message_type::REQUEST);
    assert!(request.is_some(), "Should send REQUEST message");

    let (header, data) = request.unwrap();
    let payload_len = data.len() - 16;
    assert_eq!(payload_len, 0, "Getter request should have empty payload");

    // Server responds with field value
    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        assert!(request.payload.is_empty(), "Getter request payload should be empty");
        request.responder.send_ok(b"field_value").unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    // Client receives value
    let response = pending.wait().unwrap();
    assert_eq!(response.payload, b"field_value");
}

/// feat_req_recentip_633: Getter returns current value in response
#[test]
#[ignore = "Runtime::new not implemented"]
fn field_getter_returns_current_value() {
    covers!(feat_req_recentip_633);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let getter_method = MethodId::new(0x0001);

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

    // Get current value
    let pending = available.get_field(getter_method).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Server returns current value (e.g., temperature = 25)
    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        let current_temp: u16 = 25;
        request.responder.send_ok(&current_temp.to_be_bytes()).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    let response = pending.wait().unwrap();
    assert_eq!(response.payload, 25u16.to_be_bytes());
}

// ============================================================================
// Field Setter Tests
// ============================================================================

/// feat_req_recentip_634: Setter has desired value in request payload
///
/// The setter of a field shall be a request/response call that has the
/// desired value as payload for the request.
#[test]
#[ignore = "Runtime::new not implemented"]
fn field_setter_sends_value_in_request() {
    covers!(feat_req_recentip_634);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let setter_method = MethodId::new(0x0002);

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

    // Set field to new value
    let new_value = 42u16.to_be_bytes();
    let pending = available.set_field(setter_method, &new_value).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Verify request contains the value
    let messages = find_rpc_messages(&network);
    let request = messages.iter().find(|(h, _)| h.message_type == message_type::REQUEST);
    assert!(request.is_some());

    let (_, data) = request.unwrap();
    let payload = &data[16..];
    assert_eq!(payload, &new_value, "Setter request should contain the new value");

    // Server receives and acknowledges
    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        assert_eq!(request.payload, new_value, "Server should receive the new value");
        request.responder.send_ok_empty().unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    let response = pending.wait().unwrap();
    assert_eq!(response.return_code, ReturnCode::Ok);
}

/// feat_req_recentip_634: Setter is request/response (not fire&forget)
#[test]
#[ignore = "Runtime::new not implemented"]
fn field_setter_gets_response() {
    covers!(feat_req_recentip_634);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let setter_method = MethodId::new(0x0002);

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

    // Set field
    let pending = available.set_field(setter_method, &[1, 2, 3, 4]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Server must respond (setter is request/response)
    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        request.responder.send_ok_empty().unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    // Client should receive response
    let response = pending.wait();
    assert!(response.is_ok(), "Setter should receive response");
}

// ============================================================================
// Field Notifier Tests  
// ============================================================================

/// feat_req_recentip_635: Notifier sends notification event with updated value
///
/// The notifier shall send a notification event message that communicates
/// the updated value of the field.
#[test]
#[ignore = "Runtime::new not implemented"]
fn field_notifier_sends_updated_value() {
    covers!(feat_req_recentip_635);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let eventgroup_id = EventgroupId::new(0x01).unwrap();
    // Notifier event ID (high bit set for events)
    let notifier_event = EventId::new(0x8001).unwrap();

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

    // Subscribe to field notifications
    let mut subscription = available.subscribe(eventgroup_id).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    network.clear_history();

    // Field value changes, server sends notification
    let updated_value = 100u32.to_be_bytes();
    offering.notify(eventgroup_id, notifier_event, &updated_value).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Verify notification message was sent
    let messages = find_rpc_messages(&network);
    let notification = messages.iter().find(|(h, _)| h.message_type == message_type::NOTIFICATION);
    assert!(notification.is_some(), "Should send NOTIFICATION message");

    let (_, data) = notification.unwrap();
    let payload = &data[16..];
    assert_eq!(payload, &updated_value, "Notification should contain updated value");

    // Client receives the notification
    let event = subscription.try_next_event().ok().flatten();
    assert!(event.is_some(), "Client should receive field update notification");
}

/// feat_req_recentip_631: Field is combination of getter/setter/notifier
#[test]
#[ignore = "Runtime::new not implemented"]
fn field_combines_getter_setter_notifier() {
    covers!(feat_req_recentip_631);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let eventgroup_id = EventgroupId::new(0x01).unwrap();

    // Field "Temperature" has getter, setter, and notifier
    let temp_getter = MethodId::new(0x0001);
    let temp_setter = MethodId::new(0x0002);
    let temp_notifier = EventId::new(0x8001).unwrap();

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

    // Subscribe to notifications
    let mut subscription = available.subscribe(eventgroup_id).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // 1. GET current value
    let get_pending = available.get_field(temp_getter).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        request.responder.send_ok(&20u16.to_be_bytes()).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    let current = get_pending.wait().unwrap();
    assert_eq!(current.payload, 20u16.to_be_bytes());

    // 2. SET new value
    let set_pending = available.set_field(temp_setter, &25u16.to_be_bytes()).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        // Server updates internal state and acknowledges
        request.responder.send_ok_empty().unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    let set_response = set_pending.wait().unwrap();
    assert_eq!(set_response.return_code, ReturnCode::Ok);

    // 3. Server sends NOTIFICATION of new value
    offering.notify(eventgroup_id, temp_notifier, &25u16.to_be_bytes()).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    let notification = subscription.try_next_event().ok().flatten();
    assert!(notification.is_some(), "Should receive field update notification");
}

// ============================================================================
// Field Validation Tests
// ============================================================================

/// Setter can reject invalid values with error response
#[test]
#[ignore = "Runtime::new not implemented"]
fn field_setter_can_reject_invalid_value() {
    covers!(feat_req_recentip_634, feat_req_recentip_649);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let setter_method = MethodId::new(0x0002);

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

    // Try to set invalid value (e.g., temperature > 100)
    let invalid_value = 150u16.to_be_bytes();
    let pending = available.set_field(setter_method, &invalid_value).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Server rejects with error
    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        request.responder.send_error(ReturnCode::NotOk).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    let response = pending.wait().unwrap();
    assert_eq!(response.return_code, ReturnCode::NotOk, "Invalid value should be rejected");
}
