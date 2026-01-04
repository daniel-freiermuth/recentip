//! Simple Service Offering Example
//!
//! This example offers a simple SOME/IP service that can be discovered
//! by the service_monitor example.
//!
//! # Usage
//!
//! ```bash
//! # Terminal 1: Start the monitor
//! cargo run --example service_monitor
//!
//! # Terminal 2: Start this service
//! cargo run --example simple_service
//! ```
//!
//! You should see the monitor detect this service becoming available.

use someip_runtime::prelude::*;
use someip_runtime::handle::ServiceEvent;

/// Example service definition
struct ExampleService;
impl Service for ExampleService {
    const SERVICE_ID: u16 = 0x1234;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .with_thread_ids(false)
        .init();

    println!("╔═══════════════════════════════════════════════════════╗");
    println!("║         SOME/IP Simple Service Example                ║");
    println!("╚═══════════════════════════════════════════════════════╝");
    println!();
    println!("Starting service 0x{:04X} instance 1...", ExampleService::SERVICE_ID);
    
    // Create runtime
    let config = RuntimeConfig::default();
    let runtime = Runtime::new(config).await?;
    
    // Offer the service
    let mut offering = runtime.offer::<ExampleService>(InstanceId::Id(1)).await?;
    
    println!("✓ Service is now being announced via Service Discovery");
    println!("  Press Ctrl+C to stop");
    println!();
    
    // Handle incoming requests
    loop {
        tokio::select! {
            Some(event) = offering.next() => {
                match event {
                    ServiceEvent::Call { method, payload, responder, .. } => {
                        println!("← Received method call 0x{:04X} with {} bytes", method.value(), payload.len());
                        
                        // Echo back the payload
                        if let Err(e) = responder.reply(&payload).await {
                            eprintln!("  Error sending response: {}", e);
                        } else {
                            println!("→ Sent response");
                        }
                    }
                    ServiceEvent::FireForget { method, payload, .. } => {
                        println!("← Received fire-and-forget 0x{:04X} with {} bytes", method.value(), payload.len());
                    }
                    ServiceEvent::Subscribe { eventgroup, ack, .. } => {
                        println!("← Subscription request for eventgroup 0x{:04X}", eventgroup.value());
                        if let Err(e) = ack.accept().await {
                            eprintln!("  Error accepting subscription: {}", e);
                        } else {
                            println!("→ Subscription accepted");
                        }
                    }
                    ServiceEvent::Unsubscribe { eventgroup, .. } => {
                        println!("← Unsubscribe request for eventgroup 0x{:04X}", eventgroup.value());
                    }
                }
            }
            
            _ = tokio::signal::ctrl_c() => {
                println!();
                println!("Shutting down service...");
                break;
            }
        }
    }
    
    // Graceful shutdown
    runtime.shutdown().await;
    println!("Service stopped.");
    
    Ok(())
}
