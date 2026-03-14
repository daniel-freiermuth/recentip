//! Transport Policy API Behavior Tests
//!
//! Turmoil integration tests that verify [`TransportPolicy`] behavior at
//! runtime: which transport protocol is actually used for RPC calls,
//! that fallbacks work end-to-end, and that per-proxy policy overrides
//! take effect.
//!
//! Unit tests (selection logic without a network) live in
//! `src/config.rs` next to the implementation.
//!
//! # Test Summary
//!
//! | Test | Description |
//! |------|-------------|
//! | `prefer_tcp_uses_tcp_for_tcp_only_server` | Client with prefer-TCP connects via TCP to TCP-only server |
//! | `prefer_udp_uses_udp_for_udp_only_server` | Client with prefer-UDP connects via UDP to UDP-only server |
//! | `prefer_tcp_uses_tcp_when_both_offered` | Client prefers TCP when server offers both transports |
//! | `prefer_udp_uses_udp_when_both_offered` | Client prefers UDP when server offers both transports |
//! | `prefer_tcp_falls_back_to_udp_for_udp_only_server` | No TCP available → falls back to UDP |
//! | `prefer_udp_falls_back_to_tcp_for_tcp_only_server` | No UDP available → falls back to TCP |
//! | `per_proxy_policy_overrides_global_policy` | `with_transport_policy()` overrides global setting |
//! | `server_call_event_reflects_client_transport` | `ServiceEvent::Call` `client.transport` matches actual transport |
//! | `tcp_only_policy_fails_with_udp_only_server` | TCP-only policy + UDP-only server → `call()` returns `TransportMismatch` |
//! | `udp_only_policy_fails_with_tcp_only_server` | UDP-only policy + TCP-only server → `call()` returns `TransportMismatch` |
//! | `tcp_only_policy_fire_and_forget_fails_with_udp_only_server` | TCP-only policy + UDP-only server → `fire_and_forget()` returns `TransportMismatch` |
//! | `udp_only_policy_fire_and_forget_fails_with_tcp_only_server` | UDP-only policy + TCP-only server → `fire_and_forget()` returns `TransportMismatch` |
//! | `tcp_only_policy_subscribe_fails_with_udp_only_server` | TCP-only policy + UDP-only server → `subscribe()` returns `TransportMismatch` |
//! | `udp_only_policy_subscribe_fails_with_tcp_only_server` | UDP-only policy + TCP-only server → `subscribe()` returns `TransportMismatch` |
//! | `fixed_port_policy_subscribe_uses_specified_source_port` | `with_port(N)` → server sees subscriber source port N |
//! | `port_range_policy_subscribe_uses_port_in_range` | `with_port_range(A, B)` → server sees subscriber source port in `A..=B` |
//! | `fixed_port_shared_across_two_services` | same fixed port for two services → socket is shared, both succeed, server sees port from both |
//! | `fixed_port_two_services_two_servers_different_ports` | different per-proxy ports for two services on two servers → both succeed |
//! | `fixed_port_two_instances_different_ports` | same service ID, two instances, different per-proxy ports → both succeed |
//! | `any_port_two_instances_same_host_share_one_udp_port` | same service ID, two instances on one host → server uses separate UDP ports per instance (no instance_id in RPC header), client uses one source port for both |
//! | `any_port_two_service_ids_same_host_share_one_port` | two different service IDs on one host → share one UDP port (service_id in RPC header routes unambiguously), client also uses one source port for both |
//! | `stop_offer_then_reoffer_new_instance_does_not_collide` | offer inst-1 + inst-2, stop inst-1, offer inst-3 → inst-3 must not collide with inst-2 |
//! | `offer_succeeds_when_auto_port_is_preoccupied` | ports SD+1..SD+10 held by another process → `offer()` with auto-port scans forward and succeeds |

//! TODO
//! - fixed ports and ranges on TCP
//! - think about all possible combinations
//! - port range exhaustion

