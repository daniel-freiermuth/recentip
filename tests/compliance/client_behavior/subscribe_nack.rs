//! Subscribe NACK Handling Tests (Client Under Test)
//!
//! Tests that verify client correctly handles SubscribeEventgroupNack responses.
//!
//! # Test Setup
//! - Library: Acts as **client** (discovers and subscribes)
//! - Raw socket: Acts as **server** (sends offers, ACKs one service, NACKs the other)
//!
//! # Requirements Covered
//! - feat_req_recentipsd_1137: Respond with SubscribeEventgroupNack for invalid subscribe
//! - Client must propagate NACK as an error to the caller

use crate::client_behavior::helpers::build_sd_offer_with_session;

use super::helpers::{
    build_sd_offer_tcp_only, build_sd_subscribe_ack_with_session, covers, parse_sd_message,
};
use recentip::prelude::*;
use recentip::wire::{L4Protocol, SdOption};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Build a raw SOME/IP-SD SubscribeEventgroupNack message (ACK with TTL=0)
fn build_sd_subscribe_nack(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    session_id: u16,
) -> Vec<u8> {
    // NACK is just an ACK with TTL=0
    build_sd_subscribe_ack_with_session(
        service_id,
        instance_id,
        major_version,
        eventgroup_id,
        0,
        session_id,
    )
}

/// Build a raw SOME/IP event notification message
fn build_event_notification(
    service_id: u16,
    event_id: u16,
    interface_version: u8,
    session_id: u16,
    payload: &[u8],
) -> Vec<u8> {
    let length = 8 + payload.len() as u32;
    let mut packet = Vec::with_capacity(16 + payload.len());

    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&event_id.to_be_bytes()); // event_id >= 0x8000
    packet.extend_from_slice(&length.to_be_bytes());
    packet.extend_from_slice(&0x0000u16.to_be_bytes()); // Client ID (0 for events)
    packet.extend_from_slice(&session_id.to_be_bytes());
    packet.push(0x01); // Protocol Version
    packet.push(interface_version);
    packet.push(0x02); // Message Type = NOTIFICATION
    packet.push(0x00); // Return Code = E_OK
    packet.extend_from_slice(payload);

    packet
}

/// Extract the first IPv4 UDP endpoint from an SD entry's options
fn extract_client_udp_endpoint(
    entry: &recentip::wire::SdEntry,
    options: &[SdOption],
) -> Option<(Ipv4Addr, u16)> {
    // Calculate the option indices for this entry
    let start_idx = entry.index_1st_option as usize;
    let end_idx = start_idx + entry.num_options_1 as usize;

    for idx in start_idx..end_idx {
        if let Some(SdOption::Ipv4Endpoint {
            addr,
            port,
            protocol: L4Protocol::Udp,
        }) = options.get(idx)
        {
            return Some((*addr, *port));
        }
    }
    None
}

