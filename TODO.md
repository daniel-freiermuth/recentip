# TODO: Spec Compliance Testing

## Goal
Create a comprehensive test suite that proves RECENT/IP spec compliance, with a traceability matrix documenting which requirements are tested and why untested ones are excluded.

---

## Phase 1: Requirements Extraction
- [x] **1.1** Parse all requirements from RST files ✅ 639 total (472 testable)
- [x] **1.2** Categorize by type (Requirement vs Information) ✅ in requirements.json
- [ ] **1.3** Categorize by testability (compile-time, runtime, integration, N/A)
- [x] **1.4** Generate initial requirements database ✅ spec-data/requirements.json

## Phase 2: Test Implementation
- [ ] **2.1** Identifier requirements (ServiceId, MethodId, EventId, etc.)
- [ ] **2.2** Message format requirements (header structure, fields)
- [ ] **2.3** Request/Response semantics
- [ ] **2.4** Service Discovery protocol
- [ ] **2.5** Subscription lifecycle
- [ ] **2.6** RECENT/IP-TP segmentation (deferred)

## Phase 3: Compliance Documentation
- [ ] **3.1** Script to extract `covers!()` annotations from tests
- [ ] **3.2** Auto-generate COMPLIANCE.md from annotations + requirements DB
- [ ] **3.3** Fill in justifications for untested requirements

---

## Progress

### 2025-12-23: Initial Setup
- [x] Created compliance test framework (`tests/compliance.rs`)
- [x] Added proptest for property-based testing
- [x] Created `covers!()` and `not_tested!()` macros
- [x] Initial COMPLIANCE.md structure
- [x] 20 property-based tests passing

### Current Stats
| Spec Document | Requirements | Tested | Pending |
|---------------|--------------|--------|---------|
| someip-rpc.rst | 181 | 8 | 173 |
| someip-sd.rst | 240 | 2 | 238 |
| someip-tp.rst | 37 | 0 | 37 |
| someip-ids.rst | 0 (8 info) | 1 | - |
| someip-compat.rst | 14 | 0 | 14 |
| **Total** | **472** | **11** | **461** |

### Requirements Covered by Tests
- `feat_req_recentip_538`: Service ID identification
- `feat_req_recentip_539`: Service ID is uint16
- `feat_req_recentip_627`: Service ID 0x0000, 0xFFFF reserved
- `feat_req_recentip_542`: Instance ID identification
- `feat_req_recentip_543`: Instance ID is uint16
- `feat_req_recentip_579`: Instance ID 0x0000, 0xFFFF reserved
- `feat_req_recentip_625`: Method/Event ID format
- `feat_req_recentip_545`: Eventgroup identification
- `feat_req_recentip_546`: Eventgroup ID is uint16
- `feat_req_recentip_676`: SD port 30490 reserved
- `feat_req_recentipids_555`: Eventgroup 0x0000 reserved

---

## Next Action
**Phase 1.3**: Categorize remaining requirements by testability to identify which need unit tests, integration tests, or are enforced by type system.
