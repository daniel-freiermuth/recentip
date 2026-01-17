//! Multi-Party Compliance Tests
//!
//! Integration tests for scenarios with multiple participants using turmoil.
//!
//! Key requirements tested:
//! - feat_req_recentip_354: Multiple subscribers on same ECU
//! - feat_req_recentip_636: Multiple instances of same service
//! - feat_req_recentip_804: Event delivery to multiple subscribers
//! - feat_req_recentipsd_109: SD multicast reaches all participants

use recentip::handle::ServiceEvent;
use recentip::prelude::*;
use recentip::Runtime;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

/// Type alias for turmoil-based runtime

const TEST_SERVICE_ID: u16 = 0x1234;
const TEST_SERVICE_VERSION: (u8, u32) = (1, 0);

/// Another service for multi-service tests
const SERVICE_A_ID: u16 = 0x1001;
const SERVICE_A_VERSION: (u8, u32) = (1, 0);
const SERVICE_B_ID: u16 = 0x1002;
const SERVICE_B_VERSION: (u8, u32) = (1, 0);

// ============================================================================
// MULTIPLE CLIENTS TO ONE SERVER
// ============================================================================

/// [feat_req_recentip_103] Multiple clients can call methods on the same server
///
/// Three clients connect to one server and make concurrent RPC calls.
/// All clients should receive correct responses.
#[test_log::test]
fn multiple_clients_call_same_server() {
    covers!(feat_req_recentip_103);

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server handles requests from multiple clients
    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let runtime = recentip::configure().start_turmoil().await.unwrap();

            let mut offering = runtime
                .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
                .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            // Handle 3 requests from different clients
            let mut handled = 0;
            while handled < 3 {
                if let Some(event) = tokio::time::timeout(Duration::from_secs(10), offering.next())
                    .await
                    .ok()
                    .flatten()
                {
                    if let ServiceEvent::Call {
                        payload, responder, ..
                    } = event
                    {
                        // Echo back with prefix
                        let response = format!("response_to_{}", String::from_utf8_lossy(&payload));
                        responder.reply(response.as_bytes()).await.unwrap();
                        handled += 1;
                    }
                }
            }
            *flag.lock().unwrap() = true;

            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(())
        }
    });

    // Client 1
    sim.host("client1", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client1").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(0x0001).unwrap(), b"client1"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");

        assert_eq!(response.payload.as_ref(), b"response_to_client1");
        Ok(())
    });

    // Client 2
    sim.host("client2", || async {
        tokio::time::sleep(Duration::from_millis(150)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client2").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(0x0001).unwrap(), b"client2"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");

        assert_eq!(response.payload.as_ref(), b"response_to_client2");
        Ok(())
    });

    // Client 3
    sim.host("client3", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client3").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(0x0001).unwrap(), b"client3"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");

        assert_eq!(response.payload.as_ref(), b"response_to_client3");
        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// MULTIPLE SUBSCRIBERS TO EVENTS
// ============================================================================

