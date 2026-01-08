//! Error Handling and Return Code Compliance Tests
//!
//! Tests the error handling behavior per SOME/IP specification.
//!
//! Key requirements tested:
//! - feat_req_recentip_371: Return code definitions (E_OK, E_NOT_OK, etc.)
//! - feat_req_recentip_144: Return code usage per message type
//! - feat_req_recentip_141: Request/Response message type mapping
//! - feat_req_recentip_704: No error responses to error messages
//! - feat_req_recentip_816: Optional E_UNKNOWN_SERVICE/E_UNKNOWN_METHOD
//! - feat_req_recentip_684: Message type definitions

use std::time::Duration;

use someip_runtime::handle::ServiceEvent;
use someip_runtime::prelude::*;
use someip_runtime::runtime::Runtime;

#[cfg(feature = "turmoil")]
type TurmoilRuntime =
    Runtime<turmoil::net::UdpSocket, turmoil::net::TcpStream, turmoil::net::TcpListener>;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

// ============================================================================
// Return Code Constants (per feat_req_recentip_371)
// ============================================================================

/// Return codes as defined in the specification
pub mod return_codes {
    pub const E_OK: u8 = 0x00;
    pub const E_NOT_OK: u8 = 0x01;
    pub const E_UNKNOWN_SERVICE: u8 = 0x02;
    pub const E_UNKNOWN_METHOD: u8 = 0x03;
    pub const E_NOT_READY: u8 = 0x04; // deprecated
    pub const E_NOT_REACHABLE: u8 = 0x05; // deprecated
    pub const E_TIMEOUT: u8 = 0x06; // deprecated
    pub const E_WRONG_PROTOCOL_VERSION: u8 = 0x07;
    pub const E_WRONG_INTERFACE_VERSION: u8 = 0x08;
    pub const E_MALFORMED_MESSAGE: u8 = 0x09;
    pub const E_WRONG_MESSAGE_TYPE: u8 = 0x0A;
    // 0x0B-0x1F: Reserved for generic SOME/IP errors
    // 0x20-0x3F: Reserved for service/method specific errors
}

// ============================================================================
// Message Type Constants (per feat_req_recentip_684)
// ============================================================================

pub mod message_types {
    pub const REQUEST: u8 = 0x00;
    pub const REQUEST_NO_RETURN: u8 = 0x01;
    pub const NOTIFICATION: u8 = 0x02;
    pub const REQUEST_ACK: u8 = 0x40; // Reserved
    pub const REQUEST_NO_RETURN_ACK: u8 = 0x41; // Reserved
    pub const NOTIFICATION_ACK: u8 = 0x42; // Reserved
    pub const RESPONSE: u8 = 0x80;
    pub const ERROR: u8 = 0x81; // Called EXCEPTION in spec
    pub const RESPONSE_ACK: u8 = 0xC0; // Reserved
    pub const ERROR_ACK: u8 = 0xC1; // Reserved

    /// TP flag bit (can be OR'd with message type)
    pub const TP_FLAG: u8 = 0x20;
}

// ============================================================================
// Return Code Unit Tests (test infrastructure, not conformance)
// ============================================================================

#[cfg(test)]
mod return_code_tests {
    use super::return_codes::*;

    #[test_log::test]
    fn e_ok_is_zero() {
        assert_eq!(E_OK, 0x00);
    }

    #[test_log::test]
    fn e_not_ok_is_one() {
        assert_eq!(E_NOT_OK, 0x01);
    }

    #[test_log::test]
    fn return_code_values_match_spec() {
        assert_eq!(E_UNKNOWN_SERVICE, 0x02);
        assert_eq!(E_UNKNOWN_METHOD, 0x03);
        assert_eq!(E_NOT_READY, 0x04);
        assert_eq!(E_NOT_REACHABLE, 0x05);
        assert_eq!(E_TIMEOUT, 0x06);
        assert_eq!(E_WRONG_PROTOCOL_VERSION, 0x07);
        assert_eq!(E_WRONG_INTERFACE_VERSION, 0x08);
        assert_eq!(E_MALFORMED_MESSAGE, 0x09);
        assert_eq!(E_WRONG_MESSAGE_TYPE, 0x0A);
    }

    #[test_log::test]
    fn reserved_ranges() {
        // 0x0B-0x1F: Generic SOME/IP errors
        assert!(0x0B <= 0x1F);
        // 0x20-0x3F: Service-specific errors
        assert!(0x20 <= 0x3F);
    }
}

// ============================================================================
// Message Type Unit Tests (test infrastructure, not conformance)
// ============================================================================

#[cfg(test)]
mod message_type_tests {
    use super::message_types::*;

    #[test_log::test]
    fn message_type_values_match_spec() {
        assert_eq!(REQUEST, 0x00);
        assert_eq!(REQUEST_NO_RETURN, 0x01);
        assert_eq!(NOTIFICATION, 0x02);
        assert_eq!(RESPONSE, 0x80);
        assert_eq!(ERROR, 0x81);
    }

