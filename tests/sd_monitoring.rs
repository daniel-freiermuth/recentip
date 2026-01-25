//! SD Monitoring API Integration Tests
//!
//! Tests for the `Runtime::monitor_sd()` API which allows applications
//! to monitor all Service Discovery events on the network.
//!
//! Run with: cargo test --test sd_monitoring

use recentip::prelude::*;
use recentip::SdEvent;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Type alias for turmoil-based runtime

// Wire values for TestService
const TEST_SERVICE_ID: u16 = 0x1234;
const TEST_SERVICE_VERSION: (u8, u32) = (1, 0);

// Wire values for AnotherService
const ANOTHER_SERVICE_ID: u16 = 0x5678;
const ANOTHER_SERVICE_VERSION: (u8, u32) = (2, 3);

// ============================================================================
// BASIC SD EVENT MONITORING
// ============================================================================

/// Test that monitor_sd() receives ServiceAvailable events when a service is offered.
#[test_log::test]
fn monitor_sd_receives_service_available() {
    let event_received = Arc::new(Mutex::new(None::<SdEvent>));
    let event_clone = Arc::clone(&event_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Keep server alive long enough for discovery
        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("monitor", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("monitor").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();
        let mut sd_events = runtime.monitor_sd().await.unwrap();

        // Wait for an event with timeout
        let event = tokio::time::timeout(Duration::from_secs(5), sd_events.recv())
            .await
            .expect("Timeout waiting for SD event")
            .expect("Channel should not be closed");

        *event_clone.lock().unwrap() = Some(event);
        Ok(())
    });

    sim.run().unwrap();

    let event = event_received.lock().unwrap().take();
    assert!(event.is_some(), "Should have received an SD event");

    match event.unwrap() {
        SdEvent::ServiceAvailable {
            service_id,
            instance_id,
            ..
        } => {
            assert_eq!(service_id, 0x1234, "Service ID should match");
            assert_eq!(instance_id, 0x0001, "Instance ID should match");
        }
        other => panic!("Expected got {:?}", other),
    }
}

/// Test that monitor_sd() receives ServiceUnavailable events when an offering is dropped.
#[test_log::test]
fn monitor_sd_receives_service_unavailable() {
    let events_received = Arc::new(Mutex::new(Vec::<SdEvent>::new()));
    let events_clone = Arc::clone(&events_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        // Monitor starts first so it doesn't miss the initial offer
        // Wait for monitor to be ready
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for monitor to receive initial offer
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Drop the offering to trigger StopOffer
        drop(offering);

        // Give time for StopOffer to be sent
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("monitor", async move {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("monitor").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();
        let mut sd_events = runtime.monitor_sd().await.unwrap();

        // Collect events for a while
        let collect_duration = Duration::from_secs(3);
        let deadline = tokio::time::Instant::now() + collect_duration;

        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(200), sd_events.recv()).await {
                Ok(Some(event)) => {
                    events_clone.lock().unwrap().push(event);
                }
                Ok(None) => break,  // Channel closed
                Err(_) => continue, // Timeout, keep trying
            }
        }
        Ok(())
    });

    sim.run().unwrap();

    let events = events_received.lock().unwrap();

    // Should have received at least ServiceAvailable
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SdEvent::ServiceAvailable { .. })),
        "Should have received ServiceAvailable event"
    );

    // Should have received ServiceUnavailable after offering was dropped
    assert!(
        events.iter().any(
            |e| matches!(e, SdEvent::ServiceUnavailable { service_id, instance_id }
            if *service_id == 0x1234 && *instance_id == 0x0001)
        ),
        "Should have received ServiceUnavailable event for the service. Events: {:?}",
        *events
    );
}

