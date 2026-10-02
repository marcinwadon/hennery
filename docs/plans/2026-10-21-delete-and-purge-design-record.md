# Delete and purge (plan 9): the design record

This is the record behind plans [9a](2026-10-15-session-delete.md), [9b](2026-10-17-orphan-sweep.md), [9c](2026-10-18-hat-purge.md), [9d-i](2026-10-19-host-transcripts.md) and [9d-ii](2026-10-20-codex-transcripts.md). It holds three parts:
- the design decisions and the security reviews' amendments, made on the maintainer's behalf;
- the operator's delegated decisions, decided by the parent on 2026-10-02;
- the evidence on Claude's and Codex's own session storage that 9d rests on.

The plans are the authority for what was built. This file says why. At a pin bump, re-read §3 (the 9d research) against the new adapter versions.

## 1. Decisions for plan 9 (9a–9c), with the security review's amendments


Scope: umbrella §6.10, ACP core §4.10 (delete), kernel §5.5 (purge, `purged_hats`, `forget_hat`), ACP core §3.3 (`forget_hat`),
frontend §6.6/§8 (header "Delete session", "Purge hat" listing what goes), and the hand-ons:
- 5c: purge deletes by `hat_id` across every lifecycle and host; refuses while any of them may run; `hat_rule_id` may dangle;
  sessions with `hat_id = ''` are listed for the operator.
- 5a: refuse the hat named by the `default_hat_id` setting and any host's default; rules before the hat; then
  `on_hat_purged`, `purged_hats`, `forget_hat`.
- 5d: sessions re-assigned into a hat are that hat's.
- 6a: a file is removed only when no reference to its hash remains (event_attachments AND turns.content, every owner);
  orphaned files from abandoned / lost-race turns and `.tmp` leftovers are swept.
- Plan 8 (gateway, parallel): delete revokes the session's gateway tokens through the same hook as close (`SessionMcp::revoke`,
  added by plan 8 when it rebases); `on_hat_purged(hat_id) -> anyhow::Result<()>` added here, implemented by the gateway,
  idempotent, its own transaction, called before the kernel deletes the hat row (gateway rows have composite FKs to hats).

Split: **9a** session delete (PR 1); then **9b** orphan sweep and **9c** per-hat purge in parallel (PRs 2, 3).

### 9a — session delete

1. **Tombstone = the `sessions` row, scrubbed.** `events.session_id REFERENCES sessions(id)` with `foreign_keys=ON`, so the
   spec's `session_deleted` tombstone event needs its row. The row keeps `id`, `owner_id`, `host_id`, `hat_id`, `created_at`,
   recency; it is set `lifecycle = 'deleted'`, and every column that can hold client data is cleared: `cwd = ''`,
   `agent = ''`, `title`, `git_branch`, `git_dirty`, `git_worktree`, `base_commit`, `model`, `mode`, `config_axes`,
   `agent_session_id`, `failure_reason`, `hat_rule_id`, `open_turn_id`, `activity` NULL, flags 0.
   Everything else of the session is deleted: events, turns, `turn_attachments`, `event_attachments`, pending, answer_queue,
   session_catalog. One `session_deleted` event (body `{}`) is written (before the lifecycle flips). Tombstones are kept
   (no content; a host away for months must still have its adapter closed on return). No undo.
2. **Never resurrects (schema-level).** A migration adds triggers that abort any `INSERT` into events (other than the
   `session_deleted` one), turns, pending, answer_queue, session_catalog for a `deleted` session, and any `UPDATE` of a
   deleted row of `sessions`. Explicit guards stay in the code paths (ingest, `collector_event`, `close_in`, `mark_failed`),
   so a racing writer gets a typed answer, not a trigger error.
3. **Accessors renamed** (fleet rule): `Store::session` → `Store::find_session`, `session_item` → `find_session_item`; both
   never return a tombstone, so every route answers 404 for a deleted session. `deleted` never joins `LIFECYCLES`, so the
   list (with or without `q`) never shows one. ws's ownership check uses its own accessor that sees tombstones.
4. **Host side.** A frame for a deleted session of that host is acked and discarded (nothing stored), so the host prunes
   its outbox; `hello_ack.committed` for it is 0. `reconcile_host` sends `close_session` for a deleted session the host
   lists as attached (as for `closed`); its `session_closed` is discarded; a `not_attached` answer changes nothing.
5. **`DELETE /api/sessions/{id}`**: step-up, checked before anything is read (method-layered `require_step_up`; the path
   is shared with GET and PATCH). 404 unknown or deleted. Then exactly as `POST …/close`: `starting` on a reachable host →
   409 `starting`; `active` on a reachable host → `close_session`, waiting for `session_closed`; `delivery_unknown` → 503
   and nothing is deleted (the close stays requested); anything else is closed collector-side at once (a presumed-parked
   session included — its host closes it on return, decision 4). Then the store deletes it in one transaction that
   requires `lifecycle = 'closed'` (409 with the lifecycle if a resume raced in). 204.
   *Spec says "closes an attached session first, then deletes": this is that.*
6. **Attachment references.** New `turn_attachments(turn_id, sha256, position, owner_id)` filled by `open_turn_with`
   (FK to turns, so `abandon_turn` deletes them), backfilled from `turns.content` via `json_each` where the attachments row
   exists. An image's references are `turn_attachments` ∪ `event_attachments`.
   The delete transaction collects the session's hashes, deletes its rows, then deletes the owner's `attachments` rows no
   longer referenced (owner-filtered). After commit, still holding the store's mutex, each such hash's file is removed
   when no `attachments` row of **any** owner names it (files are shared by hash) — a cross-owner existence read, in a
   function the owner audit lists as an explicit exemption with its reason. Every DELETE stays owner-filtered.
   `open_turn_with` re-writes a missing file under the same mutex, so a delete racing a prompt that re-sends the same
   image cannot leave a turn without its file. `abandon_turn` gets the same clean-up. Usage
   (`GET /api/settings/attachments`) drops by what was removed.
7. **Crash safety.** One transaction for the rows; files after commit. A crash between leaves files with no row: 9b's sweep
   (startup + hourly) removes them; until 9b lands they are orphans (recorded). A crash before commit changes nothing.
8. **Gateway**: a marked call site where plan 8 adds the session-token revoke.

### 9b — orphan sweep

