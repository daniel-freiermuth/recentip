//! Version Handling Compliance Tests
//!
//! Integration tests for SOME/IP protocol version and interface version handling.
//! Unit tests and property tests have been moved to src/wire.rs.
//!
//! # Protocol Version (feat_req_someip_300)
//! - Byte offset 12 in header
//! - Current version is 0x01
//! - Mismatch should be rejected
//!
//! # Interface Version (feat_req_someip_278)
//! - Byte offset 13 in header  
//! - Configured per service
//! - Client/server must match major version

mod wire_format;

use bytes::Bytes;
use recentip::handle::ServiceEvent;
use recentip::prelude::*;
use recentip::wire::{
    Header, SdMessage, INTERFACE_VERSION_OFFSET, PROTOCOL_VERSION, PROTOCOL_VERSION_OFFSET,
    SD_METHOD_ID, SD_SERVICE_ID,
};
use std::net::SocketAddr;
use std::time::Duration;
use wire_format::helpers::SomeIpPacketBuilder;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

const TEST_SERVICE_ID: u16 = 0x1234;
const TEST_SERVICE_VERSION: (u8, u32) = (1, 0);

/// Helper to parse a SOME/IP header from raw bytes
fn parse_header(data: &[u8]) -> Option<Header> {
    if data.len() < Header::SIZE {
        return None;
    }
    Header::parse(&mut Bytes::copy_from_slice(data))
}

/// Helper to parse an SD message from raw bytes
fn parse_sd_message(data: &[u8]) -> Option<(Header, SdMessage)> {
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

/// Build SD offer with specific major version
fn build_sd_offer_with_version(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    minor_version: u32,
    endpoint_ip: std::net::Ipv4Addr,
    endpoint_port: u16,
    ttl: u32,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(60);

    // SOME/IP Header
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes()); // Service ID (SD)
    packet.extend_from_slice(&0x8100u16.to_be_bytes()); // Method ID (SD)
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes()); // Length placeholder
    packet.extend_from_slice(&0x0000u16.to_be_bytes()); // Client ID
    packet.extend_from_slice(&0x0001u16.to_be_bytes()); // Session ID
    packet.push(0x01); // Protocol version
    packet.push(0x01); // Interface version
    packet.push(0x02); // Message type: NOTIFICATION
    packet.push(0x00); // Return code

    // SD Payload
    packet.push(0xC0); // Flags: Unicast + Reboot
    packet.extend_from_slice(&[0x00, 0x00, 0x00]); // Reserved

    // Entries array length (16 bytes for one entry)
    packet.extend_from_slice(&16u32.to_be_bytes());

    // OfferService Entry (16 bytes)
    packet.push(0x01); // Type: OfferService
    packet.push(0x00); // Index 1st options (where first option starts)
    packet.push(0x00); // Index 2nd options (no second option run)
    packet.push(0x10); // (num_options_1 << 4) | num_options_2 = (1 << 4) | 0
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    packet.push((ttl >> 16) as u8);
    packet.push((ttl >> 8) as u8);
    packet.push(ttl as u8);
    packet.extend_from_slice(&minor_version.to_be_bytes());

    // Options array length (12 bytes for IPv4 endpoint)
    packet.extend_from_slice(&12u32.to_be_bytes());

    // IPv4 Endpoint Option
    packet.extend_from_slice(&9u16.to_be_bytes()); // Length
    packet.push(0x04); // Type: IPv4 Endpoint
    packet.push(0x00); // Reserved
    packet.extend_from_slice(&endpoint_ip.octets());
    packet.push(0x00); // Reserved
    packet.push(0x11); // Protocol: UDP
    packet.extend_from_slice(&endpoint_port.to_be_bytes());

    // Fix length field
    let payload_len = (packet.len() - 12) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&payload_len.to_be_bytes());

    packet
}

// ============================================================================
// PROTOCOL VERSION WIRE TESTS
// ============================================================================

/// [feat_req_someip_300] RPC request contains protocol version 0x01 on wire
#[test_log::test]
fn rpc_request_has_protocol_version_0x01() {
    covers!(feat_req_someip_300);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Raw server captures request bytes
    sim.host("raw_server", || async {
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_server").to_string().parse().unwrap();

        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?;
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        let offer = build_sd_offer_with_version(0x1234, 0x0001, 1, 0, my_ip, 30509, 3600);
        let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

        let mut buf = [0u8; 1500];

        for _ in 0..20 {
            sd_socket.send_to(&offer, sd_multicast).await?;

            let result =
                tokio::time::timeout(Duration::from_millis(200), rpc_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                let data = &buf[..len];

                // Verify protocol version at offset 12
                assert_eq!(
                    data[PROTOCOL_VERSION_OFFSET], PROTOCOL_VERSION,
                    "Protocol version must be 0x01 at offset 12"
                );

                let header = parse_header(data).expect("Valid header");
                assert_eq!(header.protocol_version, 0x01);

                // Send response
                let response = SomeIpPacketBuilder::response(0x1234, 0x0001)
                    .client_id(header.client_id)
                    .session_id(header.session_id)
                    .payload(b"ok")
                    .build();
                rpc_socket.send_to(&response, _from).await?;
                break;
            }
        }

        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();
        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));

        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let _ = tokio::time::timeout(
            Duration::from_secs(3),
            proxy.call(MethodId::new(0x0001).unwrap(), b"test"),
        )
        .await;

        Ok(())
    });

    sim.run().unwrap();
}

