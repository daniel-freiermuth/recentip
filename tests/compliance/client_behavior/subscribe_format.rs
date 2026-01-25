//! Subscribe Format Tests (Client Under Test)
//!
//! Tests that verify client sends correct SubscribeEventgroup messages.
//!
//! # Test Setup
//! - Library: Acts as **client** (discovers and subscribes)
//! - Raw socket: Acts as **server** (sends offers, verifies subscribe format)
//!
//! # Requirements Covered
//! - feat_req_recentipsd_431: Subscribe message format
//! - feat_req_recentipsd_631: Subscriptions triggered by OfferService entries
//! - feat_req_recentip_324: Client adapts to available transport

use super::helpers::{
    build_sd_offer_dual_stack_with_session, build_sd_offer_tcp_only, build_sd_offer_with_session,
    build_sd_subscribe_ack_with_session, covers, parse_sd_message, TEST_SERVICE_ID,
};
use recentip::prelude::*;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Verify subscribe message format for initial and subsequent subscribes
/// triggered by cyclic offers.
///
/// Setup:
/// - Wire-level server offers UDP-only service and sends cyclic offers
/// - Client (our library) subscribes and stays alive
/// - Server verifies format of initial subscribe AND subsequent re-subscribes
///
/// Per feat_req_recentipsd_631: Subscriptions triggered by OfferService entries
#[test_log::test]
fn subscribe_format_udp_only_cyclic_offers() {
    covers!(feat_req_recentipsd_431, feat_req_recentipsd_631);

    let subscribe_count = Arc::new(AtomicUsize::new(0));
    let subscribe_count_server = Arc::clone(&subscribe_count);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Wire-level server that sends offers and inspects subscribes
    sim.host("wire_server", move || {
        let subscribe_count = Arc::clone(&subscribe_count_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            let mut buf = [0u8; 1500];

            // Send offers cyclically and collect subscribes
            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);
            let mut next_multicast_session_id: u16 = 1;
            let mut next_unicast_session_id: u16 = 1;

            while tokio::time::Instant::now() < deadline {
                // Send offer every 1 second (simulating cyclic offer with short TTL)
                if last_offer.elapsed() >= Duration::from_millis(1000) {
                    let offer = build_sd_offer_with_session(
                        0x1234,
                        0x0001,
                        1,
                        0,
                        my_ip,
                        30509,
                        3,
                        next_multicast_session_id,
                        next_multicast_session_id == 1, // reboot_flag only on first
                        false,                          // unicast_flag (multicast)
                    );
                    next_multicast_session_id += 1;
                    sd_socket.send_to(&offer, sd_multicast).await?;
                    eprintln!("[wire_server] Sent offer");
                    last_offer = tokio::time::Instant::now();
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
                            if entry.entry_type as u8 == 0x06 && entry.service_id == 0x1234 {
                                let count = subscribe_count.fetch_add(1, Ordering::SeqCst) + 1;
                                eprintln!(
                                    "[wire_server] Received subscribe #{}: eventgroup={:04x}, ttl={}",
                                    count, entry.eventgroup_id, entry.ttl
                                );

                                // Verify subscribe has correct format
                                assert_eq!(entry.service_id, 0x1234, "Service ID mismatch");
                                assert_eq!(entry.instance_id, 0x0001, "Instance ID mismatch");
                                assert!(entry.ttl > 0, "Subscribe TTL should be > 0");

                                // Check that subscribe includes endpoint option
                                // The client should include its endpoint in the subscribe
                                let has_udp_endpoint = sd_msg.get_udp_endpoint(entry).is_some();
                                let has_tcp_endpoint = sd_msg.get_tcp_endpoint(entry).is_some();
                                assert!(
                                    has_udp_endpoint || has_tcp_endpoint,
                                    "Subscribe #{} should include endpoint option (UDP={}, TCP={})",
                                    count, has_udp_endpoint, has_tcp_endpoint
                                );

                                // For UDP-only offer, client should send UDP endpoint
                                assert!(
                                    has_udp_endpoint,
                                    "Subscribe #{} to UDP-only service should include UDP endpoint",
                                    count
                                );

                                // Send ACK
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
                            }
                        }
                    }
                }
            }

            Ok(())
        }
    });

    // Client using our library
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .subscribe_ttl(5) // Short TTL so we can see renewals
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Discover and subscribe
        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        eprintln!("[client] Service discovered");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        eprintln!("[client] Subscribed, staying alive for cyclic offers...");

        // Stay alive to receive cyclic offers and send renewals
        tokio::time::sleep(Duration::from_secs(6)).await;

        Ok(())
    });

    sim.run().unwrap();

    let count = subscribe_count.load(Ordering::SeqCst);
    eprintln!("Total subscribes received: {}", count);
    assert!(
        count >= 2,
        "Should receive at least initial subscribe + 1 renewal, got {}",
        count
    );
}

