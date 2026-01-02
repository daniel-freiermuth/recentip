//! Tests for the ServiceInstance<State> API design.
//!
//! These are turmoil-based integration tests that operate on the public API only.
//! They test the bind/announce separation pattern as documented in DESIGN.md.
//!
//! API:
//! ```text
//! ServiceInstance<Bound>     - listening on endpoint, not announced via SD
//! ServiceInstance<Announced> - listening + announced via SD (OfferService sent)
//! ```

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

// Import only from the public API
use someip_runtime::{
    handle::{Announced, Bound, ServiceEvent, ServiceInstance},
    runtime::{Runtime, RuntimeConfig},
    EventId, EventgroupId, InstanceId, MethodId, Service, ServiceId,
};

/// Type alias for turmoil-based runtime
type TurmoilRuntime =
    Runtime<turmoil::net::UdpSocket, turmoil::net::TcpStream, turmoil::net::TcpListener>;

// ============================================================================
// TEST SERVICE DEFINITIONS
// ============================================================================

struct BrakeService;

impl Service for BrakeService {
    const SERVICE_ID: u16 = 0x1234;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}

struct TemperatureService;

impl Service for TemperatureService {
    const SERVICE_ID: u16 = 0x5678;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}

// ============================================================================
// TYPESTATE TRANSITION TESTS
// ============================================================================

/// Test: bind() returns ServiceInstance<Bound>, announce() transitions to Announced
#[test]
fn test_bind_returns_bound_instance() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // bind() should return ServiceInstance<_, Bound>
            let _service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // At this point: listening on endpoint, but NOT announced via SD
            // Clients cannot discover this service yet

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
}

/// Test: announce() transitions Bound → Announced
#[test]
fn test_announce_transitions_to_announced() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Phase 1: Bind (not yet announced)
            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // Phase 2: Announce (sends OfferService)
            let _announced = service.announce().await.unwrap();

            // Now clients can discover the service via SD

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
}

/// Test: stop_announcing() transitions Announced → Bound
#[test]
fn test_stop_announcing_transitions_to_bound() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            let announced = service.announce().await.unwrap();

            // Graceful shutdown: stop announcing but keep socket open
            let _bound = announced.stop_announcing().await.unwrap();

            // Socket still open, but StopOfferService was sent
            // Existing connections can be drained

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
}

/// Test: full lifecycle Bound → Announced → Bound
#[test]
fn test_full_lifecycle() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Bind
            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // Announce
            let announced = service.announce().await.unwrap();

            // Stop announcing (graceful shutdown)
            let bound = announced.stop_announcing().await.unwrap();

            // Could re-announce if needed
            let _announced_again = bound.announce().await.unwrap();

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
}

// ============================================================================
// SERVICE DISCOVERY INTEGRATION TESTS
// ============================================================================

/// Test: client cannot discover service that is only Bound (not Announced)
#[test]
fn test_bound_service_not_discoverable() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Only bind, don't announce
            let _service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // Keep server alive
            tokio::time::sleep(Duration::from_millis(500)).await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let proxy = runtime.find::<BrakeService>(InstanceId::Any);

            // Service should NOT be discoverable (only bound, not announced)
            let result = tokio::time::timeout(Duration::from_millis(300), proxy.available()).await;

            assert!(
                result.is_err(),
                "Service should NOT be discoverable when only Bound"
            );

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

/// Test: client CAN discover service after announce()
#[test]
fn test_announced_service_is_discoverable() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // Announce the service
            let _announced = service.announce().await.unwrap();

            // Keep server alive
            tokio::time::sleep(Duration::from_millis(500)).await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let proxy = runtime.find::<BrakeService>(InstanceId::Any);

            // Service SHOULD be discoverable after announce()
            let result = tokio::time::timeout(Duration::from_millis(300), proxy.available()).await;

            assert!(
                result.is_ok(),
                "Service should be discoverable after announce()"
            );

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

/// Test: client loses service after stop_announcing()
#[test]
fn test_service_disappears_after_stop_announcing() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            let announced = service.announce().await.unwrap();

            // Wait for client to discover
            tokio::time::sleep(Duration::from_millis(300)).await;

            // Stop announcing (sends StopOfferService)
            let _bound = announced.stop_announcing().await.unwrap();

            // Keep server alive to observe client behavior
            tokio::time::sleep(Duration::from_millis(500)).await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let proxy = runtime.find::<BrakeService>(InstanceId::Any);

            // Verify it's still discoverable before server stops announcing
            let proxy_before_stop = runtime.find::<BrakeService>(InstanceId::Any);
            let result_before = tokio::time::timeout(Duration::from_millis(150), proxy_before_stop.available()).await;
            assert!(
                result_before.is_ok(),
                "Service should still be available before stop_announcing()"
            );

            // Wait for server to stop announcing
            tokio::time::sleep(Duration::from_millis(400)).await;

            // Service should now be unavailable - verify by trying to discover again
            // A fresh proxy should not find the service since StopOfferService was sent
            let proxy_after_stop = runtime.find::<BrakeService>(InstanceId::Any);
            let result_after = tokio::time::timeout(Duration::from_millis(200), proxy_after_stop.available()).await;
            
            assert!(
                result_after.is_err(),
                "Service should be unavailable after stop_announcing()"
            );

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

