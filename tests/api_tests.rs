//! Basic API tests using turmoil for network simulation.

use someip_runtime::handle::ServiceEvent;
use someip_runtime::runtime::Runtime;
use someip_runtime::{
    EventId, EventgroupId, InstanceId, MethodId, RuntimeConfig, ServiceId,
};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Type alias for turmoil-based runtime for convenience
type TurmoilRuntime =
    Runtime<turmoil::net::UdpSocket, turmoil::net::TcpStream, turmoil::net::TcpListener>;

// Wire values for TestService
const TEST_SERVICE_ID: u16 = 0x1234;
const TEST_SERVICE_VERSION: (u8, u32) = (1, 0);

// Wire values for TestServiceNew (same service ID, different version)
const TEST_SERVICE_NEW_VERSION: (u8, u32) = (2, 0);

// Wire values for AnotherService
const ANOTHER_SERVICE_ID: u16 = 0x5678;
const ANOTHER_SERVICE_VERSION: (u8, u32) = (2, 1);

#[test_log::test]
fn test_runtime_creation() {
    let mut sim = turmoil::Builder::new().build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: Result<TurmoilRuntime, _> = Runtime::with_socket_type(config).await;

        assert!(runtime.is_ok(), "Runtime should be created successfully");

        Ok(())
    });

    sim.run().unwrap();
}
#[test_log::test]
fn test_find_service() {
    let mut sim = turmoil::Builder::new().build();

    sim.host("client", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Try to find a service that doesn't exist - should fail
        let result = runtime.find(TEST_SERVICE_ID).await;

        // Should fail since no server is offering this service
        assert!(result.is_err());

        Ok(())
    });

    sim.run().unwrap();
}

#[test_log::test]
fn test_offer_service() {
    let mut sim = turmoil::Builder::new().build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer a service
        let offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001)).version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await;

        assert!(offering.is_ok(), "Offering should succeed");

        let offering = offering.unwrap();
        assert_eq!(offering.service_id(), ServiceId::new(0x1234).unwrap());
        assert_eq!(offering.instance_id(), InstanceId::Id(0x0001));

        Ok(())
    });

    sim.run().unwrap();
}

#[test_log::test]
fn test_instance_id() {
    // Test InstanceId::Any
    assert!(InstanceId::Any.is_any());
    assert_eq!(InstanceId::Any.value(), 0xFFFF);

    // Test InstanceId::Id
    let id = InstanceId::Id(42);
    assert!(!id.is_any());
    assert_eq!(id.value(), 42);

    // Test from raw values
    assert_eq!(InstanceId::new(0xFFFF), Some(InstanceId::Any));
    assert_eq!(InstanceId::new(0x0001), Some(InstanceId::Id(0x0001)));
    assert_eq!(InstanceId::new(0x0000), None); // Reserved
}

#[test_log::test]
fn test_service_id() {
    // Valid IDs
    assert!(ServiceId::new(0x0001).is_some());
    assert!(ServiceId::new(0xFFFE).is_some());

    // Reserved IDs
    assert!(ServiceId::new(0x0000).is_none());
    assert!(ServiceId::new(0xFFFF).is_none());
}

// ============================================================================
// SERVICE DISCOVERY INTEGRATION TESTS
// ============================================================================

#[test_log::test]
fn test_service_discovery_offer_find() {
    // Test that a client can discover a server's offered service
    let mut sim = turmoil::Builder::new().build();

    // Server offers a service
    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001)).version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Keep the server running for a bit so the client can discover it
        tokio::time::sleep(Duration::from_millis(500)).await;

        Ok(())
    });

    // Client finds the service
    sim.host("client", || async {
        // Give server time to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);

        // Wait for discovery (with timeout)
        let result = tokio::time::timeout(Duration::from_millis(300), proxy).await;

        assert!(
            result.is_ok(),
            "Service should be discovered within timeout"
        );
        let available_proxy = result.unwrap().expect("Service available");
        assert_eq!(
            available_proxy.service_id(),
            ServiceId::new(0x1234).unwrap()
        );

        Ok(())
    });

    sim.run().unwrap();
}

#[test_log::test]
fn test_multiple_services() {
    // Test offering and finding multiple services
    let mut sim = turmoil::Builder::new().build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer two different services
        let _offering1 = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001)).version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();
        let _offering2 = runtime
            .offer(ANOTHER_SERVICE_ID, InstanceId::Id(0x0002)).version(ANOTHER_SERVICE_VERSION.0, ANOTHER_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Find both services
        let proxy1 = runtime.find(TEST_SERVICE_ID);
        let proxy2 = runtime.find(ANOTHER_SERVICE_ID);

        // Wait for both to be discovered
        let result1 = tokio::time::timeout(Duration::from_millis(300), proxy1).await;
        let result2 = tokio::time::timeout(Duration::from_millis(300), proxy2).await;

        assert!(result1.is_ok(), "TestService should be discovered");
        assert!(result2.is_ok(), "AnotherService should be discovered");

        Ok(())
    });

    sim.run().unwrap();
}

