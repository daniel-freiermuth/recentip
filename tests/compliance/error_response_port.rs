//! Tests for Error Response Port Correctness
//!
//! These tests verify that error responses (E_UNKNOWN_SERVICE, etc.) are sent
//! from the correct server port, not from a client ephemeral port.
//!
//! ## Bug Description
//!
//! The server incorrectly uses `Action::SendClientMessage` for certain error
//! responses, which sends from the client RPC socket (ephemeral port) instead
//! of the server socket that received the request.
//!
//! ### Affected Scenarios
//!
//! 1. **Unknown Service**: Request to a service ID that's not offered
//! 2. **Misrouted Service**: Request to a valid service on the wrong port
//!
//! ### Expected Behavior
//!
//! Per SOME/IP client-server model, responses MUST come from the same endpoint
//! (IP:port) that the client sent the request to. Otherwise:
//! - UDP: Client will receive response from unexpected port, may discard
//! - TCP: Server will try to connect to client's ephemeral port (fails)
//!
//! ### Requirements
//!
//! - Response source port MUST match request destination port
//! - Applies to both UDP and TCP transports
//! - Applies to all error responses, not just application errors

use bytes::{BufMut, BytesMut};
use recentip::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use crate::helpers::DEFAULT_SD_MULTICAST;

const TEST_SERVICE_ID: u16 = 0x1234;
const TEST_SERVICE_VERSION: (u8, u32) = (1, 0);

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

/// Helper to parse SOME/IP header from wire format
fn parse_header_wire(data: &[u8]) -> Option<recentip::wire::Header> {
    if data.len() < 16 {
        return None;
    }
    let mut buf = &data[..];
    recentip::wire::Header::parse(&mut buf)
}

/// Helper to parse SD message
fn parse_sd_message(data: &[u8]) -> Option<(recentip::wire::Header, recentip::wire::SdMessage)> {
    use bytes::Buf;
    use recentip::wire::SdMessage;
    let mut buf = &data[..];
    let header = recentip::wire::Header::parse(&mut buf)?;
    let sd_msg = SdMessage::parse(&mut buf)?;
    Some((header, sd_msg))
}

// ============================================================================
// UDP Tests
// ============================================================================

