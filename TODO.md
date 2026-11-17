# TODO

This file tracks active work items organized by granularity. Completed tasks should be removed.

---

## Final Goal

**Production-Ready SOME/IP Implementation**: A fully spec-compliant RECENT/IP library with 100% requirement coverage, comprehensive test suite, and traceability matrix proving compliance.

---

## Epics

### 2. SOME/IP-TP Implementation
Transport Protocol for segmentation/reassembly of large messages. Currently **NOT implemented** - the 9 TP tests are empty stubs with only `covers!()` macros. The TP header parsing utilities exist, but actual segmentation/reassembly in runtime is missing.

### 3. Full Spec Compliance Testing
Comprehensive test coverage with traceability matrix documenting which requirements are tested and why untested ones are excluded. Currently 354/354 tests pass, 19 ignored.

### 4. Multi-Homed Host Support
True network isolation testing infrastructure needed:
- Two separate networks with different SD multicast groups
- Host with two network interfaces (one per network)
- Services with identical service+instance IDs on both networks
- Verify Runtime instances only discover services on their network
- Requires `SO_BINDTODEVICE` (Linux) or `IP_PKTINFO` (others)

### 5. Data Plane Bypass (Performance)
Currently all RPC traffic flows through the central event loop. For high-throughput scenarios, separate control plane from data plane:
- **Control plane (event loop)**: SD messages, commands, state mutations, periodic tasks
- **Data plane (direct)**: RPC requests/responses bypass event loop, dispatch directly to handlers

Benefits:
- Event loop handles ~100 SD msgs/sec instead of potentially thousands of RPC msgs/sec
- Lower latency for RPC (no channel hop through event loop)
- Better scalability for many concurrent services

Implementation approach:
- Server socket readers dispatch directly to service handler (already know `ServiceKey`)
- Client responses: use `Arc<DashMap>` for `pending_calls` lookup from reader task
- Events already have direct channels to subscription handles

### 6. IPv6 SD Options (spec Release 26-11_v12)
Release 26-11_v12 made 27 IPv6 SD-option items normative (IPv6 Endpoint 0x06, IPv6 Multicast 0x16, IPv6 SD Endpoint 0x26). The stack is IPv4-only end to end (`SdOption`, `net` traits, config, runtime state). Plan, affected APIs and alternatives (incl. documented non-goal): `docs/MODERNIZATION-ipv6-sd-options.md`. Needs a scope decision; full migration is a breaking 0.5 release.

---

## Tasks

### Implement SOME/IP-TP (Epic 2)
- [ ] Implement TP segmentation for outgoing messages exceeding MTU
- [ ] Implement TP reassembly for incoming segmented messages
- [ ] Integrate TP with runtime event loop
- [ ] Update 9 stub tests with real assertions
- [ ] Document TP configuration options

### Fix Failing Ignored Tests
| Test | Notes |
|------|-------|
| `subscribe_to_unknown_eventgroup_should_nack` | NACK for unknown eventgroups not sent |
| `udp_events_real_network` | Real network event delivery failing |

### Session Handling Edge Cases
- [ ] Add test for event session ID handling (`feat_req_someip_667`)
- [ ] Investigate `feat_req_someip_700` - do we support disabled session handling?
- [ ] Test: StopSubscribe session regression triggers reboot detection

### IPv6 SD Options (Epic 6)
- [ ] Decide: full IPv6 migration vs. documented non-goal (`not_tested!` for the 27 IDs)
- [ ] Step 1: parse/serialize `SdOption` IPv6 Endpoint/Multicast/SD Endpoint (+ IPv4 SD Endpoint 0x24), round-trip + proptests
- [ ] Step 2: `net` traits to `SocketAddr`, v6 multicast join; re-run QNX check
- [ ] Step 3: config/public handles to `IpAddr`/`SocketAddr`
- [ ] Step 4: runtime state and SD logic per family; step 5: turmoil `IpVersion::V6` test runs

---

## Next Steps

1. **Decide IPv6 scope (Epic 6)** - full migration or documented non-goal; then implement `SdOption` IPv6 parsing (step 1)
2. **Fix `subscribe_to_unknown_eventgroup_should_nack`** - Server should NACK subscriptions to non-offered eventgroups
3. **Fix `udp_events_real_network`** - Debug real network UDP event delivery
4. **Start TP segmentation** - Begin with outgoing message segmentation logic
5. **Create fast session wraparound test** - Mock-based test for 0xFFFF→1 wrap (current takes 256s)
6. **Test: StopSubscribe session regression triggers reboot detection** - complete coverage
7. **Add test for event session ID handling** - `feat_req_someip_667`
8. **Investigate `feat_req_someip_700`** - does the implementation support disabled session handling?

---

## Backlog

Items not yet scheduled:

