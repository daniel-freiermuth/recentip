// Test: creating a Runtime without calling shutdown() should trigger a warning.

struct Runtime;

impl Runtime {
    fn new() -> Self {
        Runtime
    }

    fn shutdown(self) {}
}

fn main() {
    let runtime = Runtime::new();
    let _ = &runtime;
}
