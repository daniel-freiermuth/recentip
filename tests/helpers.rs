use recentip::ServiceOffering;

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
