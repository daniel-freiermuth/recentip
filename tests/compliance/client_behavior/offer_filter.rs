//! Offer Filtering Tests (Client Under Test)
//!
//! Tests that verify the client correctly filters and ignores malformed or
//! topologically invalid OfferService entries per the spec.
//!
//! # Test Setup
//! - Library: Acts as **client** (discovers and tries to subscribe)
//! - Raw socket: Acts as **wire server** (sends crafted malformed offers)
//!
//! # Requirements Covered
//! - feat_req_someipsd_1135: Ignore entries with topologically incorrect endpoint IPs

use super::helpers::{build_sd_offer_with_session, covers, parse_sd_message, TEST_SERVICE_ID};
use recentip::prelude::*;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// feat_req_someipsd_1135: Client ignores OfferService with unspecified endpoint IP (0.0.0.0)
///
/// A SOME/IP-SD implementation shall always check that IP Addresses received in
/// Endpoint options are topologically correct and shall ignore IP Addresses that
/// are not topologically correct as well as the entries referencing those options.
///
/// 0.0.0.0 (unspecified) is not a valid routable IP and fails the topological check.
///
/// # Setup
/// - Wire server sends SD offers using 0.0.0.0 as the UDP endpoint IP
/// - Library client tries to find and subscribe to the service
///
/// # Expected
/// - Client ignores the offers (no Subscribe sent back)
/// - `find()` times out — service is never considered reachable
#[test_log::test]
fn client_ignores_offer_with_unspecified_endpoint_ip() {
    covers!(feat_req_someipsd_1135);

    let subscribed = Arc::new(AtomicBool::new(false));
    let subscribed_server = Arc::clone(&subscribed);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(15))
        .build();

    // Wire server: sends offers with 0.0.0.0 as the endpoint IP, watches for Subscribes
    sim.host("wire_server", move || {
        let subscribed = Arc::clone(&subscribed_server);

        async move {
            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket
                .join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;
            let sd_multicast: SocketAddr = "239.255.0.1:30490".parse().unwrap();

            let mut buf = [0u8; 1500];
            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let mut last_offer = tokio::time::Instant::now() - Duration::from_secs(10);
            let mut session_id: u16 = 1;

            while tokio::time::Instant::now() < deadline {
                // Send offer with 0.0.0.0 as the UDP endpoint IP (invalid per spec)
                if last_offer.elapsed() >= Duration::from_millis(500) {
                    let invalid_offer = build_sd_offer_with_session(
                        TEST_SERVICE_ID,
                        0x0001,
                        1,                          // major_version
                        0,                          // minor_version
                        "0.0.0.0".parse().unwrap(), // INVALID endpoint IP
                        30509,
                        3600,
                        session_id,
                        session_id == 1, // reboot flag only on first offer
                        false,           // unicast flag (multicast offer)
                    );
                    session_id = session_id.wrapping_add(1);
                    sd_socket.send_to(&invalid_offer, sd_multicast).await?;
                    eprintln!(
                        "[wire_server] Sent invalid offer (0.0.0.0 endpoint), session={}",
                        session_id - 1
                    );
                    last_offer = tokio::time::Instant::now();
                }

                // Check if client sent a Subscribe (which it should NOT)
                if let Ok(Ok((len, from))) =
                    tokio::time::timeout(Duration::from_millis(50), sd_socket.recv_from(&mut buf))
                        .await
                {
                    if let Some((_hdr, sd_msg)) = parse_sd_message(&buf[..len]) {
                        for entry in &sd_msg.entries {
                            // SubscribeEventgroup = 0x06
                            if entry.entry_type as u8 == 0x06 && entry.service_id == TEST_SERVICE_ID
                            {
                                eprintln!(
                                    "[wire_server] UNEXPECTED Subscribe from {}: service={:04x}",
                                    from, entry.service_id
                                );
                                subscribed.store(true, Ordering::SeqCst);
                            }
                        }
                    }
                }
            }

            assert!(
                !subscribed.load(Ordering::SeqCst),
                "Client must NOT subscribe to an OfferService with 0.0.0.0 endpoint IP \
                 (topologically invalid per feat_req_someipsd_1135)"
            );
            Ok(())
        }
    });

    // Library client: tries to find the service — should time out
    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let runtime = recentip::configure()
            .advertised_ip(turmoil::lookup("client").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        // find() must not succeed: the offer with 0.0.0.0 should be ignored.
        // Acceptable outcomes:
        //   - Err(_) : tokio::timeout fired — service was never discovered
        //   - Ok(Err(..)) : find() exhausted repetitions returning NotAvailable
        // Unacceptable:
        //   - Ok(Ok(_)) : a proxy was returned — service was erroneously accepted
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(TEST_SERVICE_ID)
                .instance(InstanceId::Id(0x0001)),
        )
        .await;

        assert!(
            !matches!(result, Ok(Ok(_))),
            "Client must NOT discover a service whose OfferService uses 0.0.0.0 as endpoint IP \
             (topologically invalid per feat_req_someipsd_1135)"
        );

        Ok(())
    });

    sim.run().unwrap();
}
