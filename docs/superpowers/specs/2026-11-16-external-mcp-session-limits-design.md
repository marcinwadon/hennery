# External MCP session limits: per-client capacity and a creation rate fuse

Date: 2026-11-16. Status: approved for planning.

## Intent

An external MCP client (Claude at claude.ai connected through Hennery's public
OAuth MCP) today may hold at most 32 live sessions it created. The value is a
constant in `crates/hennery-sessions/src/external_control_store.rs` (checked at
admission, line ~318, and again at creation inside `external_guard_in`, line
~224). It is not configurable and not documented beyond one sentence in
`docs/specs/2026-10-29-external-session-mcp.md`.

The operator's real workload hits that constant in ordinary use: a batch of
5–10 PR reviews is started at once, one session per PR, and those sessions are
parked for days or weeks while the PRs sit in review. Parked sessions count as
live, so the stock accumulates across batches and the client is refused new
sessions until it closes old ones.

At the same time the operator does not want to simply remove the limit. A
client that creates 30 sessions in a minute is in a loop, and that must be
stopped regardless of how high the operator set the capacity.

The 32 constant conflates two different concerns. This design splits them:

- **Capacity** (how many live sessions one client may hold) is an operator
  decision, set per client, documented, discoverable by the client.
- **Rate** (how fast one client may create sessions) is a safety fuse. It is
  fixed in code and cannot be raised or disabled by configuration.

## Goals

1. The operator sets the live-session capacity per external client when
   granting consent. Default 32, so existing grants and installations keep
   today's behaviour.
2. A fixed per-client creation rate limit refuses runaway creation even when
   capacity is set very high.
3. Both limits are documented in the external MCP spec, reported by
   `hennery_capabilities`, and mentioned in the `session_create` tool
   description so a client knows the rules before it hits them.
4. Refusals carry enough structure (and plain text) for an LLM client to act:
   close old sessions, or wait N seconds.

## Non-goals

- Editing a client's capacity after consent. For now the path to change it is
  revoke + reconnect. (A later slice may add `PATCH /api/external-clients/{id}`
  and an edit control on the client list; see "Left open".)
- Making the rate limit configurable.
- Operator confirmation in the UI when the rate fuse trips. Chosen path for now
  is refusal. The error code and the single refusal site are to be shaped so a
  later "hold and ask the operator" mode slots in without renaming.
- Any limit on sessions created from the operator UI/REST, or on delegation
  children (`child_limit` already exists and is configurable in delegation
  settings).
- Retention/pruning of the per-client `external_operations` ledger. The
  ledger has a lifetime cap of 4096 receipts per client (`operation_limit`,
  `external_control_store.rs` ~line 327) and nothing prunes it. The operator
  has never reached it; a grant lives at most 90 days, which bounds it in
  practice. This design only reports the cap in `hennery_capabilities.limits`.
  Any later pruning must keep at least the last 60 s of creation receipts, as
  the rate fuse reads them.

## Anti-goals (what would make this a failure)

- A 10-session review batch started "in one go" being refused or throttled.
- A client able to make the rate fuse disappear by any configuration.
- Existing grants changing behaviour after upgrade without the operator doing
  anything.
- A capacity edit or refusal invalidating a client's in-flight operations
  (see `revision` note below).

## Design

### Capacity (per client, configurable at consent)

**Storage.** New column on `external_clients`:

```sql
ALTER TABLE external_clients ADD COLUMN session_capacity INTEGER NOT NULL DEFAULT 32
  CHECK(session_capacity BETWEEN 1 AND 1024);
```

Added as a new migration file **appended at the end** of the `MIGRATIONS`
list in `store.rs` (migrations run by index against `user_version`; inserting
in the middle breaks existing databases), following the pattern used by
`external_control.sql` for `read_transcript`/`control_sessions`. The upper bound
1024 is a sanity ceiling against typos, not a safety control; the rate fuse is
the safety control.

**Consent input.** The consent POST (`/api/external-oauth/consent`) is
deserialised into `Consent` in `external_oauth_store.rs` (around line 129,
`deny_unknown_fields`), which then builds `NewClient` (`external_clients.rs`,
also `deny_unknown_fields`) by struct literal. Both structs gain
`session_capacity: Option<u32>` (serde default `None`, meaning 32), and the
conversion passes it through. Validation in `issue_in`: `1..=1024`, else
`invalid_grant`. It is only meaningful
when `control_sessions` is true; if sent without `control_sessions` it is
accepted and stored (harmless) but the UI never offers it in that case.

