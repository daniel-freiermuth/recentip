//! Subscription Wire Format Tests
//!
//! These tests verify that SOME/IP-SD subscription-related messages have
//! the correct byte-level encoding as specified by the protocol specification.
//!
//! Tests cover:
//! - SubscribeEventgroup entry type (0x06)
//! - SubscribeEventgroupAck entry type (0x07)
//! - StopSubscribeEventgroup (TTL=0)
//! - TTL field behavior (max TTL, expiration)

use super::helpers::*;

/// feat_req_recentipsd_576: SubscribeEventgroup entry type is 0x06
///
/// When a client subscribes to an eventgroup, the SD message must contain
/// an entry with type 0x06 (SubscribeEventgroup).
#[test_log::test]
fn subscribe_eventgroup_entry_type() {
    covers!(feat_req_recentipsd_576);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Raw socket acts as server - sends SD offers, receives subscribe
    sim.host("raw_server", || async {
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_server")
            .to_string()
            .parse()
            .unwrap();

        // SD socket to send offers and receive subscribes
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

        let offer = build_sd_offer(0x1234, 0x0001, 1, 0, my_ip, 30509, 3600);

        let mut buf = [0u8; 1500];
        let mut found_subscribe = false;

        // Send offers and wait for subscribe
        for _ in 0..30 {
            sd_socket.send_to(&offer, sd_multicast).await?;

            while let Ok(Ok((len, from))) = tokio::time::timeout(
                Duration::from_millis(50),
                sd_socket.recv_from(&mut buf),
                ).await {
                // Check if this is an SD message
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // SubscribeEventgroup = 0x06
                        if entry.entry_type as u8 == 0x06 {
                            found_subscribe = true;
                            eprintln!("Received SubscribeEventgroup: entry_type=0x{:02X}, service=0x{:04X}, eventgroup=0x{:04X}, TTL={}",
                                entry.entry_type as u8, entry.service_id, entry.eventgroup_id, entry.ttl);

                            assert_eq!(
                                entry.entry_type as u8, 0x06,
                                "SubscribeEventgroup entry type must be 0x06"
                            );
                            assert!(entry.ttl > 0, "Subscribe TTL should be > 0");

                            // Send SubscribeAck back via SD socket
                            let ack = build_sd_subscribe_ack(
                                entry.service_id,
                                entry.instance_id,
                                entry.major_version,
                                entry.eventgroup_id,
                                entry.ttl, // Echo TTL
                            );
                            sd_socket.send_to(&ack, from).await?;
                        }
                    }
                }
            }

            if found_subscribe {
                break;
            }
        }

        assert!(found_subscribe, "Should receive SubscribeEventgroup entry");
        Ok(())
    });

    // Client subscribes using public API
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Should discover service via SD")
            .expect("Service available");

        // Subscribe to eventgroup using public API
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_576: SubscribeEventgroupAck entry type is 0x07
/// feat_req_recentipsd_441: Server shall send SubscribeEventgroupAck after receiving SubscribeEventgroup
///
/// When a server acknowledges a subscription, the SD message must contain
/// an entry with type 0x07 (SubscribeEventgroupAck).
#[test_log::test]
fn subscribe_ack_entry_type() {
    covers!(feat_req_recentipsd_576);
    covers!(feat_req_recentipsd_441);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service and handles subscription
    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for subscription and accept it
        if let Some(event) = tokio::time::timeout(Duration::from_secs(10), offering.next())
            .await
            .ok()
            .flatten()
        {
            if let ServiceEvent::Subscribe { .. } = event {}
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    // Raw socket sends subscribe and captures SubscribeAck
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Listen for SD offers to find server
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Find server endpoint from SD offer
        for _ in 0..20 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 {
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

        let server_addr = server_endpoint.expect("Should find server via SD");
        eprintln!("Found server at {}", server_addr);
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_client").to_string().parse().unwrap();

        // Send Subscribe message with UDP endpoint
        // Need to send from a port the server can respond to
        let client_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;
        let subscribe =
            build_sd_subscribe_with_udp_endpoint(0x1234, 0x0001, 1, 0x0001, 3600, my_ip, 12345);
        client_socket.send_to(&subscribe, server_addr).await?;

        // Wait for SubscribeAck
        let mut found_ack = false;
        for _ in 0..20 {
            let result = tokio::time::timeout(
                Duration::from_millis(200),
                client_socket.recv_from(&mut buf),
            )
            .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // SubscribeEventgroupAck = 0x07
                        if entry.entry_type as u8 == 0x07 {
                            found_ack = true;
                            eprintln!(
                                "Received SubscribeEventgroupAck: entry_type=0x{:02X}, TTL={}",
                                entry.entry_type as u8, entry.ttl
                            );

                            assert_eq!(
                                entry.entry_type as u8, 0x07,
                                "SubscribeEventgroupAck entry type must be 0x07"
                            );
                            assert!(entry.ttl > 0, "SubscribeAck TTL should be > 0");
                        }
                    }
                }
            }

            if found_ack {
                break;
            }
        }

        assert!(found_ack, "Should receive SubscribeEventgroupAck");
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_431: With max TTL (0xFFFFFF), subscription never expires
///
/// Verifies that a subscription with maximum TTL continues receiving events
/// well past the point where a finite TTL would have expired.
/// Uses turmoil's simulated time to fast-forward through what would be
/// the expiration period.
#[test_log::test]
#[cfg(feature = "slow-tests")]
fn subscription_max_ttl_doesnt_expire() {
    covers!(feat_req_recentipsd_431);

    const MAX_TTL: u32 = 0xFFFFFF; // ~194 days
    const TICK_SECS: u32 = 100; // Turmoil tick duration in seconds
    const CYCLIC_OFFER_DELAY: u32 = 10 * TICK_SECS; // Server offer interval
    const OFFER_TTL: u32 = 100 * TICK_SECS; // Server offer TTL

    // Simulation needs to run long enough to prove TTL doesn't expire
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(
            MAX_TTL as u64 + CYCLIC_OFFER_DELAY as u64 + 30 * TICK_SECS as u64,
        ))
        .tick_duration(Duration::from_secs(TICK_SECS as u64))
        .build();

    // Server offers service and sends events after long delays
    sim.host("server", || async {
        tokio::time::sleep(Duration::from_secs(2 * TICK_SECS as u64)).await;

        let config = RuntimeConfig::builder()
            .cyclic_offer_delay(CYCLIC_OFFER_DELAY as u64 * 1000)
            .offer_ttl(OFFER_TTL)
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        let offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(MAX_TTL as u64 - 10 * TICK_SECS as u64)).await;

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        let mut i: u32 = 0;
        loop {
            let _ = event_handle.notify(format!("event{}", i).as_bytes()).await;
            eprintln!("Server sent event {}", i);
            tokio::time::sleep(Duration::from_secs(TICK_SECS as u64)).await;
            i += 1;
        }
    });

    // Raw client subscribes with max TTL and NEVER renews
    sim.client("raw_client", async move {
        // Listen for SD offers
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        // Socket to receive events
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30600").await?;

        let mut buf = [0u8; 1500];
        let mut server_addr: Option<SocketAddr> = None;

        // Find server via SD offer
        for _ in 0..20 {
            let result = tokio::time::timeout(
                Duration::from_secs(2 * TICK_SECS as u64),
                sd_socket.recv_from(&mut buf),
            )
            .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 {
                            // Subscribe messages should be sent to SD port (30490)
                            server_addr = Some(SocketAddr::new(from.ip(), 30490));
                            eprintln!("Found server at {:?}", server_addr);
                        }
                    }
                }
            }
            if server_addr.is_some() {
                break;
            }
        }

        let server = server_addr.expect("Should find server via SD");

        // Subscribe with MAX TTL (0xFFFFFF = ~194 days) - never needs renewal and UDP endpoint
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_client").to_string().parse().unwrap();
        let subscribe =
            build_sd_subscribe_with_udp_endpoint(0x1234, 0x0001, 1, 0x0001, MAX_TTL, my_ip, 30600);
        event_socket.send_to(&subscribe, server).await?;
        eprintln!("Raw client sent SubscribeEventgroup with TTL=max (0xFFFFFF)");

        // Wait for SubscribeAck
        let result = tokio::time::timeout(
            Duration::from_secs(2 * TICK_SECS as u64),
            event_socket.recv_from(&mut buf),
        )
        .await;
        if let Ok(Ok((len, _))) = result {
            if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                for entry in &sd_msg.entries {
                    if entry.entry_type as u8 == 0x07 {
                        eprintln!("Received SubscribeEventgroupAck");
                    }
                }
            }
        }

        // Count events received
        let mut events_received = 0;
        let start = tokio::time::Instant::now();

        while start.elapsed()
            < Duration::from_secs(
                MAX_TTL as u64 + CYCLIC_OFFER_DELAY as u64 + 10 * TICK_SECS as u64,
            )
        {
            let result = tokio::time::timeout(
                Duration::from_secs(2 * TICK_SECS as u64),
                event_socket.recv_from(&mut buf),
            )
            .await;
            if let Ok(Ok((len, _))) = result {
                if start.elapsed() < Duration::from_secs(MAX_TTL as u64 + CYCLIC_OFFER_DELAY as u64)
                {
                    eprintln!("Got early packet");
                    continue;
                }
                assert!(len > 16, "Event packet must be at least 16 bytes");
                let service_id = u16::from_be_bytes([buf[0], buf[1]]);
                let method_id = u16::from_be_bytes([buf[2], buf[3]]);
                // Event IDs start with 0x8xxx
                if service_id == 0x1234 && (method_id & 0x8000) != 0 {
                    events_received += 1;
                    eprintln!(
                        "Raw client received event {} at {:?}",
                        events_received,
                        start.elapsed()
                    );
                }
            }
        }

        eprintln!("Raw client received {} events total", events_received);

        assert!(
            events_received > 8,
            "Should receive some events before TTL expires"
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_445: Cleanup mechanism required to avoid stale client registrations
///
/// Server must expire subscriptions when TTL elapses without renewal.
/// Raw client subscribes with short TTL, server sends events continuously,
/// subscription expires (no renewal sent), events should stop.
#[test_log::test]
fn subscription_ttl_expiration_stops_events() {
    covers!(feat_req_recentipsd_445);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service and sends events continuously
    sim.host("server", || async {
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(RuntimeConfig::default())
                .await
                .unwrap();

        let offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        let mut i: u32 = 0;
        loop {
            let _ = event_handle.notify(format!("event{}", i).as_bytes()).await;
            eprintln!("Server sent event {}", i);
            tokio::time::sleep(Duration::from_millis(500)).await;
            i += 1;
        }
    });

    // Raw client subscribes with TTL but NEVER renews
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Listen for SD offers
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        // Socket to receive events
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30600").await?;

        let mut buf = [0u8; 1500];
        let mut server_addr: Option<SocketAddr> = None;

        // Find server via SD offer
        for _ in 0..20 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 {
                            // Subscribe messages should be sent to SD port (30490)
                            server_addr = Some(SocketAddr::new(from.ip(), 30490));
                            eprintln!("Found server at {:?}", server_addr);
                        }
                    }
                }
            }
            if server_addr.is_some() {
                break;
            }
        }

        let server = server_addr.expect("Should find server via SD");

        // Send SubscribeEventgroup with short TTL (2 seconds) - and NEVER renew
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_client").to_string().parse().unwrap();
        let subscribe =
            build_sd_subscribe_with_udp_endpoint(0x1234, 0x0001, 1, 0x0001, 2, my_ip, 30600);
        event_socket.send_to(&subscribe, server).await?;
        eprintln!("Raw client sent SubscribeEventgroup with TTL=2s");

        // Wait for SubscribeAck
        let result =
            tokio::time::timeout(Duration::from_secs(2), event_socket.recv_from(&mut buf)).await;
        if let Ok(Ok((len, _))) = result {
            if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                for entry in &sd_msg.entries {
                    if entry.entry_type as u8 == 0x07 {
                        eprintln!("Received SubscribeEventgroupAck");
                    }
                }
            }
        }

        // Count events received
        let mut events_received = 0;
        let start = tokio::time::Instant::now();

        // Listen for events for 6 seconds (longer than TTL + events)
        while start.elapsed() < Duration::from_secs(6) {
            let result =
                tokio::time::timeout(Duration::from_millis(200), event_socket.recv_from(&mut buf))
                    .await;
            if let Ok(Ok((len, _))) = result {
                assert!(len > 16, "Event packet must be at least 16 bytes");
                let service_id = u16::from_be_bytes([buf[0], buf[1]]);
                let method_id = u16::from_be_bytes([buf[2], buf[3]]);
                // Event IDs start with 0x8xxx
                if service_id == 0x1234 && (method_id & 0x8000) != 0 {
                    events_received += 1;
                    eprintln!(
                        "Raw client received event {} at {:?}",
                        events_received,
                        start.elapsed()
                    );
                }
            }
        }

        eprintln!("Raw client received {} events total", events_received);

        // The key assertion: we should have received some events initially,
        // but NOT all 10 events because TTL expired after 2 seconds
        // (first 3-4 events sent within TTL, rest should be dropped)
        assert!(
            events_received > 0,
            "Should receive some events before TTL expires"
        );
        assert!(
            events_received < 8,
            "Should stop receiving events after TTL expires (got {})",
            events_received
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_178: StopSubscribeEventgroup has TTL=0
///
/// When a client unsubscribes (drops subscription), it sends a SubscribeEventgroup
/// entry with TTL=0, which means StopSubscribeEventgroup.
#[test_log::test]
fn stop_subscribe_has_ttl_zero() {
    covers!(feat_req_recentipsd_178);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Raw socket acts as server - sends SD offers, receives subscribe and stop-subscribe
    sim.host("raw_server", || async {
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_server").to_string().parse().unwrap();

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

        let offer = build_sd_offer(0x1234, 0x0001, 1, 0, my_ip, 30509, 3600);

        let mut buf = [0u8; 1500];
        let mut found_subscribe = false;
        let mut found_stop_subscribe = false;

        // Send offers and wait for subscribe, then stop-subscribe
        for _ in 0..50 {
            sd_socket.send_to(&offer, sd_multicast).await?;

            while let Ok(Ok((len, from))) =
                tokio::time::timeout(Duration::from_millis(50), sd_socket.recv_from(&mut buf)).await
            {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x06 {
                            if entry.ttl == 0 {
                                // StopSubscribe
                                found_stop_subscribe = true;
                                eprintln!("Received StopSubscribeEventgroup: TTL=0");
                                assert_eq!(entry.ttl, 0, "StopSubscribe must have TTL=0");
                            } else {
                                // Subscribe
                                found_subscribe = true;
                                eprintln!("Received SubscribeEventgroup: TTL={}", entry.ttl);

                                // Send SubscribeAck back via SD socket
                                let ack = build_sd_subscribe_ack(
                                    entry.service_id,
                                    entry.instance_id,
                                    entry.major_version,
                                    entry.eventgroup_id,
                                    entry.ttl, // Echo TTL
                                );
                                sd_socket.send_to(&ack, from).await?;
                            }
                        }
                    }
                }
            }

            if found_stop_subscribe {
                break;
            }
        }

        assert!(found_subscribe, "Should receive SubscribeEventgroup first");
        assert!(
            found_stop_subscribe,
            "Should receive StopSubscribeEventgroup (TTL=0)"
        );
        Ok(())
    });

    // Client subscribes, then unsubscribes (drop)
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Should discover service via SD")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();

        // Subscribe
        let subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        tokio::time::sleep(Duration::from_millis(500)).await;

        // Drop subscription to trigger StopSubscribe
        drop(subscription);

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
}
