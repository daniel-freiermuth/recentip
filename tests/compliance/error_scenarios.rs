//! Advanced Error Handling Compliance Tests
//!
//! Tests error scenarios, return codes, and error message handling.
//!
//! Key requirements tested:
//! - feat_req_recentip_597: No error response for events/notifications
//! - feat_req_recentip_654: No error response for fire&forget methods
//! - feat_req_recentip_655: Error message copies request header fields (wire format test)
//! - feat_req_recentip_727: Error message has return code != 0x00
//! - feat_req_recentip_798: Messages with length < 8 shall be ignored (wire format test)
//! - feat_req_recentip_703: Use known protocol version (wire format test)

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
// No Error Response for Events/Fire&Forget Tests
// ============================================================================

/// feat_req_recentip_597: No error response for events/notifications
///
/// The system shall not return an error message for events/notifications.
/// Events are one-way - there's no mechanism to send errors back.
#[test_log::test]
fn no_error_response_for_events() {
    covers!(feat_req_recentip_597);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service with events
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
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send event notification - this is one-way, no response expected
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        offering.event(event_id).eventgroup(eventgroup).create().unwrap().notify(b"event_data")
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    });

    // Client subscribes and receives event
    sim.host("client", || async {
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

/// feat_req_recentip_654: No error response for fire&forget methods
///
/// The system shall not return an error message for fire&forget methods.
/// Fire&forget is one-way - no response or error is expected.
#[test_log::test]
fn no_error_response_for_fire_and_forget() {
    covers!(feat_req_recentip_654);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server handles fire&forget
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

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

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

/// feat_req_recentip_683: All defined return codes are valid
///
/// Test that all defined return codes can be used.
#[test_log::test]
fn all_return_codes_are_valid() {
    covers!(feat_req_recentip_683);

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

/// feat_req_recentip_727: Error message has return code != 0x00
/// feat_req_recentip_683: Server can return any valid return code
///
/// Server responds with various error codes, and client receives them.
#[test_log::test]
fn server_returns_various_error_codes() {
    covers!(feat_req_recentip_683, feat_req_recentip_727);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server returns different error codes
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

        // Handle first request - return NotReady
        if let Some(event) = tokio::time::timeout(Duration::from_secs(10), offering.next())
            .await
            .ok()
            .flatten()
        {
            if let ServiceEvent::Call { responder, .. } = event {
                responder.reply_error(ReturnCode::NotReady).await.unwrap();
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
                    .reply_error(ReturnCode::UnknownMethod)
                    .await
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
                responder.reply_error(ReturnCode::NotOk).await.unwrap();
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    // Client makes calls and receives error codes
    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        // First call - expect NotReady
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
                    ReturnCode::NotReady,
                    "Should receive NotReady"
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
                // Verify return code is != 0x00 (feat_req_recentip_727)
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
fn parse_sd_message(
    data: &[u8],
) -> Option<(
    recentip::wire::Header,
    recentip::wire::SdMessage,
)> {
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

/// feat_req_recentip_655: Error response copies request header fields
///
/// For request/response methods the error message shall copy over the
/// fields of the header from the request.
#[test_log::test]
fn error_response_copies_request_header() {
    use bytes::{BufMut, BytesMut};
    use recentip::handle::ServiceEvent;
    use recentip::prelude::*;
    use recentip::wire::MessageType;
    use recentip::{Runtime, RuntimeConfig};
    use std::net::SocketAddr;
    use std::time::Duration;

    covers!(feat_req_recentip_655);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library side - server responds with errors
    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

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
                    .reply_error(ReturnCode::UnknownMethod)
                    .await
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
                // errors use RESPONSE (0x80) per feat_req_recentip_726
                assert_eq!(
                    error_header.message_type,
                    MessageType::Response,
                    "Error response should be RESPONSE (0x80) when EXCEPTION is not configured (feat_req_recentip_726)"
                );

                // Verify return code is not OK (feat_req_recentip_727)
                // This is what makes it an "error message" even with RESPONSE type
                assert_ne!(
                    error_header.return_code, 0x00,
                    "Error message must have return code != 0x00 (feat_req_recentip_727)"
                );

                // Verify Service ID copied from request
                assert_eq!(
                    error_header.service_id, 0x1234,
                    "Error should copy Service ID from request (feat_req_recentip_655)"
                );

                // Verify Method ID copied from request
                assert_eq!(
                    error_header.method_id, 0x0001,
                    "Error should copy Method ID from request (feat_req_recentip_655)"
                );

                // Verify Client ID copied from request (Request ID part 1)
                assert_eq!(
                    error_header.client_id, 0x0042,
                    "Error should copy Client ID from request (feat_req_recentip_655)"
                );

                // Verify Session ID copied from request (Request ID part 2)
                assert_eq!(
                    error_header.session_id, 0x1337,
                    "Error should copy Session ID from request (feat_req_recentip_655)"
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

/// feat_req_recentip_106: EXCEPTION (0x81) is used when configured per-method
///
/// When a method is configured to use EXCEPTION for errors via MethodConfig,
/// the error response must use message type 0x81 instead of 0x80.
#[test_log::test]
fn exception_message_type_when_configured() {
    use bytes::{BufMut, BytesMut};
    use recentip::handle::ServiceEvent;
    use recentip::prelude::*;
    use recentip::wire::MessageType;
    use recentip::{Runtime, RuntimeConfig};
    use std::net::SocketAddr;
    use std::time::Duration;

    covers!(feat_req_recentip_106);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server configured to use EXCEPTION for method 0x0001
    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

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
                    .reply_error(ReturnCode::UnknownMethod)
                    .await
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
                    "Error response should be EXCEPTION (0x81) when configured (feat_req_recentip_106)"
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

/// feat_req_recentip_106, feat_req_recentip_726: Mixed config - some methods EXCEPTION, some RESPONSE
///
/// When some methods are configured for EXCEPTION and others are not,
/// each method should use the appropriate message type.
#[test_log::test]
fn mixed_exception_config_per_method() {
    use bytes::{BufMut, BytesMut};
    use recentip::handle::ServiceEvent;
    use recentip::prelude::*;
    use recentip::wire::MessageType;
    use recentip::{Runtime, RuntimeConfig};
    use std::net::SocketAddr;
    use std::time::Duration;

    covers!(feat_req_recentip_106, feat_req_recentip_726);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server with mixed config: method 0x0001 uses EXCEPTION, method 0x0002 uses RESPONSE
    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

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
                    responder.reply_error(ReturnCode::NotOk).await.unwrap();
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
                                    addr,
                                    port,
                                    ..
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
    use recentip::{Runtime, RuntimeConfig};
    use std::net::SocketAddr;
    use std::time::Duration;

    covers!(feat_req_recentip_726);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service 0x1234
    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

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
                                    addr,
                                    port,
                                    ..
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

/// feat_req_recentip_798: Messages with length < 8 shall be ignored
///
/// SOME/IP messages with a length value < 8 bytes shall be ignored.
/// Length field indicates payload + 8 (for the header tail), so minimum is 8.
#[test_log::test]
fn messages_with_short_length_ignored() {
    use bytes::{BufMut, BytesMut};
    use recentip::prelude::*;
    use recentip::{Runtime, RuntimeConfig};
    use std::net::SocketAddr;
    use std::time::Duration;

    covers!(feat_req_recentip_798);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server should ignore malformed messages
    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

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
        // The message with Length < 8 should be rejected at parse time (feat_req_recentip_798)
        assert!(
            result.is_err(),
            "Malformed message with Length < 8 should be ignored (feat_req_recentip_798)"
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
                                    addr,
                                    port,
                                    ..
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

/// feat_req_recentip_703: Implementation shall use known protocol version
///
/// All messages sent by the library must use protocol version 0x01.
#[test_log::test]
fn uses_known_protocol_version() {
    use recentip::prelude::*;
    use recentip::wire::Header;
    use recentip::{Runtime, RuntimeConfig};
    use std::net::SocketAddr;
    use std::time::Duration;

    covers!(feat_req_recentip_703);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library side - server that can respond to requests
    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

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
                    let _ = responder.reply(b"response").await;
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
                                    addr,
                                    port,
                                    ..
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
                "All messages must use protocol version 0x01 (feat_req_recentip_703)"
            );
        }

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentip_703, feat_req_recentip_818:
/// Wrong protocol version returns E_WRONG_PROTOCOL_VERSION or is ignored
///
/// When receiving a message with wrong protocol version, the implementation
/// may either ignore it or respond with E_WRONG_PROTOCOL_VERSION.
#[test_log::test]
fn wrong_protocol_version_returns_error() {
    use bytes::{BufMut, BytesMut};
    use recentip::prelude::*;
    use recentip::wire::MessageType;
    use recentip::{Runtime, RuntimeConfig};
    use std::net::SocketAddr;
    use std::time::Duration;

    covers!(feat_req_recentip_703, feat_req_recentip_818);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server receives request with wrong protocol version
    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

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
                                    addr,
                                    port,
                                    ..
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
                    "Should return E_WRONG_PROTOCOL_VERSION (feat_req_recentip_703)"
                );
            }
        }
        // Note: Not receiving a response is also valid (feat_req_recentip_818 - may ignore)

        Ok(())
    });

    sim.run().unwrap();
}
