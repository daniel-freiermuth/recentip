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
//! | `any_port_same_instance_different_major_use_separate_ports` | same service_id + instance_id, different major versions → server uses separate ports (no version in RPC header), client reuses one source port |
//! | `stop_offer_then_reoffer_new_instance_does_not_collide` | offer inst-1 + inst-2, stop inst-1, offer inst-3 → inst-3 must not collide with inst-2 |
//! | `offer_succeeds_when_auto_port_is_preoccupied` | ports 30491–65534 pre-bound + 65535 consumed by runtime’s ephemeral client-RPC socket → `offer()` wraps around and binds on port 1 |
//! | `server_fixed_port_two_servers_same_port` | two services on the **same** host both bind via `udp_port(N)` → socket is shared, both succeed, client discovers both at port N |
//! | `server_fixed_port_same_service_id_different_instance_fails` | same service_id, two instances, same `udp_port(N)` → second offer fails (`AddrInUse`; no instance_id in RPC header) |
//! | `server_fixed_port_same_service_id_different_major_fails` | same service_id, two major versions, same `udp_port(N)` → second offer fails (`AddrInUse`; no version in RPC header) |
//! | `server_fixed_tcp_port_two_services_same_host_share_socket` | two services on the **same** host both bind via `tcp_port(N)` → listener is shared, both succeed, client discovers both at port N |
//! | `server_fixed_tcp_port_same_service_id_different_instance_fails` | same service_id, two instances, same `tcp_port(N)` → second offer fails (`AddrInUse`; no instance_id in RPC header) |
//! | `server_fixed_tcp_port_same_service_id_different_major_fails` | same service_id, two major versions, same `tcp_port(N)` → second offer fails (`AddrInUse`; no version in RPC header) |
//! | `sub_any_port_two_servers_diff_major_shares_source_port` | 2 servers, same svc+instance, diff major → client reuses one sub socket (same source port for both subscribes) |
//! | `sub_any_port_two_servers_diff_instance_shares_source_port` | 2 servers, same svc_id, diff instance → client reuses one sub socket (same source port for both subscribes) |
//! | `sub_any_port_one_server_diff_major_shares_source_port` | 1 server, same svc+instance, diff major → client reuses one sub socket (same source port for both subscribes) |
//! | `sub_any_port_one_server_diff_instance_shares_source_port` | 1 server, same svc_id, diff instance → client reuses one sub socket (same source port for both subscribes) |
//! | `sub_any_port_one_server_diff_service_shares_source_port` | 1 server, diff service IDs, same instance → client reuses one sub socket (same source port for both subscribes) |
//! | `sub_any_port_same_service_diff_eventgroup_two_proxies` | 1 server, same svc, 2 proxies subscribe to diff eventgroups → dedicated sockets, different source ports |
//! | `sub_any_port_same_service_diff_eventgroup_same_proxy` | 1 server, same svc, 1 proxy subscribes to diff eventgroups separately → dedicated sockets, different source ports |
//! | `sub_fixed_port_two_servers_diff_major_shares_socket` | 2 servers, same svc+instance, diff major → both proxies request same explicit port, socket shared, both subscribes succeed |
//! | `sub_fixed_port_two_servers_diff_instance_shares_socket` | 2 servers, same svc, diff instance → both proxies request same explicit port, socket shared, both subscribes succeed |
//! | `sub_fixed_port_one_server_diff_major_shares_socket` | 1 server, same svc+instance, diff major → both proxies request same explicit port, socket shared, both subscribes succeed |
//! | `sub_fixed_port_one_server_diff_instance_shares_socket` | 1 server, same svc, diff instance → both proxies request same explicit port, socket shared, both subscribes succeed |
//! | `sub_fixed_port_one_server_diff_service_shares_socket` | 1 server, diff service IDs, same instance → both proxies request same explicit port, socket shared, both subscribes succeed |
//! | `sub_fixed_port_same_service_two_proxies_fails` | same svc, 2 proxies both request same fixed port → second `subscribe()` fails (`AddrInUse`; socket can't be shared within same service) |
//! | `sub_fixed_port_same_service_same_proxy_second_eg_fails` | same svc, 1 proxy requests same fixed port for two eventgroups sequentially → second `subscribe()` fails (`AddrInUse`) |
//! | `sub_multi_port_policy_two_proxies_uses_second_port` | same svc, 2 proxies, policy `[port A, port B]` → first proxy takes A, second proxy falls back to B, both succeed |
//! | `sub_multi_port_policy_same_proxy_uses_second_port` | same svc, 1 proxy, policy `[port A, port B]`, two eventgroups → first sub takes A, second sub falls back to B, both succeed |

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
const ANY_PORT_DIFF_MAJOR_SVC: u16 = 0x4018; // same service_id + instance_id, different major versions → separate ports
const SERVER_FIXED_PORT_SVC_A: u16 = 0x4019; // server-side fixed port: service A on server-a
const SERVER_FIXED_PORT_SVC_B: u16 = 0x401A; // server-side fixed port: service B on server-b (same port, different host)
const SERVER_FIXED_PORT_SAME_ID_SVC: u16 = 0x401B; // same service_id fixed port: two instances on same port → second must fail
const SERVER_FIXED_PORT_SAME_ID_DIFF_MAJOR_SVC: u16 = 0x401C; // same service_id fixed port: two major versions on same port → second must fail
const SERVER_FIXED_TCP_PORT_SVC_A: u16 = 0x401D; // server-side fixed TCP port: service A
const SERVER_FIXED_TCP_PORT_SVC_B: u16 = 0x401E; // server-side fixed TCP port: service B (same port, different service_id)
const SERVER_FIXED_TCP_PORT_SAME_ID_SVC: u16 = 0x401F; // same service_id fixed TCP port: two instances → second must fail
const SERVER_FIXED_TCP_PORT_SAME_ID_DIFF_MAJOR_SVC: u16 = 0x4020; // same service_id fixed TCP port: two major versions → second must fail
const SUB_ANY_TWO_SVRS_DIFF_MAJOR_SVC: u16 = 0x4021; // sub any-port: 2 servers, same svc+instance, diff major
const SUB_ANY_TWO_SVRS_DIFF_INST_SVC: u16 = 0x4022; // sub any-port: 2 servers, same svc_id, diff instance
const SUB_ANY_ONE_SVR_DIFF_MAJOR_SVC: u16 = 0x4023; // sub any-port: 1 server, same svc+instance, diff major
const SUB_ANY_ONE_SVR_DIFF_INST_SVC: u16 = 0x4024; // sub any-port: 1 server, same svc_id, diff instance
const SUB_ANY_ONE_SVR_DIFF_SVC_A: u16 = 0x4025; // sub any-port: 1 server, diff svc_id, service A
const SUB_ANY_ONE_SVR_DIFF_SVC_B: u16 = 0x4026; // sub any-port: 1 server, diff svc_id, service B
const SUB_ANY_SAME_SVC_DIFF_EG_TWO_PROXIES_SVC: u16 = 0x4027; // sub any-port: same svc, 2 proxies, diff eventgroups → different ports
const SUB_ANY_SAME_SVC_DIFF_EG_SAME_PROXY_SVC: u16 = 0x4028; // sub any-port: same svc, same proxy, diff eventgroups → different ports
const SUB_FIXED_TWO_SVRS_DIFF_MAJOR_SVC: u16 = 0x4029; // sub fixed-port: 2 servers, same svc+instance, diff major → socket shared
const SUB_FIXED_TWO_SVRS_DIFF_INST_SVC: u16 = 0x402A; // sub fixed-port: 2 servers, same svc, diff instance → socket shared
const SUB_FIXED_ONE_SVR_DIFF_MAJOR_SVC: u16 = 0x402B; // sub fixed-port: 1 server, same svc+instance, diff major → socket shared
const SUB_FIXED_ONE_SVR_DIFF_INST_SVC: u16 = 0x402C; // sub fixed-port: 1 server, same svc, diff instance → socket shared
const SUB_FIXED_ONE_SVR_DIFF_SVC_A: u16 = 0x402D; // sub fixed-port: 1 server, diff svc, service A → socket shared
const SUB_FIXED_ONE_SVR_DIFF_SVC_B: u16 = 0x402E; // sub fixed-port: 1 server, diff svc, service B → socket shared
const SUB_FIXED_SAME_SVC_TWO_PROXIES_SVC: u16 = 0x402F; // sub fixed-port: same svc, 2 proxies, same port → second fails
const SUB_FIXED_SAME_SVC_SAME_PROXY_SVC: u16 = 0x4030; // sub fixed-port: same svc, same proxy, same port, diff eg → second fails
const SUB_MULTI_PORT_TWO_PROXIES_SVC: u16 = 0x4031; // sub multi-port policy: same svc, 2 proxies → uses port A then B, both succeed
const SUB_MULTI_PORT_SAME_PROXY_SVC: u16 = 0x4032; // sub multi-port policy: same svc, same proxy, diff egs → uses port A then B, both succeed
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

/*
// -----------------------------------------------------------------------
// port_range_policy_subscribe_uses_port_in_range
// -----------------------------------------------------------------------

/// `TransportPreference::udp().with_port_range(A, B)` causes the client to bind
/// its UDP subscription socket to the first free port in `A..=B`. The server
/// observes a source port within that range via `ServiceEvent::Subscribe`.
///
/// Port binding is a property of the subscription socket, not the RPC socket,
/// so this test uses `subscribe()` to verify the feature end-to-end.
#[test_log::test]
fn port_range_policy_subscribe_uses_port_in_range() {
    use recentip::config::TransportPreference;
    use std::net::SocketAddrV4;

    const PORT_RANGE_START: u16 = 49300;
    const PORT_RANGE_END: u16 = 49310;

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
                .offer(PORT_RANGE_SVC, InstanceId::Id(1))
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

        let policy = TransportPolicy::new(vec![
            TransportPreference::udp().with_port_range(PORT_RANGE_START, PORT_RANGE_END),
        ]);

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .transport_policy(policy)
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(PORT_RANGE_SVC).instance(InstanceId::Id(1)),
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
    assert!(
        (PORT_RANGE_START..=PORT_RANGE_END).contains(&addr.port()),
        "server must see subscriber source port in {PORT_RANGE_START}..={PORT_RANGE_END}, got {addr}"
    );
} */

// -----------------------------------------------------------------------
// fixed_port_two_services_two_servers_different_ports
// -----------------------------------------------------------------------

