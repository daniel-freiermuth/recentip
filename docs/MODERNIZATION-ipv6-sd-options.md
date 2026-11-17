# IPv6 SOME/IP-SD support (Release 26-11_v12)

Status: **RFC — needs a scope decision before implementation.**

## Why this exists

Open SOME/IP Release 26-11_v12 (upstream `some-ip-com/open-someip-spec`
`f517e5d633`, 2026-11-13; submodule now pinned at `5acd22e590`) moved 27 IPv6
SD-option items from `Information` to `Requirement`. Before that release an
IPv4-only stack could call itself complete; after it, these items count
against the traceability report:

| Option | Type | Requirements now normative |
|---|---|---|
| IPv6 Endpoint Option | 0x06 | `feat_req_someipsd_140`, `142`, `163`, `164`, `197`, `751`, `851`, `852`, `853` |
| IPv6 Multicast Option | 0x16 | `feat_req_someipsd_737`, `738`, `739`, `747`, `748`, `856`, `1083` |
| IPv6 SD Endpoint Option | 0x26 | `feat_req_someipsd_1099`–`1103`, `1111`–`1113`, `1153`–`1155` |

(The 28th re-typed item, `feat_req_someipsd_752`, is the IPv4 Endpoint Option.
`feat_req_someipsd_1102` says "IPv4-Address" in the IPv6 SD Endpoint section;
that is an upstream typo, the format in `1103` is IPv6.)

Practical impact: a peer that offers a service only via an IPv6 Endpoint Option
is not discoverable. Its option is parsed as `SdOption::Unknown`, so the offer
has no usable endpoint.

## Current state (measured on this branch)

- `src/wire.rs`: `SdOption` has `Ipv4Endpoint` (0x04), `Ipv4Multicast` (0x14)
  and `Unknown`. `SdMessage::get_udp_endpoint`/`get_tcp_endpoint` return
  `SocketAddrV4`. The IPv4 SD Endpoint Option (0x24) is not modelled either.
- `src/net/mod.rs`: the `UdpSocket`, `TcpStream` and `TcpListener` traits use
  `SocketAddrV4`/`Ipv4Addr` in every address-bearing method (`bind`,
  `send_to`, `recv_from`, `join_multicast_v4`, `leave_multicast_v4`,
  `set_multicast_if_v4`, `connect`, `connect_from`, `accept`, `local_addr`,
  `peer_addr`).
- `src/net/tokio_impl.rs` maps any `SocketAddr::V6` to `0.0.0.0:0` and logs
  it as a "BUG"; `connect_from` uses `TcpSocket::new_v4`.
  `src/net/turmoil_impl.rs` has six `SocketAddr::V6(_)` rejection arms.
- `git grep -o -E "SocketAddrV4|Ipv4Addr" -- src | wc -l` → 331 occurrences,
  concentrated in `config.rs` (40 lines), `tcp.rs` (30), `runtime/event_loop.rs`
  (29), `runtime/state.rs` (28), `wire.rs` (24), `runtime/sd.rs` (19),
  `runtime/client.rs` (18), the two net impls (18 + 17) and `net/mod.rs` (16).
- Public API exposing IPv4 types: `MulticastAddress(Ipv4Addr)` and
  `UnicastAddress(Ipv4Addr)` (`src/config.rs`),
  `TransportSelection::remote_endpoint: SocketAddrV4` (`src/config.rs:414`),
  `ClientInfo::address: SocketAddrV4` (`src/lib.rs:672`), and
  `OfferedService::endpoint() -> Option<SocketAddrV4>`
  (`src/handles/client/proxy.rs:347`).

## Target state

1. `SdOption` gains `Ipv6Endpoint { addr: Ipv6Addr, port, protocol }` (0x06,
   length 0x0015), `Ipv6Multicast { addr, port }` (0x16) and
   `Ipv6SdEndpoint { addr, port }` (0x26); ideally also `Ipv4SdEndpoint` (0x24)
   so both families share one SD-endpoint code path.
2. Endpoint lookups on `SdMessage` return `SocketAddr`.
3. The `net` traits take and return `SocketAddr`; multicast becomes
   `join_multicast(&self, group: IpAddr, interface: Interface)` (v4 by address,
   v6 by interface index), mirroring tokio/turmoil, which both provide
   `join_multicast_v6(&Ipv6Addr, u32)`.