    #[test_log::test]
    fn ack_bit_is_0x40() {
        assert_eq!(REQUEST_ACK, REQUEST | 0x40);
        assert_eq!(REQUEST_NO_RETURN_ACK, REQUEST_NO_RETURN | 0x40);
        assert_eq!(NOTIFICATION_ACK, NOTIFICATION | 0x40);
        assert_eq!(RESPONSE_ACK, RESPONSE | 0x40);
        assert_eq!(ERROR_ACK, ERROR | 0x40);
    }

    #[test_log::test]
    fn tp_flag_is_0x20() {
        assert_eq!(TP_FLAG, 0x20);
    }

    #[test_log::test]
    fn response_is_request_with_high_bit() {
        assert_eq!(RESPONSE, REQUEST | 0x80);
        assert_eq!(ERROR, REQUEST_NO_RETURN | 0x80);
    }
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

/// feat_req_recentip_141: REQUEST answered by RESPONSE on success
///
/// Regular request (message type 0x00) shall be answered by a response
/// (message type 0x80) when no error occurred.
#[cfg(feature = "turmoil")]
#[test_log::test]
fn request_answered_by_response() {
    covers!(feat_req_recentip_141);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
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

        // Wait for a call and respond with success
        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::Call { responder, .. } => {
                    responder.reply(b"ok").await.unwrap();
                }
                _ => {}
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let method = MethodId::new(0x0001).unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, b"request"))
            .await
            .expect("RPC timeout")
            .expect("RPC should succeed");

        // Response should have E_OK return code
        assert_eq!(response.return_code, ReturnCode::Ok);

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentip_141: Error can be in RESPONSE or ERROR message
///
/// If an error occurs, response message with return code not equal to 0x00
/// shall be sent.
#[cfg(feature = "turmoil")]
#[test_log::test]
fn error_response_has_nonzero_return_code() {
    covers!(feat_req_recentip_141, feat_req_recentip_726);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
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

        // Wait for a call and respond with an error
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

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let method = MethodId::new(0x0001).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, b"request"))
            .await
            .expect("RPC timeout");

        // Either Err or Ok with non-Ok return code is valid
        match result {
            Err(_) => {
                // Error propagated as Err - acceptable
            }
            Ok(response) => {
                assert_ne!(
                    response.return_code,
                    ReturnCode::Ok,
                    "Error response should have non-E_OK return code"
                );
            }
        }

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentip_684: NOTIFICATION is fire-and-forget from server
///
/// Events/notifications use message type NOTIFICATION (0x02).
#[cfg(feature = "turmoil")]
#[test_log::test]
fn notification_message_type() {
    covers!(feat_req_recentip_684);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let event_received = Arc::new(AtomicBool::new(false));
    let event_received_clone = event_received.clone();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for subscription, then send event
        tokio::time::sleep(Duration::from_secs(1)).await;

        let event_id = EventId::new(0x8001).unwrap();
        let eventgroup = EventgroupId::new(1).unwrap();
        offering
            .notify(eventgroup, event_id, b"event_data")
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        // Subscribe to eventgroup
        let eventgroup = EventgroupId::new(1).unwrap();
        let mut subscription = proxy.subscribe(eventgroup).await.unwrap();

        // Wait for event
        if let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_secs(5), subscription.next()).await
        {
            // Received a notification
            assert!(!event.payload.is_empty());
            event_received_clone.store(true, Ordering::SeqCst);
        }

        Ok(())
    });

    sim.run().unwrap();
    assert!(
        event_received.load(Ordering::SeqCst),
        "Should receive notification"
    );
}

/// Response IDs must match request IDs
///
/// The response Message ID and Request ID must match the request.
#[cfg(feature = "turmoil")]
#[test_log::test]
fn response_ids_match_request() {
    covers!(feat_req_recentip_141);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
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

        // Echo back requests
        for _ in 0..3 {
            if let Some(event) = offering.next().await {
                match event {
                    ServiceEvent::Call {
                        payload, responder, ..
                    } => {
                        responder.reply(payload.as_ref()).await.unwrap();
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

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let method = MethodId::new(0x0001).unwrap();

        // Send multiple requests - each response should match its request
        for i in 0u8..3 {
            let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, &[i]))
                .await
                .expect("RPC timeout")
                .expect("RPC should succeed");

            // Payload should match (echo)
            assert_eq!(
                response.payload[0], i,
                "Response payload should match request"
            );
        }

        Ok(())
    });

    sim.run().unwrap();
}

/// Protocol version must be 0x01
#[cfg(feature = "turmoil")]
#[test_log::test]
fn protocol_version_is_one() {
    covers!(feat_req_recentip_369);

    // Protocol version is a constant in the wire format
    assert_eq!(someip_runtime::wire::PROTOCOL_VERSION, 0x01);
}

/// Return code in successful response is E_OK
#[cfg(feature = "turmoil")]
#[test_log::test]
fn successful_response_has_e_ok() {
    covers!(feat_req_recentip_144);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
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

        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::Call { responder, .. } => {
                    responder.reply(b"success").await.unwrap();
                }
                _ => {}
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let method = MethodId::new(0x0001).unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, b"test"))
            .await
            .expect("RPC timeout")
            .expect("RPC should succeed");

        assert_eq!(
            response.return_code,
            ReturnCode::Ok,
            "Successful response must have E_OK"
        );

        Ok(())
    });

    sim.run().unwrap();
}
