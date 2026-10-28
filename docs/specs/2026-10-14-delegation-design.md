# hennery — parent/child sessions and the built-in session MCP

- **Date:** 2026-10-14.
- **Operator amendment, 2026-10-21:** Delegation is binary per hat. ON
  permits every runtime/model/mode/option offered by the live, fresh catalog
  at child spawn; OFF denies delegation. There is no execution-policy
  snapshot, grant editor, allowlist or narrowing. Enabling never probes hosts.
- **Status:** proposed design; needs parent review and the operator decisions
  in §12. No implementation or live acceptance is claimed.
- **Review:** Codex gpt-6-astra, on the maintainer's behalf, reviewed
  `0fe663b8..72f24e58`: approve after amendments, no Critical findings.
  The grant, resume, close-intent and child-loss amendments below address that
  review; scoped re-confirmation remains with the parent.
- **Scope:** sessions delegate work to independently configured children,
  receive their reports and questions, and expose their families in the UI.
  This document owns **session-bound MCP delivery**, shared with optional
  plug-ins, and its built-in session-control endpoint.
- **Authority:** the [umbrella](2026-09-25-hennery-architecture-design.md)
  wins. The parent's brief explicitly requests delegation, including any
  runtime/model offered by the host. §11 identifies the narrow spec amendments
  that must be ratified before implementation; this draft does not silently
  change the umbrella's operator-only controls.
- **Evidence:** source inspected at `0fe663b86f7d61d3ff358eac7d7e8ea57dff64bf`;
  [ACP core](2026-09-26-acp-core-design.md),
  [gateway](2026-09-26-mcp-gateway-design.md),
  [kernel](2026-09-26-kernel-design.md),
  [client view](2026-10-02-client-view-design.md), and
  [plan 8e](../plans/2026-10-16-gateway-session-delivery.md).
  The private predecessor's parent/child design and
  `internal/server/session_tools.go` were read; no private deployment details
  or dependency on that service are introduced here.

---

## 1. Goals and boundaries

A parent starts a child in a selected directory, supplies a brief, chooses its
runtime and configuration, sends follow-ups, inspects progress, answers ordinary
questions, and closes finished work. The child gets its own ACP session and
adapter process. A child can use a different runtime or model from its parent.
Its turn ending is a report to the parent, never a declaration that the task
is verified or that the operator approved anything.

Reports, questions and failures survive collector restarts. A parent busy with
another turn receives them when it can take a prompt. A parked parent can be
resumed. If the parent cannot take responsibility, the operator sees the
unresolved work and receives the existing hat-scoped notification. The operator
can always open a child and answer directly.

The first increment does not create or remove Git worktrees, adopt unrelated
sessions, move sessions between hosts, import predecessor conversations, expose
an arbitrary-session transcript reader to agents, or add a board or chat-app
orchestrator. **Core** owns session identity, scoped MCP delivery, lifecycle and
the small generic plug-in contract. Delegation's lifecycle tools are built in
but **off by default, opt-in per hat**. **Optional plug-ins**, including memory,
run as MCP services connected through existing gateway connections and mounts;
they are not compiled-in memory handlers or forced background jobs. Memory's
indexing, storage and tools belong to that plug-in's design. This distinction
implements the operator's steer of 2026-10-14.

Recommendation: launches stay on the caller's host and in its hat initially.
Any configured runtime that host actually offers remains selectable. Cross-host
placement is a later extension with an explicit host grant; no new host-wide
or installation-wide session-list capability is implicit in delegation.

## 2. Decisions

“Designer’s recommendation” means pending review, not an operator ruling.
The two OPERATOR QUESTIONS are reproduced verbatim in §12.

| # | Chosen option | Alternatives rejected and reason | Classification |
|---|---|---|---|
| D1 | Collector is canonical; existing lifecycle, pending set, answer queue and host outbox remain authoritative. | A second child-session engine or in-memory delivery goroutines lose reconciliation and restart guarantees. | **settled by spec** — umbrella §§5.2, 5.5–5.7, 6.1, 6.8. |
| D2 | One session-bound delivery/identity mechanism; opt-in built-in delegation at `/mcp/builtin/hennery`, optional plug-ins through existing mounts. | Embedded memory forces a feature on all users; a second session-token scheme duplicates identity. External plug-in endpoints are intentional, not a second session identity. | **designer's recommendation** for the mechanism; opt-in/plug-in boundary mandated by the operator, 2026-10-14. |
| D3 | Reuse rotating `hnry_session_…` bearer tokens and revocation watches; identity never comes from tool arguments. | Predecessor token-in-URL and reusable plaintext token storage conflict with current delivery and revocation. | **settled by spec** — umbrella §8.3; gateway §§3.1–3.2. |
| D4 | Built-in tools obey the same delivery/isolation decision as external mounts. | Offering memory or delegation to a non-default-hat unisolated adapter bypasses the existing boundary. | **settled by spec** — umbrella §8.5; plan 8e. |
| D5 | Child runtime/model/config are independent; explicit unavailable values fail without running the brief on a substitute. | Cloning the parent's runtime or falling back silently violates the operator's instruction. | **designer's recommendation**, implementing the explicit brief; selection is already in umbrella §1.2 and ACP core §4.3. |
| D6 | Same host and hat, direct-child authority only; canonical workspace-root check before spawn. | Arbitrary placement and transitive family control enlarge authority before there is a host grant model. | **designer's recommendation**; hat separation itself is **settled by spec**, umbrella §8. |
| D7 | Persist command acceptance and child-event delivery; reuse one-turn scheduling and reconciliation. | Blind retry after a timeout can duplicate work; polling-only parents miss the promised reporting loop. | **designer's recommendation**, constrained by umbrella §5.7 and ACP core §4.4. |
| D8 | Parent answers elicitation; permission requests raised by a child go to the operator and cannot be answered by the parent. Choosing an offered non-asking mode is allowed. | Treating every pending callback as parent-answerable silently delegates execution approval. | **OPERATOR QUESTION Q1** — default in §12; mode selection clarified by the operator, 2026-10-21. |
| D9 | One generation of children initially; preserve a schema that can represent deeper trees. | Unlimited recursive spawning makes resource and escalation bounds unclear. A permanent flat schema makes future nesting unnecessarily hard. | **OPERATOR QUESTION Q2** — default in §12. |
| D10 | Explicit parent `session_close` closes only its direct child; never cascade close or delete. Closed parents are never auto-resumed. | Closing a family destroys independent progress; auto-resuming a closed parent contradicts “I am done”. | **designer's recommendation**, requiring the narrow umbrella §6.3 amendment in §11. |
| D11 | Parent/child inputs carry typed provenance and links; assistant reports stay untrusted. | Calling them operator turns poisons memory, notification budgets and authorization reasoning. | **designer's recommendation**; supports umbrella §§5.2, 8.3. |
| D12 | Fall back after a bounded delivery/handling delay, without expiring the question. | Suppressing attention just because a parent row exists can leave a child blocked forever. | **designer's recommendation**; no pending timeout is **settled by spec**, umbrella §6.5. |
| D13 | Reject a present foreign Origin at the built-in endpoint; allow missing Origin with bearer auth. | Requiring browser Origin breaks agents; inheriting the gateway proxy's complete Origin exemption misses the MCP server transport requirement. | **designer's recommendation**; explicit kernel clarification in §11. |
| D14 | Stateless HTTP MCP responses; no server-to-agent notification stream for child events. | MCP notifications alone do not reliably start an ACP parent turn or survive a restart. | **designer's recommendation**. |
| D15 | Close/delete/reassign terminate routing authority explicitly; no family ownership can be acquired by changing an id. | Cascading deletion and automatic adoption either lose independent work or create a confused deputy. | **designer's recommendation**; delete/step-up remains **settled by spec**, umbrella §6.10 and kernel §3.4. |

## 3. Shared foundation: session-bound MCP delivery

### 3.1 Identity and delivery

The built-in session-control server name is **`hennery`**, its endpoint is
**`<public_url>/mcp/builtin/hennery`**, and the credential is the session's
existing **`Authorization: Bearer hnry_session_…`**. Optional memory and other
plug-ins are ordinary mounted MCP entries, reached at `/mcp/<slug>` with that
same session bearer. Both use `SessionMcp::servers_in`, ACP delivery, principal
resolution and revocation; there is no second agent token, guessed session
identity from cwd, or user-provided caller id. Memory tools are served by the
plug-in, not registered on the built-in endpoint.