/// Verify subscribe message format when server offers TCP only
///
/// Setup:
/// - Wire-level server offers TCP-only service
/// - Client (our library, with TCP preference) subscribes
/// - Server verifies subscribe includes TCP endpoint option
#[test_log::test]
fn subscribe_format_tcp_only_cyclic_offers() {
    covers!(feat_req_recentipsd_431, feat_req_recentipsd_631);

    let subscribe_count = Arc::new(AtomicUsize::new(0));
    let subscribe_count_server = Arc::clone(&subscribe_count);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Wire-level server offering TCP only
    sim.host("wire_server", move || {
        let subscribe_count = Arc::clone(&subscribe_count_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            // Start TCP listener on advertised port (required for client to connect before subscribing)
            let tcp_listener = turmoil::net::TcpListener::bind("0.0.0.0:30509").await?;
            tokio::spawn(async move {
                loop {
                    if let Ok((stream, peer)) = tcp_listener.accept().await {
                        eprintln!("[wire_server] Accepted TCP connection from {}", peer);
                        // Keep connection alive by holding the stream
                        tokio::spawn(async move {
                            let _stream = stream;
                            // Hold connection open for the test duration
                            tokio::time::sleep(Duration::from_secs(30)).await;
                        });
                    }
                }
            });

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            let mut buf = [0u8; 1500];

            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);
            let mut next_multicast_session_id: u16 = 1;
            let mut next_unicast_session_id: u16 = 1;

            while tokio::time::Instant::now() < deadline {
                if last_offer.elapsed() >= Duration::from_millis(1000) {
                    let offer = build_sd_offer_tcp_only(
                        0x1234,
                        0x0001,
                        1,
                        0,
                        my_ip,
                        30509,
                        3,
                        next_multicast_session_id,
                        next_multicast_session_id == 1, // reboot_flag only on first
                        false,                          // unicast_flag (multicast)
                    );
                    next_multicast_session_id += 1;
                    sd_socket.send_to(&offer, sd_multicast).await?;
                    eprintln!("[wire_server] Sent TCP-only offer");
                    last_offer = tokio::time::Instant::now();
                }

                if let Ok(Ok((len, from))) =
                    tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                        .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            if entry.entry_type as u8 == 0x06 && entry.service_id == 0x1234 {
                                let count = subscribe_count.fetch_add(1, Ordering::SeqCst) + 1;
                                eprintln!(
                                    "[wire_server] Received subscribe #{}: eventgroup={:04x}",
                                    count, entry.eventgroup_id
                                );

                                // For TCP-only offer, client MUST send TCP endpoint
                                let has_tcp_endpoint = sd_msg.get_tcp_endpoint(entry).is_some();
                                assert!(
                                    has_tcp_endpoint,
                                    "Subscribe #{} to TCP-only service MUST include TCP endpoint",
                                    count
                                );

                                // Send ACK
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
                            }
                        }
                    }
                }
            }

            Ok(())
        }
    });

    // Client with TCP preference
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .preferred_transport(recentip::Transport::Tcp)
            .subscribe_ttl(5)
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

        eprintln!("[client] Service discovered via TCP");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        eprintln!("[client] Subscribed via TCP, staying alive...");
        tokio::time::sleep(Duration::from_secs(6)).await;

        Ok(())
    });

    sim.run().unwrap();

    let count = subscribe_count.load(Ordering::SeqCst);
    assert!(
        count >= 2,
        "Should receive at least initial subscribe + 1 renewal, got {}",
        count
    );
}

/// Verify subscribe message format when server offers both UDP and TCP
/// Client prefers UDP - should send UDP endpoint
#[test_log::test]
fn subscribe_format_dual_stack_client_prefers_udp() {
    covers!(feat_req_recentipsd_431, feat_req_recentipsd_631);

    let subscribe_count = Arc::new(AtomicUsize::new(0));
    let subscribe_count_server = Arc::clone(&subscribe_count);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("wire_server", move || {
        let subscribe_count = Arc::clone(&subscribe_count_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            let mut buf = [0u8; 1500];

            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);
            let mut next_multicast_session_id: u16 = 1;
            let mut next_unicast_session_id: u16 = 1;

            while tokio::time::Instant::now() < deadline {
                if last_offer.elapsed() >= Duration::from_millis(1000) {
                    let offer = build_sd_offer_dual_stack_with_session(
                        0x1234,
                        0x0001,
                        1,
                        0,
                        my_ip,
                        30509,
                        30510,
                        3,
                        next_multicast_session_id,
                        next_multicast_session_id == 1, // reboot_flag only on first
                        false,                          // unicast_flag (multicast)
                    );
                    next_multicast_session_id += 1;
                    sd_socket.send_to(&offer, sd_multicast).await?;
                    eprintln!("[wire_server] Sent dual-stack offer");
                    last_offer = tokio::time::Instant::now();
                }

                if let Ok(Ok((len, from))) = tokio::time::timeout(
                    Duration::from_millis(100),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            if entry.entry_type as u8 == 0x06 && entry.service_id == 0x1234 {
                                let count = subscribe_count.fetch_add(1, Ordering::SeqCst) + 1;

                                let has_udp = sd_msg.get_udp_endpoint(entry).is_some();
                                let has_tcp = sd_msg.get_tcp_endpoint(entry).is_some();
                                eprintln!(
                                    "[wire_server] Subscribe #{}: UDP={}, TCP={}",
                                    count, has_udp, has_tcp
                                );

                                // Client prefers UDP, so should send UDP endpoint
                                assert!(
                                    has_udp,
                                    "Subscribe #{} from UDP-preferring client should include UDP endpoint",
                                    count
                                );

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
                            }
                        }
                    }
                }
            }

            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Default config prefers UDP
        let runtime = recentip::configure()
            .preferred_transport(recentip::Transport::Udp)
            .subscribe_ttl(5)
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

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        tokio::time::sleep(Duration::from_secs(6)).await;
        Ok(())
    });

    sim.run().unwrap();

    let count = subscribe_count.load(Ordering::SeqCst);
    assert!(
        count >= 2,
        "Should receive at least 2 subscribes, got {}",
        count
    );
}

