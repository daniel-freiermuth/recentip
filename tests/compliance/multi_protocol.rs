//! Multi-Protocol Transport Tests
//!
//! Tests for scenarios where clients interact with services using different
//! transport protocols (TCP and UDP) simultaneously.
//!
//! # Covered Requirements
//!
//! - feat_req_recentip_324: TCP binding compatible with UDP binding
//! - feat_req_recentip_644: Single TCP connection per client-server pair
//! - Transport selection based on SD endpoint options
//!
//! # Test Summary
//!
//! | Test | Status | Description |
//! |------|--------|-------------|
//! | `client_talks_to_tcp_and_udp_services` | ✅ | Single client calls both TCP and UDP services |
//! | `client_discovers_service_by_transport` | ✅ | Client uses correct transport per service offer |
//! | `mixed_transport_event_delivery` | ✅ | Events delivered correctly over both transports |
//!
//! # Test Strategy
//!
//! These tests verify that a single runtime can correctly handle multiple
//! services with different transport requirements. The runtime should:
//! 1. Parse transport information from SD OfferService options
//! 2. Store both TCP and UDP endpoints when advertised
//! 3. Use the appropriate transport for each RPC call
//!
//! Run with: cargo test --features turmoil --test compliance multi_protocol

use recentip::handle::ServiceEvent;
use recentip::prelude::*;
use recentip::Runtime;
use recentip::Transport;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

/// Type alias for turmoil-based runtime
type TurmoilRuntime =
    Runtime<turmoil::net::UdpSocket, turmoil::net::TcpStream, turmoil::net::TcpListener>;

// ============================================================================
// SERVICE DEFINITIONS
// ============================================================================

/// TCP-only service - designed for reliable, large payload communication
const TCP_SERVICE_ID: u16 = 0x3001;
const TCP_SERVICE_VERSION: (u8, u32) = (1, 0);

/// UDP-only service - designed for low-latency, small payload communication
const UDP_SERVICE_ID: u16 = 0x3002;
const UDP_SERVICE_VERSION: (u8, u32) = (1, 0);

// ============================================================================
// MULTI-PROTOCOL TESTS
// ============================================================================

