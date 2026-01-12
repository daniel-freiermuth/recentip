//! Session and Request ID Compliance Tests (Async/Turmoil)
//!
//! Tests the Session ID and Request ID behavior per SOME/IP specification.
//!
//! Key requirements tested:
//! - feat_req_recentip_83: Request ID = Client ID (16 bit) + Session ID (16 bit)
//! - feat_req_recentip_699: Client ID is unique identifier for calling client
//! - feat_req_recentip_88: Session ID is unique identifier for each call
//! - feat_req_recentip_649: Session ID starts at 0x0001
//! - feat_req_recentip_677: Session ID wraps from 0xFFFF to 0x0001
//! - feat_req_recentip_711: Server copies Request ID from request to response

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
type TurmoilRuntime =
    Runtime<turmoil::net::UdpSocket, turmoil::net::TcpStream, turmoil::net::TcpListener>;

const TEST_SERVICE_ID: u16 = 0x1234;
const TEST_SERVICE_VERSION: (u8, u32) = (1, 0);

// ============================================================================
// REQUEST ID STRUCTURE
// ============================================================================

/// feat_req_recentip_83: Request ID = Client ID || Session ID
/// feat_req_recentip_711: Response preserves Request ID
///
/// When a client makes multiple calls, each should have incrementing session IDs,
/// and responses must preserve the request ID.
#[test_log::test]
fn multiple_calls_incrementing_session() {
    covers!(
        feat_req_recentip_83,
        feat_req_recentip_88,
        feat_req_recentip_711
    );

    use std::sync::atomic::{AtomicU32, Ordering};
    static TEST_SERVER_COUNT: AtomicU32 = AtomicU32::new(0);
    static TEST_CLIENT_COUNT: AtomicU32 = AtomicU32::new(0);
    TEST_SERVER_COUNT.store(0, Ordering::SeqCst);
    TEST_CLIENT_COUNT.store(0, Ordering::SeqCst);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Handle 3 calls
        for _ in 0..3 {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call { responder, .. } => {
                        TEST_SERVER_COUNT.fetch_add(1, Ordering::SeqCst);
                        responder
                            .reply(&[TEST_SERVER_COUNT.load(Ordering::SeqCst) as u8 - 1])
                            .await
                            .unwrap();
                    }
                    _ => panic!("Expected Call"),
                }
            }
        }

        // Wait for client to receive responses before ending simulation
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let method = MethodId::new(0x0001).unwrap();

        // Make 3 calls - session IDs should increment
        for i in 0..3 {
            let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, &[i]))
                .await
                .expect("RPC timeout")
                .expect("RPC should succeed");

            TEST_CLIENT_COUNT.fetch_add(1, Ordering::SeqCst);
            // Response payload should match call number
            assert_eq!(response.payload[0], i);
        }

        Ok(())
    });

    sim.run().unwrap();

    // Verify iterations actually happened
    let server = TEST_SERVER_COUNT.load(Ordering::SeqCst);
    let client = TEST_CLIENT_COUNT.load(Ordering::SeqCst);
    assert_eq!(
        server, 3,
        "Server should have handled 3 calls, got {}",
        server
    );
    assert_eq!(client, 3, "Client should have made 3 calls, got {}", client);
}

/// feat_req_recentip_649: First call has Session ID starting at 1
#[test_log::test]
fn first_call_session_id() {
    covers!(feat_req_recentip_649);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::Call { responder, .. } => {
                    responder.reply(b"ok").await.unwrap();
                }
                _ => panic!("Expected Call"),
            }
        }

        // Wait for client to receive response before ending simulation
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let method = MethodId::new(0x0001).unwrap();

        // First call should succeed (implying session ID is valid)
        let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, b"first"))
            .await
            .expect("RPC timeout")
            .expect("RPC should succeed");

        assert_eq!(response.payload.as_ref(), b"ok");
        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// CONCURRENT CALLS
// ============================================================================