use recentip::handle::ServiceEvent;
use recentip::prelude::*;
use recentip::{Transport, TransportPolicy};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// Service ID constants — chosen to avoid conflicts with other test modules
const TCP_ONLY_SVC: u16 = 0x4001;
const UDP_ONLY_SVC: u16 = 0x4002;
const DUAL_SVC: u16 = 0x4003;
const OVERRIDE_SVC: u16 = 0x4004;
const TRANSPORT_REPORT_SVC: u16 = 0x4005;
const NO_MATCH_TCP_SVC: u16 = 0x4006; // TCP-only policy meets UDP-only server
const NO_MATCH_UDP_SVC: u16 = 0x4007; // UDP-only policy meets TCP-only server
const NO_MATCH_FF_TCP_SVC: u16 = 0x4008; // TCP-only policy meets UDP-only server, fire_and_forget
const NO_MATCH_FF_UDP_SVC: u16 = 0x4009; // UDP-only policy meets TCP-only server, fire_and_forget
const NO_MATCH_SUB_TCP_SVC: u16 = 0x400A; // TCP-only policy meets UDP-only server, subscribe
const NO_MATCH_SUB_UDP_SVC: u16 = 0x400B; // UDP-only policy meets TCP-only server, subscribe
const FIXED_PORT_SVC: u16 = 0x400C; // UDP with fixed local port
const PORT_RANGE_SVC: u16 = 0x400D; // UDP with local port range
const FIXED_PORT_CONFLICT_SVC_A: u16 = 0x400E; // port-conflict: first service
const FIXED_PORT_CONFLICT_SVC_B: u16 = 0x400F; // port-conflict: second service (same port)
const FIXED_DUAL_SVC_A: u16 = 0x4010; // two-server different-ports: service A
const FIXED_DUAL_SVC_B: u16 = 0x4011; // two-server different-ports: service B
const FIXED_INST_SVC: u16 = 0x4012; // two-instance different-ports: shared service ID
const ANY_PORT_SHARED_SVC: u16 = 0x4013; // any-port: two instances on one host share RPC socket
const ANY_PORT_SVC_A: u16 = 0x4014; // any-port multi-svc: service A (different service_ids, same host)
const ANY_PORT_SVC_B: u16 = 0x4015; // any-port multi-svc: service B (different service_ids, same host)
const STOP_REOFFER_SVC: u16 = 0x4016; // stop-then-re-offer port-counter bug
const PORT_PREOCCUPIED_SVC: u16 = 0x4017; // auto-port collides with externally held port
const SVC_VERSION: (u8, u32) = (1, 0);

// -----------------------------------------------------------------------
// prefer_tcp_uses_tcp_for_tcp_only_server
// -----------------------------------------------------------------------

/// Client configured with prefer-TCP uses TCP when the server offers TCP only.
#[test_log::test]
fn prefer_tcp_uses_tcp_for_tcp_only_server() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TCP_ONLY_SVC, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        while let Some(event) = offering.next().await {
            if let ServiceEvent::Call { responder, .. } = event {
                responder.reply(b"ok").unwrap();
            }
        }
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(TCP_ONLY_SVC).instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed");

        assert_eq!(
            proxy.transport().unwrap(),
            Transport::Tcp,
            "prefer-TCP client must use TCP for a TCP-only server"
        );

        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(1).unwrap(), b"ping"),
        )
        .await
        .expect("call timeout")
        .expect("call failed");
        assert_eq!(response.payload.as_ref(), b"ok");

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// prefer_udp_uses_udp_for_udp_only_server
// -----------------------------------------------------------------------

/// Client configured with prefer-UDP uses UDP when the server offers UDP only.
#[test_log::test]
fn prefer_udp_uses_udp_for_udp_only_server() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(UDP_ONLY_SVC, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        while let Some(event) = offering.next().await {
            if let ServiceEvent::Call { responder, .. } = event {
                responder.reply(b"ok").unwrap();
            }
        }
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .preferred_transport(Transport::Udp)
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(UDP_ONLY_SVC).instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed");

        assert_eq!(
            proxy.transport().unwrap(),
            Transport::Udp,
            "prefer-UDP client must use UDP for a UDP-only server"
        );

        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(1).unwrap(), b"ping"),
        )
        .await
        .expect("call timeout")
        .expect("call failed");
        assert_eq!(response.payload.as_ref(), b"ok");

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// prefer_tcp_uses_tcp_when_both_offered
// -----------------------------------------------------------------------

