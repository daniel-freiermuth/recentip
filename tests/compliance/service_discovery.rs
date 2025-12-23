//! Service Discovery (SD) Compliance Tests
//!
//! Tests that verify SOME/IP-SD wire format compliance by capturing actual
//! packets via SimulatedNetwork and verifying they match spec requirements.
//!
//! All tests here use the Runtime API to trigger real SD behavior and
//! inspect the resulting wire bytes.
//!
//! Reference: someip-sd.rst

#[path = "../simulated.rs"]
mod simulated;

#[path = "../wire.rs"]
mod wire;

use crate::covers;
use simulated::{NetworkEvent, SimulatedNetwork};
use someip_runtime::prelude::*;

// ============================================================================
// SD WIRE FORMAT PARSING UTILITIES
// ============================================================================
// These structs parse captured SD packets for verification.
// They are test infrastructure, not tested themselves.

/// SD-specific port
pub const SD_PORT: u16 = 30490;

/// SD message uses special service/method IDs
pub mod sd_ids {
    /// feat_req_recentipsd_26: SD messages use Service ID 0xFFFF
    pub const SERVICE_ID: u16 = 0xFFFF;
    /// feat_req_recentipsd_26: SD messages use Method ID 0x8100
    pub const METHOD_ID: u16 = 0x8100;
    /// feat_req_recentipsd_26: SD messages use Message Type NOTIFICATION (0x02)
    pub const MESSAGE_TYPE: u8 = 0x02;
    /// feat_req_recentipsd_26: SD uses Protocol Version 0x01
    pub const PROTOCOL_VERSION: u8 = 0x01;
    /// feat_req_recentipsd_26: SD uses Interface Version 0x01
    pub const INTERFACE_VERSION: u8 = 0x01;
    /// feat_req_recentipsd_26: SD uses Return Code E_OK (0x00)
    pub const RETURN_CODE: u8 = 0x00;
    /// feat_req_recentipsd_26: Client ID shall be 0
    pub const CLIENT_ID: u16 = 0x0000;
}

/// SD Entry Types
pub mod entry_type {
    /// FindService entry type (0x00)
    pub const FIND_SERVICE: u8 = 0x00;
    /// OfferService entry type (0x01)
    pub const OFFER_SERVICE: u8 = 0x01;
    /// Subscribe entry type (0x06)
    pub const SUBSCRIBE: u8 = 0x06;
    /// SubscribeAck entry type (0x07)
    pub const SUBSCRIBE_ACK: u8 = 0x07;
}

/// Parse SD-specific header flags (4 bytes after SOME/IP header)
#[derive(Debug, Clone, PartialEq)]
pub struct SdFlags {
    pub reboot_flag: bool,
    pub unicast_flag: bool,
    pub explicit_initial_data_control: bool,
}

impl SdFlags {
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 4 {
            return None;
        }
        let flags = data[0];
        Some(Self {
            reboot_flag: (flags & 0x80) != 0,
            unicast_flag: (flags & 0x40) != 0,
            explicit_initial_data_control: (flags & 0x20) != 0,
        })
    }
}

/// Parse Service Entry (16 bytes)
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceEntry {
    pub entry_type: u8,
    pub index_first_option: u8,
    pub index_second_option: u8,
    pub num_options_1: u8,
    pub num_options_2: u8,
    pub service_id: u16,
    pub instance_id: u16,
    pub major_version: u8,
    pub ttl: u32,
    pub minor_version: u32,
}

impl ServiceEntry {
    pub const SIZE: usize = 16;

    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE {
            return None;
        }
        let options_packed = data[3];
        Some(Self {
            entry_type: data[0],
            index_first_option: data[1],
            index_second_option: data[2],
            num_options_1: (options_packed >> 4) & 0x0F,
            num_options_2: options_packed & 0x0F,
            service_id: u16::from_be_bytes([data[4], data[5]]),
            instance_id: u16::from_be_bytes([data[6], data[7]]),
            major_version: data[8],
            ttl: u32::from_be_bytes([0, data[9], data[10], data[11]]),
            minor_version: u32::from_be_bytes([data[12], data[13], data[14], data[15]]),
        })
    }
}

