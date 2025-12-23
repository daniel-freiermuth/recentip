//! Subscription and Eventgroup Compliance Tests
//!
//! Tests the publish/subscribe (Pub/Sub) behavior per SOME/IP-SD specification.
//!
//! Key requirements tested:
//! - feat_req_recentipsd_109: Eventgroup Entry format (16 bytes)
//! - feat_req_recentipsd_207: Service Entry types (Find, Offer, StopOffer)
//! - feat_req_recentipsd_576: Subscribe type (0x06) and SubscribeAck type (0x07)
//! - feat_req_recentipsd_178: TTL=0 means StopSubscribeEventgroup
//! - feat_req_recentipsd_179: SubscribeEventgroupNack with TTL=0
//! - feat_req_recentipsd_1137: Client endpoint option in SubscribeEventgroup

use someip_runtime::*;

// Re-use wire format parsing from the shared module
#[path = "../wire.rs"]
mod wire;

// Re-use simulated network
#[path = "../simulated.rs"]
mod simulated;

use simulated::{NetworkEvent, SimulatedNetwork};

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}

// ============================================================================
// SD Entry Type Constants
// ============================================================================

pub mod entry_types {
    // Service Entry Types (per feat_req_recentipsd_207)
    pub const FIND_SERVICE: u8 = 0x00;
    pub const OFFER_SERVICE: u8 = 0x01;
    // 0x02-0x05: Reserved

    // Eventgroup Entry Types (per feat_req_recentipsd_576)
    pub const SUBSCRIBE_EVENTGROUP: u8 = 0x06;
    pub const SUBSCRIBE_EVENTGROUP_ACK: u8 = 0x07;
    // 0x08-0xFF: Reserved
}

// ============================================================================
// SD Option Type Constants
// ============================================================================

pub mod option_types {
    pub const CONFIGURATION: u8 = 0x01;
    pub const LOAD_BALANCING: u8 = 0x02;
    // 0x03: Reserved
    pub const IPV4_ENDPOINT: u8 = 0x04;
    pub const IPV6_ENDPOINT: u8 = 0x06;
    pub const IPV4_MULTICAST: u8 = 0x14;
    pub const IPV6_MULTICAST: u8 = 0x16;
    pub const IPV4_SD_ENDPOINT: u8 = 0x24;
    pub const IPV6_SD_ENDPOINT: u8 = 0x26;
}

// ============================================================================
// Eventgroup Entry Structure (for parsing)
// ============================================================================

/// Parsed Eventgroup Entry from SD message
#[derive(Debug, Clone, PartialEq)]
pub struct EventgroupEntry {
    pub entry_type: u8,
    pub index_first_option: u8,
    pub index_second_option: u8,
    pub num_options_1: u8,
    pub num_options_2: u8,
    pub service_id: u16,
    pub instance_id: u16,
    pub major_version: u8,
    pub ttl: u32, // 24 bits
    pub reserved: u8,
    pub initial_data_requested: bool,
    pub reserved2: u8, // 3 bits
    pub counter: u8,   // 4 bits
    pub eventgroup_id: u16,
}

impl EventgroupEntry {
    /// Parse an Eventgroup Entry from bytes (16 bytes required)
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 16 {
            return None;
        }

        let entry_type = data[0];
        let index_first_option = data[1];
        let index_second_option = data[2];
        let num_options_1 = (data[3] >> 4) & 0x0F;
        let num_options_2 = data[3] & 0x0F;
        let service_id = u16::from_be_bytes([data[4], data[5]]);
        let instance_id = u16::from_be_bytes([data[6], data[7]]);
        let major_version = data[8];
        let ttl = u32::from_be_bytes([0, data[9], data[10], data[11]]);
        let reserved = data[12];
        let initial_data_requested = (data[13] & 0x80) != 0;
        let reserved2 = (data[13] >> 4) & 0x07;
        let counter = data[13] & 0x0F;
        let eventgroup_id = u16::from_be_bytes([data[14], data[15]]);

