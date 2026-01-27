# Internals

Documentation for contributors and maintainers.

## Module Structure

| Module | Visibility | Responsibility |
|--------|------------|----------------|
| `runtime` | Internal | Event loop, socket management, state machine |
| `runtime::state` | Internal | `RuntimeState`, `ServiceKey`, internal data structures |
| `runtime::sd` | Internal | Service Discovery message handlers |
| `runtime::command` | Internal | Command enum for handle→runtime communication |
| [`handles`](crate::handles) | Public | User-facing API types |
| [`config`](crate::config) | Public | Configuration types |
| [`error`](crate::error) | Public | Error types |
| [`wire`](crate::wire) | Public | Wire format parsing |
| [`tcp`](crate::tcp) | Public | TCP framing and connection pooling |

## Event Loop Architecture

The [`SomeIp`](crate::SomeIp) runtime is a single-task event loop using `tokio::select!`:

```text
loop {
    select! {
        cmd = command_rx.recv() => { /* handle user commands */ }
        msg = sd_socket.recv() => { /* handle SD messages */ }
        msg = rpc_socket.recv() => { /* handle RPC messages */ }
        _ = timer.tick() => { /* cyclic offers, TTL expiry */ }
        // ... TCP connections
    }
}
```

All state lives in `RuntimeState`. Handles send `Command` messages through a channel;
the event loop processes them atomically. This design:

- Eliminates data races (single owner of all state)
- Enables efficient socket multiplexing
- Simplifies testing (deterministic message ordering with turmoil)

## Session ID Management

Per SOME/IP spec, session IDs:

- Are 16-bit counters wrapping from 0xFFFF → 0x0001 (never 0x0000)
- Have **separate counters** for multicast vs unicast SD messages
- Use a "reboot flag" (bit in SD header) to signal restart

Tracked in `RuntimeState`:
- `multicast_session_id: u16`
- `unicast_session_id: u16` (TODO: should be per-peer `HashMap<IpAddr, u16>`)
- `reboot_flag: bool` (set on first message after boot)

## Service Discovery Flow

### Server Side (Offering)

1. User calls `runtime.offer(service_id, instance_id).start()`
2. Command sent to event loop: `Command::Offer { ... }`
3. Event loop creates `OfferedService` in `RuntimeState.offered`
4. Timer triggers periodic `OfferService` SD messages (multicast)
5. Incoming `SubscribeEventgroup` → create subscription, send ACK

### Client Side (Finding)

1. User calls `runtime.find(service_id).await`
2. Command sent: `Command::Find { ... }`
3. Event loop sends `FindService` SD message (multicast)
4. Incoming `OfferService` → create `DiscoveredService`, resolve find future
5. User gets `OfferedService` handle for RPC/subscribe

## Testing Strategy

- **Turmoil**: Deterministic network simulation for most tests
- **Real network tests**: Marked with `#[ignore]` or feature-gated
- **Compliance tests**: In `tests/compliance/`, organized by spec area
- **Doc tests**: All examples compile-checked via `cargo test --doc`

## Code Conventions

- `#![forbid(unsafe_code)]` — no unsafe anywhere
- Prefer explicit error handling over panics (goal: zero-panic)
- Use `profiling::scope!()` for performance-sensitive paths
- Internal modules are `pub(crate)`, documented for contributors