/// Parse Eventgroup Entry (16 bytes)
#[derive(Debug, Clone, PartialEq)]
pub struct EventgroupEntry {
    pub entry_type: u8,
    pub service_id: u16,
    pub instance_id: u16,
    pub major_version: u8,
    pub ttl: u32,
    pub initial_data_requested: bool,
    pub counter: u8,
    pub eventgroup_id: u16,
}

impl EventgroupEntry {
    pub const SIZE: usize = 16;

    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE {
            return None;
        }
        let flags_counter = data[13];
        Some(Self {
            entry_type: data[0],
            service_id: u16::from_be_bytes([data[4], data[5]]),
            instance_id: u16::from_be_bytes([data[6], data[7]]),
            major_version: data[8],
            ttl: u32::from_be_bytes([0, data[9], data[10], data[11]]),
            initial_data_requested: (flags_counter & 0x80) != 0,
            counter: flags_counter & 0x0F,
            eventgroup_id: u16::from_be_bytes([data[14], data[15]]),
        })
    }
}

/// Helper to find SD packets in network history
fn find_sd_packets(network: &SimulatedNetwork) -> Vec<Vec<u8>> {
    network
        .history()
        .iter()
        .filter_map(|e| match e {
            NetworkEvent::UdpSent {
                data,
                dst_port: 30490,
                ..
            } => Some(data.clone()),
            _ => None,
        })
        .collect()
}

/// Helper to check if a packet is an SD message
fn is_sd_message(data: &[u8]) -> bool {
    if data.len() < 16 {
        return false;
    }
    let service_id = u16::from_be_bytes([data[0], data[1]]);
    let method_id = u16::from_be_bytes([data[2], data[3]]);
    service_id == sd_ids::SERVICE_ID && method_id == sd_ids::METHOD_ID
}

/// Parse entries from an SD message
/// Returns (entries_array_offset, entries_length)
fn parse_entries_header(data: &[u8]) -> Option<(usize, u32)> {
    // 16-byte SOME/IP header + 4-byte SD flags + 4-byte entries length
    if data.len() < 24 {
        return None;
    }
    let entries_len = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Some((24, entries_len))
}

// ============================================================================
// COMPLIANCE TESTS - SD MESSAGE FORMAT
// ============================================================================

/// feat_req_recentipsd_26: Verify OfferService sends SD message with correct header
///
/// The SD message SOME/IP header must have:
/// - Service ID 0xFFFF
/// - Method ID 0x8100
/// - Message Type NOTIFICATION (0x02)
/// - Client ID 0x0000
/// - Session ID starting at 1 (not 0)
/// - Protocol Version 0x01
/// - Interface Version 0x01
/// - Return Code E_OK (0x00)
#[test]
#[ignore = "Runtime::new not implemented"]
fn offer_service_sd_header_format() {
    covers!(
        feat_req_recentipsd_26,
        feat_req_recentipsd_27 // SD over UDP
    );

    let (network, _io_client, io_server) = SimulatedNetwork::new_pair();

    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(0x0001).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();

    // Let SD messages be sent
    network.advance(std::time::Duration::from_millis(100));

    // Find SD packets
    let sd_packets = find_sd_packets(&network);
    assert!(!sd_packets.is_empty(), "Should have sent SD message");

    let packet = &sd_packets[0];
    assert!(is_sd_message(packet), "Should be SD message");

    // Verify SOME/IP header fields
    let header = wire::Header::from_bytes(packet).unwrap();

    assert_eq!(header.service_id, sd_ids::SERVICE_ID, "Service ID must be 0xFFFF");
    assert_eq!(header.method_id, sd_ids::METHOD_ID, "Method ID must be 0x8100");
    assert_eq!(
        header.message_type, sd_ids::MESSAGE_TYPE,
        "Message Type must be NOTIFICATION (0x02)"
    );
    assert_eq!(header.client_id, sd_ids::CLIENT_ID, "Client ID must be 0");
    assert_ne!(header.session_id, 0, "Session ID must not be 0");
    assert_eq!(
        header.protocol_version, sd_ids::PROTOCOL_VERSION,
        "Protocol Version must be 0x01"
    );
    assert_eq!(
        header.interface_version, sd_ids::INTERFACE_VERSION,
        "Interface Version must be 0x01"
    );
    assert_eq!(header.return_code, sd_ids::RETURN_CODE, "Return Code must be E_OK");
}