/// [feat_req_someip_300] Server ignores messages with wrong protocol version
///
/// Note: This test verifies that the runtime should reject messages with
/// protocol version != 0x01. The runtime validates protocol version and drops invalid messages.
#[test_log::test]
fn server_ignores_wrong_protocol_version() {
    covers!(feat_req_someip_300);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library server
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

        // Wait for valid request (should timeout since we only send invalid ones)
        let result = tokio::time::timeout(Duration::from_secs(3), offering.next()).await;

        // Should timeout - no valid requests received
        assert!(
            result.is_err(),
            "Server should not receive requests with wrong protocol version"
        );

        Ok(())
    });

    // Raw client sends request with wrong protocol version
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

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

        // Send request with WRONG protocol version (0x02)
        let bad_request = SomeIpPacketBuilder::request(0x1234, 0x0001)
            .client_id(0xABCD)
            .session_id(0x0001)
            .protocol_version(0x02) // Wrong!
            .payload(b"bad_request")
            .build();
        rpc_socket.send_to(&bad_request, server_addr).await?;

        // Also send with version 0x00
        let bad_request_v0 = SomeIpPacketBuilder::request(0x1234, 0x0001)
            .client_id(0xABCD)
            .session_id(0x0002)
            .protocol_version(0x00) // Wrong!
            .payload(b"bad_request")
            .build();
        rpc_socket.send_to(&bad_request_v0, server_addr).await?;

        tokio::time::sleep(Duration::from_millis(300)).await;

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// INTERFACE VERSION WIRE TESTS
// ============================================================================

/// [feat_req_someip_278] RPC request contains interface version at offset 13
#[test_log::test]
fn rpc_request_has_interface_version_at_offset_13() {
    covers!(feat_req_someip_278);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("raw_server", || async {
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_server").to_string().parse().unwrap();

        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?;
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        // Offer with major version 1
        let offer = build_sd_offer_with_version(0x1234, 0x0001, 1, 0, my_ip, 30509, 3600);
        let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

        let mut buf = [0u8; 1500];

        for _ in 0..20 {
            sd_socket.send_to(&offer, sd_multicast).await?;

            let result =
                tokio::time::timeout(Duration::from_millis(200), rpc_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                let data = &buf[..len];

                // Verify interface version at offset 13
                assert_eq!(
                    data[INTERFACE_VERSION_OFFSET], 0x01,
                    "Interface version must be at offset 13"
                );

                let header = parse_header(data).expect("Valid header");
                assert_eq!(header.interface_version, 0x01);

                // Send response
                let response = SomeIpPacketBuilder::response(0x1234, 0x0001)
                    .client_id(header.client_id)
                    .session_id(header.session_id)
                    .payload(b"ok")
                    .build();
                rpc_socket.send_to(&response, from).await?;
                break;
            }
        }

        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();
        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));

        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let _ = tokio::time::timeout(
            Duration::from_secs(3),
            proxy.call(MethodId::new(0x0001).unwrap(), b"test"),
        )
        .await;

        Ok(())
    });

    sim.run().unwrap();
}

/// [feat_req_someip_278] SD offer contains major version in entry
#[test_log::test]
fn sd_offer_contains_major_version() {
    covers!(feat_req_someip_278);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

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

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.client("observer", async move {
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut found_offer = false;

        for _ in 0..20 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 {
                            // Check major version in SD entry
                            assert_eq!(
                                entry.major_version, TEST_SERVICE_VERSION.0,
                                "SD offer should contain service major version"
                            );
                            found_offer = true;
                        }
                    }
                }
            }
            if found_offer {
                break;
            }
        }

        assert!(found_offer, "Should receive SD offer with version info");

        Ok(())
    });

    sim.run().unwrap();
}

