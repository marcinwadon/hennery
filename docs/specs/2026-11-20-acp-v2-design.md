# ACP v2 and background activity

Status: implemented locally and verified; no service deployment or release performed.
Implementation was authorized in the conversation.

## Purpose

Sessions must accurately expose work continuing without LLM output. Add optional
ACP v2 without removing any existing user feature. ACP v1 remains supported.

## Agreed product behavior

- Protocol choices: `v1`, strict `v2`, and `v2/v1` (prefer v2, fall back to v1).
- Show the effective negotiated version and explain fallback.
- Background-only activity uses the label `Background` and a purple open ring.
- Preserve green `Running`, orange `Waiting on you`, and gray `Idle`.
- Show `Background tasks: active / done / all` beside the status, including
  while foreground work is running or user input is required.
- Active includes pending and paused work. Details expose the precise state.
- Done includes success, failure, and cancellation; details distinguish outcomes.
- Agreed Claude compatibility policy: when a terminal reports completion without
  a usable exit outcome (observed empty exitStatus), count it as succeeded for
  the user-facing summary. This is an explicit assumption, not evidence of exit
  code zero. Preserve raw evidence and do not fabricate a numeric exit code.
  Explicitly reported failure or cancellation takes precedence. Other adapters
  retain their reported outcome semantics; do not apply this fallback globally.
- Counters cover a current group, not session lifetime. First background work
  opens a group; subsequent background work joins it.
- Close the group only when all its tasks have ended AND foreground is idle.
  Remove the counter and append exactly one system entry to Hennery history
  summarizing succeeded, failed, and cancelled tasks. Do not send this entry as
  a user prompt to the agent.
- If all tasks finish while foreground remains running, retain `0 / N / N`.
- Never show `0 / 0 / 0`.
- Every unfinished background task blocks automatic idle parking, including
  pending and paused tasks. Explicit park/close behavior needs separate review.
- Replay, reconnect, and resume must not duplicate tasks or summary entries.
- Missing information does not mean zero tasks and cannot close a group.

## Architecture proposal

Keep foreground activity, background task registry, and observation freshness
separate. Derive the visible Background state from idle foreground plus active
background work. Do not replace the protocol's idle state with a fabricated
wire state. Operator attention retains priority; keep the counter visible.

Use separate v1 and v2 drivers behind a shared Hennery model. A v2-to-v1-only
translation would lose lifecycle and task information. A v2-only migration
would violate the required compatibility. These are the considered alternatives;
the shared model is a technical recommendation, not yet an approved interface.

Normalize message identity, tools, terminals, foreground state, background
snapshots, permissions, configuration, usage, plans, and replay boundaries.
Preserve raw protocol evidence for diagnostics and future schema changes.

Maintain task identities within the session/adapter lineage. Process-backed
work reported as a terminal must not also count as its invoking tool call.
Unknown background variants still represent active work. Missing/null/malformed
background snapshots mean not reported; an empty list confirms no active work.
Use final terminal/tool status as completion evidence. Inconsistent snapshots
must produce uncertainty rather than invented successful completions.

Persist group identity and a unique summary key so projection rebuilds and
resume are idempotent. Historical completed groups remain in history, not in
the live counter. Disconnect marks observations stale and does not fabricate
completion or reset. An unrecoverable adapter exit needs an explicit interrupted
or unknown outcome policy before implementation; do not classify it as success.

## Negotiation and compatibility

Fallback occurs only during initialization, before session mutation or prompt
submission. Accept valid v1 negotiation in automatic mode. Retry a fresh v1
process only for a demonstrated version incompatibility, not arbitrary auth,
spawn, or runtime failures. Never resend accepted prompts through fallback.
Strict v2 rejects v1 with an actionable error. Switching an existing live
connection's protocol is outside the initial scope.

Initial repository observations (before the dependency upgrade):

- Rust SDK is pinned to `agent-client-protocol = 5.2.0`.
- `crates/hennery-host/src/session.rs` uses v1 types and initializes V1.
- `crates/hennery-host/src/forget.rs` also initializes V1 and needs an audit.
- Adapters are pinned to codex-acp 2.7.0 and claude-agent-acp 0.107.3.
- Host turn completion currently depends on the prompt response. In v2 that
  response acknowledges insertion; state updates report foreground completion.
- `web/src/lib/status.ts` owns shared status presentation and filters.
- Session queue, steering, family, notifications, delegation, and idle parking
  all have assumptions about foreground turns that must be reviewed.