/// Two independent services hosted on two separate servers.  Each is subscribed
/// with a **different** fixed port via a per-proxy `TransportPolicy` override.
///
/// Both subscriptions succeed; each server observes the expected client source
/// port in its `ServiceEvent::Subscribe`.
///
/// This models the common production pattern: nodes each have a known port for
/// receiving events, using `with_transport_policy()` to vary the port per proxy
/// while keeping a simple global default policy.
#[test_log::test]
fn fixed_port_two_services_two_servers_different_ports() {
    use recentip::config::TransportPreference;
    use std::net::SocketAddrV4;

    const PORT_A: u16 = 49501;
    const PORT_B: u16 = 49502;

    let port_a_seen: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let port_b_seen: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let capture_a = Arc::clone(&port_a_seen);
    let capture_b = Arc::clone(&port_b_seen);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server-a", move || {
        let capture = Arc::clone(&capture_a);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server-a")))
                .start_turmoil()
                .await
                .unwrap();

            let mut offering = runtime
                .offer(FIXED_DUAL_SVC_A, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *capture.lock().unwrap() = Some(client.address);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(())
        }
    });

    sim.host("server-b", move || {
        let capture = Arc::clone(&capture_b);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server-b")))
                .start_turmoil()
                .await
                .unwrap();

            let mut offering = runtime
                .offer(FIXED_DUAL_SVC_B, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *capture.lock().unwrap() = Some(client.address);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(())
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        // No global fixed port — use per-proxy overrides below
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        // Each proxy gets its own fixed port via with_transport_policy()
        let proxy_a = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(FIXED_DUAL_SVC_A).instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery_a timeout")
        .expect("discovery_a failed")
        .with_transport_policy(TransportPolicy::new(vec![
            TransportPreference::udp().with_port(PORT_A)
        ]));

        let proxy_b = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(FIXED_DUAL_SVC_B).instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery_b timeout")
        .expect("discovery_b failed")
        .with_transport_policy(TransportPolicy::new(vec![
            TransportPreference::udp().with_port(PORT_B)
        ]));

        let _sub_a = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_a.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe_a timeout")
        .expect("subscribe_a failed");

        let _sub_b = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_b.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe_b timeout")
        .expect("subscribe_b failed");

        Ok(())
    });

    sim.run().unwrap();

    let addr_a = port_a_seen
        .lock()
        .unwrap()
        .expect("server-a must have seen subscriber");
    let addr_b = port_b_seen
        .lock()
        .unwrap()
        .expect("server-b must have seen subscriber");
    assert_eq!(
        addr_a.port(),
        PORT_A,
        "server-a must see subscriber source port {PORT_A}, got {addr_a}"
    );
    assert_eq!(
        addr_b.port(),
        PORT_B,
        "server-b must see subscriber source port {PORT_B}, got {addr_b}"
    );
}

// -----------------------------------------------------------------------
// fixed_port_two_instances_different_ports
// -----------------------------------------------------------------------

/// The same service ID offered on two separate servers under **different
/// instance IDs**.  The client subscribes to each instance with its own
/// per-proxy fixed port.
///
/// Instance ID and Service ID together identify an endpoint; different
/// instances are independent streams.  Both subscriptions succeed and each
/// server observes the expected client source port.
#[test_log::test]
fn fixed_port_two_instances_different_ports() {
    use recentip::config::TransportPreference;
    use std::net::SocketAddrV4;

    const PORT_INST1: u16 = 49503;
    const PORT_INST2: u16 = 49504;

    let port1_seen: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let port2_seen: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let capture1 = Arc::clone(&port1_seen);
    let capture2 = Arc::clone(&port2_seen);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server-1", move || {
        let capture = Arc::clone(&capture1);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server-1")))
                .start_turmoil()
                .await
                .unwrap();

            let mut offering = runtime
                .offer(FIXED_INST_SVC, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *capture.lock().unwrap() = Some(client.address);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(())
        }
    });

    sim.host("server-2", move || {
        let capture = Arc::clone(&capture2);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server-2")))
                .start_turmoil()
                .await
                .unwrap();

            let mut offering = runtime
                .offer(FIXED_INST_SVC, InstanceId::Id(2))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *capture.lock().unwrap() = Some(client.address);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(())
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let proxy1 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(FIXED_INST_SVC).instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery instance-1 timeout")
        .expect("discovery instance-1 failed")
        .with_transport_policy(TransportPolicy::new(vec![
            TransportPreference::udp().with_port(PORT_INST1)
        ]));

        let proxy2 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(FIXED_INST_SVC).instance(InstanceId::Id(2)),
        )
        .await
        .expect("discovery instance-2 timeout")
        .expect("discovery instance-2 failed")
        .with_transport_policy(TransportPolicy::new(vec![
            TransportPreference::udp().with_port(PORT_INST2)
        ]));

        let _sub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy1.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe instance-1 timeout")
        .expect("subscribe instance-1 failed");

        let _sub2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy2.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe instance-2 timeout")
        .expect("subscribe instance-2 failed");

        Ok(())
    });

    sim.run().unwrap();

    let addr1 = port1_seen
        .lock()
        .unwrap()
        .expect("server-1 must have seen subscriber for instance-1");
    let addr2 = port2_seen
        .lock()
        .unwrap()
        .expect("server-2 must have seen subscriber for instance-2");
    assert_eq!(
        addr1.port(),
        PORT_INST1,
        "server-1 must see subscriber source port {PORT_INST1}, got {addr1}"
    );
    assert_eq!(
        addr2.port(),
        PORT_INST2,
        "server-2 must see subscriber source port {PORT_INST2}, got {addr2}"
    );
}

// -----------------------------------------------------------------------
// any_port_two_instances_same_host_share_one_udp_port
// -----------------------------------------------------------------------

/// Same service ID, two instance IDs, same major version — all offered by one
/// server host.
///
/// **Server-side port allocation**: the SOME/IP RPC header carries `service_id`
/// but **not** `instance_id`.  When multiple instances of the same service share
/// one UDP port the server cannot distinguish which instance an incoming request
/// is addressed to.  Therefore each instance must be bound to its own port;
/// the port itself acts as the instance discriminator.  This is correct,
/// spec-compliant behaviour — not a bug.
///
/// **Client-side assertion**: subscribing to both instances with the default
/// `PortSpec::Any` policy reuses the same client-side socket.  The server
/// therefore sees the *same source port* in the `ServiceEvent::Subscribe`
/// for both eventgroups, even though they land on different server-side ports.
#[test_log::test]
fn any_port_two_instances_same_host_share_one_udp_port() {
    use std::net::SocketAddrV4;

    let sub1_client_addr: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let sub2_client_addr: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let server_port_1: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));
    let server_port_2: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));
    let cap1 = Arc::clone(&sub1_client_addr);
    let cap2 = Arc::clone(&sub2_client_addr);
    let spc1 = Arc::clone(&server_port_1);
    let spc2 = Arc::clone(&server_port_2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let c1 = Arc::clone(&cap1);
        let c2 = Arc::clone(&cap2);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
                .start_turmoil()
                .await
                .unwrap();

            let mut offering1 = runtime
                .offer(ANY_PORT_SHARED_SVC, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            let mut offering2 = runtime
                .offer(ANY_PORT_SHARED_SVC, InstanceId::Id(2))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            // Capture Subscribe events from both instances, then wait for ACKs
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering1.next().await {
                *c1.lock().unwrap() = Some(client.address);
            }
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering2.next().await {
                *c2.lock().unwrap() = Some(client.address);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(200)).await;

        // No explicit port policy — PortSpec::Any, relies on socket sharing
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let proxy1 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(ANY_PORT_SHARED_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery instance-1 timeout")
        .expect("discovery instance-1 failed");

        let proxy2 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(ANY_PORT_SHARED_SVC)
                .instance(InstanceId::Id(2)),
        )
        .await
        .expect("discovery instance-2 timeout")
        .expect("discovery instance-2 failed");

        // Record the server-side UDP ports advertised for each instance
        *spc1.lock().unwrap() = Some(
            proxy1
                .endpoint()
                .expect("instance-1 must have UDP endpoint")
                .port(),
        );
        *spc2.lock().unwrap() = Some(
            proxy2
                .endpoint()
                .expect("instance-2 must have UDP endpoint")
                .port(),
        );

        let _sub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy1.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe instance-1 timeout")
        .expect("subscribe instance-1 failed");

        let _sub2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy2.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe instance-2 timeout")
        .expect("subscribe instance-2 failed");

        Ok(())
    });

    sim.run().unwrap();

    let addr1 = sub1_client_addr
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for instance-1");
    let addr2 = sub2_client_addr
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for instance-2");

    // --- server-side: same service ID → separate UDP ports per instance ---
    let port1 = server_port_1
        .lock()
        .unwrap()
        .expect("instance-1 server port recorded");
    let port2 = server_port_2
        .lock()
        .unwrap()
        .expect("instance-2 server port recorded");
    assert_ne!(
        port1, port2,
        "each instance of the same service must bind its own UDP port \
         (no instance_id in RPC header); got same port {port1} for both"
    );

    // --- client-side: one source port for both (PortSpec::Any sharing) ---
    assert_eq!(
        addr1.port(),
        addr2.port(),
        "client must use one source port for both subscriptions (PortSpec::Any sharing); \
         got {addr1} vs {addr2}"
    );
}

// -----------------------------------------------------------------------
// any_port_two_service_ids_same_host_share_one_port
// -----------------------------------------------------------------------