/// Test that E_UNKNOWN_SERVICE response comes from server port (UDP)
///
/// When a client sends a request to a non-existent service, the error
/// response MUST come from the port the request was sent to, not from
/// a client ephemeral port.
#[test_log::test]
#[cfg(feature = "turmoil")]
fn unknown_service_error_uses_server_port_udp() {
    covers!(feat_req_someip_816);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service 0x1234 on a specific port
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

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Discover server via SD to get its actual port
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket
            .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // TODO: do we also test that all errors come from the correct requested service?

        // TODO: commit per test

        for _ in 0..20 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == TEST_SERVICE_ID {
                            // Get the option referenced by this entry, not just the first option
                            if let Some(opt) = sd_msg.options.get(entry.index_1st_option as usize) {
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
        let server_port = server_addr.port();
        tracing::info!(
            "Discovered service 0x{:04x} at {} (port {})",
            TEST_SERVICE_ID,
            server_addr,
            server_port
        );

        // Create RPC socket and send request to WRONG service ID
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        // Request header fields for verification
        let req_service_id = 0x9999u16; // WRONG Service ID
        let req_method_id = 0x0001u16;
        let req_client_id = 0x0001u16;
        let req_session_id = 0x0001u16;

        let mut request = BytesMut::with_capacity(24);
        request.put_u16(req_service_id); // WRONG Service ID - should trigger E_UNKNOWN_SERVICE
        request.put_u16(req_method_id); // Method ID
        request.put_u32(0x00000010); // Length (16 bytes = 8 header + 8 payload)
        request.put_u16(req_client_id); // Client ID
        request.put_u16(req_session_id); // Session ID
        request.put_u8(0x01); // Protocol version
        request.put_u8(0x01); // Interface version
        request.put_u8(0x00); // Message type: REQUEST
        request.put_u8(0x00); // Return code
        request.put_slice(b"testdata");

        rpc_socket.send_to(&request, server_addr).await?;
        tracing::info!("Sent request for unknown service 0x9999 to {}", server_addr);

        // Receive error response and check source port
        let result =
            tokio::time::timeout(Duration::from_secs(2), rpc_socket.recv_from(&mut buf)).await;

        match result {
            Ok(Ok((len, response_from))) => {
                tracing::info!("Received error response from {}", response_from);

                // BUG: The response comes from the wrong port!
                // Expected: response_from.port() == server_port
                // Actual: response_from.port() is an ephemeral client port
                assert_eq!(
                    response_from.port(),
                    server_port,
                    "Error response MUST come from the same port the request was sent to. \
                     Expected port {}, got port {}. \
                     This indicates the server is using the client RPC socket instead of the server socket.",
                    server_port,
                    response_from.port()
                );

                // Verify it's actually an error response with correct header fields
                let header = parse_header_wire(&buf[..len]).expect("Should parse response header");
                
                assert_eq!(
                    header.return_code, 0x02,
                    "Should be E_UNKNOWN_SERVICE"
                );
                
                // Verify error response echoes request header fields for correlation
                assert_eq!(
                    header.service_id, req_service_id,
                    "Error response must echo request service_id for client correlation"
                );
                assert_eq!(
                    header.method_id, req_method_id,
                    "Error response must echo request method_id for client correlation"
                );
                assert_eq!(
                    header.client_id, req_client_id,
                    "Error response must echo request client_id for client correlation"
                );
                assert_eq!(
                    header.session_id, req_session_id,
                    "Error response must echo request session_id for client correlation"
                );
            }
            Ok(Err(e)) => panic!("Socket error: {}", e),
            Err(_) => {
                // Timeout - no response received
                // Per feat_req_someip_816, E_UNKNOWN_SERVICE is optional
                tracing::warn!("No error response received (allowed per spec)");
            }
        }

        Ok(())
    });

    sim.run().unwrap();
}