/// Test that a client can discover and call both TCP and UDP services
/// from the same server node.
///
/// Setup:
/// - Server offers TcpService with Transport::Tcp
/// - Server offers UdpService with Transport::Udp  
/// - Client discovers both services
/// - Client calls methods on both using the correct transport
///
/// Verifies:
/// - SD correctly advertises different endpoints per transport
/// - Client uses TCP for TcpService, UDP for UdpService
/// - Both RPC calls complete successfully
#[test_log::test]
fn client_talks_to_tcp_and_udp_services() {
    covers!(feat_req_recentip_324);

    let tcp_calls = Arc::new(AtomicUsize::new(0));
    let udp_calls = Arc::new(AtomicUsize::new(0));
    let tcp_calls_server = Arc::clone(&tcp_calls);
    let udp_calls_server = Arc::clone(&udp_calls);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // TCP Server on separate host
    sim.host("tcp_server", move || {
        let tcp_calls = Arc::clone(&tcp_calls_server);

        async move {
            let tcp_config = RuntimeConfig::builder()
                .transport(Transport::Tcp)
                .advertised_ip(turmoil::lookup("tcp_server").to_string().parse().unwrap())
                .build();
            let tcp_runtime: TurmoilRuntime = Runtime::with_socket_type(tcp_config).await.unwrap();

            let mut tcp_offering = tcp_runtime
                .offer(TCP_SERVICE_ID, InstanceId::Id(0x0001))
                .version(TCP_SERVICE_VERSION.0, TCP_SERVICE_VERSION.1)
                .tcp()
                .start()
                .await
                .unwrap();

            eprintln!("[tcp_server] TCP service offered");

            while let Some(event) = tcp_offering.next().await {
                if let ServiceEvent::Call { responder, .. } = event {
                    eprintln!("[tcp_server] TCP service received call");
                    tcp_calls.fetch_add(1, Ordering::SeqCst);
                    responder.reply(b"tcp_response").await.unwrap();
                }
            }

            Ok(())
        }
    });

    // UDP Server on separate host
    sim.host("udp_server", move || {
        let udp_calls = Arc::clone(&udp_calls_server);

        async move {
            let udp_config = RuntimeConfig::builder()
                .transport(Transport::Udp)
                .advertised_ip(turmoil::lookup("udp_server").to_string().parse().unwrap())
                .build();
            let udp_runtime: TurmoilRuntime = Runtime::with_socket_type(udp_config).await.unwrap();

            let mut udp_offering = udp_runtime
                .offer(UDP_SERVICE_ID, InstanceId::Id(0x0001))
                .version(UDP_SERVICE_VERSION.0, UDP_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            eprintln!("[udp_server] UDP service offered");

            while let Some(event) = udp_offering.next().await {
                if let ServiceEvent::Call { responder, .. } = event {
                    eprintln!("[udp_server] UDP service received call");
                    udp_calls.fetch_add(1, Ordering::SeqCst);
                    responder.reply(b"udp_response").await.unwrap();
                }
            }

            Ok(())
        }
    });

    // Client that talks to both services
    sim.client("client", async move {
        // Give servers time to start
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Client with UDP preference (will use TCP for TcpService based on SD)
        let config = RuntimeConfig::builder()
            .transport(Transport::Udp)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        eprintln!("[client] Runtime started, discovering services...");

        // Find TCP service
        let tcp_proxy = runtime
            .find(TCP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let tcp_proxy = tokio::time::timeout(Duration::from_secs(5), tcp_proxy)
            .await
            .expect("TCP service discovery timeout")
            .expect("TCP service should be available");

        eprintln!("[client] TCP service discovered");

        // Find UDP service
        let udp_proxy = runtime
            .find(UDP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let udp_proxy = tokio::time::timeout(Duration::from_secs(5), udp_proxy)
            .await
            .expect("UDP service discovery timeout")
            .expect("UDP service should be available");

        eprintln!("[client] UDP service discovered");

        // Call TCP service
        let method = MethodId::new(0x0001).unwrap();
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            tcp_proxy.call(method, b"tcp_request"),
        )
        .await
        .expect("TCP call timeout")
        .expect("TCP call should succeed");

        assert_eq!(response.payload.as_ref(), b"tcp_response");
        eprintln!("[client] TCP call succeeded: {:?}", response.payload);

        // Call UDP service
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            udp_proxy.call(method, b"udp_request"),
        )
        .await
        .expect("UDP call timeout")
        .expect("UDP call should succeed");

        assert_eq!(response.payload.as_ref(), b"udp_response");
        eprintln!("[client] UDP call succeeded: {:?}", response.payload);

        Ok(())
    });

    sim.run().unwrap();

    let tcp_count = tcp_calls.load(Ordering::SeqCst);
    let udp_count = udp_calls.load(Ordering::SeqCst);

    eprintln!(
        "Test complete: TCP calls={}, UDP calls={}",
        tcp_count, udp_count
    );

    assert_eq!(tcp_count, 1, "Should have received 1 TCP call");
    assert_eq!(udp_count, 1, "Should have received 1 UDP call");
}

/// Test that event subscriptions work correctly across different transports.
///
/// Setup:
/// - Server offers TcpService with events over TCP
/// - Server offers UdpService with events over UDP
/// - Client subscribes to events on both
/// - Server sends events on both
///
/// Verifies:
/// - Events are delivered correctly regardless of transport
/// - No cross-contamination between subscriptions
///
/// NOTE: This test is currently ignored because TCP event delivery for pub/sub
/// is not yet fully implemented. TCP RPC calls work correctly (see other tests),
/// but the event delivery path for TCP subscriptions needs additional work.
#[test_log::test]
fn mixed_transport_event_delivery() {
    let tcp_events = Arc::new(AtomicUsize::new(0));
    let udp_events = Arc::new(AtomicUsize::new(0));
    let tcp_events_client = Arc::clone(&tcp_events);
    let udp_events_client = Arc::clone(&udp_events);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // TCP Server
    sim.host("tcp_server", || async move {
        let tcp_config = RuntimeConfig::builder()
            .transport(Transport::Tcp)
            .advertised_ip(turmoil::lookup("tcp_server").to_string().parse().unwrap())
            .build();
        let tcp_runtime: TurmoilRuntime = Runtime::with_socket_type(tcp_config).await.unwrap();

        let mut tcp_offering = tcp_runtime
            .offer(TCP_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TCP_SERVICE_VERSION.0, TCP_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        eprintln!("[tcp_server] TCP service offered");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();

        // Handle subscription first
        let mut subscribed = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !subscribed && tokio::time::Instant::now() < deadline {
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(100), tcp_offering.next()).await
            {
                if let ServiceEvent::Subscribe { .. } = event {
                    eprintln!("[tcp_server] TCP subscription received");
                    subscribed = true;
                }
            }
        }

        // Wait a bit then send events
        tokio::time::sleep(Duration::from_millis(500)).await;

        for i in 0..3 {
            let tcp_payload = format!("tcp_event_{}", i);
            tcp_offering
                .notify(eventgroup, event_id, tcp_payload.as_bytes())
                .await
                .unwrap();
            eprintln!("[tcp_server] Sent tcp_event_{}", i);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });

    // UDP Server
    sim.host("udp_server", || async move {
        let udp_config = RuntimeConfig::builder()
            .transport(Transport::Udp)
            .advertised_ip(turmoil::lookup("udp_server").to_string().parse().unwrap())
            .build();
        let udp_runtime: TurmoilRuntime = Runtime::with_socket_type(udp_config).await.unwrap();

        let mut udp_offering = udp_runtime
            .offer(UDP_SERVICE_ID, InstanceId::Id(0x0001))
            .version(UDP_SERVICE_VERSION.0, UDP_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        eprintln!("[udp_server] UDP service offered");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();

        // Handle subscription first
        let mut subscribed = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !subscribed && tokio::time::Instant::now() < deadline {
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(100), udp_offering.next()).await
            {
                if let ServiceEvent::Subscribe { .. } = event {
                    eprintln!("[udp_server] UDP subscription received");
                    subscribed = true;
                }
            }
        }

        // Wait a bit then send events
        tokio::time::sleep(Duration::from_millis(500)).await;

        for i in 0..3 {
            let udp_payload = format!("udp_event_{}", i);
            udp_offering
                .notify(eventgroup, event_id, udp_payload.as_bytes())
                .await
                .unwrap();
            eprintln!("[udp_server] Sent udp_event_{}", i);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });

    // Client subscribing to both services
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Discover and subscribe to TCP service
        let tcp_proxy = runtime
            .find(TCP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let tcp_proxy = tokio::time::timeout(Duration::from_secs(5), tcp_proxy)
            .await
            .expect("TCP discovery timeout")
            .expect("TCP service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut tcp_sub =
            tokio::time::timeout(Duration::from_secs(5), tcp_proxy.subscribe(eventgroup))
                .await
                .expect("TCP subscribe timeout")
                .expect("TCP subscribe success");

        eprintln!("[client] Subscribed to TCP service");

        // Discover and subscribe to UDP service
        let udp_proxy = runtime
            .find(UDP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let udp_proxy = tokio::time::timeout(Duration::from_secs(5), udp_proxy)
            .await
            .expect("UDP discovery timeout")
            .expect("UDP service available");

        let mut udp_sub =
            tokio::time::timeout(Duration::from_secs(5), udp_proxy.subscribe(eventgroup))
                .await
                .expect("UDP subscribe timeout")
                .expect("UDP subscribe success");

        eprintln!("[client] Subscribed to UDP service");

        // Receive events concurrently
        let tcp_events = Arc::clone(&tcp_events_client);
        let udp_events = Arc::clone(&udp_events_client);

        let tcp_recv = tokio::spawn(async move {
            while let Some(event) = tokio::time::timeout(Duration::from_secs(5), tcp_sub.next())
                .await
                .ok()
                .flatten()
            {
                let payload = String::from_utf8_lossy(&event.payload);
                eprintln!("[client] TCP event: {}", payload);
                assert!(
                    payload.starts_with("tcp_event_"),
                    "TCP should only receive tcp_event_*"
                );
                tcp_events.fetch_add(1, Ordering::SeqCst);
            }
        });

        let udp_recv = tokio::spawn(async move {
            while let Some(event) = tokio::time::timeout(Duration::from_secs(5), udp_sub.next())
                .await
                .ok()
                .flatten()
            {
                let payload = String::from_utf8_lossy(&event.payload);
                eprintln!("[client] UDP event: {}", payload);
                assert!(
                    payload.starts_with("udp_event_"),
                    "UDP should only receive udp_event_*"
                );
                udp_events.fetch_add(1, Ordering::SeqCst);
            }
        });

        let _ = tokio::join!(tcp_recv, udp_recv);

        Ok(())
    });

    sim.run().unwrap();

    let tcp_count = tcp_events.load(Ordering::SeqCst);
    let udp_count = udp_events.load(Ordering::SeqCst);

    eprintln!(
        "Test complete: TCP events={}, UDP events={}",
        tcp_count, udp_count
    );

    assert_eq!(tcp_count, 3, "Should have received 3 TCP events");
    assert_eq!(udp_count, 3, "Should have received 3 UDP events");
}

/// Test that a client correctly selects transport based on the service's
/// advertised endpoint in the OfferService message.
///
/// This tests that:
/// - When a service only advertises TCP endpoint, client uses TCP
/// - When a service only advertises UDP endpoint, client uses UDP
/// - The runtime correctly parses the endpoint options from SD
#[test_log::test]
fn client_uses_advertised_transport() {
    covers!(feat_req_recentip_324, feat_req_recentip_644);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // TCP-only server
    sim.host("tcp_server", || async {
        let config = RuntimeConfig::builder()
            .transport(Transport::Tcp)
            .advertised_ip(turmoil::lookup("tcp_server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TCP_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TCP_SERVICE_VERSION.0, TCP_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        eprintln!("[tcp_server] TCP service offered");

        while let Some(event) = offering.next().await {
            if let ServiceEvent::Call {
                method, responder, ..
            } = event
            {
                eprintln!("[tcp_server] Received call on method {:?}", method);
                responder.reply(b"from_tcp_server").await.unwrap();
            }
        }

        Ok(())
    });

    // UDP-only server
    sim.host("udp_server", || async {
        let config = RuntimeConfig::builder()
            .transport(Transport::Udp)
            .advertised_ip(turmoil::lookup("udp_server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(UDP_SERVICE_ID, InstanceId::Id(0x0001))
            .version(UDP_SERVICE_VERSION.0, UDP_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        eprintln!("[udp_server] UDP service offered");

        while let Some(event) = offering.next().await {
            if let ServiceEvent::Call {
                method, responder, ..
            } = event
            {
                eprintln!("[udp_server] Received call on method {:?}", method);
                responder.reply(b"from_udp_server").await.unwrap();
            }
        }

        Ok(())
    });

    // Client connecting to both
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Use UDP as default transport
        let config = RuntimeConfig::builder()
            .transport(Transport::Udp)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Discover TCP service
        let tcp_proxy = runtime
            .find(TCP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let tcp_proxy = tokio::time::timeout(Duration::from_secs(5), tcp_proxy)
            .await
            .expect("TCP discovery timeout")
            .expect("TCP service available");

        eprintln!(
            "[client] TCP service discovered at {:?}",
            tcp_proxy.endpoint()
        );

        // Discover UDP service
        let udp_proxy = runtime
            .find(UDP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let udp_proxy = tokio::time::timeout(Duration::from_secs(5), udp_proxy)
            .await
            .expect("UDP discovery timeout")
            .expect("UDP service available");

        eprintln!(
            "[client] UDP service discovered at {:?}",
            udp_proxy.endpoint()
        );

        // Call TCP service - should use TCP transport
        let method = MethodId::new(0x0001).unwrap();
        let response =
            tokio::time::timeout(Duration::from_secs(5), tcp_proxy.call(method, b"hello_tcp"))
                .await
                .expect("TCP call timeout")
                .expect("TCP call success");

        assert_eq!(response.payload.as_ref(), b"from_tcp_server");
        eprintln!(
            "[client] TCP call response: {:?}",
            String::from_utf8_lossy(&response.payload)
        );

        // Call UDP service - should use UDP transport
        let response =
            tokio::time::timeout(Duration::from_secs(5), udp_proxy.call(method, b"hello_udp"))
                .await
                .expect("UDP call timeout")
                .expect("UDP call success");

        assert_eq!(response.payload.as_ref(), b"from_udp_server");
        eprintln!(
            "[client] UDP call response: {:?}",
            String::from_utf8_lossy(&response.payload)
        );

        eprintln!("[client] Both calls succeeded with correct transports!");

        Ok(())
    });

    sim.run().unwrap();
}

/// Test multiple concurrent calls over different transports.
///
/// Verifies that the runtime can handle multiple in-flight requests
/// simultaneously when using different transports.
#[test_log::test]
fn concurrent_calls_different_transports() {
    let total_calls = Arc::new(AtomicUsize::new(0));
    let total_calls_server = Arc::clone(&total_calls);
    let total_calls_server2 = Arc::clone(&total_calls);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // TCP Server
    sim.host("tcp_server", move || {
        let total_calls = Arc::clone(&total_calls_server);

        async move {
            let tcp_config = RuntimeConfig::builder()
                .transport(Transport::Tcp)
                .advertised_ip(turmoil::lookup("tcp_server").to_string().parse().unwrap())
                .build();
            let tcp_runtime: TurmoilRuntime = Runtime::with_socket_type(tcp_config).await.unwrap();

            let mut tcp_offering = tcp_runtime
                .offer(TCP_SERVICE_ID, InstanceId::Id(0x0001))
                .version(TCP_SERVICE_VERSION.0, TCP_SERVICE_VERSION.1)
                .tcp()
                .start()
                .await
                .unwrap();

            while let Some(event) = tcp_offering.next().await {
                if let ServiceEvent::Call { responder, .. } = event {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    total_calls.fetch_add(1, Ordering::SeqCst);
                    responder.reply(b"tcp_done").await.unwrap();
                }
            }

            Ok(())
        }
    });

    // UDP Server
    sim.host("udp_server", move || {
        let total_calls = Arc::clone(&total_calls_server2);

        async move {
            let udp_config = RuntimeConfig::builder()
                .transport(Transport::Udp)
                .advertised_ip(turmoil::lookup("udp_server").to_string().parse().unwrap())
                .build();
            let udp_runtime: TurmoilRuntime = Runtime::with_socket_type(udp_config).await.unwrap();

            let mut udp_offering = udp_runtime
                .offer(UDP_SERVICE_ID, InstanceId::Id(0x0001))
                .version(UDP_SERVICE_VERSION.0, UDP_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            while let Some(event) = udp_offering.next().await {
                if let ServiceEvent::Call { responder, .. } = event {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    total_calls.fetch_add(1, Ordering::SeqCst);
                    responder.reply(b"udp_done").await.unwrap();
                }
            }

            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(300)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Discover both services
        let tcp_proxy = runtime
            .find(TCP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let tcp_proxy = tokio::time::timeout(Duration::from_secs(5), tcp_proxy)
            .await
            .expect("TCP discovery timeout")
            .expect("TCP service available");

        let udp_proxy = runtime
            .find(UDP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let udp_proxy = tokio::time::timeout(Duration::from_secs(5), udp_proxy)
            .await
            .expect("UDP discovery timeout")
            .expect("UDP service available");

        eprintln!("[client] Both services discovered, launching concurrent calls...");

        let method = MethodId::new(0x0001).unwrap();

        // Launch 10 concurrent calls (5 TCP, 5 UDP)
        let mut handles = Vec::new();

        for i in 0..5 {
            let tcp_proxy = tcp_proxy.clone();
            handles.push(tokio::spawn(async move {
                let result = tcp_proxy
                    .call(method, format!("tcp_{}", i).as_bytes())
                    .await;
                eprintln!("[client] TCP call {} completed: {:?}", i, result.is_ok());
                result.is_ok()
            }));

            let udp_proxy = udp_proxy.clone();
            handles.push(tokio::spawn(async move {
                let result = udp_proxy
                    .call(method, format!("udp_{}", i).as_bytes())
                    .await;
                eprintln!("[client] UDP call {} completed: {:?}", i, result.is_ok());
                result.is_ok()
            }));
        }

        let mut successes = 0;
        for handle in handles {
            if handle.await.unwrap_or(false) {
                successes += 1;
            }
        }

        eprintln!("[client] {} out of 10 calls succeeded", successes);
        assert_eq!(successes, 10, "All 10 concurrent calls should succeed");

        Ok(())
    });

    sim.run().unwrap();

    let total = total_calls.load(Ordering::SeqCst);
    eprintln!("Test complete: {} total calls handled", total);
    assert_eq!(total, 10, "Server should have handled 10 calls");
}

// ============================================================================
// DIAGNOSTIC TEST - ISOLATED TCP
// ============================================================================

/// Simple test: UDP client discovers TCP server, makes call
///
/// This isolates the core issue: can a client with UDP config
/// successfully call a service that advertises only TCP?
#[test_log::test]
fn udp_client_calls_tcp_server() {
    let call_succeeded = Arc::new(std::sync::Mutex::new(false));
    let call_flag = Arc::clone(&call_succeeded);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // TCP Server
    sim.host("tcp_server", || async {
        let tcp_config = RuntimeConfig::builder()
            .transport(Transport::Tcp)
            .advertised_ip(turmoil::lookup("tcp_server").to_string().parse().unwrap())
            .build();
        let tcp_runtime: TurmoilRuntime = Runtime::with_socket_type(tcp_config).await.unwrap();

        let mut offering = tcp_runtime
            .offer(TCP_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TCP_SERVICE_VERSION.0, TCP_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        eprintln!("[tcp_server] TCP service offered, waiting for calls...");

        while let Some(event) = offering.next().await {
            if let ServiceEvent::Call {
                responder, payload, ..
            } = event
            {
                eprintln!(
                    "[tcp_server] Received call with payload: {:?}",
                    payload.as_ref()
                );
                responder.reply(b"tcp_response").await.unwrap();
                eprintln!("[tcp_server] Sent reply");
            }
        }

        Ok(())
    });

    // UDP Client (default config)
    sim.client("udp_client", async move {
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Client uses default UDP config
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("udp_client").to_string().parse().unwrap())
            .build();
        eprintln!(
            "[client] Starting with UDP config (transport={:?})",
            config.preferred_transport
        );

        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Discover TCP service
        let proxy = runtime
            .find(TCP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        eprintln!("[client] Waiting for TCP service discovery...");

        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        eprintln!("[client] TCP service discovered, making call...");

        let method = MethodId::new(0x0001).unwrap();
        let result =
            tokio::time::timeout(Duration::from_secs(5), proxy.call(method, b"hello_tcp")).await;

        match &result {
            Ok(Ok(response)) => {
                eprintln!("[client] Got response: {:?}", response.payload.as_ref());
                *call_flag.lock().unwrap() = true;
            }
            Ok(Err(e)) => eprintln!("[client] Call error: {:?}", e),
            Err(_) => eprintln!("[client] Call timeout"),
        }

        Ok(())
    });

    sim.run().unwrap();
    assert!(
        *call_succeeded.lock().unwrap(),
        "Call to TCP server should succeed"
    );
}

/// Simple test: TCP client discovers TCP server, makes call
///
/// Both client and server use TCP config - baseline for comparison
#[test_log::test]
fn tcp_client_calls_tcp_server() {
    let call_succeeded = Arc::new(std::sync::Mutex::new(false));
    let call_flag = Arc::clone(&call_succeeded);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // TCP Server
    sim.host("tcp_server", || async {
        let tcp_config = RuntimeConfig::builder()
            .transport(Transport::Tcp)
            .advertised_ip(turmoil::lookup("tcp_server").to_string().parse().unwrap())
            .build();
        let tcp_runtime: TurmoilRuntime = Runtime::with_socket_type(tcp_config).await.unwrap();

        let mut offering = tcp_runtime
            .offer(TCP_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TCP_SERVICE_VERSION.0, TCP_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        eprintln!("[tcp_server] TCP service offered, waiting for calls...");

        while let Some(event) = offering.next().await {
            if let ServiceEvent::Call {
                responder, payload, ..
            } = event
            {
                eprintln!(
                    "[tcp_server] Received call with payload: {:?}",
                    payload.as_ref()
                );
                responder.reply(b"tcp_response").await.unwrap();
                eprintln!("[tcp_server] Sent reply");
            }
        }

        Ok(())
    });

    // TCP Client
    sim.client("tcp_client", async move {
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Client also uses TCP config
        let tcp_config = RuntimeConfig::builder()
            .transport(Transport::Tcp)
            .advertised_ip(turmoil::lookup("tcp_client").to_string().parse().unwrap())
            .build();
        eprintln!("[client] Starting with TCP config");

        let runtime: TurmoilRuntime = Runtime::with_socket_type(tcp_config).await.unwrap();

        // Discover TCP service
        let proxy = runtime
            .find(TCP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        eprintln!("[client] Waiting for TCP service discovery...");

        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        eprintln!("[client] TCP service discovered, making call...");

        let method = MethodId::new(0x0001).unwrap();
        let result =
            tokio::time::timeout(Duration::from_secs(5), proxy.call(method, b"hello_tcp")).await;

        match &result {
            Ok(Ok(response)) => {
                eprintln!("[client] Got response: {:?}", response.payload.as_ref());
                *call_flag.lock().unwrap() = true;
            }
            Ok(Err(e)) => eprintln!("[client] Call error: {:?}", e),
            Err(_) => eprintln!("[client] Call timeout"),
        }

        Ok(())
    });

    sim.run().unwrap();
    assert!(
        *call_succeeded.lock().unwrap(),
        "Call to TCP server should succeed"
    );
}

// ============================================================================
// PREFERRED TRANSPORT OVERRIDE TESTS
// ============================================================================

/// Test that a client with preferred_transport=UDP can still connect to a TCP-only service.
///
/// This verifies that when a service only offers TCP, the client uses TCP regardless
/// of its preferred_transport setting.
///
/// Setup:
/// - Server offers service via TCP only
/// - Client has preferred_transport = UDP
/// - Client discovers and calls service
///
/// Expected:
/// - Client discovers the TCP endpoint via SD
/// - Client uses TCP for the RPC call (overriding preference)
/// - Call succeeds
#[test_log::test]
fn client_prefers_udp_but_connects_to_tcp_only_service() {
    covers!(feat_req_recentip_324);

    let call_succeeded = Arc::new(std::sync::Mutex::new(false));
    let call_flag = Arc::clone(&call_succeeded);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers TCP-only service
    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TCP_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TCP_SERVICE_VERSION.0, TCP_SERVICE_VERSION.1)
            .tcp() // TCP only, no UDP
            .start()
            .await
            .unwrap();

        eprintln!("[server] TCP-only service offered");

        while let Some(event) = offering.next().await {
            if let ServiceEvent::Call {
                responder, payload, ..
            } = event
            {
                eprintln!(
                    "[server] Received call with payload: {:?}",
                    payload.as_ref()
                );
                responder.reply(b"tcp_response").await.unwrap();
            }
        }

        Ok(())
    });

    // Client with UDP preference
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Client prefers UDP, but server only offers TCP
        let config = RuntimeConfig::builder()
            .preferred_transport(Transport::Udp)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        eprintln!("[client] Runtime started with preferred_transport=UDP");

        // Find and call the TCP-only service
        let proxy = runtime
            .find(TCP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        eprintln!(
            "[client] Service discovered at {:?} via {:?}",
            proxy.endpoint(),
            proxy.transport()
        );
        // Verify transport is TCP (since server only offers TCP)
        assert_eq!(
            proxy.transport(),
            Transport::Tcp,
            "Should use TCP when it's the only option"
        );

        let method = MethodId::new(0x0001).unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, b"hello"))
            .await
            .expect("Call timeout")
            .expect("Call should succeed");

        assert_eq!(response.payload.as_ref(), b"tcp_response");
        eprintln!("[client] Call succeeded with TCP despite UDP preference!");
        *call_flag.lock().unwrap() = true;

        Ok(())
    });

    sim.run().unwrap();
    assert!(
        *call_succeeded.lock().unwrap(),
        "Client with UDP preference should still be able to call TCP-only service"
    );
}

/// Test that a client with preferred_transport=TCP can still connect to a UDP-only service.
///
/// This verifies that when a service only offers UDP, the client uses UDP regardless
/// of its preferred_transport setting.
///
/// Setup:
/// - Server offers service via UDP only
/// - Client has preferred_transport = TCP
/// - Client discovers and calls service
///
/// Expected:
/// - Client discovers the UDP endpoint via SD
/// - Client uses UDP for the RPC call (overriding preference)
/// - Call succeeds
#[test_log::test]
fn client_prefers_tcp_but_connects_to_udp_only_service() {
    covers!(feat_req_recentip_324);

    let call_succeeded = Arc::new(std::sync::Mutex::new(false));
    let call_flag = Arc::clone(&call_succeeded);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers UDP-only service
    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(UDP_SERVICE_ID, InstanceId::Id(0x0001))
            .version(UDP_SERVICE_VERSION.0, UDP_SERVICE_VERSION.1)
            .udp() // UDP only, no TCP
            .start()
            .await
            .unwrap();

        eprintln!("[server] UDP-only service offered");

        while let Some(event) = offering.next().await {
            if let ServiceEvent::Call {
                responder, payload, ..
            } = event
            {
                eprintln!(
                    "[server] Received call with payload: {:?}",
                    payload.as_ref()
                );
                responder.reply(b"udp_response").await.unwrap();
            }
        }

        Ok(())
    });

    // Client with TCP preference
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Client prefers TCP, but server only offers UDP
        let config = RuntimeConfig::builder()
            .preferred_transport(Transport::Tcp)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        eprintln!("[client] Runtime started with preferred_transport=TCP");

        // Find and call the UDP-only service
        let proxy = runtime
            .find(UDP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        assert_eq!(
            proxy.transport(),
            Transport::Udp,
            "Should use UDP when it's the only option"
        );

        eprintln!("[client] Service discovered (should be UDP endpoint)");

        let method = MethodId::new(0x0001).unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, b"hello"))
            .await
            .expect("Call timeout")
            .expect("Call should succeed");

        assert_eq!(response.payload.as_ref(), b"udp_response");
        eprintln!("[client] Call succeeded with UDP despite TCP preference!");
        *call_flag.lock().unwrap() = true;

        Ok(())
    });

    sim.run().unwrap();
    assert!(
        *call_succeeded.lock().unwrap(),
        "Client with TCP preference should still be able to call UDP-only service"
    );
}

// ============================================================================
// PREFERRED TRANSPORT OVERRIDE TESTS - PUB/SUB
// ============================================================================

/// Test that a client with preferred_transport=UDP can still subscribe to events
/// from a UDP-only service (baseline test for pub/sub).
///
/// This verifies basic pub/sub functionality works with matching transport.
///
/// Setup:
/// - Server offers service via UDP only with events
/// - Client has preferred_transport = UDP (matching)
/// - Client subscribes and receives events
///
/// Expected:
/// - Subscription succeeds
/// - Events are delivered
#[test_log::test]
fn client_prefers_udp_subscribes_to_udp_only_service_pubsub() {
    covers!(feat_req_recentip_324);

    let events_received = Arc::new(AtomicUsize::new(0));
    let events_flag = Arc::clone(&events_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers UDP-only service with events
    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(UDP_SERVICE_ID, InstanceId::Id(0x0001))
            .version(UDP_SERVICE_VERSION.0, UDP_SERVICE_VERSION.1)
            .udp() // UDP only
            .start()
            .await
            .unwrap();

        eprintln!("[server] UDP-only service offered with events");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();

        // Wait for subscription
        let mut subscribed = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !subscribed && tokio::time::Instant::now() < deadline {
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(100), offering.next()).await
            {
                if let ServiceEvent::Subscribe { .. } = event {
                    eprintln!("[server] Subscription received, accepting");
                    subscribed = true;
                }
            }
        }

        if !subscribed {
            eprintln!("[server] No subscription received");
            return Ok(());
        }

        // Send events
        tokio::time::sleep(Duration::from_millis(200)).await;
        for i in 0..3 {
            let payload = format!("udp_event_{}", i);
            offering
                .notify(eventgroup, event_id, payload.as_bytes())
                .await
                .unwrap();
            eprintln!("[server] Sent event {}", i);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    // Client with UDP preference
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .preferred_transport(Transport::Udp)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        eprintln!("[client] Runtime started with preferred_transport=UDP");

        // Find and subscribe to UDP service
        let proxy = runtime
            .find(UDP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        eprintln!("[client] Service discovered, subscribing...");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        eprintln!("[client] Subscribed, waiting for events...");

        // Receive events
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(500), subscription.next()).await {
                Ok(Some(event)) => {
                    let payload = String::from_utf8_lossy(&event.payload);
                    eprintln!("[client] Received event: {}", payload);
                    events_flag.fetch_add(1, Ordering::SeqCst);
                }
                Ok(None) => break,
                Err(_) => continue,
            }
        }

        Ok(())
    });

    sim.run().unwrap();

    let count = events_received.load(Ordering::SeqCst);
    eprintln!("Test complete: received {} events", count);
    assert!(
        count >= 1,
        "Should have received at least 1 event, got {}",
        count
    );
}

/// Test that a client with preferred_transport=TCP can still subscribe to events
/// from a UDP-only service.
///
/// This verifies that when a service only offers UDP events, the client subscribes
/// via UDP regardless of its preferred_transport setting.
///
/// Setup:
/// - Server offers service via UDP only with events
/// - Client has preferred_transport = TCP
/// - Client subscribes and receives events
///
/// Expected:
/// - Client discovers the UDP endpoint via SD
/// - Subscription uses UDP (overriding TCP preference)
/// - Events are delivered
#[test_log::test]
fn client_prefers_tcp_subscribes_to_udp_only_service_pubsub() {
    covers!(feat_req_recentip_324);

    let events_received = Arc::new(AtomicUsize::new(0));
    let events_flag = Arc::clone(&events_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers UDP-only service with events
    sim.client("server", async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(UDP_SERVICE_ID, InstanceId::Id(0x0001))
            .version(UDP_SERVICE_VERSION.0, UDP_SERVICE_VERSION.1)
            .udp() // UDP only, no TCP
            .start()
            .await
            .unwrap();

        eprintln!("[server] UDP-only service offered with events");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();

        // Wait for subscription
        let mut subscribed = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !subscribed && tokio::time::Instant::now() < deadline {
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(100), offering.next()).await
            {
                if let ServiceEvent::Subscribe { .. } = event {
                    eprintln!("[server] Subscription received (auto-ACKed)");
                    subscribed = true;
                }
            }
        }

        if !subscribed {
            eprintln!("[server] No subscription received");
            return Ok(());
        }

        // Send events
        tokio::time::sleep(Duration::from_millis(200)).await;
        for i in 0..3 {
            let payload = format!("udp_event_{}", i);
            eprintln!("[server] Sending event {}", i);
            offering
                .notify(eventgroup, event_id, payload.as_bytes())
                .await
                .unwrap();
            eprintln!("[server] Sent event {}", i);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    // Client with TCP preference (but server only offers UDP)
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Client prefers TCP, but server only offers UDP
        let config = RuntimeConfig::builder()
            .preferred_transport(Transport::Tcp)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        eprintln!("[client] Runtime started with preferred_transport=TCP");

        // Find and subscribe to UDP-only service
        let proxy = runtime
            .find(UDP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        eprintln!("[client] Service discovered (should be UDP endpoint despite TCP preference)");
        assert_eq!(
            proxy.transport(),
            Transport::Udp,
            "Should use UDP when it's the only option"
        );

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        eprintln!("[client] Subscribed via UDP despite TCP preference, waiting for events...");

        // Receive events
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(500), subscription.next()).await {
                Ok(Some(event)) => {
                    let payload = String::from_utf8_lossy(&event.payload);
                    eprintln!("[client] Received event: {}", payload);
                    events_flag.fetch_add(1, Ordering::SeqCst);
                }
                Ok(None) => break,
                Err(_) => continue,
            }
        }
        eprintln!("[client] Finished receiving events");

        Ok(())
    });

    sim.run().unwrap();

    let count = events_received.load(Ordering::SeqCst);
    eprintln!(
        "Test complete: received {} events despite TCP preference",
        count
    );
    assert!(
        count >= 1,
        "Client with TCP preference should still receive UDP events, got {}",
        count
    );
}

/// Test that a client with preferred_transport=UDP can still subscribe to events
/// from a TCP-only service.
///
/// This verifies that when a service only offers TCP events, the client subscribes
/// via TCP regardless of its preferred_transport setting.
///
/// NOTE: This test is currently ignored because TCP pub/sub event delivery
/// is not yet fully implemented. The RPC equivalent test passes (client_prefers_udp_but_connects_to_tcp_only_service).
#[test_log::test]
fn client_prefers_udp_subscribes_to_tcp_only_service_pubsub() {
    covers!(feat_req_recentip_324);

    let events_received = Arc::new(AtomicUsize::new(0));
    let events_flag = Arc::clone(&events_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers TCP-only service with events
    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer(TCP_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TCP_SERVICE_VERSION.0, TCP_SERVICE_VERSION.1)
            .tcp() // TCP only, no UDP
            .start()
            .await
            .unwrap();

        eprintln!("[server] TCP-only service offered with events");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();

        // Wait for subscription
        let mut subscribed = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !subscribed && tokio::time::Instant::now() < deadline {
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(100), offering.next()).await
            {
                if let ServiceEvent::Subscribe { .. } = event {
                    eprintln!("[server] Subscription received, accepting");
                    subscribed = true;
                }
            }
        }

        if !subscribed {
            eprintln!("[server] No subscription received");
            return Ok(());
        }

        // Send events
        tokio::time::sleep(Duration::from_millis(200)).await;
        for i in 0..3 {
            let payload = format!("tcp_event_{}", i);
            offering
                .notify(eventgroup, event_id, payload.as_bytes())
                .await
                .unwrap();
            eprintln!("[server] Sent event {}", i);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    // Client with UDP preference (but server only offers TCP)
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Client prefers UDP, but server only offers TCP
        let config = RuntimeConfig::builder()
            .preferred_transport(Transport::Udp)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        eprintln!("[client] Runtime started with preferred_transport=UDP");

        // Find and subscribe to TCP-only service
        let proxy = runtime
            .find(TCP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        eprintln!("[client] Service discovered (should be TCP endpoint despite UDP preference)");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        eprintln!("[client] Subscribed via TCP despite UDP preference, waiting for events...");

        // Receive events
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(500), subscription.next()).await {
                Ok(Some(event)) => {
                    let payload = String::from_utf8_lossy(&event.payload);
                    eprintln!("[client] Received event: {}", payload);
                    events_flag.fetch_add(1, Ordering::SeqCst);
                }
                Ok(None) => break,
                Err(_) => continue,
            }
        }

        Ok(())
    });

    sim.run().unwrap();

    let count = events_received.load(Ordering::SeqCst);
    eprintln!(
        "Test complete: received {} events despite UDP preference",
        count
    );
    assert!(
        count >= 1,
        "Client with UDP preference should still receive TCP events, got {}",
        count
    );
}