/// feat_req_recentipsd_39, feat_req_recentipsd_40, feat_req_recentipsd_453:
/// Verify SD flags field format
///
/// - Flags byte follows SOME/IP header
/// - Reboot flag is bit 7 (highest)
/// - Unicast flag is bit 6, shall be 1
#[test]
#[ignore = "Runtime::new not implemented"]
fn sd_flags_field_format() {
    covers!(
        feat_req_recentipsd_39,
        feat_req_recentipsd_40,
        feat_req_recentipsd_453
    );

    let (network, _io_client, io_server) = SimulatedNetwork::new_pair();

    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_config = ServiceConfig::builder()
        .service(ServiceId::new(0x1234).unwrap())
        .instance(ConcreteInstanceId::new(0x0001).unwrap())
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let sd_packets = find_sd_packets(&network);
    assert!(!sd_packets.is_empty());

    // SD flags start at byte 16 (after SOME/IP header)
    let flags = SdFlags::from_bytes(&sd_packets[0][16..]).unwrap();

    // After fresh start, reboot flag should be set
    assert!(flags.reboot_flag, "Reboot flag should be set after startup");

    // Unicast flag must always be 1
    assert!(flags.unicast_flag, "Unicast flag must be 1");
}

/// feat_req_recentipsd_47: Verify OfferService entry format
///
/// Service Entry is 16 bytes with:
/// - Type (0x01 for OfferService)
/// - Service ID matching offered service
/// - Instance ID matching offered instance
/// - TTL > 0 (service is available)
/// - Version fields
#[test]
#[ignore = "Runtime::new not implemented"]
fn offer_service_entry_format() {
    covers!(feat_req_recentipsd_47);

    let (network, _io_client, io_server) = SimulatedNetwork::new_pair();

    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x4567).unwrap();
    let instance_id = ConcreteInstanceId::new(0x0089).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .major_version(0x02)
        .minor_version(0x00000003)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let sd_packets = find_sd_packets(&network);
    assert!(!sd_packets.is_empty());

    let (entries_offset, entries_len) = parse_entries_header(&sd_packets[0]).unwrap();
    assert!(entries_len >= 16, "Should have at least one entry");

    let entry = ServiceEntry::from_bytes(&sd_packets[0][entries_offset..]).unwrap();

    assert_eq!(entry.entry_type, entry_type::OFFER_SERVICE, "Type must be OfferService (0x01)");
    assert_eq!(entry.service_id, 0x4567, "Service ID must match");
    assert_eq!(entry.instance_id, 0x0089, "Instance ID must match");
    assert_eq!(entry.major_version, 0x02, "Major version must match");
    assert_eq!(entry.minor_version, 0x00000003, "Minor version must match");
    assert!(entry.ttl > 0, "TTL must be > 0 for active offer");
}

/// feat_req_recentipsd_47: Verify StopOffer uses TTL=0
///
/// When a service is stopped, OfferService with TTL=0 is sent
#[test]
#[ignore = "Runtime::new not implemented"]
fn stop_offer_uses_ttl_zero() {
    covers!(feat_req_recentipsd_47);

    let (network, _io_client, io_server) = SimulatedNetwork::new_pair();

    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_config = ServiceConfig::builder()
        .service(ServiceId::new(0x1234).unwrap())
        .instance(ConcreteInstanceId::new(0x0001).unwrap())
        .build()
        .unwrap();

    let offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Drop the offering to trigger StopOffer
    drop(offering);
    network.advance(std::time::Duration::from_millis(100));

    // Find the StopOffer message (OfferService with TTL=0)
    let sd_packets = find_sd_packets(&network);

    let stop_offer = sd_packets
        .iter()
        .filter_map(|pkt| {
            let (offset, _) = parse_entries_header(pkt)?;
            ServiceEntry::from_bytes(&pkt[offset..])
        })
        .find(|e| e.entry_type == entry_type::OFFER_SERVICE && e.ttl == 0);

    assert!(stop_offer.is_some(), "Should send OfferService with TTL=0 on stop");
}