/// Two **different** service IDs offered by the same server host with `PortSpec::Any`.
///
/// Unlike same-service-id instances, different service IDs CAN share one
/// UDP socket — the `service_id` in the RPC header is sufficient to route
/// incoming requests unambiguously.  This test asserts that desired behaviour.
///
/// **Client-side assertion** (independent invariant): with `PortSpec::Any` the
/// client reuses one source socket for both subscriptions, so both servers see
/// the same subscriber source port.
#[test_log::test]
fn any_port_two_service_ids_same_host_share_one_port() {
    use std::net::SocketAddrV4;

    let sub_a_client_addr: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let sub_b_client_addr: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let server_port_a: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));
    let server_port_b: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));
    let cap_a = Arc::clone(&sub_a_client_addr);
    let cap_b = Arc::clone(&sub_b_client_addr);
    let spc_a = Arc::clone(&server_port_a);
    let spc_b = Arc::clone(&server_port_b);
    // Extra clones for use in client closure (originals needed after sim.run())
    let client_spc_a = Arc::clone(&server_port_a);
    let client_spc_b = Arc::clone(&server_port_b);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let ca = Arc::clone(&cap_a);
        let cb = Arc::clone(&cap_b);
        let spa = Arc::clone(&spc_a);
        let spb = Arc::clone(&spc_b);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
                .start_turmoil()
                .await
                .unwrap();

            let mut offering_a = runtime
                .offer(ANY_PORT_SVC_A, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            let mut offering_b = runtime
                .offer(ANY_PORT_SVC_B, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();

            // Capture the server-side UDP endpoint ports advertised for each service
            // (available from the OfferedService after offer() returns, but the
            // simplest observable proxy is the SD offer endpoint seen by the client —
            // we read it back from the proxy in the client task instead)

            if let Some(ServiceEvent::Subscribe { client, .. }) = offering_a.next().await {
                *ca.lock().unwrap() = Some(client.address);
            }
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering_b.next().await {
                *cb.lock().unwrap() = Some(client.address);
            }
            // placeholder — server ports are read from proxy.endpoint() in the client task
            let _ = (spa, spb);
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let proxy_a = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(ANY_PORT_SVC_A).instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery A timeout")
        .expect("discovery A failed");

        let proxy_b = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.find(ANY_PORT_SVC_B).instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery B timeout")
        .expect("discovery B failed");

        // Record which server-side ports were advertised for each service
        *client_spc_a.lock().unwrap() = Some(
            proxy_a
                .endpoint()
                .expect("service A must have UDP endpoint")
                .port(),
        );
        *client_spc_b.lock().unwrap() = Some(
            proxy_b
                .endpoint()
                .expect("service B must have UDP endpoint")
                .port(),
        );

        let _sub_a = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_a.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe A timeout")
        .expect("subscribe A failed");

        let _sub_b = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_b.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe B timeout")
        .expect("subscribe B failed");

        Ok(())
    });

    sim.run().unwrap();

    // --- server-side: different service_ids should share one UDP port ---
    // (currently fails: socket sharing not yet implemented; test is #[ignore]d)
    let port_a = server_port_a
        .lock()
        .unwrap()
        .expect("service A port recorded");
    let port_b = server_port_b
        .lock()
        .unwrap()
        .expect("service B port recorded");
    assert_eq!(
        port_a, port_b,
        "different service_ids must share one UDP socket (service_id in RPC header is \
         sufficient to route); got separate ports {port_a} and {port_b}"
    );

    // --- client-side: one source port for both (PortSpec::Any sharing) ---
    let addr_a = sub_a_client_addr
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for service A");
    let addr_b = sub_b_client_addr
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for service B");
    assert_eq!(
        addr_a.port(),
        addr_b.port(),
        "client must use one source port for both subscriptions (PortSpec::Any sharing); \
         got {addr_a} vs {addr_b}"
    );
}

// -----------------------------------------------------------------------
// any_port_same_instance_different_major_use_separate_ports
// -----------------------------------------------------------------------

/// Same service ID, same instance ID, **different major versions** — all offered
/// by one server host.
///
/// The SOME/IP RPC header carries `service_id` and `method_id` but **no
/// version information**.  Two offerings that share `(service_id, instance_id)`
/// but differ in major version cannot be demultiplexed by header content alone;
/// the port therefore acts as the version discriminator, and each major version
/// **must** bind to its own port.
///
/// **Client-side assertion**: with `PortSpec::Any` the client reuses one source
/// socket for both subscriptions, so both offerings see the same subscriber
/// source port.
#[test_log::test]
fn any_port_same_instance_different_major_use_separate_ports() {
    use std::net::SocketAddrV4;

    let sub_v1_client_addr: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let sub_v2_client_addr: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let server_port_v1: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));
    let server_port_v2: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));
    let cap_v1 = Arc::clone(&sub_v1_client_addr);
    let cap_v2 = Arc::clone(&sub_v2_client_addr);
    let spc_v1 = Arc::clone(&server_port_v1);
    let spc_v2 = Arc::clone(&server_port_v2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let c1 = Arc::clone(&cap_v1);
        let c2 = Arc::clone(&cap_v2);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
                .start_turmoil()
                .await
                .unwrap();

            let mut offering_v1 = runtime
                .offer(ANY_PORT_DIFF_MAJOR_SVC, InstanceId::Id(1))
                .version(1, 0)
                .udp()
                .start()
                .await
                .unwrap();

            let mut offering_v2 = runtime
                .offer(ANY_PORT_DIFF_MAJOR_SVC, InstanceId::Id(1))
                .version(2, 0)
                .udp()
                .start()
                .await
                .unwrap();

            // Capture Subscribe events from both version offerings
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering_v1.next().await {
                *c1.lock().unwrap() = Some(client.address);
            }
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering_v2.next().await {
                *c2.lock().unwrap() = Some(client.address);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(200)).await;

        // No explicit port policy — PortSpec::Any, relies on socket sharing
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let proxy_v1 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(ANY_PORT_DIFF_MAJOR_SVC)
                .instance(InstanceId::Id(1))
                .major_version(1u8),
        )
        .await
        .expect("discovery major-v1 timeout")
        .expect("discovery major-v1 failed");

        let proxy_v2 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(ANY_PORT_DIFF_MAJOR_SVC)
                .instance(InstanceId::Id(1))
                .major_version(2u8),
        )
        .await
        .expect("discovery major-v2 timeout")
        .expect("discovery major-v2 failed");

        // Record the server-side UDP ports advertised for each version
        *spc_v1.lock().unwrap() = Some(
            proxy_v1
                .endpoint()
                .expect("major-v1 must have UDP endpoint")
                .port(),
        );
        *spc_v2.lock().unwrap() = Some(
            proxy_v2
                .endpoint()
                .expect("major-v2 must have UDP endpoint")
                .port(),
        );

        let _sub_v1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_v1.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe major-v1 timeout")
        .expect("subscribe major-v1 failed");

        let _sub_v2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_v2.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe major-v2 timeout")
        .expect("subscribe major-v2 failed");

        Ok(())
    });

    sim.run().unwrap();

    // --- server-side: different major versions must bind separate UDP ports ---
    let port_v1 = server_port_v1
        .lock()
        .unwrap()
        .expect("major-v1 server port recorded");
    let port_v2 = server_port_v2
        .lock()
        .unwrap()
        .expect("major-v2 server port recorded");
    assert_ne!(
        port_v1, port_v2,
        "same (service_id, instance_id) with different major versions must use separate UDP \
         ports (no version in RPC header); got same port {port_v1} for both"
    );

    // --- client-side: one source port for both (PortSpec::Any sharing) ---
    let addr_v1 = sub_v1_client_addr
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for major-v1");
    let addr_v2 = sub_v2_client_addr
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for major-v2");
    assert_eq!(
        addr_v1.port(),
        addr_v2.port(),
        "client must use one source port for both subscriptions (PortSpec::Any sharing); \
         got {addr_v1} vs {addr_v2}"
    );
}

// -----------------------------------------------------------------------
// stop_offer_then_reoffer_new_instance_does_not_collide
// -----------------------------------------------------------------------

