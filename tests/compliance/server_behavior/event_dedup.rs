//! Event Deduplication Tests (Server Under Test)
//!
//! Tests that verify server correctly de-duplicates events when a client
//! subscribes to multiple eventgroups containing the same event.
//!
//! # Test Setup
//! - Library: Acts as **server** (offers service, sends events)
//! - Raw socket: Acts as **client** (subscribes via raw SD, counts event packets)
//!
//! # Requirements Covered
//! - feat_req_recentipsd_1168: Server shall not send duplicate events for overlapping eventgroups

use super::helpers::{
    build_sd_subscribe_with_tcp_endpoint, build_sd_subscribe_with_udp_endpoint, covers,
    parse_header, parse_sd_message, TEST_SERVICE_ID, TEST_SERVICE_VERSION,
};
use recentip::prelude::*;
use recentip::wire::MessageType;
use recentip::ServiceEvent;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::AsyncReadExt;

/// Wire-level event ID for our test event (0x8001)
const TEST_EVENT_ID: u16 = 0x8001;

/// feat_req_recentipsd_1168: Server shall not send duplicate events for overlapping eventgroups (TCP).
///
/// Per spec: "With TCP the client needs to open a connection to this port before subscription,
/// because this is the TCP connection the server uses for sending events."
///
/// Flow:
/// 1. Client discovers server via OfferService (extracts server's TCP endpoint)
/// 2. Client opens TCP connection TO the server's TCP port
/// 3. Client sends Subscribe messages via SD (UDP)
/// 4. Server sends events over the client-initiated TCP connection
/// 5. Client should receive exactly ONE event packet (deduplicated)
#[test_log::test]
fn server_deduplicates_events_for_overlapping_eventgroups_tcp() {
    covers!(feat_req_recentipsd_1168);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service and sends events
    sim.host("server", || async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        // Create event that belongs to both eventgroups
        let eventgroup1 = EventgroupId::new(0x0001).unwrap();
        let eventgroup2 = EventgroupId::new(0x0002).unwrap();
        let event_id = EventId::new(TEST_EVENT_ID).unwrap();

        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup1)
            .eventgroup(eventgroup2)
            .create()
            .await
            .unwrap();

        // Wait for both subscriptions
        let mut subs_received = 0;
        while subs_received < 2 {
            match tokio::time::timeout(Duration::from_secs(5), offering.next()).await {
                Ok(Some(ServiceEvent::Subscribe { .. })) => {
                    subs_received += 1;
                }
                Ok(Some(_)) | Ok(None) | Err(_) => break,
            }
        }

        // Send the event - server should de-duplicate and send only once
        event_handle.notify(b"dedup_test").await.unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    // Raw socket client: connects to server's TCP port, subscribes to both eventgroups, counts event packets
    sim.client("raw_client", async move {
        use recentip::wire::{SdOption, L4Protocol};

        tokio::time::sleep(Duration::from_millis(100)).await;

        // SD socket for subscribe messages (port 30490)
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let mut server_sd_endpoint: Option<SocketAddr> = None;
        let mut server_tcp_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService and extract TCP endpoint from options
        for _ in 0..50 {
            let result =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // OfferService = 0x01 with TTL > 0
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == TEST_SERVICE_ID
                            && entry.ttl > 0
                        {
                            server_sd_endpoint = Some(SocketAddr::new(from.ip(), 30490));

                            // Extract TCP endpoint from options
                            for option in &sd_msg.options {
                                if let SdOption::Ipv4Endpoint { addr, port, protocol: L4Protocol::Tcp } = option {
                                    server_tcp_endpoint = Some(SocketAddr::new(
                                        std::net::IpAddr::V4(*addr),
                                        *port,
                                    ));
                                }
                            }
                        }
                    }
                }
            }

            if server_sd_endpoint.is_some() && server_tcp_endpoint.is_some() {
                break;
            }
        }

        let server_sd_ep = server_sd_endpoint.expect("Should discover server via SD");
        let server_tcp_ep = server_tcp_endpoint.expect("Should get server's TCP endpoint from OfferService");

        // Per spec: Client opens TCP connection BEFORE subscribing
        let mut tcp_stream = turmoil::net::TcpStream::connect(server_tcp_ep).await?;

        // Get client IP for endpoint option
        let client_ip: std::net::Ipv4Addr =
            turmoil::lookup("raw_client").to_string().parse().unwrap();

        // Get local port of our TCP connection (to include in Subscribe endpoint option)
        let local_port = tcp_stream.local_addr()?.port();

        // Subscribe to eventgroup 1 (with TCP endpoint identifying our connection)
        let subscribe1 = build_sd_subscribe_with_tcp_endpoint(
            TEST_SERVICE_ID,
            0x0001, // instance
            TEST_SERVICE_VERSION.0,
            0x0001, // eventgroup 1
            3600,   // TTL
            client_ip,
            local_port, // our TCP connection's local port
        );
        sd_socket.send_to(&subscribe1, server_sd_ep).await?;

        // Wait briefly for ACK processing
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Subscribe to eventgroup 2 (same TCP endpoint)
        let subscribe2 = build_sd_subscribe_with_tcp_endpoint(
            TEST_SERVICE_ID,
            0x0001, // instance
            TEST_SERVICE_VERSION.0,
            0x0002, // eventgroup 2
            3600,   // TTL
            client_ip,
            local_port, // same TCP connection
        );
        sd_socket.send_to(&subscribe2, server_sd_ep).await?;

        // Wait for ACKs
        let mut acks_received = 0;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // SubscribeEventgroupAck = 0x07 with TTL > 0
                        if entry.entry_type as u8 == 0x07
                            && entry.service_id == TEST_SERVICE_ID
                            && entry.ttl > 0
                        {
                            acks_received += 1;
                        }
                    }
                }
            }

            if acks_received >= 2 {
                break;
            }
        }

        assert!(acks_received >= 2, "Should receive ACKs for both eventgroups, got {}", acks_received);

        // Read events from the TCP stream we opened to the server
        let mut event_packets_received = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);

        while tokio::time::Instant::now() < deadline {
            // SOME/IP over TCP: read 16-byte header first
            let mut header_buf = [0u8; 16];
            let read_result = tokio::time::timeout(
                Duration::from_millis(500),
                tcp_stream.read_exact(&mut header_buf)
            ).await;

            match read_result {
                Ok(Ok(_)) => {
                    if let Some(header) = parse_header(&header_buf) {
                        // Read the rest of the message (payload)
                        let payload_len = header.length as usize - 8; // length includes 8 bytes after message_id+length
                        if payload_len > 0 && payload_len < 1400 {
                            let mut payload = vec![0u8; payload_len];
                            let _ = tcp_stream.read_exact(&mut payload).await;
                        }

                        // Check if this is our event (NOTIFICATION with our event ID)
                        if header.service_id == TEST_SERVICE_ID
                            && header.method_id == TEST_EVENT_ID
                            && header.message_type == MessageType::Notification
                        {
                            event_packets_received += 1;
                        }
                    }
                }
                Ok(Err(_)) | Err(_) => break,
            }
        }

        // THE KEY ASSERTION: Server should send only ONE packet
        assert_eq!(
            event_packets_received, 1,
            "Server should send exactly ONE event packet despite client subscribing to 2 eventgroups. \
             Per feat_req_recentipsd_1168: server de-duplicates by subscriber endpoint. \
             Got {} packets.",
            event_packets_received
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_1168: Server shall not send duplicate events for overlapping eventgroups.
///
/// When a client subscribes to multiple eventgroups that share the same event,
/// the server MUST send only ONE event packet to that client (per notify call),
/// not one per eventgroup subscription.
///
/// This test uses raw sockets to verify the wire-level behavior:
/// 1. Raw socket client subscribes to eventgroup 1
/// 2. Raw socket client subscribes to eventgroup 2 (same endpoint)
/// 3. Server sends event that belongs to both eventgroups
/// 4. Raw socket should receive exactly ONE packet containing the event
#[test_log::test]
fn server_deduplicates_events_for_overlapping_eventgroups_udp() {
    covers!(feat_req_recentipsd_1168);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service and sends events
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

        // Create event that belongs to both eventgroups
        let eventgroup1 = EventgroupId::new(0x0001).unwrap();
        let eventgroup2 = EventgroupId::new(0x0002).unwrap();
        let event_id = EventId::new(TEST_EVENT_ID).unwrap();

        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup1)
            .eventgroup(eventgroup2)
            .create()
            .await
            .unwrap();

        // Wait for both subscriptions
        let mut subs_received = 0;
        while subs_received < 2 {
            match tokio::time::timeout(Duration::from_secs(5), offering.next()).await {
                Ok(Some(ServiceEvent::Subscribe { .. })) => {
                    subs_received += 1;
                }
                Ok(Some(_)) | Ok(None) | Err(_) => break,
            }
        }

        // Send the event - server should de-duplicate and send only once
        event_handle.notify(b"dedup_test").await.unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    // Raw socket client: subscribes to both eventgroups, counts event packets
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // SD socket for subscribe messages (port 30490)
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        // Event socket for receiving events (separate port)
        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:40000").await?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService
        for _ in 0..50 {
            let result =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // OfferService = 0x01 with TTL > 0
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == TEST_SERVICE_ID
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

        // Get client IP for endpoint option using turmoil::lookup
        let client_ip: std::net::Ipv4Addr =
            turmoil::lookup("raw_client").to_string().parse().unwrap();

        // Subscribe to eventgroup 1 (with UDP endpoint pointing to our event socket)
        let subscribe1 = build_sd_subscribe_with_udp_endpoint(
            TEST_SERVICE_ID,
            0x0001, // instance
            TEST_SERVICE_VERSION.0,
            0x0001, // eventgroup 1
            3600,   // TTL
            client_ip,
            40000,  // our event port
        );
        sd_socket.send_to(&subscribe1, server_ep).await?;

        // Wait briefly for ACK processing
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Subscribe to eventgroup 2 (same endpoint)
        let subscribe2 = build_sd_subscribe_with_udp_endpoint(
            TEST_SERVICE_ID,
            0x0001, // instance
            TEST_SERVICE_VERSION.0,
            0x0002, // eventgroup 2
            3600,   // TTL
            client_ip,
            40000,  // same event port
        );
        sd_socket.send_to(&subscribe2, server_ep).await?;

        // Wait for ACKs
        let mut acks_received = 0;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // SubscribeEventgroupAck = 0x07 with TTL > 0
                        if entry.entry_type as u8 == 0x07
                            && entry.service_id == TEST_SERVICE_ID
                            && entry.ttl > 0
                        {
                            acks_received += 1;
                        }
                    }
                }
            }

            if acks_received >= 2 {
                break;
            }
        }

        assert!(acks_received >= 2, "Should receive ACKs for both eventgroups, got {}", acks_received);

        // Now wait for event packets and count them
        let mut event_packets_received = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(13);

        while tokio::time::Instant::now() < deadline {
            let result =
                tokio::time::timeout(Duration::from_millis(100), event_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some(header) = parse_header(&buf[..len]) {
                    // Check if this is our event (NOTIFICATION with our event ID)
                    if header.service_id == TEST_SERVICE_ID
                        && header.method_id == TEST_EVENT_ID
                        && header.message_type == MessageType::Notification
                    {
                        event_packets_received += 1;
                    }
                }
            }
        }

        // THE KEY ASSERTION: Server should send only ONE packet
        assert_eq!(
            event_packets_received, 1,
            "Server should send exactly ONE event packet despite client subscribing to 2 eventgroups. \
             Per feat_req_recentipsd_1168: server de-duplicates by subscriber endpoint. \
             Got {} packets.",
            event_packets_received
        );

        Ok(())
    });

    sim.run().unwrap();
}

