//! Advanced Error Handling Compliance Tests
//!
//! Tests error scenarios, return codes, and error message handling.
//!
//! Key requirements tested:
//! - feat_req_someip_597: No error response for events/notifications
//! - feat_req_someip_654: No error response for fire&forget methods
//! - feat_req_someip_655: Error message copies request header fields (wire format test)
//! - feat_req_someip_727: Error message has return code != 0x00
//! - feat_req_someip_798: Messages with length < 8 shall be ignored (wire format test)
//! - feat_req_someip_703: Use known protocol version (wire format test)

use crate::helpers::DEFAULT_SD_MULTICAST;
use recentip::handle::ServiceEvent;
use recentip::prelude::*;

use std::time::Duration;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

/// Type alias for turmoil-based runtime

const TEST_SERVICE_ID: u16 = 0x1234;
const TEST_SERVICE_VERSION: (u8, u32) = (1, 0);

// ============================================================================
// No Error Response for Events/Fire&Forget Tests
// ============================================================================

/// feat_req_someip_597: No error response for events/notifications
///
/// The system shall not return an error message for events/notifications.
/// Events are one-way - there's no mechanism to send errors back.
#[test_log::test]
fn no_error_response_for_events() {
    covers!(feat_req_someip_597);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service with events
    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
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

        // Wait for subscription
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send event notification - this is one-way, no response expected
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

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    // Client subscribes and receives event
    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        // Receive event - even if there were processing errors,
        // no error response would be sent (events are one-way)
        let event = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("Event timeout");

        assert!(event.is_some(), "Should receive event");

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_654: No error response for fire&forget methods
///
/// The system shall not return an error message for fire&forget methods.
/// Fire&forget is one-way - no response or error is expected.
#[test_log::test]
fn no_error_response_for_fire_and_forget() {
    covers!(feat_req_someip_654);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server handles fire&forget
    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
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

        // Receive fire&forget - no response should be sent
        if let Some(event) = tokio::time::timeout(Duration::from_secs(10), offering.next())
            .await
            .ok()
            .flatten()
        {
            if let ServiceEvent::FireForget { payload, .. } = event {
                assert_eq!(payload.as_ref(), b"ff_payload");
                // No response mechanism - this is fire&forget
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    // Client sends fire&forget
    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        // Send fire&forget - no response expected
        proxy
            .fire_and_forget(MethodId::new(0x0010).unwrap(), b"ff_payload")
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// Return Code Tests
// ============================================================================

/// feat_req_someip_683: All defined return codes are valid
///
/// Test that all defined return codes can be used.
#[test_log::test]
fn all_return_codes_are_valid() {
    covers!(feat_req_someip_683);

    // Test that all defined return codes can be used
    let codes = [
        ReturnCode::Ok,
        ReturnCode::NotOk,
        ReturnCode::UnknownService,
        ReturnCode::UnknownMethod,
        ReturnCode::NotReady,
        ReturnCode::NotReachable,
        ReturnCode::Timeout,
        ReturnCode::WrongProtocolVersion,
        ReturnCode::WrongInterfaceVersion,
        ReturnCode::MalformedMessage,
        ReturnCode::WrongMessageType,
    ];

    for code in codes {
        // Each code should have a distinct value
        let value = code as u8;
        assert!(
            value <= 0x0A,
            "Return code {:?} should be in valid range",
            code
        );
    }

    // Verify E_OK is 0x00
    assert_eq!(ReturnCode::Ok as u8, 0x00);
}

/// feat_req_someip_727: Error message has return code != 0x00
/// feat_req_someip_683: Server can return any valid return code
///
/// Server responds with various error codes, and client receives them.
#[test_log::test]
fn server_returns_various_error_codes() {
    covers!(feat_req_someip_683, feat_req_someip_727);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server returns different error codes
    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
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

        // Handle first request - return MalformedMessage (as a test of application-controlled error)
        if let Some(event) = tokio::time::timeout(Duration::from_secs(10), offering.next())
            .await
            .ok()
            .flatten()
        {
            if let ServiceEvent::Call { responder, .. } = event {
                responder
                    .reply_error(ApplicationError::MalformedMessage)
                    .unwrap();
            }
        }

        // Handle second request - return UnknownMethod
        if let Some(event) = tokio::time::timeout(Duration::from_secs(10), offering.next())
            .await
            .ok()
            .flatten()
        {
            if let ServiceEvent::Call { responder, .. } = event {
                responder
                    .reply_error(ApplicationError::UnknownMethod)
                    .unwrap();
            }
        }

        // Handle third request - return NotOk
        if let Some(event) = tokio::time::timeout(Duration::from_secs(10), offering.next())
            .await
            .ok()
            .flatten()
        {
            if let ServiceEvent::Call { responder, .. } = event {
                responder.reply_error(ApplicationError::NotOk).unwrap();
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    // Client makes calls and receives error codes
    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        // First call - expect MalformedMessage
        let result1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(0x0001).unwrap(), b"data1"),
        )
        .await
        .expect("Timeout");

        match result1 {
            Err(_) => {
                // Error propagated - acceptable
            }
            Ok(response) => {
                assert_eq!(
                    response.return_code,
                    ReturnCode::MalformedMessage,
                    "Should receive MalformedMessage"
                );
            }
        }

        // Second call - expect UnknownMethod
        let result2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(0x0002).unwrap(), b"data2"),
        )
        .await
        .expect("Timeout");

        match result2 {
            Err(_) => {
                // Error propagated - acceptable
            }
            Ok(response) => {
                assert_eq!(
                    response.return_code,
                    ReturnCode::UnknownMethod,
                    "Should receive UnknownMethod"
                );
            }
        }

        // Third call - expect NotOk
        let result3 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(0x0003).unwrap(), b"data3"),
        )
        .await
        .expect("Timeout");

        match result3 {
            Err(_) => {
                // Error propagated - acceptable
            }
            Ok(response) => {
                assert_eq!(
                    response.return_code,
                    ReturnCode::NotOk,
                    "Should receive NotOk"
                );
                // Verify return code is != 0x00 (feat_req_someip_727)
                assert_ne!(
                    response.return_code as u8, 0x00,
                    "Error code must be != 0x00"
                );
            }
        }

        Ok(())
    });

    sim.run().unwrap()
}

// ============================================================================
// Wire Format Tests
// ============================================================================

/// Helper to parse a SOME/IP header from raw bytes
fn parse_header_wire(data: &[u8]) -> Option<recentip::wire::Header> {
    use bytes::Bytes;
    use recentip::wire::Header;

    if data.len() < Header::SIZE {
        return None;
    }
    Header::parse(&mut Bytes::copy_from_slice(data))
}

/// Helper to parse an SD message from raw bytes
fn parse_sd_message(data: &[u8]) -> Option<(recentip::wire::Header, recentip::wire::SdMessage)> {
    use bytes::Bytes;
    use recentip::wire::{Header, SdMessage};

    const SD_SERVICE_ID: u16 = 0xFFFF;
    const SD_METHOD_ID: u16 = 0x8100;

    if data.len() < Header::SIZE {
        return None;
    }
    let mut bytes = Bytes::copy_from_slice(data);
    let header = Header::parse(&mut bytes)?;
    if header.service_id == SD_SERVICE_ID && header.method_id == SD_METHOD_ID {
        let sd_msg = SdMessage::parse(&mut bytes)?;
        Some((header, sd_msg))
    } else {
        None
    }
}

