# RecentIP

> Warning! This is an alpha stage toy project for exploration.
The goal is to create a solid, easy-to-use and performant middleware implementation that is easily and fearlessly maintainable. One of the side-results is a comprehensive documentation.

[![Crate](https://img.shields.io/crates/v/recentip.svg)](https://crates.io/crates/recentip)
[![Docs](https://docs.rs/recentip/badge.svg)](https://docs.rs/recentip)
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

**📚 [See the full examples in the documentation](https://docs.rs/recentip/latest/recentip/examples/index.html)**

The documentation includes compile-checked examples covering:
- **[Quickstart](https://docs.rs/recentip/latest/recentip/examples/quickstart/)** — Minimal client, server, pub/sub
- **[RPC](https://docs.rs/recentip/latest/recentip/examples/rpc/)** — Request/response, fire-and-forget
- **[Pub/Sub](https://docs.rs/recentip/latest/recentip/examples/pubsub/)** — Events, eventgroups, subscriptions
- **[Transport](https://docs.rs/recentip/latest/recentip/examples/transport/)** — UDP, TCP, configuration
- **[Monitoring](https://docs.rs/recentip/latest/recentip/examples/monitoring/)** — Service discovery events

## Configuration

```rust
use recentip::prelude::*;

let someip = recentip::configure()
    .preferred_transport(Transport::Tcp)  // Prefer TCP when service offers both
    .magic_cookies(true)                  // Enable Magic Cookies for debugging
    .offer_ttl(3)                         // Service offer TTL in seconds
    .start().await?;
```

## API Overview

| Type | Role | Pattern |
|------|------|---------|  
| `SomeIp` | Central coordinator, owns sockets | — |
| `OfferedService` | Client proxy to remote service | — |
| `ServiceOffering` | Server handle for offered service | — |
| `SubscriptionBuilder` | Build subscription to eventgroups | Builder pattern |
| `Subscription` | Receive events from eventgroups | — |
| `Responder` | Reply to incoming RPC request | Consumed on reply |

## Identifier Types

| Type | Valid Range | Notes |
|------|-------------|-------|
| `ServiceId` | 0x0001–0xFFFE | 0x0000 and 0xFFFF reserved |
| `InstanceId` | 0x0001–0xFFFE or `Any` | 0xFFFF = wildcard |
| `MethodId` | 0x0000–0x7FFF | Bit 15 = 0 for methods |
| `EventId` | 0x8000–0xFFFE | Bit 15 = 1 for events |
| `EventgroupId` | 0x0001–0xFFFE | Groups related events |

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

The `RUNTIME_MUST_SHUTDOWN` lint warns when a `SomeIp` might be dropped without `shutdown()`.

## Specification Compliance

This implementation targets compliance with the SOME/IP specification.
Test coverage is tracked in `spec-data/coverage.json` with compliance tests in `tests/compliance/`.
We aim for 100% coverage of the open SOME/IP 2025-12 specs.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      User Application                       │
│   OfferedService       ServiceOffering        SomeIpBuilder │
└──────────┬─────────────────┬─────────────────────┬──────────┘
           │ Commands        │ Commands            │ configure()
           ▼                 ▼                     ▼  
┌─────────────────────────────────────────────────────────────┐
│                   SomeIp (Event Loop)                        │
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

## Not yet implemented
- SOME/IP-TP
- Encryption
- De-/Serialization
- Fields, Getter, Setter
- Static services (without SD)
- Configuration handling

## License

This project is licensed under the [GPL-3.0 License](LICENSE).

## Contributing

Contributions are welcome!

Make sure the following is fulfilled when filing a MR:
1. Write integration tests in `tests/`. Annotate if this is covering a spec.
2. Hack away.
3. Review test coverage of touched pieces of code.
