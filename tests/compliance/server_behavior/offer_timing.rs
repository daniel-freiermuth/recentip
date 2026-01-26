//! Offer Timing Tests (Server Under Test)
//!
//! Tests that verify the timing of cyclic OfferService messages.
//!
//! # Test Setup
//! - Library: Acts as **server** (offers service, sends cyclic SD messages)
//! - Raw socket: Acts as **observer** (captures offer timestamps)
//!
//! # Requirements Covered
//! - feat_req_someipsd_78: Wait 1*CYCLIC_OFFER_DELAY before first offer in Main Phase
//! - feat_req_someipsd_79: Send offers cyclically in Main Phase
//! - feat_req_someipsd_81: Wait 1*CYCLIC_OFFER_DELAY between offers
//!
//! # Notes on SD Timing Phases
//!
//! The SOME/IP-SD spec defines three phases for service announcement:
//! 1. **Initial Wait Phase**: Random delay before first offer
//! 2. **Repetition Phase**: Exponential backoff for initial announcements
//! 3. **Main Phase**: Cyclic offers at fixed interval
//!
//! This test focuses on the **Main Phase** behavior (cyclic offers).
//! The first few intervals may vary due to Initial/Repetition phases,
//! so we're lenient on early intervals and strict on later ones.

use super::helpers::{covers, parse_sd_message, TEST_SERVICE_ID, TEST_SERVICE_VERSION};
#[cfg(feature = "slow-tests")]
use crate::helpers::configure_tracing;
#[cfg(feature = "slow-tests")]
use proptest::prelude::*;
use recentip::prelude::*;
use std::time::Duration;

// ============================================================================
// CYCLIC OFFER TIMING PROPERTY TEST
// ============================================================================

/// Strategy for generating cyclic offer delays.
///
/// Range: 200ms to 600ms (practical test range)
/// - Lower bound: 200ms (turmoil has high variance at smaller delays)
/// - Upper bound: 600ms (keeps simulation step count reasonable)
#[cfg(feature = "slow-tests")]
fn cyclic_delay_strategy() -> impl Strategy<Value = u64> {
    // Use log-uniform distribution to get good coverage of small and large values
    // 100ms to 10_000ms (10 seconds)
    prop_oneof![
        100u64..=500u64,                // Fast: 100-500ms
        500u64..=2000u64,               // Medium: 0.5-2s
        2000u64..=10_000u64,            // Slow: 2-10s
        10_000u64..=50_000u64,          // 10s - 50s
        50_000u64..=200_000u64,         // 50s - 200s
        200_000u64..=1_000_000u64,      // 200s - 1000s
        1_000_000u64..=5_000_000u64,    // 1000s - 5000s
        5_000_000u64..=20_000_000u64,   // 5000s - 20_000s = ~5.5 hours
        20_000_000u64..=100_000_000u64, // 20_000s - 100_000s = ~27.7 hours
    ]
}