#[test_log::test]
fn subscribe_nack_then_ack() {
    covers!(feat_req_recentipsd_1137);

    const SERVICE_ID: u16 = 0x1234;
    const INSTANCE_ID: u16 = 0x0001;
    const VERSION_ACK: u8 = 1; // This version will be ACKed
    const VERSION_NACK: u8 = 2; // This version will be NACKed
    const EVENTGROUP_ID: u16 = 0x0001;
    const EVENT_ID: u16 = 0x8001;

    let ack_count = Arc::new(AtomicUsize::new(0));
    let nack_count = Arc::new(AtomicUsize::new(0));
    let events_sent = Arc::new(AtomicUsize::new(0));
    let ack_count_server = Arc::clone(&ack_count);
    let nack_count_server = Arc::clone(&nack_count);
    let events_sent_server = Arc::clone(&events_sent);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Wire-level server that offers two services, ACKs one, NACKs the other
    sim.host("wire_server", move || {
        let ack_count = Arc::clone(&ack_count_server);
        let nack_count = Arc::clone(&nack_count_server);
        let events_sent = Arc::clone(&events_sent_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            // RPC socket for sending events (v1 endpoint)
            let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?;

            let mut buf = [0u8; 1500];
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);
            let mut v1_client_endpoint: Option<SocketAddr> = None;
            let mut next_unicast_session_id = 1u16;
            let mut next_multicast_session_id = 1u16;
            let mut first_offer = true;

            let mut ack = None;
            let mut nack = None;

            while tokio::time::Instant::now() < deadline {
                // Send offers periodically
                if last_offer.elapsed() >= Duration::from_millis(500) {
                    let offer_v1 = build_sd_offer_with_session(
                        SERVICE_ID, INSTANCE_ID, VERSION_ACK, 0, my_ip, 30509, 3600,
                        next_multicast_session_id, first_offer, false,
                    );
                    next_multicast_session_id += 1;
                    sd_socket.send_to(&offer_v1, sd_multicast).await?;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let offer_v2 = build_sd_offer_with_session(
                        SERVICE_ID, INSTANCE_ID, VERSION_NACK, 0, my_ip, 30510, 3600,
                        next_multicast_session_id, first_offer, false,
                    );
                    next_multicast_session_id = next_multicast_session_id.wrapping_add(1);
                    sd_socket.send_to(&offer_v2, sd_multicast).await?;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    eprintln!("[wire_server] Sent offers for v1 and v2");
                    last_offer = tokio::time::Instant::now();
                    first_offer = false;
                }

                // Check for incoming messages
                if let Ok(Ok((len, from))) = tokio::time::timeout(
                    Duration::from_millis(100),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            // SubscribeEventgroup = 0x06
                            if entry.entry_type as u8 == 0x06 && entry.service_id == SERVICE_ID {
                                eprintln!(
                                    "[wire_server] Received subscribe for version {} eventgroup {:04x}",
                                    entry.major_version, entry.eventgroup_id
                                );

                                if entry.major_version == VERSION_ACK {
                                    // Extract client endpoint from subscribe options
                                    if let Some((client_ip, client_port)) =
                                        extract_client_udp_endpoint(entry, &sd_msg.options)
                                    {
                                        v1_client_endpoint = Some(SocketAddr::from((client_ip, client_port)));
                                        eprintln!("[wire_server] v1 client endpoint: {:?}", v1_client_endpoint);
                                    }

                                    // ACK for v1
                                    ack = Some(());
                                    ack_count.fetch_add(1, Ordering::SeqCst);
                                    eprintln!("[wire_server] Sent ACK for v1");

                                    // Send event to v1 subscriber
                                    if let Some(endpoint) = v1_client_endpoint {
                                        let payload = b"v1_event_data";
                                        let event = build_event_notification(
                                            SERVICE_ID,
                                            EVENT_ID,
                                            VERSION_ACK,
                                            1, // session_id
                                            payload,
                                        );
                                        rpc_socket.send_to(&event, endpoint).await?;
                                        events_sent.fetch_add(1, Ordering::SeqCst);
                                        eprintln!("[wire_server] Sent event to v1 subscriber at {:?}", endpoint);
                                    }
                                } else if entry.major_version == VERSION_NACK {
                                    // NACK for v2
                                    nack = Some(());
                                    nack_count.fetch_add(1, Ordering::SeqCst);
                                    eprintln!("[wire_server] Sent NACK for v2");
                                }
                                if let (Some(_), Some(_)) = (&ack, &nack) {
                                    let nack = build_sd_subscribe_nack(
                                        SERVICE_ID,
                                        INSTANCE_ID,
                                        VERSION_NACK,
                                        EVENTGROUP_ID,
                                        next_unicast_session_id,
                                    );
                                    next_unicast_session_id += 1;
                                    sd_socket.send_to(&nack, from).await?;
                                    tokio::time::sleep(Duration::from_millis(100)).await;
                                    let ack = build_sd_subscribe_ack_with_session(
                                        SERVICE_ID,
                                        INSTANCE_ID,
                                        VERSION_ACK,
                                        EVENTGROUP_ID,
                                        entry.ttl,
                                        next_unicast_session_id,
                                    );
                                    next_unicast_session_id += 1;
                                    sd_socket.send_to(&ack, from).await?;
                                    tokio::time::sleep(Duration::from_millis(10)).await;
                                }
                            }
                        }
                    }
                }
            }

            Ok(())
        }
    });

    // Client flags to track subscription results
    let v1_subscribed = Arc::new(AtomicBool::new(false));
    let v2_rejected = Arc::new(AtomicBool::new(false));
    let v1_event_received = Arc::new(AtomicBool::new(false));
    let v1_sub_clone = Arc::clone(&v1_subscribed);
    let v2_rej_clone = Arc::clone(&v2_rejected);
    let v1_event_clone = Arc::clone(&v1_event_received);

    // Client using our library
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Discover service v1
        let proxy_v1 = runtime
            .find(SERVICE_ID)
            .major_version(VERSION_ACK)
            .instance(InstanceId::Id(INSTANCE_ID));
        let proxy_v1 = tokio::time::timeout(Duration::from_secs(5), proxy_v1)
            .await
            .expect("Discovery timeout for v1")
            .expect("Service v1 available");
        eprintln!("[client] Service v1 discovered");

        // Discover service v2
        let proxy_v2 = runtime
            .find(SERVICE_ID)
            .major_version(VERSION_NACK)
            .instance(InstanceId::Id(INSTANCE_ID));
        let proxy_v2 = tokio::time::timeout(Duration::from_secs(5), proxy_v2)
            .await
            .expect("Discovery timeout for v2")
            .expect("Service v2 available");
        eprintln!("[client] Service v2 discovered");

        // Subscribe to v1 - should succeed
        let eventgroup = EventgroupId::new(EVENTGROUP_ID).unwrap();
        let async1 = async {
            let mut subscription_v1 = tokio::time::timeout(
                Duration::from_secs(5),
                proxy_v1
                    .new_subscription()
                    .eventgroup(eventgroup)
                    .subscribe(),
            )
            .await
            .expect("Subscribe timeout for v1")
            .expect("Subscribe should succeed for v1");

            eprintln!("[client] v1 subscription succeeded (expected)");
            v1_sub_clone.store(true, Ordering::SeqCst);
            subscription_v1
        };
        let async2 = async {
            // Subscribe to v2 - should fail with NACK
            let result_v2 = tokio::time::timeout(
                Duration::from_secs(5),
                proxy_v2
                    .new_subscription()
                    .eventgroup(eventgroup)
                    .subscribe(),
            )
            .await
            .expect("Subscribe timeout for v2");

            match result_v2 {
                Ok(_subscription) => {
                    panic!("[client] v2 subscription succeeded unexpectedly (expected NACK)");
                }
                Err(e) => {
                    eprintln!("[client] v2 subscription rejected as expected: {:?}", e);
                    v2_rej_clone.store(true, Ordering::SeqCst);
                }
            }
        };
        let (mut subscription_v1, _) = tokio::join!(async1, async2);

        // Wait for event from v1 subscription
        match tokio::time::timeout(Duration::from_secs(5), subscription_v1.next()).await {
            Ok(Some(event)) => {
                eprintln!(
                    "[client] Received v1 event: {:?}",
                    String::from_utf8_lossy(&event.payload)
                );
                assert_eq!(
                    event.payload.as_ref(),
                    b"v1_event_data",
                    "Event payload should match"
                );
                v1_event_clone.store(true, Ordering::SeqCst);
            }
            Ok(None) => {
                panic!("[client] v1 subscription stream ended unexpectedly");
            }
            Err(_) => {
                panic!("[client] Timeout waiting for v1 event");
            }
        }

        Ok(())
    });

    sim.run().unwrap();

    // Verify results
    assert!(
        v1_subscribed.load(Ordering::SeqCst),
        "Subscription to v1 should have succeeded"
    );
    assert!(
        v2_rejected.load(Ordering::SeqCst),
        "Subscription to v2 should have been rejected (NACK)"
    );
    assert!(
        ack_count.load(Ordering::SeqCst) >= 1,
        "Server should have sent at least 1 ACK"
    );
    assert!(
        nack_count.load(Ordering::SeqCst) >= 1,
        "Server should have sent at least 1 NACK"
    );
    assert!(
        v1_event_received.load(Ordering::SeqCst),
        "Client should have received event on v1 subscription"
    );
    assert!(
        events_sent.load(Ordering::SeqCst) >= 1,
        "Server should have sent at least 1 event"
    );
}

