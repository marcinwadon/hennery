# hennery — MCP gateway (subsystem spec)

- **Date:** 2026-09-26
- **Status:** Draft, awaiting review
- **Refines:** [architecture spec](2026-09-25-hennery-architecture-design.md) §8
  (hats) and §10 (gateway).
- **Evidence:** [per-session MCP spike](../spikes/2026-09-25-per-session-mcp.md)
  and a behaviour catalogue of the predecessor's gateway, which ran against
  real vendors (Notion, Atlassian, Miro, Datadog, Figma, Slack). "G-n" marks a
  predecessor incident or gap (§12).

The gateway lets the operator add an MCP integration **once, in one place**,
tick which hosts receive it, and have every agent session on those hosts use
it without a per-machine login. It owns connections, credentials, OAuth, the
streaming proxy and the manifests that tell agents where to connect.

---

## 1. Operator model

1. **Add a connection** in the MCP view: a name, the upstream URL, and how it
   authenticates (token, OAuth, or none). Each connection belongs to one hat;
   the default is the hat currently selected in the UI.
2. **Connect** (OAuth only): a popup completes consent once, on the collector.
3. **Tick hosts.** The grid lists **every host** for every connection (a mixed
   host isolates per session, so no host is out of a hat's scope); a tick is a
   mount. Where a host falls back to default-hat mounts for an agent (umbrella
   §8.5), the grid says so.
4. **Next session** on a ticked host, in the connection's hat, has the
   integration's tools. The UI says "applies to new and resumed sessions".

The operator never sees client tokens, never edits agent config files, and
never repeats consent per machine.

---

## 2. Data model

```sql
gw_connections(
  id TEXT PK, owner_id, slug, label, url, hat_id,
  cred_kind,               -- none | static | oauth_dcr | oauth_client
  static_header,           -- default 'Authorization' (static only)
  static_prefix,           -- default 'Bearer ', may be empty
  tool_allowlist JSON,     -- null = all tools
  internal_network BOOL,   -- operator allows non-public upstream addresses (§5.7)
  status,                  -- not_connected | ok | needs_auth | error
  status_note, account_label NULL, status_at, created_at, updated_at,
  UNIQUE(owner_id, slug))
gw_credentials(
  connection_id PK, owner_id, key_version, ciphertext BLOB,   -- AEAD, §6
  expires_at, updated_at)
gw_mounts(connection_id, host_id, owner_id, PRIMARY KEY(connection_id, host_id))
gw_session_tokens(                  -- one per session, minted at every start/resume
  session_id PK, owner_id, host_id, hat_id, token_hash UNIQUE,
  created_at, last_used_at, revoked_at)
gw_clients(                         -- standalone clients only
  id TEXT PK, owner_id, hat_id, label,
  token_hash, created_at, last_used_at, revoked_at)
gw_client_pins(client_id, connection_id, PRIMARY KEY(client_id, connection_id))
gw_oauth_clients(                   -- registered or pre-registered OAuth clients
  connection_id PK, token_endpoint, authorization_endpoint, issuer,
  client_id, client_secret_ciphertext NULL, redirect_uri, scopes JSON,
  resource, resource_param_accepted BOOL, registered_at)
gw_stdio_servers(                   -- §3.4
  id TEXT PK, owner_id, host_id, hat_id, name, command, args JSON,
  env_ciphertext NULL, created_at, updated_at)
```

`gw_mounts` has no hat column: the hat is the connection's. `gw_session_tokens`
holds session ids as opaque values; the gateway never reads session tables.

Every `gw_*` table carries `owner_id`. A connection's hat, a mount's host and
a credential's or mount's connection are the owner's by composite foreign
keys (`(hat_id, owner_id)`, `(host_id, owner_id)`, `(connection_id,
owner_id)`). Nothing cascades: the store deletes a connection's credential
and mounts itself. The `CHECK`s list every kind and status above, OAuth
included, since a table with children cannot be rebuilt later to widen one
(plan 8a).

- **`cred_kind`:**
  - `none` — no credential (public or network-trusted upstreams).
  - `static` — a personal access token or API key, sent as
    `Authorization: Bearer <token>` by default, or under a configured header
    name with an optional value prefix (`<prefix><token>`, prefix may be
    empty; e.g. `X-API-Key: <token>` or `Authorization: token <token>`). Preferred where the vendor offers one: no refresh,
    no expiry probe.
  - `oauth_dcr` — OAuth with dynamic client registration as a public client.
  - `oauth_client` — OAuth with a **pre-registered** client the operator
    enters (client id, optional secret). *(G-1: vendors without dynamic
    registration or with confidential-client-only token endpoints — Slack at
    the time of measurement — could not be connected at all.)*
- **The secret payload** (access/refresh token, static token, client secret)
  lives only in `gw_credentials` / `gw_oauth_clients` ciphertext. List
  endpoints never read those tables' secret columns: whether a credential
  exists (`has_credential`) is read by its key alone,
  `EXISTS (… WHERE k.connection_id = c.id AND k.owner_id = ?)`. A unit test
  prepares the list's statements under SQLite's authorizer and finds
  `connection_id` and `owner_id` the only columns of `gw_credentials` read
  (plan 8a).
- A connection belongs to **exactly one hat**. The same vendor in two hats is
  two connections with separate grants (umbrella §8.3). Its slug and its hat
  never change once created (moving it would carry its grant into another
  hat; a new slug would rename the agents' server).
- **Slugs:** `^[a-z0-9][a-z0-9-]{0,47}$`, unique **per owner**
  (`UNIQUE(owner_id, slug)`; plan 8a). v1 has one owner per installation, so
  this reads as per installation today; unique across owners, a create's
  `slug_taken` would tell one owner another's slugs (umbrella §7.4). The
  proxy finds a connection by the token's owner and the slug. Slugs become
  MCP server names (`hennery-<slug>`) and URL segments.
- **Mounts on a revoked host** are kept but unused and unlisted: revoking a
  host does not reach the gateway, the list joins `hosts` on
  `revoked_at IS NULL`, a mounts request naming a revoked host is refused, and
  the next full set drops it. A revoked host never becomes live again. Every
  later reader of mounts (proxy scope, delivery) applies the same join, or
  the gateway's `on_host_revoked` deletes them; the proxy's plan (8d) must
  implement that delete before any plan deletes host rows (plan 8a).
- **Limits** (plan 8a), checked before anything is written:
  - at most 256 connections per owner (409 `too_many_connections`);
  - a label is 1 to 64 bytes of UTF-8 once trimmed (stored trimmed), with no
    control or invisible format character;
  - a URL is at most 2048 bytes, and meets §5.7's rules for connection URLs;
  - an allowlist names at most 1024 tools, each 1 to 128 visible ASCII
    characters; duplicates are dropped, the order kept;
  - a mounts request names at most 1024 hosts, each id at most 64 bytes,
    counted as sent and checked before the connection is looked up; an id
    over 64 bytes is not quoted back (an unknown or revoked host's id is
    named in the message);
  - a static token is 1 to 8192 visible ASCII characters, no space;
  - the static header is an HTTP header name of at most 64 bytes, never one
    the proxy sets, frames or filters (`host`, `content-length`,
    `content-type`, `content-encoding`, `transfer-encoding`, `connection`,
    `keep-alive`, `upgrade`, `te`, `trailer`, `cookie`, `accept`,
    `accept-encoding`, `mcp-session-id`, `mcp-protocol-version`,
    `last-event-id`, `expect`, `forwarded`, `via`, `max-forwards`, `proxy-*`,
    `sec-*`); the prefix is at most 32 visible ASCII characters or spaces;
  - a request body is at most 256 KiB (413 `body_too_large`).
- **Hat purge** (kernel §5.5): the gateway's purge hook deletes the hat's
  connections with their credentials, OAuth clients and mounts, its session
  tokens, standalone clients and stdio servers.

---

## 3. Principals and delivery

### 3.1 Who presents a token

| Principal | Created | Scope |
|---|---|---|
| session | automatically, by `SessionMcp::servers_for` at every start and resume of a session (ACP core §1) | connections of the session's hat mounted on the session's host |
| `standalone` client | manually in standalone mode (`hennery gateway`) or for tools outside hennery | the connections pinned to the client (`gw_client_pins`) |

- Tokens are 32 random bytes, stored only as SHA-256 hashes. A standalone
  token is shown once; a session token exists in plaintext only inside the
  `start_session` / `resume_session` frame for that session's host.
- A session token is `hnry_session_` and 64 lowercase hexadecimal digits, a
  prefix a secret scanner can match; anything else is not looked up (plan
  8d). Standalone tokens get a prefix of their own (plan 8g).
- The gateway mints and revokes inside the sessions store's own
  transactions (`tokens::mint_in`, `revoke_in`, `revoke_host_in`, each
  taking the caller's `rusqlite::Transaction`), so a transition that rolls
  back leaves no token minted or revoked. A mint replaces the session's
  row: the previous token no longer resolves. A hat purge deletes its
  tokens, revoked ones too (`purge_hat_in`).
- **One token per session.** It is revoked on park, close, adapter exit and
  host revoke (`SessionMcp::revoke`), and superseded by the token minted at the
  next resume. A presumed park while the host is merely offline does not revoke
  it (ACP core §4.8).
- **Scope is checked at request time** from the token's (host, hat) and the
  mounts as they are now, never from anything the request claims. An unknown
  token, a revoked token, an unmounted connection, or a connection of another
  hat all return **404** (not 403: do not confirm existence). So do a
  missing, malformed or repeated `Authorization`, a superseded token, a
  token of a revoked host and an unknown slug: one body for all of them, and
  never 401 (G-17). A revoked host's tokens and mounts are out of scope
  whether or not its tokens were revoked.
- Tokens are never logged, and the `headers` of `mcp_servers` entries never
  appear in timeline events or SSE (ACP core §8).

### 3.2 Delivery to hennery sessions (primary path)

Per the spike, hennery-driven sessions get their MCP servers **per session over
ACP**, not through agent config files:

1. When the collector prepares `start_session` / `resume_session` (ACP core
   §3.3) it calls `SessionMcp::servers_for(host, hat, session)`, which mints the
   session token and returns every connection of the session's hat mounted on
   the session's host, as
   `{type: "http", name: "hennery-<slug>", url: "<public_url>/mcp/<slug>",
   headers: [{name: "Authorization", value: "Bearer <session token>"}]}`,
   followed by the stdio servers for that (host, hat) (§3.4).
2. The host passes them in `session/new` / `session/load`, with the agent's
   isolation mechanism (Claude strict flag, Codex composed home; ACP core §6),
   or applies the fallback (umbrella §8.5). The collector decides the
   fallback; a frame that delivers to an agent the host cannot isolate says
   so (`isolation_waived`), and without it the host refuses the servers
   (`mcp_isolation_unavailable`, plan 8c).
3. A mount change affects the **next** start or resume of a session on that
   host; a removed mount is refused at request time immediately.

**Token in a header, not the URL.** *(G-2: the predecessor put the token in
the URL path because one agent's CLI could not take a header from its config
at the time. URLs end up in logs and agent transcripts. Both agents accept
headers for HTTP MCP servers in ACP `session/new`, and Codex's config supports
`http_headers`.)*

**Measured limit: the token is visible in the process list.** The Claude
adapter's SDK passes `mcpServers` to the Claude CLI as `--mcp-config <JSON>` on
the command line, headers included, so another local user of that machine can
read a live session's token from the process list. Per-session tokens limit
the blast radius to live sessions of that host and hat. **Hosts shared with
untrusted local OS users are unsupported for hats with gateway connections**,
and the documentation says so. A live gate (ACP core §12) searches the process
list for the token on every adapter pin; for Codex the exposure is
unmeasured.

### 3.3 Delivery outside hennery sessions (renderers)

For standalone mode, or for terminal sessions the operator starts without
hennery, the gateway emits a **manifest** per standalone client
(`[{name, url, headers}]`) and a renderer writes it into agent config:

- `hennery mcp apply --client claude|codex`, with the client token read from a
  file (`--token-file`), the `HENNERY_MCP_TOKEN` environment variable or stdin —
  never a command-line flag (it would show in the process list).

Hosts never render MCP entries into agent config in v1. Renderer rules (§8).

### 3.4 Local stdio servers

hennery's per-session isolation strips the user's own locally configured MCP
servers from hennery sessions, and the gateway proxies only HTTP. So the MCP view
also holds, per **(host, hat)**, a list of local stdio servers
(`name`, `command`, `args`, `env`; names follow the slug rules and share
the slug namespace). The collector stores them (`env` encrypted
like credentials, §6) and passes them to sessions of that hat on that host as
ACP stdio `mcpServers` entries, named `hennery-<name>`. They run on the host as
the agent's child processes and are not proxied. Editing them requires step-up
authentication (kernel §3.4), since they are commands the host will execute.

---

## 4. OAuth

Hand-written on the kernel's egress clients (plan 8f decision 1), not on
`rmcp`'s `auth` module: that one tries the authorization server's metadata in
another order than §4.1, follows redirects during discovery itself, and brings
a second `reqwest` into the tree. hennery owns the credential store, the flow
state, the HTTP client and the policies below. Every discovery, registration and token request goes through
the egress policy (§5.7): no redirects are followed, non-public addresses are
refused unless the connection is marked "internal network", and every
authorization-server metadata and endpoint URL must be `https` (loopback
`http` allowed).

### 4.1 Discovery

For a connection URL, find the protected-resource (PR) document in order:

1. the `resource_metadata` URL from a `WWW-Authenticate` challenge (one
   unauthenticated `initialize` POST);
2. the path-inserted well-known URL
   (`<origin>/.well-known/oauth-protected-resource<path>`);
3. the origin-level well-known URL.

Then, for each authorization server named by the PR document (falling back to
the resource origin), fetch AS metadata trying the RFC 8414 inserted form
before the appended form, then OpenID configuration. The first document with
authorization and token endpoints wins.

*(G-3: a vendor served its PR document only at the path-inserted URL; trying
only the origin found a legacy authorization server whose tokens the resource
rejected — a grant that "refreshes fine and never works".)*

- The PR document's `resource` is validated against the connection URL
  (same origin, and the connection path starts with the resource path). A
  mismatch is shown to the operator with both values; the operator can accept
  it explicitly. *(G-4: one vendor's PR document names the bare origin while the
  documented endpoint has a path and query.)*
- `scopes_supported` from the PR document is requested at consent. *(G-5: one
  vendor issued tokens without product scopes unless they were requested.)*

**As built (plan 8f).**

- **The order:** the PR document from the challenge, then path-inserted, then
  origin-level; for each authorization server it names (at most 4, else the
  resource's origin), RFC 8414 inserted, RFC 8414 appended, OpenID
  configuration appended, then inserted. The first metadata with both
  endpoints and the `issuer` it was fetched for (RFC 8414 §3.3; equal but for
  one final `/`, plan 8f decision 6) wins; another `issuer` is 502
  `discovery_failed`.
- **What counts as "not here", so the next candidate is tried** (plan 8f
  decisions 5, 7, 8): any status but 200 (a 3xx included: redirects are
  never followed), a body over 1 MiB, a body that is not one JSON object, a PR document without
  `resource` (RFC 9728 §3.2), and a document that names one of its keys twice
  or spells one otherwise (a duplicate's value depends on the parser, lane
  L16). A 5xx at every candidate is 502 `upstream_unreachable`, not
  `discovery_failed`; an address the egress policy refuses ends discovery at
  once (502 `egress_refused`).
- **Every metadata URL, endpoint and the consent URL** is `https`, or plain
  `http` to loopback, even on a connection marked "internal network" (502
  `insecure_metadata`; `hennery_kernel::egress::is_https_or_loopback`).
- **The `resource` check:** same origin and the connection path starts with
  the resource path. A mismatch is 409 `resource_mismatch` on authorize, the
  value kept on the connection (`oauth.resource_mismatch`, shown whole only
  in the list item; an error names origins only, §5.8). The operator accepts
  it by sending it back exactly (`accept_resource`); it is accepted only if
  it equals what discovery finds at that moment, and a stale one is 409
  `resource_mismatch` again with the new value (plan 8f decision 14). An
  acceptance is kept (`oauth.accepted_resource`) for every later authorize
  and refresh, and any URL change, a path's too, clears it. A `resource` on
  another origin, or not `https`, or over 2048 bytes, is 502
  `resource_foreign` and cannot be accepted.

### 4.2 Registration

- `oauth_dcr`: RFC 7591 registration as a public client
  (`token_endpoint_auth_method: none`, grant types `authorization_code` and
  `refresh_token`, redirect `<public_url>/api/mcp/oauth/callback`).
  - A vendor refusal shows the RFC 7591 `error` and `error_description`,
    truncated to 300 characters. *(G-6: a refusal of a non-HTTPS redirect
    surfaced only as "400 Bad Request".)*
  - No registration endpoint → the UI suggests switching to `oauth_client`.
- `oauth_client`: the operator enters client id and, if the vendor requires a
  confidential client, the secret. The UI shows the redirect URI to register at
  the vendor.
- **Re-register** (`oauth_dcr`) when there is no client, the redirect URI
  changed (`public_url` changed), or discovery now yields a different token
  endpoint. If a grant is live, the new client replaces the old one only after
  the new consent completes. *(G-7: overwriting the client first broke refresh
  of the existing grant, permanently if the consent was abandoned.)*
- The redirect URI must be HTTPS or loopback HTTP; `public_url` enforcement in
  the umbrella §7.5 guarantees this for supported topologies.

**As built (plan 8f).**

- **One client row per connection** (`gw_oauth_clients`): the client the
  grant was made with (or, with no grant yet, the one the next Connect
  uses). A client registered at authorize is held by the flow until its
  callback stores it with the grant, and the latest unconsumed registration
  is reused by the next authorize of the same connection, so repeated clicks
  do not register again.
- **A pre-registered client is pinned** to the issuer and token endpoint its
  first Connect found; its secret only ever goes to that token endpoint. A
  later discovery that finds another is 409 `issuer_changed`: `PUT
  …/oauth-client` again (the API design's R2). A client `PUT` while a grant
  is live is stored as pending, under a field of its own, and replaces the
  client only when a Connect with it completes (G-7).
- A refusal is 502 `registration_refused`, its `error` and
  `error_description` with control and bidi characters stripped, cut to 300
  characters and prefixed "The vendor said: ".

### 4.3 Consent and exchange

- PKCE S256 only; refuse servers that do not advertise S256.
- Consent URL carries `resource=<connection URL>` (RFC 8707) and the scopes,
  and must be `https`. If the authorization server rejects the `resource`
  parameter, the flow is retried once without it and
  `resource_param_accepted = false` is recorded on the connection (and used for
  exchange and refresh).
- Flow state: in memory, keyed by `state`, single use, 15-minute TTL. The
  callback rejects an unknown `state` before touching storage and uses only
  values snapshotted when the flow started. *(G-8: a concurrent second authorize
  could swap the client under a callback that re-read it from storage.)*
- **Flow binding:** starting a flow sets a `SameSite=Lax`, `HttpOnly`,
  `Secure` flow cookie bound to its `state`; the callback requires the cookie
  to match, so a consent completed in another browser cannot be attached to
  the operator's connection.
- The popup is opened blank inside the click handler with `opener` set to
  `null`, then navigated to the consent URL (frontend spec §8).
- The exchange sends `code_verifier` and `resource`. Token-endpoint error
  bodies are never logged or echoed (they can repeat the code).
- The grant is stored **under the connection's refresh lock** (§4.5).
- After connecting, the MCP view shows the account identity the vendor
  reports, if any (`account_label`), so the operator can confirm the right
  account was connected.
- Vendor error text shown to the operator is truncated to 300 characters and
  rendered as text.

**As built (plan 8f).**

- **`resource` is the connection URL or nothing**, never another value: an
  accepted mismatching `resource` (§4.1) lets the Connect go on, and the
  `resource` sent stays the connection URL. The API design's B4 says so too;
  where it also reads "accepted or not", this section's retry without the
  parameter wins (plan 8f decision 15).
- **Where `invalid_target` is answered** (plan 8f decision 11): at consent,
  the authorization server's redirect carries the error, and the flow is
  over; the callback records it in memory for that (connection, URL), and
  the next Connect omits `resource`. At the token endpoint (exchange and
  refresh) the request is sent once more without it, server-side. Either way
  `resource_param_accepted = false` is stored with the grant.
- **The flow:** one live flow per connection (a new authorize supersedes the
  previous one, whose callback then answers `flow_unknown`), at most 16 per
  owner (429 `too_many_flows`), 15 minutes. A URL or kind edit, a client
  `PUT` or a delete drops the connection's flows (the API design's R1), and
  the callback compares the connection with the flow's snapshot again under
  the refresh lock before it stores anything (400 `connection_changed`).
- **The flow cookie** is `hennery_mcp_flow_<16 hex of sha256(state)>`, 256
  random bits, `HttpOnly`, `SameSite=Lax`, `Secure` as the kernel's
  `secure_cookies`, `Path=/api/mcp/oauth/callback`, `Max-Age=900`; every
  outcome from the cookie check on clears it. The flow is consumed by the
  cookie check whatever its result.
- **The callback's order** (`callback.rs`): `state` (before any storage,
  400 `flow_unknown`); the cookie, constant-time (400 `flow_mismatch`); the
  operator's session that started the flow still live (400
  `session_ended`); `iss` (RFC 9207: present and not the flow's issuer, or
  absent where the metadata advertises it, 400 `issuer_mismatch`); the
  vendor's `error` (400 `consent_denied`); the exchange (502
  `exchange_failed`, naming only a fixed RFC 6749 `error`;
  `upstream_unreachable`; `egress_refused`); under the lock, the connection
  unchanged; then the grant, status `ok` (`status_at` moves even if it was
  `ok`). Every failure from the flow on is kept in the connection's
  `oauth_error`; the first two name no connection, so they write nothing.
- **The page** is static HTML with no inline script and no external resource,
  naming no label, URL, account, client id, token or code; its one script
  (`/api/mcp/oauth/callback.js`) strips the query from the URL and history,
  tells the opener's origin on `BroadcastChannel("hennery-mcp-oauth")`, and
  closes the window on success. It answers `Cache-Control: no-store`,
  `Referrer-Policy: no-referrer`, `nosniff` and the kernel's CSP, layered on
  its own router.
- **`account_label`:** v1 learns none (no standard place reports one); the
  column stays `NULL`.

### 4.4 Tokens

- `expires_at = now + expires_in − 60 s`; absent `expires_in` means unknown,
  not expired.
- **Proactive refresh:** a request that finds the access token within 5
  minutes of `expires_at` refreshes first (single-flight). Expiry otherwise
  costs a 401, a refresh and a retry. *(G-9: the predecessor stored expiry and
  never read it.)*
- A refresh response without `refresh_token` keeps the old one.
- Refresh sends `resource` and the original scopes. *(G-10: the predecessor
  omitted both on refresh.)*
- As built (plan 8f): a proactive refresh is done only for a grant with a
  refresh token, so a request never waits for the lock of a grant that
  cannot be refreshed; after it, the URL and the token are read again, in one
  statement (plan 8f decision 13). A refresh that fails leaves the token
  there is: a 401 then has its own refresh and retry.

### 4.5 Single-flight refresh

- One mutex per connection. Under it: re-read the stored credential; if the
  access token already differs from the one that failed, use the new one;
  otherwise refresh.
- A started refresh always runs to completion and persists, independent of the
  caller's cancellation, bounded by a 20 s timeout. *(G-11: abandoning a refresh
  after the vendor rotated the refresh token lost the grant.)*