/// [feat_req_recentip_354, feat_req_recentip_804] Multiple clients subscribing to events
///
/// Three clients subscribe to the same eventgroup. When the server sends an event,
/// all three subscribers should receive it.
#[test_log::test]
fn multiple_clients_subscribe_to_events() {
    covers!(feat_req_recentip_354, feat_req_recentip_804);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    // Server sends event to all subscribers
    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let runtime = recentip::configure().start_turmoil().await.unwrap();

            let offering = runtime
                .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
                .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            // Wait for all 3 clients to subscribe
            tokio::time::sleep(Duration::from_millis(800)).await;

            // Send event
            let eventgroup = EventgroupId::new(0x0001).unwrap();
            let event_id = EventId::new(0x8001).unwrap();
            offering
                .event(event_id)
                .eventgroup(eventgroup)
                .create()
                .await
                .unwrap()
                .notify(b"broadcast_event")
                .await
                .unwrap();

            tokio::time::sleep(Duration::from_millis(500)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    // Subscriber 1
    sim.host("subscriber1", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("subscriber1").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        // Wait for event
        let event = tokio::time::timeout(Duration::from_secs(10), subscription.next())
            .await
            .expect("Event timeout");

        assert!(event.is_some(), "Subscriber 1 should receive event");

        Ok(())
    });

    // Subscriber 2
    sim.host("subscriber2", || async {
        tokio::time::sleep(Duration::from_millis(150)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("subscriber2").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        let event = tokio::time::timeout(Duration::from_secs(10), subscription.next())
            .await
            .expect("Event timeout");

        assert!(event.is_some(), "Subscriber 2 should receive event");

        Ok(())
    });

    // Subscriber 3
    sim.host("subscriber3", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("subscriber3").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        let event = tokio::time::timeout(Duration::from_secs(10), subscription.next())
            .await
            .expect("Event timeout");

        assert!(event.is_some(), "Subscriber 3 should receive event");

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(2000)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// SERVICE DISCOVERY REACHES ALL PARTICIPANTS
// ============================================================================

/// [feat_req_recentipsd_109] SD announcements reach all participants
///
/// When a server offers a service, all clients on the network should be able
/// to discover it via SD multicast.
#[test_log::test]
fn sd_reaches_all_participants() {
    covers!(feat_req_recentipsd_109);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    // Server offers service
    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let runtime = recentip::configure().start_turmoil().await.unwrap();

            let _offering = runtime
                .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
                .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            // Keep offering alive
            tokio::time::sleep(Duration::from_secs(10)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    // 5 clients try to discover (using macro to generate inline)
    sim.host("client1", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client1").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();
        let proxy = runtime.find(TEST_SERVICE_ID);
        tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Should not time out")
            .expect("Client 1 should discover service");
        Ok(())
    });

    sim.host("client2", || async {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client2").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();
        let proxy = runtime.find(TEST_SERVICE_ID);
        tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Should not time out")
            .expect("Client 2 should discover service");
        Ok(())
    });

    sim.host("client3", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client3").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();
        let proxy = runtime.find(TEST_SERVICE_ID);
        tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Should not time out")
            .expect("Client 3 should discover service");
        Ok(())
    });

    sim.host("client4", || async {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client4").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();
        let proxy = runtime.find(TEST_SERVICE_ID);
        tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Should not time out")
            .expect("Client 4 should discover service");
        Ok(())
    });

    sim.host("client5", || async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client5").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();
        let proxy = runtime.find(TEST_SERVICE_ID);
        tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Should not time out")
            .expect("Client 5 should discover service");
        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_secs(11)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// MIXED CLIENT/SERVER ROLES
// ============================================================================

/// [feat_req_recentip_103] Nodes can be both client and server
///
/// Node A offers ServiceA and requires ServiceB.
/// Node B offers ServiceB and requires ServiceA.
/// Both should successfully discover each other and communicate.
#[test_log::test]
fn nodes_with_mixed_client_server_roles() {
    covers!(feat_req_recentip_103);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    // Node A: offers ServiceA, requires ServiceB
    sim.host("nodeA", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let runtime = recentip::configure().start_turmoil().await.unwrap();

            // Offer ServiceA
            let mut offering = runtime
                .offer(SERVICE_A_ID, InstanceId::Id(0x0001))
                .version(SERVICE_A_VERSION.0, SERVICE_A_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            // Also require ServiceB
            let proxy_b = runtime.find(SERVICE_B_ID);
            let proxy_b = tokio::time::timeout(Duration::from_secs(5), proxy_b)
                .await
                .expect("Should discover ServiceB")
                .expect("Service available");

            // Call ServiceB
            let response = tokio::time::timeout(
                Duration::from_secs(5),
                proxy_b.call(MethodId::new(0x0001).unwrap(), b"from_nodeA"),
            )
            .await
            .expect("Timeout")
            .expect("Call should succeed");

            assert_eq!(response.payload.as_ref(), b"response_from_nodeB");

            // Handle incoming call from nodeB
            if let Some(event) = tokio::time::timeout(Duration::from_secs(5), offering.next())
                .await
                .ok()
                .flatten()
            {
                if let ServiceEvent::Call { responder, .. } = event {
                    responder.reply(b"response_from_nodeA").await.unwrap();
                }
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    // Node B: offers ServiceB, requires ServiceA
    sim.host("nodeB", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("nodeB").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Offer ServiceB
        let mut offering = runtime
            .offer(SERVICE_B_ID, InstanceId::Id(0x0001))
            .version(SERVICE_B_VERSION.0, SERVICE_B_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Handle incoming call from nodeA first
        if let Some(event) = tokio::time::timeout(Duration::from_secs(5), offering.next())
            .await
            .ok()
            .flatten()
        {
            if let ServiceEvent::Call { responder, .. } = event {
                responder.reply(b"response_from_nodeB").await.unwrap();
            }
        }

        // Then require ServiceA
        let proxy_a = runtime.find(SERVICE_A_ID);
        let proxy_a = tokio::time::timeout(Duration::from_secs(5), proxy_a)
            .await
            .expect("Should discover ServiceA")
            .expect("Service available");

        // Call ServiceA
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_a.call(MethodId::new(0x0001).unwrap(), b"from_nodeB"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");

        assert_eq!(response.payload.as_ref(), b"response_from_nodeA");

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// MULTIPLE SERVERS WITH DIFFERENT INSTANCES
// ============================================================================

/// [feat_req_recentip_636] Multiple servers offering different instances
///
/// Two servers offer the same service with different instance IDs.
/// Client should be able to discover and call both instances.
#[test_log::test]
fn multiple_servers_different_instances() {
    covers!(feat_req_recentip_636);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    // Server 1 - Instance 0x0001
    sim.host("server1", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let runtime = recentip::configure().start_turmoil().await.unwrap();

            let mut offering = runtime
                .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
                .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            // Handle one request
            if let Some(event) = tokio::time::timeout(Duration::from_secs(10), offering.next())
                .await
                .ok()
                .flatten()
            {
                if let ServiceEvent::Call { responder, .. } = event {
                    responder.reply(b"from_instance_1").await.unwrap();
                }
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    // Server 2 - Instance 0x0002
    sim.host("server2", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server2").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0002))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Handle one request
        if let Some(event) = tokio::time::timeout(Duration::from_secs(10), offering.next())
            .await
            .ok()
            .flatten()
        {
            if let ServiceEvent::Call { responder, .. } = event {
                responder.reply(b"from_instance_2").await.unwrap();
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    // Client calls both instances
    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Find instance 1
        let proxy1 = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy1 = tokio::time::timeout(Duration::from_secs(5), proxy1)
            .await
            .expect("Should discover instance 1")
            .expect("Service available");

        let response1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy1.call(MethodId::new(0x0001).unwrap(), b"hello"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");

        assert_eq!(response1.payload.as_ref(), b"from_instance_1");

        // Find instance 2
        let proxy2 = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0002));
        let proxy2 = tokio::time::timeout(Duration::from_secs(5), proxy2)
            .await
            .expect("Should discover instance 2")
            .expect("Service available");

        let response2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy2.call(MethodId::new(0x0001).unwrap(), b"hello"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");

        assert_eq!(response2.payload.as_ref(), b"from_instance_2");

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}
