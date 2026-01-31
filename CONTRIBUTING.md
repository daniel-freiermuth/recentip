# Contributing

Contributions are welcome!

Make sure the following is fulfilled when filing a MR:
1. Write integration tests in `tests/`. Annotate if this is covering a spec.
2. Hack away.
3. Update/add documentation if applicable.
4. `cargo nextest run --cargo-profile fast-release --features slow-tests --stress-count 10` passes
4. Review test coverage of touched pieces of code. Every changed line should be covered.

### Getting started
- rustup recommended
- `rustup install stable`
- `cargo install cargo-llvm-cov` // for code coverage
- `cargo install cargo-nextest` // test executor
- use `cargo llvm-cov nextest --hide-progress-bar --failure-output final --lcov --output-path ./target/lcov.info` for creating a test coverage report
- install https://marketplace.visualstudio.com/items?itemName=ryanluker.vscode-coverage-gutters for visualizing the coverage in VS Code
- recommeded: the rust-analyzer plugin for VS Code
- the .github-instructions should tell your LLM (Claude Sonnet) everything it needs to know
- Bacon is configured if you want to use it