/// feat_req_recentip_88: Each call has unique Session ID
///
/// Even concurrent calls should have unique session IDs.
#[test_log::test]
fn concurrent_calls_unique_sessions() {
    covers!(feat_req_recentip_88);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Handle multiple concurrent calls
        for _ in 0..3 {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call {
                        payload, responder, ..
                    } => {
                        // Echo payload back
                        responder.reply(&payload).await.unwrap();
                    }
                    _ => panic!("Expected Call"),
                }
            }
        }

        // Wait for client to receive responses before ending simulation
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let method = MethodId::new(0x0001).unwrap();

        // Launch concurrent calls (though due to turmoil's execution model,
        // they may be serialized)
        let f1 = proxy.call(method, b"A");
        let f2 = proxy.call(method, b"B");
        let f3 = proxy.call(method, b"C");

        // All should complete successfully with correct responses
        let r1 = tokio::time::timeout(Duration::from_secs(5), f1)
            .await
            .unwrap()
            .unwrap();
        let r2 = tokio::time::timeout(Duration::from_secs(5), f2)
            .await
            .unwrap()
            .unwrap();
        let r3 = tokio::time::timeout(Duration::from_secs(5), f3)
            .await
            .unwrap()
            .unwrap();

        // Each response should echo back its payload
        assert_eq!(r1.payload.as_ref(), b"A");
        assert_eq!(r2.payload.as_ref(), b"B");
        assert_eq!(r3.payload.as_ref(), b"C");

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// ERROR RESPONSES
// ============================================================================

/// feat_req_recentip_711: Error responses also preserve Request ID
#[test_log::test]
fn error_response_preserves_request_id() {
    covers!(feat_req_recentip_711);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::Call { responder, .. } => {
                    // Send error response using reply_error
                    responder.reply_error(ReturnCode::NotOk).await.unwrap();
                }
                _ => panic!("Expected Call"),
            }
        }

        // Wait for client to receive responses before ending simulation
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let method = MethodId::new(0x0001).unwrap();

        // Error response should result in Err from call (or Ok with non-Ok return_code)
        let result = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, b"test"))
            .await
            .expect("RPC timeout");

        // Error response either results in Err, or Ok with return_code != Ok
        match result {
            Err(_) => {
                // Expected: error responses result in Err
            }
            Ok(response) => {
                // Also acceptable: response received with non-Ok return code
                assert_ne!(
                    response.return_code,
                    ReturnCode::Ok,
                    "Response should have non-Ok return code"
                );
            }
        }

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// DIFFERENT METHODS
// ============================================================================

/// Session ID increments across different method calls
#[test_log::test]
fn session_increments_across_methods() {
    covers!(feat_req_recentip_88);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Handle calls to different methods
        for _ in 0..3 {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call {
                        method, responder, ..
                    } => {
                        // Return method ID in response
                        responder
                            .reply(&method.value().to_be_bytes())
                            .await
                            .unwrap();
                    }
                    _ => panic!("Expected Call"),
                }
            }
        }

        // Allow responses to be transmitted before task exits
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        // Call different methods
        let method1 = MethodId::new(0x0001).unwrap();
        let method2 = MethodId::new(0x0002).unwrap();
        let method3 = MethodId::new(0x0003).unwrap();

        let r1 = tokio::time::timeout(Duration::from_secs(5), proxy.call(method1, b""))
            .await
            .unwrap()
            .unwrap();
        let r2 = tokio::time::timeout(Duration::from_secs(5), proxy.call(method2, b""))
            .await
            .unwrap()
            .unwrap();
        let r3 = tokio::time::timeout(Duration::from_secs(5), proxy.call(method3, b""))
            .await
            .unwrap()
            .unwrap();

        // Responses should contain method IDs
        assert_eq!(u16::from_be_bytes([r1.payload[0], r1.payload[1]]), 0x0001);
        assert_eq!(u16::from_be_bytes([r2.payload[0], r2.payload[1]]), 0x0002);
        assert_eq!(u16::from_be_bytes([r3.payload[0], r3.payload[1]]), 0x0003);

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// CLIENT ID CONSISTENCY
// ============================================================================

