# hennery — ACP core (subsystem spec)

- **Date:** 2026-09-26
- **Status:** Draft. Amended 2026-10-01 to match what plans A (teardown), B1
  (resume), B2a (cancel, capabilities), B2b (config) and 2 (permission and
  elicitation) built, and the decisions confirmed with them; amended again
  2026-10-01 to 3a (host pairing, revoke), 3b-ii (adapter descriptors) and
  3b-iii (`owner_id`). Where a section still describes something not built
  yet, it says so.
- **Refines:** [architecture spec](2026-09-25-hennery-architecture-design.md)
  §5 (protocol), §6 (sessions), §11 (browser API) and §13 (frontend data
  flow). This document is the authoritative home of the frame catalogue
  (§3.3) and the hello fields; the umbrella states the principles and points
  here.
- **Evidence:** [per-session MCP spike](../spikes/2026-09-25-per-session-mcp.md)
  and a behaviour catalogue of the predecessor's session machinery. Each hard
  rule below names the failure it prevents ("P-n" = predecessor incident, see
  §14).

The ACP core is everything that makes a browser-driven agent session work:
the host process and its adapters, the host↔collector protocol, the session
state machine, the collector's session storage, and the session REST/SSE API.
It does not cover the MCP gateway (own spec), operator auth, pairing and hats
([kernel spec](2026-09-26-kernel-design.md)), install and service management
([distribution spec](2026-09-26-distribution-design.md)), or the frontend's
rendering (own spec).

---

## 1. Crates and processes

The Cargo workspace (Rust, umbrella §9.1):

