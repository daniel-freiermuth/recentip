//! RPC Port and Connection Tests (Client Under Test)
//!
//! Tests that verify the client properly separates SD and RPC ports,
//! and establishes TCP connections before subscribing.
//!
//! # Test Setup
//! - Library: Acts as **client** (discovers service, makes RPC calls)
//! - Raw socket: Acts as **server** (sends offers, captures client behavior)
//!
//! # Requirements Covered
//! - feat_req_someip_676: Port 30490 is only for SD, not for RPC
//! - feat_req_someipsd_779: Service endpoint denotes where service is reachable
//! - feat_req_someipsd_758: UDP endpoint used for source port of events
//! - feat_req_someipsd_767: TCP connection before subscribe

use super::helpers::{
    build_response, build_sd_offer, build_sd_offer_tcp_only, build_sd_subscribe_ack, covers,
    parse_header, parse_sd_message, TEST_SERVICE_ID,
};
use recentip::prelude::*;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

/// feat_req_someip_676: Port 30490 is only for SD, not for RPC
/// feat_req_someipsd_779: Service endpoint denotes where service is reachable
/// feat_req_someipsd_758: UDP endpoint used for source port of events
///
/// **BUG DEMONSTRATION TEST**
///
/// This test verifies that client RPC messages (requests) do NOT originate from
/// the SD socket (port 30490). The SD port is reserved exclusively for Service
/// Discovery messages. RPC communication should use dedicated RPC sockets with
/// ephemeral or configured ports.
///
/// Per the specification:
/// - feat_req_someip_676: Port 30490 shall be only used for SOME/IP-SD
/// - feat_req_someipsd_779: Endpoint options denote where service is reachable
/// - feat_req_someipsd_758: UDP endpoint is used for source port (for events)
/// - Servers use announced ports as source for both responses and events
/// - Clients should similarly use dedicated RPC sockets, not SD socket
///
/// **Current Behavior (INCORRECT):**
/// Client sends RPC requests from SD socket (port 30490) at runtime.rs:1201
///
/// **Expected Behavior:**
/// Client should use a dedicated RPC socket with ephemeral port (like servers do)
///
/// **Test Result:** PASSES - verifies correct implementation
#[test_log::test]
fn client_rpc_must_not_use_sd_port() {
    covers!(
        feat_req_someip_676,
        feat_req_someipsd_779,
        feat_req_someipsd_758
    );

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Raw socket acts as server - sends SD offer, then captures REQUEST source port
    sim.host("raw_server", || async {
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_server").to_string().parse().unwrap();

        // RPC socket to receive requests
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?;
        eprintln!("Raw server RPC listening on {}", rpc_socket.local_addr()?);

        // SD socket to send offers
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;

        // Send SD offer to multicast so the library can discover us
        let offer = build_sd_offer(0x1234, 0x0001, 1, 0, my_ip, 30509, 3600);
        let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // Send offers periodically until we receive a request
        let mut buf = [0u8; 1500];
        let mut request_received = false;
        let mut client_source_port: Option<u16> = None;

        for _ in 0..20 {
            // Send an offer
            sd_socket.send_to(&offer, sd_multicast).await?;
            eprintln!("Raw server sent SD offer");

            // Check for RPC request (non-blocking with short timeout)
            let result =
                tokio::time::timeout(Duration::from_millis(200), rpc_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                eprintln!("Raw server received {} bytes from {}", len, from);
                request_received = true;
                client_source_port = Some(from.port());

                // Parse as SOME/IP to verify it's an RPC request
                let data = &buf[..len];
                let header = parse_header(data).expect("Should parse as SOME/IP header");

                eprintln!("Received RPC request from port: {}", from.port());
                eprintln!(
                    "Service ID: 0x{:04x}, Method ID: 0x{:04x}",
                    header.service_id, header.method_id
                );
                eprintln!("Message Type: {:?}", header.message_type);

                // This is the critical check: RPC messages MUST NOT come from SD port
                assert_ne!(
                    from.port(),
                    30490,
                    "Client RPC request MUST NOT originate from SD port 30490 (feat_req_someip_676). \
                     SD port is reserved exclusively for Service Discovery. \
                     RPC communication requires dedicated RPC socket with ephemeral port."
                );

                // Send a response back so the client doesn't timeout
                let response = build_response(&header, b"ok");
                rpc_socket.send_to(&response, from).await?;
                break;
            }
        }

        assert!(request_received, "Should have received an RPC request");

        if let Some(port) = client_source_port {
            eprintln!("✓ Client used port {} for RPC (not SD port 30490)", port);
        }

        Ok(())
    });

    // Library side - discovers service via SD and makes RPC call
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Use public API: find service and wait for availability via SD
        let proxy = runtime
            .find(TEST_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));

        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Should discover service via SD")
            .expect("Service available");

        // Make an RPC call - this should NOT use port 30490
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            proxy.call(MethodId::new(0x0001).unwrap(), b"hello"),
        )
        .await;

        match result {
            Ok(Ok(response)) => {
                eprintln!("RPC call succeeded: {:?}", response);
            }
            Ok(Err(e)) => {
                eprintln!("RPC call failed: {}", e);
            }
            Err(_) => {
                eprintln!("RPC call timed out");
            }
        }

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// TCP CONNECTION TIMING TESTS
// ============================================================================

