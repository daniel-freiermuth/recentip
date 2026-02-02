//! Notification Header Compliance Tests (Server Under Test)
//!
//! Tests that verify server correctly sets SOME/IP header fields in notification messages.
//!
//! # Test Setup
//! - Library: Acts as **server** (offers service, sends events)
//! - Raw socket: Acts as **client** (subscribes via raw SD, inspects notification wire format)
//!
//! # Requirements Covered
//! - feat_req_someip_92: Interface Version shall contain Major Version of Service Interface

use super::helpers::{
    build_sd_subscribe_with_udp_endpoint, covers, parse_header, parse_sd_message,
};
use recentip::prelude::*;
use recentip::wire::MessageType;
use recentip::ServiceEvent;
use std::net::SocketAddr;
use std::time::Duration;

/// Wire-level event ID for our test event (0x8001)
const TEST_EVENT_ID: u16 = 0x8001;

/// [feat_req_someip_92] Interface Version shall contain Major Version of Service Interface
///
/// This test verifies that notification messages have their Interface Version field
/// set to the service's major version, not hardcoded.
///
/// We offer two services with different major versions and verify each notification
/// carries the correct interface_version in its SOME/IP header.
#[test_log::test]
fn notification_interface_version_matches_major_version() {
    covers!(feat_req_someip_92);

    const SERVICE_A_ID: u16 = 0x1234;
    const SERVICE_A_MAJOR: u8 = 2; // Non-default major version
    const SERVICE_A_MINOR: u32 = 0;

    const SERVICE_B_ID: u16 = 0x5678;
    const SERVICE_B_MAJOR: u8 = 7; // Different major version
    const SERVICE_B_MINOR: u32 = 3;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .max_message_latency(Duration::from_millis(50))
        .build();

    // Server offers two services with different major versions
    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Offer service A with major version 2
        let mut offering_a = runtime
            .offer(SERVICE_A_ID, InstanceId::Id(0x0001))
            .version(SERVICE_A_MAJOR, SERVICE_A_MINOR)
            .udp()
            .start()
            .await
            .unwrap();

        // Offer service B with major version 7
        let mut offering_b = runtime
            .offer(SERVICE_B_ID, InstanceId::Id(0x0001))
            .version(SERVICE_B_MAJOR, SERVICE_B_MINOR)
            .udp()
            .start()
            .await
            .unwrap();

        // Create events for both services
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(TEST_EVENT_ID).unwrap();

        let event_a = offering_a
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        let event_b = offering_b
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        // Wait for subscriptions to both services
        let mut subs_a = false;
        let mut subs_b = false;

        for _ in 0..50 {
            tokio::select! {
                biased;
                Some(ServiceEvent::Subscribe { .. }) = offering_a.next(), if !subs_a => {
                    subs_a = true;
                }
                Some(ServiceEvent::Subscribe { .. }) = offering_b.next(), if !subs_b => {
                    subs_b = true;
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
            if subs_a && subs_b {
                break;
            }
        }

        assert!(subs_a, "Should receive subscription for service A");
        assert!(subs_b, "Should receive subscription for service B");

        // Send events from both services
        event_a.notify(b"from_service_a").await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        event_b.notify(b"from_service_b").await.unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    // Raw socket client: subscribes to both services and inspects notification headers
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // SD socket for subscribe messages (port 30490)
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        // Event receive socket - client's UDP endpoint for notifications
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:40001").await?;

        let mut buf = [0u8; 1500];
        let mut server_sd_endpoint: Option<SocketAddr> = None;
        let mut service_a_discovered = false;
        let mut service_b_discovered = false;

        // Wait for OfferService messages for both services
        for _ in 0..100 {
            let result =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // OfferService = 0x01 with TTL > 0
                        if entry.entry_type as u8 == 0x01 && entry.ttl > 0 {
                            if entry.service_id == SERVICE_A_ID {
                                service_a_discovered = true;
                                server_sd_endpoint = Some(SocketAddr::new(from.ip(), 30490));
                            }
                            if entry.service_id == SERVICE_B_ID {
                                service_b_discovered = true;
                                server_sd_endpoint = Some(SocketAddr::new(from.ip(), 30490));
                            }
                        }
                    }
                }
            }

            if service_a_discovered && service_b_discovered {
                break;
            }
        }

        assert!(service_a_discovered, "Should discover service A");
        assert!(service_b_discovered, "Should discover service B");
        let server_sd_ep = server_sd_endpoint.expect("Should have server SD endpoint");

        let client_ip: std::net::Ipv4Addr =
            turmoil::lookup("raw_client").to_string().parse().unwrap();

        // Subscribe to both services in a single message burst with incrementing session IDs
        // We use session_id starting from 1 to avoid reboot detection issues
        let subscribe_a = build_sd_subscribe_with_udp_endpoint(
            SERVICE_A_ID,
            0x0001,
            SERVICE_A_MAJOR,
            0x0001, // eventgroup
            0xFFFFFF,
            client_ip,
            40001,
            0x0001, // session_id - first message
        );
        sd_socket.send_to(&subscribe_a, server_sd_ep).await?;

        // Small delay to ensure ordering
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Subscribe to service B (major version 7)
        let subscribe_b = build_sd_subscribe_with_udp_endpoint(
            SERVICE_B_ID,
            0x0001,
            SERVICE_B_MAJOR,
            0x0001, // eventgroup
            0xFFFFFF,
            client_ip,
            40001,
            0x0002, // session_id - second message (incrementing)
        );
        sd_socket.send_to(&subscribe_b, server_sd_ep).await?;

        // Wait for SubscribeAcks (skip them)
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Receive notifications and verify interface_version
        let mut service_a_interface_version: Option<u8> = None;
        let mut service_b_interface_version: Option<u8> = None;

        for _ in 0..50 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), event_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some(header) = parse_header(&buf[..len]) {
                    if header.message_type == MessageType::Notification {
                        if header.service_id == SERVICE_A_ID {
                            service_a_interface_version = Some(header.interface_version);
                        }
                        if header.service_id == SERVICE_B_ID {
                            service_b_interface_version = Some(header.interface_version);
                        }
                    }
                }
            }

            if service_a_interface_version.is_some() && service_b_interface_version.is_some() {
                break;
            }
        }

        // Verify: feat_req_someip_92 - Interface Version = Major Version
        let iv_a = service_a_interface_version.expect("Should receive notification from service A");
        let iv_b = service_b_interface_version.expect("Should receive notification from service B");

        assert_eq!(
            iv_a, SERVICE_A_MAJOR,
            "Service A notification interface_version ({}) should equal major_version ({})",
            iv_a, SERVICE_A_MAJOR
        );
        assert_eq!(
            iv_b, SERVICE_B_MAJOR,
            "Service B notification interface_version ({}) should equal major_version ({})",
            iv_b, SERVICE_B_MAJOR
        );

        Ok(())
    });

    sim.run().unwrap();
}