#[test_log::test]
fn test_specific_instance_id() {
    // Test finding a specific instance ID
    let mut sim = turmoil::Builder::new().build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer instance 0x0001
        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001)).version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Find specific instance
        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));

        let result = tokio::time::timeout(Duration::from_millis(300), proxy).await;

        assert!(result.is_ok(), "Specific instance should be discovered");
        let available = result.unwrap().expect("Service available");
        // Note: After discovery, the instance ID might be updated to the actual ID
        assert_eq!(available.service_id(), ServiceId::new(0x1234).unwrap());

        Ok(())
    });

    sim.run().unwrap();
}

#[test_log::test]
fn test_offering_handle_drop() {
    // Test that dropping an offering handle sends StopOffer
    let mut sim = turmoil::Builder::new().build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        {
            let _offering = runtime
                .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001)).version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            // offering dropped here
        }

        // Brief pause to let StopOffer propagate
        tokio::time::sleep(Duration::from_millis(50)).await;

        Ok(())
    });

    sim.run().unwrap();
}

#[test_log::test]
fn test_method_call_rpc() {
    // Test full RPC round-trip: client calls method, server responds
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer the service
        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001)).version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for and handle one method call
        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::Call {
                    method,
                    payload,
                    responder,
                    ..
                } => {
                    // Echo the payload back with method ID prepended
                    let mut response = vec![method.value() as u8];
                    response.extend_from_slice(&payload);
                    responder.reply(&response).await.unwrap();
                }
                _ => panic!("Expected Call event"),
            }
        }

        Ok(())
    });

    sim.host("client", || async {
        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Find the service
        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));

        // Wait for service to become available
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout waiting for service")
            .expect("Service available");

        // Call a method
        let method = MethodId::new(0x0042).unwrap();
        let payload = b"hello";
        let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, payload))
            .await
            .expect("Timeout waiting for response")
            .expect("Call should succeed");

        // Verify response
        assert_eq!(response.payload[0], 0x42); // method ID
        assert_eq!(&response.payload[1..], b"hello"); // echoed payload

        Ok(())
    });

    sim.run().unwrap();
}

const PUBSUB_SERVICE_ID: u16 = 0x1234;
const PUBSUB_SERVICE_VERSION: (u8, u32) = (1, 0);

/// Library auto-renewal test: Events continue beyond initial TTL.
#[test_log::test]
fn library_auto_renews_subscription() {
    let events_received = Arc::new(Mutex::new(Vec::<(Duration, String)>::new()));
    let events_clone = Arc::clone(&events_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let offering = runtime
            .offer(PUBSUB_SERVICE_ID, InstanceId::Id(0x0001)).version(PUBSUB_SERVICE_VERSION.0, PUBSUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();

        let mut i = 0;
        loop {
            offering
                .notify(eventgroup, event_id, format!("event{}", i).as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(500)).await;
            i += 1;
        }
    });

    sim.client("client", async move {
        let start = tokio::time::Instant::now();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .subscribe_ttl(2)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();

        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUBSUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription = proxy.subscribe(eventgroup).await.unwrap();

        // Collect events with timestamps
        let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(200), subscription.next()).await {
                Ok(Some(event)) => {
                    let elapsed = start.elapsed();
                    let payload = String::from_utf8_lossy(&event.payload).to_string();
                    eprintln!("Received {} at {:?}", payload, elapsed);
                    events_clone.lock().unwrap().push((elapsed, payload));
                }
                Ok(None) => break,
                Err(_) => continue,
            }
        }
        Ok(())
    });

    sim.run().unwrap();

    let events = events_received.lock().unwrap();

    // Should receive events throughout the 5s window, not just first 2s
    assert!(
        events.len() >= 10,
        "Should receive more than 9 events due to auto-renewal (got {})",
        events.len()
    );

    // Verify we received events AFTER the initial 2s TTL would have expired
    let events_after_ttl = events
        .iter()
        .filter(|(elapsed, _)| *elapsed > Duration::from_secs(2))
        .count();
    assert!(
        events_after_ttl >= 3,
        "Should receive events after initial TTL expires (auto-renewal). Got {} events after 2s",
        events_after_ttl
    );
}

#[test_log::test]
fn test_event_subscription() {
    // Test event subscription: client subscribes, server sends events
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer the service
        let offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001)).version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for client to discover and subscribe
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send some events
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();

        offering
            .notify(eventgroup, event_id, b"event1")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        offering
            .notify(eventgroup, event_id, b"event2")
            .await
            .unwrap();

        // Keep server alive for a bit
        tokio::time::sleep(Duration::from_millis(500)).await;

        Ok(())
    });

    sim.host("client", || async {
        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Find the service
        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));

        // Wait for service to become available
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout waiting for service")
            .expect("Service available");

        // Subscribe to eventgroup
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Timeout subscribing")
                .expect("Subscribe should succeed");

        // Wait for events
        let event1 = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("Timeout waiting for event1");

        assert!(event1.is_some(), "Should receive first event");
        let event1 = event1.unwrap();
        assert_eq!(event1.event_id.value(), 0x8001);
        assert_eq!(&event1.payload[..], b"event1");

        let event2 = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("Timeout waiting for event2");

        assert!(event2.is_some(), "Should receive second event");
        let event2 = event2.unwrap();
        assert_eq!(&event2.payload[..], b"event2");

        Ok(())
    });

    sim.run().unwrap();
}

