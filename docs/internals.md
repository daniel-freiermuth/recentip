# Internals

Documentation for contributors and maintainers.

```text
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
        // User commands from handles (find, offer, call, subscribe, etc.)
        cmd = command_rx.recv() => { /* dispatch command */ }
        
        // Service Discovery (multicast UDP:30490)
        msg = sd_socket.recv() => { /* handle SD messages */ }
        
        // RPC over UDP (unicast)
        msg = udp_rpc_socket.recv() => {
            // Request → route to ServiceOffering handle
            // Response → match session ID, complete pending call future
            // Notification → route to Subscription handle
        }
        
        // TCP listener for incoming connections
        conn = tcp_listener.accept() => {
            // Add to connection pool, spawn reader task
        }
        
        // TCP connection data (for each active connection)
        msg = tcp_connections.next() => {
            // Same as UDP: Request/Response/Notification
            // Plus: connection close handling, reconnect logic
        }
        
        // Timers
        _ = offer_timer.tick() => {
            // Send cyclic OfferService for each offered service
        }
        _ = ttl_timer.tick() => {
            // Expire stale discovered services
            // Expire stale subscriptions
        }
        _ = find_timer.tick() => {
            // Retry pending FindService requests
        }
    }
}
```

All state lives in `RuntimeState`. Handles send `Command` messages through a channel;
the event loop processes them atomically. This design:

- Eliminates data races (single owner of all state)
- Enables efficient socket multiplexing

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

## Code Conventions

- Clippy is configured to prevent unsafe code and code, that could panic.
- Use `profiling::scope!()` for performance-sensitive paths
- Internal modules are `pub(crate)`, documented for contributors