/// feat_req_someip_655: Error response copies request header fields
///
/// For request/response methods the error message shall copy over the
/// fields of the header from the request.
#[test_log::test]
fn error_response_copies_request_header() {
    use bytes::{BufMut, BytesMut};
    use recentip::handle::ServiceEvent;
    use recentip::prelude::*;
    use recentip::wire::MessageType;
    use std::net::SocketAddr;
    use std::time::Duration;

    covers!(feat_req_someip_655);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library side - server responds with errors
    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
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

        // Receive call and send error
        if let Some(event) = tokio::time::timeout(Duration::from_secs(10), offering.next())
            .await
            .ok()
            .flatten()
        {
            if let ServiceEvent::Call { responder, .. } = event {
                responder
                    .reply_error(ApplicationError::UnknownMethod)
                    .unwrap();
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    // Raw socket side - discovers server via SD, then sends request and verifies error
    sim.client("raw_observer", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Bind to SD multicast to discover server
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4(
            "239.255.0.1".parse().unwrap(),
            "0.0.0.0".parse().unwrap()
        )?;

        // Find server endpoint from SD offer
        let mut server_endpoint: Option<SocketAddr> = None;
        let mut buf = [0u8; 1500];

        for _ in 0..20 {
            let result = tokio::time::timeout(
                Duration::from_millis(200),
                sd_socket.recv_from(&mut buf),
            ).await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 {
                            // Found our service offer
                            if let Some(opt) = sd_msg.options.first() {
                                if let recentip::wire::SdOption::Ipv4Endpoint { addr, port, .. } = opt {
                                    let ip = if addr.is_unspecified() {
                                        from.ip()
                                    } else {
                                        std::net::IpAddr::V4(*addr)
                                    };
                                    server_endpoint = Some(SocketAddr::new(ip, *port));
                                }
                            }
                        }
                    }
                }
            }
            if server_endpoint.is_some() {
                break;
            }
        }

        let server_addr = server_endpoint.expect("Should find server via SD");

        // Now send a request to the discovered endpoint
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        // Send a request
        let mut request = BytesMut::with_capacity(24);
        request.put_u16(0x1234); // Service ID
        request.put_u16(0x0001); // Method ID
        request.put_u32(0x00000010); // Length = 16 (8 header tail + 8 payload)
        request.put_u16(0x0042); // Client ID
        request.put_u16(0x1337); // Session ID
        request.put_u8(0x01); // Protocol Version
        request.put_u8(0x01); // Interface Version
        request.put_u8(0x00); // Message Type = REQUEST
        request.put_u8(0x00); // Return Code
        request.put_slice(b"testdata"); // 8 bytes payload

        rpc_socket.send_to(&request, server_addr).await?;

        // Capture response
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            rpc_socket.recv_from(&mut buf)
        ).await;

        if let Ok(Ok((len, _))) = result {
            if let Some(error_header) = parse_header_wire(&buf[..len]) {
                // With default MethodConfig (no exception methods configured),
                // errors use RESPONSE (0x80) per feat_req_someip_726
                assert_eq!(
                    error_header.message_type,
                    MessageType::Response,
                    "Error response should be RESPONSE (0x80) when EXCEPTION is not configured (feat_req_someip_726)"
                );

                // Verify return code is not OK (feat_req_someip_727)
                // This is what makes it an "error message" even with RESPONSE type
                assert_ne!(
                    error_header.return_code, 0x00,
                    "Error message must have return code != 0x00 (feat_req_someip_727)"
                );

                // Verify Service ID copied from request
                assert_eq!(
                    error_header.service_id, 0x1234,
                    "Error should copy Service ID from request (feat_req_someip_655)"
                );

                // Verify Method ID copied from request
                assert_eq!(
                    error_header.method_id, 0x0001,
                    "Error should copy Method ID from request (feat_req_someip_655)"
                );

                // Verify Client ID copied from request (Request ID part 1)
                assert_eq!(
                    error_header.client_id, 0x0042,
                    "Error should copy Client ID from request (feat_req_someip_655)"
                );

                // Verify Session ID copied from request (Request ID part 2)
                assert_eq!(
                    error_header.session_id, 0x1337,
                    "Error should copy Session ID from request (feat_req_someip_655)"
                );
            } else {
                panic!("Failed to parse error response header");
            }
        } else {
            panic!("Did not receive error response");
        }

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_106: EXCEPTION (0x81) is used when configured per-method
///
/// When a method is configured to use EXCEPTION for errors via MethodConfig,
/// the error response must use message type 0x81 instead of 0x80.
#[test_log::test]
fn exception_message_type_when_configured() {
    use bytes::{BufMut, BytesMut};
    use recentip::handle::ServiceEvent;
    use recentip::prelude::*;
    use recentip::wire::MessageType;
    use std::net::SocketAddr;
    use std::time::Duration;

    covers!(feat_req_someip_106);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server configured to use EXCEPTION for method 0x0001
    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        // Configure method 0x0001 to use EXCEPTION for errors
        let method_config = MethodConfig::new().use_exception_for(0x0001);

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .method_config(method_config)
            .udp()
            .start()
            .await
            .unwrap();

        // Receive call and send error
        if let Some(event) = tokio::time::timeout(Duration::from_secs(10), offering.next())
            .await
            .ok()
            .flatten()
        {
            if let ServiceEvent::Call { responder, .. } = event {
                responder
                    .reply_error(ApplicationError::UnknownMethod)
                    .unwrap();
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    // Raw socket verifies EXCEPTION message type is used
    sim.client("raw_observer", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Discover server via SD
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4(
            "239.255.0.1".parse().unwrap(),
            "0.0.0.0".parse().unwrap()
        )?;

        let mut server_endpoint: Option<SocketAddr> = None;
        let mut buf = [0u8; 1500];

        for _ in 0..20 {
            let result = tokio::time::timeout(
                Duration::from_millis(200),
                sd_socket.recv_from(&mut buf),
            ).await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 {
                            if let Some(opt) = sd_msg.options.first() {
                                if let recentip::wire::SdOption::Ipv4Endpoint { addr, port, .. } = opt {
                                    let ip = if addr.is_unspecified() {
                                        from.ip()
                                    } else {
                                        std::net::IpAddr::V4(*addr)
                                    };
                                    server_endpoint = Some(SocketAddr::new(ip, *port));
                                }
                            }
                        }
                    }
                }
            }
            if server_endpoint.is_some() {
                break;
            }
        }

        let server_addr = server_endpoint.expect("Should find server via SD");

        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        // Send request to method 0x0001 (configured for EXCEPTION)
        let mut request = BytesMut::with_capacity(24);
        request.put_u16(0x1234); // Service ID
        request.put_u16(0x0001); // Method ID - configured for EXCEPTION
        request.put_u32(0x00000010); // Length = 16
        request.put_u16(0x0042); // Client ID
        request.put_u16(0x1337); // Session ID
        request.put_u8(0x01); // Protocol Version
        request.put_u8(0x01); // Interface Version
        request.put_u8(0x00); // Message Type = REQUEST
        request.put_u8(0x00); // Return Code
        request.put_slice(b"testdata"); // 8 bytes payload

        rpc_socket.send_to(&request, server_addr).await?;

        let result = tokio::time::timeout(
            Duration::from_secs(5),
            rpc_socket.recv_from(&mut buf)
        ).await;

        if let Ok(Ok((len, _))) = result {
            if let Some(error_header) = parse_header_wire(&buf[..len]) {
                // Method 0x0001 is configured for EXCEPTION, so should be 0x81
                assert_eq!(
                    error_header.message_type,
                    MessageType::Error,
                    "Error response should be EXCEPTION (0x81) when configured (feat_req_someip_106)"
                );

                assert_ne!(
                    error_header.return_code, 0x00,
                    "Error message must have return code != 0x00"
                );
            } else {
                panic!("Failed to parse error response header");
            }
        } else {
            panic!("Did not receive error response");
        }

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_106, feat_req_someip_726: Mixed config - some methods EXCEPTION, some RESPONSE
///
/// When some methods are configured for EXCEPTION and others are not,
/// each method should use the appropriate message type.
#[test_log::test]
fn mixed_exception_config_per_method() {
    use bytes::{BufMut, BytesMut};
    use recentip::handle::ServiceEvent;
    use recentip::prelude::*;
    use recentip::wire::MessageType;
    use std::net::SocketAddr;
    use std::time::Duration;

    covers!(feat_req_someip_106, feat_req_someip_726);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server with mixed config: method 0x0001 uses EXCEPTION, method 0x0002 uses RESPONSE
    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        // Only method 0x0001 configured for EXCEPTION
        let method_config = MethodConfig::new().use_exception_for(0x0001);

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .method_config(method_config)
            .udp()
            .start()
            .await
            .unwrap();

        // Handle two calls - respond with errors for both
        for _ in 0..2 {
            if let Some(event) = tokio::time::timeout(Duration::from_secs(10), offering.next())
                .await
                .ok()
                .flatten()
            {
                if let ServiceEvent::Call { responder, .. } = event {
                    responder.reply_error(ApplicationError::NotOk).unwrap();
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("raw_observer", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Discover server via SD
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut server_endpoint: Option<SocketAddr> = None;
        let mut buf = [0u8; 1500];

        for _ in 0..20 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 {
                            if let Some(opt) = sd_msg.options.first() {
                                if let recentip::wire::SdOption::Ipv4Endpoint {
                                    addr, port, ..
                                } = opt
                                {
                                    let ip = if addr.is_unspecified() {
                                        from.ip()
                                    } else {
                                        std::net::IpAddr::V4(*addr)
                                    };
                                    server_endpoint = Some(SocketAddr::new(ip, *port));
                                }
                            }
                        }
                    }
                }
            }
            if server_endpoint.is_some() {
                break;
            }
        }

        let server_addr = server_endpoint.expect("Should find server via SD");
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        // Test 1: Method 0x0001 (configured for EXCEPTION) should get 0x81
        let mut request1 = BytesMut::with_capacity(24);
        request1.put_u16(0x1234);
        request1.put_u16(0x0001); // Method configured for EXCEPTION
        request1.put_u32(0x00000010);
        request1.put_u16(0x0001);
        request1.put_u16(0x0001);
        request1.put_u8(0x01);
        request1.put_u8(0x01);
        request1.put_u8(0x00);
        request1.put_u8(0x00);
        request1.put_slice(b"testdata");

        rpc_socket.send_to(&request1, server_addr).await?;

        let result1 =
            tokio::time::timeout(Duration::from_secs(5), rpc_socket.recv_from(&mut buf)).await;

        if let Ok(Ok((len, _))) = result1 {
            if let Some(header) = parse_header_wire(&buf[..len]) {
                assert_eq!(
                    header.message_type,
                    MessageType::Error,
                    "Method 0x0001 should use EXCEPTION (0x81)"
                );
            } else {
                panic!("Failed to parse response 1");
            }
        } else {
            panic!("Did not receive response 1");
        }

        // Test 2: Method 0x0002 (NOT configured) should get 0x80
        let mut request2 = BytesMut::with_capacity(24);
        request2.put_u16(0x1234);
        request2.put_u16(0x0002); // Method NOT configured for EXCEPTION
        request2.put_u32(0x00000010);
        request2.put_u16(0x0001);
        request2.put_u16(0x0002);
        request2.put_u8(0x01);
        request2.put_u8(0x01);
        request2.put_u8(0x00);
        request2.put_u8(0x00);
        request2.put_slice(b"testdata");

        rpc_socket.send_to(&request2, server_addr).await?;

        let result2 =
            tokio::time::timeout(Duration::from_secs(5), rpc_socket.recv_from(&mut buf)).await;

        if let Ok(Ok((len, _))) = result2 {
            if let Some(header) = parse_header_wire(&buf[..len]) {
                assert_eq!(
                    header.message_type,
                    MessageType::Response,
                    "Method 0x0002 should use RESPONSE (0x80) - not configured for EXCEPTION"
                );
                assert_ne!(
                    header.return_code, 0x00,
                    "Should still have error return code"
                );
            } else {
                panic!("Failed to parse response 2");
            }
        } else {
            panic!("Did not receive response 2");
        }

        Ok(())
    });

    sim.run().unwrap();
}