9. At startup and hourly: delete the owner's `attachments` rows with no reference; then remove files in `attachments/`
   that no row of any owner names and whose mtime is over 1 hour old, and `.tmp` files over 1 hour old. `save_images`
   refreshes the mtime of a file it finds already there, so the grace protects an image between `save_images` and
   `open_prompt`. The file pass holds the store mutex (decision 6's race).

### 9c — per-hat purge

10. **Freeze first, then delete, resumable.** `POST /api/hats/{id}/purge` (step-up):
    a. 404 unknown hat (or already fully purged). 409 `hat_is_default` if it is the `default_hat_id` setting or the
       default of any host, revoked hosts included (the FK). 
    b. Running sessions: every session of the hat that is `starting` or `active` on a **reachable** host → 409
       `sessions_running` with their ids (close them first). Sessions with no adapter the collector can reach
       (presumed parked, active on an offline or revoked host, starting on an offline host) are closed collector-side
       and deleted like decision 5 does — their host closes the adapter on return (decision 4), and the gateway's
       purge hook kills the hat's tokens. *(Spec-gap: the 5c hand-on said "refuse while any may run"; refusing
       presumed-parked sessions would make a hat with a revoked host's sessions purgeable only after closing each one,
       and closing is exactly what this does.)*
    c. Kernel transaction: insert `purged_hats(hat_id, owner_id, purged_at)` (on conflict nothing) and delete the hat's
       path rules. From here the hat is **frozen**: `create_session`, `request_resume` and `reassign_hat` refuse it inside
       their own transactions (a start or resume that resolved before the freeze and commits after it is refused), and
       the kernel's `hat_exists` (rules, host defaults) treats it as unknown.
    d. `on_hat_purged(hat_id)` hooks: sessions deletes each session of the hat in its own transaction (as decision 5's
       store delete, after closing collector-side; one per session so the store mutex is not held across hundreds), then
       the files; the gateway (plan 8) deletes its rows. A session found running (a race) fails the hook: 409, the hat
       stays frozen, a re-POST resumes.
    e. Kernel transaction: delete the hat row (project recents cascade). 200 `PurgeResult {sessions, rules}`.
    A crash anywhere leaves the hat frozen (`purged_hats` row, hat row present): a re-POST resumes, and the collector
    finishes unfinished purges at startup.
11. **Preview** `GET /api/hats/{id}/purge` (no step-up, reads only): counts of sessions, rules and recents, the running
    session ids (b), and the sessions with no hat (`hat_id = ''`) for the operator (5c hand-on), as list items, at most
    100 with a count.
12. **`forget_hat{hat_id}`**: after each reconciled handshake the collector sends one per `purged_hats` row younger than
    30 days. Rows older than 30 days are pruned **only if their hat row is gone** (an unfinished purge never unfreezes).
    Host: a match arm that logs; plan 8 deletes the composed agent home there (there are none yet). Older hosts ignore an
    unknown frame (logged), so no capability is needed.
13. Tombstones of purged sessions keep the purged `hat_id` (an id, no content).

### Not decided here (raised with the parent)
- §6.10 removes the collector's copy. The agent's own transcript on the host (Claude's and Codex's session files) survives
  a delete. Should delete reach the host? Product question; plan proceeds per spec.

Generated with Claude AI — please review before distribution.

---

### Amendments after the security review (opus, 2026-10-02: "approve after amendments")

- **A1 (binding, taken):** every multi-session statement (`reconcile_host`, `revoke_host`, `presume_parked`,
  `hosts_with_active_sessions`, the hat/host scans) excludes `lifecycle = 'deleted'` in SQL; `ingest` re-checks the
  tombstone inside its own transaction → `Ok(vec![])` (acked); `collector_event`'s EXISTS gains `AND lifecycle <> 'deleted'`.
  Tests: a tombstone, listed and unlisted, through each of the three bulk functions. A comment beside the triggers: later
  migrations that UPDATE sessions must exclude tombstones.
- **A2 (binding, taken):** 10c's freeze transaction re-checks the `default_hat_id` setting and every host's default
  (revoked included); `update_hat(default_for_new_hosts)`, `update_host(default_hat_id)`, `replace_path_rules` and
  pairing refuse a frozen hat (`hat_exists` and `hats_of`-based checks both).
- **A3 (binding, taken):** `reassign_hat` refuses a frozen hat as source or target.
- **A4 (binding, taken):** the collector-side close in DELETE and in the purge hook is a compare-and-set on the
  lifecycle and `presumed_parked` the route decided on (as `close_after_rejected_reconcile_close`); otherwise 409.
- **A5 (binding, taken):** `turn_attachments` FK to turns `ON DELETE CASCADE`, and FK `(owner_id, sha256)` → attachments.
- **A6 (binding, taken):** one function `hash_named_by_any_owner(&Connection, &str) -> Result<bool>` in its own file
  (`hennery-sessions/src/shared_files.rs`), in the audit's `EXEMPT` with its reason; the delete and the sweep both use it.
- **A7 (binding, taken differently):** `purged_hats` rows are kept for good (ids only, like tombstones), not 30 days:
  `forget_hat` goes after every reconciled handshake for every purged hat, so a host away for any time still forgets.
  Spec amendment of kernel §5.5 ("kept 30 days"). Handed to plan 8: the host validates `hat_id` (`hat-<hex>`) before
  building a path from it.
- **A8 (binding, taken):** `PRAGMA secure_delete = ON` in `db::configure`; best-effort `wal_checkpoint(TRUNCATE)` after a
  delete or purge; a test greps the database file's bytes for a deleted title and cwd; docs: earlier backups keep the data.
- **A9 (optional, taken):** the events trigger refuses every insert for a deleted session (no exception).
- **A10 (optional, taken):** `GET …/events` and `GET /api/stream/sessions/{id}` answer 404 for an unknown or deleted
  session; an open stream ends after it sends `session_deleted`.
- **A11 (optional, recorded):** `Clear-Site-Data: "cache"` on DELETE/purge is left to the kernel's logout change (6a's O1)
  and the frontend plan: it would also clear the PWA's caches.
- **A12 (optional, taken):** no purge completion at startup; a re-POST resumes. The hat list item gains `purging: bool`, so a
  frozen hat is visible.
- **A13 (optional, taken):** `PurgeResult.unconfirmed`: sessions closed collector-side without their host; the preview
  counts exclude tombstones and say how many rules go.
- **A14 (optional, taken):** the sweep lists outside the mutex, re-checks and unlinks each candidate under it, in batches;
  only `is_sha256` names or the `.tmp` pattern, only regular files (`symlink_metadata`).
- **A15 (taken):** the gateway hook runs before the sessions hook (a hat stuck frozen cannot reach MCP meanwhile).
- **Missing item (taken):** a single delete removes the session's `project_recents` entry (host, hat, cwd) unless another
  kept session of that host and hat has the same cwd; a purge cascades them with the hat row.

### Scoped re-confirmation (same reviewer): "confirmed with notes"
- A7 confirmed; kernel §5.5 write-back drops "kept 30 days" ("kept"). Plan 8 hand-on (host validates `hat-<hex>`) binding.
- A11 confirmed; docs say a deleted image can stay in the operator's browser cache until the logout/frontend change.
- Recents removal, binding: R1 read the cwd in the delete transaction before the scrub; R2 "another kept session" excludes
  tombstones, same transaction; R3 owner-filtered, in an audited source, on the store's transaction; R4 exact bytes on
  (host, hat, cwd).
- A12: the frontend offers "resume purge" on a `purging` hat.

### 9d — the agent's own transcript on its host (operator delegated, parent decided 2026-10-02)
- A delete or per-hat purge also removes the agent's transcript on the host, best effort; what could not be removed is
  reported with the delete/purge result and retried when the host reconnects.
- Codex: the host runs the bundled `codex app-server` (pinned, managed set) and calls `thread/delete` for the session's
  thread. On failure (protocol mismatch, `--use-cli` unknown version, binary missing) fall back to codex-acp's archive
  then delete the archived transcript files, reporting "conversation copies may remain in Codex's own database". The
  `thread/delete` call shape is pinned per Codex version in the manifest; contract test against a fake app-server; the
  operator's live pin-bump checklist gains "thread/delete still removes everything". Never run app-server against a
  CODEX_HOME other than the session's recorded one.