/// Regression test for the port-counter bug: `base_port` was previously derived
/// from `offered.len()`, which shrinks when a service is stopped.  After stopping
/// one of two already-running instances the counter dropped and the next `offer()`
/// for a new instance picked the same auto-assigned port as an existing instance,
/// causing the bind to fail with `AddrInUse`.
///
/// Fixed by replacing `offered.len()` with a monotonically increasing
/// `next_server_rpc_port` counter in `RuntimeState`.
///
/// Repro sequence:
/// 1. Offer instance 1 (counter=SD+1 → port SD+1).  counter→SD+3
/// 2. Offer instance 2 (counter=SD+3 → port SD+3).  counter→SD+5
/// 3. Stop instance 1.                               counter stays SD+5
/// 4. Offer instance 3 (counter=SD+5 → port SD+5).  No collision with inst-2!
#[test_log::test]
fn stop_offer_then_reoffer_new_instance_does_not_collide() {
    let mut sim = turmoil::Builder::new().build();

    sim.client("server", async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        // Step 1: offer instance 1 (len=0 → port SD+1)
        let offering1 = runtime
            .offer(STOP_REOFFER_SVC, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .udp()
            .start()
            .await
            .expect("offer instance-1 must succeed");

        // Step 2: offer instance 2 (len=1 → port SD+3)
        let _offering2 = runtime
            .offer(STOP_REOFFER_SVC, InstanceId::Id(2))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .udp()
            .start()
            .await
            .expect("offer instance-2 must succeed");

        // Step 3: stop instance 1 — offered.len() drops back to 1
        drop(offering1);

        // Yield to let the runtime process the StopOffer
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Step 4: offer instance 3 — must get a fresh port.
        // BUG: base_port = SD+1+1*2 = SD+3 → AddrInUse (inst-2 is still there)
        let _offering3 = runtime
            .offer(STOP_REOFFER_SVC, InstanceId::Id(3))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .udp()
            .start()
            .await
            .expect("offer instance-3 must succeed without port collision");

        tokio::time::sleep(Duration::from_secs(3)).await;
        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// offer_succeeds_when_auto_port_is_preoccupied
// -----------------------------------------------------------------------

/// Regression test: `next_server_rpc_port` starts at a fixed offset
/// (`SD_port + 1 = 30491`) and previously did NOT retry on `AddrInUse`.
///
/// Fixed: when `port == 0` (auto-assign), the runtime now scans forward
/// with wrap-around (`u16::MAX` → 1), skipping each `AddrInUse` port.
///
/// Repro:
/// 1. Pre-bind UDP ports 30491–65534 on the simulated host (`30491..u16::MAX`,
///    exclusive). This saturates the turmoil ephemeral port pool (49152–65534).
/// 2. Call `start_turmoil()`. Internally the runtime opens an ephemeral
///    client-RPC socket (`bind(ip, 0)`); turmoil scans 49152→65534 (all
///    occupied) and assigns port 65535 to that socket.
/// 3. Call `offer()` with auto-port (udp port 0). The runtime scans
///    30491→65534 (`AddrInUse` from step 1), then 65535 (`AddrInUse`
///    from step 2), wraps to port 1, and binds there.
///
/// Port ranges for auto-assign:
/// - 1–1023: privileged (requires root/CAP_NET_BIND_SERVICE)
/// - 1024–49151: registered/well-known ports
/// - IANA: 49152–65535
/// - Linux default: 32768–60999
///
#[test_log::test]
fn offer_succeeds_when_auto_port_is_preoccupied() {
    let mut sim = turmoil::Builder::new().build();

    sim.client("server", async {
        // Pre-bind 30491..65534 (30491..u16::MAX, exclusive).
        // This saturates turmoil's ephemeral pool (49152..65534), so when
        // start_turmoil() opens the client-RPC socket with bind(ip, 0),
        // turmoil auto-assigns port 65535.  With all of 30491..65535
        // consumed, the auto-port scan wraps around to port 1.
        let mut _guards: Vec<turmoil::net::UdpSocket> = Vec::new();
        for port in 30491u16..60000 {
            let sock = turmoil::net::UdpSocket::bind(format!("0.0.0.0:{port}"))
                .await
                .unwrap_or_else(|e| panic!("pre-bind port {port} failed: {e}"));
            _guards.push(sock);
        }

        // Now start the SOME/IP runtime on the same host.
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        // All ports 30491..65535 are now occupied (pre-binds + client-RPC socket);
        // the runtime wraps around and succeeds on port 1.
        let _offering = runtime
            .offer(PORT_PREOCCUPIED_SVC, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .udp()
            .start()
            .await
            .expect("offer must succeed: wrap-around finds port 1");

        let _another_sock = turmoil::net::UdpSocket::bind(format!("0.0.0.0:0"))
            .await
            .unwrap_or_else(|e| panic!("additional bind failed: {e}"));

        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// server_fixed_port_two_servers_same_port
// -----------------------------------------------------------------------

/// Two **different** service IDs offered by the **same** server host, both
/// binding their UDP endpoint to the **same** fixed port via `.udp_port(N)`.
///
/// Different service IDs can legitimately share one UDP socket because the
/// `service_id` field in the SOME/IP RPC message header is sufficient to route
/// incoming requests to the correct handler.  This mirrors the auto-port sharing
/// behaviour tested in `any_port_two_service_ids_same_host_share_one_port`, but
/// with an explicit port instead of an OS-assigned one.
///
/// This is representative of real deployments where every service on a node is
/// reachable on a single well-known port.
///
/// Assertions:
/// - Both `offer()` calls succeed.
/// - The client discovers both services and observes each at port N
///   (`proxy.endpoint().port() == N`).
#[test_log::test]
fn server_fixed_port_two_servers_same_port() {
    const PORT: u16 = 30501;

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

        let _offering_a = runtime
            .offer(SERVER_FIXED_PORT_SVC_A, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .udp_port(PORT)
            .start()
            .await
            .expect("offer service A on fixed port must succeed");

        // Same port, different service ID — the runtime must reuse the socket.
        let _offering_b = runtime
            .offer(SERVER_FIXED_PORT_SVC_B, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .udp_port(PORT)
            .start()
            .await
            .expect("offer service B on same fixed port must succeed: socket is shared");

        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let proxy_a = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SERVER_FIXED_PORT_SVC_A)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery A timeout")
        .expect("discovery A failed");

        let proxy_b = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SERVER_FIXED_PORT_SVC_B)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery B timeout")
        .expect("discovery B failed");

        let port_a = proxy_a
            .endpoint()
            .expect("service A must have UDP endpoint")
            .port();
        let port_b = proxy_b
            .endpoint()
            .expect("service B must have UDP endpoint")
            .port();

        assert_eq!(
            port_a, PORT,
            "service A must advertise port {PORT}, got {port_a}"
        );
        assert_eq!(
            port_b, PORT,
            "service B must advertise port {PORT}, got {port_b}"
        );

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// server_fixed_port_same_service_id_different_instance_fails
// -----------------------------------------------------------------------

/// Same service_id, two **different** instance IDs, same host, same fixed
/// `udp_port(N)`.
///
/// Unlike different service IDs, same-service-id instances CANNOT share a UDP
/// socket: the SOME/IP RPC message header contains `service_id` but NOT
/// `instance_id`, so there is no way to route an incoming request to the
/// correct instance without a distinct port per instance.
///
/// The second `offer()` must fail with `Error::Io(AddrInUse)`.
#[test_log::test]
fn server_fixed_port_same_service_id_different_instance_fails() {
    use recentip::Error;

    const PORT: u16 = 30502;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.client("server", async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        let _offering1 = runtime
            .offer(SERVER_FIXED_PORT_SAME_ID_SVC, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .udp_port(PORT)
            .start()
            .await
            .expect("first offer must succeed");

        // Same service_id + same port → socket cannot be shared (no instance_id in
        // the RPC header), so this must fail.
        let result = runtime
            .offer(SERVER_FIXED_PORT_SAME_ID_SVC, InstanceId::Id(2))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .udp_port(PORT)
            .start()
            .await;

        assert!(
            matches!(&result, Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::AddrInUse),
            "second offer of same service_id on same port must fail with AddrInUse; got: {:?}",
            result.as_ref().err()
        );

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// server_fixed_port_same_service_id_different_major_fails
// -----------------------------------------------------------------------

/// Same service_id, same host, same fixed `udp_port(N)`, but different **major
/// versions** (1 vs 2).
///
/// The runtime routes by `(service_id, port)`, not by version.  Two major
/// versions of the same service_id cannot share one UDP port for the same
/// reason instances cannot: the second offer must fail with `AddrInUse`.
#[test_log::test]
fn server_fixed_port_same_service_id_different_major_fails() {
    use recentip::Error;

    const PORT: u16 = 30503;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.client("server", async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        let _offering_v1 = runtime
            .offer(SERVER_FIXED_PORT_SAME_ID_DIFF_MAJOR_SVC, InstanceId::Id(1))
            .version(1, 0)
            .udp_port(PORT)
            .start()
            .await
            .expect("first offer (major v1) must succeed");

        // Same service_id + same port, different major version → still conflicts.
        let result = runtime
            .offer(SERVER_FIXED_PORT_SAME_ID_DIFF_MAJOR_SVC, InstanceId::Id(1))
            .version(2, 0)
            .udp_port(PORT)
            .start()
            .await;

        assert!(
            matches!(&result, Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::AddrInUse),
            "second offer of same service_id on same port must fail with AddrInUse; got: {:?}",
            result.as_ref().err()
        );

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// server_fixed_tcp_port_two_services_same_host_share_socket
// -----------------------------------------------------------------------

/// Two **different** service IDs on the same host both bind via `tcp_port(N)`.
///
/// SOME/IP RPC messages carry `service_id` in the header, so incoming TCP
/// connections can be routed to the correct service even when both listen on
/// the same port.  The runtime must reuse the existing TCP listener instead of
/// trying to bind a second one.
///
/// Assertions:
/// - Both `offer()` calls succeed.
/// - The client (prefer-TCP) discovers both services and observes each at
///   TCP port N (`proxy.endpoint().port() == N`).
#[test_log::test]
fn server_fixed_tcp_port_two_services_same_host_share_socket() {
    const PORT: u16 = 30511;

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

        let _offering_a = runtime
            .offer(SERVER_FIXED_TCP_PORT_SVC_A, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .tcp_port(PORT)
            .start()
            .await
            .expect("offer service A on fixed TCP port must succeed");

        // Same port, different service ID — the runtime must reuse the listener.
        let _offering_b = runtime
            .offer(SERVER_FIXED_TCP_PORT_SVC_B, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .tcp_port(PORT)
            .start()
            .await
            .expect("offer service B on same fixed TCP port must succeed: listener is shared");

        tokio::time::sleep(Duration::from_secs(10)).await;
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

        let proxy_a = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SERVER_FIXED_TCP_PORT_SVC_A)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery A timeout")
        .expect("discovery A failed");

        let proxy_b = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SERVER_FIXED_TCP_PORT_SVC_B)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery B timeout")
        .expect("discovery B failed");

        let port_a = proxy_a
            .endpoint()
            .expect("service A must have TCP endpoint")
            .port();
        let port_b = proxy_b
            .endpoint()
            .expect("service B must have TCP endpoint")
            .port();

        assert_eq!(
            port_a, PORT,
            "service A must advertise TCP port {PORT}, got {port_a}"
        );
        assert_eq!(
            port_b, PORT,
            "service B must advertise TCP port {PORT}, got {port_b}"
        );

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// server_fixed_tcp_port_same_service_id_different_instance_fails
// -----------------------------------------------------------------------

/// Same service_id, two **different** instance IDs, same host, same fixed
/// `tcp_port(N)`.
///
/// Unlike different service IDs, same-service-id instances CANNOT share a TCP
/// listener: the SOME/IP RPC message header contains `service_id` but NOT
/// `instance_id`, so there is no way to route an incoming request to the
/// correct instance without a distinct port per instance.
///
/// The second `offer()` must fail with `Error::Io(AddrInUse)`.
#[test_log::test]
fn server_fixed_tcp_port_same_service_id_different_instance_fails() {
    use recentip::Error;

    const PORT: u16 = 30512;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.client("server", async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        let _offering1 = runtime
            .offer(SERVER_FIXED_TCP_PORT_SAME_ID_SVC, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .tcp_port(PORT)
            .start()
            .await
            .expect("first offer must succeed");

        // Same service_id + same port → listener cannot be shared (no instance_id
        // in the RPC header), so this must fail.
        let result = runtime
            .offer(SERVER_FIXED_TCP_PORT_SAME_ID_SVC, InstanceId::Id(2))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .tcp_port(PORT)
            .start()
            .await;

        assert!(
            matches!(&result, Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::AddrInUse),
            "second offer of same service_id on same TCP port must fail with AddrInUse; got: {:?}",
            result.as_ref().err()
        );

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// server_fixed_tcp_port_same_service_id_different_major_fails
// -----------------------------------------------------------------------

/// Same service_id, same host, same fixed `tcp_port(N)`, but different **major
/// versions** (1 vs 2).
///
/// The runtime routes by `(service_id, port)`, not by version.  Two major
/// versions of the same service_id cannot share one TCP listener for the same
/// reason instances cannot: the second offer must fail with `AddrInUse`.
#[test_log::test]
fn server_fixed_tcp_port_same_service_id_different_major_fails() {
    use recentip::Error;

    const PORT: u16 = 30513;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(10))
        .build();

    sim.client("server", async {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();

        let _offering_v1 = runtime
            .offer(
                SERVER_FIXED_TCP_PORT_SAME_ID_DIFF_MAJOR_SVC,
                InstanceId::Id(1),
            )
            .version(1, 0)
            .tcp_port(PORT)
            .start()
            .await
            .expect("first offer (major v1) must succeed");

        // Same service_id + same port, different major version → still conflicts.
        let result = runtime
            .offer(
                SERVER_FIXED_TCP_PORT_SAME_ID_DIFF_MAJOR_SVC,
                InstanceId::Id(1),
            )
            .version(2, 0)
            .tcp_port(PORT)
            .start()
            .await;

        assert!(
            matches!(&result, Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::AddrInUse),
            "second offer of same service_id on same TCP port must fail with AddrInUse; got: {:?}",
            result.as_ref().err()
        );

        Ok(())
    });

    sim.run().unwrap();
}

// -----------------------------------------------------------------------
// sub_any_port_two_servers_diff_major_shares_source_port
// -----------------------------------------------------------------------

/// Two servers offer the same service ID and instance ID but **different major
/// versions** (major 1 on server-a, major 2 on server-b).  The client subscribes
/// to both with the default `PortSpec::Any`.
///
/// `PortSpec::Any` allows socket reuse across subscriptions, so the client must
/// send both Subscribe SD messages from the **same** source port.
#[test_log::test]
fn sub_any_port_two_servers_diff_major_shares_source_port() {
    use std::net::SocketAddrV4;

    let addr_v1: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let addr_v2: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let cap1 = Arc::clone(&addr_v1);
    let cap2 = Arc::clone(&addr_v2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server-a", move || {
        let cap = Arc::clone(&cap1);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server-a")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering = runtime
                .offer(SUB_ANY_TWO_SVRS_DIFF_MAJOR_SVC, InstanceId::Id(1))
                .version(1, 0)
                .udp()
                .start()
                .await
                .unwrap();
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *cap.lock().unwrap() = Some(client.address);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(())
        }
    });

    sim.host("server-b", move || {
        let cap = Arc::clone(&cap2);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server-b")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering = runtime
                .offer(SUB_ANY_TWO_SVRS_DIFF_MAJOR_SVC, InstanceId::Id(1))
                .version(2, 0)
                .udp()
                .start()
                .await
                .unwrap();
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *cap.lock().unwrap() = Some(client.address);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(())
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let proxy_v1 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_ANY_TWO_SVRS_DIFF_MAJOR_SVC)
                .instance(InstanceId::Id(1))
                .major_version(1u8),
        )
        .await
        .expect("discovery major-1 timeout")
        .expect("discovery major-1 failed");

        let proxy_v2 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_ANY_TWO_SVRS_DIFF_MAJOR_SVC)
                .instance(InstanceId::Id(1))
                .major_version(2u8),
        )
        .await
        .expect("discovery major-2 timeout")
        .expect("discovery major-2 failed");

        let _sub_v1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_v1.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe major-1 timeout")
        .expect("subscribe major-1 failed");

        let _sub_v2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_v2.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe major-2 timeout")
        .expect("subscribe major-2 failed");

        Ok(())
    });

    sim.run().unwrap();

    let a1 = addr_v1
        .lock()
        .unwrap()
        .expect("server-a must have seen Subscribe for major-1");
    let a2 = addr_v2
        .lock()
        .unwrap()
        .expect("server-b must have seen Subscribe for major-2");
    assert_eq!(
        a1.port(),
        a2.port(),
        "client must use the same UDP source port for both subscriptions (PortSpec::Any); \
         got {a1} for major-1 and {a2} for major-2"
    );
}

// -----------------------------------------------------------------------
// sub_any_port_two_servers_diff_instance_shares_source_port
// -----------------------------------------------------------------------

/// Two servers offer the same service ID and major version but **different
/// instance IDs** (instance 1 on server-a, instance 2 on server-b).  The
/// client subscribes to both with the default `PortSpec::Any`.
///
/// `PortSpec::Any` allows socket reuse across subscriptions, so the client must
/// send both Subscribe SD messages from the **same** source port.
#[test_log::test]
fn sub_any_port_two_servers_diff_instance_shares_source_port() {
    use std::net::SocketAddrV4;

    let addr_inst1: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let addr_inst2: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let cap1 = Arc::clone(&addr_inst1);
    let cap2 = Arc::clone(&addr_inst2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server-a", move || {
        let cap = Arc::clone(&cap1);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server-a")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering = runtime
                .offer(SUB_ANY_TWO_SVRS_DIFF_INST_SVC, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *cap.lock().unwrap() = Some(client.address);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(())
        }
    });

    sim.host("server-b", move || {
        let cap = Arc::clone(&cap2);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server-b")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering = runtime
                .offer(SUB_ANY_TWO_SVRS_DIFF_INST_SVC, InstanceId::Id(2))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *cap.lock().unwrap() = Some(client.address);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(())
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let proxy1 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_ANY_TWO_SVRS_DIFF_INST_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery instance-1 timeout")
        .expect("discovery instance-1 failed");

        let proxy2 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_ANY_TWO_SVRS_DIFF_INST_SVC)
                .instance(InstanceId::Id(2)),
        )
        .await
        .expect("discovery instance-2 timeout")
        .expect("discovery instance-2 failed");

        let _sub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy1.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe instance-1 timeout")
        .expect("subscribe instance-1 failed");

        let _sub2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy2.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe instance-2 timeout")
        .expect("subscribe instance-2 failed");

        Ok(())
    });

    sim.run().unwrap();

    let a1 = addr_inst1
        .lock()
        .unwrap()
        .expect("server-a must have seen Subscribe for instance-1");
    let a2 = addr_inst2
        .lock()
        .unwrap()
        .expect("server-b must have seen Subscribe for instance-2");
    assert_eq!(
        a1.port(),
        a2.port(),
        "client must use the same UDP source port for both subscriptions (PortSpec::Any); \
         got {a1} for instance-1 and {a2} for instance-2"
    );
}

// -----------------------------------------------------------------------
// sub_any_port_one_server_diff_major_shares_source_port
// -----------------------------------------------------------------------

/// One server offers the same service ID and instance ID under **two different
/// major versions** (major 1 and major 2).  The client subscribes to both with
/// the default `PortSpec::Any`.
///
/// `PortSpec::Any` allows socket reuse across subscriptions, so the client must
/// send both Subscribe SD messages from the **same** source port.
#[test_log::test]
fn sub_any_port_one_server_diff_major_shares_source_port() {
    use std::net::SocketAddrV4;

    let addr_v1: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let addr_v2: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let cap1 = Arc::clone(&addr_v1);
    let cap2 = Arc::clone(&addr_v2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let c1 = Arc::clone(&cap1);
        let c2 = Arc::clone(&cap2);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering_v1 = runtime
                .offer(SUB_ANY_ONE_SVR_DIFF_MAJOR_SVC, InstanceId::Id(1))
                .version(1, 0)
                .udp()
                .start()
                .await
                .unwrap();
            let mut offering_v2 = runtime
                .offer(SUB_ANY_ONE_SVR_DIFF_MAJOR_SVC, InstanceId::Id(1))
                .version(2, 0)
                .udp()
                .start()
                .await
                .unwrap();
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering_v1.next().await {
                *c1.lock().unwrap() = Some(client.address);
            }
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering_v2.next().await {
                *c2.lock().unwrap() = Some(client.address);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let proxy_v1 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_ANY_ONE_SVR_DIFF_MAJOR_SVC)
                .instance(InstanceId::Id(1))
                .major_version(1u8),
        )
        .await
        .expect("discovery major-1 timeout")
        .expect("discovery major-1 failed");

        let proxy_v2 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_ANY_ONE_SVR_DIFF_MAJOR_SVC)
                .instance(InstanceId::Id(1))
                .major_version(2u8),
        )
        .await
        .expect("discovery major-2 timeout")
        .expect("discovery major-2 failed");

        let _sub_v1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_v1.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe major-1 timeout")
        .expect("subscribe major-1 failed");

        let _sub_v2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_v2.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe major-2 timeout")
        .expect("subscribe major-2 failed");

        Ok(())
    });

    sim.run().unwrap();

    let a1 = addr_v1
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for major-1");
    let a2 = addr_v2
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for major-2");
    assert_eq!(
        a1.port(),
        a2.port(),
        "client must use the same UDP source port for both subscriptions (PortSpec::Any); \
         got {a1} for major-1 and {a2} for major-2"
    );
}

// -----------------------------------------------------------------------
// sub_any_port_one_server_diff_instance_shares_source_port
// -----------------------------------------------------------------------

/// One server offers the same service ID and major version under **two different
/// instance IDs** (instance 1 and instance 2).  The client subscribes to both
/// with the default `PortSpec::Any`.
///
/// `PortSpec::Any` allows socket reuse across subscriptions, so the client must
/// send both Subscribe SD messages from the **same** source port.
#[test_log::test]
fn sub_any_port_one_server_diff_instance_shares_source_port() {
    use std::net::SocketAddrV4;

    let addr_inst1: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let addr_inst2: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let cap1 = Arc::clone(&addr_inst1);
    let cap2 = Arc::clone(&addr_inst2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let c1 = Arc::clone(&cap1);
        let c2 = Arc::clone(&cap2);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering1 = runtime
                .offer(SUB_ANY_ONE_SVR_DIFF_INST_SVC, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            let mut offering2 = runtime
                .offer(SUB_ANY_ONE_SVR_DIFF_INST_SVC, InstanceId::Id(2))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering1.next().await {
                *c1.lock().unwrap() = Some(client.address);
            }
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering2.next().await {
                *c2.lock().unwrap() = Some(client.address);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let proxy1 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_ANY_ONE_SVR_DIFF_INST_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery instance-1 timeout")
        .expect("discovery instance-1 failed");

        let proxy2 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_ANY_ONE_SVR_DIFF_INST_SVC)
                .instance(InstanceId::Id(2)),
        )
        .await
        .expect("discovery instance-2 timeout")
        .expect("discovery instance-2 failed");

        let _sub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy1.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe instance-1 timeout")
        .expect("subscribe instance-1 failed");

        let _sub2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy2.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe instance-2 timeout")
        .expect("subscribe instance-2 failed");

        Ok(())
    });

    sim.run().unwrap();

    let a1 = addr_inst1
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for instance-1");
    let a2 = addr_inst2
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for instance-2");
    assert_eq!(
        a1.port(),
        a2.port(),
        "client must use the same UDP source port for both subscriptions (PortSpec::Any); \
         got {a1} for instance-1 and {a2} for instance-2"
    );
}

// -----------------------------------------------------------------------
// sub_any_port_one_server_diff_service_shares_source_port
// -----------------------------------------------------------------------

/// One server offers **two different service IDs** (service A and service B),
/// both with the same instance ID and major version.  The client subscribes to
/// both with the default `PortSpec::Any`.
///
/// `PortSpec::Any` allows socket reuse across subscriptions, so the client must
/// send both Subscribe SD messages from the **same** source port.
#[test_log::test]
fn sub_any_port_one_server_diff_service_shares_source_port() {
    use std::net::SocketAddrV4;

    let addr_svc_a: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let addr_svc_b: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let cap_a = Arc::clone(&addr_svc_a);
    let cap_b = Arc::clone(&addr_svc_b);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let ca = Arc::clone(&cap_a);
        let cb = Arc::clone(&cap_b);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering_a = runtime
                .offer(SUB_ANY_ONE_SVR_DIFF_SVC_A, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            let mut offering_b = runtime
                .offer(SUB_ANY_ONE_SVR_DIFF_SVC_B, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering_a.next().await {
                *ca.lock().unwrap() = Some(client.address);
            }
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering_b.next().await {
                *cb.lock().unwrap() = Some(client.address);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let proxy_a = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_ANY_ONE_SVR_DIFF_SVC_A)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery service-A timeout")
        .expect("discovery service-A failed");

        let proxy_b = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_ANY_ONE_SVR_DIFF_SVC_B)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery service-B timeout")
        .expect("discovery service-B failed");

        let _sub_a = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_a.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe service-A timeout")
        .expect("subscribe service-A failed");

        let _sub_b = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_b.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe service-B timeout")
        .expect("subscribe service-B failed");

        Ok(())
    });

    sim.run().unwrap();

    let a_a = addr_svc_a
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for service-A");
    let a_b = addr_svc_b
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for service-B");
    assert_eq!(
        a_a.port(),
        a_b.port(),
        "client must use the same UDP source port for both subscriptions (PortSpec::Any); \
         got {a_a} for service-A and {a_b} for service-B"
    );
}

// -----------------------------------------------------------------------
// sub_any_port_same_service_diff_eventgroup_two_proxies
// -----------------------------------------------------------------------

/// One server offers one service. The client discovers it **twice** (producing
/// two separate proxy handles) and subscribes each proxy to a different
/// eventgroup.
///
/// Both subscriptions target the same `(service_id, instance_id, major_version)`.
/// Because SOME/IP event notifications carry no eventgroup ID in the wire
/// header, sharing one socket would make proper event routing impossible; the
/// runtime must therefore create a **dedicated socket** for the second
/// subscription. The server must observe **different** source ports for the two
/// Subscribe SD messages.
#[test_log::test]
fn sub_any_port_same_service_diff_eventgroup_two_proxies() {
    use std::net::SocketAddrV4;

    let addr_eg1: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let addr_eg2: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let cap1 = Arc::clone(&addr_eg1);
    let cap2 = Arc::clone(&addr_eg2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let c1 = Arc::clone(&cap1);
        let c2 = Arc::clone(&cap2);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering = runtime
                .offer(SUB_ANY_SAME_SVC_DIFF_EG_TWO_PROXIES_SVC, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            // Two Subscribe events arrive from different dedicated sockets
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *c1.lock().unwrap() = Some(client.address);
            }
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *c2.lock().unwrap() = Some(client.address);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let proxy1 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_ANY_SAME_SVC_DIFF_EG_TWO_PROXIES_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery proxy-1 timeout")
        .expect("discovery proxy-1 failed");

        let proxy2 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_ANY_SAME_SVC_DIFF_EG_TWO_PROXIES_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery proxy-2 timeout")
        .expect("discovery proxy-2 failed");

        let _sub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy1.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe eg-1 timeout")
        .expect("subscribe eg-1 failed");

        let _sub2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy2.subscribe(EventgroupId::new(2).unwrap()),
        )
        .await
        .expect("subscribe eg-2 timeout")
        .expect("subscribe eg-2 failed");

        Ok(())
    });

    sim.run().unwrap();

    let a1 = addr_eg1
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for eg-1");
    let a2 = addr_eg2
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for eg-2");
    assert_ne!(
        a1.port(),
        a2.port(),
        "subscriptions to different eventgroups of the same service must use separate UDP source \
         ports (no eventgroup in wire header → sharing would mis-route events); \
         got {a1} for eg-1 and {a2} for eg-2"
    );
}

