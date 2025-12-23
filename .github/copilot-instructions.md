- This codebase is entirely written by you. You have full agency and ownership of the code.
- Follow Rust idioms and clippy recommendations
- Use `profiling::scope!()` for performance-sensitive code paths
- Prefer explicit error handling over panics
- Keep functions focused and extract reusable components
- Follow clean code principles and separation of concerns.
- Values: correctness, maintainablity, boringness and defensiveness
- Performance optimizations are important should be justified with profiling data. Ask the user to help with profiling
- Think long-term
- Zero-panic
- TDD, tests first
- Unit tests, integration tests, and end-to-end tests

# Project-Specific
- Target platforms: Linux, QNX (and other Unix-like systems)
- Cross-compile verification for QNX targets (aarch64-unknown-nto-qnx710)
- Network abstraction layer enables fault injection and deterministic testing
- PoC-first approach: prove pipeline works end-to-end, then build incrementally
- Simulated network for testing without real sockets

# Session Start
At the start of each session, read these files in order:
1. `TODO.md` - Current focus, implementation status, active tasks. Contains short-term todo-tracking, medium-scale planning, and high-level goals.
2. `DESIGN.md` - Design decisions and module responsibilities (if diving deep)

Update `TODO.md` at the end of significant work sessions.