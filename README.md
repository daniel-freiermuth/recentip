# someip-runtime

[![Crate](https://img.shields.io/crates/v/someip-runtime.svg)](https://crates.io/crates/someip-runtime)
[![Docs](https://docs.rs/someip-runtime/badge.svg)](https://docs.rs/someip-runtime)
[![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](LICENSE)

A **type-safe, async SOME/IP protocol implementation** for [tokio](https://tokio.rs).

SOME/IP (Scalable service-Oriented MiddlewarE over IP) is the standard middleware protocol for automotive Ethernet communication, enabling service-oriented communication between ECUs in modern vehicles.

## Features

- **Type-safe API** — Compile-time guarantees via the `Service` trait and type-state patterns
- **Async/await** — Native tokio integration with zero-cost futures
- **Service Discovery** — Automatic discovery via multicast SD protocol
- **RPC** — Request/response and fire-and-forget method calls
- **Pub/Sub** — Event subscriptions with eventgroup management
- **Dual transport** — UDP (default) and TCP with Magic Cookie support
- **Spec compliance** — 269 tests covering SOME/IP specification requirements

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
someip-runtime = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

## Quick Start

### Define a Service

Services are defined by implementing the `Service` trait (typically generated from FIDL/Franca IDL):

```rust
use someip_runtime::prelude::*;

struct BrakeService;
impl Service for BrakeService {
    const SERVICE_ID: u16 = 0x1234;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}
```

### Client: Find and Call a Service

```rust
use someip_runtime::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // Create the runtime
    let runtime = Runtime::new(RuntimeConfig::default()).await?;

    // Find a remote service (waits for SD announcement)
    let proxy = runtime.find::<BrakeService>(InstanceId::Any)
        .available().await?;

    // Call a method (RPC)
    let method_id = MethodId::new(0x0001).unwrap();
    let response = proxy.call(method_id, b"request payload").await?;
    
    if response.return_code == ReturnCode::Ok {
        println!("Success: {} bytes", response.payload.len());
    }

    Ok(())
}
```

### Server: Offer a Service

```rust
use someip_runtime::prelude::*;
use someip_runtime::handle::ServiceEvent;

#[tokio::main]
async fn main() -> Result<()> {
    let runtime = Runtime::new(RuntimeConfig::default()).await?;

    // Offer a service (announces via SD)
    let mut offering = runtime.offer::<BrakeService>(InstanceId::Id(0x0001)).await?;

    // Handle incoming requests
    while let Some(event) = offering.next().await {
        match event {
            ServiceEvent::Call { method, payload, responder, .. } => {
                // Process request and send response
                responder.reply(b"OK").await?;
            }
            ServiceEvent::Subscribe { eventgroup, ack, .. } => {
                // Accept subscription
                ack.accept().await?;
            }
            _ => {}
        }
    }
    Ok(())
}
```

## Configuration

```rust
use someip_runtime::{RuntimeConfig, Transport};

let config = RuntimeConfig::builder()
    .transport(Transport::Tcp)      // Use TCP instead of UDP
    .magic_cookies(true)            // Enable Magic Cookies for debugging
    .sd_port(30490)                 // SD multicast port (default)
    .ttl(3)                         // Service TTL in seconds
    .build();

let runtime = Runtime::new(config).await?;
```

## API Overview

| Type | Role | Pattern |
|------|------|---------|
| `Runtime` | Central coordinator, owns sockets | — |
| `ProxyHandle<S, State>` | Client proxy to remote service | `Unavailable` → `Available` |
| `OfferingHandle<S>` | Server handle for offered service | — |
| `ServiceInstance<S, State>` | Advanced server with bind/announce control | `Bound` → `Announced` |
| `Subscription` | Receive events from eventgroup | — |
| `Responder` | Reply to incoming RPC request | Consumed on reply |

## Identifier Types

| Type | Valid Range | Notes |
|------|-------------|-------|
| `ServiceId` | 0x0001–0xFFFE | 0x0000 and 0xFFFF reserved |
| `InstanceId` | 0x0001–0xFFFE or `Any` | 0xFFFF = wildcard |
| `MethodId` | 0x0000–0x7FFF | Bit 15 = 0 for methods |
| `EventId` | 0x8000–0xFFFE | Bit 15 = 1 for events |
| `EventgroupId` | 0x0001–0xFFFE | Groups related events |

## Static Deployments

For systems without Service Discovery (pre-configured addresses):

```rust
// Client: connect directly to known address
let proxy = runtime.find_static::<BrakeService>(
    InstanceId::Id(1),
    "192.168.1.10:30509".parse().unwrap(),
);
// Immediately usable, no SD wait

// Server: bind without announcing
let service = runtime.bind::<BrakeService>(InstanceId::Id(1)).await?;
// Add static subscribers manually
service.add_static_subscriber("192.168.1.20:30502", &[eventgroup]);
```

## Testing

The library uses [turmoil](https://docs.rs/turmoil) for deterministic network simulation:

```bash
# Run all tests
cargo nextest run --features turmoil

# Run with coverage
cargo tarpaulin --features turmoil

# Doc tests only
cargo test --doc
```

## Specification Compliance

This implementation targets compliance with the SOME/IP specification. Test coverage is tracked in `spec-data/coverage.json` with compliance tests in `tests/compliance/`.

Key compliance areas:
- Wire format (16-byte header, session ID wrapping)
- Service Discovery (offer, find, subscribe entries)
- Transport Protocol (UDP, TCP with Magic Cookies)
- Error handling (return codes, EXCEPTION message type)

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      User Application                       │
│   ProxyHandle          OfferingHandle         ServiceInstance│
└──────────┬─────────────────┬─────────────────────┬──────────┘
           │ Commands        │ Commands            │ Commands
           ▼                 ▼                     ▼
┌─────────────────────────────────────────────────────────────┐
│                    Runtime (Event Loop)                      │
│   RuntimeState: offered, discovered, pending_calls, etc.    │
│   select! over: commands, SD socket, RPC socket, TCP, timer │
└─────────────────────────────────────────────────────────────┘
           │                              │
           ▼                              ▼
    ┌─────────────┐                ┌─────────────┐
    │ SD Socket   │                │ RPC Socket  │
    │ UDP:30490   │                │ UDP/TCP     │
    └─────────────┘                └─────────────┘
```

## License

This project is licensed under the [GPL-3.0 License](LICENSE).

## Contributing

Contributions are welcome! Please see the main repository's contributing guidelines.

When adding features:
1. Add command variant to `command.rs` if handle→runtime communication needed
2. Add state to `state.rs` if persistent tracking required  
3. Add handler to `client.rs` or `server.rs` depending on role
4. Wire up in `runtime.rs` event loop
5. Add public API to `handle.rs`
6. Write compliance tests in `tests/compliance/`