#[cfg(feature = "slow-tests")]
proptest! {
    #![proptest_config(ProptestConfig {
        cases: 10,  // Limit cases since each involves simulated time
        timeout: 60_000, // 60 second timeout per case
        ..ProptestConfig::default()
    })]

    /// Property: Cyclic offers are sent at regular intervals in Main Phase.
    ///
    /// This test verifies:
    /// 1. First to second offer: between cyclic_delay/2 and 2*cyclic_delay
    ///    (accounts for initial/repetition phase variability)
    /// 2. Subsequent offers: within 1% of cyclic_delay
    ///
    /// Requirements covered:
    /// - feat_req_someipsd_78: Wait before first offer in Main Phase
    /// - feat_req_someipsd_79: Cyclic offers in Main Phase
    /// - feat_req_someipsd_81: Wait CYCLIC_OFFER_DELAY between offers
    #[test]
    fn cyclic_offers_have_correct_spacing(cyclic_delay_ms in cyclic_delay_strategy()) {
        covers!(feat_req_someipsd_78, feat_req_someipsd_79, feat_req_someipsd_81);

        configure_tracing();

        // TTL should be at least 4x cyclic delay to ensure offers don't expire
        let offer_ttl_secs = (cyclic_delay_ms * 4 / 1000).max(1) as u32;

        // Tick duration for ~1% precision (at least 1ms)
        let tick_us = cyclic_delay_ms.max(1);

        const NUMBER_OF_OFFERS: usize = 20;

        // Simulation duration: enough for ~6 offers + startup margin
        let sim_duration_ms = cyclic_delay_ms * (NUMBER_OF_OFFERS+1) as u64 + 2000;

        let mut sim = turmoil::Builder::new()
            .simulation_duration(Duration::from_millis(sim_duration_ms))
            .max_message_latency(Duration::from_millis(0))
            .tick_duration(Duration::from_micros(tick_us))
            .build();

        // Channel to collect offer timestamps from the observer
        let (tx, rx) = std::sync::mpsc::channel::<u64>();

        let delay = cyclic_delay_ms;
        let ttl = offer_ttl_secs;

        // Server: offers a service with configured cyclic delay
        sim.host("server", move || {
            async move {
                tokio::time::sleep(Duration::from_millis(cyclic_delay_ms)).await; // Startup delay
                let runtime = recentip::configure()
                    .cyclic_offer_delay(delay)
                    .offer_ttl(ttl)
                    .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
                    .start_turmoil()
                    .await
                    .unwrap();

                let _offering = runtime
                    .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
                    .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
                    .udp()
                    .start()
                    .await
                    .unwrap();

                // Keep running to send cyclic offers
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        });

        // Observer: raw socket capturing offer timestamps
        sim.client("observer", async move {
            let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
            sd_socket.join_multicast_v4(
                "239.255.0.1".parse().unwrap(),
                "0.0.0.0".parse().unwrap(),
            )?;

            let mut buf = [0u8; 1500];
            let start_time = tokio::time::Instant::now();

            // Collect offer timestamps
            for _ in 0..NUMBER_OF_OFFERS {
                let result = tokio::time::timeout(
                    Duration::from_millis(delay * 3),
                    sd_socket.recv_from(&mut buf),
                )
                .await;

                match result {
                    Ok(Ok((len, _from))) => {
                        if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                            for entry in &sd_msg.entries {
                                // Check for OfferService entry for our service
                                if entry.entry_type as u8 == 0x01
                                    && entry.service_id == TEST_SERVICE_ID
                                {
                                    let elapsed = start_time.elapsed().as_millis() as u64;
                                    let _ = tx.send(elapsed);
                                }
                            }
                        }
                    }
                    Err(_) => {
                        // Timeout - no more offers coming
                        break;
                    }
                    Ok(Err(e)) => {
                        eprintln!("Socket error: {:?}", e);
                        break;
                    }
                }
            }

            Ok(())
        });

        sim.run().unwrap();

        // Collect all timestamps
        let timestamps: Vec<u64> = rx.try_iter().collect();

        // We need at least 4 offers to verify timing
        prop_assert!(
            timestamps.len() >= 4,
            "Expected at least 4 offers, got {} (cyclic_delay={}ms)",
            timestamps.len(),
            delay
        );

        // Calculate intervals between consecutive offers
        let intervals: Vec<i64> = timestamps
            .windows(2)
            .map(|w| w[1] as i64 - w[0] as i64)
            .collect();

        // First interval (1st to 2nd offer): more lenient due to initial phase
        // Allow between cyclic_delay/2 and 2*cyclic_delay
        let first_interval = intervals[0];
        let min_first = delay as i64 / 2;
        let max_first = delay as i64 * 2;
        prop_assert!(
            first_interval >= min_first && first_interval <= max_first,
            "First interval {}ms not in range [{}, {}]ms (cyclic_delay={}ms)",
            first_interval,
            min_first,
            max_first,
            delay
        );

        // Subsequent intervals: must be within 1% of cyclic_delay
        let tolerance_percent = 0.01;
        let tolerance_ms = (delay as f64 * tolerance_percent) as i64;
        // Minimum tolerance of tick_duration to account for simulation granularity
        assert!(tick_us as i64 * 2/1000 <= tolerance_ms, "Tick duration too coarse for desired tolerance");
        let effective_tolerance = tolerance_ms;

        for (i, &interval) in intervals.iter().enumerate().skip(1) {
            let deviation = (interval - delay as i64).abs();
            prop_assert!(
                deviation <= effective_tolerance,
                "Interval {} ({}ms) deviates by {}ms from expected {}ms (tolerance {}ms, {}%={}ms)",
                i + 1,
                interval,
                deviation,
                delay,
                effective_tolerance,
                tolerance_percent * 100.0,
                tolerance_ms
            );
        }
    }
}

