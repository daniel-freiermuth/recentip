//! SD Pub/Sub Compliance Tests
//!
//! Tests for the 45 Publish/Subscribe requirements in someip-sd.rst section
//! "Publish/Subscribe with RECENT/IP and RECENT/IP-SD".
//!
//! Run with: cargo test --test sd_pubsub
//!
//! # Requirement Categories
//!
//! 1. Basic subscription flow (OfferService → SubscribeEventgroup → SubscribeEventgroupAck)
//! 2. Subscription lifecycle (TTL, StopSubscribe, re-subscription)
//! 3. Initial events (first notification after subscribe)
//! 4. Multiple eventgroups and overlapping fields
//! 5. Error handling and link loss scenarios
//! 6. Server state management

use someip_runtime::handle::ServiceEvent;
use someip_runtime::prelude::*;
use someip_runtime::Runtime;
use std::sync::{Arc, Mutex};
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

const PUB_SUB_SERVICE_ID: u16 = 0x1234;
const PUB_SUB_SERVICE_VERSION: (u8, u32) = (1, 0);

// ============================================================================
// 1. BASIC SUBSCRIPTION FLOW
// ============================================================================

/// [feat_req_recentipsd_422] Clients needing events shall register using SD at run-time.
/// [feat_req_recentipsd_428] OfferService is trigger for Subscriptions.
///
/// When a server offers a service, clients can subscribe to eventgroups.
#[test_log::test]
fn client_registers_for_events_via_sd() {
    covers!(feat_req_recentipsd_422, feat_req_recentipsd_428);

    let subscription_succeeded = Arc::new(Mutex::new(false));
    let sub_flag = Arc::clone(&subscription_succeeded);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let _offering = runtime
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001))
            .version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        // Subscribe to eventgroup - this registers for events at runtime via SD
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let result =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup)).await;

        if result.is_ok() && result.unwrap().is_ok() {
            *sub_flag.lock().unwrap() = true;
        }
        Ok(())
    });

    sim.run().unwrap();
    assert!(
        *subscription_succeeded.lock().unwrap(),
        "Subscription should succeed"
    );
}

/// [feat_req_recentipsd_429] Server shall send OfferService on startup to discover interested clients.
///
/// When server starts, it sends OfferService which clients can respond to with Subscribe.
#[test_log::test]
fn server_offers_on_startup_to_discover_clients() {
    covers!(feat_req_recentipsd_429);

    let offer_received = Arc::new(Mutex::new(false));
    let offer_flag = Arc::clone(&offer_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Client starts first, waiting for offer
    sim.client("client", async move {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);

        // Wait for service to become available (meaning OfferService was received)
        let result = tokio::time::timeout(Duration::from_secs(10), proxy).await;
        if result.is_ok() {
            *offer_flag.lock().unwrap() = true;
        }
        Ok(())
    });

    // Server starts after client is ready
    sim.host("server", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let _offering = runtime
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001))
            .version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(
        *offer_received.lock().unwrap(),
        "Client should receive OfferService from server startup"
    );
}

/// [feat_req_recentipsd_430] Client implements service interface and signals wish using SubscribeEventgroup.
/// [feat_req_recentipsd_431] Client shall respond to OfferService with SubscribeEventgroup.
///
/// Client responds to OfferService by sending SubscribeEventgroup.
#[test_log::test]
fn client_responds_to_offer_with_subscribe() {
    covers!(feat_req_recentipsd_430, feat_req_recentipsd_431);

    let server_received_subscribe = Arc::new(Mutex::new(false));
    let sub_flag = Arc::clone(&server_received_subscribe);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let flag = Arc::clone(&sub_flag);
        async move {
            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
            let mut offering = runtime
                .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001))
                .version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            // Wait for Subscribe event from client
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while tokio::time::Instant::now() < deadline {
                match tokio::time::timeout(Duration::from_millis(100), offering.next()).await {
                    Ok(Some(ServiceEvent::Subscribe { .. })) => {
                        *flag.lock().unwrap() = true;
                        break;
                    }
                    Ok(Some(_)) => continue,
                    Ok(None) => break,
                    Err(_) => continue,
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        // Subscribe in response to OfferService
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription = proxy.subscribe(eventgroup).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(
        *server_received_subscribe.lock().unwrap(),
        "Server should receive Subscribe from client"
    );
}

/// Tests API-level behavior: `subscribe()` resolves to Ok when the server acknowledges.
///
/// Wire-level verification of SubscribeEventgroupAck (feat_req_recentipsd_441) is in
/// `wire_format::subscribe_ack_entry_type`.
#[test_log::test]
fn subscribe_resolves_on_ack() {
    let ack_received = Arc::new(Mutex::new(false));
    let ack_flag = Arc::clone(&ack_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let _offering = runtime
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001))
            .version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();

        // The subscribe() future resolves when SubscribeEventgroupAck is received.
        // Wire-level verification of entry type 0x07 is in wire_format::subscribe_ack_entry_type.
        let result =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup)).await;
        if let Ok(Ok(_)) = result {
            *ack_flag.lock().unwrap() = true;
        }
        Ok(())
    });

    sim.run().unwrap();
    assert!(
        *ack_received.lock().unwrap(),
        "subscribe() should complete successfully when ack received"
    );
}

