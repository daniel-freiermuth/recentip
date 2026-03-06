//! Split Server Compliance Tests
//!
//! Tests for scenarios where the server is split across multiple hosts:
//! - SD (Service Discovery) handled by one host
//! - RPC/Methods handled by a different host
//!
//! This reflects real-world architectures where SD proxies or gateways
//! advertise services on behalf of other ECUs.
//!
//! Test patterns:
//! - Server side: "on the wire" using raw packet builders
//! - Client side: library under test

use bytes::BytesMut;
use recentip::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

use crate::helpers::{configure_tracing, is_magic_cookie, magic_cookie_server, read_tcp_message};

#[path = "wire_format/helpers.rs"]
mod wire_format_helpers;
use wire_format_helpers::{
    build_notification, build_response, build_sd_offer, build_sd_subscribe_ack, parse_header,
    ParsedSdMessage, SdOfferBuilder, SD_METHOD_ID, SD_SERVICE_ID, SOMEIP_HEADER_SIZE,
};

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

const TEST_SERVICE_ID: u16 = 0x1234;
const TEST_SERVICE_VERSION: (u8, u32) = (1, 0);
const TEST_METHOD_ID: u16 = 0x0001;

// Constants for the server-reboot test (distinct IDs to avoid cross-test interference)
const REBOOT_TEST_SERVICE_A_ID: u16 = 0x1238;
const REBOOT_TEST_SERVICE_B_ID: u16 = 0x1239;
const REBOOT_TEST_SERVICE_VERSION: (u8, u32) = (1, 0);
const REBOOT_TEST_RPC_PORT: u16 = 40002;
const REBOOT_TEST_EVENTGROUP_ID: u16 = 0x0001;

// ============================================================================
// SPLIT SERVER UDP RPC TEST
// ============================================================================

/// [split_server_udp_rpc] UDP service on split host can be called via RPC
///
/// Scenario:
/// - SD Host: Advertises service and provides endpoint pointing to RPC Host
/// - RPC Host: Handles actual method calls on a different IP
/// - Client: Discovers service via SD Host, calls methods on RPC Host
///
/// This tests that the client correctly extracts the RPC endpoint from
/// the SD offer and routes method calls to the correct host.
#[test]
fn split_server_udp_rpc() {
    covers!(
        feat_req_someip_328,   // Request/Response pattern
        feat_req_someip_338,   // Response contains same Request ID
        feat_req_someipsd_011  // IPv4 endpoint option
    );
    configure_tracing();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // SD Host: Advertises the service pointing to RPC Host
    sim.host("sd_host", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let rpc_host_ip: Ipv4Addr = turmoil::lookup("rpc_host").to_string().parse().unwrap();
        let rpc_port = 40000u16;

        // Send SD Offer pointing to RPC Host
        let offer = build_sd_offer(
            TEST_SERVICE_ID,
            0x0001, // instance_id
            TEST_SERVICE_VERSION.0,
            TEST_SERVICE_VERSION.1,
            rpc_host_ip, // RPC endpoint IP
            rpc_port,    // RPC endpoint port
            3,           // TTL
        );

        let multicast_addr: SocketAddr = "239.255.0.1:30490".parse().unwrap();
        sd_socket.send_to(&offer, multicast_addr).await?;
        tracing::info!(
            "SD Host: Sent offer for service {:04X} pointing to {}:{}",
            TEST_SERVICE_ID,
            rpc_host_ip,
            rpc_port
        );
        Ok(())
    });

    // RPC Host: Handles the actual method calls
    sim.host("rpc_host", || async {
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:40000").await?;
        let mut buf = [0u8; 1500];

        // Wait for incoming RPC request
        let result = tokio::time::timeout(Duration::from_secs(10), rpc_socket.recv_from(&mut buf))
            .await
            .expect("Should receive RPC request")
            .expect("recv_from should succeed");
        tracing::info!("RPC Host: Received RPC request from client");

        let (len, client_addr) = result;
        let request_data = &buf[..len];

        // Parse request header
        let header = parse_header(request_data).expect("Valid SOME/IP header");

        // Validate request
        assert_eq!(header.service_id, TEST_SERVICE_ID);
        assert_eq!(header.method_id, TEST_METHOD_ID);
        assert!(header.is_request());
        assert_eq!(header.interface_version, TEST_SERVICE_VERSION.0);

        let request_payload = &request_data[SOMEIP_HEADER_SIZE..];
        assert_eq!(request_payload, b"ping");

        // Build response with same client_id and session_id
        let response = build_response(&header, b"pong");

        // Send response back to client
        rpc_socket.send_to(&response, client_addr).await?;
        tracing::info!("RPC Host: Sent response to client");

        // Keep socket alive
        tokio::time::sleep(Duration::from_secs(3)).await;

        Ok(())
    });

    // Client: Uses library to discover and call the service
    sim.client("client", async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // Find the service (discovers via SD Host)
        tracing::info!("Client: Starting service discovery");
        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery should complete")
            .expect("Service should be found");

        // Call the method (routes to RPC Host)
        tracing::info!("Client: Calling method on discovered service");
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(TEST_METHOD_ID).unwrap(), b"ping"),
        )
        .await
        .expect("Call should not timeout")
        .expect("Call should succeed");

        // Verify response
        assert_eq!(response.payload.as_ref(), b"pong");
        tracing::info!("Client: Received response from RPC Host");

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// SPLIT SERVER UDP PUB/SUB TEST
// ============================================================================

