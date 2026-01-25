//! RPC Wire Format Tests
//!
//! These tests verify that SOME/IP RPC messages have the correct byte-level
//! encoding as specified by the SOME/IP protocol specification.
//!
//! Tests cover:
//! - REQUEST (0x00) message type encoding
//! - RESPONSE (0x80) message type encoding
//! - REQUEST_NO_RETURN (0x01) for fire-and-forget
//! - Header size (16 bytes)
//! - Big-endian encoding
//! - Request ID preservation in responses
//! - Session ID increment behavior

use super::helpers::*;
use crate::client_behavior::helpers::build_sd_offer_with_session;

/// feat_req_someip_103: REQUEST (0x00) message type on wire
/// feat_req_someip_60: Message ID = Service ID || Method ID
/// feat_req_someip_45: Header is exactly 16 bytes
#[test_log::test]
fn rpc_request_wire_format() {
    covers!(
        feat_req_someip_103,
        feat_req_someip_60,
        feat_req_someip_45
    );

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Raw socket acts as server - sends SD offer, then receives request bytes
    sim.host("raw_server", || async {
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_server").to_string().parse().unwrap();

        // RPC socket to receive requests
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?;
        eprintln!("Raw server RPC listening on {}", rpc_socket.local_addr()?);

        // SD socket to send offers
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        // Send SD offer to multicast so the library can discover us
        let offer = build_sd_offer(0x1234, 0x0001, 1, 0, my_ip, 30509, 3600);
        let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // Send offers periodically until we receive a request
        let mut buf = [0u8; 1500];
        let mut request_received = false;

        for _ in 0..20 {
            // Send an offer
            sd_socket.send_to(&offer, sd_multicast).await?;
            eprintln!("Raw server sent SD offer");

            // Check for RPC request (non-blocking with short timeout)
            let result =
                tokio::time::timeout(Duration::from_millis(200), rpc_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                eprintln!("Raw server received {} bytes from {}", len, from);
                request_received = true;

                // Verify raw bytes directly
                let data = &buf[..len];

                // Header must be at least 16 bytes (feat_req_someip_45)
                assert!(
                    len >= 16,
                    "SOME/IP packet must be at least 16 bytes (header), got {}",
                    len
                );

                // Parse and verify header
                let header = parse_header(data).expect("Should parse as SOME/IP header");

                // Verify Service ID (feat_req_someip_60)
                assert_eq!(header.service_id, 0x1234, "Service ID should be 0x1234");

                // Verify Method ID (feat_req_someip_60)
                assert_eq!(header.method_id, 0x0001, "Method ID should be 0x0001");

                // Verify REQUEST message type (feat_req_someip_103)
                assert_eq!(
                    header.message_type as u8, 0x00,
                    "REQUEST message type must be 0x00"
                );

                // Verify big-endian encoding directly on wire
                assert_eq!(
                    u16::from_be_bytes([data[0], data[1]]),
                    0x1234,
                    "Service ID must be big-endian on wire"
                );
                assert_eq!(
                    u16::from_be_bytes([data[2], data[3]]),
                    0x0001,
                    "Method ID must be big-endian on wire"
                );

                // Send a response back
                let response = build_response(&header, b"ok");
                rpc_socket.send_to(&response, from).await?;
                break;
            }
        }

        assert!(request_received, "Should have received an RPC request");
        Ok(())
    });

    // Library side - discovers service via SD and makes RPC call using public API
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Use public API: find service and wait for availability via SD
        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));

        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Should discover service via SD")
            .expect("Service available");

        // Make an RPC call using public API
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            proxy.call(MethodId::new(0x0001).unwrap(), b"hello"),
        )
        .await;

        assert!(result.is_ok(), "RPC call should complete");
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_103: RESPONSE (0x80) message type on wire
/// feat_req_someip_711: Response preserves Request ID
#[test_log::test]
fn rpc_response_wire_format() {
    covers!(feat_req_someip_103, feat_req_someip_711);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library side - server that responds
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

        // Handle one call
        if let Some(event) = offering.next().await {
            if let ServiceEvent::Call { responder, .. } = event {
                responder.reply(b"response_payload").unwrap();
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    // Raw socket sends request and captures response
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // First, listen for SD to find the server
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        // Find server endpoint from SD offer
        let mut server_endpoint: Option<SocketAddr> = None;
        let mut buf = [0u8; 1500];

        for _ in 0..10 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 {
                            // Found our service offer
                            // Use the source address with the offered port
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
        eprintln!("Found server at {}", server_addr);

        // Now send a raw request
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        let request = build_request(0x1234, 0x0001, 0xABCD, 0x1234, b"test_payload");
        rpc_socket.send_to(&request, server_addr).await?;

        // Receive response
        let (len, _from) =
            tokio::time::timeout(Duration::from_secs(3), rpc_socket.recv_from(&mut buf)).await??;

        let response_data = &buf[..len];
        let response_header = parse_header(response_data).expect("Should parse response");

        // Verify RESPONSE message type (feat_req_someip_103)
        assert_eq!(
            response_header.message_type as u8, 0x80,
            "RESPONSE message type must be 0x80"
        );

        // Verify Request ID preservation (feat_req_someip_711)
        assert_eq!(
            response_header.client_id, 0xABCD,
            "Response must preserve Client ID from request"
        );
        assert_eq!(
            response_header.session_id, 0x1234,
            "Response must preserve Session ID from request"
        );

        // Verify Service/Method preserved
        assert_eq!(response_header.service_id, 0x1234);
        assert_eq!(response_header.method_id, 0x0001);

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_103: REQUEST_NO_RETURN (0x01) message type on wire
/// feat_req_someip_284: Fire-and-forget uses REQUEST_NO_RETURN
///
/// Verifies that when the library sends a fire-and-forget message,
/// the raw bytes on the wire contain message type 0x01 (REQUEST_NO_RETURN).
#[test_log::test]
fn fire_and_forget_wire_format() {
    covers!(feat_req_someip_103, feat_req_someip_284);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Raw socket acts as server - receives fire-and-forget and checks wire bytes
    sim.host("raw_server", || async {
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_server").to_string().parse().unwrap();

        // RPC socket to receive requests
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?;
        eprintln!("Raw server RPC listening on {}", rpc_socket.local_addr()?);

        // SD socket to send offers
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        // Send SD offer to multicast so the library can discover us
        let offer = build_sd_offer(0x1234, 0x0001, 1, 0, my_ip, 30509, 3600);
        let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // Send offers periodically until we receive a request
        let mut buf = [0u8; 1500];
        let mut request_received = false;

        for _ in 0..20 {
            // Send an offer
            sd_socket.send_to(&offer, sd_multicast).await?;
            eprintln!("Raw server sent SD offer");

            // Check for RPC request (non-blocking with short timeout)
            let result =
                tokio::time::timeout(Duration::from_millis(200), rpc_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                eprintln!("Raw server received {} bytes from {}", len, from);
                request_received = true;

                // Verify raw bytes directly
                let data = &buf[..len];

                // Header must be at least 16 bytes
                assert!(
                    len >= 16,
                    "SOME/IP packet must be at least 16 bytes (header), got {}",
                    len
                );

                // Parse and verify header
                let header = parse_header(data).expect("Should parse as SOME/IP header");

                // Verify Service ID
                assert_eq!(header.service_id, 0x1234, "Service ID should be 0x1234");

                // Verify Method ID
                assert_eq!(header.method_id, 0x0001, "Method ID should be 0x0001");

                // Verify REQUEST_NO_RETURN message type (feat_req_someip_103)
                assert_eq!(
                    header.message_type as u8, 0x01,
                    "REQUEST_NO_RETURN message type must be 0x01, got 0x{:02X}",
                    header.message_type as u8
                );

                // Also verify at raw byte level (byte 12 is message type)
                assert_eq!(
                    data[12], 0x01,
                    "Message type byte on wire must be 0x01 (REQUEST_NO_RETURN)"
                );

                // Verify payload
                let payload = &data[16..];
                assert_eq!(payload, b"fire_and_forget_payload");

                // No response sent for fire-and-forget
                break;
            }
        }

        assert!(
            request_received,
            "Should have received a fire-and-forget request"
        );

        // Keep alive briefly
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    // Library side - discovers service via SD and sends fire-and-forget
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Use public API: find service and wait for availability via SD
        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));

        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Should discover service via SD")
            .expect("Service available");

        // Send fire-and-forget using public API
        proxy
            .fire_and_forget(MethodId::new(0x0001).unwrap(), b"fire_and_forget_payload")
            .await
            .expect("Fire-and-forget should succeed");

        // Give time for the message to be sent
        tokio::time::sleep(Duration::from_millis(100)).await;

        Ok(())
    });

    sim.run().unwrap();
}

/// Verify that a raw REQUEST_NO_RETURN packet is correctly handled by the library.
/// Server receives fire-and-forget from raw socket and dispatches as FireForget event.
#[test_log::test]
fn fire_and_forget_received_wire_format() {
    covers!(feat_req_someip_103, feat_req_someip_284);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library side - server that receives fire-and-forget
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

        // Handle one fire-and-forget
        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::FireForget {
                    payload, method, ..
                } => {
                    assert_eq!(method.value(), 0x0001);
                    assert_eq!(payload.as_ref(), b"raw_fire_forget");
                }
                other => panic!("Expected FireForget, got {:?}", other),
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    // Raw socket sends REQUEST_NO_RETURN
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // First, listen for SD to find the server
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        // Find server endpoint from SD offer
        let mut server_endpoint: Option<SocketAddr> = None;
        let mut buf = [0u8; 1500];

        for _ in 0..10 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 {
                            // Found our service offer
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
        eprintln!("Found server at {}", server_addr);

        // Now send a raw fire-and-forget request
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        let request =
            build_fire_and_forget_request(0x1234, 0x0001, 0xABCD, 0x1234, b"raw_fire_forget");
        rpc_socket.send_to(&request, server_addr).await?;

        // No response expected - just wait a bit
        tokio::time::sleep(Duration::from_millis(200)).await;

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_45: SOME/IP header is exactly 16 bytes
/// feat_req_someip_42: Big-endian encoding verification
#[test_log::test]
fn header_size_and_endianness_on_wire() {
    covers!(feat_req_someip_45, feat_req_someip_42);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library side - offers a service (sends SD)
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

        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(())
    });

    // Raw socket verifies byte-level structure
    sim.client("raw_observer", async move {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];

        let (len, _from) =
            tokio::time::timeout(Duration::from_millis(500), socket.recv_from(&mut buf)).await??;

        let data = &buf[..len];

        // All SOME/IP packets must be at least 16 bytes (feat_req_someip_45)
        assert!(
            len >= 16,
            "SOME/IP packet must be at least 16 bytes (header), got {}",
            len
        );

        // Verify big-endian encoding (feat_req_someip_42):
        // SD Service ID = 0xFFFF in bytes [0..2]
        let service_id = u16::from_be_bytes([data[0], data[1]]);
        assert_eq!(service_id, 0xFFFF, "SD Service ID must be 0xFFFF");

        // SD Method ID = 0x8100 in bytes [2..4]
        let method_id = u16::from_be_bytes([data[2], data[3]]);
        assert_eq!(method_id, 0x8100, "SD Method ID must be 0x8100");

        // Length field in bytes [4..8]
        let length = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        assert!(
            length >= 8,
            "Length field must be >= 8 (remaining header), got {}",
            length
        );

        // Protocol Version in byte [12]
        assert_eq!(data[12], 0x01, "Protocol Version must be 0x01");

        // Interface Version in byte [13]
        assert_eq!(data[13], 0x01, "Interface Version must be 0x01 for SD");

        // Message Type in byte [14] - NOTIFICATION = 0x02
        assert_eq!(
            data[14], 0x02,
            "SD Message Type must be NOTIFICATION (0x02)"
        );

        // Return Code in byte [15] - E_OK = 0x00
        assert_eq!(data[15], 0x00, "Return Code must be E_OK (0x00)");

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someip_677: Session ID increments across calls
/// feat_req_someip_649: Session ID starts at non-zero value
#[test_log::test]
fn session_id_increment_on_wire() {
    covers!(feat_req_someip_677, feat_req_someip_649);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Raw socket acts as server - sends SD offer, receives multiple requests
    sim.host("raw_server", || async {
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_server").to_string().parse().unwrap();

        // RPC socket to receive requests
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?;

        // SD socket to send offers
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;
        let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

        let mut buf = [0u8; 1500];
        let mut session_ids = Vec::new();
        let mut next_multicast_session_id = 1u16;

        // Send offers and receive requests
        for _ in 0..30 {
            // Send an offer periodically
            let offer = build_sd_offer_with_session(
                0x1234,
                0x0001,
                1,
                0,
                my_ip,
                30509,
                3600,
                next_multicast_session_id,
                true,
                false,
            );
            next_multicast_session_id += 1;
            sd_socket.send_to(&offer, sd_multicast).await?;

            // Check for RPC request
            let result =
                tokio::time::timeout(Duration::from_millis(200), rpc_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                let header = parse_header(&buf[..len]).expect("Should parse header");
                session_ids.push(header.session_id);
                eprintln!(
                    "Request {}: session_id = {}",
                    session_ids.len(),
                    header.session_id
                );

                // Send response
                let response = build_response(&header, b"ok");
                rpc_socket.send_to(&response, from).await?;

                // Stop after 3 requests
                if session_ids.len() >= 3 {
                    break;
                }
            }
        }

        // Verify session IDs
        assert!(
            session_ids.len() >= 3,
            "Should have received 3 requests, got {}",
            session_ids.len()
        );

        // Verify non-zero (feat_req_someip_649)
        for sid in &session_ids {
            assert!(*sid > 0, "Session ID must be non-zero");
        }

        // Verify incrementing (feat_req_someip_677)
        for i in 1..session_ids.len() {
            let prev = session_ids[i - 1];
            let curr = session_ids[i];
            assert!(
                curr > prev || (prev == 0xFFFF && curr == 0x0001),
                "Session ID must increment (got {} after {})",
                curr,
                prev
            );
        }

        Ok(())
    });

    // Library side - discovers service via SD and makes multiple calls using public API
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Use public API: find service and wait for availability via SD
        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));

        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Should discover service via SD")
            .expect("Service available");

        // Make 3 calls using public API
        for _ in 0..3 {
            let _ = tokio::time::timeout(
                Duration::from_secs(3),
                proxy.call(MethodId::new(0x0001).unwrap(), b"test"),
            )
            .await
            .expect("Timeout")
            .expect("Call should succeed");
        }

        Ok(())
    });

    sim.run().unwrap();
}