/// Test that error response for misrouted service uses correct port (UDP)
///
/// When a service is offered on multiple ports and a request arrives on
/// the wrong port, the error response MUST come from the port that received
/// the request, not from the service's primary port.
#[test_log::test]
#[cfg(feature = "turmoil")]
fn misrouted_service_error_uses_receiving_port_udp() {
    covers!(feat_req_someip_816);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers TWO different services on TWO different ports
    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        // Service A on explicit port 30491
        let _offering_a = runtime
            .offer(0x1111, InstanceId::Id(0x0001))
            .version(1, 0)
            .udp_port(30491)
            .start()
            .await
            .unwrap();

        // Service B on explicit port 30492 (different from A)
        let _offering_b = runtime
            .offer(0x2222, InstanceId::Id(0x0001))
            .version(1, 0)
            .udp_port(30492)
            .start()
            .await
            .unwrap();

        // Keep server alive
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Discover both services via SD
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket
            .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut service_a_endpoint: Option<SocketAddr> = None;
        let mut service_b_endpoint: Option<SocketAddr> = None;

        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 {
                            // Get the option referenced by this entry, not just the first option
                            if let Some(opt) = sd_msg.options.get(entry.index_1st_option as usize) {
                                if let recentip::wire::SdOption::Ipv4Endpoint {
                                    addr, port, ..
                                } = opt
                                {
                                    let ip = if addr.is_unspecified() {
                                        from.ip()
                                    } else {
                                        std::net::IpAddr::V4(*addr)
                                    };
                                    let endpoint = SocketAddr::new(ip, *port);

                                    if entry.service_id == 0x1111 {
                                        service_a_endpoint = Some(endpoint);
                                    } else if entry.service_id == 0x2222 {
                                        service_b_endpoint = Some(endpoint);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if service_a_endpoint.is_some() && service_b_endpoint.is_some() {
                break;
            }
        }

        let service_a_addr = service_a_endpoint.expect("Should find service A");
        let service_b_addr = service_b_endpoint.expect("Should find service B");

        tracing::info!("Service A (0x1111) at {}", service_a_addr);
        tracing::info!("Service B (0x2222) at {}", service_b_addr);

        // Verify services are on different ports as configured
        assert_ne!(
            service_a_addr.port(),
            service_b_addr.port(),
            "Services should be on different ports (30491 and 30492)"
        );

        // Send request for Service A to Service B's port
        // This should trigger E_UNKNOWN_SERVICE from Service B's port
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        // Request header fields for verification
        let req_service_id = 0x1111u16; // Service A's ID
        let req_method_id = 0x0001u16;
        let req_client_id = 0x0001u16;
        let req_session_id = 0x0001u16;

        let mut request = BytesMut::with_capacity(24);
        request.put_u16(req_service_id); // Service A's ID
        request.put_u16(req_method_id); // Method ID
        request.put_u32(0x00000010); // Length
        request.put_u16(req_client_id); // Client ID
        request.put_u16(req_session_id); // Session ID
        request.put_u8(0x01); // Protocol version
        request.put_u8(0x01); // Interface version
        request.put_u8(0x00); // Message type: REQUEST
        request.put_u8(0x00); // Return code
        request.put_slice(b"testdata");

        // Send to Service B's port but ask for Service A
        rpc_socket.send_to(&request, service_b_addr).await?;
        tracing::info!(
            "Sent request for service 0x1111 to wrong port {}",
            service_b_addr
        );

        // Receive error response and check source port
        let result =
            tokio::time::timeout(Duration::from_secs(2), rpc_socket.recv_from(&mut buf)).await;

        match result {
            Ok(Ok((len, response_from))) => {
                tracing::info!("Received error response from {}", response_from);

                // BUG: The response may come from Service A's port or a client port!
                // Expected: response_from.port() == service_b_addr.port() (the port we sent to)
                // Actual: Could be service_a_addr.port() or an ephemeral port
                assert_eq!(
                    response_from.port(),
                    service_b_addr.port(),
                    "Error response MUST come from the same port the request was sent to. \
                     Sent to port {}, but response came from port {}. \
                     This indicates the server is either using a different service's port \
                     or using the client RPC socket.",
                    service_b_addr.port(),
                    response_from.port()
                );

                // Verify it's an error response with correct header fields
                let header = parse_header_wire(&buf[..len]).expect("Should parse response header");
                
                assert_eq!(
                    header.return_code, 0x02,
                    "Should be E_UNKNOWN_SERVICE"
                );
                
                // Verify error response echoes request header fields for correlation
                assert_eq!(
                    header.service_id, req_service_id,
                    "Error response must echo request service_id for client correlation"
                );
                assert_eq!(
                    header.method_id, req_method_id,
                    "Error response must echo request method_id for client correlation"
                );
                assert_eq!(
                    header.client_id, req_client_id,
                    "Error response must echo request client_id for client correlation"
                );
                assert_eq!(
                    header.session_id, req_session_id,
                    "Error response must echo request session_id for client correlation"
                );
            }
            Ok(Err(e)) => panic!("Socket error: {}", e),
            Err(_) => {
                tracing::warn!("No error response received (allowed per spec)");
            }
        }

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// TCP Tests
// ============================================================================

/// Test that E_UNKNOWN_SERVICE response comes from server port (TCP)
///
/// For TCP, the bug is even more serious: if the server tries to use the
/// client RPC socket, it will attempt to establish a NEW TCP connection
/// to the client's ephemeral port, which will typically fail.
#[test_log::test]
#[cfg(feature = "turmoil")]
fn unknown_service_error_uses_server_port_tcp() {
    covers!(feat_req_someip_816, feat_req_someip_644);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service on TCP
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
            .tcp()
            .start()
            .await
            .unwrap();

        // Keep server alive
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Discover server via SD to get TCP endpoint
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket
            .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_tcp_endpoint: Option<SocketAddr> = None;

        for _ in 0..20 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == TEST_SERVICE_ID {
                            // Look for TCP endpoint option
                            for opt in &sd_msg.options {
                                if let recentip::wire::SdOption::Ipv4Endpoint {
                                    addr,
                                    port,
                                    protocol,
                                } = opt
                                {
                                    if *protocol == recentip::wire::L4Protocol::Tcp {
                                        // TCP
                                        let ip = if addr.is_unspecified() {
                                            from.ip()
                                        } else {
                                            std::net::IpAddr::V4(*addr)
                                        };
                                        server_tcp_endpoint = Some(SocketAddr::new(ip, *port));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if server_tcp_endpoint.is_some() {
                break;
            }
        }

        let server_addr = server_tcp_endpoint.expect("Should find server TCP endpoint via SD");
        let server_port = server_addr.port();
        tracing::info!(
            "Discovered TCP service 0x{:04x} at {} (port {})",
            TEST_SERVICE_ID,
            server_addr,
            server_port
        );

        // Connect to server via TCP
        let mut tcp_stream = turmoil::net::TcpStream::connect(server_addr).await?;
        let local_addr = tcp_stream.local_addr()?;
        tracing::info!("Connected from {} to {}", local_addr, server_addr);

        // Request header fields for verification
        let req_service_id = 0x9999u16; // WRONG Service ID
        let req_method_id = 0x0001u16;
        let req_client_id = 0x0001u16;
        let req_session_id = 0x0001u16;

        // Send request for WRONG service ID
        let mut request = BytesMut::with_capacity(24);
        request.put_u16(req_service_id); // WRONG Service ID
        request.put_u16(req_method_id); // Method ID
        request.put_u32(0x00000010); // Length
        request.put_u16(req_client_id); // Client ID
        request.put_u16(req_session_id); // Session ID
        request.put_u8(0x01); // Protocol version
        request.put_u8(0x01); // Interface version
        request.put_u8(0x00); // Message type: REQUEST
        request.put_u8(0x00); // Return code
        request.put_slice(b"testdata");

        use tokio::io::AsyncWriteExt;
        tcp_stream.write_all(&request).await?;
        tracing::info!("Sent TCP request for unknown service 0x9999");

        // Try to receive response
        use tokio::io::AsyncReadExt;
        let mut response_buf = [0u8; 1500];
        let result =
            tokio::time::timeout(Duration::from_secs(2), tcp_stream.read(&mut response_buf)).await;

        match result {
            Ok(Ok(0)) => {
                panic!("TCP connection closed without response");
            }
            Ok(Ok(len)) => {
                tracing::info!("Received {} bytes via TCP", len);

                // For TCP, we can't directly check the source port since we're
                // connected. Instead, we verify that the response arrived on
                // the SAME connection (not a new connection attempt).
                // If the server tried to use the client RPC socket, it would:
                // 1. Try to establish a NEW connection to our ephemeral port
                // 2. Fail to connect
                // 3. We'd timeout here
                //
                // The fact that we received data on this connection means the
                // server correctly used its server socket. However, the bug
                // might still cause connection errors logged on the server side.

                let header = parse_header_wire(&response_buf[..len])
                    .expect("Should parse response header");
                
                assert_eq!(
                    header.return_code, 0x02,
                    "Should be E_UNKNOWN_SERVICE"
                );
                
                // Verify error response echoes request header fields for correlation
                assert_eq!(
                    header.service_id, req_service_id,
                    "Error response must echo request service_id for client correlation"
                );
                assert_eq!(
                    header.method_id, req_method_id,
                    "Error response must echo request method_id for client correlation"
                );
                assert_eq!(
                    header.client_id, req_client_id,
                    "Error response must echo request client_id for client correlation"
                );
                assert_eq!(
                    header.session_id, req_session_id,
                    "Error response must echo request session_id for client correlation"
                );

                tracing::info!("✓ Response received on same TCP connection (correct behavior)");
            }
            Ok(Err(e)) => panic!("TCP read error: {}", e),
            Err(_) => {
                panic!(
                    "Timeout waiting for TCP response. \
                     This likely indicates the server tried to establish a NEW TCP connection \
                     to our ephemeral port (using client RPC socket), which failed."
                );
            }
        }

        Ok(())
    });

    sim.run().unwrap();
}