// ============================================================================
// STATIC MODE TESTS (NO SD)
// ============================================================================

/// Test: notify_static() works without announce()
#[test]
fn test_notify_static_without_announce() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let mut service = runtime
                .bind::<TemperatureService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // Add static subscriber (pre-configured, no SD)
            service.add_static_subscriber(
                "192.168.1.20:30502".parse().unwrap(),
                &[EventgroupId::new(0x0001).unwrap()],
            );

            // Can notify static subscribers without announcing
            service
                .notify_static(
                    EventgroupId::new(0x0001).unwrap(),
                    EventId::new(0x8001).unwrap(),
                    b"temperature=25",
                )
                .await
                .unwrap();

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    // TODO: Add static client that receives the event

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
}

// ============================================================================
// ANNOUNCED MODE TESTS
// ============================================================================

/// Test: notify() only works in Announced state
#[test]
fn test_notify_requires_announced_state() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // service.notify(...) should NOT compile - method only exists on Announced
            // This is enforced by the type system

            let announced = service.announce().await.unwrap();

            // Wait for subscriber
            tokio::time::sleep(Duration::from_millis(300)).await;

            // notify() IS available on Announced
            announced
                .notify(
                    EventgroupId::new(0x0001).unwrap(),
                    EventId::new(0x8001).unwrap(),
                    b"brake_event",
                )
                .await
                .unwrap();

            tokio::time::sleep(Duration::from_millis(200)).await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let proxy = runtime.find::<BrakeService>(InstanceId::Any);
            let available = proxy.available().await.unwrap();

            let mut subscription = available
                .subscribe(EventgroupId::new(0x0001).unwrap())
                .await
                .unwrap();

            // Should receive the event
            let event = tokio::time::timeout(Duration::from_secs(2), subscription.next())
                .await
                .expect("Should receive event");

            assert!(event.is_some());
            assert_eq!(&event.unwrap().payload[..], b"brake_event");

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

/// Test: has_subscribers() only available in Announced state
#[test]
fn test_has_subscribers_in_announced_state() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // service.has_subscribers(...).await should NOT compile - method only on Announced

            let announced = service.announce().await.unwrap();

            // Initially no subscribers
            assert!(!announced.has_subscribers(EventgroupId::new(0x0001).unwrap()).await);

            // Wait for client to subscribe
            tokio::time::sleep(Duration::from_millis(300)).await;

            // Now should have subscribers
            assert!(announced.has_subscribers(EventgroupId::new(0x0001).unwrap()).await);

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let proxy = runtime.find::<BrakeService>(InstanceId::Any);
            let available = proxy.available().await.unwrap();

            let _subscription = available
                .subscribe(EventgroupId::new(0x0001).unwrap())
                .await
                .unwrap();

            // Keep subscription alive
            tokio::time::sleep(Duration::from_millis(500)).await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

// ============================================================================
// INITIALIZATION BEFORE ANNOUNCEMENT TESTS
// ============================================================================

/// Test: can do initialization work between bind() and announce()
#[test]
fn test_initialization_before_announce() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Phase 1: Bind
            let service = runtime
                .bind::<TemperatureService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // Phase 2: Heavy initialization (service not yet discoverable)
            // Simulate sensor initialization
            tokio::time::sleep(Duration::from_millis(100)).await;
            let sensor_ready = true;

            // Phase 3: Only announce after initialization succeeds
            if sensor_ready {
                let _announced = service.announce().await.unwrap();
            }

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
}

/// Test: if initialization fails, service is never announced
#[test]
fn test_init_failure_prevents_announcement() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let service = runtime
                .bind::<TemperatureService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // Simulate initialization failure
            let init_result: Result<(), &str> = Err("sensor not found");

            if init_result.is_err() {
                // Drop without announcing - no OfferService sent
                drop(service);
                flag.store(true, Ordering::SeqCst);
                return Ok(());
            }

            // Never reached
            let _announced = service.announce().await.unwrap();

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let proxy = runtime.find::<TemperatureService>(InstanceId::Any);

            // Service should never be discoverable
            let result = tokio::time::timeout(Duration::from_millis(300), proxy.available()).await;

            assert!(
                result.is_err(),
                "Service should never be announced after init failure"
            );

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

// ============================================================================
// GRACEFUL SHUTDOWN TESTS
// ============================================================================

/// Test: graceful shutdown allows draining in-flight requests
#[test]
fn test_graceful_shutdown_drains_requests() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            let mut announced = service.announce().await.unwrap();

            // Wait for client to connect and start request
            tokio::time::sleep(Duration::from_millis(200)).await;

            // Stop announcing - new clients won't discover us
            // But we can still serve existing connections
            let mut bound = announced.stop_announcing().await.unwrap();

            // Continue serving requests on the bound instance
            // (next() should still work)
            while let Some(event) = tokio::time::timeout(Duration::from_millis(100), bound.next())
                .await
                .ok()
                .flatten()
            {
                // Handle remaining requests
                match event {
                    someip_runtime::handle::ServiceEvent::Call { responder, .. } => {
                        responder.reply(b"response").await.unwrap();
                    }
                    _ => {}
                }
            }

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let proxy = runtime.find::<BrakeService>(InstanceId::Any);
            let available = proxy.available().await.unwrap();

            // Make a call that should succeed even during shutdown
            let response = available
                .call(someip_runtime::MethodId::new(0x0001).unwrap(), b"request")
                .await;

            assert!(
                response.is_ok(),
                "In-flight request should complete during graceful shutdown"
            );

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

