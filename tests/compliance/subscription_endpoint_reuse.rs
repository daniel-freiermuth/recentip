//! Subscription Endpoint Reuse Tests
//!
//! Tests verifying that subscription endpoints (ports) are reused efficiently
//! when subscribing to multiple different services, as opposed to creating
//! a separate socket/port for every subscription.
//!
//! These are wire-level tests where we implement a mock SOME/IP server
//! and observe what endpoints the client library uses for subscriptions.

use bytes::Buf;
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Helper to get Ipv4Addr from turmoil host lookup
fn lookup_ipv4(host: &str) -> Ipv4Addr {
    match turmoil::lookup(host) {
        std::net::IpAddr::V4(addr) => addr,
        _ => panic!("Expected IPv4 address"),
    }
}

// ============================================================================
// HELPER FUNCTIONS: Build raw SD messages
// ============================================================================

/// Build a raw SOME/IP-SD OfferService message
fn build_sd_offer(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    minor_version: u32,
    endpoint_ip: Ipv4Addr,
    endpoint_port: u16,
    protocol: u8, // 0x11=UDP, 0x06=TCP
    ttl: u32,
    session_id: u16,
    reboot_flag: bool,
    unicast_flag: bool,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(56);

    // === SOME/IP Header (16 bytes) ===
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes()); // Service ID = SD
    packet.extend_from_slice(&0x8100u16.to_be_bytes()); // Method ID = SD
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes()); // Length placeholder
    packet.extend_from_slice(&0x0000u16.to_be_bytes()); // Client ID
    packet.extend_from_slice(&session_id.to_be_bytes()); // Session ID
    packet.push(0x01); // Protocol Version
    packet.push(0x01); // Interface Version
    packet.push(0x02); // Message Type = NOTIFICATION
    packet.push(0x00); // Return Code = E_OK

    // === SD Payload ===
    let mut flags = if unicast_flag { 0x40u8 } else { 0x00u8 };
    if reboot_flag {
        flags |= 0x80;
    }
    packet.push(flags);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]); // Reserved

    // Entries array length (16 bytes for one entry)
    packet.extend_from_slice(&16u32.to_be_bytes());

    // === OfferService Entry (16 bytes) ===
    packet.push(0x01); // Type = OfferService
    packet.push(0x00); // Index 1st options
    packet.push(0x00); // Index 2nd options
    packet.push(0x10); // # of opt 1 | # of opt 2
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]); // TTL (24-bit)
    packet.extend_from_slice(&minor_version.to_be_bytes());

    // Options array length (12 bytes for IPv4 endpoint)
    packet.extend_from_slice(&12u32.to_be_bytes());

    // === IPv4 Endpoint Option (12 bytes) ===
    packet.extend_from_slice(&9u16.to_be_bytes()); // Length
    packet.push(0x04); // Type = IPv4 Endpoint
    packet.push(0x00); // Reserved
    packet.extend_from_slice(&endpoint_ip.octets());
    packet.push(0x00); // Reserved
    packet.push(protocol); // L4 Protocol
    packet.extend_from_slice(&endpoint_port.to_be_bytes());

    // Fix up length field
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}

/// Build a raw SOME/IP-SD SubscribeEventgroupAck message
fn build_sd_subscribe_ack(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    ttl: u32,
    session_id: u16,
    reboot_flag: bool,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(48);

    // === SOME/IP Header (16 bytes) ===
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes()); // Service ID = SD
    packet.extend_from_slice(&0x8100u16.to_be_bytes()); // Method ID = SD
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes()); // Length placeholder
    packet.extend_from_slice(&0x0000u16.to_be_bytes()); // Client ID
    packet.extend_from_slice(&session_id.to_be_bytes()); // Session ID
    packet.push(0x01); // Protocol Version
    packet.push(0x01); // Interface Version
    packet.push(0x02); // Message Type = NOTIFICATION
    packet.push(0x00); // Return Code = E_OK

    // === SD Payload ===
    let mut flags = 0x40u8;
    if reboot_flag {
        flags |= 0x80;
    }
    packet.push(flags);
    packet.extend_from_slice(&[0x00, 0x00, 0x00]); // Reserved

    // Entries array length (16 bytes for one entry)
    packet.extend_from_slice(&16u32.to_be_bytes());

    // === SubscribeEventgroupAck Entry (16 bytes) ===
    packet.push(0x07); // Type = SubscribeEventgroupAck
    packet.push(0x00); // Index 1st options
    packet.push(0x00); // Index 2nd options
    packet.push(0x00); // # of options
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&instance_id.to_be_bytes());
    packet.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]); // TTL (24-bit)
    packet.push(0x00); // Reserved
    packet.push(0x00); // Counter
    packet.extend_from_slice(&eventgroup_id.to_be_bytes());

    // Options array length (0 bytes)
    packet.extend_from_slice(&0u32.to_be_bytes());

    // Fix up length field
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());

    packet
}

/// Parse SD message and extract IPv4 endpoint from first option
fn extract_ipv4_endpoint(data: &[u8]) -> Option<SocketAddr> {
    if data.len() < 16 {
        return None;
    }

    let mut cursor = &data[16..]; // Skip SOME/IP header

    // Skip flags (1) + reserved (3)
    if cursor.len() < 4 {
        return None;
    }
    cursor = &cursor[4..];

    // Read entries length
    if cursor.len() < 4 {
        return None;
    }
    let entries_len = u32::from_be_bytes([cursor[0], cursor[1], cursor[2], cursor[3]]) as usize;
    cursor = &cursor[4..];

    // Skip entries
    if cursor.len() < entries_len {
        return None;
    }
    cursor = &cursor[entries_len..];

    // Read options length
    if cursor.len() < 4 {
        return None;
    }
    let _options_len = u32::from_be_bytes([cursor[0], cursor[1], cursor[2], cursor[3]]);
    cursor = &cursor[4..];

    // Parse first option
    if cursor.len() < 12 {
        return None;
    }

    let _length = u16::from_be_bytes([cursor[0], cursor[1]]);
    let option_type = cursor[2];

    if option_type != 0x04 {
        return None; // Not IPv4 endpoint
    }

    let ip = Ipv4Addr::new(cursor[4], cursor[5], cursor[6], cursor[7]);
    let port = u16::from_be_bytes([cursor[10], cursor[11]]);

    Some(SocketAddr::from((ip, port)))
}

// ============================================================================
// UDP SUBSCRIPTION ENDPOINT REUSE TESTS
// ============================================================================