#[test_log::test]
fn subscribe_ack_then_nack() {
    covers!(feat_req_recentipsd_1137);

    const SERVICE_ID: u16 = 0x1234;
    const INSTANCE_ID: u16 = 0x0001;
    const VERSION_ACK: u8 = 1; // This version will be ACKed
    const VERSION_NACK: u8 = 2; // This version will be NACKed
    const EVENTGROUP_ID: u16 = 0x0001;
    const EVENT_ID: u16 = 0x8001;

    let ack_count = Arc::new(AtomicUsize::new(0));
    let nack_count = Arc::new(AtomicUsize::new(0));
    let events_sent = Arc::new(AtomicUsize::new(0));
    let ack_count_server = Arc::clone(&ack_count);
    let nack_count_server = Arc::clone(&nack_count);
    let events_sent_server = Arc::clone(&events_sent);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Wire-level server that offers two services, ACKs one, NACKs the other
    sim.host("wire_server", move || {
        let ack_count = Arc::clone(&ack_count_server);
        let nack_count = Arc::clone(&nack_count_server);
        let events_sent = Arc::clone(&events_sent_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            // RPC socket for sending events (v1 endpoint)
            let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?;

            let mut buf = [0u8; 1500];
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);
            let mut v1_client_endpoint: Option<SocketAddr> = None;
            let mut next_unicast_session_id = 1u16;
            let mut next_multicast_session_id = 1u16;
            let mut first_offer = true;

            let mut ack = None;
            let mut nack = None;

            while tokio::time::Instant::now() < deadline {
                // Send offers periodically
                if last_offer.elapsed() >= Duration::from_millis(500) {
                    let offer_v1 = build_sd_offer_with_session(
                        SERVICE_ID, INSTANCE_ID, VERSION_ACK, 0, my_ip, 30509, 3600,
                        next_multicast_session_id, first_offer, false,
                    );
                    next_multicast_session_id += 1;
                    sd_socket.send_to(&offer_v1, sd_multicast).await?;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let offer_v2 = build_sd_offer_with_session(
                        SERVICE_ID, INSTANCE_ID, VERSION_NACK, 0, my_ip, 30510, 3600,
                        next_multicast_session_id, first_offer, false,
                    );
                    next_multicast_session_id = next_multicast_session_id.wrapping_add(1);
                    sd_socket.send_to(&offer_v2, sd_multicast).await?;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    eprintln!("[wire_server] Sent offers for v1 and v2");
                    last_offer = tokio::time::Instant::now();
                    first_offer = false;
                }

                // Check for incoming messages
                if let Ok(Ok((len, from))) = tokio::time::timeout(
                    Duration::from_millis(100),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            // SubscribeEventgroup = 0x06
                            if entry.entry_type as u8 == 0x06 && entry.service_id == SERVICE_ID {
                                eprintln!(
                                    "[wire_server] Received subscribe for version {} eventgroup {:04x}",
                                    entry.major_version, entry.eventgroup_id
                                );

                                if entry.major_version == VERSION_ACK {
                                    // Extract client endpoint from subscribe options
                                    if let Some((client_ip, client_port)) =
                                        extract_client_udp_endpoint(entry, &sd_msg.options)
                                    {
                                        v1_client_endpoint = Some(SocketAddr::from((client_ip, client_port)));
                                        eprintln!("[wire_server] v1 client endpoint: {:?}", v1_client_endpoint);
                                    }

                                    ack = Some(());
                                    ack_count.fetch_add(1, Ordering::SeqCst);
                                    eprintln!("[wire_server] Sent ACK for v1");

                                    // Send event to v1 subscriber
                                    if let Some(endpoint) = v1_client_endpoint {
                                        let payload = b"v1_event_data";
                                        let event = build_event_notification(
                                            SERVICE_ID,
                                            EVENT_ID,
                                            VERSION_ACK,
                                            1, // session_id
                                            payload,
                                        );
                                        rpc_socket.send_to(&event, endpoint).await?;
                                        events_sent.fetch_add(1, Ordering::SeqCst);
                                        eprintln!("[wire_server] Sent event to v1 subscriber at {:?}", endpoint);
                                    }
                                } else if entry.major_version == VERSION_NACK {
                                    nack = Some(());
                                    nack_count.fetch_add(1, Ordering::SeqCst);
                                    eprintln!("[wire_server] Sent NACK for v2");
                                }
                                if let (Some(_), Some(_)) = (&ack, &nack) {
                                    // ACK for v1
                                    let ack_bytes = build_sd_subscribe_ack_with_session(
                                        SERVICE_ID,
                                        INSTANCE_ID,
                                        VERSION_ACK,
                                        EVENTGROUP_ID,
                                        entry.ttl,
                                        next_unicast_session_id,
                                    );
                                    next_unicast_session_id += 1;
                                    sd_socket.send_to(&ack_bytes, from).await?;
                                    tokio::time::sleep(Duration::from_millis(100)).await;
                                    // NACK for v2
                                    let nack_bytes = build_sd_subscribe_nack(
                                        SERVICE_ID,
                                        INSTANCE_ID,
                                        VERSION_NACK,
                                        EVENTGROUP_ID,
                                        next_unicast_session_id,
                                    );
                                    next_unicast_session_id += 1;
                                    sd_socket.send_to(&nack_bytes, from).await?;
                                    tokio::time::sleep(Duration::from_millis(10)).await;
                                }
                            }
                        }
                    }
                }
            }

            Ok(())
        }
    });

    // Client flags to track subscription results
    let v1_subscribed = Arc::new(AtomicBool::new(false));
    let v2_rejected = Arc::new(AtomicBool::new(false));
    let v1_event_received = Arc::new(AtomicBool::new(false));
    let v1_sub_clone = Arc::clone(&v1_subscribed);
    let v2_rej_clone = Arc::clone(&v2_rejected);
    let v1_event_clone = Arc::clone(&v1_event_received);

    // Client using our library
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Discover service v1
        let proxy_v1 = runtime
            .find(SERVICE_ID)
            .major_version(VERSION_ACK)
            .instance(InstanceId::Id(INSTANCE_ID));
        let proxy_v1 = tokio::time::timeout(Duration::from_secs(5), proxy_v1)
            .await
            .expect("Discovery timeout for v1")
            .expect("Service v1 available");
        eprintln!("[client] Service v1 discovered");

        // Discover service v2
        let proxy_v2 = runtime
            .find(SERVICE_ID)
            .major_version(VERSION_NACK)
            .instance(InstanceId::Id(INSTANCE_ID));
        let proxy_v2 = tokio::time::timeout(Duration::from_secs(5), proxy_v2)
            .await
            .expect("Discovery timeout for v2")
            .expect("Service v2 available");
        eprintln!("[client] Service v2 discovered");

        // Subscribe to v1 - should succeed
        let eventgroup = EventgroupId::new(EVENTGROUP_ID).unwrap();
        let async1 = async {
            let mut subscription_v1 = tokio::time::timeout(
                Duration::from_secs(5),
                proxy_v1
                    .new_subscription()
                    .eventgroup(eventgroup)
                    .subscribe(),
            )
            .await
            .expect("Subscribe timeout for v1")
            .expect("Subscribe should succeed for v1");

            eprintln!("[client] v1 subscription succeeded (expected)");
            v1_sub_clone.store(true, Ordering::SeqCst);
            subscription_v1
        };

        let async2 = async {
            // Subscribe to v2 - should fail with NACK
            let result_v2 = tokio::time::timeout(
                Duration::from_secs(5),
                proxy_v2
                    .new_subscription()
                    .eventgroup(eventgroup)
                    .subscribe(),
            )
            .await
            .expect("Subscribe timeout for v2");

            match result_v2 {
                Ok(_subscription) => {
                    panic!("[client] v2 subscription succeeded unexpectedly (expected NACK)");
                }
                Err(e) => {
                    eprintln!("[client] v2 subscription rejected as expected: {:?}", e);
                    v2_rej_clone.store(true, Ordering::SeqCst);
                }
            }
        };

        let (mut subscription_v1, _) = tokio::join!(async1, async2);

        // Wait for event from v1 subscription
        match tokio::time::timeout(Duration::from_secs(5), subscription_v1.next()).await {
            Ok(Some(event)) => {
                eprintln!(
                    "[client] Received v1 event: {:?}",
                    String::from_utf8_lossy(&event.payload)
                );
                assert_eq!(
                    event.payload.as_ref(),
                    b"v1_event_data",
                    "Event payload should match"
                );
                v1_event_clone.store(true, Ordering::SeqCst);
            }
            Ok(None) => {
                panic!("[client] v1 subscription stream ended unexpectedly");
            }
            Err(_) => {
                panic!("[client] Timeout waiting for v1 event");
            }
        }

        Ok(())
    });

    sim.run().unwrap();

    // Verify results
    assert!(
        v1_subscribed.load(Ordering::SeqCst),
        "Subscription to v1 should have succeeded"
    );
    assert!(
        v2_rejected.load(Ordering::SeqCst),
        "Subscription to v2 should have been rejected (NACK)"
    );
    assert!(
        ack_count.load(Ordering::SeqCst) >= 1,
        "Server should have sent at least 1 ACK"
    );
    assert!(
        nack_count.load(Ordering::SeqCst) >= 1,
        "Server should have sent at least 1 NACK"
    );
    assert!(
        v1_event_received.load(Ordering::SeqCst),
        "Client should have received event on v1 subscription"
    );
    assert!(
        events_sent.load(Ordering::SeqCst) >= 1,
        "Server should have sent at least 1 event"
    );
}

