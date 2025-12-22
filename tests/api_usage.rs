//! API usage tests
//!
//! These tests document how users will interact with the library.
//! They use a simulated network backend for deterministic testing.

#![allow(unused_variables)]

use someip_runtime::prelude::*;

mod simulated;
use simulated::*;

// ============================================================================
// TYPE SAFETY TESTS - Invalid states should be unrepresentable
// ============================================================================

#[test]
fn reserved_service_ids_are_rejected() {
    assert!(ServiceId::new(0x0000).is_none(), "0x0000 is reserved");
    assert!(ServiceId::new(0xFFFF).is_none(), "0xFFFF is reserved");
    assert!(ServiceId::new(0x1234).is_some(), "normal IDs are valid");
}

#[test]
fn reserved_instance_ids_are_rejected() {
    // Concrete instance IDs (for servers) cannot be wildcard
    assert!(ConcreteInstanceId::new(0x0000).is_none(), "0x0000 is reserved");
    assert!(ConcreteInstanceId::new(0xFFFF).is_none(), "0xFFFF is wildcard");
    assert!(ConcreteInstanceId::new(0x0001).is_some());

    // Client-side InstanceId allows ANY (0xFFFF)
    assert!(InstanceId::new(0x0000).is_none(), "0x0000 is reserved");
    assert_eq!(InstanceId::ANY.value(), 0xFFFF);
}

#[test]
fn concrete_instance_converts_to_instance() {
    let concrete = ConcreteInstanceId::new(0x42).unwrap();
    let instance: InstanceId = concrete.into();
    assert_eq!(instance.value(), 0x42);
}

#[test]
fn event_ids_must_have_high_bit_set() {
    assert!(EventId::new(0x0001).is_none(), "methods don't have high bit");
    assert!(EventId::new(0x7FFF).is_none(), "just below event range");
    assert!(EventId::new(0x8000).is_some(), "first event ID");
    assert!(EventId::new(0xFFFE).is_some(), "last valid event ID");
    assert!(EventId::new(0xFFFF).is_none(), "0xFFFF is reserved");
}

#[test]
fn sd_port_is_reserved() {
    assert!(AppPort::new(30490).is_none(), "SD port is reserved");
    assert!(AppPort::new(30489).is_some(), "adjacent ports are fine");
    assert!(AppPort::new(30491).is_some());
}

#[test]
fn eventgroup_zero_is_reserved() {
    assert!(EventgroupId::new(0x0000).is_none());
    assert!(EventgroupId::new(0x0001).is_some());
}

// ============================================================================
// CLIENT API TESTS
// ============================================================================

#[test]
fn client_discovers_available_service() {
    let (network, io_a, io_b) = SimulatedNetwork::new_pair();

    // Server side: offer a service
    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(0x0001).unwrap();

    let server_config = ServerConfig {};
    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .endpoint("192.168.1.10", |e| e.udp(AppPort::new(30501).unwrap()))
        .build()
        .unwrap();

    let mut server = Server::new(io_b, server_config).unwrap();
    let _offering = server.offer(service_config).unwrap();

    // Client side: discover the service
    let client_config = ClientConfig::builder().build().unwrap();
    let mut client = Client::new(io_a, client_config).unwrap();

    let proxy = client.require(service_id, InstanceId::ANY);
    
    // Simulate SD exchange
    network.advance(std::time::Duration::from_millis(100));

    // Service should be available now
    let available = proxy.wait_available().unwrap();
    
    // We can call methods on available proxy
    let _response = available.call(MethodId::new(0x01), &[1, 2, 3]).unwrap();
}

#[test]
fn client_cannot_call_methods_on_unavailable_service() {
    // This is enforced at compile time through typestate!
    // An Unavailable proxy has no call() method.
    //
    // ```compile_fail
    // let proxy: ServiceProxy<_, Unavailable> = client.require(...);
    // proxy.call(method, &data);  // ERROR: no method `call` on Unavailable
    // ```

    // We can only test the API shape here - the compiler prevents misuse
}

