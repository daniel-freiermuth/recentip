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
//! cargo run --example service_monitor
//! ```
//!
//! The monitor will print events as they occur on the network.
//!
//! # Example Output
//!
//! ```text
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

use someip_runtime::{Runtime, RuntimeConfig, SdEvent};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    println!("Monitoring network for all SOME/IP service events...");
    println!("Press Ctrl+C to exit.");
    println!();
    println!("{:<25} {:<15} {:<30}", "Timestamp", "Service:Inst", "Event");
    println!("{:-<80}", "");

    // Create runtime
    let config = RuntimeConfig::default();
    let runtime = Runtime::new(config).await?;

    // Get SD event monitor channel
    let mut sd_events = runtime.monitor_sd().await?;

    // Set up Ctrl+C handler
    let mut shutdown = tokio::spawn(async {
        tokio::signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
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
