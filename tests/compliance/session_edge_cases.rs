//! Session Handling Edge Cases Compliance Tests
//!
//! Tests session ID management, wrap-around, and edge cases.
//!
//! Key requirements tested:
//! - feat_req_recentip_677: Session ID wraps from 0xFFFF to 0x0001
//! - feat_req_recentip_669: Request/Response shall use session handling
//! - feat_req_recentip_667: Events/Fire&Forget shall use session handling
//! - feat_req_recentip_79: Request ID differentiates multiple calls
//! - feat_req_recentip_80: Request IDs can be reused after response

use someip_runtime::*;

// Re-use wire format parsing from the shared module
#[path = "../wire.rs"]
mod wire;

// Re-use simulated network
#[path = "../simulated.rs"]
mod simulated;

use simulated::{NetworkEvent, SimulatedNetwork};
use wire::Header;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

// ============================================================================
// Helper Functions
// ============================================================================

fn find_requests(network: &SimulatedNetwork) -> Vec<Header> {
    network
        .history()
        .into_iter()
        .filter_map(|event| {
            if let NetworkEvent::UdpSent { data, to, .. } = event {
                if to.port() != 30490 && data.len() >= 16 {
                    if let Some(header) = Header::from_bytes(&data) {
                        if header.message_type == 0x00 {
                            return Some(header);
                        }
                    }
                }
            }
            None
        })
        .collect()
}

// ============================================================================
// Session ID Wrap-Around Tests
// ============================================================================

/// feat_req_recentip_677: Session ID wraps from 0xFFFF to 0x0001
///
/// When the Session ID reaches 0xFFFF, it shall start with 0x0001 again.
/// Note: 0x0000 is skipped (reserved for "session handling disabled").
#[test]
#[ignore = "Runtime::new not implemented"]
fn session_id_wraps_to_0001_not_0000() {
    covers!(feat_req_recentip_677);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let method_id = MethodId::new(0x0001);

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let mut offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let available = proxy.wait_available().unwrap();

    // Track session IDs near the wrap point
    let mut found_ffff = false;
    let mut id_after_ffff = None;

    // Iterate through all 65535 possible session IDs to find wrap-around
    // With simulated network this takes < 1 second
    for _ in 0..=0xFFFFu32 {
        network.clear_history();

        let pending = available.call(method_id, b"x").unwrap();
        network.advance(std::time::Duration::from_micros(10));

        let requests = find_requests(&network);
        if let Some(header) = requests.first() {
            if header.session_id == 0x0000 {
                panic!("Session ID 0x0000 should never be used (reserved for disabled)");
            }
            if found_ffff && id_after_ffff.is_none() {
                id_after_ffff = Some(header.session_id);
            }
            if header.session_id == 0xFFFF {
                found_ffff = true;
            }
        }

        // Server responds
        if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
            request.responder.send_ok_empty().unwrap();
        }
        network.advance(std::time::Duration::from_micros(10));
        let _ = pending.wait();

        // Once we've seen wrap, we can stop
        if id_after_ffff.is_some() {
            break;
        }
    }

    assert!(found_ffff, "Should have reached session ID 0xFFFF");
    assert_eq!(
        id_after_ffff,
        Some(0x0001),
        "After 0xFFFF, session ID should wrap to 0x0001, not 0x0000"
    );
}

// ============================================================================
// Request ID Behavior Tests
// ============================================================================

/// feat_req_recentip_79: Request ID differentiates multiple parallel calls
#[test]
#[ignore = "Runtime::new not implemented"]
fn request_id_differentiates_parallel_calls() {
    covers!(feat_req_recentip_79);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let method_id = MethodId::new(0x0001);

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let available = proxy.wait_available().unwrap();

    network.clear_history();

    // Send 3 parallel requests (without waiting for responses)
    let _pending1 = available.call(method_id, b"request1").unwrap();
    let _pending2 = available.call(method_id, b"request2").unwrap();
    let _pending3 = available.call(method_id, b"request3").unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // All should have unique Request IDs (Client ID + Session ID)
    let requests = find_requests(&network);
    assert!(requests.len() >= 3);

    let request_ids: Vec<u32> = requests
        .iter()
        .map(|h| ((h.client_id as u32) << 16) | (h.session_id as u32))
        .collect();

    // All Request IDs must be unique
    let mut unique_ids = request_ids.clone();
    unique_ids.sort();
    unique_ids.dedup();
    assert_eq!(
        unique_ids.len(),
        request_ids.len(),
        "All parallel requests must have unique Request IDs"
    );
}

