//! Multi-Party Compliance Tests
//!
//! Tests scenarios involving more than two participants:
//! - Multiple clients connecting to one server
//! - Multiple servers offering different instances
//! - Service discovery with many participants
//! - Multicast event delivery to multiple subscribers
//! - Failover when one server goes down
//!
//! Key requirements tested:
//! - feat_req_recentip_354: Multiple subscribers on same ECU
//! - feat_req_recentip_636: Multiple instances of same service
//! - feat_req_recentip_804: Multicast event delivery
//! - feat_req_recentipsd_109: SD multicast announcement

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

fn find_notifications(network: &SimulatedNetwork) -> Vec<(Header, Vec<u8>)> {
    network
        .history()
        .iter()
        .filter_map(|event| {
            if let NetworkEvent::UdpSent { data, to, .. } = event {
                if to.port() != 30490 && data.len() >= 16 {
                    if let Some(header) = Header::from_bytes(data) {
                        if header.message_type == 0x02 {
                            let payload = data[16..].to_vec();
                            return Some((header, payload));
                        }
                    }
                }
            }
            None
        })
        .collect()
}

fn count_sd_messages_to(network: &SimulatedNetwork, addr: std::net::SocketAddr) -> usize {
    network
        .history()
        .iter()
        .filter(|event| {
            if let NetworkEvent::UdpSent { to, dst_port, .. } = event {
                *to == addr || *dst_port == 30490
            } else {
                false
            }
        })
        .count()
}

// ============================================================================
// Multiple Clients to One Server
// ============================================================================

/// Multiple clients can call methods on the same server concurrently
#[test]
#[ignore = "Runtime::new not implemented"]
fn multiple_clients_call_same_server() {
    covers!(feat_req_recentip_103);

    let (network, mut contexts) = SimulatedNetwork::new_multi(4);
    let io_server = contexts.remove(0);
    // contexts now has 3 clients

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

    // Create 3 clients
    let mut clients: Vec<_> = contexts
        .into_iter()
        .map(|io| Runtime::new(io, RuntimeConfig::default()).unwrap())
        .collect();

    // All clients require the service and make calls
    let mut pending_responses = Vec::new();
    for (i, client) in clients.iter_mut().enumerate() {
        let proxy = client.require(service_id, InstanceId::ANY);
        network.advance(std::time::Duration::from_millis(50));
        let available = proxy.wait_available().unwrap();

        let payload = format!("from_client_{}", i);
        let pending = available.call(method_id, payload.as_bytes()).unwrap();
        pending_responses.push((i, pending));
        network.advance(std::time::Duration::from_millis(10));
    }

    // Server handles all requests
    let mut handled = 0;
    for _ in 0..10 {
        if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
            let response = format!("response_{}", handled);
            request.responder.send_ok(response.as_bytes()).unwrap();
            handled += 1;
        }
        network.advance(std::time::Duration::from_millis(10));
    }

    assert_eq!(handled, 3, "Server should handle requests from all 3 clients");

    // All clients should get responses
    for (i, pending) in pending_responses {
        let response = pending.wait().unwrap();
        assert_eq!(response.return_code, ReturnCode::Ok);
        // Response payload verification could be more specific
    }
}

/// Multiple clients subscribing to events all receive notifications
#[test]
#[ignore = "Runtime::new not implemented"]
fn multiple_clients_subscribe_to_events() {
    covers!(feat_req_recentip_354, feat_req_recentip_804);

    let (network, mut contexts) = SimulatedNetwork::new_multi(5);
    let io_server = contexts.remove(0);
    // 4 clients

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

    // Create clients and subscriptions
    let mut subscriptions = Vec::new();
    for io in contexts {
        let mut client = Runtime::new(io, RuntimeConfig::default()).unwrap();
        let proxy = client.require(service_id, InstanceId::ANY);
        network.advance(std::time::Duration::from_millis(50));
        let available = proxy.wait_available().unwrap();
        let subscription = available.subscribe(eventgroup_id).unwrap();
        subscriptions.push(subscription);
        network.advance(std::time::Duration::from_millis(50));
    }

    network.clear_history();

    // Server sends one event
    offering.notify(eventgroup_id, event_id, b"broadcast").unwrap();
    network.advance(std::time::Duration::from_millis(50));

    // All 4 subscribers should receive it
    let mut received_count = 0;
    for sub in &mut subscriptions {
        if let Some(_event) = sub.try_next_event().unwrap() {
            received_count += 1;
        }
    }

    assert_eq!(
        received_count, 4,
        "All 4 subscribers should receive the event"
    );
}