/// [feat_req_someip_278] Response preserves interface version from request
#[test_log::test]
fn response_preserves_interface_version() {
    covers!(feat_req_someip_278);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

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

        if let Some(event) = offering.next().await {
            if let ServiceEvent::Call { responder, .. } = event {
                responder.reply(b"response").unwrap();
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

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

        // Send request with interface version 0x01
        let request = SomeIpPacketBuilder::request(0x1234, 0x0001)
            .client_id(0xABCD)
            .session_id(0x1234)
            .payload(b"test")
            .build();
        rpc_socket.send_to(&request, server_addr).await?;

        // Receive response
        let (len, _) =
            tokio::time::timeout(Duration::from_secs(3), rpc_socket.recv_from(&mut buf)).await??;

        let response = parse_header(&buf[..len]).expect("Valid response");

        // Response should preserve interface version
        assert_eq!(
            response.interface_version, 0x01,
            "Response must preserve interface version from request"
        );

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// INTERFACE VERSION MISMATCH TESTS (SERVER BEHAVIOR)
// ============================================================================

/// [feat_req_someip_92, feat_req_someip_371] Server returns E_WRONG_INTERFACE_VERSION
/// when request has mismatched interface version.
///
/// Per feat_req_someip_92: Interface Version contains the Major Version.
/// Per feat_req_someip_371: E_WRONG_INTERFACE_VERSION (0x08) for interface version mismatch.
/// Per feat_req_someip_718: Check order includes "Interface Version configured for Service ID?"
#[test_log::test]
fn server_returns_wrong_interface_version_error() {
    covers!(feat_req_someip_92, feat_req_someip_371, feat_req_someip_718);

    const E_WRONG_INTERFACE_VERSION: u8 = 0x08;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library server with major version 1
    sim.client("server", async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(1, 0) // Major version 1
            .udp()
            .start()
            .await
            .unwrap();

        // The server should NOT receive requests with wrong interface version
        // They should be rejected at the protocol level
        let result = tokio::time::timeout(Duration::from_secs(3), offering.next()).await;

        // Should timeout - wrong interface version requests are rejected before reaching app
        assert!(
            result.is_err(),
            "Server should not receive requests with wrong interface version"
        );

        Ok(())
    });

    // Raw client sends request with mismatched interface version
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut server_endpoint: Option<SocketAddr> = None;
        let mut buf = [0u8; 1500];

        // Wait for SD offer
        for _ in 0..20 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == TEST_SERVICE_ID {
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

        // Send request with WRONG interface version (0x02 instead of 0x01)
        let bad_request = SomeIpPacketBuilder::request(TEST_SERVICE_ID, 0x0001)
            .client_id(0xABCD)
            .session_id(0x1234)
            .interface_version(0x02) // WRONG - server expects 0x01
            .payload(b"test_payload")
            .build();
        rpc_socket.send_to(&bad_request, server_addr).await?;

        // Should receive error response with E_WRONG_INTERFACE_VERSION
        let result =
            tokio::time::timeout(Duration::from_secs(3), rpc_socket.recv_from(&mut buf)).await;

        let (len, _) = result.expect("Should receive error response")?;
        let response = parse_header(&buf[..len]).expect("Valid response header");

        // Verify error response
        assert_eq!(
            response.return_code, E_WRONG_INTERFACE_VERSION,
            "Server must return E_WRONG_INTERFACE_VERSION (0x08) for interface version mismatch"
        );

        // Verify header fields are echoed back (per feat_req_someip_655)
        assert_eq!(response.service_id, TEST_SERVICE_ID);
        assert_eq!(response.method_id, 0x0001);
        assert_eq!(response.client_id, 0xABCD);
        assert_eq!(response.session_id, 0x1234);

        // Message type should be RESPONSE (0x80) or ERROR (0x81)
        assert!(
            response.message_type == recentip::wire::MessageType::Response
                || response.message_type == recentip::wire::MessageType::Error,
            "Error response should have message type RESPONSE or ERROR, got {:?}",
            response.message_type
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// [feat_req_someip_654] Fire-and-forget with wrong interface version is silently ignored
///
/// Per feat_req_someip_654: No error response shall be sent for fire-and-forget methods.
/// Even with wrong interface version, fire-and-forget should be silently dropped.
#[test_log::test]
fn fire_forget_with_wrong_interface_version_is_ignored() {
    covers!(feat_req_someip_654, feat_req_someip_92);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library server with major version 1
    sim.client("server", async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(1, 0) // Major version 1
            .udp()
            .start()
            .await
            .unwrap();

        // Server should NOT receive fire-and-forget with wrong interface version
        let result = tokio::time::timeout(Duration::from_secs(2), offering.next()).await;

        assert!(
            result.is_err(),
            "Server should not receive fire-and-forget with wrong interface version"
        );

        Ok(())
    });

    // Raw client sends fire-and-forget with wrong interface version
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut server_endpoint: Option<SocketAddr> = None;
        let mut buf = [0u8; 1500];

        // Wait for SD offer
        for _ in 0..20 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == TEST_SERVICE_ID {
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

        // Send fire-and-forget with WRONG interface version
        let bad_fnf = SomeIpPacketBuilder::fire_and_forget(TEST_SERVICE_ID, 0x0001)
            .client_id(0xABCD)
            .session_id(0x1234)
            .interface_version(0x02) // WRONG - server expects 0x01
            .payload(b"test_payload")
            .build();
        rpc_socket.send_to(&bad_fnf, server_addr).await?;

        // Per feat_req_someip_654: No error response for fire-and-forget
        let result =
            tokio::time::timeout(Duration::from_millis(500), rpc_socket.recv_from(&mut buf)).await;

        assert!(
            result.is_err(),
            "Fire-and-forget with wrong interface version should NOT receive any response"
        );

        Ok(())
    });

    sim.run().unwrap();
}