// -----------------------------------------------------------------------
// sub_any_port_same_service_diff_eventgroup_same_proxy
// -----------------------------------------------------------------------

/// One server offers one service. The client discovers it once and calls
/// `subscribe` **twice** on the same proxy — once for eventgroup 1, once for
/// eventgroup 2.
///
/// Because SOME/IP event notifications carry no eventgroup ID in the wire
/// header, sharing one socket would make proper event routing impossible; the
/// runtime must therefore create a **dedicated socket** for each subscription.
/// The server must observe **different** source ports for the two Subscribe SD
/// messages.
#[test_log::test]
fn sub_any_port_same_service_diff_eventgroup_same_proxy() {
    use std::net::SocketAddrV4;

    let addr_eg1: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let addr_eg2: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let cap1 = Arc::clone(&addr_eg1);
    let cap2 = Arc::clone(&addr_eg2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let c1 = Arc::clone(&cap1);
        let c2 = Arc::clone(&cap2);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering = runtime
                .offer(SUB_ANY_SAME_SVC_DIFF_EG_SAME_PROXY_SVC, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            // Two Subscribe events arrive from different dedicated sockets
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *c1.lock().unwrap() = Some(client.address);
            }
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *c2.lock().unwrap() = Some(client.address);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_ANY_SAME_SVC_DIFF_EG_SAME_PROXY_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed");

        let _sub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe eg-1 timeout")
        .expect("subscribe eg-1 failed");

        let _sub2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(EventgroupId::new(2).unwrap()),
        )
        .await
        .expect("subscribe eg-2 timeout")
        .expect("subscribe eg-2 failed");

        Ok(())
    });

    sim.run().unwrap();

    let a1 = addr_eg1
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for eg-1");
    let a2 = addr_eg2
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for eg-2");
    assert_ne!(
        a1.port(),
        a2.port(),
        "subscriptions to different eventgroups of the same service via one proxy must use \
         separate UDP source ports (no eventgroup in wire header → sharing would mis-route \
         events); got {a1} for eg-1 and {a2} for eg-2"
    );
}

