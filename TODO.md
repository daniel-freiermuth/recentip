# TODO

This file tracks active work items organized by granularity. Completed tasks should be removed.

---

## Final Goal

**Production-Ready SOME/IP Implementation**: A fully spec-compliant RECENT/IP library with 100% requirement coverage, comprehensive test suite, and traceability matrix proving compliance.

---

## Epics

### 1. Concurrent Subscribe Command Processing
The event loop currently processes Subscribe commands sequentially, which causes one slow/hanging TCP connection to block all other subscription attempts. This is a known architectural limitation documented in test `tcp_slow_service_doesnt_block_other_services`.

**Impact**: In production, subscribing to 10 services at startup where 1 service is slow will delay all 10 subscriptions by the TCP timeout (2s).

**Solution**: Spawn Subscribe handling as background tasks that send state updates back to the event loop via channels.

**Requirements**:
- RuntimeState cannot be shared across tasks (not Clone, contains DashMap)
- TCP connection must complete before sending Subscribe SD message (feat_req_someipsd_767)
- State updates must be serialized through the event loop for correctness

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

### Optimize TcpConnectionPool for true concurrent connections
**Status**: Optional performance optimization

**Context**: Subscribe commands now spawn as concurrent tasks (Epic 1 complete), but TCP connection attempts still serialize due to `Arc<Mutex<TcpConnectionPool>>`. One slow TCP connection (2s timeout) blocks other Subscribe tasks from starting their connections.

**Impact**: 
- ✅ Event loop remains responsive (Subscribe doesn't block main loop)
- ✅ Other commands (Find, Call, Unsubscribe) process normally
- ✅ UDP subscriptions are unaffected
- ⚠️ Multiple concurrent TCP subscriptions wait for each other

**Solution**: Refactor `TcpConnectionPool::ensure_connected()` to not hold mutex during TCP connect:
- Option 1: Per-target fine-grained locks (DashMap of connection states)
- Option 2: Optimistic connection start, then lock only to register/store
- Option 3: Lock-free connection tracking with async coordination

**Priority**: Medium - current behavior is acceptable for most use cases

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

### SD Session ID Per-Peer Tracking
Currently `unicast_session_id` is global. Should be `HashMap<IpAddr, u16>` per `feat_req_someipsd_765`.
- [ ] Refactor `unicast_session_id: u16` → `unicast_session_ids: HashMap<IpAddr, u16>`
- [ ] Add test: each peer gets independent unicast session counter

### Server-Side Reboot Handling
- [ ] Implement server expiring subscriptions on client reboot (unignore `server_expires_subscriptions_on_client_reboot`)
- [ ] Implement server expiring subscriptions on client session regression

### Session Handling Edge Cases
- [ ] Add test for event session ID handling (`feat_req_someip_667`)
- [ ] Investigate `feat_req_someip_700` - do we support disabled session handling?
- [ ] Test: StopSubscribe session regression triggers reboot detection

---

## Next Steps

1. **Fix `subscribe_to_unknown_eventgroup_should_nack`** - Server should NACK subscriptions to non-offered eventgroups
2. **Fix `udp_events_real_network`** - Debug real network UDP event delivery
3. **Implement per-peer unicast session counters** - Refactor `unicast_session_id` to `HashMap<IpAddr, u16>`
4. **Implement server-side client reboot detection** - Enable `server_expires_subscriptions_on_client_reboot` test
5. **Start TP segmentation** - Begin with outgoing message segmentation logic
6. **Create fast session wraparound test** - Mock-based test for 0xFFFF→1 wrap (current takes 256s)
7. **Add session ID = 0 rejection** - Unignore and implement `sd_session_zero_rejected`

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
| **Total tests** | 365 pass, 19 ignored |
| **Ignored (stubs)** | 9 TP tests, needs implementation |
| **Ignored (pass)** | ~8 session/reboot tests (mostly work) |
| **Ignored (fail)** | 2 tests need fixes |

---

## Notes

- Session ID & Reboot Flag compliance is **complete** - all core detection working
- Subscribe clustering implemented with 50ms batching window
- Reboot detection uses threshold of 100 to tolerate out-of-order delivery