/// Test that UDP subscriptions to different services SHOULD reuse the same port
/// to minimize resource usage while still maintaining protocol correctness
#[test_log::test]
fn udp_different_services_reuse_endpoint() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    let endpoints_observed = Arc::new(Mutex::new(HashSet::new()));
    let endpoints_clone = Arc::clone(&endpoints_observed);

    // Wire-level server that tracks subscription endpoints
    sim.host("server", move || {
        let endpoints = Arc::clone(&endpoints_clone);
        async move {
            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490")
                .await
                .unwrap();

            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            let mut buf = vec![0u8; 65535];
            // Track unique services subscribed (using bitmask: bit 0 = service 0x1111, bit 1 = service 0x2222)
            let mut services_subscribed: u32 = 0;
            let mut next_multicast_session_id = 1;
            let mut next_unicast_session_id = 1;

            // Handle SD messages
            loop {
                let (len, from) = match tokio::time::timeout(
                    Duration::from_secs(5),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    Ok(Ok(result)) => result,
                    Ok(Err(e)) => {
                        tracing::error!("Socket error: {}", e);
                        continue;
                    }
                    Err(_) => break, // Timeout, end test
                };

                let data = &buf[..len];

                // Check if it's an SD message (service_id=0xFFFF, method_id=0x8100)
                if data.len() < 16 {
                    continue;
                }
                let service_id = u16::from_be_bytes([data[0], data[1]]);
                let method_id = u16::from_be_bytes([data[2], data[3]]);

                if service_id != 0xFFFF || method_id != 0x8100 {
                    continue;
                }

                // Parse entry type
                if data.len() < 36 {
                    continue;
                }
                let entry_type = data[24]; // Flags(1) + Reserved(3) + EntriesLen(4) + EntryType(1)

                match entry_type {
                    0x00 => {
                        // FindService - send offers for both services
                        let offer1 = build_sd_offer(
                            0x1111,
                            0x0001,
                            1,
                            0,
                            lookup_ipv4("server"),
                            30491,
                            0x11, // UDP
                            0xFFFFFF,
                            next_multicast_session_id,
                            true,  // reboot_flag
                            false, // unicast_flag (multicast)
                        );
                        next_multicast_session_id += 1;
                        let _ = sd_socket.send_to(&offer1, from).await;
                        tokio::time::sleep(Duration::from_millis(100)).await;

                        let offer2 = build_sd_offer(
                            0x2222,
                            0x0002,
                            1,
                            0,
                            lookup_ipv4("server"),
                            30492,
                            0x11, // UDP
                            0xFFFFFF,
                            next_multicast_session_id,
                            true,  // reboot_flag
                            false, // unicast_flag (multicast)
                        );
                        next_multicast_session_id += 1;
                        let _ = sd_socket.send_to(&offer2, from).await;
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    0x06 => {
                        // SubscribeEventgroup - extract endpoint and send ACK
                        if let Some(endpoint) = extract_ipv4_endpoint(data) {
                            tracing::info!("Subscription from endpoint {}", endpoint);
                            endpoints.lock().unwrap().insert(endpoint);

                            // Parse entry to get service details
                            let service_id = u16::from_be_bytes([data[28], data[29]]);
                            let instance_id = u16::from_be_bytes([data[30], data[31]]);
                            let major_version = data[32];
                            let eventgroup_id = u16::from_be_bytes([data[38], data[39]]);
                            let ttl_bytes = [0, data[33], data[34], data[35]];
                            let ttl = u32::from_be_bytes(ttl_bytes);

                            // Track unique services by setting appropriate bit
                            if service_id == 0x1111 {
                                services_subscribed |= 0x01;
                            } else if service_id == 0x2222 {
                                services_subscribed |= 0x02;
                            }

                            let ack = build_sd_subscribe_ack(
                                service_id,
                                instance_id,
                                major_version,
                                eventgroup_id,
                                ttl,
                                next_unicast_session_id,
                                true, // reboot_flag
                            );
                            next_unicast_session_id += 1;
                            let _ = sd_socket.send_to(&ack, from).await;
                            tokio::time::sleep(Duration::from_millis(93)).await;
                        }
                    }
                    _ => {}
                }

                // After receiving subscriptions for both services, wait a bit then end
                if services_subscribed == 0x03 {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    break;
                }
            }

            tracing::info!(
                "Server observed {} unique endpoint(s)",
                endpoints.lock().unwrap().len()
            );

            Ok(())
        }
    });

    // Client subscribes to two different services
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client"))
            .start_turmoil()
            .await
            .unwrap();

        // Subscribe to service 1
        let proxy1 = runtime
            .find(0x1111)
            .instance(recentip::InstanceId::Id(0x0001))
            .await
            .unwrap();
        let _sub1 = proxy1
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0001).unwrap())
            .subscribe()
            .await
            .unwrap();

        // Subscribe to service 2
        let proxy2 = runtime
            .find(0x2222)
            .instance(recentip::InstanceId::Id(0x0002))
            .await
            .unwrap();
        let _sub2 = proxy2
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0001).unwrap())
            .subscribe()
            .await
            .unwrap();

        // Keep subscriptions alive
        tokio::time::sleep(Duration::from_secs(2)).await;

        Ok(())
    });

    sim.run().unwrap();

    let observed_endpoints = endpoints_observed.lock().unwrap();

    // DESIRED behavior: subscriptions to different services should reuse the same port
    // Each service has a unique (service_id, instance_id) pair in event notifications,
    // so the runtime can route events correctly without needing separate ports
    assert_eq!(
        observed_endpoints.len(),
        1,
        "Expected 1 shared subscription endpoint (reused for both services). Found: {}",
        observed_endpoints.len()
    );

    tracing::info!("✓ UDP subscriptions to different services reuse the same endpoint");
}

