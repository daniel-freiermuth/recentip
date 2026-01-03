# TODO: Spec Compliance Testing

## Goal
Create a comprehensive test suite that proves RECENT/IP spec compliance, with a traceability matrix documenting which requirements are tested and why untested ones are excluded.

- [ ] Dirty state weiter aufräumen
  - [ ] Commented-out lines
- [ ] Implement fire'n'forget
- [ ] Tests with real network
- [ ] Große Datei splitten
- [ ] Review. Überblick
- [ ] Docs
- [ ] TP
- [ ] Config
- [ ] **Multi-homed host testing with true network isolation**
  - Current turmoil-based tests don't support true network partitioning
  - Need infrastructure for multi-homed testing:
    - Option 1: Vagrant (multi-VM setup with separate networks)
    - Option 2: Docker + macvlan (containers with true network isolation)
    - Option 3: Linux network namespaces (netns for lightweight isolation)
    - Option 4: Real hardware (physical multi-homed setup)
  - Test requirements:
    - Two separate networks with different SD multicast groups
    - Host B with two network interfaces (one per network)
    - Services with identical service+instance IDs on both networks
    - Verify Runtime instances only discover services on their network
    - Verify no cross-talk between networks despite ID duplicates
    - This should be used together with loopback traffic
      => Requires BINDTODEVICE option (linux) or PKTINFO (others)
      => or only using _one_ multiplexer?

---

## Phase 1: Requirements Extraction ✅
- [x] **1.1** Parse all requirements from RST files ✅ 639 total
- [x] **1.2** Categorize by type (Requirement vs Information) ✅ in requirements.json
- [x] **1.3** Categorize by spec file and section ✅ see COVERAGE_REPORT.md
- [x] **1.4** Generate initial requirements database ✅ spec-data/requirements.json

## Phase 2: Test Implementation ✅
- [x] **2.1** Identifier requirements (ServiceId, MethodId, EventId, etc.) ✅ api_types module
- [x] **2.2** Message format requirements (header structure, fields) ✅ wire_format module
- [x] **2.3** Request/Response semantics ✅ rpc_flow module
- [x] **2.4** Service Discovery protocol ✅ service_discovery module
- [x] **2.5** Subscription lifecycle ✅ events module
- [x] **2.6** SOME/IP-TP segmentation ✅ transport_protocol module
- [x] **2.7** Error handling ✅ error_scenarios module
- [x] **2.8** Session handling ✅ session_edge_cases module
- [x] **2.9** Version handling ✅ version_handling module
- [x] **2.10** TCP/UDP bindings ✅ tcp_binding, udp_binding modules
- [x] **2.11** Field operations ✅ fields module
- [x] **2.12** Multiple instances ✅ instances module
- [x] **2.13** Multi-party scenarios ✅ multi_party module

## Phase 3: Compliance Documentation ✅
- [x] **3.1** Script to extract `covers!()` annotations ✅ `scripts/extract_coverage.py`
- [x] **3.2** Auto-generate COMPLIANCE.md ✅ `scripts/generate_compliance.py`
- [x] **3.3** Coverage report by spec section ✅ COVERAGE_REPORT.md

---

## Current Stats (2025-12-28)

| Metric | Value |
|--------|-------|
| Total Tests | 136 |
| Tests Passing | 136 |
| Tests Skipped (ignored) | 15 |
| Total Test Functions | 151 |

### Ignored Tests by Reason

| Reason | Count | Notes |
|--------|-------|-------|
| `Runtime::new not implemented` | 75 | Old sync API tests in unported modules |
| `SOME/IP-TP not yet implemented` | 10 | Transport Protocol segmentation |
| `fire_and_forget not yet implemented` | 2 | REQUEST_NO_RETURN message type |
| `ERROR message type handling` | 1 | Error response propagation |
| `turmoil timing limitation` | 1 | Late server + subscription edge case |
| `request ID matching` | 1 | Runtime doesn't match responses by request ID |

### Ported Modules (turmoil-based async tests)

| Module | Tests | Status |
|--------|-------|--------|
| api_types | 22 | ✅ All passing |
| integration | 8 | ✅ All passing |
| wire_format | 14 | ✅ All passing |
| service_discovery | 11 | ✅ All passing |
| subscription | 6 | ✅ All passing |
| session_handling | 16 | ✅ 14 passing, 2 ignored |
| error_handling | 14 | ✅ All passing |
| transport_protocol | 8 unit + 9 ignored | ✅ Unit tests passing, integration awaits TP |
| message_types | 23 unit + 5 proptest + 2 turmoil | ✅ Passing, 3 ignored |

### Unported Modules (still use old SimulatedNetwork)

These modules compile but all their integration tests are ignored with
`#[ignore = "Runtime::new not implemented"]`. They need migration to turmoil:

| Module | Ignored Tests |
|--------|---------------|
| error_scenarios | 9 |
| events | 11 |
| fields | 7 |
| instances | 8 |
| multi_party | 8 |
| rpc_flow | 8 |
| tcp_binding | 12 |
| udp_binding | 8 |
| version_handling | 4 |

---

## API Design Decisions Made

### Confirmed
- [x] `subscribe(&self, ...)` takes reference (allows multiple eventgroup subscriptions)
- [x] RPC calls on `ServiceProxy<Available>`, not on `Subscription`
- [x] Session handling always enabled (mandatory per spec)
- [x] No `initial_session_id` config (just iterate in tests)
- [x] `ConcreteInstanceId` implements `Ord` for sorting

### API Added
- [x] `RuntimeConfig::builder().transport().magic_cookies()`
- [x] `ServiceConfigBuilder::port()` for port sharing
- [x] `ServiceProxy::available_instances()` for multi-instance discovery
- [x] `AvailableInstance` struct with `instance_id()`, `service_id()`, `endpoint()`
- [x] `SimulatedNetwork::new_multi(count)` for N-party testing

---

## Next Steps

### Priority 1: Implement Missing Features
| Feature | Ignored Tests | Notes |
|---------|---------------|-------|
| `fire_and_forget()` | 2 | REQUEST_NO_RETURN message type |
| ERROR message handling | 1 | `reply_error()` → client `Err` |
| SOME/IP-TP | 10 | Large message segmentation |
| Request ID matching | 1 | Match responses to pending requests |

### Priority 2: Port Remaining Modules to Turmoil
The 75 tests with `#[ignore = "Runtime::new not implemented"]` are in modules
that still use the old `SimulatedNetwork` infrastructure. Port them to turmoil:
- events.rs (11 tests)
- tcp_binding.rs (12 tests)  
- error_scenarios.rs (9 tests)
- rpc_flow.rs (8 tests)
- instances.rs (8 tests)
- multi_party.rs (8 tests)
- udp_binding.rs (8 tests)
- fields.rs (7 tests)
- version_handling.rs (4 tests)

---

## Future Considerations

### ServiceConfig Builder Pattern
The old sync API had a `ServiceConfig::builder()` pattern for configuring service offerings:
```rust
let config = ServiceConfig::builder()
    .service(service_id)
    .instance(instance_id)
    .eventgroup(eventgroup_id)
    .build()?;
```

The current async API uses the `Service` trait + `runtime.offer::<S>(instance_id)` pattern instead.
Consider whether we need a runtime config for:
- Per-service eventgroup definitions
- Port configuration for port sharing
- TTL and timing parameters
- Method/event routing configuration