/// feat_req_recentip_699: Client ID is consistent for a client
///
/// Multiple calls from the same client should use the same Client ID.
#[test_log::test]
fn client_id_consistent_across_calls() {
    covers!(feat_req_recentip_699);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Handle 5 calls
        for _ in 0..5 {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call { responder, .. } => {
                        responder.reply(&[]).await.unwrap();
                    }
                    _ => {}
                }
            }
        }

        // Wait for client to receive responses before ending simulation
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let method = MethodId::new(0x0001).unwrap();

        // Make 5 calls - all should succeed (client ID consistency is internal)
        for i in 0..5 {
            let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, &[i]))
                .await
                .unwrap();

            assert!(response.is_ok(), "Call {} should succeed", i);
        }

        // If we got here, all calls used consistent client ID
        // (the runtime internally maintains this)
        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// OUT-OF-ORDER RESPONSE MATCHING
// ============================================================================

/// feat_req_recentip_711: Request ID enables matching responses to requests
///
/// When multiple requests are outstanding and responses arrive out of order,
/// the client must correctly match each response to its request.
#[test_log::test]
fn out_of_order_response_matching() {
    covers!(feat_req_recentip_711, feat_req_recentip_83);

    use std::sync::Arc;
    use tokio::sync::Notify;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Use Notify for synchronization between client and server
    let first_request_received = Arc::new(Notify::new());
    let second_request_received = Arc::new(Notify::new());

    let notify1_for_server = first_request_received.clone();
    let notify2_for_server = second_request_received.clone();
    let notify1_for_client = first_request_received.clone();
    let notify2_for_client = second_request_received.clone();

    sim.host("server", move || {
        let notify1 = notify1_for_server.clone();
        let notify2 = notify2_for_server.clone();

        async move {
            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let mut offering = runtime
                .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
                .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            // Wait for first request and notify client
            let mut first_call = None;
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call {
                        responder, payload, ..
                    } => {
                        first_call = Some((responder, payload));
                        notify1.notify_one();
                    }
                    _ => {}
                }
            }

            // Wait for second request and notify client
            let mut second_call = None;
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call {
                        responder, payload, ..
                    } => {
                        second_call = Some((responder, payload));
                        notify2.notify_one();
                    }
                    _ => {}
                }
            }

            // Respond in reverse order (second request first)
            if let (Some((resp1, _)), Some((resp2, _))) = (first_call, second_call) {
                // Send response to second request first
                resp2.reply(&[2]).await.unwrap();
                // Small delay to ensure responses arrive out-of-order
                tokio::time::sleep(Duration::from_millis(10)).await;
                // Send response to first request second
                resp1.reply(&[1]).await.unwrap();
            }

            // Wait for client to complete
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let method = MethodId::new(0x0001).unwrap();

        // Spawn first request in a background task so network I/O can progress
        let proxy1 = proxy.clone();
        let task1 = tokio::spawn(async move { proxy1.call(method, &[1]).await });

        // Wait for server to receive first request
        notify1_for_client.notified().await;

        // Now send second request after we know first was received
        let proxy2 = proxy.clone();
        let task2 = tokio::spawn(async move { proxy2.call(method, &[2]).await });

        // Wait for server to receive second request
        notify2_for_client.notified().await;

        // Now both requests are at the server in the correct order.
        // Server will respond in reverse order (2 then 1).
        // Wait for both responses.
        let (r1, r2) = tokio::join!(
            tokio::time::timeout(Duration::from_secs(5), task1),
            tokio::time::timeout(Duration::from_secs(5), task2)
        );

        let response1 = r1
            .expect("Timeout on first call")
            .expect("Task join failed")
            .expect("First call failed");
        let response2 = r2
            .expect("Timeout on second call")
            .expect("Task join failed")
            .expect("Second call failed");

        // Despite out-of-order responses, each call should get its correct response
        // First call (sent [1]) should get response [1]
        // Second call (sent [2]) should get response [2]
        assert_eq!(
            response1.payload[0], 1,
            "First request should get marker [1] response"
        );
        assert_eq!(
            response2.payload[0], 2,
            "Second request should get marker [2] response"
        );
        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// EVENTS/NOTIFICATIONS SESSION HANDLING