Add `hat_session_tools { owner_id, hat_id, delegation_enabled, revision }`,
defaulting to disabled for every existing and new hat. The operator enables it
through `PATCH /api/hats/{id}/session-tools { delegation_enabled: bool }`,
behind the existing browser Origin/cookie checks and step-up; authenticated
`GET` returns the flag and revision. Enabling takes effect at the next start or
resume; show this in the UI. Disabling immediately denies built-in calls,
cancels undispatched delegation work and routes outstanding attention to the
operator. Already delivered work continues as an ordinary session. Do not
revoke unrelated external mounts: cut built-in in-flight calls with a separate
hat-grant watch in addition to the existing token watch. Recheck this grant at
acceptance and dispatch. Re-enabling does not replay cancelled work.

The hat flag alone is insufficient authorization. Persist a nullable
`delivered_delegation_revision` on the existing `gw_session_tokens` row, bound
to that row's token hash/generation. Set it to the current hat revision only
when this start/resume actually delivers the built-in descriptor; otherwise
write NULL, including for an external-mount-only token. Mint/replacement and
this field commit atomically. Existing rows migrate to NULL. Authorization
requires a live token, a non-NULL delivered revision equal to the current
enabled hat revision, and the current isolation eligibility. Check all of
these again in the mutation transaction. Every enable/disable transition
advances the revision monotonically; never reset/reuse it. Thus enabling does
not upgrade an existing external-MCP token, and disable→enable cannot revive
an earlier delivered grant. Only a new start/resume can deliver the new grant.
Queued commands carry their accepted grant revision and cannot cross a policy
revision change, even if a fresh token has since been minted.

This is a collector-local server, not a `gw_connections` row or a proxy
upstream. External `/mcp/<slug>` routes stay external. The binary wires the
exact built-in route before the proxy's deeper-path rejection; tests must prove
that a connection whose slug is `builtin` still works at `/mcp/builtin` and
that other deeper paths remain rejected. The built-in entry's name is reserved
against delivered local stdio names; configuration must reject a collision.

Extend `SessionMcp::servers_in` with a typed optional built-in descriptor,
provided by trusted full-mode wiring only when the hat opted in. Its fixed
path/name cannot come from an agent or connection URL. Mint exactly one token
if either a built-in entry
or external HTTP mounts are to be delivered; reuse it in all their headers.
Today `session.rs` mints none when there are no external mounts: this condition
must change. The session row, family edge, token and delivery decision commit
before sending the start/resume frame. A failed transaction sends nothing.

The same `mcp_session_delivery` decision gates both kinds of entry. A host with
no MCP delivery capability, or an adapter in the fallback's non-default hat,
gets neither. The launcher/status explains `session_tools_unavailable`; an
agent launch cannot waive isolation. Until plan 8h proves Codex isolation,
this is a real deployment constraint, including for memory. The design never
restricts model selection merely to make isolation appear to work.

Explicit [per-hat source policies](2026-10-28-mcp-source-policy.md) extend this
legacy decision: a qualified combined attach intentionally permits native MCP
while retaining scoped managed delivery, including built-in session identity.
It is neither `isolated` nor the legacy `unisolated` waiver. Admission checks
current host/profile support even for an empty managed list. Children and
automatic resumes use their hat's current policy at the next real attach;
active and presumed-reconnected processes retain their acknowledged snapshot.
Built-in authorization also rechecks the current delivery eligibility. In
particular, a host default-hat change cannot leave an old unisolated token
authorized for delegation after its hat becomes non-default. Plan 8e's wider
open question about external streams remains separate; do not inherit that
gap for these more powerful built-in tools.

Mint on start and resume; revoke/cut through existing `commit_then_cut`
paths on real park, close, loss, failure, reassign, delete and host revoke.
Presumed parking does not revoke (gateway §3.1). On the built-in endpoint,
register the token watch before resolution, keep it through the response, and
recheck scope inside each state-changing transaction. Revocation stops new
acceptances; it cannot undo an operation committed before it. Such an operation
has a durable receipt and is subject to the family-lifecycle rules in §7.

Initialization and tool discovery must work while the session is `starting`:
adapters contact MCP during `session/new` and `session/load`. Do not recreate
the predecessor's pending-token/bound-token lookup race. The session id and
token are already committed together. Mutating tools return `session_starting`
until the caller is attached and reconciled; tool discovery is stable during
this interval. A plug-in startup call may use the same committed identity in
`starting`; it does not gain delegation mutation rights.

### 3.2 Module seam and contract for memory

The gateway continues to import nothing from sessions or memory (umbrella §9).
Reuse its identity resolver, token type and revocation watcher behind a small
public authentication seam. Full-mode wiring supplies the owner from the
owner-scoped store: `Principal` currently includes session/host/hat but not
owner, so the endpoint must never accept a client-provided owner to complete it.

The session-side built-in router constructs a private
`SessionToolContext { owner_id, session_id, host_id, hat_id, canonical_cwd,
token_generation }` after verifying the session row against that principal.
`token_generation` is an internal revocation fence, never returned to a tool;
built-in authorization additionally carries the verified delivered delegation
revision from §3.1. Neither is inferred from the current hat flag alone.
Handlers receive this context, bounded typed arguments, and a cancellation
watch. They do not receive the plaintext bearer. Rechecks use the same
transaction as reads/writes where authorization could race a reassign/purge.

Only session-lifecycle tools are registered in the built-in router. An external
plug-in uses the existing gateway mount and upstream service authentication;
never forward the raw hennery session bearer to it. The separate generic
contract is the memory design's **C1: generic feed/lifecycle** plan: authenticated
registration, owner/hat namespace credentials, scoped activity/lease and deletion
lifecycle. C1 also owns and defines the shared typed provenance (`Operator`,
`Parent`, `ChildReport`) on every feed item and startup-context entry, and the
typed `PluginContext` for startup. Delegation depends on these C1 types as
defined in the memory design; it does not define a parallel enum or payload.
The upstream credential authenticates exactly one owner/hat namespace;
the gateway's internal `SessionToolContext` is not automatically exported.
Existing mounts do not attest the calling session id. A tool-supplied cwd or
session id is at most a narrowing filter or explicitly unverified source hint;
remember records agent authorship, not verified session authorship. Verified
session provenance comes from C1's authorized activity feed. A future generic
signed-principal extension requires its own reviewed contract; it is neither
a hidden memory header nor a prerequisite for the reference memory tools.

The generic core contract additionally offers an **opt-in scoped event feed**
for completed turns, with owner/hat authorization derived from a connection's
explicit grant. A service feed credential is distinct from a live agent token;
it does not impersonate a session or gain session-control tools. Feed cursors
cannot widen scope. Revocation/unmount/purge cuts the feed and pending delivery;
stored plug-in copies follow its documented retention/deletion contract. Optional
startup context is the separate **C2: startup** plan, uses its explicitly scoped
plug-in grant and is marked plug-in data, never operator instruction.
It consumes C1's typed `PluginContext`, preserving each entry's C1 provenance
instead of treating retrieved context as new operator instruction. No feed or
startup callback runs without opt-in.
The memory lane owns C1/C2; delegation supplies turn provenance and the internal
identity seam, without implementing memory logic. §11 names their dependencies.

Memory may read only its verified context's owner/hat and approved subscopes.
An optional cwd can narrow a lookup after canonical validation; it cannot
choose an owner, hat or caller. Plug-ins must honor revocation, purge and
provenance (`Operator`, `Parent`, `ChildReport`) and must not promote a child's
claim of approval into an operator decision. Generic gateway-only mode still
builds without sessions or memory; session activity feed/context capabilities
are unavailable there. This design defines no automatic memory capture,
cross-hat sharing, import policy or LLM endpoint.

### 3.3 HTTP and MCP behavior

Use Streamable HTTP with JSON responses: `initialize`,
`notifications/initialized`, `ping`, `tools/list` and `tools/call`. Accepted
notifications return 202 with an empty body. Authenticated GET/DELETE return
405; there is no SSE channel or MCP session id. No sampling, resources,
prompts, server elicitation or legacy HTTP+SSE is advertised. Child questions
are ACP pending requests, not MCP server elicitation.

