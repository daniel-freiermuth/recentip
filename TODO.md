# TODO: Spec Compliance Testing

## Goal
Create a comprehensive test suite that proves RECENT/IP spec compliance, with a traceability matrix documenting which requirements are tested and why untested ones are excluded.

- [ ] Review. Überblick
[ ] 100% test coverage
[ ] 100% requirement coverage
[ ] Port rotation tests
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


---

## Next Steps

### Priority 1: Implement SOME/IP-TP
SOME/IP-TP is **NOT implemented**. The 9 TP tests that "pass" when running ignored tests are empty stubs - they only contain `covers!()` macros and comments. The TP header parsing utilities are tested, but the actual segmentation/reassembly in the runtime is not implemented.

### Priority 2: Fix Failing Ignored Tests (3 tests)
| Test | Status | Notes |
|------|--------|-------|
| multi_protocol::preferred_transport_respected_for_pubsub_when_both_available | ❌ Fail | Transport preference for pub/sub |
| server_behavior::subscription_nack::subscribe_to_unknown_eventgroup_should_nack | ❌ Fail | NACK for unknown eventgroups |
| real_network::udp_events_real_network | ❌ Fail | Real network event delivery |

### Other Ignored Tests (11 tests, all pass but some are stubs)
| Test | Notes |
|------|-------|
| transport_protocol::tp_* (9 tests) | Empty stubs awaiting TP implementation |
| session_handling::session_id_wraps_to_0001_not_0000 | Works, but takes 256s to run |

---

## Future Considerations

### Conditional Subscription Acceptance
Currently subscriptions are auto-accepted for any offered service. To support:
- Resource limits (max subscribers per eventgroup)
- Security checks (client credentials validation)
- Business logic (eventgroup availability conditions)

Would need:
1. Add responder handle to `ServiceEvent::Subscribe`
2. Defer ACK/NACK until application responds (with timeout)
3. Use `subscribe_eventgroup_nack()` for rejections

Low priority - most SOME/IP deployments auto-accept. Mainly needed for vsomeip compatibility
in security-sensitive environments.