/// Verify subscribe message format when server offers both UDP and TCP
/// Client prefers TCP - should send TCP endpoint
#[test_log::test]
fn subscribe_format_dual_stack_client_prefers_tcp() {
    covers!(feat_req_recentipsd_431, feat_req_recentipsd_631);

    let subscribe_count = Arc::new(AtomicUsize::new(0));
    let subscribe_count_server = Arc::clone(&subscribe_count);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("wire_server", move || {
        let subscribe_count = Arc::clone(&subscribe_count_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            // Start TCP listener on advertised TCP port (30510 for dual-stack)
            // Required for client to connect before subscribing per feat_req_recentipsd_767
            let tcp_listener = turmoil::net::TcpListener::bind("0.0.0.0:30510").await?;
            tokio::spawn(async move {
                loop {
                    if let Ok((stream, peer)) = tcp_listener.accept().await {
                        eprintln!("[wire_server] Accepted TCP connection from {}", peer);
                        // Keep connection alive by holding the stream
                        tokio::spawn(async move {
                            let _stream = stream;
                            // Hold connection open for the test duration
                            tokio::time::sleep(Duration::from_secs(30)).await;
                        });
                    }
                }
            });

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            let mut buf = [0u8; 1500];

            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);
            let mut next_multicast_session_id: u16 = 1;
            let mut next_unicast_session_id: u16 = 1;

            while tokio::time::Instant::now() < deadline {
                if last_offer.elapsed() >= Duration::from_millis(1000) {
                    let offer = build_sd_offer_dual_stack_with_session(
                        0x1234,
                        0x0001,
                        1,
                        0,
                        my_ip,
                        30509,
                        30510,
                        3,
                        next_multicast_session_id,
                        next_multicast_session_id == 1, // reboot_flag only on first
                        false,                          // unicast_flag (multicast)
                    );
                    next_multicast_session_id += 1;
                    sd_socket.send_to(&offer, sd_multicast).await?;
                    eprintln!("[wire_server] Sent dual-stack offer");
                    last_offer = tokio::time::Instant::now();
                }

                if let Ok(Ok((len, from))) = tokio::time::timeout(
                    Duration::from_millis(100),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            if entry.entry_type as u8 == 0x06 && entry.service_id == 0x1234 {
                                let count = subscribe_count.fetch_add(1, Ordering::SeqCst) + 1;

                                let has_udp = sd_msg.get_udp_endpoint(entry).is_some();
                                let has_tcp = sd_msg.get_tcp_endpoint(entry).is_some();
                                eprintln!(
                                    "[wire_server] Subscribe #{}: UDP={}, TCP={}",
                                    count, has_udp, has_tcp
                                );

                                // Client prefers TCP, so should send TCP endpoint
                                assert!(
                                    has_tcp,
                                    "Subscribe #{} from TCP-preferring client should include TCP endpoint",
                                    count
                                );

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
                            }
                        }
                    }
                }
            }

            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .preferred_transport(recentip::Transport::Tcp)
            .subscribe_ttl(5)
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

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        tokio::time::sleep(Duration::from_secs(6)).await;
        Ok(())
    });

    sim.run().unwrap();

    let count = subscribe_count.load(Ordering::SeqCst);
    assert!(
        count >= 2,
        "Should receive at least 2 subscribes, got {}",
        count
    );
}