/// Client with prefer-TCP picks TCP when the server offers both transports.
#[test_log::test]
fn prefer_tcp_uses_tcp_when_both_offered() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(DUAL_SVC, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .tcp()
            .udp()
            .start()
            .await
            .unwrap();

        while let Some(event) = offering.next().await {
            if let ServiceEvent::Call { responder, .. } = event {
                responder.reply(b"dual").unwrap();
            }
        }
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(DUAL_SVC).instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed");

        assert_eq!(
            proxy.transport().unwrap(),
            Transport::Tcp,
            "prefer-TCP client must pick TCP from a dual-transport server"
        );

        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(1).unwrap(), b"ping"),
        )
        .await
        .expect("call timeout")
        .expect("call failed");
        assert_eq!(response.payload.as_ref(), b"dual");

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// prefer_udp_uses_udp_when_both_offered
// -----------------------------------------------------------------------

/// Client with prefer-UDP picks UDP when the server offers both transports.
#[test_log::test]
fn prefer_udp_uses_udp_when_both_offered() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(DUAL_SVC, InstanceId::Id(2))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .tcp()
            .udp()
            .start()
            .await
            .unwrap();

        while let Some(event) = offering.next().await {
            if let ServiceEvent::Call { responder, .. } = event {
                responder.reply(b"dual").unwrap();
            }
        }
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .preferred_transport(Transport::Udp)
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(DUAL_SVC).instance(InstanceId::Id(2)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed");

        assert_eq!(
            proxy.transport().unwrap(),
            Transport::Udp,
            "prefer-UDP client must pick UDP from a dual-transport server"
        );

        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(1).unwrap(), b"ping"),
        )
        .await
        .expect("call timeout")
        .expect("call failed");
        assert_eq!(response.payload.as_ref(), b"dual");

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// prefer_tcp_falls_back_to_udp_for_udp_only_server
// -----------------------------------------------------------------------

/// When the client prefers TCP but the server only offers UDP, the client
/// falls back to UDP (the second preference in the prefer-TCP policy).
#[test_log::test]
fn prefer_tcp_falls_back_to_udp_for_udp_only_server() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(UDP_ONLY_SVC, InstanceId::Id(2))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        while let Some(event) = offering.next().await {
            if let ServiceEvent::Call { responder, .. } = event {
                responder.reply(b"udp-fallback").unwrap();
            }
        }
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .preferred_transport(Transport::Tcp) // Prefer TCP, falls back to UDP
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(UDP_ONLY_SVC).instance(InstanceId::Id(2)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed");

        assert_eq!(
            proxy.transport().unwrap(),
            Transport::Udp,
            "prefer-TCP client must fall back to UDP when TCP is not available"
        );

        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(1).unwrap(), b"ping"),
        )
        .await
        .expect("call timeout")
        .expect("call failed");
        assert_eq!(response.payload.as_ref(), b"udp-fallback");

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// prefer_udp_falls_back_to_tcp_for_tcp_only_server
// -----------------------------------------------------------------------

/// When the client prefers UDP but the server only offers TCP, the client
/// falls back to TCP (the second preference in the prefer-UDP policy).
#[test_log::test]
fn prefer_udp_falls_back_to_tcp_for_tcp_only_server() {
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(TCP_ONLY_SVC, InstanceId::Id(2))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        while let Some(event) = offering.next().await {
            if let ServiceEvent::Call { responder, .. } = event {
                responder.reply(b"tcp-fallback").unwrap();
            }
        }
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .preferred_transport(Transport::Udp) // Prefer UDP, falls back to TCP
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(TCP_ONLY_SVC).instance(InstanceId::Id(2)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed");

        assert_eq!(
            proxy.transport().unwrap(),
            Transport::Tcp,
            "prefer-UDP client must fall back to TCP when UDP is not available"
        );

        let response = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(1).unwrap(), b"ping"),
        )
        .await
        .expect("call timeout")
        .expect("call failed");
        assert_eq!(response.payload.as_ref(), b"tcp-fallback");

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// per_proxy_policy_overrides_global_policy
// -----------------------------------------------------------------------