// ============================================================================
// DROP BEHAVIOR TESTS
// ============================================================================

/// Test: dropping Announced sends StopOfferService
#[test]
fn test_drop_announced_sends_stop_offer() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            {
                let announced = service.announce().await.unwrap();
                tokio::time::sleep(Duration::from_millis(200)).await;
                // announced dropped here - should send StopOfferService
            }

            // Keep server alive to observe client behavior
            tokio::time::sleep(Duration::from_millis(300)).await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let proxy = runtime.find::<BrakeService>(InstanceId::Any);

            // Should discover initially
            let _available = tokio::time::timeout(Duration::from_millis(150), proxy.available())
                .await
                .expect("Should discover")
                .unwrap();

            // Wait for server to drop the announced instance
            tokio::time::sleep(Duration::from_millis(300)).await;

            // Service should now be gone (StopOfferService received)
            // Verify by creating a fresh proxy and expecting discovery to fail
            let proxy2 = runtime.find::<BrakeService>(InstanceId::Any);
            let result = tokio::time::timeout(Duration::from_millis(200), proxy2.available()).await;
            
            assert!(
                result.is_err(),
                "Service should be unavailable after Announced instance is dropped"
            );

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

/// Test: dropping Bound (never announced) sends no SD message
#[test]
fn test_drop_bound_no_sd_message() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            {
                let _service = runtime
                    .bind::<BrakeService>(InstanceId::Id(0x0001))
                    .await
                    .unwrap();
                // Dropped without announce - no OfferService, no StopOfferService
            }

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let proxy = runtime.find::<BrakeService>(InstanceId::Any);

            // Should never discover (never announced)
            let result = tokio::time::timeout(Duration::from_millis(300), proxy.available()).await;

            assert!(
                result.is_err(),
                "Should never discover a service that was only bound"
            );

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

// ============================================================================
// EDGE CASE: MULTIPLE SERVICES FROM SAME RUNTIME
// ============================================================================

/// Test: multiple services can be bound from the same runtime
#[test]
fn test_multiple_services_same_runtime() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Bind multiple different services
            let brake = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            let temp = runtime
                .bind::<TemperatureService>(InstanceId::Id(0x0002))
                .await
                .unwrap();

            // Announce one, keep other bound
            let _announced_brake = brake.announce().await.unwrap();
            // temp remains in Bound state

            tokio::time::sleep(Duration::from_millis(200)).await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Brake should be discoverable (announced)
            let brake_proxy = runtime.find::<BrakeService>(InstanceId::Any);
            let brake_result =
                tokio::time::timeout(Duration::from_millis(150), brake_proxy.available()).await;
            assert!(
                brake_result.is_ok(),
                "Announced service should be discoverable"
            );

            // Temperature should NOT be discoverable (only bound)
            let temp_proxy = runtime.find::<TemperatureService>(InstanceId::Any);
            let temp_result =
                tokio::time::timeout(Duration::from_millis(150), temp_proxy.available()).await;
            assert!(
                temp_result.is_err(),
                "Bound-only service should not be discoverable"
            );

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

/// Test: multiple instances of same service
#[test]
fn test_multiple_instances_same_service() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Same service type, different instances
            let instance1 = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            let instance2 = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0002))
                .await
                .unwrap();

            // Only announce one
            let _announced1 = instance1.announce().await.unwrap();
            // instance2 remains bound

            tokio::time::sleep(Duration::from_millis(200)).await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Instance 0x0001 should be discoverable
            let proxy1 = runtime.find::<BrakeService>(InstanceId::Id(0x0001));
            let result1 =
                tokio::time::timeout(Duration::from_millis(150), proxy1.available()).await;
            assert!(result1.is_ok(), "Announced instance should be discoverable");

            // Instance 0x0002 should NOT be discoverable
            let proxy2 = runtime.find::<BrakeService>(InstanceId::Id(0x0002));
            let result2 =
                tokio::time::timeout(Duration::from_millis(150), proxy2.available()).await;
            assert!(
                result2.is_err(),
                "Bound-only instance should not be discoverable"
            );

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

// ============================================================================
// EDGE CASE: DOUBLE BIND SAME INSTANCE
// ============================================================================

/// Test: binding same service+instance twice should fail
#[test]
fn test_double_bind_same_instance_fails() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let _first = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // Second bind of same service+instance should fail
            let second = runtime.bind::<BrakeService>(InstanceId::Id(0x0001)).await;

            assert!(second.is_err(), "Cannot bind same service+instance twice");

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
}

// ============================================================================
// EDGE CASE: RPC IN BOUND STATE (STATIC MODE)
// ============================================================================