/// Test that UDP subscriptions to different instances (same service) reuse the same port
#[test_log::test]
fn udp_different_instances_reuse_endpoint() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    let endpoints_observed = Arc::new(Mutex::new(HashSet::new()));
    let endpoints_clone = Arc::clone(&endpoints_observed);

    sim.host("server", move || {
        let endpoints = Arc::clone(&endpoints_clone);
        async move {
            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490")
                .await
                .unwrap();

            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            let mut buf = vec![0u8; 65535];
            // Track unique instances subscribed (using bitmask: bit 0 = instance 0x0001, bit 1 = instance 0x0002)
            let mut instances_subscribed: u32 = 0;
            let mut next_multicast_session_id = 1;
            let mut next_unicast_session_id = 1;

            loop {
                let (len, from) = match tokio::time::timeout(
                    Duration::from_secs(5),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    Ok(Ok(result)) => result,
                    Ok(Err(e)) => {
                        tracing::error!("Socket error: {}", e);
                        continue;
                    }
                    Err(_) => break,
                };

                let data = &buf[..len];

                if data.len() < 16 {
                    continue;
                }
                let service_id = u16::from_be_bytes([data[0], data[1]]);
                let method_id = u16::from_be_bytes([data[2], data[3]]);

                if service_id != 0xFFFF || method_id != 0x8100 {
                    continue;
                }

                if data.len() < 36 {
                    continue;
                }
                let entry_type = data[24];

                match entry_type {
                    0x00 => {
                        // FindService - offer two instances of service 0x4444
                        let offer1 = build_sd_offer(
                            0x4444,
                            0x0001,
                            1,
                            0,
                            lookup_ipv4("server"),
                            30491,
                            0x11,
                            0xFFFFFF,
                            next_multicast_session_id,
                            true,  // reboot_flag
                            false, // unicast_flag (multicast)
                        );
                        next_multicast_session_id += 1;
                        let _ = sd_socket.send_to(&offer1, from).await;
                        tokio::time::sleep(Duration::from_millis(93)).await;

                        let offer2 = build_sd_offer(
                            0x4444,
                            0x0002,
                            1,
                            0,
                            lookup_ipv4("server"),
                            30492,
                            0x11,
                            0xFFFFFF,
                            next_multicast_session_id,
                            true,  // reboot_flag
                            false, // unicast_flag (multicast)
                        );
                        next_multicast_session_id += 1;
                        let _ = sd_socket.send_to(&offer2, from).await;
                        tokio::time::sleep(Duration::from_millis(93)).await;
                    }
                    0x06 => {
                        if let Some(endpoint) = extract_ipv4_endpoint(data) {
                            endpoints.lock().unwrap().insert(endpoint);

                            let service_id = u16::from_be_bytes([data[28], data[29]]);
                            let instance_id = u16::from_be_bytes([data[30], data[31]]);
                            let major_version = data[32];
                            let eventgroup_id = u16::from_be_bytes([data[38], data[39]]);
                            let ttl_bytes = [0, data[33], data[34], data[35]];
                            let ttl = u32::from_be_bytes(ttl_bytes);

                            // Track unique instances by setting appropriate bit
                            if instance_id == 0x0001 {
                                instances_subscribed |= 0x01;
                            } else if instance_id == 0x0002 {
                                instances_subscribed |= 0x02;
                            }

                            let ack = build_sd_subscribe_ack(
                                service_id,
                                instance_id,
                                major_version,
                                eventgroup_id,
                                ttl,
                                next_unicast_session_id,
                                true, // reboot_flag
                            );
                            next_unicast_session_id += 1;
                            let _ = sd_socket.send_to(&ack, from).await;
                            tokio::time::sleep(Duration::from_millis(93)).await;
                        }
                    }
                    _ => {}
                }

                // Break when both instances have subscribed
                if instances_subscribed == 0x03 {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    break;
                }
            }

            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client"))
            .start_turmoil()
            .await
            .unwrap();

        let proxy1 = runtime
            .find(0x4444)
            .instance(recentip::InstanceId::Id(0x0001))
            .await
            .unwrap();
        let _sub1 = proxy1
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0001).unwrap())
            .subscribe()
            .await
            .unwrap();

        let proxy2 = runtime
            .find(0x4444)
            .instance(recentip::InstanceId::Id(0x0002))
            .await
            .unwrap();
        let _sub2 = proxy2
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0001).unwrap())
            .subscribe()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;

        Ok(())
    });

    sim.run().unwrap();

    let observed_endpoints = endpoints_observed.lock().unwrap();

    assert_eq!(
        observed_endpoints.len(),
        1,
        "Expected 1 shared subscription endpoint (reused for both instances). Found: {}",
        observed_endpoints.len()
    );

    tracing::info!("✓ UDP subscriptions to different instances reuse the same endpoint");
}

/// Test that UDP subscriptions to different major versions reuse the same port
#[test_log::test]
fn udp_different_major_versions_reuse_endpoint() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    let endpoints_observed = Arc::new(Mutex::new(HashSet::new()));
    let endpoints_clone = Arc::clone(&endpoints_observed);

    sim.host("server", move || {
        let endpoints = Arc::clone(&endpoints_clone);
        async move {
            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490")
                .await
                .unwrap();

            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            let mut buf = vec![0u8; 65535];
            // Track unique major versions subscribed (bitmask: bit 0 = v1, bit 1 = v2)
            let mut versions_subscribed: u32 = 0;
            let mut next_multicast_session_id = 1;
            let mut next_unicast_session_id = 1;

            loop {
                let (len, from) = match tokio::time::timeout(
                    Duration::from_secs(5),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    Ok(Ok(result)) => result,
                    Ok(Err(e)) => {
                        tracing::error!("Socket error: {}", e);
                        continue;
                    }
                    Err(_) => break, // Timeout, end test
                };

                let data = &buf[..len];

                if data.len() < 16 {
                    continue;
                }
                let service_id = u16::from_be_bytes([data[0], data[1]]);
                let method_id = u16::from_be_bytes([data[2], data[3]]);

                if service_id != 0xFFFF || method_id != 0x8100 {
                    continue;
                }

                if data.len() < 36 {
                    continue;
                }
                let entry_type = data[24];

                match entry_type {
                    0x00 => {
                        // FindService - offer two versions of service 0x5555
                        let offer1 = build_sd_offer(
                            0x5555,
                            0x0001,
                            1,
                            0,
                            lookup_ipv4("server"),
                            30491,
                            0x11,
                            0xFFFFFF,
                            next_multicast_session_id,
                            true,  // reboot_flag
                            false, // unicast_flag (multicast)
                        );
                        next_multicast_session_id += 1;
                        let _ = sd_socket.send_to(&offer1, from).await;
                        tokio::time::sleep(Duration::from_millis(93)).await;

                        let offer2 = build_sd_offer(
                            0x5555,
                            0x0001,
                            2,
                            0,
                            lookup_ipv4("server"),
                            30491, // Same port as v1
                            0x11,
                            0xFFFFFF,
                            next_multicast_session_id,
                            true,  // reboot_flag
                            false, // unicast_flag (multicast)
                        );
                        next_multicast_session_id += 1;
                        let _ = sd_socket.send_to(&offer2, from).await;
                        tokio::time::sleep(Duration::from_millis(93)).await;
                    }
                    0x06 => {
                        if let Some(endpoint) = extract_ipv4_endpoint(data) {
                            endpoints.lock().unwrap().insert(endpoint);

                            let service_id = u16::from_be_bytes([data[28], data[29]]);
                            let instance_id = u16::from_be_bytes([data[30], data[31]]);
                            let major_version = data[32];
                            let eventgroup_id = u16::from_be_bytes([data[38], data[39]]);
                            let ttl_bytes = [0, data[33], data[34], data[35]];
                            let ttl = u32::from_be_bytes(ttl_bytes);

                            // Track unique major versions by setting appropriate bit
                            if major_version == 1 {
                                versions_subscribed |= 0x01;
                            } else if major_version == 2 {
                                versions_subscribed |= 0x02;
                            }

                            let ack = build_sd_subscribe_ack(
                                service_id,
                                instance_id,
                                major_version,
                                eventgroup_id,
                                ttl,
                                next_unicast_session_id,
                                true, // reboot_flag
                            );
                            next_unicast_session_id += 1;
                            let _ = sd_socket.send_to(&ack, from).await;
                            tokio::time::sleep(Duration::from_millis(93)).await;
                        }
                    }
                    _ => {}
                }

                // After receiving subscriptions for both versions, wait a bit then end
                if versions_subscribed == 0x03 {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    break;
                }
            }

            tracing::info!(
                "Server observed {} unique endpoint(s)",
                endpoints.lock().unwrap().len()
            );

            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client"))
            .start_turmoil()
            .await
            .unwrap();

        let proxy1 = runtime
            .find(0x5555)
            .instance(recentip::InstanceId::Id(0x0001))
            .major_version(1)
            .await
            .unwrap();
        let _sub1 = proxy1
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0001).unwrap())
            .subscribe()
            .await
            .unwrap();

        let proxy2 = runtime
            .find(0x5555)
            .instance(recentip::InstanceId::Id(0x0001))
            .major_version(2)
            .await
            .unwrap();
        let _sub2 = proxy2
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0001).unwrap())
            .subscribe()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;

        Ok(())
    });

    sim.run().unwrap();

    let observed_endpoints = endpoints_observed.lock().unwrap();

    assert_eq!(
        observed_endpoints.len(),
        1,
        "Expected 1 shared subscription endpoint (reused for both versions). Found: {}",
        observed_endpoints.len()
    );

    tracing::info!("✓ UDP subscriptions to different major versions reuse the same endpoint");
}