- Claude: adapter `session/delete`, plus exact-name removal of `file-history/<id>/`, `session-env/<id>/`, `tasks/<id>/`,
  `debug/<id>.txt` under the session's recorded `CLAUDE_CONFIG_DIR`; the id validated against Claude's id format before
  any path use; no globbing, never follow links; nothing else besides the `projects/` transcript.
- Known limitation: a transcript started after a context clear inside the agent (new id hennery never learns) is not
  removed; stated in the plan, the spec and the result text; no heuristic discovery.
- Design: record each session's agent data roots at start; a per-host "still to remove" record written in the delete
  transaction before 9a's scrub. Stronger-model security review mandatory (path traversal, symlinks, wrong-session
  deletion, cross-hat).


## 2. Decisions for plan 9d, with the security reviews' amendments


Who decided: the operator delegated the question to the parent, and the parent decided on 2026-10-02. The decision is binding:
- A delete or a per-hat purge also removes the agent's transcript on its host, best effort.
- What could not be removed is reported with the result and retried when the host reconnects.
- **Codex:** the host runs the bundled `codex app-server` and calls `thread/delete` against the session's recorded `CODEX_HOME`, and never any other home.
  - If that fails: codex-acp's archive (`session/delete`), then delete the archived transcript files, and report "conversation copies may remain in Codex's own database".
  - The call shape is pinned per Codex version in the manifest.
  - A contract test runs against a fake app-server.
  - The pin-bump checklist gets a new item.
- **Claude:** the adapter's `session/delete`, plus the exact entries `file-history/<id>/`, `session-env/<id>/`, `tasks/<id>/` and `debug/<id>.txt` under the recorded `CLAUDE_CONFIG_DIR`.
  - The id is validated first.
  - No globbing beyond the id, and links are never followed.
  - Nothing else is removed besides the `projects/` transcript.
- **Known limitation:** a transcript started after a context clear inside the agent is not removed. This is stated in the plan, the spec and the result text, with no heuristic discovery.

Research behind it: `9d-research.md`, verified against claude-agent-acp 0.81.0 (SDK 0.3.280), codex-acp 1.13.0 and Codex 0.155.1.

### Decisions

#### Recording where an agent writes

1. **The agent's home is recorded at start.** On `start_session` and `resume_session`, the host resolves the agent's data root from the environment it spawns the adapter with:
   - `agent.env`, then its own environment;
   - **Claude:** `CLAUDE_CONFIG_DIR`, else `$HOME/.claude`, NFC on macOS, canonicalised;
   - **Codex:** `CODEX_HOME`, else `$HOME/.codex`, canonicalised, plus `CODEX_SQLITE_HOME` if set.
   - The host sends these as `agent_home{root, sqlite_root?}` in `session_started`, a new optional field. The collector stores them on the session: `sessions.agent_home`, JSON, nullable.
   - A resume whose recorded home differs from the one now resolved keeps the first one and writes a `host_note`, so a forget never chases a moved root.
   - A session from before 9d has no recorded home.

#### The record of what is left to remove

2. **A forget record is written in the delete's own transaction, before 9a's scrub.**
   - Table: `host_forgets(id, owner_id, host_id, session_id, hat_id, agent, agent_session_id, agent_home JSON NULL, created_at, attempts, last_result JSON NULL)`.
   - It is written only when the session had an `agent_session_id`. With no agent record there is nothing on the host.
   - The record keeps the agent's session id and root paths. It never keeps the cwd, a title or any content.
   - It is deleted once the host reports the removal complete (decision 6).
   - It is owner-filtered and in the owner audit.
3. **The forget record is not resurrection state.** Triggers stay as in 9a. The record names the tombstone's id, but nothing reads it as a session.

#### The protocol

4. **A new request: `forget_session{request_id, agent, agent_session_id, agent_home}`.**
   - It goes from the collector to a host that announces the new capability `forget_session`. A host without it is left pending, with the reason "host needs an update".
   - Reply: `session_forgotten{request_id, outcome: complete|partial, removed[], remaining[{what, reason, retry}]}`, or `error`.
   - `what` names a kind (transcript, file_history, session_env, tasks, debug, codex_database_copies) plus the path relative to the root, never an absolute path outside it.
   - `retry: false` marks the limits that a retry cannot change: the context-clear limitation, which is always stated, and Codex database copies after the fallback.
5. **When it is sent.**
   - Right after a delete or purge commits, if the host is ready.
   - Then the route waits at most 30 s for the reply, so it can report the result.
   - After every reconciled handshake (`ws::ready`), for every pending record of that host, one at a time.
   - A host that reports the session still attached gets `remaining{reason: attached, retry: true}`. The forget runs only once its adapter is gone.
6. **A record is done when the outcome is `complete`, or `partial` with every remaining item `retry: false`.**
   - Otherwise it stays and is retried at the next handshake. `attempts` is counted.
   - It is never dropped silently. A list endpoint shows what is left.

#### Results

7. **The delete result changes.**
   - `DELETE /api/sessions/{id}` → 200 `DeleteResult {host_transcript: TranscriptRemoval}`, instead of 9a's 204.
   - `TranscriptRemoval = {state: removed|partial|pending|none, remaining[{what, reason}], notes[]}`.
   - `pending` carries the reason: host offline, host needs an update, no reply in 30 s, or still attached.
   - `none` means the session had no agent record.
   - `notes` always includes "transcripts started after a context clear inside the agent are not removed" for Claude.
   - The purge's `PurgeResult` gains `host_transcripts: {removed, partial, pending}` counts and the pending session ids.
   - `GET /api/settings/host-removals` lists the pending records: host, agent, state, last result, attempts. No step-up, since it is read-only.

#### On the host: Claude