/// Test that ServiceAvailable event contains accurate metadata.
#[test_log::test]
fn monitor_sd_event_metadata_accuracy() {
    let event_received = Arc::new(Mutex::new(None::<SdEvent>));
    let event_clone = Arc::clone(&event_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Offer with specific instance ID
        let _offering = runtime
            .offer(ANOTHER_SERVICE_ID, InstanceId::Id(0x0042))
            .version(ANOTHER_SERVICE_VERSION.0, ANOTHER_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("monitor", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("monitor").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();
        let mut sd_events = runtime.monitor_sd().await.unwrap();

        let event = tokio::time::timeout(Duration::from_secs(5), sd_events.recv())
            .await
            .expect("Timeout waiting for SD event")
            .expect("Channel should not be closed");

        *event_clone.lock().unwrap() = Some(event);
        Ok(())
    });

    sim.run().unwrap();

    let event = event_received
        .lock()
        .unwrap()
        .take()
        .expect("Event should be received");

    match event {
        SdEvent::ServiceAvailable {
            service_id,
            instance_id,
            major_version,
            minor_version,
            endpoint,
            ttl,
        } => {
            assert_eq!(service_id, 0x5678, "Service ID should match AnotherService");
            assert_eq!(instance_id, 0x0042, "Instance ID should match");
            assert_eq!(
                major_version, 2,
                "Major version should match AnotherService"
            );
            assert_eq!(
                minor_version, 3,
                "Minor version should match AnotherService"
            );
            assert!(ttl > 0, "TTL should be positive");
            // Endpoint should be valid (port > 0)
            assert!(endpoint.port() > 0, "Endpoint port should be valid");
        }
        other => panic!("Expected got {:?}", other),
    }
}

// ============================================================================
// MULTIPLE MONITORS
// ============================================================================

/// Test that multiple monitors all receive the same events.
#[test_log::test]
fn monitor_sd_multiple_monitors_receive_events() {
    let events_monitor1 = Arc::new(Mutex::new(Vec::<SdEvent>::new()));
    let events_monitor2 = Arc::new(Mutex::new(Vec::<SdEvent>::new()));
    let events1 = Arc::clone(&events_monitor1);
    let events2 = Arc::clone(&events_monitor2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        // Server starts after monitors are ready
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("monitor1", async move {
        // Start monitors first so they don't miss events
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("monitor1").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();
        let mut sd_events = runtime.monitor_sd().await.unwrap();

        // Collect events
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(100), sd_events.recv()).await {
                Ok(Some(event)) => events1.lock().unwrap().push(event),
                Ok(None) => break,
                Err(_) => continue,
            }
        }
        Ok(())
    });

    sim.client("monitor2", async move {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("monitor2").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();
        let mut sd_events = runtime.monitor_sd().await.unwrap();

        // Collect events
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(100), sd_events.recv()).await {
                Ok(Some(event)) => events2.lock().unwrap().push(event),
                Ok(None) => break,
                Err(_) => continue,
            }
        }
        Ok(())
    });

    sim.run().unwrap();

    let m1_events = events_monitor1.lock().unwrap();
    let m2_events = events_monitor2.lock().unwrap();

    // Both monitors should have received ServiceAvailable
    assert!(
        m1_events.iter().any(|e| matches!(
            e,
            SdEvent::ServiceAvailable {
                service_id: 0x1234,
                ..
            }
        )),
        "Monitor 1 should have received ServiceAvailable"
    );
    assert!(
        m2_events.iter().any(|e| matches!(
            e,
            SdEvent::ServiceAvailable {
                service_id: 0x1234,
                ..
            }
        )),
        "Monitor 2 should have received ServiceAvailable"
    );
}

