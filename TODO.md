# TODO: Spec Compliance Testing

## Goal
Create a comprehensive test suite that proves RECENT/IP spec compliance, with a traceability matrix documenting which requirements are tested and why untested ones are excluded.

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

## Current Stats (2025-12-23)

| Metric | Value |
|--------|-------|
| Total Tests | 199 |
| Tests Passing (types/parsing) | ~40 |
| Tests Ignored (need Runtime) | ~159 |
| Requirements Covered | 99 (verified) |
| Total Requirements | 639 |
| Coverage | 15.5% |

### Test Modules

| Module              | Tests | Description                           |
|---------------------|-------|---------------------------------------|
| api_types           | 16    | ID types, ranges, reserved values     |
| wire_format         | 22    | Header parsing, message structure     |
| service_discovery   | 18    | SD protocol, entries, options         |
| transport_protocol  | 28    | TP segmentation, reassembly           |
| tcp_binding         | 15    | TCP framing, magic cookies            |
| udp_binding         | 12    | UDP message handling                  |
| rpc_flow            | 10    | Request/response patterns             |
| events              | 17    | Pub/sub, multiple subscriptions       |
| fields              | 8     | Getter/setter/notifier                |
| error_scenarios     | 13    | Error handling, malformed messages    |
| session_edge_cases  | 9     | Session ID wrap, request ID           |
| instances           | 10    | Multi-instance discovery              |
| multi_party         | 10    | N-party network scenarios             |
| message_types       | ~5    | Message type field values             |
| version_handling    | ~6    | Protocol/interface versions           |

### Coverage by Spec File

| File | Covered | Total | % |
|------|---------|-------|---|
| someip-rpc.rst | 77 | 248 | 31% |
| someip-tp.rst | 10 | 40 | 25% |
| someip-ids.rst | 1 | 8 | 12.5% |
| someip-sd.rst | 11 | 328 | 3.4% |
| someip-compat.rst | 0 | 15 | 0% |

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

### Priority 1: Implement Runtime
The 159 ignored tests will pass once `Runtime::new()` is implemented.

### Priority 2: Coverage Gaps (if desired)
| Area | Gap | Notes |
|------|-----|-------|
| SD Protocol Internals | 300+ | Timing, state machines - implementation detail |
| Serialization | 50+ | Structs, arrays, strings - future serde integration |
| TP Receiver | 18 | Reassembly behavior |
| Compatibility | 15 | Version negotiation |

### Not Prioritized
- SD timing requirements (INITIAL_DELAY, etc.) - runtime config
- Serialization tests - separate `someip-types` crate responsibility

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