#[test]
fn client_subscription_receives_events() {
    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let service_id = ServiceId::new(0x5678).unwrap();
    let instance_id = ConcreteInstanceId::new(0x0001).unwrap();
    let eventgroup = EventgroupId::new(0x01).unwrap();
    let event_id = EventId::new(0x8001).unwrap();

    // Server setup
    let mut server = Server::new(
        io_server,
        ServerConfig {},
    ).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .eventgroup(eventgroup)
        .build()
        .unwrap();

    let mut offering = server.offer(service_config).unwrap();

    // Client setup
    let mut client = Client::new(io_client, ClientConfig::builder().build().unwrap()).unwrap();
    let proxy = client.require(service_id, InstanceId::ANY);
    
    network.advance(std::time::Duration::from_millis(100));
    
    let available = proxy.wait_available().unwrap();
    let mut subscription = available.subscribe(eventgroup).unwrap();

    // Server side: handle subscription request
    network.advance(std::time::Duration::from_millis(50));

    match offering.try_next().unwrap() {
        Some(ServiceEvent::Subscribe { ack, .. }) => {
            ack.accept().unwrap();
        }
        _ => panic!("expected subscribe event"),
    }

    // Server sends event
    offering.notify(eventgroup, event_id, b"hello from server").unwrap();

    network.advance(std::time::Duration::from_millis(10));

    // Client receives event
    let event = subscription.next_event().unwrap().unwrap();
    assert_eq!(event.event_id, event_id);
    assert_eq!(event.payload, b"hello from server");
}

#[test]
fn subscription_can_call_methods() {
    // Once subscribed, we can also call methods on the same service
    // without needing to keep the proxy around
    
    let (_network, io, _) = SimulatedNetwork::new_pair();
    let client_config = ClientConfig::builder().build().unwrap();
    let mut client = Client::new(io, client_config).unwrap();
    
    let service_id = ServiceId::new(0x1234).unwrap();
    let _proxy = client.require(service_id, InstanceId::ANY);
    
    // This test just documents the API - actual implementation would:
    // let subscription = available.subscribe(eventgroup)?;
    // let response = subscription.call(method, &data)?; // can still call methods
}

#[test]
fn subscription_stops_on_drop() {
    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let service_id = ServiceId::new(0x9ABC).unwrap();
    let eventgroup = EventgroupId::new(0x01).unwrap();

    // Setup server
    let mut server = Server::new(io_server, ServerConfig {}).unwrap();
    let config = ServiceConfig::builder()
        .service(service_id)
        .instance(ConcreteInstanceId::new(0x01).unwrap())
        .eventgroup(eventgroup)
        .build()
        .unwrap();
    let mut offering = server.offer(config).unwrap();

    // Client subscribes
    let mut client = Client::new(io_client, ClientConfig::builder().build().unwrap()).unwrap();
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    
    let available = proxy.wait_available().unwrap();
    let subscription = available.subscribe(eventgroup).unwrap();

    // Handle subscription on server
    network.advance(std::time::Duration::from_millis(50));
    if let Some(ServiceEvent::Subscribe { ack, .. }) = offering.try_next().unwrap() {
        ack.accept().unwrap();
    }

    assert!(offering.has_subscribers(eventgroup));

    // Drop subscription - should trigger StopSubscribeEventgroup
    drop(subscription);
    network.advance(std::time::Duration::from_millis(50));

    // Server should receive unsubscribe
    match offering.try_next().unwrap() {
        Some(ServiceEvent::Unsubscribe { eventgroup: eg, .. }) => {
            assert_eq!(eg, eventgroup);
        }
        _ => panic!("expected unsubscribe event"),
    }

    assert!(!offering.has_subscribers(eventgroup));
}

// ============================================================================
// SERVER API TESTS
// ============================================================================

#[test]
fn server_must_respond_to_requests() {
    // Responder panics in debug mode if dropped without response
    // This test documents the expected behavior
    
    // In actual usage:
    // match offering.next()? {
    //     ServiceEvent::MethodCall { request } => {
    //         // MUST respond - responder panics on drop otherwise
    //         request.responder.send_ok(&response)?;
    //     }
    //     ...
    // }
}

#[test]
fn server_must_handle_subscribe_requests() {
    // SubscribeAck panics if dropped without accept/reject
    
    // In actual usage:
    // ServiceEvent::Subscribe { ack, eventgroup, .. } => {
    //     if can_handle(eventgroup) {
    //         ack.accept()?;
    //     } else {
    //         ack.reject(RejectReason::ResourceLimit)?;
    //     }
    // }
}