// -----------------------------------------------------------------------
// sub_fixed_port_two_servers_diff_major_shares_socket
// -----------------------------------------------------------------------

/// Two servers offer the same service ID and instance ID but **different major
/// versions** (major 1 on server-a, major 2 on server-b).  The client
/// subscribes to both with an **explicit same port** via per-proxy
/// `with_transport_policy()`.
///
/// Because the two subscriptions target different (service_id, instance_id,
/// major_version) tuples, socket sharing is permitted; both Subscribe messages
/// must be sent from the same — explicitly requested — source port.
#[test_log::test]
fn sub_fixed_port_two_servers_diff_major_shares_socket() {
    use recentip::config::TransportPreference;
    use std::net::SocketAddrV4;

    const SHARED_PORT: u16 = 49601;

    let addr_v1: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let addr_v2: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let cap1 = Arc::clone(&addr_v1);
    let cap2 = Arc::clone(&addr_v2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server-a", move || {
        let cap = Arc::clone(&cap1);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server-a")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering = runtime
                .offer(SUB_FIXED_TWO_SVRS_DIFF_MAJOR_SVC, InstanceId::Id(1))
                .version(1, 0)
                .udp()
                .start()
                .await
                .unwrap();
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *cap.lock().unwrap() = Some(client.address);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(())
        }
    });

    sim.host("server-b", move || {
        let cap = Arc::clone(&cap2);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server-b")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering = runtime
                .offer(SUB_FIXED_TWO_SVRS_DIFF_MAJOR_SVC, InstanceId::Id(1))
                .version(2, 0)
                .udp()
                .start()
                .await
                .unwrap();
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *cap.lock().unwrap() = Some(client.address);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(())
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let policy = TransportPolicy::new(vec![TransportPreference::udp().with_port(SHARED_PORT)]);

        let proxy_v1 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_FIXED_TWO_SVRS_DIFF_MAJOR_SVC)
                .instance(InstanceId::Id(1))
                .major_version(1u8),
        )
        .await
        .expect("discovery major-1 timeout")
        .expect("discovery major-1 failed")
        .with_transport_policy(policy.clone());

        let proxy_v2 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_FIXED_TWO_SVRS_DIFF_MAJOR_SVC)
                .instance(InstanceId::Id(1))
                .major_version(2u8),
        )
        .await
        .expect("discovery major-2 timeout")
        .expect("discovery major-2 failed")
        .with_transport_policy(policy);

        let _sub_v1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_v1.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe major-1 timeout")
        .expect("subscribe major-1 failed");

        let _sub_v2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_v2.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe major-2 timeout")
        .expect("subscribe major-2 failed");

        Ok(())
    });

    sim.run().unwrap();

    let a1 = addr_v1
        .lock()
        .unwrap()
        .expect("server-a must have seen Subscribe for major-1");
    let a2 = addr_v2
        .lock()
        .unwrap()
        .expect("server-b must have seen Subscribe for major-2");
    assert_eq!(
        a1.port(),
        SHARED_PORT,
        "server-a must see client port {SHARED_PORT}, got {a1}"
    );
    assert_eq!(
        a2.port(),
        SHARED_PORT,
        "server-b must see client port {SHARED_PORT}, got {a2}"
    );
}

// -----------------------------------------------------------------------
// sub_fixed_port_two_servers_diff_instance_shares_socket
// -----------------------------------------------------------------------

