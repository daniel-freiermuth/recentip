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

### Identifier Requirements

| Requirement ID | Description | Status | Test / Justification |
|----------------|-------------|--------|----------------------|
| feat_req_recentip_538 | Service identified by Service ID | ✅ TESTED | `rpc::identifiers::service_id_is_u16` |
| feat_req_recentip_539 | Service ID is uint16 | ✅ TESTED | `rpc::identifiers::service_id_is_u16`, proptest |
| feat_req_recentip_540 | Service ID 0x0000 reserved | ✅ TESTED | `rpc::identifiers::service_id_reserved_rejected` |
| feat_req_recentip_541 | Service ID 0xFFFF reserved | ✅ TESTED | `rpc::identifiers::service_id_reserved_rejected` |

### Message Format Requirements

| Requirement ID | Description | Status | Test / Justification |
|----------------|-------------|--------|----------------------|
| feat_req_recentip_45 | Header format | 📋 SERIALIZATION | Wire format tests |
| feat_req_recentip_60 | Message structure | 📋 SERIALIZATION | Wire format tests |

### Request/Response Semantics

| Requirement ID | Description | Status | Test / Justification |
|----------------|-------------|--------|----------------------|
| feat_req_recentip_XX | Fire&Forget no response | ✅ TYPE_SYSTEM | `fire_and_forget()` returns `()` |
| feat_req_recentip_XX | Response required for R/R | ✅ TYPE_SYSTEM | `Responder` must-consume pattern |

---

## SD Requirements (someip-sd.rst)

### Port Requirements

| Requirement ID | Description | Status | Test / Justification |
|----------------|-------------|--------|----------------------|
| feat_req_recentipsd_XXX | SD port is 30490 | ✅ TESTED | `sd::sd_port::sd_port_is_30490` |
| feat_req_recentipsd_XXX | App ports exclude 30490 | ✅ TESTED | proptest `non_sd_ports_valid` |

### Eventgroup Requirements

| Requirement ID | Description | Status | Test / Justification |
|----------------|-------------|--------|----------------------|
| feat_req_recentipsd_XXX | Eventgroup 0 reserved | ✅ TESTED | `sd::eventgroups::eventgroup_zero_reserved` |

### Subscription Requirements

| Requirement ID | Description | Status | Test / Justification |
|----------------|-------------|--------|----------------------|
| feat_req_recentipsd_XXX | Subscribe to available only | ✅ TYPE_SYSTEM | Typestate pattern |
| feat_req_recentipsd_XXX | Unsubscribe on cleanup | ✅ TESTED | `api_usage::subscription_stops_on_drop` |

### Offer/Find Requirements

| Requirement ID | Description | Status | Test / Justification |
|----------------|-------------|--------|----------------------|
| feat_req_recentipsd_XXX | Offer needs concrete instance | ✅ TYPE_SYSTEM | `ConcreteInstanceId` type |
| feat_req_recentipsd_XXX | Find allows ANY instance | ✅ TESTED | `InstanceId::ANY` |

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
