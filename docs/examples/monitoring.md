# Service Monitoring

Monitor SOME/IP services on the network without participating in communication.

## SD Event Monitoring

Watch for service announcements, stops, and expirations:

```rust,no_run
use recentip::prelude::*;
use recentip::SdEvent;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure().start().await?;

    // Get the SD event stream
    let mut sd_events = someip.monitor_sd().await?;

    println!("Monitoring SOME/IP services...");

    while let Some(event) = sd_events.recv().await {
        match event {
            SdEvent::ServiceAvailable { 
                service_id, 
                instance_id, 
                endpoint,
                major_version,
                minor_version,
                .. 
            } => {
                println!("✓ Service 0x{:04x}:{} AVAILABLE at {} (v{}.{})",
                    service_id, 
                    instance_id,
                    endpoint,
                    major_version, minor_version
                );
            }
            SdEvent::ServiceUnavailable { 
                service_id, 
                instance_id,
                .. 
            } => {
                println!("✗ Service 0x{:04x}:{} UNAVAILABLE",
                    service_id, instance_id
                );
            }
            SdEvent::ServiceExpired { 
                service_id, 
                instance_id,
                .. 
            } => {
                println!("⏱ Service 0x{:04x}:{} EXPIRED (TTL elapsed)",
                    service_id, instance_id
                );
            }
        }
    }

    Ok(())
}
```

## Passive Discovery

Find services without blocking:

```rust,no_run
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure().start().await?;

    // With timeout - find() waits for SD announcement
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        someip.find(0x1234)
    ).await;

    match result {
        Ok(Ok(proxy)) => println!("Found service!"),
        Ok(Err(e)) => println!("Discovery error: {}", e),
        Err(_) => println!("Discovery timed out"),
    }

    Ok(())
}
```

## Multiple Service Discovery

Wait for multiple services:

```rust,no_run
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure().start().await?;

    // Start discovery for multiple services concurrently
    let (brake, engine, transmission) = tokio::try_join!(
        someip.find(0x1001),  // Brake service
        someip.find(0x1002),  // Engine service
        someip.find(0x1003),  // Transmission service
    )?;

    println!("All services discovered!");

    Ok(())
}
```

## Service Lifecycle Awareness

React to service availability changes:

```rust,no_run
use recentip::prelude::*;
use recentip::SdEvent;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure().start().await?;
    let mut sd_events = someip.monitor_sd().await?;

    let target_service = 0x1234;

    loop {
        // Wait for service to become available
        while let Some(event) = sd_events.recv().await {
            if let SdEvent::ServiceAvailable { service_id, .. } = event {
                if service_id == target_service {
                    println!("Target service available, connecting...");
                    break;
                }
            }
        }

        // Use the service
        let proxy = someip.find(target_service).await?;
        
        // Monitor for unavailability while using
        tokio::select! {
            result = use_service(&proxy) => {
                if let Err(e) = result {
                    println!("Service error: {}", e);
                }
            }
            Some(event) = sd_events.recv() => {
                if let SdEvent::ServiceUnavailable { service_id, .. } 
                    | SdEvent::ServiceExpired { service_id, .. } = event 
                {
                    if service_id == target_service {
                        println!("Service went away, waiting for recovery...");
                        continue;
                    }
                }
            }
        }
    }
}

async fn use_service(proxy: &recentip::OfferedService) -> Result<()> {
    let method = MethodId::new(0x0001).unwrap();
    loop {
        proxy.call(method, b"ping").await?;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
```