| Crate | Kind | Owns |
|---|---|---|
| `hennery-proto` | lib | Every control frame, session body kind, collector event kind and REST payload type. Generates JSON Schema and TypeScript. No I/O. |
| `hennery-host` | lib | The host: connection manager, outbox, session actors, adapter supervisor, ACP client, capability profiles, projects/git probes. |
| `hennery-sessions` | lib | Collector-side session module: ingest, state machine, storage, REST/SSE handlers, push triggers. Depends on `hennery-kernel` and on the `SessionMcp` trait of `hennery-gateway`. |
| `hennery-kernel` | lib | Operator auth, hosts and pairing, hats and path rules, SQLite pool and migrations, config, HTTP server scaffolding, outbound HTTP policy, push delivery, the hat purge hook. |
| `hennery-gateway` | lib | MCP gateway (own spec), including the config renderers. Depends on `hennery-kernel`, never on `hennery-sessions`. |
| `hennery` | bin | CLI (including `hennery mcp apply`, which wires the gateway's renderers), supervisor, wiring. |
| `hennery-testkit` | lib + bin (dev only) | The fake ACP adapter (§12) and shared test helpers. Never a dependency of a shipped crate. |

*Built so far:* the `hennery-gateway` crate exists (plan 8a): connections,
mounts and static credentials at rest, and their API. No `SessionMcp` trait
yet (plan 8e), so `hennery-sessions` does not depend on the gateway; the
binary merges the two routers side by side.

**Sessions → gateway interface.** `hennery-sessions` obtains a session's MCP
servers through a trait that `hennery-gateway` defines and implements:

```rust
trait SessionMcp {
    /// Mints the per-session gateway token and returns the servers to pass
    /// in `session/new` / `session/load` for this session.
    fn servers_for(&self, host_id: HostId, hat_id: HatId, session_id: SessionId)
        -> Result<Vec<McpServerSpec>>;
    /// Revokes the session's token (park, close, adapter exit, host revoke).
    fn revoke(&self, session_id: SessionId);
}
```

The dependency points sessions → gateway; the gateway never sees session
types. In `hennery gateway` (standalone) the trait is simply unused.

The frontend lives in `web/` and consumes the TypeScript generated from
`hennery-proto` plus the official ACP TypeScript SDK types.

`hennery-proto` uses `agent-client-protocol-schema` types only where hennery
itself constructs ACP data (e.g. prompt content blocks). ACP payloads inside
frames are `serde_json::Value` / `RawValue`, so unknown fields survive
untouched (§3.2); the typed schema drops unknown fields on re-serialization.

---

## 2. Host architecture

```
hennery host run
├── connection task      WS to collector: hello, auth proof, ping/deadline,
│                        reconnect with backoff+jitter, ack handling
├── outbox               on-disk (SQLite, host data dir); all session frames
│                        pass through it; drained in (session, seq) order
├── session actors       one tokio task per attached session; owns its adapter
│   └── adapter process  one OS process per session (umbrella §6.9),
│                        own process group, stdio JSON-RPC
└── probes               projects/browse, path resolution, git state, agent
                         availability
```

The host data directory layout (key, config, outbox, runtimes, lock) is
fixed in the distribution spec §8. A host holds an exclusive lock on
`host.lock`; a second `hennery host run` against the same data directory refuses
to start.

### 2.1 Lifetimes

- **Adapters belong to the host process, not to the WebSocket connection.** A
  dropped connection never touches a session actor. *(P-1: in the predecessor
  every adapter was spawned under the connection's context and torn down when
  the socket dropped, so a laptop sleeping, a collector restart or a network
  blip killed every session on the machine.)*
- A session actor lives from `start_session`/`resume_session` until close,
  park, idle reap, or adapter exit. It is the only code that talks to its
  adapter.

### 2.2 Session actor

Each actor is a single task with a mailbox. Everything that concerns one
session — frames from the collector, ACP messages from the adapter, timers —
is serialised through it. Consequences:

- **The connection task never blocks on a session.** Spawning an adapter,
  `initialize`, `session/new`, `session/load` and config switches run inside the
  actor. *(P-2: the predecessor ran start/resume/config on the connection read
  loop, so one slow `session/load` stalled permission answers and cancels for
  every other session on the host.)*
- **At most one turn is in flight per session** (§4.4).
- Per-session ordering of ACP notifications is preserved end to end: the
  adapter's stdout reader forwards notifications to the actor in arrival order,
  and the actor stamps `seq` in that order.
- **Idempotency.** The host dedupes `start_session`/`resume_session` by
  `session_id` (already attached → re-emit the current state as a
  `session_started` carrying the new `request_id`; never a second adapter),
  `prompt` by `turn_id` (already seen → no second turn; a prompt is validated
  first, so an invalid one never blocks its corrected retry), and answers by
  `pending_id` (already resolved → `answer_result{delivered: false}`).
- **One ordered emitter.** The actor's `select!` loop is the only code that
  emits the session's frames. Adapter notifications, switch answers and
  adapter requests reach it on one wire-ordered channel; the ACP connection
  runs in its own task, so a turn runs concurrently with cancel, park, close,
  exit and the reaper. Whatever is queued is drained before every
  `turn_ended`. At most 64 updates in a row (`UPDATE_BURST`) are handled
  before the other arms get a pass, so a flooding adapter cannot hold off a
  cancel.
- **An ending actor.** Once a park or close is queued, or the actor begins to
  end by itself (idle reap, adapter exit, a stopped cancel), its handle is
  marked ending. A start or resume that meets it waits for it to finish, then
  attaches a fresh adapter; two adapters never run side by side for one
  session. A command that reaches an ended actor is answered `not_attached`.
- **Host shutdown** closes the session map under the lock it takes the
  handles with: a start or resume arriving afterwards is dropped unanswered
  (the collector reconciles it as `start_not_delivered`). Shutdown waits for
  the actors (bounded by the kill grace + 1 s), so adapters get SIGTERM first,
  and emits nothing: the collector handles the rest as a host restart (§5.2).

### 2.3 Adapter supervisor

- Spawned with `setsid` / a new process group, stdin/stdout piped for JSON-RPC,
  stderr captured into a 64 KiB ring buffer.
- **Environment:** the host's environment minus variables that make an agent
  refuse to start or double-report: `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`,
  `CLAUDE_CODE_SSE_PORT` *(P-3: "cannot be launched inside another Claude Code
  session")*, plus profile-specific additions (§6), and minus the host's own
  secrets (named one by one, not by prefix).
- **Descriptors:** before `exec`, every descriptor from 3 up that is not
  close-on-exec is closed, up to the hard `RLIMIT_NOFILE` (at most 65 536), so
  an agent gets only its stdio: never the pairing pipe (kernel spec §4.2) or
  whatever the host inherited from `hennery up`, a service manager or a shell.
  On Linux 5.11 and later one `close_range(3, ~0, CLOSE_RANGE_CLOEXEC)` marks
  every descriptor from 3 up close-on-exec instead, past that cap too (plan
  7a-ii); elsewhere one numbered above the cap survives.
- **Exit watcher:** the supervisor awaits the child. On exit (any cause other
  than a requested close or park) it:
  1. fails every outstanding JSON-RPC call to that adapter;
  2. ends an in-flight turn with `turn_ended{outcome: interrupted}`;
  3. resolves every pending permission/elicitation with
     `pending_resolved{resolution: cancelled, reason: adapter_lost}`;
  4. emits `adapter_exited {code, signal, stderr_tail}`;
  5. emits `session_parked{reason: adapter_exited}`.

  *(P-4: the predecessor never watched the child. After a mid-turn crash the
  prompt call blocked until the socket died, the session showed `running`
  forever and the reaper could not free it.)*
  The exit watcher first drains what the adapter wrote before dying. Step 2
  comes before step 3, and step 3 before step 4: the order is turn end,
  then questions, then the exit, then the detach (§4.8).
- **Close, park and reap kill the whole process group** (SIGTERM, then SIGKILL
  after 5 s). *(P-5: killing only the direct child left the agent CLI subtree
  alive; accumulated trees once exhausted a host's memory.)* After an
  unexpected exit, the rest of the group is SIGKILLed right after the leader
  is reaped: descendants of a crashed adapter are orphans nothing else reaps.
- **The group dies with the host.** Its leader is a guard, a `/bin/sh` that
  the adapter joins and that reads a pipe whose writing end only the host
  holds. However the host dies (SIGKILL, OOM, a panic), the guard reads
  end-of-file and SIGKILLs its own group: the adapter and everything still
  in its group. It ignores SIGTERM, so a host killed during a kill grace
  still leaves no group behind. *(Smoke test #1, F3: after a host SIGKILL, Claude's CLI outlived
  its adapter's `node` by about 7 s and could still spend tokens.)*
- **Scrubbing.** Stderr tails and `host_note` text are scrubbed of token-like
  patterns (`Bearer …`, `sk-…`, `ghp_…`, `github_pat_…`, `xox[abp]-…` and
  similar) before they are emitted. Once the stderr ring has truncated, its
  partial first line is dropped before scrubbing, so a token cut at the
  ring's edge cannot slip past the patterns. ACP payloads are **not** scrubbed: they
  stay verbatim (§3.2), which means tool output that contains a secret is
  stored as-is in the collector. The documentation says so.

### 2.4 JSON-RPC client

- Requests to the adapter carry a host-local id; a table maps ids to waiters.
- **Inbound notifications are handed to the actor synchronously and in order.**
  Inbound requests from the adapter (permission, elicitation, fs, terminal) are
  dispatched without blocking the reader, since a permission request can wait
  hours for an answer that arrives through the same actor.
- On EOF the client fails all waiters (the exit watcher's step 1).
- **Crate usage (checked against `agent-client-protocol` 2.2.0 / schema 1.9.1
  and the adapters' bundled TypeScript SDK 1.5.0, both protocol version 1):**
  - Use the crate's **connection engine** (id routing, cancellation, batching)
    and its typed `send_request` for the host's own calls: `initialize`,
    `session/new`, `session/load`, `session/set_config_option`,
    `session/cancel`.
  - Register **`UntypedMessage`** handlers for `session/update` and for every
    adapter→client request that is forwarded to the browser, and forward
    `params` verbatim. Deserialize a *copy* into schema types only to fill the
    extracts (§3.2); a parse failure means "no extracts", never "drop".
    A typed notification handler that fails to parse is logged and dropped by
    the crate, so a new update kind from an adapter bump would silently
    vanish — the predecessor's lost-plan failure (umbrella §5.2).
  - **Spawn the adapter with `tokio::process`** (process group, composed
    environment, stderr capture, exit watcher) and connect it through the
    crate's byte-stream transport. The crate's own spawner uses a second async
    runtime and does not expose what §2.3 needs.
  - Methods absent from the Rust schema (e.g. Codex's legacy
    `session/set_model`) are sent as `UntypedMessage`. Not built yet: it
    belongs to the Codex adapter profile (§6).
  - **One untyped request handler answers every adapter request itself**
    (§2.5). Falling through to the crate is not enough: its default handler
    holds any message that names a session for a per-session handler hennery
    never registers, so the adapter would wait forever.
  - The crate's `Responder` sends nothing when dropped, so every question the
    host holds is answered explicitly (answer, cancellation or withdrawal,
    §4.6). A peer's `$/cancel_request` reaches the responder's cancellation
    handle, which the host watches.
  - Switch answers (`session/set_config_option`) arrive on the actor's
    ordered channel with the notifications, so a read-back never overtakes,
    or is overtaken by, the agent's own `config_option_update`.

### 2.5 ACP client-side capabilities

The host implements, for adapters that ask:

| Method | Behaviour |
|---|---|
| `fs/read_text_file` | Whole file, or `limit` lines from 1-based `line`. Line endings preserved. |
| `fs/write_text_file` | Implemented (mkdir -p, atomic write) but only **advertised** when the adapter profile says so (§6). |
| `terminal/create` | No `args` → `sh -c <command>`; with `args` → direct exec. Env appended to the host env. Own process group. Output ring buffer honouring `outputByteLimit` (default 1 MiB) with a `truncated` flag. A spawn failure is recorded as exit −1 with the reason in the output, never an empty success. *(P-6: a missing shell looked like "tools silently never run".)* |
| `terminal/output`, `wait_for_exit`, `kill`, `release` | Standard. `kill`/`release` signal the terminal's process group. |
| `session/request_permission` | §4.6. |
| `elicitation/create` | §4.6. |
| anything else | JSON-RPC `-32601 Method not found`, answered at once by hennery's own handler (§2.4). |

**Built so far:** only the two question methods are served; `fs/*` and
`terminal/*` are answered `-32601` until they are implemented (never left
hanging).

`initialize` advertises, besides the profile's rows (§6):

- `session.configOptions.boolean = {}`. An agent offers boolean options only to
  a client that advertises this; to any other it offers an `on`/`off` select
  instead (Claude turns `fast` into one).
- `elicitation = {"form": {}}`, never a boolean (§4.6, P-19). Only form
  elicitation is advertised. An adapter that sends another mode anyway still
  reaches the operator (the payload is opaque) and can be accepted, declined
  or cancelled.

---

## 3. Host ↔ collector protocol

Transport, versioning policy, schema source and liveness are fixed in the
umbrella §5. This section is the authoritative catalogue and the rules the
umbrella leaves to it.

### 3.1 Frames

Every frame is a JSON object with a `type` discriminator. The protocol version
appears only in `hello` / `hello_ack`, never per frame.

- **Requests** (collector → host) carry a `request_id`, used for correlation
  only.
- **Session frames** (host → collector) have the single type `session`:

  ```jsonc
  { "type": "session", "session_id": "…", "seq": 42,
    "body": { "kind": "acp_update", /* kind-specific fields */ } }
  ```

  Every session frame goes through the outbox and carries a per-session
  monotonic `seq` (§3.6).
- **Correlated responses** (host → collector, not outboxed, no `seq`) exist
  only for rejections (`error`) and connection-scoped probes.

**State-bearing facts always travel as session frames**, never only as a
correlated response. A request that changes session state is completed by the
outboxed fact that carries its `request_id` (or names its turn or session);
the collector resolves the waiting HTTP call when it **ingests** that fact.
*(If the answer to "did my prompt start?" travels only on the connection, a
socket drop at the wrong moment leaves the collector guessing; on the outbox it
is resent until acknowledged.)*

**Wire order.** The host sends every outboxed fact it has before each
correlated reply, so a rejection never overtakes the fact that preceded it
(e.g. a `turn_ended` written just before a `not_running`, §3.3).

Maximum frame size **32 MiB**. A frame that would exceed it is rejected at the
sender with a visible error; it never closes the socket. *(P-7: an oversized
update closed the predecessor's socket, which tore down every session on the
host.)*

### 3.2 Session bodies and extracts

| `body.kind` | Fields | Notes |
|---|---|---|
| `acp_update` | `indexed`, `payload` | One ACP `session/update`, verbatim. `indexed.turn_id` names the turn in flight when it arrived. |
| `session_started` | `request_id`, `agent_session_id`, `indexed` (catalogue extracts) | Catalogue is the **post-switch** one (§4.3). Also completes a retried start or resume (§2.2). |
| `start_failed` | `request_id`, `code`, `message` | Any failure after the host accepted a start/resume (spawn, `initialize`, `session/new`/`load`, the 75 s start bound). `code` is the stored failure reason (§4.3). |
| `turn_started` | `request_id`, `turn_id` | The prompt reached the adapter. |
| `turn_ended` | `turn_id`, `outcome`, `stop_reason?`, `error?` | Exactly one per started turn (§4.4). |
| `pending_opened` | `pending_id`, `indexed`, `payload` | ACP request verbatim in `payload`. Its kind rides in `indexed.pending.kind`: the body's own `kind` is the body tag. `indexed.turn_id` names the turn it was asked in, if any. |
| `pending_resolved` | `pending_id`, `resolution` (`delivered` \| `cancelled`), `reason?` | `reason` only with `cancelled` (§4.6). |
| `answer_result` | `pending_id`, `request_id`, `delivered` | Umbrella §6.8. A delivered answer is followed by `pending_resolved{delivered}`. |
| `config_applied` | `request_id`, `indexed` (catalogue extracts) | Authoritative read-back after `set_config`. Also emitted, under the failed request's id, for a timed-out switch that answers late (§4.3). |
| `session_parked` | `reason` (`idle` \| `adapter_exited` \| `operator`) | `operator` also covers an adapter stopped for ignoring a cancel (§4.4). |
| `session_closed` | — | |
| `transcript_gap` | `from_seq`, `to_seq` | Its own `seq` is `to_seq` (§5.5). Not built yet. |
| `adapter_exited` | `code?`, `signal?`, `stderr_tail` | Stderr tail scrubbed (§2.3). |
| `git_state` | `branch`, `dirty`, `worktree`, `head`, `base_commit?` | After start and after each turn; bounded to 3 s. Not built yet. |
| `host_note` | `note`, `text` | hennery's own diagnostics that are not state. `note` is a machine code, `text` is scrubbed. Codes so far: `replay_unknown_dropped` (§4.5), `config_failed` / `reapply_failed` (§4.3), `cancel_unanswered` (§4.4). |

- `payload` is the ACP message exactly as received from the adapter, including
  unknown fields and `_meta`. The collector stores it as opaque JSON and
  **never parses it**.
- `indexed` holds **typed extracts** the host fills because only the host
  understands ACP. The allowed keys are a closed set in `hennery-proto`:

  | Extract | Meaning |
  |---|---|
  | `activity` | `running` \| `idle` |
  | `title` | Session title reported by the agent |
  | `turn_id` | Turn the update or question belongs to (filled on every update) |
  | `pending` | `{id, kind, option_ids?}` (`option_ids` for permissions, read from the raw request, §4.6) |
  | `commands` | Available slash commands (full list) |
  | `config_options` | Config catalogue: the adapter's ACP option objects that hennery can parse (the crate skips one it cannot read) |
  | `current_model`, `current_mode` | Current values of the model and mode options |
  | `current_axes` | Current value of every other option, by config id |
  | `plan` | Plan entries (latest snapshot) |
  | `agent_failure` | `{severity}` |
  | `usage` | Context/token usage |
  | `text_projection` | Plain text of message chunks, for future search |

- `session_catalog`, `plans` and the `model`/`mode`/`config_axes` columns
  (§8) are filled **from extracts only**. The collector validates answers
  against the stored `option_ids`, never against the payload.
- **The catalogue extracts are one snapshot.** When `config_options` is
  present and not empty, it and `current_model`, `current_mode` and
  `current_axes` describe the same moment. An absent or empty
  `config_options` means "no read-back" (an answer that failed to parse
  arrives empty), never "the adapter has no config": the collector ignores
  it and keeps what it stored. The host fills them on `session_started`,
  `config_applied` and live `config_option_update`s only, never on updates
  replayed by `session/load`, sent before the `session/new`/`load` answer, or
  sent while the start's switches ran (those are older than the catalogue
  `session_started` announces, P-13).
- **Built so far:** `turn_id`, `pending` and the four catalogue extracts. The
  others arrive with the plans that use them.

### 3.3 Frame catalogue

**Collector → host requests** (all carry `request_id`):

| Type | Key fields | Completed by |
|---|---|---|
| `start_session` | session_id (collector-minted), committed_seq, agent, cwd (canonical), model?, mode?, axes{}, first_prompt?{turn_id, content[]}, mcp_servers[], hat | `session_started` \| `start_failed` \| `error{unknown_agent}` |
| `resume_session` | session_id, committed_seq, agent, cwd, agent_session_id, model?, mode?, axes{}, mcp_servers[], hat | `session_started` \| `start_failed` \| `error` (as a start) |
| `prompt` | session_id, turn_id, content[] (ACP ContentBlocks) | `turn_started` \| `error{turn_in_progress \| not_attached \| invalid \| images_unsupported}` |
| `cancel_turn` | session_id, turn_id | That turn's `turn_ended`, **whatever its outcome** \| `error{not_running \| not_attached}` |
| `park_session` | session_id | `session_parked{reason: operator}` \| `error{not_attached}` (only to hosts with the `park` capability) |
| `close_session` | session_id | `session_closed` \| `error{not_attached}` |
| `set_config` | session_id, config_id, value (a select's value id or a boolean) | `config_applied` \| `error{unknown_option \| invalid \| config_failed \| not_attached}` |
| `answer_permission` | session_id, pending_id, option_id | `answer_result` \| `error{not_attached}` |
| `answer_elicitation` | session_id, pending_id, action, content? (only with `accept`) | `answer_result` \| `error{not_attached}` |
| `list_projects` | — (roots come from the host's config, §7) | `projects{items[], partial}` |
| `browse_directory` | path | `directory{entries[]}` \| `error` |
| `resolve_path` | path | `resolved_path{canonical, exists, is_dir}` \| `error` (kernel spec §5.4) |
| `probe_agents` | — | `agents{…}` (same shape as in `hello`) |

**Collector → host, not requests:**

| Type | Key fields | Notes |
|---|---|---|
| `hello_ack` | protocol_version, collector_version, server_time, committed{session_id: seq} | Reply to `hello`; `committed` holds the collector's highest committed seq for every session listed in `attached_sessions` (§5.1). |
| `hello_error` | code (`incompatible` \| `revoked` \| `already_connected` \| `bad_proof`), message | Then the socket closes. `incompatible`: another protocol major; `bad_proof`: the credential does not verify. |
| `ack` | session_id, ack_seq | Highest seq committed for that session (§3.6). |
| `forget_hat` | hat_id | Sent after each handshake for recently purged hats; the host deletes that hat's composed agent home once no process of the hat runs. Idempotent (kernel spec §5.5). |

**Host → collector:**

| Type | Key fields | Notes |
|---|---|---|
| `hello` | protocol_version, host_version, host_id, proof, capabilities[], agents[], workspace_roots[], attached_sessions[{session_id, last_seq, turn_id?}] | First frame. |
| `resend_complete` | — | All unacknowledged outbox frames have been resent (§5.1). |
| `session` | session_id, seq, body | Outboxed (§3.2). |
| `error` | request_id, code, message | Rejection of a request. |
| `projects`, `directory`, `resolved_path`, `agents` | request_id, … | Probe responses. |

**Request details and error codes:**

- `start_session` / `resume_session` carry `model?, mode?, axes{}` flat on the
  frame (one `SessionConfig`, flattened). `resume_session` carries the
  adapter's own `agent_session_id`, from the stored `session_started`: the
  host keeps no copy across a restart, and `session/load` needs it. A resume
  re-sends the **stored** config, which is what the agent last reported
  (§4.3, §8), not what was once asked for.
- `cancel_turn` is completed by its turn's `turn_ended`, matched by session and
  turn, not by a fact carrying the request id. A turn that finished before the
  cancel reached the agent answers with its real outcome (`completed`,
  `failed`), not `cancelled`. `not_running`: the host has no such turn in
  flight; its end, if any, is already in the outbox ahead of the rejection
  (wire order, §3.1). A repeated cancel for a turn already being cancelled
  changes nothing.
- `set_config`: `unknown_option` — the option is not in the actor's
  catalogue; `invalid` — the value is of the wrong kind for it;
  `config_failed` — the adapter refused it, did not answer within 15 s, or an
  earlier switch is still out (§4.3).
- `prompt`: `invalid` — the content is not ACP ContentBlocks;
  `images_unsupported` — it has an image block, and the session's agent
  offered no `promptCapabilities.image` in `initialize` (plan 6a). Both are
  refused before `turn_started`, so the turn never starts.
- `start_session`: `unknown_agent` — the agent is not configured on the host.
- `start_session`, `resume_session`: `cwd_not_canonical` — the cwd does not
  resolve on the host to itself as a directory (a symlink swapped in since the
  collector resolved it); nothing is spawned (plan 5c).
- Answers to a session with no live actor are refused `not_attached`. The
  collector logs that refusal; it is no verdict (§4.6).
- A command queued behind an ending actor is answered `not_attached` at once,
  never left to the collector's timeout (§2.2).

**Not on the wire yet** (they arrive with their subsystems): `first_prompt`,
`mcp_servers[]` and `hat` on start/resume (gateway, hats); `hello_ack.server_time`;
`hello.agents[]` and `workspace_roots[]`; the projects probes and their
responses; `forget_hat`. `resolve_path` / `resolved_path` are on the wire
since plan 5b: probes, answered only by the connection they went out on.

`hello` fields:

- `capabilities`: `projects` (project enumeration and browsing), `images`
  (image content blocks in prompts), `park` (explicit park), `resolve_path`
  (resolving typed paths, kernel spec §5.4). The collector
  never sends a frame, or a prompt containing images, to a host that lacks the
  capability, as the host's current connection announces it (a host that
  reconnects on an older build between the check and the send is the one
  gap, recorded by plan 6a); the UI hides the feature for that host. **Deserialized
  leniently:** an entry this build does not know (a newer host) is skipped,
  never a reason to refuse the `hello`; an absent field means none. The
  generated JSON Schema still lists the known values as a closed set, but that
  describes them, it does not constrain: a schema-validating client or proxy
  must not reject a `hello` on an unknown capability either. Gating uses the
  capabilities of the live connection (in the hub), so a host that reconnects
  on an older build loses them at once; the host registry also records the
  latest accepted `hello`'s list for display (`HostItem.capabilities`, kernel
  spec §8). The hennery host announces `park` and `images`: `images` says the
  host carries image blocks, and each session still refuses them when its
  agent takes none (`images_unsupported`, above).
- `agents[]`: per agent `{id, version, available, auth, catalog}` where
  `catalog` is the profile's **static default catalogue** (§6), so the
  New-session pickers work before the first session on a host exists.
- `workspace_roots[]`: from the host's config (§7).

Unknown frame types and unknown body kinds in either direction are logged
(rate-limited) and ignored; a frame of a known type that fails validation is
answered with `error{code: invalid}` if it had a `request_id`. Adding a type
without a handler does not compile (umbrella §5.4). *(P-8: the predecessor
dropped unknown frames silently and once shipped a result type with no
collector handler; its tests passed on a timeout.)* **Not built yet:** both
ends log and skip a frame that fails to parse, a known type that fails
validation included; the `error{invalid}` answer for it is still to come.
A host frame for a session that host does not own is logged and skipped.

### 3.4 Timeouts and disconnects

| Request | Collector timeout |
|---|---|
| `start_session`, `resume_session` | 90 s (adapter spawn + `session/load` of a large session) |
| `prompt` (acceptance only) | 60 s |
| `set_config`, `cancel_turn`, `park_session`, `close_session` | 60 s |
| probes | 15 s |

- Every timeout for a state-changing request is **at least the WebSocket read
  deadline** (45 s, umbrella §5.8), so while the connection is alive the fact
  or a rejection arrives first. The collector's build asserts it.
- **The host bounds its own side below these:** a start or resume at 75 s
  (then `start_failed`); each config switch at 15 s (`CONFIG_TIMEOUT`); a
  cancel the adapter ignores at 20 s (`CANCEL_GRACE`) plus the 5 s kill grace.
  So on a live connection the host's answer always beats the collector's
  timeout. The collector waits 10 s for a new connection's `hello`.
- **When the host connection drops**, the collector immediately fails every
  in-flight HTTP waiter for that host with "host disconnected; delivery
  unknown" (503 `delivery_unknown`, §9) and leaves the affected start or turn
  **awaiting reconciliation** — never failed. Reconciliation happens after
  the host's resend and `hello.attached_sessions` (§5.1). Waiters belong to a
  connection: a drop fails only the waiters of that connection, never those
  of a newer one of the same host.
- **A timeout on a live connection drops that connection.** It is reported
  the same way ("delivery unknown"), and because reconciliation runs only at a
  handshake, forcing a reconnect is what makes the timed-out start or turn
  reconcile at all. The other sessions on that host see a reconnect; their
  adapters are unaffected (§2.1).
- **The hub owns every waiter's deadline.** Registering a waiter arms a
  watchdog: at the deadline, if the waiter is still there for the same
  connection, the watchdog removes it, answers "delivery unknown" and drops
  that connection. So a request whose HTTP handler is gone (the client
  disconnected) still cannot leave a session `starting` or a turn `sent`
  forever.
- **A rejection is applied by the socket task.** A start, resume or prompt
  registers an undo with its waiter (start/resume: `failed` with the host's
  code, only while still `starting`; prompt: the turn is removed). When the
  host's `error` arrives, the socket task takes the waiter, applies its undo,
  then answers it, so the store is right even when no handler waits, and HTTP
  and store agree. If the undo itself fails, the answer is "delivery unknown"
  and the connection is dropped.
- **A host that is connected but not yet reconciled** is treated as offline:
  requests to it are refused, never queued (§5.1 step 4, §9). Answers
  excepted: they are queued durably and sent after reconciliation (§4.6).
- A `starting` session found in SQLite after a **collector restart** is
  reconciled the same way when its host next connects.
- Every rejection, including a failed resume, is correlated by `request_id`
  alone and returns immediately. *(P-9: the predecessor's resume errors carried
  a session id that its waiter did not match, so every failed resume cost the
  full 30 s timeout.)*

Answers have no waiter: they are queued durably (§4.6).

### 3.5 Host authentication

`hello.proof` is the host's Ed25519 signature (lowercase hex), with the key
generated at pairing, over `hennery hello proof v1` followed by the collector's
nonce, the host id and the protocol version, each prefixed by its length (u32,
big endian), so no two triples share bytes. The nonce is 32 random bytes per
upgrade; the collector sends it, lowercase hex, in the `hennery-hello-nonce`
upgrade response header before the first frame, and a host that gets none
sends no `hello`. Keys are 32-byte Ed25519 in lowercase hex; small-order keys
are refused and verification is strict. The connection is unauthenticated
until a valid `hello` arrives. The host id is never self-asserted without the
proof. A revoked host gets `hello_error{revoked}` and then stops all its
adapters; until it connects, its adapters keep running (it cannot be reached).
Pairing itself is in the kernel spec §4.

- **Order of refusals:** the `hello` must arrive within 10 s. A different
  protocol major is answered `incompatible` before the proof is looked at. An
  unknown host id gets `bad_proof`, exactly like a wrong signature. The
  signature is verified before revocation, so only the key's holder is told
  `revoked`; `already_connected` is said only after a valid proof, and
  revocation is re-checked once the connection is registered.
- **On the host,** `revoked` stops every adapter, as a shutdown does, and ends
  `host run` (exit 78); every other refusal keeps the reconnect backoff. A
  `bad_proof` warning names the remedy: remove `host.key` and `host.toml`, then
  `hennery host join`.
- *Limit:* the proof does not name the collector. A relay on the host's path
  that forwards the upgrade and its nonce to the real collector gets a valid
  `hello` through. TLS with the collector's certificate checked (`wss://`)
  closes this; the proof alone does not.

**One live connection per host.** A second connection for a `host_id` that is
already connected is rejected with `hello_error{already_connected}`; the
collector never supersedes a connection silently. It closes the older
connection only if that one has missed its liveness deadline.

*Rejected:* mTLS. It breaks behind TLS-terminating reverse proxies, which are
one of the three supported topologies (umbrella §7.5).

### 3.6 Sequence numbers and acks

- The host stamps every session frame with a per-session monotonic `seq` when
  it enters the outbox; `seq` counters are persisted with the outbox.
- **Idempotent ingest on `(session_id, seq)`.** A duplicate with the same
  body is acknowledged and discarded. A duplicate with a **different** body is
  stored as a `conflict` event `{seq, received}` (§8) and acknowledged; it is
  never silently dropped. A re-sent conflicting frame whose `received` body is
  already on record adds no second `conflict`.
- **Bodies are compared structurally** (stored vs received JSON), not by a
  stored hash: key order is not stable across builds (`serde_json`'s
  `preserve_order` is feature-unified), and a Rust-std hash is not stable
  across releases.
- **An ingest error drops the host connection without acking.** Acks are
  cumulative, so acking a later frame would tell the host an uncommitted one
  is safe to delete.
- **Ack = the highest seq the collector has committed for that session** (not
  necessarily contiguous). The host deletes outbox rows with `seq ≤ ack_seq`.
- The collector groups ingest commits (at most every 50 ms) and acks after the
  commit. *(Built so far: one commit per frame, acked after it.)*
- `seq` exists only for host → collector delivery; the browser's cursor is the
  collector's `event_id` (§9).

---

## 4. Sessions

### 4.1 Identity and immutables

- The collector mints `session_id` (UUIDv7) before the host is asked to start
  anything; the adapter's own id is stored as `agent_session_id`.
- **Immutable after creation:** host, agent, cwd (canonical). The hat is
  changed only by explicit re-assignment (§4.9). Nothing derives them from
  events. *(P-10: an event with an empty machine field once blanked a
  session's host and made it permanently unresumable.)*

### 4.2 State machine

Lifecycle × activity as in the umbrella §6.3. Transitions are applied on
ingest in `seq` order (host facts) or when the collector writes its own event
(§8); every transition writes an events row first.

| From | Trigger | To |
|---|---|---|
| — | `POST /api/sessions` (after hat resolution, §4.3) | `starting` |
| `starting` | `session_started` | `active/idle` (or `active/running` once a first prompt's `turn_started` arrives); clears any `failure_reason` |
| `starting` | `start_failed` | `failed` (reason stored) |
| `starting` | host rejects the start/resume | `failed` (the host's code) |
| `starting` | reconciliation finds no trace of the start | `failed` (`start_not_delivered`) |
| `starting` | host revoked (kernel spec §4.3) | `failed` (`host_revoked`), with a `start_not_delivered` event |
| `failed` | a late `session_started` | `active` (the real fact wins, e.g. over a `start_not_delivered` guess) |
| `failed{start_not_delivered}` | a late `start_failed` | `failed` (the host's code replaces the guess) |
| `parked`, `closed`, `failed` | resume requested (atomic) | `starting` |
| `active/idle` | `turn_started` | `active/running` |
| `active/running` | `pending_opened` | `active/blocked` |
| `active/blocked` | last pending resolved | `active/running` |
| `active/running`, `active/blocked` | `turn_ended` | `active/idle` |
| `active/*` | `session_parked` | `parked` (`closed` if a close was requested) |
| `active/*` | host offline > threshold | `parked` (presumed; §5.3) |
| `active/*` | host revoked (kernel spec §4.3) | `parked` (presumed; `closed` if a close was requested; §5.3) |
| `active/*` | host restarted (§5.2) | `parked` (`closed` if a close was requested) |
| `active/*` (attached) | `session_closed` after operator close | `closed` |
| `active/*` (attached) | the host answers a close `not_attached` | `closed` (collector-side) |
| `parked`, `failed`, unattached, presumed parked | operator close | `closed` (immediately) |
| `starting`, host connected and reconciled | operator close | refused, 409 `starting` |

A question asked **outside a turn** leaves `activity` alone: `blocked` means
a running turn waits on the operator. `turn_ended` makes the session `idle`
whatever questions remain open.

**A presumed-parked session counts as attached for its host's facts.** It
keeps its open turn and activity; resent or late facts from its host (a turn
end, a reap, a question) apply to it as to an `active` one. It refuses
prompts (409 `not_attached`) and closes at once.

**One visibility rule for host facts** (`events.applied`, §8):

- a fact **with** a transition is listed iff its guarded update changed a
  row;
- a fact **without** one (`acp_update`, `adapter_exited`, `host_note`) is
  listed unless the session is `closed`; an `acp_update` that names a turn is
  listed only while that turn is `started`;
- `pending_opened` applies only to an attached session and, if it names a
  turn, a `started` one; `pending_resolved` only to an open question;
  `config_applied` only while attached; `answer_result` only when it changes
  the verdict (§4.6).

An unlisted fact keeps its idempotency key and counts towards
`committed_seq`, and the waiter it completes still resolves from its body: a
re-emitted `session_started` still completes a retried start or resume.

**Open turns are released on detach.** When `session_parked` or
`session_closed` detaches an active (or presumed-parked) session, and again
when a resume begins, a turn still open is released: a `started` turn gets
`turn_ended_synthesized{interrupted}`, any other `turn_not_delivered`. The
host always ends its own turns before it detaches, so an open turn at that
point is one it never acknowledged. Likewise an unattached close first
resolves an open turn, and the questions left open go with the session
(`pending_cancelled`, §4.6).

`starting` blocks a second resume: the collector answers 409 to a resume or
prompt while the session is `starting`. The host also refuses to attach a
session it already has attached and re-emits the current state instead (§2.2).
*(P-11: two concurrent resumes both passed the predecessor's "not live" check
and orphaned an adapter process.)*

**Prompt or config on a session that is not attached** (parked, closed,
failed, or its host offline) → 409 `not_attached`; the UI offers resume. A
prompt to an `active` session whose host has just gone, or is not yet
reconciled, gets 409 `host_offline` (§9).

### 4.3 Start and resume

**Hat resolution comes first.** `POST /api/sessions` → `resolve_path` on the
host (canonical cwd) → path-rule match (kernel spec §5.2) → the hat is stored
with the canonical cwd → `start_session`. A host that is offline cannot start
a session, and no session is created: with no resolution there is no hat
(409 `host_offline`, plan 5c). The deciding rule is stored too
(`sessions.hat_rule_id`, audit only). A cwd that is not a directory on the host
is 400 `invalid_cwd`. A rule that would cover the canonical cwd but for case,
would win, and names another hat makes the hat doubtful: 409 `hat_ambiguous`,
naming the rule's prefix, until the host's rules are saved again (plan 5c;
verified rules count, since the case on disk can change after a rule is
saved).

**One request starts a session.** `start_session` carries agent, cwd, model,
mode, other config axes, the MCP servers for the session (from
`SessionMcp::servers_for`, §1) and an optional first prompt
`{turn_id, content[]}`. The host:

1. spawns the adapter with the agent profile (§6);
2. `initialize`;
3. `session/new {cwd, mcpServers, _meta}` (§6 for `_meta`);
4. applies **model first, then other axes, then mode**, each via
   `session/set_config_option`, keeping the catalogue returned by the last
   successful switch;
5. emits `session_started` with that catalogue;
6. if a first prompt was supplied, runs it as turn 1 (`turn_started` carries
   its `turn_id`).

*(P-12: the predecessor's browser issued five sequential calls with the
ordering rules living in the UI. A model switch can clamp the mode, so mode goes
last.)*

**Resume** is the same except that the hat is **re-resolved** first, step 3 is
`session/load {sessionId, cwd, mcpServers, _meta}` with replay suppression
(§4.5), and step 4 re-applies the stored model/axes/mode. If the re-resolved
hat differs from the stored one, the resume is refused (409 `hat_mismatch`)
with a message naming both hats; the operator must re-assign explicitly
(§4.9). Before that, the stored cwd is resolved again: a canonical form other
than the stored one (a symlink swapped in, or a session from before plan 5c
whose cwd was stored as typed) is 409 `cwd_moved`, and the near miss is
refused as at a start (`hat_ambiguous`). A session that got no hat (`''`,
stored before hats) always meets `hat_mismatch` until re-assigned. Every resume mints a fresh gateway token, superseding the previous
one. The catalogue announced is the post-switch one. *(P-13: announcing the
pre-switch catalogue made the collector overwrite a stored mode with the
adapter default, and the next resume applied the default for real.)*

A failed re-apply is logged on the timeline as a `host_note` and does not fail
the resume.

**Built so far:** hat resolution, `mcp_servers`, `_meta` and the first prompt
are not built yet; start and resume carry agent, cwd and the config.

**Config axes.** Axes are ACP config options, and model and mode are two of
them. The model is the option in category `model`; without one, the option
whose id is `model`, but only if it has no category or a custom (`_`-prefixed)
one. The same goes for `mode`. `axes` holds every other option by config id,
each value a select's value id or a boolean.

**Switches** (step 4, the same on start and resume):

- The model, then each axis in config-id order, then the mode. A value
  already current is not sent — decided only from a catalogue that is current
  and came from an adapter answer, never a stale or seeded one.
- A requested model or mode with no option to match is a failed switch, never
  dropped silently.
- Each switch gets 15 s, and none runs past the start's 75 s bound. Once a
  switch gets no answer in time, or the bound has passed, **no further switch
  is sent**: the adapter handles requests concurrently, so a late model switch
  could clamp a mode sent after it. The rest are listed as `not sent: an
  earlier switch did not answer` (or `not sent: the start deadline passed`). A
  switch the deadline cut off reads `no answer before the start deadline`; an
  option the adapter does not offer, `the adapter offers no … option`.
- After the switches, each requested value is compared with the final
  read-back; a mismatch is a failure too (`effort: asked high, agent reports
  low`).
- Every failure lands in **one** `host_note` after `session_started`: code
  `config_failed` on a start, `reapply_failed` on a resume, text `could not
  apply: …` / `could not re-apply: …` with the lines joined by `; `.
- **A switch that does not take never fails a start or a resume** (decision,
  2026-09-30; the fresh-start case was open here). The picker's value may be
  stale (another pin, another host); the note plus the real current values
  tell the operator what to change, and failing would throw away a started
  adapter for a setting fixable in one click.
- An empty or missing read-back is no catalogue: until the adapter reports
  its options again, the host announces none, and the stored values survive.
  If the `session/new`/`load` answer has no `configOptions`, the pre-switch
  catalogue is seeded from the last non-empty `config_option_update` the
  adapter sent before it (that update itself carries no extracts).
- A start switch that timed out becomes an **orphan**: `set_config` is refused
  (`config_failed`, "an earlier switch is still out") until its grace (four
  switch timeouts after it was sent) passes.

**Live `set_config`** on an attached session is one more switch:

- One `session/set_config_option` at a time per session. Requests queue on
  the host and are answered in the order they came; they may run during a
  turn. Each one's deadline is its receipt plus 15 s: one still queued then is
  `config_failed` ("an earlier switch is still out"), one sent and unanswered
  `config_failed` ("no answer within …"). A timed-out switch is an orphan
  (above): what is queued behind it is refused, nothing is sent behind it.
- An orphan's late non-empty read-back is emitted as a sequenced
  `config_applied` under its request's id: the agent now runs with it, and
  without the fact a later resume would revert it. It answers no one.
- A switch still out when the actor ends is answered at once, after the
  actor's last fact: `config_failed` if the adapter's exit failed the request
  first, else `not_attached`. Either way the collector never waits out its
  timeout.
- The actor also tracks the catalogue from the agent's own
  `config_option_update`s (e.g. leaving plan mode); those carry the extracts
  (§3.2).
- Not built yet: Codex's legacy `session/set_model` and `modes` /
  `session/set_mode` (Codex profile, §6).

**Questions during start-up.** Permissions and elicitations the adapter asks
before `session_started` (during `initialize`, `session/new`/`load` or the
switches) are held, then opened right after it, in wire order with the
start's early updates. An adapter that blocks its load on such a question
costs the start its 75 s bound; the `start_failed` then names the questions
it held back.

**Resume, collector side.** A session with no `agent_session_id` (its start
never produced one) cannot be resumed: 409 `agent_has_no_record` without
contacting the host. The host being offline is checked before any
transition (409 `host_offline`, nothing changes). The resume transition is
atomic, so of two concurrent resumes one gets 409 `starting`. A failed resume
leaves the session `failed` like a failed start, and `failed` is resumable.

**Known load failures** (reported as `start_failed`):

- `-32002 Resource not found` means the agent has no record of the session,
  typically because it never completed a turn. The session becomes `failed`
  with reason `agent_has_no_record`, and the UI offers "start a new session in
  the same project". *(P-14.)* Mapped so on `session/load` only.
- `-32000 Authentication required` means the agent CLI on that host is not
  logged in. Reason `agent_not_logged_in`, on any start call (`session/new`
  included); `hennery doctor` names the fix.
- An adapter without the `loadSession` capability: `load_unsupported`.
- Anything else (spawn, `initialize`, an exit during start, the 75 s bound):
  `start_failed`.

### 4.4 Turns

- The collector mints `turn_id` for each prompt and records the turn as `sent`
  together with its content. **The user-turn event (and its attachments'
  references) is written only when `turn_started` is ingested**, so the
  timeline never shows a prompt the agent did not receive.
- A turn left `sent` by a disconnect is reconciled after the host's resend: if
  no `turn_started` arrived, the turn becomes `not_delivered` and the UI
  offers to send its content again. It is never shown as failed.
- **One turn at a time.** A prompt while a turn is in flight is rejected with
  409 `turn_in_progress`; the UI disables Send while `running`/`blocked`.
  *(P-15: the predecessor allowed overlapping prompts; the first to finish
  cleared the "running" flag while the second was in flight, and the reaper
  could then reap mid-turn.)*
- **Empty prompts are rejected at the API** (400): no image, and no text
  but whitespace.
  *(P-16: an empty prompt reached the adapter and produced a `-32602 Invalid
  params` error on the session.)*
- **Every started turn ends with exactly one `turn_ended`**, outcome one of:
  - `completed` — the adapter returned a stop reason other than `cancelled`
    (stored as `stop_reason`), even after a cancel: the agent finished first;
  - `cancelled` — the stop reason is `cancelled` (with or without a hennery
    cancel), or `session/prompt` returned an error after a cancel was sent
    (the error is kept: an agent's aborted work may throw), or the adapter
    ignored the cancel (below);
  - `failed` — `session/prompt` returned an error with no cancel sent (stored);
  - `interrupted` — adapter exit, session close or park mid-turn, or a host
    restart.

  The host emits it in every case it can observe, including adapter exit and
  close mid-turn. The collector synthesises `turn_ended{interrupted}`
  (`turn_ended_synthesized`) only for a `started` turn the host can no longer
  end: a host restart (§5.2), a started turn the host no longer reports after
  a full resend (§5.1), and the releases of §4.2 (detach, unattached close,
  resume). A turn that never started gets `turn_not_delivered` instead: the
  agent never saw it. A `turn_ended` for a turn that has already
  ended is stored but not applied and never pushes. *(P-17: an error, a lost
  result or a disconnect left the predecessor's sessions `running` forever.)*
- Turn-in-flight covers `blocked`: a pending permission is inside a turn.
- **A late `turn_started` wins over `not_delivered`**: the adapter really has
  the turn. It takes the slot back only from a turn still `sent` (which then
  becomes `turn_not_delivered`), and never reopens a turn that has ended. A
  real `turn_ended` ends only a `started` turn.
- **Cancel** (`cancel_turn`, `POST …/cancel`). The host sends ACP
  `session/cancel` for the turn in flight and answers every open question
  cancelled (§4.6). A cancel for a `sent` turn is sent too: its prompt
  precedes it on the same socket, so the actor sees the turn first. A cancel
  writes no collector event; the turn's `turn_ended` is the timeline's
  record.
  - **An adapter that ignores the cancel is stopped** after 20 s
    (`CANCEL_GRACE`): the turn ends `cancelled` with an error saying so, the
    remaining questions are cancelled, the process group is killed, a
    `host_note{cancel_unanswered}` carries the last lines of the scrubbed
    stderr tail (no `adapter_exited` is emitted on this path), and
    `session_parked{operator}` detaches the session. It cannot take another
    prompt while the old one runs, so it cannot stay attached; 20 s + the 5 s
    kill grace stays below the collector's 60 s cancel timeout, which would
    otherwise drop the whole host connection (§3.4).
  - **Refusals:** 409 `not_attached` (not `active`, presumed parked included,
    or its host not ready), 409 `no_open_turn`, 409 `not_running` (the host has
    no such turn in flight and the store has not seen it end), 404 for an
    unknown session. If the store shows the turn ended (its end was ingested
    between reading the open turn and registering the cancel's waiter), the
    cancel answers 202 with the stored outcome instead of `not_running`.

### 4.5 Replay suppression

While a `session/load` call is outstanding, the actor drops these update kinds
from the adapter instead of emitting them:

- `user_message_chunk`, `agent_message_chunk`, `agent_thought_chunk`
- `tool_call`, `tool_call_update`
- `plan`

and passes these through, because they describe the adapter's current state
rather than history:

- `available_commands_update`, `config_option_update`, `current_mode_update`,
  `usage_update`, `session_info_update`

Unknown update kinds received during load are dropped and counted in a
`host_note` so a new history-bearing kind cannot silently duplicate history.

After a load the frames go: `session_started`, then the kept state updates in
arrival order (without catalogue extracts, §3.2), then at most one
`host_note{replay_unknown_dropped}` naming each unknown kind with its count.

*(P-18: without suppression every resume re-persisted the whole transcript
and a new "session started" row; ~50 sessions were affected before a fix and a
database cleanup. The predecessor's fix, in the collector, also dropped the
state kinds, which lost the adapter's corrective config update and forced the
post-switch-catalogue rule of §4.3.)*

### 4.6 Permission and elicitation

- The host answers the adapter only when the operator answers, the turn is
  cancelled, the session closes or parks, or the adapter is lost. **No
  timeout.** *(Umbrella §6.5. The predecessor had a 5-minute permission
  timeout and none for elicitation; hennery removes the asymmetry. Cost: an
  unanswered question pins one adapter process. The idle reaper never parks
  a session with a question open, in a turn or not, §4.7.)*
- Questions reach the actor on its ordered inbound channel, so a question is
  opened in wire order with the updates around it. One asked outside any turn
  is opened too (no `turn_id`); it leaves `activity` alone (§4.2).
- The host assigns each request a **`pending_id`** (a UUIDv7: globally unique,
  random beyond its time prefix), emits `pending_opened{pending_id, indexed,
  payload}` with `indexed.pending = {id, kind, option_ids}`, and keeps the
  waiter. The name `request_id` is reserved for request/response correlation.
- **A permission's `option_ids` are read from the raw request**: every string
  `options[i].optionId`, skipping an entry without one. A typed copy (§2.4)
  would fail on one option of a kind this build does not know, and then every
  answer would be refused. When `options` is missing or not an array, or no
  entry has an id, the question still opens but the collector accepts no
  answer to it (400 `invalid`, "stop, park or close the session").
- **Cancellation, host side.** Every open question is answered cancelled (a
  permission gets the `cancelled` outcome, an elicitation the `cancel`
  action) and announced as `pending_resolved{cancelled, reason}`:
  - on the first `session/cancel` of a turn: `turn_cancelled` (ACP asks this
    of a client after `session/cancel`). This cancels **every** open question,
    including one asked outside the turn. A question the same turn asks after
    the cancel is cancelled as it opens;
  - on park and idle reap: `session_parked`; on close: `session_closed`; on
    adapter exit: `adapter_lost` (§4.8 for the order);
  - **the adapter withdrawing its own question** (`$/cancel_request`):
    `agent_withdrew`. The host answers the request with the standard
    `-32800` cancellation error. Without this the card would stay answerable,
    an answer would report `delivered: true` to nobody, and the open question
    would keep the reaper away for good.

  Host shutdown emits nothing: the collector cancels those questions
  `host_restarted` after the next handshake (§5.2).
- **Cancellation, collector side** (`pending_cancelled{pending_id, reason}`,
  one event per question), for questions the host will never resolve: a host
  restart, including one found through a presumed-parked session
  (`host_restarted`); a host revoke (`host_revoked`, kernel spec §4.3); a close
  of an unattached session (`session_closed`); and,
  as a backstop, a host `session_parked`/`session_closed` that applies while a
  question is still open (`adapter_exited` gives `adapter_lost`, idle or
  operator `session_parked`, a close `session_closed`). A presumed park for a
  host offline leaves questions `open` (§5.3).
- **The elicitation client capability is advertised as `{"form": {}}`**, never
  a boolean. *(P-19: a boolean is silently discarded by the adapter's schema
  validator and looks exactly like not advertising the capability; the agent
  then drops its "ask the user" tool.)*
- **Answers.** The collector validates `action ∈ {accept, decline, cancel}`
  and the option id against the stored `option_ids`, writes an
  `answer_submitted` event and stores the answer in a **durable answer queue**
  keyed by `pending_id`, then returns 202 `{pending_id, request_id}`. It
  accepts an answer to any `open` question of the session, whatever the
  lifecycle and whether the host is connected. Content is allowed only with
  `accept` and must be an object; it is not checked against the requested
  schema (that is the adapter's call). The check and the insert are one
  transaction, and the queue's key is the pending id, so two concurrent
  answers queue exactly one.
- **Sending.** The endpoint sends the answer at once to a host that is
  connected and reconciled. After every reconciliation (and only after the
  host is marked ready) the queue sends every answer with no verdict whose
  question is still open, oldest first; so an answer lost with a connection
  goes again after the next handshake, and one racing a reconciliation is
  sent by one path or both, never by neither. The host dedupes by
  `pending_id`.
- **On the host** an answer for a question the actor holds is delivered:
  `answer_result{delivered: true}`, then `pending_resolved{delivered}`. Any
  other answer (already answered, cancelled, asked of an earlier adapter, of
  another kind) changes nothing and is `answer_result{delivered: false}`. An
  answer the adapter connection can no longer take is `delivered: false`,
  then `pending_resolved{cancelled, adapter_lost}`. An answer for a session
  with no live actor is refused `error{not_attached}`; the host emits no
  outboxed `answer_result` for it (after an outbox loss its seq could collide
  with a committed one, since `hello_ack` fast-forwards only attached
  sessions).
- An answer for a pending that is not `open`, or that already has a queued
  answer, → 409 (`not_open` / `already_answered`). Unknown pending → 404; an
  option the question does not offer, the wrong kind of answer, or content on
  anything but `accept` → 400 `invalid`.
- **The pending set is canonical in the collector** (`pending` table). The
  frontend drives actionability from it, not from timeline position. States:
  `open → delivered | cancelled(reason)`, reasons `turn_cancelled`,
  `session_closed`, `session_parked`, `adapter_lost`, `host_restarted`,
  `agent_withdrew`, `host_revoked`.
- **The verdict** (`answer_queue.delivered`) is NULL until one comes, and
  comes only from `answer_result`, folded monotonically (`delivered` sticks
  and a later `delivered: false` never overwrites it, umbrella §6.8), or from
  the question's cancellation, which gives `false` (nobody will take it). **A
  host's refusal of the answer's request is logged and is no verdict**: the
  answer goes again after the next handshake while its question is open. An
  `answer_result` applies only when it changes the verdict (NULL → any,
  `false` → `true`); a question can therefore end `cancelled` with
  `delivered: true`, and the frontend must render that.
- Facts that come too late are stored, not applied: a `pending_opened` for a
  detached session or an ended turn, a `pending_resolved` for a question that
  is not open.

### 4.7 Idle reaper

Host-side, default 30 minutes, configurable (`--idle-timeout-secs`), off with
`0`. Reaps only sessions with no turn in flight, **no question open** (in a
turn or not: a question has no timeout), no config switch out and no orphaned
switch pending, and no activity for the window. Its clock restarts at session
start, at each turn end, on a `set_config`, on an answer, when an orphaned
switch clears and when the adapter withdraws a question; other adapter
notifications outside a turn do not count as activity. Emits
`session_parked{reason: idle}` and kills the process group.

### 4.8 Park and close

**Teardown order** on park, close, idle reap and adapter exit: whatever the
adapter already sent is forwarded, then the turn's `turn_ended`, then the
`pending_resolved` of every open question, then (on an exit only)
`adapter_exited`, then `session_parked` / `session_closed`. The process group
is killed before the final fact.

- **Park** (`POST …/park`, `park_session`): the collector writes
  `operator_parked` and sends `park_session`, **only to a host that announced
  the `park` capability** (otherwise 409 `park_unsupported`, nothing sent).
  The host ends any turn (`interrupted`), cancels pending requests
  (`session_parked`), kills the process group and emits
  `session_parked{reason: operator}`.
- **Close, attached session:** the collector writes `operator_closed`, records
  a **durable close intent** and sends `close_session`; the host ends any turn
  (`interrupted`), cancels pending requests (`session_closed`), kills the
  process group and emits `session_closed`. The lifecycle becomes `closed`
  when that fact is ingested. A `session_parked` that overtakes the close
  (an idle reap, an exit) makes the session `closed` too. A close the host
  answers `not_attached`, or whose host went away, is closed collector-side.
  A close whose delivery is unknown stays requested and is re-sent after the
  next handshake.
- **Close, `starting` session:** 409 `starting` while its host is connected
  and reconciled (close it once the start settles); if the host is not
  connected, it is closed immediately.
- **Close, unattached session** (parked, presumed parked, failed, host
  offline): closed immediately collector-side, resolving an open turn and
  cancelling open questions first (§4.2). On the host's next `hello`, any
  attached session that the collector has closed — or whose host or hat was
  revoked or re-assigned — receives `close_session`. A `not_attached` answer
  to that close closes the session only while it is still what
  reconciliation asked to close (a resume that began since is left alone).
- **Start or resume behind an ending actor:** waits for it to finish, then
  attaches a fresh adapter (§2.2). The reachable case is reconciliation's
  `close_session` followed at once by an operator's resume.
- The session's gateway token is revoked on host-reported `session_parked`
  and `session_closed`, on adapter exit, on a close of an unattached session
  and on host revoke (`SessionMcp::revoke`). A presumed park
  (`presumed_parked{host_offline}`) does **not** revoke it: the host may still
  be running the session, which would then return with a dead token and no
  resume to mint a new one.

### 4.9 Hat re-assignment

Allowed for any session with no running adapter (`parked`, `closed` or `failed`), with a warning in the UI that
the agent's stored history came from the old hat; the change writes a
`hat_reassigned{from, to}` event. The next resume re-resolves the path and must
agree with the new hat, or the operator must also change the path rules.

`PATCH /api/sessions/{id} {hat_id}` needs step-up (kernel spec §3.4), checked
before anything is read. A presumed-parked session is refused (409
`presumed_parked`): its host is away and may still run it, with the old hat's
MCP servers, so it is closed first. `starting` and `active` are 409 with their
lifecycle. The hat must be one of the owner's (400 `invalid`); the same hat is
a no-op. One transaction re-checks the lifecycle, moves the session, clears
`hat_rule_id` (that rule no longer decided its hat) and writes the event
(plan 5d).

### 4.10 Delete

`DELETE /api/sessions/{id}` (step-up required, kernel spec §3.4, checked
before anything is read) closes an attached session first, then deletes its
events, turns, pending rows, answer queue entries and catalogue; attachment
files no longer referenced by any turn or event, of any owner, are removed. A
`session_deleted` tombstone event (no content) remains so the list stream can
emit `session_removed` with an `event_id`. Per-hat purge (kernel spec §5.5)
deletes every session of the hat the same way. As built (plan 9a):

- **Closing first** is `POST …/close`'s rule: `starting` on a reachable host
  is 409 `starting`; `active` on a reachable host gets `close_session` and the
  delete waits for it (503 `delivery_unknown`: nothing is deleted, the close
  stays requested); anything else is closed collector-side as the route read
  it, a compare-and-set in the delete's own transaction (409 with the
  lifecycle as its code if it moved). 204.
- **The tombstone** is the session's row, kept with its id, owner, host, hat
  and times, and scrubbed: `cwd` and `agent` empty, the title, git fields,
  model, mode, axes, agent session id, failure reason, rule, open turn and
  activity cleared, `lifecycle = 'deleted'`. Tombstones are kept. Triggers
  refuse any write for one, and no accessor or list returns one (404).
- **Images:** a reference is a `turn_attachments` row (a turn holds its
  images with or without its `user_turn` event) or an `event_attachments`
  row. The owner's unreferenced `attachments` rows go in the transaction; each
  file goes after commit unless another owner's row names its hash (files are
  shared by hash). A prompt re-sending an image re-writes a file a delete
  removed meanwhile.
- **Its project recent** goes too, unless another kept session of that host
  and hat has the same cwd.
- **On its host** (§5.1): frames for a tombstone are acked and discarded;
  reconciliation sends `close_session` for a listed tombstone, and so does a
  delete that lands while its host is reconnecting. Its `session_closed` and
  a `not_attached` answer change nothing.
- **Nothing is left in the database files:** `secure_delete` is on, and the
  WAL is checkpointed after a delete. A reader that holds the WAL (only one
  outside hennery can, for long) makes the checkpoint owed: it is recorded
  beside the database and retried, and at the next start, until it completes;
  the delete itself does not wait for it. Backups taken earlier keep the data,
  and the browser may keep a deleted image in its cache until logout clears it.
- **The agent's own transcript on its host** (its session files) is removed
  too, best effort, by the adapter's own call if it has one, or else only that
  session's files inside the agent's known session directory, never through a
  symlink; what could not be removed is reported and retried when the host
  reconnects (operator delegated, parent decided 2026-10-02). Not built yet:
  plan 9d.

---

## 5. Reconnect and recovery

### 5.1 Handshake

1. The host sends `hello` as its first frame, with the proof and the list of
   attached sessions.
2. The collector verifies the proof and replies `hello_ack`, whose
   `committed` map carries its highest committed seq for every session in
   `attached_sessions`.
3. The host deletes outbox rows at or below those seqs, resends every other
   unacknowledged row in `(session, seq)` order (rows of sessions no longer
   attached included; duplicates are discarded by idempotent ingest), and
   sends `resend_complete`.
   - If `hello_ack` reports a higher seq for a session than the host's own
     counter (the outbox was lost or reset), the host **fast-forwards** its
     counter past it, so new frames are never mistaken for duplicates.
     `start_session` and `resume_session` carry the same `committed_seq` for
     their session, so a session that was not attached at handshake time (an
     old session resumed after the outbox was lost) is fast-forwarded too: the
     host continues from the larger of its own counter and `committed_seq`.
4. **Only after `resend_complete`** the collector reconciles:
   - sessions it believes `active` on that host but not in
     `attached_sessions` are parked as described in §5.2;
   - sessions `presumed` parked but listed return to `active` (`reattached`);
     presumed but not listed are a host restart (§5.2);
   - starts and turns left awaiting reconciliation (§3.4) are resolved: a
     start with neither `session_started` nor `start_failed` ingested becomes
     `failed{start_not_delivered}`; an open turn the host does not report as
     its `open_turn_id` becomes `turn_not_delivered` if it never started, or
     `turn_ended_synthesized{interrupted}` if it did (reachable with a lost
     outbox, or a `turn_started` emitted between the `hello` snapshot and the
     resend; leaving it open would wedge the session at 409);
   - attached sessions the collector has closed, or deleted (§4.10),
     receive `close_session` (§4.8). A session is re-assigned only once closed or truly parked (§4.9),
     so a re-assigned session the host still has attached is one the collector
     closed;
   - the host is marked ready; then the answer queue for that host is drained
     (§4.6).

   Reconciling earlier would park sessions whose facts were still in flight.
   **Nothing is sent to a host before its reconciliation**: a request made
   between a reconnect's `hello` and its `resend_complete` is refused (409
   `host_offline` for a start, resume or prompt; `not_attached` for a cancel,
   config or park), never sent, or reconciliation would mistake it for one lost
   on the previous connection. `GET /api/hosts` shows a host `connected` only
   once it is reconciled (kernel spec §8). A host revoked meanwhile is closed
   at `resend_complete`, never reconciled or marked ready (kernel spec §4.3).
   A repeated `resend_complete` or `hello` on one connection is logged and
   ignored.

   `hello.attached_sessions` carries each attached session's `open_turn_id`;
   actors that have ended are pruned from it. The host resets its reconnect
   backoff (500 ms – 30 s) only after the first `ack` of a connection, or
   after a healthy period (60 s); a `hello_ack` alone does not prove the
   collector can commit. Connecting is bounded (10 s).

### 5.2 Host restart

The host starts with no attached sessions. Its outbox still holds unacked
frames from before the restart, which it resends first (§5.1). After
`resend_complete` the collector compares `attached_sessions` with its own
view of that host and, for sessions it believed `active` but not listed,
writes a `host_restarted` event that:

- parks the session (closes it, if a close was requested);
- if it had an open turn, synthesises `turn_ended{interrupted}`
  (`turn_ended_synthesized` event) for a started one, or `turn_not_delivered`
  for one that never started (the agent never saw it);
- cancels its pending requests with `host_restarted` (`pending_cancelled`).

Resume is explicit (operator or UI action). **No eager re-spawn on
reconnect.** *(P-20: the predecessor re-spawned every recently active session
on each reconnect, so a flapping connection churned adapters, and each
re-spawn risked the replay duplication of P-18.)*

### 5.3 Host offline

When a host's connection is gone for longer than the offline threshold
(default 10 minutes) the collector writes `presumed_parked` for its `active`
sessions: lifecycle `parked`, `presumed: true`, and a visible "host offline"
note. Pending requests stay `open` — the host may still hold them. After the
next handshake, listed sessions return to `active` (`reattached` event) with
their pending requests intact.

A **revoke** presumes the host's `active` sessions parked the same way, with
reason `host_revoked`. The host never comes back, so the revoke also ends the
open turn (`interrupted`, or `turn_not_delivered` if it never started) and
cancels open questions `host_revoked`; a queued answer's verdict becomes
`delivered: false`. Sessions already converged are skipped; none returns
`reattached`, and a resume answers 409 `host_offline`.

- The timer is armed per dropped connection and fires only if the host has
  registered no connection since; it checks and writes under the hub's lock,
  so a reconnect's reconciliation always sees the presumption, and an older
  drop never presumes a host that came back.
- A collector start arms it for every host with `active` sessions (§5.4).
- A presumed-parked session keeps its open turn and activity and counts as
  attached for its host's resent facts (§4.2). Nothing is revoked.
- `hennery collector --host-offline-secs` sets the threshold (default 600).

### 5.4 Collector restart

Indistinguishable from 5.1 from the host's side. The collector rebuilds its
view from SQLite; sessions stay in whatever state was last committed until
the host's handshake completes. `starting` sessions are reconciled then
(§3.4). A host that never returns is presumed offline after the threshold
(§5.3).

### 5.5 Outbox

- SQLite file in the host data dir: `(session_id, seq, frame, created_at)`.
- Written before sending; deleted on `ack` (`seq ≤ ack_seq`). Drained per
  session in `seq` order.
- **Coalescing.** Before enqueuing, the host merges consecutive
  `agent_message_chunk` / `agent_thought_chunk` updates of the same kind and
  message for up to ~100 ms into one `acp_update` whose text is the
  concatenation of theirs. This cuts write load for fast token streams; the
  merged update is what the adapter would have sent as one chunk.
- **Bounded** (default 64 MiB). On overflow the host **never drops
  state-bearing frames** (every body kind except `acp_update`). It drops, oldest
  first, `acp_update` frames carrying message or thought chunks, then
  `acp_update` frames carrying large tool output. Each contiguous dropped range
  is replaced by a `transcript_gap{from_seq, to_seq}` frame whose own `seq` is
  the last dropped seq, so acks keep advancing. If only state-bearing frames
  remain, the outbox grows past the bound and the host reports it (Hosts view,
  `doctor`); loss is allowed only if it is visible.

**Built so far:** the outbox without coalescing, bounds or `transcript_gap`.

---

## 6. Adapter profiles

A profile is data compiled into the host, selected by `agent`.

**Built so far:** no profiles; an agent is a command line from the host's
config, and every agent gets the same `initialize` (fs and terminal not
advertised, boolean config options and form elicitation advertised, §2.5).


| | `claude` | `codex` | `generic` |
|---|---|---|---|
| Launch | managed Node + pinned `claude-agent-acp` | managed Node + pinned `codex-acp` | command from config |
| `fs.readTextFile` | yes | yes | yes |
| `fs.writeTextFile` | **no** *(P-21: advertising it disables the adapter's native write tools without enabling a replacement; the model then has no write tool and invents edits)* | no (writes in-process) | yes |
| `terminal` | yes | yes | yes |
| `elicitation` | `{"form": {}}` | `{"form": {}}` | `{"form": {}}` |
| Extra capabilities | — | `_meta.jetbrains.air = {version: 1, capabilities: ["sessionFailure"]}` inside `clientCapabilities` | — |
| Per-session MCP isolation | `_meta.claudeCode.options.extraArgs["strict-mcp-config"] = ""` on **every** `session/new` and `session/load` | composed `CODEX_HOME` (below); on a mixed host, fallback until measured | none; fallback |
| MCP transport | `http` | `http` (no SSE) | from `initialize` |
| Default catalogue | static, shipped with the profile | static | from config or empty |
| Extra env | `MAX_THINKING_TOKENS` when set | — | from config |

**Fallback** (umbrella §8.5): on a mixed host, a session of that agent in a
non-default hat gets no gateway MCP servers, and sessions in the default hat
get only default-hat mounts; the UI says so at session start and on the mount
grid.

**Composed `CODEX_HOME`** (spike): the host builds
`<host-data>/codex-home/<hat>/` from an **allowlist** of shared entries,
symlinked to the user's `CODEX_HOME`: `auth.json`, `AGENTS.md`, `skills/`, and
`sessions/` (so a session can be resumed from the terminal). `config.toml` is
written as the user's file with all `mcp_servers` tables removed (parsed with
a real TOML parser, including inline and dotted forms). **Every other entry is
private to that hat's composed home.** The home is composed once per hat under
a lock, and never while a Codex process of that hat is running; entries are
never moved back into the user's directory.

Whether several Codex processes can safely share the symlinked state
concurrently is **unmeasured**. Until a live gate measures it, Codex on a
mixed host uses the fallback; on a single-hat host Codex runs with the
composed home and full mounts.

**Adapter overrides.** Any override (a custom adapter command,
`CLAUDE_CODE_EXECUTABLE`, `CODEX_PATH`) drops that agent to the fallback,
visibly, unless the operator explicitly accepts unverified isolation in the
host's config.

**Server naming:** hennery-injected MCP servers are named `hennery-<slug>` so they
cannot collide with the user's own server names (the spike showed Codex
silently drops a session server whose name exists in config).

**What isolation does not cover.** Strict mode and the composed home isolate
**MCP servers only**. Every Claude session still loads the user's
`~/.claude` configuration — `CLAUDE.md` and its imports, auto-memory, hooks,
skills and plugins (whether plugin MCP servers load under strict mode is
unmeasured). Codex loads the global `AGENTS.md` and any `notify` command.
These are accidental cross-hat channels outside hennery's control in v1
(umbrella §8.4); v1 documents them instead of offering a per-hat switch
(§15, decision 2).

**Agent availability** in `hello.agents[]` and `probe_agents`:
`available` = the adapter can be launched; `auth` = `ok | missing | unknown`.
Auth is taken, in order, from the adapter's `_auth/status_update` notification
sent right after `initialize` (an underscore-prefixed extension both pinned
adapters emit: `kind: "account"` or `kind: "none"`), then from the bundled CLI
(`claude auth status`, `codex login status`; exit 0 = logged in), then
`unknown`. Account details in those payloads (email, organisation) are never
forwarded; only the boolean and the method. A `-32000` on first use still maps
to `agent_not_logged_in`.

**The adapters bundle their own agent CLI** (a platform-specific native
package resolved from the adapter's `node_modules`); they do not use the
`claude`/`codex` on the user's PATH, only the user's login state
(`~/.claude`, the macOS keychain, `~/.codex/auth.json`). The adapter pin
therefore decides the CLI version.

---

## 7. Content: images, commands, projects, git

- **Prompt content** is an array of ACP ContentBlocks built by the frontend
  (text and image blocks, in order). The collector validates: image MIME in
  {png, jpeg, gif, webp}, ≤ 5 MiB decoded each, ≤ 20 images, **≤ 16 MiB
  decoded in total** per prompt. It accepts only text and image blocks, and
  an image only if its bytes begin like the type it claims (plan 6a). It
  keeps of each block only its text, or its `mimeType` and `data`: that is
  what the host is sent, so a `uri`, `annotations` or `_meta` never reach
  the agent.
- **Images are stored** in the collector as content-addressed files in the
  data directory (`attachments/<sha256>`, written to a temporary file,
  synced and renamed, 0600 in a 0700 directory), before the turn opens, and
  referenced from the turn and the user-turn event as `{type: "image",
  mimeType, sha256, size}`, so the transcript can show them later
  (retention follows the session). Neither holds the image's bytes, so no
  replay carries them. *(P-22: the predecessor never stored sent images;
  transcripts kept orphaned "[Image #N]" markers.)*
- **Slash commands** arrive as `available_commands_update` (passed through,
  with a `commands` extract); the collector keeps the latest list per session
  and serves it from the catalogue endpoint, never in the session list.
- **Projects:** workspace roots are configured **on the host** (`host.toml`
  in the host data dir, or `--workspace-root` flags) and reported in `hello`.
  `list_projects` enumerates git repositories under them (dot-dirs skipped,
  missing roots tolerated, 500-entry cap per root). `browse_directory` accepts
  only absolute paths that, **after symlink resolution**, lie under a
  workspace root or the user's home. The collector caches enumerations for
  60 s and filters recents by hat.
  - Symlinks are resolved before matching everywhere (browse fence, hat path
    rules) because the agents themselves resolve them: the spike showed Claude
    keys project config by resolved path.
- **Git state** is reported after start and after every turn, bounded to 3 s;
  `base_commit` is recorded once per session.

---

## 8. Collector storage

SQLite, WAL, one writer task (*built so far:* `IMMEDIATE` transactions on the
store's own connection, kernel spec §1). Every table carries `owner_id`.

```sql
sessions(
  id TEXT PK, owner_id, host_id, hat_id, hat_rule_id NULL, source_kind, agent, cwd,
  agent_session_id, title, lifecycle, activity, presumed_parked BOOL, close_requested BOOL,
  failure_reason, model, mode, config_axes JSON,
  git_branch, git_dirty, git_worktree, base_commit,
  open_turn_id, created_at, last_event_at, last_event_id)
session_catalog(session_id PK, owner_id, config_options JSON, commands JSON, usage JSON, updated_at)
host_agent_catalog(host_id, agent, config_options JSON, updated_at, PK(host_id, agent))
events(
  event_id INTEGER PK AUTOINCREMENT,   -- global SSE cursor
  session_id, owner_id, host_seq NULL, kind, body JSON, ts,
  applied BOOL,                        -- 0: stored host fact that did not apply
  UNIQUE(session_id, host_seq))
attachments(owner_id, sha256, mime, size, created_at, PK(owner_id, sha256))   -- file: <data>/attachments/<sha256>
event_attachments(event_id, sha256, position, owner_id, PK(event_id, position),
  FK(owner_id, sha256) -> attachments)   -- position: the block's index in the user_turn's content
turn_attachments(turn_id, sha256, position, owner_id, PK(turn_id, position),
  FK(turn_id) -> turns ON DELETE CASCADE, FK(owner_id, sha256) -> attachments)   -- plan 9a
pending(pending_id PK, session_id, owner_id, kind, turn_id NULL, option_ids JSON, payload JSON, state, reason, opened_at, resolved_at)
answer_queue(pending_id PK, session_id, owner_id, request_id UNIQUE, answer JSON, submitted_at, delivered BOOL NULL)
turns(turn_id PK, session_id, owner_id, request_id, state, content JSON, sent_at, started_at, ended_at, outcome, stop_reason, error)
plans(session_id PK, entries JSON, updated_at)
```

**Built so far** (migrations 1–8, applied in order, never edited once
shipped):

1. `sessions`, `turns`, `events` — the walking skeleton;
2. `sessions.close_requested` (the durable close intent) and `turns.state`
   (`sent → started → ended | not_delivered`) — teardown;
3. `events.applied` — teardown;
4. `sessions.presumed_parked` — resume;
5. `sessions.model`, `mode`, `config_axes` and `session_catalog` with
   `config_options` only — config;
6. `pending` (with `turn_id`) and `answer_queue` — permission and elicitation;
7. `owner_id` on `sessions`, `turns`, `events`, `session_catalog`, `pending`
   and `answer_queue`, filled with the database's owner (kernel spec §1) —
   `owner_id` everywhere. The store runs the kernel's migrations first;
8. `attachments` and `event_attachments`, with `owner_id` from the start —
   images (6a);
13. `turn_attachments` (backfilled from `turns.content`), the
   `attachments_by_hash` index, and the triggers that refuse every write for a
   deleted session's tombstone — delete (9a). Later migrations that update
   `sessions` must leave tombstones alone.

`sessions` has no `hat_id`, `source_kind`, `title`, git columns or
`last_event_id` yet, `turns` keeps only `content`, `state`, `outcome` and its
creation time, and `session_catalog` has no `commands` / `usage`; they and the
other tables arrive with the plans that need them.

- **`owner_id`** is `NOT NULL DEFAULT ''`, with no foreign key: SQLite adds a
  `REFERENCES` column only nullable, and rebuilding six tables that reference
  each other needs foreign keys off, which a migration cannot do. A row written
  without an owner is found by no query. Every statement names the owner,
  child rows and joins included; a write for a session that is not the owner's
  writes nothing and fails, and the catalogue upsert updates only the owner's
  row. The owner is always a parameter, never copied from a row.

- **Idempotent ingest:** `INSERT … ON CONFLICT(session_id, host_seq) DO
  NOTHING`; a conflicting row whose body differs (compared structurally,
  §3.6) is recorded as a `conflict` event. The ack is sent only after the
  transaction commits.
- **`events.applied`.** A host fact that is stored but did not apply (§4.2's
  visibility rule) keeps its row as the idempotency key with `applied = 0`.
  The timeline (`GET …/events`) and SSE replay list only applied rows, so a
  replay can never show, say, two ends for one turn.
- **`answer_queue.delivered`** is the verdict: NULL until `answer_result` or
  the question's cancellation (§4.6). It replaces a separate `state` column.
- **`session_catalog`** and the `model`, `mode` and `config_axes` columns hold
  the last catalogue snapshot a host reported (§3.2): every applied snapshot
  overwrites them, one without options changes nothing, and a resume
  re-sends the stored values.
- **Collector-originated events** have `host_seq = NULL` and are ordered by
  `event_id`. Their kinds are a closed enum in `hennery-proto`:

  | Kind | Written when |
  |---|---|
  | `operator_started`, `operator_resumed` | A start or resume is requested (→ `starting`). |
  | `user_turn` | `turn_started` is ingested for an operator prompt. |
  | `operator_renamed`, `operator_closed`, `operator_parked` | Operator actions. |
  | `hat_reassigned` | §4.9. |
  | `presumed_parked{reason: host_offline \| host_revoked}`, `reattached` | Host offline or revoked / back (§5.3). |
  | `host_restarted` | §5.2. |
  | `turn_ended_synthesized` | §4.4, §5.2. |
  | `start_not_delivered`, `turn_not_delivered` | Reconciliation (§5.1), releases (§4.2). |
  | `answer_submitted` | §4.6. |
  | `pending_cancelled{pending_id, reason}` | A question cancelled collector-side (§4.6). |
  | `conflict{seq, received}` | Same `(session_id, seq)` with a different body. |
  | `session_deleted` | Tombstone after delete (§4.10). |

  Not written yet: `operator_started` (a start writes no collector event so
  far) and `operator_renamed`. A cancel writes none by design (§4.4).

- **`sessions`, `session_catalog`, `plans` and the model/mode columns are
  filled from extracts** and from the fields of typed bodies, never by parsing
  ACP payloads.
- **Heavy blobs never ride the list:** the session list reads only `sessions`.
  Catalogues, commands and plans have their own endpoint. A test asserts the
  serialised list item stays under 1 KiB for a realistic session. *(P-23: the
  predecessor's session list grew to 5.6 MB for ~600 sessions because
  per-session command and config catalogues rode along, and every event
  refetched it.)*
- `host_agent_catalog` starts from the static catalogues in `hello` /
  `probe_agents` and is refined from each `session_started`; it powers the
  New-session pickers before a session exists.
- Gateway tokens and the `headers` of `mcp_servers` entries are never written
  to the events table and never sent over SSE.

---

## 9. REST and SSE API (sessions)

All endpoints require an operator session (kernel spec §3). Types come from
`hennery-proto`.

| Method & path | Purpose |
|---|---|
| `GET /api/sessions?cursor&limit&q&hat&lifecycle` | Paginated list, newest `last_event_at` first. `q` searches title, cwd, branch, id across all sessions regardless of filters except hat. Each item carries `hat_id` (`''`: a session from before hats that got none). `hat=<id>` lists that hat's sessions (an unknown hat lists none); an empty `hat=` is 400 `invalid` (plan 5c). |
| `POST /api/sessions` | Start: `{host_id, agent, cwd, model?, mode?, axes?, first_prompt?{content[]}}` → 202 `{session_id, turn_id?}` once `session_started` is ingested; 400 `unknown_host`; 400 `invalid_cwd`; 409 `hat_ambiguous`; 409 `host_offline` (no session is created, plan 5c); 502 with the host's code (`start_failed`, `unknown_agent`, …); 503 `delivery_unknown` **with `session_id`** (the session exists and may still start; the caller has no other way to learn its id). |
| `GET /api/sessions/{id}` | Session detail `SessionDetail`: the list item (lifecycle, activity, failure reason, `presumed_parked`), the open turn `{turn_id, state: sent \| started}`, and `pending[]`: the open questions as `PendingItem {pending_id, session_id, kind, state, reason?, turn_id?, option_ids?, payload, answered, delivered?}`, oldest first (`answered`: an answer is queued; `delivered`: its verdict, absent until one comes). |
| `GET /api/sessions/{id}/events?before=<event_id>&limit` | Timeline page ending before an event; without `before`, the tail. The frontend opens at the tail. |
| `GET /api/sessions/{id}/events?after=<event_id>&limit` | Timeline page after an event (applied rows only, §8). |
| `GET /api/sessions/{id}/catalog` | `SessionCatalog {session_id, config_options[], model?, mode?, axes{}}`; commands, plan and usage join it with the plans that produce them. |
| `POST /api/sessions/{id}/resume` | 202 `LifecycleResponse {session_id, lifecycle}` once `session_started` is ingested; 409 `starting` / `active` (its lifecycle); 409 `agent_has_no_record` (no agent session id, host not contacted); 409 `host_offline` (nothing changes); 409 `hat_mismatch`, `cwd_moved`, `hat_ambiguous` (§4.3); 400 `invalid_cwd`; 502 with the host's code for any rejection or `start_failed` (the session becomes `failed` with it); 503 `delivery_unknown` (stays `starting`, reconciled like a start). |
| `POST /api/sessions/{id}/prompt` | `{content[]}` → 202 `{turn_id}` once `turn_started` is ingested; 409 `not_attached` (not `active`) / `host_offline` (host not ready) / `turn_in_progress` / `images_unsupported` (the host lacks `images`, nothing sent; or its agent takes none); 400 `empty_prompt`, 400 `invalid_content` (a block other than text or image, an image of another type, or bytes that are not the type they claim), 400 `invalid` (host); 413 `content_too_large` (§11's image and text limits; a body over 24 MiB gets 413 `body_too_large`); 503 `delivery_unknown` (the turn stays open until reconciled). Everything the collector refuses is checked before a turn opens or a file is written, except the host's per-agent refusal, a failed send and a turn that opens between the files and the turn row, which leave the prompt's images stored but unreferenced. |
| `POST /api/sessions/{id}/cancel` | Cancel the open turn → 202 `CancelResponse {turn_id, outcome}` once that turn's `turn_ended` is ingested, with its real outcome (`cancelled`; `completed` or `failed` if it ended first; `interrupted` if the session was parked or closed meanwhile, or its adapter exited); 409 `not_attached` / `no_open_turn` / `not_running` (§4.4). |
| `POST /api/sessions/{id}/park` | Explicit park → 202 `LifecycleResponse` once `session_parked` is ingested; 409 `not_attached` (not `active`, or host not ready); 409 `park_unsupported` (host lacks the `park` capability, nothing sent). |
| `POST /api/sessions/{id}/close` | Close → 202 `LifecycleResponse` once closed (at once when unattached, parked, presumed parked, failed or the host is offline; on `session_closed` when attached); 409 `starting` while a start is in flight on a reachable host (§4.8). |
| `DELETE /api/sessions/{id}` | Delete (§4.10) → 204; 403 `step_up_required` before anything is read; 404 (unknown or deleted); 409 `starting` on a reachable host, or the lifecycle as its code if it moved during the delete; 503 `delivery_unknown` (nothing deleted, the close stays requested). |
| `POST /api/sessions/{id}/config` | `{config_id, value}` (a select's value id or a boolean) → 202 with the session's stored `SessionCatalog` once `config_applied` is ingested (after a read-back without options it still shows the old values, §3.2); 409 `not_attached` (not `active`, or host not ready) / `unknown_option`; 400 `invalid`; 502 `config_failed`; 422 for a value that is neither a string nor a boolean. Every viewer also gets SSE `catalog_changed`. |
| `POST /api/sessions/{id}/pending/{pending_id}/answer` | `{option_id}` (permission) or `{action, content?}` (elicitation) → 202 `{pending_id, request_id}` once queued, whatever the lifecycle or host state; 404; 409 `not_open` / `already_answered`; 400 `invalid`; 422 for a body that is neither kind. The verdict follows as SSE `pending_changed`. |
| `PATCH /api/sessions/{id}` | `UpdateSessionRequest {hat_id?}` → 200 `SessionDetail`; hat re-assignment (no running adapter, §4.9): 403 `step_up_required`, 400 `invalid` (not the owner's hat), 404, 409 lifecycle or `presumed_parked`. A body naming no hat needs no step-up; renaming (`title`) joins it later. |
| `GET /api/attachments/{sha256}` | Image bytes of the owner's, as their stored type, with `nosniff`, `Content-Security-Policy: default-src 'none'`, `Cross-Origin-Resource-Policy: same-origin` and `Cache-Control: private, max-age=31536000, immutable`; 404 for any name that is not one of the owner's images. |
| `GET /api/settings/attachments` | `AttachmentUsage {count, bytes}`: the owner's stored images, each once, for Settings (§15). |
| `GET /api/hosts/{id}/projects` / `…/browse?path=` | Project picker. |
| `GET /api/hosts/{id}/agents` | Agents, availability, auth and catalogues for that host. |

**Common answers.** 404 `not_found` for an unknown session. 503
`delivery_unknown` ("host disconnected; delivery unknown") whenever the host
connection dropped or the request timed out after it was sent (§3.4); the
fact, if it happened, still arrives and applies. 409 `host_offline` to a start,
resume or prompt when the host is not connected, or connected but not yet
reconciled; cancel, config and park answer `not_attached` then. Error bodies are
`ApiError {code, message, session_id?}`.

**Built so far:** the rows above except the session list, `events?before=`,
`PATCH`, and `GET /api/hosts/{id}/projects`, `…/browse` and
`…/agents`; a start takes no `first_prompt` yet (202 `{session_id}`). The list stream `GET /api/stream/sessions` is
not built yet either. The host registry routes are kernel spec §8.

**SSE** (umbrella §11.2). **Every state change first writes an events row, and
every SSE message carries the `event_id` that caused it** as its `id:`; both
streams send a comment keepalive every 15 s. Both end when the operator
session that opened them ends (kernel spec §3.2).

- `GET /api/stream/sessions/{id}` — every timeline event for one session plus
  `catalog_changed`, `pending_changed`, `turn_changed`. It resumes from
  `Last-Event-ID` **directly from the events table**; there is no catch-up
  window. A session the owner does not have is 404 `not_found`, like
  another owner's. The replay is read a page (500 events) at a time, the
  next when the stream is polled again, so at most a page and what the
  connection buffers are held; a failed read sends `resync_required` and
  ends the stream.
  A deleted session's stream, and its `GET …/events`, answer 404 too; an
  open stream ends after it sends `session_deleted` (plan 9a).
  - **Derived messages share their event's id.** `catalog_changed` (data: the
    `SessionCatalog`) follows every listed `session_started`,
    `config_applied` or `acp_update` whose extracts carry a snapshot; in a
    replay, only the last such event of each page gets one.
    `pending_changed` (data: the `PendingItem` **as it stands now**) follows
    every `pending_opened`, `pending_resolved`, `pending_cancelled`,
    `answer_submitted` and `answer_result`. Both are derived from the stored
    event, so a replay from `Last-Event-ID` sends them too; a replayed
    `pending_changed` carries the latest state, not the historic one, and the
    client ends in the right state either way.
  - Not built yet: `turn_changed`.
- `GET /api/stream/sessions` — list deltas: `session_upsert` (the full list
  item) and `session_removed` (only on delete). Clients apply them to a keyed
  store and never refetch the list because of an event. It resumes from
  `Last-Event-ID` within a **24-hour** window; beyond it the server sends
  `resync_required` and closes, and the client refetches the snapshot.
- Slow subscribers are disconnected with `resync_required`, never silently
  skipped. *(P-24: the predecessor's hub dropped messages for slow subscribers
  with no signal; the UI stayed wrong until the next unrelated event.)*

---

## 10. Push triggers

Evaluated on ingest, edge-triggered only:

| Edge | Default title / body |
|---|---|
| activity → `blocked` | `<session title>` / "needs your answer" |
| `turn_ended{completed}` | `<session title>` / "finished" |
| `turn_ended{failed}` or `agent_failure` (severity ≠ warning) | `<session title>` / "failed" |

- Title is the session title, falling back to the project directory name. No
  tool names, prompt text or transcript excerpts by default; per-hat settings
  may opt into more (umbrella §8.3), or into a **generic title** ("Session
  needs your answer", with no session title at all). Hats can be muted.
- The payload carries `url: /sessions/<id>`; the service worker navigates an
  existing window there (frontend spec).
- Recovery and reconciliation never push, and a `turn_ended` for an already
  ended turn never pushes. *(P-25: synthesising a turn end on reconnect would
  have pushed once per session per reconnect.)*
- Codex advisory notices (`sessionFailure` with `severity: "warning"`) are
  recorded but do not push; missing or unknown severities escalate (fail-safe).

---

## 11. Size and resource limits

| Limit | Default |
|---|---|
| WebSocket frame | 32 MiB |
| Prompt images | 20 × ≤ 5 MiB, ≤ 16 MiB decoded in total |
| Prompt text | ≤ 2 MiB in total (6a) |
| Prompt request body | 24 MiB; every other route 2 MB (6a) |
| Terminal output buffer | 1 MiB per terminal (or `outputByteLimit`) |
| Adapter stderr tail | 64 KiB |
| Outbox | 64 MiB (state-bearing frames kept beyond it, reported) |
| Chunk coalescing window | ~100 ms |
| Ingest commit batch | ≤ 50 ms |
| List-stream resume window | 24 h |
| Idle reap | 30 min |
| Host offline threshold | 10 min |
| Host start / resume bound | 75 s |
| Config switch (`CONFIG_TIMEOUT`) | 15 s; an orphan is tracked for 4× that |
| Cancel grace (`CANCEL_GRACE`) | 20 s, then the 5 s kill grace |
| Updates handled in a row (`UPDATE_BURST`) | 64 |

Configurable so far: the idle reap (`hennery host run --idle-timeout-secs`,
`0` turns it off) and the host offline threshold (`hennery collector
--host-offline-secs`). The others are constants in this build; the host's
bounds must stay below the collector's timeouts (§3.4). None is silent when
hit.

---

## 12. Testing

Beyond the umbrella §14:

**Fake adapter** (`hennery-testkit`, binary `hennery-fake-acp`): a scripted ACP
agent driven by a JSON script in `HENNERY_FAKE_ACP_SCRIPT`. Scenarios that must exist, each run over the
in-memory pipe and a real WebSocket:

1. start with model+mode+axes; announced catalogue is post-switch; mode applied last.
2. resume: replayed history kinds dropped, state kinds kept; no duplicate events.
3. resume of a never-prompted session → `-32002` → `failed/agent_has_no_record`.
4. prompt during a turn → 409; empty prompt → 400; prompt to a parked session → 409 `not_attached`.
5. adapter killed mid-turn → `turn_ended{interrupted}`, pending cancelled
   `adapter_lost`, `adapter_exited` with scrubbed stderr tail, `parked`.
6. WS drop mid-turn → no session parked; outbox resent; no gap, no duplicate;
   the in-flight prompt's HTTP call fails "delivery unknown" and the turn is
   reconciled from the resent `turn_started`.
7. host restart mid-turn → after `resend_complete` the collector synthesises
   `interrupted`, parks, cancels pending with `host_restarted`; no eager
   re-spawn; reconciliation never runs before `resend_complete`.
8. host offline past threshold → `presumed` parked; reconnect with the adapter
   alive → `active`, pending intact.
9. answer queued while the host is offline → delivered after reconnect;
   a second answer for the same pending → 409 `already_answered`; a resent
   answer is deduped by `pending_id`.
10. elicitation with no answer for a simulated 12 h → still open.
11. concurrent resume ×2 → one attach, one 409; a duplicate `start_session`
    re-emits state without a second adapter.
12. outbox overflow → only chunk frames dropped, `transcript_gap` with
    `seq = to_seq`, acks advance, no state-bearing frame lost.
13. oversized frame → sender-side error, connection intact.
14. collector restart → host reconnects, nothing parked; a `starting` session
    is reconciled.
15. idle reap never during a turn or while blocked.
16. terminal spawn failure → exit −1 with reason; kill reaches grandchildren.
17. outbox lost on the host → `hello_ack` seq higher than the host's counter →
    counter fast-forwarded; new frames not discarded as duplicates. Same for
    a resume of an unattached session after outbox loss (`committed_seq` in
    `resume_session`).
18. same `(session_id, seq)` with a different payload → `conflict` event.
19. second connection for a connected `host_id` → `already_connected`; the
    older connection is closed only after it misses its deadline.
20. resume where the path now resolves to another hat → 409 `hat_mismatch`.

**Live gates** (real adapters, logged-in CI account, every pin bump):

- shell command actually executes (file created on disk);
- form elicitation round trip with `answer_result{delivered: true}`;
- model switch read-back: the adapter's reported `currentValue` equals the
  requested model; a bogus id is never reported as current;
- resume re-applies mode (bypass-type mode survives a host restart);
- Codex session writes a file with no client fs/terminal handlers;
- per-session MCP isolation: a global probe server receives nothing
  (the spike harness, automated);
- gateway token exposure: while a Claude session runs, the process list is
  searched for the session's token (gateway spec §3.2); the result is recorded
  per pin. Codex is measured the same way.

**Timing.** A test that depends on ordering holds the session actor through
`hennery-host`'s `test-hooks` feature (enabled only by `hennery-testkit`, never
in a shipped build), or polls with a deadline, never a fixed sleep. Each such
test must pass with four copies of its test binary running at once (CI runners
have 2–4 vCPUs).

---

## 13. Out of scope here

Observed sessions (the `source_kind` column reserves the room), worktree per
session, Changes tab, config explorer, auto-naming, memory. Gateway internals
(own spec). Install and service management (distribution spec). Rendering
(frontend spec).

---

## 14. Predecessor incidents referenced

| Id | Incident | hennery rule |
|---|---|---|
| P-1 | Adapters died with the WebSocket | §2.1 |
| P-2 | Start/resume on the read loop stalled other sessions | §2.2 |
| P-3 | Nesting guard env vars made the agent refuse to start | §2.3 |
| P-4 | Adapter exit never detected; session `running` forever | §2.3 |
| P-5 | Orphaned agent process trees exhausted memory | §2.3 |
| P-6 | Terminal spawn failure looked like silent success | §2.5 |
| P-7 | Oversized frame closed the socket | §3.1 |
| P-8 | Unknown frames dropped silently; missing handler shipped | §3.3 |
| P-9 | Failed resume always waited the full timeout | §3.4 |
| P-10 | Event with empty host field made a session unresumable | §4.1 |
| P-11 | Concurrent resumes orphaned an adapter | §4.2 |
| P-12 | Start ordering lived in the browser | §4.3 |
| P-13 | Pre-switch catalogue overwrote the stored mode | §4.3 |
| P-14 | `session/load` of a never-prompted session fails | §4.3 |
| P-15 | Overlapping prompts broke the running flag | §4.4 |
| P-16 | Empty prompt produced an adapter error | §4.4 |
| P-17 | Turns without a terminal event left `running` forever | §4.4 |
| P-18 | Resume replay duplicated transcripts | §4.5 |
| P-19 | Boolean elicitation capability silently discarded | §4.6 |
| P-20 | Eager re-spawn on every reconnect | §5.2 |
| P-21 | Advertising fs write removed the model's write tools | §6 |
| P-22 | Sent images never stored | §7 |
| P-23 | Session list payload blew up to megabytes | §8 |
| P-24 | SSE slow subscribers silently skipped | §9 |
| P-25 | Reconnect-time synthetic events would push | §10 |

---

## 15. Open questions

Resolved by the maintainer on 2026-09-27:

1. **Attachment retention** — no size cap in v1. Images live as long as their
   session; Settings shows the attachment store's disk usage (frontend §8),
   from `GET /api/settings/attachments` (§9).
2. **Per-hat "isolate agent user config"** — not in v1, documentation only.
   The docs next to the hat settings say that each session still loads the
   user's own agent configuration (umbrella §8.4) and that a hat needing this
   isolation belongs on its own host. Possible later routes, unverified: the
   SDK's `settingSources` in `_meta` for Claude; leaving `AGENTS.md` out of the
   composed home and stripping `notify` for Codex.

Still open (a measurement, not a decision):

3. **Concurrent Codex processes sharing state** through the composed home —
   Codex on mixed hosts stays on the fallback until a live gate measures it
   (§6).

Decided with the implementation plans (2026-09-27 to 2026-10-01; plans A and
B1 confirmed by the maintainer, B2a, B2b and 2 by a stronger-model review on
the maintainer's behalf), where this spec was silent or open. The sections above state them;
the plans' "Decisions" lists hold the reasoning:

4. **A switch that does not take never fails a start** (§4.3), as for a
   resume.
5. **A request timeout on a live connection drops that connection** (§3.4).
6. **The `park` capability gates `park_session`** (§3.3, §9); capabilities
   are lenient (§3.3).
7. **`cancel_turn` answers with the turn's real outcome** (§3.3, §4.4); an
   adapter that ignores it is stopped after 20 s and parked.
8. **Questions outside a turn** are opened, leave `activity` alone, and keep
   the reaper away (§4.2, §4.6, §4.7); a turn cancel cancels them too.
9. **A host's refusal of an answer is no verdict** (§4.6).
10. **Duplicate detection is structural**, not by hash (§3.6).
11. **Images** (plan 6a, by a stronger-model review on the maintainer's
    behalf): checked before anything is written; only text and image
    blocks; stored and sent as checked, the turn and the `user_turn` event
    holding references, never bytes; attachments keyed by owner and hash;
    refused per agent (§3.3, §7, §8, §9).

---

_Generated with Claude AI — please review before distribution._
