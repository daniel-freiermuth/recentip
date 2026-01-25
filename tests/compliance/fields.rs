//! Field Operations Compliance Tests
//!
//! Fields are combinations of getters, setters, and notifiers per SOME/IP spec.
//!
//! Key requirements tested:
//! - feat_req_someip_631: Field is combination of getter/setter/notifier
//! - feat_req_someip_632: Field without getter/setter/notifier shall not exist
//! - feat_req_someip_633: Getter is request/response with empty request payload
//! - feat_req_someip_634: Setter is request/response with value as request payload
//! - feat_req_someip_635: Notifier sends notification event with updated value

use recentip::handle::ServiceEvent;
use recentip::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

/// Type alias for turmoil-based runtime

const TEST_SERVICE_ID: u16 = 0x1234;
const TEST_SERVICE_VERSION: (u8, u32) = (1, 0);

// ============================================================================
// Field Getter Tests
// ============================================================================

/// feat_req_someip_633: Getter has empty request payload, value in response
///
/// The getter of a field shall be a request/response call that has an empty
/// payload for the request and the current value as payload of the response.
#[test_log::test]
fn field_getter_empty_request_payload() {
    covers!(feat_req_someip_633);

    let server_called = Arc::new(Mutex::new(false));
    let client_called = Arc::new(Mutex::new(false));
    let server_flag = Arc::clone(&server_called);
    let client_flag = Arc::clone(&client_called);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Server offers service with field getter
    sim.host("server", move || {
        let flag = Arc::clone(&server_flag);
        async move {
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

            // Handle getter request - should have empty payload
            if let Some(event) = offering.next().await {
                if let ServiceEvent::Call {
                    method,
                    payload,
                    responder,
                    ..
                } = event
                {
                    assert_eq!(
                        method,
                        MethodId::new(0x0001).unwrap(),
                        "Expected getter method"
                    );
                    assert!(
                        payload.is_empty(),
                        "Getter request should have empty payload (feat_req_someip_633)"
                    );

                    // Respond with current field value
                    responder.reply(b"field_value").unwrap();
                    *flag.lock().unwrap() = true;
                }
            }

            tokio::time::sleep(Duration::from_millis(300)).await;
            Ok(())
        }
    });

    // Client calls getter
    sim.host("client", || {
        let flag = Arc::clone(&client_flag);
        async move {
            tokio::time::sleep(Duration::from_millis(300)).await;

            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
                .start_turmoil()
                .await
                .unwrap();

            let proxy = runtime.find(TEST_SERVICE_ID);
            let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
                .await
                .expect("Discovery timeout")
                .expect("Service available");

            // Call getter with empty payload
            let getter_method = MethodId::new(0x0001).unwrap();
            let response = proxy.call(getter_method, b"").await.unwrap();

            // Verify we got the field value
            assert_eq!(response.payload.as_ref(), b"field_value");
            *flag.lock().unwrap() = true;

            tokio::time::sleep(Duration::from_millis(300)).await;
            Ok(())
        }
    });

    // Add a driver client to ensure simulation runs
    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    });

    sim.run().unwrap();

    // Verify both server and client actually executed
    assert!(
        *server_called.lock().unwrap(),
        "Server code should have executed"
    );
    assert!(
        *client_called.lock().unwrap(),
        "Client code should have executed"
    );
}

/// feat_req_someip_633: Getter returns current value in response
#[test_log::test]
fn field_getter_returns_current_value() {
    covers!(feat_req_someip_633);

    let server_called = Arc::new(Mutex::new(false));
    let client_called = Arc::new(Mutex::new(false));
    let server_flag = Arc::clone(&server_called);
    let client_flag = Arc::clone(&client_called);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let flag = Arc::clone(&server_flag);
        async move {
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

            // Simulate field with current temperature value
            let current_temperature: u16 = 25;

            if let Some(event) = offering.next().await {
                if let ServiceEvent::Call { responder, .. } = event {
                    // Return current field value
                    responder.reply(&current_temperature.to_be_bytes()).unwrap();
                    *flag.lock().unwrap() = true;
                }
            }

            tokio::time::sleep(Duration::from_millis(300)).await;
            Ok(())
        }
    });

    sim.host("client", || {
        let flag = Arc::clone(&client_flag);
        async move {
            tokio::time::sleep(Duration::from_millis(300)).await;

            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
                .start_turmoil()
                .await
                .unwrap();

            let proxy = runtime.find(TEST_SERVICE_ID);
            let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
                .await
                .expect("Discovery timeout")
                .expect("Service available");

            let response = proxy
                .call(MethodId::new(0x0001).unwrap(), b"")
                .await
                .unwrap();

            // Verify we got the temperature value
            assert_eq!(
                u16::from_be_bytes([response.payload[0], response.payload[1]]),
                25,
                "Should receive current field value"
            );
            *flag.lock().unwrap() = true;

            Ok(())
        }
    });

    // Add a driver client to ensure simulation runs
    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(
        *server_called.lock().unwrap(),
        "Server should have executed"
    );
    assert!(
        *client_called.lock().unwrap(),
        "Client should have executed"
    );
}