/// Server sends events to each unique subscriber endpoint.
///
/// When multiple clients subscribe to eventgroups containing the same event,
/// the server MUST send the event to EACH client endpoint (no deduplication
/// across different endpoints, only within a single endpoint).
///
/// This test verifies:
/// 1. Client 1 on endpoint A subscribes to eventgroup 1
/// 2. Client 2 on endpoint B subscribes to eventgroup 2
/// 3. Server sends event belonging to both eventgroups
/// 4. Both clients receive exactly ONE packet each (2 total packets sent)
#[test_log::test]
fn server_sends_events_to_each_unique_endpoint() {
    covers!(feat_req_recentipsd_1168);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service and sends events
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

        // Create event that belongs to both eventgroups
        let eventgroup1 = EventgroupId::new(0x0001).unwrap();
        let eventgroup2 = EventgroupId::new(0x0002).unwrap();
        let event_id = EventId::new(TEST_EVENT_ID).unwrap();

        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup1)
            .eventgroup(eventgroup2)
            .create()
            .await
            .unwrap();

        // Wait for both subscriptions (from different clients)
        let mut subs_received = 0;
        while subs_received < 2 {
            match tokio::time::timeout(Duration::from_secs(5), offering.next()).await {
                Ok(Some(ServiceEvent::Subscribe { .. })) => {
                    subs_received += 1;
                }
                Ok(Some(_)) | Ok(None) | Err(_) => break,
            }
        }

        // Send the event - server should send to both endpoints
        event_handle.notify(b"multi_client_test").await.unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    // Client 1: subscribes to eventgroup 1 on port 40001
    sim.client("client1", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:40001").await?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService
        for _ in 0..50 {
            let result =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == TEST_SERVICE_ID
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

        let server_ep = server_endpoint.expect("Client1 should discover server");
        let client_ip: std::net::Ipv4Addr = turmoil::lookup("client1").to_string().parse().unwrap();

        // Subscribe to eventgroup 1
        let subscribe = build_sd_subscribe_with_udp_endpoint(
            TEST_SERVICE_ID,
            0x0001,
            TEST_SERVICE_VERSION.0,
            0x0001, // eventgroup 1
            3600,
            client_ip,
            40001, // client1's event port
        );
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for ACK
        let mut ack_received = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x07
                            && entry.service_id == TEST_SERVICE_ID
                            && entry.ttl > 0
                        {
                            ack_received = true;
                            break;
                        }
                    }
                }
            }

            if ack_received {
                break;
            }
        }

        assert!(ack_received, "Client1 should receive ACK");

        // Wait for event packet
        let mut event_packets_received = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);

        while tokio::time::Instant::now() < deadline {
            let result =
                tokio::time::timeout(Duration::from_millis(100), event_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some(header) = parse_header(&buf[..len]) {
                    if header.service_id == TEST_SERVICE_ID
                        && header.method_id == TEST_EVENT_ID
                        && header.message_type == MessageType::Notification
                    {
                        event_packets_received += 1;
                    }
                }
            }
        }

        assert_eq!(
            event_packets_received, 1,
            "Client1 should receive exactly ONE event packet, got {}",
            event_packets_received
        );

        Ok(())
    });

    // Client 2: subscribes to eventgroup 2 on port 40002
    sim.client("client2", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:40002").await?;

        let mut buf = [0u8; 1500];
        let mut server_endpoint: Option<SocketAddr> = None;

        // Wait for OfferService
        for _ in 0..50 {
            let result =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01
                            && entry.service_id == TEST_SERVICE_ID
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

        let server_ep = server_endpoint.expect("Client2 should discover server");
        let client_ip: std::net::Ipv4Addr = turmoil::lookup("client2").to_string().parse().unwrap();

        // Subscribe to eventgroup 2
        let subscribe = build_sd_subscribe_with_udp_endpoint(
            TEST_SERVICE_ID,
            0x0001,
            TEST_SERVICE_VERSION.0,
            0x0002, // eventgroup 2
            3600,
            client_ip,
            40002, // client2's event port
        );
        sd_socket.send_to(&subscribe, server_ep).await?;

        // Wait for ACK
        let mut ack_received = false;
        for _ in 0..30 {
            let result =
                tokio::time::timeout(Duration::from_millis(100), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x07
                            && entry.service_id == TEST_SERVICE_ID
                            && entry.ttl > 0
                        {
                            ack_received = true;
                            break;
                        }
                    }
                }
            }

            if ack_received {
                break;
            }
        }

        assert!(ack_received, "Client2 should receive ACK");

        // Wait for event packet
        let mut event_packets_received = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);

        while tokio::time::Instant::now() < deadline {
            let result =
                tokio::time::timeout(Duration::from_millis(100), event_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, _from))) = result {
                if let Some(header) = parse_header(&buf[..len]) {
                    if header.service_id == TEST_SERVICE_ID
                        && header.method_id == TEST_EVENT_ID
                        && header.message_type == MessageType::Notification
                    {
                        event_packets_received += 1;
                    }
                }
            }
        }

        assert_eq!(
            event_packets_received, 1,
            "Client2 should receive exactly ONE event packet, got {}",
            event_packets_received
        );

        Ok(())
    });

    sim.run().unwrap();
}
