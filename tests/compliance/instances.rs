//! Service Instance Management Compliance Tests
//!
//! Tests multiple service instances, instance IDs, and service lifecycle.
//!
//! Key requirements tested:
//! - feat_req_recentip_541: Different services have different Service IDs
//! - feat_req_recentip_544: Different instances have different Service ID + Instance ID
//! - feat_req_recentip_636: Instances identified by different Instance IDs
//! - feat_req_recentip_648: Messages dispatched to correct instance
//! - feat_req_recentip_967: Different instances on same server offered on different ports
//! - feat_req_recentip_445: Different services can share same port
//! - feat_req_recentip_446: Instance identified by Service ID + Instance ID + IP + Port

use someip_runtime::prelude::*;
use someip_runtime::runtime::Runtime;
use someip_runtime::handle::ServiceEvent;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

/// Type alias for turmoil-based runtime
type TurmoilRuntime = Runtime<turmoil::net::UdpSocket>;

/// Test service definitions
struct ServiceA;
impl Service for ServiceA {
    const SERVICE_ID: u16 = 0x1234;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}

struct ServiceB;
impl Service for ServiceB {
    const SERVICE_ID: u16 = 0x5678;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}

// ============================================================================
// Multiple Service Instances Tests
// ============================================================================

/// feat_req_recentip_636: Service instances identified by different Instance IDs
///
/// Service-Instances of the same Service are identified through different
/// Instance IDs.
#[test]
fn multiple_instances_have_different_ids() {
    covers!(feat_req_recentip_636);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    // Server offers 3 instances of the same service
    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let _offering1 = runtime
            .offer::<ServiceA>(InstanceId::Id(0x0001))
            .await
            .unwrap();
        let _offering2 = runtime
            .offer::<ServiceA>(InstanceId::Id(0x0002))
            .await
            .unwrap();
        let _offering3 = runtime
            .offer::<ServiceA>(InstanceId::Id(0x0003))
            .await
            .unwrap();

        // Keep offerings alive
        tokio::time::sleep(Duration::from_secs(10)).await;
        *flag.lock().unwrap() = true;
        Ok(())
        }
    });

    // Client discovers all 3 instances
    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        // Discover instance 1
        let proxy1 = runtime.find::<ServiceA>(InstanceId::Id(0x0001));
        tokio::time::timeout(Duration::from_secs(5), proxy1.available())
            .await
            .expect("Should discover instance 1");

        // Discover instance 2
        let proxy2 = runtime.find::<ServiceA>(InstanceId::Id(0x0002));
        tokio::time::timeout(Duration::from_secs(5), proxy2.available())
            .await
            .expect("Should discover instance 2");

        // Discover instance 3
        let proxy3 = runtime.find::<ServiceA>(InstanceId::Id(0x0003));
        tokio::time::timeout(Duration::from_secs(5), proxy3.available())
            .await
            .expect("Should discover instance 3");

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_secs(11)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