#[test]
fn server_handles_method_calls() {
    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let service_id = ServiceId::new(0x1111).unwrap();
    let method_id = MethodId::new(0x42);

    // Server
    let mut server = Server::new(io_server, ServerConfig {}).unwrap();
    let config = ServiceConfig::builder()
        .service(service_id)
        .instance(ConcreteInstanceId::new(0x01).unwrap())
        .build()
        .unwrap();
    let mut offering = server.offer(config).unwrap();

    // Client
    let mut client = Client::new(io_client, ClientConfig::builder().build().unwrap()).unwrap();
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));

    let available = proxy.wait_available().unwrap();
    let pending = available.call(method_id, b"request data").unwrap();

    network.advance(std::time::Duration::from_millis(50));

    // Server handles request
    match offering.next().unwrap() {
        ServiceEvent::MethodCall { request } => {
            assert_eq!(request.method, method_id);
            assert_eq!(request.payload, b"request data");
            request.responder.send_ok(b"response data").unwrap();
        }
        _ => panic!("expected method call"),
    }

    network.advance(std::time::Duration::from_millis(50));

    // Client receives response
    let response = pending.wait().unwrap();
    assert_eq!(response.return_code, ReturnCode::Ok);
    assert_eq!(response.payload, b"response data");
}

#[test]
fn server_handles_fire_and_forget() {
    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let service_id = ServiceId::new(0x2222).unwrap();
    let method_id = MethodId::new(0x99);

    // Server
    let mut server = Server::new(io_server, ServerConfig {}).unwrap();
    let config = ServiceConfig::builder()
        .service(service_id)
        .instance(ConcreteInstanceId::new(0x01).unwrap())
        .build()
        .unwrap();
    let mut offering = server.offer(config).unwrap();

    // Client
    let mut client = Client::new(io_client, ClientConfig::builder().build().unwrap()).unwrap();
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));

    let available = proxy.wait_available().unwrap();
    
    // Fire and forget - no response expected, returns immediately
    available.fire_and_forget(method_id, b"one-way data").unwrap();

    network.advance(std::time::Duration::from_millis(50));

    // Server receives the message
    match offering.try_next().unwrap() {
        Some(ServiceEvent::FireAndForget { method, payload }) => {
            assert_eq!(method, method_id);
            assert_eq!(payload, b"one-way data");
        }
        other => panic!("expected fire and forget, got {:?}", other.is_some()),
    }
}

#[test]
fn server_notify_only_sends_to_subscribers() {
    let (network, io_a, io_b) = SimulatedNetwork::new_pair();
    
    let service_id = ServiceId::new(0x3333).unwrap();
    let eventgroup1 = EventgroupId::new(0x01).unwrap();
    let eventgroup2 = EventgroupId::new(0x02).unwrap();
    let event_id = EventId::new(0x8001).unwrap();

    // Server with two eventgroups
    let mut server = Server::new(io_b, ServerConfig {}).unwrap();
    let config = ServiceConfig::builder()
        .service(service_id)
        .instance(ConcreteInstanceId::new(0x01).unwrap())
        .eventgroup(eventgroup1)
        .eventgroup(eventgroup2)
        .build()
        .unwrap();
    let mut offering = server.offer(config).unwrap();

    // Client subscribes to eventgroup1 only
    let mut client = Client::new(io_a, ClientConfig::builder().build().unwrap()).unwrap();
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));

    let available = proxy.wait_available().unwrap();
    let mut sub1 = available.subscribe(eventgroup1).unwrap();

    network.advance(std::time::Duration::from_millis(50));

    // Accept subscription
    if let Some(ServiceEvent::Subscribe { ack, eventgroup, .. }) = offering.try_next().unwrap() {
        assert_eq!(eventgroup, eventgroup1);
        ack.accept().unwrap();
    }

    // Server sends to eventgroup1 - should reach client
    assert!(offering.has_subscribers(eventgroup1));
    offering.notify(eventgroup1, event_id, b"event for group 1").unwrap();

    // Server sends to eventgroup2 - no subscribers, no-op
    assert!(!offering.has_subscribers(eventgroup2));
    // notify should still succeed, just doesn't send anything
    offering.notify(eventgroup2, event_id, b"event for group 2").unwrap();

    network.advance(std::time::Duration::from_millis(50));

    // Client only receives eventgroup1's event
    let event = sub1.try_next_event().unwrap().unwrap();
    assert_eq!(event.payload, b"event for group 1");

    // No more events
    assert!(sub1.try_next_event().unwrap().is_none());
}