// ============================================================================
// Multiple Servers
// ============================================================================

/// Multiple servers offering different instances of the same service
#[test]
#[ignore = "Runtime::new not implemented"]
fn multiple_servers_different_instances() {
    covers!(feat_req_recentip_636, feat_req_recentip_541);

    let (network, mut contexts) = SimulatedNetwork::new_multi(4);
    let io_client = contexts.remove(0);
    // 3 servers

    let service_id = ServiceId::new(0x1234).unwrap();
    let method_id = MethodId::new(0x0001);

    // Create 3 servers with different instance IDs
    let mut offerings = Vec::new();
    for (i, io) in contexts.into_iter().enumerate() {
        let mut server = Runtime::new(io, RuntimeConfig::default()).unwrap();
        let instance_id = ConcreteInstanceId::new((i + 1) as u16).unwrap();

        let config = ServiceConfig::builder()
            .service(service_id)
            .instance(instance_id)
            .build()
            .unwrap();

        let offering = server.offer(config).unwrap();
        offerings.push((instance_id, server, offering));
    }
    network.advance(std::time::Duration::from_millis(200));

    // Client using ANY should discover one of them
    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));

    // Should see all available instances
    let instances = proxy.available_instances();
    assert_eq!(
        instances.len(),
        3,
        "Client should discover all 3 instances"
    );

    // Can call each specific instance
    for (instance_id, _server, _offering) in &offerings {
        let specific_proxy = client.require(service_id, InstanceId::from(*instance_id));
        network.advance(std::time::Duration::from_millis(50));
        let available = specific_proxy.wait_available().unwrap();

        let _pending = available.call(method_id, b"ping").unwrap();
        network.advance(std::time::Duration::from_millis(10));
    }
}

/// Client failover when one server goes down
#[test]
#[ignore = "Runtime::new not implemented"]
fn client_failover_to_another_instance() {
    covers!(feat_req_recentip_636);

    let (network, mut contexts) = SimulatedNetwork::new_multi(3);
    let io_client = contexts.remove(0);
    let io_server1 = contexts.remove(0);
    let io_server2 = contexts.remove(0);

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_1 = ConcreteInstanceId::new(1).unwrap();
    let instance_2 = ConcreteInstanceId::new(2).unwrap();
    let method_id = MethodId::new(0x0001);

    // Start both servers
    let mut server1 = Runtime::new(io_server1, RuntimeConfig::default()).unwrap();
    let mut server2 = Runtime::new(io_server2, RuntimeConfig::default()).unwrap();

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

    let mut offering1 = server1.offer(config1).unwrap();
    let _offering2 = server2.offer(config2).unwrap();
    network.advance(std::time::Duration::from_millis(200));

    // Client connects using ANY
    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let available = proxy.wait_available().unwrap();

    // First call succeeds
    let pending = available.call(method_id, b"test1").unwrap();
    network.advance(std::time::Duration::from_millis(10));

    if let Some(ServiceEvent::MethodCall { request }) = offering1.try_next().ok().flatten() {
        request.responder.send_ok(b"ok").unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));
    let response = pending.wait().unwrap();
    assert_eq!(response.return_code, ReturnCode::Ok);

    // Server 1 goes down
    drop(offering1);
    drop(server1);
    network.advance(std::time::Duration::from_millis(500)); // Wait for TTL expiry

    // Client should detect unavailability and switch to server 2
    // (implementation detail: may need to re-discover or have automatic failover)
}

// ============================================================================
// Service Discovery with Many Participants
// ============================================================================

/// SD announcements reach all participants
#[test]
#[ignore = "Runtime::new not implemented"]
fn sd_reaches_all_participants() {
    covers!(feat_req_recentipsd_109);

    let (network, mut contexts) = SimulatedNetwork::new_multi(6);
    let io_server = contexts.remove(0);
    // 5 clients

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    // Server offers service
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();
    let config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // All 5 clients should be able to discover the service
    let mut discovered = 0;
    for io in contexts {
        let mut client = Runtime::new(io, RuntimeConfig::default()).unwrap();
        let proxy = client.require(service_id, InstanceId::ANY);
        network.advance(std::time::Duration::from_millis(100));

        if proxy.is_available() {
            discovered += 1;
        }
    }

    assert_eq!(
        discovered, 5,
        "All 5 clients should discover the service via SD"
    );
}

// ============================================================================
// Complex Topology Tests
// ============================================================================