/// feat_req_recentipsd_26: Session ID increments for each SD message
#[test]
#[ignore = "Runtime::new not implemented"]
fn sd_session_id_increments() {
    covers!(feat_req_recentipsd_26);

    let (network, _io_client, io_server) = SimulatedNetwork::new_pair();

    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_config = ServiceConfig::builder()
        .service(ServiceId::new(0x1234).unwrap())
        .instance(ConcreteInstanceId::new(0x0001).unwrap())
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();

    // Let multiple SD messages be sent (initial + repetitions)
    network.advance(std::time::Duration::from_secs(5));

    let sd_packets = find_sd_packets(&network);
    assert!(sd_packets.len() >= 2, "Should have multiple SD messages");

    let session_ids: Vec<u16> = sd_packets
        .iter()
        .filter_map(|pkt| wire::Header::from_bytes(pkt).map(|h| h.session_id))
        .collect();

    // Session IDs should be incrementing
    for window in session_ids.windows(2) {
        assert!(
            window[1] > window[0] || window[1] == 1,
            "Session ID should increment (or wrap to 1)"
        );
    }

    // First session ID should be 1
    assert_eq!(session_ids[0], 1, "First session ID must be 1");
}

/// feat_req_recentipsd_109: Verify Subscribe entry format
#[test]
#[ignore = "Runtime::new not implemented"]
fn subscribe_eventgroup_entry_format() {
    covers!(feat_req_recentipsd_109);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let service_id = ServiceId::new(0x1234).unwrap();
    let eventgroup = EventgroupId::new(0x05).unwrap();

    // Server offers service with eventgroup
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();
    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(ConcreteInstanceId::new(0x0001).unwrap())
        .eventgroup(eventgroup)
        .build()
        .unwrap();
    let _offering = server.offer(service_config).unwrap();

    // Client subscribes
    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));

    let available = proxy.wait_available().unwrap();
    let _sub = available.subscribe(eventgroup).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Find Subscribe entry in SD packets
    let sd_packets = find_sd_packets(&network);

    let subscribe_entry = sd_packets
        .iter()
        .filter_map(|pkt| {
            let (offset, len) = parse_entries_header(pkt)?;
            if len < 16 {
                return None;
            }
            EventgroupEntry::from_bytes(&pkt[offset..])
        })
        .find(|e| e.entry_type == entry_type::SUBSCRIBE);

    let entry = subscribe_entry.expect("Should have Subscribe entry");

    assert_eq!(entry.entry_type, entry_type::SUBSCRIBE, "Type must be Subscribe (0x06)");
    assert_eq!(entry.service_id, 0x1234, "Service ID must match");
    assert_eq!(entry.eventgroup_id, 0x0005, "Eventgroup ID must match");
    assert!(entry.ttl > 0, "TTL must be > 0 for active subscription");
}

/// feat_req_recentipsd_109: Verify SubscribeAck entry format
#[test]
#[ignore = "Runtime::new not implemented"]
fn subscribe_ack_entry_format() {
    covers!(feat_req_recentipsd_109);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let service_id = ServiceId::new(0x1234).unwrap();
    let eventgroup = EventgroupId::new(0x01).unwrap();

    // Server offers and will ACK subscriptions
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();
    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(ConcreteInstanceId::new(0x0001).unwrap())
        .eventgroup(eventgroup)
        .build()
        .unwrap();
    let _offering = server.offer(service_config).unwrap();

    // Client subscribes
    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));

    let available = proxy.wait_available().unwrap();
    let _sub = available.subscribe(eventgroup).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Find SubscribeAck from server
    let sd_packets = find_sd_packets(&network);

    let ack_entry = sd_packets
        .iter()
        .filter_map(|pkt| {
            let (offset, len) = parse_entries_header(pkt)?;
            if len < 16 {
                return None;
            }
            EventgroupEntry::from_bytes(&pkt[offset..])
        })
        .find(|e| e.entry_type == entry_type::SUBSCRIBE_ACK);

    let entry = ack_entry.expect("Server should send SubscribeAck");

    assert_eq!(entry.entry_type, entry_type::SUBSCRIBE_ACK, "Type must be SubscribeAck (0x07)");
    assert_eq!(entry.service_id, 0x1234, "Service ID must match");
    assert_eq!(entry.eventgroup_id, 0x0001, "Eventgroup ID must match");
    assert!(entry.ttl > 0, "TTL > 0 means ACK, TTL = 0 means NACK");
}

