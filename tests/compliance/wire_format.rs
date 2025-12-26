//! Wire Capture Tests
//!
//! Tests that use raw sockets on one side to capture actual network traffic
//! and verify the raw bytes match SOME/IP specification requirements.
//! 
//! Unlike unit tests that construct headers manually, these tests run real
//! protocol exchanges where one side uses the library and the other side
//! uses a raw UDP socket to directly inspect wire bytes.

use bytes::Bytes;
use someip_runtime::prelude::*;
use someip_runtime::runtime::{Runtime, RuntimeConfig};
use someip_runtime::wire::{Header, MessageType, SdMessage, SD_METHOD_ID, SD_SERVICE_ID};
use someip_runtime::handle::ServiceEvent;
use std::net::SocketAddr;
use std::time::Duration;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

/// Test service definition
struct TestService;

impl Service for TestService {
    const SERVICE_ID: u16 = 0x1234;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}

/// Helper to parse a SOME/IP header from raw bytes
fn parse_header(data: &[u8]) -> Option<Header> {
    if data.len() < Header::SIZE {
        return None;
    }
    Header::parse(&mut Bytes::copy_from_slice(data))
}

/// Helper to parse an SD message (header + payload) from raw bytes
fn parse_sd_message(data: &[u8]) -> Option<(Header, SdMessage)> {
    if data.len() < Header::SIZE {
        return None;
    }
    let mut bytes = Bytes::copy_from_slice(data);
    let header = Header::parse(&mut bytes)?;
    if header.service_id == SD_SERVICE_ID && header.method_id == SD_METHOD_ID {
        let sd_msg = SdMessage::parse(&mut bytes)?;
        Some((header, sd_msg))
    } else {
        None
    }
}

// ============================================================================
// SD WIRE FORMAT TESTS
// ============================================================================