/// `proxy.with_transport_policy(...)` overrides the global runtime policy
/// for that specific proxy.
///
/// Setup:
///   - Server offers both TCP and UDP
///   - Global runtime policy: prefer-UDP
///   - Proxy A: inherits global policy → selects UDP
///   - Proxy B: overridden with prefer-TCP → selects TCP
#[test_log::test]
fn per_proxy_policy_overrides_global_policy() {
    let udp_transport = Arc::new(Mutex::new(Transport::Udp));
    let tcp_transport = Arc::new(Mutex::new(Transport::Udp));
    let udp_capture = Arc::clone(&udp_transport);
    let tcp_capture = Arc::clone(&tcp_transport);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        let mut offering = runtime
            .offer(OVERRIDE_SVC, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .tcp()
            .udp()
            .start()
            .await
            .unwrap();

        while let Some(event) = offering.next().await {
            if let ServiceEvent::Call { responder, .. } = event {
                responder.reply(b"ok").unwrap();
            }
        }
        Ok(())
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .preferred_transport(Transport::Udp)
            .start_turmoil()
            .await
            .unwrap();

        // Proxy A: inherits global prefer-UDP → should use UDP
        let proxy_a = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(OVERRIDE_SVC).instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed");

        *udp_capture.lock().unwrap() = proxy_a.transport().unwrap();

        // Proxy B: cloned, then overridden with prefer-TCP → should use TCP
        let proxy_b = proxy_a
            .clone()
            .with_transport_policy(TransportPolicy::prefer_tcp());

        *tcp_capture.lock().unwrap() = proxy_b.transport().unwrap();

        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_a.call(MethodId::new(1).unwrap(), b"a"),
        )
        .await
        .expect("proxy_a call timeout")
        .expect("proxy_a call failed");

        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_b.call(MethodId::new(1).unwrap(), b"b"),
        )
        .await
        .expect("proxy_b call timeout")
        .expect("proxy_b call failed");

        Ok(())
    });

    sim.run().unwrap();

    assert_eq!(
        *udp_transport.lock().unwrap(),
        Transport::Udp,
        "proxy with global prefer-UDP policy must use UDP"
    );
    assert_eq!(
        *tcp_transport.lock().unwrap(),
        Transport::Tcp,
        "proxy with overridden prefer-TCP policy must use TCP"
    );
}

// -----------------------------------------------------------------------
// server_call_event_reflects_client_transport
// -----------------------------------------------------------------------

/// The `client.transport` field in `ServiceEvent::Call` accurately reflects
/// the transport protocol used by the client for that specific call.
///
/// Two clients connect to the same dual-stack server:
///   - Client A uses TCP
///   - Client B uses UDP
///
/// The server sees the correct `client.transport` for each.
#[test_log::test]
fn server_call_event_reflects_client_transport() {
    let transports_seen: Arc<Mutex<Vec<Transport>>> = Arc::new(Mutex::new(Vec::new()));
    let server_capture = Arc::clone(&transports_seen);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let capture = Arc::clone(&server_capture);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
                .start_turmoil()
                .await
                .unwrap();

            let mut offering = runtime
                .offer(TRANSPORT_REPORT_SVC, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .tcp()
                .udp()
                .start()
                .await
                .unwrap();

            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            let mut calls_received = 0usize;

            while tokio::time::Instant::now() < deadline && calls_received < 2 {
                if let Ok(Some(event)) =
                    tokio::time::timeout(Duration::from_millis(200), offering.next()).await
                {
                    if let ServiceEvent::Call {
                        client, responder, ..
                    } = event
                    {
                        capture.lock().unwrap().push(client.transport);
                        responder.reply(b"ok").unwrap();
                        calls_received += 1;
                    }
                }
            }

            // Wait for in-flight TCP responses to be delivered before the runtime
            // drops and closes connections (mirrors the pattern used for sub ACKs).
            tokio::time::sleep(Duration::from_millis(500)).await;

            Ok(())
        }
    });

    sim.client("tcp_client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("tcp_client")))
            .preferred_transport(Transport::Tcp)
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(TRANSPORT_REPORT_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed");

        tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(1).unwrap(), b"from-tcp"),
        )
        .await
        .expect("call timeout")
        .expect("call failed");

        Ok(())
    });

    sim.client("udp_client", async {
        tokio::time::sleep(Duration::from_millis(300)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("udp_client")))
            .preferred_transport(Transport::Udp)
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(TRANSPORT_REPORT_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed");

        tokio::time::timeout(
            Duration::from_secs(5),
            proxy.call(MethodId::new(1).unwrap(), b"from-udp"),
        )
        .await
        .expect("call timeout")
        .expect("call failed");

        Ok(())
    });

    sim.run().unwrap();

    let seen = transports_seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "server must see exactly 2 calls");
    assert!(
        seen.contains(&Transport::Tcp),
        "server must see a TCP call; got: {:?}",
        *seen
    );
    assert!(
        seen.contains(&Transport::Udp),
        "server must see a UDP call; got: {:?}",
        *seen
    );
}