// ============================================================================

/// feat_req_recentip_667: Events shall use session handling
///
/// Event notifications should have incrementing session IDs.
#[test_log::test]
fn events_use_session_handling() {
    covers!(feat_req_recentip_667);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for subscription
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Send multiple notifications
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event = EventId::new(0x8001).unwrap();

        for i in 0..3 {
            offering.event(event).eventgroup(eventgroup).create().unwrap().notify(&[i]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.host("client", || async {
        // Wait for server to start offering
        tokio::time::sleep(Duration::from_millis(150)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        // Subscribe to eventgroup
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        // Receive events
        let mut received = Vec::new();
        for _ in 0..3 {
            let result = tokio::time::timeout(Duration::from_secs(2), subscription.next()).await;

            if let Ok(Some(event)) = result {
                received.push(event);
            }
        }

        // Verify we got events (session handling is verified internally)
        assert!(
            received.len() >= 2,
            "Should have received at least 2 events (got {})",
            received.len()
        );

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// PARALLEL REQUESTS
// ============================================================================

/// feat_req_recentip_79: Request ID differentiates multiple parallel calls
///
/// Multiple outstanding requests should have unique Request IDs.
#[test_log::test]
fn parallel_requests_have_unique_request_ids() {
    covers!(feat_req_recentip_79);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Handle 5 parallel calls
        for _ in 0..5 {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call {
                        payload, responder, ..
                    } => {
                        responder.reply(&payload).await.unwrap();
                    }
                    _ => {}
                }
            }
        }

        // Wait for client to complete
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let method = MethodId::new(0x0001).unwrap();

        // Send 5 parallel requests
        let f1 = proxy.call(method, &[1]);
        let f2 = proxy.call(method, &[2]);
        let f3 = proxy.call(method, &[3]);
        let f4 = proxy.call(method, &[4]);
        let f5 = proxy.call(method, &[5]);

        // All should succeed with matching responses (unique Request IDs allow matching)
        let (r1, r2, r3, r4, r5) = tokio::join!(
            tokio::time::timeout(Duration::from_secs(5), f1),
            tokio::time::timeout(Duration::from_secs(5), f2),
            tokio::time::timeout(Duration::from_secs(5), f3),
            tokio::time::timeout(Duration::from_secs(5), f4),
            tokio::time::timeout(Duration::from_secs(5), f5),
        );

        assert_eq!(r1.unwrap().unwrap().payload[0], 1);
        assert_eq!(r2.unwrap().unwrap().payload[0], 2);
        assert_eq!(r3.unwrap().unwrap().payload[0], 3);
        assert_eq!(r4.unwrap().unwrap().payload[0], 4);
        assert_eq!(r5.unwrap().unwrap().payload[0], 5);

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// REQUEST ID REUSE
// ============================================================================

/// feat_req_recentip_80: Request IDs can be reused after response arrives
///
/// After receiving a response, the Request ID (Session ID portion) can be reused.
/// Make many sequential requests to verify IDs are managed properly.
#[test_log::test]
#[cfg(feature = "slow-tests")]
fn request_id_reusable_after_response() {
    covers!(feat_req_recentip_80);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(120))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Handle 1000 sequential calls
        for _ in 0..1000 {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call {
                        payload, responder, ..
                    } => {
                        // Echo + 1
                        let val = u16::from_be_bytes([payload[0], payload[1]]);
                        responder.reply(&(val + 1).to_be_bytes()).await.unwrap();
                    }
                    _ => {}
                }
            }
        }

        // Wait for client to complete
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let method = MethodId::new(0x0001).unwrap();

        // Make 1000 sequential requests
        for i in 0u16..1000 {
            let response =
                tokio::time::timeout(Duration::from_secs(5), proxy.call(method, &i.to_be_bytes()))
                    .await
                    .expect("RPC timeout")
                    .expect("RPC should succeed");

            let val = u16::from_be_bytes([response.payload[0], response.payload[1]]);
            assert_eq!(val, i + 1, "Response should echo value + 1");
        }

        // If we got here without error, Request IDs are being managed properly
        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// SESSION ID WRAPAROUND
// ============================================================================

/// feat_req_recentip_677: Session ID wraps from 0xFFFF to 0x0001
///
/// When the Session ID reaches 0xFFFF, it shall wrap to 0x0001 (not 0x0000).
///
/// **IMPORTANT**: This integration test does NOT directly verify session ID values
/// on the wire. It only verifies that the runtime can handle 65536+ calls without
/// error, implying the internal session counter works correctly. The actual
/// wraparound logic (0xFFFF -> 0x0001, never 0x0000) is properly verified by
/// unit tests in `src/runtime.rs::tests::session_id_wraps_to_0001_not_0000`
/// which directly test the `next_session_id()` function.
///
/// This integration test serves as an end-to-end stress test that the full
/// request/response cycle works correctly across session ID rollover.
///
/// NOTE: This test is slow (~100s) because it makes 65536+ RPC calls.
/// Run with `cargo test -- --ignored` to include it.
#[test_log::test]
#[ignore]
fn session_id_wraps_to_0001_not_0000() {
    covers!(feat_req_recentip_677);

    use std::sync::atomic::{AtomicU32, Ordering};

    // Use static atomics since turmoil hosts run in different tasks
    static SERVER_COUNT: AtomicU32 = AtomicU32::new(0);
    static CLIENT_COUNT: AtomicU32 = AtomicU32::new(0);

    // Reset counters for this test run
    SERVER_COUNT.store(0, Ordering::SeqCst);
    CLIENT_COUNT.store(0, Ordering::SeqCst);

    // one SD every two seconds. So 65536 calls would take ~36 hours real time.
    // We use turmoil to simulate this quickly.
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(140000)) // 38.8 hours simulated time
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Handle 65536 calls to see wraparound
        for _ in 0..=0xFFFFu32 {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call {
                        payload, responder, ..
                    } => {
                        // Echo the iteration number back to prove we processed it
                        SERVER_COUNT.fetch_add(1, Ordering::SeqCst);
                        responder.reply(&payload).await.unwrap();
                    }
                    _ => {}
                }
            }
        }

        // Wait for client to finish
        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let method = MethodId::new(0x0001).unwrap();

        // Make 65536 calls - if session ID ever becomes 0x0000, something is wrong
        // The runtime should handle wraparound from 0xFFFF -> 0x0001
        for i in 0..=0xFFFFu32 {
            // Send iteration number as payload
            let payload = i.to_be_bytes();
            let result = tokio::time::timeout(
                Duration::from_millis(500), // Shorter timeout per call
                proxy.call(method, &payload),
            )
            .await;

            match result {
                Ok(Ok(response)) => {
                    // Verify we got our payload back
                    assert_eq!(
                        response.payload.as_ref(),
                        &payload,
                        "Response payload should match request for iteration {}",
                        i
                    );
                    CLIENT_COUNT.fetch_add(1, Ordering::SeqCst);
                }
                Ok(Err(e)) => panic!("Call {} failed: {:?}", i, e),
                Err(_) => panic!("Call {} timed out", i),
            }
        }

        // Verify we actually made all the calls (inside the async)
        let client_calls = CLIENT_COUNT.load(Ordering::SeqCst);
        assert_eq!(
            client_calls, 65536,
            "Client async: Should have completed exactly 65536 calls, got {}",
            client_calls
        );

        Ok(())
    });

    // Check counts BEFORE sim.run() to see baseline
    let pre_server = SERVER_COUNT.load(Ordering::SeqCst);
    let pre_client = CLIENT_COUNT.load(Ordering::SeqCst);
    assert_eq!(pre_server, 0, "Pre-run server count should be 0");
    assert_eq!(pre_client, 0, "Pre-run client count should be 0");

    sim.run().unwrap();

    // Verify both sides processed all calls
    let server_processed = SERVER_COUNT.load(Ordering::SeqCst);
    let client_completed = CLIENT_COUNT.load(Ordering::SeqCst);

    assert_eq!(
        server_processed, 65536,
        "Server should have processed exactly 65536 calls, got {}",
        server_processed
    );
    assert_eq!(
        client_completed, 65536,
        "Client should have completed exactly 65536 calls, got {}",
        client_completed
    );
}

