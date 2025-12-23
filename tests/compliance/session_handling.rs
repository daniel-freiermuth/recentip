//! Session and Request ID Compliance Tests
//!
//! Tests the Session ID and Request ID behavior per SOME/IP specification.
//!
//! Key requirements tested:
//! - feat_req_recentip_83: Request ID = Client ID (16 bit) + Session ID (16 bit)
//! - feat_req_recentip_699: Client ID is unique identifier for calling client
//! - feat_req_recentip_88: Session ID is unique identifier for each call
//! - feat_req_recentip_700: Session ID = 0x0000 if session handling not used
//! - feat_req_recentip_649: Session ID starts at 0x0001 if session handling used
//! - feat_req_recentip_677: Session ID wraps from 0xFFFF to 0x0001
//! - feat_req_recentip_711: Server copies Request ID from request to response

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
// Request ID Structure
// ============================================================================

/// Request ID is composed of Client ID (high 16 bits) and Session ID (low 16 bits)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestId {
    pub client_id: u16,
    pub session_id: u16,
}

impl RequestId {
    /// Create a new Request ID from client and session IDs
    pub fn new(client_id: u16, session_id: u16) -> Self {
        Self {
            client_id,
            session_id,
        }
    }

    /// Parse Request ID from wire format (4 bytes at offset 8 in header)
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 {
            return None;
        }
        let client_id = u16::from_be_bytes([bytes[0], bytes[1]]);
        let session_id = u16::from_be_bytes([bytes[2], bytes[3]]);
        Some(Self {
            client_id,
            session_id,
        })
    }

    /// Convert to wire format bytes
    pub fn to_bytes(&self) -> [u8; 4] {
        let client = self.client_id.to_be_bytes();
        let session = self.session_id.to_be_bytes();
        [client[0], client[1], session[0], session[1]]
    }

    /// Get the full 32-bit Request ID value
    pub fn as_u32(&self) -> u32 {
        ((self.client_id as u32) << 16) | (self.session_id as u32)
    }

    /// Create from a 32-bit value
    pub fn from_u32(value: u32) -> Self {
        Self {
            client_id: (value >> 16) as u16,
            session_id: (value & 0xFFFF) as u16,
        }
    }
}

/// Session ID counter with proper wraparound behavior
#[derive(Debug, Clone)]
pub struct SessionIdCounter {
    current: u16,
    enabled: bool,
}

impl SessionIdCounter {
    /// Create a counter with session handling enabled (starts at 0x0001)
    pub fn new() -> Self {
        Self {
            current: 0x0001,
            enabled: true,
        }
    }

    /// Create a counter with session handling disabled (always 0x0000)
    pub fn disabled() -> Self {
        Self {
            current: 0x0000,
            enabled: false,
        }
    }

    /// Get the next session ID
    pub fn next(&mut self) -> u16 {
        if !self.enabled {
            return 0x0000;
        }

        let result = self.current;
        // Wrap from 0xFFFF to 0x0001 (skip 0x0000)
        self.current = if self.current == 0xFFFF {
            0x0001
        } else {
            self.current + 1
        };
        result
    }

    /// Peek at current value without advancing
    pub fn current(&self) -> u16 {
        self.current
    }
}

impl Default for SessionIdCounter {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Unit Tests for Request ID
// ============================================================================

#[cfg(test)]
mod request_id_tests {
    use super::*;

    /// feat_req_recentip_83: Request ID is Client ID + Session ID
    #[test]
    fn request_id_composition() {
        covers!(feat_req_recentip_83);

        let req_id = RequestId::new(0x1234, 0x5678);
        assert_eq!(req_id.client_id, 0x1234);
        assert_eq!(req_id.session_id, 0x5678);
        assert_eq!(req_id.as_u32(), 0x12345678);
    }

    /// Request ID roundtrip through bytes
    #[test]
    fn request_id_roundtrip() {
        let original = RequestId::new(0xABCD, 0xEF01);
        let bytes = original.to_bytes();
        let parsed = RequestId::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.client_id, original.client_id);
        assert_eq!(parsed.session_id, original.session_id);
    }

    /// Request ID from u32 and back
    #[test]
    fn request_id_u32_roundtrip() {
        let value: u32 = 0xDEADBEEF;
        let req_id = RequestId::from_u32(value);
        assert_eq!(req_id.client_id, 0xDEAD);
        assert_eq!(req_id.session_id, 0xBEEF);
        assert_eq!(req_id.as_u32(), value);
    }

    /// Wire format is big-endian
    #[test]
    fn request_id_big_endian() {
        let req_id = RequestId::new(0x0102, 0x0304);
        let bytes = req_id.to_bytes();
        assert_eq!(bytes, [0x01, 0x02, 0x03, 0x04]);
    }
}

// ============================================================================
// Unit Tests for Session ID Counter
// ============================================================================

#[cfg(test)]
mod session_id_tests {
    use super::*;