// ============================================================================
// TCP SUBSCRIPTION ENDPOINT REUSE TESTS
// ============================================================================

/// Test that TCP subscriptions to different services reuse the same connection
#[test_log::test]
fn tcp_different_services_reuse_connection() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .max_message_latency(Duration::from_millis(85))
        .build();

    let connections_observed = Arc::new(Mutex::new(HashSet::new()));
    let connections_clone = Arc::clone(&connections_observed);

    sim.host("server", move || {
        let connections = Arc::clone(&connections_clone);
        async move {
            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490")
                .await
                .unwrap();

            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            let tcp_listener = turmoil::net::TcpListener::bind("0.0.0.0:30500")
                .await
                .unwrap();

            let connections_task = Arc::clone(&connections);
            tokio::spawn(async move {
                loop {
                    match tcp_listener.accept().await {
                        Ok((stream, addr)) => {
                            tracing::info!("TCP connection from {}", addr);
                            connections_task.lock().unwrap().insert(addr);
                            tokio::spawn(async move {
                                let _stream = stream;
                                tokio::time::sleep(Duration::from_secs(10)).await;
                            });
                        }
                        Err(_) => break,
                    }
                }
            });

            let mut buf = vec![0u8; 65535];
            // Track unique services subscribed (using bitmask: bit 0 = service 0x6666, bit 1 = service 0x7777)
            let mut services_subscribed: u32 = 0;
            let mut next_multicast_session_id = 1;
            let mut next_unicast_session_id = 1;

            loop {
                let (len, from) = match tokio::time::timeout(
                    Duration::from_secs(5),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    Ok(Ok(result)) => result,
                    Ok(Err(e)) => {
                        tracing::error!("Socket error: {}", e);
                        continue;
                    }
                    Err(_) => break,
                };

                let data = &buf[..len];

                if data.len() < 16 {
                    continue;
                }
                let service_id = u16::from_be_bytes([data[0], data[1]]);
                let method_id = u16::from_be_bytes([data[2], data[3]]);

                if service_id != 0xFFFF || method_id != 0x8100 {
                    continue;
                }

                if data.len() < 36 {
                    continue;
                }
                let entry_type = data[24];

                match entry_type {
                    0x00 => {
                        let offer1 = build_sd_offer(
                            0x6666,
                            0x0001,
                            1,
                            0,
                            lookup_ipv4("server"),
                            30500,
                            0x06,
                            0xFFFFFF,
                            next_multicast_session_id,
                            true,  // reboot_flag
                            false, // unicast_flag (multicast)
                        );
                        next_multicast_session_id += 1;
                        let _ = sd_socket.send_to(&offer1, from).await;
                        tokio::time::sleep(Duration::from_millis(93)).await;

                        let offer2 = build_sd_offer(
                            0x7777,
                            0x0002,
                            1,
                            0,
                            lookup_ipv4("server"),
                            30500,
                            0x06,
                            0xFFFFFF,
                            next_multicast_session_id,
                            true,  // reboot_flag
                            false, // unicast_flag (multicast)
                        );
                        next_multicast_session_id += 1;
                        let _ = sd_socket.send_to(&offer2, from).await;
                        tokio::time::sleep(Duration::from_millis(93)).await;
                    }
                    0x06 => {
                        let service_id = u16::from_be_bytes([data[28], data[29]]);
                        let instance_id = u16::from_be_bytes([data[30], data[31]]);
                        let major_version = data[32];
                        let eventgroup_id = u16::from_be_bytes([data[38], data[39]]);
                        let ttl_bytes = [0, data[33], data[34], data[35]];
                        let ttl = u32::from_be_bytes(ttl_bytes);

                        // Track unique services by setting appropriate bit
                        if service_id == 0x6666 {
                            services_subscribed |= 0x01;
                        } else if service_id == 0x7777 {
                            services_subscribed |= 0x02;
                        }

                        let ack = build_sd_subscribe_ack(
                            service_id,
                            instance_id,
                            major_version,
                            eventgroup_id,
                            ttl,
                            next_unicast_session_id,
                            true, // reboot_flag
                        );
                        next_unicast_session_id += 1;
                        let _ = sd_socket.send_to(&ack, from).await;
                        tokio::time::sleep(Duration::from_millis(93)).await;
                    }
                    _ => {}
                }

                // Break when both services have subscribed
                if services_subscribed == 0x03 {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    break;
                }
            }

            tracing::info!(
                "Server observed {} TCP connection(s)",
                connections.lock().unwrap().len()
            );

            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client"))
            .preferred_transport(recentip::config::Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let proxy1 = runtime
            .find(0x6666)
            .instance(recentip::InstanceId::Id(0x0001))
            .await
            .unwrap();
        let _sub1 = proxy1
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0001).unwrap())
            .subscribe()
            .await
            .unwrap();

        let proxy2 = runtime
            .find(0x7777)
            .instance(recentip::InstanceId::Id(0x0002))
            .await
            .unwrap();
        let _sub2 = proxy2
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0001).unwrap())
            .subscribe()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;

        Ok(())
    });

    sim.run().unwrap();

    let observed_connections = connections_observed.lock().unwrap();

    // TCP subscriptions to DIFFERENT services share the same connection.
    // Routing works correctly because the event header contains service_id.
    assert_eq!(
        observed_connections.len(),
        1,
        "Expected 1 shared TCP connection for different services. Found: {}",
        observed_connections.len()
    );

    tracing::info!("✓ TCP subscriptions to different services reuse the same connection");
}