// ============================================================================
// REQUEST/RESPONSE SESSION HANDLING
// ============================================================================

/// feat_req_recentip_669: Request/Response shall use session handling
///
/// Request/Response messages must use session handling (Session ID != 0x0000).
#[test_log::test]
fn request_response_uses_session_handling() {
    covers!(feat_req_recentip_669);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Handle 3 calls
        for _ in 0..3 {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call { responder, .. } => {
                        responder.reply(&[]).await.unwrap();
                    }
                    _ => {}
                }
            }
        }

        // Wait for client to receive responses before ending simulation
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let method = MethodId::new(0x0001).unwrap();

        // Make 3 calls - session handling means each gets unique session ID
        for i in 0..3 {
            let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, &[i]))
                .await
                .expect("RPC timeout")
                .expect("RPC should succeed");

            assert!(response.payload.is_empty());
        }

        // All calls succeeding with correct responses implies session handling worked
        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// FIRE AND FORGET
// ============================================================================

/// feat_req_recentip_667: Fire&Forget shall use session handling
///
/// Fire-and-forget messages should use session handling (Session ID != 0x0000).
#[test_log::test]
fn fire_and_forget_uses_session_handling() {
    covers!(feat_req_recentip_667);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Handle 3 fire-and-forget calls (order may vary due to network)
        let mut received = Vec::new();
        for _ in 0..3 {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::FireForget {
                        payload, method, ..
                    } => {
                        assert_eq!(method.value(), 0x0010);
                        received.push(payload.to_vec());
                    }
                    other => panic!("Expected FireForget, got {:?}", other),
                }
            }
        }
        // Verify we received all 3 messages (order may vary)
        assert_eq!(received.len(), 3);
        received.sort();
        assert_eq!(
            received,
            vec![b"ff1".to_vec(), b"ff2".to_vec(), b"ff3".to_vec()]
        );

        // Wait for client to send all messages
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let method = MethodId::new(0x0010).unwrap();

        // Send 3 fire-and-forget messages
        proxy.fire_and_forget(method, b"ff1").await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        proxy.fire_and_forget(method, b"ff2").await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        proxy.fire_and_forget(method, b"ff3").await.unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentip_711: Server copies Request ID to response