8. **The order: validate, check the root, call the adapter, then remove the exact entries.**
   - **Validate:** the id must match `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$` (lowercase, as the SDK writes it). Otherwise `error invalid`, and nothing is touched.
   - **The root** must be the recorded one. It must be absolute and canonical, and still canonicalise to itself. It must not be `/`, `$HOME` itself, or a path outside the host user's reach. It must be an existing directory.
   - **The adapter:** the host spawns `claude-agent-acp` from the managed set, or the `--use-cli` override, with `CLAUDE_CONFIG_DIR` set to the recorded root. Then `initialize`. If the agent advertises `sessionCapabilities.delete`, the host sends `session/delete {sessionId}`; a not-found error counts as success. The adapter runs with no cwd inside a project: the root itself is its cwd.
   - **The exact entries:**
     - `<root>/file-history/<id>`, `<root>/session-env/<id>`, `<root>/tasks/<id>` (directories) and `<root>/debug/<id>.txt` (a file);
     - for every directory `D` directly under `<root>/projects/`: `D/<id>.jsonl`, `D/<id>` (the subagent directory), `D/<id>.ccr-tip.json` and `D/<id>.precompact.json`. *(Spec gap for the review: the parent's rule allows "the projects/ transcript". These two are the transcript's own sidecars in the same directory, named by the id, and the CLI's retention sweep removes them with the transcript.)*
   - **The safety rules:**
     - each candidate is found with `symlink_metadata`;
     - a symlink is never followed and never removed: it is reported in `remaining{reason: symlink}`;
     - a directory is removed with `remove_dir_all`, which does not follow symlinks inside it (std, since 1.58.1);
     - `<root>/projects`, `<root>/file-history`, etc. must themselves be real directories and not links. Otherwise that kind is skipped and reported;
     - no other name is ever built.
   - **The adapter's delete and the exact entries both run.** The adapter's delete is the agent's own contract; the exact entries make sure of what it leaves.

#### On the host: Codex

9. **The primary path: `codex app-server` with `thread/delete`.**
   - The binary is the managed set's bundled `codex` (from the manifest), or `--use-cli`'s `CODEX_PATH`. With `--use-cli` it runs only if its `--version` is a version the manifest pins a call shape for. Otherwise the host goes to the fallback.
   - It runs with `CODEX_HOME` and `CODEX_SQLITE_HOME` set to the recorded values, and only those. With no recorded home there is no app-server, and the item is reported unremoved (decision 11).
   - Calls: `initialize` with the pinned client info, then `thread/delete {threadId}`. The id is validated as a UUID first.
   - `thread/delete`'s refusals ("forked history still references it", a live worker, ephemeral) are reported `remaining{retry: true}` for the live worker, and `retry: false` with the reason for the others.
   - The app-server is stopped after the call, with a 30 s budget.
   - **Manifest:** `adapters/manifest.json` gains `codex_app_server: {codex_version, initialize, delete_method: "thread/delete", params_shape}`. A contract test runs the host's client against a fake app-server binary from the testkit that checks the exact frames.
10. **The fallback: the adapter's `session/delete` (an archive), then the rollout files.**
    - The fallback runs when the app-server path is unavailable or fails with a protocol error.
    - The rollout files deleted: `<root>/archived_sessions/**/rollout-*-<id>.jsonl`, `…-<id>.jsonl.zst`, `…-<id>_*.jsonl{,.zst}` and the same under `<root>/sessions/**`.
    - Matching is by an exact filename suffix after validating the id, walking with `symlink_metadata` and never descending into a link. Only regular files are removed.
    - Reported: `remaining{what: codex_database_copies, reason: "conversation copies may remain in Codex's own database", retry: false}`.

#### Common rules

11. **A session from before 9d** (no `agent_home`) is not chased in a guessed root. For both agents it is reported `remaining{reason: "no recorded agent home", retry: false}`. *(The parent's rules name the recorded root for both. Using the host's current environment could delete in the wrong root, e.g. a service host versus a foreground one.)*
12. **Cross-hat.** A forget acts only on its own session's id under its own recorded root.
    - A purge forgets each session separately. The hat plays no part in finding files.
    - When plan 8 composes per-hat agent homes, the recorded root is that composed home, and `forget_hat` deletes it whole.
    - Handed to plan 8: a composed Codex home's `sessions/` symlink means the rollout lives in the user's real `~/.codex/sessions`. The fallback never follows that link (decision 8), so plan 8 must record the real root as well.
13. **The host refuses while a session with that id is attached** (`remaining{attached, retry: true}`). The collector closes it first anyway.
14. **The pin-bump checklist** (`packaging/README.md`) gets "`thread/delete` still removes everything (rollouts, `thread_history`, the state DB, `session_index.jsonl`)" for a live check.

### Split
- **9d-i:** recording the home (decision 1); `host_forgets` (2, 3); the protocol and the capability (4–6); the results (7); Claude (8); the common rules (11–13).
- **9d-ii:** Codex (9, 10, 14).

Generated with Claude AI — please review before distribution.

---

### Amendments after the security review (opus, 2026-10-02: "approve after amendments")
All binding amendments taken as written: **B1** host-side registry `(agent, agent_session_id, roots)` written at
`session_started`, a forget acts only on an exact match with the echoed home (`unknown_to_host`, final, otherwise);
**B2** `what` = kind + count with the project directory masked (`projects/*/<id>.jsonl`), `reason` a fixed host-chosen
code, the collector checks kinds and caps sizes; **B3** removal through directory descriptors (`O_DIRECTORY|O_NOFOLLOW`,
`openat`, `fstatat(AT_SYMLINK_NOFOLLOW)`, `unlinkat`, a descriptor-relative recursive removal) — with `libc`, already a
direct dependency of `hennery-host`, not a new crate; root and kind directories owned by the euid and not group/other
writable; the root never `/`, `$HOME` or an ancestor, or the host data dir; kind directories checked before the adapter
runs; the spec says the adapter's own delete is outside the no-follow guarantee; a failure midway is `partial, retry`;
**B4** the outcome is decided by checking afterwards that nothing named remains, never by an adapter's error text;
**B5** the Codex fallback runs only on spawn/initialize failure, method not found or unpinned version — never after
`thread/delete` itself refused; anchored rollout regex from `recorder.rs`, `sessions/` walked at most three levels,
`archived_sessions/` top level only, regular files only; the notes say subagent rollouts are out of the fallback's reach;
**B6** forget processes spawned through `Adapter::spawn`'s hygiene, `CODEX_SQLITE_HOME` and
`CLAUDE_CODE_PROJECT_DIR_NAME` removed unless recorded, cwd = the root, one deadline, the group killed; **B7** the
attached check under the sessions lock covers any live actor with that `agent_session_id`; a marker refuses an attach
while a forget runs; one forget per record in flight on the collector; **B8** every distinct `(agent_session_id, roots)`
a session used is recorded (at most 8) and forgotten; the delete's scrub clears `sessions.agent_home` after copying; no
forget for an `(host, agent, agent_session_id)` another kept session still refers to (`shared`, final); **B9** the
transcript family in each project directory is the exact names `<id>.jsonl`, `<id>/`, `<id>.ccr-tip.json`,
`<id>.precompact.json`, `<id>.cast`, `<id>.dir-sync.json`, `<id>.dir-sync-empty.json`, and the prefixes
`<id>.jsonl.superseded-` and `<id>.jsonl.compact.tmp.` (checked id); anything else is named in the notes.
Optional, taken: **O10** `invalid`, `unknown_to_host` and a revoked host's records are final; a step-up
`DELETE /api/settings/host-removals/{id}` dismisses a record; a retry also follows that session's `session_closed`; a
purge has one 30 s budget in total. **O11** the canonical bytes are stored as they are, no NFC. **O12** the notes name
the other known residue (Claude: `history.jsonl`, `telemetry/1p_failed_events.*`, `todos/`; Codex: `history.jsonl`,
`logs_2.sqlite`). A collector-side shape check of `agent_home` (absolute, bounded, no NUL) before storing; docs say a
host's report is not verified.

### Scoped re-confirmation (same reviewer): "confirmed with notes"
Binding notes on B3's hand-written removal: R1 children opened with `openat(O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC)`
(`ELOOP`/`ENOTDIR` → not a directory, `unlinkat(…, 0)`); entries read via `fdopendir` on a `dup`; never `d_type` alone
(`DT_UNKNOWN` → `fstatat(AT_SYMLINK_NOFOLLOW)`); `unlinkat(AT_REMOVEDIR)` after the contents; `EINTR` retried, `ENOENT`
= gone. R2 bounded depth and open descriptors (iterative); stop at a mount point (`st_dev` differs) → `partial`. R3 tests
watched failing: a symlink at the top entry, inside a directory being removed, and as an intermediate component swapped
in after enumeration (test hook); the target untouched; each revert-probed by a path-based call. R4 the walk's tests run
on macOS and Linux CI. Non-binding, taken: the B1 registry entry is written durably (fsync) before `session_started`.


## 3. The 9d research: deleting the agent's own transcript of a session


Date: 2026-10-02. Read-only research. The only file written is this report.

**Network:** worked (npm registry, crates.io, GitHub raw/API). The pinned
tarballs were downloaded to `/tmp/9d` and read there.

**Evidence labels:**

- **[V]** verified in the pinned code or schema.
- **[O]** observed on this machine, using the operator's own CLIs and
  directories, which are not the pinned ones.
- **[I]** inferred, not proven.

### 0. Headline: what the decision as written actually achieves

| Agent | Adapter `session/delete`? | What it removes | Transcript actually gone? |
|---|---|---|---|
| Claude (`claude-agent-acp` 0.81.0) | Yes, advertised and implemented [V] | `<root>/projects/<proj>/<id>.jsonl` and the `<id>/` directory (subagent transcripts, tool results) [V] | **Mostly.** Several sidecars keyed by session id survive. The clear-context case leaks a whole transcript (§2.6). |
| Codex (`codex-acp` 1.13.0) | Yes, advertised and implemented [V] | Nothing. It calls `thread/archive`, which **moves** the rollout to `archived_sessions/` and marks the thread archived in SQLite [V] | **No.** It is a soft delete. ACP explicitly allows that. |

The decision says to use the adapter's delete call when one exists. Both
adapters have one, so the file fallback would never run. That gives an
incomplete delete for Claude and **no delete at all for Codex**.

A file-only fallback for Codex would also be incomplete. Codex 0.155.1 keeps a
second copy of the conversation in SQLite (`thread_history_1.sqlite`:
`thread_items`, `thread_turns`, `thread_realtime_items`), plus thread rows in
`state_5.sqlite` and name entries in `session_index.jsonl` [V]. Only the Codex
app-server's own `thread/delete` removes all of that [V]. codex-acp never calls
it, at the pinned version or at the latest, 2.1.1 [V].

### 1. The ACP schema

#### Versions

- `Cargo.lock`: `agent-client-protocol` 2.2.0 and
  `agent-client-protocol-schema` 1.9.1 [V].
- The latest schema on crates.io is 1.10.2. Its delete and close text is
  identical to 1.9.1 [V].
- Source read: `~/.cargo/registry/src/index.crates.io-*/agent-client-protocol-schema-1.9.1/src/v1/agent.rs`.

#### Session lifecycle methods (all stable)

These methods are not behind an `unstable_*` feature in 1.9.1. Only
`session/fork` is gated (`unstable_session_fork`) [V]:

```rust
pub(crate) const SESSION_LIST_METHOD_NAME: &str = "session/list";
pub(crate) const SESSION_DELETE_METHOD_NAME: &str = "session/delete";
##[cfg(feature = "unstable_session_fork")]
pub(crate) const SESSION_FORK_METHOD_NAME: &str = "session/fork";
pub(crate) const SESSION_RESUME_METHOD_NAME: &str = "session/resume";
pub(crate) const SESSION_CLOSE_METHOD_NAME: &str = "session/close";
```

`agent-client-protocol` 2.2.0 exposes `DeleteSessionRequest` and
`CloseSessionRequest` without a feature flag (`src/schema/enum_impls.rs`,
`client_to_agent/requests.rs`) [V]. Hennery can send them today.

- **`session/delete`**
  - `DeleteSessionRequest { session_id, _meta }`. The response is empty.
  - Doc comment: "Request parameters for deleting an existing session from
    `session/list`. Only available if the Agent supports the
    `sessionCapabilities.delete` capability."
  - Capability: `sessionCapabilities.delete: {}` [V].
- **`session/close`**
  - `CloseSessionRequest { session_id, _meta }`.
  - "the agent **must** cancel any ongoing work related to the session (treat
    it as if `session/cancel` was called) and then free up any resources
    associated with the session."
  - It is in-memory only and does not remove anything persisted [V].