        Some(Self {
            entry_type,
            index_first_option,
            index_second_option,
            num_options_1,
            num_options_2,
            service_id,
            instance_id,
            major_version,
            ttl,
            reserved,
            initial_data_requested,
            reserved2,
            counter,
            eventgroup_id,
        })
    }

    /// Convert to wire format bytes
    pub fn to_bytes(&self) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[0] = self.entry_type;
        bytes[1] = self.index_first_option;
        bytes[2] = self.index_second_option;
        bytes[3] = (self.num_options_1 << 4) | (self.num_options_2 & 0x0F);
        bytes[4..6].copy_from_slice(&self.service_id.to_be_bytes());
        bytes[6..8].copy_from_slice(&self.instance_id.to_be_bytes());
        bytes[8] = self.major_version;
        // TTL is 24 bits
        let ttl_bytes = self.ttl.to_be_bytes();
        bytes[9..12].copy_from_slice(&ttl_bytes[1..4]);
        bytes[12] = self.reserved;
        bytes[13] = ((self.initial_data_requested as u8) << 7)
            | ((self.reserved2 & 0x07) << 4)
            | (self.counter & 0x0F);
        bytes[14..16].copy_from_slice(&self.eventgroup_id.to_be_bytes());
        bytes
    }

    /// Check if this is a Subscribe entry
    pub fn is_subscribe(&self) -> bool {
        self.entry_type == entry_types::SUBSCRIBE_EVENTGROUP
    }

    /// Check if this is a SubscribeAck entry
    pub fn is_subscribe_ack(&self) -> bool {
        self.entry_type == entry_types::SUBSCRIBE_EVENTGROUP_ACK
    }

    /// Check if this is a StopSubscribe (Subscribe with TTL=0)
    pub fn is_stop_subscribe(&self) -> bool {
        self.entry_type == entry_types::SUBSCRIBE_EVENTGROUP && self.ttl == 0
    }

    /// Check if this is a SubscribeNack (SubscribeAck with TTL=0)
    pub fn is_subscribe_nack(&self) -> bool {
        self.entry_type == entry_types::SUBSCRIBE_EVENTGROUP_ACK && self.ttl == 0
    }
}

// ============================================================================
// Unit Tests for Entry Parsing
// ============================================================================

#[cfg(test)]
mod entry_parsing_tests {
    use super::*;

    /// feat_req_recentipsd_109: Eventgroup Entry is 16 bytes
    #[test]
    fn eventgroup_entry_size_is_16_bytes() {
        covers!(feat_req_recentipsd_109);

        let entry = EventgroupEntry {
            entry_type: entry_types::SUBSCRIBE_EVENTGROUP,
            index_first_option: 0,
            index_second_option: 0,
            num_options_1: 0,
            num_options_2: 0,
            service_id: 0x1234,
            instance_id: 0x5678,
            major_version: 1,
            ttl: 3,
            reserved: 0,
            initial_data_requested: false,
            reserved2: 0,
            counter: 0,
            eventgroup_id: 0x0001,
        };

        let bytes = entry.to_bytes();
        assert_eq!(bytes.len(), 16);
    }

    /// feat_req_recentipsd_576: Subscribe type is 0x06
    #[test]
    fn subscribe_eventgroup_type_is_0x06() {
        covers!(feat_req_recentipsd_576);
        assert_eq!(entry_types::SUBSCRIBE_EVENTGROUP, 0x06);
    }

    /// feat_req_recentipsd_576: SubscribeAck type is 0x07
    #[test]
    fn subscribe_eventgroup_ack_type_is_0x07() {
        covers!(feat_req_recentipsd_576);
        assert_eq!(entry_types::SUBSCRIBE_EVENTGROUP_ACK, 0x07);
    }

    /// Service entry types match spec
    #[test]
    fn service_entry_types_match_spec() {
        covers!(feat_req_recentipsd_207);
        assert_eq!(entry_types::FIND_SERVICE, 0x00);
        assert_eq!(entry_types::OFFER_SERVICE, 0x01);
    }

