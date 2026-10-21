Via `cargo nextest run`

Plain `cargo test --test compliance <filter>` can fail with a panic in
`tests/helpers.rs` `configure_tracing` ("Configure tracing") when a
`#[test_log::test]` test in the same process installed the global subscriber
first. nextest runs each test in its own process, so this does not occur there.