# Transport: UDP, TCP, and Configuration

`RecentIP` supports both UDP and TCP transports with various configuration options.

## Transport Selection

### Server: Choosing Transport

```rust,no_run
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure()
        .sd_unicast("192.168.1.100".parse().unwrap())
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .start().await?;

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
        .sd_unicast("192.168.1.100".parse().unwrap())
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .preferred_transport(Transport::Tcp)
        .start()
        .await?;

    // This will use TCP if the service offers it
    let found_service = someip.find(0x1234).await?;

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
        .sd_unicast("192.168.1.100".parse().unwrap())
        .sd_multicast_group("239.255.255.250".parse().unwrap())
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
    let someip = recentip::configure()
        .sd_unicast("192.168.1.100".parse().unwrap())
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        .start().await?;

    // Multiple calls to the same service reuse the TCP connection
    let found_service = someip.find(0x1234).await?;
    
    let method = MethodId::new(0x0001).unwrap();
    
    // All these calls use the same underlying TCP connection
    for i in 0..100 {
        let _ = found_service.call(method, &[i as u8]).await?;
    }

    Ok(())
}
```

## Service Discovery Configuration

```rust,no_run
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let someip = recentip::configure()
        .sd_unicast("192.168.1.100".parse().unwrap())
        .sd_multicast_group("239.255.255.250".parse().unwrap())
        // Offer TTL in seconds (default: 3600)
        .offer_ttl(5)
        .start()
        .await?;

    Ok(())
}
```

## vsomeip Interoperability

To communicate with vsomeip-based services:

```rust,no_run
use recentip::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // vsomeip uses 224.224.224.0 as default multicast
    let someip = recentip::configure()
        .sd_unicast("192.168.1.100".parse().unwrap())
        .sd_multicast_group("224.224.224.0".parse().unwrap())
        .start()
        .await?;

    // Now compatible with vsomeip services on the network
    let found_service = someip.find(0x1234).await?;

    Ok(())
}
```