- The caller's own cancellation never sets `needs_auth`. *(G-12: false
  "re-authorize" alerts.)*
- Outcomes are distinct: success; vendor refusal → `needs_auth`; local persist
  failure → 502 without `needs_auth` (a click cannot fix a disk error).
- Every credential write (grant storage, credential clear) takes the same lock.
  *(G-13: a late refresh of an old grant overwrote a freshly stored new one.)*

**As built (plan 8f).** One `Runtime` per collector holds the locks, shared
by the API, the proxy and the probe (plan 8f decision 3). A refresh runs as a
task of its own that the caller awaits. Its outcomes (`Refreshed`):

| Outcome | When | The proxy answers | Status |
|---|---|---|---|
| `Retry` | a token to send: refreshed, or one another refresh or a Connect stored meanwhile (then nothing is sent to the vendor) | the retry's answer; a second 401 is 502 `upstream_auth` | `needs_auth` on the second 401 |
| `NotRefreshable` | no grant, or one without a refresh token | 502 `upstream_auth` | `needs_auth` |
| `Refused` | the vendor refused the refresh (`invalid_grant` and the like, `invalid_target` without `resource` too) | 502 `upstream_auth` | `needs_auth` |
| `Unavailable` | timeout, transport failure, an egress refusal or an unreadable answer | 502 `upstream_unreachable` | unchanged |
| `Unsaved` | the grant could not be read, or the result could not be stored | 502 `credential_unsaved` | unchanged |

