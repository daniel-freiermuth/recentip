//! API Type Compliance Tests
//!
//! Tests that verify the type system enforces spec requirements.
//! Uses proptest for property-based validation of identifier ranges.

use crate::covers;
use proptest::prelude::*;
use recentip::prelude::*;

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

        /// feat_req_someip_538: A service shall be identified using the Service ID.
        /// feat_req_someip_539: Service IDs shall be of type 16 bit length unsigned integer.
        #[test_log::test]
        fn service_id_is_u16() {
            covers!(feat_req_someip_538, feat_req_someip_539);

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
                covers!(feat_req_someip_539);
                prop_assert!(ServiceId::new(value).is_some());
            }

            /// feat_req_someip_627: Service ID 0x0000 and 0xFFFF reserved
            #[test_log::test]
            fn service_id_reserved_rejected(value in prop::sample::select(vec![0x0000u16, 0xFFFF])) {
                covers!(feat_req_someip_627);
                prop_assert!(ServiceId::new(value).is_none());
            }
        }

        /// feat_req_someip_625: Methods and events identified by 16 bit Method ID
        /// Events use range 0x8000-0xFFFE (high bit set)
        #[test_log::test]
        fn method_event_id_distinction() {
            covers!(feat_req_someip_625);
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

        /// feat_req_someip_542: Service instance identified by Instance ID
        /// feat_req_someip_543: Instance IDs are uint16
        /// feat_req_someip_579: Instance IDs 0x0000 and 0xFFFF reserved
        #[test_log::test]
        fn instance_id_wildcard() {
            covers!(
                feat_req_someip_542,
                feat_req_someip_543,
                feat_req_someip_579
            );
            // 0xFFFF means "any instance" for client-side matching
            assert_eq!(InstanceId::ANY.value(), 0xFFFF);
        }
    }

    // ------------------------------------------------------------------------
    // RETURN CODES
    // ------------------------------------------------------------------------

    mod return_codes {
        use super::*;

        /// Protocol-defined return codes per feat_req_someip_371 (0x00-0x0A).
        const PROTOCOL_CODES: [(u8, ReturnCode); 11] = [
            (0x00, ReturnCode::Ok),
            (0x01, ReturnCode::NotOk),
            (0x02, ReturnCode::UnknownService),
            (0x03, ReturnCode::UnknownMethod),
            (0x04, ReturnCode::NotReady),
            (0x05, ReturnCode::NotReachable),
            (0x06, ReturnCode::Timeout),
            (0x07, ReturnCode::WrongProtocolVersion),
            (0x08, ReturnCode::WrongInterfaceVersion),
            (0x09, ReturnCode::MalformedMessage),
            (0x0A, ReturnCode::WrongMessageType),
        ];

        /// feat_req_someip_371: Each protocol-defined byte maps to its named
        /// return code and back, through both the inherent and `From` APIs.
        #[test_log::test]
        fn protocol_return_codes_map_to_spec_values() {
            covers!(feat_req_someip_371);

            for (byte, code) in PROTOCOL_CODES {
                assert_eq!(ReturnCode::from_u8(byte), code, "byte {byte:#04x}");
                assert_eq!(ReturnCode::from(byte), code, "byte {byte:#04x}");
                assert_eq!(code.as_u8(), byte, "{code:?}");
                assert_eq!(u8::from(code), byte, "{code:?}");
            }
        }

        /// feat_req_someip_371: 0x0B-0x1F and 0x40-0xFF are reserved,
        /// 0x20-0x3F are service-specific. Every byte is classified into the
        /// right range and keeps its exact value.
        #[test_log::test]
        fn non_protocol_return_codes_keep_range_and_value() {
            covers!(feat_req_someip_371);

            for byte in 0x0Bu8..=0xFF {
                let code = ReturnCode::from_u8(byte);
                match code {
                    ReturnCode::ServiceSpecific(c) => {
                        assert!((0x20..=0x3F).contains(&byte), "byte {byte:#04x}");
                        assert_eq!(c.value(), byte);
                    }
                    ReturnCode::Reserved(c) => {
                        assert!(!(0x20..=0x3F).contains(&byte), "byte {byte:#04x}");
                        assert_eq!(c.value(), byte);
                    }
                    other => panic!("byte {byte:#04x} classified as {other:?}"),
                }
                assert_eq!(u8::from(code), byte);
            }
        }

        /// feat_req_someip_371: Application errors are sent with their
        /// spec-defined return code.
        #[test_log::test]
        fn application_errors_use_spec_return_codes() {
            covers!(feat_req_someip_371);

            let service_specific = ApplicationError::service_specific(0x21).unwrap();
            let cases = [
                (ApplicationError::NotOk, 0x01),
                (ApplicationError::UnknownMethod, 0x03),
                (ApplicationError::MalformedMessage, 0x09),
                (service_specific, 0x21),
            ];
            for (error, byte) in cases {
                assert_eq!(error.as_u8(), byte, "{error:?}");
                assert_eq!(
                    ReturnCode::from(error),
                    ReturnCode::from_u8(byte),
                    "{error:?}"
                );
            }
        }
    }

    // ------------------------------------------------------------------------
    // REQUEST/RESPONSE SEMANTICS
    // ------------------------------------------------------------------------

    mod semantics {
        use super::*;

        #[test_log::test]
        fn fire_and_forget_returns_nothing() {
            covers!(feat_req_someip_15);
            // Fire&Forget: no response expected
            // The API enforces this: fire_and_forget() returns Result<()>
            // while call() returns Result<PendingResponse>
            // This is a type-system guarantee.
        }

        #[test_log::test]
        // TODO drop?
        fn responder_must_be_consumed() {
            covers!(feat_req_someip_15);
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

        /// feat_req_someipids_555: Eventgroup ID 0x0000 is reserved
        #[test_log::test]
        fn eventgroup_zero_reserved() {
            covers!(feat_req_someipids_555);
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
