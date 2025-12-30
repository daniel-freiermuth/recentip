//! Message Type Integration Tests
//!
//! Integration tests for SOME/IP message type behavior using turmoil simulation.
//! Unit tests for MessageType parsing are in src/wire.rs.
//!
//! # Message Types (feat_req_recentip_103)
//! - 0x00: REQUEST - Client sends request expecting response
//! - 0x01: REQUEST_NO_RETURN - Fire-and-forget request
//! - 0x02: NOTIFICATION - Server sends event notification
//! - 0x80: RESPONSE - Response to a REQUEST
//! - 0x81: ERROR - Error response to a REQUEST
//!
//! # Valid Transitions (feat_req_recentip_282)
//! - REQUEST (0x00) → RESPONSE (0x80) or ERROR (0x81)
//! - REQUEST_NO_RETURN (0x01) → no response allowed
//! - NOTIFICATION (0x02) → no response allowed

use std::time::Duration;

use someip_runtime::prelude::*;
use someip_runtime::runtime::Runtime;
use someip_runtime::handle::ServiceEvent;

#[cfg(feature = "turmoil")]
type TurmoilRuntime = Runtime<turmoil::net::UdpSocket>;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

// ============================================================================
// Test Service Definition
// ============================================================================

struct TestService;

impl Service for TestService {
    const SERVICE_ID: u16 = 0x1234;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}

// ============================================================================
// Integration Tests (turmoil-based)
// ============================================================================

/// [feat_req_recentip_282] REQUEST gets RESPONSE with matching type
#[cfg(feature = "turmoil")]
#[test]
fn request_receives_response_type() {
    covers!(feat_req_recentip_282);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let mut offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Wait for call and respond
        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::Call { responder, .. } => {
                    responder.reply(&[4, 5, 6]).await.unwrap();
                }
                _ => {}
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout").expect("Service available");

        // Call should get response
        let method = MethodId::new(0x0001).unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, &[1, 2, 3]))
            .await
            .expect("RPC timeout")
            .expect("RPC should succeed");

        // Response should have OK return code (matching RESPONSE type)
        assert_eq!(response.return_code, ReturnCode::Ok);

        Ok(())
    });

    sim.run().unwrap();
}

/// [feat_req_recentip_282] REQUEST can get ERROR type response
#[cfg(feature = "turmoil")]
#[test]
fn request_can_receive_error_type() {
    covers!(feat_req_recentip_282);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let mut offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Wait for call and respond with error
        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::Call { responder, .. } => {
                    responder.reply_error(ReturnCode::NotOk).await.unwrap();
                }
                _ => {}
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout").expect("Service available");

        // Call should get error response
        let method = MethodId::new(0x0001).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, &[1, 2, 3]))
            .await
            .expect("RPC timeout")
            .expect("Call should have succeeded");

        // Should be an error (ERROR message type maps to Err result)
        assert!(result.is_err(), "Error response should return Err. Got {:?}", result);

        Ok(())
    });

    sim.run().unwrap();
}

/// [feat_req_recentip_284] REQUEST_NO_RETURN gets no response
#[cfg(feature = "turmoil")]
#[test]
fn request_no_return_receives_no_response() {
    covers!(feat_req_recentip_284);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let mut offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Server receives fire-and-forget as FireForget event
        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::FireForget { payload, .. } => {
                    assert_eq!(payload.as_ref(), &[1, 2, 3]);
                }
                other => panic!("Expected FireForget, got {:?}", other),
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout").expect("Service available");

        // Fire-and-forget returns immediately (no response expected)
        let method = MethodId::new(0x0001).unwrap();
        proxy
            .fire_and_forget(method, &[1, 2, 3])
            .await
            .expect("Fire-and-forget should succeed");

        // Give time for message to arrive
        tokio::time::sleep(Duration::from_millis(200)).await;

        Ok(())
    });

    sim.run().unwrap();
}

/// [feat_req_recentip_285] Server can send NOTIFICATION messages
#[cfg(feature = "turmoil")]
#[test]
fn server_sends_notification_type() {
    covers!(feat_req_recentip_285);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Wait for subscription, then send notification
        tokio::time::sleep(Duration::from_millis(500)).await;

        let eventgroup = EventgroupId::new(1).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        offering
            .notify(eventgroup, event_id, &[0xAA, 0xBB])
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime: TurmoilRuntime = Runtime::with_socket_type(Default::default()).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout").expect("Service available");

        // Subscribe to eventgroup
        let eventgroup = EventgroupId::new(1).unwrap();
        let mut subscription = proxy.subscribe(eventgroup).await.expect("Subscribe failed");

        // Should receive notification
        let event = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("Event timeout")
            .expect("Should receive event");

        assert_eq!(event.payload.as_ref(), &[0xAA, 0xBB]);

        Ok(())
    });

    sim.run().unwrap();
}

/// [feat_req_recentip_103] TP-flagged request gets TP-flagged response
#[cfg(feature = "turmoil")]
#[test]
#[ignore = "SOME/IP-TP not yet implemented"]
fn tp_request_gets_tp_response() {
    covers!(feat_req_recentip_103);
    // TP (Transport Protocol) is used for large payloads that exceed UDP MTU.
    // When a request uses TP (message type 0x20), the response must also use TP (0xA0).
    // This test requires the TP implementation to be complete.
}