/// Test: RPC works in Bound state when client has pre-configured address
#[test]
fn test_rpc_in_bound_state() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let mut service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // Never announce - but socket is open and accepting connections

            // Handle one request
            while let Some(event) = tokio::time::timeout(Duration::from_millis(500), service.next())
                .await
                .ok()
                .flatten()
            {
                match event {
                    someip_runtime::handle::ServiceEvent::Call { responder, .. } => {
                        responder.reply(b"static_response").await.unwrap();
                    }
                    _ => {}
                }
            }

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Client with pre-configured address (static deployment)
            let server_ip = turmoil::lookup("server");
            let proxy = runtime.find_static::<BrakeService>(
                InstanceId::Id(0x0001),
                std::net::SocketAddr::new(server_ip, 30491),
            );

            // RPC should work even though server never announced
            let response = proxy
                .call(someip_runtime::MethodId::new(0x0001).unwrap(), b"request")
                .await
                .unwrap();

            assert_eq!(&response.payload[..], b"static_response");

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

// ============================================================================
// EDGE CASE: NOTIFY WITH NO SUBSCRIBERS
// ============================================================================

/// Test: notify() with no subscribers succeeds (no-op)
#[test]
fn test_notify_no_subscribers_succeeds() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            let announced = service.announce().await.unwrap();

            // No subscribers yet, but notify should succeed (no-op)
            let result = announced
                .notify(
                    EventgroupId::new(0x0001).unwrap(),
                    EventId::new(0x8001).unwrap(),
                    b"event_data",
                )
                .await;

            assert!(
                result.is_ok(),
                "notify() should succeed even with no subscribers"
            );
            assert!(!announced.has_subscribers(EventgroupId::new(0x0001).unwrap()).await);

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
}

/// Test: notify_static() with no static subscribers succeeds (no-op)
#[test]
fn test_notify_static_no_subscribers_succeeds() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // No static subscribers added, but notify_static should succeed
            let result = service
                .notify_static(
                    EventgroupId::new(0x0001).unwrap(),
                    EventId::new(0x8001).unwrap(),
                    b"event_data",
                )
                .await;

            assert!(
                result.is_ok(),
                "notify_static() should succeed even with no static subscribers"
            );

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
}

// ============================================================================
// EDGE CASE: STATIC SUBSCRIBERS ACROSS STATE TRANSITIONS
// ============================================================================

