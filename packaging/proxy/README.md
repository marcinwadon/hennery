# Same-machine HTTPS/WSS deployment

Use one TLS origin and one collector, with the plaintext backend on loopback.
A plaintext LAN hop is outside this recipe. The templates route enrollment,
health/readiness, WebSockets, browser/SSE and MCP through the same origin.
They preserve Host, Origin and the 101 nonce; they neither synthesize browser
identity nor retry enrollment. No response cache, access/body logging or
stream compression is enabled. nginx disables request/response buffering;
Caddy flushes immediately and lets streams survive a reload for five minutes.
The collector still enforces each route's body limits and 32 MiB WSS messages.
Hennery sends `X-Accel-Buffering: no` on SSE responses, so nginx needs no buffering tweak for SSE.

Choose [Caddyfile](Caddyfile) or [nginx.conf](nginx.conf). These exact files
are executed by `hennery-testkit/tests/reverse_proxy.rs` and the CLI deployment
and README tests, using disposable CA/leaf certificates and loopback ports.
Replacements use the explicit placeholders below. The heartbeat/renewal tests
also scale the fixture's idle durations from 120 s to 8 s and Caddy's body
deadline from 30 s to 2 s, retaining nine ping/pong rounds past both durations.
This checks that body deadlines do not leak into upgraded sockets. The recipe policy test pins the production
durations. Caddy's `keepalive` governs idle upstream connections; nginx's
read/send timeouts also cover upgraded WebSockets.

| Placeholder | Deployment value |
|---|---|
| `@TLS_PORT@` | Your HTTPS listener port, normally 443 |
| `@BIND_V4@`, `@BIND_V6@` | Frontend listener addresses, normally 0.0.0.0 and ::; the backend remains loopback |
| `@BACKEND_PORT@` | Collector loopback port, normally 7117 |
| `@CERT@`, `@KEY@` | Absolute certificate-chain and private-key paths, readable by the proxy; protect the key |
| `@ROOT@` | nginx's writable private runtime directory, for PID/log/temp files |
| `@ADMIN_PORT@` | Caddy's loopback-only admin port; do not expose it |