// -----------------------------------------------------------------------
// tcp_only_policy_fails_with_udp_only_server
// -----------------------------------------------------------------------

/// A strictly-TCP policy (no fallback entry) applied against a UDP-only server:
/// `call()` (and `fire_and_forget()` / `subscribe()`) must return
/// `Error::TransportMismatch` because `TransportPolicy::select()` yields `None`.
///
/// The proxy is still created successfully by `find()` — the error is
/// deferred to the first operation that requires a live transport.
#[test_log::test]
fn tcp_only_policy_fails_with_udp_only_server() {
    use recentip::config::TransportPreference;
    use recentip::Error;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        // Offer UDP only — no TCP endpoint
        let _offering = runtime
            .offer(NO_MATCH_TCP_SVC, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        // Keep server alive for the duration of the test
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        // TCP-only policy: one entry, no UDP fallback
        let tcp_only = TransportPolicy::new(vec![TransportPreference::tcp()]);

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .transport_policy(tcp_only)
            .start_turmoil()
            .await
            .unwrap();

        // Discovery succeeds — the server is there, just not via TCP
        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(NO_MATCH_TCP_SVC).instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed");

        // call() must fail immediately with TransportMismatch
        let result = proxy.call(MethodId::new(1).unwrap(), b"ping").await;
        assert!(
            matches!(result, Err(Error::TransportMismatch)),
            "expected TransportMismatch, got: {:?}",
            result
        );

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// udp_only_policy_fails_with_tcp_only_server
// -----------------------------------------------------------------------

/// A strictly-UDP policy (no fallback entry) applied against a TCP-only server:
/// `call()` must return `Error::TransportMismatch`.
///
/// Mirror of `tcp_only_policy_fails_with_udp_only_server`.
#[test_log::test]
fn udp_only_policy_fails_with_tcp_only_server() {
    use recentip::config::TransportPreference;
    use recentip::Error;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        // Offer TCP only — no UDP endpoint
        let _offering = runtime
            .offer(NO_MATCH_UDP_SVC, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        // Keep server alive for the duration of the test
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        // UDP-only policy: one entry, no TCP fallback
        let udp_only = TransportPolicy::new(vec![TransportPreference::udp()]);

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .transport_policy(udp_only)
            .start_turmoil()
            .await
            .unwrap();

        // Discovery succeeds — the server is there, just not via UDP
        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(NO_MATCH_UDP_SVC).instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed");

        // call() must fail immediately with TransportMismatch
        let result = proxy.call(MethodId::new(1).unwrap(), b"ping").await;
        assert!(
            matches!(result, Err(Error::TransportMismatch)),
            "expected TransportMismatch, got: {:?}",
            result
        );

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// tcp_only_policy_fire_and_forget_fails_with_udp_only_server
// -----------------------------------------------------------------------

/// A strictly-TCP policy (no fallback entry) applied against a UDP-only server:
/// `fire_and_forget()` must return `Error::TransportMismatch`.
///
/// Mirror of `tcp_only_policy_fails_with_udp_only_server` for fire-and-forget.
#[test_log::test]
fn tcp_only_policy_fire_and_forget_fails_with_udp_only_server() {
    use recentip::config::TransportPreference;
    use recentip::Error;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(NO_MATCH_FF_TCP_SVC, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let tcp_only = TransportPolicy::new(vec![TransportPreference::tcp()]);

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .transport_policy(tcp_only)
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(NO_MATCH_FF_TCP_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed");

        let result = proxy
            .fire_and_forget(MethodId::new(1).unwrap(), b"ping")
            .await;
        assert!(
            matches!(result, Err(Error::TransportMismatch)),
            "expected TransportMismatch, got: {:?}",
            result
        );

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// udp_only_policy_fire_and_forget_fails_with_tcp_only_server
// -----------------------------------------------------------------------

/// A strictly-UDP policy (no fallback entry) applied against a TCP-only server:
/// `fire_and_forget()` must return `Error::TransportMismatch`.
///
/// Mirror of `udp_only_policy_fails_with_tcp_only_server` for fire-and-forget.
#[test_log::test]
fn udp_only_policy_fire_and_forget_fails_with_tcp_only_server() {
    use recentip::config::TransportPreference;
    use recentip::Error;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(NO_MATCH_FF_UDP_SVC, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let udp_only = TransportPolicy::new(vec![TransportPreference::udp()]);

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .transport_policy(udp_only)
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(NO_MATCH_FF_UDP_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed");

        let result = proxy
            .fire_and_forget(MethodId::new(1).unwrap(), b"ping")
            .await;
        assert!(
            matches!(result, Err(Error::TransportMismatch)),
            "expected TransportMismatch, got: {:?}",
            result
        );

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// tcp_only_policy_subscribe_fails_with_udp_only_server
// -----------------------------------------------------------------------

/// A strictly-TCP policy (no fallback entry) applied against a UDP-only server:
/// `subscribe()` must return `Error::TransportMismatch`.
///
/// Mirror of `tcp_only_policy_fails_with_udp_only_server` for subscriptions.
#[test_log::test]
fn tcp_only_policy_subscribe_fails_with_udp_only_server() {
    use recentip::config::TransportPreference;
    use recentip::Error;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(NO_MATCH_SUB_TCP_SVC, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let tcp_only = TransportPolicy::new(vec![TransportPreference::tcp()]);

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .transport_policy(tcp_only)
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(NO_MATCH_SUB_TCP_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed");

        // subscribe() must fail immediately with TransportMismatch
        let result = proxy.subscribe(EventgroupId::new(1).unwrap()).await;
        assert!(
            matches!(&result, Err(Error::TransportMismatch)),
            "expected TransportMismatch, got: {:?}",
            result.err()
        );

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// udp_only_policy_subscribe_fails_with_tcp_only_server
// -----------------------------------------------------------------------

/// A strictly-UDP policy (no fallback entry) applied against a TCP-only server:
/// `subscribe()` must return `Error::TransportMismatch`.
///
/// Mirror of `udp_only_policy_fails_with_tcp_only_server` for subscriptions.
#[test_log::test]
fn udp_only_policy_subscribe_fails_with_tcp_only_server() {
    use recentip::config::TransportPreference;
    use recentip::Error;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", || async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        let _offering = runtime
            .offer(NO_MATCH_SUB_UDP_SVC, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .tcp()
            .start()
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let udp_only = TransportPolicy::new(vec![TransportPreference::udp()]);

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .transport_policy(udp_only)
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(NO_MATCH_SUB_UDP_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed");

        // subscribe() must fail immediately with TransportMismatch
        let result = proxy.subscribe(EventgroupId::new(1).unwrap()).await;
        assert!(
            matches!(&result, Err(Error::TransportMismatch)),
            "expected TransportMismatch, got: {:?}",
            result.err()
        );

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// fixed_port_policy_subscribe_uses_specified_source_port
// -----------------------------------------------------------------------

/// `TransportPreference::udp().with_port(N)` causes the client to bind its
/// UDP subscription socket to port N. The server observes this port in the
/// Subscribe SD message (reported as `client.address.port()` in
/// `ServiceEvent::Subscribe`).
///
/// Port binding is a property of the subscription socket, not the RPC socket,
/// so this test uses `subscribe()` to verify the feature end-to-end.
#[test_log::test]
fn fixed_port_policy_subscribe_uses_specified_source_port() {
    use recentip::config::TransportPreference;
    use std::net::SocketAddrV4;

    const CLIENT_PORT: u16 = 49200;

    let client_addr_seen: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let capture = Arc::clone(&client_addr_seen);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let capture = Arc::clone(&capture);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
                .start_turmoil()
                .await
                .unwrap();

            let mut offering = runtime
                .offer(FIXED_PORT_SVC, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            // Capture subscriber address from the Subscribe SD event
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *capture.lock().unwrap() = Some(client.address);
                // Wait for the queued SubscribeAck to be flushed (100ms cluster window)
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(())
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let policy = TransportPolicy::new(vec![TransportPreference::udp().with_port(CLIENT_PORT)]);

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .transport_policy(policy)
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(FIXED_PORT_SVC).instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed");

        let _sub = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe timeout")
        .expect("subscribe failed");

        Ok(())
    });

    sim.run().unwrap();

    let seen = client_addr_seen.lock().unwrap();
    let addr = seen.expect("server must have seen subscriber");
    assert_eq!(
        addr.port(),
        CLIENT_PORT,
        "server must see subscriber source port {CLIENT_PORT}, got {addr}"
    );
}