/// Test that TCP subscriptions to different instances reuse the same connection
#[test_log::test]
fn tcp_different_instances_reuse_connection() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    let connections_observed = Arc::new(Mutex::new(HashSet::new()));
    let connections_clone = Arc::clone(&connections_observed);

    sim.host("server", move || {
        let connections = Arc::clone(&connections_clone);
        async move {
            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490")
                .await
                .unwrap();

            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            let tcp_listener = turmoil::net::TcpListener::bind("0.0.0.0:30500")
                .await
                .unwrap();

            let connections_task = Arc::clone(&connections);
            tokio::spawn(async move {
                loop {
                    match tcp_listener.accept().await {
                        Ok((stream, addr)) => {
                            tracing::info!("TCP connection from {}", addr);
                            connections_task.lock().unwrap().insert(addr);
                            tokio::spawn(async move {
                                let _stream = stream;
                                tokio::time::sleep(Duration::from_secs(10)).await;
                            });
                        }
                        Err(_) => break,
                    }
                }
            });

            let mut buf = vec![0u8; 65535];
            // Track unique instances subscribed (using bitmask: bit 0 = instance 0x0001, bit 1 = instance 0x0002)
            let mut instances_subscribed: u32 = 0;
            let mut next_multicast_session_id = 1;
            let mut next_unicast_session_id = 1;

            loop {
                let (len, from) = match tokio::time::timeout(
                    Duration::from_secs(5),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    Ok(Ok(result)) => result,
                    Ok(Err(e)) => {
                        tracing::error!("Socket error: {}", e);
                        continue;
                    }
                    Err(_) => break,
                };

                let data = &buf[..len];

                if data.len() < 16 {
                    continue;
                }
                let service_id = u16::from_be_bytes([data[0], data[1]]);
                let method_id = u16::from_be_bytes([data[2], data[3]]);

                if service_id != 0xFFFF || method_id != 0x8100 {
                    continue;
                }

                if data.len() < 36 {
                    continue;
                }
                let entry_type = data[24];

                match entry_type {
                    0x00 => {
                        let offer1 = build_sd_offer(
                            0x8888,
                            0x0001,
                            1,
                            0,
                            lookup_ipv4("server"),
                            30500,
                            0x06,
                            0xFFFFFF,
                            next_multicast_session_id,
                            true,  // reboot_flag
                            false, // unicast_flag (multicast)
                        );
                        next_multicast_session_id += 1;
                        let _ = sd_socket.send_to(&offer1, from).await;
                        tokio::time::sleep(Duration::from_millis(100)).await;

                        let offer2 = build_sd_offer(
                            0x8888,
                            0x0002,
                            1,
                            0,
                            lookup_ipv4("server"),
                            30500,
                            0x06,
                            0xFFFFFF,
                            next_multicast_session_id,
                            true,  // reboot_flag
                            false, // unicast_flag (multicast)
                        );
                        next_multicast_session_id += 1;
                        let _ = sd_socket.send_to(&offer2, from).await;
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    0x06 => {
                        let service_id = u16::from_be_bytes([data[28], data[29]]);
                        let instance_id = u16::from_be_bytes([data[30], data[31]]);
                        let major_version = data[32];
                        let eventgroup_id = u16::from_be_bytes([data[38], data[39]]);
                        let ttl_bytes = [0, data[33], data[34], data[35]];
                        let ttl = u32::from_be_bytes(ttl_bytes);

                        // Track unique instances by setting appropriate bit
                        if instance_id == 0x0001 {
                            instances_subscribed |= 0x01;
                        } else if instance_id == 0x0002 {
                            instances_subscribed |= 0x02;
                        }

                        let ack = build_sd_subscribe_ack(
                            service_id,
                            instance_id,
                            major_version,
                            eventgroup_id,
                            ttl,
                            next_unicast_session_id,
                            true, // reboot_flag
                        );
                        next_unicast_session_id += 1;
                        let _ = sd_socket.send_to(&ack, from).await;
                        tokio::time::sleep(Duration::from_millis(93)).await;
                    }
                    _ => {}
                }

                // Break when both instances have subscribed
                if instances_subscribed == 0x03 {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    break;
                }
            }

            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client"))
            .preferred_transport(recentip::config::Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let proxy1 = runtime
            .find(0x8888)
            .instance(recentip::InstanceId::Id(0x0001))
            .await
            .unwrap();
        let _sub1 = proxy1
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0001).unwrap())
            .subscribe()
            .await
            .unwrap();

        let proxy2 = runtime
            .find(0x8888)
            .instance(recentip::InstanceId::Id(0x0002))
            .await
            .unwrap();
        let _sub2 = proxy2
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0001).unwrap())
            .subscribe()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;

        Ok(())
    });

    sim.run().unwrap();

    let observed_connections = connections_observed.lock().unwrap();

    // TCP subscriptions to DIFFERENT instances share the same connection.
    // Routing works correctly because we match by server endpoint -> instance.
    assert_eq!(
        observed_connections.len(),
        1,
        "Expected 1 shared TCP connection for different instances. Found: {}",
        observed_connections.len()
    );

    tracing::info!("✓ TCP subscriptions to different instances reuse the same connection");
}

