# hennery — remote hosts over WSS

- **Date:** 2026-10-14
- **Status:** Proposed; independent Codex review on the maintainer's behalf:
  APPROVE AFTER AMENDMENTS. Enrollment uncertainty, loopback downgrade and
  post-rename durability amendments are included below, pending scoped
  re-confirmation. No implementation or live acceptance is claimed.
- **Scope:** host enrollment, the host uplink, transport diagnostics and the
  reverse-proxy deployment contract. The
  [umbrella](2026-09-25-hennery-architecture-design.md) wins. This refines its
  §§5.8, 7.5–7.6, [ACP core](2026-09-26-acp-core-design.md) §§3.5 and 5,
  [kernel](2026-09-26-kernel-design.md) §§4, 7 and 10, and
  [distribution](2026-09-26-distribution-design.md) §7.

## 1. Outcome and boundaries

A remote machine pairs with the collector's public `https://` origin and
runs over `wss://`, without an SSH tunnel. Both requests validate the same
server name and trust roots. Reconnecting retains the host identity, running
adapters, outbox and existing reconciliation rules. A reverse proxy can renew
its certificate or restart without requiring the operator to pair again.

Goals:

- System trust roots by default; an explicit private-CA bundle for a
  self-hoster, scoped to this host's collector traffic.
- One transport policy for enrollment, probes, the running host and doctor.
- Actionable distinctions between DNS, TCP, certificate, upgrade and host
  authentication failures.
- A documented, tested reverse-proxy configuration, including client-address
  rate limiting. Public exposure must not silently turn every remote user
  into the same loopback limiter key.
- An explicit way to move an existing tunnel pairing to its public origin
  without discarding its key, sessions or outbox.

Non-goals: collector-side TLS termination or ACME; mTLS; corporate outbound
HTTP proxies; subpath hosting; proxy-provided login identities; multiple
collector replicas; changing hats, session-token grants or the ACP protocol;
installing CAs into the OS, browsers or agents. Certificate pinning is
considered in §3 but is not required for the proposed private-CA option.

## 2. Evidence and inherited contracts

At the inspected branch base, `0fe663b86f7d61d3ff358eac7d7e8ea57dff64bf`:

| Surface | Current behavior and consequence |
|---|---|
| `crates/hennery-host/src/pairing.rs` | Converts HTTPS to WSS, but `join` refuses HTTPS before spending a code. Enrollment already disables redirects and environment proxies. The URL parser still accepts userinfo. |
| `Cargo.toml`, host `connection.rs` | `tokio-tungstenite` 0.29 has no TLS feature enabled. Both `probe` and the running host use the same handshake. Reconnect delays double from 500 ms to 30 s, but the code has no jitter despite umbrella §5.8. |
| Host `identity.rs` | `host.toml` stores the WebSocket URL, host id, public key and workspace roots; writes preserve other configuration. Pending pairing files support crash recovery. |
| `crates/hennery/src/doctor/collector.rs` | HTTPS health requests use a separate reqwest client; WSS hello is explicitly skipped. Check 16 reads the configuration's public URL, not the post-setup database setting. |
| Kernel `auth_api.rs` and `ratelimit.rs`, `crates/hennery-sessions/src/hosts.rs` | Address budgets use the TCP peer. The README correctly warns that a same-machine proxy shares the loopback budget. |

The gap analysis estimated **one M plan** for basic WSS. The additional
private-CA lifecycle, tunnel migration and client-address handling below make
**two M plans** a more reviewable estimate; §11 separates them.

The predecessor's `cmd/host/main.go` maps HTTPS to WSS and uses a WebSocket
dialer with a bounded handshake. Reuse the direct outbound topology and
bounded connection establishment. Do not copy its shared `X-Monitor-Token`,
fallback of arbitrary schemes to WS, or connection-owned adapters and
backoff reset on a successful dial. Its `internal/tlsutil/tlsutil.go`
generates and replaces self-signed certificates on expiry. That is useful
evidence for self-hosting demand, but silently replacing trust material
would break pins and device trust. Hennery delegates certificate lifecycle
to the deployment and retains per-host Ed25519 credentials.

The existing invariants remain authoritative:

- Pairing codes are 40-bit, single-use, expire after ten minutes and are
  consumed in the host-creation transaction (kernel §4.1). Minting needs
  operator step-up, or the local admin socket's existing authority.
- The upgrade returns a fresh 32-byte `hennery-hello-nonce`. The negotiated
  `authenticate` and full `hello` sign the existing length-prefixed message, host id and protocol
  version. No valid nonce means no hello. Refusal order, strict Ed25519
  verification and one live connection per host remain ACP core §3.5's.
- Only `revoked` ends the host and stops its adapters (exit 78). Transport
  failures never authorize re-pairing or deletion of an outbox.
- The hello proof authenticates the host, not the collector. TLS authenticates
  the collector endpoint. A trusted terminating proxy can see and alter the
  traffic; the proof does not provide end-to-end protection from that proxy.

## 3. Trust and address policy

### 3.1 One collector transport

Introduce a host-owned `CollectorTransport`, constructed from a validated
`CollectorEndpoint` and `CollectorTrust`. Enrollment, host hello, reconnect
and doctor's check 7 use it. Check 16 uses the same transport implementation
with its own explicitly selected trust, never an arbitrary host's private CA.
Keep kernel `Egress` for its existing callers.

