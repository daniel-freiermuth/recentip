//! Integration Tests
//!
//! End-to-end tests that exercise the full stack without being tied to
//! specific spec requirements. These focus on realistic usage scenarios.
//!
//! For spec compliance tests, see tests/compliance/

mod simulated;

use simulated::SimulatedNetwork;
use someip_runtime::prelude::*;

// ============================================================================
// END-TO-END SCENARIOS
// ============================================================================

/// Client discovers server and successfully calls a method
#[test]
#[ignore = "Runtime::new not implemented"]
fn client_server_basic_interaction() {
    let (_network, io_client, io_server) = SimulatedNetwork::new_pair();
    let config = RuntimeConfig::default();

    let mut server = Runtime::new(io_server, config.clone()).unwrap();
    let mut client = Runtime::new(io_client, config).unwrap();

    // Server offers a service
    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(0x0001).unwrap();
    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let mut offering = server.offer(service_config).unwrap();

    // Client discovers and calls
    let proxy = client.require(service_id, InstanceId::ANY);
    let available = proxy.wait_available().unwrap();
    let pending = available.call(MethodId::new(0x0001), b"ping").unwrap();

    // Server responds
    if let ServiceEvent::MethodCall { request } = offering.next().unwrap() {
        request.responder.send_ok(b"pong").unwrap();
    }

    // Client receives
    let response = pending.wait().unwrap();
    assert_eq!(response.payload, b"pong");
}

/// Client subscribes to events and receives notifications
#[test]
#[ignore = "Runtime::new not implemented"]
fn event_subscription_flow() {
    let (_network, io_client, io_server) = SimulatedNetwork::new_pair();
    let config = RuntimeConfig::default();

    let mut server = Runtime::new(io_server, config.clone()).unwrap();
    let mut client = Runtime::new(io_client, config).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(0x0001).unwrap();
    let eventgroup_id = EventgroupId::new(0x01).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .eventgroup(eventgroup_id)
        .build()
        .unwrap();

    let offering = server.offer(service_config).unwrap();

    // Client subscribes
    let proxy = client.require(service_id, InstanceId::ANY);
    let available = proxy.wait_available().unwrap();
    let mut subscription = available.subscribe(eventgroup_id).unwrap();

    // Server sends events
    let event_id = EventId::new(0x8001).unwrap();
    offering.notify(eventgroup_id, event_id, b"event1").unwrap();
    offering.notify(eventgroup_id, event_id, b"event2").unwrap();

    // Client receives events
    let evt1 = subscription.next_event().unwrap().unwrap();
    assert_eq!(evt1.payload, b"event1");

    let evt2 = subscription.next_event().unwrap().unwrap();
    assert_eq!(evt2.payload, b"event2");
}

/// Multiple clients interact with same service
#[test]
#[ignore = "Runtime::new not implemented"]
fn multiple_clients_one_server() {
    // For now, we use new_pair which gives us 2 endpoints.
    // A real multi-client test would need an extended SimulatedNetwork API.
    let (_network, io_client, io_server) = SimulatedNetwork::new_pair();

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

    // Client discovers
    let proxy = client.require(service_id, InstanceId::ANY);
    let available = proxy.wait_available().unwrap();

    // Client makes two calls (simulating multiple concurrent requests)
    let pending1 = available.call(MethodId::new(0x0001), b"request1").unwrap();
    let pending2 = available.call(MethodId::new(0x0001), b"request2").unwrap();

    // Server handles both
    for _ in 0..2 {
        if let ServiceEvent::MethodCall { request } = offering.next().unwrap() {
            request.responder.send_ok(&request.payload).unwrap();
        }
    }

    // Client gets both responses
    let _resp1 = pending1.wait().unwrap();
    let _resp2 = pending2.wait().unwrap();
}

// ============================================================================
// FAULT TOLERANCE
// ============================================================================

/// Network partition causes service unavailability
#[test]
#[ignore = "Runtime::new not implemented"]
fn network_partition_handling() {
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
    let _available = proxy.wait_available().unwrap();

    // Partition the network
    network.partition("192.168.1.0/24", "192.168.2.0/24");

    // Calls should fail or timeout
    // (specific behavior depends on implementation)
}

/// Service restart after client is connected
#[test]
#[ignore = "Runtime::new not implemented"]
fn service_restart_recovery() {
    let (_network, io_client, io_server) = SimulatedNetwork::new_pair();
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

    // Initial connection
    let offering = server.offer(service_config.clone()).unwrap();
    let proxy = client.require(service_id, InstanceId::ANY);
    let _available = proxy.wait_available().unwrap();

    // Server stops offering (simulating restart)
    drop(offering);

    // Client should detect unavailability via poll events
    // (specific behavior depends on implementation)

    // Server offers again
    let _new_offering = server.offer(service_config).unwrap();

    // Client should be able to reconnect
}

// ============================================================================
// PERFORMANCE / STRESS
// ============================================================================

/// Many concurrent pending requests
#[test]
#[ignore = "Runtime::new not implemented"]
fn many_concurrent_requests() {
    let (_network, io_client, io_server) = SimulatedNetwork::new_pair();
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

    // Send 100 concurrent requests
    let mut pending = Vec::new();
    for i in 0..100u32 {
        let p = available
            .call(MethodId::new(0x0001), &i.to_be_bytes())
            .unwrap();
        pending.push(p);
    }

    // Server responds to all
    for _ in 0..100 {
        if let ServiceEvent::MethodCall { request } = offering.next().unwrap() {
            request.responder.send_ok(&request.payload).unwrap();
        }
    }

    // All clients get responses
    for p in pending {
        let _response = p.wait().unwrap();
    }
}
