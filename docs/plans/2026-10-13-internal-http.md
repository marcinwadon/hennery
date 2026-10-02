# MCP gateway (plan 8b-ii): plain `http` on an internal network Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** the operator's decision of 2026-10-02 (plan 8b's decision 6, the gateway lane's L12). A gateway connection that the owner has marked "internal network" may reach an MCP server on the LAN over plain `http`. It may do so only to internal addresses. A public address stays `https` only, even under the marking, and so does every request without it. Nothing else changes, and the change is additive: 8b's API, which 8d and 10b-ii code against, is not renamed or narrowed.

**Architecture:** one module changes, `crates/hennery-kernel/src/egress.rs`.
- `is_internal(addr)` is an allowlist: IPv4 `10/8`, `127/8`, `172.16/12` and `192.168/16`; IPv6 `::1` and `fc00::/7`.
- `check_url` under `Allowance::InternalNetwork` lets plain `http` through to an internal literal or to a name. A literal is checked there; a name is checked by the resolver, which is the only place its addresses are known.
- The `InternalNetwork` client gains a third `reqwest` client, `plain`, used only for plain `http`. Its resolver refuses a name if **any** of its addresses is not internal (`Reach::Internal`). `https` under `InternalNetwork` keeps the client that reaches any address (`Reach::Any`). `PublicOnly` keeps its one client (`Reach::Public`).
- `EgressClient::send` and `send_streaming` pick the client by the URL's scheme (`for_url`).
- The resolver's lookup after the `localhost` rule is a private `SystemLookup` function: the system resolver in production, and a table in one unit test.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), tokio, reqwest 0.12. No new crate, and no `Cargo.lock` change.

