//! Subscription Tests with Multiple Instances
//!
//! Tests event delivery when subscribing to multiple instances of the same service.
//! This is a critical scenario that exposes bugs in event routing logic.

use someip_runtime::handle::ServiceEvent;
use someip_runtime::prelude::*;
use someip_runtime::runtime::Runtime;
use std::time::Duration;

/// Type alias for turmoil-based runtime
type TurmoilRuntime =
    Runtime<turmoil::net::UdpSocket, turmoil::net::TcpStream, turmoil::net::TcpListener>;

/// Test service definition
struct TestService;

impl Service for TestService {
    const SERVICE_ID: u16 = 0x5555;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}

// ============================================================================
// MULTI-INSTANCE SUBSCRIPTION TESTS
// ============================================================================

/// Test subscribing to two different instances of the same service
///
/// This test exposes a bug where events are routed only by service_id,
/// not by instance_id. Each subscription should receive events only from
/// its corresponding instance, but the current implementation delivers
/// events from all instances to all subscriptions.
///
/// Expected behavior:
/// - Instance 0x0001 sends events with payload "instance1_eventN"
/// - Instance 0x0002 sends events with payload "instance2_eventN"
/// - Subscription to 0x0001 should receive only "instance1_*" events
/// - Subscription to 0x0002 should receive only "instance2_*" events
///
/// Actual buggy behavior:
/// - Both subscriptions receive events from both instances
#[test]
fn subscribe_to_multiple_instances() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server 1 - Instance 0x0001
    sim.host("server1", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Wait for subscription
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send events from instance 1
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();

        for i in 0..3 {
            let event_data = format!("instance1_event{}", i);
            offering
                .notify(eventgroup, event_id, event_data.as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    // Server 2 - Instance 0x0002
    sim.host("server2", || async {
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0002))
            .await
            .unwrap();

        // Wait for subscription
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send events from instance 2
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();

        for i in 0..3 {
            let event_data = format!("instance2_event{}", i);
            offering
                .notify(eventgroup, event_id, event_data.as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    // Client subscribes to both instances
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Find and subscribe to instance 1
        let proxy1 = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy1 = tokio::time::timeout(Duration::from_secs(5), proxy1.available())
            .await
            .expect("Discovery timeout for instance 1")
            .expect("Instance 1 should be available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription1 =
            tokio::time::timeout(Duration::from_secs(5), proxy1.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout for instance 1")
                .expect("Subscribe to instance 1 should succeed");

        // Find and subscribe to instance 2
        let proxy2 = runtime.find::<TestService>(InstanceId::Id(0x0002));
        let proxy2 = tokio::time::timeout(Duration::from_secs(5), proxy2.available())
            .await
            .expect("Discovery timeout for instance 2")
            .expect("Instance 2 should be available");

        let mut subscription2 =
            tokio::time::timeout(Duration::from_secs(5), proxy2.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout for instance 2")
                .expect("Subscribe to instance 2 should succeed");

        // Collect events from both subscriptions
        let mut events1 = Vec::new();
        let mut events2 = Vec::new();

        // Receive events with a timeout
        for _ in 0..6 {
            tokio::select! {
                event = subscription1.next() => {
                    if let Some(e) = event {
                        events1.push(String::from_utf8_lossy(e.payload.as_ref()).to_string());
                    }
                }
                event = subscription2.next() => {
                    if let Some(e) = event {
                        events2.push(String::from_utf8_lossy(e.payload.as_ref()).to_string());
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(3)) => {
                    break;
                }
            }
        }

        println!("Instance 1 subscription received {} events: {:?}", events1.len(), events1);
        println!("Instance 2 subscription received {} events: {:?}", events2.len(), events2);

        // EXPECTED BEHAVIOR: Each subscription should receive only events from its instance
        // - events1 should contain only "instance1_event0", "instance1_event1", "instance1_event2"
        // - events2 should contain only "instance2_event0", "instance2_event1", "instance2_event2"

        // Check that subscription1 received events from instance 1
        assert!(!events1.is_empty(), "Subscription 1 should receive events");
        for event in &events1 {
            assert!(
                event.starts_with("instance1_"),
                "Subscription to instance 1 should only receive instance1 events, got: {}",
                event
            );
        }

        // Check that subscription2 received events from instance 2
        assert!(!events2.is_empty(), "Subscription 2 should receive events");
        for event in &events2 {
            assert!(
                event.starts_with("instance2_"),
                "Subscription to instance 2 should only receive instance2 events, got: {}",
                event
            );
        }

        // Check we got all expected events
        assert_eq!(events1.len(), 3, "Should receive exactly 3 events from instance 1");
        assert_eq!(events2.len(), 3, "Should receive exactly 3 events from instance 2");

        Ok(())
    });

    // Note: This test will FAIL with the current implementation because
    // handle_incoming_notification() matches events only by service_id,
    // not by instance_id. Both subscriptions will receive all 6 events.
    sim.run().unwrap();
}
