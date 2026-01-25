# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**RecentIP** is a type-safe, async, lock-free SOME/IP protocol implementation in Rust backed by Tokio. SOME/IP (Scalable service-Oriented MiddlewarE over IP) is the standard middleware protocol for automotive Ethernet communication.

Key characteristics:
- **Alpha stage** exploration project
- Goal: solid, easy-to-use, performant, and fearlessly maintainable implementation
- Values: correctness, maintainability, boringness, and defensiveness
- **Zero-panic** policy
- **No unsafe code** (`#![forbid(unsafe_code)]`)
- TDD approach - tests first

## Essential Commands

### Building and Testing

```bash
# Run most tests (~10s) - uses nextest (required)
cargo nextest run

# Run all tests including slow tests (~40s)
cargo nextest run --features slow-tests

# Run with code coverage
cargo llvm-cov nextest --features slow-tests

# Doc tests only
cargo test --doc

# Build documentation (requires Python for spec extraction)
python3 tools/extract_requirements.py
python3 scripts/extract_coverage.py
cargo doc --no-deps
```

### Code Quality

```bash
# Format check
cargo fmt --check

# Apply formatting
cargo fmt

# Clippy lints
cargo clippy --lib --all-targets --all-features

# Custom dylint lints (requires installation first)
cargo install cargo-dylint dylint-link
cargo dylint --all
```

The `RUNTIME_MUST_SHUTDOWN` custom lint warns when a `SomeIp` might be dropped without `shutdown()`.

### Running Single Tests

```bash
# Run a specific test by name
cargo nextest run test_name

# Run tests matching a pattern
cargo nextest run 'test(real_network::)'

# Run ignored tests (mostly stubs or very slow tests)
cargo nextest run -- --ignored
```

### Cross-Compilation

```bash
# Check QNX target compatibility
rustup target add aarch64-unknown-nto-qnx710
cargo check --target aarch64-unknown-nto-qnx710 --no-default-features
```

## Architecture Overview

### Core Design Pattern: Central Event Loop

RecentIP uses a **single-threaded event loop architecture** where:
- The `SomeIp` runtime owns all state in one place (`RuntimeState`)
- User-facing handles (`OfferedService`, `ServiceOffering`) send commands to the runtime
- The runtime processes commands and I/O events in a `tokio::select!` loop
- No shared mutable state between components - eliminates data races

```
User Application
  OfferedService, ServiceOffering, SomeIpBuilder
           │ Commands
           ▼
    SomeIp Event Loop
      RuntimeState (single source of truth)
      select! over: commands, SD socket, RPC socket, TCP, timer
           │
           ▼
    Network I/O (SD Socket UDP:30490, RPC Socket UDP/TCP)
```

### Module Organization

| Module | Purpose |
|--------|---------|
| `runtime/` | Internal event loop, socket management, `SomeIp` struct |
| `runtime/command.rs` | Commands from handles to runtime |
| `runtime/state.rs` | `RuntimeState` and internal data structures |
| `runtime/client.rs` | Client-side handlers (find, call, subscribe) |
| `runtime/server.rs` | Server-side handlers (offer, notify, respond) |
| `runtime/sd.rs` | Service Discovery message handlers |
| `runtime/event_loop.rs` | Main event loop executor |
| `handles/` | Public API: `OfferedService`, `ServiceOffering` |
| `wire/` | Wire format parsing (headers, SD messages) |
| `tcp.rs` | TCP framing, connection pooling, Magic Cookies |
| `net/` | Network abstraction (tokio_impl, turmoil_impl) |
| `config.rs` | Configuration types |
| `error.rs` | Error types |

### Key Design Decisions

