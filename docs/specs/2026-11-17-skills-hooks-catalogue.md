# Skills and hooks catalogue

- **Date:** 2026-11-17
- **Status:** proposed, awaiting owner review
- **Evidence:** [spike: delivering hooks and skills](../spikes/2026-11-17-hooks-delivery.md)
  and the earlier [per-session MCP spike](../spikes/2026-09-25-per-session-mcp.md)
- **Builds on:** architecture spec §8.4 and §8.5, kernel spec (hosts, hats,
  step-up), gateway spec §3.4 (local stdio servers, the host × hat pattern)

## 1. Intent

The operator keeps one catalogue of agent skills and hook sets in hennery and
chooses, per item, which hosts receive it. Every session on such a host, in
Claude Code or Codex, then has the skill or hook available, without the
operator installing it by hand on each machine and once per agent.

Today the operator maintains `~/.claude/skills`, `~/.codex/skills` and the two
hook configurations on every machine separately, and they drift. Claude Code
and Codex read the same skill format, so one copy can serve both.

## 2. Goals, non-goals, anti-goals

**Goals**

- One catalogue entry per skill and per hook set, stored on the collector.
- Per-item assignment to hosts. A hat is only a shortcut that selects hosts in
  the UI; assignments are stored per host.
- Delivery that needs no interactive step on the host: no `/hooks` review in
  Codex, no restart of the host.
- The same entry works in Claude Code and Codex sessions started by hennery.
- The operator can see, per host, what was applied and what was refused.

**Non-goals**

- Per-hat isolation. A skill assigned to a host is visible to every hat on it
  (decided 2026-11-17 by the owner; the composed-home mechanism of the
  September spike stays unbuilt).
- Editing skill content inside hennery. v1 takes a finished skill directory.
- Installing from a remote registry (GitHub path, `skills.sh`). Deferred to a
  later revision: it needs the collector's egress policy and content trust
  rules that upload does not.
- Hooks beyond `type: command`. Claude's `prompt`, `agent`, `http` and
  `mcp_tool` handlers and Codex's `mcp_tool` handlers are refused in v1.
- Managing the operator's own skills and hooks that already live in the agent
  directories. hennery never touches a path it did not create.

**Anti-goals**

- Overwriting or deleting anything the operator installed themselves. A name
  collision is reported, never resolved by force.
- Silent delivery failures. A hook set that could not be trusted or a skill
  that could not be linked is shown as such on the host and at session start.
- Weakening the operator permission flow by accident. Hook sets are a stronger
  capability than skills and are treated as such (§8).

## 3. Measured constraints

From the spike, pinned adapters `claude-agent-acp` 0.100.0 and `codex-acp`
2.5.0 (Codex 0.167.0):

| Fact | Consequence |
|---|---|
| Both agents read `<name>/SKILL.md` with `name` and `description`; each ignores the other's extra frontmatter | one skill directory serves both |
| Claude user skills: `$CLAUDE_CONFIG_DIR/skills`, default `~/.claude/skills`; it does not scan `~/.agents/skills` | hennery links into `~/.claude/skills` |
| Codex user skills: `$CODEX_HOME/skills` (also `~/.agents/skills`) | hennery links into `$CODEX_HOME/skills` |
| Both follow symlinked skill folders | the store can live outside the agent directories |
| A Codex hook that is not trusted is skipped silently | trust must be recorded before the session starts |
| Codex trust is `hooks.state."<key>".trusted_hash` in the user `config.toml`; key is `<source path>:<event>:<group>:<handler>`, hash comes from `hooks/list` | the host can record trust itself |
| `codex-acp` reads `CODEX_CONFIG` (JSON) at start; `hooks` inside becomes `-c hooks=…` for the app-server, and the adapter's `_codex/hooks/list` and `_codex/hooks/trust` list and trust them (measured: keys `/<session-flags>/config.toml:<event>:<group>:<handler>`, trust lands in the user `config.toml`, hooks fire) | hooks reach Codex per adapter process, trust is written through the adapter |
| `claude-agent-acp` accepts `_meta.claudeCode.options.settings` with a `hooks` object at `session/new`; nothing is written under `~/.claude`; file hooks and programmatic hooks both fire (measured) | hooks reach Claude per session and add to the operator's own |
| Both hook systems share the JSON shape, the stdin contract, exit code 2 and `hookSpecificOutput` | one hook set renders to both |
| Codex ignores `UserPromptSubmit` and `Stop`, fires `PreToolUse` for `Bash` only, has no skill-level hooks | per-event support is shown in the UI; the Codex rendering drops unsupported events |
| The host already strips `CODEX_CONFIG` from the adapter environment | the host sets it deliberately for Codex sessions with hooks |

