# Quick Start

This guide walks through the basic patterns for using `RecentIP`.

## Minimal Client

The simplest way to call a remote SOME/IP service:

```rust,no_run
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // Start the runtime
    let someip = recentip::configure().start().await?;

    // Find a service by ID (blocks until SD announcement received)
    let found_service = someip.find(0x1234).await?;

    // Call a method
    let method = MethodId::new(0x0001).unwrap();
    let response = found_service.call(method, b"hello").await?;

    println!("Got {} bytes back", response.payload.len());
    Ok(())
}
```

## Minimal Server

Offer a service that handles incoming calls:

```rust,no_run
use recentip::prelude::*;
use recentip::handle::ServiceEvent;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure().start().await?;

    // Offer a service (automatically announced via SD)
    let mut offering = someip
        .offer(0x1234, InstanceId::Id(0x0001))
        .version(1, 0)
        .start()
        .await?;

    // Handle events
    while let Some(event) = offering.next().await {
        match event {
            ServiceEvent::Call { responder, payload, .. } => {
                // Echo the payload back
                responder.reply(&payload)?;
            }
            _ => {}
        }
    }

    Ok(())
}
```

## Subscribe to Events

Receive events from a publisher:

```rust,no_run
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure().start().await?;
    let proxy = someip.find(0x1234).await?;

    // Subscribe to eventgroups
    let eg = EventgroupId::new(0x0001).unwrap();
    let mut subscription = proxy
        .new_subscription()
        .eventgroup(eg)
        .subscribe()
        .await?;

    // Receive events
    while let Some(event) = subscription.next().await {
        println!("Event 0x{:04x}: {} bytes", 
            event.event_id.value(), 
            event.payload.len()
        );
    }

    Ok(())
}
```

## Publish Events

Send events to subscribers:

```rust,no_run
use recentip::prelude::*;
use recentip::handle::ServiceEvent;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure().start().await?;

    let mut offering = someip
        .offer(0x1234, InstanceId::Id(0x0001))
        .version(1, 0)
        .start()
        .await?;

    // Create an event that belongs to eventgroup 0x0001
    let eg = EventgroupId::new(0x0001).unwrap();
    let event_id = EventId::new(0x8001).unwrap();
    let event = offering
        .event(event_id)
        .eventgroup(eg)
        .create()
        .await?;

    // Publish to all subscribers
    event.notify(b"sensor data").await?;

    Ok(())
}
```