    /// feat_req_recentip_649: Session ID starts at 0x0001
    #[test]
    fn session_id_starts_at_one() {
        covers!(feat_req_recentip_649);

        let mut counter = SessionIdCounter::new();
        assert_eq!(counter.next(), 0x0001);
    }

    /// feat_req_recentip_700: Session ID = 0x0000 if disabled
    #[test]
    fn session_id_zero_when_disabled() {
        covers!(feat_req_recentip_700);

        let mut counter = SessionIdCounter::disabled();
        assert_eq!(counter.next(), 0x0000);
        assert_eq!(counter.next(), 0x0000);
        assert_eq!(counter.next(), 0x0000);
    }

    /// Session ID increments
    #[test]
    fn session_id_increments() {
        let mut counter = SessionIdCounter::new();
        assert_eq!(counter.next(), 0x0001);
        assert_eq!(counter.next(), 0x0002);
        assert_eq!(counter.next(), 0x0003);
    }

    /// feat_req_recentip_677: Session ID wraps from 0xFFFF to 0x0001
    #[test]
    fn session_id_wraps_correctly() {
        covers!(feat_req_recentip_677);

        let mut counter = SessionIdCounter {
            current: 0xFFFF,
            enabled: true,
        };

        assert_eq!(counter.next(), 0xFFFF);
        assert_eq!(counter.next(), 0x0001); // Wraps to 0x0001, not 0x0000
        assert_eq!(counter.next(), 0x0002);
    }