Do not claim the pinned adapters support v2 or background reporting until
verified against their actual binaries. Protocol support alone cannot expose
processes that an adapter does not report.

## Delivery stages and acceptance

1. Audit pinned SDK and both adapters: negotiation, background reporting,
   cancellation, resume, permissions, custom extensions. Record exact versions
   and traces. Determine whether upgrades or upstream changes are necessary.
2. Build a parity matrix for all existing user features: start/resume/history,
   configuration/models/modes/commands, permissions/elicitation, attachments,
   steering/queue, delegation/parent handling, usage/cost/plans/tools,
   cancellation/parking/close/reconnect. Assign a v2 implementation to each.
3. Design normalized events and storage migrations. Specify group persistence,
   stale observations, unexpected exits, and synthetic summaries. Preserve v1
   behavior and compatibility with older hosts/collectors.
4. Implement protocol preference and negotiation with fake-agent fixtures:
   v1-only, v2-only, negotiated downgrade, malformed response, auth failure,
   unsupported version. Assert no duplicate prompt or session creation.
5. Implement v2 lifecycle and registry. Test prompt acceptance before idle,
   autonomous foreground work, replay, repeated snapshots, paused tasks,
   unknown variants, deduplication, inconsistent snapshots, and reconnect.
6. Integrate status/counter/details/history plus API and streaming projections.
   Review filters and precedence with existing child/parent attention states.
7. Integrate queue/delegation/notifications/parking. Test that background work
   blocks automatic parking, but does not itself hold a completed foreground
   turn open. A pending permission belonging only to background work must not
   incorrectly redefine the entire foreground state.
8. Run parity checks for both real adapters across all three preferences.
   Release experimentally only after existing user features pass the matrix.

Core scenario: foreground ends with two active tasks; show Background and
`2 / 3 / 5`. One ends: `1 / 4 / 5`. Last ends while idle: remove counter,
show Idle, append one outcome summary. If foreground is running, retain
`0 / 5 / 5` until idle. New work then opens a fresh group.

## Further v2 opportunities

Agent-initiated foreground workflows; stable message IDs and whole-message
upserts; explicit replay completion; agent-owned terminal output; per-item
cancellation when advertised; subagent/session messages; streamed tool content;
plan updates; paused tool states. These require adapter capability verification
and are not promises of initial-release UI features.

## References

- https://github.com/agentclientprotocol/agent-client-protocol/blob/main/docs/protocol/v2/migration.mdx
- https://github.com/agentclientprotocol/agent-client-protocol/blob/main/schema/v2/schema.json
- https://github.com/agentclientprotocol/rust-sdk/blob/main/md/protocol-v2.md

ACP v2 is a draft. Pin the audited schema/SDK revision before implementation.

## Implemented compatibility boundary

The implementation uses Rust ACP SDK 5.4.0 / schema 1.19.0, Codex ACP 2.8.0
with Codex CLI 0.170.0, and Claude ACP 0.109.0 with Claude Agent SDK 0.3.328.
Existing sessions default to v1. Effective protocol and fallback reasons are
reported separately from the requested preference.

Managed Claude's current v2 adapter omits its existing steering extension.
Upstream support is proposed in
https://github.com/agentclientprotocol/claude-agent-acp/pull/1682.
Claude v2 remains gated until that change is merged, published in an adapter
release, and verified with steering and background tasks together.
Automatic mode therefore restarts with v1 before session creation; strict v2
refuses with an explicit feature-gap error. Do not remove steering silently or
claim that normal prompt queueing is equivalent to steering injection. Codex's
advertised v2 steering permits its v2 path. Claude's missing numeric exit outcome
policy remains implemented for compatible v2 sessions/adapters.

The actor conservatively prevents parking unresolved known background tasks even
if a snapshot prematurely removes them. Future unknown variants count as active
while present; their disappearance can close them with unknown outcome.

Session-wide display entities use immutable pre-update seeds per display group
and durable bounded-per-entity projections. Raw events remain unchanged for replay
deduplication. Explicit page boundaries and newest-version merging prevent older
entity origins from corrupting pagination.

Validated: Rust unit/integration suites for host lifecycle, background projections,
history, store migrations/deletion, queue and protocol; all-target workspace check;
Clippy with warnings denied; 2907 web tests; TypeScript and production web build;
local CLI build. Real adapter experiments are recorded in
../../spikes/2026-11-20-acp-v2/updated-results.md. This is not three-platform release
qualification or a claim that Hennery's complete v2 path was live-tested on all agents.
