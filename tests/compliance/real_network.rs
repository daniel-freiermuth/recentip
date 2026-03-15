//! Real Network Integration Tests
//!
//! These tests use the actual tokio network stack instead of turmoil simulation.
//! They verify that the runtime works correctly with real sockets.
//!
//! **NOTE**: These tests require that all participants listen on the same multicast port (30490).
//! On Linux and recent macOS, this is supported via `SO_REUSEPORT`.
//!
//! To run these tests, you need either:
//! - A platform with `SO_REUSEPORT` support (Linux, recent macOS)
//! - Separate machines or containers with their own network stacks
//!
//! The turmoil-based tests provide comprehensive
//! network testing with simulated separate hosts.

use recentip::{config, EventId, EventgroupId, InstanceId, MethodId, ServiceEvent, Transport};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;
use tokio::sync::mpsc;

use crate::helpers::configure_tracing;

// ============================================================================
// Test Service Definition
// ============================================================================

const ECHO_SERVICE_ID: u16 = 0x1234;
const ECHO_SERVICE_VERSION: (u8, u32) = (1, 0);

// ============================================================================
// UDP Double-Bind Pre-Study
// ============================================================================

/// Pre-study: What does the real Linux kernel do when two sockets bind the same
/// UDP port **without** any reuse socket options?
///
/// Expected: second `bind()` fails with `EADDRINUSE`.
#[tokio::test]
async fn udp_double_bind_without_reuse_options() {
    let addr: SocketAddr = "127.0.0.1:19876".parse().unwrap();

    let s1 = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
    s1.set_nonblocking(true).unwrap();
    s1.bind(&addr.into()).unwrap();
    let _s1 = tokio::net::UdpSocket::from_std(s1.into()).unwrap();

    let s2 = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
    s2.set_nonblocking(true).unwrap();
    let err = s2
        .bind(&addr.into())
        .expect_err("Expected EADDRINUSE for double bind without SO_REUSEPORT");
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
    tracing::info!("Second bind (no reuse options) failed as expected: {err}");
}

/// Pre-study: With `SO_REUSEPORT`, two sockets can share a port.  How are
/// incoming unicast datagrams distributed between them?
///
/// Strategy: 20 independent sender sockets (each with a distinct ephemeral
/// source port) each send one datagram.  Because the Linux kernel hashes the
/// full 4-tuple (src-IP, src-port, dst-IP, dst-port) for `SO_REUSEPORT`
/// load-balancing, distinct source ports produce varied hash inputs and are
/// expected to spread datagrams across both receivers.
///
/// Possible outcomes logged to tracing output:
///
/// - **Distribution**: kernel splits packets across both sockets (typical on Linux).
/// - **First-wins**: all packets go to socket1.
/// - **Last-wins**: all packets go to socket2.
///
/// The hard assertion is only that no packets are lost or duplicated.
#[tokio::test]
async fn udp_double_bind_with_reuseport_distribution() {
    const SENDERS: usize = 20;
    const PORT: u16 = 19877;

    let addr: SocketAddr = format!("127.0.0.1:{PORT}").parse().unwrap();

    let make_recv = |addr: SocketAddr| -> tokio::net::UdpSocket {
        let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
        s.set_reuse_port(true).unwrap();
        s.set_reuse_address(true).unwrap();
        s.set_nonblocking(true).unwrap();
        s.bind(&addr.into()).unwrap();
        tokio::net::UdpSocket::from_std(s.into()).unwrap()
    };

    let recv1 = make_recv(addr);
    let recv2 = make_recv(addr);

    // Each sender binds to an ephemeral port, giving the kernel's hash varied
    // src-port inputs and thus a chance to steer to different sockets.
    for i in 0..SENDERS {
        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender
            .send_to(format!("msg {i}").as_bytes(), addr)
            .await
            .unwrap();
    }

    // Brief yield so the kernel can deliver all queued datagrams.
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Drain each socket's independent kernel receive queue.
    let drain = |socket: &tokio::net::UdpSocket| -> usize {
        let mut buf = [0u8; 64];
        let mut count = 0usize;
        loop {
            match socket.try_recv_from(&mut buf) {
                Ok((n, from)) => {
                    tracing::info!("received {n}B from {from}");
                    count += 1;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => panic!("recv error: {e}"),
            }
        }
        count
    };

    let count1 = drain(&recv1);
    let count2 = drain(&recv2);
    let total = count1 + count2;

    tracing::info!("=== SO_REUSEPORT unicast distribution ===");
    tracing::info!("Socket1: {count1}  Socket2: {count2}  Total: {total}");

    if count1 > 0 && count2 > 0 {
        tracing::info!("OUTCOME: packets distributed across both sockets");
    } else if count1 == SENDERS {
        tracing::info!("OUTCOME: all packets went to socket1 (first-wins)");
    } else if count2 == SENDERS {
        tracing::info!("OUTCOME: all packets went to socket2 (last-wins)");
    }

    assert_eq!(total, SENDERS, "no packets should be lost or duplicated");
}

// ============================================================================
// UDP Tests
// ============================================================================

/// Test basic UDP request/response on real network
#[tokio::test]
async fn udp_request_response_real_network() {
    // Create server runtime and offer service
    let server_runtime = recentip::configure()
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .sd_unicast("127.0.0.1".parse().unwrap())
        .start()
        .await
        .expect("Server runtime");

    let mut offering = server_runtime
        .offer(ECHO_SERVICE_ID, InstanceId::Id(0x0001))
        .version(ECHO_SERVICE_VERSION.0, ECHO_SERVICE_VERSION.1)
        .udp()
        .start()
        .await
        .expect("Offer service");

    // Create client runtime
    let client_runtime = recentip::configure()
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .sd_unicast("127.0.0.1".parse().unwrap())
        .start()
        .await
        .expect("Client runtime");

    // Spawn server handler task
    let server_task = tokio::spawn(async move {
        if let Some(ServiceEvent::Call {
            responder, payload, ..
        }) = offering.next().await
        {
            let mut response = b"ECHO:".to_vec();
            response.extend_from_slice(&payload);
            responder.reply(&response).expect("Reply");
        }
    });

    // Small delay for SD messages to propagate
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Client discovers and calls service
    let proxy = client_runtime
        .find(ECHO_SERVICE_ID)
        .instance(InstanceId::Id(0x0001));
    let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
        .await
        .expect("Discovery timeout")
        .expect("Service available");

    let method_id = MethodId::new(0x0001).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method_id, b"hello"))
        .await
        .expect("Call timeout")
        .expect("Call success");

    assert_eq!(response.payload.as_ref(), b"ECHO:hello");

    server_task.await.expect("Server task");

    // Shutdown runtimes and wait for cleanup
    client_runtime.shutdown().await;
    server_runtime.shutdown().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// Test UDP service discovery on real network