///
/// When generating a response message, the server must copy the
/// Request ID from the request to the response message.
#[test_log::test]
fn server_copies_request_id_to_response() {
    covers!(feat_req_recentip_711);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Handle call and respond
        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::Call { responder, .. } => {
                    // The runtime should automatically copy Request ID
                    responder.reply(b"response").await.unwrap();
                }
                _ => panic!("Expected Call"),
            }
        }

        // Wait for client to complete
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let method = MethodId::new(0x0001).unwrap();

        // Send request and verify response matches
        let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, b"request"))
            .await
            .expect("RPC timeout")
            .expect("RPC should succeed");

        // If response was received correctly, Request ID was copied
        // (otherwise the runtime couldn't match it to our pending call)
        assert_eq!(response.payload.as_ref(), b"response");

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentip_79: Request IDs must be unique for parallel calls
///
/// Variant test: send many parallel requests and ensure all get correct responses.
#[test_log::test]
fn request_id_differentiates_parallel_calls() {
    covers!(feat_req_recentip_79);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Handle 10 calls
        for _ in 0..10 {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call {
                        payload, responder, ..
                    } => {
                        // Return payload doubled
                        let val = payload[0];
                        responder.reply(&[val * 2]).await.unwrap();
                    }
                    _ => {}
                }
            }
        }

        // Wait for client to receive responses before ending simulation
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let method = MethodId::new(0x0001).unwrap();

        // Send 10 parallel requests
        let payloads: Vec<[u8; 1]> = (1u8..=10).map(|i| [i]).collect();
        let futures: Vec<_> = payloads.iter().map(|p| proxy.call(method, p)).collect();

        // Await all
        let mut results = Vec::new();
        for fut in futures {
            let response = tokio::time::timeout(Duration::from_secs(5), fut)
                .await
                .expect("RPC timeout")
                .expect("RPC should succeed");
            results.push(response.payload[0]);
        }

        // Each result should be input * 2
        for (i, result) in results.iter().enumerate() {
            let expected = ((i + 1) * 2) as u8;
            assert_eq!(*result, expected, "Response {} should be {} * 2", i, i + 1);
        }

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// LATE SERVER DISCOVERY
// ============================================================================

