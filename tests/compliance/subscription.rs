//! Subscription and Eventgroup Compliance Tests (Async/Turmoil)
//!
//! Tests the publish/subscribe (Pub/Sub) behavior per SOME/IP-SD specification.
//!
//! Reference: someip-sd.rst (Eventgroup entries and subscription handling)

use someip_runtime::prelude::*;
use someip_runtime::runtime::Runtime;
use someip_runtime::handle::ServiceEvent;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

/// Type alias for turmoil-based runtime
type TurmoilRuntime = Runtime<turmoil::net::UdpSocket>;

/// Test service definition
struct EventService;

impl Service for EventService {
    const SERVICE_ID: u16 = 0x1234;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}

// ============================================================================
// SUBSCRIPTION FLOW
// ============================================================================

/// feat_req_recentipsd_576: Subscribe entry type (0x06)
/// feat_req_recentipsd_109: Eventgroup Entry is 16 bytes
///
/// Client can subscribe to an eventgroup and receive events.
///
/// TODO: This test is ignored because the runtime does not yet advertise
/// eventgroups in Service Discovery. Once eventgroup advertisement is implemented,
/// remove the #[ignore] attribute.
#[test]
#[ignore]
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
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let offering = runtime
            .offer::<EventService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Wait for subscription
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send events
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();

        offering.notify(eventgroup, event_id, b"event1").await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        offering.notify(eventgroup, event_id, b"event2").await.unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;
        *flag.lock().unwrap() = true;
        Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<EventService>(InstanceId::Any);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout");

        // Subscribe to eventgroup
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(eventgroup),
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
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

/// feat_req_recentipsd_576: SubscribeAck entry type (0x07)
///
/// Server must acknowledge subscriptions with SubscribeAck.
///
/// TODO: This test is ignored because the runtime does not yet advertise
/// eventgroups in Service Discovery. Once eventgroup advertisement is implemented,
/// remove the #[ignore] attribute.
#[test]
#[ignore]
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
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let _offering = runtime
            .offer::<EventService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(1000)).await;
        *flag.lock().unwrap() = true;
        Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<EventService>(InstanceId::Any);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        
        // Subscribe should succeed (implying SubscribeAck was received)
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(eventgroup),
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
///
/// TODO: This test is ignored because the runtime does not yet advertise
/// eventgroups in Service Discovery. Once eventgroup advertisement is implemented,
/// remove the #[ignore] attribute.
#[test]
#[ignore]
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
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let offering = runtime
            .offer::<EventService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Wait for subscribe
        tokio::time::sleep(Duration::from_millis(500)).await;

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        
        // Send an event - should be delivered
        offering.notify(eventgroup, event_id, b"before_unsub").await.unwrap();

        // Wait for unsubscribe
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send another event - should NOT be delivered (client unsubscribed)
        // (We can't directly test this without packet inspection, but the flow is verified)
        let _ = offering.notify(eventgroup, event_id, b"after_unsub").await;

        tokio::time::sleep(Duration::from_millis(200)).await;
        *flag.lock().unwrap() = true;
        Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<EventService>(InstanceId::Any);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        
        {
            let mut subscription = tokio::time::timeout(
                Duration::from_secs(5),
                proxy.subscribe(eventgroup),
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
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// MULTIPLE EVENTGROUPS
// ============================================================================

/// feat_req_recentipsd_109: Multiple eventgroups can be subscribed
///
/// TODO: This test is ignored because the runtime does not yet advertise
/// eventgroups in Service Discovery. Once eventgroup advertisement is implemented,
/// remove the #[ignore] attribute.
#[test]
#[ignore]
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
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let offering = runtime
            .offer::<EventService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send to different eventgroups
        let eg1 = EventgroupId::new(0x0001).unwrap();
        let eg2 = EventgroupId::new(0x0002).unwrap();
        let event_id = EventId::new(0x8001).unwrap();

        offering.notify(eg1, event_id, b"group1_event").await.unwrap();
        offering.notify(eg2, event_id, b"group2_event").await.unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;
        *flag.lock().unwrap() = true;
        Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<EventService>(InstanceId::Any);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout");

        // Subscribe to both eventgroups
        let eg1 = EventgroupId::new(0x0001).unwrap();
        let eg2 = EventgroupId::new(0x0002).unwrap();

        let mut sub1 = tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eg1))
            .await
            .expect("Sub1 timeout")
            .expect("Sub1 should succeed");

        let mut sub2 = tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eg2))
            .await
            .expect("Sub2 timeout")
            .expect("Sub2 should succeed");

        // Receive from both
        let event1 = tokio::time::timeout(Duration::from_secs(5), sub1.next())
            .await
            .expect("Event1 timeout");
        let event2 = tokio::time::timeout(Duration::from_secs(5), sub2.next())
            .await
            .expect("Event2 timeout");

        assert!(event1.is_some());
        assert!(event2.is_some());
        assert_eq!(event1.unwrap().payload.as_ref(), b"group1_event");
        assert_eq!(event2.unwrap().payload.as_ref(), b"group2_event");

        Ok(())
    });

    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// EVENT ID COMPLIANCE
// ============================================================================

/// feat_req_recentip_101: Event IDs have high bit set (0x8000-0xFFFF)
///
/// TODO: This test is ignored because the runtime does not yet advertise
/// eventgroups in Service Discovery. Once eventgroup advertisement is implemented,
/// remove the #[ignore] attribute.
#[test]
#[ignore]
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
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let offering = runtime
            .offer::<EventService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        // Event ID with high bit set
        let event_id = EventId::new(0x8100).unwrap();

        offering.notify(eventgroup, event_id, b"data").await.unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;
        *flag.lock().unwrap() = true;
        Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<EventService>(InstanceId::Any);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(eventgroup),
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
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}

// ============================================================================
// RPC AND EVENTS COMBINED
// ============================================================================

/// Services can handle both RPC calls and emit events
///
/// TODO: This test is ignored because the runtime does not yet advertise
/// eventgroups in Service Discovery. Once eventgroup advertisement is implemented,
/// remove the #[ignore] attribute.
#[test]
#[ignore]
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
        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer::<EventService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Wait for subscription
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Handle one RPC call, then emit event
        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::Call { responder, .. } => {
                    responder.reply(b"response").await.unwrap();
                }
                _ => panic!("Expected Call"),
            }
        }

        // Emit event after RPC
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();
        offering.notify(eventgroup, event_id, b"post_rpc_event").await.unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;
        *flag.lock().unwrap() = true;
        Ok(())
        }
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::default();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<EventService>(InstanceId::Any);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout");

        // Subscribe to events
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(eventgroup),
        )
        .await
        .expect("Subscribe timeout")
        .expect("Subscribe should succeed");

        // Make RPC call
        let method = MethodId::new(0x0001);
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(method, b"request"),
        )
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
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(*executed.lock().unwrap(), "Test should have executed");
}
