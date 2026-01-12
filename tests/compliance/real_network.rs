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
//! The turmoil-based tests (run with `--features turmoil`) provide comprehensive
//! network testing with simulated separate hosts.

use recentip::{
    EventId, EventgroupId, InstanceId, MethodId, Runtime, RuntimeConfig, ServiceEvent, Transport,
};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;
use tokio::sync::mpsc;

// ============================================================================
// Test Service Definition
// ============================================================================

const ECHO_SERVICE_ID: u16 = 0x1234;
const ECHO_SERVICE_VERSION: (u8, u32) = (1, 0);

// ============================================================================
// Helper Functions
// ============================================================================

/// Create config bound to the local network IP for proper multicast
/// Note: We bind to INADDR_ANY on the SD multicast port (30490) so that multicast discovery works.
/// Different runtimes on the same machine can share this port via SO_REUSEPORT.
fn test_config(_port: u16) -> RuntimeConfig {
    RuntimeConfig::builder()
        .bind_addr(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            30490,
        )))
        .sd_multicast(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::new(239, 255, 255, 250),
            30490,
        )))
        .build()
}

fn tcp_test_config(_port: u16) -> RuntimeConfig {
    RuntimeConfig::builder()
        .bind_addr(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            30490,
        )))
        .sd_multicast(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::new(239, 255, 255, 250),
            30490,
        )))
        .transport(Transport::Tcp)
        .build()
}

fn tcp_test_config_with_magic_cookies(_port: u16) -> RuntimeConfig {
    RuntimeConfig::builder()
        .bind_addr(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            30490,
        )))
        .sd_multicast(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::new(239, 255, 255, 250),
            30490,
        )))
        .transport(Transport::Tcp)
        .magic_cookies(true)
        .build()
}

// ============================================================================
// UDP Tests
// ============================================================================

