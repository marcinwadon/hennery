# Public ingress hardening

The collector authenticates every host and operator, and also bounds work done
before that authentication. TLS remains the deployment's responsibility; a
public URL never enables encryption on the backend listener.

The shared HTTP/1 server limits live connections to 512, request-header buffers
to 16 KiB and header reads to ten seconds. Capacity is shared across listeners.
The public router admits at most 512 ordinary requests and 128 per effective client;
their permits remain on response bodies until EOF or cancellation, including
SSE. Trusted proxy attribution runs before this admission. Neither static
response size hints nor long-running response streams are changed. This leaves
room for a gateway connection's existing 64 requests and 32 streams beside
browser traffic. Health probes do not compete with these streaming reservations;
readiness has two separate permits held until its blocking DB work completes,
even if the HTTP caller disconnects or the readiness deadline expires.

The JSON extractor bounds auth/setup/enrollment body reads to five seconds
and other browser JSON reads to 30 seconds. A deadline returns the fixed JSON
request-timeout error and closes HTTP/1. Gateway streaming routes retain their
own existing read and stream policies. These limits never measure agent turns.

Host WebSocket upgrades reserve one of 64 pending-handshake slots, at most
four per effective client (IPv6 /64). Reservations are released on failed
upgrade, bad input, disconnect, timeout, shutdown or authenticated completion.
There is no historical address map to fill or evict. The nonce-bound key proof
and revoke checks remain authoritative; client addresses grant no identity.

Protocol 1.1 adds `authenticate {protocol_version,host_id,proof}`. A collector
advertises support only with `Hennery-Hello-Auth: 1` in its ordinary 101
response, alongside the existing nonce. A capable host sends that small proof
before its full hello, without waiting for another response. Both frames name
the same host and protocol version and use the existing signature construction.
The complete handshake still has one ten-second deadline. Full hello is
checked again, including revocation, before registration and reconciliation.

Before the first valid signature, frame headers and their declared lengths
pass through a 64 KiB cumulative wire budget. The existing WebSocket library's
header parser is reused; an oversized declaration never reaches its payload
allocator. Headers already prefetched during HTTP upgrade pass through the
same gate. Fragmentation and control frames cannot obtain fresh budgets.
After authentication, frames retain the existing 32 MiB limit, including large
configuration and reconciliation state.

Older collectors do not advertise support and receive the ordinary hello.
Older hosts can use the ordinary hello when it fits the pre-auth budget.
Deployments with larger legacy hellos must update hosts before collectors.
Unknown minor versions retain the existing major-version compatibility rule;
the new frame is sent only after explicit advertisement.

Rejected host identities are length-checked before registry lookup and never
logged in full. Fixed refusal messages avoid attacker-controlled log volume.
Container deployments still require log rotation, memory/process limits and
external TLS admission controls. Caddy's core configuration does not provide
a per-client TLS connection limiter.

The regression gate includes declared oversized frames without payload,
fragmented cumulative overflow, bytes prefetched with HTTP upgrade, old-host
small hello, authenticated large hello, admission release and spoof refusal,
response-body reservation lifetime, stalled headers and JSON bodies, and log
reflection. Public deployment qualification still requires the separate live
adapter/MCP acceptance described in `packaging/proxy/README.md`.