/// [split_server_udp_pubsub] UDP pub/sub on split host  
///
/// Scenario:
/// - SD Host: Advertises service pointing to RPC Host, parses SubscribeEventgroup
///   messages to extract client's subscription endpoint
/// - RPC Host: Sends events to client's subscription endpoint
/// - Client: Subscribes and receives events
///
/// This test verifies that:
/// - Client can discover services offered by a remote SD host
/// - Client can subscribe to event groups when SD and RPC are on different hosts
/// - Events are delivered to the correct endpoint (not the SD port)
/// - Subscribe message parsing extracts the client's subscription endpoint from SD options
#[test]
fn split_server_udp_pubsub() {
    covers!(
        feat_req_someip_352,   // Events describe Publish/Subscribe concept
        feat_req_someipsd_011  // IPv4 endpoint option
    );
    configure_tracing();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Coordination channel: SD host notifies RPC host of client's subscription endpoint
    let (sub_tx, sub_rx) = tokio::sync::mpsc::channel::<SocketAddr>(1);
    let sub_rx = Arc::new(Mutex::new(sub_rx));

    // SD Host: Handles SD messages and signals subscriptions
    sim.host("sd_host", move || {
        let sub_tx = sub_tx.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket.join_multicast_v4(
                "239.255.0.1".parse().unwrap(),
                "0.0.0.0".parse().unwrap(),
            )?;

            let rpc_host_ip: Ipv4Addr = turmoil::lookup("rpc_host").to_string().parse().unwrap();
            let rpc_port = 40000u16;

            // Send SD Offer (basic service endpoint)
            let offer = build_sd_offer(
                TEST_SERVICE_ID,
                0x0001,
                TEST_SERVICE_VERSION.0,
                TEST_SERVICE_VERSION.1,
                rpc_host_ip,
                rpc_port,
                3, // TTL
            );

            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();
            sd_socket.send_to(&offer, sd_multicast).await?;
            tracing::info!("SD Host: Sent service offer");

            // Wait for SubscribeEventgroup from client
            let mut buf = [0u8; 1500];
            let client_subscription_endpoint = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if let Ok((len, addr)) = sd_socket.recv_from(&mut buf).await {
                        tracing::debug!("SD Host: Received {} bytes from {}", len, addr);
                        if let Some(header) = parse_header(&buf[..len]) {
                            tracing::debug!("SD Host: Parsed header - service_id={:04x}, method_id={:04x}",
                                header.service_id, header.method_id);

                            // Check if this is an SD message
                            if header.service_id == SD_SERVICE_ID && header.method_id == SD_METHOD_ID {
                                // Parse SD message payload (skip SOME/IP header)
                                if let Some(sd_msg) = ParsedSdMessage::parse(&buf[SOMEIP_HEADER_SIZE..len]) {
                                    // Check if this message contains SubscribeEventgroup entries
                                    if let Some(sub_entry) = sd_msg.subscribe_entries().next() {
                                        tracing::info!("SD Host: Detected subscription from {}", addr);

                                        // Extract client's subscription endpoint from the Subscribe message
                                        // The endpoint MUST be in the first option of the entry per spec
                                        let client_endpoint = match (
                                            sd_msg.endpoint_port_for_entry(sub_entry),
                                            sd_msg.option_at(sub_entry.index_1st_option).and_then(|opt| opt.addr())
                                        ) {
                                            (Some(port), Some(addr)) => {
                                                SocketAddr::new(addr.into(), port)
                                            }
                                            (port, addr) => {
                                                tracing::error!(
                                                    "Malformed SubscribeEventgroup: missing endpoint option (port={:?}, addr={:?})",
                                                    port, addr
                                                );
                                                panic!("Protocol violation: SubscribeEventgroup must include valid IPv4 endpoint option");
                                            }
                                        };

                                        tracing::info!("SD Host: Client subscription endpoint: {}", client_endpoint);

                                        // Send SubscribeEventgroupAck
                                        let ack = build_sd_subscribe_ack(
                                            TEST_SERVICE_ID,
                                            0x0001,
                                            TEST_SERVICE_VERSION.0,
                                            0x0001, // eventgroup_id
                                            3,
                                        );
                                        sd_socket.send_to(&ack, addr).await?;
                                        tracing::info!("SD Host: Sent SubscribeEventgroupAck");
                                        return Ok::<_, std::io::Error>(client_endpoint);
                                    }
                                }
                            }
                        } else {
                            tracing::debug!("SD Host: Failed to parse header");
                        }
                    }
                }
            })
            .await
            .expect("Should receive subscription")
            .expect("recv should succeed");

            // Notify RPC host of client subscription endpoint
            let _ = sub_tx.send(client_subscription_endpoint).await;
            Ok(())
        }
    });

    // RPC Host: Sends events when notified
    sim.host("rpc_host", move || {
        let sub_rx = Arc::clone(&sub_rx);
        async move {
            // Wait for subscription notification with client endpoint
            let client_event_endpoint =
                tokio::time::timeout(Duration::from_secs(10), sub_rx.lock().await.recv())
                    .await
                    .expect("Should receive subscription signal")
                    .expect("Channel should not close");

            tracing::info!(
                "RPC Host: Subscription received, client endpoint: {}, sending event",
                client_event_endpoint
            );

            let event_socket = turmoil::net::UdpSocket::bind("0.0.0.0:40000").await?;

            // Build and send event to client
            let event = build_notification(
                TEST_SERVICE_ID,
                0x8001, // event_id
                0x0000,
                1,
                TEST_SERVICE_VERSION.0,
                b"event_data",
            );

            // Send event to client's subscription endpoint (not SD port!)
            event_socket.send_to(&event, client_event_endpoint).await?;
            tracing::info!("RPC Host: Sent event to {}", client_event_endpoint);

            tokio::time::sleep(Duration::from_secs(3)).await;
            Ok(())
        }
    });

    // Client: Subscribes and receives events
    sim.client("client", async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        tracing::info!("Client: Finding service");
        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery should complete")
            .expect("Service should be found");

        tracing::info!("Client: Subscribing to eventgroup");
        let mut subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(EventgroupId::new(0x0001).unwrap()),
        )
        .await
        .expect("Subscribe should not timeout")
        .expect("Subscribe should succeed");

        tracing::info!("Client: Waiting for event");
        let event = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("Event should arrive")
            .expect("Should receive event");

        assert_eq!(event.event_id, EventId::new(0x8001).unwrap());
        assert_eq!(event.payload.as_ref(), b"event_data");
        tracing::info!("Client: Received event successfully");

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// SPLIT SERVER TCP RPC TEST
// ============================================================================

/// [split_server_tcp_rpc] TCP service on split host can be called via RPC
///
/// Scenario:
/// - SD Host: Advertises service with TCP endpoint pointing to RPC Host
/// - RPC Host: Handles TCP connection and method calls
/// - Client: Discovers service, connects via TCP, calls methods
///
/// This test verifies that:
/// - Client can discover services with TCP endpoints on split hosts
/// - Client correctly establishes TCP connection to the RPC host
/// - Method calls are routed to the correct TCP endpoint
/// - Magic cookies are handled properly in TCP communication
#[test]
fn split_server_tcp_rpc() {
    covers!(
        feat_req_someip_328,   // Request/Response pattern
        feat_req_someip_324,   // TCP binding
        feat_req_someipsd_011  // IPv4 endpoint option
    );
    configure_tracing();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // SD Host
    sim.host("sd_host", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let rpc_host_ip: Ipv4Addr = turmoil::lookup("rpc_host").to_string().parse().unwrap();
        let rpc_port = 40000u16;

        // Build SD Offer with TCP endpoint
        let offer = SdOfferBuilder::new(TEST_SERVICE_ID, 0x0001, rpc_host_ip, rpc_port)
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .tcp()
            .session_id(1)
            .reboot_flag(true)
            .build();

        let multicast_addr: SocketAddr = "239.255.0.1:30490".parse().unwrap();
        sd_socket.send_to(&offer, multicast_addr).await?;
        tracing::info!(
            "SD Host: Sent TCP offer for service {:04X} pointing to {}:{}",
            TEST_SERVICE_ID,
            rpc_host_ip,
            rpc_port
        );

        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });

    // RPC Host: TCP server
    sim.host("rpc_host", || async {
        let listener = turmoil::net::TcpListener::bind("0.0.0.0:40000").await?;
        tracing::info!("RPC Host: TCP listener ready");

        tokio::time::timeout(Duration::from_secs(10), async {
            let (mut stream, client_addr) = listener.accept().await?;
            tracing::info!("RPC Host: Accepted TCP connection from {}", client_addr);

            // Use proper TCP framing to read the request
            let mut buffer = BytesMut::new();

            // Read first message - could be magic cookie or actual request
            let first_msg = read_tcp_message(&mut stream, &mut buffer)
                .await?
                .expect("Should receive message");

            // Check if it's a magic cookie, if so read the actual request
            let request_data = if is_magic_cookie(&first_msg) {
                tracing::info!("RPC Host: Skipped magic cookie");
                read_tcp_message(&mut stream, &mut buffer)
                    .await?
                    .expect("Should receive request after magic cookie")
            } else {
                first_msg
            };

            tracing::info!(
                "RPC Host: Read framed message, {} bytes",
                request_data.len()
            );

            // Parse the request header
            let header = parse_header(&request_data).expect("Valid header");
            tracing::info!(
                "RPC Host: Parsed header - service_id={:04X}, method_id={:04X}",
                header.service_id,
                header.method_id
            );

            assert_eq!(header.service_id, TEST_SERVICE_ID);
            assert_eq!(header.method_id, TEST_METHOD_ID);
            assert!(header.is_request());

            let request_payload = &request_data[SOMEIP_HEADER_SIZE..];
            assert_eq!(request_payload, b"tcp_ping");

            // Build response with same client_id and session_id
            let response = build_response(&header, b"tcp_pong");

            // Send response with magic cookie (server-side)
            stream.write_all(&magic_cookie_server()).await?;
            stream.write_all(&response).await?;
            tracing::info!("RPC Host: Sent TCP response");

            tokio::time::sleep(Duration::from_secs(3)).await;
            Ok::<_, std::io::Error>(())
        })
        .await
        .expect("Should handle connection")
        .expect("Connection should succeed");

        Ok(())
    });

    // Client
    sim.client("client", async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .magic_cookies(true)
            .start_turmoil()
            .await
            .unwrap();

        tracing::info!("Client: Finding service");
        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery should complete")
            .expect("Service should be found");

        tracing::info!("Client: Calling TCP method");
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(TEST_METHOD_ID).unwrap(), b"tcp_ping"),
        )
        .await
        .expect("Call should not timeout")
        .expect("Call should succeed");

        assert_eq!(response.payload.as_ref(), b"tcp_pong");
        tracing::info!("Client: Received TCP response");

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// SPLIT SERVER TCP PUB/SUB TEST
// ============================================================================