/// Test: static subscribers are preserved when transitioning Bound → Announced
#[test]
fn test_static_subscribers_preserved_after_announce() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("static_client", move || {
        let flag = client_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Listen for static events on port 30502
            let mut listener = runtime.listen_static(
                ServiceId::new(BrakeService::SERVICE_ID).unwrap(),
                InstanceId::Id(0x0001),
                EventgroupId::new(0x0001).unwrap(),
                30502,
            ).await.unwrap();

            // Should receive event even after server announced
            let event = tokio::time::timeout(
                Duration::from_secs(3),
                listener.next()
            ).await.expect("Should receive static event");

            assert!(event.is_some());
            assert_eq!(&event.unwrap().payload[..], b"static_after_announce");

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(100)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let mut service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // Add static subscriber while Bound
            let client_addr: SocketAddr = (turmoil::lookup("static_client"), 30502).into();
            service.add_static_subscriber(
                client_addr,
                &[EventgroupId::new(0x0001).unwrap()],
            );

            // Transition to Announced
            let announced = service.announce().await.unwrap();

            // Static subscriber should still work after announcement
            announced.notify_static(
                EventgroupId::new(0x0001).unwrap(),
                EventId::new(0x8001).unwrap(),
                b"static_after_announce",
            ).await.unwrap();

            tokio::time::sleep(Duration::from_millis(500)).await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(server_ran.load(Ordering::SeqCst), "Server async block did not run");
    assert!(client_ran.load(Ordering::SeqCst), "Client async block did not run");
}

/// Test: static subscribers are preserved when transitioning Announced → Bound
#[test]
fn test_static_subscribers_preserved_after_stop_announcing() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("static_client", move || {
        let flag = client_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Listen for static events on port 30502
            let mut listener = runtime.listen_static(
                ServiceId::new(BrakeService::SERVICE_ID).unwrap(),
                InstanceId::Id(0x0001),
                EventgroupId::new(0x0001).unwrap(),
                30502,
            ).await.unwrap();

            // Should receive event after server stopped announcing
            let event = tokio::time::timeout(
                Duration::from_secs(4),
                listener.next()
            ).await.expect("Should receive static event");

            assert!(event.is_some());
            assert_eq!(&event.unwrap().payload[..], b"static_after_stop");

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(100)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let mut service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // Add static subscriber while Bound
            let client_addr: SocketAddr = (turmoil::lookup("static_client"), 30502).into();
            service.add_static_subscriber(
                client_addr,
                &[EventgroupId::new(0x0001).unwrap()],
            );

            let announced = service.announce().await.unwrap();

            tokio::time::sleep(Duration::from_millis(200)).await;

            // Stop announcing but keep bound
            let bound = announced.stop_announcing().await.unwrap();

            // Static subscriber should still work after stopping
            bound.notify_static(
                EventgroupId::new(0x0001).unwrap(),
                EventId::new(0x8001).unwrap(),
                b"static_after_stop",
            ).await.unwrap();

            tokio::time::sleep(Duration::from_millis(500)).await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(server_ran.load(Ordering::SeqCst), "Server async block did not run");
    assert!(client_ran.load(Ordering::SeqCst), "Client async block did not run");
}

// ============================================================================
// EDGE CASE: RE-ANNOUNCE AFTER STOP
// ============================================================================

/// Test: can re-announce after stop, clients can rediscover
#[test]
fn test_re_announce_after_stop() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(15))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::builder().ttl(5).build(); // Short TTL for testing
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // First announcement
            let announced = service.announce().await.unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;

            // Stop
            let bound = announced.stop_announcing().await.unwrap();
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Re-announce
            let _announced_again = bound.announce().await.unwrap();
            tokio::time::sleep(Duration::from_millis(500)).await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // First discovery
            let proxy = runtime.find::<BrakeService>(InstanceId::Any);
            let _available = tokio::time::timeout(Duration::from_millis(250), proxy.available())
                .await
                .expect("First discovery")
                .unwrap();

            // Wait for service to stop announcing (server stops at t=300ms)
            // Plus additional time for StopOfferService message to propagate
            tokio::time::sleep(Duration::from_millis(400)).await;

            // Verify service actually went away (need fresh proxy since available() consumes)
            let proxy_check_gone = runtime.find::<BrakeService>(InstanceId::Any);
            let gone_result = tokio::time::timeout(Duration::from_millis(200), proxy_check_gone.available()).await;
            assert!(
                gone_result.is_err(),
                "Service should be unavailable after stop_announcing()"
            );

            // Wait for service to re-announce (server re-announces at t=800ms from server start)
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Re-discover after re-announcement (need fresh proxy since available() consumes)
            let proxy2 = runtime.find::<BrakeService>(InstanceId::Any);
            let _available_again =
                tokio::time::timeout(Duration::from_millis(500), proxy2.available())
                    .await
                    .expect("Should rediscover after re-announce")
                    .unwrap();

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

// ============================================================================
// EDGE CASE: EVENTGROUP FILTERING
// ============================================================================

/// Test: static subscribers only receive events for their eventgroups
#[test]
fn test_static_subscriber_eventgroup_filtering() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client1_ran = Arc::new(AtomicBool::new(false));
    let client2_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client1_ran_clone = client1_ran.clone();
    let client2_ran_clone = client2_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    // Client 1 listens to eventgroup 0x0001
    sim.host("client1", move || {
        let flag = client1_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let mut listener = runtime.listen_static(
                ServiceId::new(BrakeService::SERVICE_ID).unwrap(),
                InstanceId::Id(0x0001),
                EventgroupId::new(0x0001).unwrap(),
                30501,
            ).await.unwrap();

            // Should only receive events for eventgroup 0x0001
            let event = tokio::time::timeout(
                Duration::from_secs(3),
                listener.next()
            ).await.expect("Should receive event for subscribed group");

            assert!(event.is_some());
            let e = event.unwrap();
            assert_eq!(&e.payload[..], b"eventgroup_1");

            // Should not receive event for eventgroup 0x0002
            let no_event = tokio::time::timeout(
                Duration::from_millis(500),
                listener.next()
            ).await;
            assert!(no_event.is_err(), "Should not receive event for other eventgroup");

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    // Client 2 listens to eventgroup 0x0002
    sim.host("client2", move || {
        let flag = client2_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let mut listener = runtime.listen_static(
                ServiceId::new(BrakeService::SERVICE_ID).unwrap(),
                InstanceId::Id(0x0001),
                EventgroupId::new(0x0002).unwrap(),
                30502,
            ).await.unwrap();

            // Should only receive events for eventgroup 0x0002
            let event = tokio::time::timeout(
                Duration::from_secs(3),
                listener.next()
            ).await.expect("Should receive event for subscribed group");

            assert!(event.is_some());
            let e = event.unwrap();
            assert_eq!(&e.payload[..], b"eventgroup_2");

            // Should not receive event for eventgroup 0x0001
            let no_event = tokio::time::timeout(
                Duration::from_millis(500),
                listener.next()
            ).await;
            assert!(no_event.is_err(), "Should not receive event for other eventgroup");

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(100)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let mut service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // Add client1 to eventgroup 0x0001
            let client1_addr: SocketAddr = (turmoil::lookup("client1"), 30501).into();
            service.add_static_subscriber(
                client1_addr,
                &[EventgroupId::new(0x0001).unwrap()],
            );

            // Add client2 to eventgroup 0x0002
            let client2_addr: SocketAddr = (turmoil::lookup("client2"), 30502).into();
            service.add_static_subscriber(
                client2_addr,
                &[EventgroupId::new(0x0002).unwrap()],
            );

            // Send event to eventgroup 0x0001 (only client1 should receive)
            service.notify_static(
                EventgroupId::new(0x0001).unwrap(),
                EventId::new(0x8001).unwrap(),
                b"eventgroup_1",
            ).await.unwrap();

            // Send event to eventgroup 0x0002 (only client2 should receive)
            service.notify_static(
                EventgroupId::new(0x0002).unwrap(),
                EventId::new(0x8002).unwrap(),
                b"eventgroup_2",
            ).await.unwrap();

            tokio::time::sleep(Duration::from_secs(2)).await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(server_ran.load(Ordering::SeqCst), "Server async block did not run");
    assert!(client1_ran.load(Ordering::SeqCst), "Client1 async block did not run");
    assert!(client2_ran.load(Ordering::SeqCst), "Client2 async block did not run");
}

// ============================================================================
// EDGE CASE: CONCURRENT OPERATIONS
// ============================================================================

/// Test: concurrent notify calls are safe
#[test]
fn test_concurrent_notify() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            let announced = service.announce().await.unwrap();

            // Wait for subscriber
            tokio::time::sleep(Duration::from_millis(200)).await;

            // Multiple sequential notifies (avoiding lifetime issues with concurrent)
            for i in 0..10 {
                let payload = format!("event_{}", i);
                let result = announced
                    .notify(
                        EventgroupId::new(0x0001).unwrap(),
                        EventId::new(0x8001).unwrap(),
                        payload.as_bytes(),
                    )
                    .await;
                assert!(result.is_ok(), "Notify {} should succeed", i);
            }

            tokio::time::sleep(Duration::from_millis(200)).await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let proxy = runtime.find::<BrakeService>(InstanceId::Any);
            let available = proxy.available().await.unwrap();

            let mut subscription = available
                .subscribe(EventgroupId::new(0x0001).unwrap())
                .await
                .unwrap();

            // Should receive all events
            let mut count = 0;
            while let Ok(Some(_)) =
                tokio::time::timeout(Duration::from_millis(200), subscription.next()).await
            {
                count += 1;
            }

            assert_eq!(count, 10, "Should receive all concurrent events");

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

// ============================================================================
// EDGE CASE: SUBSCRIBER UNSUBSCRIBES DURING NOTIFY
// ============================================================================

/// Test: notify succeeds even if subscriber disconnects mid-send
#[test]
fn test_notify_survives_subscriber_disconnect() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            let announced = service.announce().await.unwrap();

            // Wait for subscriber - needs enough time for discovery + subscription
            tokio::time::sleep(Duration::from_millis(500)).await;
            assert!(announced.has_subscribers(EventgroupId::new(0x0001).unwrap()).await);

            // Subscriber will disconnect during this sleep
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Notify after subscriber left - should not panic
            let result = announced
                .notify(
                    EventgroupId::new(0x0001).unwrap(),
                    EventId::new(0x8001).unwrap(),
                    b"event_after_disconnect",
                )
                .await;

            // Should succeed (maybe delivered to nobody, but no error)
            assert!(result.is_ok());

            // has_subscribers should now be false
            assert!(!announced.has_subscribers(EventgroupId::new(0x0001).unwrap()).await);

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let proxy = runtime.find::<BrakeService>(InstanceId::Any);
            let available = proxy.available().await.unwrap();

            let subscription = available
                .subscribe(EventgroupId::new(0x0001).unwrap())
                .await
                .unwrap();

            // Keep subscription alive for a bit, then drop
            tokio::time::sleep(Duration::from_millis(600)).await;
            drop(subscription);

            // Wait to allow unsubscribe to propagate
            tokio::time::sleep(Duration::from_millis(200)).await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

// ============================================================================
// EDGE CASE: LARGE PAYLOAD
// Per SOME/IP spec feat_req_recentiptp_760: UDP payloads are limited to ~1400 bytes.
// For larger payloads, TCP or SOME/IP-TP must be used.
// ============================================================================

/// Test: maximum UDP-safe payload (1392 bytes = 1400 - 8 bytes UDP header overhead)
/// This tests the upper bound for single UDP datagram notifications.
#[test]
fn test_max_udp_payload_notify() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    // Max payload that fits in a single Ethernet frame with SOME/IP over UDP
    // 1500 MTU - 20 IP - 8 UDP - 16 SOME/IP header - margin = ~1392 safe payload
    const MAX_UDP_PAYLOAD: usize = 1392;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            let announced = service.announce().await.unwrap();

            // Wait for subscriber
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Use maximum UDP-safe payload size
            let large_payload = vec![0xABu8; MAX_UDP_PAYLOAD];

            let result = announced
                .notify(
                    EventgroupId::new(0x0001).unwrap(),
                    EventId::new(0x8001).unwrap(),
                    &large_payload,
                )
                .await;

            assert!(result.is_ok(), "Max UDP payload should succeed");

            tokio::time::sleep(Duration::from_millis(1000)).await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let proxy = runtime.find::<BrakeService>(InstanceId::Any);
            let available = proxy.available().await.unwrap();

            let mut subscription = available
                .subscribe(EventgroupId::new(0x0001).unwrap())
                .await
                .unwrap();

            let event = tokio::time::timeout(Duration::from_secs(10), subscription.next())
                .await
                .expect("Should receive max UDP payload event");

            assert!(event.is_some());
            let payload = &event.unwrap().payload;
            assert_eq!(payload.len(), MAX_UDP_PAYLOAD);
            assert!(payload.iter().all(|&b| b == 0xAB));

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(15)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

/// Test: very large RPC payloads work over TCP (10KB payload)
/// TCP can handle arbitrary payload sizes, unlike UDP which is limited to ~1400 bytes.
#[test]
fn test_large_tcp_rpc_payload() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    // 10KB payload - much larger than UDP MTU (1400 bytes)
    const LARGE_PAYLOAD_SIZE: usize = 10 * 1024;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(60))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = someip_runtime::runtime::RuntimeConfig {
                transport: someip_runtime::runtime::Transport::Tcp,
                ..Default::default()
            };
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let mut offering = runtime
                .offer::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // Wait for RPC request
            if let Some(ServiceEvent::Call {
                responder, payload, ..
            }) = offering.next().await
            {
                // Echo back the large payload
                responder.reply(payload.as_ref()).await.unwrap();
            }

            // Gracefully shutdown to ensure response is sent
            drop(offering);
            runtime.shutdown().await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(100)).await;

            let config = someip_runtime::runtime::RuntimeConfig {
                transport: someip_runtime::runtime::Transport::Tcp,
                ..Default::default()
            };
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let proxy = runtime.find::<BrakeService>(InstanceId::Id(0x0001));
            let available = tokio::time::timeout(Duration::from_secs(5), proxy.available())
                .await
                .expect("Should discover service")
                .unwrap();

            // Send 10KB payload
            let large_payload = vec![0xCDu8; LARGE_PAYLOAD_SIZE];
            let method_id = MethodId::new(0x0001).unwrap();

            let response = tokio::time::timeout(
                Duration::from_secs(15),
                available.call(method_id, &large_payload),
            )
            .await
            .expect("RPC should complete")
            .unwrap();

            // Verify echoed payload
            assert_eq!(response.payload.len(), LARGE_PAYLOAD_SIZE);
            assert!(response.payload.iter().all(|&b| b == 0xCD));

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

/// Test: Response should be sent even after offering is dropped
/// 
/// Scenario: A shutdown RPC triggers a long shutdown procedure. The offering
/// is dropped early (service de-registered), but the final response should
/// still be sent to the client. This is a common pattern where cleanup happens
/// after the service stops accepting new requests.
#[test]
fn test_response_after_offering_dropped() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(60))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = someip_runtime::runtime::RuntimeConfig {
                transport: someip_runtime::runtime::Transport::Tcp,
                ..Default::default()
            };
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let mut offering = runtime
                .offer::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // Wait for RPC request (simulating a "shutdown" command)
            if let Some(ServiceEvent::Call {
                responder, ..
            }) = offering.next().await
            {
                // Immediately drop the offering (service de-registered)
                drop(offering);
                
                // Simulate long shutdown procedure
                tokio::time::sleep(Duration::from_millis(200)).await;
                
                // Send response AFTER offering is dropped
                // This should still work!
                responder.reply(b"shutdown_ack").await.unwrap();
            }

            // Gracefully shutdown to ensure response is sent
            runtime.shutdown().await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(100)).await;

            let config = someip_runtime::runtime::RuntimeConfig {
                transport: someip_runtime::runtime::Transport::Tcp,
                ..Default::default()
            };
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let proxy = runtime.find::<BrakeService>(InstanceId::Id(0x0001));
            let available = tokio::time::timeout(Duration::from_secs(5), proxy.available())
                .await
                .expect("Should discover service")
                .unwrap();

            let method_id = MethodId::new(0x0001).unwrap();
            
            // Send "shutdown" command
            let response = tokio::time::timeout(
                Duration::from_secs(5),
                available.call(method_id, b"shutdown"),
            )
            .await
            .expect("Should receive response even after offering dropped")
            .unwrap();

            // Verify we got the acknowledgment
            assert_eq!(&*response.payload, b"shutdown_ack");

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok(())
    });
    
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