## 4. Approaches considered

1. **Symlinks for skills, session channels for hooks (chosen).** A
   hennery-owned store on the host; a symlink per agent skill directory; hooks
   injected per session through the adapter options above. Fewest new
   mechanisms, no files outside the two skill directories, compatible with
   the operator's own `npx skills` layout.
2. **No symlink for Claude.** Claude would get `additionalDirectories`
   pointing at the store (it loads `.claude/skills` from added directories).
   Codex still needs the symlink, so two mechanisms for one feature, and
   sessions the operator starts in a terminal would not see hennery's skills.
   Rejected.
3. **Composed agent homes.** A hennery-owned `CLAUDE_CONFIG_DIR` and
   `CODEX_HOME` per host, as the September spike proposed. Full control but
   it is the isolation mechanism the owner decided not to build, and the
   spike recorded file-divergence risks. Rejected.

## 5. Catalogue model (collector)

Two item kinds, both owner-scoped like every kernel row.

**Skill.** `name` (Agent Skills rules: 1–64 chars, `a-z0-9-`, no leading,
trailing or double hyphen), `description` (read from `SKILL.md`), `archive`
(the directory as a tar.gz, stored content-addressed in the collector's data
directory next to `attachments/`, same 0700/0600 and atomic-write discipline),
`content_hash` (sha256 of the canonical archive), `source` (`upload` in v1),
`created_at`, `updated_at`. Replacing the archive bumps `content_hash`; the
name may not change.