// ============================================================================
// 2. SUBSCRIPTION LIFECYCLE
// ============================================================================

/// [feat_req_recentipsd_432] Server must keep state of SubscribeEventgroup to know if events should be sent.
///
/// Two clients subscribe to different eventgroups; each receives only their eventgroup's events.
#[test_log::test]
fn server_tracks_subscription_state() {
    covers!(feat_req_recentipsd_432);

    let client1_events = Arc::new(Mutex::new(Vec::<u16>::new()));
    let client2_events = Arc::new(Mutex::new(Vec::<u16>::new()));
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
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001))
            .version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for both subscriptions
        tokio::time::sleep(Duration::from_millis(500)).await;

        let eg1 = EventgroupId::new(0x0001).unwrap();
        let eg2 = EventgroupId::new(0x0002).unwrap();
        let event1 = EventId::new(0x8001).unwrap();
        let event2 = EventId::new(0x8002).unwrap();

        // Send events to both eventgroups
        offering.notify(eg1, event1, b"eg1").await.unwrap();
        offering.notify(eg2, event2, b"eg2").await.unwrap();

        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(())
    });

    // Client 1 subscribes to eventgroup 1 only
    sim.client("client1", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client1").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eg1 = EventgroupId::new(0x0001).unwrap();
        let mut sub = proxy.subscribe(eg1).await.unwrap();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(100), sub.next()).await {
                Ok(Some(event)) => c1_events.lock().unwrap().push(event.event_id.value()),
                Ok(None) => break,
                Err(_) => continue,
            }
        }
        Ok(())
    });

    // Client 2 subscribes to eventgroup 2 only
    sim.client("client2", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client2").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eg2 = EventgroupId::new(0x0002).unwrap();
        let mut sub = proxy.subscribe(eg2).await.unwrap();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(100), sub.next()).await {
                Ok(Some(event)) => c2_events.lock().unwrap().push(event.event_id.value()),
                Ok(None) => break,
                Err(_) => continue,
            }
        }
        Ok(())
    });

    sim.run().unwrap();

    let c1 = client1_events.lock().unwrap();
    let c2 = client2_events.lock().unwrap();

    // Client 1 should only have event 0x8001 (from eg1)
    assert!(
        c1.iter().all(|&e| e == 0x8001),
        "Client1 should only receive eg1 events: {:?}",
        *c1
    );
    // Client 2 should only have event 0x8002 (from eg2)
    assert!(
        c2.iter().all(|&e| e == 0x8002),
        "Client2 should only receive eg2 events: {:?}",
        *c2
    );
    // At least one client should have received something
    assert!(
        !c1.is_empty() && !c2.is_empty(),
        "Both client should receive events"
    );
}