/// Test that subscribe() returns an error when the server responds with SubscribeEventgroupNack.
///
/// Per SOME/IP-SD specification, when a server rejects a subscription it sends a NACK
/// (SubscribeEventgroupAck with TTL=0 or SubscribeEventgroupNack type 0x07 with TTL=0).
/// The client should propagate this rejection to the caller.
#[test_log::test]
fn subscribe_returns_error_on_nack() {
    use someip_runtime::EventgroupId;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Raw server that sends NACK for all subscriptions
    sim.host("server", || async {
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server").to_string().parse().unwrap();

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let sd_multicast: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // Build offer message
        let offer = build_sd_offer(0x1234, 0x0001, 1, 0, my_ip, 30509, 3600);

        let mut buf = [0u8; 1500];

        loop {
            // Send periodic offers
            sd_socket.send_to(&offer, sd_multicast).await?;

            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // SubscribeEventgroup is type 0x06
                        if entry.entry_type as u8 == 0x06 && entry.service_id == 0x1234 {
                            // Send NACK (TTL=0)
                            let nack = build_sd_subscribe_nack(
                                entry.service_id,
                                entry.instance_id,
                                entry.major_version,
                                entry.eventgroup_id,
                            );
                            sd_socket.send_to(&nack, from).await?;
                        }
                    }
                }
            }
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime
            .find(PUBSUB_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();

        // Subscribe should return an error when server sends NACK
        let result =
            tokio::time::timeout(Duration::from_secs(2), proxy.subscribe(eventgroup)).await;

        match result {
            Ok(Ok(_)) => panic!("Subscribe should have returned an error on NACK"),
            Ok(Err(_)) => { /* Expected: subscribe failed due to NACK */ }
            Err(_) => panic!("Subscribe should not timeout waiting for NACK"),
        }

        Ok(())
    });

    sim.run().unwrap();
}

#[test_log::test]
fn test_many_version_subscribe_to_one() {
    let mut sim = turmoil::Builder::new().build();

    let sub_arrived = Arc::new(AtomicBool::new(false));

    let sub_arrived_clone = Arc::clone(&sub_arrived);
    sim.host("host", move || {
        let flag_clone = sub_arrived_clone.clone(); async move {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("host").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Macro to create an offering that panics on Subscribe
        macro_rules! unexpected_offering {
            ($runtime:expr, $version:expr) => {{
                let mut offering = $runtime
                    .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
                    .version($version, TEST_SERVICE_VERSION.1)
                    .start()
                    .await
                    .unwrap();
                async move {
                    while let Some(event) = offering.next().await {
                        match event {
                            ServiceEvent::Subscribe { .. } => {
                                panic!(
                                    "Unexpected Subscribe for TestService v{}",
                                    $version
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }};
        }

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001)).version(27, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();
        let expected_offering = async {
            while let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Subscribe { .. } => {
                        tracing::info!("Handled expected Subscribe for TestService v27");
                        flag_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    _ => {}
                }
            }
        };
        tokio::join!(
            unexpected_offering!(runtime, 1),
            unexpected_offering!(runtime, 2),
            unexpected_offering!(runtime, 3),
            unexpected_offering!(runtime, 4),
            unexpected_offering!(runtime, 5),
            unexpected_offering!(runtime, 6),
            unexpected_offering!(runtime, 7),
            unexpected_offering!(runtime, 8),
            unexpected_offering!(runtime, 9),
            unexpected_offering!(runtime, 10),
            unexpected_offering!(runtime, 11),
            unexpected_offering!(runtime, 12),
            unexpected_offering!(runtime, 13),
            unexpected_offering!(runtime, 14),
            unexpected_offering!(runtime, 15),
            unexpected_offering!(runtime, 16),
            unexpected_offering!(runtime, 17),
            unexpected_offering!(runtime, 18),
            unexpected_offering!(runtime, 19),
            unexpected_offering!(runtime, 20),
            unexpected_offering!(runtime, 21),
            unexpected_offering!(runtime, 22),
            unexpected_offering!(runtime, 23),
            unexpected_offering!(runtime, 24),
            unexpected_offering!(runtime, 25),
            unexpected_offering!(runtime, 26),
            expected_offering,
            unexpected_offering!(runtime, 28),
            unexpected_offering!(runtime, 29),
            unexpected_offering!(runtime, 30),
            unexpected_offering!(runtime, 31),
            unexpected_offering!(runtime, 32),
            unexpected_offering!(runtime, 33),
            unexpected_offering!(runtime, 34),
            unexpected_offering!(runtime, 35),
            unexpected_offering!(runtime, 36),
            unexpected_offering!(runtime, 37),
            unexpected_offering!(runtime, 38),
            unexpected_offering!(runtime, 39),
            unexpected_offering!(runtime, 40),
            unexpected_offering!(runtime, 41),
            unexpected_offering!(runtime, 42),
            unexpected_offering!(runtime, 43),
            unexpected_offering!(runtime, 44),
            unexpected_offering!(runtime, 45),
            unexpected_offering!(runtime, 46),
            unexpected_offering!(runtime, 47),
            unexpected_offering!(runtime, 48),
            unexpected_offering!(runtime, 49),
            unexpected_offering!(runtime, 50),
            unexpected_offering!(runtime, 51),
            unexpected_offering!(runtime, 52),
            unexpected_offering!(runtime, 53),
            unexpected_offering!(runtime, 54),
            unexpected_offering!(runtime, 55),
            unexpected_offering!(runtime, 56),
        );

        Ok(())

    }});

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy_v1 = runtime
            .find(TEST_SERVICE_ID)
            .major_version(27);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy_v1)
            .await
            .expect("Discovery timeout")
            .expect("Service v1 available");

        proxy
            .subscribe(EventgroupId::new(0x0001).unwrap())
            .await
            .unwrap();

        Ok(())
    });

    sim.run().unwrap();

    assert!(
        sub_arrived.load(std::sync::atomic::Ordering::SeqCst),
        "Expected subscription for version 27 did not arrive"
    );
}

