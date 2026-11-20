# ACP v2 audit — 2026-11-20

Throwaway spike; no product behavior changes. Run `python3
spikes/2026-11-20-acp-v2/probe.py` from the repository root. Requires the pinned
adapter packages already installed in Hennery's local adapter sets and Node.
The probe reads those packages, uses a fresh temporary HOME, removes credentials
from the child environment, sends only initialize, and terminates the adapter.
It never logs in, creates a session, or sends an LLM prompt.

## Observed results

| Component | Version | Result |
|---|---|---|
| Rust SDK | 5.2.0 | Local crate exposes unstable_protocol_v2; not enabled by Hennery |
| Rust schema | 1.17.0 (Cargo.lock) | StateUpdate::background(), terminal/tool background variants present |
| codex-acp | 2.7.0 | Live initialize succeeds for both requested V1 and V2 |
| claude-agent-acp | 0.107.3 | Live initialize succeeds for both requested V1 and V2 |

The probe initially closed stdin immediately after sending initialize. Codex
exited without replying. Keeping stdin open until a response fixes the harness;
this was not evidence of unsupported ACP. Negative SIGTERM exit codes in the
corrected probe are deliberate teardown after a successful response.

No authenticated model/background job execution has been tested. Results below
are static inspection of the exact installed distribution, not end-to-end proof.

## Background support: significant adapter difference

Claude ships `dist/v2/background.js` and `dist/v2/tasks.js`. V2Background stamps
state updates with background lists, tracks session reporting, and handles
terminal/tool cancellation capabilities. SessionTasks documents runtime
background_tasks_changed and task lifecycle handling. It maps background Bash
to display terminals, Monitor/Workflow work to tool calls, and otherwise creates
synthetic task_<taskId> tool calls. It also handles background subagents.

Important gate: tasks.js explicitly says sessions without perTaskStopAffordance
report no lists. Verify runtime negotiation enables this for the actual session.
The registry unit alone is not proof that a real runtime emits those events.

Codex bundles the v2 background schema, but its AcpV2ForegroundState emits
foreground snapshots without background. Searching its bundle for `background:`
finds schema definitions plus an unrelated false flag, not task-list publication.
Its async task updates render to `_async_task_state_update`. This is strong static
evidence that standard background snapshots are not implemented on this path;
confirm with a live background command before treating this as definitive.
Enabling v2 alone therefore cannot yet be promised to fix Codex background status.

## Compatibility observations from initialize

Both v2 responses retain image/embedded-context, stdio/HTTP MCP, delete, fork,
additional directories, title, and system-prompt capability declarations.
V2-required methods must not be judged unsupported merely because their v1
optional capability marker disappeared.

Codex returns `_meta.steering.supported: true` in both versions. Claude returns
it in v1 but omits it in v2. Investigate actual v2 prompt/steering behavior before
mapping this to the existing Hennery extension; this is a parity risk, not proof
that Claude cannot be steered. Claude declares status/options and promptQueueing
in v2; Codex's response does not declare status/options. MCP SSE advertised in
Claude v1 is absent in v2: verify preservation or an explicit compatible path.

Not verified: permissions/elicitation round trips, model config, queue behavior,
native children, replay, cost/usage, attachments, cancel semantics, all existing
Hennery extensions. Initialize capabilities do not establish behavioral parity.

## Recommended next spike

1. Extend a standalone driver to create isolated authenticated test sessions;
   preserve real Hennery sessions/configuration and do not copy secrets to traces.
2. For each adapter, request two finite background shell commands, one success
   and one failure. Capture announcements, idle, background snapshots, final
   statuses, and empty list in wire order. Keep pipes open after foreground idle.
3. Verify Claude's perTaskStopAffordance gate, idle list changes, individual
   cancellation, and foreground cancel leaving background work running.
4. Confirm Codex's standard-list gap. If confirmed, choose adapter/upstream work
   or an explicitly separate experimental extension bridge; do not present a
   missing list as zero or infer tasks from arbitrary transcript text.
