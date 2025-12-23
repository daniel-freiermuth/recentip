//! Service Instance Management Compliance Tests
//!
//! Tests multiple service instances, instance IDs, and service lifecycle.
//!
//! Key requirements tested:
//! - feat_req_recentip_541: Different services have different Service IDs
//! - feat_req_recentip_544: Different instances have different Service ID + Instance ID
//! - feat_req_recentip_636: Instances identified by different Instance IDs
//! - feat_req_recentip_648: Messages dispatched to correct instance
//! - feat_req_recentip_967: Different instances on same server offered on different ports
//! - feat_req_recentip_445: Different services can share same port
//! - feat_req_recentip_446: Instance identified by Service ID + Instance ID + IP + Port

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
// Multiple Service Instances Tests
// ============================================================================

/// feat_req_recentip_636: Service instances identified by different Instance IDs
///
/// Service-Instances of the same Service are identified through different
/// Instance IDs.
#[test]
#[ignore = "Runtime::new not implemented"]
fn multiple_instances_have_different_ids() {
    covers!(feat_req_recentip_636);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_1 = ConcreteInstanceId::new(1).unwrap();
    let instance_2 = ConcreteInstanceId::new(2).unwrap();
    let instance_3 = ConcreteInstanceId::new(3).unwrap();

    // Server offers 3 instances of the same service
    let config1 = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_1)
        .build()
        .unwrap();
    let config2 = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_2)
        .build()
        .unwrap();
    let config3 = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_3)
        .build()
        .unwrap();

    let _offering1 = server.offer(config1).unwrap();
    let _offering2 = server.offer(config2).unwrap();
    let _offering3 = server.offer(config3).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Client discovers all instances
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));

    // Should be able to get multiple available instances
    let instances = proxy.available_instances();
    assert!(
        instances.len() >= 3,
        "Should discover all 3 instances"
    );

    // All instance IDs should be unique
    let mut ids: Vec<_> = instances.iter().map(|i| i.instance_id()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), instances.len(), "All Instance IDs must be unique");
}

/// feat_req_recentip_648: Messages dispatched to correct instance
///
/// If a server runs different instances of the same service, messages
/// belonging to the different instances shall be dispatched correctly.
#[test]
#[ignore = "Runtime::new not implemented"]
fn messages_dispatched_to_correct_instance() {
    covers!(feat_req_recentip_648);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_1 = ConcreteInstanceId::new(1).unwrap();
    let instance_2 = ConcreteInstanceId::new(2).unwrap();
    let method_id = MethodId::new(0x0001);

    let config1 = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_1)
        .build()
        .unwrap();
    let config2 = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_2)
        .build()
        .unwrap();

    let mut offering1 = server.offer(config1).unwrap();
    let mut offering2 = server.offer(config2).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Client requests specific instance
    let proxy1 = client.require(service_id, InstanceId::from(instance_1));
    let proxy2 = client.require(service_id, InstanceId::from(instance_2));
    network.advance(std::time::Duration::from_millis(100));

    let available1 = proxy1.wait_available().unwrap();
    let available2 = proxy2.wait_available().unwrap();

    // Call instance 1
    let pending1 = available1.call(method_id, b"for_instance_1").unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Only offering1 should receive the request
    let event1 = offering1.try_next().ok().flatten();
    let event2 = offering2.try_next().ok().flatten();

    assert!(event1.is_some(), "Instance 1 should receive request");
    assert!(event2.is_none(), "Instance 2 should NOT receive request for instance 1");

    if let Some(ServiceEvent::MethodCall { request }) = event1 {
        assert_eq!(request.payload, b"for_instance_1");
        request.responder.send_ok(b"from_instance_1").unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    let response1 = pending1.wait().unwrap();
    assert_eq!(response1.payload, b"from_instance_1");

    // Now call instance 2
    let pending2 = available2.call(method_id, b"for_instance_2").unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Only offering2 should receive
    let event1 = offering1.try_next().ok().flatten();
    let event2 = offering2.try_next().ok().flatten();

    assert!(event1.is_none(), "Instance 1 should NOT receive request for instance 2");
    assert!(event2.is_some(), "Instance 2 should receive request");

    if let Some(ServiceEvent::MethodCall { request }) = event2 {
        assert_eq!(request.payload, b"for_instance_2");
        request.responder.send_ok(b"from_instance_2").unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    let response2 = pending2.wait().unwrap();
    assert_eq!(response2.payload, b"from_instance_2");
}

// ============================================================================
// Service ID Uniqueness Tests
// ============================================================================

/// feat_req_recentip_541: Different services have different Service IDs
#[test]
#[ignore = "Runtime::new not implemented"]
fn different_services_have_different_service_ids() {
    covers!(feat_req_recentip_541);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    // Two different services
    let service_a = ServiceId::new(0x1234).unwrap();
    let service_b = ServiceId::new(0x5678).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let config_a = ServiceConfig::builder()
        .service(service_a)
        .instance(instance_id)
        .build()
        .unwrap();
    let config_b = ServiceConfig::builder()
        .service(service_b)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering_a = server.offer(config_a).unwrap();
    let _offering_b = server.offer(config_b).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Client can discover both services separately
    let proxy_a = client.require(service_a, InstanceId::ANY);
    let proxy_b = client.require(service_b, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));

    let available_a = proxy_a.wait_available();
    let available_b = proxy_b.wait_available();

    assert!(available_a.is_ok(), "Should discover service A");
    assert!(available_b.is_ok(), "Should discover service B");
}

