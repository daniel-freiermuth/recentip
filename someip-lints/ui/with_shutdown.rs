// Test: creating a Runtime and calling shutdown() should NOT trigger a warning.

struct Runtime;

impl Runtime {
    fn new() -> Self {
        Runtime
    }

    fn shutdown(self) {}
}

fn main() {
    let runtime = Runtime::new();
    runtime.shutdown();
}
