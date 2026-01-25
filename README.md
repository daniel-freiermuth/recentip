# RecentIP

> Warning! This is an alpha stage hobby project for exploration.
The goal is to create a solid, easy-to-use and performant middleware implementation that is easily and fearlessly maintainable.

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

## Automotive Runtime Considerations

SOME/IP is designed for automotive ECUs where runtime predictability matters.
This section documents the library's behavior for systems with timing requirements.

### Safety & Certification

⚠️ **This library is NOT ASIL-qualified and NOT suitable for safety-critical functions.**

- No formal verification or safety certification
- Not developed according to ISO 26262 processes
- Use only for QM (non-safety) applications
- For ASIL-rated functions, use a certified SOME/IP implementation

### Performance & Latency

| Aspect | Status | Notes |
|--------|--------|-------|
| Throughput | Untested | No benchmarks yet; contributions welcome |
| Latency bounds | ❌ Unbounded | Tokio provides no worst-case guarantees |
| Jitter | ❌ Variable | Work-stealing scheduler, GC-free but not deterministic |
| Zero-copy | ❌ Not yet | Message parsing allocates; zero-copy design possible |

### Tokio Runtime Behavior

This library uses [Tokio](https://tokio.rs/) as its async runtime. Key characteristics
relevant to timing (see [Tokio runtime docs](https://docs.rs/tokio/latest/tokio/runtime/index.html)):

**Bounded delay guarantee** (from Tokio docs):

> Under the following two assumptions:
> - There is some number `MAX_TASKS` such that the total number of tasks never exceeds `MAX_TASKS`.
> - There is some number `MAX_SCHEDULE` such that calling `poll` on any task returns within `MAX_SCHEDULE` time units.
>
> Then, there is some number `MAX_DELAY` such that when a task is woken, it will be scheduled within `MAX_DELAY` time units.

**Additional runtime characteristics:**
- Tasks may be scheduled in any order; no priority support
- A task may be scheduled 5× before another ready task runs
- Work-stealing between threads adds non-deterministic delays
- IO/timer checks occur every ~61 scheduled tasks (configurable via `event_interval`)
- Global queue checked every ~31 local tasks (configurable via `global_queue_interval`)
- **Not NUMA-aware** — consider multiple runtimes on NUMA systems

**What this means for SOME/IP:**
- Message latency depends on system load and task count
- No guarantee that a high-priority service response arrives before low-priority work
- Cyclic SD announcements may jitter under load
- For sub-millisecond latency requirements, Tokio is not suitable

### Async Runtime Abstraction Status

While recentIP is right now using Tokio, it was written with other async runtimes and mind and can be ported or made agnostic with medium effort.

### Current Limitations

This library is **not suitable for hard real-time** applications:

| Concern | Status | Notes |
|---------|--------|-------|
| Heap allocation | ⚠️ Dynamic | Allocates during operation (buffers, connections) |
| Async runtime | ⚠️ Tokio | Work-stealing scheduler, not deterministic |
| Blocking | ⚠️ Possible | Async mutex in TCP connection pool |
| Panic paths | ⚠️ Minimal | 3 unwraps in runtime code (will be eliminated) |
| Unsafe code | ✅ Forbidden | `#![forbid(unsafe_code)]` enforced |
| Priority inversion | ⚠️ Possible | No priority-aware scheduling |

### Suitable Use Cases

- **Prototyping and simulation** — Fast iteration on service interfaces
- **Test environments** — Deterministic network simulation via turmoil
- **Non-safety-critical ECUs** — Infotainment, logging, diagnostics
- **Development tooling** — Service monitors, traffic analyzers
- **Linux-based gateways** — Central compute platforms

### ASIL Compliance Considerations

This library uses async Rust for efficient I/O multiplexing — waiting on multiple sockets, timers, and channels without threads or manual state machines. Async itself compiles to state machines, but the Tokio **runtime** is probably not ASIL-certifiable. However, recentIP is well-suited for **QM (Quality Management)** domains, which covers the majority of SOME/IP deployments: infotainment, telematics, diagnostics, logging, and service orchestration.

### Future Directions

For safety-critical or hard real-time deployments, consider:

1. **Alternative async runtimes** — The `net` module is abstracted; could support [embassy](https://embassy.dev/) for embedded
2. **Pre-allocation** — Buffer pools, bounded collections could be added
3. **no_std support** — Currently requires std; a no_std core is architecturally possible
4. **Static configuration** — Compile-time service definitions to eliminate runtime allocation

Contributions toward these goals are welcome. See [DESIGN.md](DESIGN.md) for architecture details.

### Relation to Real-Time Rust Ecosystems

This library uses **Tokio**, which is designed for high-throughput servers, not real-time systems.
For hard real-time requirements, consider these Rust ecosystems:

#### Pure Rust Runtimes

| Project | Model | Notes |
|---------|-------|-------|
| [RTIC](https://rtic.rs/) | Interrupt-driven, priority-based | Ideal for bare-metal MCUs with hardware priorities |
| [Embassy](https://embassy.dev/) | Async, `no_std`, embedded | Could be a backend for this library's `net` abstraction |
| [Drone OS](https://www.drone-os.com/) | Async, preemptive threads | RTOS with async/await, Cortex-M focus |
| [Hubris](https://hubris.oxide.computer/) | IPC-based microkernel | Oxide's secure embedded OS |
| [Tock](https://www.tockos.org/) | Process isolation, embedded | Security-focused embedded OS |

#### Traditional RTOSes with Rust Bindings

| Project | Rust Support | Notes |
|---------|--------------|-------|
| [FreeRTOS](https://www.freertos.org/) | [`freertos-rust`](https://crates.io/crates/freertos-rust) | Industry standard, 40+ architectures, AWS-maintained |
| [RIOT](https://riot-os.org/) | [`riot-wrappers`](https://crates.io/crates/riot-wrappers) | IoT-focused, threading, network stacks, 8/16/32-bit |

#### Automotive-Focused Rust Frameworks

| Project | Model | Notes |
|---------|-------|-------|
| [OxidOS](https://oxidos.io/) | Tock-based, sandboxed apps | Automotive ASIL targeting, RISC-V ready, ST Stellar MCUs |
| [Veecle OS](https://github.com/veecle/veecle-os) | Actor-based, OSAL abstraction | Supports std/Embassy/FreeRTOS backends; SOME/IP support built-in based on vsomeip |
| [veecle-pxros](https://github.com/veecle/veecle-pxros) | PXROS-HR runtime | AURIX platform, ASIL-D certified kernel, Infineon/HighTec ecosystem |

#### Robotics Middleware

| Project | Rust Support | Notes |
|---------|--------------|-------|
| [ROS 2](https://docs.ros.org/) | [`ros2_rust`](https://github.com/ros2-rust/ros2_rust) | Pub/sub, services, zero-copy; used in ADAS/autonomous driving |

#### Automotive Linux Distributions

| Project | Rust Support | Notes |
|---------|--------------|-------|
| [Red Hat In-Vehicle OS](https://www.redhat.com/en/solutions/automotive) | Full Rust toolchain | RHEL-based, ASIL-B (ISO 26262); VW, Audi |
| [Automotive Grade Linux](https://www.automotivelinux.org/) | Full Rust toolchain | Linux Foundation; Toyota, Honda, Mazda, Subaru, Suzuki, Mercedes-Benz |

**Potential integration paths:**

- **RTIC/Embassy**: Port the wire-format parsing ([`wire`](src/wire.rs)) and state machine logic; replace Tokio I/O with platform-specific drivers
- **Veecle OS**: Already has SOME/IP support via `veecle-os-data-support-someip`; could potentially share wire-format code
- **FreeRTOS/RIOT**: Use their Rust bindings for threading and networking, port our state machine logic
- **ROS 2**: Bridge SOME/IP services to ROS 2 topics/services for ADAS integration
- **Hypervisor approach**: Run this library in a Linux VM/container alongside an RTOS partition for non-safety workloads
- **Shared memory**: Use this library on a Linux core, communicate with RTIC/Zephyr via shared memory for safety-critical functions

Currently, no Rust SOME/IP implementation targets `no_std` or real-time runtimes. This represents an opportunity for the ecosystem.

## LoC
Implementation + unit tests
```
❯ tokei src/
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Language              Files        Lines         Code     Comments       Blanks
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Rust                     28         9209         7428          683         1098
 |- Markdown              28         3210           25         2515          670
 (Total)                            12419         7453         3198         1768
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Total                    28        12419         7453         3198         1768
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

Integration tests
```
❯ tokei tests/
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Language              Files        Lines         Code     Comments       Blanks
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Rust                     46        41453        31391         3903         6159
 |- Markdown              44         2017            0         1619          398
 (Total)                            43470        31391         5522         6557
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Total                    46        43470        31391         5522         6557
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

vsomeip as comparison
```
❯ tokei vsomeip/implementation/
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Language              Files        Lines         Code     Comments       Blanks
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Autoconf                  1          196           23          117           56
 C++                     138        46169        36852         2496         6821
 C++ Header              189        13799         8802         1341         3656
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Total                   328        60164        45677         3954        10533
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

```
❯ tokei vsomeip/test/
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Language              Files        Lines         Code     Comments       Blanks
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Autoconf                321        18553        16231         1430          892
 CMake                    59         3821         2549          703          569
 C++                     175        39092        29506         3306         6280
 C++ Header              104         6735         3861         1607         1267
 JSON                     12         4654         4654            0            0
 PlantUML                 38         1696         1083            0          613
 Python                    3          131           95            8           28
 Plain Text                3          749            0          610          139
─────────────────────────────────────────────────────────────────────────────────
 Markdown                 28         1419            0          982          437
 |- BASH                   1           15           15            0            0
 |- C++                    1           43           21           14            8
 (Total)                             1477           36          996          445
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Total                   743        76908        58015         8660        10233
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## License

This project is licensed under the [GPL-3.0 License](LICENSE).

## Contributing

Contributions are welcome!

Make sure the following is fulfilled when filing a MR:
1. Write integration tests in `tests/`. Annotate if this is covering a spec.
2. Hack away.
3. Review test coverage of touched pieces of code.
