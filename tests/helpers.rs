use bytes::{Bytes, BytesMut};
use recentip::ServiceOffering;
use tokio::io::AsyncReadExt;

// Import builders from compliance test's wire_format helpers
#[path = "compliance/wire_format/helpers.rs"]
mod wire_format_helpers;
use wire_format_helpers::{SdOfferBuilder, SdSubscribeAckBuilder};

pub(crate) async fn wait_for_subscription(offering: &mut ServiceOffering) -> Result<(), ()> {
    loop {
        match offering.next().await {
            Some(e) => match e {
                recentip::ServiceEvent::Subscribe { .. } => break Ok(()),
                _ => continue,
            },
            None => break Err(()),
        }
    }
}

/// A simulated wire-level SOME/IP-SD server for testing.
///
/// Tracks session ID and reboot flag properly:
/// - Session ID increments with each message sent
/// - Reboot flag is set only on the first message after construction
///
/// This prevents false reboot detection in clients receiving messages from this server.
#[derive(Debug)]
pub struct WireServer {
    session_id: u16,
    sent_first_message: bool,
}

impl WireServer {
    /// Create a new WireServer with session_id=1 and reboot flag pending.
    pub fn new() -> Self {
        Self {
            session_id: 1,
            sent_first_message: false,
        }
    }

    /// Get the current session ID and reboot flag, then advance state.
    ///
    /// Returns (session_id, reboot_flag) for use in the next message.
    /// - First call: session_id=1, reboot=true
    /// - Second call: session_id=2, reboot=false
    /// - etc.
    fn next_session(&mut self) -> (u16, bool) {
        let session = self.session_id;
        let reboot = !self.sent_first_message;

        self.sent_first_message = true;
        self.session_id = self.session_id.wrapping_add(1);
        if self.session_id == 0 {
            self.session_id = 1; // Skip 0 per spec
        }

        (session, reboot)
    }

    /// Build a SOME/IP-SD OfferService message with proper session handling.
    pub fn build_offer(
        &mut self,
        service_id: u16,
        instance_id: u16,
        major_version: u8,
        minor_version: u32,
        addr: std::net::Ipv4Addr,
        port: u16,
        ttl: u32,
    ) -> Vec<u8> {
        let (session_id, reboot) = self.next_session();

        SdOfferBuilder::new(service_id, instance_id, addr, port)
            .major_version(major_version)
            .minor_version(minor_version)
            .ttl(ttl)
            .session_id(session_id)
            .reboot_flag(reboot)
            .unicast_flag(true) // WireServer uses unicast session channel
            .build()
    }

    /// Build a SOME/IP-SD SubscribeEventgroupNack message (TTL=0) with proper session handling.
    pub fn build_subscribe_nack(
        &mut self,
        service_id: u16,
        instance_id: u16,
        major_version: u8,
        eventgroup_id: u16,
    ) -> Vec<u8> {
        let (session_id, reboot) = self.next_session();

        SdSubscribeAckBuilder::new(service_id, instance_id, eventgroup_id)
            .major_version(major_version)
            .ttl(0) // TTL=0 means NACK
            .session_id(session_id)
            .reboot_flag(reboot)
            .unicast_flag(true) // WireServer uses unicast session channel
            .build()
    }
}

/// Read a framed SOME/IP message from a TCP stream (test helper).
///
/// This helper is used in tests that simulate the server side "on the wire"
/// and need to manually handle TCP framing. Normal users should use the library's
/// internal TCP handling which manages framing automatically.
///
/// SOME/IP TCP framing uses the length field in the header:
/// - Header is 16 bytes, length field at offset 4-8
/// - Length field = 8 + payload length (includes `client_id` through end)
/// - Total message size = 8 (service_id, method_id, length) + length value
pub(crate) async fn read_tcp_message<S: AsyncReadExt + Unpin>(
    stream: &mut S,
    buffer: &mut BytesMut,
) -> std::io::Result<Option<Bytes>> {
    loop {
        // Check if we have enough data for a complete message
        if buffer.len() >= 16 {
            // Parse length from header (offset 4-8, big-endian u32)
            let length = u32::from_be_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]);

            // Total message size = 8 (service_id, method_id, length) + length
            let total_size = 8 + length as usize;

            if buffer.len() >= total_size {
                // We have a complete message
                let message = buffer.split_to(total_size).freeze();
                return Ok(Some(message));
            }
        }

        // Need more data - read from stream
        let mut read_buf = [0u8; 8192];
        let n = stream.read(&mut read_buf).await?;
        if n == 0 {
            // Connection closed
            return if buffer.is_empty() {
                Ok(None)
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Connection closed with partial message",
                ))
            };
        }
        buffer.extend_from_slice(&read_buf[..n]);
    }
}

/// Check if bytes represent a Magic Cookie message (test helper).
///
/// Returns true if the first 4 bytes match Magic Cookie pattern
/// (Service ID 0xFFFF, Method ID 0x0000 or 0x8000).
///
/// This is used in tests that simulate TCP "on the wire" behavior.
pub(crate) fn is_magic_cookie(data: &[u8]) -> bool {
    matches!(data.get(..4), Some([0xFF, 0xFF, 0x00 | 0x80, 0x00]))
}

/// Generate a client-side Magic Cookie message (test helper).
///
/// This is used in tests that simulate TCP "on the wire" behavior.
pub(crate) const fn magic_cookie_client() -> [u8; 16] {
    [
        0xFF, 0xFF, // Service ID: 0xFFFF
        0x00, 0x00, // Method ID: 0x0000 (client)
        0x00, 0x00, 0x00, 0x08, // Length: 8
        0xDE, 0xAD, // Client ID: 0xDEAD
        0xBE, 0xEF, // Session ID: 0xBEEF
        0x01, // Protocol Version
        0x01, // Interface Version
        0x01, // Message Type: Request
        0x00, // Return Code
    ]
}

/// Generate a server-side Magic Cookie message (test helper).
///
/// This is used in tests that simulate TCP "on the wire" behavior.
pub(crate) const fn magic_cookie_server() -> [u8; 16] {
    [
        0xFF, 0xFF, // Service ID: 0xFFFF
        0x80, 0x00, // Method ID: 0x8000 (server)
        0x00, 0x00, 0x00, 0x08, // Length: 8
        0xDE, 0xAD, // Client ID: 0xDEAD
        0xBE, 0xEF, // Session ID: 0xBEEF
        0x01, // Protocol Version
        0x01, // Interface Version
        0x01, // Message Type: Request
        0x00, // Return Code
    ]
}

pub(crate) fn configure_tracing() {
    use std::sync::OnceLock;
    static TRACING_INIT: OnceLock<()> = OnceLock::new();
    TRACING_INIT.get_or_init(|| {
        tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::builder()
                        .with_default_directive(tracing::level_filters::LevelFilter::INFO.into())
                        .from_env_lossy(),
                )
                .with_test_writer()
                .with_timer(SimElapsedTime)
                .finish(),
        )
        .expect("Configure tracing");
    });
}

#[derive(Clone)]
struct SimElapsedTime;
impl tracing_subscriber::fmt::time::FormatTime for SimElapsedTime {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        // Prints real time and sim elapsed time. Example: 2024-01-10T17:06:57.020452Z [76ms]
        tracing_subscriber::fmt::time()
            .format_time(w)
            .and_then(|()| write!(w, " [{:?}]", turmoil::sim_elapsed().unwrap_or_default()))
    }
}