/// Test that client correctly handles mixed ACK/NACK responses from server.
///
/// Setup:
/// - Wire-level server offers two services (different versions of same service ID)
/// - Server ACKs subscriptions to service v1, but NACKs subscriptions to service v2
/// - Client subscribes to both services
/// - Verify: subscription to v1 succeeds, subscription to v2 returns an error
///
/// Per feat_req_recentipsd_1137: Server can reject subscriptions with NACK
#[test_log::test]
fn subscribe_ack_and_nack_different_services() {
    covers!(feat_req_recentipsd_1137);

    const SERVICE_ID: u16 = 0x1234;
    const INSTANCE_ID: u16 = 0x0001;
    const VERSION_ACK: u8 = 1; // This version will be ACKed
    const VERSION_NACK: u8 = 2; // This version will be NACKed
    const EVENTGROUP_ID: u16 = 0x0001;
    const EVENT_ID: u16 = 0x8001;

    let ack_count = Arc::new(AtomicUsize::new(0));
    let nack_count = Arc::new(AtomicUsize::new(0));
    let events_sent = Arc::new(AtomicUsize::new(0));
    let ack_count_server = Arc::clone(&ack_count);
    let nack_count_server = Arc::clone(&nack_count);
    let events_sent_server = Arc::clone(&events_sent);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Wire-level server that offers two services, ACKs one, NACKs the other
    sim.host("wire_server", move || {
        let ack_count = Arc::clone(&ack_count_server);
        let nack_count = Arc::clone(&nack_count_server);
        let events_sent = Arc::clone(&events_sent_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            // RPC socket for sending events (v1 endpoint)
            let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?;

            let mut buf = [0u8; 1500];
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);
            let mut v1_client_endpoint: Option<SocketAddr> = None;
            let mut next_unicast_session_id = 1u16;
            let mut next_multicast_session_id = 1u16;
            let mut first_offer = true;

            while tokio::time::Instant::now() < deadline {
                // Send offers periodically
                if last_offer.elapsed() >= Duration::from_millis(500) {
                    let offer_v1 = build_sd_offer_with_session(
                        SERVICE_ID, INSTANCE_ID, VERSION_ACK, 0, my_ip, 30509, 3600,
                        next_multicast_session_id, first_offer, false,
                    );
                    next_multicast_session_id += 1;
                    sd_socket.send_to(&offer_v1, sd_multicast).await?;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let offer_v2 = build_sd_offer_with_session(
                        SERVICE_ID, INSTANCE_ID, VERSION_NACK, 0, my_ip, 30510, 3600,
                        next_multicast_session_id, first_offer, false,
                    );
                    next_multicast_session_id = next_multicast_session_id.wrapping_add(1);
                    sd_socket.send_to(&offer_v2, sd_multicast).await?;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    eprintln!("[wire_server] Sent offers for v1 and v2");
                    last_offer = tokio::time::Instant::now();
                    first_offer = false;
                }

                // Check for incoming messages
                if let Ok(Ok((len, from))) = tokio::time::timeout(
                    Duration::from_millis(100),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            // SubscribeEventgroup = 0x06
                            if entry.entry_type as u8 == 0x06 && entry.service_id == SERVICE_ID {
                                eprintln!(
                                    "[wire_server] Received subscribe for version {} eventgroup {:04x}",
                                    entry.major_version, entry.eventgroup_id
                                );

                                if entry.major_version == VERSION_ACK {
                                    // Extract client endpoint from subscribe options
                                    if let Some((client_ip, client_port)) =
                                        extract_client_udp_endpoint(entry, &sd_msg.options)
                                    {
                                        v1_client_endpoint = Some(SocketAddr::from((client_ip, client_port)));
                                        eprintln!("[wire_server] v1 client endpoint: {:?}", v1_client_endpoint);
                                    }

                                    // ACK for v1
                                    let ack = build_sd_subscribe_ack_with_session(
                                        entry.service_id,
                                        entry.instance_id,
                                        entry.major_version,
                                        entry.eventgroup_id,
                                        entry.ttl,
                                        next_unicast_session_id,
                                    );
                                    next_unicast_session_id += 1;
                                    sd_socket.send_to(&ack, from).await?;
                                    tokio::time::sleep(Duration::from_millis(10)).await;
                                    ack_count.fetch_add(1, Ordering::SeqCst);
                                    eprintln!("[wire_server] Sent ACK for v1");

                                    // Send event to v1 subscriber
                                    if let Some(endpoint) = v1_client_endpoint {
                                        let payload = b"v1_event_data";
                                        let event = build_event_notification(
                                            SERVICE_ID,
                                            EVENT_ID,
                                            VERSION_ACK,
                                            1, // session_id
                                            payload,
                                        );
                                        rpc_socket.send_to(&event, endpoint).await?;
                                        events_sent.fetch_add(1, Ordering::SeqCst);
                                        eprintln!("[wire_server] Sent event to v1 subscriber at {:?}", endpoint);
                                    }
                                } else if entry.major_version == VERSION_NACK {
                                    // NACK for v2
                                    let nack = build_sd_subscribe_nack(
                                        entry.service_id,
                                        entry.instance_id,
                                        entry.major_version,
                                        entry.eventgroup_id,
                                        next_unicast_session_id,
                                    );
                                    next_unicast_session_id += 1;
                                    sd_socket.send_to(&nack, from).await?;
                                    tokio::time::sleep(Duration::from_millis(10)).await;
                                    nack_count.fetch_add(1, Ordering::SeqCst);
                                    eprintln!("[wire_server] Sent NACK for v2");
                                }
                            }
                        }
                    }
                }
            }

            Ok(())
        }
    });

    // Client flags to track subscription results
    let v1_subscribed = Arc::new(AtomicBool::new(false));
    let v2_rejected = Arc::new(AtomicBool::new(false));
    let v1_event_received = Arc::new(AtomicBool::new(false));
    let v1_sub_clone = Arc::clone(&v1_subscribed);
    let v2_rej_clone = Arc::clone(&v2_rejected);
    let v1_event_clone = Arc::clone(&v1_event_received);

    // Client using our library
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Discover service v1
        let proxy_v1 = runtime
            .find(SERVICE_ID)
            .major_version(VERSION_ACK)
            .instance(InstanceId::Id(INSTANCE_ID));
        let proxy_v1 = tokio::time::timeout(Duration::from_secs(5), proxy_v1)
            .await
            .expect("Discovery timeout for v1")
            .expect("Service v1 available");
        eprintln!("[client] Service v1 discovered");

        // Discover service v2
        let proxy_v2 = runtime
            .find(SERVICE_ID)
            .major_version(VERSION_NACK)
            .instance(InstanceId::Id(INSTANCE_ID));
        let proxy_v2 = tokio::time::timeout(Duration::from_secs(5), proxy_v2)
            .await
            .expect("Discovery timeout for v2")
            .expect("Service v2 available");
        eprintln!("[client] Service v2 discovered");

        // Subscribe to v1 - should succeed
        let eventgroup = EventgroupId::new(EVENTGROUP_ID).unwrap();
        let mut subscription_v1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_v1
                .new_subscription()
                .eventgroup(eventgroup)
                .subscribe(),
        )
        .await
        .expect("Subscribe timeout for v1")
        .expect("Subscribe should succeed for v1");

        eprintln!("[client] v1 subscription succeeded (expected)");
        v1_sub_clone.store(true, Ordering::SeqCst);

        // Wait for event from v1 subscription
        match tokio::time::timeout(Duration::from_secs(5), subscription_v1.next()).await {
            Ok(Some(event)) => {
                eprintln!(
                    "[client] Received v1 event: {:?}",
                    String::from_utf8_lossy(&event.payload)
                );
                assert_eq!(
                    event.payload.as_ref(),
                    b"v1_event_data",
                    "Event payload should match"
                );
                v1_event_clone.store(true, Ordering::SeqCst);
            }
            Ok(None) => {
                panic!("[client] v1 subscription stream ended unexpectedly");
            }
            Err(_) => {
                panic!("[client] Timeout waiting for v1 event");
            }
        }

        // Subscribe to v2 - should fail with NACK
        let result_v2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_v2
                .new_subscription()
                .eventgroup(eventgroup)
                .subscribe(),
        )
        .await
        .expect("Subscribe timeout for v2");

        match result_v2 {
            Ok(_subscription) => {
                panic!("[client] v2 subscription succeeded unexpectedly (expected NACK)");
            }
            Err(e) => {
                eprintln!("[client] v2 subscription rejected as expected: {:?}", e);
                v2_rej_clone.store(true, Ordering::SeqCst);
            }
        }

        // Wait for event from v1 subscription
        match tokio::time::timeout(Duration::from_secs(5), subscription_v1.next()).await {
            Ok(Some(event)) => {
                eprintln!(
                    "[client] Received v1 event: {:?}",
                    String::from_utf8_lossy(&event.payload)
                );
                assert_eq!(
                    event.payload.as_ref(),
                    b"v1_event_data",
                    "Event payload should match"
                );
                v1_event_clone.store(true, Ordering::SeqCst);
            }
            Ok(None) => {
                panic!("[client] v1 subscription stream ended unexpectedly");
            }
            Err(_) => {
                panic!("[client] Timeout waiting for v1 event");
            }
        }

        Ok(())
    });

    sim.run().unwrap();

    // Verify results
    assert!(
        v1_subscribed.load(Ordering::SeqCst),
        "Subscription to v1 should have succeeded"
    );
    assert!(
        v2_rejected.load(Ordering::SeqCst),
        "Subscription to v2 should have been rejected (NACK)"
    );
    assert!(
        ack_count.load(Ordering::SeqCst) >= 1,
        "Server should have sent at least 1 ACK"
    );
    assert!(
        nack_count.load(Ordering::SeqCst) >= 1,
        "Server should have sent at least 1 NACK"
    );
    assert!(
        v1_event_received.load(Ordering::SeqCst),
        "Client should have received event on v1 subscription"
    );
    assert!(
        events_sent.load(Ordering::SeqCst) >= 1,
        "Server should have sent at least 1 event"
    );
}

