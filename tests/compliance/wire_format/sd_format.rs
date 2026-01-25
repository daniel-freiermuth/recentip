//! SD Wire Format Tests
//!
//! These tests verify that SOME/IP-SD messages have the correct byte-level
//! encoding as specified by the SOME/IP-SD protocol specification.
//!
//! Tests cover:
//! - SD header constants (Service ID 0xFFFF, Method ID 0x8100, etc.)
//! - SD port (30490)
//! - Entry type encoding (OfferService = 0x01, FindService = 0x00, etc.)
//! - TTL field encoding (24-bit)
//! - SubscribeEventgroupAck TTL echoing

use crate::client_behavior::helpers::{
    build_sd_offer_with_session, build_sd_subscribe_ack_with_session,
};

use super::helpers::*;

/// feat_req_someipsd_141: SD Service ID is 0xFFFF
/// feat_req_someipsd_142: SD Method ID is 0x8100
/// feat_req_someipsd_144: SD Client ID is 0x0000
/// feat_req_someip_103: SD uses NOTIFICATION (0x02) message type
#[test_log::test]
fn sd_offer_wire_format() {
    covers!(
        feat_req_someipsd_141,
        feat_req_someipsd_142,
        feat_req_someipsd_144,
        feat_req_someip_103
    );

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library side - offers a service
    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Offer a service - this sends SD Offer messages
        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Keep server alive
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    // Raw socket side - captures SD multicast
    sim.client("raw_observer", async move {
        // Bind to SD multicast port
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        // Join multicast group for SD
        socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut captured_sd = Vec::new();

        // Capture a few SD messages
        for _ in 0..3 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), socket.recv_from(&mut buf)).await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    captured_sd.push((header, sd_msg));
                }
            }
        }

        // Verify captured SD offers
        assert!(!captured_sd.is_empty(), "Should have captured SD messages");

        for (header, _sd_msg) in &captured_sd {
            // Verify SD header constants per specification
            assert_eq!(
                header.service_id, 0xFFFF,
                "SD Service ID must be 0xFFFF (feat_req_someipsd_141)"
            );
            assert_eq!(
                header.method_id, 0x8100,
                "SD Method ID must be 0x8100 (feat_req_someipsd_142)"
            );
            assert_eq!(
                header.client_id, 0x0000,
                "SD Client ID must be 0x0000 (feat_req_someipsd_144)"
            );
            assert_eq!(
                header.message_type,
                MessageType::Notification,
                "SD uses NOTIFICATION message type (feat_req_someip_103)"
            );
            assert_eq!(
                header.protocol_version, 0x01,
                "Protocol version must be 0x01"
            );
        }

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someipsd_27: SD uses UDP port 30490
#[test_log::test]
fn sd_uses_port_30490() {
    covers!(feat_req_someipsd_27);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library offers a service - verify SD is sent to port 30490
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

        // Keep offering for a while
        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    // Raw socket listens on port 30490 to verify SD uses this port
    sim.client("raw_observer", async move {
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut received_sd = false;

        for _ in 0..20 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, _sd_msg)) = parse_sd_message(&buf[..len]) {
                    received_sd = true;
                    eprintln!("Received SD message on port 30490 from {}", from);

                    // Verify we received on port 30490
                    let local = sd_socket.local_addr()?;
                    assert_eq!(local.port(), 30490, "SD must use port 30490");
                    break;
                }
            }
        }

        assert!(received_sd, "Should receive SD messages on port 30490");
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someipsd_147: OfferService entry type = 0x01
#[test_log::test]
fn sd_offer_entry_type_wire_format() {
    covers!(feat_req_someipsd_147);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library side - offers a service
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

    // Raw socket captures and verifies OfferService entry
    sim.client("raw_observer", async move {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut found_offer = false;

        for _ in 0..5 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), socket.recv_from(&mut buf)).await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 {
                            // OfferService
                            found_offer = true;
                            assert_eq!(
                                entry.service_id, 0x1234,
                                "OfferService should advertise our service"
                            );
                        }
                    }
                }
            }
        }

        assert!(
            found_offer,
            "Server should send OfferService entries (type 0x01)"
        );
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someipsd_47: Service Entry Type Format (includes TTL field)
/// feat_req_someipsd_252: OfferService entry type shall be used to offer a service
///
/// Verify OfferService entry contains configured offer_ttl on the wire
#[test_log::test]
fn offer_service_ttl_on_wire() {
    covers!(feat_req_someipsd_47, feat_req_someipsd_252);
    const CUSTOM_OFFER_TTL: u32 = 1800; // 30 minutes

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server with custom offer_ttl
    sim.host("server", || async {
        let runtime = recentip::configure()
            .offer_ttl(CUSTOM_OFFER_TTL)
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

        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Raw socket verifies TTL on wire
    sim.client("raw_observer", async move {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut found_offer = false;

        for _ in 0..20 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), socket.recv_from(&mut buf)).await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        eprintln!("Received SD entry: type=0x{:02x}, service=0x{:04x}, ttl={}",
                            entry.entry_type as u8, entry.service_id, entry.ttl);
                        // OfferService with TTL > 0 (TTL=0 means StopOffer)
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 && entry.ttl > 0 {
                            found_offer = true;
                            assert_eq!(
                                entry.ttl, CUSTOM_OFFER_TTL,
                                "OfferService TTL on wire should match configured offer_ttl (expected {}, got {})",
                                CUSTOM_OFFER_TTL, entry.ttl
                            );
                        }
                    }
                }
            }

            if found_offer {
                break;
            }
        }

        assert!(found_offer, "Should receive OfferService entry");
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someipsd_109: Eventgroup Entry Type Format (includes TTL field)
/// feat_req_someipsd_431: Client sends SubscribeEventgroup entries
///
/// Verify SubscribeEventgroup entry contains configured subscribe_ttl on the wire
#[test_log::test]
fn subscribe_eventgroup_ttl_on_wire() {
    covers!(feat_req_someipsd_109, feat_req_someipsd_431);
    const CUSTOM_SUBSCRIBE_TTL: u32 = 900; // 15 minutes

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Raw server sends offers, receives subscribe with custom TTL
    sim.host("raw_server", || async {
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_server").to_string().parse().unwrap();

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
        let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

        let mut buf = [0u8; 1500];
        let mut found_subscribe = false;

        let mut next_unicast_session_id: u16 = 1;
        let mut next_multicast_session_id: u16 = 1;

        for _ in 0..30 {
            let offer = build_sd_offer_with_session(0x1234, 0x0001, 1, 0, my_ip, 30509, 3600, next_multicast_session_id, true, false);
            sd_socket.send_to(&offer, sd_multicast).await?;
            next_multicast_session_id = next_multicast_session_id.wrapping_add(1);
            tokio::time::sleep(Duration::from_millis(100)).await;


            while let Ok(Ok((len, from))) = tokio::time::timeout(Duration::from_millis(50), sd_socket.recv_from(&mut buf))
                    .await {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x06 && entry.service_id == 0x1234 {
                            found_subscribe = true;
                            assert_eq!(
                                entry.ttl, CUSTOM_SUBSCRIBE_TTL,
                                "SubscribeEventgroup TTL on wire should match configured subscribe_ttl (expected {}, got {})",
                                CUSTOM_SUBSCRIBE_TTL, entry.ttl
                            );

                            // Send ACK back via SD socket
                            let ack = build_sd_subscribe_ack_with_session(
                                entry.service_id,
                                entry.instance_id,
                                entry.major_version,
                                entry.eventgroup_id,
                                CUSTOM_SUBSCRIBE_TTL, // Echo client's TTL per spec
                                next_unicast_session_id
                            );
                            next_unicast_session_id = next_unicast_session_id.wrapping_add(1);
                            sd_socket.send_to(&ack, from).await?;
                            tokio::time::sleep(Duration::from_millis(100)).await;
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

    // Client with custom subscribe_ttl
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .subscribe_ttl(CUSTOM_SUBSCRIBE_TTL)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Should discover service via SD")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someipsd_47: Service Entry Type Format (includes TTL field)
/// feat_req_someipsd_238: FindService entry type shall be used for finding service instances
///
/// Verify FindService entry contains configured find_ttl on the wire
#[test_log::test]
fn find_service_ttl_on_wire() {
    covers!(feat_req_someipsd_47, feat_req_someipsd_238);
    const CUSTOM_FIND_TTL: u32 = 600; // 10 minutes

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Raw server receives FindService messages and verifies TTL
    sim.client("raw_server",  async {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut found_find = false;

        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), socket.recv_from(&mut buf)).await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        eprintln!(
                            "Received SD entry: type=0x{:02x}, service=0x{:04x}, ttl={}",
                            entry.entry_type as u8, entry.service_id, entry.ttl
                        );
                        // FindService entry type is 0x00
                        if entry.entry_type as u8 == 0x00 && entry.service_id == 0x1234 {
                            found_find = true;
                            assert_eq!(
                                entry.ttl, CUSTOM_FIND_TTL,
                                "FindService TTL on wire should match configured find_ttl (expected {}, got {})",
                                CUSTOM_FIND_TTL, entry.ttl
                            );
                        }
                    }
                }
            }

            if found_find {
                break;
            }
        }

        assert!(found_find, "Should receive FindService entry");
        Ok(())
    });

    // Client with custom find_ttl
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .find_ttl(CUSTOM_FIND_TTL)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Start finding service - calling  triggers Command::Find which sends FindService
        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));

        // Use tokio::select to attempt available() but don't wait forever
        // (service won't be found, but the FindService message will be sent)
        let _ = tokio::time::timeout(Duration::from_millis(500), proxy).await;

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someipsd_614: SubscribeEventgroupAck TTL shall be the same as in SubscribeEventgroup
/// Verify that the server echoes the client's TTL in the Ack, not the server's offer_ttl
#[test_log::test]
fn subscribe_ack_echoes_client_ttl_123() {
    covers!(feat_req_someipsd_614);
    const CLIENT_SUBSCRIBE_TTL: u32 = 123; // Unusual value to ensure it's echoed, not server's default

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server with default offer_ttl (3600) - we verify the Ack uses client's TTL, not this
    sim.host("server", || async {
        let runtime = recentip::configure().start_turmoil().await.unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Keep server running
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Raw client: manually sends Subscribe, receives Ack, verifies TTL
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Bind to SD multicast to receive offers
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService to discover the server
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // OfferService is type 0x01 with TTL > 0
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
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_client").to_string().parse().unwrap();

        // Build and send SubscribeEventgroup with custom TTL
        let subscribe = build_sd_subscribe_with_udp_endpoint(
            0x1234, 0x0001, 1, 0x0001, CLIENT_SUBSCRIBE_TTL,
            my_ip,
            12345
        );
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for SubscribeEventgroupAck
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // SubscribeEventgroupAck is type 0x07 with TTL > 0
                        if entry.entry_type as u8 == 0x07
                            && entry.service_id == 0x1234
                            && entry.ttl > 0
                        {
                            found_ack = true;
                            assert_eq!(
                                entry.ttl, CLIENT_SUBSCRIBE_TTL,
                                "feat_req_someipsd_614: SubscribeEventgroupAck TTL should echo client's TTL (expected {}, got {})",
                                CLIENT_SUBSCRIBE_TTL, entry.ttl
                            );
                        }
                    }
                }
            }

            if found_ack {
                break;
            }
        }

        assert!(found_ack, "Should receive SubscribeEventgroupAck entry");
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someipsd_614: SubscribeEventgroupAck TTL shall be the same as in SubscribeEventgroup
/// Verify that the server echoes the client's TTL in the Ack, not the server's offer_ttl
#[test_log::test]
fn subscribe_ack_echoes_client_ttl_1000_000() {
    covers!(feat_req_someipsd_614);
    const CLIENT_SUBSCRIBE_TTL: u32 = 1000_000; // Unusual value to ensure it's echoed, not server's default

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server with default offer_ttl (3600) - we verify the Ack uses client's TTL, not this
    sim.host("server", || async {
        let runtime = recentip::configure().start_turmoil().await.unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Keep server running
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Raw client: manually sends Subscribe, receives Ack, verifies TTL
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Bind to SD multicast to receive offers
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService to discover the server
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // OfferService is type 0x01 with TTL > 0
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
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_client").to_string().parse().unwrap();

        // Build and send SubscribeEventgroup with custom TTL
        let subscribe = build_sd_subscribe_with_udp_endpoint(
            0x1234, 0x0001, 1, 0x0001, CLIENT_SUBSCRIBE_TTL,
            my_ip,
            12345
        );
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for SubscribeEventgroupAck
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // SubscribeEventgroupAck is type 0x07 with TTL > 0
                        if entry.entry_type as u8 == 0x07
                            && entry.service_id == 0x1234
                            && entry.ttl > 0
                        {
                            found_ack = true;
                            assert_eq!(
                                entry.ttl, CLIENT_SUBSCRIBE_TTL,
                                "feat_req_someipsd_614: SubscribeEventgroupAck TTL should echo client's TTL (expected {}, got {})",
                                CLIENT_SUBSCRIBE_TTL, entry.ttl
                            );
                        }
                    }
                }
            }

            if found_ack {
                break;
            }
        }

        assert!(found_ack, "Should receive SubscribeEventgroupAck entry");
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_someipsd_614: SubscribeEventgroupAck TTL shall be the same as in SubscribeEventgroup
/// Verify that the server echoes the client's TTL in the Ack, not the server's offer_ttl
#[test_log::test]
fn subscribe_ack_echoes_client_ttl_1() {
    covers!(feat_req_someipsd_614);
    const CLIENT_SUBSCRIBE_TTL: u32 = 1; // Unusual value to ensure it's echoed, not server's default

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server with default offer_ttl (3600) - we verify the Ack uses client's TTL, not this
    sim.host("server", || async {
        let runtime = recentip::configure().start_turmoil().await.unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Keep server running
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Raw client: manually sends Subscribe, receives Ack, verifies TTL
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Bind to SD multicast to receive offers
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService to discover the server
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // OfferService is type 0x01 with TTL > 0
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
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_client").to_string().parse().unwrap();

        // Build and send SubscribeEventgroup with custom TTL
        let subscribe = build_sd_subscribe_with_udp_endpoint(
            0x1234, 0x0001, 1, 0x0001, CLIENT_SUBSCRIBE_TTL,
            my_ip,
            12345
        );
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for SubscribeEventgroupAck
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // SubscribeEventgroupAck is type 0x07 with TTL > 0
                        if entry.entry_type as u8 == 0x07
                            && entry.service_id == 0x1234
                            && entry.ttl > 0
                        {
                            found_ack = true;
                            assert_eq!(
                                entry.ttl, CLIENT_SUBSCRIBE_TTL,
                                "feat_req_someipsd_614: SubscribeEventgroupAck TTL should echo client's TTL (expected {}, got {})",
                                CLIENT_SUBSCRIBE_TTL, entry.ttl
                            );
                        }
                    }
                }
            }

            if found_ack {
                break;
            }
        }

        assert!(found_ack, "Should receive SubscribeEventgroupAck entry");
        Ok(())
    });

    sim.run().unwrap();
}
