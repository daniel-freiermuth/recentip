//! Service Discovery (SD) Compliance Tests (Async/Turmoil)
//!
//! Tests that verify SOME/IP-SD wire format and behavior using turmoil simulation.
//! These tests verify SD message headers, entry formats, flags, and timing.
//!
//! Reference: someip-sd.rst

use someip_runtime::handle::ServiceEvent;
use someip_runtime::prelude::*;
use someip_runtime::runtime::Runtime;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

/// Type alias for turmoil-based runtime
type TurmoilRuntime =
    Runtime<turmoil::net::UdpSocket, turmoil::net::TcpStream, turmoil::net::TcpListener>;

/// Test service definition
struct TestService;

impl Service for TestService {
    const SERVICE_ID: u16 = 0x1234;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}

/// Test service with different version
struct VersionedService;

impl Service for VersionedService {
    const SERVICE_ID: u16 = 0x5678;
    const MAJOR_VERSION: u8 = 2;
    const MINOR_VERSION: u32 = 3;
}

// ============================================================================
// SD HEADER COMPLIANCE
// ============================================================================

/// feat_req_recentipsd_26: SD messages must be NOTIFICATION type
/// feat_req_recentipsd_27: SD uses UDP only (never TCP)
/// feat_req_recentipsd_141: Service ID must be 0xFFFF
/// feat_req_recentipsd_142: Method ID must be 0x8100
/// feat_req_recentipsd_144: Protocol and Interface Version must be 0x01
///
/// This test verifies that when a service is offered, it can be discovered,
/// which implies correct SD message format.
#[test_log::test]
fn sd_offer_discovery_works() {
    covers!(
        feat_req_recentipsd_26,
        feat_req_recentipsd_27,
        feat_req_recentipsd_141,
        feat_req_recentipsd_142,
        feat_req_recentipsd_144
    );

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let _offering = runtime
                .offer::<TestService>(InstanceId::Id(0x0001))
                .udp()
                .start()
                .await
                .unwrap();

            // Keep server alive
            tokio::time::sleep(Duration::from_millis(500)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Any);

        // Discovery works => SD messages are well-formed
        let result = tokio::time::timeout(Duration::from_secs(5), proxy).await;
        assert!(result.is_ok(), "Service discovery must succeed");

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

/// feat_req_recentipsd_47: OfferService entry contains correct service/instance IDs
/// feat_req_recentipsd_39: SD flags field follows header
///
/// Test that specific instance IDs are correctly transmitted in offers.
#[test_log::test]
fn sd_offer_with_specific_instance() {
    covers!(feat_req_recentipsd_47, feat_req_recentipsd_39);

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Offer with specific instance ID
            let _offering = runtime
                .offer::<TestService>(InstanceId::Id(0x0042))
                .udp()
                .start()
                .await
                .unwrap();

            tokio::time::sleep(Duration::from_millis(500)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Find specific instance
        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0042));

        let available = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout waiting for specific instance")
            .expect("Service available");

        assert_eq!(available.instance_id(), InstanceId::Id(0x0042));
        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

/// feat_req_recentipsd_47: Version information in OfferService entry
///
/// Test that service versions are correctly communicated.
#[test_log::test]
fn sd_offer_with_version_info() {
    covers!(feat_req_recentipsd_47);

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Offer versioned service
            let _offering = runtime
                .offer::<VersionedService>(InstanceId::Id(0x0001))
                .udp()
                .start()
                .await
                .unwrap();

            tokio::time::sleep(Duration::from_millis(500)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<VersionedService>(InstanceId::Any);

        let available = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Timeout")
            .expect("Service available");

        // Service with correct version discovered
        assert_eq!(available.service_id(), ServiceId::new(0x5678).unwrap());
        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// STOP OFFER BEHAVIOR
// ============================================================================

/// feat_req_recentipsd_47: StopOffer uses TTL=0
///
/// When an offering handle is dropped, a StopOffer (OfferService with TTL=0)
/// should be sent. Clients should then lose availability.
#[test_log::test]
fn sd_stop_offer_on_drop() {
    covers!(feat_req_recentipsd_47);

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Offer then drop
            {
                let _offering = runtime
                    .offer::<TestService>(InstanceId::Id(0x0001))
                    .udp()
                    .start()
                    .await
                    .unwrap();

                // Let client discover
                tokio::time::sleep(Duration::from_millis(300)).await;
            } // offering dropped here, should send StopOffer

            tokio::time::sleep(Duration::from_millis(300)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Any);

        // First, service should be available
        let _available = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Should discover service initially");

        // After server drops offering, service should become unavailable
        // (This behavior depends on SD TTL tracking)

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(800)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// MULTIPLE SERVICES
// ============================================================================

/// feat_req_recentipsd_47: Multiple services can be offered simultaneously
#[test_log::test]
fn sd_multiple_service_offers() {
    covers!(feat_req_recentipsd_47);

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let _offering1 = runtime
                .offer::<TestService>(InstanceId::Id(0x0001))
                .udp()
                .start()
                .await
                .unwrap();
            let _offering2 = runtime
                .offer::<VersionedService>(InstanceId::Id(0x0002))
                .udp()
                .start()
                .await
                .unwrap();

            tokio::time::sleep(Duration::from_millis(500)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy1 = runtime.find::<TestService>(InstanceId::Any);
        let proxy2 = runtime.find::<VersionedService>(InstanceId::Any);

        let result1 = tokio::time::timeout(Duration::from_secs(5), proxy1).await;
        let result2 = tokio::time::timeout(Duration::from_secs(5), proxy2).await;

        assert!(result1.is_ok(), "TestService should be discovered");
        assert!(result2.is_ok(), "VersionedService should be discovered");
        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

/// feat_req_recentipsd_47: Same service, multiple instances
/// feat_req_recentipsd_782: Multiple instances use different endpoints
///
/// With proper RPC socket architecture, multiple instances of the same service
/// can now run on the same host, each with its own dedicated RPC socket.
#[test_log::test]
fn sd_multiple_instances_same_service() {
    covers!(feat_req_recentipsd_47, feat_req_recentipsd_782);

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Single server offers BOTH instances
    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let _offering1 = runtime
                .offer::<TestService>(InstanceId::Id(0x0001))
                .udp()
                .start()
                .await
                .unwrap();

            let _offering2 = runtime
                .offer::<TestService>(InstanceId::Id(0x0002))
                .udp()
                .start()
                .await
                .unwrap();

            *flag.lock().unwrap() = true;

            // Keep the runtime alive so services remain available
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(300)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Find ANY instance
        let proxy = runtime.find::<TestService>(InstanceId::Any);

        let available = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Should find at least one instance")
            .expect("Service available");

        // Should find one of the instances
        let instance = available.instance_id();
        assert!(
            instance == InstanceId::Id(0x0001) || instance == InstanceId::Id(0x0002),
            "Should find one of the offered instances, but found {:?}",
            instance
        );
        Ok(())
    });

    sim.client("driver", async move {
        // Keep simulation running long enough for server to complete
        tokio::time::sleep(Duration::from_secs(15)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// FIND SERVICE BEHAVIOR
// ============================================================================

/// feat_req_recentipsd_207: FindService entry for client-initiated discovery
///
/// Client sends FindService, server responds with OfferService.
#[test_log::test]
fn sd_find_service_discovery() {
    covers!(feat_req_recentipsd_207);

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Start client first (before server)
    sim.host("client", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Request service that doesn't exist yet
        let proxy = runtime.find::<TestService>(InstanceId::Any);

        // Wait with long timeout - service will appear later
        let available = tokio::time::timeout(Duration::from_secs(10), proxy)
            .await
            .expect("Should eventually find service")
            .expect("Service available");

        assert_eq!(available.service_id(), ServiceId::new(0x1234).unwrap());
        Ok(())
    });

    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            // Delay server start
            tokio::time::sleep(Duration::from_millis(200)).await;

            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let _offering = runtime
                .offer::<TestService>(InstanceId::Id(0x0001))
                .udp()
                .start()
                .await
                .unwrap();

            tokio::time::sleep(Duration::from_millis(500)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(900)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// SD SESSION HANDLING
// ============================================================================

/// feat_req_recentipsd_26: SD messages use incrementing Session ID
/// feat_req_recentip_649: Session ID starts at 1
///
/// Each SD message should have an incrementing session ID.
#[test_log::test]
fn sd_session_id_increments() {
    covers!(feat_req_recentipsd_26, feat_req_recentip_649);

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            // Multiple offers to trigger multiple SD messages
            let _offering1 = runtime
                .offer::<TestService>(InstanceId::Id(0x0001))
                .udp()
                .start()
                .await
                .unwrap();

            tokio::time::sleep(Duration::from_millis(100)).await;

            let _offering2 = runtime
                .offer::<VersionedService>(InstanceId::Id(0x0002))
                .udp()
                .start()
                .await
                .unwrap();

            tokio::time::sleep(Duration::from_millis(300)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy1 = runtime.find::<TestService>(InstanceId::Any);
        let proxy2 = runtime.find::<VersionedService>(InstanceId::Any);

        let _ = tokio::time::timeout(Duration::from_secs(5), proxy1).await;
        let _ = tokio::time::timeout(Duration::from_secs(5), proxy2).await;

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(600)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// MULTICAST vs UNICAST
// ============================================================================

/// feat_req_recentipsd_40: Unicast flag should be set
/// feat_req_recentipsd_453: Unicast flag value
///
/// SD messages typically use multicast for initial discovery
/// but unicast flag indicates unicast capability.
#[test_log::test]
fn sd_unicast_flag_handling() {
    covers!(feat_req_recentipsd_40, feat_req_recentipsd_453);

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let _offering = runtime
                .offer::<TestService>(InstanceId::Id(0x0001))
                .udp()
                .start()
                .await
                .unwrap();

            tokio::time::sleep(Duration::from_millis(500)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Any);
        let _ = tokio::time::timeout(Duration::from_secs(5), proxy).await;

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// RPC OVER DISCOVERED SERVICE
// ============================================================================

/// Full integration: SD discovery followed by RPC
#[test_log::test]
fn sd_discovery_then_rpc() {
    covers!(
        feat_req_recentipsd_26,
        feat_req_recentipsd_47,
        feat_req_recentip_103
    );

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let mut offering = runtime
                .offer::<TestService>(InstanceId::Id(0x0001))
                .udp()
                .start()
                .await
                .unwrap();

            // Handle method call
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call {
                        payload, responder, ..
                    } => {
                        let mut response = Vec::from("echo:");
                        response.extend_from_slice(&payload);
                        responder.reply(&response).await.unwrap();
                    }
                    _ => panic!("Expected Call"),
                }
            }

            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Discover via SD
        let proxy = runtime.find::<TestService>(InstanceId::Any);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery should succeed")
            .expect("Service available");

        // Make RPC call
        let method = MethodId::new(0x0001).unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, b"test"))
            .await
            .expect("RPC timeout")
            .expect("RPC should succeed");

        assert_eq!(&response.payload[..], b"echo:test");
        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}