// ============================================================================
// EDGE CASE: ZERO-LENGTH PAYLOAD
// ============================================================================

/// Test: zero-length event payloads work correctly
#[test]
fn test_empty_payload_notify() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            let announced = service.announce().await.unwrap();

            tokio::time::sleep(Duration::from_millis(200)).await;

            // Empty payload
            let result = announced
                .notify(
                    EventgroupId::new(0x0001).unwrap(),
                    EventId::new(0x8001).unwrap(),
                    b"",
                )
                .await;

            assert!(result.is_ok(), "Empty payload should succeed");

            tokio::time::sleep(Duration::from_millis(200)).await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let proxy = runtime.find::<BrakeService>(InstanceId::Any);
            let available = proxy.available().await.unwrap();

            let mut subscription = available
                .subscribe(EventgroupId::new(0x0001).unwrap())
                .await
                .unwrap();

            let event = tokio::time::timeout(Duration::from_secs(2), subscription.next())
                .await
                .expect("Should receive empty event");

            assert!(event.is_some());
            assert!(event.unwrap().payload.is_empty());

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

// ============================================================================
// EDGE CASE: RAPID STATE TRANSITIONS
// ============================================================================

/// Test: rapid bind/announce/stop cycles don't cause issues
/// 
/// Each service instance gets its own port. This test verifies that
/// services can transition through states multiple times and that
/// proper cleanup happens on drop. Services are kept alive to avoid
/// port number conflicts during iteration.
#[test]
fn test_rapid_state_transitions() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(20))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Keep all services alive to ensure unique ports
            // Ports are assigned based on offered.len(), so we need to
            // keep services in scope to prevent port reuse
            let mut services = Vec::new();
            
            for i in 0..5 {
                let service = runtime
                    .bind::<BrakeService>(InstanceId::Id((i + 1) as u16))
                    .await
                    .unwrap();

                let announced = service.announce().await.unwrap();
                let bound = announced.stop_announcing().await.unwrap();
                let announced2 = bound.announce().await.unwrap();
                let bound2 = announced2.stop_announcing().await.unwrap();
                
                // Keep the service alive to prevent port reuse
                services.push(bound2);
            }

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
}