Negotiate supported protocol versions with pinned-adapter fixtures; reject
unsupported version headers. Tool results provide typed `structuredContent`
and a matching JSON text block for clients that use text. Tool execution
failures use `isError`; malformed protocol messages use JSON-RPC errors.
These choices follow the official [transport specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
and [tool result contract](https://modelcontextprotocol.io/specification/2025-11-25/server/tools).

Bearer authentication never falls back to cookies. Missing, repeated,
malformed, revoked, superseded or foreign tokens get the gateway's uniform
404. A present Origin must equal `public_url`'s origin; invalid Origin gets
403 before reading the body. Missing Origin is accepted. No CORS; no tokens
in URLs, logs, tool results or timeline events; `no-store` and `nosniff` on
responses. Use bounded early-reject draining so a large refused body does not
turn a useful refusal into a reset (plan 8e's deferred item).

Parse a single JSON-RPC message once with strict typed dispatch. Reject batches,
duplicate keys, invalid encodings and ambiguous argument shapes. Never validate
one representation and execute another. Share safe primitives with the proxy,
not its upstream forwarding path. Retain the gateway's body/deadline ceilings;
tool-specific limits below are smaller. A disconnected HTTP caller does not
cancel an already committed spawn or send.

## 4. Agent-facing tools and launch configuration

All new public shapes live in Rust in `hennery-proto` (view types in
`hennery-view`), derive JSON Schema and TypeScript as appropriate, and join
the existing generator checks. The descriptions below specify types; they are
not a second hand-written TS contract. `agent` means the configured runtime
identifier already used by `StartSessionRequest`.

| Tool | Typed arguments | Result and behavior |
|---|---|---|
| `session_capabilities` | `CapabilitiesArgs { agent: Option<String>, cwd: Option<String> }` | Caller-host runtimes, availability, model-facing config catalogs, catalog freshness, delivery modes and limits. Every offered model/mode/option is allowed with Delegation on. Catalogs omit internal `security` and installed-profile metadata. Omitted cwd means caller cwd; supplied cwd is validated in the same hat. No other hats' paths, sessions or catalogs. |
| `session_spawn` | `SpawnArgs { operation_key: String, cwd: String, brief: String, name: Option<String>, agent: Option<String>, config: SessionConfig }` | `OperationReceipt` with a durable child id once allocated, launch/config state, and initial-brief delivery state. No caller/parent/host/hat argument. |
| `session_send` | `SendArgs { operation_key: String, child_id: String, text: String }` | Accept a follow-up into the child's bounded durable queue. Resume a genuinely parked child first; never reopen a closed one. |
| `session_answer` | `AnswerArgs { operation_key: String, child_id: String, pending_id: String, action: ElicitationAction, content: Option<Value> }` | Existing answer acceptance receipt plus delivery state, for direct-child elicitation only under Q1's default. `accept` content is an object; validate through the same answer service as the operator. |
| `session_status` | `StatusArgs { child_id: String, operation_key: Option<String> }` | Lifecycle/activity, selected and requested config, open questions, last turn outcome, bounded last assistant report, queue/receipt states and fallback reason. |
| `session_list` | `ListArgs { cursor: Option<String>, limit: Option<u16> }` | Paginated direct children and brief state summaries; no recursive or owner-wide enumeration. |
| `session_close` | `CloseArgs { operation_key: String, child_id: String }` | Explicit close of that child, cancellation of unstarted follow-ups, and existing confirmed/unconfirmed close outcome. No delete or filesystem cleanup. |

`OperationReceipt` carries `operation_id`, optional `child_id`, `turn_id` or
`pending_id`, and a tagged state: `accepted`, `waiting_config`, `queued`,
`awaiting_reconciliation`, `delivered`, `completed`, `not_delivered`, `cancelled`
or `failed { code }`. Delivery is not completion. Spawn's idempotency key can
also be queried by retrying the same spawn; it must not require knowing the
child id from a lost response. `session_status` presents per-operation records
alongside session state rather than overloading lifecycle with these values.

Every mutation requires a bounded opaque `operation_key`. Uniqueness is
`(owner_id, caller_session_id, operation_key)` across tool names. Same key and
same canonical typed request returns the existing receipt; same key with a
different request returns `operation_conflict`. Persist the digest and receipt
before side effects; token rotation does not invalidate this session-scoped
deduplication. Explain this retry contract in tool descriptions.

### 4.1 Catalogs and first prompt

Use one runtime/config catalog service for this tool and the dashboard launcher.
Operator previews persist in SQLite by owner, host, hat, canonical cwd, runtime
and adapter revision, with the collector observation time. On insertion, prune
older revisions in that exact scope in the same transaction. Authoritative
real-session start/resume catalogs and explicit operator Refresh probes write
through to it. Never borrow another host's, hat's, cwd's or runtime's catalog.
The most recently observed revision is only a saved preview: reconnects and
collector restarts retain it, and a later real session or Refresh replaces it.
Return `cached` with its age, or `unknown` when no exact-context snapshot exists.
`cached` never satisfies delegation's `Fresh` checks or configuration evidence.
Preview admission limits (256 KiB, 128 options, 256-byte revision) skip only
the saved preview; they never reject a valid durable session fact. An earlier
snapshot keeps its original age. Deleting a session removes only previews
sourced by that session, including copies supplied before a hat reassignment.
A newer preview supplied by another session in the same scope, and an
independent Refresh snapshot, remain intact. Track the baseline's source
separately when its offered options come from a later attachment: deleting
the baseline's session clears only those defaults, preserving another session's
options. Hat purge, host forget and owner deletion still remove their scope.

The operator preview's `defaults` are the adapter's own `session/new` values
before hennery applies switches, carried by host identity metadata only for
an operator new attachment, or the result of an explicit Refresh. Session
write-through always updates offered options; resume/restart and delegated
attachments preserve an existing baseline for the same adapter revision.
Without such a baseline, defaults are empty and unknown, never copied from
the session's current model/mode/axes. A revision change cannot borrow the
previous revision's baseline. Delegation continues to use only fresh live
catalogs, with its existing inheritance and configuration checks.

Opening New session or Defaults only reads the saved preview; no automatic,
background or timer-driven probe runs. Refresh is an explicit operator POST.
No catalog means Agent defaults and an immediately enabled Start once host,
runtime and canonical cwd are valid. Without a catalog, send every hat override
and explicit pick, including while the preview loads. With a catalog, keep
security overrides strict even when unsupported; unsupported nonsecurity
overrides fall back visibly to the supported baseline or agent default
(UI spec §6.3). Configuration refusals are readable and keep the first prompt;
the no-preview flow offers an explicit new start omitting only a selected
unconfirmed nonsecurity override. Saved choices remain subject to the real
session's post-start verification. The collector independently ignores
`agent_defaults` from any child with a delegation edge, even a detached one.

For a cold catalog, a new capability-gated host probe obtains the adapter's
catalog in the validated cwd without a user prompt. If discovery requires
`session/new`, use a bounded disposable probe adapter with no MCP entries,
publish no user session, and release its whole process group on success, error,
timeout or cancellation. A host advertises `catalog_probe_cancel` separately
from `catalog_probe`. Dropping an unanswered collector waiter sends a
connection-scoped cancel frame only to a host advertising cancellation; that
host drops the probe's work and releases its permit. For a legacy host with
only `catalog_probe`, the collector removes its waiter and ignores late
responses; the host finishes at its own bounded timeout, without early
cancellation. Connection loss also cancels disposable probes on new hosts. Agent
hooks can still run: this is a trusted-host operation, not a side-effect-free
filesystem read. Limit concurrent probes and deduplicate identical requests.
Independent keys never hold a collector-wide mutex across I/O. Descendants
that create another session with `setsid` escape process-group containment;
the fake-agent regression documents this boundary. Cgroups and subreapers are
outside this change.
If the adapter cannot supply a fresh pre-start catalog, fail only that spawn
with its readable reason. An unauthenticated runtime returns `auth_required`
(not signed in), including when authentication fails during child attach.
Offline, probe and authentication failures never block enabling the hat.
Do not silently substitute a runtime or configuration.

An omitted agent inherits the parent's agent. An omitted model inherits the
parent's current model only for the same agent; otherwise use that agent's
reported default. Operator decision, 2026-10-21: an omitted permission mode
inherits the parent's current host-confirmed mode for the same runtime. Across
different runtime ids, only installed `Profile::Claude` with `bypassPermissions`
maps to installed `Profile::Codex` with `agent-full-access`, and vice versa.
Use typed host-authored `hello.runtime_profiles` (`RuntimeProfile`), never runtime
names, display labels or security baseline values, to identify these profiles.
The authenticated connection owns this map; replacement/disconnection drops it.
Lookups never probe a runtime or consult another catalog. Installed-profile
metadata appears only in hello, never in `HostCatalog` or the operator catalog API.
The map defaults to empty for older hosts; unknown future profile strings
decode to `Unknown` and never map modes. Missing or unverified profile evidence,
other profiles and other cross-runtime modes use the child's
runtime default. An inherited mode must be offered in the child's live catalog;
otherwise use the child's runtime default without failing the spawn. An explicit
`config.mode` always wins, even if unavailable (then it fails normally). Other
axes use runtime defaults unless explicitly supplied. Explicit values are
checked against the target catalog and then
against the newly started child's reported configuration. Apply model, axes,
then mode, as ACP core §4.3 requires.

Operator decision, 2026-10-21: the enabled flag is the entire delegation
authorization. Every configuration offered by the current fresh catalog is
allowed, including Generic and ClaudeOwnCli profiles with unknown security
classification. Profile metadata remains useful for defaults and warnings,
but cannot narrow delegation. No execution-policy table, API or grant snapshot
is used. A runtime signed in later, or a newly offered model/mode/option,
is usable without toggling again. This preserves 0.2.0 ON's union of all
catalog values while making “all” current. Narrowed v0.2.0 grants are dropped
on upgrade without changing the enabled flag or its revision. Children cannot
start children. Offered modes may run without asking for permission; when an
adapter does ask, permission requests remain operator-owned.

Model-facing clarification, 2026-10-21: `CapabilitiesResult.catalogs` contains
`CapabilityCatalog` projections with `agent`, `adapter_revision`,
`config_options`, `defaults` and `freshness`. `HostCatalog.security` stays on
the host/operator API for warnings and internal applied-configuration evidence;
it is not a limit on selectable modes and never appears in the tool result,
including unknown catalogs. The dashboard keeps the complete `HostCatalog`.
Installed profile identity is internal to mode mapping and also omitted from
the model-facing projection.

The model-facing `session_spawn` description says:

> With Delegation on, you may pick any model, mode and option in the child runtime's catalog (session_capabilities), including modes that don't ask for permission (Claude bypassPermissions, Codex agent-full-access).

Its single inheritance sentence is:

> If mode is omitted, inherit the parent's mode for the same runtime, or map installed Claude bypassPermissions to Codex agent-full-access (and vice versa) across runtimes when offered; otherwise use the runtime default.

`session_spawn`, `session_answer` and `session_status` all clarify:

> Permission requests raised by a child go to the operator; the parent cannot answer them.

This distinguishes selecting a non-asking mode from answering a request that
the child actually raises; no additional approval is needed to select an
offered mode with Delegation on.

Child-report notices use the same permission-request clarification. Tool
descriptions, notices, error hints and receipts must not use the ambiguous
phrase “permissions remain operator-only”.

Pinned adapter source inspection (Claude ACP 0.93.0, Codex ACP 2.2.2) confirms
that a standalone legacy `current_mode_update` must be handled: Claude's
`SessionModeController.publishCurrent` emits it, including fallback paths;
Codex's inspected command/config paths emit `config_option_update` snapshots.
Live mode deltas update the host's cached mode and the collector's current
mode without clearing unrelated settings. They never promote stale options to
complete evidence. When the current catalog has no mode axis, legacy mode
notifications leave configuration evidence and its revision untouched, so queued
follow-ups retain their proof. Replayed deltas cannot overwrite post-attach
configuration.

Operator review amendment, 2026-10-21: reject nested callers, child-count and
start-budget excess, and other-hat paths before catalog probing; retain the
transactional checks at allocation. Independent catalog contexts probe without
holding a collector-wide cache lock. At attach, `auth_required` and
`runtime_unavailable` fail accepted work with that delegation result; new
starts also record a visible session failure. Stored sign-in failures remain
`agent_not_logged_in`, preserving adapter messages and login instructions.
Second operator review amendment, 2026-10-21: before any probe, read the
eligible queue head and lifecycle. A handed-off start (`launch_sent=1`) waits
for host evidence without catalog probes. Only unlaunched starts and attachable
parked work discover catalogs. Reuse a fresh catalog captured after operation
admission (including the tool's own spawn probe), within its freshness TTL;
recheck all allocation/attach predicates transactionally. A fresh catalog that
no longer offers the queued configuration settles that head as
`configuration_unavailable`; a missing resume record uses `agent_has_no_record`.
Parked sessions remain parked and
new starts record the failure. These pre-attach failures differ from applied
configuration mismatches on an already attached child below.

Busy, timeout, adapter and transient catalog failures retry for 30 seconds from the
first failed probe for that pending operation, irrespective of how long it was
queued beforehand. Accepting a fresh catalog clears the target’s pending
retry clocks, so a recovered outage cannot expire a later follow-up. In-memory
retry clocks may also reset after collector restart.
Authentication/runtime unavailability remain immediate, and offline behavior
is unchanged. Concurrent catalog callers share success and failure results;
workers back off shared failures briefly without blocking independent contexts.
Final operator review amendment, 2026-10-21: cached failures and Unknown catalogs
may be reused only if captured at or after that operation’s admission. A host
that disconnects or is not yet reconciled defers attach; only a ready host’s
known missing capability is a permanent configuration refusal.
Evidence confirms requested keys plus catalog security axes, ignoring unrelated
current settings outside the adapter's option list.

Start the child **without its brief**, wait for `session_started` and config
read-back, then submit the brief through the normal turn path. A rejected or
clamped explicit setting leaves an attached, visible child in `waiting_config`
with no brief delivered. This preserves ACP core's rule that a config failure
does not fail an otherwise successful start. The operator can fix configuration
in the existing UI; the saved brief is released only when its required values
are confirmed, or cancelled explicitly. A fresh spawn with another operation
key is a distinct child, not an implicit retry.

Apply the same release gate after **every automatic resume**, whether the
destination is a child receiving a follow-up or a parent receiving a report.
Normal resume success does not prove configuration: `session_started` can
precede a `reapply_failed` note, and an absent read-back leaves old stored
values intact. Before starting the resume, invalidate dispatch eligibility and
allocate an attach generation. Require a host-produced typed
`ConfigEvidence { attach_generation, config_revision, completeness, actual }`
from the completed reapply/read-back for that generation, carried on the
sequenced session stream. No evidence, partial evidence, a stale generation,
or a retained `sessions.model/mode/config_axes` value is proof of current config.

Release queued work only after fresh complete evidence satisfies the saved
required configuration and that configuration is offered by the fresh catalog.
On mismatch or unknown configuration hold it in
`waiting_config`, expose requested/actual/unknown values, and apply the normal
attention deadline/fallback. A current-generation repair/read-back may release
it after revalidation; never silently run under adapter defaults. Record the
validated attach/config revisions on dispatch; invalidate eligibility on
later relevant configuration changes or delegation disable. The host's serialized prompt admission
checks the expected attach/config revision too, so a switch after collector
validation cannot run the queued prompt under another configuration. Hosts
without this evidence/fencing capability cannot auto-resume delegated work.

The canonical cwd must exist, be a directory under the host's canonical workspace
roots, and resolve to the parent's stored hat. Check before allocating the
child, and again at the host spawn boundary; a symlink/root/rule change cannot
start work in a different directory/hat using a stale grant. Use bounded opaque
refusals, not another hat's name or matched path rule. Starting in a worktree
is supported, but creating one remains the parent/operator's responsibility.

### 4.2 Questions and authority

Use the existing `pending_id`, never the ACP request id. The tool does not parse
raw ACP itself: question display/schema comes from `hennery-view`; pending kind,
option ids and answerability come from canonical typed extracts. Match existing
answer validation; the adapter remains responsible for its elicitation schema
semantics (ACP core §4.6). Parent and operator submissions share the transaction
and unique pending-id answer slot; first accepted submission wins, and the UI
only marks answered after `answer_result { delivered: true }`.

Under Q1's default a parent may accept/decline/cancel ordinary elicitation,
including answering from its brief. Permission requests always remain operator
attention and are rejected by `session_answer` with `operator_required`.
The parent sees a status notification that intervention is needed, without an
answer-capable permission schema. A parent must escalate new product decisions,
missing facts and unauthorized outward actions through its own question tool.
This semantic boundary is guidance to a model; it cannot reliably classify an
arbitrary elicitation as “actually a product decision”. Enforce privileged API
restrictions independently of that guidance.

## 5. Durable commands and reporting

### 5.1 One prompt at a time

Both accepted parent follow-ups and child-to-parent reports use a collector
queue. A worker reserves the destination's existing turn slot atomically and
sends the normal `prompt` control frame. Host actor serialization remains the
final guard; `blocked` counts as busy. Do not add the predecessor's separate
unsequenced `inject` frame or overlap an operator prompt with a report.

Queue acceptance and the destination turn reservation are distinct. When
dispatching, persist the operation-to-turn mapping and its reserved slot in
one transaction. `turn_started` acknowledges delivery; `turn_ended` records the
outcome. Never mark delivered from a socket write, MCP success or model prose.
Operator amendment (2026-10-21, sessions migration 29): operator prompts submitted while busy,
waiting, parked or behind accepted work join this same FIFO, with typed
`Operator` provenance and no delegation grant dependency. Both sources use
one position order; an immediate operator prompt cannot overtake a queued
head. Child notices drain only when this prompt FIFO has no ready head.
Queued work never interrupts a turn. Operator items are editable/removable
only before turn reservation; failed items retain their checked content until
operator dismissal or seven-day expiry; deletion and hat purge scrub them with
the transcript, including optional prompt request-key receipts.
Delegation grant changes do not cancel operator input.
The shared 32-active-item / 1-MiB bound includes operator image bytes and excludes
retained failed operator items. ACP core §4.4 describes operator failure/cancel/restart
semantics and the queue API; adapter promptQueueing remains unused.

Acceptance and dispatch must test **close intent**, not just lifecycle. In the
transaction that accepts a command, reserves a turn or begins an automatic
resume, require `close_requested = 0` and non-closed/non-deleted endpoints on
both sides of the family transfer, together with the current family/grant
revision. Committing close intent atomically cancels all undispatched transfers
involving that endpoint. A prior eligibility read or worker lease cannot
authorize a later commit. Serialize final host handoff with close through the
same per-session dispatch fence: a reservation not handed off when close wins
is cancelled; a frame already handed off is reconciled and then torn down.
Closing the source also fences a target start/dispatch already being prepared.

An idle eligible destination drains FIFO. Automatic resume shares the ordinary
resume machinery but has a separate transactional compare-and-set predicate:
`lifecycle = parked`, `presumed_parked = 0`, `close_requested = 0`, both family
endpoints eligible, and the host connected and reconciled. It must not call
the ordinary operator path with a precheck alone: that path accepts closed and
failed sessions and clears close intent. A presumed park first reconciles;
confirmed attachment uses that actor, confirmed loss may enter the genuinely
parked path. Automatic resume never clears close intent. After resume, §4.1's
fresh generation-bound configuration gate precedes every queued prompt.
An explicit operator Park sets a durable hold across both FIFO sources; only
operator Resume or a new operator Send clears it. Idle and host-restart parking
do not set a hold; an existing explicit hold survives host restart. A reconciled
attached session releases an ambiguous Park request that did not detach it.
Operator automatic resumes require `operator_resume` and
`delegation_config`, with an explicit hennery 0.3.0 upgrade reason on older hosts.
Adapter exit fails unsent operator input with a retained readable reason;
parked offline/revoked hosts also fail waiting operator input readably.
Awaiting operator reconciliation fences both FIFO sources. Reconciliation
settles the receipt before commit; a proven unsent operator head returns to
the same queue position only with the independent `prompt_mailbox_snapshot`
capability; legacy-host uncertain delivery fails readably with input retained.
Host `turn_in_progress` refusals also requeue the head, remove the unsent turn
and image links, and persist exponential backoff capped at ten seconds. Each
session's worker error is contained so it cannot starve later sessions.
Closed, deleted, failed-to-resume or hat-mismatched destinations stop dispatch
and cause the visible fallback below. Pending questions in the destination
prevent report prompts, even when asked outside a turn.

**Idle parking amendment (operator decision, 2026-10-20):** a parent's idle
reaper is held while any direct child's lifecycle is not `parked`, `closed`,
`failed` or `deleted`, including an attached child between turns, or a notice
for that parent is still `parent_queued`, `waiting_config` or
`awaiting_reconciliation`. `parent_delivered`, `operator_required`, resolved
and obsolete notices are not undelivered automatic work. Routing detach and
close intent do not stop a running child; the lifecycle condition therefore
does not depend on routing eligibility or host equality. Notice creation and
delivery/obsolescence invalidate the parent's hold in their own transactions.
On release the host starts a full idle window, without extending it again for
duplicate releases. Explicit Park and Close remain available.

Designer’s recommendation: combine this with a persisted operator pin in one
effective hold, computed by the collector and sent as capability-gated
`set_idle_hold { session_id, held }` to `idle_hold` hosts. Retain the owner gate
through snapshot and handoff; send before a child's start/resume handoff and
restore all current states after reconciled `resend_complete`. Retained host
state covers updates arriving before attachment and survives reconnect. Hosts
without the capability keep the existing reaper and durable automatic-resume
fallback. The pin API, Origin/auth rules and observed limits of adapter
background-job detection are specified in ACP core §4.7. No new ACP job parser
or UI toggle belongs to this amendment.

### 5.2 Source events and delivery identities

On applied child turn, pending, failure and availability transitions, record a
`DelegationNotice` in the same transaction as the fact/transition. Its kind is
`TurnEnded { turn_id, outcome }`, `Question { pending_id, kind }`,
`QuestionCancelled { pending_id, reason }`, `SessionFailed { code }` or
`SessionUnavailable { attach_generation, reason, affected_pending_ids }`.
Include `AdapterExited` outside a turn, confirmed loss after reconciliation,
host revoke, reported park, and pending cancellations from adapter loss, host
restart, park, close or withdrawal. Presumed offline parking is an availability
notice, not proof that pending callbacks were lost. An ordinary idle park with
no pending work is status-only and does not repeatedly wake the parent.
An isolated withdrawal uses `QuestionCancelled`, not a claim that the session
is unavailable; cancellation caused by loss joins that loss incident below.
Unique source key:
`(owner_id, child_id, source_event_id, notice_kind)`.
Use applied outcomes only; replayed/non-applied facts create nothing. One
failed/interrupted turn produces one report with its failure detail, not a
second redundant generic-error report. Additionally coalesce loss-related
adapter-exit, park, cancellation and interrupted-turn facts by one persisted
incident identity for the child's attach generation/detach transition. Merge
their source references and reasons into one notice; if already delivered,
update status rather than starting another parent turn for the same incident.
Do not fold an unrelated later question withdrawal into that incident.

In the transaction that applies a loss or pending cancellation, also mark all
affected undelivered question notices obsolete and cancel their queued prompt
transfers. This includes diagnostic `AdapterExited` before its subsequent park:
its callbacks are already unusable, even if the canonical pending cancellation
arrives in the next frame. The loss notice explains that the parent must inspect
or resume instead of answering the old question. Canonical pending transitions
and answer verdicts retain their existing sequencing; delivered question text
cannot be recalled, and stale answers still consult live state. An idle adapter
crash or loss of an out-of-turn question must therefore notify the parent even
when no `turn_ended` exists. With a closing/absent parent, use operator fallback.

Persist references to source events, not a second full transcript. At dispatch,
read the bounded assistant report through `hennery-view`, preserve its ending
on a UTF-8 boundary, and include `truncated`, turn id and a dashboard link.
Never parse raw ACP a second time in sessions to find “the final answer”.
A completed turn without assistant text still yields an outcome and link.
Unrecognized adapter output is visible, not silently treated as success.

Each delivery receives a stable `delivery_id`; allocation of its destination
turn and association to its notice are atomic. Workers claim rows with a lease,
retry pending claims after restart, and recheck family/hat/pending state before
dispatch. A resolving question is marked obsolete before dispatch; a report
already delivered cannot be taken out of the model's context, so a later
answer still checks the live pending set.

### 5.3 Reconnect and honest retry semantics

No new exactly-once claim is made about an LLM or ACP side effect. The durable
queue guarantees that every accepted item has a recorded disposition and that
the collector does not knowingly dispatch the same delivered turn twice.

If a connection drops after sending, mark `awaiting_reconciliation`, retaining
the same request/turn association. Do not resend a prompt or spawn while it
may have executed. After the host's outbox resend and reconciliation:

- An ingested start/turn fact settles the existing receipt.
- A proven undispatched queue item can be sent normally.
- A dispatched delegated turn that becomes `not_delivered` stays visible and requires an
  explicit retry, as ACP core §4.4 does; notify the parent or operator. Late
  `turn_started` can still win. Do not create a replacement turn automatically.
- A started interrupted turn is a report of partial work; it is never replayed
  automatically. An answer, unlike a prompt, follows the existing durable
  pending-id resend/deduplication path.

Retries after an uncertain spawn reuse the operation key and child id; they
do not create a second worker. A parent token superseded on resume can query
its prior operations through its new token. Revalidation still denies a
detached family or another hat.

### 5.4 Fallback and limits

Routing state is separate from lifecycle: `parent_queued`, `parent_delivered`,
`operator_required`, `resolved` or `obsolete`, with a reason. Default
operator settings (2026-10-21): 8 non-closed children per parent, and 32 child
starts per operator-prompt epoch of that parent. These limits are global to
the owner, not per hat. Each is an integer from 1 to 64; unset settings retain
these defaults. The epoch starts with the operator's last own prompt to that
parent; injected parent/child messages do not reset it. Fixed collector
protection caps remain 32 queued prompts/1 MiB per
destination, 64 KiB per brief/follow-up, 8 KiB per injected report and a page
of at most 100 child summaries. Bound child creation per
operator-prompt epoch, so closing/replacing children cannot bypass the
concurrent-child limit. Enforce caps transactionally. Do not silently
drop an accepted message. Queue overflow produces a receipt/refusal and an
operator attention record, never an unbounded payload backlog.

Persist a budget of automatic parent wake-ups per family (default 20) between actual
operator prompts to the root. Reserve budget in the same transaction as the
dispatch; retries of that delivery spend none again. Parent/child prompts and
memory jobs do not reset it. Closing and replacing children cannot reset it.
If Q2 permits nesting, apply the root budget and a maximum depth before enabling
it; do not ship recursion with a budget per edge alone.

The wake-up budget is also a global owner setting, bounded 1–64. Settings has
a Delegation limits card beside Delegation by hat, with all three values,
their defaults and reset rules. Authenticated `GET /api/settings/delegation-limits`
returns the effective settings; `PATCH` changes supplied fields after the same
fresh step-up used by the Delegation switch. Invalid values are rejected by
the server. Spawn checks and wake reservations read settings in their existing
transactions, and `session_capabilities.limits` reports the effective values.
Changes apply to the next allocation/reservation without closing existing
children, resetting spent counters or replaying escalated notices. This is the
operator decision of 2026-10-21.

Saving pins all three effective values in the owner's row, including values
omitted from a partial PATCH. Later code-default changes only reach owners
without a saved row; they do not change previously saved limits.

Immediate fallback: parent closed/deleted, routing detached, permanent resume
failure, unsupported delivery, budget exhausted, or permission question under
Q1's default. A disconnected/busy parent gets a **five-minute handling grace**
from the notice's creation, persisted across restarts. An unresolved question
falls back even if its notification was delivered but its parent never answered.
This is an attention deadline, not a pending-request timeout: the question
stays open indefinitely. A report delivered to the parent needs no answer.
**Operator amendment, 2026-10-28:** an unreserved report of a successfully
completed child turn also needs no operator answer: it stays queued while its
parent is busy or offline, without a handling deadline escalation. Reconcile
older `handling_deadline` fallbacks for these reports back to the queue,
clearing their attention and invalidating the family summaries. This repair
requires current routing eligibility, no reserved target turn and an applied
`completed` source event; it must not replay uncertain delivery, clear an
explicit delivery/configuration/budget refusal or suppress a failed turn.
Other undelivered notices and unanswered questions retain the deadline.

Fallback commits an operator attention record before asking the existing push
service to notify. The kernel push queue is bounded and in memory today; do not
claim guaranteed push delivery. **Operator amendment, 2026-10-28:** try push
only once per open attention episode, including across collector restarts.
The previous five-minute retry sent the same alert indefinitely and could
produce over 100 notifications for a few unresolved child sessions. A new
operator rearm, cancelled answer or reassignment may create a new episode;
the existing stable tag and current-state guard still apply. A dropped queue,
muted hat or failed push may leave no device notification. The dashboard is
the durable source in those cases and remains visible until resolution.
Recovered parents do not automatically inject items already escalated; an
explicit operator prompt can re-arm unresolved items. An operator answer
always obsoletes a queued question delivery.

## 6. Storage, migrations and provenance

Add forward-only sessions-component migrations; choose actual migration numbers
when the plans are based on current main. Do not edit a merged migration.
Existing sessions have no family edge and their existing turns default to
`Operator`, matching the present operator-only prompt API.

| Store addition | Required data and invariants |
|---|---|
| `hat_session_tools` (kernel component) | Owner/hat, delegation enabled flag and grant revision. Disabled by default, changed only through stepped-up operator settings; feeds/plug-ins have separate explicit grants defined by the plug-in contract. |
| Existing `gw_session_tokens` extension (gateway component) | Nullable `delivered_delegation_revision`, bound to the current token hash/generation; NULL unless the built-in entry was delivered, including for migrated rows. Mint/replacement updates it atomically; every built-in authorization compares it to the current enabled hat revision. |
| Session dispatch evidence | Attach generation and fresh complete config evidence/revision; invalidate at automatic resume/config change. Existing `close_requested` participates in every acceptance/dispatch/automatic-resume compare-and-set and final handoff fence. |
| `session_families` | `owner_id`, unique `child_id`, `parent_id`, `root_id`, `hat_id`, `created_event_id`, routing state/reason. Both sessions have the same owner/hat; no self-edge or cycle. Parent is assigned once at child creation; disabling an edge grants no new parent. |
| `session_operations` | Owner, caller, operation key/digest, tool kind, target child, receipt state, linked request/turn/pending ids, timestamps. Unique owner/caller/key; no plaintext credentials. |
| `session_dispatch_queue` | Owner, source/target sessions, origin, operation or delivery id, bounded content or source reference, accepted enablement revision, queue order, claim lease, attach/config revisions and receipt. One dispatch association per delivery; cancellation is explicit. |
| `delegation_notices` | Owner/hat/family ids, source events and kind, loss-incident identity, attention routing, destination turn, deadline and resolution. Unique source identity and coalesced loss incident; indexed by pending work/deadline and target. |
| `delegation_budgets` | Owner/root id, operator-prompt epoch and reserved wake-ups. Durable across collector restarts and child replacement. |
| Turn/answer provenance | Import C1's typed provenance (`Operator`, `Parent`, `ChildReport`) from the memory design; persist the corresponding server-verified source/operation/delivery references. Submitted answer origin is operator or parent. Never supplied in public prompt JSON; no independently defined delegation enum. |

Use composite owner foreign keys where possible, owner predicates in every
query, and transaction checks/triggers for same-hat and lifecycle invariants.
Indices cover `(owner_id, parent_id)`, queue readiness, source deduplication
and pending question lookup. Add all new SQL sources to the owner-filter audit.
There is no second database or token table. Completed queue payloads are
cleared after transcript/provenance persistence; receipts/deduplication survive
for the life of the sessions so a delayed retry cannot restart old work.

`hennery-view` renders `Parent` prompts as “From parent” and reports as “Child
report”, with server provenance. On the ACP wire these may still be prompt
content blocks, because adapters need not support a distinct author role.
Wrap child content as untrusted data with escaped metadata; restate that it is
not operator input or approval. The wrapper is defense in depth, not an
authorization mechanism. Consumers of user turns, including memory, automatic
naming, push suppression and budget resets, must use provenance explicitly.
Every feed item and startup-context entry carries C1's typed provenance through
projection and replay. Startup uses C1's typed `PluginContext`; C2 owns delivery,
not another type definition. Hostile text or spoofed role/delimiter strings
cannot set provenance, turn retrieved material into operator authorization, or
reset the delegation budget. Signed per-session attestation remains deferred
to a separately reviewed extension.

## 7. Close, delete, reassign and family ownership

- A parent may operate only on an active routing edge to its direct child,
  within the same current owner/hat. It cannot control a sibling, ancestor,
  unrelated session or grandchild. Children receive no parent transcript
  capability. Family UI visibility for the operator does not grant agent access.
- Closing a child is explicit and non-cascading. Use existing teardown order,
  pending cancellation and token revocation; cancel queued follow-ups. A close
  while busy ends the turn according to ACP core, and its report remains visible.
  No auto-close on a “done” message; no worktree deletion.
- The fence begins when `close_requested` commits, while lifecycle may still
  be active. Apply §5.1 at either endpoint; do not wait for `session_closed`.
  Terminal facts remain visible through status/operator attention, but do not
  start a new automatic prompt across a closed/closing endpoint's fence.
- Closing a parent stops automatic wake-ups and cancels its undispatched
  outbound commands, retaining receipts. Already delivered child work continues;
  children fall back to the operator. Resuming the parent explicitly may restore
  routing after revalidation, but never replay a cancelled command implicitly.
- Deleting either endpoint, or purging the hat, cancels dispatch before scrubbing
  content, removes relevant queues/notices/budgets/edges, and honors
  existing tombstones and host-forget behavior. Surviving children are detached
  and operator-controlled; deletion never implicitly deletes the family.
- Scrub deleted-target operation receipts to an opaque `gone` disposition,
  retaining only caller/key/digest deduplication while that caller exists.
  Remove these when the caller is deleted. Retrying an old spawn after its
  child was deleted must not create another child; keep no deleted title, path,
  brief or report in the deduplication tombstone.
- Generated report prompts already delivered into another session are copies
  in that session's transcript and possibly its adapter's conversation. Deleting
  the original child cannot promise to erase them. The delete preview says this
  and links surviving related sessions; purge of the hat includes all of them.
  Do not leak deleted titles/paths through surviving chips or orphaned metadata.
- Reassignment remains parked-only and stepped-up. It atomically disables every
  incident routing edge, revokes the session token and cancels queued transfers.
  Show a warning that historical copies in the old family's transcripts remain
  and the moved session retains its history. Agents in either hat cannot traverse
  the disabled edge or fetch its other endpoint. Returning to the old hat does
  not adopt the old family automatically. No bulk family move is added here.
  The optional plug-in contract can impose a stricter exposure guard: C1/C2's
  memory design refuses cross-hat reassignment after registered plug-in exposure
  and requires a clean session. Disabling a delegation edge does not bypass
  that guard; the reassignment behavior above applies only when it permits it.

Every worker rereads deletion/purge/reassignment/close state at its commit
boundary; a lease acquired before a transition is insufficient authority.
A dispatch already sent is reconciled, not erased from the history as though
it never occurred.

## 8. Operator experience and API

Keep the current day/project/session list arrangement. Add parent links on
child rows and bounded child summaries on parent rows; a family panel pages the
rest. Preserve runtime/model, host, cwd, lifecycle and activity for each child.
“Waiting on children” is a derived subtitle for an idle parent with outstanding
child work, not a new lifecycle. An open parent permission/question still takes
precedence as “Waiting on you”.

The parent timeline contains a spawn card with actual launch/config/delivery
state, report cards with source links and truncation labels, and question/answer
exchanges. Child questions stay visible and answerable to the operator while
“Parent is handling” is shown; the tag changes to an explicit fallback reason
when the operator must act. No optimistic “answered” or “completed successfully”
from an accepted MCP request.

Extend `SessionSummary` with optional `FamilySummary { parent, child_counts,
attention }`, and add an operator-only paginated
`GET /api/view/sessions/{id}/family` for actual children and delivery states.
Each summary link is owner-authorized; agents use the tools instead of this
operator route. Existing view/list SSE upserts carry family changes; reading a
child update invalidates the corresponding parent summary. Reconnect resumes
or requests resync under the existing view contract, not a full-tree refresh
on every token.

Keep `question_waits` as the factual pending count. Add explicit
`operator_attention` and `parent_handled_questions` so the waiting badge does
not silently count parent-handled questions as operator work. Show all pending
questions inside the family view. Fallback changes the operator attention
count and push eligibility together. Permission questions always count under
Q1's default. The phone lane uses this same routing result and opens the child
that needs intervention; it does not independently guess whether the parent
is alive. Push payloads retain the hat's generic-title/detail/muting policy.

## 9. Security and failures

The threat model remains umbrella §8.4: hats protect hennery-mediated flows,
not the filesystem or a shared OS user. Agents can expose live MCP tokens
through process arguments or output; the built-in token now also authorizes
bounded delegation and permitted memory access. Reuse redaction and document
this increased authority. No shell sandbox claim is made. Strong isolation
still requires separate OS users/hosts and a separated collector.

No session tool mints credentials, authorizes OAuth, changes mounts/path rules,
pairs/revokes hosts, changes hats, deletes sessions or purges data. These remain
operator routes with their existing Origin/cookie/step-up checks. MCP cannot
submit an “operator approved” string to bypass them. The same-hat checks apply
to reads and errors as well as mutations, including status cursors and receipts.

| Failure/race | Required result |
|---|---|
| Lost MCP response after accepted spawn/send | Same operation key returns the existing receipt; no duplicate child/turn. |
| Parent busy, blocked, asleep or host offline | Durable queue; no overlapping prompt; attention deadline/fallback survives restart. |
| Collector restarts with active adapters | Existing host resend/reconcile first, then queue recovery; no gratuitous parent resume. |
| Child adapter crashes or cannot start | One factual failure notice; partial transcript stays readable; no invented success report. |
| Requested model vanished or config read-back differs | Brief held; child visible with requested/actual config and a repair path. |
| Parent closes during spawn | Recheck before start dispatch; undispatched child start is cancelled. A start already sent reconciles and its child becomes operator-controlled. |
| Parent and operator answer together | One accepted answer, existing pending-id dedupe; delivered verdict is monotonic. |
| Question withdrawn before delivery | Mark obsolete; stale answer gets the existing refusal. |
| Parent token revoked during a tool call | Cut the response; no commit after revocation wins. Earlier accepted operation retains its receipt and lifecycle handling. |
| Family reassigned/deleted/purged during delivery | Transaction denies stale scope, cancels queued transfer, and scrubs references; no worker resurrects deleted content. |
| Runaway exchanges or queue full | Persisted caps, explicit paused/fallback state; no lost question or silent eviction. |
| Adapter emits activity without a normal prompt/turn-end pair | Show unsupported/unrecognized behavior; verify a host-side typed activity/turn-end solution before claiming unattended delegation works for that adapter. Never infer completion from idle text. |

## 10. Verification strategy

This is a documentation task. Implementation plans must make these tests
concrete, execute their commands, and report actual results; fake tests do not
replace live adapter checks.

1. **Identity and HTTP:** opted-in hat with no external mounts still yields the
   built-in entry; opted-out hats get none and cannot invoke it using a token
   obtained for an external mount. Disable during a call/dispatch, rollback a
   grant change, re-enable without replay, and prove external mounts still work.
   Enable after an external-only token was minted: that token stays denied.
   Disable/re-enable with an old delivered token: denied until a fresh
   start/resume delivers the current revision. Test token/grant transaction
   rollback, stale queued grant revisions and positive fresh-token admission.
   `starting` can initialize/list; invalid and standalone credentials cannot
   invoke tools; foreign Origin, cookie-only, header duplication, duplicate or
   Unicode-escaped JSON keys, BOM, invalid UTF-8 and batches have deterministic
   refusals. Check exact routing against the proxy's wildcard. Revoke before
   resolve, during a call and after acceptance; rollback must not cut a token.
   Assert no secrets in events, errors, SSE, logs or generated snapshots.
2. **Authorization:** two owners, two hats, sibling/unrelated/ancestor ids,
   forged caller fields, scope-changing cursors, guessed operation keys,
   symlink escape/root changes, reassign/purge races and runtime isolation
   fallback. Differential decode tests share the exact bytes the handler uses.
   Add positive authorized cases so a blanket deny cannot pass.
3. **Configuration:** every offered runtime, same/cross-runtime defaults,
   arbitrary catalog model ids, stale/cold catalogs, config clamp, probe timeout,
   adapter override, unavailable model, and no brief before successful read-back.
   Test the same catalog service through tools and launcher-facing API.
   After automatic parent and child resume, test reapply failure, absent/partial
   read-back with old stored values, late old-generation evidence, delegation
   disable and a config switch between validation and host admission. Cover
   enabling with an unauthenticated runtime, sign-in and new catalog values
   after enabling without re-toggling, and `auth_required` spawn refusal.
   No queued prompt runs until current-generation evidence proves eligibility.
4. **Durability:** fault injection after every acceptance/source-event/claim/
   dispatch/ack transaction boundary; collector and host restart separately;
   two workers; operator prompt versus delivery; late `turn_started`; interrupted
   work; lost spawn response; resend of the same pending answer. Prove queue
   depth and budget caps under concurrency. A restart never resets the budget.
   Put close intent between the eligibility read and commit, at either endpoint,
   and between reservation and host handoff; assert no dispatch when close wins.
   Race close with automatic resume; test closed, failed and presumed-parked
   rows cannot pass its genuinely-parked-only transactional predicate.
5. **Attention and deletion:** busy and unanswered-parent deadline, immediate
   permission fallback, notification failure/muting, question withdrawal,
   duplicate notices, close in flight, child delete with retained report copies,
   parent delete with surviving child, reassign and complete hat purge. Test
   source filters so non-applied facts cannot wake a parent.
   Crash an idle child and a child with an out-of-turn pending question; repeat
   with reconciliation loss and host revoke. Assert one coalesced loss notice,
   obsolete queued questions in the same transaction, and no duplicate wake-up
   from the subsequent park/cancellations/interrupted-turn facts.
6. **UI:** golden view fixtures for provenance, partial reports, all receipt
   states and unknown ACP; Playwright family navigation, operator takeover,
   waiting badge, SSE resync and mobile layout. Exact text or role locators.
   C1 contract tests cover hostile text and spoofed delimiters in feed items,
   child reports and startup-context entries: typed provenance survives, no
   operator authority is inferred, and no retrieved text resets a budget.
7. **Live acceptance:** on pinned Claude and Codex adapters run Claude→Claude,
   Claude→Codex, Codex→Claude and Codex→Codex. Prove the discovered tool list,
   independent model/effort choice with actual read-back before prompt, a real
   elicitation/schema answer, operator permission fallback, parent busy delivery,
   parked-parent resume, token rotation, collector reconnect, child failure and
   closed-parent fallback. Check native background/self-woken activity if the
   adapter supports it. Capture scrubbed ACP as new golden fixtures. Inspect
   process-token exposure per pin. Non-default-hat Codex remains a refusal test
   until 8h passes its own live isolation gate.

Each plan runs targeted tests during work, then the repository's full Rust,
protocol/view codegen and affected web/e2e checks at completion. New processes
and filesystem behavior run in Linux CI as well as macOS. Race/timing tests
run under load and alone for diagnosis; every security/notification/revocation
side effect has its own executed revert-probe. Live work uses scratch data and
projects; never restart the service hosting the implementation fleet.

## 11. Plans, amendments and other lanes

Sizes describe implementation plus verification, not elapsed time. Each plan
uses plan 3c's structure, builds its code, embeds it, replays it, and is reviewed
through the parent. This design does not attempt to supply executable plans.

| Order | Plan | Size | Exit condition |
|---|---|---|---|
| 1 | **D1: session-bound MCP and family identity** | M | Token/descriptor/auth seam, generation-bound delivered opt-in revision, shared session context, family storage, owner/hat checks, scope lifecycle hooks and generated types; actual Claude/Codex tool discovery. Optional plug-ins reuse this identity seam. No externally enabled spawn yet. |
| 2 | **D2: child control and launch configuration** | L | Shared catalog service/probe, all seven tools, operation receipts, bounded follow-up queue, fresh config evidence on start/automatic resume, close-intent fencing and permission enforcement. Can split catalog and tools into separate small PRs. Delegation stays preview-gated pending D3. |
| 3 | **D3: durable parent delivery and operator fallback** | L | Transactional notices including out-of-turn loss/coalescing, scheduler, provenance, wake-up budget, restart reconciliation, attention deadlines, push routing, delete/reassign races. End-to-end parent/child loop; no polling-only release. |
| 4 | **D4: families and live acceptance** | M | Family API/view fold/UI, correct attention counts and operator takeover, cross-runtime live matrix and mobile/phone integration. Enables delegation for ordinary use. |

**Explicit cross-lane dependencies:** the memory design's **C1 (generic
feed/lifecycle, M–L)** owns authenticated plug-in registration/namespace context,
activity/lease, unmount access/feed revocation, deletion receipts, the shared
provenance types (`Operator`, `Parent`, `ChildReport`) and typed `PluginContext`.
These types are defined in the memory design and are a dependency of
delegation's provenance integration. **C2 (startup, S–M)** owns optional scoped
startup delivery using C1's types. External full plug-in integration
depends on C1; startup integration additionally depends on C2. A simple MCP
mount needs neither new feed nor startup grant. D1–D4 implement no replacement
for C1/C2, and built-in delegation can ship without a memory service. Coordinate
C1's shared type definitions to land before D2/D3 consume them; using those
types does not enable a plug-in, its feed or its startup worker. Coordinate
C1 with D1's identity seam and D3's provenance/loss events; coordinate C2 with
D2/D3's prompt/config/close fences before combining startup and delegation.
The common authority is session-bound gateway delivery, not a shared endpoint
or an upstream assertion of session identity that the proxy does not provide.

Review the following spec changes before D1/D2, and land their text with the
appropriate plan rather than letting conflicting rules persist:

- Umbrella §1.2 names parent/child delegation in the approved migration scope;
  §6.3 allows explicit close by an authorized direct parent in addition to
  the operator, with parent provenance and no cascading close.
- ACP core §§4.6, 9 distinguish the operator answer path from scoped parent
  elicitation answers; §§4.4, 8 add queued acceptance and input provenance
  without changing direct operator prompt semantics or delivery acknowledgement.
- Gateway §§3.1–3.2 describe minting for an opted-in built-in entry even with no
  external mount, and the shared descriptor/auth seam. Its module independence
  remains. Kernel hat settings document opt-in and immediate disable semantics.
- Kernel §3.3 distinguishes the built-in MCP server's present-Origin check
  from the external proxy's exemption. This follows the MCP transport's
  explicit Origin validation rule; it does not retrofit unrelated browser
  or proxy routes as an unreviewed side effect.
- Client view §§3–4 and session push rules add family/provenance/attention
  types and server routing. Memory remains an optional external plug-in; its
  generic feed/startup contract requires its own reviewed spec amendments.

**Memory:** reference §3 under the exact name **session-bound MCP delivery**:
existing `SessionMcp`, rotating session token and gateway principal. Serve
memory tools externally through existing connection mounts, not on built-in
`hennery`. C1/C2 are the explicit authenticated namespace/feed/lifecycle and
startup dependencies above; consume provenance and purge hooks. Match memory's
credential-bound owner/hat context and unverified agent-supplied session hints;
signed per-session attestation is deferred to a separately reviewed extension.
C1 defines the shared provenance and `PluginContext`; delegation references
those definitions, including hostile-text/spoofed-delimiter tests. No
competing agent token scheme, cwd-derived identity, embedded memory module or
forced capture.

**Phone:** consume canonical operator-attention routing and stable child links.
Do not suppress a notification merely because `parent_id` exists, and do not
put question bodies in push outside the hat's configured policy.

**WSS:** this design is independent of host transport. The host must still be
able to reach `public_url` for its MCP HTTP requests; fixing host WebSocket TLS
alone does not prove that reachability. Reconnect preserves existing fencing
and `resend_complete` rules. No tunneling of MCP over host frames is proposed.

**8h / launch UI:** reuse the delivery-mode mapping and the D2 catalog service.
Mixed-hat Codex availability is conditional on isolation, not a reason to
silently substitute Claude or the host's default model.

## 12. Operator questions

- **Q1:** May a parent answer a child's ACP permission requests, or only its
  ordinary elicitation questions? **Recommended default:** ordinary elicitation
  only; permissions go directly to the operator, while the parent sees that
  intervention is needed. If permission delegation is wanted, define an
  explicit operator-granted policy before implementation; child prose cannot
  grant it.
- **Q2:** Should the first release allow children to spawn grandchildren?
  **Recommended default:** one generation, as in the predecessor; keep the
  schema extensible, then add nesting with a root-wide budget, depth limit and
  explicit escalation rules after the direct-parent flow is proven.

Generated with Claude AI — please review before distribution.
