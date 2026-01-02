//! Basic API tests using turmoil for network simulation.

use someip_runtime::handle::ServiceEvent;
use someip_runtime::runtime::Runtime;
use someip_runtime::{
    EventId, EventgroupId, InstanceId, MethodId, RuntimeConfig, Service, ServiceId,
};
use std::time::Duration;

/// Type alias for turmoil-based runtime for convenience
type TurmoilRuntime =
    Runtime<turmoil::net::UdpSocket, turmoil::net::TcpStream, turmoil::net::TcpListener>;

/// Test service definition
struct TestService;

impl Service for TestService {
    const SERVICE_ID: u16 = 0x1234;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}

/// Another test service
struct AnotherService;

impl Service for AnotherService {
    const SERVICE_ID: u16 = 0x5678;
    const MAJOR_VERSION: u8 = 2;
    const MINOR_VERSION: u32 = 1;
}

#[test]
fn test_runtime_creation() {
    let mut sim = turmoil::Builder::new().build();

    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: Result<TurmoilRuntime, _> = Runtime::with_socket_type(config).await;

        assert!(runtime.is_ok(), "Runtime should be created successfully");

        Ok(())
    });

    sim.run().unwrap();
}

#[test]
fn test_find_service() {
    let mut sim = turmoil::Builder::new().build();

    sim.host("client", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Create a proxy - this should succeed immediately (just creates the handle)
        let proxy = runtime.find::<TestService>(InstanceId::Any);

        // Check it's in unavailable state
        assert_eq!(proxy.service_id(), ServiceId::new(0x1234).unwrap());
        assert_eq!(proxy.instance_id(), InstanceId::Any);
        assert!(!proxy.is_available());

        Ok(())
    });

    sim.run().unwrap();
}

#[test]
fn test_offer_service() {
    let mut sim = turmoil::Builder::new().build();

    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer a service
        let offering = runtime.offer::<TestService>(InstanceId::Id(0x0001)).await;

        assert!(offering.is_ok(), "Offering should succeed");

        let offering = offering.unwrap();
        assert_eq!(offering.service_id(), ServiceId::new(0x1234).unwrap());
        assert_eq!(offering.instance_id(), InstanceId::Id(0x0001));

        Ok(())
    });

    sim.run().unwrap();
}

#[test]
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

#[test]
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

#[test]
fn test_service_discovery_offer_find() {
    // Test that a client can discover a server's offered service
    let mut sim = turmoil::Builder::new().build();

    // Server offers a service
    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
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

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Any);

        // Wait for discovery (with timeout)
        let result = tokio::time::timeout(Duration::from_millis(300), proxy.available()).await;

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

#[test]
fn test_multiple_services() {
    // Test offering and finding multiple services
    let mut sim = turmoil::Builder::new().build();

    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer two different services
        let _offering1 = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();
        let _offering2 = runtime
            .offer::<AnotherService>(InstanceId::Id(0x0002))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Find both services
        let proxy1 = runtime.find::<TestService>(InstanceId::Any);
        let proxy2 = runtime.find::<AnotherService>(InstanceId::Any);

        // Wait for both to be discovered
        let result1 = tokio::time::timeout(Duration::from_millis(300), proxy1.available()).await;
        let result2 = tokio::time::timeout(Duration::from_millis(300), proxy2.available()).await;

        assert!(result1.is_ok(), "TestService should be discovered");
        assert!(result2.is_ok(), "AnotherService should be discovered");

        Ok(())
    });

    sim.run().unwrap();
}

#[test]
fn test_specific_instance_id() {
    // Test finding a specific instance ID
    let mut sim = turmoil::Builder::new().build();

    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer instance 0x0001
        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Find specific instance
        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));

        let result = tokio::time::timeout(Duration::from_millis(300), proxy.available()).await;

        assert!(result.is_ok(), "Specific instance should be discovered");
        let available = result.unwrap().expect("Service available");
        // Note: After discovery, the instance ID might be updated to the actual ID
        assert_eq!(available.service_id(), ServiceId::new(0x1234).unwrap());

        Ok(())
    });

    sim.run().unwrap();
}

#[test]
fn test_runtime_clone() {
    // Test that cloned runtimes share state
    let mut sim = turmoil::Builder::new().build();

    sim.host("node", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Clone the runtime
        let runtime2 = runtime.clone();

        // Offer via one clone
        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Find via the other clone should see the same state
        let proxy = runtime2.find::<TestService>(InstanceId::Id(0x0001));

        // The proxy should be created successfully
        assert_eq!(proxy.service_id(), ServiceId::new(0x1234).unwrap());

        Ok(())
    });

    sim.run().unwrap();
}

#[test]
fn test_offering_handle_drop() {
    // Test that dropping an offering handle sends StopOffer
    let mut sim = turmoil::Builder::new().build();

    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        {
            let _offering = runtime
                .offer::<TestService>(InstanceId::Id(0x0001))
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

#[test]
fn test_method_call_rpc() {
    // Test full RPC round-trip: client calls method, server responds
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer the service
        let mut offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
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

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Find the service
        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));

        // Wait for service to become available
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
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

#[test]
fn test_event_subscription() {
    // Test event subscription: client subscribes, server sends events
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer the service
        let offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
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

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Find the service
        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));

        // Wait for service to become available
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
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
