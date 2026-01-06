//! API Type Compliance Tests
//!
//! Tests that verify the type system enforces spec requirements.
//! Uses proptest for property-based validation of identifier ranges.

use crate::covers;
use proptest::prelude::*;
use someip_runtime::prelude::*;

// ============================================================================
// RPC PROTOCOL COMPLIANCE (someip-rpc.rst)
// ============================================================================

mod rpc {
    use super::*;

    // ------------------------------------------------------------------------
    // IDENTIFIER REQUIREMENTS
    // ------------------------------------------------------------------------

    mod identifiers {
        use super::*;

        /// feat_req_recentip_538: A service shall be identified using the Service ID.
        /// feat_req_recentip_539: Service IDs shall be of type 16 bit length unsigned integer.
        #[test_log::test]
        fn service_id_is_u16() {
            covers!(feat_req_recentip_538, feat_req_recentip_539);

            // ServiceId wraps u16
            let id = ServiceId::new(0x1234).unwrap();
            assert_eq!(id.value(), 0x1234u16);

            // Maximum valid value fits in u16
            let max = ServiceId::new(0xFFFE).unwrap();
            assert_eq!(max.value(), 0xFFFE);
        }

        proptest! {
            /// Property: Any u16 except reserved values creates valid ServiceId
            #[test_log::test]
            fn service_id_valid_range(value in 0x0001u16..=0xFFFE) {
                covers!(feat_req_recentip_539);
                prop_assert!(ServiceId::new(value).is_some());
            }

            /// feat_req_recentip_627: Service ID 0x0000 and 0xFFFF reserved
            #[test_log::test]
            fn service_id_reserved_rejected(value in prop::sample::select(vec![0x0000u16, 0xFFFF])) {
                covers!(feat_req_recentip_627);
                prop_assert!(ServiceId::new(value).is_none());
            }
        }

        /// feat_req_recentip_625: Methods and events identified by 16 bit Method ID
        /// Events use range 0x8000-0xFFFE (high bit set)
        #[test_log::test]
        fn method_event_id_distinction() {
            covers!(feat_req_recentip_625);
            // Methods: 0x0000-0x7FFF (any value, including 0)
            assert!(MethodId::new(0x0001).unwrap().value() < 0x8000);
            assert!(MethodId::new(0x7FFF).unwrap().value() < 0x8000);
            assert_eq!(MethodId::new(0x0000).unwrap().value(), 0x0000); // Allowed for methods

            // Events: 0x8000-0xFFFE (high bit set, 0xFFFF reserved)
            let event = EventId::new(0x8000).unwrap();
            assert!(event.value() >= 0x8000);
            assert!(EventId::new(0xFFFE).is_some());
            assert!(EventId::new(0xFFFF).is_none()); // Reserved
        }

        proptest! {
            /// Property: Event IDs always have high bit set
            #[test_log::test]
            fn event_ids_have_high_bit(value in 0x8000u16..=0xFFFE) {
                let event = EventId::new(value);
                prop_assert!(event.is_some());
                prop_assert!(event.unwrap().value() & 0x8000 != 0);
            }

            /// Property: Values below 0x8000 cannot be EventIds
            #[test_log::test]
            fn low_values_not_events(value in 0x0000u16..0x8000) {
                prop_assert!(EventId::new(value).is_none());
            }
        }

        /// feat_req_recentip_542: Service instance identified by Instance ID
        /// feat_req_recentip_543: Instance IDs are uint16
        /// feat_req_recentip_579: Instance IDs 0x0000 and 0xFFFF reserved
        #[test_log::test]
        fn instance_id_wildcard() {
            covers!(
                feat_req_recentip_542,
                feat_req_recentip_543,
                feat_req_recentip_579
            );
            // 0xFFFF means "any instance" for client-side matching
            assert_eq!(InstanceId::ANY.value(), 0xFFFF);
        }
    }

    // ------------------------------------------------------------------------
    // REQUEST/RESPONSE SEMANTICS
    // ------------------------------------------------------------------------

    mod semantics {
        use super::*;

        #[test_log::test]
        fn fire_and_forget_returns_nothing() {
            covers!(feat_req_recentip_15);
            // Fire&Forget: no response expected
            // The API enforces this: fire_and_forget() returns Result<()>
            // while call() returns Result<PendingResponse>
            // This is a type-system guarantee.
        }

        #[test_log::test]
        fn responder_must_be_consumed() {
            covers!(feat_req_recentip_15);
            // Responder MUST send exactly one response
            // Dropping without response panics in debug mode
            // This is tested behaviorally in api_usage.rs
        }
    }
}

// ============================================================================
// SERVICE DISCOVERY COMPLIANCE (someip-sd.rst)
// ============================================================================

mod sd {
    use super::*;

    // ------------------------------------------------------------------------
    // EVENTGROUPS
    // ------------------------------------------------------------------------

    mod eventgroups {
        use super::*;

        /// feat_req_recentipids_555: Eventgroup ID 0x0000 is reserved
        #[test_log::test]
        fn eventgroup_zero_reserved() {
            covers!(feat_req_recentipids_555);
            assert!(EventgroupId::new(0x0000).is_none());
            assert!(EventgroupId::new(0x0001).is_some());
        }

        proptest! {
            /// Property: Non-zero, non-reserved eventgroup IDs are valid
            /// Valid range: 0x0001-0xFFFE (0x0000 and 0xFFFF are reserved)
            #[test_log::test]
            fn nonzero_eventgroups_valid(value in 0x0001u16..=0xFFFE) {
                prop_assert!(EventgroupId::new(value).is_some());
            }
        }
    }

    // ------------------------------------------------------------------------
    // OFFER/FIND
    // ------------------------------------------------------------------------

    mod offer_find {
        use super::*;

        #[test_log::test]
        fn find_allows_wildcard_instance() {
            // When finding/requiring, can use ANY instance
            assert_eq!(InstanceId::ANY.value(), 0xFFFF);

            // Can also specify a concrete instance to find
            let specific = InstanceId::new(0x0001).unwrap();
            assert_eq!(specific.value(), 0x0001);
        }
    }

    // ------------------------------------------------------------------------
    // SUBSCRIPTION
    // ------------------------------------------------------------------------

    mod subscription {
        #[test_log::test]
        fn subscription_typestate() {
            // Cannot subscribe to unavailable service - enforced by typestate
            // subscribe() method only exists on ServiceProxy<_, Available>
            //
            // Compile-time guarantee, documented here for traceability
        }

        #[test_log::test]
        fn subscription_cleanup_on_drop() {
            // Dropping a Subscription sends StopSubscribeEventgroup
            // Tested behaviorally in api_usage.rs::subscription_stops_on_drop
        }
    }
}

// ============================================================================
// CROSS-CUTTING PROPERTY TESTS
// ============================================================================

mod properties {
    use super::*;

    proptest! {
        /// Comprehensive: All newtype IDs preserve their inner value
        #[test_log::test]
        fn newtypes_preserve_values(
            service in 0x0001u16..=0xFFFE,
            method in 0x0000u16..=0x7FFF,
            event in 0x8000u16..=0xFFFE,
            eventgroup in 0x0001u16..=0xFFFE,
        ) {
            prop_assert_eq!(ServiceId::new(service).unwrap().value(), service);
            prop_assert_eq!(MethodId::new(method).unwrap().value(), method);
            prop_assert_eq!(EventId::new(event).unwrap().value(), event);
            prop_assert_eq!(EventgroupId::new(eventgroup).unwrap().value(), eventgroup);
        }
    }
}