/// [feat_req_recentipsd_433] Client shall deregister by sending StopSubscribeEventgroup (TTL=0).
///
/// Dropping subscription sends StopSubscribeEventgroup.
#[test_log::test]
fn client_deregisters_with_stop_subscribe() {
    covers!(feat_req_recentipsd_433);

    let unsubscribe_received = Arc::new(Mutex::new(false));
    let unsub_flag = Arc::clone(&unsubscribe_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let flag = Arc::clone(&unsub_flag);
        async move {
            let config = RuntimeConfig::builder()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .build();
            let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
            let mut offering = runtime
                .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001))
                .version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            // Wait for events
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while tokio::time::Instant::now() < deadline {
                match tokio::time::timeout(Duration::from_millis(100), offering.next()).await {
                    Ok(Some(ServiceEvent::Unsubscribe { .. })) => {
                        *flag.lock().unwrap() = true;
                        break;
                    }
                    Ok(Some(_)) => continue,
                    Ok(None) => break,
                    Err(_) => continue,
                }
            }
            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();

        // Subscribe then drop to trigger StopSubscribe
        {
            let _subscription = proxy.subscribe(eventgroup).await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
            // subscription dropped here
        }

        // Give time for StopSubscribe to be sent
        runtime.shutdown().await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(
        *unsubscribe_received.lock().unwrap(),
        "Server should receive Unsubscribe/StopSubscribe"
    );
}

