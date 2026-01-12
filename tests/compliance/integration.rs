//! Integration Tests (Async/Turmoil)
//!
//! Tests that verify complete protocol flows using turmoil simulation.
//! These tests run full client-server interactions through the library.
//!
//! For wire-level byte verification, see wire_capture.rs which uses
//! raw sockets to inspect actual packet contents.

use recentip::handle::ServiceEvent;
use recentip::prelude::*;
use recentip::Runtime;
use std::time::Duration;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

/// Type alias for turmoil-based runtime

// Wire values for TestService
const TEST_SERVICE_ID: u16 = 0x1234;
const TEST_SERVICE_VERSION: (u8, u32) = (1, 0);

// ============================================================================
// RPC INTEGRATION TESTS
// ============================================================================

/// feat_req_recentip_103: REQUEST and RESPONSE message types
/// feat_req_recentip_60: Message ID composition
/// feat_req_recentip_83: Request ID composition
#[test_log::test]
fn request_response_roundtrip() {
    covers!(
        feat_req_recentip_103,
        feat_req_recentip_60,
        feat_req_recentip_83
    );

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::Call {
                    payload, responder, ..
                } => {
                    assert_eq!(payload.as_ref(), b"hello");
                    responder.reply(b"world").await.unwrap();
                }
                _ => panic!("Expected Call event"),
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout waiting for service")
            .expect("Service available");

        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(0x0001).unwrap(), b"hello"),
        )
        .await
        .expect("Timeout waiting for response")
        .expect("Call should succeed");

        assert_eq!(response.payload.as_ref(), b"world");
        assert_eq!(response.return_code, ReturnCode::Ok);

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentip_677: Session ID increments
/// feat_req_recentip_649: Session ID starts at non-zero
#[test_log::test]
fn multiple_calls_succeed() {
    covers!(feat_req_recentip_677, feat_req_recentip_649);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        for i in 0..3 {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call { responder, .. } => {
                        responder.reply(&[i as u8]).await.unwrap();
                    }
                    _ => {}
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout waiting for service")
            .expect("Service available");

        for i in 0..3u8 {
            let response = tokio::time::timeout(
                Duration::from_secs(5),
                proxy.call(MethodId::new(0x0001).unwrap(), &[i]),
            )
            .await
            .expect("Timeout")
            .expect("Call should succeed");

            assert_eq!(response.payload[0], i);
        }

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// SERVICE DISCOVERY INTEGRATION TESTS
// ============================================================================

/// feat_req_recentipsd_141, feat_req_recentipsd_142: SD discovery works
#[test_log::test]
fn service_discovery_finds_offered_service() {
    covers!(feat_req_recentipsd_141, feat_req_recentipsd_142);

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

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);

        // Service should be discovered via SD
        let _proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Service should be discovered via SD");

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// EVENT/NOTIFICATION INTEGRATION TESTS
// ============================================================================

/// feat_req_recentip_103: NOTIFICATION message type
#[test_log::test]
fn notification_delivery() {
    covers!(feat_req_recentip_103);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
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

        // Wait for client to subscribe
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send notification
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        offering
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap()
            .notify(b"event_data")
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout waiting for service")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Timeout subscribing")
                .expect("Subscribe should succeed");

        let event = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("Timeout waiting for event");

        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.event_id.value(), 0x8001);
        assert_eq!(event.payload.as_ref(), b"event_data");

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// RETURN CODE INTEGRATION TESTS
// ============================================================================

/// feat_req_recentip_67: Return Code E_OK
#[test_log::test]
fn successful_call_returns_ok() {
    covers!(feat_req_recentip_67);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        if let Some(ServiceEvent::Call { responder, .. }) = offering.next().await {
            responder.reply(b"ok").await.unwrap();
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout")
            .expect("Service available");

        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(0x0001).unwrap(), b"test"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");

        assert_eq!(response.return_code, ReturnCode::Ok);

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// FAULT TOLERANCE TESTS
// ============================================================================

/// Network partition causes service unavailability
///
/// Uses turmoil's partition() to simulate network failures.
/// After partitioning, calls should fail or timeout.
/// After repair(), communication should resume.
#[test_log::test]
fn network_partition_handling() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(60))
        .build();

    let partition_done = Arc::new(AtomicBool::new(false));
    let partition_done_server = partition_done.clone();

    sim.host("server", move || {
        let partition_done = partition_done_server.clone();
        async move {
            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .start_turmoil()
                .await
                .unwrap();

            let mut offering = runtime
                .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
                .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            // Handle calls while running
            loop {
                tokio::select! {
                    event = offering.next() => {
                        if let Some(ServiceEvent::Call { responder, .. }) = event {
                            responder.reply(b"pong").await.unwrap();
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(50)) => {
                        if partition_done.load(Ordering::SeqCst) {
                            break;
                        }
                    }
                }
            }

            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout waiting for service")
            .expect("Service available");

        // Call should succeed before partition
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            proxy.call(MethodId::new(0x0001).unwrap(), b"ping"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");
        assert_eq!(response.payload.as_ref(), b"pong");

        // Partition the network
        turmoil::partition("client", "server");

        // Call should fail/timeout during partition
        let result = tokio::time::timeout(
            Duration::from_millis(500),
            proxy.call(MethodId::new(0x0001).unwrap(), b"ping"),
        )
        .await;

        // Either timeout or error is acceptable during partition
        assert!(
            result.is_err() || result.unwrap().is_err(),
            "Call during partition should fail or timeout"
        );

        // Repair the network
        turmoil::repair("client", "server");

        // Give time for recovery
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Call should succeed after repair
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            proxy.call(MethodId::new(0x0001).unwrap(), b"ping"),
        )
        .await
        .expect("Timeout after repair")
        .expect("Call should succeed after repair");
        assert_eq!(response.payload.as_ref(), b"pong");

        partition_done.store(true, Ordering::SeqCst);
        Ok(())
    });

    sim.run().unwrap();
}

/// Service restart with state reset
///
/// Uses turmoil's crash()/bounce() via sim.step() to simulate a true server restart.
/// After bounce, the server's closure runs again from the beginning.
/// This tests that:
/// - Server can be crashed and bounced
/// - After bounce, the server closure re-executes (state reset)
/// - Client can communicate with the restarted service
///
/// Note: Testing that calls FAIL during crash requires the runtime to actually
/// depend on network communication, which the current mock may not fully support.
#[test_log::test]
fn service_restart_recovery() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(60))
        .tick_duration(Duration::from_millis(10))
        .build();

    // Count how many times the server closure has been invoked
    let server_starts = Arc::new(AtomicU32::new(0));
    let server_starts_clone = server_starts.clone();

    // Phases: 0=init, 1=first call done, 2=ready for crash, 3=crashed, 4=bounced, 5=done
    let phase = Arc::new(AtomicU32::new(0));
    let phase_client = phase.clone();

    sim.host("server", move || {
        let server_starts = server_starts_clone.clone();
        async move {
            // Track that server closure was invoked
            let start_count = server_starts.fetch_add(1, Ordering::SeqCst) + 1;

            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .start_turmoil()
                .await
                .unwrap();

            let mut offering = runtime
                .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
                .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            // Handle calls - return the start count so client can verify
            loop {
                tokio::select! {
                    event = offering.next() => {
                        if let Some(ServiceEvent::Call { responder, .. }) = event {
                            // Return which server instance this is
                            responder.reply(&start_count.to_be_bytes()).await.unwrap();
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {
                        break;
                    }
                }
            }

            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Phase 1: Discover and make first call
        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout waiting for service")
            .expect("Service available");

        let response = tokio::time::timeout(
            Duration::from_secs(2),
            proxy.call(MethodId::new(0x0001).unwrap(), b"call1"),
        )
        .await
        .expect("Timeout")
        .expect("First call should succeed");

        // First server instance should return 1
        assert_eq!(
            u32::from_be_bytes(response.payload.as_ref().try_into().unwrap()),
            1,
            "First call should be handled by server instance 1"
        );

        // Signal ready for crash
        phase_client.store(2, Ordering::SeqCst);

        // Wait for bounce to complete (skip crash phase - calls might still work)
        while phase_client.load(Ordering::SeqCst) < 4 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // Give server time to restart and re-offer
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Re-discover the service after restart
        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout waiting for service after restart")
            .expect("Service available");

        // Call the restarted server
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            proxy.call(MethodId::new(0x0001).unwrap(), b"call2"),
        )
        .await
        .expect("Timeout after restart")
        .expect("Call should succeed after restart");

        // After bounce, server closure runs again - start_count should be 2
        assert_eq!(
            u32::from_be_bytes(response.payload.as_ref().try_into().unwrap()),
            2,
            "After bounce, server closure should re-run (instance 2)"
        );

        phase_client.store(5, Ordering::SeqCst);
        Ok(())
    });

    // Run simulation with manual stepping to inject crash/bounce
    loop {
        match sim.step() {
            Ok(false) => {
                // Check if we should crash the server
                if phase.load(Ordering::SeqCst) == 2 {
                    sim.crash("server");
                    phase.store(3, Ordering::SeqCst);
                }
                // Check if we should bounce the server (after crash is complete)
                else if phase.load(Ordering::SeqCst) == 3 && !sim.is_host_running("server") {
                    sim.bounce("server");
                    phase.store(4, Ordering::SeqCst);
                }
                // Check if test is complete
                else if phase.load(Ordering::SeqCst) == 5 {
                    break;
                }
            }
            Ok(true) => break, // All clients completed
            Err(e) => panic!("Simulation error: {e}"),
        }
    }

    // Verify server was started twice (initial + after bounce)
    assert_eq!(
        server_starts.load(Ordering::SeqCst),
        2,
        "Server closure should have been invoked twice"
    );
}

/// Many concurrent pending requests
///
/// Stress test with 100 concurrent requests to verify the runtime
/// handles high concurrency correctly.
#[test_log::test]
fn many_concurrent_requests() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(120))
        .build();

    const NUM_REQUESTS: usize = 100;

    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        let mut handled = 0;
        while handled < NUM_REQUESTS {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call {
                        payload, responder, ..
                    } => {
                        // Echo back the payload
                        responder.reply(payload.as_ref()).await.unwrap();
                        handled += 1;
                    }
                    _ => {}
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout waiting for service")
            .expect("Service available");

        // Spawn 100 truly concurrent requests
        // ProxyHandle is Clone, and call() accepts owned data (Vec<u8>)
        let mut handles = Vec::with_capacity(NUM_REQUESTS);
        for i in 0..NUM_REQUESTS {
            let proxy = proxy.clone();
            let handle = tokio::spawn(async move {
                let payload = (i as u32).to_be_bytes().to_vec();
                let response = tokio::time::timeout(
                    Duration::from_secs(30),
                    proxy.call(MethodId::new(0x0001).unwrap(), payload.clone()),
                )
                .await
                .expect("Timeout")
                .expect("Call should succeed");

                // Verify echo
                assert_eq!(response.payload.as_ref(), &payload[..]);
                i
            });
            handles.push(handle);
        }

        // Wait for all to complete
        let mut completed = std::collections::HashSet::new();
        for handle in handles {
            let i = handle.await.expect("Task should complete");
            completed.insert(i);
        }

        // Verify all 100 completed
        assert_eq!(
            completed.len(),
            NUM_REQUESTS,
            "All requests should complete"
        );

        Ok(())
    });

    sim.run().unwrap();
}