#[test_log::test]
fn test_multiple_versions_subscribe_both_data() {
    // TODO: intended to show that pending subscriptions is broken, but it works
    let mut sim = turmoil::Builder::new()
    .simulation_duration(Duration::from_secs(30))
    .min_message_latency(Duration::from_millis(500))
    .max_message_latency(Duration::from_millis(800))
    .build();

    sim.host("host", move || { async move {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("host").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let offering1 = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(1, TEST_SERVICE_VERSION.1)
            .start() .await .unwrap();

        let offering2 = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(2, TEST_SERVICE_NEW_VERSION.1)
            .start() .await .unwrap();

        let mut offering3 = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(3, TEST_SERVICE_NEW_VERSION.1)
            .start() .await .unwrap();

        let offering4 = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(4, TEST_SERVICE_NEW_VERSION.1)
            .start() .await .unwrap();

        let offering5 = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(5, TEST_SERVICE_NEW_VERSION.1)
            .start() .await .unwrap();

        loop {
            offering1.notify(EventgroupId::new(1).unwrap(), EventId::new(0x8001).unwrap(), "1".as_bytes()).await.unwrap();
            offering2.notify(EventgroupId::new(1).unwrap(), EventId::new(0x8002).unwrap(), "2".as_bytes()).await.unwrap();
            offering3.notify(EventgroupId::new(1).unwrap(), EventId::new(0x8003).unwrap(), "3".as_bytes()).await.unwrap();
            offering4.notify(EventgroupId::new(1).unwrap(), EventId::new(0x8004).unwrap(), "4".as_bytes()).await.unwrap();
            offering5.notify(EventgroupId::new(1).unwrap(), EventId::new(0x8005).unwrap(), "5".as_bytes()).await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }});

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut proxy_v1 = runtime
            .find(TEST_SERVICE_ID)
            .major_version(1)
            .await.unwrap();
        let async1 = proxy_v1.subscribe(EventgroupId::new(1).unwrap());

        let mut proxy_v2 = runtime
            .find(TEST_SERVICE_ID)
            .major_version(2)
            .await.unwrap();
        let async2 = proxy_v2.subscribe(EventgroupId::new(1).unwrap());

        let mut proxy_v3 = runtime
            .find(TEST_SERVICE_ID)
            .major_version(3)
            .await.unwrap();
        let async3 = proxy_v3.subscribe(EventgroupId::new(1).unwrap());

        let mut proxy_v4 = runtime
            .find(TEST_SERVICE_ID)
            .major_version(4)
            .await.unwrap();
        let async4 = proxy_v4.subscribe(EventgroupId::new(1).unwrap());

        let mut proxy_v5 = runtime
            .find(TEST_SERVICE_ID)
            .major_version(5)
            .await.unwrap();
        let async5 = proxy_v5.subscribe(EventgroupId::new(1).unwrap());

        let (sub1, sub2, sub3, sub4, sub5) =tokio::join!(async1, async2, async3, async4, async5);
        let mut sub1 = sub1.expect("Sub 1 shuld have succeeded");
        let mut sub2 = sub2.expect("Sub 2 shuld have succeeded");
        let mut sub3 = sub3.expect("Sub 3 shuld have succeeded");
        let mut sub4 = sub4.expect("Sub 4 shuld have succeeded");
        let mut sub5 = sub5.expect("Sub 5 shuld have succeeded");

        assert_eq!(sub1.next().await.expect("Should receive event on sub 1").payload, "1".as_bytes());
        assert_eq!(sub2.next().await.expect("Should receive event on sub 2").payload, "2".as_bytes());
        assert_eq!(sub3.next().await.expect("Should receive event on sub 3").payload, "3".as_bytes());
        assert_eq!(sub4.next().await.expect("Should receive event on sub 4").payload, "4".as_bytes());
        assert_eq!(sub5.next().await.expect("Should receive event on sub 5").payload, "5".as_bytes());
        Ok(())
    });

    sim.run().unwrap();
}