/// Test that TCP subscriptions to different major versions reuse the same connection
#[test_log::test]
fn tcp_different_major_versions_reuse_connection() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    let connections_observed = Arc::new(Mutex::new(HashSet::new()));
    let connections_clone = Arc::clone(&connections_observed);

    sim.host("server", move || {
        let connections = Arc::clone(&connections_clone);
        async move {
            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490")
                .await
                .unwrap();

            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            let tcp_listener = turmoil::net::TcpListener::bind("0.0.0.0:30500")
                .await
                .unwrap();

            let connections_task = Arc::clone(&connections);
            tokio::spawn(async move {
                loop {
                    match tcp_listener.accept().await {
                        Ok((stream, addr)) => {
                            tracing::info!("TCP connection from {}", addr);
                            connections_task.lock().unwrap().insert(addr);
                            tokio::spawn(async move {
                                let _stream = stream;
                                tokio::time::sleep(Duration::from_secs(10)).await;
                            });
                        }
                        Err(_) => break,
                    }
                }
            });

            let mut buf = vec![0u8; 65535];
            // Track unique major versions subscribed (using bitmask: bit 0 = version 1, bit 1 = version 2)
            let mut versions_subscribed: u32 = 0;
            let mut next_multicast_session_id = 1;
            let mut next_unicast_session_id = 1;

            loop {
                let (len, from) = match tokio::time::timeout(
                    Duration::from_secs(5),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    Ok(Ok(result)) => result,
                    Ok(Err(e)) => {
                        tracing::error!("Socket error: {}", e);
                        continue;
                    }
                    Err(_) => break,
                };

                let data = &buf[..len];

                if data.len() < 16 {
                    continue;
                }
                let service_id = u16::from_be_bytes([data[0], data[1]]);
                let method_id = u16::from_be_bytes([data[2], data[3]]);

                if service_id != 0xFFFF || method_id != 0x8100 {
                    continue;
                }

                if data.len() < 36 {
                    continue;
                }
                let entry_type = data[24];

                match entry_type {
                    0x00 => {
                        // Offer two major versions of the same service on SAME port
                        let offer_v1 = build_sd_offer(
                            0x9999,
                            0x0001,
                            1,
                            0,
                            lookup_ipv4("server"),
                            30500,
                            0x06,
                            0xFFFFFF,
                            next_multicast_session_id,
                            true,  // reboot_flag
                            false, // unicast_flag (multicast)
                        );
                        next_multicast_session_id += 1;
                        let _ = sd_socket.send_to(&offer_v1, from).await;
                        tokio::time::sleep(Duration::from_millis(93)).await;

                        let offer_v2 = build_sd_offer(
                            0x9999,
                            0x0001,
                            2,
                            0,
                            lookup_ipv4("server"),
                            30500, // Same port as v1
                            0x06,
                            0xFFFFFF,
                            next_multicast_session_id,
                            true,  // reboot_flag
                            false, // unicast_flag (multicast)
                        );
                        next_multicast_session_id += 1;
                        let _ = sd_socket.send_to(&offer_v2, from).await;
                        tokio::time::sleep(Duration::from_millis(93)).await;
                    }
                    0x06 => {
                        let service_id = u16::from_be_bytes([data[28], data[29]]);
                        let instance_id = u16::from_be_bytes([data[30], data[31]]);
                        let major_version = data[32];
                        let eventgroup_id = u16::from_be_bytes([data[38], data[39]]);
                        let ttl_bytes = [0, data[33], data[34], data[35]];
                        let ttl = u32::from_be_bytes(ttl_bytes);

                        // Track unique major versions by setting appropriate bit
                        if major_version == 1 {
                            versions_subscribed |= 0x01;
                        } else if major_version == 2 {
                            versions_subscribed |= 0x02;
                        }

                        let ack = build_sd_subscribe_ack(
                            service_id,
                            instance_id,
                            major_version,
                            eventgroup_id,
                            ttl,
                            next_unicast_session_id,
                            true, // reboot_flag
                        );
                        next_unicast_session_id += 1;
                        let _ = sd_socket.send_to(&ack, from).await;
                        tokio::time::sleep(Duration::from_millis(93)).await;
                    }
                    _ => {}
                }

                // Break when both major versions have subscribed
                if versions_subscribed == 0x03 {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    break;
                }
            }

            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client"))
            .preferred_transport(recentip::config::Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let proxy_v1 = runtime
            .find(0x9999)
            .instance(recentip::InstanceId::Id(0x0001))
            .major_version(1)
            .await
            .unwrap();

        let _sub1 = proxy_v1
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0001).unwrap())
            .subscribe()
            .await
            .unwrap();

        let proxy_v2 = runtime
            .find(0x9999)
            .instance(recentip::InstanceId::Id(0x0001))
            .major_version(2)
            .await
            .unwrap();

        let _sub2 = proxy_v2
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0001).unwrap())
            .subscribe()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;

        Ok(())
    });

    sim.run().unwrap();

    let observed_connections = connections_observed.lock().unwrap();

    // TCP subscriptions to DIFFERENT major versions share the same connection.
    // Routing works correctly because we match by (service_id, server_endpoint).
    assert_eq!(
        observed_connections.len(),
        1,
        "Expected 1 shared TCP connection for different major versions. Found: {}",
        observed_connections.len()
    );

    tracing::info!("✓ TCP subscriptions to different major versions reuse the same connection");
}

// ============================================================================
// MULTIPLE EVENTGROUPS PER SERVICE ENDPOINT REUSE TESTS
// ============================================================================