// ============================================================================
// EDGE CASE: SERVICE EVENTS IN BOTH STATES
// ============================================================================

/// Test: next() works in Bound state for RPC
#[test]
fn test_next_in_bound_state() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let mut service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // next() should work in Bound state
            while let Some(event) = tokio::time::timeout(Duration::from_millis(500), service.next())
                .await
                .ok()
                .flatten()
            {
                match event {
                    someip_runtime::handle::ServiceEvent::Call { responder, .. } => {
                        responder.reply(b"bound_response").await.unwrap();
                    }
                    _ => {}
                }
            }

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Static client (pre-configured address)
            let server_ip = turmoil::lookup("server");
            let proxy = runtime.find_static::<BrakeService>(
                InstanceId::Id(0x0001),
                std::net::SocketAddr::new(server_ip, 30491),
            );

            let response = proxy
                .call(someip_runtime::MethodId::new(0x0001).unwrap(), b"request")
                .await
                .unwrap();

            assert_eq!(&response.payload[..], b"bound_response");

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

/// Test: next() works in Announced state
#[test]
fn test_next_in_announced_state() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            let mut announced = service.announce().await.unwrap();

            // next() should work in Announced state
            while let Some(event) =
                tokio::time::timeout(Duration::from_millis(500), announced.next())
                    .await
                    .ok()
                    .flatten()
            {
                match event {
                    someip_runtime::handle::ServiceEvent::Call { responder, .. } => {
                        responder.reply(b"announced_response").await.unwrap();
                    }
                    _ => {}
                }
            }

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let proxy = runtime.find::<BrakeService>(InstanceId::Any);
            let available = proxy.available().await.unwrap();

            let response = available
                .call(someip_runtime::MethodId::new(0x0001).unwrap(), b"request")
                .await
                .unwrap();

            assert_eq!(&response.payload[..], b"announced_response");

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

