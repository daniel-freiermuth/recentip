QNX cross-checking (researched 2026-10):

- QNX targets are rustc tier 3: not in `rustup target list`, `rustup target add` fails. Use
  `cargo +nightly-2026-08-14 check -Zbuild-std --target <t> --locked` with the `rust-src`
  component. `check` does not link, so no QNX SDP is needed. ~30 s cold per target.
  CI job: `qnx-check` (GitHub + GitLab), matrix aarch64-unknown-nto-qnx710, x86_64-pc-nto-qnx710.
- socket2 0.6.5 on target_os = "nto": `TcpKeepalive::with_interval`/`with_retries` do not exist,
  and `set_tcp_keepalive` only sets TCP_KEEPALIVE (idle time, as timeval). libc 0.2.189 nto
  defines no TCP_KEEPINTVL/TCP_KEEPCNT. Keep these calls behind cfg in src/net/tokio_impl.rs.
- *_iosock targets (QNX 7.1 io-sock stack) do not compile because of dependencies, not our code:
  socket2 0.6.5 imports `libc::TCP_KEEPIDLE` (missing in libc 0.2.189); with libc 0.2.190
  tokio 1.53.1 then fails on `libc::unpcbid`/`LOCAL_PEEREID`.
- QNX SDP 8.0 targets (aarch64-unknown-qnx, target_os = "qnx"): mio 1.2.4 (missing selector/waker
  modules) and socket2 0.6.5 (IPV6_ADD_MEMBERSHIP) do not compile yet.