    /// Session ID never becomes 0x0000 when enabled
    #[test]
    fn session_id_skips_zero_on_wrap() {
        covers!(feat_req_recentip_677);

        let mut counter = SessionIdCounter {
            current: 0xFFFE,
            enabled: true,
        };

        assert_eq!(counter.next(), 0xFFFE);
        assert_eq!(counter.next(), 0xFFFF);
        assert_eq!(counter.next(), 0x0001); // Skips 0x0000
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Extract Request ID from a SOME/IP message (bytes 8-11)
#[allow(dead_code)]
fn extract_request_id(msg: &[u8]) -> Option<RequestId> {
    if msg.len() < 12 {
        return None;
    }
    RequestId::from_bytes(&msg[8..12])
}

/// Find all SOME/IP RPC messages (non-SD)
#[allow(dead_code)]
fn find_rpc_messages(network: &SimulatedNetwork) -> Vec<Vec<u8>> {
    network
        .history()
        .iter()
        .filter_map(|event| {
            if let NetworkEvent::UdpSent { data, dst_port, .. } = event {
                if *dst_port != 30490 && data.len() >= 16 {
                    return Some(data.clone());
                }
            }
            None
        })
        .collect()
}

/// Find REQUEST messages (message type 0x00)
#[allow(dead_code)]
fn find_requests(network: &SimulatedNetwork) -> Vec<Vec<u8>> {
    find_rpc_messages(network)
        .into_iter()
        .filter(|msg| msg.len() >= 16 && msg[14] == 0x00)
        .collect()
}

/// Find RESPONSE messages (message type 0x80)
#[allow(dead_code)]
fn find_responses(network: &SimulatedNetwork) -> Vec<Vec<u8>> {
    find_rpc_messages(network)
        .into_iter()
        .filter(|msg| msg.len() >= 16 && msg[14] == 0x80)
        .collect()
}

// ============================================================================
// Integration Tests (require Runtime implementation)
// ============================================================================

/// feat_req_recentip_711: Server copies Request ID to response
///
/// When generating a response message, the server has to copy the
/// Request ID from the request to the response message.
#[test]
#[ignore = "Runtime::new not implemented"]
fn server_copies_request_id_to_response() {
    covers!(feat_req_recentip_711);

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

    let mut offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    // Send a request
    let method_id = MethodId::new(0x0001);
    let pending = proxy.call(method_id, &[1, 2, 3]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Server handles and responds
    if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
        request.responder.send_ok(&[4, 5, 6]).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    let _response = pending.wait().unwrap();

    // Verify Request IDs match
    let requests = find_requests(&network);
    let responses = find_responses(&network);

    assert!(!requests.is_empty() && !responses.is_empty());

    let req_id = extract_request_id(&requests[0]).unwrap();
    let resp_id = extract_request_id(&responses[0]).unwrap();

    assert_eq!(
        req_id.as_u32(),
        resp_id.as_u32(),
        "Response Request ID must match Request"
    );
}

/// feat_req_recentip_649: Session ID starts at 0x0001 for first call
#[test]
#[ignore = "Runtime::new not implemented"]
fn first_call_has_session_id_one() {
    covers!(feat_req_recentip_649);

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

    // First call
    let method_id = MethodId::new(0x0001);
    let _pending = proxy.call(method_id, &[]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    let requests = find_requests(&network);
    assert!(!requests.is_empty());

    let req_id = extract_request_id(&requests[0]).unwrap();
    assert_eq!(
        req_id.session_id, 0x0001,
        "First call should have Session ID 0x0001"
    );
}

/// Session ID increments for each call
#[test]
#[ignore = "Runtime::new not implemented"]
fn session_id_increments_each_call() {
    covers!(feat_req_recentip_88);

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

    let mut offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    let method_id = MethodId::new(0x0001);

    // Make multiple calls
    for expected_session in 1u16..=5 {
        network.clear_history();

        let pending = proxy.call(method_id, &[]).unwrap();
        network.advance(std::time::Duration::from_millis(10));

        // Server responds
        if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
            request.responder.send_ok(&[]).unwrap();
        }
        network.advance(std::time::Duration::from_millis(10));

        let _response = pending.wait().unwrap();

        let requests = find_requests(&network);
        let req_id = extract_request_id(&requests[0]).unwrap();

        assert_eq!(
            req_id.session_id, expected_session,
            "Session ID should increment"
        );
    }
}

/// feat_req_recentip_699: Client ID is consistent for a client
#[test]
#[ignore = "Runtime::new not implemented"]
fn client_id_consistent_across_calls() {
    covers!(feat_req_recentip_699);

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

    let mut offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    let method_id = MethodId::new(0x0001);
    let mut client_ids = Vec::new();

    // Make multiple calls and collect client IDs
    for _ in 0..3 {
        network.clear_history();

        let pending = proxy.call(method_id, &[]).unwrap();
        network.advance(std::time::Duration::from_millis(10));

        if let Some(ServiceEvent::MethodCall { request }) = offering.try_next().ok().flatten() {
            request.responder.send_ok(&[]).unwrap();
        }
        network.advance(std::time::Duration::from_millis(10));

        let _response = pending.wait().unwrap();

        let requests = find_requests(&network);
        let req_id = extract_request_id(&requests[0]).unwrap();
        client_ids.push(req_id.client_id);
    }

    // All client IDs should be the same
    assert!(
        client_ids.iter().all(|&id| id == client_ids[0]),
        "Client ID should be consistent across calls"
    );
}

/// Multiple outstanding requests have unique Request IDs
#[test]
#[ignore = "Runtime::new not implemented"]
fn parallel_requests_have_unique_request_ids() {
    covers!(feat_req_recentip_711);

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

    let method_id = MethodId::new(0x0001);

    // Send multiple requests without waiting for responses
    let _pending1 = proxy.call(method_id, &[1]).unwrap();
    let _pending2 = proxy.call(method_id, &[2]).unwrap();
    let _pending3 = proxy.call(method_id, &[3]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    let requests = find_requests(&network);
    assert_eq!(requests.len(), 3, "Should have 3 pending requests");

    let req_ids: Vec<_> = requests
        .iter()
        .filter_map(|r| extract_request_id(r))
        .map(|id| id.as_u32())
        .collect();

    // All Request IDs should be unique
    let mut unique_ids = req_ids.clone();
    unique_ids.sort();
    unique_ids.dedup();
    assert_eq!(
        unique_ids.len(),
        req_ids.len(),
        "All Request IDs must be unique"
    );
}

/// Request ID enables matching responses to requests
#[test]
#[ignore = "Runtime::new not implemented"]
fn request_id_enables_response_matching() {
    covers!(feat_req_recentip_711);

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

    let mut offering = server.offer(service_config).unwrap();
    network.advance(std::time::Duration::from_millis(100));

    let proxy = client.require(service_id, InstanceId::ANY);
    network.advance(std::time::Duration::from_millis(100));
    let proxy = proxy.wait_available().unwrap();

    let method_id = MethodId::new(0x0001);

    // Send two requests
    let pending1 = proxy.call(method_id, &[1]).unwrap();
    let pending2 = proxy.call(method_id, &[2]).unwrap();
    network.advance(std::time::Duration::from_millis(10));

    // Server handles both, but responds in reverse order
    let mut requests_received = Vec::new();
    while let Some(event) = offering.try_next().ok().flatten() {
        if let ServiceEvent::MethodCall { request } = event {
            requests_received.push(request);
        }
    }

    // Respond to second request first (drain to take ownership)
    if requests_received.len() == 2 {
        let mut drain = requests_received.drain(..);
        let req0 = drain.next().unwrap();
        let req1 = drain.next().unwrap();

        req1.responder.send_ok(&[20]).unwrap();
        req0.responder.send_ok(&[10]).unwrap();
    }
    network.advance(std::time::Duration::from_millis(10));

    // Client should correctly match responses despite out-of-order delivery
    let response1 = pending1.wait().unwrap();
    let response2 = pending2.wait().unwrap();

    // First request should get its response ([10]), second should get [20]
    assert_eq!(response1.payload, vec![10], "Response should match request");
    assert_eq!(response2.payload, vec![20], "Response should match request");
}