// ============================================================================
// Field Setter Tests
// ============================================================================

/// feat_req_someip_634: Setter has desired value in request payload
///
/// The setter of a field shall be a request/response call that has the
/// desired value as payload for the request.
#[test_log::test]
fn field_setter_sends_value_in_request() {
    covers!(feat_req_someip_634);

    let server_called = Arc::new(Mutex::new(false));
    let client_called = Arc::new(Mutex::new(false));
    let server_flag = Arc::clone(&server_called);
    let client_flag = Arc::clone(&client_called);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let flag = Arc::clone(&server_flag);
        async move {
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

            if let Some(event) = offering.next().await {
                if let ServiceEvent::Call {
                    method,
                    payload,
                    responder,
                    ..
                } = event
                {
                    assert_eq!(
                        method,
                        MethodId::new(0x0002).unwrap(),
                        "Expected setter method"
                    );

                    // Verify payload contains the new value
                    assert_eq!(payload.len(), 2, "Setter should have value payload");
                    let new_value = u16::from_be_bytes([payload[0], payload[1]]);
                    assert_eq!(
                        new_value, 42,
                        "Setter payload should contain new value (feat_req_someip_634)"
                    );

                    // Acknowledge setter
                    responder.reply(b"").unwrap();
                    // Allow response to be transmitted before task exits
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    *flag.lock().unwrap() = true;
                }
            }

            Ok(())
        }
    });

    sim.host("client", || {
        let flag = Arc::clone(&client_flag);
        async move {
            tokio::time::sleep(Duration::from_millis(300)).await;

            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
                .start_turmoil()
                .await
                .unwrap();

            let proxy = runtime.find(TEST_SERVICE_ID);
            let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
                .await
                .expect("Discovery timeout")
                .expect("Service available");

            // Set field to new value
            let setter_method = MethodId::new(0x0002).unwrap();
            let new_value = 42u16.to_be_bytes();
            let response = proxy.call(setter_method, &new_value).await.unwrap();

            assert!(
                response.payload.is_empty(),
                "Setter response typically empty"
            );
            *flag.lock().unwrap() = true;

            Ok(())
        }
    });

    // Add a driver client to ensure simulation runs
    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(
        *server_called.lock().unwrap(),
        "Server should have executed"
    );
    assert!(
        *client_called.lock().unwrap(),
        "Client should have executed"
    );
}

/// feat_req_someip_634: Setter is request/response (not fire&forget)
#[test_log::test]
fn field_setter_gets_response() {
    covers!(feat_req_someip_634);

    let server_called = Arc::new(Mutex::new(false));
    let client_called = Arc::new(Mutex::new(false));
    let server_flag = Arc::clone(&server_called);
    let client_flag = Arc::clone(&client_called);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let flag = Arc::clone(&server_flag);
        async move {
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

            if let Some(event) = offering.next().await {
                if let ServiceEvent::Call { responder, .. } = event {
                    // Setter must send response (not fire-and-forget)
                    responder.reply(b"").unwrap();
                    // Allow response to be transmitted before task exits
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    *flag.lock().unwrap() = true;
                }
            }

            Ok(())
        }
    });

    sim.host("client", || {
        let flag = Arc::clone(&client_flag);
        async move {
            tokio::time::sleep(Duration::from_millis(300)).await;

            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
                .start_turmoil()
                .await
                .unwrap();

            let proxy = runtime.find(TEST_SERVICE_ID);
            let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
                .await
                .expect("Discovery timeout")
                .expect("Service available");

            // Call setter
            let setter_method = MethodId::new(0x0002).unwrap();
            let result = proxy.call(setter_method, &[1, 2, 3, 4]).await;

            // Should receive response (proving it's request/response, not fire-and-forget)
            assert!(
                result.is_ok(),
                "Setter should receive response (feat_req_someip_634)"
            );
            *flag.lock().unwrap() = true;

            tokio::time::sleep(Duration::from_millis(300)).await;
            Ok(())
        }
    });

    // Add a driver client to ensure simulation runs
    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(
        *server_called.lock().unwrap(),
        "Server should have executed"
    );
    assert!(
        *client_called.lock().unwrap(),
        "Client should have executed"
    );
}

// ============================================================================
// Field Notifier Tests
// ============================================================================