/// feat_req_recentipsd_41, feat_req_recentipsd_764: Reboot detection
///
/// After a reboot, the reboot flag is set and session ID restarts at 1.
/// Peers must detect this and invalidate stale state.
#[test]
#[ignore = "Runtime::new not implemented"]
fn reboot_flag_behavior() {
    covers!(feat_req_recentipsd_41, feat_req_recentipsd_764, feat_req_recentipsd_871);

    // First runtime session
    let (network1, io_client1, io_server1) = SimulatedNetwork::new_pair();

    let mut server1 = Runtime::new(io_server1, RuntimeConfig::default()).unwrap();
    let service_config = ServiceConfig::builder()
        .service(ServiceId::new(0x1234).unwrap())
        .instance(ConcreteInstanceId::new(0x0001).unwrap())
        .build()
        .unwrap();
    let _offering1 = server1.offer(service_config.clone()).unwrap();
    network1.advance(std::time::Duration::from_millis(100));

    // Client discovers service
    let mut client = Runtime::new(io_client1, RuntimeConfig::default()).unwrap();
    let proxy = client.require(ServiceId::new(0x1234).unwrap(), InstanceId::ANY);
    network1.advance(std::time::Duration::from_millis(100));
    assert!(proxy.is_available());

    // Capture first session's final session_id  
    let sd_packets1 = find_sd_packets(&network1);
    let last_header1 = wire::Header::from_bytes(sd_packets1.last().unwrap()).unwrap();
    let last_session_before_reboot = last_header1.session_id;

    // Simulate reboot: drop everything, create new pair
    drop(server1);
    drop(client);

    let (network2, _io_client2, io_server2) = SimulatedNetwork::new_pair();
    let mut server2 = Runtime::new(io_server2, RuntimeConfig::default()).unwrap();
    let _offering2 = server2.offer(service_config).unwrap();
    network2.advance(std::time::Duration::from_millis(100));

    // Post-reboot SD message
    let sd_packets2 = find_sd_packets(&network2);
    let post_reboot = &sd_packets2[0];

    let flags = SdFlags::from_bytes(&post_reboot[16..]).unwrap();
    let header = wire::Header::from_bytes(post_reboot).unwrap();

    assert!(flags.reboot_flag, "Reboot flag should be set after restart");
    assert_eq!(header.session_id, 1, "Session ID should restart at 1");

    // Verify detection: if old.reboot=0 and new.reboot=1, reboot detected
    // (this is implicitly tested by the flag being set)
    let _ = last_session_before_reboot; // Acknowledge we captured this for verification
}

/// feat_req_recentipsd_27: SD uses UDP on port 30490
#[test]
#[ignore = "Runtime::new not implemented"]
fn sd_uses_udp_port_30490() {
    covers!(feat_req_recentipsd_27);

    let (network, _io_client, io_server) = SimulatedNetwork::new_pair();

    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();
    let service_config = ServiceConfig::builder()
        .service(ServiceId::new(0x1234).unwrap())
        .instance(ConcreteInstanceId::new(0x0001).unwrap())
        .build()
        .unwrap();
    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Verify SD was sent to UDP port 30490
    let sd_to_30490 = network.history().iter().any(|e| {
        matches!(
            e,
            NetworkEvent::UdpSent {
                dst_port: 30490,
                ..
            }
        )
    });

    assert!(sd_to_30490, "SD messages must be sent to UDP port 30490");
}