/// Two servers offer the same service ID and major version but **different
/// instance IDs** (instance 1 on server-a, instance 2 on server-b).  The
/// client subscribes to both with the **same explicit source port**.
///
/// Different instances are independent tuples; socket sharing is permitted and
/// both Subscribe messages must arrive from the explicitly requested port.
#[test_log::test]
fn sub_fixed_port_two_servers_diff_instance_shares_socket() {
    use recentip::config::TransportPreference;
    use std::net::SocketAddrV4;

    const SHARED_PORT: u16 = 49602;

    let addr_inst1: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let addr_inst2: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let cap1 = Arc::clone(&addr_inst1);
    let cap2 = Arc::clone(&addr_inst2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server-a", move || {
        let cap = Arc::clone(&cap1);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server-a")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering = runtime
                .offer(SUB_FIXED_TWO_SVRS_DIFF_INST_SVC, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *cap.lock().unwrap() = Some(client.address);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(())
        }
    });

    sim.host("server-b", move || {
        let cap = Arc::clone(&cap2);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server-b")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering = runtime
                .offer(SUB_FIXED_TWO_SVRS_DIFF_INST_SVC, InstanceId::Id(2))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *cap.lock().unwrap() = Some(client.address);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(())
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let policy = TransportPolicy::new(vec![TransportPreference::udp().with_port(SHARED_PORT)]);

        let proxy1 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_FIXED_TWO_SVRS_DIFF_INST_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery instance-1 timeout")
        .expect("discovery instance-1 failed")
        .with_transport_policy(policy.clone());

        let proxy2 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_FIXED_TWO_SVRS_DIFF_INST_SVC)
                .instance(InstanceId::Id(2)),
        )
        .await
        .expect("discovery instance-2 timeout")
        .expect("discovery instance-2 failed")
        .with_transport_policy(policy);

        let _sub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy1.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe instance-1 timeout")
        .expect("subscribe instance-1 failed");

        let _sub2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy2.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe instance-2 timeout")
        .expect("subscribe instance-2 failed");

        Ok(())
    });

    sim.run().unwrap();

    let a1 = addr_inst1
        .lock()
        .unwrap()
        .expect("server-a must have seen Subscribe for instance-1");
    let a2 = addr_inst2
        .lock()
        .unwrap()
        .expect("server-b must have seen Subscribe for instance-2");
    assert_eq!(
        a1.port(),
        SHARED_PORT,
        "server-a must see client port {SHARED_PORT}, got {a1}"
    );
    assert_eq!(
        a2.port(),
        SHARED_PORT,
        "server-b must see client port {SHARED_PORT}, got {a2}"
    );
}

// -----------------------------------------------------------------------
// sub_fixed_port_one_server_diff_major_shares_socket
// -----------------------------------------------------------------------

/// One server offers the same service ID and instance ID at **two different
/// major versions**.  The client subscribes to both with the **same explicit
/// source port**.
///
/// Because the subscriptions target different major versions, their
/// `(service_id, instance_id, major_version)` tuples differ; socket sharing is
/// permitted and both Subscribe messages must arrive from the explicitly
/// requested port.
#[test_log::test]
fn sub_fixed_port_one_server_diff_major_shares_socket() {
    use recentip::config::TransportPreference;
    use std::net::SocketAddrV4;

    const SHARED_PORT: u16 = 49603;

    let addr_v1: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let addr_v2: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let cap1 = Arc::clone(&addr_v1);
    let cap2 = Arc::clone(&addr_v2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let c1 = Arc::clone(&cap1);
        let c2 = Arc::clone(&cap2);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering_v1 = runtime
                .offer(SUB_FIXED_ONE_SVR_DIFF_MAJOR_SVC, InstanceId::Id(1))
                .version(1, 0)
                .udp()
                .start()
                .await
                .unwrap();
            let mut offering_v2 = runtime
                .offer(SUB_FIXED_ONE_SVR_DIFF_MAJOR_SVC, InstanceId::Id(1))
                .version(2, 0)
                .udp()
                .start()
                .await
                .unwrap();
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering_v1.next().await {
                *c1.lock().unwrap() = Some(client.address);
            }
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering_v2.next().await {
                *c2.lock().unwrap() = Some(client.address);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let policy = TransportPolicy::new(vec![TransportPreference::udp().with_port(SHARED_PORT)]);

        let proxy_v1 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_FIXED_ONE_SVR_DIFF_MAJOR_SVC)
                .instance(InstanceId::Id(1))
                .major_version(1u8),
        )
        .await
        .expect("discovery major-1 timeout")
        .expect("discovery major-1 failed")
        .with_transport_policy(policy.clone());

        let proxy_v2 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_FIXED_ONE_SVR_DIFF_MAJOR_SVC)
                .instance(InstanceId::Id(1))
                .major_version(2u8),
        )
        .await
        .expect("discovery major-2 timeout")
        .expect("discovery major-2 failed")
        .with_transport_policy(policy);

        let _sub_v1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_v1.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe major-1 timeout")
        .expect("subscribe major-1 failed");

        let _sub_v2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_v2.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe major-2 timeout")
        .expect("subscribe major-2 failed");

        Ok(())
    });

    sim.run().unwrap();

    let a1 = addr_v1
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for major-1");
    let a2 = addr_v2
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for major-2");
    assert_eq!(
        a1.port(),
        SHARED_PORT,
        "must see client port {SHARED_PORT} for major-1, got {a1}"
    );
    assert_eq!(
        a2.port(),
        SHARED_PORT,
        "must see client port {SHARED_PORT} for major-2, got {a2}"
    );
}

// -----------------------------------------------------------------------
// sub_fixed_port_one_server_diff_instance_shares_socket
// -----------------------------------------------------------------------

/// One server offers the same service ID at **two different instance IDs**.
/// The client subscribes to both with the **same explicit source port**.
///
/// Different instances have independent `(service_id, instance_id)` tuples;
/// socket sharing is permitted and both Subscribe messages must arrive from the
/// explicitly requested port.
#[test_log::test]
fn sub_fixed_port_one_server_diff_instance_shares_socket() {
    use recentip::config::TransportPreference;
    use std::net::SocketAddrV4;

    const SHARED_PORT: u16 = 49604;

    let addr_inst1: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let addr_inst2: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let cap1 = Arc::clone(&addr_inst1);
    let cap2 = Arc::clone(&addr_inst2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let c1 = Arc::clone(&cap1);
        let c2 = Arc::clone(&cap2);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering1 = runtime
                .offer(SUB_FIXED_ONE_SVR_DIFF_INST_SVC, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            let mut offering2 = runtime
                .offer(SUB_FIXED_ONE_SVR_DIFF_INST_SVC, InstanceId::Id(2))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering1.next().await {
                *c1.lock().unwrap() = Some(client.address);
            }
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering2.next().await {
                *c2.lock().unwrap() = Some(client.address);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let policy = TransportPolicy::new(vec![TransportPreference::udp().with_port(SHARED_PORT)]);

        let proxy1 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_FIXED_ONE_SVR_DIFF_INST_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery instance-1 timeout")
        .expect("discovery instance-1 failed")
        .with_transport_policy(policy.clone());

        let proxy2 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_FIXED_ONE_SVR_DIFF_INST_SVC)
                .instance(InstanceId::Id(2)),
        )
        .await
        .expect("discovery instance-2 timeout")
        .expect("discovery instance-2 failed")
        .with_transport_policy(policy);

        let _sub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy1.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe instance-1 timeout")
        .expect("subscribe instance-1 failed");

        let _sub2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy2.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe instance-2 timeout")
        .expect("subscribe instance-2 failed");

        Ok(())
    });

    sim.run().unwrap();

    let a1 = addr_inst1
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for instance-1");
    let a2 = addr_inst2
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for instance-2");
    assert_eq!(
        a1.port(),
        SHARED_PORT,
        "must see client port {SHARED_PORT} for inst-1, got {a1}"
    );
    assert_eq!(
        a2.port(),
        SHARED_PORT,
        "must see client port {SHARED_PORT} for inst-2, got {a2}"
    );
}

// -----------------------------------------------------------------------
// sub_fixed_port_one_server_diff_service_shares_socket
// -----------------------------------------------------------------------

/// One server offers **two different service IDs**, same instance and major.
/// The client subscribes to both with the **same explicit source port**.
///
/// Different service IDs are fully independent; socket sharing is permitted and
/// both Subscribe messages must arrive from the explicitly requested port.
#[test_log::test]
fn sub_fixed_port_one_server_diff_service_shares_socket() {
    use recentip::config::TransportPreference;
    use std::net::SocketAddrV4;

    const SHARED_PORT: u16 = 49605;

    let addr_svc_a: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let addr_svc_b: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let cap_a = Arc::clone(&addr_svc_a);
    let cap_b = Arc::clone(&addr_svc_b);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let ca = Arc::clone(&cap_a);
        let cb = Arc::clone(&cap_b);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering_a = runtime
                .offer(SUB_FIXED_ONE_SVR_DIFF_SVC_A, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            let mut offering_b = runtime
                .offer(SUB_FIXED_ONE_SVR_DIFF_SVC_B, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering_a.next().await {
                *ca.lock().unwrap() = Some(client.address);
            }
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering_b.next().await {
                *cb.lock().unwrap() = Some(client.address);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        }
    });

    sim.client("client", async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let policy = TransportPolicy::new(vec![TransportPreference::udp().with_port(SHARED_PORT)]);

        let proxy_a = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_FIXED_ONE_SVR_DIFF_SVC_A)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery service-A timeout")
        .expect("discovery service-A failed")
        .with_transport_policy(policy.clone());

        let proxy_b = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_FIXED_ONE_SVR_DIFF_SVC_B)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery service-B timeout")
        .expect("discovery service-B failed")
        .with_transport_policy(policy);

        let _sub_a = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_a.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe service-A timeout")
        .expect("subscribe service-A failed");

        let _sub_b = tokio::time::timeout(
            Duration::from_secs(5),
            proxy_b.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe service-B timeout")
        .expect("subscribe service-B failed");

        Ok(())
    });

    sim.run().unwrap();

    let a_a = addr_svc_a
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for service-A");
    let a_b = addr_svc_b
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for service-B");
    assert_eq!(
        a_a.port(),
        SHARED_PORT,
        "must see client port {SHARED_PORT} for svc-A, got {a_a}"
    );
    assert_eq!(
        a_b.port(),
        SHARED_PORT,
        "must see client port {SHARED_PORT} for svc-B, got {a_b}"
    );
}

// -----------------------------------------------------------------------
// sub_fixed_port_same_service_two_proxies_fails
// -----------------------------------------------------------------------

