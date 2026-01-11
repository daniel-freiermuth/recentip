# RecentIP

> Warning! This is an alpha stage toy project for exploration.
The goal is to create a solid, easy-to-use and performant middleware implementation that is easily and fearlessly maintainable. One of the side-results is a comprehensive documentation.

[![Crate](https://img.shields.io/crates/v/someip-runtime.svg)](https://crates.io/crates/someip-runtime)
[![Docs](https://docs.rs/someip-runtime/badge.svg)](https://docs.rs/someip-runtime)
[![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](LICENSE)

An opinionated **type-safe, async, lock-free, no-unsafe, no-panic, boring SOME/IP protocol implementation** backed by [tokio](https://tokio.rs).

SOME/IP (Scalable service-Oriented MiddlewarE over IP) is the standard middleware protocol for automotive Ethernet communication, enabling service-oriented communication between ECUs in modern vehicles.

## Features

- **Type-safe API** — Compile-time guarantees via type-state patterns
- **Async/await** — Native tokio integration with zero-cost futures
- **Service Discovery** — Automatic discovery via multicast SD protocol
- **RPC** — Request/response and fire-and-forget method calls
- **Pub/Sub** — Event subscriptions with eventgroup management
- **Dual transport** — UDP (default) and TCP with Magic Cookie support
- **Spec compliance** — tests covering SOME/IP specification requirements

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
recentip = "0.1"
```

## Quick Start

### Client: Find and Call a Service

```rust
use someip_runtime::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // Create the runtime
    let runtime = Runtime::new(RuntimeConfig::default()).await?;

    // Find a remote service (waits for SD announcement)
    let proxy = runtime.find(BRAKE_SERVICE_ID).await?;

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
    let mut offering = runtime.offer(BRAKE_SERVICE_ID, InstanceId::Id(0x0001))
        .version(BRAKE_VERSION.0, BRAKE_VERSION.1)
        .start()
        .await?;

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

## Examples
Check the `examples` folder for examples.

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
| `ProxyHandle` | Client proxy to remote service | — |
| `OfferingHandle` | Server handle for offered service | — |
| `ServiceInstance<State>` | Advanced server with bind/announce control | `Bound` → `Announced` |
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
let proxy = runtime.find_static(
    BRAKE_SERVICE_ID,
    InstanceId::Id(1),
    "192.168.1.10:30509".parse().unwrap(),
    Transport::Tcp,  // explicit transport selection
);
// Immediately usable, no SD wait

// Server: bind without announcing
let service = runtime.bind(BRAKE_SERVICE_ID, InstanceId::Id(1), BRAKE_VERSION, Transport::Udp).await?;
// Add static subscribers manually
service.add_static_subscriber("192.168.1.20:30502", &[eventgroup]);
```

## SMIP integration
I haven't looked into this yet, but https://github.com/thoughtworks/smip looks
like it would be great on top of this lib. https://rocket.rs/ -like annotations
for SOME/IP.

## Other Rust Some/IP libs
- SummR https://github.com/eclipse-sommr seems dead or never kicked off?
- https://crates.io/crates/someip-rs fresh and new. Sync IO, no SD. Blocking
  on calls.

## Testing

The library comes with several test suites:

- unit tests in each module
- compliance tests in `tests/compliance` certifying compliance with the SOME/IP specs backed
- a few real network tests. most other tests are backed by turmoil
- API behavior tests
- (in planning) vsomeip compat tests certying compatibility to vsomeip
- (in planning) docker tests for exotic setups (e.g. multi-homed with separate networks on the same multicast)
- (in planning) multi-platform tests

The library uses [turmoil](https://docs.rs/turmoil) for deterministic network simulation.
Tests should be executed using nextest as it is configured to honor a subset of tests that needs to be executed sequentially.
We aim for 100% code coverage.

```bash
# Run most tests (~10s)
cargo nextest run

# Run all tests (~40s)
cargo nextest run --features slow-tests

# Run with coverage
cargo llvm-cov nextest --features slow-tests

# Doc tests only
cargo test --doc
```

### Custom Lints

For **compile-time** checking, use the included [dylint](https://github.com/trailofbits/dylint) lint crate:

```bash
# Install dylint
cargo install cargo-dylint dylint-link

# Run lints
cargo dylint --all
```

The `RUNTIME_MUST_SHUTDOWN` lint warns when a `Runtime` might be dropped without `shutdown()`.

## Specification Compliance

This implementation targets compliance with the SOME/IP specification.
Test coverage is tracked in `spec-data/coverage.json` with compliance tests in `tests/compliance/`.
We aim for 100% coverage of the open SOME/IP 2025-12 specs.

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

Contributions are welcome!

Make sure the following is fulfilled when filing a MR:
1. Write integration tests in `tests/`. Annotate if this is covering a spec.
2. Hack away.
3. Review test coverage of touched pieces of code.