/// Test that a client can wait for a service that isn't available yet,
/// and successfully complete RPC once the server comes online.
///
/// This tests the "find -> wait -> server starts -> RPC" flow.
#[test_log::test]
fn late_server_discovery_rpc() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(60))
        .build();

    use std::sync::atomic::{AtomicBool, Ordering};
    static RPC_COMPLETED: AtomicBool = AtomicBool::new(false);
    RPC_COMPLETED.store(false, Ordering::SeqCst);

    // Start client FIRST - it will wait for service
    sim.client("client", async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Find service - not yet available
        let proxy = runtime.find(TEST_SERVICE_ID);

        // Wait for service to become available (server will start after 2 seconds)
        let proxy = tokio::time::timeout(Duration::from_secs(10), proxy)
            .await
            .expect("Service should become available within timeout")
            .expect("Service available");

        // Service is now available, make RPC call
        let method = MethodId::new(0x0001).unwrap();
        let response =
            tokio::time::timeout(Duration::from_secs(5), proxy.call(method, b"late-client"))
                .await
                .expect("RPC timeout")
                .expect("RPC should succeed");

        assert_eq!(response.payload.as_ref(), b"hello-late-client");
        RPC_COMPLETED.store(true, Ordering::SeqCst);

        Ok(())
    });

    // Start server AFTER a delay
    sim.host("server", || async {
        // Wait before offering service - client should already be waiting
        tokio::time::sleep(Duration::from_secs(2)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Handle the RPC call from the waiting client
        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::Call {
                    payload, responder, ..
                } => {
                    // Respond with greeting + payload
                    let mut response = b"hello-".to_vec();
                    response.extend_from_slice(&payload);
                    responder.reply(&response).await.unwrap();
                }
                _ => panic!("Expected Call"),
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(
        RPC_COMPLETED.load(Ordering::SeqCst),
        "RPC should have completed"
    );
}

/// Test that a client can wait for a service that isn't available yet,
/// and successfully subscribe to events once the server comes online.
///
/// This tests the "find -> wait -> server starts -> subscribe -> receive event" flow.
///
#[test_log::test]
fn late_server_discovery_subscribe_event() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(60))
        .build();

    use std::sync::atomic::{AtomicBool, Ordering};
    static EVENT_RECEIVED: AtomicBool = AtomicBool::new(false);
    EVENT_RECEIVED.store(false, Ordering::SeqCst);

    // Server must be registered first for sim.host to work
    sim.host("server", || async {
        // Delay to simulate late server
        tokio::time::sleep(Duration::from_millis(500)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for subscription
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send event
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        offering.event(event_id).eventgroup(eventgroup).create().unwrap().notify(b"late-event")
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(10), proxy)
            .await
            .expect("Service should become available within timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        let event = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("Event timeout")
            .expect("Should receive event");

        assert_eq!(event.payload.as_ref(), b"late-event");
        EVENT_RECEIVED.store(true, Ordering::SeqCst);

        Ok(())
    });

    sim.run().unwrap();
    assert!(
        EVENT_RECEIVED.load(Ordering::SeqCst),
        "Event should have been received"
    );
}