#[test_log::test]
fn test_multiple_versions_subscribed_one_dropped() {
    let mut sim = turmoil::Builder::new()
    .simulation_duration(Duration::from_secs(30))
    .build();

    let sub_arrived = Arc::new(AtomicBool::new(false));
    let sub_arrived_clone = Arc::clone(&sub_arrived);

    sim.host("host", move || { let flag=sub_arrived_clone.clone(); async move {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("host").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let offering1 = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(1, TEST_SERVICE_VERSION.1)
            .start() .await .unwrap();

        let offering2 = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(2, TEST_SERVICE_NEW_VERSION.1)
            .start() .await .unwrap();

        let mut offering3 = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(3, TEST_SERVICE_NEW_VERSION.1)
            .start() .await .unwrap();

        let offering4 = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(4, TEST_SERVICE_NEW_VERSION.1)
            .start() .await .unwrap();

        let offering5 = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(5, TEST_SERVICE_NEW_VERSION.1)
            .start() .await .unwrap();

        loop {
            offering1.notify(EventgroupId::new(1).unwrap(), EventId::new(0x8001).unwrap(), "1".as_bytes()).await.unwrap();
            offering2.notify(EventgroupId::new(1).unwrap(), EventId::new(0x8002).unwrap(), "2".as_bytes()).await.unwrap();
            offering3.notify(EventgroupId::new(1).unwrap(), EventId::new(0x8003).unwrap(), "3".as_bytes()).await.unwrap();
            offering4.notify(EventgroupId::new(1).unwrap(), EventId::new(0x8004).unwrap(), "4".as_bytes()).await.unwrap();
            offering5.notify(EventgroupId::new(1).unwrap(), EventId::new(0x8005).unwrap(), "5".as_bytes()).await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
            if let Ok(Some(e)) = tokio::time::timeout(Duration::from_millis(50), offering3.next()).await {
                match e {
                    ServiceEvent::Unsubscribe { .. } => {
                        tracing::info!("Received Unsubscribe for v3 offering, stopping notifications");
                        flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    _ => {}
                }
            }
        }
    }});

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut proxy_v1 = runtime
            .find(TEST_SERVICE_ID)
            .major_version(1)
            .instance(InstanceId::Id(0x0001)).await.unwrap()
            .subscribe(EventgroupId::new(1).unwrap()).await.unwrap();
        let async1 = async {
            let mut received = 0;
            let now = tokio::time::Instant::now();
            while now.elapsed() < Duration::from_secs(15) {
                let event = proxy_v1.next().await.unwrap();
                received += 1;
                let payload_str = String::from_utf8_lossy(&event.payload);
                assert_eq!(payload_str, "1");
                tracing::info!("Received v1 event: {:?}", String::from_utf8_lossy(&event.payload));
            }
            assert!(received > 13, "Should have received v1 events");
        };

        let mut proxy_v2 = runtime
            .find(TEST_SERVICE_ID)
            .major_version(2)
            .instance(InstanceId::Id(0x0001)).await.unwrap()
            .subscribe(EventgroupId::new(1).unwrap()).await.unwrap();
        let async2 = async {
            let mut received = 0;
            let now = tokio::time::Instant::now();
            while now.elapsed() < Duration::from_secs(15) {
                let event = proxy_v2.next().await.unwrap();
                received += 1;
                let payload_str = String::from_utf8_lossy(&event.payload);
                assert_eq!(payload_str, "2");
                tracing::info!("Received v2 event: {:?}", String::from_utf8_lossy(&event.payload));
            }
            assert!(received > 13, "Should have received v2 events");
        };

        let mut proxy_v3 = runtime
            .find(TEST_SERVICE_ID)
            .major_version(3)
            .instance(InstanceId::Id(0x0001)).await.unwrap()
            .subscribe(EventgroupId::new(1).unwrap()).await.unwrap();
        let async3 = async {
            let _event = proxy_v3.next().await.unwrap();
            tracing::info!("Received first v3 event");
            let _event = proxy_v3.next().await.unwrap();
            tracing::info!("Received second v3 event, now dropping subscription");
            drop(proxy_v3);
        };

        let mut proxy_v4 = runtime
            .find(TEST_SERVICE_ID)
            .major_version(4)
            .instance(InstanceId::Id(0x0001)).await.unwrap()
            .subscribe(EventgroupId::new(1).unwrap()).await.unwrap();
        let async4 = async {
            let mut received = 0;
            let now = tokio::time::Instant::now();
            while now.elapsed() < Duration::from_secs(15) {
                let event = proxy_v4.next().await.unwrap();
                received += 1;
                let payload_str = String::from_utf8_lossy(&event.payload);
                assert_eq!(payload_str, "4");
                tracing::info!("Received v4 event: {:?}", String::from_utf8_lossy(&event.payload));
            }
            assert!(received > 13, "Should have received v4 events");
        };

        let mut proxy_v5 = runtime
            .find(TEST_SERVICE_ID)
            .major_version(5)
            .instance(InstanceId::Id(0x0001)).await.unwrap()
            .subscribe(EventgroupId::new(1).unwrap()).await.unwrap();
        let async5 = async {
            let mut received = 0;
            let now = tokio::time::Instant::now();
            while now.elapsed() < Duration::from_secs(15) {
                let event = proxy_v5.next().await.unwrap();
                received += 1;
                let payload_str = String::from_utf8_lossy(&event.payload);
                assert_eq!(payload_str, "5");
                tracing::info!("Received v5 event: {:?}", String::from_utf8_lossy(&event.payload));
            }
            assert!(received > 13, "Should have received v5 events");
        };

        tokio::join!(async1, async2, async3, async4, async5);

        Ok(())
    });

    sim.run().unwrap();

    assert!(
        sub_arrived.load(std::sync::atomic::Ordering::SeqCst),
        "Expected unsubscribe for version 3 did not arrive"
    );
}

