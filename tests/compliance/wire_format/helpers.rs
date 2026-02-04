//! Shared helpers for wire format tests
//!
//! This module contains common imports, test service definitions,
//! parser functions, and packet builders used across wire format tests.

use bytes::Bytes;
pub use recentip::handle::ServiceEvent;
pub use recentip::prelude::*;
pub use recentip::wire::{Header, MessageType, SdMessage, SD_METHOD_ID, SD_SERVICE_ID};
pub use std::net::SocketAddr;
pub use std::time::Duration;

/// Macro for documenting which spec requirements a test covers
macro_rules! covers {
    ($($req:ident),+ $(,)?) => {
        let _ = ($(stringify!($req)),+);
    };
}
pub(crate) use covers;

// Wire values for TestService
pub const TEST_SERVICE_ID: u16 = 0x1234;
pub const TEST_SERVICE_VERSION: (u8, u32) = (1, 0);

// ============================================================================
// PARSER FUNCTIONS
// ============================================================================

/// Helper to parse a SOME/IP header from raw bytes
pub fn parse_header(data: &[u8]) -> Option<Header> {
    if data.len() < Header::SIZE {
        return None;
    }
    Header::parse(&mut Bytes::copy_from_slice(data))
}

/// Helper to parse an SD message (header + payload) from raw bytes
pub fn parse_sd_message(data: &[u8]) -> Option<(Header, SdMessage)> {
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
// PACKET BUILDER FUNCTIONS
// ============================================================================

/// Builder for constructing raw SOME/IP packets for wire-level testing.
///
/// Provides a fluent API for creating test packets with customizable fields.
/// Defaults to protocol version 0x01 and interface version 0x01.
///
/// # Example
/// ```ignore
/// let packet = SomeIpPacketBuilder::request(0x1234, 0x0001)
///     .client_id(0xABCD)
///     .session_id(0x1234)
///     .interface_version(0x02)
///     .payload(b"test")
///     .build();
/// ```
pub struct SomeIpPacketBuilder {
    service_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    protocol_version: u8,
    interface_version: u8,
    message_type: u8,
    return_code: u8,
    payload: Vec<u8>,
}

impl SomeIpPacketBuilder {
    /// Create a new REQUEST packet builder with default protocol/interface version
    pub fn request(service_id: u16, method_id: u16) -> Self {
        Self {
            service_id,
            method_id,
            client_id: 0x0000,
            session_id: 0x0001,
            protocol_version: 0x01,
            interface_version: 0x01,
            message_type: 0x00, // REQUEST
            return_code: 0x00,
            payload: Vec::new(),
        }
    }

    /// Create a REQUEST_NO_RETURN (fire-and-forget) packet builder
    pub fn fire_and_forget(service_id: u16, method_id: u16) -> Self {
        Self {
            message_type: 0x01, // REQUEST_NO_RETURN
            ..Self::request(service_id, method_id)
        }
    }

    /// Create a RESPONSE packet builder
    pub fn response(service_id: u16, method_id: u16) -> Self {
        Self {
            message_type: 0x80, // RESPONSE
            ..Self::request(service_id, method_id)
        }
    }

    /// Create a NOTIFICATION packet builder (typically for SD messages)
    pub fn notification(service_id: u16, method_id: u16) -> Self {
        Self {
            message_type: 0x02, // NOTIFICATION
            ..Self::request(service_id, method_id)
        }
    }

    pub fn client_id(mut self, client_id: u16) -> Self {
        self.client_id = client_id;
        self
    }

    pub fn session_id(mut self, session_id: u16) -> Self {
        self.session_id = session_id;
        self
    }

    pub fn protocol_version(mut self, version: u8) -> Self {
        self.protocol_version = version;
        self
    }

    pub fn interface_version(mut self, version: u8) -> Self {
        self.interface_version = version;
        self
    }

    pub fn payload(mut self, payload: &[u8]) -> Self {
        self.payload = payload.to_vec();
        self
    }

    pub fn build(self) -> Vec<u8> {
        let length = 8 + self.payload.len() as u32;
        let mut packet = Vec::with_capacity(16 + self.payload.len());

        packet.extend_from_slice(&self.service_id.to_be_bytes());
        packet.extend_from_slice(&self.method_id.to_be_bytes());
        packet.extend_from_slice(&length.to_be_bytes());
        packet.extend_from_slice(&self.client_id.to_be_bytes());
        packet.extend_from_slice(&self.session_id.to_be_bytes());
        packet.push(self.protocol_version);
        packet.push(self.interface_version);
        packet.push(self.message_type);
        packet.push(self.return_code);
        packet.extend_from_slice(&self.payload);

        packet
    }
}

/// Build a raw SOME/IP request packet
pub fn build_request(
    service_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    payload: &[u8],
) -> Vec<u8> {
    SomeIpPacketBuilder::request(service_id, method_id)
        .client_id(client_id)
        .session_id(session_id)
        .payload(payload)
        .build()
}

/// Build a raw SOME/IP fire-and-forget (REQUEST_NO_RETURN) packet
pub fn build_fire_and_forget_request(
    service_id: u16,
    method_id: u16,
    client_id: u16,
    session_id: u16,
    payload: &[u8],
) -> Vec<u8> {
    SomeIpPacketBuilder::fire_and_forget(service_id, method_id)
        .client_id(client_id)
        .session_id(session_id)
        .payload(payload)
        .build()
}

/// Build a raw SOME/IP response packet based on a request header
pub fn build_response(request: &Header, payload: &[u8]) -> Vec<u8> {
    SomeIpPacketBuilder::response(request.service_id, request.method_id)
        .client_id(request.client_id)
        .session_id(request.session_id)
        .interface_version(request.interface_version)
        .payload(payload)
        .build()
}

/// L4 Protocol constants for SD endpoint options
pub const L4_PROTOCOL_UDP: u8 = 0x11;
pub const L4_PROTOCOL_TCP: u8 = 0x06;

/// Builder for constructing raw SOME/IP-SD OfferService messages.
///
/// Provides a fluent API for creating test packets with customizable fields.
/// Defaults to UDP protocol, TTL=0xFFFFFF, session_id=1, reboot_flag=true.
///
/// # Example
/// ```ignore
/// let offer = SdOfferBuilder::new(0x1234, 0x0001, "192.168.1.1".parse().unwrap(), 30501)
///     .major_version(2)
///     .tcp()
///     .session_id(5)
///     .build();
/// ```
pub struct SdOfferBuilder {
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    minor_version: u32,
    endpoint_ip: std::net::Ipv4Addr,
    endpoint_port: u16,
    protocol: u8,
    ttl: u32,
    session_id: u16,
    reboot_flag: bool,
    unicast_flag: bool,
}

impl SdOfferBuilder {
    /// Create a new OfferService builder with required parameters
    ///
    /// Defaults:
    /// - protocol: UDP
    /// - TTL: 0xFFFFFF (max)
    /// - session_id: 1
    /// - reboot_flag: true
    /// - unicast_flag: false (offers are typically multicast)
    pub fn new(
        service_id: u16,
        instance_id: u16,
        endpoint_ip: std::net::Ipv4Addr,
        endpoint_port: u16,
    ) -> Self {
        Self {
            service_id,
            instance_id,
            major_version: 1,
            minor_version: 0,
            endpoint_ip,
            endpoint_port,
            protocol: L4_PROTOCOL_UDP,
            ttl: 0xFFFFFF,
            session_id: 1,
            reboot_flag: true,
            unicast_flag: false, // Offers typically don't have unicast flag
        }
    }

    pub fn major_version(mut self, version: u8) -> Self {
        self.major_version = version;
        self
    }

    pub fn minor_version(mut self, version: u32) -> Self {
        self.minor_version = version;
        self
    }

    pub fn version(mut self, major: u8, minor: u32) -> Self {
        self.major_version = major;
        self.minor_version = minor;
        self
    }

    /// Set protocol to TCP (0x06)
    pub fn tcp(mut self) -> Self {
        self.protocol = L4_PROTOCOL_TCP;
        self
    }

    /// Set protocol to UDP (0x11) - this is the default
    pub fn udp(mut self) -> Self {
        self.protocol = L4_PROTOCOL_UDP;
        self
    }

    /// Set raw protocol byte
    pub fn protocol(mut self, protocol: u8) -> Self {
        self.protocol = protocol;
        self
    }

    pub fn ttl(mut self, ttl: u32) -> Self {
        self.ttl = ttl;
        self
    }

    pub fn session_id(mut self, session_id: u16) -> Self {
        self.session_id = session_id;
        self
    }

    pub fn reboot_flag(mut self, flag: bool) -> Self {
        self.reboot_flag = flag;
        self
    }

    pub fn unicast_flag(mut self, flag: bool) -> Self {
        self.unicast_flag = flag;
        self
    }

    pub fn build(self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(40);

        // SD Payload - Flags
        let mut flags = 0u8;
        if self.reboot_flag {
            flags |= 0x80;
        }
        if self.unicast_flag {
            flags |= 0x40;
        }
        payload.push(flags);
        payload.extend_from_slice(&[0x00, 0x00, 0x00]); // Reserved

        // Entries array
        payload.extend_from_slice(&16u32.to_be_bytes()); // Length

        // OfferService Entry
        payload.push(0x01); // Type
        payload.push(0x00); // Index 1st options
        payload.push(0x00); // Index 2nd options
        payload.push(0x10); // # of options (1 in run 1)
        payload.extend_from_slice(&self.service_id.to_be_bytes());
        payload.extend_from_slice(&self.instance_id.to_be_bytes());
        payload.push(self.major_version);
        let ttl_bytes = self.ttl.to_be_bytes();
        payload.extend_from_slice(&ttl_bytes[1..4]);
        payload.extend_from_slice(&self.minor_version.to_be_bytes());

        // Options array
        payload.extend_from_slice(&12u32.to_be_bytes()); // Length

        // IPv4 Endpoint Option
        payload.extend_from_slice(&9u16.to_be_bytes()); // Length
        payload.push(0x04); // Type
        payload.push(0x00); // Reserved
        payload.extend_from_slice(&self.endpoint_ip.octets());
        payload.push(0x00); // Reserved
        payload.push(self.protocol);
        payload.extend_from_slice(&self.endpoint_port.to_be_bytes());

        SomeIpPacketBuilder::notification(SD_SERVICE_ID, SD_METHOD_ID)
            .client_id(0x0000)
            .session_id(self.session_id)
            .payload(payload.as_slice())
            .build()
    }
}

/// Build a raw SOME/IP-SD OfferService message (convenience function)
pub fn build_sd_offer(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    minor_version: u32,
    endpoint_ip: std::net::Ipv4Addr,
    endpoint_port: u16,
    ttl: u32,
) -> Vec<u8> {
    SdOfferBuilder::new(service_id, instance_id, endpoint_ip, endpoint_port)
        .major_version(major_version)
        .minor_version(minor_version)
        .ttl(ttl)
        .build()
}

/// Builder for constructing raw SOME/IP-SD SubscribeEventgroupAck messages.
///
/// Provides a fluent API for creating test packets with customizable fields.
/// Defaults to TTL=0xFFFFFF, session_id=1, reboot_flag=true, unicast_flag=true.
///
/// # Example
/// ```ignore
/// let ack = SdSubscribeAckBuilder::new(0x1234, 0x0001, 0x0001)
///     .major_version(2)
///     .session_id(5)
///     .build();
/// ```
pub struct SdSubscribeAckBuilder {
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    ttl: u32,
    session_id: u16,
    reboot_flag: bool,
    unicast_flag: bool,
    counter: u8,
}

impl SdSubscribeAckBuilder {
    /// Create a new SubscribeEventgroupAck builder with required parameters
    pub fn new(service_id: u16, instance_id: u16, eventgroup_id: u16) -> Self {
        Self {
            service_id,
            instance_id,
            major_version: 1,
            eventgroup_id,
            ttl: 0xFFFFFF,
            session_id: 1,
            reboot_flag: true,
            unicast_flag: true,
            counter: 0,
        }
    }

    pub fn major_version(mut self, version: u8) -> Self {
        self.major_version = version;
        self
    }

    pub fn ttl(mut self, ttl: u32) -> Self {
        self.ttl = ttl;
        self
    }

    pub fn session_id(mut self, session_id: u16) -> Self {
        self.session_id = session_id;
        self
    }

    pub fn reboot_flag(mut self, flag: bool) -> Self {
        self.reboot_flag = flag;
        self
    }

    pub fn unicast_flag(mut self, flag: bool) -> Self {
        self.unicast_flag = flag;
        self
    }

    pub fn counter(mut self, counter: u8) -> Self {
        self.counter = counter;
        self
    }

    pub fn build(self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(48);

        // SD Payload - Flags
        let mut flags = 0u8;
        if self.reboot_flag {
            flags |= 0x80;
        }
        if self.unicast_flag {
            flags |= 0x40;
        }
        payload.push(flags);
        payload.extend_from_slice(&[0x00, 0x00, 0x00]); // Reserved

        // Entries array
        payload.extend_from_slice(&16u32.to_be_bytes());

        // SubscribeEventgroupAck Entry
        payload.push(0x07); // Type
        payload.push(0x00);
        payload.push(0x00);
        payload.push(0x00);
        payload.extend_from_slice(&self.service_id.to_be_bytes());
        payload.extend_from_slice(&self.instance_id.to_be_bytes());
        payload.push(self.major_version);
        let ttl_bytes = self.ttl.to_be_bytes();
        payload.extend_from_slice(&ttl_bytes[1..4]);
        payload.push(0x00); // Reserved
        payload.push(self.counter);
        payload.extend_from_slice(&self.eventgroup_id.to_be_bytes());

        // Options array (empty)
        payload.extend_from_slice(&0u32.to_be_bytes());

        SomeIpPacketBuilder::notification(SD_SERVICE_ID, SD_METHOD_ID)
            .client_id(0x0000)
            .session_id(self.session_id)
            .payload(payload.as_slice())
            .build()
    }
}

/// Build a raw SOME/IP-SD SubscribeEventgroupAck message (convenience function)
pub fn build_sd_subscribe_ack(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    ttl: u32,
) -> Vec<u8> {
    SdSubscribeAckBuilder::new(service_id, instance_id, eventgroup_id)
        .major_version(major_version)
        .ttl(ttl)
        .build()
}

/// Build a raw SOME/IP-SD SubscribeEventgroup with a UDP endpoint option
pub fn build_sd_subscribe_with_udp_endpoint(
    service_id: u16,
    instance_id: u16,
    major_version: u8,
    eventgroup_id: u16,
    ttl: u32,
    client_ip: std::net::Ipv4Addr,
    client_port: u16,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(64);

    // SD Payload
    payload.push(0xC0);
    payload.extend_from_slice(&[0x00, 0x00, 0x00]);

    // Entries array length
    payload.extend_from_slice(&16u32.to_be_bytes());

    // SubscribeEventgroup Entry
    payload.push(0x06); // Type = SubscribeEventgroup
    payload.push(0x00); // Index 1st options = 0
    payload.push(0x00); // Index 2nd options
    payload.push(0x10); // # of opts: 1 in run1, 0 in run2
    payload.extend_from_slice(&service_id.to_be_bytes());
    payload.extend_from_slice(&instance_id.to_be_bytes());
    payload.push(major_version);
    let ttl_bytes = ttl.to_be_bytes();
    payload.extend_from_slice(&ttl_bytes[1..4]);
    payload.push(0x00);
    payload.push(0x00);
    payload.extend_from_slice(&eventgroup_id.to_be_bytes());

    // Options array
    payload.extend_from_slice(&12u32.to_be_bytes());

    // IPv4 Endpoint Option with UDP
    payload.extend_from_slice(&9u16.to_be_bytes()); // Length = 9
    payload.push(0x04); // Type = IPv4 Endpoint
    payload.push(0x00); // Reserved
    payload.extend_from_slice(&client_ip.octets());
    payload.push(0x00); // Reserved
    payload.push(0x11); // L4 Protocol = UDP (0x11)
    payload.extend_from_slice(&client_port.to_be_bytes());

    // Build SOME/IP-SD packet using builder
    SomeIpPacketBuilder::notification(SD_SERVICE_ID, SD_METHOD_ID)
        .client_id(0x0001)
        .session_id(0x0001)
        .payload(payload.as_slice())
        .build()
}
