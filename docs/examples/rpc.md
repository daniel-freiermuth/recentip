# RPC: Request/Response & Fire-and-Forget

`RecentIP` supports two RPC patterns:
- **Request/Response**: Client sends request, server sends response
- **Fire-and-Forget**: Client sends request, no response expected

## Request/Response

### Client Side

```rust,no_run
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure().start().await?;
    let proxy = someip.find(0x1234).await?;

    let method = MethodId::new(0x0001).unwrap();
    
    // Synchronous call - waits for response
    let response = proxy.call(method, b"request data").await?;
    
    match response.return_code {
        ReturnCode::Ok => println!("Success: {:?}", response.payload),
        code => println!("Error: {:?}", code),
    }

    Ok(())
}
```

### Server Side

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
        if let ServiceEvent::Call { method, payload, responder, .. } = event {
            // Process based on method ID
            let response = match method.value() {
                0x0001 => process_get_status(&payload),
                0x0002 => process_set_value(&payload),
                _ => b"unknown method".to_vec(),
            };
            
            responder.reply(&response)?;
        }
    }

    Ok(())
}

fn process_get_status(_payload: &[u8]) -> Vec<u8> {
    b"status: OK".to_vec()
}

fn process_set_value(_payload: &[u8]) -> Vec<u8> {
    b"value set".to_vec()
}
```

## Fire-and-Forget

For one-way messages where no response is needed (e.g., notifications, logging):

### Client Side

```rust,no_run
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure().start().await?;
    let proxy = someip.find(0x1234).await?;

    // Fire-and-forget - no response expected
    let method = MethodId::new(0x0010).unwrap();
    proxy.fire_and_forget(method, b"log: button pressed").await?;

    Ok(())
}
```

### Server Side

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
        if let ServiceEvent::FireForget { method, payload, .. } = event {
            // No responder - just process the message
            println!("Fire-and-forget 0x{:04x}: {:?}", 
                method.value(), 
                String::from_utf8_lossy(&payload)
            );
        }
    }

    Ok(())
}
```

## Error Responses

Send error responses for failed requests:

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
        if let ServiceEvent::Call { method, payload, responder, .. } = event {
            if payload.is_empty() {
                // Send error response
                responder.reply_error(ApplicationError::MalformedMessage)?;
            } else {
                responder.reply(b"OK")?;
            }
        }
    }

    Ok(())
}
```