/// feat_req_recentip_544: Different instances have unique Service ID + Instance ID
#[test]
#[ignore = "Runtime::new not implemented"]
fn instance_uniquely_identified_by_service_and_instance_id() {
    covers!(feat_req_recentip_544);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    // Two services, each with instance ID 1
    // They should be distinguishable by Service ID
    let service_a = ServiceId::new(0x1234).unwrap();
    let service_b = ServiceId::new(0x5678).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let config_a = ServiceConfig::builder()
        .service(service_a)
        .instance(instance_id)
        .build()
        .unwrap();
    let config_b = ServiceConfig::builder()
        .service(service_b)
        .instance(instance_id)
        .build()
        .unwrap();

    let mut offering_a = server.offer(config_a).unwrap();
    let mut offering_b = server.offer(config_b).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy_a = client.require(service_a, InstanceId::from(instance_id));
    let proxy_b = client.require(service_b, InstanceId::from(instance_id));
    network.advance(std::time::Duration::from_millis(100));

    let available_a = proxy_a.wait_available().unwrap();
    let available_b = proxy_b.wait_available().unwrap();

    let method_id = MethodId::new(0x0001);

    // Call service A
    let pending_a = available_a.call(method_id, b"call_a").unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Only offering_a receives
    let event_a = offering_a.try_next().ok().flatten();
    let event_b = offering_b.try_next().ok().flatten();
    assert!(event_a.is_some());
    assert!(event_b.is_none());

    if let Some(ServiceEvent::MethodCall { request }) = event_a {
        request.responder.send_ok(b"resp_a").unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));
    let _ = pending_a.wait();

    // Call service B
    let pending_b = available_b.call(method_id, b"call_b").unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Only offering_b receives
    let event_a = offering_a.try_next().ok().flatten();
    let event_b = offering_b.try_next().ok().flatten();
    assert!(event_a.is_none());
    assert!(event_b.is_some());

    if let Some(ServiceEvent::MethodCall { request }) = event_b {
        request.responder.send_ok(b"resp_b").unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));
    let _ = pending_b.wait();
}

// ============================================================================
// Port Sharing Tests
// ============================================================================

/// feat_req_recentip_445: Different services can share same port
///
/// While different Services shall be able to share the same port number
/// of the transport protocol.
#[test]
#[ignore = "Runtime::new not implemented"]
fn different_services_can_share_port() {
    covers!(feat_req_recentip_445);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_a = ServiceId::new(0x1234).unwrap();
    let service_b = ServiceId::new(0x5678).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    // Both services on same port
    let config_a = ServiceConfig::builder()
        .service(service_a)
        .instance(instance_id)
        .port(30501)
        .build()
        .unwrap();
    let config_b = ServiceConfig::builder()
        .service(service_b)
        .instance(instance_id)
        .port(30501)  // Same port!
        .build()
        .unwrap();

    let mut offering_a = server.offer(config_a).unwrap();
    let mut offering_b = server.offer(config_b).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy_a = client.require(service_a, InstanceId::ANY);
    let proxy_b = client.require(service_b, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));

    let available_a = proxy_a.wait_available().unwrap();
    let available_b = proxy_b.wait_available().unwrap();

    let method_id = MethodId::new(0x0001);

    // Both services work via same port
    let pending_a = available_a.call(method_id, b"a").unwrap();
    let pending_b = available_b.call(method_id, b"b").unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Each offering receives its own request
    if let Some(ServiceEvent::MethodCall { request }) = offering_a.try_next().ok().flatten() {
        request.responder.send_ok(b"resp_a").unwrap();
    }
    if let Some(ServiceEvent::MethodCall { request }) = offering_b.try_next().ok().flatten() {
        request.responder.send_ok(b"resp_b").unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    let resp_a = pending_a.wait().unwrap();
    let resp_b = pending_b.wait().unwrap();

    assert_eq!(resp_a.payload, b"resp_a");
    assert_eq!(resp_b.payload, b"resp_b");
}

// ============================================================================
// Instance Discovery Tests
// ============================================================================

/// Client can request ANY instance and get one
#[test]
#[ignore = "Runtime::new not implemented"]
fn client_can_request_any_instance() {
    covers!(feat_req_recentip_636);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(42).unwrap();

    let config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Request ANY instance
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));

    let available = proxy.wait_available();
    assert!(available.is_ok(), "Should find an instance when using ANY");
}

/// Client can request specific instance by ID
#[test]
#[ignore = "Runtime::new not implemented"]
fn client_can_request_specific_instance() {
    covers!(feat_req_recentip_636);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_1 = ConcreteInstanceId::new(1).unwrap();
    let instance_2 = ConcreteInstanceId::new(2).unwrap();

    let config1 = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_1)
        .build()
        .unwrap();
    let config2 = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_2)
        .build()
        .unwrap();

    let _offering1 = server.offer(config1).unwrap();
    let _offering2 = server.offer(config2).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Request specific instance 2
    let proxy = client.require(service_id, InstanceId::from(instance_2));
    network.advance(std::time::Duration::from_millis(100));

    let available = proxy.wait_available();
    assert!(available.is_ok(), "Should find specific instance");
}

/// Requesting non-existent instance times out
#[test]
#[ignore = "Runtime::new not implemented"]
fn nonexistent_instance_not_found() {
    covers!(feat_req_recentip_636);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_1 = ConcreteInstanceId::new(1).unwrap();
    let instance_99 = ConcreteInstanceId::new(99).unwrap();

    let config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_1)
        .build()
        .unwrap();

    let _offering = server.offer(config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Request instance that doesn't exist
    let proxy = client.require(service_id, InstanceId::from(instance_99));
    network.advance(std::time::Duration::from_millis(500));

    let available = proxy.try_available();
    assert!(available.is_err(), "Non-existent instance should not be found");
}
