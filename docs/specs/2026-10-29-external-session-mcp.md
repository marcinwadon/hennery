# Public external session MCP: scoped session management

The primary integration is a public HTTPS Streamable HTTP endpoint used directly
by Claude Desktop and other remote MCP clients. A desktop bridge is optional.
It exposes discovery, session metadata and separately granted session control
and transcript reads. Accepted operations survive client disconnects and can be
recovered by request key. Ordinary answers and post-start configuration remain
future additions.

Public deployment support does not mean this change exposes an existing
collector. No live service, Desktop account/configuration, DNS or firewall was
changed. The public deployment and real Desktop acceptance gates below remain
explicitly separate from automated isolated tests.

## Transport and authorization

Use `https://hennery.example/mcp/external/hennery`. The collector serves
Streamable HTTP with single JSON responses, no required MCP session ID and no
persistent GET stream (authenticated GET/DELETE return 405). It supports the
negotiated `2025-11-25` and `2025-06-18` profiles. The March 2025 profile is
not supported because it permits batches, which this adapter does not implement.
It does not claim the newer 2026 transport's handshake-free metadata semantics. An unsupported
offered version negotiates the supported profile; unsupported version headers
are rejected. Present Origin must equal the configured collector origin.

Current MCP authorization guidance and Claude's remote OAuth requirements
informed the authorization path:

- Unauthenticated calls return 401 with a Bearer `resource_metadata` challenge.
  Protected-resource metadata includes the exact MCP URL and one issuer.
- The authorization server advertises S256 PKCE, public-client authentication
  (`none`), authorization code and refresh grants, CIMD and a DCR fallback.
- The canonical resource is mandatory at authorization and token exchange.
  A token for another resource, an operator cookie or a delegation token cannot
  authenticate an OAuth external call.
- Invalid or expired authority returns 401. A read needing a scope the operator
  withheld returns 403 with an insufficient-scope challenge; it does not widen
  the grant. The challenge includes both discovery and read scopes, so a client
  can use that set for a new PKCE request and explicit operator consent.
- Consent returns `iss` and the original `state` to the validated callback.
  Codes are bound to the client, exact callback, resource and S256 verifier.
- Refresh tokens rotate. Reuse of a spent code or refresh token revokes that
  grant's family. Clients must serialize refresh; an uncertain refresh response
  can require reconnecting through consent.

The public metadata and OAuth paths are:

| Path | Purpose |
| --- | --- |
| `/.well-known/oauth-protected-resource/mcp/external/hennery` | Resource discovery; root well-known is also served. |
| `/.well-known/oauth-authorization-server` | Issuer and OAuth endpoint/capability discovery. |
| `/oauth/external/register` | JSON DCR for public clients, with `token_endpoint_auth_method: "none"`. |
| `/oauth/external/authorize` | Validate client, callback, scope, resource and PKCE; open Hennery consent. |
| `/oauth/external/token` | Form-urlencoded authorization-code exchange or refresh. |
| `/oauth/external/revoke` | Form-urlencoded token revocation; unknown tokens return success. |

CIMD client IDs are HTTPS metadata-document URLs with a path. The document's
`client_id` must match exactly and its declared callback must match the request.
Only public clients using `none` are supported in this slice; client secrets and
`private_key_jwt` are not advertised or silently downgraded. Metadata is fetched
for each new authorization through the existing public-only egress policy:
no redirects, environment proxy or private/mixed DNS results; five-second
deadline and 32 KiB body cap. No logo or secondary metadata URL is fetched.
CIMD caching is deferred. DCR provides compatibility for clients without CIMD.
Unused registrations expire after ten minutes; explicit consent extends them to
30 days, independently of already-consented grants. Anonymous registration
cannot reserve the bounded registry for a month without user consent.

Requests to the public OAuth flow are limited per attributed network address,
with stored registration/pending-request caps. OAuth registration, authorization,
token/revocation and external MCP requests share 16 concurrent reservations,
at most four per effective client (IPv6 /64), acquired before authentication or
body reads and held until the handler completes. Only configured exact proxy
peers may supply the client address; untrusted forwarding headers cannot obtain
more capacity. Saturation returns 429 with `Retry-After: 1`; the OAuth attempt
rate limiter retains its separate backoff. Public discovery remains available.
These application limits complement the reverse proxy's TLS/socket/request limits.
Discovery is available without cookies. Consent and grant management require
the existing operator login and same-origin browser rules.