#[tokio::test]
async fn udp_service_discovery_real_network() {
    let (ready_tx, mut ready_rx) = mpsc::channel::<()>(1);
    let (done_tx, mut done_rx) = mpsc::channel::<()>(1);

    let server_handle = tokio::spawn(async move {
        let runtime = recentip::configure()
            .sd_multicast_group("239.255.255.250".parse().unwrap())
            .sd_unicast("127.0.0.1".parse().unwrap())
            .start()
            .await
            .expect("Server runtime");

        let _offering = runtime
            .offer(ECHO_SERVICE_ID, InstanceId::Id(0x0002))
            .version(ECHO_SERVICE_VERSION.0, ECHO_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .expect("Offer service");

        ready_tx.send(()).await.ok();
        done_rx.recv().await;
    });

    ready_rx.recv().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let runtime = recentip::configure()
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .sd_unicast("127.0.0.1".parse().unwrap())
        .start()
        .await
        .expect("Client runtime");

    let proxy = runtime
        .find(ECHO_SERVICE_ID)
        .instance(InstanceId::Id(0x0002));

    let result = tokio::time::timeout(Duration::from_secs(5), proxy).await;
    assert!(result.is_ok(), "Should discover service");
    assert!(result.unwrap().is_ok(), "Service should be available");

    done_tx.send(()).await.ok();
    server_handle.await.expect("Server task");
}

// ============================================================================
// TCP Tests
// ============================================================================

/// Test basic TCP request/response on real network
#[tokio::test]
async fn tcp_request_response_real_network() {
    let (ready_tx, mut ready_rx) = mpsc::channel::<()>(1);
    let (done_tx, mut done_rx) = mpsc::channel::<()>(1);

    let server_handle = tokio::spawn(async move {
        let runtime = recentip::configure()
            .sd_multicast_group("239.255.255.250".parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .sd_unicast("127.0.0.1".parse().unwrap())
            .start()
            .await
            .expect("Server runtime");

        let mut offering = runtime
            .offer(ECHO_SERVICE_ID, InstanceId::Id(0x0001))
            .version(ECHO_SERVICE_VERSION.0, ECHO_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .expect("Offer service");

        ready_tx.send(()).await.ok();

        if let Some(ServiceEvent::Call {
            responder, payload, ..
        }) = offering.next().await
        {
            let mut response = b"TCP:".to_vec();
            response.extend_from_slice(&payload);
            responder.reply(&response).expect("Reply");
        }

        done_rx.recv().await;
    });

    ready_rx.recv().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let runtime = recentip::configure()
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .sd_unicast("127.0.0.1".parse().unwrap())
        .preferred_transport(Transport::Tcp)
        .start()
        .await
        .expect("Client runtime");

    let proxy = runtime
        .find(ECHO_SERVICE_ID)
        .instance(InstanceId::Id(0x0001));
    let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
        .await
        .expect("Discovery timeout")
        .expect("Service available");

    let method_id = MethodId::new(0x0001).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method_id, b"world"))
        .await
        .expect("Call timeout")
        .expect("Call success");

    assert_eq!(response.payload.as_ref(), b"TCP:world");

    done_tx.send(()).await.ok();
    server_handle.await.expect("Server task");

    // Shutdown runtime and wait for cleanup
    runtime.shutdown().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// Test TCP with Magic Cookies on real network
#[tokio::test]
async fn tcp_magic_cookies_real_network() {
    let (ready_tx, mut ready_rx) = mpsc::channel::<()>(1);
    let (done_tx, mut done_rx) = mpsc::channel::<()>(1);

    let server_handle = tokio::spawn(async move {
        let runtime = recentip::configure()
            .sd_multicast_group("239.255.255.250".parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .magic_cookies(true)
            .sd_unicast("127.0.0.1".parse().unwrap())
            .start()
            .await
            .expect("Server runtime");

        let mut offering = runtime
            .offer(ECHO_SERVICE_ID, InstanceId::Id(0x0001))
            .version(ECHO_SERVICE_VERSION.0, ECHO_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .expect("Offer service");

        ready_tx.send(()).await.ok();

        if let Some(ServiceEvent::Call {
            responder, payload, ..
        }) = offering.next().await
        {
            let mut response = b"MAGIC:".to_vec();
            response.extend_from_slice(&payload);
            responder.reply(&response).expect("Reply");
        }

        done_rx.recv().await;
    });

    ready_rx.recv().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let runtime = recentip::configure()
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .sd_unicast("127.0.0.1".parse().unwrap())
        .preferred_transport(Transport::Tcp)
        .magic_cookies(true)
        .start()
        .await
        .expect("Client runtime");

    let proxy = runtime
        .find(ECHO_SERVICE_ID)
        .instance(InstanceId::Id(0x0001));
    let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
        .await
        .expect("Discovery timeout")
        .expect("Service available");

    let method_id = MethodId::new(0x0001).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method_id, b"cookie"))
        .await
        .expect("Call timeout")
        .expect("Call success");

    assert_eq!(response.payload.as_ref(), b"MAGIC:cookie");

    done_tx.send(()).await.ok();
    server_handle.await.expect("Server task");
}

/// Test multiple TCP requests on same connection
#[tokio::test]
async fn tcp_multiple_requests_real_network() {
    let (ready_tx, mut ready_rx) = mpsc::channel::<()>(1);
    let (done_tx, mut done_rx) = mpsc::channel::<()>(1);

    let server_handle = tokio::spawn(async move {
        let runtime = recentip::configure()
            .sd_multicast_group("239.255.255.250".parse().unwrap())
            .preferred_transport(Transport::Tcp)
            .sd_unicast("127.0.0.1".parse().unwrap())
            .start()
            .await
            .expect("Server runtime");

        let mut offering = runtime
            .offer(ECHO_SERVICE_ID, InstanceId::Id(0x0001))
            .version(ECHO_SERVICE_VERSION.0, ECHO_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .expect("Offer service");

        ready_tx.send(()).await.ok();

        for _ in 0..5 {
            if let Some(ServiceEvent::Call {
                responder, payload, ..
            }) = offering.next().await
            {
                let mut response = b"MULTI:".to_vec();
                response.extend_from_slice(&payload);
                responder.reply(&response).expect("Reply");
            }
        }

        done_rx.recv().await;
    });

    ready_rx.recv().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let runtime = recentip::configure()
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .sd_unicast("127.0.0.1".parse().unwrap())
        .preferred_transport(Transport::Tcp)
        .start()
        .await
        .expect("Client runtime");

    let proxy = runtime
        .find(ECHO_SERVICE_ID)
        .instance(InstanceId::Id(0x0001));
    let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
        .await
        .expect("Discovery timeout")
        .expect("Service available");

    let method_id = MethodId::new(0x0001).unwrap();

    for i in 0..5 {
        let request = format!("req{}", i);
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(method_id, request.as_bytes()),
        )
        .await
        .expect("Call timeout")
        .expect("Call success");

        let expected = format!("MULTI:req{}", i);
        assert_eq!(response.payload.as_ref(), expected.as_bytes());
    }

    done_tx.send(()).await.ok();
    server_handle.await.expect("Server task");
}

// ============================================================================
// Event/Subscription Tests
// ============================================================================

/// Test event subscription on real UDP network with dual-socket SD.
///
/// This test verifies unicast event delivery on a real network with two runtimes
/// sharing the loopback interface.  Each runtime uses a distinct loopback IP as
/// its `advertised_ip`:
///
/// - Server: `127.0.0.2` (unicast SD socket bound to `127.0.0.2:0`)
/// - Client: `127.0.0.1` (unicast SD socket bound to `127.0.0.1:0`)
///
/// Both multicast sockets bind to `0.0.0.0:30490` (SO_REUSEPORT) and receive
/// multicast SD (Offer, FindService).  Unicast SD responses such as
/// `SubscribeEventgroupAck` are sent from and received on the per-runtime
/// unicast socket at a unique ephemeral port, so SO_REUSEPORT load-balancing
/// on port 30490 can never misdirect them.
#[tokio::test]
async fn udp_events_real_network() {
    configure_tracing();
    let (ready_tx, mut ready_rx) = mpsc::channel::<()>(1);
    let (subscribed_tx, mut subscribed_rx) = mpsc::channel::<()>(1);
    let (done_tx, mut done_rx) = mpsc::channel::<()>(1);

    let server_handle = tokio::spawn(async move {
        let runtime = recentip::configure()
            .sd_multicast_group("239.255.255.250".parse().unwrap())
            .sd_unicast("127.0.0.2".parse().unwrap())
            .start()
            .await
            .expect("Server runtime");

        let mut offering = runtime
            .offer(ECHO_SERVICE_ID, InstanceId::Id(0x0001))
            .version(ECHO_SERVICE_VERSION.0, ECHO_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .expect("Offer service");

        ready_tx.send(()).await.ok();

        loop {
            match offering.next().await {
                Some(ServiceEvent::Subscribe { eventgroup, .. }) => {
                    let event_id = EventId::new(0x8001).unwrap();
                    let event_handle = offering
                        .event(event_id)
                        .eventgroup(eventgroup)
                        .create()
                        .await
                        .unwrap();
                    for i in 0..3 {
                        let event_data = format!("event{}", i);
                        event_handle
                            .notify(event_data.as_bytes())
                            .await
                            .expect("Notify");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    subscribed_tx.send(()).await.ok();
                    break;
                }
                Some(_) => continue,
                None => break,
            }
        }

        // Wait for client to finish collecting events BEFORE shutting down.
        // If we shut down first, the StopOffer multicast arrives at the client
        // and removes the subscription state before events are delivered.
        done_rx.recv().await;
        runtime.shutdown().await;
    });

    ready_rx.recv().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let runtime = recentip::configure()
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .sd_unicast("127.0.0.1".parse().unwrap())
        .start()
        .await
        .expect("Client runtime");

    let proxy = runtime
        .find(ECHO_SERVICE_ID)
        .instance(InstanceId::Id(0x0001));
    let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
        .await
        .expect("Discovery timeout")
        .expect("Service available");

    let eventgroup_id = EventgroupId::new(0x0001).unwrap();
    let mut subscription =
        tokio::time::timeout(Duration::from_secs(1), proxy.subscribe(eventgroup_id))
            .await
            .expect("Sub in time")
            .expect("Subscribe success");

    subscribed_rx.recv().await;

    let mut received = Vec::new();
    for _ in 0..3 {
        if let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_secs(2), subscription.next()).await
        {
            received.push(event.payload.to_vec());
        }
    }
    runtime.shutdown().await;

    assert_eq!(received.len(), 3);
    assert_eq!(received[0], b"event0");
    assert_eq!(received[1], b"event1");
    assert_eq!(received[2], b"event2");

    done_tx.send(()).await.ok();
    server_handle.await.expect("Server task");
}

// ============================================================================
// SO_REUSEPORT Tests
// ============================================================================

/// Test two runtimes binding to the same SD port (SO_REUSEPORT)
#[tokio::test]
async fn udp_two_runtimes_same_sd_port() {
    let sd_port = 30490;

    // Both runtimes bind to INADDR_ANY on the SD multicast port
    // This tests SO_REUSEPORT functionality
    let server_runtime = recentip::configure()
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .sd_unicast("127.0.0.1".parse().unwrap())
        .start()
        .await
        .expect("Server runtime");

    let mut offering = server_runtime
        .offer(ECHO_SERVICE_ID, InstanceId::Id(0x0003))
        .version(ECHO_SERVICE_VERSION.0, ECHO_SERVICE_VERSION.1)
        .udp()
        .start()
        .await
        .expect("Offer service");

    let client_runtime = recentip::configure()
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .sd_unicast("127.0.0.1".parse().unwrap())
        .start()
        .await
        .expect("Client runtime");

    let server_task = tokio::spawn(async move {
        if let Some(ServiceEvent::Call {
            responder, payload, ..
        }) = offering.next().await
        {
            let mut response = b"REUSEPORT:".to_vec();
            response.extend_from_slice(&payload);
            responder.reply(&response).expect("Reply");
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let proxy = client_runtime
        .find(ECHO_SERVICE_ID)
        .instance(InstanceId::Id(0x0003));
    let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
        .await
        .expect("Discovery timeout")
        .expect("Service available");

    let method_id = MethodId::new(0x0001).unwrap();
    let response =
        tokio::time::timeout(Duration::from_secs(5), proxy.call(method_id, b"reuseport"))
            .await
            .expect("Call timeout")
            .expect("Call success");

    assert_eq!(response.payload.as_ref(), b"REUSEPORT:reuseport");
    server_task.await.expect("Server task");
}

// ============================================================================
// Non-Interference Tests
// ============================================================================

/// Non-interference test: an observer socket bound to `0.0.0.0:SD_PORT` receives
/// **all** unicast traffic destined for `127.0.0.3:SD_PORT` even when two SOME/IP
/// runtimes are running on the same host.
///
/// ## Setup
///
/// - **Runtime A** (server): `advertised_ip = 127.0.0.2`, offers an eventgroup.
/// - **Runtime B** (client): `advertised_ip = 127.0.0.1`, subscribes to events.
/// - **Observer socket**: bound to `0.0.0.0:30490` with `SO_REUSEPORT`.
///
/// ## Why this test passes
///
/// Each runtime's mc_socket is bound to the SD **multicast group address**
/// (`239.255.255.250:30490`) rather than `0.0.0.0:30490`.  A socket bound to a
/// multicast group address only receives datagrams whose destination IP matches
/// that group — unicast packets to `127.0.0.3:30490` never reach it.  Crucially,
/// multicast-address sockets are also **not** part of the `SO_REUSEPORT`
/// load-balancing pool for wildcard sockets, so the runtime mc_sockets do not
/// compete with the observer for packets.
///
/// The runtime uc_sockets (`127.0.0.1:30490` / `127.0.0.2:30490`) are
/// specific-IP binds and only match packets destined for their exact IP, so they
/// also do not intercept traffic for `127.0.0.3`.
///
/// Result: the observer is the sole recipient of all unicast to `127.0.0.3:30490`.
#[tokio::test]
async fn sd_port_non_interference_wildcard_observer() {
    const SD_PORT: u16 = 30490;
    const OBSERVER_IP: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 3);
    const SENDERS: usize = 30;

    // ── Start two runtimes on loopback IPs so their sockets are open ──────
    let server_runtime = recentip::configure()
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .sd_unicast("127.0.0.2".parse().unwrap())
        .start()
        .await
        .expect("Server runtime");

    let client_runtime = recentip::configure()
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .sd_unicast("127.0.0.1".parse().unwrap())
        .start()
        .await
        .expect("Client runtime");

    // ── Observer: bind 0.0.0.0:SD_PORT with SO_REUSEPORT (same as runtimes) ─
    let observer = {
        let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
        s.set_reuse_port(true).unwrap();
        s.set_reuse_address(true).unwrap();
        s.set_nonblocking(true).unwrap();
        let bind_addr: SocketAddr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), SD_PORT);
        s.bind(&bind_addr.into()).unwrap();
        tokio::net::UdpSocket::from_std(s.into()).unwrap()
    };

    // ── Send SENDERS datagrams from distinct ephemeral ports to OBSERVER_IP ─
    // Using distinct source ports forces varied 4-tuple hashes, which distributes
    // packets across all SO_REUSEPORT sockets (instead of all going to one).
    let dst: SocketAddr = SocketAddr::new(OBSERVER_IP.into(), SD_PORT);
    for i in 0..SENDERS {
        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender
            .send_to(format!("pkt{i}").as_bytes(), dst)
            .await
            .unwrap();
    }

    // Brief pause for the kernel to deliver all queued datagrams.
    tokio::time::sleep(Duration::from_millis(30)).await;

    // ── Drain the observer socket ──────────────────────────────────────────
    let mut buf = [0u8; 64];
    let mut observer_count = 0usize;
    loop {
        match observer.try_recv_from(&mut buf) {
            Ok((_, from)) => {
                tracing::info!("observer received packet from {from}");
                observer_count += 1;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) => panic!("observer recv error: {e}"),
        }
    }

    tracing::info!(
        "Observer received {observer_count}/{SENDERS} packets \
         (expected ~{} with 3-way SO_REUSEPORT split)",
        SENDERS / 3
    );

    server_runtime.shutdown().await;
    client_runtime.shutdown().await;

    assert_eq!(
        observer_count,
        SENDERS,
        "Non-interference broken: observer only received {observer_count}/{SENDERS} packets. \
         The SOME/IP runtimes consumed {consumed} packets that were destined for \
         127.0.0.3:{SD_PORT}. This means the runtime mc_socket is back in the \
         wildcard SO_REUSEPORT pool — check that it binds to the multicast group \
         address when advertised_ip is set.",
        consumed = SENDERS - observer_count,
    );
}

// ============================================================================
// TCP fixed client-port via TransportPolicy — real_network
// ============================================================================

/// A client subscribes to a TCP service with an explicit local TCP port set via
/// [`TransportPolicy`].  The tokio implementation uses `TcpSocket::bind` before
/// connecting, so the server-observed Subscribe endpoint should carry exactly
/// the requested port.
///
/// This test requires real sockets because turmoil's simulated
/// [`TcpStream::connect_from`](recentip::net::TcpStream::connect_from) ignores
/// the bind address and lets turmoil assign the source port.
#[tokio::test]
async fn tcp_fixed_client_port_real_network() {
    use recentip::config::{TransportPolicy, TransportPreference};

    const SVC_ID: u16 = 0x1236;
    const SVC_VERSION: (u8, u32) = (1, 0);
    // Port 19879 is below the Linux default ephemeral range (32768–60999) so
    // the OS will never assign it to another socket as an ephemeral port.
    const CLIENT_TCP_PORT: u16 = 19879;

    let (ready_tx, mut ready_rx) = mpsc::channel::<()>(1);
    let (port_tx, mut port_rx) = mpsc::channel::<u16>(1);
    let (done_tx, mut done_rx) = mpsc::channel::<()>(1);

    let server_handle = tokio::spawn(async move {
        let runtime = recentip::configure()
            .sd_multicast_group("239.255.255.250".parse().unwrap())
            .sd_unicast("127.0.0.2".parse().unwrap())
            .start()
            .await
            .expect("Server runtime");

        let mut offering = runtime
            .offer(SVC_ID, InstanceId::Id(0x0001))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .tcp()
            .start()
            .await
            .expect("Offer service");

        ready_tx.send(()).await.ok();

        if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
            port_tx.send(client.address.port()).await.ok();
        }

        done_rx.recv().await;
    });

    ready_rx.recv().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let runtime = recentip::configure()
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .sd_unicast("127.0.0.1".parse().unwrap())
        .start()
        .await
        .expect("Client runtime");

    let policy = TransportPolicy::new(vec![TransportPreference::tcp().with_port(CLIENT_TCP_PORT)]);

    let proxy = tokio::time::timeout(
        Duration::from_secs(5),
        runtime.find(SVC_ID).instance(InstanceId::Id(0x0001)),
    )
    .await
    .expect("Discovery timeout")
    .expect("Service available")
    .with_transport_policy(policy);

    let _sub = tokio::time::timeout(
        Duration::from_secs(5),
        proxy.subscribe(EventgroupId::new(1).unwrap()),
    )
    .await
    .expect("Subscribe timeout")
    .expect("Subscribe success");

    let observed_port = tokio::time::timeout(Duration::from_secs(5), port_rx.recv())
        .await
        .expect("port observation timeout")
        .expect("server must observe Subscribe");

    assert_eq!(
        observed_port, CLIENT_TCP_PORT,
        "server must see fixed TCP client port {CLIENT_TCP_PORT}, got {observed_port}"
    );

    done_tx.send(()).await.ok();
    server_handle.await.expect("Server task");

    runtime.shutdown().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
}

// ============================================================================
// TCP two-service same-server-port + fixed client port — real_network
// ============================================================================

/// Server (127.0.0.2) offers **two different service IDs** on the same TCP port.
/// Client (127.0.0.1) subscribes to both using a `TransportPolicy` that pins
/// the local TCP source port to an explicit value.
///
/// Because both services share the same server TCP endpoint the runtime's
/// connection pool returns the same pooled connection for both subscriptions.
/// As a result both Subscribe SD messages carry the **same** client TCP port —
/// the one specified in the `TransportPolicy`.
///
/// Assertions:
/// - both subscriptions succeed,
/// - server-observed Subscribe A port == `CLIENT_TCP_PORT`,
/// - server-observed Subscribe B port == `CLIENT_TCP_PORT` (shared connection).
#[tokio::test]
async fn tcp_two_services_same_server_port_fixed_client_port_real_network() {
    use recentip::config::{TransportPolicy, TransportPreference};

    const SVC_A: u16 = 0x1237;
    const SVC_B: u16 = 0x1238;
    const SVC_VERSION: (u8, u32) = (1, 0);
    const SERVER_TCP_PORT: u16 = 19881;
    // Below the Linux default ephemeral range (32768–60999).
    const CLIENT_TCP_PORT: u16 = 19882;

    let (ready_tx, mut ready_rx) = mpsc::channel::<()>(1);
    let (port_a_tx, mut port_a_rx) = mpsc::channel::<u16>(1);
    let (port_b_tx, mut port_b_rx) = mpsc::channel::<u16>(1);
    let (done_tx, mut done_rx) = mpsc::channel::<()>(1);

    let server_handle = tokio::spawn(async move {
        let runtime = recentip::configure()
            .sd_multicast_group("239.255.255.250".parse().unwrap())
            .sd_unicast("127.0.0.2".parse().unwrap())
            .start()
            .await
            .expect("Server runtime");

        let mut offering_a = runtime
            .offer(SVC_A, InstanceId::Id(0x0001))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .tcp_port(SERVER_TCP_PORT)
            .start()
            .await
            .expect("Offer service A");

        let mut offering_b = runtime
            .offer(SVC_B, InstanceId::Id(0x0001))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .tcp_port(SERVER_TCP_PORT)
            .start()
            .await
            .expect("Offer service B on shared TCP port");

        ready_tx.send(()).await.ok();

        if let Some(ServiceEvent::Subscribe { client, .. }) = offering_a.next().await {
            port_a_tx.send(client.address.port()).await.ok();
        }
        if let Some(ServiceEvent::Subscribe { client, .. }) = offering_b.next().await {
            port_b_tx.send(client.address.port()).await.ok();
        }

        done_rx.recv().await;
    });

    ready_rx.recv().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let runtime = recentip::configure()
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .sd_unicast("127.0.0.1".parse().unwrap())
        .start()
        .await
        .expect("Client runtime");

    let policy = TransportPolicy::new(vec![TransportPreference::tcp().with_port(CLIENT_TCP_PORT)]);

    let proxy_a = tokio::time::timeout(
        Duration::from_secs(5),
        runtime.find(SVC_A).instance(InstanceId::Id(0x0001)),
    )
    .await
    .expect("Discovery A timeout")
    .expect("Service A available")
    .with_transport_policy(policy.clone());

    let proxy_b = tokio::time::timeout(
        Duration::from_secs(5),
        runtime.find(SVC_B).instance(InstanceId::Id(0x0001)),
    )
    .await
    .expect("Discovery B timeout")
    .expect("Service B available")
    .with_transport_policy(policy);

    let _sub_a = tokio::time::timeout(
        Duration::from_secs(5),
        proxy_a.subscribe(EventgroupId::new(1).unwrap()),
    )
    .await
    .expect("Subscribe A timeout")
    .expect("Subscribe A success");

    let _sub_b = tokio::time::timeout(
        Duration::from_secs(5),
        proxy_b.subscribe(EventgroupId::new(1).unwrap()),
    )
    .await
    .expect("Subscribe B timeout")
    .expect("Subscribe B success");

    let port_a = tokio::time::timeout(Duration::from_secs(5), port_a_rx.recv())
        .await
        .expect("port A observation timeout")
        .expect("server must observe Subscribe A");
    let port_b = tokio::time::timeout(Duration::from_secs(5), port_b_rx.recv())
        .await
        .expect("port B observation timeout")
        .expect("server must observe Subscribe B");

    assert_eq!(
        port_a, CLIENT_TCP_PORT,
        "service A Subscribe must carry fixed client port {CLIENT_TCP_PORT}, got {port_a}"
    );
    assert_eq!(
        port_b, CLIENT_TCP_PORT,
        "service B Subscribe must carry fixed client port {CLIENT_TCP_PORT}, got {port_b}"
    );

    done_tx.send(()).await.ok();
    server_handle.await.expect("Server task");

    runtime.shutdown().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
}

// ============================================================================
// TCP same-service-id different major version: shared-port fails, diff ports +
// fixed client port succeeds — real_network
// ============================================================================

/// Same service_id, same instance, but **different major versions** on the server.
///
/// Phase 1 – verify the offer conflict:
/// Offering both major versions of the service on the **same** TCP port must
/// fail with `AddrInUse` (the runtime cannot route by version since the RPC
/// header carries no major version field).
///
/// Phase 2 – subscribe with a fixed client port to both:
/// After re-offering major v2 on a *different* port, the client subscribes to
/// **both versions** using a `TransportPolicy` that pins the local TCP source
/// port to an explicit value.  Because the two services sit on different server
/// TCP ports the connections have distinct 4-tuples
/// `(client_ip:CLIENT_TCP_PORT, server_ip:PORT_V1)` and
/// `(client_ip:CLIENT_TCP_PORT, server_ip:PORT_V2)`, so both `bind()` calls
/// succeed (Linux allows this with `SO_REUSEADDR`).
///
/// Assertions:
/// - second offer on same port → `AddrInUse`,
/// - both subscriptions succeed,
/// - server-observed Subscribe v1 port == `CLIENT_TCP_PORT`,
/// - server-observed Subscribe v2 port == `CLIENT_TCP_PORT`.
#[tokio::test]
async fn tcp_same_id_diff_major_same_client_port_real_network() {
    use recentip::config::{TransportPolicy, TransportPreference};
    use recentip::Error;

    const SVC_ID: u16 = 0x1239;
    const SERVER_TCP_PORT_V1: u16 = 19883;
    const SERVER_TCP_PORT_V2: u16 = 19884;
    // Below the Linux default ephemeral range (32768–60999).
    const CLIENT_TCP_PORT: u16 = 19885;

    let (ready_tx, mut ready_rx) = mpsc::channel::<()>(1);
    let (port_v1_tx, mut port_v1_rx) = mpsc::channel::<u16>(1);
    let (port_v2_tx, mut port_v2_rx) = mpsc::channel::<u16>(1);
    let (done_tx, mut done_rx) = mpsc::channel::<()>(1);

    let server_handle = tokio::spawn(async move {
        let runtime = recentip::configure()
            .sd_multicast_group("239.255.255.250".parse().unwrap())
            .sd_unicast("127.0.0.2".parse().unwrap())
            .start()
            .await
            .expect("Server runtime");

        // Phase 1: major v1 at PORT_V1 — must succeed.
        let mut offering_v1 = runtime
            .offer(SVC_ID, InstanceId::Id(0x0001))
            .version(1, 0)
            .tcp_port(SERVER_TCP_PORT_V1)
            .start()
            .await
            .expect("offering major v1 on its own port must succeed");

        // Phase 1: major v2 on the SAME port as v1 — must fail.
        let conflict = runtime
            .offer(SVC_ID, InstanceId::Id(0x0001))
            .version(2, 0)
            .tcp_port(SERVER_TCP_PORT_V1)
            .start()
            .await;
        assert!(
            matches!(&conflict, Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::AddrInUse),
            "offering same service_id/same port with different major version must fail AddrInUse; got: {:?}",
            conflict.as_ref().err()
        );

        // Phase 2: major v2 on a different port — must succeed.
        let mut offering_v2 = runtime
            .offer(SVC_ID, InstanceId::Id(0x0001))
            .version(2, 0)
            .tcp_port(SERVER_TCP_PORT_V2)
            .start()
            .await
            .expect("offering major v2 on a distinct port must succeed");

        ready_tx.send(()).await.ok();

        if let Some(ServiceEvent::Subscribe { client, .. }) = offering_v1.next().await {
            port_v1_tx.send(client.address.port()).await.ok();
        }
        if let Some(ServiceEvent::Subscribe { client, .. }) = offering_v2.next().await {
            port_v2_tx.send(client.address.port()).await.ok();
        }

        done_rx.recv().await;
    });

    ready_rx.recv().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let runtime = recentip::configure()
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .sd_unicast("127.0.0.1".parse().unwrap())
        .start()
        .await
        .expect("Client runtime");

    let policy = TransportPolicy::new(vec![TransportPreference::tcp().with_port(CLIENT_TCP_PORT)]);

    // Discover and subscribe to major v1.
    let proxy_v1 = tokio::time::timeout(
        Duration::from_secs(5),
        runtime
            .find(SVC_ID)
            .instance(InstanceId::Id(0x0001))
            .major_version(1u8),
    )
    .await
    .expect("Discovery v1 timeout")
    .expect("Service v1 available")
    .with_transport_policy(policy.clone());

    // Discover and subscribe to major v2.
    let proxy_v2 = tokio::time::timeout(
        Duration::from_secs(5),
        runtime
            .find(SVC_ID)
            .instance(InstanceId::Id(0x0001))
            .major_version(2u8),
    )
    .await
    .expect("Discovery v2 timeout")
    .expect("Service v2 available")
    .with_transport_policy(policy);

    // The two services live on different server ports → different 4-tuples →
    // the same local port can be reused for both connections.
    let _sub_v1 = tokio::time::timeout(
        Duration::from_secs(5),
        proxy_v1.subscribe(EventgroupId::new(1).unwrap()),
    )
    .await
    .expect("Subscribe v1 timeout")
    .expect("Subscribe v1 success");

    let _sub_v2 = tokio::time::timeout(
        Duration::from_secs(5),
        proxy_v2.subscribe(EventgroupId::new(1).unwrap()),
    )
    .await
    .expect("Subscribe v2 timeout")
    .expect("Subscribe v2 success");

    let port_v1 = tokio::time::timeout(Duration::from_secs(5), port_v1_rx.recv())
        .await
        .expect("port v1 observation timeout")
        .expect("server must observe Subscribe v1");
    let port_v2 = tokio::time::timeout(Duration::from_secs(5), port_v2_rx.recv())
        .await
        .expect("port v2 observation timeout")
        .expect("server must observe Subscribe v2");

    assert_eq!(
        port_v1, CLIENT_TCP_PORT,
        "service v1 Subscribe must carry fixed client port {CLIENT_TCP_PORT}, got {port_v1}"
    );
    assert_eq!(
        port_v2, CLIENT_TCP_PORT,
        "service v2 Subscribe must carry fixed client port {CLIENT_TCP_PORT}, got {port_v2}"
    );

    done_tx.send(()).await.ok();
    server_handle.await.expect("Server task");

    runtime.shutdown().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
}

// ============================================================================
// TCP one service, two subscriptions, same fixed client port — real_network
// ============================================================================

/// One server offers one service on one TCP port.  The client holds a
/// `TransportPolicy` that pins the local TCP source port to an explicit value
/// and subscribes to **two different eventgroups** sequentially.
///
/// The first subscription opens a TCP connection
/// `(client_ip:CLIENT_TCP_PORT → server_ip:SERVER_TCP_PORT)`.
/// The second subscription targets the same server endpoint and therefore needs
/// a second TCP connection with the **same** source port — which would produce
/// an identical 4-tuple.  The Linux TCP stack rejects this: even with
/// `SO_REUSEADDR`, two simultaneous connections cannot share the same 4-tuple.
///
/// Assertions:
/// - first subscription succeeds; server observes `CLIENT_TCP_PORT`,
/// - second subscription fails with an I/O error.
#[tokio::test]
async fn tcp_one_service_two_subs_same_client_port_second_fails_real_network() {
    use recentip::config::{TransportPolicy, TransportPreference};
    use recentip::Error;

    const SVC_ID: u16 = 0x123A;
    const SVC_VERSION: (u8, u32) = (1, 0);
    const SERVER_TCP_PORT: u16 = 19886;
    // Below the Linux default ephemeral range (32768–60999).
    const CLIENT_TCP_PORT: u16 = 19887;

    let (ready_tx, mut ready_rx) = mpsc::channel::<()>(1);
    let (port_tx, mut port_rx) = mpsc::channel::<u16>(1);
    let (done_tx, mut done_rx) = mpsc::channel::<()>(1);

    let server_handle = tokio::spawn(async move {
        let runtime = recentip::configure()
            .sd_multicast_group("239.255.255.250".parse().unwrap())
            .sd_unicast("127.0.0.2".parse().unwrap())
            .start()
            .await
            .expect("Server runtime");

        let mut offering = runtime
            .offer(SVC_ID, InstanceId::Id(0x0001))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .tcp_port(SERVER_TCP_PORT)
            .start()
            .await
            .expect("Offer service");

        ready_tx.send(()).await.ok();

        // Only the first subscribe arrives (second fails client-side).
        if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
            port_tx.send(client.address.port()).await.ok();
        }

        done_rx.recv().await;
    });

    ready_rx.recv().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let runtime = recentip::configure()
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .sd_unicast("127.0.0.1".parse().unwrap())
        .start()
        .await
        .expect("Client runtime");

    let policy = TransportPolicy::new(vec![TransportPreference::tcp().with_port(CLIENT_TCP_PORT)]);

    let proxy = tokio::time::timeout(
        Duration::from_secs(5),
        runtime.find(SVC_ID).instance(InstanceId::Id(0x0001)),
    )
    .await
    .expect("Discovery timeout")
    .expect("Service available")
    .with_transport_policy(policy);

    // First subscription: must succeed and bind CLIENT_TCP_PORT.
    let _sub1 = tokio::time::timeout(
        Duration::from_secs(5),
        proxy.subscribe(EventgroupId::new(1).unwrap()),
    )
    .await
    .expect("Subscribe eg1 timeout")
    .expect("Subscribe eg1 must succeed");

    let observed_port = tokio::time::timeout(Duration::from_secs(5), port_rx.recv())
        .await
        .expect("port observation timeout")
        .expect("server must observe first Subscribe");
    assert_eq!(
        observed_port, CLIENT_TCP_PORT,
        "first Subscribe must carry fixed client port {CLIENT_TCP_PORT}, got {observed_port}"
    );

    // Second subscription: targets the same server endpoint → duplicate TCP
    // 4-tuple → OS rejects the connection attempt.
    let result2 = tokio::time::timeout(
        Duration::from_secs(5),
        proxy.subscribe(EventgroupId::new(2).unwrap()),
    )
    .await
    .expect("Subscribe eg2 timeout (should return Err fast, not hang)");

    assert!(
        matches!(result2, Err(Error::Io(_))),
        "second Subscribe to the same service with the same fixed TCP port must fail with an I/O \
         error (duplicate 4-tuple); got: {:?}",
        result2.as_ref().err()
    );

    done_tx.send(()).await.ok();
    server_handle.await.expect("Server task");

    runtime.shutdown().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
}
