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

---

## Next Steps

1. **Fix `subscribe_to_unknown_eventgroup_should_nack`** - Server should NACK subscriptions to non-offered eventgroups
2. **Fix `udp_events_real_network`** - Debug real network UDP event delivery
3. **Start TP segmentation** - Begin with outgoing message segmentation logic
4. **Create fast session wraparound test** - Mock-based test for 0xFFFF→1 wrap (current takes 256s)
5. **Test: StopSubscribe session regression triggers reboot detection** - complete coverage
6. **Add test for event session ID handling** - `feat_req_someip_667`
7. **Investigate `feat_req_someip_700`** - does the implementation support disabled session handling?

---

## Backlog

Items not yet scheduled:

- **Server-side static binding API** - Add `runtime.bind()` for server-side services without SD (parallel to client-side `OfferedService::new()`)
- Multi-homed host testing infrastructure (Vagrant/Docker/netns options)
- Port rotation tests
- Configuration validation tests
- vsomeip interoperability testing
- Conditional subscription acceptance (application-controlled ACK/NACK)
- TTL expiry vs reboot-triggered cancellation test
- FindService → OfferService session continuity test
- Sort out hardcoded timings
  - Set turmoils max_message_latency for all tests
  - Offer distance timing
  - SD unicast clustering timing
  - SD message slowdown in tests
  - Offer timing test (proptest and basic test)
  - Unsub timing test

---

## Test Status Summary

| Category | Status |
|----------|--------|
| **Total tests** | 420 pass, 11 ignored |
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