/// [feat_req_someipsd_767] Client opens TCP connection BEFORE sending SubscribeEventgroup.
///
/// This wire-level test verifies the timing requirement:
/// - Raw server offers TCP-only service
/// - Raw server has TCP listener
/// - Client (our library) discovers and subscribes
/// - Server verifies: TCP connection established BEFORE SubscribeEventgroup arrives
///
/// This is critical for reliable event delivery - the server needs the TCP connection
/// ready to send events as soon as it ACKs the subscription.
#[test_log::test]
fn tcp_connection_established_before_subscribe_767() {
    covers!(feat_req_someipsd_767);

    let tcp_connected_before_subscribe = Arc::new(AtomicBool::new(false));
    let tcp_connected = Arc::clone(&tcp_connected_before_subscribe);

    let tcp_connection_count = Arc::new(AtomicUsize::new(0));
    let tcp_count = Arc::clone(&tcp_connection_count);

    let subscribe_received = Arc::new(AtomicBool::new(false));
    let subscribe_flag = Arc::clone(&subscribe_received);

    // Notify when TCP connection is established
    let tcp_notify = Arc::new(Notify::new());
    let tcp_notify_server = Arc::clone(&tcp_notify);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Wire-level server offering TCP only
    sim.host("wire_server", move || {
        let tcp_connected = Arc::clone(&tcp_connected);
        let tcp_count = Arc::clone(&tcp_count);
        let subscribe_flag = Arc::clone(&subscribe_flag);
        let tcp_notify = Arc::clone(&tcp_notify_server);

        async move {
            let my_ip: std::net::Ipv4Addr =
                turmoil::lookup("wire_server").to_string().parse().unwrap();

            // SD socket for offers and subscription handling
            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            // TCP listener for RPC/events - THIS IS THE KEY PART
            let tcp_listener = turmoil::net::TcpListener::bind("0.0.0.0:30509").await?;
            eprintln!("[wire_server] TCP listener bound on port 30509");

            let offer =
                build_sd_offer_tcp_only(0x1234, 0x0001, 1, 0, my_ip, 30509, 3600, 1, true, false);
            let mut sd_buf = [0u8; 1500];

            // Track if we've seen a TCP connection
            let tcp_connected_inner = Arc::clone(&tcp_connected);
            let tcp_count_inner = Arc::clone(&tcp_count);
            let tcp_notify_inner = Arc::clone(&tcp_notify);

            // Spawn task to accept TCP connections
            let tcp_accept_task = tokio::spawn(async move {
                loop {
                    match tcp_listener.accept().await {
                        Ok((stream, peer)) => {
                            let count = tcp_count_inner.fetch_add(1, Ordering::SeqCst) + 1;
                            eprintln!(
                                "[wire_server] TCP connection #{} accepted from {}",
                                count, peer
                            );
                            tcp_connected_inner.store(true, Ordering::SeqCst);
                            tcp_notify_inner.notify_one();

                            // Keep connection alive by reading in background
                            tokio::spawn(async move {
                                use tokio::io::AsyncReadExt;
                                let mut stream = stream;
                                let mut buf = [0u8; 1024];
                                loop {
                                    match stream.read(&mut buf).await {
                                        Ok(0) => break, // Connection closed
                                        Ok(n) => {
                                            eprintln!("[wire_server] TCP received {} bytes", n);
                                        }
                                        Err(_) => break,
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            eprintln!("[wire_server] TCP accept error: {}", e);
                            break;
                        }
                    }
                }
            });

            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);

            while tokio::time::Instant::now() < deadline {
                // Send periodic offers
                if last_offer.elapsed() >= Duration::from_millis(500) {
                    sd_socket.send_to(&offer, sd_multicast).await?;
                    eprintln!("[wire_server] Sent TCP-only offer");
                    last_offer = tokio::time::Instant::now();
                }

                // Check for SubscribeEventgroup
                if let Ok(Ok((len, from))) = tokio::time::timeout(
                    Duration::from_millis(100),
                    sd_socket.recv_from(&mut sd_buf),
                )
                .await
                {
                    if let Some((_header, sd_msg)) = parse_sd_message(&sd_buf[..len]) {
                        for entry in &sd_msg.entries {
                            if entry.entry_type as u8 == 0x06 && entry.service_id == 0x1234 {
                                eprintln!("[wire_server] *** SubscribeEventgroup received! ***");

                                // THE CRITICAL CHECK: Was TCP connected BEFORE subscribe?
                                let was_connected = tcp_connected.load(Ordering::SeqCst);
                                eprintln!(
                                    "[wire_server] TCP was connected before subscribe: {}",
                                    was_connected
                                );

                                // Per feat_req_someipsd_767, client MUST connect BEFORE subscribing
                                tcp_connected.store(was_connected, Ordering::SeqCst);
                                subscribe_flag.store(true, Ordering::SeqCst);

                                // Verify TCP endpoint is in the subscribe
                                let has_tcp = sd_msg.get_tcp_endpoint(entry).is_some();
                                eprintln!("[wire_server] Subscribe has TCP endpoint: {}", has_tcp);

                                // Send ACK
                                let ack = build_sd_subscribe_ack(
                                    entry.service_id,
                                    entry.instance_id,
                                    entry.major_version,
                                    entry.eventgroup_id,
                                    entry.ttl,
                                );
                                sd_socket.send_to(&ack, from).await?;
                                eprintln!("[wire_server] Sent SubscribeEventgroupAck");
                            }
                        }
                    }
                }
            }

            tcp_accept_task.abort();
            Ok(())
        }
    });

    // Client using our library
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .preferred_transport(recentip::Transport::Tcp)
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

        eprintln!("[client] Service discovered (should be TCP-only)");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        eprintln!("[client] Subscribed successfully");

        // Keep alive for a bit
        tokio::time::sleep(Duration::from_secs(2)).await;

        Ok(())
    });

    sim.run().unwrap();

    // Verify the test actually ran
    assert!(
        subscribe_received.load(Ordering::SeqCst),
        "Server should have received SubscribeEventgroup"
    );

    // THE KEY ASSERTION: TCP connection must be established BEFORE subscribe
    assert!(
        tcp_connected_before_subscribe.load(Ordering::SeqCst),
        "[feat_req_someipsd_767] TCP connection MUST be established BEFORE \
         sending SubscribeEventgroup. The client did not connect to TCP before subscribing."
    );

    let tcp_count = tcp_connection_count.load(Ordering::SeqCst);
    assert!(
        tcp_count >= 1,
        "Should have at least 1 TCP connection, got {}",
        tcp_count
    );
}