4. Config: `MulticastAddress`/`UnicastAddress` wrap `IpAddr`, validated to be
   the same family as each other (one runtime = one address family, as the spec
   ties the SD Endpoint Option family to the transport, `feat_req_someipsd_1155`).
5. Runtime state keyed by `SocketAddr`; offers emit IPv6 options when the
   runtime runs on IPv6; received IPv6 options are honoured with the same
   topological checks as IPv4 (`feat_req_someipsd_1135`).

## Migration steps (each one a separately green commit)

1. **Wire only, additive.** Add the three IPv6 `SdOption` variants with parse
   and serialize, round-trip and proptest coverage, and length validation
   (0x0015). No runtime change; `covers!` for 140/163/164/737/738/739/1101/1103.
   Non-breaking unless downstream code matches `SdOption` exhaustively (it is
   not `#[non_exhaustive]`; consider adding that in the same release).
2. **Net traits → `SocketAddr`.** Change `net/mod.rs` and both impls
   (`tokio_impl.rs`, `turmoil_impl.rs`). The tokio impl creates v4/v6 sockets
   by family; the "BUG" fallback is removed. Breaking for anyone implementing
   the traits themselves.
3. **Config and public handles → `IpAddr`/`SocketAddr`.** `MulticastAddress`,
   `UnicastAddress`, `TransportSelection`, `ClientInfo` and
   `OfferedService::endpoint()`.
   Breaking for every user who constructs a config with `Ipv4Addr` literals
   (mitigation: `From<Ipv4Addr>` impls keep most call sites compiling).
4. **Runtime state and SD logic.** `runtime/{state,sd,client,server,event_loop}`
   and `tcp.rs` move to `SocketAddr`; offer/subscribe paths pick the option
   family from the runtime's family; SD Endpoint Option first-in-array and
   at-most-once rules (1099, 1153, 1154).
5. **Tests.** turmoil `Builder::ip_version(IpVersion::V6)` (available in
   turmoil 0.7.1) to run the existing compliance suite once per family where
   meaningful; new IPv6-only compliance tests for the 27 requirements.
6. **QNX.** `.notes-to-the-agent/qnx.md`: socket2 0.6.5 does not compile
   `IPV6_ADD_MEMBERSHIP` on the QNX SDP 8.0 targets (`target_os = "qnx"`).
   The CI QNX check covers the QNX 7.1 targets (`*-nto-qnx710`), so it needs
   re-running after step 2; v6 multicast may have to be `cfg`-gated per target.

## What breaks, for whom

- Library users: config builder inputs, `ClientInfo`, `TransportSelection` and
  the `OfferedService::endpoint()` return type (step 3); anyone matching
  `SdOption` exhaustively (step 1); anyone implementing the `net` traits
  (step 2). This is a breaking 0.x release
  (0.4 → 0.5).
- No wire-level break for IPv4 peers: IPv4 behaviour is unchanged.

## Effort estimate

Step 1: about half a day. Steps 2–4: 3–5 days, mostly mechanical, but they touch
about 18 source files and the test helpers that build raw IPv4 packets.
Step 5: 2–3 days to parametrise the turmoil suite over the address family.
Total about 1.5–2 weeks of focused work.

## Alternatives

- **Explicit non-goal.** Keep IPv4-only, and list the 27 IDs with
  `not_tested!(…, "IPv6 out of scope")` so the traceability report shows a
  documented exclusion instead of silent gaps. Cheap, but README's "full
  compliance" wording would then need a qualifier (already added on this
  branch).
- **Parse-only.** Do step 1 and ignore IPv6 endpoints at the runtime layer.
  Covers the wire-format requirements, but peers that offer only via IPv6
  remain undiscoverable, so it adds little value over the non-goal option.

## Recommendation

Do step 1 now (additive, low risk, closes the wire-format requirements), then
decide between the full migration (steps 2–5, breaking 0.5 release) and the
explicit non-goal. The project's stated final goal is "100 % requirement
coverage", which argues for the full migration.