/// Mixed client/server roles - some nodes are both
#[test]
#[ignore = "Runtime::new not implemented"]
fn nodes_with_mixed_client_server_roles() {
    covers!(feat_req_recentip_103);

    let (network, contexts) = SimulatedNetwork::new_multi(3);

    let service_a = ServiceId::new(0x1001).unwrap();
    let service_b = ServiceId::new(0x1002).unwrap();
    let service_c = ServiceId::new(0x1003).unwrap();

    // Node 0: offers A, requires B
    // Node 1: offers B, requires C
    // Node 2: offers C, requires A
    // Forms a ring of dependencies

    let mut runtimes: Vec<_> = contexts
        .into_iter()
        .map(|io| Runtime::new(io, RuntimeConfig::default()).unwrap())
        .collect();

    let instance_id = ConcreteInstanceId::new(1).unwrap();

    // Set up offerings
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
    let config_c = ServiceConfig::builder()
        .service(service_c)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering_a = runtimes[0].offer(config_a).unwrap();
    let _offering_b = runtimes[1].offer(config_b).unwrap();
    let _offering_c = runtimes[2].offer(config_c).unwrap();
    network.advance(std::time::Duration::from_millis(200));

    // Set up requirements
    let proxy_b = runtimes[0].require(service_b, InstanceId::ANY);
    let proxy_c = runtimes[1].require(service_c, InstanceId::ANY);
    let proxy_a = runtimes[2].require(service_a, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(200));

    // All should find their required services
    assert!(proxy_a.is_available(), "Node 2 should find service A");
    assert!(proxy_b.is_available(), "Node 0 should find service B");
    assert!(proxy_c.is_available(), "Node 1 should find service C");
}

/// Network partition isolates some nodes
#[test]
#[ignore = "Runtime::new not implemented"]
fn network_partition_isolates_nodes() {
    covers!(feat_req_recentipsd_109);

    let (network, mut contexts) = SimulatedNetwork::new_multi(4);
    let io_server = contexts.remove(0); // 192.168.1.10
    // clients: .11, .12, .13

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();
    let config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Create partition: .13 is isolated from the server (.10)
    network.partition("192.168.1.10/32", "192.168.1.13/32");

    // Clients .11 and .12 should discover, .13 should not
    let mut results = Vec::new();
    for (i, io) in contexts.into_iter().enumerate() {
        let mut client = Runtime::new(io, RuntimeConfig::default()).unwrap();
        let proxy = client.require(service_id, InstanceId::ANY);
        network.advance(std::time::Duration::from_millis(200));

        results.push((i, proxy.is_available()));
    }

    // .11 and .12 should see service, .13 should not
    assert!(results[0].1, "192.168.1.11 should discover service");
    assert!(results[1].1, "192.168.1.12 should discover service");
    assert!(!results[2].1, "192.168.1.13 should NOT discover (partitioned)");

    // Heal partition
    network.heal();
    network.advance(std::time::Duration::from_millis(500));

    // Now .13 should eventually discover (via new SD cycle)
}

// ============================================================================
// Stress / Scale Tests
// ============================================================================

/// Many clients making concurrent requests
#[test]
#[ignore = "Runtime::new not implemented"]
fn stress_many_concurrent_clients() {
    covers!(feat_req_recentip_103);

    let num_clients = 20;
    let (network, mut contexts) = SimulatedNetwork::new_multi(num_clients + 1);
    let io_server = contexts.remove(0);

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let method_id = MethodId::new(0x0001);

    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();
    let config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let mut offering = server.offer(config).unwrap();
    network.advance(std::time::Duration::from_millis(200));

    // All clients connect and make a call
    let mut pending_list = Vec::new();
    for io in contexts {
        let mut client = Runtime::new(io, RuntimeConfig::default()).unwrap();
        let proxy = client.require(service_id, InstanceId::ANY);
        network.advance(std::time::Duration::from_millis(10));
        let available = proxy.wait_available().unwrap();
        let pending = available.call(method_id, b"x").unwrap();
        pending_list.push(pending);
    }
    network.advance(std::time::Duration::from_millis(100));

    // Server handles all
    let mut handled = 0;
    for _ in 0..100 {
        if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
            request.responder.send_ok(b"ok").unwrap();
            handled += 1;
        }
        network.advance(std::time::Duration::from_millis(5));
        if handled >= num_clients {
            break;
        }
    }

    assert_eq!(
        handled, num_clients,
        "Server should handle all {} requests",
        num_clients
    );
}