**Hook set.** `name` (same rules), `hooks` (the JSON object, the shape of
`settings.json`'s `hooks`), optional `files` archive for scripts the handlers
call, `content_hash` over both, timestamps.

**Assignment.** `(item_id, host_id)`, unique. Deleting a host's row revokes
the item there. A revoked host keeps its rows (gateway decision E5); the host
removes its copy when it next connects and learns the item is gone, or never
if it never returns.

**Hat shortcut.** The assignment editor offers "hosts of hat X": the hosts
whose default hat is X plus hosts that have run at least one session in X.
This is a UI selection helper; nothing about hats is stored with the
assignment. *(My call: the kernel knows both facts and nothing else about a
hat ties it to a host.)*

**Limits** (refused at upload, before storage):

| | Skill | Hook set |
|---|---|---|
| Archive, uncompressed | 8 MiB | 2 MiB |
| Files | 512 | 128 |
| Single file | 2 MiB | 2 MiB |
| Entries | regular files and directories only; no symlinks, hard links, devices; no `..`; no absolute paths; UTF-8 names | same |
| Required | `SKILL.md` at the root with frontmatter `name` equal to the catalogue name and a non-empty `description` ≤ 1024 chars | `hooks` parses as event → matcher groups → command handlers |
| Per owner | 256 skills, 64 hook sets | |

Executable bits inside `scripts/` are preserved; everything else is written
`0644`. Archive handling never follows a link and extracts into a fresh
directory (the kernel's existing descriptor-walk discipline for deletion
applies to removal).

## 6. Transfer to hosts

Two new capabilities in `Hello`: `Capability::Catalog` (skills, this
section and §7) and `Capability::CatalogHooks` (§8). They are separate so
that a host built for phase 1 cannot silently ignore `hooks` in a
`StartSession`; the collector refuses to deliver hook sets to a host without
the second one and the UI says so per host. A host without the first gets
nothing.

After every handshake, and after every change to an item or an assignment, the
collector sends the host its desired state:

```
CatalogSync { revision, items: [{ id, kind, name, content_hash }] }
```

The host compares with its local registry and, for each item whose hash it
does not hold, requests the archive:

```
CatalogFetch { id, content_hash }           host → collector
CatalogArchive { id, content_hash, bytes }  collector → host, ≤ 256 KiB per frame,
                                            `last: bool`, in order
```

The host applies the set and answers once per sync:

```
CatalogState { revision, applied: [id], failed: [{ id, reason }] }
```

`reason` is a small enum: `conflict` (a path hennery does not own is in the
way), `agent_dir_missing` (no `~/.claude` or `$CODEX_HOME`), `io`, `invalid`.
The collector stores the latest `CatalogState` per host and shows it in the
UI. Syncs are idempotent; a host that reconnects mid-transfer starts again
from the new `CatalogSync`.

Frame sizes stay within the host link's existing bounds; the builder picks the
chunk size against those limits. The transfer is collector-initiated only
because the host never has anything to upload.

## 7. Materialisation on the host

The host keeps a store under its data directory, next to `agent-homes.db`:

```
<data>/catalog/
  skills/<id>/<name>/…        the extracted skill
  hooks/<id>/…                a hook set's files, if any
  catalog.db                  what is applied, with hashes and link targets
```

**Skills.** For each skill the host creates
`~/.claude/skills/<name>` and `$CODEX_HOME/skills/<name>` as symlinks to the
store. The agent directories are resolved as `agent_home::resolve` already
does for forgetting: `CLAUDE_CONFIG_DIR` else `~/.claude`, `CODEX_HOME` else
`~/.codex`, from the host's own environment. If the agent directory does not
exist, the skill is `failed: agent_dir_missing` for that agent and applied for
the other; the state names both.

Conflict rule: if the target path exists and is not a symlink into hennery's
store, the host reports `conflict` and leaves it. Hennery never replaces,
renames or removes a path it did not create. On revocation the host removes
only symlinks that resolve into its store, then the store directory. Updates
extract into a new directory and switch the symlink; a session that already
loaded the old copy keeps working until it restarts. The old directory is
removed at the host's next `skill_sync`, not by the update itself.

Codex also reads `~/.agents/skills`; hennery does not write there, so a skill
the operator installed with `npx skills` and a hennery skill of the same name
collide only in `~/.claude/skills`, where the conflict rule applies.

**Hook sets.** Files are extracted to `hooks/<id>/`. The handler commands may
use `${HENNERY_HOOK_DIR}`; the host substitutes the absolute store path when it
renders the set for a session. Nothing is written into any agent directory for
hooks.

## 8. Hooks at session start

The collector adds the host's assigned hook sets to `StartSession` and
`ResumeSession`, next to `McpDelivery`:

```
hooks: Option<HookDelivery { sets: [{ id, name, content_hash, hooks: JSON }] }>
```

The host merges the sets in catalogue order (event by event, groups appended)
and renders per agent:

- **Claude:** `_meta.claudeCode.options.settings = { "hooks": merged }` on
  `session/new` and `session/load`, beside the existing `extraArgs`.
- **Codex:** the adapter process is spawned with
  `CODEX_CONFIG = {"hooks": merged-without-unsupported}`; events Codex ignores
  and handlers that are not `command` are dropped from that rendering. After
  `initialize` and before `session/new`, the host calls `_codex/hooks/list`,
  then `_codex/hooks/trust` with every `{key, currentHash}` returned, and
  checks that the answer is `trusted: true`. A changed handler changes the
  hash, so every start re-trusts what it sends; stale `hooks.state` entries
  for hashes no longer sent are left to Codex.

**Delivery is fail-closed, like MCP delivery.** If the host cannot apply a
delivered hook set (the trust call answers `trusted: false`, the adapter
lacks the extension methods, a `${HENNERY_HOOK_DIR}` file is missing), the
start fails with a new `start_failed` reason, `hooks_refused { id, reason }`,
the way an MCP delivery the host cannot honour fails today
(`profile::mcp_refusal`). The reason is shown on the session's start card and
in the catalogue's per-host state. *(My call, open for the owner: a hook set
is often a guard rail, and a guard rail that silently did not load is worse
than a session that did not start. The alternative, fail-open with a visible
notice, is a one-line change in the collector's handling of the reason.)*
`SessionStarted` additionally carries `hooks_applied: [{ id, content_hash }]`
so the start card can name what ran.

The operator's own hooks in `~/.claude/settings.json` and `~/.codex` keep
running as they do today; hennery adds to them and never edits them. The only
file hennery changes for hooks is Codex's `config.toml`, through the
adapter's trust call, and only its `hooks.state` table.

**Capability.** A hook set is a command run on every host it reaches, and a
`PreToolUse` handler that answers `allow` bypasses hennery's operator
permission flow. Therefore: saving or assigning a hook set needs step-up
(like a stdio server); the host's Settings page lists the hook sets the host
received; the session start card names them. hennery does not inspect or
filter hook output in v1. *(My call: filtering `allow` would need the host to
proxy every hook, which neither adapter offers; the owner is the only user of
their collector and chooses what to install.)*

## 9. API and UI

Routes under `/api/catalog`, owner-scoped, JSON, with the kernel's existing
step-up middleware on every write:

| Route | Purpose |
|---|---|
| `GET /skills`, `GET /hook-sets` | lists with `content_hash`, assignment count, per-host state summary |
| `POST /skills` (step-up) | create from a base64 tar.gz; name read from `SKILL.md` |
| `PUT /skills/{id}/archive` (step-up) | replace content |
| `DELETE /skills/{id}` (step-up) | delete; hosts remove on next sync |
| `POST /hook-sets`, `PUT /hook-sets/{id}` (step-up) | JSON body with `hooks` and optional base64 files archive |
| `PUT /{kind}/{id}/hosts` (step-up) | replace the assignment set `{ host_ids: [] }` |
| `GET /state?host_id=` | the host's last `CatalogState`, its capability, and the time of its last sync |

Upload follows the hat-logo pattern (base64 JSON, bounded, refused before
storage), not multipart, but with its own per-route body limit: the shared
JSON body cap is 96 KiB, and a skill archive may be 8 MiB. The two upload
routes get a limit of their own, sized to §5 plus base64 overhead; every
other catalogue route keeps the shared cap.

**Screen.** A `/catalog` route, "Skills" in the More menu, behind a
`catalog` feature flag like `/mcp`. Two tabs: Skills, Hook sets. An item
shows name, description, hash prefix, the rendered `SKILL.md` (Markdown
component), and a host checklist with per-host state (applied, pending,
conflict, host offline, host without capability). Above the checklist a
hat selector pre-ticks that hat's hosts (§5). A hook set additionally shows
each event with a Claude and Codex support mark, and its handlers' commands.
Uploading is a file picker that accepts a `.tar.gz`, or a directory where the
browser allows it, packed client-side.

## 10. Host restart, upgrades and purge

- The host reconciles from `catalog.db` at start and again on every
  `CatalogSync`; a missing store entry is re-fetched, a dangling symlink into
  the store is removed.
- A collector downgrade that no longer sends `CatalogSync` leaves the host's
  copy in place; the host does not remove what it cannot confirm is gone.
- Deleting a hat does nothing here. Revoking or deleting a host deletes its
  assignments; the host's files follow on its next connect or stay.
- The live adapter contracts in `hennery-testkit` (where the delegation
  tool contracts live, `delegation_live.rs`) gain two probes: that
  `claude-agent-acp` still accepts `settings.hooks` in `_meta` and runs them
  beside file hooks, and that `codex-acp` still exposes `_codex/hooks/list`
  and `_codex/hooks/trust` with `{key, currentHash}` and `sessionFlags`
  keys. Both are re-run at every pin bump. (`hennery-pins` only generates
  the manifest; it is not the place for these.)

## 11. What is left to the builder

- Exact frame and table names, chunk size and the host-link budget they fit.
- Where the archive bytes live (content-addressed file next to
  `attachments/` is the suggestion, not a requirement).
- How the hat → hosts helper is computed on the client versus served by the
  collector (the sessions table has `host_id` and `hat_id`, so it is one
  query either way).
- Where `CatalogSync` is pushed from: the hub already pushes non-session
  frames to hosts (`SetIdleTimeout`), which is the natural home.
- The client-side packer for directory upload, or `.tar.gz` only if the
  browser APIs are not worth it.
- Test plan: archive validation (zip-slip, links, limits, name rules), host
  materialisation and conflict handling against synthetic agent directories,
  rendering per agent (events dropped for Codex, placeholder substitution),
  and a live acceptance with both pinned adapters that a `SessionStart` hook's
  `additionalContext` reaches the model and that an assigned skill appears in
  the agent's skill list.

## 12. Phasing

1. Catalogue, transfer, skills on hosts, UI without hook sets.
2. Hook sets, session delivery, Codex trust, start-card reporting.
3. Later: install from a GitHub path, behind the egress policy.

### Phase 1 built (2026-11-17)

Phase 1 is implemented. Code uses `skills` rather than "catalog", because
`catalog` already means the runtime configuration catalogue.

- Kernel migration 17 adds `skill_items`, `skill_assignments`,
  `skill_host_states` and `skill_revisions`.
- Frames: `SkillSync`, `SkillArchive`, `SkillFetch`, `SkillState`; capability
  `Capability::Skills`; feature flag `skills`.
- Routes under `/api/skills`: `GET /`, `POST /`, `GET /{id}`,
  `PUT /{id}/archive`, `DELETE /{id}`, `GET|PUT /{id}/hosts`,
  `GET /hat-hosts/{hat_id}`.
- Host store: `<data_dir>/skills/<id>.<hash12>/<name>/`, registry `skills.db`.
- Web: route `/skills`, label "Skills".

Deviations from this design:

- The archive is stored as a BLOB in `skill_items`, not as a file beside
  `attachments/`.
- One frame carries one archive (base64, no chunking).

### Phase 2 built (2026-11-18)

Phase 2 (hook sets, §8) is implemented.

- Kernel migration 18 adds `hook_sets` and `hook_set_assignments`; feature
  flag `hook_sets`; `Capability::HookSets`.
- Routes: `GET|POST /api/hook-sets`, `GET /api/hook-sets/events`,
  `GET|PUT|DELETE /api/hook-sets/{id}`, `GET|PUT /api/hook-sets/{id}/hosts`.
- `StartSession`, `ResumeSession` and `DelegatedAttach` carry
  `hooks: HookDelivery { sets }`. The host acknowledges what it applied in
  `Indexed.hook_sets_applied` on `SessionStarted`; the collector stores it on
  the session and session details show a "Hook sets" row.
- Rendering: Claude gets `_meta.claudeCode.options.settings.hooks` beside the
  unchanged `extraArgs` and `strictMcpConfig`. Codex gets `CODEX_CONFIG` with
  hennery's hooks merged after the operator's own `CODEX_CONFIG` hooks (which
  hennery then trusts too, since they come from the operator's agent command).
  The operator's value is the one the adapter runs with: the `CODEX_CONFIG=`
  argument of an `env(1)` wrapper (`/usr/bin/env CODEX_CONFIG=… <cmd>`, the
  documented form) when there is one, rewritten in place with the merged
  config, else the agent's environment. An `env` wrapper that drops the
  environment (`-i`, `-u CODEX_CONFIG`) and sets no `CODEX_CONFIG` itself is
  refused (`hook_sets_refused`) before spawning.
  Only `type: "command"` handlers are accepted. Codex receives only
  `SessionStart, SessionEnd, PreToolUse, PostToolUse, PermissionRequest,
  PreCompact, PostCompact, SubagentStart, SubagentStop`.
- Fail closed: the collector refuses a start to a host without the capability
  (`hook_sets_unsupported`; the session is marked failed and delegated
  attaches settle with the same code). The host refuses a `Generic` profile
  before spawning (`HostFrame::Error hook_sets_unsupported`), refuses a hooks
  change on a live actor (`hook_sets_conflict`), and fails the start
  (`StartFailed hook_sets_refused`) when Codex's `_codex/hooks/list` does not
  list exactly the delivered handlers (by `<event_snake>:<group>:<handler>`
  position, and at each position a `command` handler with the delivered
  command and matcher; the error names positions only), `_codex/hooks/trust`
  does not answer `trusted: true`, or the adapter lacks the extension.
- Hook sets apply at the next session start; there is no live sync.

Deviations from this design:

- Unsupported is refused pre-spawn with `HostFrame::Error`, not `start_failed`
  as §8 says; `start_failed` (`hook_sets_refused`) is used for refusals after
  the adapter exists.
- No bundled files: commands must name programs already on the host.
- The two hook-set JSON write routes take a body of 64 KiB plus 4 KiB of
  room, not the plan's flat 64 KiB.
- Per-host status for a hook set is `assigned | unsupported | offline`, shown
  on the Hook sets tab and in session details, instead of a host Settings
  listing of received sets.

Limitation: an operator `CODEX_CONFIG` whose own `hooks` use non-command
handlers or extra fields blocks hook-set delivery to that session with
`hook_sets_refused`.

Live probes (§10): `claude_runs_delivered_hooks_beside_file_hooks` and
`codex_trusts_and_runs_delivered_hooks` in
`crates/hennery-testkit/tests/hook_sets_live.rs` are ignored by default. Run
them with `HENNERY_D4_PINNED_ADAPTER_ROOT`, `HENNERY_D4_CLAUDE_AUTH` and
`HENNERY_D4_CODEX_AUTH` set:
`cargo test -p hennery-testkit --test hook_sets_live -- --ignored --nocapture`.
Re-run them at every adapter pin bump (§10). They have not been run against
live adapters yet.
