# Pub/Sub: Events and Subscriptions

SOME/IP supports publish/subscribe patterns via eventgroups. Publishers send events
to subscribers who have registered interest in specific eventgroups.

## Core Concepts

- **Event**: A message sent from server to subscribed clients
- **Eventgroup**: A logical grouping of related events
- **Subscription**: Client's registration to receive events from eventgroup(s)

## Publishing Events

### Setup

```rust,no_run
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure().start().await?;

    let mut offering = someip
        .offer(0x1234, InstanceId::Id(0x0001))
        .version(1, 0)
        .start()
        .await?;

    // Define eventgroups
    let sensor_eg = EventgroupId::new(0x0001).unwrap();
    let status_eg = EventgroupId::new(0x0002).unwrap();

    // Create events with their eventgroup membership
    let temperature = offering
        .event(EventId::new(0x8001).unwrap())
        .eventgroup(sensor_eg)
        .create()
        .await?;
    
    let humidity = offering
        .event(EventId::new(0x8002).unwrap())
        .eventgroup(sensor_eg)
        .create()
        .await?;
    
    let status = offering
        .event(EventId::new(0x8010).unwrap())
        .eventgroup(status_eg)
        .create()
        .await?;

    // Publish periodically
    loop {
        temperature.notify(b"22.5C").await?;
        humidity.notify(b"45%").await?;
        status.notify(b"OK").await?;
        
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
```

### Events in Multiple Eventgroups

An event can belong to multiple eventgroups:

```rust,no_run
# use recentip::prelude::*;
# async fn example(offering: &mut recentip::ServiceOffering) -> Result<()> {
let eg1 = EventgroupId::new(0x0001).unwrap();
let eg2 = EventgroupId::new(0x0002).unwrap();

// This event is sent to subscribers of EITHER eventgroup
let shared_event = offering
    .event(EventId::new(0x8001).unwrap())
    .eventgroup(eg1)
    .eventgroup(eg2)
    .create()
    .await?;
# Ok(())
# }
```

## Subscribing to Events

### Single Eventgroup

```rust,no_run
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure().start().await?;
    let proxy = someip.find(0x1234).await?;

    let sensor_eg = EventgroupId::new(0x0001).unwrap();
    
    let mut subscription = proxy
        .new_subscription()
        .eventgroup(sensor_eg)
        .await?;

    while let Some(event) = subscription.next().await {
        println!("Received event 0x{:04x}", event.event_id.value());
    }

    Ok(())
}
```

### Multiple Eventgroups

Subscribe to multiple eventgroups in a single subscription:

```rust,no_run
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure().start().await?;
    let proxy = someip.find(0x1234).await?;

    let sensor_eg = EventgroupId::new(0x0001).unwrap();
    let status_eg = EventgroupId::new(0x0002).unwrap();
    
    // Subscribe to both eventgroups
    let mut subscription = proxy
        .new_subscription()
        .eventgroup(sensor_eg)
        .eventgroup(status_eg)
        .await?;

    // Receive events from all subscribed eventgroups
    while let Some(event) = subscription.next().await {
        match event.event_id.value() {
            0x8001 => println!("Temperature: {:?}", event.payload),
            0x8002 => println!("Humidity: {:?}", event.payload),
            0x8010 => println!("Status: {:?}", event.payload),
            id => println!("Unknown event 0x{:04x}", id),
        }
    }

    Ok(())
}
```

## Handling Subscription Lifecycle

### Server Side: Subscription Events

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

    while let Some(event) = offering.next().await {
        match event {
            ServiceEvent::Subscribe { eventgroup, client } => {
                println!("Client {:?} subscribed to EG 0x{:04x}", 
                    client, eventgroup.value());
                // Subscriptions are auto-accepted
            }
            ServiceEvent::Unsubscribe { eventgroup, client } => {
                println!("Client {:?} unsubscribed from EG 0x{:04x}", 
                    client, eventgroup.value());
            }
            _ => {}
        }
    }

    Ok(())
}
```

### Client Side: Subscription Drops

When a subscription handle is dropped, an unsubscribe message is sent automatically:

```rust,no_run
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure().start().await?;
    let proxy = someip.find(0x1234).await?;

    {
        let eg = EventgroupId::new(0x0001).unwrap();
        let mut sub = proxy
            .new_subscription()
            .eventgroup(eg)
            .await?;
        
        // Receive some events...
        if let Some(event) = sub.next().await {
            println!("Got event: {:?}", event);
        }
        
        // sub dropped here -> automatic unsubscribe
    }

    // Subscription is now unsubscribed
    Ok(())
}
```