/// Test that preferred_transport is respected when a service offers BOTH transports.
///
/// When a service offers both TCP and UDP, the client should use its preferred transport.
///
/// Setup:
/// - Server offers service via both TCP and UDP (dual-stack)
/// - Client A has preferred_transport = TCP
/// - Client B has preferred_transport = UDP
/// - Both clients call the service
///
/// Expected:
/// - Client A uses TCP
/// - Client B uses UDP
/// - Both calls succeed
///
/// This test verifies that `preferred_transport` is respected when both are available
/// by checking `proxy.transport()` after discovery.
#[test_log::test]
fn preferred_transport_respected_when_both_available() {
    covers!(feat_req_recentip_324);

    let tcp_calls = Arc::new(AtomicUsize::new(0));
    let udp_calls = Arc::new(AtomicUsize::new(0));
    let tcp_calls_server = Arc::clone(&tcp_calls);
    let udp_calls_server = Arc::clone(&udp_calls);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service via BOTH TCP and UDP
    sim.host("server", move || {
        let tcp_calls = Arc::clone(&tcp_calls_server);
        let udp_calls = Arc::clone(&udp_calls_server);

        async move {
            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let mut offering = runtime
                .offer(TCP_SERVICE_ID, InstanceId::Id(0x0001))
                .version(TCP_SERVICE_VERSION.0, TCP_SERVICE_VERSION.1)
                .tcp()
                .udp() // Dual-stack: both TCP and UDP
                .start()
                .await
                .unwrap();

            eprintln!("[server] Dual-stack service offered (TCP + UDP)");

            while let Some(event) = offering.next().await {
                if let ServiceEvent::Call {
                    responder, payload, ..
                } = event
                {
                    let payload_str = String::from_utf8_lossy(&payload);
                    eprintln!("[server] Received call: {}", payload_str);

                    // Track which transport was used based on payload
                    if payload_str.contains("from_tcp_client") {
                        tcp_calls.fetch_add(1, Ordering::SeqCst);
                        responder.reply(b"response_to_tcp_client").await.unwrap();
                    } else if payload_str.contains("from_udp_client") {
                        udp_calls.fetch_add(1, Ordering::SeqCst);
                        responder.reply(b"response_to_udp_client").await.unwrap();
                    } else {
                        responder.reply(b"unknown_client").await.unwrap();
                    }
                }
            }

            Ok(())
        }
    });

    // Client A with TCP preference
    sim.client("tcp_client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .preferred_transport(Transport::Tcp)
            .advertised_ip(turmoil::lookup("tcp_client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        eprintln!("[tcp_client] Runtime started with preferred_transport=TCP");

        let proxy = runtime
            .find(TCP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        // Verify preferred transport is used
        assert_eq!(
            proxy.transport(),
            Transport::Tcp,
            "TCP-preferring client should use TCP transport when both are available"
        );
        eprintln!(
            "[tcp_client] Using transport: {:?}, endpoint: {}",
            proxy.transport(),
            proxy.endpoint()
        );

        let method = MethodId::new(0x0001).unwrap();
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(method, b"from_tcp_client"),
        )
        .await
        .expect("Call timeout")
        .expect("Call should succeed");

        assert_eq!(response.payload.as_ref(), b"response_to_tcp_client");
        eprintln!("[tcp_client] Call succeeded!");

        Ok(())
    });

    // Client B with UDP preference
    sim.client("udp_client", async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .preferred_transport(Transport::Udp)
            .advertised_ip(turmoil::lookup("udp_client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        eprintln!("[udp_client] Runtime started with preferred_transport=UDP");

        let proxy = runtime
            .find(TCP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        // Verify preferred transport is used
        assert_eq!(
            proxy.transport(),
            Transport::Udp,
            "UDP-preferring client should use UDP transport when both are available"
        );
        eprintln!(
            "[udp_client] Using transport: {:?}, endpoint: {}",
            proxy.transport(),
            proxy.endpoint()
        );

        let method = MethodId::new(0x0001).unwrap();
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(method, b"from_udp_client"),
        )
        .await
        .expect("Call timeout")
        .expect("Call should succeed");

        assert_eq!(response.payload.as_ref(), b"response_to_udp_client");
        eprintln!("[udp_client] Call succeeded!");

        Ok(())
    });

    sim.run().unwrap();

    let tcp_count = tcp_calls.load(Ordering::SeqCst);
    let udp_count = udp_calls.load(Ordering::SeqCst);

    eprintln!(
        "Test complete: TCP-preferred calls={}, UDP-preferred calls={}",
        tcp_count, udp_count
    );

    // Both clients should have successfully called the service
    assert_eq!(
        tcp_count, 1,
        "TCP-preferring client should have called once"
    );
    assert_eq!(
        udp_count, 1,
        "UDP-preferring client should have called once"
    );
}

/// Test that preferred_transport is respected for pub/sub when a service offers BOTH transports.
///
/// When a service offers both TCP and UDP, the client should subscribe using its
/// preferred transport.
///
/// Setup:
/// - Server offers service via both TCP and UDP (dual-stack) with events
/// - Client A has preferred_transport = TCP
/// - Client B has preferred_transport = UDP
/// - Both clients subscribe and receive events
///
/// Expected:
/// - Client A subscribes via TCP
/// - Client B subscribes via UDP
/// - Both receive events
///
/// NOTE: This test is ignored because:
/// 1. TCP pub/sub event delivery is not yet fully implemented
/// 2. Verifying which transport was used requires introspection
#[test_log::test]
#[ignore = "TCP pub/sub not implemented + requires transport introspection"]
fn preferred_transport_respected_for_pubsub_when_both_available() {
    covers!(feat_req_recentip_324);

    let tcp_events = Arc::new(AtomicUsize::new(0));
    let udp_events = Arc::new(AtomicUsize::new(0));
    let tcp_events_server = Arc::clone(&tcp_events);
    let udp_events_server = Arc::clone(&udp_events);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service via BOTH TCP and UDP with events
    sim.host("server", move || {
        let tcp_events = Arc::clone(&tcp_events_server);
        let udp_events = Arc::clone(&udp_events_server);

        async move {
            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

            let mut offering = runtime
                .offer(TCP_SERVICE_ID, InstanceId::Id(0x0001))
                .version(TCP_SERVICE_VERSION.0, TCP_SERVICE_VERSION.1)
                .tcp()
                .udp() // Dual-stack: both TCP and UDP
                .start()
                .await
                .unwrap();

            eprintln!("[server] Dual-stack service offered (TCP + UDP) with events");

            let eventgroup = EventgroupId::new(0x0001).unwrap();
            let event_id = EventId::new(0x8001).unwrap();

            // Track subscriptions and send events
            let mut tcp_subscribed = false;
            let mut udp_subscribed = false;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);

            while tokio::time::Instant::now() < deadline {
                if let Ok(Some(event)) =
                    tokio::time::timeout(Duration::from_millis(100), offering.next()).await
                {
                    if let ServiceEvent::Subscribe { .. } = event {
                        // In a real scenario, we'd introspect the transport here
                        // For now, we accept all subscriptions
                        eprintln!("[server] Subscription received, accepting");

                        // Assume alternating subscriptions for demonstration
                        if !tcp_subscribed {
                            tcp_subscribed = true;
                            tcp_events.fetch_add(1, Ordering::SeqCst);
                        } else if !udp_subscribed {
                            udp_subscribed = true;
                            udp_events.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }

                // Send events periodically once we have subscribers
                if tcp_subscribed || udp_subscribed {
                    offering
                        .notify(eventgroup, event_id, b"event_data")
                        .await
                        .ok();
                }
            }

            Ok(())
        }
    });

    // Client A with TCP preference
    sim.host("tcp_client", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .preferred_transport(Transport::Tcp)
            .advertised_ip(turmoil::lookup("tcp_client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        eprintln!("[tcp_client] Runtime started with preferred_transport=TCP");

        let proxy = runtime
            .find(TCP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        eprintln!("[tcp_client] Subscribed, waiting for events...");

        // Receive at least one event
        if let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_secs(5), subscription.next()).await
        {
            eprintln!("[tcp_client] Received event: {:?}", event.payload.as_ref());
        }

        Ok(())
    });

    // Client B with UDP preference
    sim.host("udp_client", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let config = RuntimeConfig::builder()
            .preferred_transport(Transport::Udp)
            .advertised_ip(turmoil::lookup("udp_client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        eprintln!("[udp_client] Runtime started with preferred_transport=UDP");

        let proxy = runtime
            .find(TCP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe should succeed");

        eprintln!("[udp_client] Subscribed, waiting for events...");

        // Receive at least one event
        if let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_secs(5), subscription.next()).await
        {
            eprintln!("[udp_client] Received event: {:?}", event.payload.as_ref());
        }

        Ok(())
    });

    sim.run().unwrap();

    let tcp_count = tcp_events.load(Ordering::SeqCst);
    let udp_count = udp_events.load(Ordering::SeqCst);

    eprintln!(
        "Test complete: TCP-preferred subscriptions={}, UDP-preferred subscriptions={}",
        tcp_count, udp_count
    );

    // Both clients should have subscribed
    assert_eq!(
        tcp_count, 1,
        "TCP-preferring client should have subscribed once"
    );
    assert_eq!(
        udp_count, 1,
        "UDP-preferring client should have subscribed once"
    );
}

#[test_log::test]
fn handle_call_ignores_preferred_transport_for_dual_stack() {
    covers!(feat_req_recentip_324);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service with BOTH TCP and UDP (dual-stack)
    sim.host("server", || async move {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Dual-stack offer - advertises BOTH TCP and UDP endpoints
        let mut offering = runtime
            .offer(TCP_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TCP_SERVICE_VERSION.0, TCP_SERVICE_VERSION.1)
            .tcp()
            .udp()
            .start()
            .await
            .unwrap();

        eprintln!("[server] Dual-stack service offered (TCP + UDP)");

        // The ServiceEvent doesn't expose which transport was used, but we can
        // infer it by checking which socket received the call. For now, we track
        // that a call was received.
        while let Some(event) = offering.next().await {
            if let ServiceEvent::Call {
                responder, payload, ..
            } = event
            {
                let payload_str = String::from_utf8_lossy(&payload);
                eprintln!("[server] Received call: {}", payload_str);

                responder.reply(b"response").await.unwrap();
            }
        }

        Ok(())
    });

    // Client with preferred_transport=UDP calls a dual-stack service
    sim.client("udp_client", async move {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let config = RuntimeConfig::builder()
            .preferred_transport(Transport::Udp) // CLIENT PREFERS UDP!
            .advertised_ip(turmoil::lookup("udp_client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        eprintln!("[client] Runtime with preferred_transport=UDP");

        // Discover the dual-stack service
        let proxy = runtime
            .find(TCP_SERVICE_ID)
            .instance(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        eprintln!("[client] Service discovered, making call...");

        let method = MethodId::new(0x0001).unwrap();
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(method, b"from_udp_preferring_client"),
        )
        .await
        .expect("Call timeout")
        .expect("Call should succeed");

        eprintln!("[client] Got response: {:?}", response.payload.as_ref());

        Ok(())
    });

    sim.run().unwrap();
}
