//! Split Server Compliance Tests
//!
//! Tests for scenarios where the server is split across multiple hosts:
//! - SD (Service Discovery) handled by one host
//! - RPC/Methods handled by a different host
//!
//! This reflects real-world architectures where SD proxies or gateways
//! advertise services on behalf of other ECUs.
//!
//! Test patterns:
//! - Server side: "on the wire" using raw packet builders
//! - Client side: library under test

use recentip::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

// Import test helpers
use crate::client_behavior::helpers::{build_sd_offer_with_session, parse_header, Header, MessageType};
use crate::helpers::configure_tracing;
use crate::wire_format::helpers::SomeIpPacketBuilder;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

const TEST_SERVICE_ID: u16 = 0x1234;
const TEST_SERVICE_VERSION: (u8, u32) = (1, 0);
const TEST_METHOD_ID: u16 = 0x0001;

// ============================================================================
// SPLIT SERVER UDP RPC TEST
// ============================================================================

/// [split_server_udp_rpc] UDP service on split host can be called via RPC
///
/// Scenario:
/// - SD Host: Advertises service and provides endpoint pointing to RPC Host
/// - RPC Host: Handles actual method calls on a different IP
/// - Client: Discovers service via SD Host, calls methods on RPC Host
///
/// This tests that the client correctly extracts the RPC endpoint from
/// the SD offer and routes method calls to the correct host.
#[test]
fn split_server_udp_rpc() {
    covers!(
        feat_req_someip_328, // Request/Response pattern
        feat_req_someip_338, // Response contains same Request ID
        feat_req_someipsd_011 // IPv4 endpoint option
    );
    configure_tracing();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // SD Host: Advertises the service pointing to RPC Host
    sim.host("sd_host", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket
            .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let rpc_host_ip: Ipv4Addr = turmoil::lookup("rpc_host").to_string().parse().unwrap();
        let rpc_port = 40000u16;

        // Send SD Offer pointing to RPC Host
        let offer = build_sd_offer_with_session(
            TEST_SERVICE_ID,
            0x0001, // instance_id
            TEST_SERVICE_VERSION.0,
            TEST_SERVICE_VERSION.1,
            rpc_host_ip, // RPC endpoint IP
            rpc_port,    // RPC endpoint port
            3,           // TTL
            1,           // session_id
            true,        // reboot_flag
            false,       // unicast_flag (multicast offer)
        );

        let multicast_addr: SocketAddr = "239.255.0.1:30490".parse().unwrap();
        sd_socket.send_to(&offer, multicast_addr).await?;
        tracing::info!("SD Host: Sent offer for service {:04X} pointing to {}:{}", TEST_SERVICE_ID, rpc_host_ip, rpc_port);
        Ok(())
    });

    // RPC Host: Handles the actual method calls
    sim.host("rpc_host", || async {
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:40000").await?;
        let mut buf = [0u8; 1500];

        // Wait for incoming RPC request
        let result = tokio::time::timeout(Duration::from_secs(10), rpc_socket.recv_from(&mut buf))
            .await
            .expect("Should receive RPC request")
            .expect("recv_from should succeed");
        tracing::info!("RPC Host: Received RPC request from client");

        let (len, client_addr) = result;
        let request_data = &buf[..len];

        // Parse request header
        let header = parse_header(request_data).expect("Valid SOME/IP header");

        // Validate request
        assert_eq!(header.service_id, TEST_SERVICE_ID);
        assert_eq!(header.method_id, TEST_METHOD_ID);
        assert_eq!(header.message_type, MessageType::Request);
        assert_eq!(header.interface_version, TEST_SERVICE_VERSION.0);

        let request_payload = &request_data[Header::SIZE..];
        assert_eq!(request_payload, b"ping");

        // Build response with same client_id and session_id
        let response = SomeIpPacketBuilder::response(TEST_SERVICE_ID, TEST_METHOD_ID)
            .client_id(header.client_id)
            .session_id(header.session_id)
            .interface_version(TEST_SERVICE_VERSION.0)
            .payload(b"pong")
            .build();

        // Send response back to client
        rpc_socket.send_to(&response, client_addr).await?;
        tracing::info!("RPC Host: Sent response to client");

        // Keep socket alive
        tokio::time::sleep(Duration::from_secs(3)).await;

        Ok(())
    });

    // Client: Uses library to discover and call the service
    sim.client("client", async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Find the service (discovers via SD Host)
        tracing::info!("Client: Starting service discovery");
        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery should complete")
            .expect("Service should be found");

        // Call the method (routes to RPC Host)
        tracing::info!("Client: Calling method on discovered service");
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(TEST_METHOD_ID).unwrap(), b"ping"),
        )
        .await
        .expect("Call should not timeout")
        .expect("Call should succeed");

        // Verify response
        assert_eq!(response.payload.as_ref(), b"pong");
        tracing::info!("Client: Received response from RPC Host");

        Ok(())
    });

    sim.run().unwrap();
}
