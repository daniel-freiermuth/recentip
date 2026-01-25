//! Request/Response Communication Flow Compliance Tests
//!
//! Integration tests for SOME/IP RPC patterns using turmoil network simulation.
//!
//! Key requirements tested:
//! - feat_req_someip_328: Request/Response communication pattern
//! - feat_req_someip_329: Request triggers response from server
//! - feat_req_someip_338: Response contains same Request ID as request
//! - feat_req_someip_345: Fire&Forget (REQUEST_NO_RETURN)
//! - feat_req_someip_348: Fire&Forget shall not return error
//! - feat_req_someip_141: Request (0x00) answered by Response (0x80)

use bytes::Bytes;
use recentip::handle::ServiceEvent;
use recentip::prelude::*;
use recentip::wire::{Header, MessageType, SdMessage, SD_METHOD_ID, SD_SERVICE_ID};
use std::net::SocketAddr;
use std::time::Duration;

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

/// Build SD offer message
#[allow(dead_code)]
fn build_sd_offer(
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
    packet.push(0x00); // Index 1st options
    packet.push(0x10); // Index 2nd options + # of opts 1
    packet.push(0x00); // # of opts 2
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

/// Build raw SOME/IP fire-and-forget request
fn build_fire_and_forget(
    service_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    interface_version: u8,
    payload: &[u8],
) -> Vec<u8> {
    let length = 8 + payload.len() as u32;
    let mut packet = Vec::with_capacity(16 + payload.len());

    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&method_id.to_be_bytes());
    packet.extend_from_slice(&length.to_be_bytes());
    packet.extend_from_slice(&client_id.to_be_bytes());
    packet.extend_from_slice(&session_id.to_be_bytes());
    packet.push(0x01); // Protocol version
    packet.push(interface_version);
    packet.push(0x01); // Message type: REQUEST_NO_RETURN
    packet.push(0x00); // Return code
    packet.extend_from_slice(payload);

    packet
}

// ============================================================================
// FIRE & FORGET NO RESPONSE TESTS
// ============================================================================

/// [feat_req_someip_348] Fire&Forget shall not receive any response or error
///
/// Fire&Forget messages (REQUEST_NO_RETURN) shall not return an error.
/// This test verifies that when a server receives a fire-and-forget request,
/// it does NOT send any response or error message back.
#[test_log::test]
fn fire_and_forget_no_response_or_error() {
    covers!(feat_req_someip_348, feat_req_someip_654);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library server receives F&F but sends no response
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

        // Wait for the fire-and-forget request
        let result = tokio::time::timeout(Duration::from_secs(5), offering.next()).await;

        if let Ok(Some(event)) = result {
            match event {
                ServiceEvent::FireForget {
                    method, payload, ..
                } => {
                    assert_eq!(method, MethodId::new(0x0001).unwrap());
                    assert_eq!(payload.as_ref(), b"ff_data");
                    // Server receives F&F but does NOT respond
                }
                other => panic!("Expected FireForget event, got {:?}", other),
            }
        } else {
            panic!("Did not receive fire-and-forget request");
        }

        // Keep server running to capture any errant responses
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    // Raw client sends F&F and monitors for any responses
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut server_endpoint: Option<SocketAddr> = None;
        let mut buf = [0u8; 1500];

        // Discover server via SD
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

        // Send fire-and-forget from RPC socket
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:40000").await?;
        let ff_request = build_fire_and_forget(0x1234, 0x0001, 0xABCD, 0x0001, 0x01, b"ff_data");
        rpc_socket.send_to(&ff_request, server_addr).await?;

        // Wait and check for any response (there should be NONE)
        let mut received_messages = Vec::new();
        for _ in 0..5 {
            let result =
                tokio::time::timeout(Duration::from_millis(100), rpc_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some(header) = parse_header(&buf[..len]) {
                    received_messages.push(header.message_type);
                }
            }
        }

        // Verify no RESPONSE or ERROR was received
        assert!(
            !received_messages
                .iter()
                .any(|mt| *mt == MessageType::Response),
            "Fire&Forget should NOT receive RESPONSE"
        );
        assert!(
            !received_messages.iter().any(|mt| *mt == MessageType::Error),
            "Fire&Forget should NOT receive ERROR"
        );

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// CONCURRENT REQUEST MATCHING TESTS
// ============================================================================

/// [feat_req_someip_338] Concurrent requests are correctly matched with responses
///
/// When multiple requests are pending and responses arrive out of order,
/// each response must be matched to its correct request using the Request ID.
#[test_log::test]
fn concurrent_requests_matched_by_request_id() {
    covers!(feat_req_someip_338, feat_req_someip_88);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server responds to requests in REVERSE order
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

        // Collect 3 requests before responding
        let mut pending_requests = Vec::new();
        for _ in 0..3 {
            if let Some(event) = tokio::time::timeout(Duration::from_secs(5), offering.next())
                .await
                .ok()
                .flatten()
            {
                if let ServiceEvent::Call {
                    payload, responder, ..
                } = event
                {
                    pending_requests.push((payload, responder));
                }
            }
        }

        // Small delay to simulate processing
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Respond in REVERSE order (3, 2, 1)
        for (payload, responder) in pending_requests.into_iter().rev() {
            let value = payload[0];
            // Response contains the request value + 100 to identify it
            responder.reply(&[value + 100]).unwrap();
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    // Client sends 3 concurrent requests and verifies each gets correct response
    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

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
            .expect("Timeout waiting for service")
            .expect("Service available");

        // Send 3 requests concurrently
        let call1 = proxy.call(MethodId::new(0x0001).unwrap(), &[1u8]);
        let call2 = proxy.call(MethodId::new(0x0001).unwrap(), &[2u8]);
        let call3 = proxy.call(MethodId::new(0x0001).unwrap(), &[3u8]);

        // Wait for all responses
        let (r1, r2, r3) = tokio::join!(call1, call2, call3);

        // Each response should contain request_value + 100
        let r1 = r1.expect("Call 1 should succeed");
        let r2 = r2.expect("Call 2 should succeed");
        let r3 = r3.expect("Call 3 should succeed");

        assert_eq!(r1.payload[0], 101, "Request 1 should get response 101");
        assert_eq!(r2.payload[0], 102, "Request 2 should get response 102");
        assert_eq!(r3.payload[0], 103, "Request 3 should get response 103");

        Ok(())
    });

    sim.run().unwrap();
}