/// Test basic UDP request/response on real network
#[tokio::test]
async fn udp_request_response_real_network() {
    // Use different ports to avoid conflicts
    let server_port = 40100;
    let client_port = 40200;

    // Create server runtime and offer service
    let server_config = test_config(server_port);
    let server_runtime = Runtime::new(server_config).await.expect("Server runtime");

    let mut offering = server_runtime
        .offer(ECHO_SERVICE_ID, InstanceId::Id(0x0001))
        .version(ECHO_SERVICE_VERSION.0, ECHO_SERVICE_VERSION.1)
        .udp()
        .start()
        .await
        .expect("Offer service");

    // Create client runtime
    let client_config = test_config(client_port);
    let client_runtime = Runtime::new(client_config).await.expect("Client runtime");

    // Spawn server handler task
    let server_task = tokio::spawn(async move {
        if let Some(ServiceEvent::Call {
            responder, payload, ..
        }) = offering.next().await
        {
            let mut response = b"ECHO:".to_vec();
            response.extend_from_slice(&payload);
            responder.reply(&response).await.expect("Reply");
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
    let server_port = 40300;
    let client_port = 40400;

    let (ready_tx, mut ready_rx) = mpsc::channel::<()>(1);
    let (done_tx, mut done_rx) = mpsc::channel::<()>(1);

    let server_handle = tokio::spawn(async move {
        let config = test_config(server_port);
        let runtime = Runtime::new(config).await.expect("Server runtime");

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

    let config = test_config(client_port);
    let runtime = Runtime::new(config).await.expect("Client runtime");

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
    let server_port = 40500;
    let client_port = 40600;

    let (ready_tx, mut ready_rx) = mpsc::channel::<()>(1);
    let (done_tx, mut done_rx) = mpsc::channel::<()>(1);

    let server_handle = tokio::spawn(async move {
        let config = tcp_test_config(server_port);
        let runtime = Runtime::new(config).await.expect("Server runtime");

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
            responder.reply(&response).await.expect("Reply");
        }

        done_rx.recv().await;
    });

    ready_rx.recv().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let config = tcp_test_config(client_port);
    let runtime = Runtime::new(config).await.expect("Client runtime");

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
    let server_port = 40700;
    let client_port = 40800;

    let (ready_tx, mut ready_rx) = mpsc::channel::<()>(1);
    let (done_tx, mut done_rx) = mpsc::channel::<()>(1);

    let server_handle = tokio::spawn(async move {
        let config = tcp_test_config_with_magic_cookies(server_port);
        let runtime = Runtime::new(config).await.expect("Server runtime");

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
            responder.reply(&response).await.expect("Reply");
        }

        done_rx.recv().await;
    });

    ready_rx.recv().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let config = tcp_test_config_with_magic_cookies(client_port);
    let runtime = Runtime::new(config).await.expect("Client runtime");

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
    let server_port = 40900;
    let client_port = 41000;

    let (ready_tx, mut ready_rx) = mpsc::channel::<()>(1);
    let (done_tx, mut done_rx) = mpsc::channel::<()>(1);

    let server_handle = tokio::spawn(async move {
        let config = tcp_test_config(server_port);
        let runtime = Runtime::new(config).await.expect("Server runtime");

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
                responder.reply(&response).await.expect("Reply");
            }
        }

        done_rx.recv().await;
    });

    ready_rx.recv().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let config = tcp_test_config(client_port);
    let runtime = Runtime::new(config).await.expect("Client runtime");

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

/// Test event subscription on real UDP network
///
/// This test verifies unicast event delivery on a real network.
/// Multicast event delivery is not yet implemented, but this test uses unicast only.
///
/// NOTE: This test is currently unreliable on same-machine setups due to SO_REUSEPORT.
/// When both server and client bind to the same SD port (30490) via SO_REUSEPORT,
/// unicast SubscribeEventgroupAck messages may be delivered to the wrong socket.
/// Run with `cargo test -- --ignored` to include it.
/// This should finally be tested using network namespaces, docker or vagrant
#[tokio::test]
#[ignore = "Requires .bind_sd_unicast() implementation + network namespaces/docker"]
async fn udp_events_real_network() {
    let server_port = 41100;
    let client_port = 41200;

    let (ready_tx, mut ready_rx) = mpsc::channel::<()>(1);
    let (subscribed_tx, mut subscribed_rx) = mpsc::channel::<()>(1);
    let (done_tx, mut done_rx) = mpsc::channel::<()>(1);

    let server_handle = tokio::spawn(async move {
        let config = test_config(server_port);
        let runtime = Runtime::new(config).await.expect("Server runtime");

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
        runtime.shutdown().await;

        done_rx.recv().await;
    });

    ready_rx.recv().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let config = test_config(client_port);
    let runtime = Runtime::new(config).await.expect("Client runtime");

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
    let server_config = RuntimeConfig::builder()
        .bind_addr(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            sd_port,
        )))
        .sd_multicast(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::new(239, 255, 255, 250),
            sd_port,
        )))
        .build();
    let client_config = RuntimeConfig::builder()
        .bind_addr(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            sd_port,
        )))
        .sd_multicast(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::new(239, 255, 255, 250),
            sd_port,
        )))
        .build();

    let server_runtime = Runtime::new(server_config).await.expect("Server runtime");
    let mut offering = server_runtime
        .offer(ECHO_SERVICE_ID, InstanceId::Id(0x0003))
        .version(ECHO_SERVICE_VERSION.0, ECHO_SERVICE_VERSION.1)
        .udp()
        .start()
        .await
        .expect("Offer service");

    let client_runtime = Runtime::new(client_config).await.expect("Client runtime");

    let server_task = tokio::spawn(async move {
        if let Some(ServiceEvent::Call {
            responder, payload, ..
        }) = offering.next().await
        {
            let mut response = b"REUSEPORT:".to_vec();
            response.extend_from_slice(&payload);
            responder.reply(&response).await.expect("Reply");
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