/// Test that internal errors (UNKNOWN_SERVICE) use RESPONSE (0x80)
///
/// When the runtime itself generates an error (not the application),
/// it uses RESPONSE with error code since there's no method config available.
#[test_log::test]
fn internal_unknown_service_error_uses_response() {
    use bytes::{BufMut, BytesMut};
    use recentip::prelude::*;
    use recentip::wire::MessageType;
    use std::net::SocketAddr;
    use std::time::Duration;

    covers!(feat_req_someip_726);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service 0x1234
    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
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

        // Keep server alive
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });

    sim.client("raw_observer", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Discover server via SD
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut server_endpoint: Option<SocketAddr> = None;
        let mut buf = [0u8; 1500];

        for _ in 0..20 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 {
                            if let Some(opt) = sd_msg.options.first() {
                                if let recentip::wire::SdOption::Ipv4Endpoint {
                                    addr, port, ..
                                } = opt
                                {
                                    let ip = if addr.is_unspecified() {
                                        from.ip()
                                    } else {
                                        std::net::IpAddr::V4(*addr)
                                    };
                                    server_endpoint = Some(SocketAddr::new(ip, *port));
                                }
                            }
                        }
                    }
                }
            }
            if server_endpoint.is_some() {
                break;
            }
        }

        let server_addr = server_endpoint.expect("Should find server via SD");
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        // Send request for WRONG service ID (0x9999 instead of 0x1234)
        let mut request = BytesMut::with_capacity(24);
        request.put_u16(0x9999); // WRONG Service ID - runtime will return UNKNOWN_SERVICE
        request.put_u16(0x0001);
        request.put_u32(0x00000010);
        request.put_u16(0x0001);
        request.put_u16(0x0001);
        request.put_u8(0x01);
        request.put_u8(0x01);
        request.put_u8(0x00);
        request.put_u8(0x00);
        request.put_slice(b"testdata");

        rpc_socket.send_to(&request, server_addr).await?;

        let result =
            tokio::time::timeout(Duration::from_secs(2), rpc_socket.recv_from(&mut buf)).await;

        if let Ok(Ok((len, _))) = result {
            if let Some(header) = parse_header_wire(&buf[..len]) {
                // Internal errors should use RESPONSE (0x80) since there's no config
                assert_eq!(
                    header.message_type,
                    MessageType::Response,
                    "Internal UNKNOWN_SERVICE error should use RESPONSE (0x80)"
                );
                assert_eq!(
                    header.return_code,
                    ReturnCode::UnknownService as u8,
                    "Should return UNKNOWN_SERVICE error code"
                );
            } else {
                panic!("Failed to parse response");
            }
        }
        // Note: Not receiving a response is also acceptable (runtime may silently ignore)

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_798: Messages with length < 8 shall be ignored
///
/// SOME/IP messages with a length value < 8 bytes shall be ignored.
/// Length field indicates payload + 8 (for the header tail), so minimum is 8.
#[test_log::test]
fn messages_with_short_length_ignored() {
    use bytes::{BufMut, BytesMut};
    use recentip::prelude::*;
    use std::net::SocketAddr;
    use std::time::Duration;

    covers!(feat_req_someip_798);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server should ignore malformed messages
    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
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

        // Wait for any events - malformed message should be ignored
        let result = tokio::time::timeout(Duration::from_secs(2), offering.next()).await;

        // Should timeout - no valid event should be received
        // The message with Length < 8 should be rejected at parse time (feat_req_someip_798)
        assert!(
            result.is_err(),
            "Malformed message with Length < 8 should be ignored (feat_req_someip_798)"
        );

        Ok(())
    });

    // Raw socket discovers server and injects malformed message
    sim.client("attacker", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Discover server via SD
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut server_endpoint: Option<SocketAddr> = None;
        let mut buf = [0u8; 1500];

        for _ in 0..20 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 {
                            if let Some(opt) = sd_msg.options.first() {
                                if let recentip::wire::SdOption::Ipv4Endpoint {
                                    addr, port, ..
                                } = opt
                                {
                                    let ip = if addr.is_unspecified() {
                                        from.ip()
                                    } else {
                                        std::net::IpAddr::V4(*addr)
                                    };
                                    server_endpoint = Some(SocketAddr::new(ip, *port));
                                }
                            }
                        }
                    }
                }
            }
            if server_endpoint.is_some() {
                break;
            }
        }

        let server_addr = server_endpoint.expect("Should find server via SD");

        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        // Create malformed message with Length = 4 (< 8)
        let mut malformed = BytesMut::with_capacity(16);
        malformed.put_u16(0x1234); // Service ID
        malformed.put_u16(0x0001); // Method ID
        malformed.put_u32(0x00000004); // Length = 4 (INVALID! Must be >= 8)
        malformed.put_u16(0x0001); // Client ID
        malformed.put_u16(0x0001); // Session ID
        malformed.put_u8(0x01); // Protocol Version
        malformed.put_u8(0x01); // Interface Version
        malformed.put_u8(0x00); // Message Type = REQUEST
        malformed.put_u8(0x00); // Return Code

        socket.send_to(&malformed, server_addr).await?;

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_703: Implementation shall use known protocol version
///
/// All messages sent by the library must use protocol version 0x01.
#[test_log::test]
fn uses_known_protocol_version() {
    use recentip::prelude::*;
    use recentip::wire::Header;
    use std::net::SocketAddr;
    use std::time::Duration;

    covers!(feat_req_someip_703);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library side - server that can respond to requests
    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
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

        // Respond to a few calls
        for _ in 0..3 {
            if let Some(event) = tokio::time::timeout(Duration::from_secs(5), offering.next())
                .await
                .ok()
                .flatten()
            {
                if let recentip::handle::ServiceEvent::Call { responder, .. } = event {
                    let _ = responder.reply(b"response");
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    // Raw socket side - discovers server, sends requests, and captures responses
    sim.client("raw_observer", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Discover server via SD
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut server_endpoint: Option<SocketAddr> = None;
        let mut buf = [0u8; 1500];

        // Also capture SD messages for protocol version check
        let mut captured_messages = Vec::new();

        for _ in 0..20 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                // Check protocol version of SD messages
                if len >= Header::SIZE {
                    if let Some(header) = parse_header_wire(&buf[..len]) {
                        captured_messages.push(header);
                    }
                }

                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 {
                            if let Some(opt) = sd_msg.options.first() {
                                if let recentip::wire::SdOption::Ipv4Endpoint {
                                    addr, port, ..
                                } = opt
                                {
                                    let ip = if addr.is_unspecified() {
                                        from.ip()
                                    } else {
                                        std::net::IpAddr::V4(*addr)
                                    };
                                    server_endpoint = Some(SocketAddr::new(ip, *port));
                                }
                            }
                        }
                    }
                }
            }
            if server_endpoint.is_some() {
                break;
            }
        }

        let server_addr = server_endpoint.expect("Should find server via SD");

        // Send some RPC requests and capture responses
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        for i in 0..3 {
            use bytes::{BufMut, BytesMut};

            let mut request = BytesMut::with_capacity(24);
            request.put_u16(0x1234); // Service ID
            request.put_u16(0x0001); // Method ID
            request.put_u32(0x00000010); // Length = 16
            request.put_u16(0x0001); // Client ID
            request.put_u16(i as u16); // Session ID
            request.put_u8(0x01); // Protocol Version
            request.put_u8(0x01); // Interface Version
            request.put_u8(0x00); // Message Type = REQUEST
            request.put_u8(0x00); // Return Code
            request.put_slice(b"testdata"); // 8 bytes payload

            rpc_socket.send_to(&request, server_addr).await?;

            // Capture response
            if let Ok(Ok((len, _))) =
                tokio::time::timeout(Duration::from_millis(500), rpc_socket.recv_from(&mut buf))
                    .await
            {
                if len >= Header::SIZE {
                    if let Some(header) = parse_header_wire(&buf[..len]) {
                        captured_messages.push(header);
                    }
                }
            }
        }

        // Verify all captured messages use protocol version 0x01
        assert!(
            !captured_messages.is_empty(),
            "Should capture at least some messages"
        );

        for header in &captured_messages {
            assert_eq!(
                header.protocol_version, 0x01,
                "All messages must use protocol version 0x01 (feat_req_someip_703)"
            );
        }

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_703, feat_req_someip_818:
/// Wrong protocol version returns E_WRONG_PROTOCOL_VERSION or is ignored
///
/// When receiving a message with wrong protocol version, the implementation
/// may either ignore it or respond with E_WRONG_PROTOCOL_VERSION.
#[test_log::test]
fn wrong_protocol_version_returns_error() {
    use bytes::{BufMut, BytesMut};
    use recentip::prelude::*;
    use recentip::wire::MessageType;
    use std::net::SocketAddr;
    use std::time::Duration;

    covers!(feat_req_someip_703, feat_req_someip_818);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server receives request with wrong protocol version
    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
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

        // Wait for any valid events - should not receive the malformed request
        let result = tokio::time::timeout(Duration::from_secs(2), offering.next()).await;

        // Malformed request should be ignored (no event delivered)
        assert!(
            result.is_err(),
            "Request with wrong protocol version should be ignored or rejected"
        );

        Ok(())
    });

    // Raw socket discovers server and injects request with wrong protocol version
    sim.client("attacker", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Discover server via SD
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut server_endpoint: Option<SocketAddr> = None;
        let mut buf = [0u8; 1500];

        for _ in 0..20 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 {
                            if let Some(opt) = sd_msg.options.first() {
                                if let recentip::wire::SdOption::Ipv4Endpoint {
                                    addr, port, ..
                                } = opt
                                {
                                    let ip = if addr.is_unspecified() {
                                        from.ip()
                                    } else {
                                        std::net::IpAddr::V4(*addr)
                                    };
                                    server_endpoint = Some(SocketAddr::new(ip, *port));
                                }
                            }
                        }
                    }
                }
            }
            if server_endpoint.is_some() {
                break;
            }
        }

        let server_addr = server_endpoint.expect("Should find server via SD");

        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        // Create request with wrong protocol version
        let mut bad_request = BytesMut::with_capacity(24);
        bad_request.put_u16(0x1234); // Service ID
        bad_request.put_u16(0x0001); // Method ID
        bad_request.put_u32(0x00000010); // Length = 16
        bad_request.put_u16(0x0001); // Client ID
        bad_request.put_u16(0x0001); // Session ID
        bad_request.put_u8(0x99); // Wrong Protocol Version (should be 0x01)
        bad_request.put_u8(0x01); // Interface Version
        bad_request.put_u8(0x00); // Message Type = REQUEST
        bad_request.put_u8(0x00); // Return Code
        bad_request.put_slice(b"testdata"); // 8 bytes payload

        socket.send_to(&bad_request, server_addr).await?;

        // Check if server responds with error (optional per spec)
        let result = tokio::time::timeout(Duration::from_secs(1), socket.recv_from(&mut buf)).await;

        if let Ok(Ok((len, _))) = result {
            // Server responded - should be error with WrongProtocolVersion
            if let Some(header) = parse_header_wire(&buf[..len]) {
                assert_eq!(
                    header.message_type,
                    MessageType::Error,
                    "Response should be ERROR"
                );
                assert_eq!(
                    header.return_code,
                    ReturnCode::WrongProtocolVersion as u8,
                    "Should return E_WRONG_PROTOCOL_VERSION (feat_req_someip_703)"
                );
            }
        }
        // Note: Not receiving a response is also valid (feat_req_someip_818 - may ignore)

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// Service ID Validation Tests
// ============================================================================

