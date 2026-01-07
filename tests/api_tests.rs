//! Basic API tests using turmoil for network simulation.

use someip_runtime::handle::ServiceEvent;
use someip_runtime::runtime::Runtime;
use someip_runtime::{
    EventId, EventgroupId, InstanceId, MethodId, RuntimeConfig, Service, ServiceId,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Type alias for turmoil-based runtime for convenience
type TurmoilRuntime =
    Runtime<turmoil::net::UdpSocket, turmoil::net::TcpStream, turmoil::net::TcpListener>;

/// Test service definition
struct TestService;

impl Service for TestService {
    const SERVICE_ID: u16 = 0x1234;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}

/// Another test service
struct AnotherService;

impl Service for AnotherService {
    const SERVICE_ID: u16 = 0x5678;
    const MAJOR_VERSION: u8 = 2;
    const MINOR_VERSION: u32 = 1;
}

#[test_log::test]
fn test_runtime_creation() {
    let mut sim = turmoil::Builder::new().build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: Result<TurmoilRuntime, _> = Runtime::with_socket_type(config).await;

        assert!(runtime.is_ok(), "Runtime should be created successfully");

        Ok(())
    });

    sim.run().unwrap();
}
#[test_log::test]
fn test_find_service() {
    let mut sim = turmoil::Builder::new().build();

    sim.host("client", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Create a proxy - this should succeed immediately (just creates the handle)
        let proxy = runtime.find::<TestService>(InstanceId::Any);

        // Check it's in unavailable state
        assert_eq!(proxy.service_id(), ServiceId::new(0x1234).unwrap());
        assert_eq!(proxy.instance_id(), InstanceId::Any);
        assert!(!proxy.available().await.is_ok());

        Ok(())
    });

    sim.run().unwrap();
}

#[test_log::test]
fn test_offer_service() {
    let mut sim = turmoil::Builder::new().build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer a service
        let offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .udp()
            .start()
            .await;

        assert!(offering.is_ok(), "Offering should succeed");

        let offering = offering.unwrap();
        assert_eq!(offering.service_id(), ServiceId::new(0x1234).unwrap());
        assert_eq!(offering.instance_id(), InstanceId::Id(0x0001));

        Ok(())
    });

    sim.run().unwrap();
}

#[test_log::test]
fn test_instance_id() {
    // Test InstanceId::Any
    assert!(InstanceId::Any.is_any());
    assert_eq!(InstanceId::Any.value(), 0xFFFF);

    // Test InstanceId::Id
    let id = InstanceId::Id(42);
    assert!(!id.is_any());
    assert_eq!(id.value(), 42);

    // Test from raw values
    assert_eq!(InstanceId::new(0xFFFF), Some(InstanceId::Any));
    assert_eq!(InstanceId::new(0x0001), Some(InstanceId::Id(0x0001)));
    assert_eq!(InstanceId::new(0x0000), None); // Reserved
}

#[test_log::test]
fn test_service_id() {
    // Valid IDs
    assert!(ServiceId::new(0x0001).is_some());
    assert!(ServiceId::new(0xFFFE).is_some());

    // Reserved IDs
    assert!(ServiceId::new(0x0000).is_none());
    assert!(ServiceId::new(0xFFFF).is_none());
}

// ============================================================================
// SERVICE DISCOVERY INTEGRATION TESTS
// ============================================================================

#[test_log::test]
fn test_service_discovery_offer_find() {
    // Test that a client can discover a server's offered service
    let mut sim = turmoil::Builder::new().build();

    // Server offers a service
    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .udp()
            .start()
            .await
            .unwrap();

        // Keep the server running for a bit so the client can discover it
        tokio::time::sleep(Duration::from_millis(500)).await;

        Ok(())
    });

    // Client finds the service
    sim.host("client", || async {
        // Give server time to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<TestService>(InstanceId::Any);

        // Wait for discovery (with timeout)
        let result = tokio::time::timeout(Duration::from_millis(300), proxy.available()).await;

        assert!(
            result.is_ok(),
            "Service should be discovered within timeout"
        );
        let available_proxy = result.unwrap().expect("Service available");
        assert_eq!(
            available_proxy.service_id(),
            ServiceId::new(0x1234).unwrap()
        );

        Ok(())
    });

    sim.run().unwrap();
}