/// feat_req_someip_635: Notifier sends notification event with updated value
///
/// The notifier shall send a notification event message that communicates
/// the updated value of the field.
#[test_log::test]
fn field_notifier_sends_updated_value() {
    covers!(feat_req_someip_635);

    let server_called = Arc::new(Mutex::new(false));
    let client_called = Arc::new(Mutex::new(false));
    let server_flag = Arc::clone(&server_called);
    let client_flag = Arc::clone(&client_called);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let flag = Arc::clone(&server_flag);
        async move {
            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                .start_turmoil()
                .await
                .unwrap();

            let offering = runtime
                .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
                .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            // Wait for subscription to be established
            tokio::time::sleep(Duration::from_millis(800)).await;

            // Field value changes - send notification with updated value
            let updated_value = 100u32.to_be_bytes();
            let eventgroup_id = EventgroupId::new(0x01).unwrap();
            let notifier_event = EventId::new(0x8001).unwrap();
            let event_handle = offering
                .event(notifier_event)
                .eventgroup(eventgroup_id)
                .create()
                .await
                .unwrap();
            event_handle.notify(&updated_value).await.unwrap();
            *flag.lock().unwrap() = true;

            tokio::time::sleep(Duration::from_millis(300)).await;
            Ok(())
        }
    });

    sim.host("client", || {
        let flag = Arc::clone(&client_flag);
        async move {
            tokio::time::sleep(Duration::from_millis(300)).await;

            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
                .start_turmoil()
                .await
                .unwrap();

            let proxy = runtime.find(TEST_SERVICE_ID);
            let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
                .await
                .expect("Discovery timeout")
                .expect("Service available");

            // Subscribe to field notifications
            let eventgroup_id = EventgroupId::new(0x01).unwrap();
            let mut subscription = proxy
                .new_subscription()
                .eventgroup(eventgroup_id)
                .subscribe()
                .await
                .unwrap();

            // Wait for notification
            if let Some(event) = tokio::time::timeout(Duration::from_secs(5), subscription.next())
                .await
                .ok()
                .flatten()
            {
                // Verify notification contains updated field value
                let value = u32::from_be_bytes([
                    event.payload[0],
                    event.payload[1],
                    event.payload[2],
                    event.payload[3],
                ]);
                assert_eq!(
                    value, 100,
                    "Notification should contain updated field value (feat_req_someip_635)"
                );
                *flag.lock().unwrap() = true;
            } else {
                panic!("Should receive field update notification");
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
            Ok(())
        }
    });

    // Add a driver client to ensure simulation runs
    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(
        *server_called.lock().unwrap(),
        "Server should have executed"
    );
    assert!(
        *client_called.lock().unwrap(),
        "Client should have executed"
    );
}

// ============================================================================
// Field Combination Tests
// ============================================================================