/// feat_req_someip_816: Service ID must match socket's service when using service-specific RPC sockets
///
/// When a request is sent to a service-specific RPC socket (e.g., the UDP port announced
/// in an Offer entry), the service_id in the SOME/IP header must match the socket's service.
/// If mismatched, the server should reject with E_UNKNOWN_SERVICE (0x02).
///
/// This prevents a malicious or misconfigured client from routing requests to the wrong
/// service by sending requests with incorrect service IDs to service-specific endpoints.
#[test_log::test]
fn service_id_mismatch_on_service_socket_rejected() {
    use bytes::{BufMut, BytesMut};
    use recentip::prelude::*;
    use recentip::wire::MessageType;
    use std::net::SocketAddr;
    use std::time::Duration;

    covers!(feat_req_someip_816);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service 0x1234
    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(0x1234, InstanceId::Id(0x0001))
            .version(1, 0)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for requests - should NOT receive any (only mismatched ones sent)
        let result = tokio::time::timeout(Duration::from_secs(3), offering.next()).await;

        // Verify the service did NOT receive the mismatched request
        assert!(
            result.is_err(),
            "Service should NOT receive requests with mismatched service_id"
        );

        Ok(())
    });

    // Raw client sends request with wrong service_id to server's RPC port
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut server_endpoint: Option<SocketAddr> = None;
        let mut buf = [0u8; 1500];

        // Discover server's RPC endpoint via SD
        for _ in 0..20 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // Look for OfferService entry for 0x1234
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 {
                            if let Some(opt) = sd_msg.options.first() {
                                if let recentip::wire::SdOption::Ipv4Endpoint {
                                    addr, port, ..
                                } = opt
                                {
                                    let ip = if addr.is_unspecified() {
                                        from.ip()
                                    } else {
                                        std::net::IpAddr::V4(*addr)
                                    };
                                    server_endpoint = Some(SocketAddr::new(ip, *port));
                                }
                            }
                        }
                    }
                }
            }
            if server_endpoint.is_some() {
                break;
            }
        }

        let server_addr = server_endpoint.expect("Should find server via SD");

        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        // Create request with WRONG service_id (0x9999 instead of 0x1234)
        // This is sent to the socket belonging to service 0x1234
        let mut bad_request = BytesMut::with_capacity(24);
        bad_request.put_u16(0x9999); // WRONG Service ID (socket belongs to 0x1234)
        bad_request.put_u16(0x0001); // Method ID
        bad_request.put_u32(0x00000010); // Length = 16
        bad_request.put_u16(0x0001); // Client ID
        bad_request.put_u16(0x0001); // Session ID
        bad_request.put_u8(0x01); // Protocol Version
        bad_request.put_u8(0x01); // Interface Version
        bad_request.put_u8(0x00); // Message Type = REQUEST
        bad_request.put_u8(0x00); // Return Code
        bad_request.put_slice(b"testdata"); // 8 bytes payload

        socket.send_to(&bad_request, server_addr).await?;

        // Check if server responds with E_UNKNOWN_SERVICE error
        let result = tokio::time::timeout(Duration::from_secs(1), socket.recv_from(&mut buf)).await;

        if let Ok(Ok((len, _))) = result {
            // Server responded - should be error with E_UNKNOWN_SERVICE
            if let Some(header) = parse_header_wire(&buf[..len]) {
                assert!(
                    header.message_type == MessageType::Response
                        || header.message_type == MessageType::Error,
                    "Response should be RESPONSE or ERROR"
                );
                assert_eq!(
                    header.return_code, 0x02,
                    "Should return E_UNKNOWN_SERVICE (0x02) for mismatched service_id"
                );
                // Verify header fields are echoed correctly
                assert_eq!(header.service_id, 0x9999, "Service ID should be echoed");
                assert_eq!(header.method_id, 0x0001, "Method ID should be echoed");
                assert_eq!(header.client_id, 0x0001, "Client ID should be echoed");
                assert_eq!(header.session_id, 0x0001, "Session ID should be echoed");
            } else {
                panic!("Failed to parse response header");
            }
        } else {
            // Per feat_req_someip_816, E_UNKNOWN_SERVICE is optional
            // But if no response, the request must NOT have been processed
            tracing::info!("No response received - server silently rejected (also valid)");
        }

        Ok(())
    });

    sim.run().unwrap();
}