Sources checked on 2026-10-29:

- [MCP authorization](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization).
- [Client registration and CIMD](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/client-registration).
- [Authorization discovery](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/authorization-server-discovery).
- [Supported Streamable HTTP profile](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports).
- [Claude's remote authentication requirements](https://claude.com/docs/connectors/building/authentication).
- [Claude connector setup](https://support.claude.com/en/articles/11176164-use-connectors-to-extend-claude-s-capabilities).
- [Remote connector network requirements](https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp).

## Consent, identity and revocation

An OAuth application is distinct from both the operator and a delegated agent.
It never receives the operator's cookie or an agent's delegation token.
The browser uses Hennery's existing password/passkey login and step-up UI;
the external client does not collect those credentials.

Consent shows the untrusted application name, client ID, exact callback and
callback host, with a loopback warning where relevant. Nothing is granted by
visiting a link or by discovering/registering a client. The operator must choose
a hat, host, canonical root and expiry, then explicitly allow. The current
screen selects one host/root pair per connection; the operator API can issue
multiple pairs. Existing-session access is unchecked by default and needs its
own explicit choice within the selected boundaries. Denial issues no grant.

Issuance checks owner/hat/purge, paired non-revoked hosts and server path-rule
resolution in the same transaction as code admission. Execution additionally
checks the host's actual canonical workspace and granted root before create,
prompt and resume, then rechecks authority in the mutation transaction and at
the synchronous host handoff.
Filters and cursors only narrow access. Path segments are checked, so
`/workspace/demo-other` is outside `/workspace/demo`.

The default created-by-client scope returns only sessions this client created.
Metadata permission is separate from discovery;
a client cannot widen it through arguments, protocol metadata or a scope string.
Permissions requested by agents remain operator UI decisions. No external
permission-approval authority is introduced.

`hennery:sessions:control` and `hennery:sessions:transcript` are independent,
explicit consent choices requiring metadata access. Both default off, including
after migrating old grants. Requesting a scope never grants it automatically.
Control includes choosing a model and permission mode when creating a session;
consent states this explicitly. Applied agent permissions follow that mode.
Changing a grant requires fresh consent; a token refresh cannot expand it.

Each consent creates an expiring, separately revocable external grant. The
operator can list/revoke it at `/external-mcp`, linked from Settings. Management
API `GET/POST /api/external-clients` and `DELETE /api/external-clients/{id}` also
remain available; issuing/revoking needs fresh step-up. Manual static credentials
are an optional alternative, not the primary Desktop setup.
Grant management pages load at most 100 records at once, with an ID continuation
cursor and an explicit Load more control; old grants remain reachable for revocation.

The store keeps code/access/refresh hashes. Consent expires after ten minutes,
a code after one minute, and an access token after at most fifteen minutes.
The refresh family expires with the chosen one-to-90-day grant. Refresh replaces
the previous access token too. Refresh may omit scope or repeat the same granted
scope set in any order. Expansion and downscoping are rejected; a new explicit
consent is needed to change scope. Cached principals are revalidated at the database
gate and before publishing collected results; expiration, grant revocation,
hat purge and host revocation cannot be bypassed by reconnecting or replaying.

## Read tools

| Tool | Result |
| --- | --- |
| `hennery_capabilities` | Scope/revision/expiry/limits, transcript and execution capabilities. |
| `host_list` | Granted hosts, connectivity and currently connected runtime IDs. |
| `workspace_list(host_id)` | Explicitly granted roots, no arbitrary browsing. |
| `session_list(after?, limit?)` | Scoped metadata, at most 50 rows, ID continuation cursor. |
| `session_get(session_id)` | One authorized metadata record; missing and out-of-scope both report not_found. |

Session metadata tools require metadata permission. Reads do not acknowledge unseen or
needs-you state. Transcript access requires its separate scope. Tools for
approving permissions or administering credentials and connections are not
provided. A session listing is eventual metadata,
not a transcript snapshot/delta cursor. Responses are no-store, bodies/results
are bounded, and consent/management pages are excluded from PWA cache/reload
navigation.

Session metadata is authorized and collected under the same database gate,
using the current stored session hat, host and root boundaries. That query is
the authorization point: a reassignment or deletion afterwards does not retract
an already-collected snapshot. The final grant/root check catches invalidated
client authority, but does not lock sessions until HTTP delivery. Each later
read checks current state again. Workspace discovery lists the explicitly
granted paths; changing path rules or a host default hat does not rewrite those
grants or historical session hats. Nested paths whose sessions belong to a
different hat remain excluded.

These read snapshots never authorize execution. Admission, dispatch and resume
resolve the current grant, host realpath, actual hat placement and
per-(hat, actual ACP) source policy again.

Unknown tool names return a JSON-RPC `-32602` error with the original request
ID. Known tool input/business failures use `result.isError`; unexpected server
failures use JSON-RPC `-32603`.

## Public HTTPS deployment recipe

Provision a separate deployment deliberately. Reuse the tested
[HTTPS/WSS proxy recipes](../../packaging/proxy/README.md) and the
[collector-on-a-server setup](../../README.md#collector-on-a-vmserver-behind-caddy).
Use a real public DNS name and publicly trusted TLS chain; a host's private CA
does not configure Anthropic's cloud trust.

For a new deployment, the collector's private backend configuration is:

```toml
listen = ["127.0.0.1:7117"]
public_url = "https://hennery.example"
proxy_client_ip_peers = ["127.0.0.1"]
```

Terminate TLS at the same-machine Caddy/nginx proxy, forwarding to that loopback
listener. Replace all placeholders in the existing template with deployment
values. The templates preserve Host/Origin/Authorization, forward all required
paths, disable retries/caches and attribute the exact client address. Keep the
backend and proxy administration listener private. Configure/test edge limits;
Caddy core needs a separately qualified ingress limiter.

Do not add proxy Basic authentication or an interactive WAF challenge in front
of discovery, registration, authorization, token or MCP endpoints. Hennery's
OAuth challenge and browser login must be reachable. Do not strip
`WWW-Authenticate`, rewrite the issuer/resource host/path or log request bodies
and token/code-bearing query strings. The supplied recipes disable access logs.
The whole origin is served; path-prefix relocation is not supported.

First setup must store the final HTTPS `public_url`; passing a flag on an
already-set-up collector does not migrate its identity. Follow the existing
admin/public-URL change procedure deliberately for migrations. This work did
not perform that procedure on any existing service.

Check the deployment from outside its network: TLS verification, 401 challenge,
both discovery documents, authorization/login, token refresh and scoped MCP.
Claude remote connections originate from Anthropic's cloud even on Desktop.
A private DNS name, localhost certificate or successful laptop request does not
prove Anthropic reachability. Preserve the existing
[public deployment gate](../../packaging/proxy/README.md#public-deployment-gate).

## Direct Claude Desktop acceptance

In the installed client's custom connector setup, enter the public MCP HTTPS
URL directly and choose OAuth sign-in. Use Claude's published identity when
available (CIMD); the server advertises both required CIMD settings. Automatic
registration (DCR) is a fallback. Do not require an installed bridge, shared
static header or an inbound OAuth client secret.

Acceptance remains pending until performed in an intentionally provisioned
public scratch deployment:

1. Record Desktop version, account/setup options, public origin, proxy version,
   valid TLS chain and external reachability. No private account details or
   credentials belong in evidence.
2. Add the URL directly. Verify 401/discovery, browser login/passkey or password,
   visible callback identity and explicit hat/host/root/expiry consent. Returning
   to the client must succeed. Verify Deny creates no grant.
3. Read the authorized host/root. Confirm created-only access excludes existing
   sessions; a separate explicitly selected existing-session grant permits only
   the chosen boundaries. Other hat/host/root/session IDs remain denied.
4. Observe a refresh and a reconnect. Revoke in Hennery while the client is
   connected; subsequent reads and refresh must fail. A new consent is required.
5. Record offered/negotiated protocol versions, actual tool calls and results.
   Protocol fixtures or inventory of an installed app are not Desktop acceptance.

Automated tests exercise the real HTTP handlers, durable store/reopen,
PKCE/audience/callback/scope/replay/expiry/revocation guards, actual TLS through
both shipped reverse proxies, and browser login/consent/revocation in isolated
HTTPS fixtures. Fixture CA trust is confined to test clients. These tests do
not establish public Internet reachability or a live Claude connection.

The existing `hennery external-mcp` stdio bridge remains optional and uses the
same scoped HTTP adapter. Local extension packaging is deferred. The primary
integration and acceptance above require no desktop binary.

## Session control and transcripts

| Tool | Contract |
| --- | --- |
| `session_create(request_key, host_id, cwd, agent, config?)` | Durable creation of one standalone session; optional model, mode, axes and cache retention. |
| `session_prompt(request_key, session_id, text)` | Durable text admission, queues while busy and auto-resumes a parked session. |
| `operation_get(operation_id? or request_key?)` | Recover this client's operation, delivery state, session/turn IDs and outcome. Supply exactly one lookup key. |
| `session_wait(operation_id, timeout_ms?)` | Bounded wait, up to ten seconds, returning operation state and whether operator input is needed. |
| `session_read(session_id, cursor?, before_turn?, max_items?)` | Canonical transcript excerpts, up to ten items, with opaque continuation and explicit resync on revision changes. |
| `session_cancel(request_key, session_id, turn_id)` | Cancel exactly the named turn; an already-ended turn returns its outcome and never cancels a successor. |
| `session_park`, `session_resume`, `session_close` | Explicit lifecycle operations, each taking request_key and session_id; close preserves history. |

Mutation tools, operation lookup and wait require session control. Transcript
read requires transcript scope. Missing and out-of-scope resources remain
indistinguishable. Inputs reject unknown fields and cannot supply provenance.
Prompts are server-labelled `ExternalClient {client_id, operation_id}` in stored
turns, canonical views, activity and memory provenance; the UI never labels
them as an operator message.

Request keys are printable ASCII, at most 128 bytes, unique within a client.
The durable ledger stores their hash and a canonical argument digest. Repeating
the same key and arguments returns the same operation/session/turn; changing
arguments conflicts. A lost HTTP response does not cancel accepted work.
Acceptance is not launch or turn completion: poll the operation, then read the
transcript. A running command interrupted by collector restart becomes
delivery_unknown and is never blindly resent. Existing turn/attach facts may
later establish completion. Reusing the key cannot launch a second copy.

Workers run at most four commands concurrently. Each client can retain 4096
operation receipts, with at most 32 pending operations and 1 MiB queued payload;
new admission fails at a bound rather than silently expiring deduplication.
Clients may have at most 32 live created sessions. Busy external prompts use a
durable FIFO per session and yield to the existing session dispatch queue.
Authority is checked again at dispatch and auto-resume. Grant revocation cancels
queued payloads; it does not claim to recall an already-handed agent action.
Unsent cleanup is fenced by operation/turn/intent generation so it cannot undo
a newer operator attachment or close. Session deletion scrubs operation payload,
result and session/turn/request references atomically.

Transcript excerpts have a 12 KiB text budget per item and report
`content_truncated`. To retrieve a complete large item, call session_read with
`item_id`, its `before_turn` page selector when needed, and byte `offset`.
Concatenate `item_json` fragments until next_offset is null; subsequent chunks
must include the returned `version`. UTF-8 boundaries are preserved and a
changed item requires resynchronization. A read snapshot is not a live lock;
each next read checks scope again. Wait disconnects never cancel the turn.
Question items may describe pending permission requests; their canonical
answerable state describes the operator UI, not external approval authority.

Ordinary elicitation answers, attachments, runtime catalog discovery and
post-start configuration are future additions. Permission requests continue
through Hennery's operator UI; no approval tool is provided.
