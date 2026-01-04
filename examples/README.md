# SOME/IP Runtime Examples

This directory contains example programs demonstrating the `someip-runtime` API.

## Available Examples

### 1. Service Monitor (`service_monitor.rs`)

A tool that listens for SOME/IP Service Discovery announcements and reports when services become available on the network.

**Features:**
- Monitors multiple service IDs simultaneously
- Displays service ID, instance, endpoint, and timestamp
- Runs until Ctrl+C

**Usage:**
```bash
cargo run --example service_monitor
```

**Output:**
```text
╔═══════════════════════════════════════════════════════╗
║       SOME/IP Service Discovery Monitor v0.1          ║
╚═══════════════════════════════════════════════════════╝

Monitoring network for SOME/IP service announcements...
Press Ctrl+C to exit.

Listening for services:
  - 0x1111, 0x1234, 0x2222, 0x5678, 0xFFFF

Timestamp                 Service         Instance   Endpoint                      
--------------------------------------------------------------------------------
[2026-01-04 12:00:00] ✓ Service 0x1234 instance 1 AVAILABLE at 127.0.0.1:30501
```

### 2. Simple Service (`simple_service.rs`)

A basic SOME/IP service that announces itself via Service Discovery and handles incoming requests.

**Features:**
- Announces service 0x1234 instance 1
- Echoes back method call payloads
- Accepts subscription requests
- Logs all incoming events

**Usage:**
```bash
cargo run --example simple_service
```

**Output:**
```text
╔═══════════════════════════════════════════════════════╗
║         SOME/IP Simple Service Example                ║
╚═══════════════════════════════════════════════════════╝

Starting service 0x1234 instance 1...
✓ Service is now being announced via Service Discovery
  Press Ctrl+C to stop

← Received method call 0x0001 with 10 bytes
→ Sent response
```

## Testing the Examples Together

The best way to test these examples is to run them in separate terminals:

**Terminal 1 - Start the monitor:**
```bash
cargo run --example service_monitor
```

**Terminal 2 - Start the service:**
```bash
cargo run --example simple_service
```

You should see the monitor detect the service within a few seconds:
```text
[2026-01-04 12:00:05] ✓ Service 0x1234 instance 1 AVAILABLE at 127.0.0.1:30501
```

## Customizing the Examples

### Adding More Services to Monitor

Edit `service_monitor.rs` and add new service type definitions:

```rust
struct MyCustomService;
impl Service for MyCustomService {
    const SERVICE_ID: u16 = 0xABCD;
    const MAJOR_VERSION: u8 = 1;
    const MINOR_VERSION: u32 = 0;
}

// Then add to the monitoring macro:
spawn_monitor!(MyCustomService);
```

### Changing Service Configuration

Edit `simple_service.rs` to change the service ID or instance:

```rust
struct ExampleService;
impl Service for ExampleService {
    const SERVICE_ID: u16 = 0x5555;  // Change this
    const MAJOR_VERSION: u8 = 2;
    const MINOR_VERSION: u32 = 1;
}

// And change the instance when offering:
let mut offering = runtime.offer::<ExampleService>(InstanceId::Id(42)).await?;
```

## Network Configuration

By default, both examples use the standard SOME/IP Service Discovery multicast:
- Multicast address: `224.224.224.245:30490`
- Local binding: `0.0.0.0:30490`

To use different network settings, modify the `RuntimeConfig`:

```rust
let config = RuntimeConfigBuilder::default()
    .local_addr("192.168.1.100:30490".parse().unwrap())
    .sd_multicast("224.224.224.245:30490".parse().unwrap())
    .build();

let runtime = Runtime::new(config).await?;
```

## Troubleshooting

### Monitor doesn't detect services

1. **Check firewall settings** - UDP port 30490 must be open for multicast
2. **Verify network interface** - Multicast must be enabled on your network interface
3. **Same subnet** - Both programs must be on the same network segment
4. **Run with logging** - Enable debug logging to see SD messages:
   ```rust
   tracing_subscriber::fmt()
       .with_max_level(tracing::Level::DEBUG)
       .init();
   ```

### Services on different machines

If running on different machines, ensure:
- Multicast routing is enabled
- No firewall blocking UDP 30490
- Both machines are on the same subnet or have multicast routing configured

### Permission errors on Linux

You may need elevated privileges for multicast on some systems:
```bash
sudo cargo run --example service_monitor
```

Or configure your system to allow multicast for regular users.
