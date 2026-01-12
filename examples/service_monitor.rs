//! Service Monitor Example
//!
//! This example demonstrates how to monitor SOME/IP service announcements
//! on the network using the SD monitoring API. It listens for all Service
//! Discovery events and reports service lifecycle:
//! - Service announcements (ServiceAvailable)
//! - Service stops (ServiceUnavailable)
//! - Service TTL expirations (ServiceExpired)
//!
//! # Usage
//!
//! ```bash
//! # Use default multicast (239.255.0.1 - SOME/IP spec standard)
//! cargo run --example service_monitor
//!
//! # Use vsomeip's default multicast address
//! cargo run --example service_monitor -- --multicast 224.224.224.0
//!
//! # Specify local IP address (useful for multicast routing)
//! cargo run --example service_monitor -- -m 224.224.224.0 -l 192.168.1.100
//!
//! # Short form
//! cargo run --example service_monitor -- -m 224.224.224.0 -l 192.168.1.100
//!
//! # Show help
//! cargo run --example service_monitor -- --help
//! ```
//!
//! The monitor will print events as they occur on the network.
//!
//! # Example Output
//!
//! ```text
//! Using multicast address: 224.224.224.0:30490
//! [2026-01-04 12:00:00] ✓ Service 0x1234:1 AVAILABLE at 192.168.1.100:30500 (v1.0, TTL=3s)
//! [2026-01-04 12:00:15] ✗ Service 0x1234:1 UNAVAILABLE
//! [2026-01-04 12:00:18] ⏱ Service 0x5678:2 EXPIRED
//! ```
//!
//! # Testing
//!
//! To test this monitor, run the `simple_service` example in another terminal:
//!
//! ```bash
//! cargo run --example simple_service
//! ```
//!
//! To test with vsomeip (which uses 224.224.224.0 by default):
//!
//! ```bash
//! cargo run --example service_monitor -- -m 224.224.224.0
//! ```

use clap::Parser;
use recentip::{Runtime, RuntimeConfig, SdEvent};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

/// SOME/IP Service Discovery Monitor
///
/// Monitors the network for SOME/IP service announcements, stops, and expirations.
/// Supports both the standard SOME/IP multicast address (239.255.0.1) and vsomeip's
/// default address (224.224.224.0).
#[derive(Parser, Debug)]
#[command(name = "service_monitor")]
#[command(version, about, long_about = None)]
struct Args {
    /// Multicast address for Service Discovery
    ///
    /// Common values:
    ///   239.255.0.1  - SOME/IP specification default (standard)
    ///   224.224.224.0 - vsomeip default
    #[arg(short, long, default_value = "239.255.0.1")]
    multicast: Ipv4Addr,

    /// Local IP address to bind to
    ///
    /// Use your network interface IP (e.g., 192.168.1.100) for multicast to work
    /// across the network. Use 0.0.0.0 to bind to all interfaces (default).
    #[arg(short, long)]
    local: Option<Ipv4Addr>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments
    let args = Args::parse();

    // Initialize logging (set to WARN to reduce noise)
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_target(false)
        .with_thread_ids(false)
        .init();

    println!("╔═══════════════════════════════════════════════════════╗");
    println!("║       SOME/IP Service Discovery Monitor v0.2          ║");
    println!("╚═══════════════════════════════════════════════════════╝");
    println!();
    println!("Using multicast address: {}:30490", args.multicast);
    if args.multicast == Ipv4Addr::new(224, 224, 224, 0) {
        println!("  (vsomeip default - non-standard)");
    } else if args.multicast == Ipv4Addr::new(239, 255, 0, 1) {
        println!("  (SOME/IP specification standard)");
    }
    if let Some(local) = args.local {
        println!("Binding to local address: {}:30490", local);
    } else {
        println!("Binding to all interfaces (0.0.0.0:30490)");
    }
    println!("Monitoring network for all SOME/IP service events...");
    println!("Press Ctrl+C to exit.");
    println!();
    println!("{:<25} {:<15} {:<30}", "Timestamp", "Service:Inst", "Event");
    println!("{:-<80}", "");

    // Create runtime with custom multicast address
    let config = RuntimeConfig {
        bind_addr: if let Some(local) = args.local {
            SocketAddr::V4(SocketAddrV4::new(local, 30490))
        } else {
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 30490))
        },
        sd_multicast: SocketAddr::V4(SocketAddrV4::new(args.multicast, 30490)),
        ..Default::default()
    };
    let runtime = Runtime::new(config).await?;

    // Get SD event monitor channel
    let mut sd_events = runtime.monitor_sd().await?;

    // Set up Ctrl+C handler
    let mut shutdown = tokio::spawn(async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen for Ctrl+C");
    });

    // Process SD events
    loop {
        tokio::select! {
            Some(event) = sd_events.recv() => {
                let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
                match event {
                    SdEvent::ServiceAvailable { service_id, instance_id, major_version, minor_version, endpoint, ttl } => {
                        println!(
                            "[{}] ✓ Service 0x{:04X}:{} AVAILABLE at {} (v{}.{}, TTL={}s)",
                            timestamp, service_id, instance_id, endpoint, major_version, minor_version, ttl
                        );
                    }
                    SdEvent::ServiceUnavailable { service_id, instance_id } => {
                        println!(
                            "[{}] ✗ Service 0x{:04X}:{} UNAVAILABLE (stopped)",
                            timestamp, service_id, instance_id
                        );
                    }
                    SdEvent::ServiceExpired { service_id, instance_id } => {
                        println!(
                            "[{}] ⏱ Service 0x{:04X}:{} EXPIRED (TTL timeout)",
                            timestamp, service_id, instance_id
                        );
                    }
                }
            }
            _ = &mut shutdown => {
                println!();
                println!("Shutting down monitor...");
                break;
            }
        }
    }

    runtime.shutdown().await;
    Ok(())
}
