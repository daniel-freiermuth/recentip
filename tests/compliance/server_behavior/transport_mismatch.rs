//! Transport Mismatch Tests (Server Under Test)
//!
//! Tests that verify server correctly NACKs subscriptions when client sends
//! incompatible transport endpoints.
//!
//! # Test Setup
//! - Library: Acts as **server** (offers service)
//! - Raw socket: Acts as **client** (sends malformed subscribe)
//!
//! # Requirements Covered
//! - feat_req_recentipsd_1144: If options are in conflict → respond negatively (NACK)
//! - feat_req_recentipsd_1137: Respond with SubscribeEventgroupNack for invalid subscribe

use super::helpers::{
    build_sd_subscribe_with_tcp_endpoint, build_sd_subscribe_with_udp_endpoint, covers,
    parse_sd_message, TEST_SERVICE_ID, TEST_SERVICE_VERSION,
};
use someip_runtime::prelude::*;
use someip_runtime::runtime::Runtime;
use std::net::SocketAddr;
use std::time::Duration;

/// feat_req_recentipsd_1144: Transport mismatch should result in NACK
/// feat_req_recentipsd_1137: Respond with SubscribeEventgroupNack for invalid subscribe
///
/// When a server offers events only via UDP, but a client sends a SubscribeEventgroup
/// with only a TCP endpoint option, the server MUST respond with a SubscribeEventgroupNack.
///
/// Current behavior: Server incorrectly accepts and sends ACK (falling back to source address)
/// Expected behavior: Server rejects with NACK (TTL=0, entry type 0x07)
#[test_log::test]
fn subscribe_tcp_endpoint_to_udp_only_server_should_nack() {
    covers!(feat_req_recentipsd_1144, feat_req_recentipsd_1137);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers events via UDP only
    sim.host("server", || async {
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(Default::default()).await.unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp() // UDP only!
            .start()
            .await
            .unwrap();

        // Keep server running
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Raw client: sends Subscribe with TCP endpoint only
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == 0x1234
                            && entry.ttl > 0
                        {
                            // Subscribe messages should be sent to SD port (30490)
                            server_endpoint = Some(SocketAddr::new(from.ip(), 30490));
                        }
                    }
                }
            }

            if server_endpoint.is_some() {
                break;
            }
        }

        let server_ep = server_endpoint.expect("Should discover server");

        // Get our local address for the endpoint option
        let local_addr = sd_socket.local_addr()?;
        let client_ip: std::net::Ipv4Addr = match local_addr.ip() {
            std::net::IpAddr::V4(ip) => ip,
            std::net::IpAddr::V6(_) => "0.0.0.0".parse().unwrap(),
        };

        // Send SubscribeEventgroup with TCP endpoint (mismatch!)
        let subscribe = build_sd_subscribe_with_tcp_endpoint(
            0x1234, 0x0001, 1, 0x0001, // eventgroup
            3600,   // TTL
            client_ip, 40000, // TCP port we're "listening" on
        );
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for response - should be NACK (type 0x07 with TTL=0)
        let mut found_nack = false;
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234 && entry.eventgroup_id == 0x0001 {
                            if entry.entry_type as u8 == 0x07 {
                                if entry.ttl == 0 {
                                    // NACK (type 0x07 with TTL=0)
                                    found_nack = true;
                                } else {
                                    // ACK (type 0x07 with TTL>0)
                                    found_ack = true;
                                }
                            }
                        }
                    }
                }
            }

            if found_nack || found_ack {
                break;
            }
        }

        assert!(
            found_nack,
            "Server should NACK subscribe with TCP endpoint when only UDP is offered. \
             Got ACK={}, NACK={}. \
             Per feat_req_recentipsd_1144: incompatible options should be responded negatively.",
            found_ack, found_nack
        );
        assert!(
            !found_ack,
            "Server should NOT ACK subscribe with incompatible transport"
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_1144: Transport mismatch should result in NACK
/// feat_req_recentipsd_1137: Respond with SubscribeEventgroupNack for invalid subscribe
///
/// When a server offers events only via TCP, but a client sends a SubscribeEventgroup
/// with only a UDP endpoint option, the server MUST respond with a SubscribeEventgroupNack.
#[test_log::test]
fn subscribe_udp_endpoint_to_tcp_only_server_should_nack() {
    covers!(feat_req_recentipsd_1144, feat_req_recentipsd_1137);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers events via TCP only
    sim.host("server", || async {
        let runtime: Runtime<
            turmoil::net::UdpSocket,
            turmoil::net::TcpStream,
            turmoil::net::TcpListener,
        > = Runtime::with_socket_type(Default::default()).await.unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .tcp() // TCP only!
            .start()
            .await
            .unwrap();

        // Keep server running
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Raw client: sends Subscribe with UDP endpoint only
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];

        // Wait for OfferService (should have TCP endpoint)
        let mut sd_server_addr: Option<SocketAddr> = None;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == 0x1234
                            && entry.ttl > 0
                        {
                            // Use SD source address for sending subscribe
                            // (not the RPC endpoint from the option)
                            sd_server_addr = Some(from);
                        }
                    }
                }
            }

            if sd_server_addr.is_some() {
                break;
            }
        }

        let server_ep = sd_server_addr.expect("Should discover server");

        let local_addr = sd_socket.local_addr()?;
        let client_ip: std::net::Ipv4Addr = match local_addr.ip() {
            std::net::IpAddr::V4(ip) => ip,
            std::net::IpAddr::V6(_) => "0.0.0.0".parse().unwrap(),
        };

        // Send SubscribeEventgroup with UDP endpoint (mismatch - server is TCP only!)
        let subscribe =
            build_sd_subscribe_with_udp_endpoint(0x1234, 0x0001, 1, 0x0001, 3600, client_ip, 40000);
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for response - should be NACK
        let mut found_nack = false;
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234 && entry.eventgroup_id == 0x0001 {
                            if entry.entry_type as u8 == 0x07 {
                                if entry.ttl == 0 {
                                    found_nack = true;
                                } else {
                                    found_ack = true;
                                }
                            }
                        }
                    }
                }
            }

            if found_nack || found_ack {
                break;
            }
        }

        assert!(
            found_nack,
            "Server should NACK subscribe with UDP endpoint when only TCP is offered. \
             Got ACK={}, NACK={}. \
             Per feat_req_recentipsd_1144: incompatible options should be responded negatively.",
            found_ack, found_nack
        );
        assert!(
            !found_ack,
            "Server should NOT ACK subscribe with incompatible transport"
        );

        Ok(())
    });

    sim.run().unwrap();
}
