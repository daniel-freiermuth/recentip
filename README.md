# RecentIP

> Warning! This is an alpha stage hobby project for exploration.
The goal is to create a solid, easy-to-use and performant middleware implementation that is easily and fearlessly maintainable.

[![Crate](https://img.shields.io/crates/v/recentip.svg)](https://crates.io/crates/recentip)
[![Docs](https://docs.rs/recentip/badge.svg)](https://docs.rs/recentip)
[![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](LICENSE)

An opinionated **type-safe, async, lock-free, no-unsafe, no-panic, boring SOME/IP protocol implementation** backed by [tokio](https://tokio.rs).

SOME/IP (Scalable service-Oriented MiddlewarE over IP) is the standard middleware protocol for automotive Ethernet communication, enabling service-oriented communication between ECUs in modern vehicles.

## Features

- **Service Discovery** — Automatic discovery via multicast SD protocol
- **RPC** — Request/response and fire-and-forget method calls
- **Pub/Sub** — Event subscriptions with eventgroup management
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
           │                 │                     │
           ▼                 ▼                     ▼  
┌─────────────────────────────────────────────────────────────┐
│                     SomeIp (Event Loop)                     │
│   Multiplexes: commands, SD, RPC, TCP, timers               │
└─────────────────────────────────────────────────────────────┘
           │                              │
           ▼                              ▼
    ┌─────────────┐                ┌─────────────┐
    │ SD Socket   │                │ RPC Socket  │
    │ UDP:30490   │                │ UDP/TCP     │
    └─────────────┘                └─────────────┘
```

Handles (`OfferedService`, `ServiceOffering`) don't perform I/O directly—they send commands to the central event loop, which owns all sockets and state.

## Not yet implemented
- SOME/IP-TP
- Encryption
- De-/Serialization
- Fields, Getter, Setter
- Static services (without SD)
- Configuration handling

## SMIP integration
I haven't looked into this yet, but https://github.com/thoughtworks/smip looks
like it would be great on top of this lib. https://rocket.rs/ -like annotations
for SOME/IP.

## Other Rust Some/IP libs
- SummR https://github.com/eclipse-sommr seems dead or never kicked off?
- https://crates.io/crates/someip-rs fresh and new. Sync IO, no SD. Blocking
  on calls.

## Automotive Runtime Considerations

SOME/IP is designed for automotive ECUs where runtime predictability matters.
This section documents the library's behavior for systems with timing requirements.

### Safety & Certification

⚠️ **This library is NOT ASIL-qualified and NOT suitable for safety-critical functions.**
Use only for QM (non-safety) applications. For ASIL-rated functions, use a certified SOME/IP implementation.

### Runtime Characteristics

This library uses [Tokio](https://tokio.rs/), designed for high-throughput servers, not real-time systems.

| Aspect | Status | Notes |
|--------|--------|-------|
| Throughput | Untested | No benchmarks yet |
| Latency bounds | ❌ Unbounded | Tokio provides no worst-case guarantees |
| Jitter | ❌ Variable | Work-stealing scheduler, not deterministic |
| Heap allocation | ⚠️ Dynamic | Allocates during operation |
| Unsafe code | ✅ Forbidden | `#![forbid(unsafe_code)]` enforced |

**Implications for SOME/IP:** Message latency depends on system load. Cyclic SD announcements may jitter under load. For sub-millisecond latency requirements, Tokio is not suitable.

The library is written with runtime abstraction in mind and can be ported to other async runtimes with medium effort.

### Suitable Use Cases

- **Prototyping and simulation** — Fast iteration on service interfaces
- **Test environments** — Deterministic network simulation via turmoil
- **Non-safety-critical ECUs** — Infotainment, logging, diagnostics
- **Development tooling** — Service monitors, traffic analyzers
- **Linux-based gateways** — Central compute platforms

### Future Directions

For real-time or safety-critical deployments, the `net` module is abstracted and could support alternative runtimes like [Embassy](https://embassy.dev/). See [DESIGN.md](DESIGN.md) for architecture details. Contributions welcome.

## License

This project is licensed under the [GPL-3.0 License](LICENSE).

## Contributing

Contributions are welcome!

Make sure the following is fulfilled when filing a MR:
1. Write integration tests in `tests/`. Annotate if this is covering a spec.
2. Hack away.
3. Review test coverage of touched pieces of code.