/// Fire-and-forget with mismatched service_id should be silently ignored
///
/// Similar to the above test, but for RequestNoReturn messages.
/// Since fire-and-forget doesn't expect responses, mismatched requests
/// should be silently ignored.
#[test_log::test]
fn fire_and_forget_service_id_mismatch_ignored() {
    use bytes::{BufMut, BytesMut};
    use recentip::prelude::*;
    use std::net::SocketAddr;
    use std::time::Duration;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service 0x1234
    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(0x1234, InstanceId::Id(0x0001))
            .version(1, 0)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for requests - should NOT receive any (only mismatched ones sent)
        let result = tokio::time::timeout(Duration::from_secs(2), offering.next()).await;

        // Verify the service did NOT receive the mismatched fire-and-forget
        assert!(
            result.is_err(),
            "Service should NOT receive fire-and-forget with mismatched service_id"
        );

        Ok(())
    });

    // Raw client sends fire-and-forget with wrong service_id
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut server_endpoint: Option<SocketAddr> = None;
        let mut buf = [0u8; 1500];

        // Discover server's RPC endpoint via SD
        for _ in 0..20 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 {
                            if let Some(opt) = sd_msg.options.first() {
                                if let recentip::wire::SdOption::Ipv4Endpoint {
                                    addr, port, ..
                                } = opt
                                {
                                    let ip = if addr.is_unspecified() {
                                        from.ip()
                                    } else {
                                        std::net::IpAddr::V4(*addr)
                                    };
                                    server_endpoint = Some(SocketAddr::new(ip, *port));
                                }
                            }
                        }
                    }
                }
            }
            if server_endpoint.is_some() {
                break;
            }
        }

        let server_addr = server_endpoint.expect("Should find server via SD");

        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        // Create fire-and-forget with WRONG service_id (0x9999 instead of 0x1234)
        let mut bad_request = BytesMut::with_capacity(24);
        bad_request.put_u16(0x9999); // WRONG Service ID (socket belongs to 0x1234)
        bad_request.put_u16(0x0001); // Method ID
        bad_request.put_u32(0x00000010); // Length = 16
        bad_request.put_u16(0x0001); // Client ID
        bad_request.put_u16(0x0001); // Session ID
        bad_request.put_u8(0x01); // Protocol Version
        bad_request.put_u8(0x01); // Interface Version
        bad_request.put_u8(0x01); // Message Type = REQUEST_NO_RETURN (fire-and-forget)
        bad_request.put_u8(0x00); // Return Code
        bad_request.put_slice(b"testdata"); // 8 bytes payload

        socket.send_to(&bad_request, server_addr).await?;

        // Fire-and-forget should not get any response
        let result =
            tokio::time::timeout(Duration::from_millis(500), socket.recv_from(&mut buf)).await;

        assert!(
            result.is_err(),
            "Fire-and-forget should not receive any response (even for mismatched service_id)"
        );

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// Request on subscriber connection
// ============================================================================