/// feat_req_recentip_648: Messages dispatched to correct instance
///
/// If a server runs different instances of the same service, messages
/// belonging to the different instances shall be dispatched correctly.
/// 
/// NOTE: Per SOME/IP spec feat_req_recentipsd_782, multiple instances must use
/// different endpoints (ports). This test uses separate hosts for each instance.
#[test]
fn messages_dispatched_to_correct_instance() {
    covers!(feat_req_recentip_648);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    // Server 1 offers instance 1
    sim.host("server1", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let mut offering1 = runtime
            .offer::<ServiceA>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Handle request for instance 1
        if let Some(event) = offering1.next().await {
            if let ServiceEvent::Call { payload, responder, .. } = event {
                assert_eq!(payload.as_ref(), b"for_instance_1");
                responder.reply(b"from_instance_1").await.unwrap();
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        *flag.lock().unwrap() = true;
        Ok(())
        }
    });

    // Server 2 offers instance 2
    sim.host("server2", || async {
        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let mut offering2 = runtime
            .offer::<ServiceA>(InstanceId::Id(0x0002))
            .await
            .unwrap();

        // Handle request for instance 2
        if let Some(event) = offering2.next().await {
            if let ServiceEvent::Call { payload, responder, .. } = event {
                assert_eq!(payload.as_ref(), b"for_instance_2");
                responder.reply(b"from_instance_2").await.unwrap();
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    // Client calls specific instances
    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        // Call instance 1 specifically
        let proxy1 = runtime.find::<ServiceA>(InstanceId::Id(0x0001));
        let proxy1 = tokio::time::timeout(Duration::from_secs(5), proxy1.available())
            .await
            .expect("Should discover instance 1");

        let response1 = tokio::time::timeout(
            Duration::from_secs(15),
            proxy1.call(MethodId::new(0x0001), b"for_instance_1"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");

        assert_eq!(response1.payload.as_ref(), b"from_instance_1");

        // Call instance 2 specifically
        let proxy2 = runtime.find::<ServiceA>(InstanceId::Id(0x0002));
        let proxy2 = tokio::time::timeout(Duration::from_secs(5), proxy2.available())
            .await
            .expect("Should discover instance 2");

        let response2 = tokio::time::timeout(
            Duration::from_secs(15),
            proxy2.call(MethodId::new(0x0001), b"for_instance_2"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");

        assert_eq!(response2.payload.as_ref(), b"from_instance_2");

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_secs(25)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

/// feat_req_recentip_648 + feat_req_recentipsd_782: Two instances on same host
///
/// Tests that two instances of the same service can run on the same host
/// with proper message routing. This works because each instance gets its own
/// dedicated RPC socket (separate from the SD socket on port 30490).
#[test]
fn two_instances_same_host() {
    covers!(feat_req_recentip_648);
    covers!(feat_req_recentipsd_782);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    // Single server host offering TWO instances of the same service
    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

            // Offer instance 1
            let mut offering1 = runtime
                .offer::<ServiceA>(InstanceId::Id(0x0001))
                .await
                .unwrap();

            // Offer instance 2 on the SAME runtime/host
            let mut offering2 = runtime
                .offer::<ServiceA>(InstanceId::Id(0x0002))
                .await
                .unwrap();

            // Handle requests concurrently
            tokio::select! {
                Some(event) = offering1.next() => {
                    if let ServiceEvent::Call { payload, responder, .. } = event {
                        assert_eq!(payload.as_ref(), b"for_instance_1");
                        responder.reply(b"from_instance_1").await.unwrap();
                    }
                }
                Some(event) = offering2.next() => {
                    if let ServiceEvent::Call { payload, responder, .. } = event {
                        assert_eq!(payload.as_ref(), b"for_instance_2");
                        responder.reply(b"from_instance_2").await.unwrap();
                    }
                }
            }

            // Handle the second request
            tokio::select! {
                Some(event) = offering1.next() => {
                    if let ServiceEvent::Call { payload, responder, .. } = event {
                        assert_eq!(payload.as_ref(), b"for_instance_1");
                        responder.reply(b"from_instance_1").await.unwrap();
                    }
                }
                Some(event) = offering2.next() => {
                    if let ServiceEvent::Call { payload, responder, .. } = event {
                        assert_eq!(payload.as_ref(), b"for_instance_2");
                        responder.reply(b"from_instance_2").await.unwrap();
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    // Client calls both instances
    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        // Call instance 1
        let proxy1 = runtime.find::<ServiceA>(InstanceId::Id(0x0001));
        let proxy1 = tokio::time::timeout(Duration::from_secs(5), proxy1.available())
            .await
            .expect("Should discover instance 1");

        let response1 = tokio::time::timeout(
            Duration::from_secs(15),
            proxy1.call(MethodId::new(0x0001), b"for_instance_1"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");

        assert_eq!(response1.payload.as_ref(), b"from_instance_1");

        // Call instance 2
        let proxy2 = runtime.find::<ServiceA>(InstanceId::Id(0x0002));
        let proxy2 = tokio::time::timeout(Duration::from_secs(5), proxy2.available())
            .await
            .expect("Should discover instance 2");

        let response2 = tokio::time::timeout(
            Duration::from_secs(15),
            proxy2.call(MethodId::new(0x0001), b"for_instance_2"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");

        assert_eq!(response2.payload.as_ref(), b"from_instance_2");

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_secs(25)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// Service ID Uniqueness Tests
// ============================================================================

/// feat_req_recentip_541: Different services have different Service IDs
#[test]
fn different_services_have_different_service_ids() {
    covers!(feat_req_recentip_541);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    // Server offers two different services
    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let _offering_a = runtime
            .offer::<ServiceA>(InstanceId::Id(0x0001))
            .await
            .unwrap();
        let _offering_b = runtime
            .offer::<ServiceB>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(10)).await;
        *flag.lock().unwrap() = true;
        Ok(())
        }
    });

    // Client discovers both services separately
    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        // Discover ServiceA
        let proxy_a = runtime.find::<ServiceA>(InstanceId::Any);
        tokio::time::timeout(Duration::from_secs(5), proxy_a.available())
            .await
            .expect("Should discover service A");

        // Discover ServiceB
        let proxy_b = runtime.find::<ServiceB>(InstanceId::Any);
        tokio::time::timeout(Duration::from_secs(5), proxy_b.available())
            .await
            .expect("Should discover service B");

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_secs(11)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

/// feat_req_recentip_544: Different instances have unique Service ID + Instance ID
#[test]
fn instance_uniquely_identified_by_service_and_instance_id() {
    covers!(feat_req_recentip_544);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    // Two different services, each with instance ID 1
    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let mut offering_a = runtime
            .offer::<ServiceA>(InstanceId::Id(0x0001))
            .await
            .unwrap();
        let mut offering_b = runtime
            .offer::<ServiceB>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Handle call for ServiceA
        if let Some(event) = tokio::time::timeout(
            Duration::from_secs(10),
            offering_a.next()
        ).await.ok().flatten() {
            if let ServiceEvent::Call { responder, .. } = event {
                responder.reply(b"resp_a").await.unwrap();
            }
        }

        // Handle call for ServiceB
        if let Some(event) = tokio::time::timeout(
            Duration::from_secs(10),
            offering_b.next()
        ).await.ok().flatten() {
            if let ServiceEvent::Call { responder, .. } = event {
                responder.reply(b"resp_b").await.unwrap();
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        *flag.lock().unwrap() = true;
        Ok(())
        }
    });

    // Client calls both services (same instance ID, different service IDs)
    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        // Call ServiceA instance 1
        let proxy_a = runtime.find::<ServiceA>(InstanceId::Id(0x0001));
        let proxy_a = tokio::time::timeout(Duration::from_secs(5), proxy_a.available())
            .await
            .expect("Should discover ServiceA");

        let response_a = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_a.call(MethodId::new(0x0001), b"call_a"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");

        assert_eq!(response_a.payload.as_ref(), b"resp_a");

        // Call ServiceB instance 1
        let proxy_b = runtime.find::<ServiceB>(InstanceId::Id(0x0001));
        let proxy_b = tokio::time::timeout(Duration::from_secs(5), proxy_b.available())
            .await
            .expect("Should discover ServiceB");

        let response_b = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_b.call(MethodId::new(0x0001), b"call_b"),
        )
        .await
        .expect("Timeout")
        .expect("Call should succeed");

        assert_eq!(response_b.payload.as_ref(), b"resp_b");

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(5000)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// Instance Discovery Tests
// ============================================================================

/// Client can request ANY instance and get one
#[test]
fn client_can_request_any_instance() {
    covers!(feat_req_recentip_636);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    // Server offers instance 42
    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let _offering = runtime
            .offer::<ServiceA>(InstanceId::Id(0x002A))  // 42 in hex
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(10)).await;
        *flag.lock().unwrap() = true;
        Ok(())
        }
    });

    // Client requests ANY instance
    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let proxy = runtime.find::<ServiceA>(InstanceId::Any);
        tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Should find an instance when using ANY");

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_secs(11)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

/// Client can request specific instance by ID
#[test]
fn client_can_request_specific_instance() {
    covers!(feat_req_recentip_636);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    // Server offers 2 instances
    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let _offering1 = runtime
            .offer::<ServiceA>(InstanceId::Id(0x0001))
            .await
            .unwrap();
        let _offering2 = runtime
            .offer::<ServiceA>(InstanceId::Id(0x0002))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(10)).await;
        *flag.lock().unwrap() = true;
        Ok(())
        }
    });

    // Client requests specific instance 2
    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let proxy = runtime.find::<ServiceA>(InstanceId::Id(0x0002));
        tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Should find specific instance");

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_secs(11)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

/// Requesting non-existent instance times out
#[test]
fn nonexistent_instance_not_found() {
    covers!(feat_req_recentip_636);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    // Server offers instance 1
    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let _offering = runtime
            .offer::<ServiceA>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(5)).await;
        *flag.lock().unwrap() = true;
        Ok(())
        }
    });

    // Client requests instance 99 that doesn't exist
    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let proxy = runtime.find::<ServiceA>(InstanceId::Id(0x0063));  // 99 in hex
        let result = tokio::time::timeout(Duration::from_secs(2), proxy.available()).await;

        assert!(result.is_err(), "Non-existent instance should not be found");

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_secs(6)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}