/// Test that events are delivered through an established subscription.
///
/// Setup:
/// - Wire-level server offers a service
/// - Client subscribes, server ACKs
/// - Server sends event notification to client's endpoint
/// - Verify: client receives the event with correct payload
///
/// This tests the full pub/sub flow with a wire-level server.
#[test_log::test]
fn events_received_after_subscribe_ack() {
    covers!(feat_req_recentipsd_1137);

    const SERVICE_ID: u16 = 0x5678;
    const INSTANCE_ID: u16 = 0x0001;
    const MAJOR_VERSION: u8 = 1;
    const EVENTGROUP_ID: u16 = 0x0001;
    const EVENT_ID: u16 = 0x8001; // Event IDs >= 0x8000

    let event_count = Arc::new(AtomicUsize::new(0));
    let event_count_server = Arc::clone(&event_count);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Wire-level server that offers a service, ACKs subscriptions, and sends events
    sim.host("wire_server", move || {
        let event_count = Arc::clone(&event_count_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            // RPC socket to send events (same port as advertised in offer)
            let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?;

            let mut buf = [0u8; 1500];
            let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);
            let mut client_endpoint: Option<SocketAddr> = None;
            let mut events_sent = 0;

            let mut next_unicast_session_id = 1u16;
            let mut next_multicast_session_id = 1u16;

            while tokio::time::Instant::now() < deadline {
                // Send offers periodically
                if last_offer.elapsed() >= Duration::from_millis(500) {
                    // Build offer
                    let offer = build_sd_offer_with_session(
                        SERVICE_ID,
                        INSTANCE_ID,
                        MAJOR_VERSION,
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
                    eprintln!("[wire_server] Sent offer");
                    last_offer = tokio::time::Instant::now();
                }

                // Check for incoming SD messages
                if let Ok(Ok((len, from))) =
                    tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                        .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            // SubscribeEventgroup = 0x06
                            if entry.entry_type as u8 == 0x06 && entry.service_id == SERVICE_ID {
                                eprintln!(
                                    "[wire_server] Received subscribe for eventgroup {:04x}",
                                    entry.eventgroup_id
                                );

                                // Extract client's endpoint from the subscribe entry options
                                if let Some((client_ip, client_port)) =
                                    extract_client_udp_endpoint(entry, &sd_msg.options)
                                {
                                    client_endpoint =
                                        Some(SocketAddr::from((client_ip, client_port)));
                                    eprintln!(
                                        "[wire_server] Client endpoint: {:?}",
                                        client_endpoint
                                    );
                                }

                                // ACK the subscription
                                let ack = build_sd_subscribe_ack_with_session(
                                    entry.service_id,
                                    entry.instance_id,
                                    entry.major_version,
                                    entry.eventgroup_id,
                                    entry.ttl,
                                    next_unicast_session_id,
                                );
                                next_unicast_session_id += 1;
                                sd_socket.send_to(&ack, from).await?;
                                tokio::time::sleep(Duration::from_millis(100)).await;
                                eprintln!("[wire_server] Sent ACK");
                            }
                        }
                    }
                }

                // If we have a client endpoint, send events
                if let Some(endpoint) = client_endpoint {
                    if events_sent < 3 {
                        let payload = format!("event_payload_{}", events_sent);
                        let event = build_event_notification(
                            SERVICE_ID,
                            EVENT_ID,
                            MAJOR_VERSION,
                            (events_sent + 1) as u16, // session_id
                            payload.as_bytes(),
                        );
                        rpc_socket.send_to(&event, endpoint).await?;
                        eprintln!("[wire_server] Sent event {} to {:?}", events_sent, endpoint);
                        event_count.fetch_add(1, Ordering::SeqCst);
                        events_sent += 1;
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                }
            }

            Ok(())
        }
    });

    // Client flags
    let events_received = Arc::new(AtomicUsize::new(0));
    let events_received_clone = Arc::clone(&events_received);

    // Client using our library
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Discover service
        let proxy = runtime
            .find(SERVICE_ID)
            .major_version(MAJOR_VERSION)
            .instance(InstanceId::Id(INSTANCE_ID));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");
        eprintln!("[client] Service discovered");

        // Subscribe
        let eventgroup = EventgroupId::new(EVENTGROUP_ID).unwrap();
        let mut subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");
        eprintln!("[client] Subscription established");

        // Receive events
        for i in 0..3 {
            match tokio::time::timeout(Duration::from_secs(5), subscription.next()).await {
                Ok(Some(event)) => {
                    let expected_payload = format!("event_payload_{}", i);
                    eprintln!(
                        "[client] Received event {}: {:?}",
                        i,
                        String::from_utf8_lossy(&event.payload)
                    );
                    assert_eq!(
                        event.payload.as_ref(),
                        expected_payload.as_bytes(),
                        "Event {} payload should match",
                        i
                    );
                    events_received_clone.fetch_add(1, Ordering::SeqCst);
                }
                Ok(None) => {
                    panic!("[client] Subscription stream ended unexpectedly");
                }
                Err(_) => {
                    panic!("[client] Timeout waiting for event {}", i);
                }
            }
        }

        eprintln!("[client] All events received successfully");
        Ok(())
    });

    sim.run().unwrap();

    // Verify results
    assert_eq!(
        events_received.load(Ordering::SeqCst),
        3,
        "Client should have received 3 events"
    );
    assert_eq!(
        event_count.load(Ordering::SeqCst),
        3,
        "Server should have sent 3 events"
    );
}