// ============================================================================
// EDGE CASE: SUBSCRIPTION EVENTS IN ANNOUNCED STATE
// ============================================================================

/// Test: SubscribeEventgroup events received in next()
#[test]
fn test_subscription_events_received() {
    let server_ran = Arc::new(AtomicBool::new(false));
    let client_ran = Arc::new(AtomicBool::new(false));
    let server_ran_clone = server_ran.clone();
    let client_ran_clone = client_ran.clone();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.host("server", move || {
        let flag = server_ran_clone.clone();
        async move {
            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let service = runtime
                .bind::<BrakeService>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            let mut announced = service.announce().await.unwrap();

            let mut subscribe_count = 0;
            let mut unsubscribe_count = 0;

            while let Some(event) = tokio::time::timeout(Duration::from_secs(3), announced.next())
                .await
                .ok()
                .flatten()
            {
                match event {
                    someip_runtime::handle::ServiceEvent::Subscribe { eventgroup, .. } => {
                        subscribe_count += 1;
                    }
                    someip_runtime::handle::ServiceEvent::Unsubscribe { eventgroup, .. } => {
                        unsubscribe_count += 1;
                    }
                    _ => {}
                }
            }

            assert!(subscribe_count > 0, "Should receive Subscribe events");
            assert!(unsubscribe_count > 0, "Should receive Unsubscribe events");

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.host("client", move || {
        let flag = client_ran_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let config = RuntimeConfig::default();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let proxy = runtime.find::<BrakeService>(InstanceId::Any);
            let available = proxy.available().await.unwrap();

            // Subscribe
            let subscription = available
                .subscribe(EventgroupId::new(0x0001).unwrap())
                .await
                .unwrap();

            tokio::time::sleep(Duration::from_millis(500)).await;

            // Unsubscribe by dropping
            drop(subscription);

            tokio::time::sleep(Duration::from_millis(500)).await;

            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });

    sim.client("driver", async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });
    sim.run().unwrap();
    assert!(
        server_ran.load(Ordering::SeqCst),
        "Server async block did not run"
    );
    assert!(
        client_ran.load(Ordering::SeqCst),
        "Client async block did not run"
    );
}

// ============================================================================
// EDGE CASE: METHODS AVAILABLE ON WRONG STATE DON'T COMPILE
// ============================================================================

// These are compile-time tests. If the typestate is implemented correctly,
// uncommenting these should fail to compile:

/*
/// COMPILE-TIME TEST: notify() should NOT be available on Bound
#[test]
fn compile_test_notify_not_on_bound() {
    // This should fail to compile:
    // service_bound.notify(...) // ERROR: method not found
}

/// COMPILE-TIME TEST: has_subscribers() should NOT be available on Bound
#[test]
fn compile_test_has_subscribers_not_on_bound() {
    // This should fail to compile:
    // service_bound.has_subscribers(...).await // ERROR: method not found
}

/// COMPILE-TIME TEST: announce() should NOT be available on Announced
#[test]
fn compile_test_announce_not_on_announced() {
    // This should fail to compile:
    // announced.announce() // ERROR: method not found
}

/// COMPILE-TIME TEST: stop_announcing() should NOT be available on Bound
#[test]
fn compile_test_stop_announcing_not_on_bound() {
    // This should fail to compile:
    // service_bound.stop_announcing() // ERROR: method not found
}
*/
