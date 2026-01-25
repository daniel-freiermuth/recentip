//! Subscription and Eventgroup Compliance Tests (Async/Turmoil)
//!
//! Tests the publish/subscribe (Pub/Sub) behavior per SOME/IP-SD specification.
//!
//! Reference: someip-sd.rst (Eventgroup entries and subscription handling)

use recentip::handle::ServiceEvent;
use recentip::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::helpers::configure_tracing;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

/// Type alias for turmoil-based runtime

const EVENT_SERVICE_ID: u16 = 0x1234;
const EVENT_SERVICE_VERSION: (u8, u32) = (1, 0);

// ============================================================================
// SUBSCRIPTION FLOW
// ============================================================================

/// feat_req_recentipsd_576: Subscribe entry type (0x06)
/// feat_req_recentipsd_109: Eventgroup Entry is 16 bytes
///
/// Client can subscribe to an eventgroup and receive events.
#[test_log::test]
fn subscribe_and_receive_events() {
    covers!(feat_req_recentipsd_576, feat_req_recentipsd_109);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .start_turmoil()
                .await
                .unwrap();

            let offering = runtime
                .offer(EVENT_SERVICE_ID, InstanceId::Id(0x0001))
                .version(EVENT_SERVICE_VERSION.0, EVENT_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            // Wait for subscription
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Send events
            let eventgroup = EventgroupId::new(0x0001).unwrap();
            let event_id = EventId::new(0x8001).unwrap();

            let event_handle = offering
                .event(event_id)
                .eventgroup(eventgroup)
                .create()
                .await
                .unwrap();

            event_handle.notify(b"event1").await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            event_handle.notify(b"event2").await.unwrap();

            tokio::time::sleep(Duration::from_millis(500)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(EVENT_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        // Subscribe to eventgroup
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        // Receive first event
        let event1 = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("Event1 timeout");
        assert!(event1.is_some());
        assert_eq!(event1.unwrap().payload.as_ref(), b"event1");

        // Receive second event
        let event2 = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("Event2 timeout");
        assert!(event2.is_some());
        assert_eq!(event2.unwrap().payload.as_ref(), b"event2");

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(2000)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

/// feat_req_recentipsd_576: SubscribeAck entry type (0x07)
///
/// Server must acknowledge subscriptions with SubscribeAck.
#[test_log::test]
fn subscribe_receives_ack() {
    covers!(feat_req_recentipsd_576);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .start_turmoil()
                .await
                .unwrap();

            let _offering = runtime
                .offer(EVENT_SERVICE_ID, InstanceId::Id(0x0001))
                .version(EVENT_SERVICE_VERSION.0, EVENT_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            tokio::time::sleep(Duration::from_millis(1000)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(EVENT_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();

        // Subscribe should succeed (implying SubscribeAck was received)
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout");

        assert!(result.is_ok(), "Subscription should be acknowledged");

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

/// feat_req_recentipsd_178: StopSubscribe uses TTL=0
///
/// When subscription handle is dropped, StopSubscribe should be sent.
#[test_log::test]
fn unsubscribe_on_drop() {
    covers!(feat_req_recentipsd_178);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .start_turmoil()
                .await
                .unwrap();

            let offering = runtime
                .offer(EVENT_SERVICE_ID, InstanceId::Id(0x0001))
                .version(EVENT_SERVICE_VERSION.0, EVENT_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            // Wait for subscribe
            tokio::time::sleep(Duration::from_millis(500)).await;

            let eventgroup = EventgroupId::new(0x0001).unwrap();
            let event_id = EventId::new(0x8001).unwrap();

            let event_handle = offering
                .event(event_id)
                .eventgroup(eventgroup)
                .create()
                .await
                .unwrap();

            // Send an event - should be delivered
            event_handle.notify(b"before_unsub").await.unwrap();

            // Wait for unsubscribe
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Send another event - should NOT be delivered (client unsubscribed)
            // (We can't directly test this without packet inspection, but the flow is verified)
            let _ = event_handle.notify(b"after_unsub").await;

            tokio::time::sleep(Duration::from_millis(200)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(EVENT_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();

        {
            let mut subscription = tokio::time::timeout(
                Duration::from_secs(5),
                proxy.new_subscription().eventgroup(eventgroup).subscribe(),
            )
            .await
            .expect("Subscribe timeout")
            .expect("Subscribe should succeed");

            // Receive the first event
            let event = tokio::time::timeout(Duration::from_secs(5), subscription.next())
                .await
                .expect("Event timeout");
            assert!(event.is_some());
            assert_eq!(event.unwrap().payload.as_ref(), b"before_unsub");

            // subscription dropped here - triggers StopSubscribe
        }

        // Brief wait
        tokio::time::sleep(Duration::from_millis(100)).await;

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(2000)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// MULTIPLE EVENTGROUPS
// ============================================================================

/// feat_req_recentipsd_109: Multiple eventgroups can be subscribed
#[test_log::test]
fn subscribe_multiple_eventgroups() {
    covers!(feat_req_recentipsd_109);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .start_turmoil()
                .await
                .unwrap();

            let offering = runtime
                .offer(EVENT_SERVICE_ID, InstanceId::Id(0x0001))
                .version(EVENT_SERVICE_VERSION.0, EVENT_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            tokio::time::sleep(Duration::from_millis(500)).await;

            // Send to different eventgroups with different event IDs
            let eg1 = EventgroupId::new(0x0001).unwrap();
            let eg2 = EventgroupId::new(0x0002).unwrap();
            let event_id1 = EventId::new(0x8001).unwrap();
            let event_id2 = EventId::new(0x8002).unwrap();
            let event_handle1 = offering
                .event(event_id1)
                .eventgroup(eg1)
                .create()
                .await
                .unwrap();
            let event_handle2 = offering
                .event(event_id2)
                .eventgroup(eg2)
                .create()
                .await
                .unwrap();

            event_handle1.notify(b"group1_event").await.unwrap();
            event_handle2.notify(b"group2_event").await.unwrap();

            tokio::time::sleep(Duration::from_millis(500)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(EVENT_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        // Subscribe to both eventgroups
        let eg1 = EventgroupId::new(0x0001).unwrap();
        let eg2 = EventgroupId::new(0x0002).unwrap();

        let mut sub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eg1).subscribe(),
        )
        .await
        .expect("Sub1 timeout")
        .expect("Sub1 should succeed");

        let mut sub2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eg2).subscribe(),
        )
        .await
        .expect("Sub2 timeout")
        .expect("Sub2 should succeed");

        // Receive from both subscriptions
        // With per-subscription UDP sockets, each subscription receives only events
        // for its eventgroup (proper isolation per SOME/IP spec)
        let event1 = tokio::time::timeout(Duration::from_secs(5), sub1.next())
            .await
            .expect("Event1 timeout");
        let event2 = tokio::time::timeout(Duration::from_secs(5), sub2.next())
            .await
            .expect("Event2 timeout");

        // Verify both events were received
        assert!(event1.is_some());
        assert!(event2.is_some());

        // Each subscription receives only its eventgroup's events
        assert_eq!(event1.unwrap().payload.as_ref(), b"group1_event");
        assert_eq!(event2.unwrap().payload.as_ref(), b"group2_event");

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(2000)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// EVENT ID COMPLIANCE
// ============================================================================

/// feat_req_recentip_101: Event IDs have high bit set (0x8000-0xFFFF)
#[test_log::test]
fn event_id_has_high_bit() {
    covers!(feat_req_recentip_101);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .start_turmoil()
                .await
                .unwrap();

            let offering = runtime
                .offer(EVENT_SERVICE_ID, InstanceId::Id(0x0001))
                .version(EVENT_SERVICE_VERSION.0, EVENT_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            tokio::time::sleep(Duration::from_millis(500)).await;

            let eventgroup = EventgroupId::new(0x0001).unwrap();
            // Event ID with high bit set
            let event_id = EventId::new(0x8100).unwrap();
            let event_handle = offering
                .event(event_id)
                .eventgroup(eventgroup)
                .create()
                .await
                .unwrap();

            event_handle.notify(b"data").await.unwrap();

            tokio::time::sleep(Duration::from_millis(500)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(EVENT_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        let event = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("Event timeout");

        assert!(event.is_some());
        let event = event.unwrap();
        // Event ID should have high bit set
        assert!(event.event_id.value() >= 0x8000);
        assert_eq!(event.event_id.value(), 0x8100);

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(2000)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// RPC AND EVENTS COMBINED
// ============================================================================

/// Services can handle both RPC calls and emit events
#[test_log::test]
fn mixed_rpc_and_events() {
    covers!(feat_req_recentip_103, feat_req_recentipsd_576);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    let executed = Arc::new(Mutex::new(false));
    let exec_flag = Arc::clone(&executed);

    sim.host("server", move || {
        let flag = Arc::clone(&exec_flag);
        async move {
            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .start_turmoil()
                .await
                .unwrap();

            let mut offering = runtime
                .offer(EVENT_SERVICE_ID, InstanceId::Id(0x0001))
                .version(EVENT_SERVICE_VERSION.0, EVENT_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            // Wait for subscription
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Handle events - may receive Subscribe first, then Call
            let mut call_handled = false;
            for _ in 0..5 {
                if let Some(event) = tokio::time::timeout(Duration::from_secs(5), offering.next())
                    .await
                    .ok()
                    .flatten()
                {
                    match event {
                        ServiceEvent::Call { responder, .. } => {
                            responder.reply(b"response").unwrap();
                            call_handled = true;
                            break;
                        }
                        ServiceEvent::Subscribe { .. } => {
                            // Expected - subscription arrived, continue to wait for call
                        }
                        _ => {}
                    }
                }
            }
            assert!(call_handled, "Should have handled an RPC call");

            // Emit event after RPC
            let eventgroup = EventgroupId::new(0x0001).unwrap();
            let event_id = EventId::new(0x8001).unwrap();
            let event_handle = offering
                .event(event_id)
                .eventgroup(eventgroup)
                .create()
                .await
                .unwrap();
            event_handle.notify(b"post_rpc_event").await.unwrap();

            tokio::time::sleep(Duration::from_millis(500)).await;
            *flag.lock().unwrap() = true;
            Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let proxy = runtime.find(EVENT_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        // Subscribe to events
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.new_subscription().eventgroup(eventgroup).subscribe(),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        // Make RPC call
        let method = MethodId::new(0x0001).unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, b"request"))
            .await
            .expect("RPC timeout")
            .expect("RPC should succeed");

        assert_eq!(response.payload.as_ref(), b"response");

        // Receive event
        let event = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("Event timeout");
        assert!(event.is_some());
        assert_eq!(event.unwrap().payload.as_ref(), b"post_rpc_event");

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(2000)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// MULTI-EVENTGROUP SUBSCRIPTION LIFECYCLE
// ============================================================================

/// Tests multi-eventgroup subscription lifecycle:
/// - Server offers one service with 4 eventgroups (EG1, EG2, EG3, EG4)
/// - Event1 belongs to EG1 and EG2
/// - Event2 belongs to EG3 and EG4
/// - Client subscribes to EG1+EG2 (sub1) and EG3+EG4 (sub2)
/// - Server sends Event1 and Event2
/// - Client drops sub1, server sees unsubscribe
/// - Client re-subscribes to EG1+EG2
/// - Server sends Event1 again
#[test]
fn multi_eventgroup_subscription_lifecycle() {
    covers!(feat_req_recentipsd_109, feat_req_recentipsd_433);

    configure_tracing();

    // Tracking
    let subscribe_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let unsubscribe_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let events_sub1 = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let events_sub2 = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));

    let subscribe_count_server = Arc::clone(&subscribe_count);
    let unsubscribe_count_server = Arc::clone(&unsubscribe_count);
    let events_sub1_client = Arc::clone(&events_sub1);
    let events_sub2_client = Arc::clone(&events_sub2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Channels for synchronization
    let (phase1_tx, phase1_rx) = tokio::sync::oneshot::channel::<()>();
    let (phase2_tx, phase2_rx) = tokio::sync::oneshot::channel::<()>();
    let (phase3_tx, phase3_rx) = tokio::sync::oneshot::channel::<()>();
    let (phase4_tx, phase4_rx) = tokio::sync::oneshot::channel::<()>();

    let phase1_tx = Arc::new(Mutex::new(Some(phase1_tx)));
    let phase2_tx = Arc::new(Mutex::new(Some(phase2_tx)));
    let phase3_tx = Arc::new(Mutex::new(Some(phase3_tx)));
    let phase4_tx = Arc::new(Mutex::new(Some(phase4_tx)));

    sim.host("server", move || {
        let subscribe_count = Arc::clone(&subscribe_count_server);
        let unsubscribe_count = Arc::clone(&unsubscribe_count_server);
        let phase1_tx = Arc::clone(&phase1_tx);
        let phase2_tx = Arc::clone(&phase2_tx);
        let phase3_tx = Arc::clone(&phase3_tx);
        let phase4_tx = Arc::clone(&phase4_tx);

        async move {
            let runtime = tokio::time::timeout(
                Duration::from_secs(5),
                recentip::configure()
                    .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                    .start_turmoil(),
            )
            .await
            .expect("Timeout starting UDP server runtime")
            .unwrap();

            let mut offering = tokio::time::timeout(
                Duration::from_secs(5),
                runtime
                    .offer(EVENT_SERVICE_ID, InstanceId::Id(0x0001))
                    .version(EVENT_SERVICE_VERSION.0, EVENT_SERVICE_VERSION.1)
                    .udp()
                    .start(),
            )
            .await
            .expect("Timeout starting UDP offering")
            .unwrap();

            // Create eventgroups and events
            let eg1 = EventgroupId::new(0x0001).unwrap();
            let eg2 = EventgroupId::new(0x0002).unwrap();
            let eg3 = EventgroupId::new(0x0003).unwrap();
            let eg4 = EventgroupId::new(0x0004).unwrap();

            // Event1 belongs to EG1 and EG2
            let event1_handle = offering
                .event(EventId::new(0x8001).unwrap())
                .eventgroup(eg1)
                .eventgroup(eg2)
                .create()
                .await
                .unwrap();

            // Event2 belongs to EG3 and EG4
            let event2_handle = offering
                .event(EventId::new(0x8002).unwrap())
                .eventgroup(eg3)
                .eventgroup(eg4)
                .create()
                .await
                .unwrap();

            // Wait for initial subscriptions (should be 4 unique eventgroups: EG1, EG2, EG3, EG4)
            let mut eg_subs: std::collections::HashSet<u16> = std::collections::HashSet::new();
            while eg_subs.len() < 4 {
                match tokio::time::timeout(Duration::from_secs(5), offering.next()).await {
                    Ok(Some(ServiceEvent::Subscribe { eventgroup, .. })) => {
                        tracing::info!(
                            "[server] Subscribe received for EG {:04X}",
                            eventgroup.value()
                        );
                        subscribe_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        eg_subs.insert(eventgroup.value());
                    }
                    Ok(Some(_)) => continue,
                    Ok(None) => panic!("[server] Offering stream ended unexpectedly"),
                    Err(_) => panic!(
                        "[server] Timeout waiting for subscriptions. Got: {:?}",
                        eg_subs
                    ),
                }
            }
            tracing::info!("[server] All 4 subscriptions received: {:?}", eg_subs);

            // Phase 1: Send Event1 and Event2
            event1_handle.notify(b"event1_first").await.unwrap();
            event2_handle.notify(b"event2_first").await.unwrap();
            tracing::info!("[server] Sent event1 and event2");

            // Signal phase 1 complete
            if let Some(tx) = phase1_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }

            // Wait for unsubscribe (client drops sub1 - EG1 and EG2)
            let mut unsubs_received = 0;
            while unsubs_received < 2 {
                match tokio::time::timeout(Duration::from_secs(50), offering.next()).await {
                    Ok(Some(ServiceEvent::Unsubscribe { eventgroup, .. })) => {
                        tracing::info!(
                            "[server] Unsubscribe received for EG {:04X}",
                            eventgroup.value()
                        );
                        unsubscribe_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        unsubs_received += 1;
                    }
                    Ok(Some(_)) => continue,
                    Ok(None) => panic!("[server] Offering stream ended unexpectedly"),
                    Err(_) => panic!("[server] Timeout waiting for unsubscribe"),
                }
            }
            tracing::info!("[server] Unsubscribes received for EG1 and EG2");

            // Signal phase 2 complete (unsubscribes received)
            if let Some(tx) = phase2_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }

            // Wait for re-subscription (EG1 and EG2)
            let mut resubs_received = 0;
            while resubs_received < 2 {
                match tokio::time::timeout(Duration::from_secs(5), offering.next()).await {
                    Ok(Some(ServiceEvent::Subscribe { eventgroup, .. })) => {
                        tracing::info!(
                            "[server] Re-subscribe received for EG {:04X}",
                            eventgroup.value()
                        );
                        subscribe_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        resubs_received += 1;
                    }
                    Ok(Some(_)) => continue,
                    Ok(None) => panic!("[server] Offering stream ended unexpectedly"),
                    Err(_) => panic!("[server] Timeout waiting for re-subscription"),
                }
            }
            tracing::info!("[server] Re-subscriptions received for EG1 and EG2");

            // Signal phase 3 complete (re-subscriptions received)
            if let Some(tx) = phase3_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }

            // Phase 4: Send Event1 again
            event1_handle.notify(b"event1_second").await.unwrap();
            tracing::info!("[server] Sent event1 second time");

            // Signal phase 4 complete
            if let Some(tx) = phase4_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = tokio::time::timeout(
            Duration::from_secs(5),
            recentip::configure()
                .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
                .start_turmoil(),
        )
        .await
        .expect("Timeout starting UDP client runtime")
        .unwrap();

        let proxy = runtime.find(EVENT_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");
        tracing::info!("[client] Service discovered");

        let eg1 = EventgroupId::new(0x0001).unwrap();
        let eg2 = EventgroupId::new(0x0002).unwrap();
        let eg3 = EventgroupId::new(0x0003).unwrap();
        let eg4 = EventgroupId::new(0x0004).unwrap();

        // Subscribe to EG1+EG2 (sub1)
        let mut sub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy
                .new_subscription()
                .eventgroup(eg1)
                .eventgroup(eg2)
                .subscribe(),
        )
        .await
        .expect("Timeout subscribing to EG1+EG2")
        .expect("Sub1 should succeed");
        tracing::info!("[client] Subscription 1 (EG1+EG2) established");

        // Subscribe to EG3+EG4 (sub2)
        let mut sub2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy
                .new_subscription()
                .eventgroup(eg3)
                .eventgroup(eg4)
                .subscribe(),
        )
        .await
        .expect("Timeout subscribing to EG3+EG4")
        .expect("Sub2 should succeed");
        tracing::info!("[client] Subscription 2 (EG3+EG4) established");

        // Wait for phase 1 (server sends events)
        tokio::time::timeout(Duration::from_secs(5), phase1_rx)
            .await
            .expect("Timeout waiting for phase 1 signal")
            .expect("Phase 1 signal");

        // Collect events with timeout
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        while tokio::time::Instant::now() < deadline {
            tokio::select! {
                event = sub1.next() => {
                    if let Some(e) = event {
                        tracing::info!("[client] Sub1 received: {:?}", String::from_utf8_lossy(&e.payload));
                        events_sub1_client.lock().unwrap().push(e.payload.to_vec());
                    }
                }
                event = sub2.next() => {
                    if let Some(e) = event {
                        tracing::info!("[client] Sub2 received: {:?}", String::from_utf8_lossy(&e.payload));
                        events_sub2_client.lock().unwrap().push(e.payload.to_vec());
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        // Drop sub1 to trigger unsubscribe
        tracing::info!("[client] Dropping subscription 1");
        drop(sub1);

        // Wait for phase 2 (server receives unsubscribes)
        tokio::time::timeout(Duration::from_secs(5), phase2_rx)
            .await
            .expect("Timeout waiting for phase 2 signal")
            .expect("Phase 2 signal");

        // Re-subscribe to EG1+EG2
        let mut sub1_new = tokio::time::timeout(
            Duration::from_secs(5),
            proxy
                .new_subscription()
                .eventgroup(eg1)
                .eventgroup(eg2)
                .subscribe(),
        )
        .await
        .expect("Timeout re-subscribing to EG1+EG2")
        .expect("Sub1 re-subscription should succeed");
        tracing::info!("[client] Subscription 1 (EG1+EG2) re-established");

        // Wait for phase 3 (server receives re-subscriptions)
        tokio::time::timeout(Duration::from_secs(2), phase3_rx)
            .await
            .expect("Timeout waiting for phase 3 signal")
            .expect("Phase 3 signal");

        // Wait for phase 4 (server sends event1 again)
        tokio::time::timeout(Duration::from_secs(2), phase4_rx)
            .await
            .expect("Timeout waiting for phase 4 signal")
            .expect("Phase 4 signal");

        // Collect event from new sub1
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(1)).await;
            match tokio::time::timeout(Duration::from_millis(100), sub1_new.next()).await {
                Ok(Some(e)) => {
                    tracing::info!(
                        "[client] Sub1 (new) received: {:?}",
                        String::from_utf8_lossy(&e.payload)
                    );
                    events_sub1_client.lock().unwrap().push(e.payload.to_vec());
                    break;
                }
                Ok(None) => break,
                Err(_) => continue,
            }
        }

        // Keep sub2 alive until end
        drop(sub2);
        drop(sub1_new);

        Ok(())
    });

    sim.run().unwrap();

    // Verify results
    let sub_count = subscribe_count.load(std::sync::atomic::Ordering::SeqCst);
    let unsub_count = unsubscribe_count.load(std::sync::atomic::Ordering::SeqCst);
    let sub1_events = events_sub1.lock().unwrap();
    let sub2_events = events_sub2.lock().unwrap();

    eprintln!("Subscribe count: {}", sub_count);
    eprintln!("Unsubscribe count: {}", unsub_count);
    eprintln!("Sub1 events: {:?}", *sub1_events);
    eprintln!("Sub2 events: {:?}", *sub2_events);

    // At least 6 subscribes (4 initial + 2 re-subscribe), may have some duplicates
    assert!(
        sub_count >= 6,
        "Should have at least 6 subscribe events (4 initial + 2 re-subscribe), got {}",
        sub_count
    );
    // Exactly 2 unsubscribes (EG1 and EG2)
    assert_eq!(
        unsub_count, 2,
        "Should have 2 unsubscribe events (EG1 and EG2), got {}",
        unsub_count,
    );

    // Sub1 should have received event1_first at least once (before drop) and event1_second at least once (after re-subscribe)
    // Note: may receive duplicates because event is in multiple eventgroups
    let has_event1_first = sub1_events.iter().any(|e| e == b"event1_first");
    let has_event1_second = sub1_events.iter().any(|e| e == b"event1_second");
    assert!(
        has_event1_first,
        "Sub1 should receive event1_first before drop"
    );
    assert!(
        has_event1_second,
        "Sub1 should receive event1_second after re-subscribe"
    );

    // Sub2 should have received event2 once
    let sub2_event2_count = sub2_events
        .iter()
        .filter(|e| e.starts_with(b"event2"))
        .count();
    assert_eq!(sub2_event2_count, 1, "Sub2 should receive event2 once");
}

/// TCP version: Tests multi-eventgroup subscription lifecycle over TCP:
/// - Server offers one service with 4 eventgroups (EG1, EG2, EG3, EG4) via TCP
/// - Event1 belongs to EG1 and EG2
/// - Event2 belongs to EG3 and EG4
/// - Client subscribes to EG1+EG2 (sub1) and EG3+EG4 (sub2) via TCP
/// - Server sends Event1 and Event2
/// - Client drops sub1, server sees unsubscribe
/// - Client re-subscribes to EG1+EG2
/// - Server sends Event1 again
///
/// This validates TCP connection slot-based reuse: when sub1 is dropped and resub1 is created,
/// the library reuses existing TCP connections based on slot assignment.
#[test]
fn multi_eventgroup_subscription_lifecycle_tcp() {
    covers!(feat_req_recentipsd_109, feat_req_recentipsd_433);

    configure_tracing();

    // Tracking
    let subscribe_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let unsubscribe_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let events_sub1 = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let events_sub2 = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));

    let subscribe_count_server = Arc::clone(&subscribe_count);
    let unsubscribe_count_server = Arc::clone(&unsubscribe_count);
    let events_sub1_client = Arc::clone(&events_sub1);
    let events_sub2_client = Arc::clone(&events_sub2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Channels for synchronization
    let (phase1_tx, phase1_rx) = tokio::sync::oneshot::channel::<()>();
    let (phase2_tx, phase2_rx) = tokio::sync::oneshot::channel::<()>();
    let (phase2b_tx, phase2b_rx) = tokio::sync::oneshot::channel::<()>(); // Event2 while sub1 dropped
    let (phase3_tx, phase3_rx) = tokio::sync::oneshot::channel::<()>();
    let (phase4_tx, phase4_rx) = tokio::sync::oneshot::channel::<()>();
    let (phase5_tx, phase5_rx) = tokio::sync::oneshot::channel::<()>(); // Final Event2

    let phase1_tx = Arc::new(Mutex::new(Some(phase1_tx)));
    let phase2_tx = Arc::new(Mutex::new(Some(phase2_tx)));
    let phase2b_tx = Arc::new(Mutex::new(Some(phase2b_tx)));
    let phase3_tx = Arc::new(Mutex::new(Some(phase3_tx)));
    let phase4_tx = Arc::new(Mutex::new(Some(phase4_tx)));
    let phase5_tx = Arc::new(Mutex::new(Some(phase5_tx)));

    sim.host("server", move || {
        let subscribe_count = Arc::clone(&subscribe_count_server);
        let unsubscribe_count = Arc::clone(&unsubscribe_count_server);
        let phase1_tx = Arc::clone(&phase1_tx);
        let phase2_tx = Arc::clone(&phase2_tx);
        let phase2b_tx = Arc::clone(&phase2b_tx);
        let phase3_tx = Arc::clone(&phase3_tx);
        let phase4_tx = Arc::clone(&phase4_tx);
        let phase5_tx = Arc::clone(&phase5_tx);

        async move {
            let runtime = tokio::time::timeout(
                Duration::from_secs(5),
                recentip::configure()
                    .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                    .start_turmoil(),
            )
            .await
            .expect("Timeout starting TCP server runtime")
            .unwrap();

            // Offer service via TCP
            let mut offering = tokio::time::timeout(
                Duration::from_secs(5),
                runtime
                    .offer(EVENT_SERVICE_ID, InstanceId::Id(0x0001))
                    .version(EVENT_SERVICE_VERSION.0, EVENT_SERVICE_VERSION.1)
                    .tcp()
                    .start(),
            )
            .await
            .expect("Timeout starting TCP offering")
            .unwrap();

            // Create eventgroups and events
            let eg1 = EventgroupId::new(0x0001).unwrap();
            let eg2 = EventgroupId::new(0x0002).unwrap();
            let eg3 = EventgroupId::new(0x0003).unwrap();
            let eg4 = EventgroupId::new(0x0004).unwrap();

            // Event1 belongs to EG1 and EG2
            let event1_handle = offering
                .event(EventId::new(0x8001).unwrap())
                .eventgroup(eg1)
                .eventgroup(eg2)
                .create()
                .await
                .unwrap();

            // Event2 belongs to EG3 and EG4
            let event2_handle = offering
                .event(EventId::new(0x8002).unwrap())
                .eventgroup(eg3)
                .eventgroup(eg4)
                .create()
                .await
                .unwrap();

            // Wait for initial subscriptions (should be 4 unique eventgroups: EG1, EG2, EG3, EG4)
            let mut eg_subs: std::collections::HashSet<u16> = std::collections::HashSet::new();
            let subscription_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            while eg_subs.len() < 4 {
                if tokio::time::Instant::now() > subscription_deadline {
                    panic!(
                        "[tcp_server] Overall timeout waiting for subscriptions. Got: {:?}",
                        eg_subs
                    );
                }
                match tokio::time::timeout(Duration::from_secs(2), offering.next()).await {
                    Ok(Some(ServiceEvent::Subscribe { eventgroup, .. })) => {
                        tracing::info!(
                            "[tcp_server] Subscribe received for EG {:04X}",
                            eventgroup.value()
                        );
                        subscribe_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        eg_subs.insert(eventgroup.value());
                    }
                    Ok(Some(_)) => continue,
                    Ok(None) => panic!("[tcp_server] Offering stream ended unexpectedly"),
                    Err(_) => panic!(
                        "[tcp_server] Timeout waiting for subscriptions. Got: {:?}",
                        eg_subs
                    ),
                }
            }
            tracing::info!(
                "[tcp_server] All 4 TCP subscriptions received: {:?}",
                eg_subs
            );

            // Phase 1: Send Event1 and Event2
            event1_handle.notify(b"tcp_event1_first").await.unwrap();
            event2_handle.notify(b"tcp_event2_first").await.unwrap();
            tracing::info!("[tcp_server] Sent tcp_event1 and tcp_event2");

            // Signal phase 1 complete
            if let Some(tx) = phase1_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }

            // Wait for unsubscribe (client drops sub1 - EG1 and EG2)
            let mut unsubs_received = 0;
            let unsubscribe_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            while unsubs_received < 2 {
                if tokio::time::Instant::now() > unsubscribe_deadline {
                    panic!(
                        "[tcp_server] Overall timeout waiting for unsubscribes. Got: {}",
                        unsubs_received
                    );
                }
                match tokio::time::timeout(Duration::from_secs(5), offering.next()).await {
                    Ok(Some(ServiceEvent::Unsubscribe { eventgroup, .. })) => {
                        tracing::info!(
                            "[tcp_server] Unsubscribe received for EG {:04X}",
                            eventgroup.value()
                        );
                        unsubscribe_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        unsubs_received += 1;
                    }
                    Ok(Some(_)) => continue,
                    Ok(None) => panic!("[tcp_server] Offering stream ended unexpectedly"),
                    Err(_) => panic!("[tcp_server] Timeout waiting for unsubscribe"),
                }
            }
            tracing::info!("[tcp_server] Unsubscribes received for EG1 and EG2");

            // Signal phase 2 complete (unsubscribes received)
            if let Some(tx) = phase2_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }

            // Phase 2b: Send Event2 while sub1 is dropped (sub2 should still receive it)
            event2_handle
                .notify(b"tcp_event2_while_sub1_dropped")
                .await
                .unwrap();
            tracing::info!("[tcp_server] Sent tcp_event2 while sub1 is dropped");

            // Signal phase 2b complete
            if let Some(tx) = phase2b_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }

            // Wait for re-subscription (EG1 and EG2 specifically)
            let mut resubs_received = std::collections::HashSet::new();
            let resubscribe_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            while !resubs_received.contains(&eg1) || !resubs_received.contains(&eg2) {
                if tokio::time::Instant::now() > resubscribe_deadline {
                    panic!(
                        "[tcp_server] Overall timeout waiting for re-subscriptions. Got: {:?}",
                        resubs_received
                    );
                }
                match tokio::time::timeout(Duration::from_secs(2), offering.next()).await {
                    Ok(Some(ServiceEvent::Subscribe { eventgroup, .. })) => {
                        tracing::info!(
                            "[tcp_server] Re-subscribe received for EG {:04X}",
                            eventgroup.value()
                        );
                        subscribe_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        if eventgroup == eg1 || eventgroup == eg2 {
                            resubs_received.insert(eventgroup);
                        }
                    }
                    Ok(Some(_)) => continue,
                    Ok(None) => panic!("[tcp_server] Offering stream ended unexpectedly"),
                    Err(_) => panic!("[tcp_server] Timeout waiting for re-subscription"),
                }
            }
            tracing::info!("[tcp_server] Re-subscriptions received for EG1 and EG2");

            // Signal phase 3 complete (re-subscriptions received)
            if let Some(tx) = phase3_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }

            tokio::time::sleep(Duration::from_millis(1000)).await;

            // Phase 4: Send Event1 and Event2 after re-subscribe
            event1_handle.notify(b"tcp_event1_second").await.unwrap();
            event2_handle
                .notify(b"tcp_event2_after_resub")
                .await
                .unwrap();
            tracing::info!("[tcp_server] Sent tcp_event1 and tcp_event2 after re-subscribe");

            // Signal phase 4 complete
            if let Some(tx) = phase4_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }

            // Small delay for events to be received
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Signal phase 5 complete (all events sent)
            if let Some(tx) = phase5_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }

            tokio::time::sleep(Duration::from_millis(5000)).await;
            Ok(())
        }
    });

    sim.client("client", async move {
        let runtime = tokio::time::timeout(
            Duration::from_secs(5),
            recentip::configure()
                .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
                .preferred_transport(recentip::Transport::Tcp)
                .start_turmoil(),
        )
        .await
        .expect("Timeout starting client runtime")
        .unwrap();

        let proxy = runtime.find(EVENT_SERVICE_ID);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .expect("Discovery timeout")
            .expect("Service available");
        tracing::info!("[tcp_client] Service discovered via TCP");

        let eg1 = EventgroupId::new(0x0001).unwrap();
        let eg2 = EventgroupId::new(0x0002).unwrap();
        let eg3 = EventgroupId::new(0x0003).unwrap();
        let eg4 = EventgroupId::new(0x0004).unwrap();

        // Subscribe to EG1+EG2 (sub1) via TCP
        let mut sub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy
                .new_subscription()
                .eventgroup(eg1)
                .eventgroup(eg2)
                .subscribe(),
        )
        .await
        .expect("Timeout subscribing to EG1+EG2")
        .expect("TCP Sub1 should succeed");
        tracing::info!("[tcp_client] TCP Subscription 1 (EG1+EG2) established");

        // Subscribe to EG3+EG4 (sub2) via TCP
        let mut sub2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy
                .new_subscription()
                .eventgroup(eg3)
                .eventgroup(eg4)
                .subscribe(),
        )
        .await
        .expect("Timeout subscribing to EG3+EG4")
        .expect("TCP Sub2 should succeed");
        tracing::info!("[tcp_client] TCP Subscription 2 (EG3+EG4) established");

        // Wait for phase 1 (server sends events)
        tokio::time::timeout(Duration::from_secs(5), phase1_rx)
            .await
            .expect("Timeout waiting for phase 1 signal")
            .expect("Phase 1 signal");

        // Collect events with timeout
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        while tokio::time::Instant::now() < deadline {
            tokio::select! {
                event = sub1.next() => {
                    if let Some(e) = event {
                        tracing::info!("[tcp_client] Sub1 received: {:?}", String::from_utf8_lossy(&e.payload));
                        events_sub1_client.lock().unwrap().push(e.payload.to_vec());
                    }
                }
                event = sub2.next() => {
                    if let Some(e) = event {
                        tracing::info!("[tcp_client] Sub2 received: {:?}", String::from_utf8_lossy(&e.payload));
                        events_sub2_client.lock().unwrap().push(e.payload.to_vec());
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        // Drop sub1 to trigger unsubscribe
        tracing::info!("[tcp_client] Dropping TCP subscription 1");
        drop(sub1);

        // Wait for phase 2 (server receives unsubscribes)
        tokio::time::timeout(Duration::from_secs(3), phase2_rx)
            .await
            .expect("Timeout waiting for phase 2 signal")
            .expect("Phase 2 signal");

        // Wait for phase 2b (server sends Event2 while sub1 is dropped)
        tokio::time::timeout(Duration::from_secs(3), phase2b_rx)
            .await
            .expect("Timeout waiting for phase 2b signal")
            .expect("Phase 2b signal");

        // Collect Event2 that was sent while sub1 was dropped
        let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(1)).await;
            match tokio::time::timeout(Duration::from_millis(50), sub2.next()).await {
                Ok(Some(e)) => {
                    eprintln!("[tcp_client] Sub2 received while sub1 dropped: {:?}", String::from_utf8_lossy(&e.payload));
                    events_sub2_client.lock().unwrap().push(e.payload.to_vec());
                }
                Ok(None) => break,
                Err(_) => continue,
            }
        }

        // Re-subscribe to EG1+EG2 via TCP
        let mut sub1_new = tokio::time::timeout(
            Duration::from_secs(5),
            proxy
                .new_subscription()
                .eventgroup(eg1)
                .eventgroup(eg2)
                .subscribe(),
        )
        .await
        .expect("Timeout re-subscribing to EG1+EG2")
        .expect("TCP Sub1 re-subscription should succeed");
        tracing::info!("[tcp_client] TCP Subscription 1 (EG1+EG2) re-established");

        // Wait for phase 3 (server receives re-subscriptions)
        tokio::time::timeout(Duration::from_secs(3), phase3_rx)
            .await
            .expect("Timeout waiting for phase 3 signal")
            .expect("Phase 3 signal");

        // Wait for phase 4 (server sends event1 and event2 again)
        tokio::time::timeout(Duration::from_secs(3), phase4_rx)
            .await
            .expect("Timeout waiting for phase 4 signal")
            .expect("Phase 4 signal");

        // Wait for phase 5 (all events sent)
        tokio::time::timeout(Duration::from_secs(3), phase5_rx)
            .await
            .expect("Timeout waiting for phase 5 signal")
            .expect("Phase 5 signal");

        // Collect events from both subscriptions
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        while tokio::time::Instant::now() < deadline {
            tokio::select! {
                event = sub1_new.next() => {
                    if let Some(e) = event {
                        tracing::info!("[tcp_client] Sub1 (new) received: {:?}", String::from_utf8_lossy(&e.payload));
                        events_sub1_client.lock().unwrap().push(e.payload.to_vec());
                    } else {
                        tracing::debug!("[tcp_client] Sub1 (new) stream ended");
                    }
                }
                event = sub2.next() => {
                    if let Some(e) = event {
                        tracing::info!("[tcp_client] Sub2 received after resub: {:?}", String::from_utf8_lossy(&e.payload));
                        events_sub2_client.lock().unwrap().push(e.payload.to_vec());
                    } else {
                        tracing::debug!("[tcp_client] Sub2 stream ended");
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    tracing::debug!("[tcp_client] No more events received in this interval");
                }
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        // Keep sub2 alive until end
        drop(sub2);
        drop(sub1_new);

        Ok(())
    });

    sim.run().unwrap();

    // Verify results
    let sub_count = subscribe_count.load(std::sync::atomic::Ordering::SeqCst);
    let unsub_count = unsubscribe_count.load(std::sync::atomic::Ordering::SeqCst);
    let sub1_events = events_sub1.lock().unwrap();
    let sub2_events = events_sub2.lock().unwrap();

    eprintln!("TCP Subscribe count: {}", sub_count);
    eprintln!("TCP Unsubscribe count: {}", unsub_count);
    eprintln!(
        "TCP Sub1 events: {:?}",
        sub1_events
            .iter()
            .map(|e| String::from_utf8_lossy(e).to_string())
            .collect::<Vec<_>>()
    );
    eprintln!(
        "TCP Sub2 events: {:?}",
        sub2_events
            .iter()
            .map(|e| String::from_utf8_lossy(e).to_string())
            .collect::<Vec<_>>()
    );

    // At least 6 subscribes (4 initial + 2 re-subscribe), may have some duplicates
    assert!(
        sub_count >= 6,
        "Should have at least 6 TCP subscribe events (4 initial + 2 re-subscribe), got {}",
        sub_count
    );
    // Exactly 2 unsubscribes (EG1 and EG2)
    assert_eq!(
        unsub_count, 2,
        "Should have 2 TCP unsubscribe events (EG1 and EG2), got {}",
        unsub_count,
    );

    // Sub1 should have received tcp_event1_first at least once (before drop) and tcp_event1_second at least once (after re-subscribe)
    let has_event1_first = sub1_events.iter().any(|e| e == b"tcp_event1_first");
    let has_event1_second = sub1_events.iter().any(|e| e == b"tcp_event1_second");
    assert!(
        has_event1_first,
        "TCP Sub1 should receive tcp_event1_first before drop. Got: {:?}",
        *sub1_events
    );
    assert!(
        has_event1_second,
        "TCP Sub1 should receive tcp_event1_second after re-subscribe. Got: {:?}",
        *sub1_events
    );

    // Sub1 should NOT receive Event2 (it's only subscribed to EG1+EG2, not EG3+EG4)
    let sub1_has_any_event2 = sub1_events.iter().any(|e| {
        let s = String::from_utf8_lossy(e);
        s.contains("event2")
    });
    assert!(
        !sub1_has_any_event2,
        "TCP Sub1 should NOT receive any Event2 (subscribed to EG1+EG2, not EG3+EG4). Got: {:?}",
        sub1_events
            .iter()
            .map(|e| String::from_utf8_lossy(e).to_string())
            .collect::<Vec<_>>()
    );

    // Sub2 should have received all three Event2 sends:
    // - tcp_event2_first (initial)
    // - tcp_event2_while_sub1_dropped (while sub1 was dropped)
    // - tcp_event2_after_resub (after re-subscribe)
    let has_event2_first = sub2_events.iter().any(|e| e == b"tcp_event2_first");
    let has_event2_while_dropped = sub2_events
        .iter()
        .any(|e| e == b"tcp_event2_while_sub1_dropped");
    let has_event2_after_resub = sub2_events.iter().any(|e| e == b"tcp_event2_after_resub");

    assert!(
        has_event2_first,
        "TCP Sub2 should receive tcp_event2_first. Got: {:?}",
        *sub2_events
    );
    assert!(
        has_event2_while_dropped,
        "TCP Sub2 should receive tcp_event2_while_sub1_dropped (routing unaffected by sub1 drop). Got: {:?}",
        *sub2_events
    );
    assert!(
        has_event2_after_resub,
        "TCP Sub2 should receive tcp_event2_after_resub. Got: {:?}",
        *sub2_events
    );
}