- Investigate: a dedicated UDP subscription socket reused by another service (`find_reusable_subscription_endpoint`) forwards every notification to the socket owner's `events_tx` without checking `service_id` — the reusing service's subscription may never receive its events
- **Server-side static binding API** - Add `runtime.bind()` for server-side services without SD (parallel to client-side `OfferedService::new()`)
- Multi-homed host testing infrastructure (Vagrant/Docker/netns options)
- Port rotation tests
- Configuration validation tests
- vsomeip interoperability testing
- Conditional subscription acceptance (application-controlled ACK/NACK)
- **TCP receive-buffer bound** - `next_tcp_frame` (`src/tcp.rs`) waits indefinitely for a frame whose Length field is huge (up to `u32::MAX`); a peer can grow a per-connection `BytesMut` without limit. Needs a decided max message size (config option, interplay with SOME/IP-TP) before rejecting oversized frames
- **TCP handlers keep reading after `msg_tx` closes** - in `handle_client_tcp_connection` / `handle_tcp_connection`, a failed `msg_tx.send` only breaks the framing loop, not the connection loop, despite logging "stopping"
- TTL expiry vs reboot-triggered cancellation test
- FindService → OfferService session continuity test
- TCP eventgroup isolation when an RPC connection exists first: every subsequent TCP subscription to that server reuses the RPC connection (conn_key 0), so events of different eventgroups of the same service are delivered to all of those subscribers (subscribe-first order keeps per-eventgroup connections)
- `handle_unsubscribe`: `port_still_in_use` compares ports across transports; a UDP sub and a TCP sub of the same service on the same port number keep the UDP endpoint registration alive after the UDP sub is dropped. Filter by transport.
- SD send path has no datagram-size check: a clustered SD message (e.g. many initial offers in one flush, `src/runtime/event_loop.rs`) may exceed the UDP payload/MTU. Option-index overflow (>256 options) is already handled by splitting in `cluster_sd_actions`
- Sort out hardcoded timings
  - Set turmoils max_message_latency for all tests
  - Offer distance timing
  - SD unicast clustering timing
  - SD message slowdown in tests
  - Offer timing test (proptest and basic test)
  - Unsub timing test
- QNX follow-ups (see `.notes-to-the-agent/qnx.md`):
  - Add `*_qnx710_iosock` targets to `qnx-check` once socket2/libc/tokio compile for them
  - Add QNX SDP 8.0 (`aarch64-unknown-qnx`) to `qnx-check` once mio/socket2 support `target_os = "qnx"`
  - TCP keepalive `interval`/`retries` are ignored on QNX 7.x (socket2 only sets the idle time there)
  - Keep the `QNX_TOOLCHAIN` nightly pin in CI moving (currently same date as `someip-lints/rust-toolchain`)
- `cargo test` (shared test process) fails ~26 tests at `tests/helpers.rs:248` `.expect("Configure tracing")` — global tracing subscriber init races between tests; `cargo nextest run` (process-per-test) is unaffected. Make init idempotent (`try_init`/`Once`)
- Backpressure on shared/TCP event paths: `handle_incoming_notification` (`src/runtime/client.rs`, `let _ = sub.events_tx.try_send(..)` and static listeners) drops events on a full channel without any log; align with the dedicated UDP socket path (warn on drop, rate-limited)
- `test_event_subscription` (`tests/api_tests.rs`) registers both nodes via `sim.host`, so `sim.run()` returns before either runs; client must be `sim.client`
- Requirement-citation audit: SD header tests (`sd_offer_wire_format`, `sd_offer_discovery_works`, `service_discovery_finds_offered_service`) cite `feat_req_someipsd_141` (IPv4 Endpoint Option figure) for "Service ID 0xFFFF" and `feat_req_someipsd_144` (Configuration Option figure) for Client ID / versions; both belong to `feat_req_someipsd_26`. Same pattern (pre-open-spec numbering) likely elsewhere
- IPv4 SD Endpoint Option (0x24) is not modelled in `SdOption` either; fold into Epic 6 step 1
- `tools/extract_requirements.py` truncates requirement text to 500 chars; e.g. `feat_req_someipsd_210` loses its "shall allow receiving unsorted entries" sentence in the traceability report
- Flaky (1 failure in 56 local stress iterations): `tcp_binding::server_detects_client_reboot_clears_subscriptions_port_reuse` — turmoil can deliver the wire client's post-reboot SD Subscribes (session 1, then 2) out of order; the server sees session 2 → 1, detects a reboot after the fresh connections exist and closes SERVICE3's new TCP connection ("SERVICE3 TCP connection closed unexpectedly!"). Fix ordering in the test (await SERVICE2's ACK before subscribing SERVICE3, or one SD message with both entries)

---

## Test Status Summary

| Category | Status |
|----------|--------|
| **Total tests** | 547 pass, 11 skipped (nextest) |
| **Ignored (stubs)** | 9 TP tests, needs implementation |
| **Ignored (design choice)** | 1 test (`subscribe_to_unknown_eventgroup_should_nack`) |
| **Ignored (network)** | 1 test (`udp_events_real_network`) |

---

## Notes

- Session ID & Reboot Flag compliance is **complete** - all core detection working
- Subscribe clustering implemented with 50ms batching window
- Reboot detection uses threshold of 100 to tolerate out-of-order delivery
- **Session zero rejection** implemented (`sd_session_zero_rejected` passing)
- **Server-side client reboot detection** implemented (server expires subscriptions on client reboot/session regression)
- **8 previously-ignored tests** now passing: `sd_session_zero_rejected`, `server_expires_subscriptions_on_client_reboot`, `server_expires_subscriptions_on_client_session_regression`, `normal_session_wraparound_does_not_trigger_reboot`, `multicast_session_wraparound_does_not_affect_subscriptions`, `client_tracks_session_ids_per_server_independently`, `client_tracks_reboot_flags_per_server_independently`, `server_detects_client_reboot_clears_subscriptions_port_reuse`
