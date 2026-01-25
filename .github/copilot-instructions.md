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

Update `TODO.md` at the end of every work sessions. I.e. update completed tasks, make sure the next tasks are clear.


# Knowledge base
In the folder .notes-to-the-agent, you can keep notes to yourself. It would, e.g. be a good idea to note down the results of expensive researchs or solutions to problems you encountered. Also you should make yourself familiar with the contents of these files.

# TODO file
Some more instructions regarding the TODO.md file:
- This file is for keeping track of tasks to todo, completed tasks should be removed.
- It shall contain tasks of varying granularity, from small (e.g. "Implement feature X") to large (e.g. "Design module Y architecture"). At least the levels: next steps, tasks, epics and final goals should be present. They could also be interlinked or organized hierarchically. Make sure, that there are at least 5 next steps, 5 tasks, 3 epics and 1 final goal at any time.
- Additionally, there might be a section for todos, that are not (yet) put in the organazition yet.
- Whenever you context-switch or find a task that you do not complete right away, make sure to write it down in the TODO.md file.