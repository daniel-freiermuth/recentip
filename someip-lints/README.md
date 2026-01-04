# someip-lints

Custom Clippy-style lints for the `someip-runtime` library.

## Available Lints

### `runtime_shutdown_required`

**Default level:** `warn`

Checks for `someip_runtime::Runtime` variables that are dropped without
explicitly calling `.shutdown().await`.

#### Why is this bad?

Dropping a `Runtime` without calling `shutdown()` may cause pending RPC
responses to be lost. The runtime task may be terminated before outgoing
messages have been sent.

#### Example

Bad:
```rust
async fn bad_example() {
    let runtime = Runtime::new(config).await?;
    let proxy = runtime.find::<MyService>().await?;
    // BAD: runtime dropped without shutdown()
}
```

Good:
```rust
async fn good_example() {
    let runtime = Runtime::new(config).await?;
    let proxy = runtime.find::<MyService>().await?;
    runtime.shutdown().await;  // GOOD: explicit shutdown
}
```

## Installation

These lints use [dylint](https://github.com/trailofbits/dylint), a tool for
running Rust lints from dynamic libraries.

### 1. Install dylint

```bash
cargo install cargo-dylint dylint-link
```

### 2. Build the lint library

```bash
cd someip-lints
cargo build --release
```

### 3. Configure your project

Add to your project's `.dylint.toml`:

```toml
[workspace.metadata.dylint]
libraries = [
    { path = "someip-lints" },
]
```

Or in `Cargo.toml`:

```toml
[workspace.metadata.dylint]
libraries = [
    { path = "someip-lints" },
]
```

### 4. Run the lints

```bash
cargo dylint runtime_shutdown_required
```

Or run all lints:

```bash
cargo dylint --all
```

## Development

### Prerequisites

This crate requires the nightly Rust toolchain with `rustc-dev` component:

```bash
rustup component add rustc-dev llvm-tools-preview --toolchain nightly
```

### Building

```bash
cargo build
```

### Testing

```bash
cargo test
```

The UI tests are in the `ui/` directory. Each test file has a corresponding
`.stderr` file with expected lint output.

### Updating test expectations

After modifying the lint behavior:

```bash
cargo test --bless
```

## How It Works

The lint uses rustc's Late Lint Pass system to analyze the HIR (High-level
Intermediate Representation) of Rust code:

1. **Detection Phase**: The lint visitor scans function bodies for `let`
   bindings where the type is `Runtime`.

2. **Tracking Phase**: For each `Runtime` binding, the visitor tracks whether
   `.shutdown()` is called on that variable.

3. **Reporting Phase**: At the end of each function, variables without
   shutdown calls trigger a warning.

### Limitations

- Cannot track ownership through function calls (if you pass the runtime to
  another function that shuts it down)
- May not detect shutdown() called in complex control flow
- Uses heuristic type matching which may have false positives/negatives

## License

Same license as the parent `someip-runtime` crate.