- **`session/list`**
  - `ListSessionsRequest { cwd?, cursor? }` returns `SessionInfo[]` and
    `nextCursor` [V].
- **Archive and forget:** there is no ACP method for either [V].

#### `session/delete` semantics

These come from upstream `docs/protocol/v1/session-delete.mdx`, fetched from
the `main` branch, not a pinned tag:

> - Deleted sessions no longer appear in future `session/list` results.
> - Deleting an already-deleted session, or a session that never existed, **SHOULD** succeed silently.
> - Agents may implement soft delete or hard delete. ACP only specifies the user-facing session-list behavior.
> - Behavior for `session/load` on a deleted session is implementation-defined.
> - Behavior for deleting an active session is implementation-defined.

So ACP gives **no guarantee** that `session/delete` removes data from disk.

### 2. Claude: `@agentclientprotocol/claude-agent-acp` 0.81.0

Pins come from `adapters/manifest.json`:

- `claude-agent-acp` 0.81.0
- `@anthropic-ai/claude-agent-sdk` 0.3.280
- the bundled native CLI, `@anthropic-ai/claude-agent-sdk-darwin-arm64` 0.3.280

The `claude` on PATH reports 2.1.286. That is the operator's CLI, not the
pinned one.

#### 2.1 Capabilities and handlers

From `dist/acp-agent.js` [V]:

```js
const sessionCapabilities = {
    additionalDirectories: {}, close: {}, delete: {}, fork: {}, list: {}, resume: {}, subagents: {},
};
...
.onRequest(methods.agent.session.delete, (ctx) => agent.deleteSession(ctx.params))
.onRequest(methods.agent.session.close, (ctx) => agent.closeSession(ctx.params))
```

```js
async closeSession(params) {
    if (!this.sessions[params.sessionId]) { throw new Error("Session not found"); }
    await this.teardownSession(params.sessionId);   // in-memory teardown only
    return {};
}
async deleteSession(params) {
    // Tear down any active in-memory state first so the on-disk file isn't
    // recreated by an outstanding query writing to it.
    if (this.sessions[params.sessionId]) { await this.teardownSession(params.sessionId); }
    await deleteSession(params.sessionId);          // @anthropic-ai/claude-agent-sdk
    return {};
}
```

The only extension methods are steering, async-task stop and goal (`_session/*`).
None of them deletes anything [V]. The latest adapter, 0.85.1, has the same
`deleteSession` [V].

#### 2.2 What the SDK's `deleteSession` does

The doc comment in `sdk.d.ts` (0.3.280) [V]:

> Without `sessionStore`: removes `{sessionId}.jsonl` and the `{sessionId}/`
> subagent-transcript subdirectory from the local projects dir. Throws if the
> session is not found.
>
> `dir?` … When omitted, all project directories are searched for the session file.

The implementation in `sdk.mjs` (minified; this is the `zY` function) [V]:

```js
if(!mt(e))throw Error(`Invalid sessionId: ${e}`);          // must match a UUID regex
for(let o of await VVe(t,r)){                                // every dir under <root>/projects
  let s=li(o,`${e}.jsonl`),i;
  try{({size:i}=await KVe(s))}catch(a){... ENOENT/ENOTDIR → continue}
  if(i===0)continue;                                         // a zero-byte file counts as absent
  await NY(s,{force:!0}),await NY(li(o,e),{recursive:!0,force:!0});return}
throw Error(... `Session ${e} not found in any project directory`)
```

