# Transport: UDP, TCP, and Configuration

`RecentIP` supports both UDP and TCP transports with various configuration options.

## Transport Selection

### Server: Choosing Transport

```rust,no_run
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure().start().await?;

    // UDP only (default)
    let _udp_service = someip
        .offer(0x1234, InstanceId::Id(0x0001))
        .version(1, 0)
        .udp()
        .start()
        .await?;

    // TCP only
    let _tcp_service = someip
        .offer(0x1235, InstanceId::Id(0x0001))
        .version(1, 0)
        .tcp()
        .start()
        .await?;

    // Both transports (client chooses)
    let _dual_service = someip
        .offer(0x1236, InstanceId::Id(0x0001))
        .version(1, 0)
        .udp()
        .tcp()
        .start()
        .await?;

    Ok(())
}
```

### Client: Preferred Transport

When a service offers both UDP and TCP, clients can specify preference:

```rust,no_run
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // Prefer TCP when available
    let someip = recentip::configure()
        .preferred_transport(Transport::Tcp)
        .start()
        .await?;

    // This will use TCP if the service offers it
    let proxy = someip.find(0x1234).await?;

    Ok(())
}
```

## TCP Features

### Magic Cookies

Magic Cookies help debug TCP message boundaries:

```rust,no_run
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure()
        .magic_cookies(true)  // Enable magic cookie insertion
        .start()
        .await?;

    // TCP messages will include magic cookies for debugging
    Ok(())
}
```

### Connection Reuse

TCP connections are automatically pooled and reused:

```rust,no_run
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure().start().await?;

    // Multiple calls to the same service reuse the TCP connection
    let proxy = someip.find(0x1234).await?;
    
    let method = MethodId::new(0x0001).unwrap();
    
    // All these calls use the same underlying TCP connection
    for i in 0..100 {
        let _ = proxy.call(method, &[i as u8]).await?;
    }

    Ok(())
}
```

## Service Discovery Configuration

```rust,no_run
use recentip::prelude::*;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure()
        // Service Discovery multicast address (default: 239.255.0.1:30490)
        .sd_multicast(SocketAddr::from(([224, 224, 224, 0], 30490)))
        
        // Offer TTL in seconds (default: 3600)
        .offer_ttl(5)
        
        // Local IP to advertise (auto-detected if not set)
        .advertised_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)))
        
        .start()
        .await?;

    Ok(())
}
```

## vsomeip Interoperability

To communicate with vsomeip-based services:

```rust,no_run
use recentip::prelude::*;
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> Result<()> {
    // vsomeip uses 224.224.224.0 as default multicast
    let someip = recentip::configure()
        .sd_multicast(SocketAddr::from(([224, 224, 224, 0], 30490)))
        .start()
        .await?;

    // Now compatible with vsomeip services on the network
    let proxy = someip.find(0x1234).await?;

    Ok(())
}
```