/// [feat_req_someip_328, feat_req_someip_329] Basic request/response pattern works
///
/// One communication partner sends a request and receives a response.
/// This is a basic integration test for the request/response pattern.
#[test_log::test]
fn request_triggers_response() {
    covers!(feat_req_someip_328, feat_req_someip_329);

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

        if let Some(event) = tokio::time::timeout(Duration::from_secs(5), offering.next())
            .await
            .ok()
            .flatten()
        {
            match event {
                ServiceEvent::Call {
                    payload, responder, ..
                } => {
                    assert_eq!(payload.as_ref(), b"request_data");
                    responder.reply(b"response_data").unwrap();
                }
                _ => panic!("Expected Call event"),
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

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
            .expect("Timeout waiting for service")
            .expect("Service available");

        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(0x0001).unwrap(), b"request_data"),
        )
        .await
        .expect("Timeout waiting for response")
        .expect("Call should succeed");

        assert_eq!(response.payload.as_ref(), b"response_data");

        Ok(())
    });

    sim.run().unwrap();
}

/// [feat_req_someip_141] Request (0x00) is answered by Error (0x81) on failure
///
/// When the server cannot process a request successfully, it responds with
/// an ERROR message type (0x81) instead of RESPONSE (0x80).
#[test_log::test]
fn request_can_receive_error_response() {
    covers!(feat_req_someip_141);

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

        if let Some(event) = tokio::time::timeout(Duration::from_secs(5), offering.next())
            .await
            .ok()
            .flatten()
        {
            match event {
                ServiceEvent::Call { responder, .. } => {
                    // Server replies with error
                    responder.reply_error(ApplicationError::NotOk).unwrap();
                }
                _ => panic!("Expected Call event"),
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

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
            .expect("Timeout waiting for service")
            .expect("Service available");

        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(0x0001).unwrap(), b"request"),
        )
        .await
        .expect("Timeout waiting for response")
        .expect("Call should complete (even with error)");

        // The response should indicate an error
        assert_ne!(
            response.return_code,
            ReturnCode::Ok,
            "Error response should have non-Ok return code"
        );

        Ok(())
    });

    sim.run().unwrap();
}