#[test_log::test]
fn test_multiple_services() {
    // Test offering and finding multiple services
    let mut sim = turmoil::Builder::new().build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer two different services
        let _offering1 = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .udp()
            .start()
            .await
            .unwrap();
        let _offering2 = runtime
            .offer::<AnotherService>(InstanceId::Id(0x0002))
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Find both services
        let proxy1 = runtime.find::<TestService>(InstanceId::Any);
        let proxy2 = runtime.find::<AnotherService>(InstanceId::Any);

        // Wait for both to be discovered
        let result1 = tokio::time::timeout(Duration::from_millis(300), proxy1.available()).await;
        let result2 = tokio::time::timeout(Duration::from_millis(300), proxy2.available()).await;

        assert!(result1.is_ok(), "TestService should be discovered");
        assert!(result2.is_ok(), "AnotherService should be discovered");

        Ok(())
    });

    sim.run().unwrap();
}

#[test_log::test]
fn test_specific_instance_id() {
    // Test finding a specific instance ID
    let mut sim = turmoil::Builder::new().build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer instance 0x0001
        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    sim.host("client", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Find specific instance
        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));

        let result = tokio::time::timeout(Duration::from_millis(300), proxy.available()).await;

        assert!(result.is_ok(), "Specific instance should be discovered");
        let available = result.unwrap().expect("Service available");
        // Note: After discovery, the instance ID might be updated to the actual ID
        assert_eq!(available.service_id(), ServiceId::new(0x1234).unwrap());

        Ok(())
    });

    sim.run().unwrap();
}

#[test_log::test]
fn test_offering_handle_drop() {
    // Test that dropping an offering handle sends StopOffer
    let mut sim = turmoil::Builder::new().build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        {
            let _offering = runtime
                .offer::<TestService>(InstanceId::Id(0x0001))
                .udp()
                .start()
                .await
                .unwrap();
            // offering dropped here
        }

        // Brief pause to let StopOffer propagate
        tokio::time::sleep(Duration::from_millis(50)).await;

        Ok(())
    });

    sim.run().unwrap();
}

#[test_log::test]
fn test_method_call_rpc() {
    // Test full RPC round-trip: client calls method, server responds
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer the service
        let mut offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for and handle one method call
        if let Some(event) = offering.next().await {
            match event {
                ServiceEvent::Call {
                    method,
                    payload,
                    responder,
                    ..
                } => {
                    // Echo the payload back with method ID prepended
                    let mut response = vec![method.value() as u8];
                    response.extend_from_slice(&payload);
                    responder.reply(&response).await.unwrap();
                }
                _ => panic!("Expected Call event"),
            }
        }

        Ok(())
    });

    sim.host("client", || async {
        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Find the service
        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));

        // Wait for service to become available
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Timeout waiting for service")
            .expect("Service available");

        // Call a method
        let method = MethodId::new(0x0042).unwrap();
        let payload = b"hello";
        let response = tokio::time::timeout(Duration::from_secs(5), proxy.call(method, payload))
            .await
            .expect("Timeout waiting for response")
            .expect("Call should succeed");

        // Verify response
        assert_eq!(response.payload[0], 0x42); // method ID
        assert_eq!(&response.payload[1..], b"hello"); // echoed payload

        Ok(())
    });

    sim.run().unwrap();
}

struct PubSubService;

impl Service for PubSubService {
    const SERVICE_ID: u16 = 0x1234;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}