/// One server offers one service.  The client creates **two proxies** to the
/// same `(service_id, instance_id, major_version)` and attempts to subscribe
/// both to different eventgroups using the **same explicit fixed port**.
///
/// Sharing a socket within the same service is illegal — the SOME/IP wire
/// format carries no eventgroup discriminator, so events would be mis-routed.
/// The runtime must therefore attempt to bind a second socket on port N, which
/// fails with `AddrInUse`.  The first subscription must succeed; the second
/// must return `Err(Error::Io(AddrInUse))`.
#[test_log::test]
fn sub_fixed_port_same_service_two_proxies_fails() {
    use recentip::config::TransportPreference;
    use recentip::Error;

    const PORT: u16 = 49610;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || async move {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();
        let mut offering = runtime
            .offer(SUB_FIXED_SAME_SVC_TWO_PROXIES_SVC, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();
        // Only one Subscribe will arrive (the second sub fails client-side)
        let _ = offering.next().await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    let sub2_result: Arc<Mutex<Option<bool>>> = Arc::new(Mutex::new(None));
    let cap = Arc::clone(&sub2_result);

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let policy = TransportPolicy::new(vec![TransportPreference::udp().with_port(PORT)]);

        let proxy1 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_FIXED_SAME_SVC_TWO_PROXIES_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery proxy-1 timeout")
        .expect("discovery proxy-1 failed")
        .with_transport_policy(policy.clone());

        let proxy2 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_FIXED_SAME_SVC_TWO_PROXIES_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery proxy-2 timeout")
        .expect("discovery proxy-2 failed")
        .with_transport_policy(policy);

        let _sub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy1.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe proxy-1 timeout")
        .expect("subscribe proxy-1 must succeed");

        let result2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy2.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe proxy-2 timeout");

        *cap.lock().unwrap() = Some(
            matches!(&result2, Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::AddrInUse),
        );
        Ok(())
    });

    sim.run().unwrap();

    assert!(
        sub2_result
            .lock()
            .unwrap()
            .expect("client must have captured result"),
        "second subscribe to the same service with the same fixed port must fail with AddrInUse"
    );
}

// -----------------------------------------------------------------------
// sub_fixed_port_same_service_same_proxy_second_eg_fails
// -----------------------------------------------------------------------

/// One server offers one service.  The client creates **one proxy** and
/// calls `subscribe` twice on it — once for eventgroup 1, once for eventgroup
/// 2 — both with the **same explicit fixed source port**.
///
/// The first subscription binds the socket to port N.  The second subscription
/// targets the same `(service_id, instance_id, major_version)` and would need
/// its own socket (no eventgroup in wire header), but port N is already bound.
/// The second `subscribe()` must return `Err(Error::Io(AddrInUse))`.
#[test_log::test]
fn sub_fixed_port_same_service_same_proxy_second_eg_fails() {
    use recentip::config::TransportPreference;
    use recentip::Error;

    const PORT: u16 = 49611;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || async move {
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
            .start_turmoil()
            .await
            .unwrap();
        let mut offering = runtime
            .offer(SUB_FIXED_SAME_SVC_SAME_PROXY_SVC, InstanceId::Id(1))
            .version(SVC_VERSION.0, SVC_VERSION.1)
            .udp()
            .start()
            .await
            .unwrap();
        // Only one Subscribe will arrive (the second sub fails client-side)
        let _ = offering.next().await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    });

    let sub2_result: Arc<Mutex<Option<bool>>> = Arc::new(Mutex::new(None));
    let cap = Arc::clone(&sub2_result);

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let policy = TransportPolicy::new(vec![TransportPreference::udp().with_port(PORT)]);

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_FIXED_SAME_SVC_SAME_PROXY_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed")
        .with_transport_policy(policy);

        let _sub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe eg-1 timeout")
        .expect("subscribe eg-1 must succeed");

        let result2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(EventgroupId::new(2).unwrap()),
        )
        .await
        .expect("subscribe eg-2 timeout");

        *cap.lock().unwrap() = Some(
            matches!(&result2, Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::AddrInUse),
        );
        Ok(())
    });

    sim.run().unwrap();

    assert!(
        sub2_result
            .lock()
            .unwrap()
            .expect("client must have captured result"),
        "second subscribe to the same service with the same fixed port must fail with AddrInUse"
    );
}

// -----------------------------------------------------------------------
// sub_multi_port_policy_two_proxies_uses_second_port
// -----------------------------------------------------------------------

/// One server offers one service.  **Two independent proxies** (sharing
/// the same client runtime) each carry a `TransportPolicy` with two
/// explicit UDP source ports `[PORT_A, PORT_B]`.
///
/// * **proxy-1** subscribes first → port A is free for this service → it
///   binds to `PORT_A`.
/// * **proxy-2** subscribes next → port A is already owned by this service
///   instance → the runtime falls back to `PORT_B` and binds there
///   successfully.
///
/// Both subscriptions must succeed and the server must observe the expected
/// source ports.
#[test_log::test]
fn sub_multi_port_policy_two_proxies_uses_second_port() {
    use recentip::config::TransportPreference;
    use std::net::SocketAddrV4;

    const PORT_A: u16 = 49620;
    const PORT_B: u16 = 49621;

    let addr1: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let addr2: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let cap1 = Arc::clone(&addr1);
    let cap2 = Arc::clone(&addr2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let c1 = Arc::clone(&cap1);
        let c2 = Arc::clone(&cap2);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering = runtime
                .offer(SUB_MULTI_PORT_TWO_PROXIES_SVC, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            // First subscription arrives from PORT_A.
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *c1.lock().unwrap() = Some(client.address);
            }
            // Second subscription arrives from PORT_B.
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *c2.lock().unwrap() = Some(client.address);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let policy = TransportPolicy::new(vec![
            TransportPreference::udp().with_port(PORT_A),
            TransportPreference::udp().with_port(PORT_B),
        ]);

        let proxy1 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_MULTI_PORT_TWO_PROXIES_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("proxy-1 discovery timeout")
        .expect("proxy-1 discovery failed")
        .with_transport_policy(policy.clone());

        let proxy2 = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_MULTI_PORT_TWO_PROXIES_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("proxy-2 discovery timeout")
        .expect("proxy-2 discovery failed")
        .with_transport_policy(policy);

        let _sub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy1.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe proxy-1 timeout")
        .expect("subscribe proxy-1 must succeed");

        let _sub2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy2.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe proxy-2 timeout")
        .expect("subscribe proxy-2 must succeed — should fall back to PORT_B");

        Ok(())
    });

    sim.run().unwrap();

    let a1 = addr1
        .lock()
        .unwrap()
        .expect("server must have seen first Subscribe");
    let a2 = addr2
        .lock()
        .unwrap()
        .expect("server must have seen second Subscribe");
    assert_eq!(
        a1.port(),
        PORT_A,
        "first subscriber must use PORT_A, got {a1}"
    );
    assert_eq!(
        a2.port(),
        PORT_B,
        "second subscriber must fall back to PORT_B, got {a2}"
    );
}

// -----------------------------------------------------------------------
// sub_multi_port_policy_same_proxy_uses_second_port
// -----------------------------------------------------------------------

/// One server offers one service with **two distinct eventgroups**.  A single
/// proxy carries a `TransportPolicy` with two explicit UDP source ports
/// `[PORT_A, PORT_B]`.
///
/// * **eventgroup 1** subscription → port A is free for this service → binds
///   to `PORT_A`.
/// * **eventgroup 2** subscription → port A is already owned by this service
///   instance → the runtime falls back to `PORT_B` and binds there
///   successfully.
///
/// Both subscriptions must succeed and the server must observe the expected
/// source ports.
#[test_log::test]
fn sub_multi_port_policy_same_proxy_uses_second_port() {
    use recentip::config::TransportPreference;
    use std::net::SocketAddrV4;

    const PORT_A: u16 = 49622;
    const PORT_B: u16 = 49623;

    let addr1: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let addr2: Arc<Mutex<Option<SocketAddrV4>>> = Arc::new(Mutex::new(None));
    let cap1 = Arc::clone(&addr1);
    let cap2 = Arc::clone(&addr2);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(30))
        .build();

    sim.host("server", move || {
        let c1 = Arc::clone(&cap1);
        let c2 = Arc::clone(&cap2);
        async move {
            let runtime = recentip::configure()
                .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
                .sd_unicast(crate::helpers::unicast(turmoil::lookup("server")))
                .start_turmoil()
                .await
                .unwrap();
            let mut offering = runtime
                .offer(SUB_MULTI_PORT_SAME_PROXY_SVC, InstanceId::Id(1))
                .version(SVC_VERSION.0, SVC_VERSION.1)
                .udp()
                .start()
                .await
                .unwrap();
            // Subscription for eg-1 arrives from PORT_A.
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *c1.lock().unwrap() = Some(client.address);
            }
            // Subscription for eg-2 arrives from PORT_B.
            if let Some(ServiceEvent::Subscribe { client, .. }) = offering.next().await {
                *c2.lock().unwrap() = Some(client.address);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(())
        }
    });

    sim.client("client", async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let runtime = recentip::configure()
            .sd_multicast_group(crate::helpers::DEFAULT_SD_MULTICAST)
            .sd_unicast(crate::helpers::unicast(turmoil::lookup("client")))
            .start_turmoil()
            .await
            .unwrap();

        let policy = TransportPolicy::new(vec![
            TransportPreference::udp().with_port(PORT_A),
            TransportPreference::udp().with_port(PORT_B),
        ]);

        let proxy = tokio::time::timeout(
            Duration::from_secs(5),
            runtime
                .find(SUB_MULTI_PORT_SAME_PROXY_SVC)
                .instance(InstanceId::Id(1)),
        )
        .await
        .expect("discovery timeout")
        .expect("discovery failed")
        .with_transport_policy(policy);

        let _sub1 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(EventgroupId::new(1).unwrap()),
        )
        .await
        .expect("subscribe eg-1 timeout")
        .expect("subscribe eg-1 must succeed");

        let _sub2 = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.subscribe(EventgroupId::new(2).unwrap()),
        )
        .await
        .expect("subscribe eg-2 timeout")
        .expect("subscribe eg-2 must succeed — should fall back to PORT_B");

        Ok(())
    });

    sim.run().unwrap();

    let a1 = addr1
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for eg-1");
    let a2 = addr2
        .lock()
        .unwrap()
        .expect("server must have seen Subscribe for eg-2");
    assert_eq!(
        a1.port(),
        PORT_A,
        "eg-1 subscriber must use PORT_A, got {a1}"
    );
    assert_eq!(
        a2.port(),
        PORT_B,
        "eg-2 subscriber must fall back to PORT_B, got {a2}"
    );
}
