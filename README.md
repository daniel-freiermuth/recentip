# RecentIP

> Warning! This is an alpha stage hobby project for exploration.
The goal is to create a solid, easy-to-use and performant middleware implementation that is easily and fearlessly maintainable.

[![Crate](https://img.shields.io/crates/v/recentip.svg)](https://crates.io/crates/recentip)
[![Docs](https://docs.rs/recentip/badge.svg)](https://docs.rs/recentip)
[![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](LICENSE)

An opinionated **async and boring SOME/IP protocol implementation**.

[SOME/IP](https://some-ip.com/) (Scalable service-Oriented MiddlewarE over IP) is the standard middleware protocol for automotive Ethernet communication, enabling service-oriented communication between ECUs in modern vehicles.

## Features

- **Spec compliance testsuite and report**
- **Lint rule for proper usage**
- **Zero-panic** expect, unwrap, indexing forbidden by clippy rule
- **Few mutexes** Most data is passed using channels for uninterrupted non-blocking flows
- **Tokio-backed async** scales from single to multicore execution
- **No-unsafe** forbidden by clippy rule

## Supported SOME/IP
Right now, this lib implements these core parts of the SOME/IP protocol:
- **Service Discovery** — Automatic discovery via multicast SD protocol
- **RPC** — Request/response and fire-and-forget method calls
- **Pub/Sub** — Event subscriptions with eventgroup management

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
recentip = "0.1"
```

## Documentation

**[API Documentation](https://docs.rs/recentip)** — Configuration, API overview, and compile-checked examples

- [Quickstart examples](https://docs.rs/recentip/latest/recentip/examples/quickstart/)
- [All examples](https://docs.rs/recentip/latest/recentip/examples/index.html)

## Testing

The library heavily leverages [turmoil](https://docs.rs/turmoil) for deterministic and fast network and time simulation. There are several test suites:

- Unit tests in each module
- Compliance tests in `tests/compliance` certifying compliance with the SOME/IP specs backed.
- Real network tests not using turmoil.
- API behavior tests for non-spec behavior.

```bash
# Run most tests (~10s)
cargo nextest run

# Run all tests (~10s)
cargo nextest run --cargo-profile fast-release --features slow-tests

# Generate coverage report
cargo llvm-cov nextest --features slow-tests

# Doc tests
cargo test --doc
```

## RecentIP Lints

RecentIP comes with a lint rule for detecting wrong usage of the it that cannot be enforced by types.

- The `RUNTIME_MUST_SHUTDOWN` lint warns when a `SomeIp` might be dropped without `shutdown()`.

Use the included [dylint](https://github.com/trailofbits/dylint) lint crate:

```bash
# Install dylint
cargo install cargo-dylint dylint-link

# Run lints
cargo dylint --all
```

## Specification Compliance

This implementation targets full compliance with the [open SOME/IP 2025-12 specification](https://github.com/some-ip-com/open-someip-spec/commit/dcdfbd8f772ebfa973317e3cd4580874d848ae7e). Support per requirement can be verified using the [traceability report](https://docs.rs/recentip/0.1.0-alpha.4/recentip/compliance/traceability/index.html). 

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      User Application                       │
│ SomeIp-Handle     OfferedService          ServiceOffering   │
└────┬────────────────────┬─────────────────────┬─────────────┘
     │                    │▲ Events             │▲ Events
     │           Commands ▼│           Commands ▼│
┌─────────────────────────────────────────────────────────────┐
│                     SomeIp (Event Loop)                     │
│   Multiplexes: commands, SD, RPC, TCP, timers               │
└─────────────────────────────────────────────────────────────┘
           │                              │
           ▼                              ▼
    ┌─────────────┐                ┌─────────────────────────┐
    │ SD Socket   │                │ Pub/Sub and RPC Sockets │
    │ UDP:30490   │                │ UDP/TCP                 │
    └─────────────┘                └─────────────────────────┘
```

Handles (`OfferedService`, `ServiceOffering`) don't perform I/O directly—they send commands to the central event loop, which owns all sockets and state.

## Not yet implemented
- SOME/IP-TP
- Encryption
- De-/Serialization
- Fields, Getter, Setter
- Configuration handling

## License

This project is licensed under the [GPL-3.0 License](LICENSE).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).