// ============================================================================
// FIELD TESTS
// ============================================================================

#[test]
fn client_can_get_and_set_fields() {
    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let service_id = ServiceId::new(0x4444).unwrap();
    let field = MethodId::new(0x01); // Fields use method IDs

    // Server
    let mut server = Server::new(io_server, ServerConfig {}).unwrap();
    let config = ServiceConfig::builder()
        .service(service_id)
        .instance(ConcreteInstanceId::new(0x01).unwrap())
        .build()
        .unwrap();
    let mut offering = server.offer(config).unwrap();

    let mut field_value: Vec<u8> = b"initial".to_vec();

    // Client
    let mut client = Client::new(io_client, ClientConfig::builder().build().unwrap()).unwrap();
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));

    let available = proxy.wait_available().unwrap();

    // GET field
    let pending = available.get_field(field).unwrap();
    network.advance(std::time::Duration::from_millis(50));

    match offering.next().unwrap() {
        ServiceEvent::GetField { field: f, responder } => {
            assert_eq!(f, field);
            responder.send_ok(&field_value).unwrap();
        }
        _ => panic!("expected get field"),
    }

    network.advance(std::time::Duration::from_millis(50));
    let response = pending.wait().unwrap();
    assert_eq!(response.payload, b"initial");

    // SET field
    let pending = available.set_field(field, b"updated").unwrap();
    network.advance(std::time::Duration::from_millis(50));

    match offering.next().unwrap() {
        ServiceEvent::SetField { field: f, value, responder } => {
            assert_eq!(f, field);
            field_value = value;
            responder.send_ok_empty().unwrap();
        }
        _ => panic!("expected set field"),
    }

    assert_eq!(field_value, b"updated");
}

// ============================================================================
// CONFIGURATION TESTS
// ============================================================================

#[test]
fn service_config_requires_all_fields() {
    // Missing service ID
    let result = ServiceConfig::builder()
        .instance(ConcreteInstanceId::new(0x01).unwrap())
        .build();
    assert!(result.is_err());

    // Missing instance ID
    let result = ServiceConfig::builder()
        .service(ServiceId::new(0x1234).unwrap())
        .build();
    assert!(result.is_err());

    // Complete config
    let result = ServiceConfig::builder()
        .service(ServiceId::new(0x1234).unwrap())
        .instance(ConcreteInstanceId::new(0x01).unwrap())
        .build();
    assert!(result.is_ok());
}

// ============================================================================
// ERROR HANDLING TESTS
// ============================================================================

#[test]
fn method_call_returns_error_response() {
    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let service_id = ServiceId::new(0x5555).unwrap();
    let method_id = MethodId::new(0x01);

    // Server
    let mut server = Server::new(io_server, ServerConfig {}).unwrap();
    let config = ServiceConfig::builder()
        .service(service_id)
        .instance(ConcreteInstanceId::new(0x01).unwrap())
        .build()
        .unwrap();
    let mut offering = server.offer(config).unwrap();

    // Client
    let mut client = Client::new(io_client, ClientConfig::builder().build().unwrap()).unwrap();
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));

    let available = proxy.wait_available().unwrap();
    let pending = available.call(method_id, b"bad request").unwrap();

    network.advance(std::time::Duration::from_millis(50));

    // Server rejects with error
    match offering.next().unwrap() {
        ServiceEvent::MethodCall { request } => {
            request.responder.send_error(ReturnCode::NotReady).unwrap();
        }
        _ => panic!("expected method call"),
    }

    network.advance(std::time::Duration::from_millis(50));

    let response = pending.wait().unwrap();
    assert_eq!(response.return_code, ReturnCode::NotReady);
}