1. **Type-state patterns** enforce correct usage at compile time (e.g., can't subscribe before service is discovered)
2. **Pull-based API** (`next()`) instead of callbacks for simpler reasoning and better FFI compatibility
3. **Subscription is the event source** - you receive events through your `Subscription` handle, drop semantics send `StopSubscribe`
4. **Deterministic testing** via `turmoil` for network simulation (most tests don't need real sockets)

## Testing Infrastructure

### Test Organization

Tests are organized by purpose in `tests/compliance/`:

| Directory | Purpose |
|-----------|---------|
| `wire_format/` | Wire format parsing tests |
| `client_behavior/` | Client-side behavior tests |
| `server_behavior/` | Server-side behavior tests |
| Root `tests/compliance/` | Protocol compliance tests (RPC, pub/sub, SD, etc.) |

### Nextest Configuration

The project uses nextest with custom configuration (`.config/nextest.toml`):
- **Real network tests run serially** (test-group: `real-network`, max-threads: 1)
- Most tests use turmoil for deterministic network simulation
- Filter real network tests with: `test(real_network::)`

### Coverage and Compliance

The project tracks **specification compliance** via:
1. `covers!()` macro in tests annotates which spec requirements are tested
2. Build script (`build.rs`) runs Python scripts to extract coverage
3. Requirements extracted from `specs/` submodule (SOME/IP spec documents)
4. `spec-data/coverage.json` tracks requirement → test mapping
5. Goal: **100% requirement coverage** and **100% code coverage**

Running compliance reports:
```bash
python3 tools/extract_requirements.py    # Extract requirements from specs
python3 scripts/extract_coverage.py      # Extract covers!() from tests
python3 scripts/generate_compliance.py   # Generate compliance report
```

### Turmoil Network Simulation

Most tests use [turmoil](https://docs.rs/turmoil) for deterministic network simulation:
- Simulates UDP/TCP without real sockets
- Deterministic packet delivery (controllable delays, drops)
- No port conflicts between test runs
- Faster and more reliable than real network tests

See `.notes-to-the-agent/turmoil.md` for details on turmoil usage patterns.

## Session Start Routine

When starting a new session, read these files in order:
1. **`TODO.md`** - Current focus, implementation status, active tasks (short/medium-term planning)
2. **`DESIGN.md`** - Design decisions and module responsibilities (when diving deep)
3. **`.notes-to-the-agent/`** - Agent's knowledge base (expensive research results, solutions to problems)

**Update `TODO.md` at the end of work sessions** with completed tasks and clear next steps.

## Project-Specific Practices

### Code Values
- **Correctness over cleverness** - boring solutions preferred
- **Defensiveness** - validate inputs, handle errors explicitly
- **Long-term thinking** - this is production-quality code
- **TDD** - write tests first
- **Performance optimizations require profiling data** - ask user to help with profiling

### Target Platforms
- **Primary**: Linux (full async/tokio)
- **Secondary**: QNX (sync-first, limited async ecosystem)
- **Future**: Embedded/RTOS (no_std core is architecturally possible)

### Important Protocol Details

1. **Service Discovery (SD)** uses UDP port 30490 (reserved, app traffic must not use it)
2. **Session IDs**:
   - 16-bit, wrap from 0xFFFF → 0x0001 (never 0x0000)
   - Separate counters for: multicast SD, per-peer unicast SD, RPC calls
   - Reboot flag (bit 7) signals peer restart
3. **Endpoints**: Service instances use unique ports; different services can share ports
4. **Events**: SHALL NOT be sent without active subscriptions (enforced by type system)

### Current Implementation Gaps (from TODO.md)

Major missing features:
- **SOME/IP-TP** (segmentation/reassembly) - parsing exists but runtime not implemented
- **Session ID per-relation tracking** - currently one counter, spec requires multicast + per-peer
- **Reboot detection case 2** - only detects flag 0→1, missing session regression detection
- **Service expiry on peer reboot** - detection exists but doesn't expire services/subscriptions

See `TODO.md` for full details and test status.

## Important Files

- **`Cargo.toml`** - Features: `turmoil` (default), `slow-tests`
- **`build.rs`** - Generates compliance docs for rustdoc from spec-data
- **`.gitlab-ci.yml`** - CI pipeline (check, test, build stages)
- **`.config/nextest.toml`** - Test execution configuration
- **`src/lib.rs`** - Entry point, exports, extensive internal documentation
- **`DESIGN.md`** - Deep design rationale and protocol analysis
- **`TODO.md`** - Active work tracking and test status
- **`specs/`** - Git submodule with SOME/IP specification PDFs

## Dependencies and Ecosystem

Key dependencies:
- **tokio** - async runtime (net, sync, time, macros)
- **turmoil** - deterministic network simulation for tests
- **bytes** - zero-copy buffer management
- **socket2** - low-level socket options
- **proptest** - property-based testing (dev)

## Lints and Code Standards

Cargo lints configured in `Cargo.toml`:
- `unsafe_code = "forbid"` - NO unsafe code allowed
- Clippy: `all`, `pedantic`, `nursery`, `cargo` groups enabled
- `panic = "deny"` - no explicit panics
- `todo = "deny"` - no TODO markers in code
- `unwrap_used` and `expect_used` are currently allowed but being eliminated

## Common Pitfalls

1. **Don't use real network tests unless necessary** - prefer turmoil simulation
2. **Session ID management is per-relation** - one counter per communication relation (not global)
3. **Port 30490 is SD-only** - application traffic must not use it
4. **Tests must use nextest** - plain `cargo test` doesn't respect serial test configuration
5. **Spec compliance requires covers!() annotations** - new tests should annotate which requirements they cover
6. **The Python scripts need the specs submodule** - run `git submodule update --init` if needed

## Naming Conventions

- Service Discovery: **SD**
- Request/Response: **RPC**
- Publish/Subscribe: **Pub/Sub**
- Transport Protocol (segmentation): **TP**
- Event groups: **eventgroups** (one word)