Supply certificates for the actual public DNS name through your deployment's
certificate management. This Caddy template uses explicit PEM files and turns
ACME off; the test does not contact certificate services. Renew the leaf under
the same CA, atomically replacing each complete PEM file, then reload:
`caddy reload --force --config <rendered-Caddyfile>`
(with its configured local admin address) or send SIGHUP to your nginx master.
nginx can inherit cached certificates when both file inode and modification time
are unchanged; replacing files avoids that same-timestamp renewal trap. See
[nginx SSL object inheritance](https://nginx.org/en/docs/ngx_core_module.html#ssl_object_cache_inheritable).
The fixture replaces the files, runs reload and observes a new leaf over the
same trust.

Before setup, use the README's `up-public` block with your public HTTPS origin.
Already set up? Use the existing admin public-URL reset: changing the flag alone
does not replace the stored browser origin and passkey identity. Remote hosts
use the README's `join-public` block and saved WSS endpoint. An SSH tunnel is an
optional fallback. A private host CA never configures browser or adapter trust.

In `<collector-data>/config.toml`, set `proxy_client_ip_peers = ["127.0.0.1"]`
for these templates. Only the final proxy's exact TCP peer is trusted; changing
the upstream address to IPv6 or a restricted container IP requires explicitly
changing this setting. It accepts at most eight exact unicast addresses, no
CIDR/hostname/port or canonical duplicate. Both templates overwrite all incoming
`Hennery-Client-IP` values with the socket peer, including duplicate spoofed
fields. Other forwarding fields have no attribution or authentication effect.
Missing dedicated headers use the TCP peer, allowing `up`'s direct enrollment;
that fallback also retains the shared budget if the proxy is misconfigured.
Keep backend access restricted. Local processes able to reach a trusted loopback
peer can assert budget addresses; this grants no operator access.

nginx caps connections per address at 20 and requests at 100/s with a burst of
200, including unauthenticated upgrades, with 256 active requests across the
server. Both recipes bound header reads to 10 seconds; Caddy bounds complete
body reads to 30 seconds, and nginx bounds idle body reads to 10 seconds.
Tune deployment capacity deliberately.
Caddy core does not supply a per-client connection/request limiter: add and test
firewall/ingress caps before public exposure. Backend rate limits do not bound
TLS handshakes. The collector separately caps pending host handshakes at four
per client and 64 total, with a ten-second deadline. HTTP requests keep their
capacity reservation through the response body, including SSE. A second ingress hop requires
its own tested trust chain; never take the leftmost user-supplied XFF address.
The optional memory plug-in uses the same attribution through its own
[recipe](memory.md).
Tailscale Serve is separately qualified: if it cannot overwrite authenticated
client addresses, retain the shared-budget warning and restrict ingress to a
trusted VPN population.

The collector's HTTP/1 listener caps connections at 512 and header buffers at
16 KiB. Small auth/setup/enrollment JSON bodies have a five-second deadline;
other browser JSON bodies have 30 seconds. These are request/read limits,
not a timeout on a running agent or an SSE response.

New collectors advertise `Hennery-Hello-Auth: 1` with the existing upgrade
nonce. New hosts first send a small signed `authenticate`, then the full
`hello`. Before the signature is verified, a 64 KiB total wire budget is
enforced on frame headers before the WebSocket parser can reserve the declared
payload. Fragmented frames share that budget. Authenticated messages retain
the existing 32 MiB limit. Older collectors receive the ordinary hello; older
hosts can send a hello that fits the new budget. Upgrade hosts before the
collector if their large configuration/reconciliation state exceeds 64 KiB.
No extra round trip or stored credential is introduced.

## Public deployment gate

Fixture gates run through plain `nix develop -c`: the `client_address` kernel
suite, `proxy_attribution` and `reverse_proxy` testkit suites, and the binary's
`deployment` and `readme` suites. CI exposes the flake's Caddy/nginx binaries to
its Linux/macOS Rust jobs; unavailable binaries fail tests, never silently skip.
These gates require overwritten duplicates, two separate login/enrollment
budgets and client-specific success clearing, preserved nonce/Origin, 32 MiB
frames, ping/pong traffic past the fixture's configured idle duration, SSE/MCP
chunks before stream end, certificate reload and reconnect, and direct
backend/spoof refusal. CLI checks
lose committed replies through both proxies (reset, 502, 504, truncated and
malformed success), restart join/run/up/doctor without a second POST/key, reject
plaintext redirects/overrides with zero hello, and retry a visible transport
save after an injected post-rename directory-fsync failure.

**Public acceptance remains open.** Before advertising unrestricted public
exposure, record a disposable remote system-root HTTPS deployment with both
pinned Claude and Codex adapters: pair without a tunnel, stream a turn, answer a
question, interrupt/recover without adapter respawn, park/resume and revoke;
verify mounted session MCP. Repeat transport/MCP with a private CA and separately
configured adapter trust. Record proxy/adapter versions, leaf renewal/reload,
edge caps and the two-client result. Fixture adapters and localhost certificates
cannot satisfy this gate. No disposable public origin, remote fixture machine
or scratch adapter credentials were supplied to this lane. Until that evidence
exists, keep ingress restricted to a trusted population. Do not infer deployment
acceptance from health success, a configured peer list or a passing build.

References: [Caddy reverse proxy](https://caddyserver.com/docs/caddyfile/directives/reverse_proxy),
[nginx proxy module](https://nginx.org/en/docs/http/ngx_http_proxy_module.html),
[nginx WebSocket proxying](https://nginx.org/en/docs/http/websocket.html).

Generated with Claude AI — please review before distribution.