/// Verify that client adapts transport when preference doesn't match offer
/// Client prefers TCP but server offers UDP only - client should adapt and use UDP
#[test_log::test]
fn subscribe_format_client_adapts_to_available_transport() {
    covers!(feat_req_recentipsd_431, feat_req_recentip_324);

    let subscribe_count = Arc::new(AtomicUsize::new(0));
    let subscribe_count_server = Arc::clone(&subscribe_count);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("wire_server", move || {
        let subscribe_count = Arc::clone(&subscribe_count_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            let mut buf = [0u8; 1500];

            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);
            let mut next_multicast_session_id: u16 = 1;
            let mut next_unicast_session_id: u16 = 1;

            while tokio::time::Instant::now() < deadline {
                if last_offer.elapsed() >= Duration::from_millis(1000) {
                    // Offer UDP ONLY with incrementing session ID
                    let offer = build_sd_offer_with_session(
                        0x1234,
                        0x0001,
                        1,
                        0,
                        my_ip,
                        30509,
                        3,
                        next_multicast_session_id,
                        next_multicast_session_id == 1, // reboot_flag only on first
                        false,                          // unicast_flag (multicast)
                    );
                    next_multicast_session_id += 1;
                    sd_socket.send_to(&offer, sd_multicast).await?;
                    eprintln!("[wire_server] Sent UDP-only offer");
                    last_offer = tokio::time::Instant::now();
                }

                if let Ok(Ok((len, from))) = tokio::time::timeout(
                    Duration::from_millis(100),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            if entry.entry_type as u8 == 0x06 && entry.service_id == 0x1234 {
                                let count = subscribe_count.fetch_add(1, Ordering::SeqCst) + 1;

                                let has_udp = sd_msg.get_udp_endpoint(entry).is_some();
                                let has_tcp = sd_msg.get_tcp_endpoint(entry).is_some();
                                eprintln!(
                                    "[wire_server] Subscribe #{}: UDP={}, TCP={}",
                                    count, has_udp, has_tcp
                                );

                                // Client prefers TCP but server only offers UDP
                                // Client MUST adapt and send UDP endpoint
                                assert!(
                                    has_udp,
                                    "Subscribe #{} to UDP-only service MUST include UDP endpoint \
                                     even when client prefers TCP",
                                    count
                                );
                                assert!(
                                    !has_tcp,
                                    "Subscribe #{} to UDP-only service should NOT include TCP endpoint",
                                    count
                                );

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
                            }
                        }
                    }
                }
            }

            Ok(())
        }
    });

    // Client PREFERS TCP but server only offers UDP
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .preferred_transport(recentip::Transport::Tcp) // Prefers TCP!
            .subscribe_ttl(5)
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

        // Verify the proxy detected UDP transport despite TCP preference
        assert_eq!(
            proxy.transport(),
            recentip::Transport::Udp,
            "Proxy should use UDP transport when that's all that's offered"
        );

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        tokio::time::sleep(Duration::from_secs(6)).await;
        Ok(())
    });

    sim.run().unwrap();

    let count = subscribe_count.load(Ordering::SeqCst);
    assert!(
        count >= 2,
        "Should receive at least 2 subscribes, got {}",
        count
    );
}

