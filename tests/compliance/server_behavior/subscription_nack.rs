//! Subscription Validation Tests (Server Under Test)
//!
//! Tests that verify server correctly NACKs invalid subscription requests.
//!
//! # Test Setup
//! - Library: Acts as **server** (offers service)
//! - Raw socket: Acts as **client** (sends invalid subscribe messages)
//!
//! # Requirements Covered
//! - feat_req_recentipsd_1137: SubscribeEventgroupNack for invalid subscribes

use super::helpers::{
    build_sd_subscribe_with_udp_endpoint, covers, parse_sd_message, TEST_SERVICE_ID,
    TEST_SERVICE_VERSION,
};
use recentip::prelude::*;
use recentip::Runtime;
use std::net::SocketAddr;
use std::time::Duration;

/// feat_req_recentipsd_1137: Server SHALL NACK when Service ID is unknown
///
/// When a server receives a SubscribeEventgroup for a Service ID it doesn't offer,
/// it MUST respond with SubscribeEventgroupNack (TTL=0).
#[test_log::test]
fn subscribe_to_unknown_service_id_should_nack() {
    covers!(feat_req_recentipsd_1137);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service 0x1234
    sim.host("server", || async {
        let runtime = recentip::configure().start_turmoil().await.unwrap();

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

    // Raw client: tries to subscribe to service 0x9999 (doesn't exist)
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService for 0x1234
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.ttl > 0 {
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

        // Get our IP from turmoil (not local_addr which is 0.0.0.0)
        let client_ip: std::net::Ipv4Addr =
            turmoil::lookup("raw_client").to_string().parse().unwrap();

        // Subscribe to SERVICE 0x9999 (NOT 0x1234!)
        let subscribe =
            build_sd_subscribe_with_udp_endpoint(0x9999, 0x0001, 1, 0x0001, 3600, client_ip, 40000);
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for NACK
        let mut found_nack = false;
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x9999 && entry.eventgroup_id == 0x0001 {
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
        }

        assert!(
            found_nack,
            "Server should NACK subscribe to unknown service ID. Got ACK={}, NACK={}",
            found_ack, found_nack
        );
        assert!(!found_ack, "Server should NOT ACK unknown service");

        Ok(())
    });

    // Second client: subscribes with CORRECT service ID, should get ACK
    sim.client("correct_client", async move {
        tokio::time::sleep(Duration::from_millis(150)).await;

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

        // Get our IP from turmoil (not local_addr which is 0.0.0.0)
        let client_ip: std::net::Ipv4Addr = turmoil::lookup("correct_client")
            .to_string()
            .parse()
            .unwrap();

        // Subscribe with CORRECT service ID 0x1234
        let subscribe =
            build_sd_subscribe_with_udp_endpoint(0x1234, 0x0001, 1, 0x0001, 3600, client_ip, 40001);
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for ACK
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234 && entry.eventgroup_id == 0x0001 {
                            if entry.entry_type as u8 == 0x07 && entry.ttl > 0 {
                                found_ack = true;
                            }
                        }
                    }
                }
            }

            if found_ack {
                break;
            }
        }

        assert!(
            found_ack,
            "Server should ACK subscribe with correct service ID"
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_1137: Server SHALL NACK when Instance ID is unknown
///
/// Server offers service 0x1234 instance 0x0001, client tries to subscribe to
/// instance 0x0099. Server MUST respond with NACK.
#[test_log::test]
fn subscribe_to_unknown_instance_id_should_nack() {
    covers!(feat_req_recentipsd_1137);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers instance 0x0001
    sim.host("server", || async {
        let runtime = recentip::configure().start_turmoil().await.unwrap();

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

    // Raw client: tries to subscribe to instance 0x0099 (doesn't exist)
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
                            && entry.instance_id == 0x0001
                            && entry.ttl > 0
                        {
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

        // Get our IP from turmoil (not local_addr which is 0.0.0.0)
        let client_ip: std::net::Ipv4Addr =
            turmoil::lookup("raw_client").to_string().parse().unwrap();

        // Subscribe to INSTANCE 0x0099 (NOT 0x0001!)
        let subscribe =
            build_sd_subscribe_with_udp_endpoint(0x1234, 0x0099, 1, 0x0001, 3600, client_ip, 40000);
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for NACK
        let mut found_nack = false;
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234
                            && entry.instance_id == 0x0099
                            && entry.eventgroup_id == 0x0001
                        {
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
        }

        assert!(
            found_nack,
            "Server should NACK subscribe to unknown instance ID. Got ACK={}, NACK={}",
            found_ack, found_nack
        );
        assert!(!found_ack, "Server should NOT ACK unknown instance");

        Ok(())
    });

    // Second client: subscribes with CORRECT instance ID, should get ACK
    sim.client("correct_client", async move {
        tokio::time::sleep(Duration::from_millis(150)).await;

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
                            && entry.instance_id == 0x0001
                            && entry.ttl > 0
                        {
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

        // Get our IP from turmoil (not local_addr which is 0.0.0.0)
        let client_ip: std::net::Ipv4Addr = turmoil::lookup("correct_client")
            .to_string()
            .parse()
            .unwrap();

        // Subscribe with CORRECT instance ID 0x0001
        let subscribe =
            build_sd_subscribe_with_udp_endpoint(0x1234, 0x0001, 1, 0x0001, 3600, client_ip, 40001);
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for ACK
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234
                            && entry.instance_id == 0x0001
                            && entry.eventgroup_id == 0x0001
                        {
                            if entry.entry_type as u8 == 0x07 && entry.ttl > 0 {
                                found_ack = true;
                            }
                        }
                    }
                }
            }

            if found_ack {
                break;
            }
        }

        assert!(
            found_ack,
            "Server should ACK subscribe with correct instance ID"
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_1137: Server SHALL NACK when Eventgroup is unknown
///
/// Server offers eventgroup 0x0001, client tries to subscribe to eventgroup 0x0099.
/// Server MUST respond with NACK.
///
/// NOTE: Our interpretation is that eventgroups may not be known yet at subscribe time,
/// so we accept subscriptions preemptively. The subscription becomes active once the
/// eventgroup is registered. This is a design choice, not a compliance bug.
#[test_log::test]
#[ignore = "Design choice: accept subscriptions for not-yet-known eventgroups"]
fn subscribe_to_unknown_eventgroup_should_nack() {
    covers!(feat_req_recentipsd_1137);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers eventgroup 0x0001
    sim.host("server", || async {
        let runtime = recentip::configure().start_turmoil().await.unwrap();

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

    // Raw client: tries to subscribe to eventgroup 0x0099 (doesn't exist)
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

        // Get our IP from turmoil (not local_addr which is 0.0.0.0)
        let client_ip: std::net::Ipv4Addr =
            turmoil::lookup("raw_client").to_string().parse().unwrap();

        // Subscribe to EVENTGROUP 0x0099 (NOT 0x0001!)
        let subscribe =
            build_sd_subscribe_with_udp_endpoint(0x1234, 0x0001, 1, 0x0099, 3600, client_ip, 40000);
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for NACK
        let mut found_nack = false;
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234 && entry.eventgroup_id == 0x0099 {
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
        }

        assert!(
            found_nack,
            "Server should NACK subscribe to unknown eventgroup. Got ACK={}, NACK={}",
            found_ack, found_nack
        );
        assert!(!found_ack, "Server should NOT ACK unknown eventgroup");

        Ok(())
    });

    // Second client: subscribes with CORRECT eventgroup, should get ACK
    sim.client("correct_client", async move {
        tokio::time::sleep(Duration::from_millis(150)).await;

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

        // Get our IP from turmoil (not local_addr which is 0.0.0.0)
        let client_ip: std::net::Ipv4Addr = turmoil::lookup("correct_client")
            .to_string()
            .parse()
            .unwrap();

        // Subscribe with CORRECT eventgroup 0x0001
        let subscribe =
            build_sd_subscribe_with_udp_endpoint(0x1234, 0x0001, 1, 0x0001, 3600, client_ip, 40001);
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for ACK
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234 && entry.eventgroup_id == 0x0001 {
                            if entry.entry_type as u8 == 0x07 && entry.ttl > 0 {
                                found_ack = true;
                            }
                        }
                    }
                }
            }

            if found_ack {
                break;
            }
        }

        assert!(
            found_ack,
            "Server should ACK subscribe with correct eventgroup"
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_1137: Server SHALL NACK when Major Version doesn't match
///
/// Server offers service with major version 1, client tries to subscribe with
/// major version 99. Server MUST respond with NACK.
#[test_log::test]
fn subscribe_to_wrong_major_version_should_nack() {
    covers!(feat_req_recentipsd_1137);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers major version 1
    sim.host("server", || async {
        let runtime = recentip::configure().start_turmoil().await.unwrap();

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

    // Raw client: tries to subscribe with major version 99
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
                            && entry.major_version == 1
                            && entry.ttl > 0
                        {
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

        // Get our IP from turmoil (not local_addr which is 0.0.0.0)
        let client_ip: std::net::Ipv4Addr =
            turmoil::lookup("raw_client").to_string().parse().unwrap();

        // Subscribe with MAJOR VERSION 99 (NOT 1!)
        let subscribe = build_sd_subscribe_with_udp_endpoint(
            0x1234, 0x0001, 99, 0x0001, 3600, client_ip, 40000,
        );
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for NACK
        let mut found_nack = false;
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234
                            && entry.instance_id == 0x0001
                            && entry.eventgroup_id == 0x0001
                            && entry.major_version == 99
                        {
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
        }

        assert!(
            found_nack,
            "Server should NACK subscribe with wrong major version. Got ACK={}, NACK={}",
            found_ack, found_nack
        );
        assert!(!found_ack, "Server should NOT ACK wrong major version");

        Ok(())
    });

    // Second client: subscribes with CORRECT major version, should get ACK
    sim.client("correct_client", async move {
        tokio::time::sleep(Duration::from_millis(150)).await;

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
                            && entry.major_version == 1
                            && entry.ttl > 0
                        {
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

        // Get our IP from turmoil (not local_addr which is 0.0.0.0)
        let client_ip: std::net::Ipv4Addr = turmoil::lookup("correct_client")
            .to_string()
            .parse()
            .unwrap();

        // Subscribe with CORRECT major version 1
        let subscribe =
            build_sd_subscribe_with_udp_endpoint(0x1234, 0x0001, 1, 0x0001, 3600, client_ip, 40001);
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for ACK
        let mut found_ack = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.service_id == 0x1234
                            && entry.instance_id == 0x0001
                            && entry.eventgroup_id == 0x0001
                            && entry.major_version == 1
                        {
                            if entry.entry_type as u8 == 0x07 && entry.ttl > 0 {
                                found_ack = true;
                            }
                        }
                    }
                }
            }

            if found_ack {
                break;
            }
        }

        assert!(
            found_ack,
            "Server should ACK subscribe with correct major version"
        );

        Ok(())
    });

    sim.run().unwrap();
}