Consequences:

- The adapter calls it without `dir`, so it **scans every project
  directory**. The cwd is not needed.
- It **throws** when the session is missing, and also when its transcript has
  zero bytes. That is contrary to ACP's "SHOULD succeed silently". Hennery
  must treat a not-found error as success.

#### 2.3 Root and project-directory encoding

From `sdk.mjs` [V]:

```js
function Ay(){return process.env.CLAUDE_CONFIG_DIR}
var an=bp(()=>(Ay()??dhe(uhe(),".claude")).normalize("NFC"),Ay);   // uhe = os.homedir, dhe = path.join
function Bt(){return Ho(an(),"projects")}
function md(e){return e.replace(/[^a-zA-Z0-9]/g,"-")}
function sh(e){let t=md(e);if(t.length<=ri)return t;return`${t.slice(0,ri)}-${WKe(e)}`}   // ri=200
function WKe(e){return Math.abs(KR(e)).toString(36)}
function KR(e){let t=0;for(let n=0;n<e.length;n++)t=(t<<5)-t+e.charCodeAt(n)|0;return t}
function oi(e){return Ij()??sh(e)}
var Ij=bp(()=>Ay()?mR(Pj()):void 0,mhe);   // Pj = CLAUDE_CODE_PROJECT_DIR_NAME; mR checks it against /^[A-Za-z0-9_-]{1,64}$/
function pt(e){return process.platform==="darwin"?e.normalize("NFC"):e}
```

- **Root:** `$CLAUDE_CONFIG_DIR`, otherwise `os.homedir()/.claude`,
  NFC-normalized. Transcripts live under `<root>/projects/`.
- **Project directory name:**
  1. Take the cwd, resolved and `realpathSync`'d, and NFC-normalized on macOS.
  2. Replace every character outside `[A-Za-z0-9]` with `-`.
  3. If the result is longer than 200 characters, keep the first 200 and append
     `-` plus `base36(|javaStringHash(rawPath)|)`. The hash is computed on the
     **unencoded** path.
- **Override:** if `CLAUDE_CONFIG_DIR` is set **and**
  `CLAUDE_CODE_PROJECT_DIR_NAME` is a valid name (`[A-Za-z0-9_-]{1,64}`, not a
  reserved device name), that name replaces the encoding.
- **[O]** In the operator's tree,
  `/home/user/projects/app/.worktree` is
  stored as `-home-user-projects-app--worktree`.
  `/.` becomes `--`.
- **[I]** I did not confirm that the bundled CLI binary, which writes the
  files, uses the same truncation for paths over 200 characters. A fallback
  should **scan** `<root>/projects/*/<id>.jsonl`, as the SDK does, rather than
  re-derive the name.

#### 2.4 Files keyed by session id

Inside the project directory:

- `<id>.jsonl` (the transcript) and `<id>/` [V]. The directory holds
  `subagents/agent-*.jsonl` and, in the operator's tree, also `tool-results/`
  [O].
- The bundled CLI's own retention sweep (`cleanupPeriodDays`, read from the
  0.3.280 binary's strings) treats these as transcript companions too [V, from
  the binary]:
  - `<id>.ccr-tip.json` and `<id>.precompact.json`. The sweep unlinks them
    together with `<id>.jsonl`.
  - `.cast`, `.dir-sync.json`, `.dir-sync-empty.json`,
    `.jsonl.superseded-*` and `.jsonl.compact.tmp.*`.
  - The SDK's `deleteSession` removes **none** of these.

Outside the project directory, under `<root>/`:

| Path | Evidence |
|---|---|
| `file-history/<id>/` (backups of edited file contents) | [V] in the binary, `VO(we(),"file-history",n\|\|K(),e)`; [O] |
| `session-env/<id>/` | [V] in the binary, `O5(we(),"session-env",K())`; [O] |
| `tasks/<id>/` | [V] the sweep covers `tasks`; [O] directories named by UUID holding `N.json` |
| `debug/<id>.txt` | [O] |
| `todos/` | [V] swept by mtime. File naming not confirmed. |
| `telemetry/1p_failed_events.<id>.<uuid>.json` | [O] |
| `history.jsonl` (prompt history, one line per prompt with a `sessionId` field) | [O] the field exists. **Unverified** whether SDK or ACP sessions write to it at all. |
| `shell-snapshots/`, `plans/` | not keyed by session id (timestamped or named by slug) [O]; swept by mtime |

The CLI's retention sweeps already age out all of these by mtime. That is a
backstop, not a delete.

#### 2.5 The ACP session id is the Claude session id, with one exception

From `createSession` [V]:

- A new session uses `sessionId = randomUUID()`, passed to the SDK as
  `options.sessionId`.
- A load or resume uses `sessionId = creationOpts.resume`.
- A fork gets a fresh UUID, which is also the fork's ACP id.

The code comment says: "`resume` names the Claude session, which shares the
ACP session id". So in the normal case, the id hennery stores **is** the
transcript's file name.

#### 2.6 The exception: clear-context leaks a transcript

On ExitPlanMode with the "clear context" option,
`clear-context-coordinator.js` restarts with
`{ publicSessionId: sessionId }`. In `createSession`:

```js
options.sessionId = creationOpts.publicSessionId ? randomUUID() : sessionId;
```

The ACP id stays the same, but the conversation continues in a new transcript
under a **random** UUID [V].

- `session/delete` with the stored id removes only the original transcript.
  The continuation is orphaned.
- I found no field or `_meta` through which the adapter reports the internal
  id to the client. The id appears only in SDK messages (`message.session_id`)
  inside the adapter. This is [I] from a grep, not proven exhaustive.

Report this as a **known leak**. A fallback could find the orphan only by
content, for example by matching the continuation message. I did not
investigate that.

#### 2.7 Environment

The adapter does not touch `CLAUDE_CONFIG_DIR` [V, grep]. The SDK spawns the
CLI with the adapter's environment (the SDK default; [I] for the exact merge).
So the adapter and the CLI resolve the same root from the adapter process's
environment.

### 3. Codex: `@agentclientprotocol/codex-acp` 1.13.0

The pin bundles `@openai/codex` 0.155.1. Codex source was read at tag
`rust-v0.155.1` on GitHub.

#### 3.1 Capabilities and handlers

codex-acp is a TypeScript bundle (`dist/index.js`) that drives
`codex app-server`. It advertises
`sessionCapabilities: { resume, list, close, delete, fork, additionalDirectories, subagents }`
and `loadSession: true` [V].

```js
async closeSession(sessionId) {
  try { await this.codexClient.threadUnsubscribe({ threadId: sessionId }); }
  finally { this.codexClient.clearThreadHandlers(sessionId); this.subagents.clear(sessionId); }
}
async deleteSession(sessionId) {
  await this.codexClient.threadArchive({ threadId: sessionId });   // soft delete
}
```

The ACP-level `deleteSession` closes any local session first and then calls
the method above [V].

- **Extension methods:** `_session/steering`, `_session/async_task/stop`,
  `_codex/session/goal_control`, `_session/goal`, `authentication/*`, and the
  legacy set-model method. None of them deletes [V].