/// feat_req_someip_631: Field is combination of getter/setter/notifier
///
/// A field is a combination of a getter method, setter method, and notifier event.
#[test_log::test]
fn field_combines_getter_setter_notifier() {
    covers!(feat_req_someip_631);

    let server_called = Arc::new(Mutex::new(false));
    let client_called = Arc::new(Mutex::new(false));
    let server_flag = Arc::clone(&server_called);
    let client_flag = Arc::clone(&client_called);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let flag = Arc::clone(&server_flag);
        async move {
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

            let mut field_value: u16 = 20; // Initial temperature

            // Wait for subscription
            tokio::time::sleep(Duration::from_millis(300)).await;

            // Handle events - may receive Subscribe first
            let mut getter_handled = false;
            let mut setter_handled = false;

            for _ in 0..10 {
                if let Some(event) = tokio::time::timeout(Duration::from_secs(5), offering.next())
                    .await
                    .ok()
                    .flatten()
                {
                    match event {
                        ServiceEvent::Call {
                            method,
                            payload,
                            responder,
                            ..
                        } => {
                            if method == MethodId::new(0x0001).unwrap() && !getter_handled {
                                // Getter - return current value
                                responder.reply(&field_value.to_be_bytes()).unwrap();
                                getter_handled = true;
                            } else if method == MethodId::new(0x0002).unwrap() && !setter_handled {
                                // Setter - update value
                                field_value = u16::from_be_bytes([payload[0], payload[1]]);
                                responder.reply(b"").unwrap();

                                // Send notification of changed value
                                tokio::time::sleep(Duration::from_millis(10)).await;
                                let eventgroup_id = EventgroupId::new(0x01).unwrap();
                                let notifier_event = EventId::new(0x8001).unwrap();
                                let event_handle = offering
                                    .event(notifier_event)
                                    .eventgroup(eventgroup_id)
                                    .create()
                                    .await
                                    .unwrap();
                                event_handle
                                    .notify(&field_value.to_be_bytes())
                                    .await
                                    .unwrap();
                                *flag.lock().unwrap() = true;
                                setter_handled = true;
                            }
                        }
                        ServiceEvent::Subscribe { .. } => {
                            // Expected - continue processing
                        }
                        _ => {}
                    }

                    if getter_handled && setter_handled {
                        break;
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(300)).await;
            Ok(())
        }
    });

    sim.host("client", || {
        let flag = Arc::clone(&client_flag);
        async move {
            tokio::time::sleep(Duration::from_millis(300)).await;

            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
                .start_turmoil()
                .await
                .unwrap();

            let proxy = runtime.find(TEST_SERVICE_ID);
            let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
                .await
                .expect("Discovery timeout")
                .expect("Service available");

            // Subscribe to get notifier updates
            let eventgroup_id = EventgroupId::new(0x01).unwrap();
            let mut subscription = proxy
                .new_subscription()
                .eventgroup(eventgroup_id)
                .subscribe()
                .await
                .unwrap();

            tokio::time::sleep(Duration::from_millis(50)).await;

            // 1. GET current value (Getter)
            let getter_method = MethodId::new(0x0001).unwrap();
            let get_response = proxy.call(getter_method, b"").await.unwrap();
            let current_value =
                u16::from_be_bytes([get_response.payload[0], get_response.payload[1]]);
            assert_eq!(current_value, 20, "Getter should return current value");

            // 2. SET new value (Setter)
            let setter_method = MethodId::new(0x0002).unwrap();
            let new_value = 25u16.to_be_bytes();
            proxy.call(setter_method, &new_value).await.unwrap();

            // 3. Receive notification of changed value (Notifier)
            if let Some(event) = tokio::time::timeout(Duration::from_secs(5), subscription.next())
                .await
                .ok()
                .flatten()
            {
                let notified_value = u16::from_be_bytes([event.payload[0], event.payload[1]]);
                assert_eq!(notified_value, 25, "Notifier should send updated value");
                *flag.lock().unwrap() = true;
            } else {
                panic!("Should receive notification after setter");
            }

            tokio::time::sleep(Duration::from_millis(300)).await;
            Ok(())
        }
    });

    // Add a driver client to ensure simulation runs
    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(
        *server_called.lock().unwrap(),
        "Server should have executed"
    );
    assert!(
        *client_called.lock().unwrap(),
        "Client should have executed"
    );
}

// ============================================================================
// Field Validation Tests
// ============================================================================

/// Setter can reject invalid values with error response
#[test_log::test]
fn field_setter_can_reject_invalid_value() {
    covers!(feat_req_someip_634);

    let server_called = Arc::new(Mutex::new(false));
    let client_called = Arc::new(Mutex::new(false));
    let server_flag = Arc::clone(&server_called);
    let client_flag = Arc::clone(&client_called);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let flag = Arc::clone(&server_flag);
        async move {
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

            if let Some(event) = offering.next().await {
                if let ServiceEvent::Call {
                    payload, responder, ..
                } = event
                {
                    // Validate value
                    let new_value = u16::from_be_bytes([payload[0], payload[1]]);

                    if new_value > 100 {
                        // Reject invalid value
                        responder.reply_error(ApplicationError::NotOk).unwrap();
                    } else {
                        responder.reply(b"").unwrap();
                    }
                    *flag.lock().unwrap() = true;
                }
            }

            tokio::time::sleep(Duration::from_millis(300)).await;
            Ok(())
        }
    });

    sim.host("client", || {
        let flag = Arc::clone(&client_flag);
        async move {
            tokio::time::sleep(Duration::from_millis(300)).await;

            let runtime = recentip::configure()
                .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
                .start_turmoil()
                .await
                .unwrap();

            let proxy = runtime.find(TEST_SERVICE_ID);
            let proxy = tokio::time::timeout(Duration::from_secs(5), proxy)
                .await
                .expect("Discovery timeout")
                .expect("Service available");

            // Try to set invalid value (> 100)
            let setter_method = MethodId::new(0x0002).unwrap();
            let invalid_value = 150u16.to_be_bytes();
            let result = proxy.call(setter_method, &invalid_value).await;

            // Should receive error response (either as Err or Ok with non-Ok return code)
            match result {
                Err(_) => {
                    // Error propagated as Err - acceptable
                }
                Ok(response) => {
                    assert_ne!(
                        response.return_code,
                        ReturnCode::Ok,
                        "Invalid value should be rejected with error return code"
                    );
                }
            }
            *flag.lock().unwrap() = true;

            Ok(())
        }
    });

    // Add a driver client to ensure simulation runs
    sim.client("driver", async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    });

    sim.run().unwrap();
    assert!(
        *server_called.lock().unwrap(),
        "Server should have executed"
    );
    assert!(
        *client_called.lock().unwrap(),
        "Client should have executed"
    );
}