    /// Eventgroup entry roundtrip
    #[test]
    fn eventgroup_entry_roundtrip() {
        let original = EventgroupEntry {
            entry_type: entry_types::SUBSCRIBE_EVENTGROUP,
            index_first_option: 1,
            index_second_option: 2,
            num_options_1: 3,
            num_options_2: 4,
            service_id: 0x1234,
            instance_id: 0x5678,
            major_version: 2,
            ttl: 0x00ABCDEF,
            reserved: 0,
            initial_data_requested: true,
            reserved2: 0,
            counter: 5,
            eventgroup_id: 0x8001,
        };

        let bytes = original.to_bytes();
        let parsed = EventgroupEntry::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.entry_type, original.entry_type);
        assert_eq!(parsed.index_first_option, original.index_first_option);
        assert_eq!(parsed.index_second_option, original.index_second_option);
        assert_eq!(parsed.num_options_1, original.num_options_1);
        assert_eq!(parsed.num_options_2, original.num_options_2);
        assert_eq!(parsed.service_id, original.service_id);
        assert_eq!(parsed.instance_id, original.instance_id);
        assert_eq!(parsed.major_version, original.major_version);
        // TTL is 24-bit, so mask the comparison
        assert_eq!(parsed.ttl, original.ttl & 0x00FFFFFF);
        assert_eq!(
            parsed.initial_data_requested,
            original.initial_data_requested
        );
        assert_eq!(parsed.counter, original.counter);
        assert_eq!(parsed.eventgroup_id, original.eventgroup_id);
    }

    /// TTL=0 means StopSubscribeEventgroup
    #[test]
    fn ttl_zero_is_stop_subscribe() {
        covers!(feat_req_recentipsd_178);

        let entry = EventgroupEntry {
            entry_type: entry_types::SUBSCRIBE_EVENTGROUP,
            index_first_option: 0,
            index_second_option: 0,
            num_options_1: 0,
            num_options_2: 0,
            service_id: 0x1234,
            instance_id: 0x5678,
            major_version: 1,
            ttl: 0, // TTL=0 = StopSubscribe
            reserved: 0,
            initial_data_requested: false,
            reserved2: 0,
            counter: 0,
            eventgroup_id: 0x0001,
        };

        assert!(entry.is_stop_subscribe());
        assert!(!entry.is_subscribe_nack());
    }

    /// TTL=0 in SubscribeAck means Nack
    #[test]
    fn ttl_zero_in_ack_is_nack() {
        covers!(feat_req_recentipsd_179);

        let entry = EventgroupEntry {
            entry_type: entry_types::SUBSCRIBE_EVENTGROUP_ACK,
            index_first_option: 0,
            index_second_option: 0,
            num_options_1: 0,
            num_options_2: 0,
            service_id: 0x1234,
            instance_id: 0x5678,
            major_version: 1,
            ttl: 0, // TTL=0 in Ack = Nack
            reserved: 0,
            initial_data_requested: false,
            reserved2: 0,
            counter: 0,
            eventgroup_id: 0x0001,
        };

        assert!(entry.is_subscribe_nack());
        assert!(!entry.is_stop_subscribe());
    }

    /// Initial data requested flag parsing
    #[test]
    fn initial_data_requested_flag() {
        covers!(feat_req_recentipsd_109);

        // With flag set
        let entry_with_flag = EventgroupEntry {
            entry_type: entry_types::SUBSCRIBE_EVENTGROUP,
            index_first_option: 0,
            index_second_option: 0,
            num_options_1: 0,
            num_options_2: 0,
            service_id: 0x1234,
            instance_id: 0x5678,
            major_version: 1,
            ttl: 3,
            reserved: 0,
            initial_data_requested: true,
            reserved2: 0,
            counter: 0,
            eventgroup_id: 0x0001,
        };

        let bytes = entry_with_flag.to_bytes();
        let parsed = EventgroupEntry::from_bytes(&bytes).unwrap();
        assert!(parsed.initial_data_requested);

        // Without flag
        let entry_without_flag = EventgroupEntry {
            initial_data_requested: false,
            ..entry_with_flag
        };

        let bytes = entry_without_flag.to_bytes();
        let parsed = EventgroupEntry::from_bytes(&bytes).unwrap();
        assert!(!parsed.initial_data_requested);
    }

    /// Counter field parsing (4 bits)
    #[test]
    fn counter_field_4_bits() {
        covers!(feat_req_recentipsd_109);

        for counter_value in [0, 1, 7, 15] {
            let entry = EventgroupEntry {
                entry_type: entry_types::SUBSCRIBE_EVENTGROUP,
                index_first_option: 0,
                index_second_option: 0,
                num_options_1: 0,
                num_options_2: 0,
                service_id: 0x1234,
                instance_id: 0x5678,
                major_version: 1,
                ttl: 3,
                reserved: 0,
                initial_data_requested: false,
                reserved2: 0,
                counter: counter_value,
                eventgroup_id: 0x0001,
            };

            let bytes = entry.to_bytes();
            let parsed = EventgroupEntry::from_bytes(&bytes).unwrap();
            assert_eq!(parsed.counter, counter_value);
        }
    }
}

// ============================================================================
// Option Type Unit Tests
// ============================================================================

#[cfg(test)]
mod option_type_tests {
    use super::option_types::*;

    /// Option type values match spec
    #[test]
    fn option_type_values() {
        assert_eq!(CONFIGURATION, 0x01);
        assert_eq!(LOAD_BALANCING, 0x02);
        assert_eq!(IPV4_ENDPOINT, 0x04);
        assert_eq!(IPV6_ENDPOINT, 0x06);
        assert_eq!(IPV4_MULTICAST, 0x14);
        assert_eq!(IPV6_MULTICAST, 0x16);
        assert_eq!(IPV4_SD_ENDPOINT, 0x24);
        assert_eq!(IPV6_SD_ENDPOINT, 0x26);
    }