/// Test that multiple monitors on the SAME runtime all receive events.
#[test_log::test]
fn monitor_sd_multiple_monitors_same_runtime() {
    let events_monitor1 = Arc::new(Mutex::new(Vec::<SdEvent>::new()));
    let events_monitor2 = Arc::new(Mutex::new(Vec::<SdEvent>::new()));
    let events1 = Arc::clone(&events_monitor1);
    let events2 = Arc::clone(&events_monitor2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.client("server", async {
        let runtime = recentip::configure()
            .cyclic_offer_delay(1000)
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("monitor_host", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("monitor_host").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Create two monitors on the same runtime
        let mut sd_events1 = runtime.monitor_sd().await.unwrap();
        let mut sd_events2 = runtime.monitor_sd().await.unwrap();

        // Spawn tasks to collect from both monitors concurrently
        let events1_clone = Arc::clone(&events1);
        let events2_clone = Arc::clone(&events2);

        let task1 = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            while tokio::time::Instant::now() < deadline {
                match tokio::time::timeout(Duration::from_millis(100), sd_events1.recv()).await {
                    Ok(Some(event)) => events1_clone.lock().unwrap().push(event),
                    Ok(None) => break,
                    Err(_) => continue,
                }
            }
        });

        let task2 = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            while tokio::time::Instant::now() < deadline {
                match tokio::time::timeout(Duration::from_millis(100), sd_events2.recv()).await {
                    Ok(Some(event)) => events2_clone.lock().unwrap().push(event),
                    Ok(None) => break,
                    Err(_) => continue,
                }
            }
        });

        let _ = tokio::join!(task1, task2);
        Ok(())
    });

    sim.run().unwrap();

    let m1_events = events_monitor1.lock().unwrap();
    let m2_events = events_monitor2.lock().unwrap();

    // Both monitors on same runtime should have received ServiceAvailable
    assert!(
        m1_events.iter().any(|e| matches!(
            e,
            SdEvent::ServiceAvailable {
                service_id: 0x1234,
                ..
            }
        )),
        "Monitor 1 should have received ServiceAvailable. Events: {:?}",
        *m1_events
    );
    assert!(
        m2_events.iter().any(|e| matches!(
            e,
            SdEvent::ServiceAvailable {
                service_id: 0x1234,
                ..
            }
        )),
        "Monitor 2 should have received ServiceAvailable. Events: {:?}",
        *m2_events
    );
}

// ============================================================================
// MULTIPLE SERVICES
// ============================================================================

/// Test that monitor receives events from multiple different services.
#[test_log::test]
fn monitor_sd_multiple_services() {
    let events_received = Arc::new(Mutex::new(Vec::<SdEvent>::new()));
    let events_clone = Arc::clone(&events_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server1", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server1").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.host("server2", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server2").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(ANOTHER_SERVICE_ID, InstanceId::Id(0x0002))
            .version(ANOTHER_SERVICE_VERSION.0, ANOTHER_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("monitor", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("monitor").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();
        let mut sd_events = runtime.monitor_sd().await.unwrap();

        // Collect events for a while
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(200), sd_events.recv()).await {
                Ok(Some(event)) => events_clone.lock().unwrap().push(event),
                Ok(None) => break,
                Err(_) => continue,
            }
        }
        Ok(())
    });

    sim.run().unwrap();

    let events = events_received.lock().unwrap();

    // Should have received events from both services
    let has_test_service = events.iter().any(|e| {
        matches!(
            e,
            SdEvent::ServiceAvailable {
                service_id: 0x1234,
                instance_id: 0x0001,
                ..
            }
        )
    });
    let has_another_service = events.iter().any(|e| {
        matches!(
            e,
            SdEvent::ServiceAvailable {
                service_id: 0x5678,
                instance_id: 0x0002,
                ..
            }
        )
    });

    assert!(
        has_test_service,
        "Should have received TestService event. Events: {:?}",
        *events
    );
    assert!(
        has_another_service,
        "Should have received AnotherService event. Events: {:?}",
        *events
    );
}

// ============================================================================
// TTL EXPIRATION (ServiceExpired)
// ============================================================================