#[test_log::test]
fn test_finds_late() {
    let mut sim = turmoil::Builder::new().build();

    sim.host("host", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("host").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // TODO make sure that the first round of FINDS has already happened by then
        // requires FindSchedule config in client
        tokio::time::sleep(Duration::from_secs(3)).await;

        let mut offering1 = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001)).version(1, TEST_SERVICE_VERSION.1)
            .start()
            .await
            .unwrap();

        loop {
            if let Some(event) = offering1.next().await {
                match event {
                    ServiceEvent::Subscribe { .. } => {
                        tracing::info!("Handled expected Subscribe for TestService v1");
                    }
                    _ => {}
                }
            }
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        runtime
            .find(TEST_SERVICE_ID).major_version(1)
            .instance(InstanceId::Id(0x0001))
            .await
            .expect("Service v1 available");

        Ok(())
    });

    sim.run().unwrap();
}

proptest::proptest! {
    /// Property: Client can find services by major version with instance wildcard.
    /// Two services are offered with different instance IDs and major versions,
    /// and the client finds them using only the major version (any instance).
    #[test_log::test]
    fn test_find_instance_wildcard(
        instance1 in 1u16..=0xFFFEu16,
        instance2 in 1u16..=0xFFFEu16,
        version1 in 1u8..=0xFEu8,
        version2 in 1u8..=0xFEu8,
    ) {
        // Ensure versions are different so we can distinguish them
        proptest::prop_assume!(version1 != version2);

        let mut sim = turmoil::Builder::new().build();

        let v1 = version1;
        let v2 = version2;
        let i1 = instance1;
        let i2 = instance2;

        sim.host("host", move || async move {
            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("host").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let mut offering1 = runtime
                .offer(TEST_SERVICE_ID, InstanceId::Id(i1)).version(v1, 0)
                .start()
                .await
                .unwrap();

            let mut offering2 = runtime
                .offer(TEST_SERVICE_ID, InstanceId::Id(i2)).version(v2, 0)
                .start()
                .await
                .unwrap();

            loop {
                tokio::select! {
                    Some(event) = offering1.next() => {
                        match event {
                            ServiceEvent::Subscribe { .. } => {
                                tracing::info!("Handled Subscribe for instance {} version {}", i1, v1);
                            }
                            _ => {}
                        }
                    }
                    Some(event) = offering2.next() => {
                        match event {
                            ServiceEvent::Subscribe { .. } => {
                                tracing::info!("Handled Subscribe for instance {} version {}", i2, v2);
                            }
                            _ => {}
                        }
                    }
                }
            }
        });

        sim.client("client", async move {
            tokio::time::sleep(Duration::from_millis(100)).await;

            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Find by major version only (instance wildcard)
            let proxy_v1 = runtime
                .find(TEST_SERVICE_ID).major_version(v1)
                .await
                .expect("Service with version1 available");

            let proxy_v2 = runtime
                .find(TEST_SERVICE_ID).major_version(v2)
                .await
                .expect("Service with version2 available");

            Ok(())
        });

        sim.run().unwrap();
    }
}

proptest::proptest! {
    /// Property: Client can find services by specific instance ID with version wildcard.
    /// Two services are offered with different instance IDs and major versions,
    /// and the client finds them using the specific instance ID (any version).
    #[test_log::test]
    fn test_find_version_wildcard(
        instance1 in 1u16..=0xFFFEu16,
        instance2 in 1u16..=0xFFFEu16,
        version1 in 1u8..=0xFEu8,
        version2 in 1u8..=0xFEu8,
    ) {
        // Ensure instances are different so we can distinguish them
        proptest::prop_assume!(instance1 != instance2);

        let mut sim = turmoil::Builder::new().build();

        let v1 = version1;
        let v2 = version2;
        let i1 = instance1;
        let i2 = instance2;

        sim.host("host", move || async move {
            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("host").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let mut offering1 = runtime
                .offer(TEST_SERVICE_ID, InstanceId::Id(i1)).version(v1, 0)
                .start()
                .await
                .unwrap();

            let mut offering2 = runtime
                .offer(TEST_SERVICE_ID, InstanceId::Id(i2)).version(v2, 0)
                .start()
                .await
                .unwrap();

            loop {
                tokio::select! {
                    Some(event) = offering1.next() => {
                        match event {
                            ServiceEvent::Subscribe { .. } => {
                                tracing::info!("Handled Subscribe for instance {} version {}", i1, v1);
                            }
                            _ => {}
                        }
                    }
                    Some(event) = offering2.next() => {
                        match event {
                            ServiceEvent::Subscribe { .. } => {
                                tracing::info!("Handled Subscribe for instance {} version {}", i2, v2);
                            }
                            _ => {}
                        }
                    }
                }
            }
        });

        sim.client("client", async move {
            tokio::time::sleep(Duration::from_millis(100)).await;

            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Find by instance ID only (version wildcard via default)
            let proxy_i1 = runtime
                .find(TEST_SERVICE_ID)
                .instance(InstanceId::Id(i1))
                .await
                .expect("Service with instance1 available");

            let proxy_i2 = runtime
                .find(TEST_SERVICE_ID)
                .instance(InstanceId::Id(i2))
                .await
                .expect("Service with instance2 available");

            Ok(())
        });

        sim.run().unwrap();
    }
}