#[test]
fn subscription_rejection_is_reported() {
    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let service_id = ServiceId::new(0x6666).unwrap();
    let eventgroup = EventgroupId::new(0x99).unwrap();

    // Server
    let mut server = Server::new(io_server, ServerConfig {}).unwrap();
    let config = ServiceConfig::builder()
        .service(service_id)
        .instance(ConcreteInstanceId::new(0x01).unwrap())
        .eventgroup(eventgroup)
        .build()
        .unwrap();
    let mut offering = server.offer(config).unwrap();

    // Client tries to subscribe
    let mut client = Client::new(io_client, ClientConfig::builder().build().unwrap()).unwrap();
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));

    let available = proxy.wait_available().unwrap();
    
    // Subscription will eventually fail
    let sub_result = std::thread::scope(|s| {
        let handle = s.spawn(|| available.subscribe(eventgroup));
        
        network.advance(std::time::Duration::from_millis(50));
        
        // Server rejects
        if let Some(ServiceEvent::Subscribe { ack, .. }) = offering.try_next().unwrap() {
            ack.reject(RejectReason::NotAuthorized).unwrap();
        }
        
        network.advance(std::time::Duration::from_millis(50));
        
        handle.join().unwrap()
    });
    
    // Client should see the rejection
    assert!(sub_result.is_err());
}

// ============================================================================
// SIMULATED NETWORK TESTS
// ============================================================================

#[test]
fn simulated_network_can_drop_packets() {
    let (network, io_a, io_b) = SimulatedNetwork::new_pair();
    
    network.set_drop_rate(0.5); // 50% packet loss
    
    let service_id = ServiceId::new(0x7777).unwrap();
    
    // Start server
    let mut server = Server::new(io_b, ServerConfig {}).unwrap();
    let config = ServiceConfig::builder()
        .service(service_id)
        .instance(ConcreteInstanceId::new(0x01).unwrap())
        .build()
        .unwrap();
    let _offering = server.offer(config).unwrap();

    // Client - discovery should still work due to retries
    let mut client = Client::new(io_a, ClientConfig::builder().build().unwrap()).unwrap();
    let proxy = client.require(service_id, InstanceId::ANY);
    
    // Need more time with packet loss
    network.advance(std::time::Duration::from_secs(5));
    
    // Should eventually succeed despite losses
    assert!(proxy.is_available());
}

#[test]
fn simulated_network_can_partition() {
    let (network, io_a, io_b) = SimulatedNetwork::new_pair();
    
    let service_id = ServiceId::new(0x8888).unwrap();
    
    // Establish connection
    let mut server = Server::new(io_b, ServerConfig {}).unwrap();
    let config = ServiceConfig::builder()
        .service(service_id)
        .instance(ConcreteInstanceId::new(0x01).unwrap())
        .build()
        .unwrap();
    let _offering = server.offer(config).unwrap();

    let mut client = Client::new(io_a, ClientConfig::builder().build().unwrap()).unwrap();
    let proxy = client.require(service_id, InstanceId::ANY);
    
    network.advance(std::time::Duration::from_millis(100));
    assert!(proxy.is_available());
    
    // Create network partition
    network.partition("192.168.1.0/24", "192.168.2.0/24");
    network.advance(std::time::Duration::from_secs(10));
    
    // Service should become unavailable due to SD timeout
    assert!(!proxy.is_available());
    
    // Heal partition
    network.heal();
    network.advance(std::time::Duration::from_millis(500));
    
    // Should rediscover
    assert!(proxy.is_available());
}

#[test]
fn simulated_network_tracks_history() {
    let (network, io_a, io_b) = SimulatedNetwork::new_pair();
    
    let service_id = ServiceId::new(0x9999).unwrap();
    
    // Some activity
    let mut server = Server::new(io_b, ServerConfig {}).unwrap();
    let config = ServiceConfig::builder()
        .service(service_id)
        .instance(ConcreteInstanceId::new(0x01).unwrap())
        .build()
        .unwrap();
    let _offering = server.offer(config).unwrap();

    network.advance(std::time::Duration::from_millis(100));

    // Check history
    let history = network.history();
    
    // Should see SD messages
    assert!(history.iter().any(|e| matches!(e, NetworkEvent::UdpSent { dst_port: 30490, .. })));
}