`CollectorEndpoint` carries the canonical HTTP(S) origin and derived WS(S)
endpoint `/api/hosts/ws`. Accept HTTPS, or HTTP only to a loopback literal or
the exact name `localhost`; reject userinfo, query, fragment, non-root path,
unsupported schemes and port zero. Stored WS(S) URLs must have exactly the
host endpoint path and the same restrictions. Parse before logging: invalid
input is described without echoing possible credentials. Scheme and port
are part of the origin; case and default ports are canonicalized.

For `localhost`, use loopback addresses directly, without consulting DNS or
the hosts file, in HTTP, WSS and doctor alike. Validate the actual addresses
used for plain HTTP/WS so no second library lookup escapes the rule. This
closes the gap explicitly handed on by
[plan 7d-ii](../plans/2026-10-14-doctor-ii.md), “After this plan”. Other HTTPS
names use DNS with SNI and certificate verification against the original
name, including when connecting to an explicitly resolved socket address.
An HTTPS IP literal requires the matching IP subject alternative name.

The collector address is supplied by the local operator, not by an upstream
MCP server. It may resolve to a public, private or VPN address over HTTPS.
This is the explicit host-to-collector allowance; it does not change kernel
§7.1's public-only defaults or extend the gateway's internal-HTTP exception
to hosts. An internal network never makes remote plain HTTP/WS acceptable.

Retain the shared egress safeguards: no redirects, no environment or explicit
forward proxy, bounded connections, safe errors. No cookies or bearer token
are added to these clients. A reverse proxy serving the configured origin
is distinct from an outbound proxy. Test HTTP(S)/ALL_PROXY and lowercase
variants against trap servers for all transport paths.

### 3.2 Root selection

The proposed Rust configuration enum is `CollectorTrust::System` (default)
or `CollectorTrust::CaBundle { pem: String }`. The latter uses **only** the
provided roots, not their union with public roots. Both use the standard
rustls certificate verifier: chain signatures, name, validity and server
authentication usage; minimum TLS 1.2, with TLS 1.3 allowed. No insecure
switch, trust-on-first-use prompt or automatic certificate download.
Plain loopback HTTP/WS needs no root-store loading; reject a private-CA option
on that endpoint rather than implying it encrypts the connection.