// ============================================================================
// BASIC OFFER TIMING TEST (non-proptest, for quick verification)
// ============================================================================

/// Basic test that cyclic offers are sent at the configured interval.
/// Uses a fixed 500ms cyclic delay for quick verification.
#[test_log::test]
fn cyclic_offers_basic_timing() {
    covers!(
        feat_req_someipsd_78,
        feat_req_someipsd_79,
        feat_req_someipsd_81
    );

    const CYCLIC_DELAY_MS: u64 = 500;
    const OFFER_TTL: u32 = 5; // 5 seconds

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .max_message_latency(Duration::from_millis(0))
        .build();

    use std::sync::{Arc, Mutex};
    let timestamps: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let timestamps_clone = Arc::clone(&timestamps);

    sim.host("server", move || async move {
        let runtime = recentip::configure()
            .cyclic_offer_delay(CYCLIC_DELAY_MS)
            .offer_ttl(OFFER_TTL)
            .advertised_ip(turmoil::lookup("server").to_string().parse().unwrap())
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(TEST_SERVICE_ID, InstanceId::Id(0x0001))
            .version(TEST_SERVICE_VERSION.0, TEST_SERVICE_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    sim.client("observer", async move {
        let sd_socket = turmoil::net::UdpSocket::bind("0.0.0.0:30490").await?;
        sd_socket.join_multicast_v4("239.255.0.1".parse().unwrap(), "0.0.0.0".parse().unwrap())?;

        let mut buf = [0u8; 1500];
        let start_time = tokio::time::Instant::now();

        for _ in 0..20 {
            let result = tokio::time::timeout(
                Duration::from_millis(CYCLIC_DELAY_MS * 3),
                sd_socket.recv_from(&mut buf),
            )
            .await;

            if let Ok(Ok((len, _))) = result {
                if let Some((_header, sd_msg)) = parse_sd_message(&buf[..len]) {
                    for entry in &sd_msg.entries {
                        if entry.entry_type as u8 == 0x01 && entry.service_id == TEST_SERVICE_ID {
                            let elapsed = start_time.elapsed().as_millis() as u64;
                            timestamps_clone.lock().unwrap().push(elapsed);
                        }
                    }
                }
            }
        }

        Ok(())
    });

    sim.run().unwrap();

    let ts = timestamps.lock().unwrap();
    assert!(
        ts.len() >= 4,
        "Expected at least 4 offers, got {}",
        ts.len()
    );

    // Calculate intervals
    let intervals: Vec<i64> = ts.windows(2).map(|w| w[1] as i64 - w[0] as i64).collect();

    // First interval: lenient (cyclic_delay/2 to 2*cyclic_delay)
    let first = intervals[0];
    assert!(
        first >= (CYCLIC_DELAY_MS as i64 / 2) && first <= (CYCLIC_DELAY_MS as i64 * 2),
        "First interval {}ms not in expected range",
        first
    );

    // Subsequent intervals: within 5% (more lenient for basic test)
    for (i, &interval) in intervals.iter().enumerate().skip(1) {
        let deviation = (interval - CYCLIC_DELAY_MS as i64).abs();
        let tolerance = 5;
        assert!(
            deviation <= tolerance,
            "Interval {} = {}ms, deviation {}ms exceeds tolerance {}ms",
            i + 1,
            interval,
            deviation,
            tolerance
        );
    }

    tracing::info!(
        "Verified {} offers with intervals: {:?}",
        ts.len(),
        intervals
    );
}