**Client record.** `ClientRecord` gains `session_capacity: u32`, read in the
same SELECT as the other grant flags, serialised to the REST client list.

**Consent screen** (`web/src/screens/ExternalMcp.tsx`). Under the
"Create, prompt and control sessions" checkbox, when checked: a numeric field
"Live sessions this client may hold" with default 32, min 1, max 1024, and a
one-line hint: "Parked sessions count. Closed sessions do not." The value is
sent as `session_capacity` in the consent POST only when control is checked.

**Client list** (same screen, non-consent mode) shows the capacity on the line
that already states "session control enabled", e.g. "session control enabled,
up to 32 live sessions".

**Enforcement.** Both existing `ensure!(live < 32, "session_limit")` sites
compare against `client.session_capacity` instead. `live` = sessions in
`external_client_sessions` for this client whose lifecycle is not `closed`,
`deleted` or `failed` **and whose `close_requested` is 0**. Parked sessions
count. Sessions adopted via `existing_sessions` do not count (they are not in
`external_client_sessions`).

The `close_requested` exclusion is new (assistant's call): a parked session
closes collector-side at once, but an active one stays `active` with
`close_requested = 1` until the host confirms. Once the client has asked for
the close it has done the actionable thing, and the refusal would otherwise
depend on host latency. The UI hint stays "Closed sessions do not count"; the
test for "close one → next create admitted" is then deterministic for both
parked and active sessions.

**`revision` must not change.** `external_clients.revision` fences in-flight
operations (`external_guard_in` checks `revision = ?`). Capacity is set only at
consent in this slice, so nothing bumps it; the future edit endpoint must also
leave `revision` alone.

### Rate fuse (per client, fixed in code)

**Rule.** A client may be admitted for at most **16 session creations in any
rolling 60-second window**. Creations are `session_create` and `session_clone`
admissions. The numbers are `pub const` in `external_control_store.rs`
(`CREATE_RATE_LIMIT: i64 = 16`, `CREATE_RATE_WINDOW_SECS: i64 = 60`) so tests
and docs reference one place. There is no configuration surface.

Why 16/60s: a batch of 10 reviews passes without friction with headroom for a
retry or two; 30 in a minute stops after the 16th.

**Source of truth.** The `external_operations` ledger, not the `sessions`
table. Count rows for this `client_id` with `tool IN ('session_create',
'session_clone')` and `created_at >= now - 60` (the ledger's `created_at` is
unix seconds, written in `external_admit`). Reasons:

- It is already per client and already written inside the admission
  transaction, so the check and the insert are atomic under the same
  `IMMEDIATE` transaction with no new state.
- Retries with the same `request_key` return the existing receipt before
  reaching the check, so idempotent retries do not consume budget.
- Sessions that were created and then deleted (or whose rows were scrubbed)
  still count as creations.

The builder should confirm the ledger keeps rows (payload scrubbed is fine)
when a session is deleted; the spec says deletion scrubs payload, not rows.

**Where.** In `external_admit`, immediately after the capacity check for
`session_create`/`session_clone` and before the `operation_limit` check.
Order: capacity first (closing sessions is the actionable fix), then rate.
Only the admission site needs it; the second guard inside
`external_guard_in` runs for an already-admitted operation and must not
re-check rate (that would refuse a legitimately admitted creation on
dispatch).

**Retry hint.** `retry_after` = seconds until the oldest counted creation
leaves the window, minimum 1.

### Errors (tool result with `isError`, HTTP 200)

Business errors from tools are **not** JSON-RPC `error` members today. In
`external_mcp.rs` (around lines 352–379) a code like `session_limit` is
returned as a JSON-RPC *result* of the shape
`{"content":[{"type":"text","text":"<code>"}],"isError":true}`; `rpc()` is
used only for protocol failures. The integration test
`crates/hennery/tests/external_mcp.rs` asserts that shape. Both new refusals
keep it. HTTP 429 is reserved for transport/OAuth saturation and would read to
the claude.ai connector as a connector failure rather than a tool outcome the
model can act on.

For these two codes the result carries more than the bare code:

- `content[0].text` is a plain sentence with the numbers, because it is not
  verified that the claude.ai client shows anything but text to the model.
- `structuredContent` holds the same numbers as JSON for clients that read it,
  wrapped as `{"data": {...}}` so it conforms to the `outputSchema` every tool
  declares (`{"data": {}}` only, no additional properties); a validating MCP
  client would otherwise reject the refusal. (Added after review.)

Shapes:

- `session_limit` — text:
  `session_limit: 32 of 32 live sessions in use; close sessions created by this client to free capacity`
  structuredContent: `{ "data": { "code": "session_limit", "live": 32, "limit": 32 } }`
- `session_rate_limit` — text:
  `session_rate_limit: 16 sessions created in the last 60 s; retry after 23 s`
  structuredContent: `{ "data": { "code": "session_rate_limit", "limit": 16, "window_secs": 60, "used": 16, "retry_after": 23 } }`

`session_rate_limit` must be added to the code `match` in `external_mcp.rs`;
an unknown string there falls through to `-32603 internal_error`. The other
existing codes keep their bare-text shape; only these two get the richer
result. The internal `ensure!` strings stay bare codes so the match sites and
existing tests keep working; the numbers travel on a typed error or
side-channel of the builder's choosing.

### Discovery and docs

- `hennery_capabilities` response gains
  `"limits": { "session_capacity": 32, "create_rate": { "limit": 16, "window_secs": 60 }, "pending_operations": 32, "operation_receipts": 4096, "max_page": 50 }`.
  Existing top-level `max_page` stays for compatibility.
- `session_create` and `session_clone` tool descriptions
  (`external_control.rs`) gain one sentence: "Counts toward this client's live
  session capacity (parked sessions included) and a fixed creation rate limit;
  see hennery_capabilities.limits."
- `docs/specs/2026-10-29-external-session-mcp.md`: replace the sentence
  "Clients may have at most 32 live created sessions." with a short paragraph
  describing capacity (per client, set at consent, default 32, parked counts,
  revoke+reconnect to change) and the fixed rate fuse (16/60 s, not
  configurable), plus the two error codes and their `data` fields.
- There is no consent-endpoint doc in `crates/hennery-proto/src/rest.rs`
  (the OAuth docs there are for MCP connections); `session_capacity` is
  documented in the external MCP spec's consent section instead.

### Tests (builder decides exact placement; these are the behaviours)

- Consent stores `session_capacity`; omitted → 32; 0 or 1025 → `invalid_grant`.
- Existing DB upgraded: old clients read back with capacity 32.
- Capacity N: N live sessions (mix of active and parked) → next create refused
  with `session_limit` and `data.live == N`; closing one → next create admitted.
- Rate: 16 admissions inside the window succeed, the 17th is refused with
  `session_rate_limit` and a positive `retry_after`; after the window it
  succeeds. Same-key retry during the window returns the receipt, not a
  refusal, and does not consume budget.
- Rate fuse applies with capacity 1024.
- `hennery_capabilities.limits` reflects the client's capacity.
- Frontend: consent sends `session_capacity` only when control is checked;
  client list renders it.

## Left open for a later slice

- Edit capacity after consent (`PATCH /api/external-clients/{id}`, UI edit,
  must not bump `revision`).
- "Hold and ask the operator" instead of refusal when the rate fuse trips.
  Likely shape: the admission is stored as `queued` with a `held_reason`, a
  pending item appears in the operator UI, and release/deny resolves it. The
  refusal site in `external_admit` is the only place that would change.

## Decisions recorded

| Decision | By |
|---|---|
| Limits are per external MCP client, not global | operator |
| Capacity set only at consent for now; change = revoke + reconnect | operator |
| Rate fuse trips to a refusal, not an operator prompt, for now | operator |
| Errors stay tool results on HTTP 200 (`isError`), no 429 | assistant, operator agreed |
| Sessions with `close_requested = 1` do not count toward capacity | assistant |
| Rate fuse fixed at 16 per 60 s, not configurable | assistant |
| Capacity default 32, bounds 1..=1024 | assistant |
| Ledger receipt cap (4096) out of scope; reported only | operator |
| Rate counted from the `external_operations` ledger | assistant |