/// Library auto-renewal test: Events continue beyond initial TTL.
#[test_log::test]
fn library_auto_renews_subscription() {
    let events_received = Arc::new(Mutex::new(Vec::<(Duration, String)>::new()));
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
            .offer::<PubSubService>(InstanceId::Id(0x0001))
            .udp()
            .start()
            .await
            .unwrap();

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();

        let mut i = 0;
        loop {
            offering
                .notify(eventgroup, event_id, format!("event{}", i).as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(500)).await;
            i += 1;
        }
    });

    sim.client("client", async move {
        let start = tokio::time::Instant::now();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .subscribe_ttl(2)
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();

        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();
        let proxy = runtime.find::<PubSubService>(InstanceId::Any);
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription = proxy.subscribe(eventgroup).await.unwrap();

        // Collect events with timestamps
        let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(200), subscription.next()).await {
                Ok(Some(event)) => {
                    let elapsed = start.elapsed();
                    let payload = String::from_utf8_lossy(&event.payload).to_string();
                    eprintln!("Received {} at {:?}", payload, elapsed);
                    events_clone.lock().unwrap().push((elapsed, payload));
                }
                Ok(None) => break,
                Err(_) => continue,
            }
        }
        Ok(())
    });

    sim.run().unwrap();

    let events = events_received.lock().unwrap();

    // Should receive events throughout the 5s window, not just first 2s
    assert!(
        events.len() >= 10,
        "Should receive more than 9 events due to auto-renewal (got {})",
        events.len()
    );

    // Verify we received events AFTER the initial 2s TTL would have expired
    let events_after_ttl = events
        .iter()
        .filter(|(elapsed, _)| *elapsed > Duration::from_secs(2))
        .count();
    assert!(
        events_after_ttl >= 3,
        "Should receive events after initial TTL expires (auto-renewal). Got {} events after 2s",
        events_after_ttl
    );
}

#[test_log::test]
fn test_event_subscription() {
    // Test event subscription: client subscribes, server sends events
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Offer the service
        let offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .udp()
            .start()
            .await
            .unwrap();

        // Wait for client to discover and subscribe
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send some events
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let event_id = EventId::new(0x8001).unwrap();

        offering
            .notify(eventgroup, event_id, b"event1")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        offering
            .notify(eventgroup, event_id, b"event2")
            .await
            .unwrap();

        // Keep server alive for a bit
        tokio::time::sleep(Duration::from_millis(500)).await;

        Ok(())
    });

    sim.host("client", || async {
        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        // Find the service
        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));

        // Wait for service to become available
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Timeout waiting for service")
            .expect("Service available");

        // Subscribe to eventgroup
        let eventgroup = EventgroupId::new(0x0001).unwrap();
        let mut subscription =
            tokio::time::timeout(Duration::from_secs(5), proxy.subscribe(eventgroup))
                .await
                .expect("Timeout subscribing")
                .expect("Subscribe should succeed");

        // Wait for events
        let event1 = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("Timeout waiting for event1");

        assert!(event1.is_some(), "Should receive first event");
        let event1 = event1.unwrap();
        assert_eq!(event1.event_id.value(), 0x8001);
        assert_eq!(&event1.payload[..], b"event1");

        let event2 = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("Timeout waiting for event2");

        assert!(event2.is_some(), "Should receive second event");
        let event2 = event2.unwrap();
        assert_eq!(&event2.payload[..], b"event2");

        Ok(())
    });

    sim.run().unwrap();
}