- **The write is a compare-and-swap** on the sealed blob, URL and kind it
  read (plan 8f decision 9): an edit that deleted the grant meanwhile, or a
  newer grant a Connect stored, is never overwritten by a late refresh; the
  refresh then uses what is stored. A writer that cannot take the lock (a
  hat purge from the kernel's hook) is safe for the same reason.
- **A token comes with where it goes** (plan 8f decision 13): `Retry`
  carries the URL and the internal-network allowance read with its token.
  The retry is sent only if both equal what the first attempt was sent to;
  otherwise nothing more is sent and the answer is 502 `upstream_changed`
  ("send again"), without `needs_auth`. (A URL edit and a new Connect while a
  401 waited for the lock would otherwise send the new upstream's token to
  the old one.)

### 4.6 Credential invalidation on edit

- Changing a connection's URL **origin** or its `cred_kind` deletes its
  credential and OAuth client before the change is saved. *(G-14: otherwise an
  edited URL sends the stored token to a different host; a leftover static
  token would be sent as an OAuth access token.)*
- An omitted field in an update keeps its stored value; an explicit empty value
  clears it.
- As built (plan 8a): the delete runs in the update's own transaction, after
  every check and before the row is saved, so a refused update deletes
  nothing. It also starts the status over (`not_connected`, no note, no
  account label, `status_at` now). The origin compared is `Url::origin()`
  (scheme, host, port, the default port filled in); another path, header,
  prefix or `internal_network` keeps the credential, each behind step-up
  (§9). A `null` allowlist clears it (every tool), `[]` allows none, `""`
  clears the prefix; the label, URL, kind and header cannot be empty, and a
  `null` for any field but the allowlist reads as absent. The OAuth plan
  deletes the OAuth client in the same place.

---

## 5. Proxy

Endpoint: `POST|GET|DELETE <public_url>/mcp/<slug>`, authenticated by
`Authorization: Bearer <client token>`.

### 5.1 Authorization

Resolve token → principal → the connection with that slug, which must be in
the principal's scope (§3.1). Otherwise 404. The request body is capped at
4 MiB (413 only for the size limit).

Every answer the proxy makes itself is an `ApiError`, `{code, message}`, as
every other route's: 404 `not_found`, 400 `invalid_request`, 408
`request_timeout` (a body has 30 s to arrive), 413 `body_too_large`, 503
`busy`, 502 `upstream_auth`, `upstream_unreachable`, `upstream_redirect`,
`upstream_content_type`, `upstream_too_large` or `upstream_invalid`, 500
`internal`. None names more of the upstream URL than its connection's label.
A JSON-RPC error inside an MCP answer (§5.5, §5.6) stays JSON-RPC. Anything
deeper under a slug (`/mcp/<slug>/…`, a bare trailing slash too), a slug
that is not UTF-8, and any other method (`HEAD` included) is the same 404,
and nothing goes up.

### 5.2 Forwarding

- Hand-written on `axum` + `reqwest` streaming. Not built on an MCP library:
  those parse and re-serialise, and the proxy must change nothing but auth
  (and, for `tools/list`, the allowlist).
- **Request headers forwarded:** `Content-Type`, `Accept` (default
  `application/json, text/event-stream`), `Mcp-Session-Id`,
  `Mcp-Protocol-Version`, `Last-Event-ID`. `Authorization` is replaced by the
  upstream credential (or removed for `none`). Everything else is dropped.
  `Content-Type: application/json` goes up with a `POST` (the gateway checked
  the body is JSON), `Accept-Encoding: identity` is sent, and the upstream
  URL is the connection's as stored: the client's path beyond the slug and
  its query are not forwarded. A static credential is read in one statement
  with the URL, header and prefix it goes with.
- **A `POST` body must be JSON with no key twice in any object**, or it is
  400 `invalid_request` and nothing is sent: a parser upstream that keeps
  the first of two `"method"`s or `"name"`s would otherwise run what the
  allowlist never saw. Nor may it spell a key the gateway reads otherwise
  than exactly, folding ASCII case and `ſ` to `s` and ignoring `_` and `-`,
  as Go's `encoding/json` (v1 and v2) can match names, and cutting the key
  at its first NUL, as json-c and cJSON keep keys in C strings: a message's
  `jsonrpc`, `id`, `method`, `params` and `result`, a `tools/call`'s
  `name`, an `initialize`'s `capabilities` and the capabilities not
  forwarded. `METHOD`, `Name` or `method\u0000x` would be read upstream as
  what the allowlist never saw. Nor may a `method` be anything but a
  string, or spell a method the gateway reads (`tools/call`, `tools/list`,
  `initialize`, and §5.6's refused requests) otherwise, folded the same
  way (`tools/call\u0000x` is `tools/call` to json-c and cJSON); nor may a
  batch hold anything but objects, nor an `initialize` have `params` or
  `capabilities` that is not an object (`["sampling"]` holds `sampling` to
  a server that tests membership). The bytes go up as they came, except an
  `initialize` rewritten (§5.6). `GET` and `DELETE` bodies are neither read
  nor sent.
- **Response headers forwarded:** `Mcp-Session-Id`; `Content-Type` is the
  gateway's (below). `Cache-Control` is always `no-store`, the proxy's own
  answers' too: an answer to a request that carries a token is no shared
  cache's to keep, whatever the upstream says. Everything else is dropped,
  including `Set-Cookie` and `WWW-Authenticate`. *(G-15: the predecessor
  passed `Set-Cookie` through.)*
  The gateway always adds `X-Content-Type-Options: nosniff`, and forwards only
  `application/json` and `text/event-stream` bodies; any other upstream content
  type becomes a 502. So does a body without a content type, more than one
  `Content-Type`, and any `Content-Encoding` but `identity`. A body-less
  answer (202, 204, or `Content-Length: 0`) without a type passes, empty. The
  `Content-Type` sent down is the gateway's, the one type it judged the body
  by, without parameters: a parameter or a second header cannot make a
  client read as an event stream what the gateway passed as JSON.
- Upstream redirects are never followed (§5.7): a 3xx is 502
  `upstream_redirect`, its `Location` never forwarded.
- **Sessions:** `Mcp-Session-Id` passes through in both directions, so each
  downstream client session maps to its own upstream session and the gateway
  holds no session table. `DELETE` is forwarded so client terminations reach
  the upstream. *(G-16: the predecessor answered DELETE with 405, leaving
  upstream sessions until the vendor expired them.)*
- `GET` (the optional server-to-client SSE channel) is forwarded and streamed.

### 5.3 Streaming

- Event streams are passed on event by event (below), and there is **no
  compression layer on the proxy route** (compression middleware delays
  SSE).
- **Guarantee:** for an event stream, every chunk that ends an event
  reaches the client before the upstream finishes writing; a partial event
  waits for its end. A test asserts it against an upstream that writes one
  event and then blocks; the test must fail within seconds, not hang, if the
  proxy buffers.
- **A JSON answer is read whole** (cap 8 MiB, past it 502
  `upstream_too_large`, never truncated) before any of it goes on, with an
  allowlist or without: a client can use none of it before its end, and it
  may hold a server request (§5.6). It is read as an event's data is
  (below): JSON with a key twice, or spelling a key or method the gateway
  reads otherwise, is 502 `upstream_invalid`. It goes on as it came, or
  rewritten when the tools filter touched it (§5.5); an empty one passes,
  empty.
- An event stream is passed on **event by event**: every complete event at
  once, a partial one held until its end, so an event can be rewritten or
  dropped (§5.5, §5.6). One event is at most 8 MiB; past that the stream
  ends. It fails closed: a byte-order mark at its start is dropped, as a
  client's parser would; an event that is not UTF-8 (a client may decode
  an invalid or overlong byte otherwise than as a replacement character),
  or whose data is not JSON (to serde_json: a
  lone surrogate, say), has a key twice in an object, spells a key or
  method the gateway reads otherwise (§5.2; going down also a `result`'s
  `tools` and a tool's `name`), or has a line that starts with a
  byte-order mark, is dropped, since a client's parser might read what the
  gateway cannot; a last event the stream never ends is
  dropped (a final lone `\r` included). A rewritten event keeps its other
  fields (`id:`, `event:`) and gets one `data:` line.
  Events without data (comments, pings) pass byte for byte.

### 5.4 Upstream 401

- Every response status other than 401 is committed and streamed immediately.
- A 401 is held: static or `none` credential, or OAuth without a refresh
  token → **502** `{"code": "upstream_auth", "message": "connection <label>
  needs re-authorization in hennery"}`; OAuth → single-flight refresh and one
  retry, and a second 401 → `needs_auth` + 502 `upstream_auth`.
- **A 401 is never passed to the client and `WWW-Authenticate` is never
  forwarded.** *(G-17: agents answer an upstream 401 by starting their own
  per-machine OAuth against the gateway — exactly what the gateway exists to
  prevent.)*
- A failure before the response is committed is a clean 502 with no stray
  upstream headers.
- A `static` connection without a token, or of a kind the proxy does not take
  yet, sends nothing: 502 `upstream_auth`.
- A 401 for a static or `none` connection sets it `needs_auth` (§7).
- As built (plan 8f): an OAuth 401 goes through §4.5's single-flight refresh
  and its outcomes table: 502 `upstream_auth` (with `needs_auth`),
  `upstream_unreachable`, `credential_unsaved` or `upstream_changed` (neither
  of the last three sets `needs_auth`). The proxy and the probe share one
  forwarding path, `proxy::send_through`.

### 5.5 Tool allowlist

- **Every `result.tools` coming down is filtered to the allowlist,
  whatever the message's id**: in JSON and in SSE framing (per event, other
  events passed through byte for byte), on a `POST`'s answer, a `GET`
  stream, and a `GET` replaying one with `Last-Event-ID`. An allowlist
  matching nothing, or a `tools` that is not an array, yields `"tools":
  []`, never `null`. JSON-RPC batches are filtered element by element. A
  result without `tools` is untouched.
- **`tools/call` for a tool outside the allowlist is rejected** by the gateway
  with a JSON-RPC error (`-32602`, "tool not available through hennery") and
  never reaches the upstream. *(G-18: in the predecessor the allowlist only hid
  tools; a direct call still executed.)* A call whose `params.name` is not a
  string is outside it. A batch holding one is answered whole by the
  gateway and nothing in it is sent: that call `-32602`, every other request
  in it `-32600` ("batch refused"); notifications get nothing, and a body
  with nothing to answer is 202.
- **The filter hides; the `tools/call` refusal enforces.** Since the
  filter reads every `result.tools`, it needs no request's id, and these
  are accepted (the fleet parent's ruling of 2026-10-02):
  - *An id of another type.* The gateway's own answers echo an id as the
    client sent it, integers digit for digit and a string as a string; a
    client that coerces ids (MCP's TypeScript SDK matches with
    `Number(id)`) does so on its own. An upstream's answer is filtered
    whatever its id.
  - *An answer on another stream.* The proxy routes nothing between
    requests: each `POST` or `GET` is its own upstream exchange, and every
    stream is filtered alike. The upstream's `Mcp-Session-Id` is the only
    thing that separates two sessions' streams: the gateway forwards the
    client's, does not bind it to the token, and every token on a connection
    uses the same upstream credential, so a token presenting another
    session's id gets what the upstream serves for it (open for 8e).
  - *A tool list in an error* (`error.data.tools`) is passed on: no client
    reads tools from an error, and the refusal still enforces.
  - A response whose `id`, `result`, `tools` or a tool's `name` is spelt
    otherwise or twice is not passed on (§5.3).

### 5.6 Capabilities not forwarded in v1

The gateway rewrites `initialize.params.capabilities` sent upstream, removing
`sampling`, `elicitation` and `roots`. Server-to-client requests of those kinds
arriving in any answer are refused by the gateway and never reach the
client. *(G-19: the predecessor's spec said these were not forwarded, but its
code passed the client's `initialize` through verbatim.)* The requests are
`sampling/createMessage`, `elicitation/create` and `roots/list`. In an
event stream (a `POST`'s answer or a `GET`), each is answered `-32601` with
a `POST` on the same upstream session
(`Mcp-Session-Id`, the answer's or else the request's) and credential, in the
background, and it is not passed on: its event is dropped or, in a batch, its
element. Each answer reads the connection again, as a request does, and is
not sent if the connection left the token's scope or changed its URL or
internal marking since the stream opened. A notification of those names
passes. At the connection's request cap (§5.7) the answer is skipped and
logged. **In a JSON answer**, one of them (with an id) makes the
whole answer 502 `upstream_invalid`, and nothing is answered upstream: a
JSON body is the `POST`'s response, MCP's streamable HTTP sends server
requests only in an event stream, and MCP's TypeScript SDK and rmcp would
both dispatch it. Passing the rest on without it would hand the client a
partial answer.

Other methods (including ones the gateway does not know, like
`server/discover`) are forwarded unchanged.

### 5.7 Egress policy and limits

The proxy, every OAuth HTTP client and Web Push delivery share one outbound
HTTP policy, which lives in the kernel (kernel §7.1) so that push delivery can
use it without depending on the gateway:

- **Redirects are never followed.**
- DNS is resolved by hennery and the address checked before connecting;
  loopback, link-local (including `169.254.169.254`), RFC 1918, unique-local
  IPv6 and other non-public ranges are refused — unless the operator has marked
  the connection **"internal network"**, which allows them for that connection
  only. Web Push endpoints are always public-only.
- The upstream URL is `https`, or plain `http` to loopback; a connection
  marked "internal network" may also use plain `http` to an internal address
  (RFC 1918, loopback, unique-local IPv6 except `fd00:ec2::254`), never to a
  public one (the operator's decision of 2026-10-02; kernel §7.1). §4's OAuth
  URLs stay `https` (loopback `http` allowed) even then.
- Per connection, a cap on concurrent upstream requests and on open streaming
  responses, idle or not; beyond it the proxy answers 503.
  - `POST` and `DELETE` requests in flight: 64, each held from before its
    body is read until its answer's body ends or the client goes, a `POST`
    answered with an event stream included. A body has 30 s to arrive.
  - Open `GET` streams, the server-to-client channel: 32, counted whether
    idle or not (kernel §7.1's limiter); no idle timeout. A `GET` takes no
    request permit.
  - The response head must arrive within 300 s (a long `tools/call` may
    answer in JSON only when done).
- The egress client is chosen at every request from the connection's
  stored `internal_network` flag, read with its URL from the same row, never
  from anything in the request; it applies the rules above (kernel §7.1).
  The stored scheme and authority are sent verbatim, and the client's `Host`
  is never forwarded.

**Connection URLs** (plan 8a, at save time): `http` or `https`, absolute,
with a host, no user name or password, no fragment, at most 2048 bytes; a
query is kept, but a secret belongs in the credential. Stored as `url::Url`
serialises it. **`http` is saved only on a connection marked
`internal_network`**, on create or by one `PATCH` naming both (step-up):
unmarked plain `http` is refused (400 `invalid`), loopback included, and
clearing the mark while the URL is `http` is refused and changes nothing
(move the URL to `https` in the same `PATCH`). This is stricter than the
egress policy's scheme rule (§5.7, kernel §7.1) for an unmarked connection;
for a marked one the policy is the stricter, sending plain `http` to
internal addresses only (plan 8b-ii). Neither implies the other, and both
apply: the proxy sends every request through the egress policy (plan 8d),
so a saved URL the policy refuses is refused when used. Non-public addresses
are not refused when saving: the egress policy refuses them at request time
unless the connection is internal.

### 5.8 Upstream URLs in logs

An upstream URL is **logged and shown only as its origin**,
`scheme://host[:port]`, with the connection's id (plan 8a): no log line,
error body, trace or `Debug` of a connection type shows its path, query or
user info, where some vendors put a secret. The gateway's `url_for_logs` and
the wire crate's `url_origin` parse with `url::Url`, as the store does, so
raw input fails closed (`<not a url>`, or `null` for a scheme without an
origin). The connection types' `Debug` is written by hand. Only the API's
answers (`McpConnectionItem`, from the list, create, update and mounts
routes) show the URL whole, as stored: it is the owner's data, behind the
operator's session. Whether a secret inside a vendor's URL should be sealed
or refused instead is open for the maintainer (plan 8a's Q2; default:
documented, never logged whole).

---

## 6. Credentials at rest

- AEAD: **XChaCha20-Poly1305**, random 24-byte nonce per write,
  AAD = `connection_id ‖ field ‖ key_version`, stored as
  `key_version ‖ nonce ‖ ciphertext`.
- Master key: 32 random bytes generated on first run at `<data>/master.key`
  (created exclusively, mode 0600, permissions checked on load), or supplied
  through an environment variable or a systemd credential. Held in zeroizing
  memory.
- `key_version` exists from day one; `hennery gateway rotate-key` re-encrypts
  every row.
- Without the master key, credentials are unrecoverable; backups must carry it
  (umbrella §12.4). *(G-20: the predecessor stored credentials in plaintext.)*

**As built (plan 8a).**

- **The encoding.** AAD = `len ‖ connection_id ‖ len ‖ field ‖ len ‖
  key_version`, each `len` 4 bytes big-endian and the version 4 bytes
  big-endian, so no two (id, field) pairs give one AAD and the version is
  bound by the tag, not only checked beside it. The blob is `key_version`
  (4 bytes, big-endian) ‖ the 24-byte nonce ‖ the ciphertext with its 16-byte
  tag; the row's `key_version` column must equal the blob's prefix
  (else malformed), and both must be the key's. The nonce comes from the
  operating system's generator. The **field is named by table**:
  `gw_credentials.static_token`, taken from the connection's kind, so a row
  left under another kind does not open as another field (a second guard
  behind §4.6), and later tables (`gw_oauth_clients.client_secret`, the stdio
  servers' environments) never share a field. The version is `1` until
  `rotate-key`.
- **Not in the AAD:** the connection's origin and owner. Binding them would
  cost little (§4.6 already deletes the credential on every origin change,
  and the owner never changes) and would stop someone who can write
  `hennery.db` but not read the key (a copied volume or a restored backup,
  the key supplied by the environment or systemd) from pointing a stored
  token at another origin. Not binding them was decided after the security
  review and confirmed by the gateway lane parent; reversible by a re-seal
  at start with the key.
- **The key's sources, in order:** `HENNERY_MASTER_KEY` (64 hexadecimal
  digits); the systemd credential `$CREDENTIALS_DIRECTORY/hennery-master-key`
  (32 raw bytes, or 64 hexadecimal digits with an optional newline, for
  `SetCredential=`); `<data>/master.key` (32 raw bytes).
  - Both of the first two at once is refused: which one sealed the stored
    credentials would be a guess. A supplied key with a `master.key` beside
    it warns that the file is not used.
  - A source the operator set that cannot be read is an error, never a
    reason to make a key file: a `HENNERY_MASTER_KEY` that is not text, a
    credential whose look-up fails other than "not found".
    `$CREDENTIALS_DIRECTORY` set without the credential warns before a key
    file is made.
  - `master.key` is opened without following a symlink and checked through
    its descriptor: a regular file, no group or other bits, one link, exactly
    32 bytes. A credential file: a regular file, not a symlink, no bits for
    other users (systemd owns it, so its owner and group bits are not
    checked).
  - A new `master.key` is created exclusively, 0600 whatever the umask,
    written and synced, and its directory synced; a write cut short leaves a
    file of the wrong length, which the next start refuses rather than
    replaces.
  - The variable is read by the collector itself, never as a CLI argument
    (it would show in `--help`), and is stripped from what the host child and
    every agent inherit. Error messages never quote the key.
- **A missing or wrong key stops the start.** The collector opens the
  gateway after the admin socket's bind and before serving. A key missing
  while the owner has a stored credential is an error (no new key is made);
  so is one that does not open the owner's newest credential (only the
  newest is checked, so a damaged older row shows when it is used). Starting
  would seal new rows under one key beside rows only another opens. Both
  errors name the way out: restore the key (or supply it through the
  variable or the credential), or stop the collector and give the
  credentials up (`sqlite3 <data>/hennery.db 'DELETE FROM gw_credentials'`),
  then set each one again. `hennery_gateway::open` wraps every key error it
  meets in `KeyUnavailable`, so the binary can tell them from a store that
  does not open.
  - *Built so far:* the other key errors name no way out yet (a `master.key`
    of the wrong length, a systemd credential that does not read, and
    `KeySource::from_env`'s own, which `run_collector` gets before `open` and
    so without `KeyUnavailable`); plan 8g, with `rotate-key` and the key's
    docs.
  - **Open for the maintainer (plan 8a's Q1):** whether a lost or wrong key
    should refuse the start (the default, built) or start with the gateway
    disabled (its routes 503, sessions without gateway servers). Switching
    is `run_collector` matching `KeyUnavailable`.

**As built (plan 8f).**

- **Fields:** an OAuth grant (its access and refresh tokens, one JSON object)
  is sealed in `gw_credentials` as `gw_credentials.oauth_tokens`, taken from
  the connection's kind as the static token's field is; a client secret in
  `gw_oauth_clients` as `gw_oauth_clients.client_secret`, and a pending
  client's (§4.2) as `gw_oauth_clients.pending_client_secret`, so neither
  opens as the other. The key check at start opens each kind with its own
  field, and counts a stored client secret as ciphertext.
- **`gw_oauth_clients`** holds one row per connection: the client id, whether
  a secret is stored (a column apart, so a list never reads a ciphertext),
  the sealed secret, its `key_version`, the authentication method, the
  pinned issuer and endpoints, the redirect URI, scopes, `resource` and
  `resource_param_accepted`, and the pending client's id, secret, issuer and
  token endpoint. **One `key_version` covers both secrets of the row**: a
  pending `PUT` re-seals it, so `rotate-key` (plan 8g) must re-seal the
  active and the pending secret together.
- **What a grant counts as:** a grant is a credential (`has_credential`, and
  lane L20's hats with credentials when that lands); a client secret alone
  is not.
- A §4.6 change of origin or kind deletes the OAuth client with the
  credential, in the same transaction; so do a delete and a hat purge.

**Stated plainly in the docs:** the collector holding the gateway is a single
point of compromise for every integration it holds. Hats limit what one
session's token reaches; they do not protect against compromise of the
collector. In the default `hennery up` install the collector runs as the same OS
user as every agent, so any agent can read `master.key` and `hennery.db` and
decrypt every grant. Whenever the gateway holds credentials for more than one
hat, the collector should run as a separate OS user or in a container (the
Docker image); `hennery up` warns in that situation (kernel §10). In v1
`master.key` stays a file, not an OS keystore entry (kernel §10).

---

## 7. Health, probe and notification

- **Status values:** `not_connected` (no credential yet), `ok`,
  `needs_auth` (the vendor refused the credential), `error` (outage or
  transport failure — a click will not fix it).
- **Probe:** every 15 minutes, for OAuth connections with a credential only
  (static and `none` connections are not probed; `not_connected` ones are never
  probed). The probe performs a real MCP handshake through the same forwarding
  path: `initialize` → `notifications/initialized` → `tools/list` on that
  session → `DELETE`. *(G-21: a session-less `tools/list` gets 400 from
  stateful servers; the first probe design reported healthy connections as
  down.)*
  - 2xx → `ok`; 401 after one single-flight refresh → `needs_auth`; 5xx or
    transport failure → `error`; any other 4xx → **no change**.
- **Live traffic** also updates status: a 2xx through a connection marked
  `needs_auth`/`error` sets `ok` (and `not_connected`: a 2xx proves the
  upstream takes what was sent), if it has a JSON or event-stream body: a
  body-less 202, or an empty body under a type (`Content-Length: 0`),
  proves nothing. A 401 sets a
  static or `none` connection `needs_auth`. Either only if the connection
  still has the URL the request went to, and `ok` on a static one only if it
  still has a token; no other status changes it.
- A `needs_auth` connection stays mounted (unmounting on a possibly transient
  failure would churn every session's configuration).
- **Notifier** (`Notifier` interface, umbrella §10.2): fires on **transitions**
  into or out of `needs_auth`/`error` only; a problem present at startup is
  announced once. Full mode: Web Push + a badge in the MCP view. Standalone:
  log line and an optional webhook. `error` wording never asks the operator to
  reconnect. *(G-22: re-posting every tick trains people to ignore it; first
  implementation told the operator to click "Connect" during an outage.)*
- Every background loop has panic isolation and a per-tick timeout.

**As built (plan 8f).**

- **The probe** (`probe.rs`) goes through `proxy::send_through` with the
  connection's own credential and allowance: every step 2xx → `ok`; a 401
  after one refresh → `needs_auth`; a 5xx, a 3xx or a transport failure →
  `error`, with a note; any other 4xx → no change. The `DELETE`'s answer is
  not read (many servers take none). Each probe moves `checked_at`, and each
  is bounded at 30 s. Only the probe sets `error`; live traffic never does.
- **The loop** runs every 15 minutes over the OAuth connections with a grant;
  each tick is a task of its own bounded at 10 minutes, so a panic or a hang
  ends that tick, not the loop.
- **Probe now** (`POST …/probe`, §9): single-flight per connection; within
  10 s of a completed probe its result stands without a new request; static
  and `none` connections can be probed this way too (409 `no_credential`
  for a `static` or OAuth connection without one).
- **Live traffic's `ok`** moves `checked_at` too, at most once a minute when
  nothing else changes (the throttle of `last_used_at`).
- **Every status write is guarded by the URL** the request or probe went to,
  so an answer about an old URL changes nothing.
- **The Notifier** is told of a move into `needs_auth` or `error`, and out of
  either into `ok`, by the store that made it, in the statement that made
  it; a problem already there at start is announced once. Full mode: Web
  Push (`hennery_kernel::push::Push`), the title the connection's label, the
  generic title "An MCP connection needs attention", the body "needs sign-in
  again", "is failing" or "is working again", the URL `/mcp`, the tag
  `mcp-<connection id>`, urgency normal; the hat's push policy applies.

---

## 8. Renderers (standalone and terminal use)

- Targets: `~/.claude.json` (`mcpServers`) and `~/.codex/config.toml`
  (`[mcp_servers.<name>]`).
- **Ownership is in the data, not in comments.** An entry is hennery's iff its
  name starts with `hennery-` **and** its URL starts with `<public_url>/mcp/`.
  *(G-23: `codex mcp add/remove` rewrites `config.toml` and drops comments; the
  predecessor's comment-delimited block then became "foreign" and its entries
  were orphaned forever.)*
- An existing entry with a hennery name that is not hennery-owned is skipped and
  reported, never overwritten.
- **Semantic no-op:** if the owned entries already equal the desired set, the
  file is left byte-identical. *(G-24: Claude Code rewrites the file in its own
  key order, so a re-marshal never byte-matched and every reconcile rewrote
  it.)*
- Malformed input is refused without writing. Writes are atomic
  (temp + rename) and leave mode 0600, enforced even when content is
  unchanged. Large integers round-trip exactly.
- TOML is edited with a format-preserving parser (`toml_edit`), never with
  line scanners, and entry names are validated as safe TOML keys.
- `headers` are written (`headers` object for Claude, `http_headers` table for
  Codex).

---

## 9. API

All operator endpoints require an operator session. Endpoints marked
**step-up** also require a fresh passkey or password check (kernel §3.4).

The connection routes are built (plan 8a); their wire types in
`hennery-proto` (`McpConnectionItem`, `CreateMcpConnectionRequest`,
`UpdateMcpConnectionRequest`, `McpMountsRequest`, `McpCredentialRequest`)
document each route's answer and codes, and reach the TypeScript with their
docs. Every one of them answers `Cache-Control: no-store`, its refusals
included, and an error is the shared `ApiError`, exactly `{code, message}`.
Codes every route may answer:

- 401 `unauthenticated` (no live session);
- 403 `setup_required`, `origin_mismatch` or `cross_site` (the browser rules,
  kernel §3.2); 403 `step_up_required` on the routes marked step-up;
- 415 `unsupported_media_type` (a body that is not `application/json`);
- 400 `invalid_body` (not JSON), 422 `invalid_body` (not the route's shape,
  an unknown field included: every request type refuses unknown fields); the
  body is never quoted back;
- 413 `body_too_large` (over 256 KiB);
- 405 with no body (a method the path does not take);
- 500 `internal`;
- 404 `not_found` for an unknown connection or another owner's, on every
  route with `{id}`.

Step-up on `POST`, `DELETE` and `PUT …/credential` is layered per method, so
`GET` stays free and a method added later gets none unless layered too.
`PATCH` checks it in its handler on what the body names, before anything is
read.

| Method & path | Purpose |
|---|---|
| `GET /api/mcp/connections` | List: 200, an array of `McpConnectionItem`, oldest first (no secrets; `has_credential`, status, `account_label`, mounts on unrevoked hosts, sorted). |
| `POST /api/mcp/connections` | Create (**step-up**): `CreateMcpConnectionRequest` → 201 `McpConnectionItem`, `not_connected`, no credential, no mounts. 400 `invalid` (a field refused, or a hat that is not the owner's); 409 `slug_taken`; 409 `too_many_connections`. |
| `PATCH /api/mcp/connections/{id}` | Update: `UpdateMcpConnectionRequest` → 200 `McpConnectionItem`. **Step-up** when the body names `url`, `cred_kind`, `internal_network`, `static_header` or `static_prefix`, even with the stored value (a `null` reads as absent); the label and the allowlist need none. Origin or kind change clears the credential (§4.6). The slug and hat cannot change (an unknown field, 422). 400 `invalid`; 400 `unsupported_cred_kind`. A refused change changes nothing. |
| `DELETE /api/mcp/connections/{id}` | Delete (**step-up**: it destroys a grant that may need a consent to get back, as revoking a host does) with mounts, credential and OAuth client, and its OAuth flows dropped: 204. |
| `PUT /api/mcp/connections/{id}/mounts` | Replace the host set (full set, never a delta): `McpMountsRequest {host_ids}` → 200 `McpConnectionItem`. No step-up: a mount reaches only a host the owner paired, and pairing needs it. 400 `invalid` (a host not the owner's or revoked, more than 1024 hosts, an id over 64 bytes). |
| `PUT /api/mcp/connections/{id}/credential` | Set a static token (write-only, **step-up**): `McpCredentialRequest {token}` → 204, sealed, replacing any before it. 409 `wrong_cred_kind` (not `static`); 400 `invalid` (the token is never quoted). No route reads a credential back or clears one: changing the origin or kind, or deleting the connection, does. |
| `GET /api/mcp/oauth/redirect-uri` | The redirect URI to register at a vendor for `oauth_client` (plan 8f). |
| `PUT /api/mcp/connections/{id}/oauth-client` | Set a pre-registered client (**step-up**): `McpOauthClientRequest` → 200 `McpConnectionItem`; a secret left out keeps the stored one only for the same client id; pending while a grant is live (§4.2). 409 `wrong_cred_kind`; 400 `invalid`. Drops the connection's flows. |
| `POST /api/mcp/connections/{id}/authorize` | Start OAuth (**step-up**: kernel §3.4 lists gateway credentials and pre-registered clients, the broader rule over this table's first draft; the API design's F2): `McpAuthorizeRequest` (`{}` or `{accept_resource}`) → 200 `McpAuthorizeResponse` `{consent_url, expires_at}`, setting the flow cookie (§4.3). Its codes are on `McpAuthorizeRequest` in `hennery-proto`; among them 409 `resource_mismatch` (also for a stale `accept_resource`), 400 `invalid` (an `accept_resource` where no PR document names one), 409 `no_oauth_client`, `no_registration_endpoint`, `issuer_changed`, `wrong_cred_kind`, 429 `too_many_flows`, 502 `discovery_failed`, `insecure_metadata`, `pkce_unsupported`, `registration_refused`, `resource_foreign`, `egress_refused`, `upstream_unreachable`. Each failure after the connection was found is kept in `oauth_error`, and a failed Connect never changes the status. |
| `GET /api/mcp/oauth/callback` | OAuth redirect target (`state` plus flow cookie, §4.3), outside the operator's session and the browser rules; renders the page. Its script is `GET /api/mcp/oauth/callback.js`. |
| `POST /api/mcp/connections/{id}/probe` | Probe now (§7): `{}` → 200 `McpConnectionItem` after the probe. 409 `no_credential`. No step-up: it changes no credential and no destination. |
| `GET/PUT /api/mcp/stdio-servers?host_id&hat_id` | Local stdio servers for one (host, hat), full set (§3.4; **step-up** on `PUT`). *Later.* |
| `GET /api/mcp/clients` / `POST` / `DELETE /{id}` | Standalone clients. `POST {label, hat_id, connection_ids[]}` creates the client and its pins; the token is shown once. *Later: plan 8g.* |
| `PUT /api/mcp/clients/{id}/pins` | Replace a client's pinned connections (`{connection_ids[]}`). *Later: plan 8g.* |
| `GET /api/mcp/manifest` | Manifest for the presenting standalone client token (renderers). *Later: plan 8g.* |

The collector merges the gateway's router beside the sessions module's; the
kernel's CSP layer does not cover a router merged beside it. The JSON routes
need none; the callback, the one gateway route that answers HTML, layers the
kernel's `csp::on_html` itself (plan 8f).

No endpoint returns a session token, and no endpoint lets one host read another
host's tokens. *(G-25: the predecessor's pull endpoint took the target machine
from the path behind a shared token, so any host could read any other host's
token.)* Session tokens reach a host only inside the
`start_session`/`resume_session` frames for that session, over the
authenticated host connection.

The consent popup is opened blank inside the click handler, its `opener` set to
`null`, and navigated when the authorize call returns (passing `noopener` to
`window.open` makes it return `null`; opening after an `await` is blocked).
Frontend spec §8.

---

## 10. Standalone mode

`hennery gateway` runs the kernel (operator auth, storage, HTTP) and this crate
only. `ClientIdentity` resolves standalone client tokens, `MountPolicy` reads
the client's pins (`gw_client_pins`), `Notifier` logs and calls an optional
webhook. There are no session tokens and no stdio servers. The frontend shows only the MCP and Settings views. The build proves
the boundary: the `hennery-gateway` crate does not depend on `hennery-sessions`.

---

## 11. Testing

- **Streaming:** an event stream's first event before upstream completion;
  fails fast on a buffering implementation. SSE through the proxy with a
  long `tools/call`. A JSON answer comes down only whole (§5.3).
- **Differential:** every filter on parsed input, through the proxy, under
  the decoders of `tests/support/differential.rs` (§5.2, §5.3, §5.5, §5.6).
- **401 handling:** static token rejected → 502 `upstream_auth`, no
  `WWW-Authenticate`; OAuth refresh+retry; retry rejected → `needs_auth`;
  non-401 errors pass through.
- **Fake OAuth server:** PR discovery (challenge, path-inserted, origin),
  AS metadata forms, DCR success and refusal text, pre-registered confidential
  client, PKCE verification, `resource` on consent/exchange/refresh and the
  retry without it, rotating refresh tokens with reuse detection (concurrent
  401s share one refresh), refresh without a new refresh token, caller
  cancellation mid-refresh; callback without the matching flow cookie refused;
  `http` metadata endpoints refused.
- **Egress:** redirects not followed (proxy and OAuth); loopback, link-local,
  metadata-service and private addresses refused unless the connection is
  marked internal; per-connection concurrency cap; proxy responses carry
  `nosniff` and other content types become 502.
- **Scope negative tests:** a session token for hat A requesting a hat-B
  connection → 404; unmounted connection → 404; token of a parked, closed or
  superseded session → 404; standalone client outside its pins → 404.
- **Token hygiene:** no token or `mcp_servers` header appears in logs, events
  or SSE output.
- **Upstream URLs** (§5.8; plan 8a): a canary in a connection URL's path and
  query, through create, patch, list and refused requests at `TRACE` (raw
  input `url_origin` must not show, and a 422 whose canary is a string in a
  boolean field), appears in no log line and no error body.
- **Allowlist:** JSON and SSE `tools/list` filtering, batch, empty result `[]`,
  blocked `tools/call`.
- **Capabilities:** `initialize` upstream lacks `sampling`/`elicitation`/`roots`.
- **Credential edits:** origin change and kind change clear credentials.
- **Renderers:** on real Claude and Codex config files — foreign entries
  untouched, hennery-named foreign entry skipped, semantic no-op byte-identical,
  malformed input refused, mode 0600, entries survive a `codex mcp add/remove`
  rewrite.
- **Encryption:** AAD binding (swapping ciphertext between rows fails),
  key rotation, missing key → clear error. As built (plan 8a): a blob moved
  to another row, read as another field or relabelled with another version
  does not open; a missing key while credentials exist, or a wrong one,
  stops the start and creates nothing.
- **Live gate** (per release, optional vendor accounts): one static-token
  connection and one OAuth connection end to end through a real agent session.

---

## 12. Predecessor incidents and gaps referenced

| Id | Incident or gap | Rule |
|---|---|---|
| G-1 | No pre-registered client kind; some vendors impossible | §2 |
| G-2 | Token in URL path | §3.2 |
| G-3 | Origin-only discovery found a legacy AS | §4.1 |
| G-4 | PR `resource` never validated | §4.1 |
| G-5 | Scope-less tokens unless requested | §4.1 |
| G-6 | DCR refusal reason hidden | §4.2 |
| G-7 | Re-registration broke a live grant | §4.2 |
| G-8 | Callback re-read mutable state | §4.3 |
| G-9 | Expiry stored but never used | §4.4 |
| G-10 | Refresh omitted `resource` and scopes | §4.4 |
| G-11 | Abandoned refresh lost a rotated grant | §4.5 |
| G-12 | Caller cancellation flagged `needs_auth` | §4.5 |
| G-13 | Late refresh overwrote a new grant | §4.5 |
| G-14 | Edited URL would send the token elsewhere | §4.6 |
| G-15 | `Set-Cookie` passed through | §5.2 |
| G-16 | DELETE not forwarded | §5.2 |
| G-17 | Upstream 401 triggered agent-side OAuth | §5.4 |
| G-18 | Allowlist did not block calls | §5.5 |
| G-19 | Sampling/elicitation not actually stripped | §5.6 |
| G-20 | Plaintext credentials | §6 |
| G-21 | Session-less probe gave false outages | §7 |
| G-22 | Notification noise and wrong advice | §7 |
| G-23 | Comment markers lost on Codex rewrite | §8 |
| G-24 | Needless rewrites of `~/.claude.json` | §8 |
| G-25 | Pull endpoint leaked other hosts' tokens | §9 |

---

## 13. Open questions

1. **Vendor coverage.** Measured behaviour exists for six vendors, but token
   lifetimes, rotation and reuse detection were never observed over days.
   Worth a small live-gate matrix once the gateway exists.
2. **`GET` SSE channel.** Forwarded in v1; whether any vendor uses it for
   server-initiated messages that hennery then has to refuse (§5.6) is unknown.
3. ~~**Static-token header name.**~~ Resolved 2026-09-27: header name plus an
   optional value prefix (§1).