/// feat_req_recentip_80: Request IDs can be reused after response arrives
#[test]
#[ignore = "Runtime::new not implemented"]
fn request_id_reusable_after_response() {
    covers!(feat_req_recentip_80);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let method_id = MethodId::new(0x0001);

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let mut offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let available = proxy.wait_available().unwrap();

    // Make many requests (more than might fit in a small ID pool)
    // If IDs weren't reused, this would exhaust them
    for i in 0u16..1000 {
        let pending = available.call(method_id, &i.to_be_bytes()).unwrap();
        network.advance(std::time::Duration::from_millis(1));

        if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
            request.responder.send_ok(&(i + 1).to_be_bytes()).unwrap();
        }
        network.advance(std::time::Duration::from_millis(1));

        let response = pending.wait().unwrap();
        assert_eq!(response.payload, (i + 1).to_be_bytes());
    }

    // If we got here without error, Request IDs were being reused
}

// ============================================================================
// Session Handling per Message Type Tests
// ============================================================================

/// feat_req_recentip_669: Request/Response shall use session handling
#[test]
#[ignore = "Runtime::new not implemented"]
fn request_response_uses_session_handling() {
    covers!(feat_req_recentip_669);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let method_id = MethodId::new(0x0001);

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let available = proxy.wait_available().unwrap();

    network.clear_history();

    // Send two requests
    let _pending1 = available.call(method_id, b"req1").unwrap();
    network.advance(std::time::Duration::from_millis(5));
    let _pending2 = available.call(method_id, b"req2").unwrap();
    network.advance(std::time::Duration::from_millis(5));

    let requests = find_requests(&network);
    assert!(requests.len() >= 2);

    // Session IDs should be different (incrementing)
    assert_ne!(
        requests[0].session_id, requests[1].session_id,
        "Request/Response should use session handling (incrementing Session IDs)"
    );

    // Session IDs should not be 0x0000
    assert_ne!(requests[0].session_id, 0x0000);
    assert_ne!(requests[1].session_id, 0x0000);
}

/// feat_req_recentip_667: Events shall use session handling
#[test]
#[ignore = "Runtime::new not implemented"]
fn events_use_session_handling() {
    covers!(feat_req_recentip_667);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let eventgroup_id = EventgroupId::new(0x01).unwrap();
    let event_id = EventId::new(0x8001).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .eventgroup(eventgroup_id)
        .build()
        .unwrap();

    let mut offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let available = proxy.wait_available().unwrap();

    let _subscription = available.subscribe(eventgroup_id).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    network.clear_history();

    // Send multiple events
    offering.notify(eventgroup_id, event_id, b"event1").unwrap();
    network.advance(std::time::Duration::from_millis(5));
    offering.notify(eventgroup_id, event_id, b"event2").unwrap();
    network.advance(std::time::Duration::from_millis(5));
    offering.notify(eventgroup_id, event_id, b"event3").unwrap();
    network.advance(std::time::Duration::from_millis(5));

    // Find notification messages
    let notifications: Vec<Header> = network
        .history()
        .into_iter()
        .filter_map(|e| {
            if let NetworkEvent::UdpSent { data, to, .. } = e {
                if to.port() != 30490 && data.len() >= 16 {
                    if let Some(h) = Header::from_bytes(&data) {
                        if h.message_type == 0x02 {
                            return Some(h);
                        }
                    }
                }
            }
            None
        })
        .collect();

    assert!(notifications.len() >= 3);

    // Session IDs should increment
    let mut prev_session = 0u16;
    for (i, header) in notifications.iter().enumerate() {
        if i > 0 {
            // Session ID should be greater than previous (with wrap consideration)
            let expected_next = if prev_session == 0xFFFF { 0x0001 } else { prev_session + 1 };
            assert_eq!(
                header.session_id, expected_next,
                "Event session ID should increment"
            );
        }
        prev_session = header.session_id;
    }
}

/// feat_req_recentip_667: Fire&Forget shall use session handling
#[test]
#[ignore = "Runtime::new not implemented"]
fn fire_and_forget_uses_session_handling() {
    covers!(feat_req_recentip_667);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();
    let ff_method = MethodId::new(0x0010);

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let available = proxy.wait_available().unwrap();

    network.clear_history();

    // Send multiple fire&forget
    available.fire_and_forget(ff_method, b"ff1").unwrap();
    network.advance(std::time::Duration::from_millis(5));
    available.fire_and_forget(ff_method, b"ff2").unwrap();
    network.advance(std::time::Duration::from_millis(5));
    available.fire_and_forget(ff_method, b"ff3").unwrap();
    network.advance(std::time::Duration::from_millis(5));

    // Find REQUEST_NO_RETURN messages
    let ff_messages: Vec<Header> = network
        .history()
        .into_iter()
        .filter_map(|e| {
            if let NetworkEvent::UdpSent { data, to, .. } = e {
                if to.port() != 30490 && data.len() >= 16 {
                    if let Some(h) = Header::from_bytes(&data) {
                        if h.message_type == 0x01 {
                            return Some(h);
                        }
                    }
                }
            }
            None
        })
        .collect();

    assert!(ff_messages.len() >= 3);

    // Session IDs should increment (or at least not be 0x0000)
    for header in &ff_messages {
        assert_ne!(
            header.session_id, 0x0000,
            "Fire&Forget should use session handling (Session ID != 0x0000)"
        );
    }
}
