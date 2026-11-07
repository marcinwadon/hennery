# Shared session work observations

The collector exposes one factual read model to the UI and external MCP.
Lifecycle and activity retain their existing meanings. This change does not
define a new status enum or infer completion of the operator's task.

`SessionWork`, generated from Rust in `hennery-view`, contains:

- The greatest applied session event (`revision`), lifecycle, activity, current
  turn, presumed parking, close intent and configuration error.
- Last runtime activity, last nonempty assistant text update and the latest
  applied turn outcome with its timestamp. UI acknowledgements and sidebar
  changes do not become runtime activity. A completed turn is not proof that
  the overall task is complete.
- Counts of open permission requests, ordinary questions and answers awaiting
  reconciliation. These are observations, not a claim that all work is blocked.
- The held operator queue, outstanding operator/delegation prompts and queued
  external prompts. External counts are explicitly scoped: the UI sees the
  session; an external client sees only its own external prompts. No other
  client's prompt text, operation IDs or configuration operations are exposed.
- Background coverage, currently `not_reported`. It never means an empty task
  inventory. In particular, no `all_finished` boolean is derived from ACP v1.
- Optional transcript content: latest assistant message excerpt and reported
  plan, carrying turn IDs, timestamps and explicit truncation indicators.
  Plans describe the agent's intentions; they do not prove running processes.

## Access and consistency

Operator session summaries and their existing SSE upserts include metadata
observations without transcript excerpts. `GET /api/view/sessions/{id}/work`
reads the full bounded observation. It shares operator authentication and
owner/deletion/purge guards with the view API and never acknowledges results,
unseen activity or pending input.

External `session_list` and `session_get` include the same `work` object inside
each authorized session's metadata. Existing hat/host/root/created-session
grants still select the sessions. Transcript excerpts and plans are omitted
unless the existing `read_transcript` grant permits them; metadata consent does
not implicitly become transcript consent. Collection and authorization share
the store gate. Queue count scope is explicit rather than implying that a
client-specific queue is the complete session queue.

The session-bound delegation `session_list` adds the same work observations to
each direct child, inside its existing owner/parent/hat/host transaction. It
retains the direct-child visibility restriction and cursor binding. Pages are
bounded by the requested child count and a 192 KiB JSON budget; a byte-limited
page returns a continuation instead of implying the remaining children ended.
This is a view projection over the existing child summary, not an expansion of
the delegation principal's session scope.

List-stream deduplication compares observed facts while ignoring changes only
to the work read anchor. A metadata-only event must not produce a duplicate
upsert when the displayed summary is otherwise identical.

Content uses the canonical display fold, retaining message boundaries instead
of joining text across tools or turns. Each read inspects at most 512 applied
events and 128 KiB of raw event bodies. A body larger than the input budget
ends the window without materializing that body. The result explicitly reports
omitted history, possibly partial messages and shortened plans. The message
excerpt is limited to 1024 JSON-encoded bytes; a plan contains at most four
entries with 128 JSON-encoded bytes per label. Full transcripts remain available
through the existing paged read APIs. Streaming text is an observation, not a
declaration of a final response.

Partial event indexes maintain the two activity timestamps, so list/SSE metadata
reads do not scan long transcripts. The migration builds these indexes once;
it adds no second transcript store or background polling process.

## ACP v2 direction

The upstream v2 draft provides `state_update` for foreground work, including
agent-initiated work. Its `idle` state allows background work. The separate
background inventory distinguishes an explicit empty snapshot from missing
telemetry. Hennery currently negotiates v1; implementing v2 is a separate
protocol change covering prompt acceptance, completion, cancellation, replay
and reconnect. It is not a prerequisite for this shared observation API.

The pinned Claude adapter 0.100.0 includes conditional v2 background reporting;
availability depends on its reporting configuration. The pinned
Codex adapter 2.5.0 supports v2 foreground state and has an AIR async-task
extension for background terminals; switching versions alone does not establish
equivalent background coverage. These are source findings, not live acceptance.

Sources checked 2026-11-06:

- [ACP v2 migration](https://github.com/agentclientprotocol/agent-client-protocol/blob/main/docs/protocol/v2/migration.mdx).
- [ACP v2 lifecycle](https://github.com/agentclientprotocol/agent-client-protocol/blob/main/docs/protocol/v2/prompt-lifecycle.mdx).
- [Background work change](https://github.com/agentclientprotocol/agent-client-protocol/pull/2738).
- [Claude 0.100.0 changelog](https://github.com/agentclientprotocol/claude-agent-acp/blob/v0.100.0/CHANGELOG.md).
- [Codex 2.5.0 foreground state](https://github.com/agentclientprotocol/codex-acp/blob/v2.5.0/src/AcpV2ForegroundState.ts).
- [Codex 2.5.0 background terminals](https://github.com/agentclientprotocol/codex-acp/blob/v2.5.0/src/async-tasks/CodexBackgroundTerminalTasks.ts).