/// [split_server_tcp_pubsub] TCP pub/sub on split host
///
/// Scenario:
/// - SD Host: Advertises service with TCP endpoint for events, handles subscription messages
/// - RPC Host: Accepts TCP connections and sends events over TCP
/// - Client: Subscribes and receives events via TCP
///
/// For TCP pub/sub, the client establishes a TCP connection to receive events
/// (unlike UDP where events are sent to the client's subscription endpoint).
#[test]
fn split_server_tcp_pubsub() {
    covers!(
        feat_req_someip_352,   // Events describe Publish/Subscribe concept
        feat_req_someip_324,   // TCP binding
        feat_req_someipsd_011  // IPv4 endpoint option
    );
    configure_tracing();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Coordination channel: SD host tells RPC host which eventgroups were subscribed
    let (eventgroup_tx, eventgroup_rx) = tokio::sync::mpsc::channel::<u16>(2);
    let eventgroup_rx = Arc::new(Mutex::new(eventgroup_rx));

    // SD Host: Advertises service and handles subscription messages
    sim.host("sd_host", move || {
        let eventgroup_tx = eventgroup_tx.clone();
        async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4(
            "239.255.0.1".parse().unwrap(),
            "0.0.0.0".parse().unwrap(),
        )?;

        let rpc_host_ip: Ipv4Addr = turmoil::lookup("rpc_host").to_string().parse().unwrap();
        let rpc_port = 40001u16;

        // Build SD Offer with TCP endpoint
        let offer = SdOfferBuilder::new(TEST_SERVICE_ID, 0x0001, rpc_host_ip, rpc_port)
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .tcp()
            .session_id(1)
            .reboot_flag(true)
            .build();

        let multicast_addr: SocketAddr = "239.255.0.1:30490".parse().unwrap();
        sd_socket.send_to(&offer, multicast_addr).await?;
        tracing::info!("SD Host: Sent TCP pub/sub offer for service {:04X} pointing to {}:{}", 
            TEST_SERVICE_ID, rpc_host_ip, rpc_port);

        // Wait for SubscribeEventgroup from client (expecting 2 subscriptions)
        let mut buf = [0u8; 1500];
        let mut subscriptions_received = 0;
        tokio::time::timeout(Duration::from_secs(10), async {
            while subscriptions_received < 2 {
                if let Ok((len, addr)) = sd_socket.recv_from(&mut buf).await {
                    if let Some(header) = parse_header(&buf[..len]) {
                        // Check if this is an SD message
                        if header.service_id == SD_SERVICE_ID && header.method_id == SD_METHOD_ID {
                            // Parse SD message to check for subscription(s)
                            if let Some(sd_msg) = ParsedSdMessage::parse(&buf[SOMEIP_HEADER_SIZE..len]) {
                                let sub_entries: Vec<_> = sd_msg.subscribe_entries().collect();
                                for sub_entry in sub_entries {
                                    subscriptions_received += 1;
                                    let eventgroup_id = sub_entry.eventgroup_id;
                                    tracing::info!("SD Host: Detected TCP subscription for eventgroup {:04X} from {}", 
                                        eventgroup_id, addr);

                                    // Send SubscribeEventgroupAck for this eventgroup
                                    let ack = build_sd_subscribe_ack(
                                        TEST_SERVICE_ID,
                                        0x0001,
                                        TEST_SERVICE_VERSION.0,
                                        eventgroup_id,
                                        3,
                                    );
                                    sd_socket.send_to(&ack, addr).await?;
                                    tracing::info!("SD Host: Sent SubscribeEventgroupAck for eventgroup {:04X}", eventgroup_id);

                                    // Notify RPC host about this subscription
                                    let _ = eventgroup_tx.send(eventgroup_id).await;
                                }
                            }
                        }
                    }
                }
            }
            Ok::<_, std::io::Error>(())
        })
        .await
        .expect("Should receive subscriptions")
        .expect("recv should succeed");

        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    }});

    // RPC Host: TCP event sender - accepts multiple connections
    sim.host("rpc_host", move || {
        let eventgroup_rx = Arc::clone(&eventgroup_rx);
        async move {
            let listener = turmoil::net::TcpListener::bind("0.0.0.0:40001").await?;
            tracing::info!("RPC Host: TCP event listener ready on port 40001");

            tokio::time::timeout(Duration::from_secs(10), async {
                // Accept TCP connections eagerly (client needs to connect BEFORE subscribing)
                let mut connections = Vec::new();

                for i in 0..2 {
                    // Accept TCP connection first (per spec: client connects before SubscribeEventgroup)
                    let (stream, addr) = listener.accept().await?;
                    tracing::info!("RPC Host: Accepted TCP connection #{} from {}", i + 1, addr);

                    // Now wait for corresponding subscription notification
                    let eg = eventgroup_rx
                        .lock()
                        .await
                        .recv()
                        .await
                        .expect("Should receive eventgroup ID");
                    tracing::info!("RPC Host: Received subscription for eventgroup {:04X}", eg);
                    connections.push((eg, stream));
                }

                // Small delay to ensure both subscriptions are fully established
                tokio::time::sleep(Duration::from_millis(200)).await;

                //Build events for both eventgroups
                let event1 = build_notification(
                    TEST_SERVICE_ID,
                    0x8001, // event_id for eventgroup 1
                    0x0000,
                    1,
                    TEST_SERVICE_VERSION.0,
                    b"tcp_event_eg1",
                );

                let event2 = build_notification(
                    TEST_SERVICE_ID,
                    0x8002, // event_id for eventgroup 2
                    0x0000,
                    2,
                    TEST_SERVICE_VERSION.0,
                    b"tcp_event_eg2",
                );

                // Send correct event to each connection based on its eventgroup
                for (eg, mut stream) in connections {
                    let event = if eg == 0x0001 { &event1 } else { &event2 };
                    stream.write_all(&magic_cookie_server()).await?;
                    stream.write_all(event).await?;
                    tracing::info!("RPC Host: Sent event for eventgroup {:04X}", eg);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }

                tokio::time::sleep(Duration::from_secs(3)).await;
                Ok::<_, std::io::Error>(())
            })
            .await
            .expect("Should handle connections")
            .expect("Connections should succeed");

            Ok(())
        }
    });

    // Client: Subscribes and receives events via TCP
    sim.client("client", async {
        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .magic_cookies(true)
            .start_turmoil()
            .await
            .unwrap();

        tracing::info!("Client: Finding service");
        let proxy = runtime.find(TEST_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery should complete")
            .expect("Service should be found");

        // Randomize which subscription gets which eventgroup
        use rand::Rng;
        let sub_a_gets_eg1 = rand::thread_rng().gen::<bool>();
        let (eg_a, eg_b) = if sub_a_gets_eg1 {
            (0x0001u16, 0x0002u16)
        } else {
            (0x0002u16, 0x0001u16)
        };

        tracing::info!(
            "Client: Subscribing to TCP eventgroups (subscription A -> {:04X}, B -> {:04X})",
            eg_a,
            eg_b
        );

        let mut subscription_a = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(EventgroupId::new(eg_a).unwrap()),
        )
        .await
        .expect("Subscribe A should not timeout")
        .expect("Subscribe A should succeed");

        let mut subscription_b = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(EventgroupId::new(eg_b).unwrap()),
        )
        .await
        .expect("Subscribe B should not timeout")
        .expect("Subscribe B should succeed");

        tracing::info!("Client: Waiting for TCP events");
        let event_a = tokio::time::timeout(Duration::from_secs(5), subscription_a.next())
            .await
            .expect("Event A should arrive")
            .expect("Should receive event A");

        let event_b = tokio::time::timeout(Duration::from_secs(5), subscription_b.next())
            .await
            .expect("Event B should arrive")
            .expect("Should receive event B");

        // Verify subscription A got the event for its eventgroup
        let expected_event_id_a = EventId::new(0x8000 | eg_a).unwrap();
        let expected_payload_a = if eg_a == 0x0001 {
            b"tcp_event_eg1"
        } else {
            b"tcp_event_eg2"
        };
        assert_eq!(event_a.event_id, expected_event_id_a);
        assert_eq!(event_a.payload.as_ref(), expected_payload_a);
        tracing::info!(
            "Client: Subscription A received event for eventgroup {:04X}",
            eg_a
        );

        // Verify subscription B got the event for its eventgroup
        let expected_event_id_b = EventId::new(0x8000 | eg_b).unwrap();
        let expected_payload_b = if eg_b == 0x0001 {
            b"tcp_event_eg1"
        } else {
            b"tcp_event_eg2"
        };
        assert_eq!(event_b.event_id, expected_event_id_b);
        assert_eq!(event_b.payload.as_ref(), expected_payload_b);
        tracing::info!(
            "Client: Subscription B received event for eventgroup {:04X}",
            eg_b
        );

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// SPLIT SERVER TCP PUB/SUB WITH SERVER REBOOT TEST
// ============================================================================

/// [split_server_tcp_pubsub_server_reboot] TCP pub/sub on split host with server reboot
/// and service change across the reboot boundary.
///
/// Scenario:
/// - Phase 1: SD Host offers service A (TCP endpoint on RPC Host) with an initial
///   startup offer (session_id=1, reboot_flag=true) followed by a settled offer
///   (session_id=2, reboot_flag=false).  Client discovers service A, subscribes
///   via TCP, and receives events.
///
/// - Reboot: SD Host resets its session counter to 1 and sets reboot_flag=true
///   again, signalling a node reboot.  It now offers service B instead of service A.
///
/// - Phase 2: The client detects the reboot (reboot_flag transitions false→true,
///   session ID regresses).  The client must actively close its TCP connection to
///   service A as part of reboot handling.  The RPC Host verifies this by reading
///   EOF from the accepted socket — we do **not** drop from the server side.
///   The client then discovers service B from the post-reboot offer, subscribes
///   via TCP, and receives events.
///
/// This tests:
/// - Reboot signalling via session-ID regression + reboot-flag transition
/// - Service change across a reboot boundary (A → B)
/// - Client subscription termination when the server-side TCP connection is dropped
/// - Client ability to re-subscribe to a new service after detecting a peer reboot
#[test]
fn split_server_tcp_pubsub_server_reboot() {
    covers!(
        feat_req_someip_352,   // Events describe Publish/Subscribe concept
        feat_req_someip_324,   // TCP binding
        feat_req_someipsd_011, // IPv4 endpoint option
    );
    configure_tracing();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // sd_host signals rpc_host once each subscription has been acknowledged,
    // so rpc_host knows it is safe to start sending events.
    let (sub_a_tx, sub_a_rx) = tokio::sync::mpsc::channel::<()>(1);
    let (sub_b_tx, sub_b_rx) = tokio::sync::mpsc::channel::<()>(1);
    let sub_a_rx = Arc::new(Mutex::new(sub_a_rx));
    let sub_b_rx = Arc::new(Mutex::new(sub_b_rx));

    // -----------------------------------------------------------------------
    // SD Host
    // -----------------------------------------------------------------------
    sim.host("sd_host", move || {
        let sub_a_tx = sub_a_tx.clone();
        let sub_b_tx = sub_b_tx.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            let rpc_host_ip: Ipv4Addr = turmoil::lookup("rpc_host").to_string().parse().unwrap();
            let multicast_addr: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            // ----------------------------------------------------------------
            // PHASE 1 – offer service A
            // ----------------------------------------------------------------

            // Initial startup offer: session_id=1, reboot_flag=true
            let offer_a_initial = SdOfferBuilder::new(
                REBOOT_TEST_SERVICE_A_ID,
                0x0001,
                rpc_host_ip,
                REBOOT_TEST_RPC_PORT,
            )
            .version(REBOOT_TEST_SERVICE_VERSION.0, REBOOT_TEST_SERVICE_VERSION.1)
            .tcp()
            .session_id(1)
            .reboot_flag(true)
            .build();

            sd_socket.send_to(&offer_a_initial, multicast_addr).await?;
            tracing::info!("SD Host: Sent initial offer for service A (session=1, reboot=true)");

            // Settled offer: session_id=2, reboot_flag=false
            tokio::time::sleep(Duration::from_millis(150)).await;
            let offer_a_settled = SdOfferBuilder::new(
                REBOOT_TEST_SERVICE_A_ID,
                0x0001,
                rpc_host_ip,
                REBOOT_TEST_RPC_PORT,
            )
            .version(REBOOT_TEST_SERVICE_VERSION.0, REBOOT_TEST_SERVICE_VERSION.1)
            .tcp()
            .session_id(2)
            .reboot_flag(false)
            .build();

            sd_socket.send_to(&offer_a_settled, multicast_addr).await?;
            tracing::info!("SD Host: Sent settled offer for service A (session=2, reboot=false)");

            // Wait for SubscribeEventgroup for service A
            let mut buf = [0u8; 1500];
            let subscribe_a_addr = tokio::time::timeout(Duration::from_secs(8), async {
                loop {
                    if let Ok((len, addr)) = sd_socket.recv_from(&mut buf).await {
                        if let Some(header) = parse_header(&buf[..len]) {
                            if header.service_id == SD_SERVICE_ID
                                && header.method_id == SD_METHOD_ID
                            {
                                if let Some(sd_msg) =
                                    ParsedSdMessage::parse(&buf[SOMEIP_HEADER_SIZE..len])
                                {
                                    for sub_entry in sd_msg.subscribe_entries() {
                                        if sub_entry.service_id == REBOOT_TEST_SERVICE_A_ID {
                                            tracing::info!(
                                                "SD Host: Received Subscribe for service A from {}",
                                                addr
                                            );
                                            return Ok::<_, std::io::Error>(addr);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            })
            .await
            .expect("Should receive Subscribe for service A")
            .expect("recv should succeed");

            // Acknowledge the subscription
            let ack_a = build_sd_subscribe_ack(
                REBOOT_TEST_SERVICE_A_ID,
                0x0001,
                REBOOT_TEST_SERVICE_VERSION.0,
                REBOOT_TEST_EVENTGROUP_ID,
                0xFF_FFFF,
            );
            sd_socket.send_to(&ack_a, subscribe_a_addr).await?;
            tracing::info!("SD Host: Sent SubscribeAck for service A");

            // Signal rpc_host: subscription A is active, start sending events
            let _ = sub_a_tx.send(()).await;

            // ----------------------------------------------------------------
            // PHASE 2 – reboot and offer service B
            // ----------------------------------------------------------------

            // Wait long enough for the A events to be delivered before rebooting
            tokio::time::sleep(Duration::from_millis(700)).await;

            // Reboot offer for service B:
            //   session_id resets to 1  (regresses from 2)
            //   reboot_flag goes false → true  (case-1 reboot detection)
            //   service B is offered; service A is no longer advertised
            let offer_b_reboot = SdOfferBuilder::new(
                REBOOT_TEST_SERVICE_B_ID,
                0x0001,
                rpc_host_ip,
                REBOOT_TEST_RPC_PORT,
            )
            .version(REBOOT_TEST_SERVICE_VERSION.0, REBOOT_TEST_SERVICE_VERSION.1)
            .tcp()
            .session_id(1) // reset: reboot indicator
            .reboot_flag(true) // flag goes false → true: peer detects reboot
            .build();

            sd_socket.send_to(&offer_b_reboot, multicast_addr).await?;
            tracing::info!("SD Host: Sent REBOOT offer for service B (session=1, reboot=true)");

            // Wait for SubscribeEventgroup for service B
            let subscribe_b_addr = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if let Ok((len, addr)) = sd_socket.recv_from(&mut buf).await {
                        if let Some(header) = parse_header(&buf[..len]) {
                            if header.service_id == SD_SERVICE_ID
                                && header.method_id == SD_METHOD_ID
                            {
                                if let Some(sd_msg) =
                                    ParsedSdMessage::parse(&buf[SOMEIP_HEADER_SIZE..len])
                                {
                                    for sub_entry in sd_msg.subscribe_entries() {
                                        if sub_entry.service_id == REBOOT_TEST_SERVICE_B_ID {
                                            tracing::info!(
                                                "SD Host: Received Subscribe for service B from {}",
                                                addr
                                            );
                                            return Ok::<_, std::io::Error>(addr);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            })
            .await
            .expect("Should receive Subscribe for service B")
            .expect("recv should succeed");

            let ack_b = build_sd_subscribe_ack(
                REBOOT_TEST_SERVICE_B_ID,
                0x0001,
                REBOOT_TEST_SERVICE_VERSION.0,
                REBOOT_TEST_EVENTGROUP_ID,
                0xFF_FFFF,
            );
            sd_socket.send_to(&ack_b, subscribe_b_addr).await?;
            tracing::info!("SD Host: Sent SubscribeAck for service B");

            // Signal rpc_host: subscription B is active, start sending events
            let _ = sub_b_tx.send(()).await;

            tokio::time::sleep(Duration::from_secs(3)).await;
            Ok(())
        }
    });

    // -----------------------------------------------------------------------
    // RPC Host
    // -----------------------------------------------------------------------
    sim.host("rpc_host", move || {
        let sub_a_rx = Arc::clone(&sub_a_rx);
        let sub_b_rx = Arc::clone(&sub_b_rx);
        async move {
            let listener = turmoil::net::TcpListener::bind("0.0.0.0:40002").await?;
            tracing::info!(
                "RPC Host: TCP listener ready on port {}",
                REBOOT_TEST_RPC_PORT
            );

            // ----------------------------------------------------------------
            // PHASE 1 – service A connection
            // ----------------------------------------------------------------
            // Per spec the client opens the TCP connection before sending
            // SubscribeEventgroup, so the connection arrives before the signal.
            let (mut stream_a, client_addr_a) =
                tokio::time::timeout(Duration::from_secs(8), listener.accept())
                    .await
                    .expect("Timeout waiting for service A TCP connection")
                    .expect("Accept failed for service A");
            tracing::info!(
                "RPC Host: Accepted TCP connection from {} for service A",
                client_addr_a
            );

            // Wait until sd_host has sent the SubscribeAck (subscription fully established)
            tokio::time::timeout(Duration::from_secs(5), sub_a_rx.lock().await.recv())
                .await
                .expect("Timeout waiting for service A subscription signal")
                .expect("sub_a channel closed unexpectedly");

            // Deliver one event for service A
            let event_a = build_notification(
                REBOOT_TEST_SERVICE_A_ID,
                0x8001, // event ID
                0x0000,
                1, // session_id
                REBOOT_TEST_SERVICE_VERSION.0,
                b"service_a_event",
            );
            stream_a.write_all(&magic_cookie_server()).await?;
            stream_a.write_all(&event_a).await?;
            tracing::info!("RPC Host: Sent event for service A");

            // Do NOT drop the connection from our side.  It is the client's
            // responsibility to close TCP connections to a rebooted peer when
            // it detects the reboot via the SD messages sent by sd_host.
            // We verify that by waiting for EOF here; a timeout is a test failure.
            let mut drain = [0u8; 1];
            let close_result =
                tokio::time::timeout(Duration::from_secs(5), stream_a.read(&mut drain)).await;
            match close_result {
                Ok(Ok(0)) => {
                    tracing::info!(
                        "RPC Host: Client closed service A connection (reboot detected correctly)"
                    );
                }
                Ok(Ok(n)) => {
                    panic!(
                        "RPC Host: Expected EOF from client, received {} unexpected bytes",
                        n
                    );
                }
                Ok(Err(e)) => {
                    // Some TCP stacks surface a graceful close as an error.
                    tracing::info!("RPC Host: Connection closed with error (acceptable): {}", e);
                }
                Err(_) => {
                    panic!(
                        "RPC Host: Timeout — client did not close service A TCP connection \
                         within 5 s; reboot detection must close TCP connections to rebooted peers"
                    );
                }
            }

            // ----------------------------------------------------------------
            // PHASE 2 – service B connection
            // ----------------------------------------------------------------
            let (mut stream_b, client_addr_b) =
                tokio::time::timeout(Duration::from_secs(10), listener.accept())
                    .await
                    .expect("Timeout waiting for service B TCP connection")
                    .expect("Accept failed for service B");
            tracing::info!(
                "RPC Host: Accepted TCP connection from {} for service B",
                client_addr_b
            );

            tokio::time::timeout(Duration::from_secs(5), sub_b_rx.lock().await.recv())
                .await
                .expect("Timeout waiting for service B subscription signal")
                .expect("sub_b channel closed unexpectedly");

            let event_b = build_notification(
                REBOOT_TEST_SERVICE_B_ID,
                0x8001, // event ID
                0x0000,
                1, // session_id
                REBOOT_TEST_SERVICE_VERSION.0,
                b"service_b_event",
            );
            stream_b.write_all(&magic_cookie_server()).await?;
            stream_b.write_all(&event_b).await?;
            tracing::info!("RPC Host: Sent event for service B");

            tokio::time::sleep(Duration::from_secs(3)).await;
            Ok(())
        }
    });

    // -----------------------------------------------------------------------
    // Client
    // -----------------------------------------------------------------------
    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .magic_cookies(true)
            .start_turmoil()
            .await
            .unwrap();

        // ----------------------------------------------------------------
        // PHASE 1 – discover service A, subscribe, receive event
        // ----------------------------------------------------------------
        tracing::info!("Client: Finding service A");
        let proxy_a = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(REBOOT_TEST_SERVICE_A_ID),
        )
        .await
        .expect("Discovery timeout for service A")
        .expect("Service A should be found");

        tracing::info!("Client: Subscribing to service A eventgroup");
        let mut subscription_a = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_a.subscribe(EventgroupId::new(REBOOT_TEST_EVENTGROUP_ID).unwrap()),
        )
        .await
        .expect("Subscribe A timeout")
        .expect("Subscribe A should succeed");

        let event_a = tokio::time::timeout(Duration::from_secs(5), subscription_a.next())
            .await
            .expect("Event A should arrive in time")
            .expect("Should receive event A");

        assert_eq!(event_a.event_id, EventId::new(0x8001).unwrap());
        assert_eq!(event_a.payload.as_ref(), b"service_a_event");
        tracing::info!("Client: Received event from service A");

        // ----------------------------------------------------------------
        // PHASE 2 – reboot detected; subscription A terminates; subscribe to B
        // ----------------------------------------------------------------

        // The RPC Host drops the TCP connection as part of the simulated reboot.
        // This causes the client's subscription stream to reach EOF: next() must
        // return None, not time out, to prove the runtime actually closed the
        // subscription rather than just silently stalling.
        tracing::info!("Client: Waiting for service A subscription to terminate (reboot)");
        let terminated = tokio::time::timeout(Duration::from_secs(5), subscription_a.next())
            .await
            .expect("Subscription A should terminate within timeout — TCP EOF must propagate");
        assert!(
            terminated.is_none(),
            "Service A subscription should return None after server TCP disconnect"
        );
        tracing::info!("Client: Service A subscription terminated cleanly");

        // Service B is now offered via the reboot SD message.
        // find() will wait until the multicast offer is received.
        tracing::info!("Client: Finding service B after reboot");
        let proxy_b = tokio::time::timeout(
            Duration::from_secs(8),
            runtime.find(REBOOT_TEST_SERVICE_B_ID),
        )
        .await
        .expect("Discovery timeout for service B after reboot")
        .expect("Service B should be found after reboot");

        tracing::info!("Client: Subscribing to service B eventgroup");
        let mut subscription_b = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_b.subscribe(EventgroupId::new(REBOOT_TEST_EVENTGROUP_ID).unwrap()),
        )
        .await
        .expect("Subscribe B timeout")
        .expect("Subscribe B should succeed");

        let event_b = tokio::time::timeout(Duration::from_secs(5), subscription_b.next())
            .await
            .expect("Event B should arrive in time")
            .expect("Should receive event B");

        assert_eq!(event_b.event_id, EventId::new(0x8001).unwrap());
        assert_eq!(event_b.payload.as_ref(), b"service_b_event");
        tracing::info!("Client: Received event from service B after reboot");

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// SPLIT SERVER TCP PUB/SUB WITH ISOLATED PEER REBOOT TEST
// ============================================================================

/// Constants for the isolated-reboot test (distinct from all other tests)
const ISOLATED_REBOOT_SERVICE_A_ID: u16 = 0x123A;
const ISOLATED_REBOOT_SERVICE_B_ID: u16 = 0x123B;
const ISOLATED_REBOOT_SERVICE_VERSION: (u8, u32) = (1, 0);
/// Port on rpc_host that serves service A events over TCP
const ISOLATED_REBOOT_RPC_PORT_A: u16 = 40003;
/// Port on rpc_host that serves service B events over TCP (second wireserver)
const ISOLATED_REBOOT_RPC_PORT_B: u16 = 40004;
const ISOLATED_REBOOT_EVENTGROUP_ID: u16 = 0x0001;
/// Near-infinite TTL so no TTL expiry occurs during the test.
const LONG_TTL: u32 = 0x00FF_FFFF;

/// [split_server_tcp_pubsub_isolated_peer_reboot] A peer reboot only terminates
/// subscriptions to **that** peer; subscriptions to unrelated peers are unaffected.
///
/// Topology:
/// - `sd_host_a` (IP A) offers service A with TCP endpoint on `rpc_host` (IP R).
///   Very long offer TTL and subscribe TTL — no renewal traffic occurs.
/// - `rpc_host` (IP R) **also** offers and hosts service B over TCP — it acts as
///   a second independent wireserver on the same IP. Both services therefore have
///   the same TCP host IP, making this the harder test variant.
/// - Client subscribes to both A and B and receives one event from each.
///
/// Reboot sequence:
/// - `rpc_host`'s service B resets its session counter to 1 and sets `reboot_flag=true`.
///   This triggers reboot detection for `peer_ip = rpc_host_ip`.
/// - The client **must** close its TCP connection to service B
///   (verified by reading EOF with a 5 s timeout).
/// - `subscription_b.next()` must return `None`.
/// - Service A **must** continue: the client receives a second event from `rpc_host`
///   to prove the subscription was not disrupted.
///
/// This exercises the invariant that reboot detection is scoped to the SD peer
/// IP that sent the reboot offer: only services whose `sd_endpoint.ip() == rpc_host_ip`
/// are expired (only service B). Service A — announced by `sd_host_a` (IP A) even
/// though its TCP also runs on `rpc_host` (IP R) — must not be touched.
///
/// Without the fix (`sd_endpoint`-only scoping), a naive implementation would
/// close ALL TCP connections to `rpc_host` on reboot, incorrectly breaking service A.
#[test]
fn split_server_tcp_pubsub_isolated_peer_reboot() {
    covers!(
        feat_req_someip_352,   // Events describe Publish/Subscribe concept
        feat_req_someip_324,   // TCP binding
        feat_req_someipsd_011, // IPv4 endpoint option
    );
    configure_tracing();

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // sd_host_a signals rpc_host once subscription A is acknowledged
    let (sub_a_tx, sub_a_rx) = tokio::sync::mpsc::channel::<()>(1);
    let sub_a_rx = Arc::new(Mutex::new(sub_a_rx));
    // -----------------------------------------------------------------------
    // SD Host A: offers service A pointing to rpc_host; never reboots
    // -----------------------------------------------------------------------
    sim.host("sd_host_a", move || {
        let sub_a_tx = sub_a_tx.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            let rpc_host_ip: Ipv4Addr = turmoil::lookup("rpc_host").to_string().parse().unwrap();
            let multicast_addr: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            // Startup offer for service A
            let offer_a = SdOfferBuilder::new(
                ISOLATED_REBOOT_SERVICE_A_ID,
                0x0001,
                rpc_host_ip,
                ISOLATED_REBOOT_RPC_PORT_A,
            )
            .version(
                ISOLATED_REBOOT_SERVICE_VERSION.0,
                ISOLATED_REBOOT_SERVICE_VERSION.1,
            )
            .tcp()
            .ttl(LONG_TTL)
            .session_id(1)
            .reboot_flag(true)
            .build();
            sd_socket.send_to(&offer_a, multicast_addr).await?;
            tracing::info!("SD Host A: Sent startup offer for service A");

            // Settled offer
            tokio::time::sleep(Duration::from_millis(150)).await;
            let offer_a_settled = SdOfferBuilder::new(
                ISOLATED_REBOOT_SERVICE_A_ID,
                0x0001,
                rpc_host_ip,
                ISOLATED_REBOOT_RPC_PORT_A,
            )
            .version(
                ISOLATED_REBOOT_SERVICE_VERSION.0,
                ISOLATED_REBOOT_SERVICE_VERSION.1,
            )
            .tcp()
            .ttl(LONG_TTL)
            .session_id(2)
            .reboot_flag(false)
            .build();
            sd_socket.send_to(&offer_a_settled, multicast_addr).await?;
            tracing::info!("SD Host A: Sent settled offer for service A");

            // Wait for SubscribeEventgroup for service A, ACK, signal rpc_host
            let mut buf = [0u8; 1500];
            let subscribe_addr = tokio::time::timeout(Duration::from_secs(8), async {
                loop {
                    if let Ok((len, addr)) = sd_socket.recv_from(&mut buf).await {
                        if let Some(hdr) = parse_header(&buf[..len]) {
                            if hdr.service_id == SD_SERVICE_ID && hdr.method_id == SD_METHOD_ID {
                                if let Some(sd_msg) =
                                    ParsedSdMessage::parse(&buf[SOMEIP_HEADER_SIZE..len])
                                {
                                    for sub in sd_msg.subscribe_entries() {
                                        if sub.service_id == ISOLATED_REBOOT_SERVICE_A_ID {
                                            tracing::info!(
                                                "SD Host A: Subscribe for service A from {}",
                                                addr
                                            );
                                            return Ok::<_, std::io::Error>(addr);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            })
            .await
            .expect("Should receive Subscribe for service A")
            .expect("recv should succeed");

            let ack_a = build_sd_subscribe_ack(
                ISOLATED_REBOOT_SERVICE_A_ID,
                0x0001,
                ISOLATED_REBOOT_SERVICE_VERSION.0,
                ISOLATED_REBOOT_EVENTGROUP_ID,
                LONG_TTL,
            );
            sd_socket.send_to(&ack_a, subscribe_addr).await?;
            tracing::info!("SD Host A: Sent SubscribeAck for service A");
            let _ = sub_a_tx.send(()).await;

            // Stay up for the entire test — no further offers
            tokio::time::sleep(Duration::from_secs(20)).await;
            Ok(())
        }
    });

    // -----------------------------------------------------------------------
    // RPC Host: TCP server for service A events AND combined SD + TCP for
    // service B.  Both services share this host's IP, so a reboot of service B
    // (whose SD source = rpc_host IP) must not disturb service A (whose SD
    // source = sd_host_a IP, even though its TCP endpoint is also here).
    // -----------------------------------------------------------------------
    sim.host("rpc_host", move || {
        let sub_a_rx = Arc::clone(&sub_a_rx);
        async move {
            let rpc_host_ip: Ipv4Addr =
                turmoil::lookup("rpc_host").to_string().parse().unwrap();
            let multicast_addr: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            // Internal one-shot channels:
            //   sub_b_ack: sd_b task → tcp_b task  (subscription B has been ACKed)
            //   reboot_sent: sd_b task → tcp_a task (reboot offer has been sent)
            let (sub_b_ack_tx, sub_b_ack_rx) = tokio::sync::oneshot::channel::<()>();
            let (reboot_sent_tx, reboot_sent_rx) = tokio::sync::oneshot::channel::<()>();

            // Service B SD handler — runs as a "second wireserver" on this host.
            let sd_b = async move {
                let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
                sd_socket.join_multicast_v4(
                    "239.255.0.1".parse().unwrap(),
                    "0.0.0.0".parse().unwrap(),
                )?;

                let offer_b_startup = SdOfferBuilder::new(
                    ISOLATED_REBOOT_SERVICE_B_ID,
                    0x0001,
                    rpc_host_ip,
                    ISOLATED_REBOOT_RPC_PORT_B,
                )
                .version(
                    ISOLATED_REBOOT_SERVICE_VERSION.0,
                    ISOLATED_REBOOT_SERVICE_VERSION.1,
                )
                .tcp()
                .ttl(LONG_TTL)
                .session_id(1)
                .reboot_flag(true)
                .build();
                sd_socket.send_to(&offer_b_startup, multicast_addr).await?;
                tracing::info!("RPC Host / SD-B: Sent startup offer for service B");

                tokio::time::sleep(Duration::from_millis(150)).await;

                let offer_b_settled = SdOfferBuilder::new(
                    ISOLATED_REBOOT_SERVICE_B_ID,
                    0x0001,
                    rpc_host_ip,
                    ISOLATED_REBOOT_RPC_PORT_B,
                )
                .version(
                    ISOLATED_REBOOT_SERVICE_VERSION.0,
                    ISOLATED_REBOOT_SERVICE_VERSION.1,
                )
                .tcp()
                .ttl(LONG_TTL)
                .session_id(2)
                .reboot_flag(false)
                .build();
                sd_socket.send_to(&offer_b_settled, multicast_addr).await?;
                tracing::info!("RPC Host / SD-B: Sent settled offer for service B");

                // Wait for SubscribeEventgroup for service B, then ACK
                let mut buf = [0u8; 1500];
                let subscribe_b_addr =
                    tokio::time::timeout(Duration::from_secs(8), async {
                        loop {
                            if let Ok((len, addr)) = sd_socket.recv_from(&mut buf).await {
                                if let Some(hdr) = parse_header(&buf[..len]) {
                                    if hdr.service_id == SD_SERVICE_ID
                                        && hdr.method_id == SD_METHOD_ID
                                    {
                                        if let Some(sd_msg) = ParsedSdMessage::parse(
                                            &buf[SOMEIP_HEADER_SIZE..len],
                                        ) {
                                            for sub in sd_msg.subscribe_entries() {
                                                if sub.service_id
                                                    == ISOLATED_REBOOT_SERVICE_B_ID
                                                {
                                                    tracing::info!(
                                                        "RPC Host / SD-B: Subscribe from {}",
                                                        addr
                                                    );
                                                    return Ok::<_, std::io::Error>(addr);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    })
                    .await
                    .expect("Should receive Subscribe for service B")
                    .expect("recv should succeed");

                let ack_b = build_sd_subscribe_ack(
                    ISOLATED_REBOOT_SERVICE_B_ID,
                    0x0001,
                    ISOLATED_REBOOT_SERVICE_VERSION.0,
                    ISOLATED_REBOOT_EVENTGROUP_ID,
                    LONG_TTL,
                );
                sd_socket.send_to(&ack_b, subscribe_b_addr).await?;
                tracing::info!("RPC Host / SD-B: Sent SubscribeAck for service B");
                let _ = sub_b_ack_tx.send(());

                // Wait, then reboot service B
                tokio::time::sleep(Duration::from_millis(700)).await;

                let offer_b_reboot = SdOfferBuilder::new(
                    ISOLATED_REBOOT_SERVICE_B_ID,
                    0x0001,
                    rpc_host_ip,
                    ISOLATED_REBOOT_RPC_PORT_B,
                )
                .version(
                    ISOLATED_REBOOT_SERVICE_VERSION.0,
                    ISOLATED_REBOOT_SERVICE_VERSION.1,
                )
                .tcp()
                .ttl(LONG_TTL)
                .session_id(1) // regresses → reboot indicator
                .reboot_flag(true) // flag flips false → true
                .build();
                sd_socket.send_to(&offer_b_reboot, multicast_addr).await?;
                tracing::info!(
                    "RPC Host / SD-B: Sent REBOOT offer (session=1, reboot=true)"
                );
                let _ = reboot_sent_tx.send(());

                tokio::time::sleep(Duration::from_secs(8)).await;
                Ok::<_, std::io::Error>(())
            };

            // Service A TCP handler — service A's events flow through this host
            // even though SD for service A is on sd_host_a.
            let tcp_a = {
                let sub_a_rx = sub_a_rx;
                async move {
                    let listener = turmoil::net::TcpListener::bind(
                        format!("0.0.0.0:{}", ISOLATED_REBOOT_RPC_PORT_A),
                    )
                    .await?;
                    tracing::info!(
                        "RPC Host / TCP-A: Listener ready on port {}",
                        ISOLATED_REBOOT_RPC_PORT_A
                    );

                    let (mut stream_a, client_addr) =
                        tokio::time::timeout(Duration::from_secs(10), listener.accept())
                            .await
                            .expect("Timeout waiting for service A TCP connection")
                            .expect("Accept failed for service A");
                    tracing::info!(
                        "RPC Host / TCP-A: Accepted connection from {}",
                        client_addr
                    );

                    tokio::time::timeout(Duration::from_secs(5), sub_a_rx.lock().await.recv())
                        .await
                        .expect("Timeout waiting for service A subscription signal")
                        .expect("sub_a channel closed");

                    // First event — before service B reboots
                    let event_a1 = build_notification(
                        ISOLATED_REBOOT_SERVICE_A_ID,
                        0x8001,
                        0x0000,
                        1,
                        ISOLATED_REBOOT_SERVICE_VERSION.0,
                        b"service_a_event_1",
                    );
                    stream_a.write_all(&magic_cookie_server()).await?;
                    stream_a.write_all(&event_a1).await?;
                    tracing::info!("RPC Host / TCP-A: Sent first event for service A");

                    // Wait until the service B reboot offer has been sent, then
                    // give the client time to process it before sending event A2.
                    let _ = reboot_sent_rx.await;
                    tokio::time::sleep(Duration::from_millis(800)).await;

                    let event_a2 = build_notification(
                        ISOLATED_REBOOT_SERVICE_A_ID,
                        0x8001,
                        0x0000,
                        2,
                        ISOLATED_REBOOT_SERVICE_VERSION.0,
                        b"service_a_event_2",
                    );
                    stream_a.write_all(&event_a2).await?;
                    tracing::info!(
                        "RPC Host / TCP-A: Sent second event (after service B reboot)"
                    );

                    tokio::time::sleep(Duration::from_secs(5)).await;
                    Ok::<_, std::io::Error>(())
                }
            };

            // Service B TCP handler
            let tcp_b = async move {
                let listener = turmoil::net::TcpListener::bind(
                    format!("0.0.0.0:{}", ISOLATED_REBOOT_RPC_PORT_B),
                )
                .await?;
                tracing::info!(
                    "RPC Host / TCP-B: Listener ready on port {}",
                    ISOLATED_REBOOT_RPC_PORT_B
                );

                let (mut stream_b, client_addr_b) =
                    tokio::time::timeout(Duration::from_secs(10), listener.accept())
                        .await
                        .expect("Timeout waiting for service B TCP connection")
                        .expect("Accept failed for service B");
                tracing::info!(
                    "RPC Host / TCP-B: Accepted connection from {}",
                    client_addr_b
                );

                // Wait until subscription B is ACKed before sending the event
                let _ = sub_b_ack_rx.await;

                let event_b = build_notification(
                    ISOLATED_REBOOT_SERVICE_B_ID,
                    0x8001,
                    0x0000,
                    1,
                    ISOLATED_REBOOT_SERVICE_VERSION.0,
                    b"service_b_event",
                );
                stream_b.write_all(&magic_cookie_server()).await?;
                stream_b.write_all(&event_b).await?;
                tracing::info!("RPC Host / TCP-B: Sent event for service B");

                // Client must close the TCP connection after the reboot
                let mut drain = [0u8; 1];
                let close_result =
                    tokio::time::timeout(Duration::from_secs(5), stream_b.read(&mut drain))
                        .await;
                match close_result {
                    Ok(Ok(0)) | Ok(Err(_)) => {
                        tracing::info!(
                            "RPC Host / TCP-B: Client closed service B connection (reboot detected correctly)"
                        );
                    }
                    Ok(Ok(n)) => {
                        panic!(
                            "RPC Host / TCP-B: Expected EOF from client, got {} unexpected bytes",
                            n
                        );
                    }
                    Err(_) => {
                        panic!(
                            "RPC Host / TCP-B: Timeout — client did not close service B \
                             TCP connection within 5 s after reboot"
                        );
                    }
                }

                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok::<_, std::io::Error>(())
            };

            tokio::try_join!(sd_b, tcp_a, tcp_b)?;
            Ok(())
        }
    });

    // -----------------------------------------------------------------------
    // Client
    // -----------------------------------------------------------------------
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .magic_cookies(true)
            .start_turmoil()
            .await
            .unwrap();

        tracing::info!("Client: Finding service A");
        let proxy_a = tokio::time::timeout(
            Duration::from_secs(8),
            runtime.find(ISOLATED_REBOOT_SERVICE_A_ID),
        )
        .await
        .expect("Discovery timeout for service A")
        .expect("Service A should be found");

        tracing::info!("Client: Finding service B");
        let proxy_b = tokio::time::timeout(
            Duration::from_secs(8),
            runtime.find(ISOLATED_REBOOT_SERVICE_B_ID),
        )
        .await
        .expect("Discovery timeout for service B")
        .expect("Service B should be found");

        tracing::info!("Client: Subscribing to service A");
        let mut subscription_a = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_a.subscribe(EventgroupId::new(ISOLATED_REBOOT_EVENTGROUP_ID).unwrap()),
        )
        .await
        .expect("Subscribe A timeout")
        .expect("Subscribe A should succeed");

        tracing::info!("Client: Subscribing to service B");
        let mut subscription_b = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_b.subscribe(EventgroupId::new(ISOLATED_REBOOT_EVENTGROUP_ID).unwrap()),
        )
        .await
        .expect("Subscribe B timeout")
        .expect("Subscribe B should succeed");

        // Receive first event from service A
        let event_a1 = tokio::time::timeout(Duration::from_secs(5), subscription_a.next())
            .await
            .expect("Event A1 should arrive in time")
            .expect("Should receive event A1");
        assert_eq!(event_a1.payload.as_ref(), b"service_a_event_1");
        tracing::info!("Client: Received first event from service A");

        // Receive event from service B
        let event_b = tokio::time::timeout(Duration::from_secs(5), subscription_b.next())
            .await
            .expect("Event B should arrive in time")
            .expect("Should receive event B");
        assert_eq!(event_b.payload.as_ref(), b"service_b_event");
        tracing::info!("Client: Received event from service B");

        // Subscription B must terminate after server_b reboot
        tracing::info!("Client: Waiting for subscription B to terminate (server_b reboot)");
        let terminated = tokio::time::timeout(Duration::from_secs(5), subscription_b.next())
            .await
            .expect("Subscription B should terminate within timeout after reboot");
        assert!(
            terminated.is_none(),
            "Subscription B must return None after server_b reboot"
        );
        tracing::info!("Client: Subscription B terminated as expected");

        // Service A must survive — receive its second event
        tracing::info!(
            "Client: Waiting for second event from service A (must survive server_b reboot)"
        );
        let event_a2 = tokio::time::timeout(Duration::from_secs(5), subscription_a.next())
            .await
            .expect(
                "Event A2 should arrive — service A must not be disrupted by server_b reboot",
            )
            .expect("Subscription A must still be alive after server_b reboot");
        assert_eq!(event_a2.payload.as_ref(), b"service_a_event_2");
        tracing::info!(
            "Client: Received second event from service A — subscription survived reboot of unrelated peer"
        );

        Ok(())
    });

    sim.run().unwrap();
}