**Spec:**
- Kernel §7.1 (plan 8b's write-back): "`https`, or plain `http` to loopback only … for every caller and allowance".
- Kernel §12: "**Plain `http` to a LAN address for an "internal network" connection** — refused until the maintainer decides".
- Gateway §5.7: non-public addresses are refused "unless the operator has marked the connection **"internal network"**, which allows them for that connection only".
- Gateway §4: OAuth URLs "must be `https` (loopback `http` allowed)".
- Gateway §9, the API table: `PATCH /api/mcp/connections/{id}` needs "**step-up** when the URL, credential kind or `internal_network` changes".

It builds on plan 8b (merged as #63, `ef75d4f`) and on the gateway lane's L7 (no test-only bypass), L9 (the frozen API) and L12 (this decision). Every anchor was taken from `main` at `ef75d4f`.

**Status:** written 2026-10-02; amended after the security review of 2026-10-02 (approve after amendments: decision 1 narrowed, O1–O8 taken) and re-confirmed (confirmed with notes). Three product questions are open for the operator ("After this plan").

**How the code blocks were made and checked:**
- Every block below was generated from the reviewed code on `scratch/gateway-8b-ii-tasks`, one commit per step.
- The plan was replayed from its own text onto `ef75d4f`, in a scratch worktree. The extractor applied 24 blocks, and the tree matched byte for byte.
- The five checks passed: 1022 tests, from 1019.
- All 72 revert-probes were run, and each failed its test: Task 1's new ones, and plan 8b's, with their anchors moved.

## Execution status

_Not executed yet._

## Scope

**In:**
- plain `http` to internal addresses under `InternalNetwork`, its tests and revert-probes;
- the write-back: kernel §7.1 and §12, gateway §5.7, and the decision's row in `docs/README.md` (Task 2).

**Out:**
- any consumer. 8d picks the allowance from the connection's flag (see "After this plan").
- 8a's save-time check. It already accepts `http` only for a connection marked internal (plan 8a's decision 9, `parse_url`).
- OAuth's own `https` rule (gateway §4): 8f's.

## Decisions this plan makes where the spec is silent

1. **"Internal" is an allowlist, not "not public".**
   - `is_public` is false for many addresses that lead to a public host or to no host: IPv4-mapped `::ffff:0:0/96`, NAT64 `64:ff9b::/96`, 6to4 `2002::/16` and Teredo all embed an IPv4 address, which may be public, and some of them are translated to it.
   - Others are false too: documentation ranges, benchmarking, multicast, broadcast, `0.0.0.0` (which Linux connects to as loopback) and `::`.
   - Plain `http` to any of those could cross the public internet in clear, which is exactly what the decision keeps `https` only. So plain `http` needs an address that is positively internal: RFC 1918 and loopback in IPv4; loopback and unique-local in IPv6.
   - **Link-local and CGNAT are left out** (the security review's Q1 and Q2; amended from the first draft, which had them).
     - Link-local (`169.254/16`, `fe80::/10`) is where cloud metadata services answer plain `http`, and only plain `http`: `https` under the marking cannot reach them in practice.
     - CGNAT `100.64/10` is Tailscale's range, and also, on some ISPs, the carrier's own shared network; Alibaba's metadata service lives in it too.
     - Whether either should count is the operator's question. Leaving them out is the narrower reading of "private/LAN", and widening it later breaks nothing.
   - A known limit: AWS's IPv6 metadata address `fd00:ec2::254` is inside unique-local `fc00::/7`, and is reachable over plain `http` under the marking. Refusing single addresses is a denylist; it is put to the operator with Q1.
   - The rule is stricter than the lane's wording, "every resolved address non-public", for every address that is neither public nor internal.
2. **A third client, chosen by scheme.**
   - reqwest's `Resolve` gets only the name, not the scheme, so one resolver cannot be strict for `http` and lax for `https`. A separate client, whose resolver allows internal addresses only, carries every plain `http` request under `InternalNetwork`, and nothing else.
   - Its own pool is a consequence: a plain-`http` request never reuses a connection opened under `Reach::Any`. Pools are keyed by scheme and authority, so the two would not share one anyway.
   - `Egress` still builds every client once, and clones share all three (8b's `clones_share_the_policy`).
3. **A name is refused, not filtered** (8b's decision 4, applied again): one address that is not internal refuses the name, and the connection goes only to the addresses that were checked.
4. **No new error variant.**
   - Plain `http` to a literal that is not internal is `Refused::Scheme("http")`.
   - A name with an address that is not internal is `Refused::Resolved { host, addr }`.
   - Adding a variant would break an exhaustive `match` in 8d or 10b-ii (L9: the API is frozen), so only the variants' docs and `Display` texts change, saying what is refused without claiming why.
5. **`check_url` under `InternalNetwork` accepts plain `http` to any name.** A name's addresses are unknown until it is resolved, so the internal-only resolver refuses it then. A caller that checks a URL ahead of time (a save-time check) learns about a public literal at once, and about a public name only when it is used.
6. **Where names are looked up is a private function pointer, not a bypass.**
   - `EgressClient::build_with(allowance, timeouts, system)` is private to the module. `Egress::new` always passes `system_lookup`.
   - The `localhost` rule runs before it and `checked` runs after it, so no lookup function can widen what a request may reach.
   - The unit test `plain_http_to_a_name_reaches_only_internal_addresses` uses a table, because no portable name resolves to a public address without a network. CI's Linux job has none but loopback (#54).
   - The same seam makes 8b's `localhost_is_loopback_without_a_lookup` exact on every platform: its lookup now panics, which was 8b's re-confirmation's optional hardening.
7. **Nothing else moves.**
   - `PublicOnly` behaves as in 8b, whatever the URL.
   - `localhost` and loopback literals take plain `http` under either allowance, as before. Under `InternalNetwork` they now go through the internal-only client, which accepts loopback.
   - Credentials, other schemes and a missing host are refused under either allowance.
   - `https` under `InternalNetwork` reaches any address.
8. **Only the owner can set the marking.**
   - The marking is the connection's `internal_network` flag. In 8a it is set by `POST /api/mcp/connections`, which needs step-up, and changed only by the owner's `PATCH`, which needs step-up when it touches the flag. It is stored per owner (L6).
   - The egress side cannot see the flag: it trusts the `Allowance` it is given. So 8d must derive the `Allowance` from the stored flag at request time, in one function, and never from anything in the request ("After this plan").

## The security review's answers

**The security review (opus, 2026-10-02), on the maintainer's behalf: approve after amendments.**
- **What it checked:**
  - hyper-util 0.1.21 skips the resolver only for a host that `Ipv4Addr` or `Ipv6Addr` parses, and every such host is a `Host::Ipv4` or `Host::Ipv6` in `url`, so no literal escapes `may_be_internal`;
  - all three clients have redirects and proxies off, and pools of their own;
  - the lookup seam is private, and `checked` always runs after it.
- **Decision 8, end to end against 8a** (`scratch/gateway-8a-r`):
  - every connection route is behind `operator_only`: browser rules and the session cookie. An MCP client, a session token or a host cannot reach them;
  - `POST` needs step-up through its route layer;
  - `PATCH` needs step-up whenever it carries `internal_network`;
  - the store is owner-scoped, and `parse_url` checks the flag after the patch is merged, so a stored `http` URL always has the flag. Switching between `http` and `https` changes the origin, which clears the credential.

| Finding | Taken how |
|---|---|
| O1 `Allowance::InternalNetwork`'s doc said "any address"; `Egress`'s said "two clients" | Both docs corrected |
| O2 the gateway §5.7 write-back listed other ranges than kernel §7.1 | Same list in both |
| O3 nothing tested that `https` under `InternalNetwork` keeps the any-reach client | `https://mixed.test` in `plain_http_to_a_name_reaches_only_internal_addresses` connects, and fails TLS rather than being refused; probe "`for_url` always the internal-only client" |
| O4 the `localhost` test's panicking lookup did not cover `Reach::Public` | All three reaches; `Public` refuses it with `Refused`, without a lookup |
| O5 the test table mapped names to a real public host, which four probes would have connected to | `192.0.2.1`, a documentation address that is not internal |
| O6 only narrowing probes | One widening probe per range: the addresses just outside each catch it |
| O7 an optional save-time `check_url` | "After this plan", for 8a/8d |
| O8 8d's obligations were understated | Spelled out in "After this plan" |
| Q1 cloud metadata over plain `http` (link-local; `fd00:ec2::254` in unique-local; Alibaba's in CGNAT) | **Product question for the operator.** Link-local is left out for now (decision 1); `fd00:ec2::254` stays a known limit |
| Q2 CGNAT may be the carrier's network | **Product question.** Left out for now |
| Q3 should the UI warn that a credential on a plain-`http` connection goes in clear | **Product question**, for plan 4 (the frontend) |

Decisions 1–8 were confirmed, decision 1 after its amendment.

**The re-confirmation (a fresh opus reviewer, 2026-10-02), scoped to the amendments: confirmed with notes.**
- **Confirmed:**
  - the narrowed list is the same in the code, the module and `Allowance` docs, kernel §7.1 and §12, gateway §5.7 and the README's row, with every range tested from inside and just outside;
  - O3's assertion tells the two `InternalNetwork` clients apart, and needs no network: the first address is the loopback listener;
  - O1, O2, O4, O5, O6 and O8 are done;
  - Q1–Q3 are recorded, not decided;
  - every new item is private, so the API is unchanged.
- **Its notes, all taken:**
  1. kernel §12 now lists the open questions;
  2. the `fd00:ec2::254` limit is in kernel §7.1 too;
  3. §7.1's address bullet says the allowance reaches any address over `https` only;
  4. `127.255.255.254` is tested, so narrowing `127/8` is caught;
  5. Q3 is in kernel §12 as a frontend question.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`.
- After every task the five checks pass: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; `cargo clippy -p hennery --locked -- -D warnings`; `cargo test --workspace --locked`; `cargo run -p hennery-proto --bin gen -- --check`.
- **Additive** (L9): no public name of `hennery_kernel::egress` is renamed or removed, and no variant is added or removed.
- **No test-only bypass in production code** (L7); decision 6's seam is private and cannot widen a check.
- **No network in tests** (#54): literals, `localhost` and the private table only.
- Commits: Conventional Commits, gmail identity, unsigned.

## Review Focus

1. **Plain `http` to a public literal, or one that is neither public nor internal, under `InternalNetwork`** (`http://8.8.8.8/`, `http://[::ffff:10.0.0.1]/`, NAT64, 6to4, Teredo, `0.0.0.0`, multicast, documentation).
   - Expected: `Refused::Scheme`, on both send paths, before anything is resolved.
   - Tests: `plain_http_under_internal_network_only_to_internal_addresses`; `plain_http_is_refused_except_to_loopback_or_an_internal_address`.
2. **Plain `http` to a name with any address that is not internal**, under `InternalNetwork`.
   - Expected: `Refused::Resolved`, on both send paths, and the listener sees nothing; a name that is all internal connects.
   - Tests: `plain_http_to_a_name_reaches_only_internal_addresses`; `a_name_for_plain_http_needs_every_address_internal`.
3. **Every internal range, at its edges,** and the addresses just outside each one.
   - Test: `plain_http_under_internal_network_only_to_internal_addresses`, with one revert-probe per range.
4. **`PublicOnly` and the rest unchanged.**
   - Expected: plain `http` refused except to loopback; credentials and other schemes refused under both allowances; `https` under `InternalNetwork` still reaches any address.
   - Tests: `plain_http_only_to_loopback`, the new test's last loop, `https://mixed.test` in `plain_http_to_a_name_reaches_only_internal_addresses`, and plan 8b's tests.
5. **`localhost` is never looked up,** under any reach.
   - Test: `localhost_is_loopback_without_a_lookup`, whose lookup panics.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-kernel/src/egress.rs` | `is_internal`, `Reach`, the internal-only client, `for_url`, `SystemLookup`; unit tests | 1 |
| `crates/hennery-kernel/tests/egress.rs` | the scheme test, now per allowance | 1 |
| `docs/specs/2026-09-26-kernel-design.md` | §7.1 and §12 | 2 |
| `docs/specs/2026-09-26-mcp-gateway-design.md` | §5.7 | 2 |
| `docs/README.md` | the decision's row | 2 |

**Reading the steps:** as in plan 8b. "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point, as whole lines (earlier blocks of the same task already applied, in order), then "with:" and its replacement. "Run:" lines only check.

---

### Task 1: Plain `http` on an internal network

**Files:**
- Modify: `crates/hennery-kernel/src/egress.rs`, `crates/hennery-kernel/tests/egress.rs`

**Interfaces:**
- Produces: no new public item. `check_url(url, Allowance::InternalNetwork)` now accepts plain `http` to an internal literal or a name, and `EgressClient::send` and `send_streaming` under `InternalNetwork` send plain `http` only to internal addresses.
- Consumes: 8b's module as merged.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust

    /// Plan 8b (e): https, or http to loopback.
    #[test]
```

with:

```rust

    /// Plan 8b (e): https, or http to loopback; under `PublicOnly`, plain
    /// http to anything else is refused by its scheme.
    #[test]
```

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust
            assert_eq!(check_url(&parsed, Allowance::InternalNetwork), Ok(()), "{url}");
        }
        for url in [
            "http://example.com/",
            "http://10.0.0.1/",
            "http://localhost.example.com/",
            "http://8.8.8.8/",
            "ws://localhost/",
            "file:///etc/passwd",
        ] {
            let parsed = Url::parse(url).unwrap();
```

with:

```rust
            assert_eq!(check_url(&parsed, Allowance::InternalNetwork), Ok(()), "{url}");
            // Loopback passes the scheme rule under `PublicOnly` too (its
            // address is what refuses it there).
            assert!(
                !matches!(check_url(&parsed, Allowance::PublicOnly), Err(Refused::Scheme(_))),
                "{url}"
            );
        }
        for url in [
            "http://example.com/",
            "http://10.0.0.1/",
            "http://localhost.example.com/",
            "http://8.8.8.8/",
        ] {
            let parsed = Url::parse(url).unwrap();
            assert!(
                matches!(check_url(&parsed, Allowance::PublicOnly), Err(Refused::Scheme(_))),
                "{url}"
            );
        }
        for url in ["ws://localhost/", "ws://10.0.0.1/", "file:///etc/passwd"] {
            let parsed = Url::parse(url).unwrap();
```

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust

    /// Plan 8b (d): refuse, not filter: one inward record refuses the name.
    #[test]
    fn a_name_with_any_non_public_address_is_refused() {
        let public: SocketAddr = "93.184.215.14:0".parse().unwrap();
        let private: SocketAddr = "10.0.0.1:0".parse().unwrap();
        let mapped: SocketAddr = "[::ffff:127.0.0.1]:0".parse().unwrap();
        assert_eq!(
            checked("example.com", vec![public], Allowance::PublicOnly),
            Ok(vec![public])
        );
        for addrs in [vec![public, private], vec![private, public], vec![public, mapped]] {
            let Err(Refused::Resolved { host, addr }) = checked("example.com", addrs.clone(), Allowance::PublicOnly)
            else {
                panic!("{addrs:?} was not refused");
            };
            assert_eq!(host, "example.com");
            assert!(!is_public(addr));
            assert_eq!(
                checked("example.com", addrs.clone(), Allowance::InternalNetwork),
                Ok(addrs)
            );
        }
```

with:

```rust

    /// Plan 8b-ii: under `InternalNetwork`, plain `http` may go to an
    /// internal literal — every internal range, at its edges — or to a
    /// name (the internal-only client's resolver checks it). A public
    /// literal, or one that is neither public nor internal, stays `https`
    /// only. Under `PublicOnly` nothing changes.
    #[test]
    fn plain_http_under_internal_network_only_to_internal_addresses() {
        for url in [
            "http://10.0.0.1/",
            "http://10.255.255.254/",
            "http://127.0.0.2/",
            "http://127.255.255.254/",
            "http://172.16.0.1/",
            "http://172.31.255.254/",
            "http://192.168.0.1/",
            "http://192.168.255.254/",
            "http://[::1]/",
            "http://[fc00::1]/",
            "http://[fdff:ffff::1]/",
            "http://nas.lan/",
            "http://example.com/",
        ] {
            let parsed = Url::parse(url).unwrap();
            assert_eq!(check_url(&parsed, Allowance::InternalNetwork), Ok(()), "{url}");
            if !is_loopback_host(&parsed) {
                assert_eq!(
                    check_url(&parsed, Allowance::PublicOnly),
                    Err(Refused::Scheme("http".into())),
                    "{url}"
                );
            }
        }
        for url in [
            "http://8.8.8.8/",
            "http://9.255.255.255/",
            "http://11.0.0.0/",
            "http://126.255.255.255/",
            "http://128.0.0.0/",
            "http://100.64.0.1/",
            "http://100.100.100.200/",
            "http://169.254.169.254/",
            "http://169.254.0.1/",
            "http://172.15.255.255/",
            "http://172.32.0.0/",
            "http://192.167.255.255/",
            "http://192.169.0.0/",
            "http://0.0.0.0/",
            "http://192.0.2.1/",
            "http://198.18.0.1/",
            "http://224.0.0.1/",
            "http://255.255.255.255/",
            "http://[2606:4700:4700::1111]/",
            "http://[::]/",
            "http://[::2]/",
            "http://[fbff:ffff::1]/",
            "http://[fe00::]/",
            "http://[fe80::1]/",
            "http://[fec0::1]/",
            "http://[ff02::1]/",
            "http://[::ffff:10.0.0.1]/",
            "http://[::ffff:8.8.8.8]/",
            "http://[64:ff9b::a00:1]/",
            "http://[2002:a00:1::1]/",
            "http://[2001:0:4136:e378:8000:63bf:3fff:fdd2]/",
        ] {
            let parsed = Url::parse(url).unwrap();
            for allowance in [Allowance::PublicOnly, Allowance::InternalNetwork] {
                assert_eq!(
                    check_url(&parsed, allowance),
                    Err(Refused::Scheme("http".into())),
                    "{url} {allowance:?}"
                );
            }
        }
        // Everything else a public-only request may not carry, it still may not.
        for (url, refusal) in [
            ("http://user@10.0.0.1/", Refused::Credentials),
            ("http://user:secret@nas.lan/", Refused::Credentials),
            ("ws://10.0.0.1/", Refused::Scheme("ws".into())),
            ("ftp://nas.lan/", Refused::Scheme("ftp".into())),
        ] {
            let parsed = Url::parse(url).unwrap();
            assert_eq!(check_url(&parsed, Allowance::InternalNetwork), Err(refusal), "{url}");
        }
    }

    /// Plan 8b-ii: the internal-only client's resolver refuses a name with
    /// **any** address that is not internal, public or not, in any order.
    #[test]
    fn a_name_for_plain_http_needs_every_address_internal() {
        let lan: SocketAddr = "192.168.1.10:0".parse().unwrap();
        let ula: SocketAddr = "[fd12:3456::1]:0".parse().unwrap();
        let public: SocketAddr = "93.184.215.14:0".parse().unwrap();
        let doc: SocketAddr = "192.0.2.1:0".parse().unwrap();
        let mapped: SocketAddr = "[::ffff:10.0.0.1]:0".parse().unwrap();
        assert_eq!(checked("nas.lan", vec![lan, ula], Reach::Internal), Ok(vec![lan, ula]));
        for addrs in [
            vec![public],
            vec![lan, public],
            vec![public, lan],
            vec![lan, doc],
            vec![lan, mapped],
        ] {
            let Err(Refused::Resolved { host, addr }) = checked("nas.lan", addrs.clone(), Reach::Internal) else {
                panic!("{addrs:?} was not refused");
            };
            assert_eq!(host, "nas.lan");
            assert!(!is_internal(addr), "{addr}");
        }
    }

    /// A lookup table standing in for the system resolver.
    fn table(host: String) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send>> {
        Box::pin(async move {
            let addrs: &[&str] = match host.as_str() {
                "lan.test" => &["127.0.0.1:0"],
                "mixed.test" => &["127.0.0.1:0", "192.0.2.1:0"],
                "public.test" => &["192.0.2.1:0"],
                _ => return Err(std::io::Error::other("no such name")),
            };
            Ok(addrs.iter().map(|addr| addr.parse().unwrap()).collect())
        })
    }

    /// Plan 8b-ii: under `InternalNetwork`, plain `http` to a name goes
    /// through the internal-only client: a name whose addresses are all
    /// internal connects, one with any other address is refused and nothing
    /// connects; `https` does not go through it. (Names come from `table`,
    /// whose other address is a documentation one, never connected to; the
    /// listener is on loopback, which is internal.)
    #[tokio::test]
    async fn plain_http_to_a_name_reaches_only_internal_addresses() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let count = accepted.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf).await;
                    let _ = stream
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
                        .await;
                });
            }
        });
        let timeouts = Timeouts {
            connect: Duration::from_secs(2),
            request: Duration::from_secs(5),
        };
        let client = EgressClient::build_with(Allowance::InternalNetwork, timeouts, table).unwrap();
        let get = |host: &str| Request::new(Method::GET, Url::parse(&format!("http://{host}:{port}/x")).unwrap());
        let response = client.send(get("lan.test")).await.unwrap();
        assert_eq!(response.text().await.unwrap(), "ok");
        assert_eq!(accepted.load(Ordering::SeqCst), 1);
        for host in ["mixed.test", "public.test"] {
            for streaming in [false, true] {
                let sent = if streaming {
                    client.send_streaming(get(host)).await
                } else {
                    client.send(get(host)).await
                };
                let Err(EgressError::Refused(Refused::Resolved { addr, .. })) = sent else {
                    panic!("{host}: {sent:?}");
                };
                assert_eq!(addr, "192.0.2.1".parse::<IpAddr>().unwrap());
            }
        }
        assert_eq!(accepted.load(Ordering::SeqCst), 1, "a refused name connected");
        // `https` under `InternalNetwork` keeps the client that reaches any
        // address: `mixed.test` connects (and fails the TLS handshake with
        // the plain listener), rather than being refused.
        let https = Request::new(
            Method::GET,
            Url::parse(&format!("https://mixed.test:{port}/x")).unwrap(),
        );
        let sent = client.send(https).await;
        assert!(matches!(sent, Err(EgressError::Http(_))), "{sent:?}");
        assert_eq!(accepted.load(Ordering::SeqCst), 2, "https did not connect");
    }

    /// Plan 8b (d): refuse, not filter: one inward record refuses the name.
    #[test]
    fn a_name_with_any_non_public_address_is_refused() {
        let public: SocketAddr = "93.184.215.14:0".parse().unwrap();
        let private: SocketAddr = "10.0.0.1:0".parse().unwrap();
        let mapped: SocketAddr = "[::ffff:127.0.0.1]:0".parse().unwrap();
        assert_eq!(checked("example.com", vec![public], Reach::Public), Ok(vec![public]));
        for addrs in [vec![public, private], vec![private, public], vec![public, mapped]] {
            let Err(Refused::Resolved { host, addr }) = checked("example.com", addrs.clone(), Reach::Public) else {
                panic!("{addrs:?} was not refused");
            };
            assert_eq!(host, "example.com");
            assert!(!is_public(addr));
            assert_eq!(checked("example.com", addrs.clone(), Reach::Any), Ok(addrs));
        }
```

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust
    async fn localhost_is_loopback_without_a_lookup() {
        let resolver = CheckedResolver {
            allowance: Allowance::InternalNetwork,
        };
        for name in ["localhost", "localhost.", "LocalHost."] {
            let addrs: Vec<SocketAddr> = resolver.resolve(name.parse().unwrap()).await.unwrap().collect();
            assert_eq!(addrs, LOCALHOST.to_vec(), "{name}");
        }
        assert!(!is_localhost("localhost.example.com"));
        assert!(!is_localhost("foo.localhost"));
        // And under `PublicOnly`, loopback is refused like any inward answer.
        let Err(Refused::Resolved { .. }) = checked("localhost", LOCALHOST.to_vec(), Allowance::PublicOnly) else {
            panic!("localhost was not refused");
```

with:

```rust
    async fn localhost_is_loopback_without_a_lookup() {
        fn no_lookup(host: String) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send>> {
            panic!("{host} was looked up")
        }
        for reach in [Reach::Any, Reach::Internal, Reach::Public] {
            let resolver = CheckedResolver {
                reach,
                system: no_lookup,
            };
            for name in ["localhost", "localhost.", "LocalHost."] {
                match resolver.resolve(name.parse().unwrap()).await {
                    Ok(addrs) => {
                        assert_ne!(reach, Reach::Public, "{name}");
                        assert_eq!(addrs.collect::<Vec<_>>(), LOCALHOST.to_vec(), "{name}");
                    }
                    Err(err) => {
                        assert_eq!(reach, Reach::Public, "{name}: {err}");
                        assert!(err.downcast_ref::<Refused>().is_some(), "{name}: {err}");
                    }
                }
            }
        }
        assert!(!is_localhost("localhost.example.com"));
        assert!(!is_localhost("foo.localhost"));
        // And under `PublicOnly`, loopback is refused like any inward answer.
        let Err(Refused::Resolved { .. }) = checked("localhost", LOCALHOST.to_vec(), Reach::Public) else {
            panic!("localhost was not refused");
```

In `crates/hennery-kernel/tests/egress.rs`, replace:

```rust

/// Plan 8b (e): https, or plain http to loopback only, for every caller and
/// allowance; the scheme is refused before anything is resolved.
#[tokio::test]
async fn plain_http_is_refused_except_to_loopback() {
    let client = egress().client(Allowance::InternalNetwork);
    for url in [
        "http://hennery-egress-test.invalid/",
        "http://10.0.0.1/",
        "ftp://example.com/",
    ] {
        let err = within(client.send(get(Url::parse(url).unwrap()))).await.unwrap_err();
        assert!(
            matches!(err, EgressError::Refused(Refused::Scheme(_))),
            "{url}: {err:?}"
        );
    }
```

with:

```rust

/// Plan 8b (e), 8b-ii: https, or plain http to loopback; under
/// `InternalNetwork` also plain http to an internal address, never to a
/// public one. The scheme is refused before anything is resolved.
#[tokio::test]
async fn plain_http_is_refused_except_to_loopback_or_an_internal_address() {
    let egress = egress();
    for (allowance, url) in [
        (Allowance::PublicOnly, "http://hennery-egress-test.invalid/"),
        (Allowance::PublicOnly, "http://10.0.0.1/"),
        (Allowance::InternalNetwork, "http://8.8.8.8/"),
        (Allowance::InternalNetwork, "http://[::ffff:10.0.0.1]/"),
        (Allowance::InternalNetwork, "ftp://example.com/"),
    ] {
        let client = egress.client(allowance);
        for streaming in [false, true] {
            let request = get(Url::parse(url).unwrap());
            let err = if streaming {
                within(client.send_streaming(request)).await.unwrap_err()
            } else {
                within(client.send(request)).await.unwrap_err()
            };
            assert!(
                matches!(err, EgressError::Refused(Refused::Scheme(_))),
                "{url} {allowance:?}: {err:?}"
            );
        }
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-kernel --locked --lib egress::`
Expected: FAIL to compile: `Reach`, `is_internal`, `Pin`, `EgressClient::build_with` and `CheckedResolver`'s new fields do not exist.

- [ ] **Step 3: Commit the tests**

```bash
git add crates/hennery-kernel/src/egress.rs crates/hennery-kernel/tests/egress.rs
git commit -m "test(egress): plain http on an internal network, to internal addresses only"
```

- [ ] **Step 4: The code**

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust
//!
//! - **Built once, cloned freely.** [`Egress::new`] builds one `reqwest`
//!   client per [`Allowance`]; [`Egress`] and [`EgressClient`] are cheap to
//!   clone (`reqwest::Client` is an `Arc` inside) and share connection pools
//!   with their clones. The two allowances never share a pool: a pooled
//!   connection skips the resolver, so a public-only request must never
```

with:

```rust
//!
//! - **Built once, cloned freely.** [`Egress::new`] builds its `reqwest`
//!   clients once: one per [`Allowance`], and a second, internal-only one
//!   for plain `http` under [`Allowance::InternalNetwork`]. [`Egress`] and
//!   [`EgressClient`] are cheap to clone (`reqwest::Client` is an `Arc`
//!   inside) and share connection pools with their clones. The two
//!   allowances never share a pool: a pooled
//!   connection skips the resolver, so a public-only request must never
```

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust
//! - **The URL check** ([`check_url`]): `https`, or plain `http` to
//!   loopback only (a loopback literal or `localhost`), for every caller and
//!   allowance; no credentials in the URL; an IP-literal host must be public
//!   under [`Allowance::PublicOnly`]. reqwest never asks the resolver about a
//!   literal, so this check is the only one a literal gets. `url::Url` has
//!   already normalised the odd IPv4 spellings (`2130706433`, `0x7f.1`,
```

with:

```rust
//! - **The URL check** ([`check_url`]): `https`, or plain `http` to
//!   loopback (a loopback literal or `localhost`); under
//!   [`Allowance::InternalNetwork`] also plain `http` to an internal
//!   address or a name (plan 8b-ii, below). No credentials in the URL; an
//!   IP-literal host must be public under [`Allowance::PublicOnly`].
//!   reqwest never asks the resolver about a literal, so this check is the
//!   only one a literal gets. `url::Url` has
//!   already normalised the odd IPv4 spellings (`2130706433`, `0x7f.1`,
```

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust
//!   multicast `ff00::/8` are never public.
//! - **Never** a proxy from the environment (`HTTP_PROXY`, `HTTPS_PROXY`,
```

with:

```rust
//!   multicast `ff00::/8` are never public.
//! - **Plain `http` inside the internal network** (plan 8b-ii, the
//!   operator's decision of 2026-10-02): only under
//!   [`Allowance::InternalNetwork`], and only to internal addresses — RFC
//!   1918, loopback and unique-local IPv6. A public address stays `https`
//!   only even then, and so do the addresses that are neither: link-local
//!   (cloud metadata services answer plain `http` there), CGNAT (it may be
//!   the carrier's network), documentation, multicast, `0.0.0.0`,
//!   IPv4-mapped, NAT64, 6to4 and Teredo (they may lead to a public host). A
//!   literal is checked by the URL check; a name goes through a third
//!   client, used for nothing but plain `http` under `InternalNetwork`,
//!   whose resolver refuses the name if **any** address is not internal.
//! - **Never** a proxy from the environment (`HTTP_PROXY`, `HTTPS_PROXY`,
```

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
```

with:

```rust
use std::fmt;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
```

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust
    PublicOnly,
    /// Any address: a gateway connection the operator marked "internal
    /// network" (gateway spec §5.7), and nothing else.
    InternalNetwork,
```

with:

```rust
    PublicOnly,
    /// Any address over `https`, and internal addresses only over plain
    /// `http`: a gateway connection the operator marked "internal network"
    /// (gateway spec §5.7), and nothing else.
    InternalNetwork,
```

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust

/// The policy's two clients, one per [`Allowance`].
#[derive(Debug, Clone)]
```

with:

```rust

/// The policy's clients: one per [`Allowance`], and the internal-network
/// one with a second, for plain `http`.
#[derive(Debug, Clone)]
```

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust
    http: reqwest::Client,
    allowance: Allowance,
    timeouts: Timeouts,
}

impl EgressClient {
    fn build(allowance: Allowance, timeouts: Timeouts) -> anyhow::Result<EgressClient> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("hennery/", env!("CARGO_PKG_VERSION")))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .dns_resolver(Arc::new(CheckedResolver { allowance }))
            .connect_timeout(timeouts.connect)
            .build()?;
        Ok(EgressClient {
            http,
            allowance,
            timeouts,
        })
    }
```

with:

```rust
    http: reqwest::Client,
    /// Plain `http` under [`Allowance::InternalNetwork`]: its resolver lets
    /// a name reach internal addresses only. `None` under `PublicOnly`.
    plain: Option<reqwest::Client>,
    allowance: Allowance,
    timeouts: Timeouts,
}

impl EgressClient {
    fn build(allowance: Allowance, timeouts: Timeouts) -> anyhow::Result<EgressClient> {
        EgressClient::build_with(allowance, timeouts, system_lookup)
    }

    /// [`EgressClient::build`] with the lookup a name goes to after the
    /// `localhost` rule: the system resolver, or a unit test's table. The
    /// checks on its answer are the same either way.
    fn build_with(allowance: Allowance, timeouts: Timeouts, system: SystemLookup) -> anyhow::Result<EgressClient> {
        let (reach, plain) = match allowance {
            Allowance::PublicOnly => (Reach::Public, None),
            Allowance::InternalNetwork => (Reach::Any, Some(http_client(Reach::Internal, timeouts, system)?)),
        };
        Ok(EgressClient {
            http: http_client(reach, timeouts, system)?,
            plain,
            allowance,
            timeouts,
        })
    }

    /// The client a request to `url` goes through: plain `http` under
    /// `InternalNetwork` through the internal-only one.
    fn for_url(&self, url: &Url) -> &reqwest::Client {
        match &self.plain {
            Some(plain) if url.scheme() == "http" => plain,
            _ => &self.http,
        }
    }
```

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust
        }
        self.http.execute(request).await.map_err(classify)
    }
```

with:

```rust
        }
        self.for_url(request.url()).execute(request).await.map_err(classify)
    }
```

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust
        let head = request.timeout_mut().take().unwrap_or(self.timeouts.request);
        match tokio::time::timeout(head, self.http.execute(request)).await {
            Ok(sent) => sent.map_err(classify),
            Err(_) => Err(EgressError::Timeout),
        }
    }
}

/// Why the policy refused a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Not `https`, and not `http` to loopback.
    Scheme(String),
```

with:

```rust
        let head = request.timeout_mut().take().unwrap_or(self.timeouts.request);
        let http = self.for_url(request.url());
        match tokio::time::timeout(head, http.execute(request)).await {
            Ok(sent) => sent.map_err(classify),
            Err(_) => Err(EgressError::Timeout),
        }
    }
}

fn http_client(reach: Reach, timeouts: Timeouts, system: SystemLookup) -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("hennery/", env!("CARGO_PKG_VERSION")))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(CheckedResolver { reach, system }))
        .connect_timeout(timeouts.connect)
        .build()?)
}

/// Why the policy refused a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Not `https`, and not `http` to loopback (or, under
    /// [`Allowance::InternalNetwork`], to an internal address or a name).
    Scheme(String),
```

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust
    Address(IpAddr),
    /// A name that resolved to an address that is not public.
    Resolved { host: String, addr: IpAddr },
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::Scheme(scheme) => write!(f, "{scheme:?} URLs are refused: https only, or http to loopback"),
            Refused::Credentials => f.write_str("URLs with credentials are refused"),
            Refused::NoHost => f.write_str("the URL has no host"),
            Refused::Address(addr) => write!(f, "{addr} is not a public address"),
            Refused::Resolved { host, addr } => write!(f, "{host} resolves to {addr}, not a public address"),
        }
```

with:

```rust
    Address(IpAddr),
    /// A name that resolved to an address the request may not reach: not
    /// public, or, for plain `http` under `InternalNetwork`, not internal.
    Resolved { host: String, addr: IpAddr },
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::Scheme(scheme) => write!(
                f,
                "{scheme:?} URLs are refused: https only, or http to loopback or, on an internal network, to an internal address or a name"
            ),
            Refused::Credentials => f.write_str("URLs with credentials are refused"),
            Refused::NoHost => f.write_str("the URL has no host"),
            Refused::Address(addr) => write!(f, "{addr} is not a public address"),
            Refused::Resolved { host, addr } => {
                write!(f, "{host} resolves to {addr}, which this request may not reach")
            }
        }
```

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust
        "http" if is_loopback_host(url) => {}
        other => return Err(Refused::Scheme(other.to_owned())),
```

with:

```rust
        "http" if is_loopback_host(url) => {}
        "http" if allowance == Allowance::InternalNetwork && may_be_internal(url) => {}
        other => return Err(Refused::Scheme(other.to_owned())),
```

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust
        None => false,
    }
```

with:

```rust
        None => false,
    }
}

/// Whether plain `http` under `InternalNetwork` may go to `url`'s host: an
/// internal literal, or a name, which the internal-only client's resolver
/// then checks.
fn may_be_internal(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(_)) => true,
        Some(Host::Ipv4(addr)) => is_internal(IpAddr::V4(addr)),
        Some(Host::Ipv6(addr)) => is_internal(IpAddr::V6(addr)),
        None => false,
    }
}

/// IPv4 ranges that are internal: plain `http` may reach them under
/// `InternalNetwork` (plan 8b-ii).
const V4_INTERNAL: &[(Ipv4Addr, u8)] = &[
    (Ipv4Addr::new(10, 0, 0, 0), 8),     // RFC 1918
    (Ipv4Addr::new(127, 0, 0, 0), 8),    // loopback
    (Ipv4Addr::new(172, 16, 0, 0), 12),  // RFC 1918
    (Ipv4Addr::new(192, 168, 0, 0), 16), // RFC 1918
];

/// IPv6 ranges that are internal; no IPv4-mapped, NAT64, 6to4 or Teredo
/// address is, since any of them may lead to a public IPv4 host.
const V6_INTERNAL: &[(Ipv6Addr, u8)] = &[
    (Ipv6Addr::LOCALHOST, 128),                      // loopback
    (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7), // unique-local
];

/// Whether `addr` is internal; see the module docs. Only an allowlist, so
/// everything else — public or neither — is not.
fn is_internal(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(addr) => V4_INTERNAL.iter().any(|&range| in_v4(addr, range)),
        IpAddr::V6(addr) => V6_INTERNAL.iter().any(|&range| in_v6(addr, range)),
    }
```

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust

/// The addresses a name resolved to, if the allowance permits all of them.
fn checked(host: &str, addrs: Vec<SocketAddr>, allowance: Allowance) -> Result<Vec<SocketAddr>, Refused> {
    if allowance == Allowance::PublicOnly
        && let Some(addr) = addrs.iter().find(|addr| !is_public(addr.ip()))
    {
        return Err(Refused::Resolved {
```

with:

```rust

/// Which addresses a client's resolver lets a name reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// Public only: every `PublicOnly` request.
    Public,
    /// Any: `https` under `InternalNetwork`.
    Any,
    /// Internal only: plain `http` under `InternalNetwork`.
    Internal,
}

/// The addresses a name resolved to, if `reach` permits all of them.
fn checked(host: &str, addrs: Vec<SocketAddr>, reach: Reach) -> Result<Vec<SocketAddr>, Refused> {
    let permitted = |addr: &SocketAddr| match reach {
        Reach::Public => is_public(addr.ip()),
        Reach::Any => true,
        Reach::Internal => is_internal(addr.ip()),
    };
    if let Some(addr) = addrs.iter().find(|addr| !permitted(addr)) {
        return Err(Refused::Resolved {
```

In `crates/hennery-kernel/src/egress.rs`, replace:

```rust

/// The addresses `host` resolves to: [`LOCALHOST`] for `localhost`, the
/// system resolver's answer for any other name.
async fn lookup(host: &str) -> std::io::Result<Vec<SocketAddr>> {
    if is_localhost(host) {
        return Ok(LOCALHOST.to_vec());
    }
    Ok(tokio::net::lookup_host((host, 0)).await?.collect())
}

/// Resolves names with [`lookup`] and applies [`checked`].
struct CheckedResolver {
    allowance: Allowance,
}

impl Resolve for CheckedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let allowance = self.allowance;
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let addrs = lookup(&host).await?;
            let addrs = checked(&host, addrs, allowance)?;
            Ok(Box::new(addrs.into_iter()) as Addrs)
```

with:

```rust

/// Where a name other than `localhost` is looked up.
type SystemLookup = fn(String) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send>>;

/// The system resolver.
fn system_lookup(host: String) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send>> {
    Box::pin(async move { Ok(tokio::net::lookup_host((host.as_str(), 0)).await?.collect()) })
}

/// The addresses `host` resolves to: [`LOCALHOST`] for `localhost`, the
/// system resolver's answer for any other name.
async fn lookup(host: &str, system: SystemLookup) -> std::io::Result<Vec<SocketAddr>> {
    if is_localhost(host) {
        return Ok(LOCALHOST.to_vec());
    }
    system(host.to_owned()).await
}

/// Resolves names with [`lookup`] and applies [`checked`].
struct CheckedResolver {
    reach: Reach,
    system: SystemLookup,
}

impl Resolve for CheckedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let (reach, system) = (self.reach, self.system);
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let addrs = lookup(&host, system).await?;
            let addrs = checked(&host, addrs, reach)?;
            Ok(Box::new(addrs.into_iter()) as Addrs)
```

- [ ] **Step 5: Run them to see them pass**

Run: `cargo test -p hennery-kernel --locked --lib egress::`
Run: `cargo test -p hennery-kernel --locked --test egress --test egress_proxy`
Expected: PASS: 10 unit tests, 15 and 1.

- [ ] **Step 6: Revert-probes**

Make each edit alone in `crates/hennery-kernel/src/egress.rs`, run the named test, see it **fail as a test**, and restore. The ledger's `probe.py` runs these and plan 8b's, with their anchors moved: 72 in all.

| Edit | Test that fails |
|---|---|
| Remove the `"http" if allowance == Allowance::InternalNetwork && may_be_internal(url)` arm | `plain_http_under_internal_network_only_to_internal_addresses` |
| Drop `allowance == Allowance::InternalNetwork &&` from it | the same (the `PublicOnly` assertions) |
| In `may_be_internal`, an IPv4 literal, then alone an IPv6 one, always `true`; a name `false` | the same (each) |
| Each of the 4 lines of `V4_INTERNAL`, and of the 2 of `V6_INTERNAL`, removed alone | the same (each); for the two loopback lines, `localhost_is_loopback_without_a_lookup` (`localhost` through the internal-only resolver), since the scheme rule lets loopback literals through anyway |
| `Reach::Internal => true` | `a_name_for_plain_http_needs_every_address_internal`; `plain_http_to_a_name_reaches_only_internal_addresses` |
| In `for_url`, never the internal-only client | `plain_http_to_a_name_reaches_only_internal_addresses` |
| The internal-only client built with `Reach::Any` | the same |
| In `for_url`, the internal-only client for every scheme | the same (`https` is refused; the review's O3) |
| Each internal range widened by one bit (`/8` to `/7`, `/12` to `/11`, `/16` to `/15`, `::1/128` to `/127`, `fc00::/7` to `/6`) | `plain_http_under_internal_network_only_to_internal_addresses` (each; the address just outside it; the review's O6) |
| `send`, then alone `send_streaming`, using `self.http` in place of `for_url` | the same (each) |
| In the resolver, `system(host.clone())` in place of `lookup(&host, system)` (8b's B1 probe, moved) | `localhost_is_loopback_without_a_lookup` (the lookup panics) |

- [ ] **Step 7: Load**

Run the `egress` test binary and the kernel's unit tests six times in parallel, three rounds.
Expected: 18 runs each, no failure.

- [ ] **Step 8: The full checks**

Expected: all pass; **1022 tests**, from 1019.

- [ ] **Step 9: Commit**

```bash
git add crates/hennery-kernel/src/egress.rs
git commit -m "feat(egress): plain http on an internal network, to internal addresses only"
```

### Task 2: The write-back

**Files:**
- Modify: `docs/specs/2026-09-26-kernel-design.md`, `docs/specs/2026-09-26-mcp-gateway-design.md`, `docs/README.md`

- [ ] **Step 1: Write back the decision**

In `docs/specs/2026-09-26-kernel-design.md`, replace:

```markdown
  from the environment is ever used;
- `https`, or plain `http` to loopback only (a loopback literal or
  `localhost`, which always resolves to loopback, without a lookup), for
  every caller and allowance; a URL with credentials in it is refused;
- DNS is resolved by hennery and each address is checked before connecting:
  loopback, link-local (including `169.254.169.254`), RFC 1918, unique-local
  IPv6, CGNAT and other non-public ranges are refused, unless the caller passes
  an explicit "internal network" allowance (a gateway connection the operator
  marked so; never Web Push). A name with **any** non-public address is
  refused, not filtered. An IP-literal host never reaches the resolver, so the
```

with:

```markdown
  from the environment is ever used;
- `https`, or plain `http` to loopback (a loopback literal or
  `localhost`, which always resolves to loopback, without a lookup); under
  the "internal network" allowance, plain `http` to an internal address too
  — RFC 1918, loopback and unique-local IPv6 — never to a public one, nor
  to one that is neither (link-local, where cloud metadata services answer
  plain `http`; CGNAT, which may be the carrier's network; documentation,
  multicast, `0.0.0.0`, IPv4-mapped, NAT64, 6to4, Teredo). A literal is
  checked by the URL check; a name goes through a third client, used only
  for plain `http` under that allowance, whose resolver refuses the name if
  **any** address is not internal (the operator's decision of 2026-10-02,
  plan 8b-ii). A known limit: AWS's IPv6 metadata address
  `fd00:ec2::254` is unique-local, so plain `http` reaches it under the
  allowance. A URL with credentials in it is refused, under either
  allowance;
- DNS is resolved by hennery and each address is checked before connecting:
  loopback, link-local (including `169.254.169.254`), RFC 1918, unique-local
  IPv6, CGNAT and other non-public ranges are refused, unless the caller passes
  an explicit "internal network" allowance (a gateway connection the operator
  marked so; never Web Push), which reaches any address over `https`, and
  internal ones only over plain `http` (above). A name with **any** non-public address is
  refused, not filtered. An IP-literal host never reaches the resolver, so the
```

In `docs/specs/2026-09-26-kernel-design.md`, replace:

```markdown

*Built so far:* `hennery_kernel::egress` (plan 8b) — `Egress`, its
`EgressClient::send` and `send_streaming`, `check_url`, `is_public` and
```

with:

```markdown

*Built so far:* `hennery_kernel::egress` (plans 8b and 8b-ii) — `Egress`, its
`EgressClient::send` and `send_streaming`, `check_url`, `is_public` and
```

In `docs/specs/2026-09-26-kernel-design.md`, replace:

```markdown

Open:

- **Plain `http` to a LAN address for an "internal network" connection** —
  refused until the maintainer decides (§7.1; plan 8b, decision 6); widening
  it later breaks nothing.

```

with:

```markdown

Open (for the operator; plan 8b-ii's security review, Q1–Q3):

- **Link-local and CGNAT for plain `http`** under the "internal network"
  allowance: cloud metadata services answer plain `http` on link-local, and
  CGNAT may be the carrier's network. Until answered, both are `https` only
  (§7.1).
- **A warning in the UI** that a credential on a plain-`http` connection
  crosses the network in clear (frontend).

Resolved by the operator on 2026-10-02:

- **Plain `http` to a LAN address for an "internal network" connection** —
  allowed only under that explicit marking, and only to internal addresses
  (RFC 1918, loopback, unique-local IPv6); a public address stays `https`
  only (§7.1; plan 8b decision 6, built in plan 8b-ii).

```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
  only. Web Push endpoints are always public-only.
- Per connection, a cap on concurrent upstream requests and on idle streaming
```

with:

```markdown
  only. Web Push endpoints are always public-only.
- The upstream URL is `https`, or plain `http` to loopback; a connection
  marked "internal network" may also use plain `http` to an internal address
  (RFC 1918, loopback, unique-local IPv6), never to a public one
  (the operator's decision of 2026-10-02; kernel §7.1). §4's OAuth URLs stay
  `https` (loopback `http` allowed) even then.
- Per connection, a cap on concurrent upstream requests and on idle streaming
```

In `docs/README.md`, replace:

```markdown
| 6e | Windowing threshold, vendor token behaviour, `GET` SSE | Measured once the code exists | frontend §15, gateway §13 |

```

with:

```markdown
| 6e | Windowing threshold, vendor token behaviour, `GET` SSE | Measured once the code exists | frontend §15, gateway §13 |
| 7 | Plain `http` to a LAN MCP server (operator, 2026-10-02) | Only for a connection marked "internal network", and only to internal addresses; a public address stays `https` only | kernel §7.1, gateway §5.7 |

```

- [ ] **Step 2: Commit**

```bash
git add docs/specs/2026-09-26-kernel-design.md docs/specs/2026-09-26-mcp-gateway-design.md docs/README.md
git commit -m "docs(spec): write back plan 8b-ii's plain http on an internal network"
```

## After this plan

**Obligations this plan hands on:**
- **The proxy (8d)**, so that the allowance comes only from the stored flag (the review's O8):
  - derive the `Allowance` from the stored connection's `internal_network` flag, in one small function (for example `fn allowance(connection: &ConnectionRecord) -> Allowance`);
  - read the flag and the URL from the same stored row, on every request, and cache no `EgressClient` or allowance across a `PATCH`;
  - send to the stored URL's scheme and authority verbatim: nothing from the client (a path, a query, `Url::join` with a `//host` path) may change them;
  - never forward the client's `Host` header;
  - never take the allowance from the session, a header, the path or anything else in the request;
  - pin with a test that a connection not marked internal is refused plain `http` to a LAN address, and revert-probe it.
- **Connections (8a/8d):** at save time, `parse_url` already allows `http` only when marked. `check_url(url, allowance)` would also refuse a public or link-local `http` literal at once, rather than on first use. Optional (the review's O7).
- **OAuth (8f):** gateway §4 keeps every authorization-server URL `https` (loopback `http` allowed), even for a connection marked internal. The egress client under `InternalNetwork` now accepts plain `http` to internal addresses, so 8f checks the scheme itself before sending, and pins that with a test.
- **Web Push (10b):** nothing changes; `PublicOnly` only.

**Questions for the operator** (the security review's Q1–Q3; none blocks this plan, and each answer widens or adds without breaking anything):
1. Should link-local (`169.254/16`, `fe80::/10`) count as internal for plain `http`? It is where cloud metadata services answer. For now it does not.
2. Should CGNAT `100.64/10` count? It covers Tailscale, and also some carriers' shared networks. For now it does not.
3. Should the UI warn that a credential on a plain-`http` connection crosses the network in clear? That is for plan 4, the frontend.

**Spec amendments:** kernel §7.1 and §12 (the question is closed), gateway §5.7, and the maintainer decisions table's row 7.

Generated with Claude AI — please review before distribution.