5. Exercise steering and Claude SSE parity before enabling v2 for users.

Decision: proceed with a dual-protocol host design, but do not claim full parity
or background observability for both pinned adapters. No dependency upgrade is
needed merely to begin the host v2 experiment.

## Authenticated experiment results

`live.py` creates temporary projects and isolated runtime config directories,
using existing authentication through credential symlinks. Raw results stay in
the printed /tmp evidence directory. Model calls incur ordinary runtime usage.
The harness rejects interactive requests; it does not blanket-approve tools.

Two runs were performed. The sandboxed run could create a Codex session and
accept a prompt, but model routing failed; Claude session/new timed out.
The network-enabled run created sessions and accepted prompts on both adapters.

Claude 0.107.3, observed on the wire:

- Native Bash run_in_background=true for two commands, exit 0 and exit 7.
- State snapshots: running with [], running with one then two terminals,
  idle with two terminals, idle with one terminal, idle with [].
- Terminal announcements precede list membership; terminal exitStatus precedes
  removal. Tool calls complete before the corresponding terminals: counting
  completed launching tools as completed background work would be wrong.
- Both terminal exitStatus values were `{}`, including the exit-7 command.
  Exit codes appeared only in terminal display output. Do not parse display
  text as a reliable outcome contract. A truthful summary needs an additional
  unknown-outcome count or an adapter fix before promising success/failure totals.
- Background completion also triggered autonomous running/idle cycles without
  another user prompt. Group closure and summary must be idempotent across these.
- Repeated snapshots reordered the two items; membership must be set-based.

Codex 2.7.0:

- Accepted prompt returns messageId before foreground completion.
- Emitted terminal/tool updates and final idle with usage, without background.
- Model used shell `&` with redirection, not a retained runtime execution handle.
  This run does NOT prove what happens with a native retained background task.
  Strengthen the prompt/tool control before asserting a definitive runtime gap.
- session/status returned -32601, matching its absent optional declaration.

Remaining experiments: native retained Codex commands, individual cancellation,
foreground cancel with live background work, resume/replay, steering, permissions,
config changes, and MCP transport parity. These have NOT passed merely because
initialize and the initial background scenario succeeded.

## SDK 5.4 audit

Downloaded published agent-client-protocol 5.4.0 to /tmp and compared its manifest
and packaged changelog with cached 5.2.0. No repository dependency was changed.
5.4.0 (2026-11-18) pins schema 1.19.0 instead of 1.17.0 and derive 5.4.0.
It adds session cursors and prompt idempotency keys and fixes invalid UTF-8 input
closing a connection. 5.3 adds schema 1.18 skills, prompt suggestions and speed
category. MSRV remains 1.88.0. unstable_protocol_v2 remains an explicit feature.
Recommendation: evaluate 5.4 for the implementation baseline, especially replay
and prompt deduplication. This was a source audit, not a compile/parity test.
It cannot repair missing adapter background/outcome emissions.

### Follow-up: native Codex task and cancellation/resume

A second Codex prompt explicitly required exec_command with yield_time_ms=1000,
sleep 12, no shell ampersands or redirection, and no write_stdin polling. It
produced a native terminal, foreground idle, then terminal exitCode=0 after idle.
Neither running nor idle state carried background. This confirms missing standard
background reporting for this native retained execution path, beyond the earlier
shell-& experiment. close and resume succeeded; resume emitted an idle snapshot.

Claude terminal/cancel returned success and emitted exitStatus={} with capabilities
cleared, followed by background=[]; close and resume also succeeded. The first
cancellation run incorrectly sent session/cancel as a request, which returned
-32601 (it is a notification). The harness was corrected and the foreground
cancellation test repeated. Do not interpret that request error as an adapter bug.

The corrected notification run also completed with terminal/cancel success and
successful resume. Because the harness immediately follows foreground cancel
with terminal/cancel, it does not independently establish survival for the full
25 seconds after foreground cancellation; a dedicated survival test remains.
The harness is a smoke driver, not a complete permission/steering/MCP parity suite.