- `thread/deleted` notifications are listed as "ignored events" [V].
- **Latest codex-acp (2.1.1, bundling `@openai/codex` ^0.159.1):** still
  `threadArchive`. It now treats an unknown thread as already deleted [V]. A
  pin bump does not fix this.

#### 3.2 What the Codex app-server offers

`codex-rs/app-server-protocol/src/protocol/common.rs` [V]:

```rust
ThreadArchive => "thread/archive" { params: v2::ThreadArchiveParams, ... },
ThreadDelete  => "thread/delete"  { params: v2::ThreadDeleteParams, ... },   // not #[experimental]
ThreadUnarchive => "thread/unarchive" { ... },
```

Both take `{ threadId }`.

**`thread/archive`** (`thread-store/src/local/archive_thread.rs`) [V]:

- It `rename`s the rollout files from `sessions/…` to `archived_sessions/`.
- It calls `state_db.mark_archived(...)`.
- It archives spawned descendant threads too.
- It refuses with `Conflict` if the thread still has an active writer.

**`thread/delete`** (`app-server/src/request_processors/thread_delete.rs` and
`thread-store/src/local/delete_thread.rs`). This is a hard delete [V]:

- It deletes the whole spawn subtree (subagent threads), descendants first.
- It refuses if forked history in another thread still references the rollout:
  "cannot delete thread …: forked history still references it".
- It refuses for ephemeral (non-persisted) threads, and for a live internal
  worker (`-32600`, per the app-server README).
- It removes:
  - the rollout files in `sessions/` **and** `archived_sessions/`, both
    `.jsonl` and `.jsonl.zst`
  - `thread_history` SQLite rows: `DELETE FROM thread_items / thread_realtime_items / thread_turns WHERE thread_id = ?`
  - the state-DB thread rows (`delete_threads_strict`)
  - the `session_index.jsonl` name entries (`remove_thread_name_entries`)
- `history.jsonl`: nothing in `thread_delete.rs` or `delete_thread.rs`
  touches it. I did not read `prepare_thread_for_removal`, so this is [I].
- `logs_2.sqlite`: the handler flushes the log DB before deleting, but nothing
  I read deletes thread-scoped log rows. This is unexamined residue [I].

#### 3.3 Layout

**Rollout path** (`rollout/src/recorder.rs`, `rollout_file_name.rs`) [V]:

- `<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-YYYY-MM-DDTHH-MM-SS-<threadId>.jsonl`.
- The date directory and timestamp use **local** time
  (`OffsetDateTime::now_local()`).
- A reverted thread uses `…-<threadId>_<rolloutId>.jsonl`.
- Files may be compressed to `.jsonl.zst`.
- Archived rollouts live in `<CODEX_HOME>/archived_sessions/<same file name>`,
  with a flat layout (`ARCHIVED_SESSIONS_SUBDIR`).
- [O] The operator's `~/.codex` matches this:
  `sessions/2026/05/13/rollout-2026-05-13T21-37-33-<uuid>.jsonl`.

**SQLite files** (`state/src/sqlite.rs`) [V]:

- The files are `state_5.sqlite`, `thread_history_1.sqlite`, `logs_2.sqlite`,
  `goals_1.sqlite`, `memories_1.sqlite`, `memories_v2_1.sqlite` and
  `queue_1.sqlite`.
- They live in `sqlite_home`. In `core/src/config/mod.rs`, that is
  `cfg.sqlite_home` (from `config.toml`), then `$CODEX_SQLITE_HOME`, then
  `codex_home`.

**Other per-thread files** [O]:

- `session_index.jsonl`: `{id, thread_name, updated_at}` per line.
- `history.jsonl`: `{session_id, text, ts}` per line.
- `shell_snapshots/<threadId>.<nanos>.sh`. These are removed when the snapshot
  is dropped and kept 3 days at most (`core/src/shell_snapshot.rs`) [V].
- `thread-writer-locks/<threadId>.lock`.

#### 3.4 `CODEX_HOME`

`codex-rs/utils/home-dir/src/lib.rs` [V]: if `CODEX_HOME` is set and
non-empty, it must exist and be a directory, and it is **canonicalized**.
Otherwise the home is `~/.codex`, and its existence is not checked.

codex-acp does not read `CODEX_HOME` itself. It records `codexHome` from the
app-server's initialize response (`this.configPath = response?.codexHome`) [V].

#### 3.5 The ACP session id is the Codex thread id

- `newSession` returns `sessionId: response.thread.id` from `thread/start`.
- Load, resume and fork pass `threadId: request.sessionId`.
- Delete and close pass `threadId: sessionId` [V].

The rollout file name embeds this id.

### 4. What the host can safely use as the root

#### 4.1 How hennery spawns adapters

From `crates/hennery-host/src/adapter.rs`, `Adapter::spawn` [V]:

- There is **no `env_clear`**. The adapter inherits the host's whole
  environment.
- It removes `INHERITED_OVERRIDE_VARS` (`CLAUDE_CODE_EXECUTABLE`,
  `CODEX_PATH`, `CODEX_CONFIG`, `DISABLE_MCP_CONFIG_FILTERING`,
  `APP_SERVER_LOGS`) before applying `agent.env`.
- After that, it removes `NESTING_VARS`, `HOST_SECRET_VARS` and
  `HOST_LOG_VARS`.
- It sets `current_dir(cwd)`.
- `HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME` and `CODEX_SQLITE_HOME` are
  **neither stripped nor set**. They pass through from the host's own
  environment.

`agent.env` is filled only in `runtime/agents.rs::from_set`, and only with the
`--use-cli` override variable (`CLAUDE_CODE_EXECUTABLE` or `CODEX_PATH`) [V].
`session.rs` calls `Adapter::spawn(&agent, &cwd)` and adds no environment of
its own [V].

#### 4.2 The service gap

How the host gets its environment depends on how it runs:

- **Foreground `hennery host`:** it inherits the operator's shell, including
  any `CLAUDE_CONFIG_DIR` or `CODEX_HOME` exported there.
- **Service:** `hennery service install` captures the login shell but keeps
  **only PATH** (`service::path::service_path`). The launchd plist and the
  systemd unit carry `PATH` and `HENNERY_SERVICE` on top of the
  launchd/systemd defaults (`service/unit.rs`) [V]. So under the service,
  `CLAUDE_CONFIG_DIR` and `CODEX_HOME` are **unset** unless the operator adds
  them by hand.

[I] So a session started while the host ran in the foreground with
`CLAUDE_CONFIG_DIR` or `CODEX_HOME` set, and deleted later under the service,
resolves a different root. Both the adapter's `session/delete` and any
host-side path would then **miss the transcript, silently**. In the Claude
case the adapter returns a not-found error, which the plan intends to swallow.

#### 4.3 Recommendation [I]

- **Record the resolved roots per session at start.** For Claude that is
  `CLAUDE_CONFIG_DIR` or `HOME/.claude`. For Codex that is `CODEX_HOME` or
  `HOME/.codex`, plus `CODEX_SQLITE_HOME` if set.
- **When deleting, spawn the adapter with those values in `agent.env`.**
  `INHERITED_OVERRIDE_VARS` does not strip them, so they pass. The adapter then
  resolves the same root it wrote to.
