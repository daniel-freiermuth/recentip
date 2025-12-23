# Spec Compliance Matrix

This document tracks compliance of the `someip-runtime` library with the RECENT/IP specification.

## Overview

| Spec Document | Total Reqs | Tested | Justified N/A | Pending |
|---------------|------------|--------|---------------|---------|
| someip-rpc.rst | 181 | TBD | TBD | TBD |
| someip-sd.rst | 240 | TBD | TBD | TBD |
| someip-tp.rst | 37 | TBD | TBD | TBD |
| someip-ids.rst | 8+ | TBD | TBD | TBD |
| someip-compat.rst | 14 | TBD | TBD | TBD |

*Note: "Information" type requirements (167 total) are not counted as they document terminology, not testable behavior.*

## Testing Methodology

### Property-Based Testing with Proptest

We use [proptest](https://docs.rs/proptest) for property-based testing of protocol invariants. This allows us to verify requirements hold across the entire input space, not just example values.

Example:
```rust
proptest! {
    /// Property: Any u16 except reserved values creates valid ServiceId
    #[test]
    fn service_id_valid_range(value in 0x0001u16..=0xFFFE) {
        prop_assert!(ServiceId::new(value).is_some());
    }
}
```

### Test Categories

1. **Type System Tests**: Requirements enforced at compile time via newtypes and typestate
2. **Property Tests**: Invariants verified across input ranges with proptest
3. **Behavioral Tests**: Protocol state machine and interaction tests
4. **Serialization Tests**: Wire format compliance
5. **Integration Tests**: End-to-end scenarios with simulated network

### Justifications for Not Testing

| Justification Code | Meaning |
|--------------------|---------|
| `INFO` | Requirement type is "Information" - defines terminology only |
| `TYPE_SYSTEM` | Enforced at compile time by Rust's type system |
| `SERIALIZATION` | Tested at the serialization/wire-format layer |
| `INTEGRATION` | Tested in integration tests with full stack |
| `TIMING` | Requires real-time testing infrastructure |
| `CONFIG` | Verified by configuration validation |
| `OUT_OF_SCOPE` | Not applicable to this abstraction layer |

---

## RPC Requirements (someip-rpc.rst)

Total: 181 requirements | Tested: 8 | Pending: 173

### Identifier Requirements

| Requirement ID | Description | Status | Test / Justification |
|----------------|-------------|--------|----------------------|
| feat_req_recentip_538 | Service identified by Service ID | ✅ TESTED | `rpc::identifiers::service_id_is_u16` |
| feat_req_recentip_539 | Service ID is uint16 | ✅ TESTED | `rpc::identifiers::service_id_is_u16`, proptest |
| feat_req_recentip_624 | Service ID 0xFFFE for non-RECENT/IP | 📋 CONFIG | Configuration validation |
| feat_req_recentip_627 | Service ID 0x0000, 0xFFFF reserved | ✅ TESTED | `rpc::identifiers::service_id_reserved_rejected` |
| feat_req_recentip_541 | Different services have different IDs | 📋 CONFIG | Configuration validation |
| feat_req_recentip_542 | Instance identified by Instance ID | ✅ TESTED | `rpc::identifiers::instance_id_wildcard` |
| feat_req_recentip_543 | Instance ID is uint16 | ✅ TESTED | `rpc::identifiers::instance_id_wildcard` |
| feat_req_recentip_579 | Instance ID 0x0000, 0xFFFF reserved | ✅ TESTED | `rpc::identifiers::instance_id_wildcard` |
| feat_req_recentip_625 | Method/Event ID 16-bit, events 0x8000+ | ✅ TESTED | `rpc::identifiers::method_event_id_distinction` |
| feat_req_recentip_545 | Eventgroup identified by ID | ✅ TESTED | `sd::eventgroups::eventgroup_zero_reserved` |
| feat_req_recentip_546 | Eventgroup ID is uint16 | ✅ TESTED | proptest in `sd::eventgroups` |

### Message Format Requirements

| Requirement ID | Description | Status | Test / Justification |
|----------------|-------------|--------|----------------------|
| feat_req_recentip_42 | Headers in big endian | 📋 SERIALIZATION | Wire format tests |
| feat_req_recentip_44 | Header layout identical for all impls | 📋 SERIALIZATION | Wire format tests |
| feat_req_recentip_45 | Header format | 📋 SERIALIZATION | Wire format tests |

### Port Requirements

| Requirement ID | Description | Status | Test / Justification |
|----------------|-------------|--------|----------------------|
| feat_req_recentip_676 | Port 30490 reserved for SD | ✅ TESTED | `sd::port::sd_port_reserved` |
| feat_req_recentip_658 | SD default port 30490 | ✅ TESTED | `sd::port::sd_port_reserved` |

### Request/Response Semantics

| Requirement ID | Description | Status | Test / Justification |
|----------------|-------------|--------|----------------------|
| feat_req_recentip_348 | Fire&Forget no error return | ✅ TYPE_SYSTEM | `fire_and_forget()` returns `()` |
| feat_req_recentip_141 | Request answered by response | ✅ TYPE_SYSTEM | `Responder` must-consume pattern |

---

## SD Requirements (someip-sd.rst)

Total: 240 requirements | Tested: 2 | Pending: 238

### Eventgroup Requirements

| Requirement ID | Description | Status | Test / Justification |
|----------------|-------------|--------|----------------------|
| feat_req_recentipids_555 | Eventgroup 0x0000 reserved | ✅ TESTED | `sd::eventgroups::eventgroup_zero_reserved` |

### Subscription Requirements

| Requirement ID | Description | Status | Test / Justification |
|----------------|-------------|--------|----------------------|
| feat_req_recentipsd_433 | Unsubscribe with TTL=0 | ✅ TYPE_SYSTEM | RAII: Subscription drop sends StopSubscribe |

### Offer/Find Requirements

| Requirement ID | Description | Status | Test / Justification |
|----------------|-------------|--------|----------------------|
| feat_req_recentip_579 | Concrete instance != 0xFFFF | ✅ TYPE_SYSTEM | `ConcreteInstanceId` rejects wildcard |

---

## TP Requirements (someip-tp.rst)

*RECENT/IP-TP (segmentation) is not yet implemented. These requirements will be tracked when the TP layer is added.*

| Requirement ID | Description | Status | Test / Justification |
|----------------|-------------|--------|----------------------|
| feat_req_recentiptp_* | All TP requirements | 📋 NOT_IMPLEMENTED | Deferred to TP implementation |

---

## How to Update This Document

1. **Add new tests**: Create tests in `tests/compliance/` with `covers!()` macro
2. **Regenerate matrix**: Run `cargo run --bin compliance-matrix` (TODO)
3. **Fill in requirement IDs**: Use grep to find actual IDs from spec RST files

### Finding Requirement IDs

```bash
# Find requirements about "Service ID"
grep -B5 -A10 "Service ID" src/someip-rpc.rst | grep feat_req

# Count requirements per section  
grep ":id: feat_req_" src/someip-rpc.rst | wc -l
```

---

## Compliance Checklist

- [ ] All `Requirement` type entries have test or justification
- [ ] All `Information` type entries marked as INFO
- [ ] Property tests cover edge cases and boundaries
- [ ] Serialization layer tests cover wire format
- [ ] Integration tests cover timing-dependent behavior