/* WIP: To be reviewed
// ============================================================================
// 3. INITIAL EVENTS
// ============================================================================

/// [feat_req_recentipsd_691] Server shall send initial events after SubscribeEventgroupAck.
/// [feat_req_recentipsd_107] Initial events should be sent after SubscribeEventgroupAck.
///
/// After acknowledging subscription, server sends initial field values.
#[test_log::test]
fn server_sends_initial_events_after_ack() {
    covers!(feat_req_recentipsd_691, feat_req_recentipsd_107);

    let initial_event_received = Arc::new(Mutex::new(false));
    let event_flag = Arc::clone(&initial_event_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let offering = runtime
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001)).version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for subscription, then send initial event
        tokio::time::sleep(Duration::from_millis(500)).await;

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();

        // This simulates initial event after SubscribeAck
        offering.notify(eventgroup, event_id, b"initial_value").await.unwrap();

        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription = proxy.subscribe(eventgroup).await.unwrap();

        // Should receive initial event shortly after subscription
        let result = tokio::time::timeout(Duration::from_secs(3), subscription.next()).await;
        if let Ok(Some(event)) = result {
            if event.payload.as_ref() == b"initial_value" {
                *event_flag.lock().unwrap() = true;
            }
        }
        Ok(())
    });

    sim.run().unwrap();
    assert!(*initial_event_received.lock().unwrap(), "Should receive initial event after subscribe");
}

/// [feat_req_recentipsd_1191] Client shall request Initial Events by setting Initial Data Requested Flag.
/// [feat_req_recentipsd_1192] Client shall not request Initial Events if TTL not expired.
///
/// Initial events are only requested on first subscription, not renewals.
#[test_log::test]
fn initial_events_only_on_first_subscribe() {
    covers!(feat_req_recentipsd_1191, feat_req_recentipsd_1192);

    let events_received = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let events_clone = Arc::clone(&events_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let offering = runtime
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001)).version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();

        // Wait for initial subscribe
        tokio::time::sleep(Duration::from_millis(500)).await;
        offering.notify(eventgroup, event_id, b"initial").await.unwrap();

        // Send updates
        tokio::time::sleep(Duration::from_millis(500)).await;
        offering.notify(eventgroup, event_id, b"update1").await.unwrap();

        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription = proxy.subscribe(eventgroup).await.unwrap();

        // Collect events
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(100), subscription.next()).await {
                Ok(Some(event)) => events_clone.lock().unwrap().push(event.payload.to_vec()),
                Ok(None) => break,
                Err(_) => continue,
            }
        }
        Ok(())
    });

    sim.run().unwrap();

    let events = events_received.lock().unwrap();
    assert!(!events.is_empty(), "Should receive at least initial event");
    // First event should be the initial one
    if !events.is_empty() {
        assert_eq!(events[0], b"initial", "First event should be initial value");
    }
}

// ============================================================================
// 4. MULTIPLE EVENTGROUPS
// ============================================================================

/// [feat_req_recentipsd_1168] Server shall not send duplicate events for overlapping eventgroups.
///
/// If client subscribes to multiple eventgroups with same event, no duplicates sent.
#[test_log::test]
fn no_duplicate_events_for_overlapping_eventgroups() {
    covers!(feat_req_recentipsd_1168);

    let events_received = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let events_clone = Arc::clone(&events_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let offering = runtime
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001)).version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for subscriptions
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send event on eventgroup 1
        let eventgroup1 = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        offering.notify(eventgroup1, event_id, b"shared_event").await.unwrap();

        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        // Subscribe to two eventgroups (conceptually overlapping)
        let eventgroup1 = EventgroupId::new(0x0001).unwrap();
        let eventgroup2 = EventgroupId::new(0x0002).unwrap();

        let mut sub1 = proxy.subscribe(eventgroup1).await.unwrap();
        let _sub2 = proxy.subscribe(eventgroup2).await; // May or may not succeed

        // Collect events from sub1
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(100), sub1.next()).await {
                Ok(Some(event)) => events_clone.lock().unwrap().push(event.payload.to_vec()),
                Ok(None) => break,
                Err(_) => continue,
            }
        }
        Ok(())
    });

    sim.run().unwrap();

    let events = events_received.lock().unwrap();
    // Should receive event exactly once, not duplicated
    let shared_count = events.iter().filter(|e| e.as_slice() == b"shared_event").count();
    assert!(shared_count <= 1, "Should not receive duplicate events: got {} copies", shared_count);
}

/// [feat_req_recentipsd_1166] Server may optimize initial events for same field in same SD message.
/// [feat_req_recentipsd_1167] Server shall send initial events separately for different SD messages.
///
/// Initial events behavior for multiple eventgroups.
#[test_log::test]
fn initial_events_for_multiple_eventgroups() {
    covers!(feat_req_recentipsd_1166, feat_req_recentipsd_1167);

    let events_received = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let events_clone = Arc::clone(&events_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let offering = runtime
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001)).version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for subscriptions
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send initial events for subscribed eventgroups
        let eventgroup1 = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        offering.notify(eventgroup1, event_id, b"initial_eg1").await.unwrap();

        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup1 = EventgroupId::new(0x0001).unwrap();
        let mut sub1 = proxy.subscribe(eventgroup1).await.unwrap();

        // Collect events
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(100), sub1.next()).await {
                Ok(Some(event)) => events_clone.lock().unwrap().push(event.payload.to_vec()),
                Ok(None) => break,
                Err(_) => continue,
            }
        }
        Ok(())
    });

    sim.run().unwrap();

    let events = events_received.lock().unwrap();
    assert!(!events.is_empty(), "Should receive initial events for eventgroup");
}

// ============================================================================
// 5. LINK LOSS AND ERROR HANDLING
// ============================================================================

/// [feat_req_recentipsd_435] Server shall delete subscription if RECENT/IP error received.
///
/// Server cleans up subscription on error.
#[test_log::test]
fn server_deletes_subscription_on_error() {
    covers!(feat_req_recentipsd_435);

    // This test verifies the error handling concept
    // Actual error injection would require network simulation features
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let _offering = runtime
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001)).version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Server maintains subscription list, cleans up on errors
        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription = proxy.subscribe(eventgroup).await.unwrap();

        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(())
    });

    sim.run().unwrap();
    // Test passes if no panic - error handling mechanisms exist
}

/// [feat_req_recentipsd_436] Server shall send OfferService when link comes up again.
/// [feat_req_recentipsd_437] Server shall delete subscriptions when link goes down.
///
/// Link loss handling on server side.
#[test_log::test]
fn server_handles_link_loss() {
    covers!(feat_req_recentipsd_436, feat_req_recentipsd_437);

    // This test verifies the concept - actual link simulation requires network features
    let offer_after_link_up = Arc::new(Mutex::new(false));
    let offer_flag = Arc::clone(&offer_after_link_up);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Monitor will see offer when link comes up
    sim.client("monitor", async move {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("monitor").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);

        // Wait for offer
        let result = tokio::time::timeout(Duration::from_secs(10), proxy).await;
        if result.is_ok() {
            *offer_flag.lock().unwrap() = true;
        }
        Ok(())
    });

    sim.host("server", || async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer service - this sends OfferService
        let _offering = runtime
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001)).version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*offer_after_link_up.lock().unwrap(), "Should receive offer when server link is up");
}

/// [feat_req_recentipsd_439] Client shall resubscribe if no events received for configured time.
///
/// Timeout-based resubscription.
#[test_log::test]
fn client_resubscribes_on_timeout() {
    covers!(feat_req_recentipsd_439);

    // This test verifies the concept - actual timeout resubscription depends on configuration
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let _offering = runtime
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001)).version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let _subscription = proxy.subscribe(eventgroup).await.unwrap();

        // Client would resubscribe if no events received within timeout
        // (timeout-based resubscription is configurable per eventgroup)
        tokio::time::sleep(Duration::from_secs(3)).await;
        Ok(())
    });

    sim.run().unwrap();
    // Test passes - resubscription mechanism exists
}

/// [feat_req_recentipsd_440] Link-up shall start Initial Wait Phase and trigger SubscribeEventgroup.
///
/// Client subscribes after link comes up.
#[test_log::test]
fn client_subscribes_after_link_up() {
    covers!(feat_req_recentipsd_440);

    let subscription_succeeded = Arc::new(Mutex::new(false));
    let sub_flag = Arc::clone(&subscription_succeeded);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let _offering = runtime
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001)).version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        Ok(())
    });

    // Client starts (simulating link-up)
    sim.client("client", async move {
        // Initial wait phase
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let result = proxy.subscribe(eventgroup).await;
        if result.is_ok() {
            *sub_flag.lock().unwrap() = true;
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*subscription_succeeded.lock().unwrap(), "Client should subscribe after link-up");
}

// ============================================================================
// 6. TCP/UDP READINESS
// ============================================================================

/// [feat_req_recentipsd_1182] Client shall have UDP port ready before sending SubscribeEventgroup.
///
/// Client must be ready to receive before subscribing.
#[test_log::test]
fn client_udp_port_ready_before_subscribe() {
    covers!(feat_req_recentipsd_1182);

    let events_received = Arc::new(Mutex::new(false));
    let event_flag = Arc::clone(&events_received);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let offering = runtime
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001)).version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for subscription, then send event immediately
        tokio::time::sleep(Duration::from_millis(500)).await;

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        offering.notify(eventgroup, event_id, b"immediate").await.unwrap();

        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();

        // When subscribe() completes, client is ready to receive
        let mut subscription = proxy.subscribe(eventgroup).await.unwrap();

        // Should receive event immediately (port was ready)
        let result = tokio::time::timeout(Duration::from_secs(2), subscription.next()).await;
        if let Ok(Some(_)) = result {
            *event_flag.lock().unwrap() = true;
        }
        Ok(())
    });

    sim.run().unwrap();
    assert!(*events_received.lock().unwrap(), "Client should receive events (port was ready)");
}

/// [feat_req_recentipsd_767] Client shall open TCP connection before SubscribeEventgroup for reliable.
///
/// TCP must be ready before subscribing to reliable eventgroups.
#[test_log::test]
fn client_tcp_ready_before_subscribe_reliable() {
    covers!(feat_req_recentipsd_767);

    // This test verifies that TCP subscription works correctly
    // The actual TCP connection setup happens internally
    let subscription_succeeded = Arc::new(Mutex::new(false));
    let sub_flag = Arc::clone(&subscription_succeeded);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let mut config = RuntimeConfig::default();
        config.transport = someip_runtime::Transport::Tcp;

        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let _offering = runtime
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001)).version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut config = RuntimeConfig::default();
        config.transport = someip_runtime::Transport::Tcp;

        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let result = proxy.subscribe(eventgroup).await;
        if result.is_ok() {
            *sub_flag.lock().unwrap() = true;
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*subscription_succeeded.lock().unwrap(), "TCP subscription should succeed");
}

// ============================================================================
// 7. CYCLIC SUBSCRIBE TIMERS
// ============================================================================

/// [feat_req_recentipsd_828] Timer for cyclic SubscribeEventgroup shall be reset on OfferService.
///
/// Receiving OfferService resets subscription renewal timer.
#[test_log::test]
fn subscribe_timer_reset_on_offer() {
    covers!(feat_req_recentipsd_828);

    // This test verifies the subscription renewal concept
    let subscription_maintained = Arc::new(Mutex::new(false));
    let sub_flag = Arc::clone(&subscription_maintained);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let offering = runtime
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001)).version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for subscription
        tokio::time::sleep(Duration::from_millis(500)).await;

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();

        // Send events over time - subscription should be maintained
        for i in 0..3 {
            offering.notify(eventgroup, event_id, format!("event{}", i).as_bytes()).await.unwrap();
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription = proxy.subscribe(eventgroup).await.unwrap();

        // Collect events over time
        let mut event_count = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(200), subscription.next()).await {
                Ok(Some(_)) => event_count += 1,
                Ok(None) => break,
                Err(_) => continue,
            }
        }

        if event_count >= 2 {
            *sub_flag.lock().unwrap() = true;
        }
        Ok(())
    });

    sim.run().unwrap();
    assert!(*subscription_maintained.lock().unwrap(), "Subscription should be maintained over time");
}

/// [feat_req_recentipsd_829] If no cyclic SubscribeEventgroups configured, timer stays off.
///
/// Cyclic subscription renewal is optional.
#[test_log::test]
fn no_cyclic_subscribe_if_not_configured() {
    covers!(feat_req_recentipsd_829);

    // This test verifies that subscriptions work without cyclic renewal
    let subscription_works = Arc::new(Mutex::new(false));
    let sub_flag = Arc::clone(&subscription_works);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let offering = runtime
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001)).version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        offering.notify(eventgroup, event_id, b"test").await.unwrap();

        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription = proxy.subscribe(eventgroup).await.unwrap();

        let result = tokio::time::timeout(Duration::from_secs(2), subscription.next()).await;
        if let Ok(Some(_)) = result {
            *sub_flag.lock().unwrap() = true;
        }
        Ok(())
    });

    sim.run().unwrap();
    assert!(*subscription_works.lock().unwrap(), "Subscription should work without cyclic renewal");
}

// ============================================================================
// 8. IMPLICIT/PRE-CONFIGURED SUBSCRIPTIONS
// ============================================================================

/// [feat_req_recentipsd_444] Implicit (pre-configured) registration shall be supported.
///
/// Some deployments use pre-configured subscriptions.
#[test_log::test]
fn implicit_registration_supported() {
    covers!(feat_req_recentipsd_444);

    // This test verifies that explicit subscription works (implicit would be config-based)
    let subscription_works = Arc::new(Mutex::new(false));
    let sub_flag = Arc::clone(&subscription_works);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let _offering = runtime
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001)).version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        // Explicit registration (implicit would happen automatically via config)
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let result = proxy.subscribe(eventgroup).await;
        if result.is_ok() {
            *sub_flag.lock().unwrap() = true;
        }
        Ok(())
    });

    sim.run().unwrap();
    assert!(*subscription_works.lock().unwrap(), "Registration mechanism works");
}

// ============================================================================
// 9. STATE DIAGRAMS (conceptual verification)
// ============================================================================

/// [feat_req_recentipsd_442] Pub/Sub State Diagram - overall behavior.
/// [feat_req_recentipsd_625] State diagram for unicast eventgroups.
/// [feat_req_recentipsd_626] State diagram for multicast eventgroups.
/// [feat_req_recentipsd_823] State diagram for adaptive unicast/multicast.
///
/// Verifies the overall pub/sub state machine flow.
#[test_log::test]
fn pubsub_state_machine_flow() {
    covers!(
        feat_req_recentipsd_442,
        feat_req_recentipsd_625,
        feat_req_recentipsd_626,
        feat_req_recentipsd_823
    );

    let full_flow_completed = Arc::new(Mutex::new(false));
    let flow_flag = Arc::clone(&full_flow_completed);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let offering = runtime
            .offer(PUB_SUB_SERVICE_ID, InstanceId::Id(0x0001)).version(PUB_SUB_SERVICE_VERSION.0, PUB_SUB_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for subscription
        tokio::time::sleep(Duration::from_millis(500)).await;

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();

        // Send events
        offering.notify(eventgroup, event_id, b"state1").await.unwrap();
        offering.notify(eventgroup, event_id, b"state2").await.unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // State: Not Subscribed -> Find service
        let proxy = runtime.find(PUB_SUB_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();

        // State: Subscribe -> Subscribed
        let mut subscription = proxy.subscribe(eventgroup).await.unwrap();

        // State: Receiving events
        let mut event_count = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(100), subscription.next()).await {
                Ok(Some(_)) => event_count += 1,
                Ok(None) => break,
                Err(_) => continue,
            }
        }

        // State: Unsubscribe (implicit on drop)
        drop(subscription);

        if event_count >= 1 {
            *flow_flag.lock().unwrap() = true;
        }
        Ok(())
    });

    sim.run().unwrap();
    assert!(*full_flow_completed.lock().unwrap(), "Full pub/sub state machine flow should complete");
} */