/// Test that a multi-eventgroup subscription fails if ANY eventgroup is NACKed.
///
/// # Scenario
/// - Server offers service with two eventgroups (EG1 and EG2)
/// - Client creates ONE subscription to BOTH eventgroups
/// - Server ACKs EG1 but NACKs EG2
/// - The entire subscription should fail (not partially succeed)
///
/// This tests the "all or nothing" semantics for multi-eventgroup subscriptions.
#[test_log::test]
fn multi_eventgroup_subscription_fails_if_one_nacked() {
    covers!(feat_req_recentipsd_1137);

    const SERVICE_ID: u16 = 0x1234;
    const INSTANCE_ID: u16 = 0x0001;
    const MAJOR_VERSION: u8 = 1;
    const EVENTGROUP_ACK: u16 = 0x0001; // This eventgroup will be ACKed
    const EVENTGROUP_NACK: u16 = 0x0002; // This eventgroup will be NACKed

    let ack_count = Arc::new(AtomicUsize::new(0));
    let nack_count = Arc::new(AtomicUsize::new(0));
    let ack_count_server = Arc::clone(&ack_count);
    let nack_count_server = Arc::clone(&nack_count);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Wire-level server that offers one service with two eventgroups
    // ACKs subscriptions to EG1, NACKs subscriptions to EG2
    sim.host("wire_server", move || {
        let ack_count = Arc::clone(&ack_count_server);
        let nack_count = Arc::clone(&nack_count_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            // Build offer for the service (multicast, not unicast)
            let offer_bytes = build_sd_offer_with_session(
                SERVICE_ID,
                INSTANCE_ID,
                MAJOR_VERSION,
                0x00000001,
                my_ip,
                30509,
                0xFFFFFF,
                1,     // session_id
                true,  // reboot_flag
                false, // unicast_flag (multicast offer)
            );

            let mut buf = [0u8; 65535];
            let mut offer_sent = false;
            let mut next_unicast_session_id = 1u16;

            loop {
                let (len, from) = sd_socket.recv_from(&mut buf).await?;

                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        match entry.entry_type {
                            recentip::wire::SdEntryType::FindService => {
                                // Send offer on first FindService
                                if !offer_sent {
                                    sd_socket.send_to(&offer_bytes, sd_multicast).await?;
                                    offer_sent = true;
                                    eprintln!("[wire_server] Sent offer for service");
                                }
                            }
                            recentip::wire::SdEntryType::SubscribeEventgroup => {
                                let eventgroup_id = entry.eventgroup_id;
                                eprintln!(
                                    "[wire_server] Received subscribe for eventgroup 0x{:04X}",
                                    eventgroup_id
                                );

                                if eventgroup_id == EVENTGROUP_ACK {
                                    // ACK this eventgroup
                                    let ack_bytes = build_sd_subscribe_ack_with_session(
                                        SERVICE_ID,
                                        INSTANCE_ID,
                                        MAJOR_VERSION,
                                        EVENTGROUP_ACK,
                                        0xFFFFFF,
                                        next_unicast_session_id,
                                    );
                                    next_unicast_session_id += 1;
                                    sd_socket.send_to(&ack_bytes, from).await?;
                                    tokio::time::sleep(Duration::from_millis(100)).await;
                                    ack_count.fetch_add(1, Ordering::SeqCst);
                                    eprintln!("[wire_server] Sent ACK for EG1");
                                } else if eventgroup_id == EVENTGROUP_NACK {
                                    // NACK this eventgroup
                                    let nack_bytes = build_sd_subscribe_nack(
                                        SERVICE_ID,
                                        INSTANCE_ID,
                                        MAJOR_VERSION,
                                        EVENTGROUP_NACK,
                                        next_unicast_session_id,
                                    );
                                    next_unicast_session_id += 1;
                                    sd_socket.send_to(&nack_bytes, from).await?;
                                    nack_count.fetch_add(1, Ordering::SeqCst);
                                    eprintln!("[wire_server] Sent NACK for EG2");
                                    tokio::time::sleep(Duration::from_millis(100)).await;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    });

    // Client flags
    let subscription_failed = Arc::new(AtomicBool::new(false));
    let subscription_failed_clone = Arc::clone(&subscription_failed);

    // Client using our library
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Discover the service
        let proxy = runtime
            .find(SERVICE_ID)
            .major_version(MAJOR_VERSION)
            .instance(InstanceId::Id(INSTANCE_ID));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");
        eprintln!("[client] Service discovered");

        // Subscribe to BOTH eventgroups in a single subscription
        let eg1 = EventgroupId::new(EVENTGROUP_ACK).unwrap();
        let eg2 = EventgroupId::new(EVENTGROUP_NACK).unwrap();

        let result = tokio::time::timeout(
            Duration::from_secs(5),
            proxy
                .new_subscription()
                .eventgroup(eg1)
                .eventgroup(eg2)
                .subscribe(),
        )
        .await
        .expect("Subscribe timeout");

        match result {
            Ok(_subscription) => {
                panic!("[client] Multi-eventgroup subscription succeeded unexpectedly (expected failure due to NACK on EG2)");
            }
            Err(e) => {
                eprintln!(
                    "[client] Multi-eventgroup subscription failed as expected: {:?}",
                    e
                );
                subscription_failed_clone.store(true, Ordering::SeqCst);
            }
        }

        tokio::time::sleep(Duration::from_secs(1)).await;

        Ok(())
    });

    sim.run().unwrap();

    // Verify results
    assert!(
        subscription_failed.load(Ordering::SeqCst),
        "Multi-eventgroup subscription should have failed when one eventgroup was NACKed"
    );
    // The server must have sent at least one NACK (for EG2)
    // Note: It may not have sent an ACK if the NACK arrived before the other subscribe
    assert!(
        nack_count.load(Ordering::SeqCst) >= 1,
        "Server should have sent at least 1 NACK (for EG2)"
    );
}

/// Test that a multi-eventgroup subscription over TCP fails if ANY eventgroup is NACKed.
///
/// Same as `multi_eventgroup_subscription_fails_if_one_nacked` but uses TCP transport.
#[test_log::test]
fn multi_eventgroup_subscription_fails_if_one_nacked_tcp() {
    covers!(feat_req_recentipsd_1137);

    const SERVICE_ID: u16 = 0x1234;
    const INSTANCE_ID: u16 = 0x0001;
    const MAJOR_VERSION: u8 = 1;
    const EVENTGROUP_ACK: u16 = 0x0001; // This eventgroup will be ACKed
    const EVENTGROUP_NACK: u16 = 0x0002; // This eventgroup will be NACKed
    const TCP_PORT: u16 = 30509;

    let ack_count = Arc::new(AtomicUsize::new(0));
    let nack_count = Arc::new(AtomicUsize::new(0));
    let ack_count_server = Arc::clone(&ack_count);
    let nack_count_server = Arc::clone(&nack_count);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Wire-level server that offers one service with two eventgroups via TCP
    // ACKs subscriptions to EG1, NACKs subscriptions to EG2
    sim.host("wire_server", move || {
        let ack_count = Arc::clone(&ack_count_server);
        let nack_count = Arc::clone(&nack_count_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            // TCP listener for the service (needed for client to connect before subscribing)
            let tcp_listener =
                turmoil::net::TcpListener::bind(format!("0.0.0.0:{}", TCP_PORT)).await?;

            // Build offer for the service with TCP endpoint (multicast, not unicast)
            let offer_bytes = build_sd_offer_tcp_only(
                SERVICE_ID,
                INSTANCE_ID,
                MAJOR_VERSION,
                0x00000001,
                my_ip,
                TCP_PORT,
                0xFFFFFF,
                1,     // session_id
                true,  // reboot_flag
                false, // unicast_flag (multicast offer)
            );

            let mut buf = [0u8; 65535];
            let mut offer_sent = false;
            let mut next_unicast_session_id = 1u16;

            // Spawn TCP accept task (just accept connections, don't need to do anything)
            tokio::spawn(async move {
                loop {
                    if let Ok((stream, addr)) = tcp_listener.accept().await {
                        eprintln!("[wire_server] TCP connection accepted from {}", addr);
                        // Keep stream alive by holding it
                        let _ = stream;
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    }
                }
            });

            loop {
                let (len, from) = sd_socket.recv_from(&mut buf).await?;

                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        match entry.entry_type {
                            recentip::wire::SdEntryType::FindService => {
                                // Send offer on first FindService
                                if !offer_sent {
                                    sd_socket.send_to(&offer_bytes, sd_multicast).await?;
                                    offer_sent = true;
                                    eprintln!("[wire_server] Sent TCP offer for service");
                                }
                            }
                            recentip::wire::SdEntryType::SubscribeEventgroup => {
                                let eventgroup_id = entry.eventgroup_id;
                                eprintln!(
                                    "[wire_server] Received subscribe for eventgroup 0x{:04X}",
                                    eventgroup_id
                                );

                                if eventgroup_id == EVENTGROUP_ACK {
                                    // ACK this eventgroup
                                    let ack_bytes = build_sd_subscribe_ack_with_session(
                                        SERVICE_ID,
                                        INSTANCE_ID,
                                        MAJOR_VERSION,
                                        EVENTGROUP_ACK,
                                        0xFFFFFF,
                                        next_unicast_session_id,
                                    );
                                    next_unicast_session_id += 1;
                                    sd_socket.send_to(&ack_bytes, from).await?;
                                    tokio::time::sleep(Duration::from_millis(10)).await;
                                    ack_count.fetch_add(1, Ordering::SeqCst);
                                    eprintln!("[wire_server] Sent ACK for EG1");
                                } else if eventgroup_id == EVENTGROUP_NACK {
                                    // NACK this eventgroup
                                    let nack_bytes = build_sd_subscribe_nack(
                                        SERVICE_ID,
                                        INSTANCE_ID,
                                        MAJOR_VERSION,
                                        EVENTGROUP_NACK,
                                        next_unicast_session_id,
                                    );
                                    next_unicast_session_id += 1;
                                    sd_socket.send_to(&nack_bytes, from).await?;
                                    nack_count.fetch_add(1, Ordering::SeqCst);
                                    tokio::time::sleep(Duration::from_millis(10)).await;
                                    eprintln!("[wire_server] Sent NACK for EG2");
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    });

    // Client flags
    let subscription_failed = Arc::new(AtomicBool::new(false));
    let subscription_failed_clone = Arc::clone(&subscription_failed);

    // Client using our library
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Discover the service
        let proxy = runtime
            .find(SERVICE_ID)
            .major_version(MAJOR_VERSION)
            .instance(InstanceId::Id(INSTANCE_ID));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");
        eprintln!("[client] TCP service discovered");

        // Subscribe to BOTH eventgroups in a single subscription
        let eg1 = EventgroupId::new(EVENTGROUP_ACK).unwrap();
        let eg2 = EventgroupId::new(EVENTGROUP_NACK).unwrap();

        let result = tokio::time::timeout(
            Duration::from_secs(5),
            proxy
                .new_subscription()
                .eventgroup(eg1)
                .eventgroup(eg2)
                .subscribe(),
        )
        .await
        .expect("Subscribe timeout");

        match result {
            Ok(_subscription) => {
                panic!("[client] Multi-eventgroup TCP subscription succeeded unexpectedly (expected failure due to NACK on EG2)");
            }
            Err(e) => {
                eprintln!(
                    "[client] Multi-eventgroup TCP subscription failed as expected: {:?}",
                    e
                );
                subscription_failed_clone.store(true, Ordering::SeqCst);
            }
        }

        Ok(())
    });

    sim.run().unwrap();

    // Verify results
    assert!(
        subscription_failed.load(Ordering::SeqCst),
        "Multi-eventgroup TCP subscription should have failed when one eventgroup was NACKed"
    );
    assert!(
        nack_count.load(Ordering::SeqCst) >= 1,
        "Server should have sent at least 1 NACK (for EG2)"
    );
}
