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
    build_sd_offer, build_sd_offer_dual_stack, build_sd_offer_tcp_only, build_sd_subscribe_ack,
    covers, parse_sd_message, TEST_SERVICE_ID,
};
use recentip::prelude::*;
use recentip::{Runtime, RuntimeConfig};
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

            let offer = build_sd_offer(0x1234, 0x0001, 1, 0, my_ip, 30509, 3);
            let mut buf = [0u8; 1500];

            // Send offers cyclically and collect subscribes
            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);

            while tokio::time::Instant::now() < deadline {
                // Send offer every 1 second (simulating cyclic offer with short TTL)
                if last_offer.elapsed() >= Duration::from_millis(1000) {
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
                                let ack = build_sd_subscribe_ack(
                                    entry.service_id,
                                    entry.instance_id,
                                    entry.major_version,
                                    entry.eventgroup_id,
                                    entry.ttl,
                                );
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

            let offer = build_sd_offer_tcp_only(0x1234, 0x0001, 1, 0, my_ip, 30509, 3);
            let mut buf = [0u8; 1500];

            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);

            while tokio::time::Instant::now() < deadline {
                if last_offer.elapsed() >= Duration::from_millis(1000) {
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
                                let ack = build_sd_subscribe_ack(
                                    entry.service_id,
                                    entry.instance_id,
                                    entry.major_version,
                                    entry.eventgroup_id,
                                    entry.ttl,
                                );
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

            let offer = build_sd_offer_dual_stack(0x1234, 0x0001, 1, 0, my_ip, 30509, 30510, 3);
            let mut buf = [0u8; 1500];

            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);

            while tokio::time::Instant::now() < deadline {
                if last_offer.elapsed() >= Duration::from_millis(1000) {
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

                                let ack = build_sd_subscribe_ack(
                                    entry.service_id,
                                    entry.instance_id,
                                    entry.major_version,
                                    entry.eventgroup_id,
                                    entry.ttl,
                                );
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

            let offer = build_sd_offer_dual_stack(0x1234, 0x0001, 1, 0, my_ip, 30509, 30510, 3);
            let mut buf = [0u8; 1500];

            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);

            while tokio::time::Instant::now() < deadline {
                if last_offer.elapsed() >= Duration::from_millis(1000) {
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

                                let ack = build_sd_subscribe_ack(
                                    entry.service_id,
                                    entry.instance_id,
                                    entry.major_version,
                                    entry.eventgroup_id,
                                    entry.ttl,
                                );
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

            // Offer UDP ONLY
            let offer = build_sd_offer(0x1234, 0x0001, 1, 0, my_ip, 30509, 3);
            let mut buf = [0u8; 1500];

            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);

            while tokio::time::Instant::now() < deadline {
                if last_offer.elapsed() >= Duration::from_millis(1000) {
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

                                let ack = build_sd_subscribe_ack(
                                    entry.service_id,
                                    entry.instance_id,
                                    entry.major_version,
                                    entry.eventgroup_id,
                                    entry.ttl,
                                );
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