/// Test that subscriptions to multiple eventgroups across two services use
/// minimal endpoints by sharing across services.
///
/// Key insight: Events contain service_id in the header, so we can always
/// distinguish events from different services. We only need separate endpoints
/// for different eventgroups within the SAME service.
///
/// Scenario:
/// - Server offers S1 (0x1111) and S2 (0x2222) with 2 eventgroups each
/// - Each service has 2 events: Event1 and Event2
/// - EG1 (0x0001) contains only Event1
/// - EG2 (0x0002) contains both Event1 and Event2
/// - Client subscribes to both eventgroups on both services (4 subscriptions)
///
/// Expected (ideal): 2 endpoints
/// - Endpoint A: S1-EG1 + S2-EG1 (share across services)
/// - Endpoint B: S1-EG2 + S2-EG2 (share across services)
#[test_log::test]
fn udp_multiple_eventgroups_per_service_share_endpoint() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    let endpoints_observed = Arc::new(Mutex::new(HashSet::new()));
    let endpoints_clone = Arc::clone(&endpoints_observed);

    // Track subscriptions by (service_id, eventgroup_id)
    let subscriptions_received = Arc::new(Mutex::new(HashSet::new()));
    let subscriptions_clone = Arc::clone(&subscriptions_received);

    sim.host("server", move || {
        let endpoints = Arc::clone(&endpoints_clone);
        let subscriptions = Arc::clone(&subscriptions_clone);
        async move {
            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490")
                .await
                .unwrap();

            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            let mut buf = vec![0u8; 65535];
            let mut next_multicast_session_id = 1;
            let mut next_unicast_session_id = 1;

            loop {
                let (len, from) = match tokio::time::timeout(
                    Duration::from_secs(5),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    Ok(Ok(result)) => result,
                    Ok(Err(e)) => {
                        tracing::error!("Socket error: {}", e);
                        continue;
                    }
                    Err(_) => break, // Timeout, end test
                };

                let data = &buf[..len];

                if data.len() < 16 {
                    continue;
                }
                let service_id = u16::from_be_bytes([data[0], data[1]]);
                let method_id = u16::from_be_bytes([data[2], data[3]]);

                if service_id != 0xFFFF || method_id != 0x8100 {
                    continue;
                }

                if data.len() < 36 {
                    continue;
                }
                let entry_type = data[24];

                match entry_type {
                    0x00 => {
                        // FindService - offer both services S1 and S2
                        let offer_s1 = build_sd_offer(
                            0x1111, // S1
                            0x0001,
                            1,
                            0,
                            lookup_ipv4("server"),
                            30491,
                            0x11, // UDP
                            0xFFFFFF,
                            next_multicast_session_id,
                            true,  // reboot_flag
                            false, // unicast_flag (multicast)
                        );
                        next_multicast_session_id += 1;
                        let _ = sd_socket.send_to(&offer_s1, from).await;
                        tokio::time::sleep(Duration::from_millis(100)).await;

                        let offer_s2 = build_sd_offer(
                            0x2222, // S2
                            0x0001,
                            1,
                            0,
                            lookup_ipv4("server"),
                            30492,
                            0x11, // UDP
                            0xFFFFFF,
                            next_multicast_session_id,
                            true,  // reboot_flag
                            false, // unicast_flag (multicast)
                        );
                        next_multicast_session_id += 1;
                        let _ = sd_socket.send_to(&offer_s2, from).await;
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    0x06 => {
                        // SubscribeEventgroup
                        if let Some(endpoint) = extract_ipv4_endpoint(data) {
                            let sub_service_id = u16::from_be_bytes([data[28], data[29]]);
                            let instance_id = u16::from_be_bytes([data[30], data[31]]);
                            let major_version = data[32];
                            let eventgroup_id = u16::from_be_bytes([data[38], data[39]]);
                            let ttl_bytes = [0, data[33], data[34], data[35]];
                            let ttl = u32::from_be_bytes(ttl_bytes);

                            tracing::info!(
                                "Subscription from {} for service {:04x} eventgroup {:04x}",
                                endpoint,
                                sub_service_id,
                                eventgroup_id
                            );

                            endpoints.lock().unwrap().insert(endpoint);
                            subscriptions
                                .lock()
                                .unwrap()
                                .insert((sub_service_id, eventgroup_id));

                            let ack = build_sd_subscribe_ack(
                                sub_service_id,
                                instance_id,
                                major_version,
                                eventgroup_id,
                                ttl,
                                next_unicast_session_id,
                                true, // reboot_flag
                            );
                            next_unicast_session_id += 1;
                            let _ = sd_socket.send_to(&ack, from).await;
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                    _ => {}
                }

                // After receiving all 4 subscriptions (2 services × 2 eventgroups), exit
                if subscriptions.lock().unwrap().len() >= 4 {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    break;
                }
            }

            tracing::info!(
                "Server observed {} unique endpoint(s) for {} subscriptions",
                endpoints.lock().unwrap().len(),
                subscriptions.lock().unwrap().len()
            );

            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client"))
            .start_turmoil()
            .await
            .unwrap();

        // Discover S1
        let proxy_s1 = runtime
            .find(0x1111)
            .instance(recentip::InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Subscribe to S1 EG1 (contains Event1)
        let _sub_s1_eg1 = proxy_s1
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0001).unwrap())
            .subscribe()
            .await
            .unwrap();

        // Subscribe to S1 EG2 (contains Event1 + Event2)
        let _sub_s1_eg2 = proxy_s1
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0002).unwrap())
            .subscribe()
            .await
            .unwrap();

        // Discover S2
        let proxy_s2 = runtime
            .find(0x2222)
            .instance(recentip::InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Subscribe to S2 EG1 (contains Event1)
        let _sub_s2_eg1 = proxy_s2
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0001).unwrap())
            .subscribe()
            .await
            .unwrap();

        // Subscribe to S2 EG2 (contains Event1 + Event2)
        let _sub_s2_eg2 = proxy_s2
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0002).unwrap())
            .subscribe()
            .await
            .unwrap();

        // Keep subscriptions alive
        tokio::time::sleep(Duration::from_secs(2)).await;

        Ok(())
    });

    sim.run().unwrap();

    let observed_endpoints = endpoints_observed.lock().unwrap();
    let subs = subscriptions_received.lock().unwrap();

    // Verify we got all 4 subscriptions
    assert_eq!(
        subs.len(),
        4,
        "Expected 4 subscriptions (2 services × 2 eventgroups). Got: {}",
        subs.len()
    );

    // Endpoint reuse strategy:
    // - Events contain service_id in header → easy to route across services
    // - Multiple eventgroups within SAME service are harder to distinguish
    //
    // Expected: share endpoints across services, separate by eventgroup "slot"
    // - Endpoint A: S1-EG1 + S2-EG1 (same eventgroup, different services)
    // - Endpoint B: S1-EG2 + S2-EG2 (same eventgroup, different services)
    // → 2 endpoints total
    assert_eq!(
        observed_endpoints.len(),
        2,
        "Expected 2 endpoints (share across services, separate by eventgroup). \
         Received subscriptions on {} endpoints: {:?}",
        observed_endpoints.len(),
        observed_endpoints
    );

    tracing::info!(
        "✓ Multiple eventgroups test: {} endpoints for 4 subscriptions",
        observed_endpoints.len()
    );
}