/// A wire server sends a SOME/IP Request on a TCP subscription connection.
///
/// When a lib client subscribes over TCP, **the client** establishes the TCP
/// connection to the server. The server then sends events back on that same
/// connection. A buggy or malicious server could instead send a SOME/IP
/// Request — essentially trying to call a method on the client via the
/// subscriber connection.
///
/// The runtime should silently drop such requests: they arrived on a
/// client-initiated connection, not on the service's own server socket.
/// Dispatching them to the offered service would give a remote peer unexpected
/// access to method-call semantics.
#[test_log::test]
fn request_on_subscriber_tcp_connection_is_ignored() {
    use crate::wire_format::helpers::{SdOfferBuilder, SdSubscribeAckBuilder, SomeIpPacketBuilder};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Service IDs chosen to avoid collisions with other tests in this file.
    const CLIENT_SVC_ID: u16 = 0x6001; // offered by the lib client
    const SERVER_SVC_ID: u16 = 0x6000; // offered by the wire server
    const EVENTGROUP_ID: u16 = 0x0001;
    const SERVER_TCP_PORT: u16 = 51001;
    const CLIENT_TCP_PORT: u16 = 52001;
    const MAJOR_VERSION: u8 = 1;
    const METHOD_ID: u16 = 0x0001;

    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_client = Arc::clone(&call_count);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(15))
        .build();

    // Wire server: offers SERVER_SVC_ID via SD with a TCP endpoint.
    // Once the lib client subscribes and the TCP connection is established,
    // sends a SOME/IP Request for CLIENT_SVC_ID on that connection.
    sim.host("wire-server", move || async move {
        let server_ip: std::net::Ipv4Addr = match turmoil::lookup("wire-server") {
            std::net::IpAddr::V4(a) => a,
            _ => unreachable!("expected IPv4"),
        };

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4(
            "239.255.0.1".parse().unwrap(),
            "0.0.0.0".parse().unwrap(),
        )?;

        let tcp_listener =
            turmoil::net::TcpListener::bind(format!("0.0.0.0:{SERVER_TCP_PORT}")).await?;

        // Accept the subscriber's incoming TCP connection in background.
        let accepted: Arc<tokio::sync::Mutex<Option<turmoil::net::TcpStream>>> =
            Arc::new(tokio::sync::Mutex::new(None));
        let accepted_bg = Arc::clone(&accepted);
        tokio::spawn(async move {
            if let Ok((stream, addr)) = tcp_listener.accept().await {
                tracing::info!("Wire server: TCP connection from {addr}");
                *accepted_bg.lock().await = Some(stream);
            }
        });

        let sd_multicast: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();
        let mut buf = vec![0u8; 65535];
        let mut multicast_session = 1u16;
        let mut unicast_session = 1u16;

        // Send periodic SD offers until the client subscribes.
        // A single up-front offer would be lost if the client hasn't joined multicast yet.
        let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);
        let subscribe_from = loop {
            if last_offer.elapsed() >= Duration::from_secs(1) {
                let offer = SdOfferBuilder::new(SERVER_SVC_ID, 0x0001, server_ip, SERVER_TCP_PORT)
                    .tcp()
                    .session_id(multicast_session)
                    .build();
                multicast_session += 1;
                sd_socket.send_to(&offer, sd_multicast).await?;
                last_offer = tokio::time::Instant::now();
            }

            let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf)).await
            else {
                continue;
            };
            let data = &buf[..len];
            if data.len() < 36 {
                continue;
            }
            let svc_id_hdr = u16::from_be_bytes([data[0], data[1]]);
            let mth_id_hdr = u16::from_be_bytes([data[2], data[3]]);
            if svc_id_hdr != 0xFFFF || mth_id_hdr != 0x8100 {
                continue;
            }
            // Entry array starts at offset 24 in the SD payload (after SOME/IP+SD headers).
            let entry_type = data[24];
            let entry_svc = u16::from_be_bytes([data[28], data[29]]);
            if entry_type == 0x06 /* SubscribeEventgroup */ && entry_svc == SERVER_SVC_ID {
                tracing::info!("Wire server: SubscribeEventgroup from {from}");
                break from;
            }
        };

        // Acknowledge the subscription (unicast).
        let ack = SdSubscribeAckBuilder::new(SERVER_SVC_ID, 0x0001, EVENTGROUP_ID)
            .major_version(MAJOR_VERSION)
            .ttl(3000)
            .session_id(unicast_session)
            .build();
        unicast_session += 1;
        sd_socket.send_to(&ack, subscribe_from).await?;

        // Wait for the TCP connection to be established.
        let mut stream = loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let mut g = accepted.lock().await;
            if g.is_some() {
                break g.take().unwrap();
            }
        };

        // Give the lib client time to fully process the ACK.
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Send a SOME/IP Request for CLIENT_SVC_ID on the **subscription** TCP connection.
        // The lib client established this TCP connection to receive events; the server must
        // NOT abuse it to trigger method calls on the lib client's service.
        let request = SomeIpPacketBuilder::request(CLIENT_SVC_ID, METHOD_ID)
            .client_id(0xDEAD)
            .session_id(0x0001)
            .payload(b"bad_request_on_sub_conn")
            .build();
        stream.write_all(&request).await?;
        tracing::info!("Wire server: sent Request for CLIENT_SVC_ID on subscription connection");

        // Expect no response; a brief read confirms nothing is sent back.
        let mut resp = vec![0u8; 256];
        match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut resp)).await {
            Ok(Ok(0)) => tracing::info!("Wire server: connection closed by client"),
            Ok(Ok(n)) => tracing::warn!("Wire server: unexpected {n}-byte response"),
            Err(_) => tracing::info!("Wire server: no response (request dropped, as expected)"),
            Ok(Err(e)) => tracing::warn!("Wire server: read error: {e}"),
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        // Offer CLIENT_SVC_ID — this is what the wire server will try to call.
        let mut offering = runtime
            .offer(CLIENT_SVC_ID, InstanceId::Id(1))
            .version(MAJOR_VERSION, 0)
            .tcp_port(CLIENT_TCP_PORT)
            .start()
            .await
            .expect("offer client service");

        let cc = Arc::clone(&call_count_client);
        tokio::spawn(async move {
            while let Some(event) = offering.next().await {
                if let ServiceEvent::Call { responder, .. } = event {
                    cc.fetch_add(1, Ordering::SeqCst);
                    let _ = responder.reply(&[]);
                }
            }
        });

        // Find SERVER_SVC_ID and subscribe over TCP.
        // This causes the runtime to connect TCP to the wire server's SERVER_TCP_PORT.
        let proxy = tokio::time::timeout(Duration::from_secs(5), runtime.find(SERVER_SVC_ID))
            .await
            .expect("service discovery must not timeout")
            .expect("service must be found");

        let _subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(EventgroupId::new(EVENTGROUP_ID).unwrap()),
        )
        .await
        .expect("subscribe must not timeout")
        .expect("subscribe must succeed");

        // Allow time for the wire server to send the bad request and the runtime to process it.
        tokio::time::sleep(Duration::from_secs(4)).await;

        assert_eq!(
            call_count_client.load(Ordering::SeqCst),
            0,
            "A Request on a subscriber TCP connection must NOT dispatch to the offered service"
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// A wire server sends a SOME/IP Request to the UDP socket B uses as its
/// subscription endpoint.
///
/// When a lib client (B) subscribes over UDP, B advertises a UDP endpoint in
/// the SubscribeEventgroup SD message. A (the wire server) is expected to send
/// only event *Notifications* to that endpoint. A buggy or malicious server
/// could instead send a SOME/IP *Request* to that same UDP socket, attempting
/// to invoke a method on B's offered service via B's subscription socket.
///
/// B's subscription socket is the shared client RPC socket (ephemeral port),
/// not B's server socket. The runtime must silently drop such requests.
#[test_log::test]
fn request_on_subscriber_udp_socket_is_ignored() {
    use crate::wire_format::helpers::{
        parse_sd_packet, SdOfferBuilder, SdSubscribeAckBuilder, SomeIpPacketBuilder,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // Different IDs from the TCP variant to avoid conflicts.
    const CLIENT_SVC_ID: u16 = 0x6003;
    const SERVER_SVC_ID: u16 = 0x6002;
    const EVENTGROUP_ID: u16 = 0x0001;
    const CLIENT_UDP_PORT: u16 = 52002;
    const MAJOR_VERSION: u8 = 1;
    const METHOD_ID: u16 = 0x0001;

    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_client = Arc::clone(&call_count);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(15))
        .build();

    // Wire server (A): offers SERVER_SVC_ID with a UDP endpoint.
    // Once B subscribes, extracts B's subscription socket port from the
    // SubscribeEventgroup SD options, then sends a SOME/IP Request for
    // CLIENT_SVC_ID directly to that endpoint.
    sim.host("wire-server", move || async move {
        let server_ip: std::net::Ipv4Addr = match turmoil::lookup("wire-server") {
            std::net::IpAddr::V4(a) => a,
            _ => unreachable!("expected IPv4"),
        };

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4(
            "239.255.0.1".parse().unwrap(),
            "0.0.0.0".parse().unwrap(),
        )?;

        let sd_multicast: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();
        let mut buf = vec![0u8; 65535];
        let mut multicast_session = 1u16;
        let mut unicast_session = 1u16;

        // Periodically offer SERVER_SVC_ID (UDP endpoint) until B subscribes.
        let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);
        let (subscribe_from, subscription_port) = loop {
            if last_offer.elapsed() >= Duration::from_secs(1) {
                let offer =
                    SdOfferBuilder::new(SERVER_SVC_ID, 0x0001, server_ip, 50002)
                        .session_id(multicast_session)
                        .build();
                multicast_session += 1;
                sd_socket.send_to(&offer, sd_multicast).await?;
                last_offer = tokio::time::Instant::now();
            }

            let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await
            else {
                continue;
            };

            let Some((_hdr, sd)) = parse_sd_packet(&buf[..len]) else {
                continue;
            };

            let found_port = sd
                .subscribe_entries()
                .filter(|e| e.service_id == SERVER_SVC_ID)
                .find_map(|e| sd.endpoint_port_for_entry(e));

            if let Some(port) = found_port {
                tracing::info!("Wire server: SubscribeEventgroup from {from}, subscription port {port}");
                break (from, port);
            }
        };

        // Acknowledge the subscription (unicast to B's SD socket).
        let ack = SdSubscribeAckBuilder::new(SERVER_SVC_ID, 0x0001, EVENTGROUP_ID)
            .major_version(MAJOR_VERSION)
            .ttl(3000)
            .session_id(unicast_session)
            .build();
        unicast_session += 1;
        sd_socket.send_to(&ack, subscribe_from).await?;

        // Give B time to process the ACK.
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Derive B's subscription endpoint: the IP from the subscribe sender,
        // the port from the SD options.
        let subscriber_ip = match subscribe_from {
            std::net::SocketAddr::V4(a) => *a.ip(),
            std::net::SocketAddr::V6(_) => unreachable!("IPv4 only"),
        };
        let subscription_addr: std::net::SocketAddr = (subscriber_ip, subscription_port).into();

        // Send a SOME/IP Request for CLIENT_SVC_ID to B's subscription socket.
        // B's subscription socket is its shared client RPC socket — the runtime
        // must NOT route this as a method call to B's offered service.
        let request = SomeIpPacketBuilder::request(CLIENT_SVC_ID, METHOD_ID)
            .client_id(0xDEAD)
            .session_id(0x0001)
            .payload(b"bad_udp_request_on_sub_socket")
            .build();
        sd_socket.send_to(&request, subscription_addr).await?;
        tracing::info!("Wire server: sent Request for CLIENT_SVC_ID to {subscription_addr}");

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        // Offer CLIENT_SVC_ID — this is what the wire server will try to call.
        let mut offering = runtime
            .offer(CLIENT_SVC_ID, InstanceId::Id(1))
            .version(MAJOR_VERSION, 0)
            .udp_port(CLIENT_UDP_PORT)
            .start()
            .await
            .expect("offer client service");

        let cc = Arc::clone(&call_count_client);
        tokio::spawn(async move {
            while let Some(event) = offering.next().await {
                if let ServiceEvent::Call { responder, .. } = event {
                    cc.fetch_add(1, Ordering::SeqCst);
                    let _ = responder.reply(&[]);
                }
            }
        });

        // Find SERVER_SVC_ID and subscribe via UDP.
        // With default port selection (PortSpec::Any) and no prior subscriptions
        // for this service, B will use its client_rpc_endpoint as the subscription
        // socket — the same socket it uses for outgoing RPC calls.
        let proxy = tokio::time::timeout(Duration::from_secs(5), runtime.find(SERVER_SVC_ID))
            .await
            .expect("service discovery must not timeout")
            .expect("service must be found");

        let _subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(EventgroupId::new(EVENTGROUP_ID).unwrap()),
        )
        .await
        .expect("subscribe must not timeout")
        .expect("subscribe must succeed");

        // Allow time for the wire server to send the bad Request and for the runtime to process it.
        tokio::time::sleep(Duration::from_secs(4)).await;

        assert_eq!(
            call_count_client.load(Ordering::SeqCst),
            0,
            "A Request on B's subscription UDP socket must NOT dispatch to the offered service"
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// Wire-Party A has a TCP server. Lib-Party B subscribes to A and also offers
/// its own TCP service. A then opens a TCP connection *to B's server socket*
/// and sends SOME/IP Notification messages for A's service through that channel.
///
/// Per the SOME/IP spec, events flow server→subscriber over the TCP connection
/// that the *subscriber* (B) established. A connection that A opens to B's
/// server is a **client** connection — only Requests and Responses are valid on
/// it. Notifications sent this way must be silently dropped and must not be
/// delivered to B's subscription handle.
///
/// Two independent defences make this safe:
/// 1. The `tcp_rpc_rx` arm explicitly drops `Notification` messages (only
///    `Request`, `RequestNoReturn`, and `Response`/`Error` are valid on the
///    server TCP path).
/// 2. `allocate_tcp_conn_key` starts from 1, so no subscription ever gets
///    key == 0, which is the sentinel passed from `tcp_rpc_rx`.
#[test_log::test]
fn notification_on_server_tcp_connection_is_ignored() {
    use crate::wire_format::helpers::{
        parse_sd_packet, SdOfferBuilder, SdSubscribeAckBuilder, SomeIpPacketBuilder,
    };
    use tokio::io::AsyncWriteExt;

    const SERVER_SVC_ID: u16 = 0x6004;
    const CLIENT_SVC_ID: u16 = 0x6005;
    const EVENTGROUP_ID: u16 = 0x0001;
    const SERVER_TCP_PORT: u16 = 50003;
    const CLIENT_TCP_PORT: u16 = 52003;
    const MAJOR_VERSION: u8 = 1;
    const EVENT_ID: u16 = 0x8001; // method IDs 0x8000–0x8FFF are events

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(15))
        .build();

    // Wire server A: offers SERVER_SVC_ID via TCP.
    // Waits for B to subscribe (B connects TCP to A's listener then sends SubscribeEventgroup),
    // sends SubscribeEventgroupAck, then opens a TCP connection to B's TCP server and sends a
    // Notification on it.
    sim.host("wire-server", move || async move {
        let server_ip: std::net::Ipv4Addr = match turmoil::lookup("wire-server") {
            std::net::IpAddr::V4(a) => a,
            _ => unreachable!("expected IPv4"),
        };
        let client_ip: std::net::Ipv4Addr = match turmoil::lookup("client") {
            std::net::IpAddr::V4(a) => a,
            _ => unreachable!("expected IPv4"),
        };

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4(
            "239.255.0.1".parse().unwrap(),
            "0.0.0.0".parse().unwrap(),
        )?;

        let tcp_listener =
            turmoil::net::TcpListener::bind(format!("0.0.0.0:{SERVER_TCP_PORT}")).await?;

        let sd_multicast: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();
        let mut buf = vec![0u8; 65535];
        let mut mc_session = 1u16;
        let mut uc_session = 1u16;

        // Accept B's TCP connection (B connects before sending SubscribeEventgroup)
        // and receive the SD subscribe — handle both concurrently.
        let accepted: std::sync::Arc<tokio::sync::Mutex<Option<turmoil::net::TcpStream>>> =
            std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let accepted_bg = std::sync::Arc::clone(&accepted);
        tokio::spawn(async move {
            if let Ok((stream, addr)) = tcp_listener.accept().await {
                tracing::info!("Wire server: B connected for events from {addr}");
                *accepted_bg.lock().await = Some(stream);
            }
        });

        // Periodically offer SERVER_SVC_ID (TCP) until B subscribes.
        let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);
        let subscribe_from = loop {
            if last_offer.elapsed() >= Duration::from_secs(1) {
                let offer = SdOfferBuilder::new(SERVER_SVC_ID, 0x0001, server_ip, SERVER_TCP_PORT)
                    .tcp()
                    .session_id(mc_session)
                    .build();
                mc_session += 1;
                sd_socket.send_to(&offer, sd_multicast).await?;
                last_offer = tokio::time::Instant::now();
            }

            let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await
            else {
                continue;
            };

            let Some((_hdr, sd)) = parse_sd_packet(&buf[..len]) else {
                continue;
            };

            if sd.subscribe_entries().any(|e| e.service_id == SERVER_SVC_ID) {
                tracing::info!("Wire server: SubscribeEventgroup from {from}");
                break from;
            }
        };

        // Acknowledge the subscription.
        let ack = SdSubscribeAckBuilder::new(SERVER_SVC_ID, 0x0001, EVENTGROUP_ID)
            .major_version(MAJOR_VERSION)
            .ttl(3000)
            .session_id(uc_session)
            .build();
        uc_session += 1;
        sd_socket.send_to(&ack, subscribe_from).await?;

        // Give B time to process the ACK and finalise the subscription.
        tokio::time::sleep(Duration::from_millis(300)).await;

        // A now connects to B's TCP server — an *incoming* connection on B's server socket.
        // B receives this on its `tcp_rpc_rx` path (server side), NOT on `tcp_client_rx`.
        let b_server_addr: std::net::SocketAddr =
            std::net::SocketAddr::from((client_ip, CLIENT_TCP_PORT));
        let mut stream_to_b = turmoil::net::TcpStream::connect(b_server_addr).await?;
        tracing::info!("Wire server: connected to B's TCP server at {b_server_addr}");

        // Send a Notification for SERVER_SVC_ID via B's *server* connection.
        // Events must only be delivered via the connection B established to A;
        // this reverse connection must not route the notification to B's subscription.
        let notification = SomeIpPacketBuilder::notification(SERVER_SVC_ID, EVENT_ID)
            .client_id(0x0000)
            .session_id(0x0001)
            .payload(b"sneaky_notification_via_b_server")
            .build();
        stream_to_b.write_all(&notification).await?;
        tracing::info!("Wire server: sent Notification for SERVER_SVC_ID to B's server socket");

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        // Offer CLIENT_SVC_ID over TCP — this is the server A will connect to.
        let _offering = runtime
            .offer(CLIENT_SVC_ID, InstanceId::Id(1))
            .version(MAJOR_VERSION, 0)
            .tcp_port(CLIENT_TCP_PORT)
            .start()
            .await
            .expect("offer B's TCP service");

        // Subscribe to A's SERVER_SVC_ID via TCP.
        // B opens a TCP connection to A's TCP listener (A_IP:SERVER_TCP_PORT)
        // before sending SubscribeEventgroup, per feat_req_someipsd_767.
        let proxy = tokio::time::timeout(Duration::from_secs(5), runtime.find(SERVER_SVC_ID))
            .await
            .expect("find must not timeout")
            .expect("service must be found");

        let mut subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(EventgroupId::new(EVENTGROUP_ID).unwrap()),
        )
        .await
        .expect("subscribe must not timeout")
        .expect("subscribe must succeed");

        // Wait for A to send its sneaky notification via B's server socket.
        // If it were incorrectly delivered, subscription.next() would resolve within the timeout.
        let result = tokio::time::timeout(Duration::from_secs(4), subscription.next()).await;
        assert!(
            result.is_err(),
            "A Notification sent via B's TCP server connection must NOT be delivered to B's subscription"
        );

        Ok(())
    });

    sim.run().unwrap();
}