/// feat_req_recentipsd_141: SD Service ID is 0xFFFF
/// feat_req_recentipsd_142: SD Method ID is 0x8100
/// feat_req_recentipsd_144: SD Client ID is 0x0000
/// feat_req_recentip_103: SD uses NOTIFICATION (0x02) message type
#[test]
fn sd_offer_wire_format() {
    covers!(
        feat_req_recentipsd_141,
        feat_req_recentipsd_142,
        feat_req_recentipsd_144,
        feat_req_recentip_103
    );

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library side - offers a service
    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        // Offer a service - this sends SD Offer messages
        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Keep server alive
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    // Raw socket side - captures SD multicast
    sim.client("raw_observer", async move {
        // Bind to SD multicast port
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        // Join multicast group for SD
        socket.join_multicast_v4(
            "239.255.0.1".parse().unwrap(),
            "0.0.0.0".parse().unwrap(),
        )?;

        let mut buf = [0u8; 1500];
        let mut captured_sd = Vec::new();

        // Capture a few SD messages
        for _ in 0..3 {
            let result = tokio::time::timeout(
                Duration::from_millis(200),
                socket.recv_from(&mut buf),
            ).await;
            
            if let Ok(Ok((len, _from))) = result {
                if let Some((header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    captured_sd.push((header, sd_msg));
                }
            }
        }

        // Verify captured SD offers
        assert!(!captured_sd.is_empty(), "Should have captured SD messages");

        for (header, _sd_msg) in &captured_sd {
            // Verify SD header constants per specification
            assert_eq!(
                header.service_id, 0xFFFF,
                "SD Service ID must be 0xFFFF (feat_req_recentipsd_141)"
            );
            assert_eq!(
                header.method_id, 0x8100,
                "SD Method ID must be 0x8100 (feat_req_recentipsd_142)"
            );
            assert_eq!(
                header.client_id, 0x0000,
                "SD Client ID must be 0x0000 (feat_req_recentipsd_144)"
            );
            assert_eq!(
                header.message_type,
                MessageType::Notification,
                "SD uses NOTIFICATION message type (feat_req_recentip_103)"
            );
            assert_eq!(
                header.protocol_version, 0x01,
                "Protocol version must be 0x01"
            );
        }

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentipsd_147: OfferService entry type = 0x01
#[test]
fn sd_offer_entry_type_wire_format() {
    covers!(feat_req_recentipsd_147);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library side - offers a service
    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    // Raw socket captures and verifies OfferService entry
    sim.client("raw_observer", async move {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        socket.join_multicast_v4(
            "239.255.0.1".parse().unwrap(),
            "0.0.0.0".parse().unwrap(),
        )?;

        let mut buf = [0u8; 1500];
        let mut found_offer = false;

        for _ in 0..5 {
            let result = tokio::time::timeout(
                Duration::from_millis(200),
                socket.recv_from(&mut buf),
            ).await;
            
            if let Ok(Ok((len, _from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 {
                            // OfferService
                            found_offer = true;
                            assert_eq!(
                                entry.service_id, 0x1234,
                                "OfferService should advertise our service"
                            );
                        }
                    }
                }
            }
        }

        assert!(found_offer, "Server should send OfferService entries (type 0x01)");
        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// RPC WIRE FORMAT TESTS
// ============================================================================

/// feat_req_recentip_103: REQUEST (0x00) message type on wire
/// feat_req_recentip_60: Message ID = Service ID || Method ID
/// feat_req_recentip_45: Header is exactly 16 bytes
#[test]
fn rpc_request_wire_format() {
    covers!(feat_req_recentip_103, feat_req_recentip_60, feat_req_recentip_45);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Raw socket acts as server - sends SD offer, then receives request bytes
    sim.host("raw_server", || async {
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_server")
            .to_string()
            .parse()
            .unwrap();

        // RPC socket to receive requests
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?;
        eprintln!("Raw server RPC listening on {}", rpc_socket.local_addr()?);

        // SD socket to send offers
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;
        
        // Send SD offer to multicast so the library can discover us
        let offer = build_sd_offer(0x1234, 0x0001, 1, 0, my_ip, 30509, 3600);
        let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();
        
        // Send offers periodically until we receive a request
        let mut buf = [0u8; 1500];
        let mut request_received = false;
        
        for _ in 0..20 {
            // Send an offer
            sd_socket.send_to(&offer, sd_multicast).await?;
            eprintln!("Raw server sent SD offer");
            
            // Check for RPC request (non-blocking with short timeout)
            let result = tokio::time::timeout(
                Duration::from_millis(200),
                rpc_socket.recv_from(&mut buf),
            ).await;
            
            if let Ok(Ok((len, from))) = result {
                eprintln!("Raw server received {} bytes from {}", len, from);
                request_received = true;

                // Verify raw bytes directly
                let data = &buf[..len];
                
                // Header must be at least 16 bytes (feat_req_recentip_45)
                assert!(
                    len >= 16,
                    "SOME/IP packet must be at least 16 bytes (header), got {}",
                    len
                );

                // Parse and verify header
                let header = parse_header(data).expect("Should parse as SOME/IP header");

                // Verify Service ID (feat_req_recentip_60)
                assert_eq!(
                    header.service_id, 0x1234,
                    "Service ID should be 0x1234"
                );

                // Verify Method ID (feat_req_recentip_60)
                assert_eq!(
                    header.method_id, 0x0001,
                    "Method ID should be 0x0001"
                );

                // Verify REQUEST message type (feat_req_recentip_103)
                assert_eq!(
                    header.message_type as u8, 0x00,
                    "REQUEST message type must be 0x00"
                );

                // Verify big-endian encoding directly on wire
                assert_eq!(
                    u16::from_be_bytes([data[0], data[1]]),
                    0x1234,
                    "Service ID must be big-endian on wire"
                );
                assert_eq!(
                    u16::from_be_bytes([data[2], data[3]]),
                    0x0001,
                    "Method ID must be big-endian on wire"
                );

                // Send a response back
                let response = build_response(&header, b"ok");
                rpc_socket.send_to(&response, from).await?;
                break;
            }
        }

        assert!(request_received, "Should have received an RPC request");
        Ok(())
    });

    // Library side - discovers service via SD and makes RPC call using public API
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::default();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        // Use public API: find service and wait for availability via SD
        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        
        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.available(),
        ).await.expect("Should discover service via SD");

        // Make an RPC call using public API
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            proxy.call(MethodId::new(0x0001), b"hello"),
        ).await;
        
        assert!(result.is_ok(), "RPC call should complete");
        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentip_103: RESPONSE (0x80) message type on wire
/// feat_req_recentip_711: Response preserves Request ID
#[test]
fn rpc_response_wire_format() {
    covers!(feat_req_recentip_103, feat_req_recentip_711);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library side - server that responds
    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        let mut offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        // Handle one call
        if let Some(event) = offering.next().await {
            if let ServiceEvent::Call { responder, .. } = event {
                responder.reply(b"response_payload").await.unwrap();
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });

    // Raw socket sends request and captures response
    sim.client("raw_client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // First, listen for SD to find the server
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4(
            "239.255.0.1".parse().unwrap(),
            "0.0.0.0".parse().unwrap(),
        )?;

        // Find server endpoint from SD offer
        let mut server_endpoint: Option<SocketAddr> = None;
        let mut buf = [0u8; 1500];
        
        for _ in 0..10 {
            let result = tokio::time::timeout(
                Duration::from_millis(200),
                sd_socket.recv_from(&mut buf),
            ).await;
            
            if let Ok(Ok((len, from))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == 0x1234 {
                            // Found our service offer
                            // Use the source address with the offered port
                            if let Some(opt) = sd_msg.options.first() {
                                if let someip_runtime::wire::SdOption::Ipv4Endpoint { addr, port, .. } = opt {
                                    let ip = if addr.is_unspecified() {
                                        from.ip()
                                    } else {
                                        std::net::IpAddr::V4(*addr)
                                    };
                                    server_endpoint = Some(SocketAddr::new(ip, *port));
                                }
                            }
                        }
                    }
                }
            }
            if server_endpoint.is_some() {
                break;
            }
        }

        let server_addr = server_endpoint.expect("Should find server via SD");
        eprintln!("Found server at {}", server_addr);

        // Now send a raw request
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;
        
        let request = build_request(0x1234, 0x0001, 0xABCD, 0x1234, b"test_payload");
        rpc_socket.send_to(&request, server_addr).await?;

        // Receive response
        let (len, _from) = tokio::time::timeout(
            Duration::from_secs(3),
            rpc_socket.recv_from(&mut buf),
        ).await??;

        let response_data = &buf[..len];
        let response_header = parse_header(response_data).expect("Should parse response");

        // Verify RESPONSE message type (feat_req_recentip_103)
        assert_eq!(
            response_header.message_type as u8, 0x80,
            "RESPONSE message type must be 0x80"
        );

        // Verify Request ID preservation (feat_req_recentip_711)
        assert_eq!(
            response_header.client_id, 0xABCD,
            "Response must preserve Client ID from request"
        );
        assert_eq!(
            response_header.session_id, 0x1234,
            "Response must preserve Session ID from request"
        );

        // Verify Service/Method preserved
        assert_eq!(response_header.service_id, 0x1234);
        assert_eq!(response_header.method_id, 0x0001);

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentip_45: SOME/IP header is exactly 16 bytes
/// feat_req_recentip_42: Big-endian encoding verification
#[test]
fn header_size_and_endianness_on_wire() {
    covers!(feat_req_recentip_45, feat_req_recentip_42);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Library side - offers a service (sends SD)
    sim.host("server", || async {
        let config = RuntimeConfig::default();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        let _offering = runtime
            .offer::<TestService>(InstanceId::Id(0x0001))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(())
    });

    // Raw socket verifies byte-level structure
    sim.client("raw_observer", async move {
        let socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        socket.join_multicast_v4(
            "239.255.0.1".parse().unwrap(),
            "0.0.0.0".parse().unwrap(),
        )?;

        let mut buf = [0u8; 1500];
        
        let (len, _from) = tokio::time::timeout(
            Duration::from_millis(500),
            socket.recv_from(&mut buf),
        ).await??;

        let data = &buf[..len];

        // All SOME/IP packets must be at least 16 bytes (feat_req_recentip_45)
        assert!(
            len >= 16,
            "SOME/IP packet must be at least 16 bytes (header), got {}",
            len
        );

        // Verify big-endian encoding (feat_req_recentip_42):
        // SD Service ID = 0xFFFF in bytes [0..2]
        let service_id = u16::from_be_bytes([data[0], data[1]]);
        assert_eq!(service_id, 0xFFFF, "SD Service ID must be 0xFFFF");

        // SD Method ID = 0x8100 in bytes [2..4]
        let method_id = u16::from_be_bytes([data[2], data[3]]);
        assert_eq!(method_id, 0x8100, "SD Method ID must be 0x8100");

        // Length field in bytes [4..8]
        let length = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        assert!(
            length >= 8,
            "Length field must be >= 8 (remaining header), got {}",
            length
        );

        // Protocol Version in byte [12]
        assert_eq!(data[12], 0x01, "Protocol Version must be 0x01");

        // Interface Version in byte [13]
        assert_eq!(data[13], 0x01, "Interface Version must be 0x01 for SD");

        // Message Type in byte [14] - NOTIFICATION = 0x02
        assert_eq!(data[14], 0x02, "SD Message Type must be NOTIFICATION (0x02)");

        // Return Code in byte [15] - E_OK = 0x00
        assert_eq!(data[15], 0x00, "Return Code must be E_OK (0x00)");

        Ok(())
    });

    sim.run().unwrap();
}

/// feat_req_recentip_677: Session ID increments across calls
/// feat_req_recentip_649: Session ID starts at non-zero value
#[test]
fn session_id_increment_on_wire() {
    covers!(feat_req_recentip_677, feat_req_recentip_649);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    // Raw socket acts as server - sends SD offer, receives multiple requests
    sim.host("raw_server", || async {
        let my_ip: std::net::Ipv4Addr = turmoil::lookup("raw_server")
            .to_string()
            .parse()
            .unwrap();

        // RPC socket to receive requests
        let rpc_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30509").await?;
        
        // SD socket to send offers
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:0").await?;
        let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();
        
        let offer = build_sd_offer(0x1234, 0x0001, 1, 0, my_ip, 30509, 3600);
        
        let mut buf = [0u8; 1500];
        let mut session_ids = Vec::new();

        // Send offers and receive requests
        for _ in 0..30 {
            // Send an offer periodically
            sd_socket.send_to(&offer, sd_multicast).await?;
            
            // Check for RPC request
            let result = tokio::time::timeout(
                Duration::from_millis(200),
                rpc_socket.recv_from(&mut buf),
            ).await;
            
            if let Ok(Ok((len, from))) = result {
                let header = parse_header(&buf[..len]).expect("Should parse header");
                session_ids.push(header.session_id);
                eprintln!("Request {}: session_id = {}", session_ids.len(), header.session_id);
                
                // Send response
                let response = build_response(&header, b"ok");
                rpc_socket.send_to(&response, from).await?;
                
                // Stop after 3 requests
                if session_ids.len() >= 3 {
                    break;
                }
            }
        }

        // Verify session IDs
        assert!(
            session_ids.len() >= 3,
            "Should have received 3 requests, got {}",
            session_ids.len()
        );

        // Verify non-zero (feat_req_recentip_649)
        for sid in &session_ids {
            assert!(*sid > 0, "Session ID must be non-zero");
        }

        // Verify incrementing (feat_req_recentip_677)
        for i in 1..session_ids.len() {
            let prev = session_ids[i - 1];
            let curr = session_ids[i];
            assert!(
                curr > prev || (prev == 0xFFFF && curr == 0x0001),
                "Session ID must increment (got {} after {})",
                curr,
                prev
            );
        }

        Ok(())
    });

    // Library side - discovers service via SD and makes multiple calls using public API
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = RuntimeConfig::default();
        let runtime: Runtime<turmoil::net::UdpSocket> =
            Runtime::with_socket_type(config).await.unwrap();

        // Use public API: find service and wait for availability via SD
        let proxy = runtime.find::<TestService>(InstanceId::Id(0x0001));
        
        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.available(),
        ).await.expect("Should discover service via SD");

        // Make 3 calls using public API
        for _ in 0..3 {
            let _ = tokio::time::timeout(
                Duration::from_secs(3),
                proxy.call(MethodId::new(0x0001), b"test"),
            ).await.expect("Timeout").expect("Call should succeed");
        }

        Ok(())
    });

    sim.run().unwrap();
}

// ============================================================================
// Helper functions for building raw SOME/IP packets
// ============================================================================

/// Build a raw SOME/IP request packet
fn build_request(
    service_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    payload: &[u8],
) -> Vec<u8> {
    let length = 8 + payload.len() as u32; // 8 bytes of header after length + payload
    let mut packet = Vec::with_capacity(16 + payload.len());
    
    // Message ID (Service ID + Method ID)
    packet.extend_from_slice(&service_id.to_be_bytes());
    packet.extend_from_slice(&method_id.to_be_bytes());
    
    // Length
    packet.extend_from_slice(&length.to_be_bytes());
    
    // Request ID (Client ID + Session ID)
    packet.extend_from_slice(&client_id.to_be_bytes());
    packet.extend_from_slice(&session_id.to_be_bytes());
    
    // Protocol Version
    packet.push(0x01);
    
    // Interface Version
    packet.push(0x01);
    
    // Message Type (REQUEST = 0x00)
    packet.push(0x00);
    
    // Return Code (E_OK = 0x00)
    packet.push(0x00);
    
    // Payload
    packet.extend_from_slice(payload);
    
    packet
}

/// Build a raw SOME/IP response packet based on a request header
fn build_response(request: &Header, payload: &[u8]) -> Vec<u8> {
    let length = 8 + payload.len() as u32;
    let mut packet = Vec::with_capacity(16 + payload.len());
    
    // Message ID (preserve from request)
    packet.extend_from_slice(&request.service_id.to_be_bytes());
    packet.extend_from_slice(&request.method_id.to_be_bytes());
    
    // Length
    packet.extend_from_slice(&length.to_be_bytes());
    
    // Request ID (preserve from request)
    packet.extend_from_slice(&request.client_id.to_be_bytes());
    packet.extend_from_slice(&request.session_id.to_be_bytes());
    
    // Protocol Version
    packet.push(0x01);
    
    // Interface Version
    packet.push(request.interface_version);
    
    // Message Type (RESPONSE = 0x80)
    packet.push(0x80);
    
    // Return Code (E_OK = 0x00)
    packet.push(0x00);
    
    // Payload
    packet.extend_from_slice(payload);
    
    packet
}

/// Build a raw SOME/IP-SD OfferService message
fn build_sd_offer(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    minor_version: u32,
    endpoint_ip: std::net::Ipv4Addr,
    endpoint_port: u16,
    ttl: u32,
) -> Vec<u8> {
    // SD message structure:
    // - SOME/IP header (16 bytes)
    // - SD header (12 bytes): flags(1) + reserved(3) + entries_length(4) + entries + options_length(4) + options
    // - Entry (16 bytes): OfferService entry
    // - Option (12 bytes): IPv4 Endpoint option
    
    let mut packet = Vec::with_capacity(56);
    
    // === SOME/IP Header (16 bytes) ===
    // Service ID = 0xFFFF (SD)
    packet.extend_from_slice(&0xFFFFu16.to_be_bytes());
    // Method ID = 0x8100 (SD)
    packet.extend_from_slice(&0x8100u16.to_be_bytes());
    // Length = 8 (remaining header) + SD payload
    // We'll fill this in at the end
    let length_offset = packet.len();
    packet.extend_from_slice(&0u32.to_be_bytes()); // placeholder
    // Client ID
    packet.extend_from_slice(&0x0000u16.to_be_bytes());
    // Session ID
    packet.extend_from_slice(&0x0001u16.to_be_bytes());
    // Protocol Version
    packet.push(0x01);
    // Interface Version
    packet.push(0x01);
    // Message Type = NOTIFICATION (0x02)
    packet.push(0x02);
    // Return Code = E_OK
    packet.push(0x00);
    
    // === SD Payload ===
    // Flags (1 byte): Unicast flag = 0x40, Reboot = 0x80
    packet.push(0xC0);
    // Reserved (3 bytes)
    packet.extend_from_slice(&[0x00, 0x00, 0x00]);
    
    // Entries array length (4 bytes) - 16 bytes for one entry
    packet.extend_from_slice(&16u32.to_be_bytes());
    
    // === OfferService Entry (16 bytes) ===
    // Type = OfferService (0x01)
    packet.push(0x01);
    // Index 1st options = 0 (first option)
    packet.push(0x00);
    // Index 2nd options = 0
    packet.push(0x00);
    // # of opt 1 (4 bits) | # of opt 2 (4 bits) = 0x10 (1 option in run 1)
    packet.push(0x10);
    // Service ID
    packet.extend_from_slice(&service_id.to_be_bytes());
    // Instance ID
    packet.extend_from_slice(&instance_id.to_be_bytes());
    // Major Version
    packet.push(major_version);
    // TTL (24-bit, big-endian)
    let ttl_bytes = ttl.to_be_bytes();
    packet.extend_from_slice(&ttl_bytes[1..4]); // 3 bytes
    // Minor Version (32-bit)
    packet.extend_from_slice(&minor_version.to_be_bytes());
    
    // Options array length (4 bytes) - 12 bytes for IPv4 endpoint
    packet.extend_from_slice(&12u32.to_be_bytes());
    
    // === IPv4 Endpoint Option (12 bytes) ===
    // Length = 9 (option content length, excluding length+type)
    packet.extend_from_slice(&9u16.to_be_bytes());
    // Type = IPv4 Endpoint (0x04)
    packet.push(0x04);
    // Reserved
    packet.push(0x00);
    // IPv4 address
    packet.extend_from_slice(&endpoint_ip.octets());
    // Reserved
    packet.push(0x00);
    // L4 Protocol = UDP (0x11)
    packet.push(0x11);
    // Port
    packet.extend_from_slice(&endpoint_port.to_be_bytes());
    
    // Fix up length field: total - 8 (first 8 bytes of header)
    let length = (packet.len() - 8) as u32;
    packet[length_offset..length_offset + 4].copy_from_slice(&length.to_be_bytes());
    
    packet
}