Use a native-root loader for system mode and one explicitly built rustls
configuration for both HTTP and WebSocket clients. Merely enabling different
library defaults would let HTTPS enrollment succeed where WSS fails.
`tokio-tungstenite` exposes an explicit TLS connector and reqwest accepts a
preconfigured TLS backend; the implementation plan must pin compatible
rustls types and prove this pairing with the locked dependency graph.
See the primary API references for
[the WebSocket connector](https://docs.rs/tokio-tungstenite/0.29.0/tokio_tungstenite/fn.connect_async_tls_with_config.html)
and [reqwest TLS configuration](https://docs.rs/reqwest/0.12.28/reqwest/struct.ClientBuilder.html#method.use_preconfigured_tls).
Do not alter the trust defaults of gateway, push or runtime-download clients
as a side effect of feature unification.

“System roots” means native trust anchors validated by rustls, not a promise
to reproduce every platform verifier policy. Native-root loading may honor
`SSL_CERT_FILE`/`SSL_CERT_DIR`; make that source visible in doctor and use the
same effective service environment as the host. Explicit CA mode ignores
these overrides. No fallback to bundled public roots when loading fails or
produces an empty store. Partial loading with usable roots reports a warning
and the count of failures, without certificate contents. This behavior must
be checked against the chosen
[native-root loader](https://docs.rs/rustls-native-certs/latest/rustls_native_certs/).

`--ca-cert <file>` imports a PEM CA bundle supplied through an already
trusted channel. At most 256 KiB and 32 certificates; require at least one
valid CA certificate, reject malformed blocks, private keys and nonregular
input. Copy the validated PEM into the host configuration, so removal of a
temporary input file cannot break the service later. The signer may issue
new server certificates without changing the host's trust. For CA rotation,
import an old-plus-new bundle, renew the server, then remove the old CA.
Trust changes take effect after a host restart; say explicitly that this
parks adapters and needs normal session resume. Ordinary leaf renewal under
the same CA needs no host restart.

Certificate pinning is an alternative, not another spelling of a CA bundle.
A leaf-certificate digest breaks on every renewal; an SPKI pin survives
renewal only while the key stays the same. A pin-only mode additionally needs
a carefully reviewed name/validity policy and rotation mechanism. Recommend
the private-CA mode for this delivery and defer pinning; do not implement a
permissive verifier to make a self-signed leaf work. Publicly trusted
certificates remain the easiest choice for browsers and agents too.

## 4. User-visible flows and persistence

### 4.1 New pairing

The existing `hennery host join <https-origin> [<code>]` works as printed by
Hosts → Add host. Omitting the code keeps the existing bounded stdin input.
The optional `--ca-cert <file>` chooses private-CA trust; omitting it for a
new pairing selects system trust. `host run` reads the saved policy and needs
no repeated TLS flags. The local all-in-one path retains loopback WS.

Take `host.lock` before pairing-state reads or writes (or receive the
caller's held lock for `up`'s internal pairing); a running host must be
stopped before CLI `join` can alter its identity. Before spending a code,
validate the URL, CA data and destination storage, then preflight the actual
WSS upgrade and nonce without sending a hello (WS on the local path).
Close it within a deadline. This catches a proxy with working HTTPS but no
upgrade or nonce forwarding. It proves reachability now, not that the later
enrollment and connection cannot fail. Send the enrollment POST once through
the same TLS policy. Do not automatically retry a POST whose result is
unknown; disable lower-layer retries for enrollment too.

Keep `EnrollRequest { code, public_key, name, host_version, platform }`,
`EnrollResponse { host_id }` and their existing errors unchanged. Validate
the returned host id and bound the response before storing it. Use the
existing pending-key/config commit protocol; store the trust policy together
with the URL, host id and enrolled public key in the pending configuration.
A confirmed enrollment followed by a failed host connection reports “paired;
connection failed” and leaves that identity available for retry. Runtime
installation failure also leaves a completed pairing intact, as today.

**Enrollment uncertainty is durable state, not an error-message convention.**
Before the POST can begin, persist and fsync both `host.key.pending` and a
0600 `enrollment.pending.toml` journal, including their directory entries.
The journal's Rust shape is `EnrollmentAttempt { version, attempt_id,
phase: MayHaveCommitted, collector_origin, trust, public_key, prior_host_id }`.
It binds the pending key to the exact destination and trust policy; it stores
no pairing code. Generate it under `host.lock`, refuse to overwrite an
existing attempt, and send no POST if either write or fsync fails. Marking
the attempt before sending deliberately also covers death just before the
POST: local state cannot prove whether the request reached the collector.

A timeout, connection reset, proxy 502/504, truncated or malformed success
body, invalid returned host id, or process death after the POST can all leave
the collector enrolled while the host lacks a confirmed result. Keep the
journal and its key unchanged. Neither a failed request nor a fresh code
authorizes another POST or replacement key. Only a complete, validated
collector rejection whose contract guarantees no enrollment (such as
`invalid_code`) permits clearing the attempt; generic HTTP errors and
`already_paired` do not prove that. A crash before clearing even a definitive
rejection leaves the conservative uncertain state.

On validated success, durably stage `host.toml.pending` with the returned id,
destination, trust and matching public key before installing the pairing.
That staged receipt permits the existing roll-forward without another POST.
Remove and directory-fsync the journal only after the matching installed
key/config are durable. A leftover journal with that complete matching
staged receipt or installed pairing is confirmed recovery work; a mismatch
is an error, never guessed away. Failures while staging the receipt still
leave the original uncertain attempt. Do not delete its key in a generic
enrollment-error cleanup.

Every entry point inspects these states before normal pairing loads:

- `join` blocks on an uncertain attempt before prompting for a code,
  generating a key or sending enrollment. A supplied code does not bypass it.
- `host run` refuses to start an uncertain identity. `up` propagates that
  diagnostic and suppresses automatic host pairing/re-enrollment, including
  supervisor restarts; it must not treat the state as “not paired”.
- `doctor` reports a failure with the attempt id and sanitized destination,
  distinguishing uncertain enrollment from confirmed roll-forward work. It
  changes no files and sends no identity probe for the uncertain attempt.
- `transport set` also refuses an unresolved attempt. A lone pending key,
  journal without its matching pending or installed key, or malformed or
  mismatched journal is a blocking recovery condition, never ignored or
  overwritten. This also protects
  artifacts left by the old implementation, even when the destination is
  unavailable from them.

Explicit recovery means the operator checks the collector's Hosts list,
revokes any entry created by this attempt, and deliberately archives the
attempt's journal and key under the host lock before a fresh join. The
implementation plan must provide a local recovery action bound to the
attempt id that explains the possible remote enrollment and preserves those
artifacts durably before clearing the active attempt; it cannot assert that
a remote revoke happened from local state. Interrupted archive recovery
must remain recoverable and must not permit an automatic fresh enrollment.
No timeout, restart or ordinary join performs that action. Preserve an older
pairing and its outbox throughout. Automatic discovery of a lost host id is
outside this change; there is no new unauthenticated key-lookup endpoint.

For an existing pairing, `join` retains the stored trust when no TLS option
is supplied and performs the authenticated probe before asking for a code.
`Accepted`/`Connected` spends none; `Revoked` permits the existing new-key
flow; `Unknown` refuses. A different endpoint or requested trust policy uses
the transport-change flow below rather than silently mutating a pairing.

### 4.2 Tunnel migration and trust changes

Propose `hennery host transport set <https-origin>` with mutually exclusive
`--ca-cert <file>` and `--system-roots`, plus the existing host data-directory
selection. This is local operator configuration, not a browser API.

1. Require an existing pairing and take `host.lock`; refuse while its service
   runs. Tell the operator to park sessions and stop the host service first.
2. For a different origin require an explicit trust choice; for the same
   origin an omitted choice retains the stored policy. This prevents a
   private CA silently becoming trusted for another destination.
3. Validate and probe the requested endpoint over verified TLS with the
   existing key and id, no attached sessions, no `resend_complete` and no
   pairing code. Only `Accepted` or `Connected` permits saving. For the
   latter, explain that another connection still uses this identity.
4. Atomically replace only the endpoint and trust fields in `host.toml`,
   preserving host id, key, workspace roots, CLI overrides and outbox. Before
   the rename, a failure leaves the old configuration byte-for-byte intact.
   After a successful rename, a directory-fsync failure means the new file
   is visible but its survival across a crash is uncertain. Report
   `DurabilityUncertain`, not “unchanged” or success; do not attempt an
   automatic rollback or begin connecting. Read back the file when possible
   for the diagnostic and direct the operator to inspect the saved endpoint and
   repeat the explicit save once storage is healthy. After a restart, validate
   whichever complete configuration survived; never assume the old one won.
   Only a durably completed save prints the ordinary restart instruction,
   without claiming that sessions are already connected.

This authorizes a new endpoint by the local operator's explicit URL and TLS
trust, not by treating a `hello_ack` as a collector signature. A malicious
endpoint with a valid certificate can lie about accepting a host; the host
proof does not prove two origins are the same installation. No automatic
cross-origin redirect or discovery is added. A rejected move never suggests
deleting the old key as the way to repair a certificate failure.

Restrict the legacy loopback alias exception to **two validated plaintext
loopback endpoints** (`http`/`ws` after the normal HTTP-to-WS conversion).
Only that case may equate different loopback names or ports. If either
endpoint is HTTPS/WSS, require exact canonical endpoint equality and the
stored trust, or the explicit endpoint/trust-change flow. A stored
`wss://localhost` pairing must never probe a plaintext loopback candidate.
WSS-to-WS downgrades are refused; the proposed `transport set` takes HTTPS
destinations only. WSS-to-WSS name or port changes require explicit handling
even if both names resolve to loopback; WS-to-WSS is an explicit upgrade.

Apply this comparison **before any hello** in `join`, `host run`, `up`,
doctor's derivation of `up`'s local endpoint, and `--collector-url`. Neither
`up`'s listener-derived URL nor the development override bypasses a stored
WSS endpoint or its trust. Refuse incompatible overrides with a migration
diagnostic, rather than probing them. Plaintext-to-plaintext aliases still
serve existing all-in-one installations. Never equate loopback with a public
origin or copy private-CA trust across origins implicitly.

### 4.3 Storage and compatibility

Extend `host.toml` with optional `[collector_tls]`, containing `mode =
"system"`, or `mode = "ca_bundle"` plus `ca_pem`. Missing means system trust.
Reject unknown modes, extra trust fields and invalid combinations rather
than silently reverting to defaults. Keep the existing `collector` WS(S)
field; do not persist two independent URLs. CA data is public but its
integrity matters: maintain 0600 atomic writes, refuse group/other-writable
transport configuration and redact PEM from diagnostics and `Debug` output.

The versioned enrollment journal is new local persistent state. Loaders must
check it and pending-key fragments before interpreting absent `host.toml` as
an unpaired host; this replaces the current lone-pending-key behavior. Its
writes and pairing/transport writes distinguish pre-rename failure from
post-rename durability uncertainty, including failures removing the journal.

No SQLite migration, host-registry column, frame field or protocol-version
bump is needed. Old loopback configurations keep working. Old binaries do
not understand this trust contract and must not be used after adopting it;
document upgrade-before-migration and restoring the previous tunnel config
before a binary rollback. Trust is a local deployment setting, never sent
in hello or served to the browser.

## 5. Reverse proxy and collector TLS

### 5.1 Supported deployment

Use one HTTPS origin, a TLS-terminating proxy and one collector. Place the
plaintext backend on loopback or an isolated same-machine container network
with access restricted to the proxy. A plaintext hop across an untrusted LAN
is outside the supported secure topology; use an encrypted deployment link
instead. Hosts need outbound access only. Public URL still controls browser
Origin checks, passkey RP identity, OAuth callbacks and push links.

The deployment recipe must cover `/api/hosts/enroll`, `/api/hosts/ws`,
`/healthz`, `/readyz`, browser/SSE routes and `/mcp/*` on the same origin:

- HTTP/1.1 WebSocket upgrade to the collector, with `Upgrade`/`Connection`
  forwarded where the proxy requires it and `hennery-hello-nonce` preserved
  in the 101 response. Preserve `Host`, `Origin` and normal browser security
  headers; do not synthesize a trusted browser Origin.
- No login page, redirect, response cache, body logging, or automatic POST
  retries on enrollment. No proxy authentication bypass inside hennery.
- No buffering/compression of WebSocket or SSE traffic; preserve MCP
  streaming. Allow the existing 32 MiB WebSocket message limit and current
  HTTP route-specific body limits; do not apply the enrollment limit to
  image prompts.
- Idle timeout greater than the 45 s read deadline, recommended at least
  120 s, and no short absolute stream lifetime. Ping/pong must pass. Proxy
  reloads may close sockets; the host reconnects normally.
- Edge connection/request caps for unauthenticated upgrade floods. A TLS
  handshake or open WebSocket is not yet an authenticated host.

Ship tested Caddy and nginx examples in the implementation plan. nginx
requires explicit handling of upgrade headers; its documented idle timeout
also makes the heartbeat relevant. Caddy provides WebSocket proxying, with
stream lifecycle behavior to verify during reload. References:
[nginx WebSocket proxying](https://nginx.org/en/docs/http/websocket.html),
[Caddy reverse proxy](https://caddyserver.com/docs/caddyfile/directives/reverse_proxy).
Tailscale Serve remains a supported TLS front end; qualify its client-address
behavior separately instead of assuming it matches either recipe.

### 5.2 Client addresses for rate limits

Do not erase the README warning merely because WSS works. Recommend an
opt-in, **address-only** proxy policy in collector `config.toml`:
`proxy_client_ip_peers: Vec<IpAddr>`, default empty, at most eight exact
peer addresses. Use a fixed header, `Hennery-Client-IP`, to avoid ambiguous
forwarded chains and accidental trust of a common user-supplied header.

The last proxy strips every incoming instance and sets exactly one header to
the real client IP it established. For a single directly exposed proxy this
is the connection peer. Multiple ingress hops require that proxy's separately
configured, tested trust chain; hennery does not guess the leftmost XFF entry.

Resolve a Rust `ClientAddress { peer: IpAddr, effective: IpAddr, source }`
once. Use `effective` for enrollment, password login, password step-up and
passkey limiter charging **and success clearing**. Preserve the existing
budgets, IPv6 /64 grouping, IPv4-mapped normalization, bounded storage and
overflow behavior. Do not use it for owner, hat, session or host identity.

- Unconfigured peer: ignore all client-address headers and use TCP peer.
- Configured peer with no header: use TCP peer, preserving direct loopback
  clients such as `up`'s enrollment and local diagnostics. This fallback is
  visible in deployment diagnostics; it does not prove proxy attribution.
- Configured peer with the header: require one field and one bare IP literal,
  no list, port, zone, hostname, control characters or duplicate. Malformed
  input is 400 before authentication work, with no reflected value. Refuse
  unspecified and multicast values. Canonicalize before limiter lookup.
- Ignore `Forwarded`, `X-Forwarded-For`, `X-Real-IP`, forwarded scheme/host
  and identity headers for this purpose. Never infer TLS, Secure cookies or
  operator authentication from them.

Only collector-machine configuration enables this. Trusting a loopback proxy
also trusts other local processes to assert limiter addresses; this affects
abuse accounting, not authentication. The local same-user threat model is
already stronger (kernel §10). Restrict backend access and keep edge caps.
The recommendation is a narrow amendment to kernel §§3.2, 4.1 and 7's client
address interpretation; it does not undo umbrella §7.1's ban on header login.

Without this support, a same-machine proxy still works functionally but
retains the documented shared-budget denial of service. Edge rate limiting
alone does not repair the backend's shared lockout. The public-deployment
recipe is complete only after two-client tests demonstrate separate budgets;
for a front end unable to provide authenticated client attribution, document
the limitation and recommend restricting ingress to a trusted VPN population.

### 5.3 Should the collector terminate TLS?

| Choice | Benefits | Costs and recommendation |
|---|---|---|
| Reverse proxy / Tailscale Serve | Existing v1 contract; certificate issuance/renewal outside the collector; browser, SSE, MCP and WSS share the origin; mature edge controls. | Trusted plaintext termination boundary, proxy setup, upgrade and client-address configuration. **Keep for this delivery.** |
| Native TLS from supplied cert/key files | One fewer service; direct peer addresses; no plaintext backend hop. | New listener configuration, key permissions, certificate-chain loading, atomic reload/expiry behavior, container/service packaging and doctor coverage. Does not solve certificate issuance or renewal. Separate future design if requested. |
| Native ACME or automatically generated self-signed certificate | Fewer initial commands in selected environments. | ACME challenge/renewal/storage ownership; or manual trust on every host, browser and agent plus rollover outages. Built-in ACME is explicitly outside v1; automatic self-signed trust is not a safe default. |

Kernel §7 already says the collector does not terminate TLS in v1. No
operator question is needed to retain that decision. Native TLS would require
an explicit amendment; neither changing `public_url` nor enabling WSS means
the collector's existing listeners become TLS listeners.

## 6. Reconnect and error behavior

Each new connection repeats DNS/TCP/TLS validation and obtains a fresh nonce.
Bound the combined DNS, TCP, TLS and upgrade attempt to ten seconds, including
fallback across addresses; do not multiply that deadline by address count.
Disable TLS early data. Keep the separate hello deadline and the existing
15 s ping / 45 s read timers.

Retain exponential backoff from 500 ms to 30 s and reset only after the first
committed ack or sixty healthy seconds, never on TLS or hello success alone.
Add bounded jitter to satisfy umbrella §5.8: choose a delay uniformly between
half and all of the current doubled/capped ceiling, with a 500 ms floor.
An injected clock/RNG makes boundary tests deterministic. Respect a valid
HTTP 429 `Retry-After` by waiting at least the normal backoff, capped at five
minutes; malformed values use normal backoff and never enter logs verbatim.

TLS validation failures retry with this backoff, retain adapters and outbox,
and report the specific repair. Do not downgrade to WS, switch trust mode,
re-enroll or interpret HTTP 401/403 as host revocation. Configuration errors
fail startup before spawning adapters. A running connection is unaffected
by file edits until restart; certificate expiry is checked on new TLS
handshakes, not as a timer that kills an established stream.

On recovery, resend unacked facts, deduplicate by the existing sequence keys,
then `resend_complete` and reconcile before dispatching queued answers or
new commands (ACP core §5). Proxy/collector restarts must not restart adapters
or replay delivered prompts. Host restart remains different: it parks
sessions and requires explicit resume.

Suggested internal error categories are `InvalidEndpoint`, `TrustConfig`,
`Dns`, `Connect`, `Tls`, `Upgrade { status }`, `MissingNonce`, `HelloTimeout`
and `HelloRejected { code }`. Classify structured source errors, not a search
for “tls” in text. TLS subreasons distinguish unknown issuer, name mismatch,
expiry/not-yet-valid and protocol failure where the library supplies them.
Errors expose a sanitized origin, stage and remedy, never PEM, codes, keys,
proofs, tokens, response bodies or arbitrary proxy headers.

## 7. Doctor

Keep `ok | warn | fail`, fixes and existing exit semantics (1 on any failure).
No new browser API or independently handwritten TypeScript model is needed;
if a later host-report lane publishes these types, generate its wire types
from Rust through `hennery-proto` as usual.

- **Check 7:** first report unresolved enrollment as specified in §4.1,
  without network pairing work. Otherwise show the effective collector origin
  and trust source, then
  configuration, DNS, TCP, TLS/name/chain, HTTP health, WSS upgrade/nonce and
  authenticated hello. Reuse actual transport code and overall deadlines.
  A failed certificate check stops before a code or hello is sent. A 200
  health response is never presented as proof of working WSS.
- Preserve the `host.lock` rule: while a host may be running, do not send
  hello or compete for its identity. A bounded unauthenticated upgrade/nonce
  check is permitted; label authentication **not checked**. When idle, the
  probe never sends `resend_complete`; acknowledge that accepted hello can
  update last-seen metadata. Do not turn doctor into a config repair tool.
- **Check 8:** use `Date` only from the successfully verified health response;
  warn over 30 s and fail over five minutes as today. Missing Date warns.
  TLS failure may prevent a Date sample; suggest checking the local clock
  without making an unverified retry. A proxy may supply Date: this checks
  the endpoint clock, not proof of the collector process's own clock.
- **Check 14:** include transport-config integrity and parsing; inspect
  private keys only through existing identity handling, never print them.
- **Check 16:** distinguish local HTTP listeners from the external HTTPS
  origin. Add `doctor --collector-ca-cert <file>` for a collector-only
  external health check using a private CA; it is read-only and does not
  override check 7's saved host trust. Retain the explicit limitation that
  this check reads configuration rather than the database's saved origin.
  Show whether proxy-address attribution is configured, and warn that health
  success cannot prove header stripping or separate limiter budgets.

Fixes name the stage: install the intended CA, renew a certificate, use its
correct name, forward the upgrade/nonce, remove an ingress login redirect,
restore reachability, or re-pair **only** for the existing revoked/unknown-key
cases. Do not recommend disabling validation.

## 8. Security and failure boundaries

WSS protects the host proof, enrollment code, transcripts, commands, answers
and session MCP tokens from a network observer or active attacker outside
the selected trust chain. It does not protect against a compromised paired
host, trusted TLS terminator, collector account, CA, or agent running as the
same OS user with access to the host key/config. CA selection is authority
over this collector connection; keep it out of remote session commands.

Owner-filtered queries, hat resolution and MCP mount/token scope remain
unchanged. A client-address header never supplies an `owner_id`, `hat_id`,
host id or token. Mint/revoke still require existing step-up. Enrollment and
host WS remain Origin-exempt because they use their own credentials; browser
routes retain exact `public_url` Origin and fetch-site checks on every
listener, without CORS or proxy identity exemptions. Moving a host endpoint
does not change the collector's public URL or bypass that setting's session,
passkey, OAuth and push consequences.

Private-CA trust here covers hennery's host transport only. Adapters contact
`/mcp/*` independently, and browsers/phones have their own trust stores.
Do not distribute CA environment variables broadly to agent processes or
install a global CA as a hidden side effect. A self-hoster must configure
those clients separately; the live gate tests this distinction. The phone
lane must not assume this flag makes a private certificate acceptable to iOS.

| Failure | Result and recovery |
|---|---|
| Unknown CA, wrong SAN, expired certificate, TLS sent to a plaintext port | Fail before HTTP/hello secrets; retain identity and retry on the running host. Repair certificate/trust/clock. |
| Health works, upgrade is redirected or nonce stripped | Preflight/doctor identifies the stage; no enrollment during failed preflight. Repair the proxy. |
| Enrollment result unknown, including reset, 502/504, malformed success or process death | Durable journal retains destination, trust and pending key; `join`, `run`, `up` and doctor recognize it after restart. No automatic POST or key replacement; explicit recovery only (§4.1). |
| Proxy/collector outage or address change in DNS | Backoff and reconnect; adapters keep running, bounded outbox rules still apply. No endpoint discovery or automatic re-pair. |
| Certificate renewed under the same CA | New connections validate normally; no pin or pairing changes. |
| Trust bundle replaced or origin moved | Explicit local change under the host lock, atomic save, restart/resume; failed probe preserves old settings. |
| Loopback alias or override would downgrade WSS to WS | Refuse before hello, including in `up` and doctor. Only two plaintext loopback endpoints qualify as legacy aliases. |
| Directory fsync fails after a config rename | New config is visible; crash durability is uncertain. Report that distinction, without rollback or an “unchanged” promise (§4.2). |
| Proxy passes spoofed or duplicate client-address headers | Reject malformed trusted-peer headers; deployment test must prove inbound headers are overwritten. Default mode ignores headers. |
| Revoked, bad proof or duplicate live identity | Preserve ACP core refusal ordering and existing remedies; only authenticated `revoked` shuts down adapters. |

## 9. Verification strategy

Use isolated fixture data directories and the repo's process helpers. No test
changes real trust stores, uses the operator's live data, or restarts a
deployed collector. Run on Linux and macOS; a build without TLS enabled must
fail a positive WSS test, not merely pass rejection tests.

1. **Transport matrix:** real local TLS server and collector, generated test
   CA/leaf chains with controlled SANs and dates. Exercise enrollment, probe,
   host reconnect and doctor under system roots (injected loader in unit
   tests) and explicit CA mode. Test untrusted root, wrong DNS/IP SAN,
   expired/not-yet-valid leaf, malformed/empty bundle, private-key input,
   missing intermediate, plaintext-on-TLS and duplicate/malformed nonce.
   Capture server-side requests to prove no enrollment or hello crossed a
   failed verifier. Test actual native root loading separately on both OSes.
2. **Policy parity:** reject credential-bearing URLs, non-root paths,
   redirects/downgrades, non-loopback plaintext and poisoned `localhost`
   resolution on every call site. Proxy traps receive nothing. Test stored
   configuration and the development override as well as CLI parsing.
   Cover every loopback scheme/name/port combination: plaintext aliases
   remain accepted; any WSS transition needs exact equality or explicit
   secure migration. Drive `join`, `run`, `up`, doctor and `--collector-url`
   against a plaintext trap and assert **zero hello frames**, particularly
   from a stored `wss://localhost` pairing.
   Prove that public-only egress and runtime downloads retain their policy.
3. **Pairing/storage:** successful HTTPS pairing and idempotent rerun;
   unspent code on preflight failure; rejected trust changes; write failures
   at pending-file boundaries; lost enrollment reply; post-enroll upgrade
   failure; revoked re-pair and orphaned outbox; transport move preserving
   identity, roots, overrides and outbox. Inject failures before rename and
   after rename at directory fsync: assert old bytes only in the former;
   assert new visible bytes plus `DurabilityUncertain`, no automatic rollback
   and correct read-back/retry behavior in the latter. Both old loopback
   configurations and private-CA service startup need positive tests.
   Crash at each key/journal/POST/receipt/config durability boundary. For
   timeouts, resets, 502/504, truncated/malformed success and process death
   after a committed POST, restart each of `join`, `run`, `up` and doctor:
   assert no second enrollment, no regenerated/overwritten key, preserved
   destination/trust and no loss of an older identity/outbox. Also cover
   death before POST, lone pending artifacts, explicit archive recovery,
   definitive rejection cleanup and confirmed receipt roll-forward without
   POST. Journal cleanup failure must not turn confirmed recovery into a
   new enrollment.
4. **Reconnect:** put a fake adapter in an active turn with a pending
   question, cut the proxy, produce facts offline, reconnect and verify one
   adapter PID, exact ordered transcript, one delivery acknowledgement and
   no duplicated prompt. Exercise collector restart, renewal, flapping
   upgrades, jitter bounds and read deadline. A revoked host still stops.
5. **Real reverse proxies:** execute the shipped Caddy and nginx recipes
   with fixture certificates. Verify nonce forwarding, idle traffic beyond
   proxy timeout, reload/reconnect, large frames, SSE/MCP streaming and Origin
   refusals. A two-client test locks one login/enrollment budget without
   locking the other; successful login clears only that client's budget.
   Include direct backend access, untrusted peer, spoofed headers, duplicates,
   comma lists, mapped IPv4, IPv6 grouping and missing-header fallback.
   Differential header vectors must not produce competing interpretations.
   Have the proxy lose a committed enrollment response with reset, 502/504
   and truncated/malformed success, then restart the host entry points and
   prove durable refusal to re-enroll or replace the key. Reject a proxy
   redirect or override from WSS to a plaintext loopback trap with zero
   hello frames. Exercise the deployment recovery instructions after an
   injected post-rename fsync failure and distinguish visible settings from
   durability, rather than claiming the old settings were restored.
6. **Doctor:** each stage and each verdict has a positive assertion and a
   revert-probe. Include a URL containing “tls” whose failure is DNS, no
   Date, skew, active host lock, accepted/revoked/unknown/connected hello,
   public-origin check with a CA, and health success with broken WSS.
7. **Live acceptance:** on a disposable HTTPS deployment, pair a remote host
   without a tunnel using system roots. With both pinned Claude and Codex
   adapters, start a session, stream a turn, answer a question, interrupt the
   proxy, recover without adapter respawn, then park/resume and revoke.
   Verify one session's mounted MCP through the same public origin. Repeat
   transport and MCP checks with a private CA and separately configured
   adapter trust. Verify Caddy renewal/reload, nginx upgrade behavior and a
   Tailscale Serve deployment if it is advertised. Record versions, actual
   outcomes and any unavailable credentials; fake adapters alone cannot
   establish this acceptance.

The implementation plans run the full repo Rust/web/generated-code checks
once per completed task as required by AGENTS, plus their targeted tests.
Every new runnable quickstart/proxy command gets a test or CI runner. This
design-only change needs link, diff and scope checks, not a claim of passing
runtime tests.

## 10. Decisions

| # | Decision and status | Choice; alternatives and reason |
|---|---|---|
| 1 | **settled by spec** — umbrella §§7.5–7.6, kernel §4.1 | Verified HTTPS/WSS off loopback. Reject plain LAN WS and automatic downgrade. |
| 2 | **settled by spec** — ACP core §3.5 | Retain nonce-bound Ed25519 hello and per-host revocation. Reject shared fleet tokens and mTLS as a prerequisite behind terminating proxies. |
| 3 | **settled by spec** — kernel §7, umbrella §15 | External TLS termination in v1; no native ACME. Native cert/key listeners have benefits but need a later scope amendment. |
| 4 | **designer's recommendation** | Native roots by default; explicit collector-only private-CA roots replace defaults. Reject bundled-roots-only behavior and an implicit union in private-CA mode because trust should be predictable and narrow. |
| 5 | **designer's recommendation** | Deliver CA bundles, defer pinning. Leaf pins add renewal work; SPKI/pin-only modes add verifier and rollover surface without being necessary for this brief's private-CA-or-pin requirement. |
| 6 | **designer's recommendation** — refines kernel §7.1; plan 7d-ii decision 21 | Host-owned transport for a locally authorized destination, retaining no-proxy/no-redirect/bounds and strict loopback HTTP. Do not route through public-only egress or broaden its caller allowance. |
| 7 | **designer's recommendation** | WSS preflight before new enrollment; durably journal possible enrollment before POST and block automatic re-enrollment after restart. Reject treating a pending key alone or an in-memory error as sufficient recovery state. |
| 8 | **designer's recommendation** | Persist CA PEM with pairing; explicit transport changes under host lock; legacy aliases only between plaintext loopback endpoints. Distinguish pre-rename failure from post-rename durability uncertainty. Reject silent WSS downgrade, transient trust and deleting an identity to move off a tunnel. |
| 9 | **settled by spec** — umbrella §5.8, ACP core §5 | Adapters survive disconnection; outbox reconciliation precedes readiness; backoff with jitter. Reject reconnect-driven respawn and reset-on-dial behavior. Exact jitter distribution is a designer's recommendation (§6). |
| 10 | **designer's recommendation** | Explicit exact proxy peers plus one overwritten address-only header; preserve all limiter budgets. Reject trusting arbitrary XFF, treating loopback as authenticated, disabling backend limits or claiming edge limits alone fix shared lockout. |
| 11 | **settled by spec** — umbrella §§7.1–7.4, kernel §§3.3–3.4 | No proxy identity; keep owner/hat/token checks, step-up and browser Origin semantics. Transport trust never grants operator access. |
| 12 | **designer's recommendation** | Two ordered M plans with real-proxy and live-adapter gates. One dependency toggle would leave join, diagnostics, migration and public-ingress behavior inconsistent. |

**OPERATOR QUESTIONS:** none required for the proposed scope. Existing specs
settle native TLS's exclusion; technical recommendations above still need
independent review. If native TLS or pin-only authentication is desired for
this release, that is a scope change, not an implicit decision in this doc.

## 11. Plans and handoffs

Sizes measure implementation plus verification, not elapsed time. Each plan
uses plan 3c's structure, built code embedded verbatim, replay, revert-probes,
task reviews and whole-branch review. The parent commissions the reviews.

| Order | Plan | Size | Deliverables and completion gate |
|---|---|---|---|
| 1 | **WSS host transport and diagnostics** | **M**, approximately 5–6 tasks | Shared endpoint/trust/connector; HTTPS enrollment and pending config; migration/CA command; reconnect jitter; doctor parity; TLS, storage and disconnect tests. Old tunnel path works. Amend the umbrella/kernel “built so far” notes only when implementation exists. Real remote host with both adapters passes the transport gate. |
| 2 | **Proxy attribution and public deployment** | **M**, approximately 3–4 tasks | Exact-peer/header policy shared by all rate-limit consumers, config and doctor findings, Caddy/nginx examples and two-client regression tests, README quickstart and acceptance evidence. Review the kernel address-policy amendment. Replace the tunnel-first instructions only when tested WSS and public-ingress gates pass. |

Required review-amendment tests in **plan 1**: §9's persistent uncertainty
matrix across all four entry points and each crash boundary; no enrollment
replay or key replacement after restart; no hello to a plaintext downgrade
target through aliases, `up` or overrides; and separate assertions for
pre-rename failure and post-rename directory-fsync uncertainty. Test the
recovery action and receipt roll-forward, not just the error text.

Required review-amendment tests in **plan 2**: §9's actual proxy reset,
502/504 and truncated/malformed-response cases followed by process restart;
WSS-to-plaintext redirect/override traps receiving no hello; and deployment
recovery instructions exercised with an injected post-rename fsync failure.
These are integration checks of plan 1's state machine, not a second one.

Plan 2 depends on plan 1 for its end-to-end deployment checks. Plan 1 can ship
WSS with the current warning retained; it must not claim the unrestricted
public deployment recipe is finished. Preserve the SSH tunnel as an optional
deployment choice. Native TLS would be a separate **M–L** plan plus an
approved spec amendment; pinning a separate **S–M** plan if later requested.

Review focus: TLS policy parity and no downgrades; enrollment uncertainty;
identity-preserving endpoint changes; proxy-header spoofing and limiter
success clearing; certificate renewal and reconciliation; no trust leakage
into gateway, push or agent processes. Independent review found no Critical
issues and approved after the three amendments above; the parent should
request scoped re-confirmation of durable enrollment uncertainty, loopback
transitions and post-rename durability reporting. Final approval is pending.

Other-lane assumptions:

- **Phone:** same HTTPS `public_url`, proxy and unbuffered streams. Host CA
  configuration does not provision device trust or relax secure-context
  requirements. Share deployment acceptance; no service-worker dependency.
- **Delegation and memory:** unchanged host protocol, owner/hat scope and
  session MCP token path. Their sessions tolerate existing `host_offline`
  and delivery-unknown behavior. Private-CA MCP trust remains the adapter's
  separate deployment concern; do not invent a second host credential.
- **Distribution/host reports:** CLI doctor is included; a persisted remote
  doctor report, native TLS listeners and the read-only database public-URL
  lookup remain separate work. No new generated web type is required here.

Generated with Claude AI — please review before distribution.