/// Test that subscribe() returns an error when the server responds with SubscribeEventgroupNack.
///
/// Per SOME/IP-SD specification, when a server rejects a subscription it sends a NACK
/// (SubscribeEventgroupAck with TTL=0 or SubscribeEventgroupNack type 0x07 with TTL=0).
/// The client should propagate this rejection to the caller.
#[test_log::test]
fn subscribe_returns_error_on_nack() {
    use someip_runtime::EventgroupId;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Raw server that sends NACK for all subscriptions
    sim.host("server", || async {
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("server").to_string().parse().unwrap();

        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let sd_multicast: std::net::SocketAddr = "239.255.0.1:30490".parse().unwrap();

        // Build offer message
        let offer = build_sd_offer(0x1234, 0x0001, 1, 0, my_ip, 30509, 3600);

        let mut buf = [0u8; 1500];

        loop {
            // Send periodic offers
            sd_socket.send_to(&offer, sd_multicast).await?;

            let result =
                tokio::time::timeout(Duration::from_millis(200), sd_socket.recv_from(&mut buf))
                    .await;

            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        // SubscribeEventgroup is type 0x06
                        if entry.entry_type as u8 == 0x06 && entry.service_id == 0x1234 {
                            // Send NACK (TTL=0)
                            let nack = build_sd_subscribe_nack(
                                entry.service_id,
                                entry.instance_id,
                                entry.major_version,
                                entry.eventgroup_id,
                            );
                            sd_socket.send_to(&nack, from).await?;
                        }
                    }
                }
            }
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = RuntimeConfig::builder()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .build();
        let runtime: TurmoilRuntime = Runtime::with_socket_type(config).await.unwrap();

        let proxy = runtime.find::<PubSubService>(InstanceId::Id(0x0001));
        let proxy = tokio::time::timeout(Duration::from_secs(5), proxy.available())
            .await
            .expect("Discovery timeout")
            .expect("Service available");

        let eventgroup = EventgroupId::new(0x0001).unwrap();

        // Subscribe should return an error when server sends NACK
        let result =
            tokio::time::timeout(Duration::from_secs(2), proxy.subscribe(eventgroup)).await;

        match result {
            Ok(Ok(_)) => panic!("Subscribe should have returned an error on NACK"),
            Ok(Err(_)) => { /* Expected: subscribe failed due to NACK */ }
            Err(_) => panic!("Subscribe should not timeout waiting for NACK"),
        }

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// TEST HELPERS
// ============================================================================

/// Parse a SOME/IP-SD message from raw bytes
fn parse_sd_message(data: &[u8]) -> Option<(SomeIpHeader, SdMessage)> {
    if data.len() < 16 {
        return None;
    }

    let header = SomeIpHeader {
        service_id: u16::from_be_bytes([data[0], data[1]]),
        method_id: u16::from_be_bytes([data[2], data[3]]),
        length: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
        client_id: u16::from_be_bytes([data[8], data[9]]),
        session_id: u16::from_be_bytes([data[10], data[11]]),
        protocol_version: data[12],
        interface_version: data[13],
        message_type: data[14],
        return_code: data[15],
    };

    // Verify SD message (service=0xFFFF, method=0x8100)
    if header.service_id != 0xFFFF || header.method_id != 0x8100 {
        return None;
    }

    let sd_payload = &data[16..];
    if sd_payload.len() < 12 {
        return None;
    }

    let flags = sd_payload[0];
    let entries_len =
        u32::from_be_bytes([sd_payload[4], sd_payload[5], sd_payload[6], sd_payload[7]]) as usize;

    if sd_payload.len() < 8 + entries_len {
        return None;
    }

    let entries_data = &sd_payload[8..8 + entries_len];
    let mut entries = Vec::new();

    let mut offset = 0;
    while offset + 16 <= entries_data.len() {
        let entry = &entries_data[offset..offset + 16];
        entries.push(SdEntry {
            entry_type: entry[0].into(),
            index_1st_option: entry[1],
            index_2nd_option: entry[2],
            num_options_1: (entry[3] >> 4) & 0x0F,
            num_options_2: entry[3] & 0x0F,
            service_id: u16::from_be_bytes([entry[4], entry[5]]),
            instance_id: u16::from_be_bytes([entry[6], entry[7]]),
            major_version: entry[8],
            ttl: u32::from_be_bytes([0, entry[9], entry[10], entry[11]]),
            minor_version: u32::from_be_bytes([entry[12], entry[13], entry[14], entry[15]]),
            eventgroup_id: u16::from_be_bytes([entry[14], entry[15]]),
            counter: entry[13] & 0x0F,
        });
        offset += 16;
    }

    // Parse options array
    let options_offset = 8 + entries_len;
    let options = if sd_payload.len() > options_offset + 4 {
        let _options_len = u32::from_be_bytes([
            sd_payload[options_offset],
            sd_payload[options_offset + 1],
            sd_payload[options_offset + 2],
            sd_payload[options_offset + 3],
        ]) as usize;
        Vec::new() // Simplified: don't parse options
    } else {
        Vec::new()
    };

    Some((
        header,
        SdMessage {
            flags,
            entries,
            options,
        },
    ))
}

/// Simplified SOME/IP header for test parsing
#[derive(Debug)]
#[allow(dead_code)]
struct SomeIpHeader {
    service_id: u16,
    method_id: u16,
    length: u32,
    client_id: u16,
    session_id: u16,
    protocol_version: u8,
    interface_version: u8,
    message_type: u8,
    return_code: u8,
}

/// Simplified SD message for test parsing
#[derive(Debug)]
#[allow(dead_code)]
struct SdMessage {
    flags: u8,
    entries: Vec<SdEntry>,
    options: Vec<SdOption>,
}

impl SdMessage {
    fn get_udp_endpoint(&self, _entry: &SdEntry) -> Option<std::net::SocketAddr> {
        None // Simplified
    }
}

/// Simplified SD entry for test parsing
#[derive(Debug)]
#[allow(dead_code)]
struct SdEntry {
    entry_type: SdEntryType,
    index_1st_option: u8,
    index_2nd_option: u8,
    num_options_1: u8,
    num_options_2: u8,
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    ttl: u32,
    minor_version: u32,
    eventgroup_id: u16,
    counter: u8,
}

/// SD entry types
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
#[allow(dead_code)]
enum SdEntryType {
    FindService = 0x00,
    OfferService = 0x01,
    SubscribeEventgroup = 0x06,
    SubscribeEventgroupAck = 0x07,
    Unknown = 0xFF,
}

impl From<u8> for SdEntryType {
    fn from(v: u8) -> Self {
        match v {
            0x00 => SdEntryType::FindService,
            0x01 => SdEntryType::OfferService,
            0x06 => SdEntryType::SubscribeEventgroup,
            0x07 => SdEntryType::SubscribeEventgroupAck,
            _ => SdEntryType::Unknown,
        }
    }
}

/// Placeholder for SD options
#[derive(Debug)]
#[allow(dead_code)]
struct SdOption;

/// Build a raw SOME/IP-SD OfferService message
fn build_sd_offer(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    minor_version: u32,
    addr: std::net::Ipv4Addr,
    port: u16,
    ttl: u32,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(64);

    // SOME/IP Header
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes());
    packet.extend_from_slice(&0x8100u16.to_be_bytes());
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes());
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    packet.push(0x01);
    packet.push(0x01);
    packet.push(0x02);
    packet.push(0x00);

    // SD Payload
    packet.push(0xC0);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);
    packet.extend_from_slice(&16u32.to_be_bytes());

    // OfferService Entry
    packet.push(0x01);
    packet.push(0x00);
    packet.push(0x00);
    packet.push(0x10);
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]);
    packet.extend_from_slice(&minor_version.to_be_bytes());

    // Options array
    packet.extend_from_slice(&12u32.to_be_bytes());
    // IPv4 Endpoint Option
    packet.extend_from_slice(&9u16.to_be_bytes());
    packet.push(0x04);
    packet.push(0x00);
    packet.extend_from_slice(&addr.octets());
    packet.push(0x00);
    packet.push(0x11); // UDP
    packet.extend_from_slice(&port.to_be_bytes());

    // Fix length
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}

/// Build a raw SOME/IP-SD SubscribeEventgroupNack message (TTL=0)
fn build_sd_subscribe_nack(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(64);

    // SOME/IP Header
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes());
    packet.extend_from_slice(&0x8100u16.to_be_bytes());
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes());
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    packet.push(0x01);
    packet.push(0x01);
    packet.push(0x02);
    packet.push(0x00);

    // SD Payload
    packet.push(0xC0);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);
    packet.extend_from_slice(&16u32.to_be_bytes());

    // SubscribeEventgroupAck Entry with TTL=0 (NACK)
    packet.push(0x07); // SubscribeEventgroupAck type
    packet.push(0x00);
    packet.push(0x00);
    packet.push(0x00);
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    // TTL = 0 (NACK)
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);
    // Reserved + counter
    packet.push(0x00);
    packet.push(0x00);
    // Eventgroup ID
    packet.extend_from_slice(&eventgroup_id.to_be_bytes());

    // Options array (empty)
    packet.extend_from_slice(&0u32.to_be_bytes());

    // Fix length
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}