/// Test that service lifecycle (available then unavailable) is tracked.
///
/// Tests that dropping an offering results in ServiceUnavailable.
/// Note: True TTL expiration is hard to test since cyclic offers renew automatically.
#[test_log::test]
fn monitor_sd_receives_service_expired() {
    let events_received = Arc::new(Mutex::new(Vec::<SdEvent>::new()));
    let events_clone = Arc::clone(&events_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        // Wait for monitor to be ready
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for offer to be sent and received
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Drop the offering - this sends StopOffer and we'll get ServiceUnavailable
        drop(offering);

        // Keep server alive briefly to let StopOffer be sent
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("monitor", async move {
        // Monitor starts first
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("monitor").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();
        let mut sd_events = runtime.monitor_sd().await.unwrap();

        // Collect events for longer to catch the full lifecycle
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(200), sd_events.recv()).await {
                Ok(Some(event)) => {
                    tracing::info!("Received event: {:?}", event);
                    events_clone.lock().unwrap().push(event);
                }
                Ok(None) => break,
                Err(_) => continue,
            }
        }
        Ok(())
    });

    sim.run().unwrap();

    let events = events_received.lock().unwrap();

    // Should have received ServiceAvailable initially
    assert!(
        events.iter().any(|e| matches!(
            e,
            SdEvent::ServiceAvailable {
                service_id: 0x1234,
                ..
            }
        )),
        "Should have received ServiceAvailable event. Events: {:?}",
        *events
    );

    // Should have received either ServiceUnavailable or ServiceExpired
    // (depending on whether StopOffer was received or TTL expired)
    let has_unavailable_or_expired = events.iter().any(|e| {
        matches!(
            e,
            SdEvent::ServiceUnavailable {
                service_id: 0x1234,
                ..
            } | SdEvent::ServiceExpired {
                service_id: 0x1234,
                ..
            }
        )
    });

    assert!(
        has_unavailable_or_expired,
        "Should have received ServiceUnavailable or ServiceExpired. Events: {:?}",
        *events
    );
}

// ============================================================================
// EDGE CASES
// ============================================================================

/// Test that dropping a monitor receiver doesn't affect other monitors.
#[test_log::test]
fn monitor_sd_dropped_receiver_cleanup() {
    let events_received = Arc::new(Mutex::new(Vec::<SdEvent>::new()));
    let events_clone = Arc::clone(&events_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(3)).await;
        Ok(())
    });

    sim.client("monitor", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("monitor").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Create first monitor and immediately drop it
        {
            let _sd_events_dropped = runtime.monitor_sd().await.unwrap();
            // Receiver is dropped here
        }

        // Create second monitor - should still work
        let mut sd_events = runtime.monitor_sd().await.unwrap();

        // Wait for event
        let event = tokio::time::timeout(Duration::from_secs(5), sd_events.recv())
            .await
            .expect("Timeout waiting for SD event")
            .expect("Channel should not be closed");

        events_clone.lock().unwrap().push(event);
        Ok(())
    });

    sim.run().unwrap();

    let events = events_received.lock().unwrap();
    assert!(
        events.iter().any(|e| matches!(
            e,
            SdEvent::ServiceAvailable {
                service_id: 0x1234,
                ..
            }
        )),
        "Second monitor should still receive events after first was dropped. Events: {:?}",
        *events
    );
}

/// Test that monitor_sd works even when called before any services exist.
#[test_log::test]
fn monitor_sd_before_services_exist() {
    let event_received = Arc::new(Mutex::new(None::<SdEvent>));
    let event_clone = Arc::clone(&event_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.client("monitor", async move {
        // Start monitoring FIRST, before any server exists
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("monitor").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();
        let mut sd_events = runtime.monitor_sd().await.unwrap();

        // Wait for event (server will start later)
        let event = tokio::time::timeout(Duration::from_secs(10), sd_events.recv())
            .await
            .expect("Timeout waiting for SD event")
            .expect("Channel should not be closed");

        *event_clone.lock().unwrap() = Some(event);
        Ok(())
    });

    // Server starts slightly after monitor
    sim.host("server", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.run().unwrap();

    let event = event_received.lock().unwrap().take();
    assert!(
        event.is_some(),
        "Should have received SD event even when monitoring started first"
    );

    match event.unwrap() {
        SdEvent::ServiceAvailable { service_id, .. } => {
            assert_eq!(service_id, 0x1234);
        }
        other => panic!("Expected got {:?}", other),
    }
}