/// Verify that client reuses the same endpoint port across subscribe/unsubscribe/resubscribe cycles.
///
/// Setup (matches lib-level multi_eventgroup_subscription_lifecycle test):
/// - Wire-level server offers service with 4 eventgroups
/// - Client subscribes to EG1+EG2 (sub1) and EG3+EG4 (sub2), server captures endpoint ports
/// - Client drops sub1 (sends StopSubscribe for EG1+EG2)
/// - Client re-subscribes to EG1+EG2
/// - Server verifies same endpoint port is used in re-subscription
///
/// This tests port reuse behavior which is important for:
/// - Efficient resource usage
/// - Consistent endpoint identity
/// - Firewall/NAT compatibility
#[test_log::test]
fn subscribe_reuses_endpoint_port_after_resubscribe() {
    covers!(feat_req_recentipsd_431);

    use std::sync::Mutex;

    // Track endpoint ports seen for each subscription phase
    // sub1_ports: ports for EG1+EG2 initial subscription
    // sub2_ports: ports for EG3+EG4 subscription
    // resub1_ports: ports for EG1+EG2 re-subscription
    let sub1_ports: Arc<Mutex<Vec<u16>>> = Arc::new(Mutex::new(Vec::new()));
    let sub2_ports: Arc<Mutex<Vec<u16>>> = Arc::new(Mutex::new(Vec::new()));
    let resub1_ports: Arc<Mutex<Vec<u16>>> = Arc::new(Mutex::new(Vec::new()));

    let sub1_ports_server = Arc::clone(&sub1_ports);
    let sub2_ports_server = Arc::clone(&sub2_ports);
    let resub1_ports_server = Arc::clone(&resub1_ports);

    // Synchronization channels
    let (all_subs_done_tx, all_subs_done_rx) = tokio::sync::oneshot::channel::<()>();
    let (unsub_done_tx, unsub_done_rx) = tokio::sync::oneshot::channel::<()>();
    let (resub_done_tx, resub_done_rx) = tokio::sync::oneshot::channel::<()>();

    let all_subs_done_tx = Arc::new(Mutex::new(Some(all_subs_done_tx)));
    let unsub_done_tx = Arc::new(Mutex::new(Some(unsub_done_tx)));
    let resub_done_tx = Arc::new(Mutex::new(Some(resub_done_tx)));

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("wire_server", move || {
        let sub1_ports = Arc::clone(&sub1_ports_server);
        let sub2_ports = Arc::clone(&sub2_ports_server);
        let resub1_ports = Arc::clone(&resub1_ports_server);
        let all_subs_done_tx = Arc::clone(&all_subs_done_tx);
        let unsub_done_tx = Arc::clone(&unsub_done_tx);
        let resub_done_tx = Arc::clone(&resub_done_tx);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            // Offer service with 4 eventgroups
            let mut buf = [0u8; 1500];

            // Phase 1: Send offers cyclically until we receive 4 unique initial subscriptions
            // (EG1, EG2 from sub1 and EG3, EG4 from sub2)
            let mut initial_eg_set: std::collections::HashSet<u16> =
                std::collections::HashSet::new();
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            let mut next_multicast_session_id = 1;
            let mut next_unicast_session_id = 1;
            while initial_eg_set.len() < 4 && tokio::time::Instant::now() < deadline {
                // Send offer every 500ms
                if last_offer.elapsed() >= Duration::from_millis(500) {
                    let offer = build_sd_offer_with_session(
                        0x1234,
                        0x0001,
                        1,
                        0,
                        my_ip,
                        30509,
                        10,
                        next_multicast_session_id,
                        true,  // reboot_flag
                        false, // unicast_flag (multicast)
                    );
                    next_multicast_session_id += 1;
                    sd_socket.send_to(&offer, sd_multicast).await?;
                    eprintln!("[wire_server] Sent offer");
                    last_offer = tokio::time::Instant::now();
                }
                if let Ok(Ok((len, from))) =
                    tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                        .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            // SubscribeEventgroup = 0x06 with TTL > 0
                            if entry.entry_type as u8 == 0x06
                                && entry.service_id == 0x1234
                                && entry.ttl > 0
                            {
                                if let Some(endpoint) = sd_msg.get_udp_endpoint(entry) {
                                    let port = endpoint.port();
                                    let eg = entry.eventgroup_id;
                                    eprintln!(
                                        "[wire_server] Initial subscribe EG {:04X} from port {}",
                                        eg, port
                                    );
                                    // Only count new eventgroups
                                    if initial_eg_set.insert(eg) {
                                        // EG1 and EG2 go to sub1_ports, EG3 and EG4 go to sub2_ports
                                        if eg == 0x0001 || eg == 0x0002 {
                                            sub1_ports.lock().unwrap().push(port);
                                        } else {
                                            sub2_ports.lock().unwrap().push(port);
                                        }
                                    }

                                    // Send ACK
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
                                    // Sleep here so that the messages don't arrive at the same time
                                    // This spares us having to deal with clustering
                                    tokio::time::sleep(Duration::from_millis(100)).await;
                                }
                            }
                        }
                    }
                }
            }
            assert_eq!(
                initial_eg_set.len(),
                4,
                "Should receive 4 unique initial subscribes (EG1-4)"
            );
            eprintln!("[wire_server] Received 4 initial subscribes, signaling client");

            // Signal all subscriptions complete
            if let Some(tx) = all_subs_done_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }

            // Phase 2: Wait for StopSubscribe (TTL=0) for EG1 and EG2 only
            let mut stop_eg_set: std::collections::HashSet<u16> = std::collections::HashSet::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while stop_eg_set.len() < 2 && tokio::time::Instant::now() < deadline {
                if let Ok(Ok((len, _from))) =
                    tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                        .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            // SubscribeEventgroup with TTL=0 is StopSubscribe
                            if entry.entry_type as u8 == 0x06
                                && entry.service_id == 0x1234
                                && entry.ttl == 0
                            {
                                let eg = entry.eventgroup_id;
                                eprintln!("[wire_server] StopSubscribe for EG {:04X}", eg);
                                // Only expect EG1 and EG2 to be stopped
                                if eg == 0x0001 || eg == 0x0002 {
                                    stop_eg_set.insert(eg);
                                }
                            }
                        }
                    }
                }
            }
            assert_eq!(
                stop_eg_set.len(),
                2,
                "Should receive StopSubscribe for EG1 and EG2"
            );
            eprintln!("[wire_server] Received StopSubscribe for EG1+EG2, signaling client");

            // Signal unsubscribe complete
            if let Some(tx) = unsub_done_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }

            // Phase 3: Send another offer to trigger re-subscription of EG1+EG2
            let offer = build_sd_offer_with_session(
                0x1234,
                0x0001,
                1,
                0,
                my_ip,
                30509,
                10,
                next_multicast_session_id,
                true,  // reboot_flag
                false, // unicast_flag (multicast)
            );
            next_multicast_session_id += 1;
            sd_socket.send_to(&offer, sd_multicast).await?;
            eprintln!("[wire_server] Sent offer for re-subscription");

            // Wait for re-subscriptions (only EG1 and EG2)
            let mut resub_eg_set: std::collections::HashSet<u16> = std::collections::HashSet::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while resub_eg_set.len() < 2 && tokio::time::Instant::now() < deadline {
                if let Ok(Ok((len, from))) =
                    tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                        .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            // SubscribeEventgroup with TTL > 0 for EG1 or EG2
                            if entry.entry_type as u8 == 0x06
                                && entry.service_id == 0x1234
                                && entry.ttl > 0
                            {
                                let eg = entry.eventgroup_id;
                                if eg == 0x0001 || eg == 0x0002 {
                                    if let Some(endpoint) = sd_msg.get_udp_endpoint(entry) {
                                        let port = endpoint.port();
                                        eprintln!(
                                            "[wire_server] Re-subscribe EG {:04X} from port {}",
                                            eg, port
                                        );
                                        // Only count new eventgroups
                                        if resub_eg_set.insert(eg) {
                                            resub1_ports.lock().unwrap().push(port);
                                        }

                                        // Send ACK
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
                                        // Sleep here so that the messages don't arrive at the same time
                                        // This spares us having to deal with clustering
                                        tokio::time::sleep(Duration::from_millis(100)).await;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            assert_eq!(
                resub_eg_set.len(),
                2,
                "Should receive re-subscribes for EG1 and EG2"
            );
            eprintln!("[wire_server] Received 2 re-subscribes, signaling client");

            // Signal re-subscription complete
            if let Some(tx) = resub_done_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }

            Ok(())
        }
    });

    sim.client("client", async move {
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
            .expect("Discovery timeout")
            .expect("Service available");

        eprintln!("[client] Service discovered");

        let eg1 = EventgroupId::new(0x0001).unwrap();
        let eg2 = EventgroupId::new(0x0002).unwrap();
        let eg3 = EventgroupId::new(0x0003).unwrap();
        let eg4 = EventgroupId::new(0x0004).unwrap();

        // Phase 1: Subscribe to EG1+EG2 (sub1)
        let sub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy
                .new_subscription()
                .eventgroup(eg1)
                .eventgroup(eg2)
                .subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");
        eprintln!("[client] Subscription 1 (EG1+EG2) established");

        // Subscribe to EG3+EG4 (sub2)
        let _sub2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy
                .new_subscription()
                .eventgroup(eg3)
                .eventgroup(eg4)
                .subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");
        eprintln!("[client] Subscription 2 (EG3+EG4) established");

        // Wait for server to receive all subscriptions
        all_subs_done_rx.await.expect("All subs signal");

        // Phase 2: Drop sub1 to trigger StopSubscribe for EG1+EG2
        drop(sub1);
        eprintln!("[client] Dropped subscription 1 (EG1+EG2)");

        // Wait for server to receive StopSubscribe
        unsub_done_rx.await.expect("Unsub signal");

        // Phase 3: Re-subscribe to EG1+EG2
        let _sub1_new = tokio::time::timeout(
            Duration::from_secs(5),
            proxy
                .new_subscription()
                .eventgroup(eg1)
                .eventgroup(eg2)
                .subscribe(),
        )
        .await
        .expect("Re-subscribe timeout")
        .expect("Re-subscribe should succeed");
        eprintln!("[client] Subscription 1 (EG1+EG2) re-established");

        // Wait for server to receive re-subscriptions
        resub_done_rx.await.expect("Resub signal");

        // Keep alive briefly
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    sim.run().unwrap();

    // Verify port reuse
    let sub1 = sub1_ports.lock().unwrap();
    let sub2 = sub2_ports.lock().unwrap();
    let resub1 = resub1_ports.lock().unwrap();

    eprintln!("Sub1 (EG1+EG2) ports: {:?}", *sub1);
    eprintln!("Sub2 (EG3+EG4) ports: {:?}", *sub2);
    eprintln!("Resub1 (EG1+EG2) ports: {:?}", *resub1);

    assert_eq!(
        sub1.len(),
        2,
        "Should have captured 2 ports for sub1 (EG1+EG2)"
    );
    assert_eq!(
        sub2.len(),
        2,
        "Should have captured 2 ports for sub2 (EG3+EG4)"
    );
    assert_eq!(
        resub1.len(),
        2,
        "Should have captured 2 ports for resub1 (EG1+EG2)"
    );

    // All ports in sub1 should appear in resub1 (port reuse after unsubscribe/resubscribe)
    for port in sub1.iter() {
        assert!(
            resub1.contains(port),
            "Port {} from sub1 should be reused in resub1. Sub1: {:?}, Resub1: {:?}",
            port,
            *sub1,
            *resub1
        );
    }
}

/// TCP version: Verify that client reuses TCP connection port across subscribe/unsubscribe/resubscribe cycles.
///
/// Setup (matches lib-level multi_eventgroup_subscription_lifecycle test):
/// - Wire-level server offers TCP-only service with 4 eventgroups
/// - Client subscribes to EG1+EG2 (sub1) and EG3+EG4 (sub2), server captures TCP endpoint ports
/// - Client drops sub1 (sends StopSubscribe for EG1+EG2)
/// - Client re-subscribes to EG1+EG2
/// - Server verifies same TCP endpoint port is used in re-subscription
///
/// This tests TCP connection/port reuse behavior which is important for:
/// - Efficient resource usage (avoiding connection churn)
/// - Consistent endpoint identity
/// - Connection state preservation
#[test_log::test]
fn subscribe_tcp_reuses_endpoint_port_after_resubscribe() {
    covers!(feat_req_recentipsd_431);

    use std::sync::Mutex;

    // Track TCP endpoint ports seen for each subscription phase
    let sub1_ports: Arc<Mutex<Vec<u16>>> = Arc::new(Mutex::new(Vec::new()));
    let sub2_ports: Arc<Mutex<Vec<u16>>> = Arc::new(Mutex::new(Vec::new()));
    let resub1_ports: Arc<Mutex<Vec<u16>>> = Arc::new(Mutex::new(Vec::new()));

    let sub1_ports_server = Arc::clone(&sub1_ports);
    let sub2_ports_server = Arc::clone(&sub2_ports);
    let resub1_ports_server = Arc::clone(&resub1_ports);

    // Synchronization channels
    let (all_subs_done_tx, all_subs_done_rx) = tokio::sync::oneshot::channel::<()>();
    let (unsub_done_tx, unsub_done_rx) = tokio::sync::oneshot::channel::<()>();
    let (resub_done_tx, resub_done_rx) = tokio::sync::oneshot::channel::<()>();

    let all_subs_done_tx = Arc::new(Mutex::new(Some(all_subs_done_tx)));
    let unsub_done_tx = Arc::new(Mutex::new(Some(unsub_done_tx)));
    let resub_done_tx = Arc::new(Mutex::new(Some(resub_done_tx)));

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("wire_server", move || {
        let sub1_ports = Arc::clone(&sub1_ports_server);
        let sub2_ports = Arc::clone(&sub2_ports_server);
        let resub1_ports = Arc::clone(&resub1_ports_server);
        let all_subs_done_tx = Arc::clone(&all_subs_done_tx);
        let unsub_done_tx = Arc::clone(&unsub_done_tx);
        let resub_done_tx = Arc::clone(&resub_done_tx);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            // Start TCP listener for client connections (required before subscribe per feat_req_recentipsd_767)
            let tcp_listener = turmoil::net::TcpListener::bind("0.0.0.0:30509").await?;
            tokio::spawn(async move {
                loop {
                    if let Ok((stream, peer)) = tcp_listener.accept().await {
                        eprintln!("[wire_server] Accepted TCP connection from {}", peer);
                        tokio::spawn(async move {
                            let _stream = stream;
                            tokio::time::sleep(Duration::from_secs(30)).await;
                        });
                    }
                }
            });

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            // Offer TCP-only service with 4 eventgroups
            let mut buf = [0u8; 1500];

            // Phase 1: Send offers cyclically until we receive 4 unique initial subscriptions
            let mut initial_eg_set: std::collections::HashSet<u16> = std::collections::HashSet::new();
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            let mut next_unicast_session_id = 1;
            let mut next_multicast_session_id = 1;
            while initial_eg_set.len() < 4 && tokio::time::Instant::now() < deadline {
                if last_offer.elapsed() >= Duration::from_millis(500) {
                    let offer = build_sd_offer_tcp_only(0x1234, 0x0001, 1, 0, my_ip, 30509, 10, next_multicast_session_id, true, false);
                    next_multicast_session_id += 1;
                    sd_socket.send_to(&offer, sd_multicast).await?;
                    eprintln!("[wire_server] Sent TCP-only offer");
                    last_offer = tokio::time::Instant::now();
                }
                if let Ok(Ok((len, from))) = tokio::time::timeout(
                    Duration::from_millis(100),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            // SubscribeEventgroup = 0x06 with TTL > 0
                            if entry.entry_type as u8 == 0x06
                                && entry.service_id == 0x1234
                                && entry.ttl > 0
                            {
                                // For TCP, get TCP endpoint
                                if let Some(endpoint) = sd_msg.get_tcp_endpoint(entry) {
                                    let port = endpoint.port();
                                    let eg = entry.eventgroup_id;
                                    eprintln!(
                                        "[wire_server] Initial TCP subscribe EG {:04X} from port {}",
                                        eg, port
                                    );
                                    if initial_eg_set.insert(eg) {
                                        if eg == 0x0001 || eg == 0x0002 {
                                            sub1_ports.lock().unwrap().push(port);
                                        } else {
                                            sub2_ports.lock().unwrap().push(port);
                                        }
                                    }

                                    // Send ACK
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
                                }
                            }
                        }
                    }
                }
            }
            assert_eq!(
                initial_eg_set.len(),
                4,
                "Should receive 4 unique initial TCP subscribes (EG1-4)"
            );
            eprintln!("[wire_server] Received 4 initial TCP subscribes, signaling client");

            if let Some(tx) = all_subs_done_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }

            // Phase 2: Wait for StopSubscribe (TTL=0) for EG1 and EG2 only
            let mut stop_eg_set: std::collections::HashSet<u16> = std::collections::HashSet::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while stop_eg_set.len() < 2 && tokio::time::Instant::now() < deadline {
                if let Ok(Ok((len, _from))) = tokio::time::timeout(
                    Duration::from_millis(100),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            if entry.entry_type as u8 == 0x06
                                && entry.service_id == 0x1234
                                && entry.ttl == 0
                            {
                                let eg = entry.eventgroup_id;
                                eprintln!("[wire_server] TCP StopSubscribe for EG {:04X}", eg);
                                if eg == 0x0001 || eg == 0x0002 {
                                    stop_eg_set.insert(eg);
                                }
                            }
                        }
                    }
                }
            }
            assert_eq!(stop_eg_set.len(), 2, "Should receive StopSubscribe for EG1 and EG2");
            eprintln!("[wire_server] Received TCP StopSubscribe for EG1+EG2, signaling client");

            if let Some(tx) = unsub_done_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }

            // Phase 3: Send another offer to trigger re-subscription of EG1+EG2
            let offer = build_sd_offer_tcp_only(0x1234, 0x0001, 1, 0, my_ip, 30509, 10, next_multicast_session_id, true, false);
            sd_socket.send_to(&offer, sd_multicast).await?;
            next_multicast_session_id += 1;
            eprintln!("[wire_server] Sent offer for TCP re-subscription");

            // Wait for re-subscriptions (only EG1 and EG2)
            let mut resub_eg_set: std::collections::HashSet<u16> = std::collections::HashSet::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while resub_eg_set.len() < 2 && tokio::time::Instant::now() < deadline {
                if let Ok(Ok((len, from))) = tokio::time::timeout(
                    Duration::from_millis(100),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            if entry.entry_type as u8 == 0x06
                                && entry.service_id == 0x1234
                                && entry.ttl > 0
                            {
                                let eg = entry.eventgroup_id;
                                if eg == 0x0001 || eg == 0x0002 {
                                    if let Some(endpoint) = sd_msg.get_tcp_endpoint(entry) {
                                        let port = endpoint.port();
                                        eprintln!(
                                            "[wire_server] TCP Re-subscribe EG {:04X} from port {}",
                                            eg, port
                                        );
                                        if resub_eg_set.insert(eg) {
                                            resub1_ports.lock().unwrap().push(port);
                                        }

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
                                    }
                                }
                            }
                        }
                    }
                }
            }
            assert_eq!(
                resub_eg_set.len(),
                2,
                "Should receive TCP re-subscribes for EG1 and EG2"
            );
            eprintln!("[wire_server] Received 2 TCP re-subscribes, signaling client");

            if let Some(tx) = resub_done_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }

            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(recentip::Transport::Tcp)
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

        eprintln!("[client] TCP service discovered");

        let eg1 = EventgroupId::new(0x0001).unwrap();
        let eg2 = EventgroupId::new(0x0002).unwrap();
        let eg3 = EventgroupId::new(0x0003).unwrap();
        let eg4 = EventgroupId::new(0x0004).unwrap();

        // Phase 1: Subscribe to EG1+EG2 (sub1) via TCP
        let sub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy
                .new_subscription()
                .eventgroup(eg1)
                .eventgroup(eg2)
                .subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        eprintln!("[client] TCP Subscription 1 (EG1+EG2) established");

        // Subscribe to EG3+EG4 (sub2)
        let sub2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy
                .new_subscription()
                .eventgroup(eg3)
                .eventgroup(eg4)
                .subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        eprintln!("[client] TCP Subscription 2 (EG3+EG4) established");

        // Wait for server to receive all subscriptions
        tokio::time::timeout(Duration::from_secs(5), all_subs_done_rx)
            .await
            .expect("Timeout waiting for subscriptions")
            .expect("Channel closed");

        // Phase 2: Drop sub1 (will send StopSubscribe for EG1+EG2)
        drop(sub1);
        eprintln!("[client] Dropped TCP subscription 1 (EG1+EG2)");

        // Wait for server to receive StopSubscribe
        tokio::time::timeout(Duration::from_secs(5), unsub_done_rx)
            .await
            .expect("Timeout waiting for unsubscribe")
            .expect("Channel closed");

        // Phase 3: Re-subscribe to EG1+EG2 (resub1)
        let _resub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy
                .new_subscription()
                .eventgroup(eg1)
                .eventgroup(eg2)
                .subscribe(),
        )
        .await
        .expect("Re-subscribe timeout")
        .expect("Re-subscribe should succeed");

        eprintln!("[client] TCP Subscription 1 (EG1+EG2) re-established");

        // Wait for server to receive re-subscriptions
        tokio::time::timeout(Duration::from_secs(5), resub_done_rx)
            .await
            .expect("Timeout waiting for re-subscription")
            .expect("Channel closed");

        // Keep sub2 alive for assertions
        drop(sub2);

        Ok(())
    });

    sim.run().unwrap();

    // Verify port collection and reuse
    let sub1 = sub1_ports.lock().unwrap();
    let sub2 = sub2_ports.lock().unwrap();
    let resub1 = resub1_ports.lock().unwrap();

    eprintln!("TCP Sub1 (EG1+EG2) ports: {:?}", *sub1);
    eprintln!("TCP Sub2 (EG3+EG4) ports: {:?}", *sub2);
    eprintln!("TCP Resub1 (EG1+EG2) ports: {:?}", *resub1);

    // Verify each subscription uses a consistent port within itself
    assert_eq!(
        sub1.len(),
        2,
        "Should have captured 2 ports for TCP sub1 (EG1+EG2)"
    );
    assert_eq!(
        sub2.len(),
        2,
        "Should have captured 2 ports for TCP sub2 (EG3+EG4)"
    );
    assert_eq!(
        resub1.len(),
        2,
        "Should have captured 2 ports for TCP resub1 (EG1+EG2)"
    );

    assert_eq!(
        sub1[0], sub1[1],
        "TCP Sub1 should use same port for both eventgroups"
    );
    assert_eq!(
        sub2[0], sub2[1],
        "TCP Sub2 should use same port for both eventgroups"
    );
    assert_eq!(
        resub1[0], resub1[1],
        "TCP Resub1 should use same port for both eventgroups"
    );

    // Different subscriptions use different connections (per-subscription connection strategy)
    assert_ne!(
        sub1[0], sub2[0],
        "Different TCP subscriptions should use different connection ports"
    );

    // TCP connection slot reuse behavior:
    // Each subscription gets a unique "slot" (conn_key) for event routing purposes.
    // When sub1 (slot 0) is dropped and resub1 is created, resub1 gets the smallest
    // available slot, which is slot 0 (the freed slot from sub1).
    //
    // This ensures correct event routing: events on each connection only go to
    // subscriptions with matching tcp_conn_key.
    //
    // Port assignment follows slot assignment - resub1 reuses sub1's original port.
    assert_eq!(
        resub1[0], sub1[0],
        "TCP Resub1 should reuse sub1's freed slot (port)"
    );
    assert_ne!(
        resub1[0], sub2[0],
        "TCP Resub1 should NOT share connection with sub2 (different slots)"
    );
    eprintln!(
        "TCP slot reuse verified: Resub1 reuses freed slot/port {} (same as original sub1)",
        resub1[0]
    );
}