- **Sessions created before 9d have no recorded roots.** They fall back to
  whatever environment the host has at delete time, so the service gap applies
  to every existing session.
- **Prefer letting the agent delete over the host recomputing paths.** Codex's
  `sqlite_home` can also come from `config.toml`, which the host would have to
  parse.
- **If the host must compute a root:**
  - Use `agent.env[VAR]`, then the host's `VAR`, then `$HOME/.claude` or
    `$HOME/.codex`.
  - Canonicalize `CODEX_HOME`, as Codex does.
  - NFC-normalize on macOS for Claude.
  - Never follow a symlink out of the root when deleting.

#### 4.4 Composed CODEX_HOME interplay (spec only, not built)

This is from ACP core spec §6 and distribution spec, `codex-home/`. Neither is
on this branch or on `origin/main`. Only a doc comment in `adapter.rs`
mentions it.

- The composed home `<host-data>/codex-home/<hat>/` symlinks `sessions/` into
  the user's real `CODEX_HOME`. Every other entry is private to the hat. That
  includes `archived_sessions/`, the SQLite files and `session_index.jsonl`.
- [I] Two consequences:
  - codex-acp's archive would move a rollout from the user's `sessions/` into
    the hat-private `archived_sessions/`.
  - `forget_hat` (deleting the composed home) would remove the SQLite copies
    and archived rollouts, but **not** rollouts still under the symlinked
    `sessions/`. A hat purge alone therefore leaves those transcripts in the
    user's `~/.codex/sessions`.
- Unverified: whether Codex's `scoped_rollout_path` check accepts a symlinked
  `sessions/` root during delete or archive.

### 5. Notes for designing the fallback [I]

These are not verified live.

#### Codex options (for the operator to choose)

- **(a) Host-driven hard delete.**
  1. The host spawns the bundled `codex app-server`, honoring a `CODEX_PATH`
     `--use-cli` override, with the recorded `CODEX_HOME`.
  2. It sends `initialize`, then `thread/delete { threadId }`.
  3. Afterwards it removes any leftovers.

  This is the only path that clears the SQLite copies. Costs:
  - The host now speaks a second protocol, the app-server JSON-RPC.
  - It needs the bundled binary path, which lives in the installed set
    (`node_modules/@openai/codex…`).
  - It must not race a live codex-acp on the same thread. Hennery closes the
    session first.
  - It inherits `thread/delete`'s refusals: forked-history references, and
    live internal workers.
- **(b) Adapter `session/delete` (archive) plus a host-side file delete.**
  1. Call the adapter's `session/delete`, which archives.
  2. The host then deletes `archived_sessions/rollout-*-<id>{,_*}.jsonl{,.zst}`,
     and the same in `sessions/**`.

  This accepts transcript residue in `thread_history_1.sqlite` and
  `state_5.sqlite`, which the host must not edit while Codex may hold them.
- **(c) Upstream change.** Ask codex-acp to call `thread/delete` from
  `session/delete`, or to offer it as a `_codex/…` extension.

#### Claude

- Call `session/delete`, and treat a not-found error as success.
- Then the host removes these best effort, scanning `<root>/projects/*/` for
  `<id>.*` rather than re-deriving the directory name:
  - `<id>.ccr-tip.json`, `<id>.precompact.json`, `<id>.cast`
  - `<root>/file-history/<id>/`, `<root>/session-env/<id>/`,
    `<root>/tasks/<id>/`, `<root>/debug/<id>.txt`
- If the plan restricts the host to the "otherwise" fallback, all of these
  stay. That matters most for `file-history/<id>/`, which holds copies of the
  files the agent edited.

#### Both agents

- **Validate the id before building any path.** For Claude it is a UUID (the
  SDK enforces `/^[0-9a-f]{8}-…-[0-9a-f]{12}$/i`). For Codex it is a
  `ThreadId` (a UUID in practice; v7 in the operator's files [O]).
- Never delete by a prefix glob without that check.
- **Absence from `session/list` is no proof of deletion.** Archived Codex
  threads disappear from the list too [V].
  - codex-acp's `listSessions` calls `thread/list` with no `archived` field.
  - `ThreadListParams.archived`: "If false or null, only non-archived threads
    are returned."
  - Sources: `codex-acp` `dist/index.js` and Codex `v2/thread.rs`.
- **Deleting a session that never had a turn:**
  - Claude: the SDK throws not-found if no transcript exists [V].
  - Codex: Codex 0.155.1's archive returns "no rollout found for thread id"
    when no rollout exists [V, `thread-store/src/local/archive_thread.rs`].
    codex-acp 1.13.0 passes that error through. It has no not-found swallow;
    2.1.1 added one [V]. So under the pin, deleting a Codex session that never
    got a turn probably errors, and hennery has to swallow it. Whether such a
    thread has a rollout yet is [I].

### Summary

1. **ACP (schema 1.9.1, latest 1.10.2).**
   - `session/delete`, `session/close` and `session/list` are all stable.
     `agent-client-protocol` 2.2.0 exposes them without a feature flag.
   - `session/delete` only promises removal from `session/list`. Soft delete is
     explicitly allowed.
   - `session/close` frees in-memory resources only.
   - There is no archive or forget method.
2. **Claude (adapter 0.81.0, SDK 0.3.280).**
   - The adapter implements `session/delete` through the SDK's
     `deleteSession`. That scans every directory in
     `${CLAUDE_CONFIG_DIR:-~/.claude}/projects/*/` and removes `<id>.jsonl`
     and `<id>/`.
   - It throws when the session is not found.
   - It leaves `file-history/<id>/`, `session-env/<id>/`, `tasks/<id>/`,
     `debug/<id>.txt` and `<id>.ccr-tip.json` / `<id>.precompact.json`.
   - The project directory name is the realpath'd cwd with every
     non-alphanumeric character replaced by `-` (over 200 characters: truncated
     plus a hash). `CLAUDE_CODE_PROJECT_DIR_NAME` overrides it when
     `CLAUDE_CONFIG_DIR` is set.
   - The ACP session id equals the Claude session id, except after a
     clear-context restart, which continues in an untracked new transcript (a
     known leak).
3. **Codex (adapter 1.13.0, Codex 0.155.1).**
   - The adapter's `session/delete` is `thread/archive`, a soft delete that
     moves the rollout to `archived_sessions/`. The latest adapter (2.1.1)
     does the same.
   - The ACP session id equals the Codex thread id.
   - Rollouts live at
     `${CODEX_HOME:-~/.codex}/sessions/YYYY/MM/DD/rollout-<local ts>-<id>[_<rollout>].jsonl[.zst]`.
   - The conversation is **also** stored in `thread_history_1.sqlite`, with
     thread rows in `state_5.sqlite` (under `sqlite_home`, which defaults to
     `CODEX_HOME`).
   - Only the app-server's stable `thread/delete` removes all of it. No ACP or
     extension call reaches it.
4. **Host root.**
   - Adapters inherit the host's environment unchanged for `HOME`,
     `CLAUDE_CONFIG_DIR` and `CODEX_HOME`. Hennery neither strips nor sets
     them.
   - The service environment carries only PATH, so roots can differ between a
     foreground host and the service.
   - Record the roots per session and pass them back through `agent.env` when
     deleting.

---


---

_Generated with Claude AI — please review before distribution._