/// Test that TCP subscriptions to multiple eventgroups across two services use
/// minimal connections by sharing across services.
///
/// Key insight: Events contain service_id in the header, so we can always
/// distinguish events from different services. We only need separate connections
/// for different eventgroups within the SAME service.
///
/// Scenario:
/// - Server offers S1 (0x1111) and S2 (0x2222) with 2 eventgroups each via TCP
/// - Each service has 2 events: Event1 and Event2
/// - EG1 (0x0001) contains only Event1
/// - EG2 (0x0002) contains both Event1 and Event2
/// - Client subscribes to both eventgroups on both services (4 subscriptions)
///
/// Expected (ideal): 2 TCP connections
/// - Connection A: S1-EG1 + S2-EG1 (share across services)
/// - Connection B: S1-EG2 + S2-EG2 (share across services)
#[test_log::test]
fn tcp_multiple_eventgroups_per_service_share_connection() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .max_message_latency(Duration::from_millis(90))
        .build();

    let connections_observed = Arc::new(Mutex::new(HashSet::new()));
    let connections_clone = Arc::clone(&connections_observed);

    // Track subscriptions by (service_id, eventgroup_id)
    let subscriptions_received = Arc::new(Mutex::new(HashSet::new()));
    let subscriptions_clone = Arc::clone(&subscriptions_received);

    sim.host("server", move || {
        let connections = Arc::clone(&connections_clone);
        let subscriptions = Arc::clone(&subscriptions_clone);
        async move {
            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490")
                .await
                .unwrap();

            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

            // Single TCP listener for both services
            let tcp_listener = turmoil::net::TcpListener::bind("0.0.0.0:30500")
                .await
                .unwrap();

            let connections_task = Arc::clone(&connections);
            tokio::spawn(async move {
                loop {
                    match tcp_listener.accept().await {
                        Ok((stream, addr)) => {
                            tracing::info!("TCP connection from {}", addr);
                            connections_task.lock().unwrap().insert(addr);
                            tokio::spawn(async move {
                                let _stream = stream;
                                tokio::time::sleep(Duration::from_secs(10)).await;
                            });
                        }
                        Err(_) => break,
                    }
                }
            });

            let mut buf = vec![0u8; 65535];
            let mut next_multicast_session_id = 1;
            let mut next_unicast_session_id = 1;

            loop {
                let (len, from) = match tokio::time::timeout(
                    Duration::from_secs(5),
                    sd_socket.recv_from(&mut buf),
                )
                .await
                {
                    Ok(Ok(result)) => result,
                    Ok(Err(e)) => {
                        tracing::error!("Socket error: {}", e);
                        continue;
                    }
                    Err(_) => break, // Timeout, end test
                };

                let data = &buf[..len];

                if data.len() < 16 {
                    continue;
                }
                let service_id = u16::from_be_bytes([data[0], data[1]]);
                let method_id = u16::from_be_bytes([data[2], data[3]]);

                if service_id != 0xFFFF || method_id != 0x8100 {
                    continue;
                }

                if data.len() < 36 {
                    continue;
                }
                let entry_type = data[24];

                match entry_type {
                    0x00 => {
                        // FindService - offer both services S1 and S2 via TCP
                        let offer_s1 = build_sd_offer(
                            0x1111, // S1
                            0x0001,
                            1,
                            0,
                            lookup_ipv4("server"),
                            30500,
                            0x06, // TCP
                            0xFFFFFF,
                            next_multicast_session_id,
                            true,  // reboot_flag
                            false, // unicast_flag (multicast)
                        );
                        next_multicast_session_id += 1;
                        let _ = sd_socket.send_to(&offer_s1, from).await;
                        tokio::time::sleep(Duration::from_millis(93)).await;

                        let offer_s2 = build_sd_offer(
                            0x2222, // S2
                            0x0001,
                            1,
                            0,
                            lookup_ipv4("server"),
                            30500,
                            0x06, // TCP
                            0xFFFFFF,
                            next_multicast_session_id,
                            true,  // reboot_flag
                            false, // unicast_flag (multicast)
                        );
                        next_multicast_session_id += 1;
                        let _ = sd_socket.send_to(&offer_s2, from).await;
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    0x06 => {
                        // SubscribeEventgroup
                        let sub_service_id = u16::from_be_bytes([data[28], data[29]]);
                        let instance_id = u16::from_be_bytes([data[30], data[31]]);
                        let major_version = data[32];
                        let eventgroup_id = u16::from_be_bytes([data[38], data[39]]);
                        let ttl_bytes = [0, data[33], data[34], data[35]];
                        let ttl = u32::from_be_bytes(ttl_bytes);

                        tracing::info!(
                            "Subscription for service {:04x} eventgroup {:04x}",
                            sub_service_id,
                            eventgroup_id
                        );

                        subscriptions
                            .lock()
                            .unwrap()
                            .insert((sub_service_id, eventgroup_id));

                        let ack = build_sd_subscribe_ack(
                            sub_service_id,
                            instance_id,
                            major_version,
                            eventgroup_id,
                            ttl,
                            next_unicast_session_id,
                            true, // reboot_flag
                        );
                        next_unicast_session_id += 1;
                        let _ = sd_socket.send_to(&ack, from).await;
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    _ => {}
                }

                // After receiving all 4 subscriptions, exit
                if subscriptions.lock().unwrap().len() >= 4 {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    break;
                }
            }

            tracing::info!(
                "Server observed {} TCP connection(s) for {} subscriptions",
                connections.lock().unwrap().len(),
                subscriptions.lock().unwrap().len()
            );

            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client"))
            .preferred_transport(recentip::config::Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        // Discover S1
        let proxy_s1 = runtime
            .find(0x1111)
            .instance(recentip::InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Subscribe to S1 EG1 (contains Event1)
        let _sub_s1_eg1 = proxy_s1
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0001).unwrap())
            .subscribe()
            .await
            .unwrap();

        // Subscribe to S1 EG2 (contains Event1 + Event2)
        let _sub_s1_eg2 = proxy_s1
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0002).unwrap())
            .subscribe()
            .await
            .unwrap();

        // Discover S2
        let proxy_s2 = runtime
            .find(0x2222)
            .instance(recentip::InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Subscribe to S2 EG1 (contains Event1)
        let _sub_s2_eg1 = proxy_s2
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0001).unwrap())
            .subscribe()
            .await
            .unwrap();

        // Subscribe to S2 EG2 (contains Event1 + Event2)
        let _sub_s2_eg2 = proxy_s2
            .new_subscription()
            .eventgroup(recentip::EventgroupId::new(0x0002).unwrap())
            .subscribe()
            .await
            .unwrap();

        // Keep subscriptions alive
        tokio::time::sleep(Duration::from_secs(2)).await;

        Ok(())
    });

    sim.run().unwrap();

    let observed_connections = connections_observed.lock().unwrap();
    let subs = subscriptions_received.lock().unwrap();

    // Verify we got all 4 subscriptions
    assert_eq!(
        subs.len(),
        4,
        "Expected 4 subscriptions (2 services × 2 eventgroups). Got: {}",
        subs.len()
    );

    // TCP connection reuse strategy:
    // - Events contain service_id in header → easy to route across services
    // - Multiple eventgroups within SAME service are harder to distinguish
    //
    // Expected: share connections across services, separate by eventgroup "slot"
    // - Connection A: S1-EG1 + S2-EG1 (same eventgroup, different services)
    // - Connection B: S1-EG2 + S2-EG2 (same eventgroup, different services)
    // → 2 connections total
    assert_eq!(
        observed_connections.len(),
        2,
        "Expected 2 TCP connections (share across services, separate by eventgroup). \
         Received subscriptions on {} connections: {:?}",
        observed_connections.len(),
        observed_connections
    );

    tracing::info!(
        "✓ Multiple eventgroups TCP test: {} connections for 4 subscriptions",
        observed_connections.len()
    );
}
