//! TCP Pub/Sub Compliance Tests
//!
//! Tests for TCP-based event delivery in the Publish/Subscribe flow.
//! These complement the UDP tests in sd_pubsub.rs.
//!
//! Run with: cargo test --test tcp_pubsub
//!
//! # Status
//!
//! **NOTE**: TCP event delivery for pub/sub is not yet fully implemented.
//! The server-to-client event path over TCP requires the client to establish
//! a TCP connection to the server BEFORE subscribing (per feat_req_recentipsd_767),
//! and the server sends events back on that connection.
//!
//! These tests are marked `#[ignore]` until TCP pub/sub is implemented.
//! UDP pub/sub works correctly (see sd_pubsub.rs).
//!
//! # Test Categories
//!
//! 1. Basic TCP subscription and event delivery
//! 2. Multiple subscribers receiving events over TCP
//! 3. TCP connection handling (reconnection, cleanup)
//! 4. Large payload handling (TCP advantage over UDP)

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

const TCP_PUB_SUB_SERVICE_ID: u16 = 0x2345;
const TCP_PUB_SUB_SERVICE_VERSION: (u8, u32) = (1, 0);

// ============================================================================
// 1. BASIC TCP SUBSCRIPTION AND EVENT DELIVERY
// ============================================================================

/// Basic test: Server offers TCP, client subscribes and receives events.
///
/// [feat_req_recentipsd_767] Client opens TCP connection before SubscribeEventgroup.
/// [feat_req_recentipsd_786] SubscribeEventgroup can reference TCP endpoint option.
#[test_log::test]
fn tcp_basic_subscribe_and_receive_events() {
    covers!(feat_req_recentipsd_767, feat_req_recentipsd_786);

    let events_received = Arc::new(AtomicUsize::new(0));
    let events_clone = Arc::clone(&events_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer service via TCP only
        let offering = runtime
            .offer(TCP_PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TCP_PUB_SUB_SERVICE_VERSION.0, TCP_PUB_SUB_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        eprintln!("[server] TCP service offered");

        // Wait for subscription
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send events via TCP
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        for i in 0..5 {
            let payload = format!("tcp_event_{}", i);
            event_handle.notify(payload.as_bytes()).await.unwrap();
            eprintln!("[server] Sent: {}", payload);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TCP_PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        eprintln!("[client] Service discovered");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Subscribe timeout")
                .expect("Subscribe success");

        eprintln!("[client] Subscribed to eventgroup via TCP");

        // Receive events
        while let Some(event) = tokio::time::timeout(Duration::from_secs(3), subscription.next())
            .await
            .ok()
            .flatten()
        {
            let payload = String::from_utf8_lossy(&event.payload);
            eprintln!("[client] Received: {}", payload);
            events_clone.fetch_add(1, Ordering::SeqCst);
        }

        Ok(())
    });

    sim.run().unwrap();

    let count = events_received.load(Ordering::SeqCst);
    assert!(
        count >= 3,
        "Should have received at least 3 TCP events, got {}",
        count
    );
}

/// Test that multiple clients can subscribe to the same TCP eventgroup.
///
/// [feat_req_recentipsd_432] Server tracks subscription state per client.
#[test_log::test]
fn tcp_multiple_subscribers_receive_events() {
    covers!(feat_req_recentipsd_432);

    let client1_events = Arc::new(AtomicUsize::new(0));
    let client2_events = Arc::new(AtomicUsize::new(0));
    let c1_events = Arc::clone(&client1_events);
    let c2_events = Arc::clone(&client2_events);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let offering = runtime
            .offer(TCP_PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TCP_PUB_SUB_SERVICE_VERSION.0, TCP_PUB_SUB_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        eprintln!("[server] TCP service offered");

        // Wait for both subscriptions
        tokio::time::sleep(Duration::from_millis(800)).await;

        // Send events - both clients should receive
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        for i in 0..3 {
            let payload = format!("broadcast_{}", i);
            event_handle.notify(payload.as_bytes()).await.unwrap();
            eprintln!("[server] Broadcast: {}", payload);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("client1", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client1").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TCP_PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut sub = proxy.subscribe(eventgroup).await.unwrap();
        eprintln!("[client1] Subscribed");

        while let Some(event) = tokio::time::timeout(Duration::from_secs(3), sub.next())
            .await
            .ok()
            .flatten()
        {
            let payload = String::from_utf8_lossy(&event.payload);
            eprintln!("[client1] Received: {}", payload);
            c1_events.fetch_add(1, Ordering::SeqCst);
        }

        Ok(())
    });

    sim.client("client2", async move {
        tokio::time::sleep(Duration::from_millis(150)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client2").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TCP_PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut sub = proxy.subscribe(eventgroup).await.unwrap();
        eprintln!("[client2] Subscribed");

        while let Some(event) = tokio::time::timeout(Duration::from_secs(3), sub.next())
            .await
            .ok()
            .flatten()
        {
            let payload = String::from_utf8_lossy(&event.payload);
            eprintln!("[client2] Received: {}", payload);
            c2_events.fetch_add(1, Ordering::SeqCst);
        }

        Ok(())
    });

    sim.run().unwrap();

    let c1 = client1_events.load(Ordering::SeqCst);
    let c2 = client2_events.load(Ordering::SeqCst);

    eprintln!("Results: client1={}, client2={}", c1, c2);
    assert!(
        c1 >= 2,
        "Client1 should have received at least 2 events, got {}",
        c1
    );
    assert!(
        c2 >= 2,
        "Client2 should have received at least 2 events, got {}",
        c2
    );
}

// ============================================================================
// 2. TCP LARGE PAYLOAD HANDLING
// ============================================================================

/// Test that TCP can handle large payloads that would exceed UDP MTU.
///
/// UDP is typically limited to ~1400 bytes without SOME/IP-TP.
/// TCP should handle much larger payloads seamlessly.
#[test_log::test]
fn tcp_large_payload_events() {
    let events_received = Arc::new(AtomicUsize::new(0));
    let payload_sizes_received = Arc::new(std::sync::Mutex::new(Vec::new()));
    let events_clone = Arc::clone(&events_received);
    let sizes_clone = Arc::clone(&payload_sizes_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let offering = runtime
            .offer(TCP_PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TCP_PUB_SUB_SERVICE_VERSION.0, TCP_PUB_SUB_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        eprintln!("[server] TCP service offered");
        tokio::time::sleep(Duration::from_millis(500)).await;

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        // Send payloads of increasing sizes
        let sizes = [100, 1000, 5000, 10000, 30000];
        for size in sizes {
            let payload: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
            event_handle.notify(&payload).await.unwrap();
            eprintln!("[server] Sent {} byte payload", size);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TCP_PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut sub = proxy.subscribe(eventgroup).await.unwrap();
        eprintln!("[client] Subscribed");

        while let Some(event) = tokio::time::timeout(Duration::from_secs(5), sub.next())
            .await
            .ok()
            .flatten()
        {
            let size = event.payload.len();
            eprintln!("[client] Received {} byte payload", size);
            sizes_clone.lock().unwrap().push(size);
            events_clone.fetch_add(1, Ordering::SeqCst);
        }

        Ok(())
    });

    sim.run().unwrap();

    let count = events_received.load(Ordering::SeqCst);
    let sizes = payload_sizes_received.lock().unwrap();

    eprintln!("Received {} events with sizes: {:?}", count, *sizes);

    assert!(
        count >= 3,
        "Should have received at least 3 large payload events, got {}",
        count
    );

    // Verify we received at least one large payload (> 1400 bytes, which exceeds UDP MTU)
    let has_large = sizes.iter().any(|&s| s > 1400);
    assert!(
        has_large,
        "Should have received at least one payload > 1400 bytes (UDP MTU limit)"
    );
}

// ============================================================================
// 3. TCP SUBSCRIPTION TO DIFFERENT EVENTGROUPS
// ============================================================================

/// Test that clients can subscribe to different eventgroups via TCP.
///
/// [feat_req_recentipsd_432] Server tracks subscription state for each eventgroup.
#[test_log::test]
fn tcp_different_eventgroups() {
    let eg1_events = Arc::new(AtomicUsize::new(0));
    let eg2_events = Arc::new(AtomicUsize::new(0));
    let eg1_clone = Arc::clone(&eg1_events);
    let eg2_clone = Arc::clone(&eg2_events);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let offering = runtime
            .offer(TCP_PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TCP_PUB_SUB_SERVICE_VERSION.0, TCP_PUB_SUB_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        eprintln!("[server] TCP service offered");
        tokio::time::sleep(Duration::from_millis(600)).await;

        let eg1 = EventgroupId::new(0x0001).unwrap();
        let eg2 = EventgroupId::new(0x0002).unwrap();
        let event1 = EventId::new(0x8001).unwrap();
        let event2 = EventId::new(0x8002).unwrap();
        let event_handle1 = offering
            .event(event1)
            .eventgroup(eg1)
            .create()
            .await
            .unwrap();
        let event_handle2 = offering
            .event(event2)
            .eventgroup(eg2)
            .create()
            .await
            .unwrap();

        // Send events to eventgroup 1
        for i in 0..3 {
            event_handle1
                .notify(format!("eg1_{}", i).as_bytes())
                .await
                .unwrap();
            eprintln!("[server] Sent to EG1");
        }

        tokio::time::sleep(Duration::from_millis(200)).await;

        // Send events to eventgroup 2
        for i in 0..3 {
            event_handle2
                .notify(format!("eg2_{}", i).as_bytes())
                .await
                .unwrap();
            eprintln!("[server] Sent to EG2");
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    // Client 1 subscribes to eventgroup 1 only
    sim.client("client1", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client1").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TCP_PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eg1 = EventgroupId::new(0x0001).unwrap();
        let mut sub = proxy.subscribe(eg1).await.unwrap();
        eprintln!("[client1] Subscribed to EG1");

        while let Some(event) = tokio::time::timeout(Duration::from_secs(3), sub.next())
            .await
            .ok()
            .flatten()
        {
            let payload = String::from_utf8_lossy(&event.payload);
            eprintln!("[client1] EG1 event: {}", payload);
            eg1_clone.fetch_add(1, Ordering::SeqCst);
        }

        Ok(())
    });

    // Client 2 subscribes to eventgroup 2 only
    sim.client("client2", async move {
        tokio::time::sleep(Duration::from_millis(150)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client2").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TCP_PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eg2 = EventgroupId::new(0x0002).unwrap();
        let mut sub = proxy.subscribe(eg2).await.unwrap();
        eprintln!("[client2] Subscribed to EG2");

        while let Some(event) = tokio::time::timeout(Duration::from_secs(3), sub.next())
            .await
            .ok()
            .flatten()
        {
            let payload = String::from_utf8_lossy(&event.payload);
            eprintln!("[client2] EG2 event: {}", payload);
            eg2_clone.fetch_add(1, Ordering::SeqCst);
        }

        Ok(())
    });

    sim.run().unwrap();

    let eg1_count = eg1_events.load(Ordering::SeqCst);
    let eg2_count = eg2_events.load(Ordering::SeqCst);

    eprintln!("Results: EG1={}, EG2={}", eg1_count, eg2_count);
    assert!(
        eg1_count >= 2,
        "Client1 should have received EG1 events, got {}",
        eg1_count
    );
    assert!(
        eg2_count >= 2,
        "Client2 should have received EG2 events, got {}",
        eg2_count
    );
}

// ============================================================================
// 4. TCP AND UDP MIXED IN SAME SERVICE
// ============================================================================

/// Test service offering both TCP and UDP, client selects based on preference.
///
/// [feat_req_recentipsd_786] SubscribeEventgroup can reference UDP and/or TCP endpoint.
#[test_log::test]
fn dual_stack_service_client_prefers_tcp() {
    covers!(feat_req_recentipsd_786);

    let events_received = Arc::new(AtomicUsize::new(0));
    let events_clone = Arc::clone(&events_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer service via both TCP and UDP
        let offering = runtime
            .offer(TCP_PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TCP_PUB_SUB_SERVICE_VERSION.0, TCP_PUB_SUB_SERVICE_VERSION.1)
            .tcp()
            .udp()
            .start()
            .await
            .unwrap();

        eprintln!("[server] Dual-stack service offered (TCP + UDP)");
        tokio::time::sleep(Duration::from_millis(500)).await;

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        for i in 0..5 {
            let payload = format!("event_{}", i);
            event_handle.notify(payload.as_bytes()).await.unwrap();
            eprintln!("[server] Sent: {}", payload);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Client prefers TCP
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TCP_PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut sub = proxy.subscribe(eventgroup).await.unwrap();
        eprintln!("[client] Subscribed (should use TCP)");

        while let Some(event) = tokio::time::timeout(Duration::from_secs(3), sub.next())
            .await
            .ok()
            .flatten()
        {
            let payload = String::from_utf8_lossy(&event.payload);
            eprintln!("[client] Received via TCP: {}", payload);
            events_clone.fetch_add(1, Ordering::SeqCst);
        }

        Ok(())
    });

    sim.run().unwrap();

    let count = events_received.load(Ordering::SeqCst);
    assert!(
        count >= 3,
        "Should have received at least 3 events, got {}",
        count
    );
}

/// Test service offering both TCP and UDP, client prefers UDP.
#[test_log::test]
fn dual_stack_service_client_prefers_udp() {
    let events_received = Arc::new(AtomicUsize::new(0));
    let events_clone = Arc::clone(&events_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer service via both TCP and UDP
        let offering = runtime
            .offer(TCP_PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TCP_PUB_SUB_SERVICE_VERSION.0, TCP_PUB_SUB_SERVICE_VERSION.1)
            .tcp()
            .udp()
            .start()
            .await
            .unwrap();

        eprintln!("[server] Dual-stack service offered (TCP + UDP)");
        tokio::time::sleep(Duration::from_millis(500)).await;

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        for i in 0..5 {
            let payload = format!("event_{}", i);
            event_handle.notify(payload.as_bytes()).await.unwrap();
            eprintln!("[server] Sent: {}", payload);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Client prefers UDP (default)
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Udp)
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TCP_PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut sub = proxy.subscribe(eventgroup).await.unwrap();
        eprintln!("[client] Subscribed (should use UDP)");

        while let Some(event) = tokio::time::timeout(Duration::from_secs(3), sub.next())
            .await
            .ok()
            .flatten()
        {
            let payload = String::from_utf8_lossy(&event.payload);
            eprintln!("[client] Received via UDP: {}", payload);
            events_clone.fetch_add(1, Ordering::SeqCst);
        }

        Ok(())
    });

    sim.run().unwrap();

    let count = events_received.load(Ordering::SeqCst);
    assert!(
        count >= 3,
        "Should have received at least 3 events, got {}",
        count
    );
}

// ============================================================================
// 5. TCP-ONLY SERVER BEHAVIOR
// ============================================================================

/// Test that a client with UDP preference still works when server is TCP-only.
///
/// The client should adapt to the available transport.
#[test_log::test]
fn tcp_only_server_udp_preferring_client() {
    let events_received = Arc::new(AtomicUsize::new(0));
    let events_clone = Arc::clone(&events_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer service via TCP only
        let offering = runtime
            .offer(TCP_PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TCP_PUB_SUB_SERVICE_VERSION.0, TCP_PUB_SUB_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        eprintln!("[server] TCP-only service offered");
        tokio::time::sleep(Duration::from_millis(500)).await;

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        let event_handle = offering
            .event(event_id)
            .eventgroup(eventgroup)
            .create()
            .await
            .unwrap();

        for i in 0..3 {
            event_handle
                .notify(format!("tcp_{}", i).as_bytes())
                .await
                .unwrap();
            eprintln!("[server] Sent via TCP");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Client prefers UDP, but server is TCP-only
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .preferred_transport(Transport::Udp) // Preference, not requirement
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find(TCP_PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        // Should succeed even though client prefers UDP - it adapts to TCP
        let mut sub = proxy.subscribe(eventgroup).await.unwrap();
        eprintln!("[client] Subscribed (adapted to TCP)");

        while let Some(event) = tokio::time::timeout(Duration::from_secs(3), sub.next())
            .await
            .ok()
            .flatten()
        {
            let payload = String::from_utf8_lossy(&event.payload);
            eprintln!("[client] Received: {}", payload);
            events_clone.fetch_add(1, Ordering::SeqCst);
        }

        Ok(())
    });

    sim.run().unwrap();

    let count = events_received.load(Ordering::SeqCst);
    assert!(
        count >= 2,
        "Should have received events despite transport preference mismatch, got {}",
        count
    );
}