#[test_log::test]
fn test_find_two_versions_late() {
    let mut sim = turmoil::Builder::new().build();

    sim.host("host", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("host").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // TODO make sure that the first round of FINDS has already happened by then
        // requires FindSchedule config in client
        tokio::time::sleep(Duration::from_secs(3)).await;

        let mut offering1 = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001)).version(1, TEST_SERVICE_VERSION.1)
            .start()
            .await
            .unwrap();

        let mut offering2 = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001)).version(2, TEST_SERVICE_NEW_VERSION.1)
            .start()
            .await
            .unwrap();

        loop {
            tokio::select! {
                Some(event) = offering1.next() => {
                    match event {
                        ServiceEvent::Subscribe { .. } => {
                            tracing::info!("Handled expected Subscribe for TestService v1");
                        }
                        _ => {}
                    }
                }
                Some(event) = offering2.next() => {
                    match event {
                        ServiceEvent::Subscribe { .. } => {
                            tracing::info!("Handled expected Subscribe for TestServiceNew v2");
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy_v1 = runtime
            .find(TEST_SERVICE_ID).major_version(1)
            .instance(InstanceId::Id(0x0001));

        let proxy_v2 = runtime
            .find(TEST_SERVICE_ID).major_version(2)
            .instance(InstanceId::Id(0x0001));

        let (found_1, found_2) = tokio::join!(
            proxy_v1,
            proxy_v2,
        );

        found_1.expect("Service v1 available");
        found_2.expect("Service v2 available");

        Ok(())
    });

    sim.run().unwrap();
}

#[test_log::test]
fn test_two_concurrent_versions() {
    let mut sim = turmoil::Builder::new().build();

    sim.host("host", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("host").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering1 = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001)).version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        let mut offering2 = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001)).version(TEST_SERVICE_NEW_VERSION.0, TEST_SERVICE_NEW_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        loop {
            tokio::select! {
                Some(event) = offering1.next() => {
                    match event {
                        ServiceEvent::Call { method, payload, responder, .. } => {
                            let mut response = vec![method.value() as u8];
                            response.extend_from_slice(&payload);
                            responder.reply(&response).await.unwrap();
                            tracing::info!("Handled expected call for TestService v1");
                        }
                        ServiceEvent::FireForget { method, payload, .. } => {
                            panic!("Unexpected FireForget for TestService v1: method={:?}", method);
                        }
                        _ => {}
                    }
                }
                Some(event) = offering2.next() => {
                    match event {
                        ServiceEvent::Call { method, payload, responder, .. } => {
                            panic!("Unexpected Call for TestServiceNew v2: method={:?}", method);
                        }
                        ServiceEvent::FireForget { method, payload, .. } => {
                            tracing::info!("Handled expected FireForget for TestServiceNew v2: method={:?}", method);
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy_v1 = runtime
            .find(TEST_SERVICE_ID)
            .major_version(1)
            .instance(InstanceId::Id(0x0001));
        let proxy_v1 = tokio::time::timeout(Duration::from_secs(5), proxy_v1)
            .await
            .expect("Discovery timeout")
            .expect("Service v1 available");

        let proxy_v2 = runtime
            .find(TEST_SERVICE_ID).major_version(2)
            .instance(InstanceId::Id(0x0001));
        let proxy_v2 = tokio::time::timeout(Duration::from_secs(5), proxy_v2)
            .await
            .expect("Discovery timeout")
            .expect("Service v2 available");

        // Call method on v1
        let method = MethodId::new(0x0042).unwrap();
        let payload = b"hello v1";
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_v1.call(method, payload),
        )
        .await
        .expect("Timeout waiting for response")
        .expect("Call should succeed");

        assert_eq!(response.payload[0], 0x42);
        assert_eq!(&response.payload[1..], b"hello v1");

        // Fire-and-forget on v2
        let method_ff = MethodId::new(0x0050).unwrap();
        proxy_v2.fire_and_forget(method_ff, b"hello v2").await.unwrap();

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// TEST HELPERS
// ============================================================================

/// Parse a SOME/IP-SD message from raw bytes
fn parse_sd_message(data: &[u8]) -> Option<(SomeIpHeader, SdMessage)> {
    if data.len() < 16 {
        return None;
    }

    let header = SomeIpHeader {
        service_id: u16::from_be_bytes([data[0], data[1]]),
        method_id: u16::from_be_bytes([data[2], data[3]]),
        length: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
        client_id: u16::from_be_bytes([data[8], data[9]]),
        session_id: u16::from_be_bytes([data[10], data[11]]),
        protocol_version: data[12],
        interface_version: data[13],
        message_type: data[14],
        return_code: data[15],
    };

    // Verify SD message (service=0xFFFF, method=0x8100)
    if header.service_id != 0xFFFF || header.method_id != 0x8100 {
        return None;
    }

    let sd_payload = &data[16..];
    if sd_payload.len() < 12 {
        return None;
    }

    let flags = sd_payload[0];
    let entries_len =
        u32::from_be_bytes([sd_payload[4], sd_payload[5], sd_payload[6], sd_payload[7]]) as usize;

    if sd_payload.len() < 8 + entries_len {
        return None;
    }

    let entries_data = &sd_payload[8..8 + entries_len];
    let mut entries = Vec::new();

    let mut offset = 0;
    while offset + 16 <= entries_data.len() {
        let entry = &entries_data[offset..offset + 16];
        entries.push(SdEntry {
            entry_type: entry[0].into(),
            index_1st_option: entry[1],
            index_2nd_option: entry[2],
            num_options_1: (entry[3] >> 4) & 0x0F,
            num_options_2: entry[3] & 0x0F,
            service_id: u16::from_be_bytes([entry[4], entry[5]]),
            instance_id: u16::from_be_bytes([entry[6], entry[7]]),
            major_version: entry[8],
            ttl: u32::from_be_bytes([0, entry[9], entry[10], entry[11]]),
            minor_version: u32::from_be_bytes([entry[12], entry[13], entry[14], entry[15]]),
            eventgroup_id: u16::from_be_bytes([entry[14], entry[15]]),
            counter: entry[13] & 0x0F,
        });
        offset += 16;
    }

    // Parse options array
    let options_offset = 8 + entries_len;
    let options = if sd_payload.len() > options_offset + 4 {
        let _options_len = u32::from_be_bytes([
            sd_payload[options_offset],
            sd_payload[options_offset + 1],
            sd_payload[options_offset + 2],
            sd_payload[options_offset + 3],
        ]) as usize;
        Vec::new() // Simplified: don't parse options
    } else {
        Vec::new()
    };

    Some((
        header,
        SdMessage {
            flags,
            entries,
            options,
        },
    ))
}

/// Simplified SOME/IP header for test parsing
#[derive(Debug)]
#[allow(dead_code)]
struct SomeIpHeader {
    service_id: u16,
    method_id: u16,
    length: u32,
    client_id: u16,
    session_id: u16,
    protocol_version: u8,
    interface_version: u8,
    message_type: u8,
    return_code: u8,
}

/// Simplified SD message for test parsing
#[derive(Debug)]
#[allow(dead_code)]
struct SdMessage {
    flags: u8,
    entries: Vec<SdEntry>,
    options: Vec<SdOption>,
}

impl SdMessage {
    fn get_udp_endpoint(&self, _entry: &SdEntry) -> Option<std::net::SocketAddr> {
        None // Simplified
    }
}

/// Simplified SD entry for test parsing
#[derive(Debug)]
#[allow(dead_code)]
struct SdEntry {
    entry_type: SdEntryType,
    index_1st_option: u8,
    index_2nd_option: u8,
    num_options_1: u8,
    num_options_2: u8,
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    ttl: u32,
    minor_version: u32,
    eventgroup_id: u16,
    counter: u8,
}

/// SD entry types
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
#[allow(dead_code)]
enum SdEntryType {
    FindService = 0x00,
    OfferService = 0x01,
    SubscribeEventgroup = 0x06,
    SubscribeEventgroupAck = 0x07,
    Unknown = 0xFF,
}

impl From<u8> for SdEntryType {
    fn from(v: u8) -> Self {
        match v {
            0x00 => SdEntryType::FindService,
            0x01 => SdEntryType::OfferService,
            0x06 => SdEntryType::SubscribeEventgroup,
            0x07 => SdEntryType::SubscribeEventgroupAck,
            _ => SdEntryType::Unknown,
        }
    }
}

/// Placeholder for SD options
#[derive(Debug)]
#[allow(dead_code)]
struct SdOption;

/// Build a raw SOME/IP-SD OfferService message
fn build_sd_offer(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    minor_version: u32,
    addr: std::net::Ipv4Addr,
    port: u16,
    ttl: u32,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(64);

    // SOME/IP Header
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes());
    packet.extend_from_slice(&0x8100u16.to_be_bytes());
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes());
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    packet.push(0x01);
    packet.push(0x01);
    packet.push(0x02);
    packet.push(0x00);

    // SD Payload
    packet.push(0xC0);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);
    packet.extend_from_slice(&16u32.to_be_bytes());

    // OfferService Entry
    packet.push(0x01);
    packet.push(0x00);
    packet.push(0x00);
    packet.push(0x10);
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]);
    packet.extend_from_slice(&minor_version.to_be_bytes());

    // Options array
    packet.extend_from_slice(&12u32.to_be_bytes());
    // IPv4 Endpoint Option
    packet.extend_from_slice(&9u16.to_be_bytes());
    packet.push(0x04);
    packet.push(0x00);
    packet.extend_from_slice(&addr.octets());
    packet.push(0x00);
    packet.push(0x11); // UDP
    packet.extend_from_slice(&port.to_be_bytes());

    // Fix length
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}

/// Build a raw SOME/IP-SD SubscribeEventgroupNack message (TTL=0)
fn build_sd_subscribe_nack(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(64);

    // SOME/IP Header
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes());
    packet.extend_from_slice(&0x8100u16.to_be_bytes());
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes());
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    packet.push(0x01);
    packet.push(0x01);
    packet.push(0x02);
    packet.push(0x00);

    // SD Payload
    packet.push(0xC0);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);
    packet.extend_from_slice(&16u32.to_be_bytes());

    // SubscribeEventgroupAck Entry with TTL=0 (NACK)
    packet.push(0x07); // SubscribeEventgroupAck type
    packet.push(0x00);
    packet.push(0x00);
    packet.push(0x00);
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    // TTL = 0 (NACK)
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);
    // Reserved + counter
    packet.push(0x00);
    packet.push(0x00);
    // Eventgroup ID
    packet.extend_from_slice(&eventgroup_id.to_be_bytes());

    // Options array (empty)
    packet.extend_from_slice(&0u32.to_be_bytes());

    // Fix length
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}