    /// Multicast options have 0x10 more than endpoint options
    #[test]
    fn multicast_option_offset() {
        assert_eq!(IPV4_MULTICAST, IPV4_ENDPOINT + 0x10);
        assert_eq!(IPV6_MULTICAST, IPV6_ENDPOINT + 0x10);
    }

    /// SD Endpoint options have 0x20 more than endpoint options
    #[test]
    fn sd_endpoint_option_offset() {
        assert_eq!(IPV4_SD_ENDPOINT, IPV4_ENDPOINT + 0x20);
        assert_eq!(IPV6_SD_ENDPOINT, IPV6_ENDPOINT + 0x20);
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Find all SD messages in network history
#[allow(dead_code)]
fn find_sd_messages(network: &SimulatedNetwork) -> Vec<Vec<u8>> {
    network
        .history()
        .iter()
        .filter_map(|event| {
            if let NetworkEvent::UdpSent { data, dst_port, .. } = event {
                // SD messages go to port 30490
                if *dst_port == 30490 && data.len() >= 12 {
                    return Some(data.clone());
                }
            }
            None
        })
        .collect()
}

/// Extract eventgroup entries from an SD message
#[allow(dead_code)]
fn extract_eventgroup_entries(sd_message: &[u8]) -> Vec<EventgroupEntry> {
    if sd_message.len() < 28 {
        return vec![];
    }

    // SD header is 12 bytes after SOME/IP header (16 bytes)
    // Entries start at offset 28 (16 + 12)
    let entries_len = u32::from_be_bytes([
        sd_message[20],
        sd_message[21],
        sd_message[22],
        sd_message[23],
    ]) as usize;

    let entries_start = 24;
    let entries_end = (entries_start + entries_len).min(sd_message.len());

    let mut entries = vec![];
    let mut offset = entries_start;

    while offset + 16 <= entries_end {
        if let Some(entry) = EventgroupEntry::from_bytes(&sd_message[offset..]) {
            // Only include eventgroup entries (types 0x06, 0x07)
            if entry.entry_type == entry_types::SUBSCRIBE_EVENTGROUP
                || entry.entry_type == entry_types::SUBSCRIBE_EVENTGROUP_ACK
            {
                entries.push(entry);
            }
        }
        offset += 16;
    }

    entries
}

// ============================================================================
// Integration Tests (require Runtime implementation)
// ============================================================================

/// feat_req_recentipsd_576: Client sends SubscribeEventgroup
///
/// When a client subscribes to an eventgroup, it should send a
/// SubscribeEventgroup entry (type 0x06).
#[test]
#[ignore = "Runtime::new not implemented"]
fn client_sends_subscribe_eventgroup() {
    covers!(feat_req_recentipsd_576);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    // Subscribe to an eventgroup
    let eventgroup = EventgroupId::new(1).unwrap();
    let _subscription = proxy.subscribe(eventgroup).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Find Subscribe entries
    let sd_messages = find_sd_messages(&network);
    let subscribe_entries: Vec<_> = sd_messages
        .iter()
        .flat_map(|msg| extract_eventgroup_entries(msg))
        .filter(|e| e.is_subscribe())
        .collect();

    assert!(
        !subscribe_entries.is_empty(),
        "Client should send SubscribeEventgroup"
    );

    // Verify entry content
    let entry = &subscribe_entries[0];
    assert_eq!(
        entry.entry_type,
        entry_types::SUBSCRIBE_EVENTGROUP,
        "Entry type should be 0x06"
    );
    assert!(entry.ttl > 0, "TTL should be > 0 for Subscribe");
}

/// feat_req_recentipsd_576: Server responds with SubscribeAck
///
/// When a server receives a valid SubscribeEventgroup, it should
/// respond with a SubscribeEventgroupAck (type 0x07).
#[test]
#[ignore = "Runtime::new not implemented"]
fn server_sends_subscribe_ack() {
    covers!(feat_req_recentipsd_576);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    let eventgroup = EventgroupId::new(1).unwrap();
    let _subscription = proxy.subscribe(eventgroup).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Find SubscribeAck entries
    let sd_messages = find_sd_messages(&network);
    let ack_entries: Vec<_> = sd_messages
        .iter()
        .flat_map(|msg| extract_eventgroup_entries(msg))
        .filter(|e| e.is_subscribe_ack() && !e.is_subscribe_nack())
        .collect();

    assert!(
        !ack_entries.is_empty(),
        "Server should send SubscribeEventgroupAck"
    );

    // Verify Ack content
    let entry = &ack_entries[0];
    assert_eq!(
        entry.entry_type,
        entry_types::SUBSCRIBE_EVENTGROUP_ACK,
        "Entry type should be 0x07"
    );
    assert!(entry.ttl > 0, "TTL should be > 0 for Ack");
}

/// feat_req_recentipsd_178: StopSubscribeEventgroup has TTL=0
#[test]
#[ignore = "Runtime::new not implemented"]
fn stop_subscribe_has_ttl_zero() {
    covers!(feat_req_recentipsd_178);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    // Subscribe and then unsubscribe
    let eventgroup = EventgroupId::new(1).unwrap();
    let subscription = proxy.subscribe(eventgroup).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // Unsubscribe
    drop(subscription);
    network.advance(std::time::Duration::from_millis(100));

    // Find StopSubscribe entries
    let sd_messages = find_sd_messages(&network);
    let stop_subscribe_entries: Vec<_> = sd_messages
        .iter()
        .flat_map(|msg| extract_eventgroup_entries(msg))
        .filter(|e| e.is_stop_subscribe())
        .collect();

    assert!(
        !stop_subscribe_entries.is_empty(),
        "Should send StopSubscribeEventgroup on unsubscribe"
    );

    for entry in stop_subscribe_entries {
        assert_eq!(entry.ttl, 0, "StopSubscribe must have TTL=0");
    }
}

/// feat_req_recentipsd_179: SubscribeEventgroupNack has TTL=0
#[test]
#[ignore = "Runtime::new not implemented"]
fn subscribe_nack_has_ttl_zero() {
    covers!(feat_req_recentipsd_179);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut _client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        // Note: eventgroup 0x9999 is not configured
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    // TODO: Inject a SubscribeEventgroup for non-existent eventgroup
    // Server should respond with Nack

    network.advance(std::time::Duration::from_millis(100));

    // Find SubscribeNack entries
    let sd_messages = find_sd_messages(&network);
    let nack_entries: Vec<_> = sd_messages
        .iter()
        .flat_map(|msg| extract_eventgroup_entries(msg))
        .filter(|e| e.is_subscribe_nack())
        .collect();

    for entry in nack_entries {
        assert_eq!(entry.ttl, 0, "SubscribeNack must have TTL=0");
    }
}

/// Subscribe entry must include service/instance matching the service
#[test]
#[ignore = "Runtime::new not implemented"]
fn subscribe_entry_ids_match_service() {
    covers!(feat_req_recentipsd_109);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(0x5678).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    let eventgroup = EventgroupId::new(1).unwrap();
    let _subscription = proxy.subscribe(eventgroup).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let sd_messages = find_sd_messages(&network);
    let subscribe_entries: Vec<_> = sd_messages
        .iter()
        .flat_map(|msg| extract_eventgroup_entries(msg))
        .filter(|e| e.is_subscribe())
        .collect();

    assert!(!subscribe_entries.is_empty());

    let entry = &subscribe_entries[0];
    assert_eq!(entry.service_id, 0x1234);
    // Instance might be specific or 0xFFFF for ANY
}

/// Subscription includes eventgroup ID
#[test]
#[ignore = "Runtime::new not implemented"]
fn subscribe_entry_has_eventgroup_id() {
    covers!(feat_req_recentipsd_109);

    let (network, io_client, io_server) = SimulatedNetwork::new_pair();

    let mut client = Runtime::new(io_client, RuntimeConfig::default()).unwrap();
    let mut server = Runtime::new(io_server, RuntimeConfig::default()).unwrap();

    let service_id = ServiceId::new(0x1234).unwrap();
    let instance_id = ConcreteInstanceId::new(1).unwrap();

    let service_config = ServiceConfig::builder()
        .service(service_id)
        .instance(instance_id)
        .build()
        .unwrap();

    let _offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    let eventgroup = EventgroupId::new(0x42).unwrap();
    let _subscription = proxy.subscribe(eventgroup).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let sd_messages = find_sd_messages(&network);
    let subscribe_entries: Vec<_> = sd_messages
        .iter()
        .flat_map(|msg| extract_eventgroup_entries(msg))
        .filter(|e| e.is_subscribe())
        .collect();

    assert!(!subscribe_entries.is_empty());

    let entry = &subscribe_entries[0];
    assert_eq!(entry.eventgroup_id, 0x42);
}
