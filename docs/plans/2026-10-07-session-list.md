# Session list and search (plan 6b) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** The collector serves the session list and searches it, and the host reports what the list shows (ACP core §3.2, §7, §8, §9; frontend §5; umbrella §1.2):
- the `title` extract (from `session_info_update`) and the `commands` extract (from `available_commands_update`), filled by the host, live and when a load replays them;
- the session's title in `sessions.title`, on one line and capped; the commands in `session_catalog.commands`, served by `GET …/catalog` and SSE `catalog_changed`, never by the list;
- `GET /api/sessions?cursor&limit&q&hat&lifecycle`: newest `last_event_at` first, keyset pages, a search over title, cwd, branch and id across every lifecycle; `hat` refused until sessions have hats;
- a list item under 1 KiB whatever the agent reports (P-23), and the detail carrying the same item, with the model and mode (B2b's "model / mode in the list and detail items");
- the `git_state` body: after the start and after each turn, bounded to 3 s, off the session actor; `base_commit` recorded once (6b-ii).

**Architecture:**
- **Wire** (`hennery-proto`):
  - `Indexed.commands` and `Indexed.early` (Task 2);
  - `SessionCatalog.commands` (Task 3);
  - `SessionItem` with its caps and `bounded()`, `SessionDetail` flattening it (Task 4);
  - `SessionPage` (Task 5);
  - `SessionBody::GitState` (Task 6).

  The schema and TypeScript files are regenerated in each of those tasks.
- **Host** (`hennery-host`):
  - `session.rs`: the two extracts in `update()`, early updates marked (Task 2); the git probe's task and `Inbound::Git` (Task 7);
  - `git.rs` (new): `find_git`, the isolated `git` command, `probe` (Task 7);
  - `connection.rs`: `HostConfig.git`, found once at start (Task 7).
- **Collector** (`hennery-sessions`):
  - `store.rs`:
    - migration 8, the one step for every column of this plan (Task 1);
    - fixed-width stamps, and recency moved only by listed events (Task 1);
    - the title, the commands (Task 3), the list item (Task 4), the list (Task 5), the git state (Task 6).
  - `api.rs`: `catalog_changed` from the store, once per replay (Task 3); the detail from the item, the start's checks (Task 4); `GET /api/sessions` (Task 5).
- **Tests:** the store's (`crates/hennery-sessions/tests/store.rs`, `owner.rs` and `store.rs`'s unit tests), the host actor's against the fake adapter (`crates/hennery-testkit/tests/host_session.rs`; the fake gains `prompt_updates`), HTTP and SSE against a scripted host (`reconcile.rs`), the wire's (`crates/hennery-proto/tests/frames.rs`), `git.rs`'s unit tests against a real `git`, and the owner-filter audit (`owner_filter.rs`'s floor for `store.rs`: 77 → 83).

**Tech Stack:** Rust (edition 2024, MSRV 1.88), rusqlite 0.40 (bundled SQLite 3.53.2), tokio, axum 0.8, agent-client-protocol 2.2.0 (schema 1.9.1). No new crates; `Cargo.lock` does not change. `git` ≥ 2.31 on the host for the probe (`rev-parse --path-format=absolute`); with an older one, or none, no `git_state` is sent.

**Spec:** [`docs/specs/2026-09-26-acp-core-design.md`](../specs/2026-09-26-acp-core-design.md), [`docs/specs/2026-09-26-frontend-design.md`](../specs/2026-09-26-frontend-design.md) and the umbrella [`docs/specs/2026-09-25-hennery-architecture-design.md`](../specs/2026-09-25-hennery-architecture-design.md), these sections:
- ACP core §3.2: the `title` and `commands` extracts; "`git_state` | `branch`, `dirty`, `worktree`, `head`, `base_commit?` | After start and after each turn; bounded to 3 s."
- ACP core §7: "Slash commands arrive as `available_commands_update` (passed through, with a `commands` extract); the collector keeps the latest list per session and serves it from the catalogue endpoint, never in the session list." "Git state is reported after start and after every turn, bounded to 3 s; `base_commit` is recorded once per session."
- ACP core §8: `sessions(… title, … git_branch, git_dirty, git_worktree, base_commit, … last_event_at, last_event_id)`, `session_catalog(… commands JSON …)`; "the session list reads only `sessions`"; "A test asserts the serialised list item stays under 1 KiB for a realistic session" (P-23).
- ACP core §9: "`GET /api/sessions?cursor&limit&q&hat&lifecycle` | Paginated list, newest `last_event_at` first. `q` searches title, cwd, branch, id across all sessions regardless of filters except hat." `GET …/catalog`: "commands, plan and usage join it with the plans that produce them." SSE `catalog_changed`.
- Frontend §5: one sort key, `last_event_at` descending; the row's title, branch, host, agent, status and relative time; search over title, cwd, branch and id across all sessions, the volume filters bypassed and only the hat applying (F-10).
- Umbrella §1.2: "Session list sorted by time with day headers; search across all sessions"; "slash commands".

It builds on the executed [passkeys plan 3c](2026-10-05-passkeys.md), [`owner_id` everywhere 3b-iii](2026-10-04-owner-id.md), [session config B2b](2026-09-30-session-config.md) and [permissions (2)](2026-10-01-permissions.md). From B2b's and (2)'s "After this plan" it takes "the rest of the catalogue" (commands; plan and usage stay out) and "`model` / `mode` in the list and detail items". Every anchor below was taken from `main` at `7f779e4`, which merged PR #18. Where the code and a spec disagree, the code wins, and the plan says so.

**Status:** not executed; amended after two security reviews (2026-10-02). The first, on the decisions, approved after amendments A1–A11 (O1, O3, O4, O6 taken); the second, on the written plan, approved after amendments B1–B4 (P1–P5 taken, P6 and P7 recorded) and re-confirmed the amended code at its final commit (see "Decisions", "What the reviews changed"). Both reviews were by a stronger model (Claude Opus) on the maintainer's behalf.

Every code block below was built and tested in a scratch copy of `7f779e4`, two commits per task (the task's tests alone, then the whole task), and generated from those commits. The plan was then replayed from its own text, task by task, onto a fresh copy of `7f779e4`: after each task's Step 1 the tree matched the scratch's tests-only commit, and after the task its task commit, byte for byte, the generated files included (114 blocks; after every task the replay ran fmt, both clippy runs, the codegen check and the workspace tests: 537, 539, 546, 551, 559, 562 and 571 tests, up from 533). The revert-probes (each task's Step 5) were run: 43 probes, every one caught. The timing-sensitive test binaries (`host_session`, `reconcile`) ran with four copies at once, three rounds: one pre-existing test failed once under that load and passed alone 15 times of 15 (see "Not tested here").

## Execution status

Not executed yet.

## Scope

The pieces, from the specs above and the plan 6 brief:
- the `title` and `commands` extracts, filled by the host;
- `session_catalog.commands`; `GET …/catalog` and `catalog_changed` carrying commands;
- the `git_state` session body, after the start and after each turn, bounded to 3 s, `base_commit` recorded once;
- the `sessions` columns `title`, `git_*` and `last_event_*`, in one migration step;
- `GET /api/sessions`, with keyset pagination;
- B2b's model and mode in the list and detail items, which the 1 KiB bound allows once every field is capped (decision 9).

That is **7 tasks in two PRs**, as 3b-ii shipped:
- **6b-i** (Tasks 1–5): the list, search, the title and the commands.
  1. recency and the migration;
  2. the host's title and commands extracts;
  3. the collector's title and commands, and `catalog_changed`;
  4. the list item and the detail;
  5. `GET /api/sessions`.
- **6b-ii** (Tasks 6–7): the git state.
  6. the `git_state` body and the collector's git columns;
  7. the host's git probe.

The split follows the brief: 6b-i alone is a full PR (five tasks, three wire changes), and the probe is the one piece with process and security concerns of its own. Migration 8 ships in 6b-i with the git columns, so 6b-ii needs none.

**In:**
- Wire: `Indexed.commands`, `Indexed.early`, `SessionCatalog.commands`, `SessionItem`, `SessionPage`, `SessionBody::GitState`; `SessionDetail` reshaped around the item.
- Host: the extracts (Task 2); the git probe (Task 7).
- Collector: migration 8, stamps and recency (Task 1); title, commands, `catalog_changed` (Task 3); item, detail, start checks (Task 4); list (Task 5); git columns (Task 6).
- Tests: as in "Architecture", plus the audit's floor for `store.rs` (Tasks 3–6) and a second owner's sessions in the list and search (Task 5).

**Out** (see "After this plan"):
- the list stream `GET /api/stream/sessions` (`session_upsert`);
- `PATCH /api/sessions/{id}`, rename and hat re-assignment (hats 5c);
- the `hat` filter itself and `hat_id` in the item (hats 5c);
- transcript full-text search (out of v1);
- the `text_projection`, `usage` and `plan` extracts;
- the frontend (plan 4).

## Decisions this plan makes where the spec is silent

These were put to a stronger-model security review on the maintainer's behalf twice: the decisions first (2026-10-02, "approve after amendments", every amendment taken below), then the written plan (2026-10-02, "approve after amendments": B1–B4 required, taken; P1–P5 taken, P6 and P7 recorded). Each gives the choice, the alternatives, and the cost if it is wrong. Items marked **(amendment)** depart from explicit spec text and should be written back into it.

**What the reviews changed.** The first (2026-10-02, on the decisions): (2026-10-02, on the decisions):
- A1: the 1 KiB bound covers every field of the item, not only the title: a byte cap per field as JSON writes it, enforced when the item is served (model, mode and failure reason left out past theirs, the cwd shown by its end), and a start refused for an unpaired host or an agent name past 32 bytes (decisions 9, 10; Task 4).
- A2: bidi controls and zero-width characters are dropped from titles and branches (decision 1; Tasks 3, 6).
- A3: a title the adapter sent before the session was announced (replayed by a load) only fills an empty title, so `Indexed` gains `early` (decision 2; Tasks 2, 3).
- A4: the commands extract only from a list the adapter really sent (decision 3; Task 2).
- A5: `last_event_id` never moves back (decision 6; Task 1).
- A6: a replay sends `catalog_changed` once, not one whole catalogue per event (decision 4; Task 3).
- A7: the migration's backfill correlates the owner by hand (migrations are not audited); the audit's floor is the exact count (Tasks 1, 3–6).
- A8: the probe compares absolute git dirs (decision 11; Task 7).
- A9: the probe's hygiene: git's own process group, killed whole; `--ignore-submodules=all`, `-unormal`; changes read only up to the first; `git` an absolute path found once; nothing of git's output in an event (decision 11; Task 7).
- A10: the residual risk is stated: a repository can make `git status` run a clean filter (decision 11).
- A11: a query-plan test and the normalisation cases (Tasks 1, 5).
- Optional hardening taken: O1 (a `git_state` that changes nothing is stored, not listed; Task 6), O3 (`base_commit` only from a new session's start; Task 7), O4 (the cursor validated; Task 5), O6 (`GIT_CEILING_DIRECTORIES=$HOME`; Task 7). Not taken, recorded: O2 (clamping `last_event_at` against clock steps), O5 (cutting titles at grapheme boundaries).

The second (2026-10-02, on the written plan; it found A1–A11, O1, O3, O4 and O6 implemented as claimed):
- B1: a quick first turn no longer aborts the start's probe, which alone records the base commit: a newer probe waits for it instead (decision 11; Task 7, `a_quick_first_turn_still_gets_the_base_commit`).
- B2: git loses what an agent never inherits too (`NESTING_VARS`, `HOST_SECRET_VARS`), so a repository's filter runs with no more than the agent has (decision 11; Task 7).
- B3: decision 11's wording: git's stderr and errors never reach an event; `branch` and `head` ride the event as git printed them, like a title; the column is normalised and capped.
- B4: the frontend isolates the model, mode and failure reason too ("After this plan").
- Optional hardening taken:
  - P1: `bounded()` leaves out a model, mode or failure reason with a control or hidden character, and cuts an older row's agent (32 bytes) and host id (32) (decision 9; Task 4);
  - P2: the 1 KiB test takes a cwd of exactly its cap and a 30-character `created_at` (decision 10; Task 4);
  - P3: `git status` is read up to 64 KiB (decision 11; Task 7);
  - P4: a search with a control character is refused 400 `invalid` (a NUL cut SQLite's pattern short; decision 8; Task 5);
  - P5: `find_git` asks `git --version` once and keeps only 2.31 or newer, saying so otherwise (decision 11; Task 7).
- Recorded, not taken: P6 (the detail's two reads in one transaction), P7 (`PROTOCOL_VERSION` to 1.1: the collector compares majors only, and the parallel lanes would each bump it). Decision 12 now uses `#[expect]`, as the review suggested.

1. **A title is stored on one line and capped, on the collector.** (A1, A2)
   - **Choice:**
     - The host sends what the agent sent: `session_info_update` with a string title → `indexed.title = Some(title)`; with `null` (ACP's "clear") → `Some("")`; without a title → no extract.
     - When the update applies, the collector drops bidi controls and zero-width characters (U+061C, U+200B–200F, U+202A–202E, U+2060–2069, U+FEFF), turns every other control character into a space, collapses whitespace and trims, then cuts the title, never inside a character, to 120 characters and 160 bytes as JSON writes it (`"` and `\` take two). Empty → `title = NULL`. No ellipsis. The event keeps the agent's text.
   - **Why the collector:** it is the boundary the list's 1 KiB bound relies on, whatever host version sends the title. JSON writes a control character as six bytes, so a cap on the raw bytes would not hold.
   - **Alternatives:** the host caps it (an older host would not); a raw byte cap (does not bound the JSON).
   - **Cost if wrong:** a cap is easy to change; titles stored before keep their cut.
2. **Title and commands extracts apply whenever their update does, a load's replay included; an early title only fills an empty one.** (A3)
   - **Choice:** the host extracts both in its stateless `update()`, so they ride live updates, updates sent before `session_started` and updates a `session/load` replays. Updates of the last two kinds are marked `early: true` (they are emitted after `session_started`, in wire order). The collector applies the extracts when the `acp_update` applies (`fact_applies`): commands replace the stored list either way; an early title only fills a `NULL` title (and an early clear clears nothing); a live title always wins.
   - **Why:** both are state kinds a load passes through (ACP core §4.5's `STATE_KINDS`). Commands describe the adapter just started. A title, though, was already ingested live while the session ran; a replay can only repeat it or regress it, and must not undo an operator's rename (5c).
   - **Unlike the catalogue (P-13),** which the host never extracts from early updates: the config is switched during the start, so an early catalogue is older than the one `session_started` announces.
   - **Alternatives:** drop both at load (a resumed session of a database from before this plan would never get its title, nor any adapter that reports commands only at start); apply early titles like live ones (the review's A3).
   - **Cost if wrong:** one rule in `store_state`.
3. **The commands are the full list, stored apart from the config.** (A4)
   - **Choice:** the extract is the crate's `AvailableCommand` objects, re-serialised (the crate skips an entry it cannot read). It is filled only when the raw `update.availableCommands` is a JSON array, and not when every entry of a non-empty array was skipped: the crate reads a missing or malformed list as an empty one, and an empty list is real ("no commands", unlike an empty config read-back). The collector stores it in `session_catalog.commands` (JSON, `NULL` until reported), inserting the row with an empty `config_options` if the session has no catalogue yet, and `SessionCatalog.commands` serves it (empty when none). A commands update never touches `config_options`, `model`, `mode` or `config_axes`, and `Indexed::current_config()` stays `None` for it.
   - **No size cap:** commands are served by the catalogue endpoint only, under the 32 MiB frame cap, and once per replay (decision 4).
   - **Cost if wrong:** the column holds the list as sent; a cap can be added on read.
4. **`catalog_changed` carries the catalogue as it stands when sent, and a replay sends it once.** (A6; **(amendment)** of ACP core §9, which derives it from its event)
   - **Choice:** `catalog_changed` follows every listed event whose extracts carry a config snapshot or the commands, with the event's id, and its data is the `SessionCatalog` read from the store when the message is built, like `pending_changed`. A replay from `Last-Event-ID` sends one, after the last event of the backlog that changed the catalogue, with that event's id. `SessionCatalog::from_indexed` is gone.
   - **Why:** the catalogue now has two parts that change apart. A message built from a config event alone would carry no commands, and one from a commands event no config: the client would wipe the other part. A replay of N such events would otherwise send N whole catalogues (P-23).
   - **Live,** each message is read when the stream task handles the event, so it may already show a later change; the client ends right either way.
   - **Cost if wrong:** contained in `sse_messages` and `stream_session`.
5. **Stamps have one width, so text order is time order.**
   - **Found:** `time`'s RFC 3339 output trims trailing fractional zeros, so `…:05.1Z` sorted after `…:05.12Z`, and `…:05Z` after `…:05.9Z`. `last_event_at` is TEXT.
   - **Choice:** `now()` writes RFC 3339 UTC with exactly three fractional digits, truncated (`2026-10-07T12:34:56.789Z`), for every stamp the store writes. Migration 8 rewrites `sessions.last_event_at` to the same form (`COALESCE(strftime('%Y-%m-%dT%H:%M:%fZ', …), …)`, which keeps a value that is not a time); older events' `ts` stay as written.
   - **Alternatives:** sort by `last_event_id` (the spec and the frontend sort by, and show, `last_event_at`; a session with no event yet has none); six or nine digits (old rows can only be normalised to `strftime`'s three).
   - **Cost if wrong:** ties within a millisecond fall to the id.
6. **Recency moves only with a listed event, and never back.** (A5)
   - **Choice:** `last_event_at` and the new `last_event_id` are set by every collector event and by every host fact that applies, to the stamp and the highest event id written. A fact kept only as the idempotency key (unapplied) and an exact duplicate move neither. A fact's own collector events (`user_turn`, `turn_not_delivered`, `pending_cancelled`) come after it, so the highest id is the last of them. The migration backfills `last_event_id` from the highest applied event.
   - **Was:** every stored fact bumped `last_event_at`, unapplied ones too, so a hidden event moved a session up the list.
   - **Not changed:** `mark_failed` and `mark_failed_if_starting` change a session without an event, so without recency (recorded for the list stream). `presume_parked` writes an event per session, so a host gone to sleep moves its sessions up the list, as the spec has it.
7. **One migration for every column of this plan.** Migration 8 adds `sessions.title`, `git_branch`, `git_dirty`, `git_worktree`, `base_commit`, `last_event_id` and `session_catalog.commands`, normalises recency (decision 5), backfills `last_event_id` (decision 6, correlating `owner_id` by hand: migrations start with `ALTER`, and the audit reads only statements that start with a verb), and adds `sessions_by_recency(owner_id, last_event_at DESC, id DESC)`. The git columns stay `NULL` until 6b-ii.
8. **`GET /api/sessions`.** It answers `SessionPage { sessions: SessionItem[], next_cursor? }`.
   - **Order:** `last_event_at DESC, id DESC`.
   - **Keyset:** one static statement, `(last_event_at, id) < (?2, ?3)`, walking `sessions_by_recency` with no sort of its own (pinned by a query-plan test, A11). With no cursor the bound is a sentinel above every stamp. `next_cursor` is set exactly when a further row exists (`limit + 1` fetched).
   - **Cursor:** opaque, the hex of `<last_event_at>\n<id>` of the page's last row. Anything else is refused 400 `invalid_cursor`: not hex, not UTF-8, no separator, a part empty or past 64 bytes, or past 256 characters (O4). A cursor is a position, so one from another query is harmless.
   - **`limit`:** 50 by default, clamped to 1..=200, like `events`' silent `min(5000)`; not a whole number → 400 `invalid`.
   - **`lifecycle`:** a comma-separated subset of `starting,active,parked,closed,failed`, duplicates and empty entries ignored; an unknown name → 400 `invalid`; absent or empty → every lifecycle. The frontend's "Hide closed" is `lifecycle=starting,active,parked,failed`. A presumed park is `parked`.
   - **`q`:** trimmed; empty is no search; past 200 characters, or with a control character (P4), → 400 `invalid`. A substring of `title`, `cwd`, `git_branch` or `id`, by `LIKE '%…%' ESCAPE '\'`, with `%`, `_` and `\` escaped, so taken literally. SQLite's `LIKE` ignores case for ASCII letters only. While `q` is set, `lifecycle` is ignored (frontend §5, F-10).
   - **`hat`:** refused 400 `hat_filter_unavailable`, present even empty, while sessions have no `hat_id`: never accepted and ignored. The seam is the check at the top of `list_sessions`; plan 5c replaces it (see "After this plan").
   - **Not consistent across pages:** a session that moves above the cursor between two fetches is not on the next page. The list stream covers it (frontend §5's keyed store).
9. **The list item, its caps, and the detail.** (A1)
   - **Item:** `session_id, host_id, agent, cwd, title?, lifecycle, activity?, failure_reason?, presumed_parked, git_branch?, git_dirty?, model?, mode?, created_at, last_event_at`, read from `sessions` alone. The host is its id (the client joins `GET /api/hosts` for the name). `hat_id` joins with hats 5c.
   - **Caps, in bytes as JSON writes each field** (`hennery_proto::rest`): title 160 and 120 characters, branch 120, model 48, mode 48, failure reason 32, agent 32, cwd 128.
     - The title and the branch are stored within theirs (decisions 1, 11).
     - `bounded()`, applied by the list, leaves out a model, mode or failure reason past its cap or holding a control or hidden character (P1; the stored value stays whole: a resume re-applies it), cuts a title or branch past its cap, cuts an older row's agent and host id (32 bytes each: P1), and shows a cwd past its cap by its end after `…` (the search still matches the whole column).
     - `POST /api/sessions` refuses an agent past 32 bytes (400 `invalid`) and a host id no host of the owner's is paired under (400 `unknown_host`), before any session exists: such a start no longer leaves a `failed{host_offline}` row behind. A paired host that is offline or revoked still answers 409 `host_offline` as before. This also closes 3b-iii's "`start_session` stores whatever `host_id` it is given".
   - **Detail:** `SessionDetail` is the item, flattened and as stored (the full cwd and model), plus `open_turn` and `pending`. So the detail gains the title, the git fields, the model and the mode, and `created_at` and `last_event_at`.
10. **The 1 KiB test is worst-case.** (A1, P2) `a_listed_item_stays_under_1_kib_with_every_field_at_its_worst` serialises a bounded item with every field at its cap or past it, each with its worst content (quotes, backslashes, control and four-byte characters), a cwd of exactly its cap (kept whole), a host id and an agent past theirs (cut), and a 30-character `created_at` (a stamp from before decision 5, which the migration does not rewrite), and asserts at most 960 bytes, leaving 64 for the `hat_id` hats add (48 for a 36-character id). It measures 929 bytes.
11. **The git probe (6b-ii).** (A8–A10, O1, O3, O6)
    - **When:** after `session_started` and the updates and notes that follow it, and after every `turn_ended` the actor's main loop emits (a completed, cancelled or failed turn). Not after a teardown, which ends the actor.
    - **How:** `git` runs on a task of its own, so it never holds the actor; the result comes back on the actor's ordered inbound channel (`Inbound::Git`) and is emitted there. So a `git_state` can never be ahead of, or delay, the `turn_ended` it follows. A newer probe replaces and aborts the older, except the start's (which records the base commit): the newer waits for that one first (B1), so the states stay in order. The actor's end aborts whichever runs. 3 s bound for each probe.
    - **What:**
      - `git rev-parse --path-format=absolute --git-dir --git-common-dir`: `worktree` is true when they differ, a linked work tree (A8: relative paths would call the main checkout's subdirectories linked);
      - `git status --porcelain=v2 --branch --ignore-submodules=all -unormal`: `branch` from `branch.head` (`None` when detached), `head` from `branch.oid` (`None` before the first commit); `dirty` at the first line that is not a header, where the probe stops reading; never past 64 KiB (P3).
      - `base_commit` is `head`, on the first probe after a **new** session's start only (O3: a resume's would name a later commit).
    - **Isolation:**
      - `git` is the absolute path found on `PATH` once, when the host starts (`HostConfig.git`), skipping relative entries: a `git` the agent wrote into its cwd never runs; it must answer `git --version` with 2.31 or newer, asked then too, or the host says so at `info` and sends no `git_state` (P5: on macOS, `/usr/bin/git` without the developer tools would otherwise offer to install them at every probe);
      - every `GIT_*` the host inherited is removed, and so is what an agent never inherits (`NESTING_VARS`, `HOST_SECRET_VARS`: B2); `GIT_OPTIONAL_LOCKS=0`, so `git status` never takes `index.lock` under the agent's own git; `GIT_CEILING_DIRECTORIES=$HOME` (if absolute, O6), so a cwd in no repository never scans a home kept in git; `-c core.fsmonitor=false`; stdin and stderr null;
      - git runs in a process group of its own, killed whole (`killpg`) when a probe is given up or replaced, as well as by `kill_on_drop`.
    - **Failure:** git missing or unusable, the cwd in no work tree, git failing or past 3 s: no `git_state`; the stored columns keep their last values. Not an error and not a `host_note`. git's stderr and errors never reach an event; `branch` and `head` ride the event as git printed them, like a title's text, and the collector normalises and caps the column (B3).
    - **Collector:** a `git_state` applies unless the session is closed. `git_branch` is normalised and capped like a title (decision 1's rule, 120 characters and 120 bytes); `git_dirty` and `git_worktree` are overwritten; `base_commit` is set only while `NULL`, and only to 4 to 64 hex digits. A state that changes no column is stored but not listed (O1), so it does not move the session up the list.
    - **Residual risk (A10):** a repository can still make `git status` run a command it configures, a clean filter named in `.git/info/attributes`; no flag turns that off in general. Accepted: the agent working in that cwd can run anything already (umbrella §8.4).
    - **Alternatives:** `worktree` as the work tree's top-level path (longer, and not for the list); one probe per turn on the actor itself (the brief forbids delaying `turn_ended`); `--untracked-files=no` (misses a new file).
12. **`HostFrame` allows `clippy::large_enum_variant`.** `Indexed` grew, and `Session` is now much the largest variant. Boxing its body would add an allocation to nearly every frame, which are almost all `Session`s, to shrink the rare `hello` and `error`; it would also touch 53 construction and match sites.

**Spec amendments:**
- decision 1: ACP core §3.2: `title` is the agent's text; empty means cleared; the collector stores it on one line within 120 characters and 160 JSON bytes;
- decision 2: ACP core §3.2: the `early` extract; `title` and `commands` are extracted from load-replayed and pre-start updates too, an early title only filling an empty one;
- decision 3: ACP core §3.2, §7: `commands` only from an array the adapter sent; an empty list is real;
- decision 4: ACP core §9: `catalog_changed` carries the catalogue as it stands when sent, once per replay; `SessionCatalog` gains `commands`;
- decision 5: ACP core §8: stamps are RFC 3339 UTC with three fractional digits;
- decision 6: ACP core §8: recency (`last_event_at`, `last_event_id`) moves only with a listed event;
- decision 8: ACP core §9: the list's cursor, `limit`, `lifecycle` and `q` rules, and `hat`'s 400 until hats;
- decision 9: ACP core §8, §9: the item's fields and caps; `POST /api/sessions`' 400 `unknown_host` and agent cap; the detail as the item plus the open turn and questions;
- decision 11: ACP core §3.2, §7: `git_state`'s fields, when it is sent, its failure semantics, and the probe's isolation and residual risk;
- ACP core §8's "Built so far": migrations 7 (`owner_id`, 3b-iii) and 8 (this plan).

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`; crate names prefixed `hennery-`.
- After every task these pass:
  - `nix develop -c cargo fmt --all --check` (`max_width = 120`);
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy -p hennery --locked -- -D warnings` (test hooks off);
  - `cargo test --workspace --locked`;
  - `cargo run -p hennery-proto --bin gen -- --check`.
- **No new crates;** `Cargo.lock` does not change. Every command runs `--locked`.
- **Wire types change in Tasks 2, 3, 4, 5 and 6.** The generated files (`schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`) are regenerated there with `cargo run -p hennery-proto --bin gen`; new root types (`SessionItem`, `SessionPage`) go in both of `codegen.rs`' lists. New optional fields follow their struct's existing TS-optional annotations (`#[ts(type = "… | undefined", optional)]`); `Indexed.early`, a `bool` left out when false, is `#[ts(as = "Option<bool>", optional)]` (`early?: boolean`).
- Storage (kernel §1): every table has `owner_id` and every query names it. The audit (`owner_filter.rs`) reads every statement of the files in `SOURCES`; a `{CONST}` in a statement resolves only when the constant is written `const NAME: &str = "…` on one line. Its floor for `store.rs` is raised to the exact count in each task that adds SQL: 80 (Task 3), 81 (Task 4), 82 (Task 5), 83 (Task 6).
- **No Linux-only code,** and nothing new that is platform-specific: `process_group` and `killpg` are POSIX, as the adapter supervisor already uses them. CI runs `ubuntu-latest` and `macos-latest`; only macOS ran here.
- **Tests that run `git`** skip, saying so, when `find_git()` finds none on `PATH`. CI's runners have one.
- No global installs: tooling comes from the flake dev shell (which builds no package, so no test runs in a sandbox without `git`).
- **The probe's git** is checked once per `HostConfig::new` (`git --version`); `SessionOptions::default()` has no git, so actor tests probe only when they ask to.
- Commits follow Conventional Commits and use the repository's own identity (gmail, unsigned). Push the feature branch after every completed task; never push `main`.

## Review Focus

These are the inputs most likely to bite a real user that the obvious tests would not exercise, most likely first. Each is pinned by the named tests.

1. **An agent's title or branch built to break the list** (P-23).
   - Expected: on one line, without bidi or zero-width characters, within its caps; the whole item under 1 KiB with room for `hat_id`, whatever every field holds; a model, mode or failure reason past its cap left out of the list but kept for a resume.
   - Tests: Task 3 `a_title_is_stored_on_one_line_and_capped_and_an_empty_one_clears_it`, `one_line_collapses_whitespace_and_cuts_to_the_caps`; Task 4 `a_listed_item_stays_under_1_kib_with_every_field_at_its_worst`, `a_listed_item_is_bounded_field_by_field`, `a_start_names_a_paired_host_and_an_agent_of_at_most_32_bytes`; Task 5 `the_list_serves_items_bounded`; Task 6 `a_git_state_fills_the_git_columns_and_records_the_base_commit_once`.
2. **Searching for a closed session, with `%`, `_` or `\` in the query.**
   - Expected: found across every lifecycle while `q` is set, the wildcards literal, ASCII case ignored; another owner's never.
   - Tests: Task 5 `search_matches_title_cwd_branch_and_id_literally_across_every_lifecycle`, `a_search_escapes_like_wildcards`, `the_session_list_pages_searches_filters_and_refuses_what_it_cannot_honour`, `another_owners_sessions_are_invisible_to_the_store`.
3. **Paging while sessions move.**
   - Expected: newest first, ties by id, the next page unshifted by a session moving to the top; the statement walks the index; a forged cursor refused.
   - Tests: Task 5 `the_list_is_newest_first_and_pages_by_keyset`, `the_list_walks_the_recency_index`, `a_cursor_round_trips_and_a_malformed_one_is_refused`; Task 1 `stamps_have_one_width_so_text_order_is_time_order`, `the_session_list_migration_normalises_recency_on_an_older_database`.
4. **A resume that replays an old title, or commands.**
   - Expected: the stored title kept (an empty one filled), the commands replaced; commands never wiping the config, nor the config the commands, live or on replay; one catalogue per replay.
   - Tests: Task 2 `a_resume_passes_the_replayed_title_and_commands_through_with_their_extracts`, `live_state_updates_carry_the_title_and_the_commands`; Task 3 `an_early_title_only_fills_an_empty_one`, `commands_replace_the_stored_list_and_never_touch_the_config`, `live_catalog_changed_carries_the_whole_catalogue_commands_included`, `a_replay_sends_the_catalogue_once_after_the_last_event_that_changed_it`.
5. **A slow, hostile or absent `git`.**
   - Expected: a turn's end never waits for it; a hung git and whatever it started are killed when replaced or past 3 s; no state outside a work tree or without git; the host's `GIT_*`, and what an agent never inherits, never reach it; no `index.lock`; a relative `git` never runs; a linked work tree told from the main checkout's subdirectories.
   - Tests: Task 7 `a_quick_first_turn_still_gets_the_base_commit`, `only_git_2_31_or_newer_is_used`, `a_hung_git_never_delays_a_turn_end_and_is_killed_with_its_group`, `no_git_state_is_reported_outside_a_work_tree_or_without_git`, `a_command_is_isolated_from_the_hosts_git_environment`, `git_is_found_on_absolute_path_entries_only`, `a_probe_reports_the_branch_changes_and_linked_work_trees`, `the_git_state_follows_the_start_and_every_turn_end`.
6. **Hidden events moving sessions.**
   - Expected: an unapplied fact, a duplicate and a `git_state` that changes nothing move no session up the list; recency never goes back.
   - Tests: Task 1 `recency_moves_only_with_a_listed_event`, `a_fact_that_writes_collector_events_leaves_recency_at_the_last_of_them`; Task 6 as in 1.
7. **A `hat` filter** before hats exist.
   - Expected: 400 `hat_filter_unavailable`, even empty; never ignored.
   - Tests: Task 5 `the_session_list_pages_searches_filters_and_refuses_what_it_cannot_honour`.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-proto/src/frames.rs` | `Indexed.commands`, `Indexed.early`, `HostFrame`'s clippy allow; `SessionBody::GitState` | 2, 6 |
| `crates/hennery-proto/src/rest.rs`, `codegen.rs`, generated files | `SessionCatalog.commands`; `SessionItem`, its caps and `bounded()`, `json_width`; `SessionDetail`; `SessionPage` | 3, 4, 5 (generated also 2, 6) |
| `crates/hennery-host/src/session.rs` | `update()`'s extracts, `early_update`; `SessionOptions.git`, `probe_git`, `Inbound::Git` | 2, 7 |
| `crates/hennery-host/src/git.rs` (new), `lib.rs` | The probe | 7 |
| `crates/hennery-host/src/connection.rs` | `HostConfig.git` | 7 |
| `crates/hennery-sessions/src/store.rs` | Migration 8, `stamp`, recency; `one_line`, `store_state`, the catalogue's commands; `session_item`; `Cursor`, `ListQuery`, `list`; the `git_state` arm | 1, 3, 4, 5, 6 |
| `crates/hennery-sessions/src/api.rs` | `catalog_changed`; the detail and the start's checks; `GET /api/sessions` | 3, 4, 5 |
| `crates/hennery-testkit/src/lib.rs`, `src/bin/hennery-fake-acp.rs` | `FakeScript::prompt_updates` | 2 |
| Tests: `crates/hennery-sessions/tests/{store,owner}.rs`, `crates/hennery-testkit/tests/{host_session,reconcile,owner_filter}.rs`, `crates/hennery-proto/tests/frames.rs`, unit tests in `store.rs` and `git.rs` | | all |

All commands run from the repository root inside the dev shell (`nix develop -c …`, or direnv). Work on a feature branch off `main` (`plan/session-list`). Each task leaves the workspace compiling, clippy-clean and green, and the binary working: `up` serves through every task.

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block (and a final newline).
- "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point, as whole lines (earlier blocks of the same task already applied, in order), then "with:" and its replacement.

"Run: `cargo run -p hennery-proto --bin gen`" regenerates the protocol files; it changes no other file. Other "Run:" lines only check. The plan was replayed exactly this way, from its own text, onto `7f779e4`.

---

## 6b-i: the session list, search, the title and the commands (PR 1)

### Task 1: Recency, and the migration

**Files:**
- Modify: `crates/hennery-sessions/src/store.rs` (migration 8, `stamp`, `SessionRow`'s recency, `collector_event`, `ingest`)
- Test: `crates/hennery-sessions/tests/store.rs`, `tests/owner.rs` (its schema rollback undoes migration 8 first), `store.rs`'s unit tests

**Interfaces:**
- Produces: `SessionRow.last_event_at: String`, `SessionRow.last_event_id: Option<i64>`; store migration 8 (decision 7); `stamp(OffsetDateTime) -> String` (private).
- Consumes: nothing new.

- [ ] **Step 1: Write the failing tests**

The recency tests drive `ingest` with an applied fact, a duplicate, an unapplied end and a fact that writes collector events. The migration test builds a database at version 7 with odd stamps and an unapplied event. Two older tests roll a database back to an earlier schema by dropping columns; SQLite will not drop a column an index names, so they undo migration 8 first.

In `crates/hennery-sessions/src/store.rs`, replace:

```rust

    /// The kernel's first two migrations, as 3b-ii shipped them.
```

with:

```rust

    /// Plan 6b decision 5: every stamp has the same width, so comparing
    /// them as text compares the times, within a second too.
    #[test]
    fn stamps_have_one_width_so_text_order_is_time_order() {
        let at = |nanos: i64| {
            super::stamp(
                time::OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap() + time::Duration::nanoseconds(nanos),
            )
        };
        let stamps = [
            at(0),
            at(100_000_000),
            at(120_000_000),
            at(999_999_999),
            at(1_000_000_000),
        ];
        assert_eq!(stamps[0], "2027-01-15T08:00:00.000Z");
        assert_eq!(stamps[1], "2027-01-15T08:00:00.100Z");
        assert_eq!(stamps[3], "2027-01-15T08:00:00.999Z");
        assert!(stamps.iter().all(|s| s.len() == 24), "{stamps:?}");
        assert!(stamps.windows(2).all(|w| w[0] < w[1]), "{stamps:?}");
    }

    /// Plan 6b's migration on a database from before it: `last_event_at`
    /// rewritten to the fixed width (an unreadable value left alone),
    /// `last_event_id` from the session's last listed event, and the list's
    /// index in place.
    #[test]
    fn the_session_list_migration_normalises_recency_on_an_older_database() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        {
            let mut conn = hennery_kernel::db::open(&db).unwrap();
            let owner = hennery_kernel::db::kernel_owner(&mut conn).unwrap();
            hennery_kernel::db::migrate(&mut conn, &MIGRATIONS[..7]).unwrap();
            conn.execute_batch(&format!(
                "
                INSERT INTO sessions(id, host_id, agent, cwd, lifecycle, created_at, last_event_at, owner_id) VALUES
                    ('s1', 'h1', 'fake', '/tmp', 'active', 't', '2026-10-01T10:00:05.12Z', '{owner}'),
                    ('s2', 'h1', 'fake', '/tmp', 'active', 't', '2026-10-01T10:00:05Z', '{owner}'),
                    ('s3', 'h1', 'fake', '/tmp', 'active', 't', 't', '{owner}');
                INSERT INTO events(session_id, host_seq, kind, body, ts, applied, owner_id) VALUES
                    ('s1', 1, 'session_started', '{{}}', 't', 1, '{owner}'),
                    ('s1', 2, 'acp_update', '{{}}', 't', 1, '{owner}'),
                    ('s1', 3, 'turn_ended', '{{}}', 't', 0, '{owner}');
                "
            ))
            .unwrap();
        }
        Store::open(&db).unwrap();
        let conn = Connection::open(&db).unwrap();
        let mut stmt = conn
            .prepare("SELECT id, last_event_at, last_event_id FROM sessions ORDER BY id")
            .unwrap();
        let rows: Vec<(String, String, Option<i64>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            rows,
            [
                ("s1".into(), "2026-10-01T10:00:05.120Z".into(), Some(2)),
                ("s2".into(), "2026-10-01T10:00:05.000Z".into(), None),
                ("s3".into(), "t".into(), None),
            ]
        );
        let index: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'sessions_by_recency'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(index, 1);
    }

    /// The kernel's first two migrations, as 3b-ii shipped them.
```

In `crates/hennery-sessions/tests/owner.rs`, replace:

```rust
        // Back to the store's schema before this plan (version 6).
```

with:

```rust
        // Back to the store's schema before this plan (version 6): plan
        // 6b's migration (version 8) undone first, its index on `owner_id`
        // included.
        conn.execute_batch(
            "
            DROP INDEX sessions_by_recency;
            ALTER TABLE sessions DROP COLUMN title;
            ALTER TABLE sessions DROP COLUMN git_branch;
            ALTER TABLE sessions DROP COLUMN git_dirty;
            ALTER TABLE sessions DROP COLUMN git_worktree;
            ALTER TABLE sessions DROP COLUMN base_commit;
            ALTER TABLE sessions DROP COLUMN last_event_id;
            ALTER TABLE session_catalog DROP COLUMN commands;
            ",
        )
        .unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
            "ALTER TABLE turns DROP COLUMN state;
```

with:

```rust
            "DROP INDEX sessions_by_recency;
             ALTER TABLE sessions DROP COLUMN title;
             ALTER TABLE sessions DROP COLUMN git_branch;
             ALTER TABLE sessions DROP COLUMN git_dirty;
             ALTER TABLE sessions DROP COLUMN git_worktree;
             ALTER TABLE sessions DROP COLUMN base_commit;
             ALTER TABLE sessions DROP COLUMN last_event_id;
             ALTER TABLE turns DROP COLUMN state;
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    assert_eq!(store.pending_item("p1").unwrap().unwrap().delivered, Some(false));
}

```

with:

```rust
    assert_eq!(store.pending_item("p1").unwrap().unwrap().delivered, Some(false));
}

// Plan 6b: a session's recency (the list's sort key, ACP core §9) moves
// only with an event the timeline lists (decision 6).

fn recency(store: &Store) -> (String, Option<i64>) {
    let s = store.session("s1").unwrap().unwrap();
    (s.last_event_at, s.last_event_id)
}

#[test]
fn recency_moves_only_with_a_listed_event() {
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
    let (created_at, none) = recency(&store);
    assert_eq!(none, None);
    let first = store
        .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
        .unwrap();
    let after_start = recency(&store);
    assert_eq!(after_start, (first[0].ts.clone(), Some(first[0].event_id)));
    assert!(after_start.0 >= created_at);
    // Later than any stamp so far, so a wrongly moved recency would show.
    std::thread::sleep(std::time::Duration::from_millis(5));
    // An exact duplicate, and a fact that does not apply (an end for a turn
    // that is not open), move neither.
    assert!(
        store
            .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
            .unwrap()
            .is_empty()
    );
    assert!(store.ingest("s1", 2, &ended("t-none")).unwrap().is_empty());
    assert_eq!(recency(&store), after_start);
    // A listed update moves both.
    let listed = store.ingest("s1", 3, &update(1)).unwrap();
    assert_eq!(recency(&store), (listed[0].ts.clone(), Some(listed[0].event_id)));
    assert!(listed[0].ts > after_start.0);
}

#[test]
fn a_fact_that_writes_collector_events_leaves_recency_at_the_last_of_them() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    let created = store.ingest("s1", 2, &turn_started("t1")).unwrap();
    assert_eq!(kinds(&created), ["turn_started", "user_turn"]);
    assert!(created[1].event_id > created[0].event_id);
    assert_eq!(recency(&store).1, Some(created[1].event_id));
    // A collector event of its own moves it too.
    let parked = store.record_park_request("s1").unwrap();
    assert_eq!(recency(&store), (parked.ts.clone(), Some(parked.event_id)));
}

```

- [ ] **Step 2: Run the tests to see them fail**

Run: `nix develop -c cargo test -p hennery-sessions --locked`
Expected: FAIL to compile: ``error[E0425]: cannot find function `stamp` in module `super` ``.

- [ ] **Step 3: The migration, the stamps and recency**

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    UPDATE answer_queue SET owner_id = (SELECT id FROM owners ORDER BY created_at, id LIMIT 1);
",
];
```

with:

```rust
    UPDATE answer_queue SET owner_id = (SELECT id FROM owners ORDER BY created_at, id LIMIT 1);
",
    // The session list and its extracts (plan 6b, ACP core §8): the title
    // and the git state the host reports, the session's recency (the list's
    // sort key: the time and id of its last listed event), and the latest
    // slash commands, off the list (P-23). Recency is rewritten to one
    // fixed width, so text order is time order (decision 5); a value that
    // is not a time is left as it is. The git columns are filled from
    // `git_state` (6b-ii).
    "
    ALTER TABLE sessions ADD COLUMN title TEXT;
    ALTER TABLE sessions ADD COLUMN git_branch TEXT;
    ALTER TABLE sessions ADD COLUMN git_dirty INTEGER;
    ALTER TABLE sessions ADD COLUMN git_worktree INTEGER;
    ALTER TABLE sessions ADD COLUMN base_commit TEXT;
    ALTER TABLE sessions ADD COLUMN last_event_id INTEGER;
    ALTER TABLE session_catalog ADD COLUMN commands TEXT;
    UPDATE sessions SET last_event_at = COALESCE(strftime('%Y-%m-%dT%H:%M:%fZ', last_event_at), last_event_at);
    UPDATE sessions SET last_event_id = (
        SELECT MAX(e.event_id) FROM events e
        WHERE e.session_id = sessions.id AND e.applied = 1 AND e.owner_id = sessions.owner_id);
    CREATE INDEX sessions_by_recency ON sessions(owner_id, last_event_at DESC, id DESC);
",
];
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    pub config: SessionConfig,
```

with:

```rust
    pub config: SessionConfig,
    /// When the session's last listed event was written (`stamp`), or its
    /// creation; the session list's sort key.
    pub last_event_at: String,
    /// That event's id; `None` until the session has one.
    pub last_event_id: Option<i64>,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC 3339 formatting of the current time")
```

with:

```rust
/// `at` as RFC 3339 UTC with exactly three fractional digits
/// (`2026-10-07T12:34:56.789Z`, truncated to the millisecond). Every stamp
/// has the same width, so comparing two as text compares the times: the
/// session list sorts on them (plan 6b decision 5). `time`'s own RFC 3339
/// output trims trailing zeros, so `…:05.1Z` would sort after `…:05.12Z`.
fn stamp(at: time::OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
        at.millisecond()
    )
}

fn now() -> String {
    stamp(time::OffsetDateTime::now_utc())
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    tx.execute(
        "UPDATE sessions SET last_event_at = ?2 WHERE id = ?1 AND owner_id = ?3",
        params![session_id, ts, owner],
    )?;
    Ok(EventDto {
        event_id: tx.last_insert_rowid(),
```

with:

```rust
    let event_id = tx.last_insert_rowid();
    tx.execute(
        "UPDATE sessions SET last_event_at = ?2, last_event_id = ?3 WHERE id = ?1 AND owner_id = ?4",
        params![session_id, ts, event_id, owner],
    )?;
    Ok(EventDto {
        event_id,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
                        presumed_parked, model, mode, config_axes
```

with:

```rust
                        presumed_parked, model, mode, config_axes, last_event_at, last_event_id
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
                        config: SessionConfig::default(),
```

with:

```rust
                        config: SessionConfig::default(),
                        last_event_at: r.get(13)?,
                        last_event_id: r.get(14)?,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
        tx.execute(
            "UPDATE sessions SET last_event_at = ?2 WHERE id = ?1 AND owner_id = ?3",
            params![session_id, ts, self.owner],
        )?;
```

with:

```rust
        // Only a listed event moves the session's recency (plan 6b decision
        // 6): a fact kept just as the idempotency key is hidden from the
        // timeline, so it must not move the session up the list either.
        // The fact's collector events come after it, so the last of them
        // is the session's last event.
        if let Some(last) = created.iter().map(|e| e.event_id).max() {
            tx.execute(
                "UPDATE sessions SET last_event_at = ?2, last_event_id = ?3 WHERE id = ?1 AND owner_id = ?4",
                params![session_id, ts, last, self.owner],
            )?;
        }
```

- [ ] **Step 4: Run the tests to see them pass, then the checks**

Run: `nix develop -c cargo test -p hennery-sessions --locked`
Expected: PASS (`store`: 71 passed). Then the five checks of "Global Constraints": 537 tests in the workspace.

- [ ] **Step 5: Revert-probes**

Each probe below was applied to the task's commit, its test run, and the change undone. Every one was caught (5 of 5):
- recency moved by every stored fact again, as before (`store.rs`): `recency_moves_only_with_a_listed_event` fails.
- `last_event_id` set to the fact's own id, behind its collector events (`store.rs`): `a_fact_that_writes_collector_events_leaves_recency_at_the_last_of_them` fails.
- stamps back to `time`'s RFC 3339, which trims zeros (`store.rs`): `stamps_have_one_width` fails.
- the migration without the normalisation (`store.rs`): `the_session_list_migration_normalises` fails.
- the backfill counting unapplied events (`store.rs`): `the_session_list_migration_normalises` fails.

- [ ] **Step 6: Commit**

`git -c commit.gpgsign=false commit -m "feat(sessions): keep recency to listed events, in fixed-width stamps"`

---

### Task 2: The host's title and commands extracts

**Files:**
- Modify: `crates/hennery-proto/src/frames.rs` (`Indexed.commands`, `Indexed.early`; `HostFrame`'s allow), `crates/hennery-host/src/session.rs` (`update`, `early_update`, `state_extracts`), generated files
- Modify (test support): `crates/hennery-testkit/src/lib.rs`, `src/bin/hennery-fake-acp.rs` (`prompt_updates`)
- Test: `crates/hennery-testkit/tests/host_session.rs` (two new tests; two older ones now expect `early` on a replayed or pre-start update)

**Interfaces:**
- Produces: `Indexed.commands: Option<Vec<Value>>`, `Indexed.early: bool` (decisions 2, 3); `FakeScript::prompt_updates: Vec<Value>`.
- Consumes: nothing new.

- [ ] **Step 1: Write the failing tests**

The fake adapter gains `prompt_updates`: raw updates sent at the start of every prompt, untyped like `replay`. The tests read the extracts of the frames the actor writes.

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

```rust
                    let mut cancelled = cancel.subscribe();
```

with:

```rust
                    let mut cancelled = cancel.subscribe();
                    for update in &script.prompt_updates {
                        cx.send_notification(UntypedMessage::new(
                            "session/update",
                            serde_json::json!({ "sessionId": req.session_id, "update": update }),
                        )?)?;
                    }
```

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust
    pub withdraw_asks: bool,
```

with:

```rust
    pub withdraw_asks: bool,
    /// Raw ACP `update` objects streamed as `session/update` at the start of
    /// every prompt, in order, before its asks and chunks. Sent untyped,
    /// like `replay`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompt_updates: Vec<serde_json::Value>,
```

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust
            withdraw_asks: false,
```

with:

```rust
            withdraw_asks: false,
            prompt_updates: Vec::new(),
```

In `crates/hennery-testkit/tests/host_session.rs`, replace:

```rust
        assert_eq!(*indexed, Indexed::default(), "{attach:?}");
```

with:

```rust
        // Marked early (plan 6b decision 2), and no catalogue.
        assert_eq!(
            *indexed,
            Indexed {
                early: true,
                ..Indexed::default()
            },
            "{attach:?}"
        );
```

In `crates/hennery-testkit/tests/host_session.rs`, replace:

```rust
    assert_eq!(replayed, Indexed::default(), "a replayed update carried a catalogue");
```

with:

```rust
    // Marked early (plan 6b decision 2), and nothing else.
    assert_eq!(
        replayed,
        Indexed {
            early: true,
            ..Indexed::default()
        },
        "a replayed update carried a catalogue"
    );
```

In `crates/hennery-testkit/tests/host_session.rs`, replace:

```rust
    let pending = opened(&frames)[0].0.id.clone();
    assert!(handle.send(choose("ra", &pending, "allow")));
    let frames = wait_until(&uplink, |f| !verdicts(f).is_empty()).await;
    assert_eq!(verdicts(&frames), [("ra".to_string(), pending, false)]);
}

```

with:

```rust
    let pending = opened(&frames)[0].0.id.clone();
    assert!(handle.send(choose("ra", &pending, "allow")));
    let frames = wait_until(&uplink, |f| !verdicts(f).is_empty()).await;
    assert_eq!(verdicts(&frames), [("ra".to_string(), pending, false)]);
}

// Plan 6b: the `title` and `commands` extracts (ACP core §3.2, §7).

/// What one `acp_update` frame's extracts say of the adapter's state.
type StateExtracts = (Option<String>, Option<Vec<serde_json::Value>>, Option<String>, bool);

/// The `title`, `commands`, `turn_id` and `early` extracts of every
/// `acp_update` frame of one `sessionUpdate` kind.
fn state_extracts(frames: &[HostFrame], kind: &str) -> Vec<StateExtracts> {
    frames
        .iter()
        .filter_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::AcpUpdate { indexed, payload },
                ..
            } if payload["update"]["sessionUpdate"] == kind => Some((
                indexed.title.clone(),
                indexed.commands.clone(),
                indexed.turn_id.clone(),
                indexed.early,
            )),
            _ => None,
        })
        .collect()
}

/// A live `session_info_update` carries its title (`null` clears it: an
/// empty title; no title at all: no extract), and a live
/// `available_commands_update` the full list of the commands the crate
/// can read, but only when the adapter sent a list: one that is missing or
/// not an array, or whose every entry is unreadable, carries none (the
/// crate would read each as an empty list, which would wipe the stored
/// commands). Neither carries the other's extract or the catalogue's.
#[tokio::test]
async fn live_state_updates_carry_the_title_and_the_commands() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        prompt_updates: vec![
            json!({"sessionUpdate": "session_info_update", "title": "Fix the login bug"}),
            json!({"sessionUpdate": "available_commands_update", "availableCommands": [
                {"name": "review", "description": "Review the diff"},
                {"name": 7, "description": "not a command"},
                {"name": "plan", "description": "Plan it", "input": {"hint": "what to plan"}}
            ]}),
            json!({"sessionUpdate": "session_info_update", "title": null}),
            json!({"sessionUpdate": "session_info_update", "updatedAt": "2026-10-07T12:00:00Z"}),
            json!({"sessionUpdate": "available_commands_update", "availableCommands": []}),
            json!({"sessionUpdate": "available_commands_update"}),
            json!({"sessionUpdate": "available_commands_update", "availableCommands": "review"}),
            json!({"sessionUpdate": "available_commands_update", "availableCommands": [{"name": 7}]}),
        ],
        ..FakeScript::default()
    };
    let handle = session::start(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&script),
        std::env::temp_dir(),
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    let frames = wait_until(&uplink, has("turn_ended")).await;
    let t1 = Some("t1".to_string());
    assert_eq!(
        state_extracts(&frames, "session_info_update"),
        [
            (Some("Fix the login bug".into()), None, t1.clone(), false),
            (Some(String::new()), None, t1.clone(), false),
            (None, None, t1.clone(), false),
        ]
    );
    assert_eq!(
        state_extracts(&frames, "available_commands_update"),
        [
            (
                None,
                Some(vec![
                    json!({"name": "review", "description": "Review the diff"}),
                    json!({"name": "plan", "description": "Plan it", "input": {"hint": "what to plan"}}),
                ]),
                t1.clone(),
                false
            ),
            (None, Some(vec![]), t1.clone(), false),
            (None, None, t1.clone(), false),
            (None, None, t1.clone(), false),
            (None, None, t1.clone(), false),
        ]
    );
    for frame in &frames {
        if let HostFrame::Session {
            body: SessionBody::AcpUpdate { indexed, .. },
            ..
        } = frame
        {
            assert!(indexed.current_config().is_none(), "{indexed:?}");
        }
    }
}

/// Plan 6b decision 2: a title and commands replayed by `session/load` are
/// passed through with their extracts, outside any turn, unlike the
/// catalogue (P-13), and marked `early`: the collector lets such a title
/// only fill an empty one.
#[tokio::test]
async fn a_resume_passes_the_replayed_title_and_commands_through_with_their_extracts() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        replay: vec![
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "old answer"}}),
            json!({"sessionUpdate": "session_info_update", "title": "Old title"}),
            json!({"sessionUpdate": "available_commands_update", "availableCommands": [
                {"name": "review", "description": "Review the diff"}
            ]}),
        ],
        ..FakeScript::default()
    };
    let _handle = resuming(&uplink, &script);
    let frames = wait_until(&uplink, |frames| updates_of(frames).len() == 2).await;
    assert_eq!(
        state_extracts(&frames, "session_info_update"),
        [(Some("Old title".into()), None, None, true)]
    );
    assert_eq!(
        state_extracts(&frames, "available_commands_update"),
        [(
            None,
            Some(vec![json!({"name": "review", "description": "Review the diff"})]),
            None,
            true
        )]
    );
}

```

- [ ] **Step 2: Run the tests to see them fail**

Run: `nix develop -c cargo test -p hennery-testkit --locked --test host_session -- state`
Expected: FAIL to compile: ``struct `Indexed` has no field named `early` ``, ``no field `commands` on type `&Indexed` ``.

- [ ] **Step 3: The extracts**

In `crates/hennery-host/src/session.rs`, replace:

```rust
use crate::uplink::Uplink;
use agent_client_protocol::schema::ProtocolVersion;
```

with:

```rust
use crate::uplink::Uplink;
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, ErrorCode, Responder, UntypedMessage};
```

with:

```rust
};
use agent_client_protocol::schema::{MaybeUndefined, ProtocolVersion};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, ErrorCode, Responder, UntypedMessage};
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                Early::Update(payload) => self.emit(update(payload, None)),
```

with:

```rust
                Early::Update(payload) => self.emit(early_update(payload)),
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
fn update(payload: Value, turn: Option<&str>) -> SessionBody {
    SessionBody::AcpUpdate {
        indexed: Indexed {
            turn_id: turn.map(str::to_string),
            ..Indexed::default()
        },
        payload,
```

with:

```rust
///
/// A `session_info_update`'s title and an `available_commands_update`'s
/// commands are extracted here, wherever the update comes from: live, sent
/// before the start was announced, or replayed by `session/load`. Unlike
/// the catalogue (P-13), they are the adapter's current state, and they are
/// emitted in wire order, so the latest wins (plan 6b decision 2).
fn update(payload: Value, turn: Option<&str>) -> SessionBody {
    let (title, commands) = state_extracts(&payload);
    SessionBody::AcpUpdate {
        indexed: Indexed {
            turn_id: turn.map(str::to_string),
            title,
            commands,
            ..Indexed::default()
        },
        payload,
    }
}

/// An update the adapter sent before `session_started`, emitted after it:
/// replayed by a load, or sent while the start ran. Marked `early`, so the
/// collector lets its title only fill an empty one (plan 6b decision 2).
fn early_update(payload: Value) -> SessionBody {
    let mut body = update(payload, None);
    if let SessionBody::AcpUpdate { indexed, .. } = &mut body {
        indexed.early = true;
    }
    body
}

/// The `title` and `commands` extracts (ACP core §3.2) of a
/// `session_info_update` or an `available_commands_update`. A title of
/// `null` clears it, and is sent as an empty title; an update without one
/// changes nothing. Commands are extracted only from a list the adapter
/// sent: the crate reads a missing or malformed one, and one whose every
/// entry it skips, as an empty list, which would wipe the stored commands.
fn state_extracts(payload: &Value) -> (Option<String>, Option<Vec<Value>>) {
    if !matches!(
        payload["update"]["sessionUpdate"].as_str(),
        Some("session_info_update" | "available_commands_update")
    ) {
        return (None, None);
    }
    let Ok(notification) = serde_json::from_value::<SessionNotification>(payload.clone()) else {
        return (None, None);
    };
    match notification.update {
        SessionUpdate::SessionInfoUpdate(info) => match info.title {
            MaybeUndefined::Value(title) => (Some(title), None),
            MaybeUndefined::Null => (Some(String::new()), None),
            MaybeUndefined::Undefined => (None, None),
        },
        SessionUpdate::AvailableCommandsUpdate(update) => {
            let Some(sent) = payload["update"]["availableCommands"].as_array() else {
                return (None, None);
            };
            let commands: Vec<Value> = update
                .available_commands
                .iter()
                .filter_map(|command| serde_json::to_value(command).ok())
                .collect();
            if commands.is_empty() && !sent.is_empty() {
                return (None, None);
            }
            (None, Some(commands))
        }
        _ => (None, None),
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
```

with:

```rust
    /// On a `session_info_update` that names a title: the title the agent
    /// reported, as it sent it; empty when it cleared it (ACP `null`). The
    /// collector normalises and caps it for the session list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// On an `available_commands_update`: the adapter's slash commands, the
    /// full list (ACP `AvailableCommand` objects, those hennery can parse).
    /// An empty list means the adapter has none, unlike an empty
    /// `config_options`. Never part of the catalogue snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown[] | undefined", optional)]
    pub commands: Option<Vec<Value>>,
    /// On an update the adapter sent before the session was announced:
    /// replayed by `session/load`, or sent while the start ran (ACP core
    /// §4.5). What it says may be older than what the collector holds, so
    /// its title only fills an empty one (plan 6b decision 2).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub early: bool,
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
/// Host -> collector.
```

with:

```rust
/// Host -> collector.
// `Session` is the largest variant by far (its body's extracts) and nearly
// every frame is one: boxing it would add an allocation to almost every
// frame to shrink the rare ones (`hello`, `error`), which live briefly.
// `expect`, so the attribute goes once the lint no longer fires.
#[expect(clippy::large_enum_variant)]
```

Run: `cargo run -p hennery-proto --bin gen`

- [ ] **Step 4: Run the tests to see them pass, then the checks**

Run: `nix develop -c cargo test -p hennery-testkit --locked --test host_session -- state`
Expected: PASS (2 passed). Then the five checks: 539 tests.

- [ ] **Step 5: Revert-probes**

Each probe below was applied to the task's commit, its test run, and the change undone. Every one was caught (4 of 4):
- commands extracted from a list the adapter never sent (`session.rs`): `live_state_updates_carry_the_title_and_the_commands` fails.
- a list whose every entry was skipped taken as "no commands" (`session.rs`): `live_state_updates_carry_the_title_and_the_commands` fails.
- a `null` title not sent as a clear (`session.rs`): `live_state_updates_carry_the_title_and_the_commands` fails.
- replayed updates not marked early (`session.rs`): `a_resume_passes_the_replayed_title_and_commands_through_with_their_extracts` fails.

- [ ] **Step 6: Commit**

`git -c commit.gpgsign=false commit -m "feat(host): extract the title and the commands from their updates"`

---

### Task 3: The collector's title and commands, and `catalog_changed`

**Files:**
- Modify: `crates/hennery-sessions/src/store.rs` (`one_line`, `store_state`, `SessionRow.title`, the catalogue's commands), `crates/hennery-sessions/src/api.rs` (`changes_catalogue`, `sse_messages`, the replay), `crates/hennery-proto/src/rest.rs` (`SessionCatalog.commands`; `from_indexed` removed), generated files
- Test: `crates/hennery-sessions/tests/store.rs`, `store.rs`'s unit tests, `crates/hennery-testkit/tests/reconcile.rs`, `owner_filter.rs` (floor 80)

**Interfaces:**
- Produces: `SessionRow.title: Option<String>`; `SessionCatalog.commands: Vec<Value>`; `one_line(raw, max_chars, max_json_bytes) -> Option<String>` (private).
- Consumes: Task 1's recency, Task 2's extracts.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-sessions/src/store.rs`, replace:

```rust

    /// Plan 6b decision 5: every stamp has the same width, so comparing
```

with:

```rust

    /// Plan 6b decision 1: one line, then the caps, by characters and by
    /// the bytes JSON takes; never a broken character.
    #[test]
    fn one_line_collapses_whitespace_and_cuts_to_the_caps() {
        assert_eq!(one_line("  a\n\tb \u{7}\u{2028} c ", 10, 10).as_deref(), Some("a b c"));
        assert_eq!(one_line(" \n\t\u{0} ", 10, 10), None);
        assert_eq!(one_line("", 10, 10), None);
        assert_eq!(one_line("abcdef", 4, 100).as_deref(), Some("abcd"));
        assert_eq!(one_line("abcdef", 100, 4).as_deref(), Some("abcd"));
        // `"` and `\` take two bytes in JSON.
        assert_eq!(one_line("a\"b\\c", 100, 3).as_deref(), Some("a\""));
        assert_eq!(one_line("a\"b\\c", 100, 5).as_deref(), Some("a\"b"));
        // Two bytes each, then three: the cut never splits a character.
        assert_eq!(one_line("ééé€€", 100, 7).as_deref(), Some("ééé"));
        // A cut that ends on a space drops it.
        assert_eq!(one_line("ab cd", 3, 100).as_deref(), Some("ab"));
        // Bidi and zero-width characters, which could make a row read as
        // something else, are dropped.
        assert_eq!(
            one_line("a\u{202E}b\u{200B}c\u{FEFF}d\u{061C}e\u{2066}f\u{200F}", 100, 100).as_deref(),
            Some("abcdef")
        );
    }

    /// Plan 6b decision 5: every stamp has the same width, so comparing
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    assert_eq!(recency(&store), (parked.ts.clone(), Some(parked.event_id)));
}

```

with:

```rust
    assert_eq!(recency(&store), (parked.ts.clone(), Some(parked.event_id)));
}

// Plan 6b: the title and the commands, from their extracts (ACP core §3.2,
// §7, §8).

fn titled(title: &str) -> SessionBody {
    SessionBody::AcpUpdate {
        indexed: Indexed {
            title: Some(title.into()),
            ..Indexed::default()
        },
        payload: json!({ "update": { "sessionUpdate": "session_info_update" } }),
    }
}

fn commands(names: &[&str]) -> SessionBody {
    let list = names.iter().map(|n| json!({ "name": n, "description": n })).collect();
    SessionBody::AcpUpdate {
        indexed: Indexed {
            commands: Some(list),
            ..Indexed::default()
        },
        payload: json!({ "update": { "sessionUpdate": "available_commands_update" } }),
    }
}

fn title_of(store: &Store) -> Option<String> {
    store.session("s1").unwrap().unwrap().title
}

/// Decision 1: the title is kept on one line and capped, for the list; an
/// empty one (the agent cleared it) clears it; one in an update that does
/// not apply changes nothing. The event keeps what the agent sent.
#[test]
fn a_title_is_stored_on_one_line_and_capped_and_an_empty_one_clears_it() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    let created = store
        .ingest("s1", 2, &titled("  Fix\nthe\tlogin \u{1}  bug  "))
        .unwrap();
    assert_eq!(title_of(&store).as_deref(), Some("Fix the login bug"));
    assert_eq!(created[0].body["indexed"]["title"], "  Fix\nthe\tlogin \u{1}  bug  ");
    store.ingest("s1", 3, &titled(&"x".repeat(300))).unwrap();
    assert_eq!(title_of(&store), Some("x".repeat(120)));
    // `"` takes two bytes in JSON: 80 of them is the byte cap (160).
    store.ingest("s1", 4, &titled(&"\"".repeat(150))).unwrap();
    assert_eq!(title_of(&store), Some("\"".repeat(80)));
    store.ingest("s1", 5, &titled("")).unwrap();
    assert_eq!(title_of(&store), None);
    store.ingest("s1", 6, &titled("Kept")).unwrap();
    store.close_now("s1").unwrap();
    assert!(store.ingest("s1", 7, &titled("Too late")).unwrap().is_empty());
    assert_eq!(title_of(&store).as_deref(), Some("Kept"));
}

fn early_titled(title: &str) -> SessionBody {
    let SessionBody::AcpUpdate { mut indexed, payload } = titled(title) else {
        unreachable!()
    };
    indexed.early = true;
    SessionBody::AcpUpdate { indexed, payload }
}

/// Decision 2: a title the adapter sent before the session was announced
/// (replayed by a load) may be older than the stored one: it only fills an
/// empty title. A live one always wins.
#[test]
fn an_early_title_only_fills_an_empty_one() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.ingest("s1", 2, &early_titled("Old")).unwrap();
    assert_eq!(title_of(&store).as_deref(), Some("Old"));
    // Still listed: only the title is left as it was.
    assert_eq!(store.ingest("s1", 3, &early_titled("Older")).unwrap().len(), 1);
    assert_eq!(title_of(&store).as_deref(), Some("Old"));
    store.ingest("s1", 4, &titled("New")).unwrap();
    assert_eq!(title_of(&store).as_deref(), Some("New"));
    // An early clear clears nothing.
    store.ingest("s1", 5, &early_titled("")).unwrap();
    assert_eq!(title_of(&store).as_deref(), Some("New"));
    store.ingest("s1", 6, &titled("")).unwrap();
    store.ingest("s1", 7, &early_titled("Replayed")).unwrap();
    assert_eq!(title_of(&store).as_deref(), Some("Replayed"));
}

/// Decision 3: the latest list replaces the stored one (an empty list too),
/// and commands never touch the config catalogue or its current values.
#[test]
fn commands_replace_the_stored_list_and_never_touch_the_config() {
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
    let snapshot = Indexed {
        config_options: Some(vec![json!({"id": "mode", "currentValue": "plan"})]),
        current_mode: Some("plan".into()),
        current_axes: Some(Default::default()),
        ..Indexed::default()
    };
    store
        .ingest(
            "s1",
            1,
            &SessionBody::SessionStarted {
                request_id: "r0".into(),
                agent_session_id: "a1".into(),
                indexed: snapshot,
            },
        )
        .unwrap();
    let before = store.catalog("s1").unwrap().unwrap();
    assert!(before.commands.is_empty());
    store.ingest("s1", 2, &commands(&["review", "plan"])).unwrap();
    let after = store.catalog("s1").unwrap().unwrap();
    assert_eq!(
        after.commands,
        [
            json!({"name": "review", "description": "review"}),
            json!({"name": "plan", "description": "plan"})
        ]
    );
    assert_eq!(
        (&after.config_options, &after.current),
        (&before.config_options, &before.current)
    );
    assert_eq!(store.session("s1").unwrap().unwrap().config, before.current);
    store.ingest("s1", 3, &commands(&[])).unwrap();
    let emptied = store.catalog("s1").unwrap().unwrap();
    assert!(emptied.commands.is_empty());
    assert_eq!(emptied.config_options, before.config_options);
}

/// Commands reported before any config (or by an adapter that has none)
/// are served with an empty catalogue.
#[test]
fn commands_reported_before_any_config_are_served_with_an_empty_catalogue() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.ingest("s1", 2, &commands(&["review"])).unwrap();
    let catalog = store.catalog("s1").unwrap().unwrap();
    assert_eq!(catalog.commands, [json!({"name": "review", "description": "review"})]);
    assert!(catalog.config_options.is_empty() && catalog.current.is_empty());
}

```

In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

```rust
        include_str!("../../hennery-sessions/src/store.rs"),
        77,
```

with:

```rust
        include_str!("../../hennery-sessions/src/store.rs"),
        80,
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
        json!(["cancelled", "host_revoked", false])
    );
}

```

with:

```rust
        json!(["cancelled", "host_revoked", false])
    );
}

// Plan 6b: commands join the catalogue (ACP core §7, §9).

fn commands_update(names: &[&str]) -> SessionBody {
    let list = names.iter().map(|n| json!({ "name": n, "description": n })).collect();
    SessionBody::AcpUpdate {
        indexed: hennery_proto::frames::Indexed {
            commands: Some(list),
            ..Default::default()
        },
        payload: json!({"update": {"sessionUpdate": "available_commands_update"}}),
    }
}

fn mode_update(mode: &str) -> SessionBody {
    SessionBody::AcpUpdate {
        indexed: catalogue(mode),
        payload: json!({"update": {"sessionUpdate": "config_option_update"}}),
    }
}

/// The `(id, data)` of every `catalog_changed` message in `stream`.
fn catalog_messages(stream: &str) -> Vec<(String, Value)> {
    stream
        .split("\n\n")
        .filter(|m| m.contains("event: catalog_changed"))
        .map(|m| {
            let field = |name: &str| m.lines().find_map(|l| l.strip_prefix(name)).unwrap().to_string();
            (field("id: "), serde_json::from_str(&field("data: ")).unwrap())
        })
        .collect()
}

/// The `id:` of the `event` message whose data contains `marker`.
fn event_id(stream: &str, marker: &str) -> String {
    let message = stream
        .split("\n\n")
        .find(|m| m.contains("event: event") && m.contains(marker))
        .unwrap();
    message
        .lines()
        .find_map(|l| l.strip_prefix("id: "))
        .unwrap()
        .to_string()
}

/// Decision 4: live, a commands update is a `catalog_changed` too, and each
/// one carries the whole catalogue as it stands: a later config change
/// does not wipe the commands, nor commands the config.
#[tokio::test]
async fn live_catalog_changed_carries_the_whole_catalogue_commands_included() {
    use futures::StreamExt;
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    host.emit(&session, mode_update("plan")).await;
    let resp = client(&collector)
        .get(collector.url(&format!("/api/stream/sessions/{session}")))
        .send()
        .await
        .unwrap();
    let mut body = resp.bytes_stream();
    let mut buf = String::new();
    let mut read_until = async |count: usize| {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while catalog_messages(&buf).len() < count {
            let chunk = tokio::time::timeout_at(deadline, body.next())
                .await
                .unwrap_or_else(|_| panic!("stream stalled: {buf}"))
                .unwrap()
                .unwrap();
            buf.push_str(&String::from_utf8_lossy(&chunk));
        }
        buf.clone()
    };
    // The replayed one, then one per live change.
    read_until(1).await;
    host.emit(&session, commands_update(&["review"])).await;
    let stream = read_until(2).await;
    let (id, data) = catalog_messages(&stream)[1].clone();
    assert_eq!(id, event_id(&stream, "available_commands_update"));
    assert_eq!(data["mode"], "plan", "{data}");
    assert_eq!(
        data["commands"],
        json!([{"name": "review", "description": "review"}]),
        "{data}"
    );
    host.emit(&session, mode_update("bypass")).await;
    let stream = read_until(3).await;
    let (_, data) = catalog_messages(&stream)[2].clone();
    assert_eq!(data["mode"], "bypass", "{data}");
    assert_eq!(
        data["commands"],
        json!([{"name": "review", "description": "review"}]),
        "{data}"
    );
    let catalog: Value = client(&collector)
        .get(collector.url(&format!("/api/sessions/{session}/catalog")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(catalog, data);
}

/// The review's A6: a replay from `Last-Event-ID` sends the catalogue once,
/// after the last event in it that changed the catalogue and with that
/// event's id, however many did: never one whole catalogue per event
/// (P-23).
#[tokio::test]
async fn a_replay_sends_the_catalogue_once_after_the_last_event_that_changed_it() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    host.emit(&session, mode_update("plan")).await;
    host.emit(&session, commands_update(&["review"])).await;
    host.emit(&session, mode_update("bypass")).await;
    host.emit(
        &session,
        SessionBody::AcpUpdate {
            indexed: Default::default(),
            payload: json!({"update": {"sessionUpdate": "agent_message_chunk", "marker": "last"}}),
        },
    )
    .await;
    let stream = read_stream(&collector, &session, |s| s.contains(r#""marker":"last""#)).await;
    let changed = catalog_messages(&stream);
    assert_eq!(changed.len(), 1, "{stream}");
    let (id, data) = &changed[0];
    let bypass = stream
        .split("\n\n")
        .filter(|m| m.contains("event: event") && m.contains("config_option_update"))
        .last()
        .unwrap();
    assert_eq!(Some(id.as_str()), bypass.lines().find_map(|l| l.strip_prefix("id: ")));
    // Right after that event, before the next one.
    let at = |needle: &str| stream.find(needle).unwrap();
    assert!(at("event: catalog_changed") > at(bypass) && at("event: catalog_changed") < at(r#""marker":"last""#));
    assert_eq!(data["mode"], "bypass", "{data}");
    assert_eq!(
        data["commands"],
        json!([{"name": "review", "description": "review"}]),
        "{data}"
    );
}

```

- [ ] **Step 2: Run the tests to see them fail**

Run: `nix develop -c cargo test -p hennery-sessions --locked`
Expected: FAIL to compile: ``error[E0425]: cannot find function `one_line` in this scope`` (×10).

- [ ] **Step 3: Store them, and send the catalogue as it stands**

In `crates/hennery-proto/src/rest.rs`, replace:

```rust
use crate::frames::{ConfigValue, ElicitationAction, Indexed, PendingKind, PendingReason, SessionConfig};
```

with:

```rust
use crate::frames::{ConfigValue, ElicitationAction, PendingKind, PendingReason, SessionConfig};
```

In `crates/hennery-proto/src/rest.rs`, replace:

```rust
/// A session's config catalogue and its current values: `GET
/// /api/sessions/{id}/catalog`, the answer to `POST …/config`, and the data
/// of the SSE `catalog_changed` message (ACP core §9). Commands, plan and
/// usage join it with the plans that produce them.
```

with:

```rust
/// A session's catalogue: its config options and their current values,
/// and its slash commands. `GET /api/sessions/{id}/catalog`, the answer to
/// `POST …/config`, and the data of the SSE `catalog_changed` message (ACP
/// core §9), always as it stands when sent. Plan and usage join it with
/// the plans that produce them.
```

In `crates/hennery-proto/src/rest.rs`, replace:

```rust
    #[serde(flatten)]
    pub current: SessionConfig,
}

impl SessionCatalog {
    /// The catalogue an event's extracts report, if they carry a snapshot.
    pub fn from_indexed(session_id: &str, indexed: &Indexed) -> Option<Self> {
        let current = indexed.current_config()?;
        Some(Self {
            session_id: session_id.to_string(),
            config_options: indexed.config_options.clone().unwrap_or_default(),
            current,
        })
    }
```

with:

```rust
    /// The adapter's slash commands (ACP `AvailableCommand` objects), as
    /// last reported; empty until it reports any (ACP core §7).
    #[serde(default)]
    #[ts(type = "unknown[]")]
    pub commands: Vec<Value>,
    #[serde(flatten)]
    pub current: SessionConfig,
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
    PendingItem, PromptRequest, PromptResponse, SessionCatalog, SessionDetail, StartSessionRequest,
    StartSessionResponse,
```

with:

```rust
    PendingItem, PromptRequest, PromptResponse, SessionDetail, StartSessionRequest, StartSessionResponse,
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
/// with the same id if it carries a catalogue snapshot, and
/// `pending_changed` with the same id if it concerns a pending request
/// (ACP core §9). A listed event with a snapshot is one that changed the
/// stored catalogue, and both come from the stored row, so a replay from
/// `Last-Event-ID` sends them too. `pending_changed` carries the request as
/// it stands when the message is sent.
fn sse_messages(store: &Store, e: &EventDto) -> Vec<Result<Event, Infallible>> {
    let mut out = vec![Ok(sse_event(e))];
    if let Some(catalog) = catalog_in(e) {
```

with:

```rust
/// with the same id if it changed the catalogue (and `catalog` is asked
/// for), and `pending_changed` with the same id if it concerns a pending
/// request (ACP core §9). Both are derived from the stored row, so a replay
/// from `Last-Event-ID` sends them too, and both carry what they describe
/// as it stands when the message is sent (plan 6b decision 4: the
/// catalogue has parts that change apart, the config and the commands, so
/// one built from the event alone would wipe the part it does not carry).
fn sse_messages(store: &Store, e: &EventDto, catalog: bool) -> Vec<Result<Event, Infallible>> {
    let mut out = vec![Ok(sse_event(e))];
    if catalog
        && changes_catalogue(e)
        && let Ok(Some(catalog)) = store.catalog(&e.session_id)
    {
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
/// The catalogue snapshot a stored host fact carries in its extracts.
fn catalog_in(e: &EventDto) -> Option<SessionCatalog> {
    if !matches!(e.kind.as_str(), "session_started" | "config_applied" | "acp_update") {
        return None;
    }
    let indexed: Indexed = serde_json::from_value(e.body.get("indexed")?.clone()).ok()?;
    SessionCatalog::from_indexed(&e.session_id, &indexed)
```

with:

```rust
/// Whether a stored host fact changed the catalogue: its extracts carry a
/// config snapshot or the commands. Listed events only reach here, and a
/// listed one with either changed the stored catalogue.
fn changes_catalogue(e: &EventDto) -> bool {
    if !matches!(e.kind.as_str(), "session_started" | "config_applied" | "acp_update") {
        return false;
    }
    let Some(indexed) = e.body.get("indexed") else {
        return false;
    };
    serde_json::from_value::<Indexed>(indexed.clone())
        .is_ok_and(|indexed| indexed.current_config().is_some() || indexed.commands.is_some())
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
    let replay = stream::iter(
        backlog
            .iter()
            .flat_map(|e| sse_messages(&state.store, e))
```

with:

```rust
    // The catalogue once, after the last event of the backlog that changed
    // it: every `catalog_changed` carries the whole catalogue as it stands,
    // so one per event would only repeat it (the review's A6, P-23).
    let last_catalogue = backlog.iter().rposition(changes_catalogue);
    let replay = stream::iter(
        backlog
            .iter()
            .enumerate()
            .flat_map(|(at, e)| sse_messages(&state.store, e, Some(at) == last_catalogue))
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
                    Ok(e) if e.session_id == session && e.event_id > last => Some(sse_messages(&store, &e)),
```

with:

```rust
                    Ok(e) if e.session_id == session && e.event_id > last => Some(sse_messages(&store, &e, true)),
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    pub last_event_id: Option<i64>,
```

with:

```rust
    pub last_event_id: Option<i64>,
    /// The title the agent last reported, on one line and capped (plan 6b
    /// decision 1); `None` until it reports one, or once it clears it.
    pub title: Option<String>,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust

/// A session's `model`, `mode` and `config_axes` columns.
```

with:

```rust

/// The caps on a title in the session list (plan 6b decision 1, the
/// review's A1): with every field of a list item at its cap, the item stays
/// under 1 KiB (P-23).
const TITLE_MAX_CHARS: usize = 120;
const TITLE_MAX_JSON_BYTES: usize = 160;

/// Bidi controls and zero-width characters: dropped from what the list
/// shows, since they could make a row read as something else (the review's
/// A2). Every other control character is a space by then.
fn is_hidden_format(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
    )
}

/// `raw` as one line for the session list (plan 6b decision 1): bidi and
/// zero-width characters dropped, every other control character a space,
/// runs of whitespace one space and the ends trimmed; then cut, never
/// inside a character, to `max_chars` characters and `max_json_bytes` bytes
/// as JSON writes it. With control characters gone, only `"` and `\` are
/// escaped, as two bytes each; a cap on the raw bytes would not hold, since
/// JSON writes a control character as six. `None` when nothing is left.
fn one_line(raw: &str, max_chars: usize, max_json_bytes: usize) -> Option<String> {
    let spaced: String = raw
        .chars()
        .filter(|c| !is_hidden_format(*c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut out = String::new();
    let (mut chars, mut bytes) = (0, 0);
    for c in spaced.split_whitespace().collect::<Vec<_>>().join(" ").chars() {
        let width = match c {
            '"' | '\\' => 2,
            c => c.len_utf8(),
        };
        if chars == max_chars || bytes + width > max_json_bytes {
            break;
        }
        out.push(c);
        chars += 1;
        bytes += width;
    }
    let out = out.trim_end();
    (!out.is_empty()).then(|| out.to_string())
}

/// Store the title and the commands a fact's extracts carry (ACP core §3.2,
/// §7). The title goes on one line and is capped (decision 1); a title the
/// adapter sent before the session was announced (`early`, replayed by a
/// load) may be older than the stored one, so it only fills an empty title
/// (decision 2). The commands replace the stored list, and never touch the
/// config catalogue (decision 3).
fn store_state(tx: &Transaction<'_>, owner: &str, session_id: &str, indexed: &Indexed, ts: &str) -> Result<()> {
    if let Some(title) = indexed.title.as_deref() {
        let title = one_line(title, TITLE_MAX_CHARS, TITLE_MAX_JSON_BYTES);
        if !indexed.early {
            tx.execute(
                "UPDATE sessions SET title = ?2 WHERE id = ?1 AND owner_id = ?3",
                params![session_id, title, owner],
            )?;
        } else if title.is_some() {
            tx.execute(
                "UPDATE sessions SET title = ?2 WHERE id = ?1 AND title IS NULL AND owner_id = ?3",
                params![session_id, title, owner],
            )?;
        }
    }
    if let Some(commands) = &indexed.commands {
        tx.execute(
            "INSERT INTO session_catalog(session_id, config_options, commands, updated_at, owner_id)
             VALUES (?1, '[]', ?2, ?3, ?4)
             ON CONFLICT(session_id) DO UPDATE SET commands = excluded.commands, updated_at = excluded.updated_at
                 WHERE session_catalog.owner_id = excluded.owner_id",
            params![session_id, serde_json::to_string(commands)?, ts, owner],
        )?;
    }
    Ok(())
}

/// A session's `model`, `mode` and `config_axes` columns.
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
                        presumed_parked, model, mode, config_axes, last_event_at, last_event_id
```

with:

```rust
                        presumed_parked, model, mode, config_axes, last_event_at, last_event_id, title
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
                        last_event_id: r.get(14)?,
```

with:

```rust
                        last_event_id: r.get(14)?,
                        title: r.get(15)?,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    /// The session's config catalogue and current values (ACP core §9);
    /// `None` for an unknown session, an empty catalogue for one whose
    /// host has reported none.
    pub fn catalog(&self, session_id: &str) -> Result<Option<SessionCatalog>> {
        let row: Option<(ConfigColumns, Option<String>)> = self
            .conn()
            .query_row(
                "SELECT s.model, s.mode, s.config_axes, c.config_options
                 FROM sessions s LEFT JOIN session_catalog c ON c.session_id = s.id AND c.owner_id = s.owner_id
                 WHERE s.id = ?1 AND s.owner_id = ?2",
                [session_id, &self.owner],
                |r| Ok(((r.get(0)?, r.get(1)?, r.get(2)?), r.get(3)?)),
            )
            .optional()?;
        let Some((config, options)) = row else {
            return Ok(None);
        };
        Ok(Some(SessionCatalog {
            session_id: session_id.to_string(),
            config_options: match options {
                Some(options) => serde_json::from_str(&options)?,
                None => Vec::new(),
            },
```

with:

```rust
    /// The session's catalogue (ACP core §9): its config options and
    /// current values, and its slash commands; `None` for an unknown
    /// session, an empty catalogue for one whose host has reported none.
    pub fn catalog(&self, session_id: &str) -> Result<Option<SessionCatalog>> {
        let row: Option<(ConfigColumns, Option<String>, Option<String>)> = self
            .conn()
            .query_row(
                "SELECT s.model, s.mode, s.config_axes, c.config_options, c.commands
                 FROM sessions s LEFT JOIN session_catalog c ON c.session_id = s.id AND c.owner_id = s.owner_id
                 WHERE s.id = ?1 AND s.owner_id = ?2",
                [session_id, &self.owner],
                |r| Ok(((r.get(0)?, r.get(1)?, r.get(2)?), r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((config, options, commands)) = row else {
            return Ok(None);
        };
        let list = |json: Option<String>| -> Result<Vec<Value>> {
            Ok(match json {
                Some(json) => serde_json::from_str(&json)?,
                None => Vec::new(),
            })
        };
        Ok(Some(SessionCatalog {
            session_id: session_id.to_string(),
            config_options: list(options)?,
            commands: list(commands)?,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
                    // with extracts.
                    store_catalogue(&tx, &self.owner, session_id, indexed, &ts)?;
                }
```

with:

```rust
                    // with extracts.
                    store_catalogue(&tx, &self.owner, session_id, indexed, &ts)?;
                    store_state(&tx, &self.owner, session_id, indexed, &ts)?;
                }
```

Run: `cargo run -p hennery-proto --bin gen`

- [ ] **Step 4: Run the tests to see them pass, then the checks**

Run: `nix develop -c cargo test -p hennery-sessions --locked` and `nix develop -c cargo test -p hennery-testkit --locked --test reconcile -- catalog`
Expected: PASS (`store`: 75 passed; `reconcile`: 5 passed). Then the five checks: 546 tests.

- [ ] **Step 5: Revert-probes**

Each probe below was applied to the task's commit, its test run, and the change undone. Every one was caught (6 of 6):
- an early title overwriting the stored one (`store.rs`): `an_early_title_only_fills_an_empty_one` fails.
- bidi and zero-width characters kept (`store.rs`): `one_line_collapses_whitespace_and_cuts_to_the_caps` fails.
- the title capped by its raw bytes, not as JSON writes them (`store.rs`): `a_title_is_stored_on_one_line_and_capped` fails.
- a commands update wiping the config options (`store.rs`): `commands_replace_the_stored_list_and_never_touch_the_config` fails.
- `catalog_changed` not sent for a commands update (`api.rs`): `live_catalog_changed_carries_the_whole_catalogue` fails.
- a replay sending one catalogue per event (`api.rs`): `a_replay_sends_the_catalogue_once` fails.

- [ ] **Step 6: Commit**

`git -c commit.gpgsign=false commit -m "feat(sessions): store the title and the commands, and send the whole catalogue"`

---

### Task 4: The list item and the detail

**Files:**
- Modify: `crates/hennery-proto/src/rest.rs` (the caps, `json_char_width`, `json_width`, `SessionItem`, `bounded`, `SessionDetail`), `codegen.rs`, generated files; `crates/hennery-sessions/src/store.rs` (`SESSION_ITEM_COLUMNS`, `read_item`, `session_item`; the title caps from the wire crate), `crates/hennery-sessions/src/api.rs` (`session_detail`, `start_session`'s checks)
- Test: `crates/hennery-proto/tests/frames.rs`, `crates/hennery-sessions/tests/store.rs`, `crates/hennery-testkit/tests/reconcile.rs` (two new tests; the detail test now expects the stamps), `owner_filter.rs` (floor 81)

**Interfaces:**
- Produces: `hennery_proto::rest::{SessionItem, TITLE_MAX_CHARS, TITLE_MAX_JSON_BYTES, BRANCH_MAX_CHARS, BRANCH_MAX_JSON_BYTES, MODEL_MAX_JSON_BYTES, MODE_MAX_JSON_BYTES, FAILURE_REASON_MAX_JSON_BYTES, AGENT_MAX_JSON_BYTES, CWD_MAX_JSON_BYTES, json_char_width, json_width}`, `SessionItem::bounded(self) -> Self`; `SessionDetail { session: SessionItem (flattened), open_turn, pending }`; `Store::session_item(&str) -> Result<Option<SessionItem>>`.
- Consumes: Task 3's title.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
}

#[test]
fn session_detail_leaves_out_absent_optionals() {
```

with:

```rust
}

/// A list item with only what every session has.
fn bare_item() -> hennery_proto::rest::SessionItem {
    hennery_proto::rest::SessionItem {
        session_id: "s".into(),
        host_id: "h".into(),
        agent: "claude".into(),
        cwd: "/tmp".into(),
        title: None,
        lifecycle: "active".into(),
        activity: Some("running".into()),
        failure_reason: None,
        presumed_parked: false,
        git_branch: None,
        git_dirty: None,
        model: None,
        mode: None,
        created_at: "2026-10-07T12:00:00.000Z".into(),
        last_event_at: "2026-10-07T12:00:01.000Z".into(),
    }
}

#[test]
fn session_detail_leaves_out_absent_optionals() {
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
        session_id: "s".into(),
        host_id: "h".into(),
        agent: "claude".into(),
        cwd: "/tmp".into(),
        lifecycle: "active".into(),
        activity: Some("running".into()),
        failure_reason: None,
        presumed_parked: false,
```

with:

```rust
        session: bare_item(),
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
            "open_turn": {"turn_id": "t", "state": "started"}, "pending": []
        })
    );
```

with:

```rust
            "created_at": "2026-10-07T12:00:00.000Z", "last_event_at": "2026-10-07T12:00:01.000Z",
            "open_turn": {"turn_id": "t", "state": "started"}, "pending": []
        })
    );
}

/// Plan 6b, the review's A1: as the list serves it, a model, mode or
/// failure reason past its cap is left out (the stored value is kept for a
/// resume), a title or branch past its cap is cut, and a long cwd keeps
/// its end, after `…`; within the caps nothing changes.
#[test]
fn a_listed_item_is_bounded_field_by_field() {
    use hennery_proto::rest::{
        AGENT_MAX_JSON_BYTES, CWD_MAX_JSON_BYTES, FAILURE_REASON_MAX_JSON_BYTES, HOST_ID_MAX_JSON_BYTES,
        MODE_MAX_JSON_BYTES, MODEL_MAX_JSON_BYTES,
    };
    let within = hennery_proto::rest::SessionItem {
        title: Some("Fix the login bug".into()),
        git_branch: Some("fix/login".into()),
        model: Some("m".repeat(MODEL_MAX_JSON_BYTES)),
        mode: Some("\"".repeat(MODE_MAX_JSON_BYTES / 2)),
        failure_reason: Some("f".repeat(FAILURE_REASON_MAX_JSON_BYTES)),
        cwd: "c".repeat(CWD_MAX_JSON_BYTES),
        ..bare_item()
    };
    assert_eq!(within.clone().bounded(), within);
    assert_eq!(AGENT_MAX_JSON_BYTES, 32);

    let past = hennery_proto::rest::SessionItem {
        title: Some("t".repeat(500)),
        git_branch: Some("b".repeat(500)),
        model: Some("m".repeat(MODEL_MAX_JSON_BYTES + 1)),
        mode: Some("\"".repeat(MODE_MAX_JSON_BYTES / 2 + 1)),
        failure_reason: Some("f".repeat(FAILURE_REASON_MAX_JSON_BYTES + 1)),
        cwd: format!("/home/someone/{}webapp", "deep/".repeat(40)),
        ..bare_item()
    }
    .bounded();
    assert_eq!(past.title, Some("t".repeat(120)));
    assert_eq!(past.git_branch, Some("b".repeat(120)));
    assert_eq!((past.model, past.mode, past.failure_reason), (None, None, None));
    // The second review's P1: within its cap, but with a control or hidden
    // character, a model, mode or failure reason is left out too; an agent
    // or host id from before the start checked them is cut.
    let odd = hennery_proto::rest::SessionItem {
        model: Some("op\u{202E}us".into()),
        mode: Some("pl\u{200B}an".into()),
        failure_reason: Some("bad\nreason".into()),
        agent: "a".repeat(40),
        host_id: "h".repeat(100),
        ..bare_item()
    }
    .bounded();
    assert_eq!((odd.model, odd.mode, odd.failure_reason), (None, None, None));
    assert_eq!(
        (odd.agent, odd.host_id),
        ("a".repeat(AGENT_MAX_JSON_BYTES), "h".repeat(HOST_ID_MAX_JSON_BYTES))
    );
    assert!(
        past.cwd.starts_with('…') && past.cwd.ends_with("/deep/webapp"),
        "{}",
        past.cwd
    );
    assert!(serde_json::to_string(&past.cwd).unwrap().len() - 2 <= CWD_MAX_JSON_BYTES);
    // Never inside a character: four bytes each.
    let emoji = hennery_proto::rest::SessionItem {
        cwd: "😀".repeat(100),
        ..bare_item()
    }
    .bounded();
    assert_eq!(emoji.cwd, format!("…{}", "😀".repeat((CWD_MAX_JSON_BYTES - 3) / 4)));
}

/// P-23 (ACP core §8): a list item stays under 1 KiB, here with every field
/// at its cap or past it, the worst content for each (quotes, backslashes,
/// control and four-byte characters), a cwd of exactly its cap (kept
/// whole), a 30-character `created_at` from before stamps had one width,
/// and room left for the `hat_id` hats add (plan 6b decision 10, the
/// reviews' A1 and P2).
#[test]
fn a_listed_item_stays_under_1_kib_with_every_field_at_its_worst() {
    let item = hennery_proto::rest::SessionItem {
        session_id: "0199a4c2-7e1f-7c3a-9b2d-4f6e8a0c1d2e".into(),
        // Past their caps (an older row's): cut to 32 bytes each.
        host_id: "\"".repeat(40),
        agent: "\"".repeat(40),
        // Exactly 128 bytes as JSON writes it: kept whole.
        cwd: format!("/{}", "\"\\😀".repeat(15)) + &"x".repeat(7),
        title: Some("\"".repeat(500)),
        lifecycle: "starting".into(),
        activity: Some("blocked".into()),
        failure_reason: Some("\\".repeat(16)),
        presumed_parked: false,
        git_branch: Some("\\😀".repeat(200)),
        git_dirty: Some(false),
        model: Some("\"".repeat(24)),
        mode: Some("\\".repeat(24)),
        created_at: "2026-10-07T12:34:56.123456789Z".into(),
        last_event_at: "2026-10-07T12:34:56.789Z".into(),
    }
    .bounded();
    assert!(!item.cwd.starts_with('…'), "{}", item.cwd);
    assert_eq!(serde_json::to_string(&item.cwd).unwrap().len() - 2, 128);
    let json = serde_json::to_string(&item).unwrap();
    // `,"hat_id":"…"` with a 36-character id is 48 bytes; 64 are kept.
    assert!(json.len() <= 1024 - 64, "{} bytes: {json}", json.len());
    assert!(item.model.is_some() && item.mode.is_some() && item.failure_reason.is_some());
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    assert!(catalog.config_options.is_empty() && catalog.current.is_empty());
}

```

with:

```rust
    assert!(catalog.config_options.is_empty() && catalog.current.is_empty());
}

// Plan 6b: the list item (ACP core §8, §9).

/// Decision 9: the item is the row, read from `sessions` alone, with the
/// title and the current model and mode (B2b's "model / mode in the list
/// and detail items"), unbounded: the detail serves it as it is.
#[test]
fn a_session_item_is_the_row_with_its_title_and_current_model_and_mode() {
    let store = Store::open_in_memory().unwrap();
    let cwd = format!("/home/someone/{}", "deep/".repeat(40));
    store.create_session("s1", "h1", "fake", &cwd).unwrap();
    let snapshot = Indexed {
        config_options: Some(vec![json!({"id": "model"}), json!({"id": "mode"})]),
        current_model: Some("opus".into()),
        current_mode: Some("plan".into()),
        current_axes: Some(Default::default()),
        ..Indexed::default()
    };
    store
        .ingest(
            "s1",
            1,
            &SessionBody::SessionStarted {
                request_id: "r0".into(),
                agent_session_id: "a1".into(),
                indexed: snapshot,
            },
        )
        .unwrap();
    store.ingest("s1", 2, &titled("Fix the login bug")).unwrap();
    let row = store.session("s1").unwrap().unwrap();
    let item = store.session_item("s1").unwrap().unwrap();
    assert_eq!(
        item,
        hennery_proto::rest::SessionItem {
            session_id: "s1".into(),
            host_id: "h1".into(),
            agent: "fake".into(),
            cwd,
            title: Some("Fix the login bug".into()),
            lifecycle: "active".into(),
            activity: Some("idle".into()),
            failure_reason: None,
            presumed_parked: false,
            git_branch: None,
            git_dirty: None,
            model: Some("opus".into()),
            mode: Some("plan".into()),
            created_at: item.created_at.clone(),
            last_event_at: row.last_event_at,
        }
    );
    assert_eq!(item.created_at.len(), 24);
    assert!(store.session_item("nope").unwrap().is_none());
}

```

In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

```rust
        include_str!("../../hennery-sessions/src/store.rs"),
        80,
```

with:

```rust
        include_str!("../../hennery-sessions/src/store.rs"),
        81,
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let (status, body) = get(&client(&collector), collector.url(&format!("/api/sessions/{session}"))).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
```

with:

```rust
    let (status, body) = get(&client(&collector), collector.url(&format!("/api/sessions/{session}"))).await;
    assert_eq!(status, 200, "{body}");
    let item = collector.state.store.session_item(&session).unwrap().unwrap();
    assert_eq!(
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
            "lifecycle": "active", "activity": "running", "presumed_parked": false,
```

with:

```rust
            "lifecycle": "active", "activity": "running", "presumed_parked": false,
            "created_at": item.created_at, "last_event_at": item.last_event_at,
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
        json!([{"name": "review", "description": "review"}]),
        "{data}"
    );
}

```

with:

```rust
        json!([{"name": "review", "description": "review"}]),
        "{data}"
    );
}

// Plan 6b: the list item in the detail; what a start may name (the review's
// A1).

/// The detail is the list item (with the title and the current model and
/// mode), the open turn and the open questions.
#[tokio::test]
async fn the_session_detail_carries_the_title_and_the_current_model_and_mode() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    host.emit(&session, mode_update("plan")).await;
    host.emit(
        &session,
        SessionBody::AcpUpdate {
            indexed: hennery_proto::frames::Indexed {
                title: Some("Fix the login bug".into()),
                ..Default::default()
            },
            payload: json!({"update": {"sessionUpdate": "session_info_update"}}),
        },
    )
    .await;
    wait_for("the title", || async {
        collector
            .state
            .store
            .session(&session)
            .unwrap()
            .unwrap()
            .title
            .map(|_| ())
    })
    .await;
    let (status, detail) = get(&client(&collector), collector.url(&format!("/api/sessions/{session}"))).await;
    assert_eq!(status, 200, "{detail}");
    assert_eq!(detail["title"], "Fix the login bug");
    assert_eq!(detail["mode"], "plan");
    assert_eq!(detail["lifecycle"], "active");
    assert_eq!(detail["pending"], json!([]));
    assert_eq!(detail["last_event_at"].as_str().unwrap().len(), 24);
}

/// A start names a paired host (an unknown one is refused before any
/// session exists), and an agent of at most 32 bytes as JSON writes them,
/// so every field of the list item is bounded.
#[tokio::test]
async fn a_start_names_a_paired_host_and_an_agent_of_at_most_32_bytes() {
    let collector = Collector::start().await;
    let c = client(&collector);
    let (status, body) = post(
        &c,
        collector.url("/api/sessions"),
        json!({ "host_id": "host-unknown", "agent": "fake", "cwd": "/tmp" }),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (400, Some("unknown_host")), "{body}");
    let (status, body) = post(
        &c,
        collector.url("/api/sessions"),
        json!({ "host_id": HOST, "agent": "a".repeat(33), "cwd": "/tmp" }),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    let (status, body) = post(
        &c,
        collector.url("/api/sessions"),
        json!({ "host_id": HOST, "agent": "\"".repeat(17), "cwd": "/tmp" }),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    let sessions: i64 = rusqlite::Connection::open(collector._dir.path().join("hennery.db"))
        .unwrap()
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(sessions, 0);
    // A paired host that is offline still gets a session, failed as before.
    let (status, body) = post(
        &c,
        collector.url("/api/sessions"),
        json!({ "host_id": HOST, "agent": "a".repeat(32), "cwd": "/tmp" }),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")), "{body}");
}

```

- [ ] **Step 2: Run the tests to see them fail**

Run: `nix develop -c cargo test -p hennery-proto --locked --test frames`
Expected: FAIL to compile: ``cannot find struct, variant or union type `SessionItem` in module `hennery_proto::rest` ``, ``unresolved imports `hennery_proto::rest::AGENT_MAX_JSON_BYTES`, …``, ``struct `SessionDetail` has no field named `session` ``.

- [ ] **Step 3: The item, its caps, the detail and the start's checks**

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::LifecycleResponse,
        rest::SessionDetail,
```

with:

```rust
        rest::LifecycleResponse,
        rest::SessionItem,
        rest::SessionDetail,
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::OpenTurn,
```

with:

```rust
        rest::OpenTurn,
        rest::SessionItem,
```

In `crates/hennery-proto/src/rest.rs`, replace:

```rust
/// `GET /api/sessions/{id}` (ACP core §9): the list item, the open turn and
/// the pending requests still open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionDetail {
```

with:

```rust
/// The session list's caps, in bytes as JSON writes each field (plan 6b
/// decision 9, the review's A1). With every field at its cap, a list item
/// stays under 1 KiB with room for a `hat_id` (P-23). The title and the
/// branch are stored within theirs; an agent past its cap is refused at
/// the start; a model, mode or failure reason past its cap is left out of
/// the item (the stored value is kept: a resume re-applies it); a cwd past
/// its cap is shown by its end.
pub const TITLE_MAX_CHARS: usize = 120;
pub const TITLE_MAX_JSON_BYTES: usize = 160;
pub const BRANCH_MAX_CHARS: usize = 120;
pub const BRANCH_MAX_JSON_BYTES: usize = 120;
pub const MODEL_MAX_JSON_BYTES: usize = 48;
pub const MODE_MAX_JSON_BYTES: usize = 48;
pub const FAILURE_REASON_MAX_JSON_BYTES: usize = 32;
pub const AGENT_MAX_JSON_BYTES: usize = 32;
pub const CWD_MAX_JSON_BYTES: usize = 128;
/// A paired host's id is `host-` and 16 hex digits (21 bytes); an older
/// row's, from before the start checked it, is cut to this.
pub const HOST_ID_MAX_JSON_BYTES: usize = 32;

/// Bidi controls and zero-width characters: never shown as they are, since
/// they could make a row read as something else (plan 6b, the review's A2).
pub fn is_hidden_format(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
    )
}

/// How many bytes JSON takes to write `c` inside a string, as serde_json
/// escapes it: `"`, `\` and the short escapes take two, other control
/// characters six (`\u00XX`).
pub fn json_char_width(c: char) -> usize {
    match c {
        '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
        c if (c as u32) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// How many bytes JSON takes to write `s` inside a string.
pub fn json_width(s: &str) -> usize {
    s.chars().map(json_char_width).sum()
}

/// The longest start of `s` with at most `max_chars` characters and
/// `max_json_bytes` bytes as JSON writes them; never cut inside a character.
fn cut(s: &str, max_chars: usize, max_json_bytes: usize) -> String {
    let mut bytes = 0;
    s.chars()
        .take(max_chars)
        .take_while(|c| {
            bytes += json_char_width(*c);
            bytes <= max_json_bytes
        })
        .collect()
}

/// `s` if it fits in `max_json_bytes` as JSON writes it, else its longest
/// end that fits after `…`; never cut inside a character.
fn tail(s: &str, max_json_bytes: usize) -> String {
    if json_width(s) <= max_json_bytes {
        return s.to_string();
    }
    let mut bytes = '…'.len_utf8();
    let kept: Vec<char> = s
        .chars()
        .rev()
        .take_while(|c| {
            bytes += json_char_width(*c);
            bytes <= max_json_bytes
        })
        .collect();
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}

/// One session as the session list shows it (ACP core §8, §9; frontend
/// §5), read from `sessions` alone: never the catalogue, commands or plan
/// (P-23). The detail serves it as stored; the list, `bounded`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionItem {
```

In `crates/hennery-proto/src/rest.rs`, replace:

```rust
    pub cwd: String,
    pub lifecycle: String,
```

with:

```rust
    pub cwd: String,
    /// The title the agent reported, on one line and capped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub title: Option<String>,
    pub lifecycle: String,
```

In `crates/hennery-proto/src/rest.rs`, replace:

```rust
    pub presumed_parked: bool,
```

with:

```rust
    pub presumed_parked: bool,
    /// The branch checked out in `cwd`, as the host last reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub git_branch: Option<String>,
    /// Whether `cwd`'s work tree had changes, as the host last reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "boolean | undefined", optional)]
    pub git_dirty: Option<bool>,
    /// The current model and mode, as the host last reported them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub mode: Option<String>,
    /// RFC 3339, UTC.
    pub created_at: String,
    /// When its last listed event was written (RFC 3339, UTC, three
    /// fractional digits): the list's sort key, newest first.
    pub last_event_at: String,
}

impl SessionItem {
    /// The item as the session list serves it, every field within its cap
    /// (see `TITLE_MAX_CHARS`): with them, it stays under 1 KiB.
    pub fn bounded(mut self) -> Self {
        // Shown as they are in every row: one with a control or hidden
        // character is left out too (the second review's P1).
        let within = |value: Option<String>, max: usize| {
            value.filter(|v| json_width(v) <= max && !v.chars().any(|c| c.is_control() || is_hidden_format(c)))
        };
        self.title = self.title.map(|t| cut(&t, TITLE_MAX_CHARS, TITLE_MAX_JSON_BYTES));
        self.git_branch = self
            .git_branch
            .map(|b| cut(&b, BRANCH_MAX_CHARS, BRANCH_MAX_JSON_BYTES));
        self.model = within(self.model, MODEL_MAX_JSON_BYTES);
        self.mode = within(self.mode, MODE_MAX_JSON_BYTES);
        self.failure_reason = within(self.failure_reason, FAILURE_REASON_MAX_JSON_BYTES);
        self.cwd = tail(&self.cwd, CWD_MAX_JSON_BYTES);
        // An older row's, from before the start checked them.
        self.agent = cut(&self.agent, usize::MAX, AGENT_MAX_JSON_BYTES);
        self.host_id = cut(&self.host_id, usize::MAX, HOST_ID_MAX_JSON_BYTES);
        self
    }
}

/// `GET /api/sessions/{id}` (ACP core §9): the list item, as stored, the
/// open turn and the pending requests still open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionDetail {
    #[serde(flatten)]
    pub session: SessionItem,
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
    AnswerRequest, AnswerResponse, ApiError, CancelResponse, ConfigRequest, EventDto, LifecycleResponse, OpenTurn,
    PendingItem, PromptRequest, PromptResponse, SessionDetail, StartSessionRequest, StartSessionResponse,
```

with:

```rust
    AGENT_MAX_JSON_BYTES, AnswerRequest, AnswerResponse, ApiError, CancelResponse, ConfigRequest, EventDto,
    LifecycleResponse, OpenTurn, PendingItem, PromptRequest, PromptResponse, SessionDetail, StartSessionRequest,
    StartSessionResponse, json_width,
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
async fn start_session(State(state): State<AppState>, Json(req): Json<StartSessionRequest>) -> Response {
```

with:

```rust
async fn start_session(State(state): State<AppState>, Json(req): Json<StartSessionRequest>) -> Response {
    // What a list item shows must be bounded (plan 6b, the review's A1): a
    // paired host's id, and an agent's name within its cap. Refused before
    // any session exists.
    if json_width(&req.agent) > AGENT_MAX_JSON_BYTES {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid",
            format!("an agent's name is at most {AGENT_MAX_JSON_BYTES} bytes"),
        );
    }
    match state.hosts.host(&req.host_id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return error(
                StatusCode::BAD_REQUEST,
                "unknown_host",
                "no host is paired with that id",
            );
        }
        Err(err) => return internal(err),
    }
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
/// Session detail (ACP core §9): the list item plus the open turn.
async fn session_detail(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let session = match state.store.session(&id) {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
```

with:

```rust
/// Session detail (ACP core §9): the list item, as stored, plus the open
/// turn and the open questions.
async fn session_detail(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let (session, item) = match (state.store.session(&id), state.store.session_item(&id)) {
        (Ok(Some(s)), Ok(Some(item))) => (s, item),
        (Ok(None), _) | (_, Ok(None)) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        (Err(err), _) | (_, Err(err)) => return internal(err),
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        session_id: session.id,
        host_id: session.host_id,
        agent: session.agent,
        cwd: session.cwd,
        lifecycle: session.lifecycle,
        activity: session.activity,
        failure_reason: session.failure_reason,
        presumed_parked: session.presumed_parked,
```

with:

```rust
        session: item,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
use hennery_proto::rest::{AnswerRequest, EventDto, PendingItem, PendingState, SessionCatalog};
```

with:

```rust
use hennery_proto::rest::{
    AnswerRequest, EventDto, PendingItem, PendingState, SessionCatalog, SessionItem, TITLE_MAX_CHARS,
    TITLE_MAX_JSON_BYTES, json_char_width,
};
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust

/// The caps on a title in the session list (plan 6b decision 1, the
/// review's A1): with every field of a list item at its cap, the item stays
/// under 1 KiB (P-23).
const TITLE_MAX_CHARS: usize = 120;
const TITLE_MAX_JSON_BYTES: usize = 160;

/// Bidi controls and zero-width characters: dropped from what the list
/// shows, since they could make a row read as something else (the review's
/// A2). Every other control character is a space by then.
fn is_hidden_format(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
    )
}

```

with:

```rust

```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
        .filter(|c| !is_hidden_format(*c))
```

with:

```rust
        .filter(|c| !hennery_proto::rest::is_hidden_format(*c))
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
        let width = match c {
            '"' | '\\' => 2,
            c => c.len_utf8(),
        };
```

with:

```rust
        let width = json_char_width(c);
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
        )?;
    }
    Ok(())
}

/// A session's `model`, `mode` and `config_axes` columns.
```

with:

```rust
        )?;
    }
    Ok(())
}

/// The columns of a list item, in `read_item`'s order.
const SESSION_ITEM_COLUMNS: &str = "id, host_id, agent, cwd, title, lifecycle, activity, failure_reason,
     presumed_parked, git_branch, git_dirty, model, mode, created_at, last_event_at";

/// A row of `SESSION_ITEM_COLUMNS` as a list item, as stored.
fn read_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionItem> {
    Ok(SessionItem {
        session_id: r.get(0)?,
        host_id: r.get(1)?,
        agent: r.get(2)?,
        cwd: r.get(3)?,
        title: r.get(4)?,
        lifecycle: r.get(5)?,
        activity: r.get(6)?,
        failure_reason: r.get(7)?,
        presumed_parked: r.get(8)?,
        git_branch: r.get(9)?,
        git_dirty: r.get(10)?,
        model: r.get(11)?,
        mode: r.get(12)?,
        created_at: r.get(13)?,
        last_event_at: r.get(14)?,
    })
}

/// A session's `model`, `mode` and `config_axes` columns.
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
        Ok(Some(row))
```

with:

```rust
        Ok(Some(row))
    }

    /// One session as a list item, as stored (the detail's; the list serves
    /// it `bounded`).
    pub fn session_item(&self, id: &str) -> Result<Option<SessionItem>> {
        Ok(self
            .conn()
            .query_row(
                &format!("SELECT {SESSION_ITEM_COLUMNS} FROM sessions WHERE id = ?1 AND owner_id = ?2"),
                [id, &self.owner],
                read_item,
            )
            .optional()?)
```

Run: `cargo run -p hennery-proto --bin gen`

- [ ] **Step 4: Run the tests to see them pass, then the checks**

Run: `nix develop -c cargo test -p hennery-proto --locked --test frames`
Expected: PASS (20 passed). Then the five checks: 551 tests.

- [ ] **Step 5: Revert-probes**

Each probe below was applied to the task's commit, its test run, and the change undone. Every one was caught (7 of 7):
- a model past its cap kept in the list (`rest.rs`): `a_listed_item_is_bounded_field_by_field` fails.
- the cwd served whole (`rest.rs`): `a_listed_item_is_bounded_field_by_field` fails.
- the title not re-cut when served (`rest.rs`): `a_listed_item_stays_under_1_kib` fails.
- a start to an unpaired host (`api.rs`): `a_start_names_a_paired_host` fails.
- an agent's name measured in raw bytes (`api.rs`): `a_start_names_a_paired_host` fails.
- a model with a hidden character kept (`rest.rs`): `a_listed_item_is_bounded_field_by_field` fails.
- an older row's agent served whole (`rest.rs`): `a_listed_item_stays_under_1_kib` fails.

- [ ] **Step 6: Commit**

`git -c commit.gpgsign=false commit -m "feat(sessions): a bounded list item, and the detail built from it"`

---

### Task 5: `GET /api/sessions`

**Files:**
- Modify: `crates/hennery-proto/src/rest.rs` (`SessionPage`), `codegen.rs`, generated files; `crates/hennery-sessions/src/store.rs` (`LIFECYCLES`, `LIST_DEFAULT_LIMIT`, `LIST_MAX_LIMIT`, `Cursor`, `ListQuery`, `like_pattern`, `list_statement`, `list`), `crates/hennery-sessions/src/api.rs` (`list_sessions` and its route)
- Test: `crates/hennery-sessions/tests/store.rs`, `tests/owner.rs`, `store.rs`'s unit tests, `crates/hennery-testkit/tests/reconcile.rs`, `owner_filter.rs` (floor 82)

**Interfaces:**
- Produces: `hennery_proto::rest::SessionPage { sessions, next_cursor }`; `hennery_sessions::store::{Cursor { last_event_at, session_id }, Cursor::encode, Cursor::decode, ListQuery { after, limit, lifecycles, search }, LIFECYCLES, LIST_DEFAULT_LIMIT, LIST_MAX_LIMIT}`, `Store::list(&ListQuery) -> Result<SessionPage>`.
- Consumes: Task 1's index, Task 4's item.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-sessions/src/store.rs`, replace:

```rust

    /// Plan 6b decision 5: every stamp has the same width, so comparing
```

with:

```rust

    /// The review's A11: the list's one statement walks the recency index,
    /// with no sort of its own, with or without a search.
    #[test]
    fn the_list_walks_the_recency_index() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        Store::open(&db).unwrap();
        let conn = Connection::open(&db).unwrap();
        let plan: Vec<String> = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {}", list_statement()))
            .unwrap()
            .query_map(
                params!["o", "~", "", "active", "parked", None::<String>, None::<String>, None::<String>, "%x%", 50],
                |r| r.get(3),
            )
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let plan = plan.join("\n");
        assert!(plan.contains("USING INDEX sessions_by_recency"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    }

    /// Decision 8, the review's O4: a cursor goes out opaque and comes back
    /// the same; anything else is refused.
    #[test]
    fn a_cursor_round_trips_and_a_malformed_one_is_refused() {
        let cursor = Cursor {
            last_event_at: "2026-10-07T12:00:00.000Z".into(),
            session_id: "0199a4c2-7e1f-7c3a-9b2d-4f6e8a0c1d2e".into(),
        };
        let encoded = cursor.encode();
        assert!(encoded.chars().all(|c| c.is_ascii_hexdigit()), "{encoded}");
        assert_eq!(Cursor::decode(&encoded), Some(cursor));
        for bad in [
            "",
            "zz",
            "abc",
            &hex::encode("no separator"),
            &hex::encode("\nid"),
            &hex::encode("at\n"),
            &hex::encode([0xff, b'\n', b'a']),
            &hex::encode(format!("at\n{}", "x".repeat(65))),
            // Well formed, each part within 64 bytes, but past 256 characters.
            &hex::encode(format!("{}\n{}", "a".repeat(64), "b".repeat(64))),
        ] {
            assert_eq!(Cursor::decode(bad), None, "{bad}");
        }
    }

    /// `%`, `_` and `\` in a search are literal (decision 8).
    #[test]
    fn a_search_escapes_like_wildcards() {
        assert_eq!(like_pattern("100%_a\\b"), "%100\\%\\_a\\\\b%");
        assert_eq!(like_pattern("plain"), "%plain%");
    }

    /// Plan 6b decision 5: every stamp has the same width, so comparing
```

In `crates/hennery-sessions/tests/owner.rs`, replace:

```rust
use hennery_sessions::store::{AnswerSubmission, Reconciliation, ResumeRequest, Store};
```

with:

```rust
use hennery_sessions::store::{AnswerSubmission, ListQuery, Reconciliation, ResumeRequest, Store};
```

In `crates/hennery-sessions/tests/owner.rs`, replace:

```rust
    assert_eq!(store.session("session-b").unwrap(), None);
```

with:

```rust
    assert_eq!(store.session("session-b").unwrap(), None);
    assert_eq!(store.session_item("session-b").unwrap(), None);
    // The list, and a search that would match only the other owner's.
    let listed: Vec<String> = store
        .list(&ListQuery::default())
        .unwrap()
        .sessions
        .into_iter()
        .map(|s| s.session_id)
        .collect();
    assert_eq!(listed, ["session-a"]);
    let search = ListQuery {
        search: Some("session-b"),
        ..ListQuery::default()
    };
    assert!(store.list(&search).unwrap().sessions.is_empty());
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    assert!(store.session_item("nope").unwrap().is_none());
}

```

with:

```rust
    assert!(store.session_item("nope").unwrap().is_none());
}

// Plan 6b: the session list (ACP core §9; frontend §5).

use hennery_sessions::store::{Cursor, ListQuery};

/// A store over a file, with a session per `(id, last_event_at, title,
/// cwd)`, its recency set by hand so the order is known.
fn listed_store(dir: &std::path::Path, sessions: &[(&str, &str, Option<&str>, &str)]) -> Store {
    let db = dir.join("hennery.db");
    let store = Store::open(&db).unwrap();
    for (id, _, _, cwd) in sessions {
        store.create_session(id, "h1", "fake", cwd).unwrap();
    }
    let conn = rusqlite::Connection::open(&db).unwrap();
    for (id, at, title, _) in sessions {
        conn.execute(
            "UPDATE sessions SET last_event_at = ?2, title = ?3 WHERE id = ?1",
            rusqlite::params![id, at, title],
        )
        .unwrap();
    }
    store
}

fn ids(page: &hennery_proto::rest::SessionPage) -> Vec<&str> {
    page.sessions.iter().map(|s| s.session_id.as_str()).collect()
}

/// Stamps from before any real clock this test runs on.
const T: &str = "2020-01-07T12:00:0";

/// One sort key, newest `last_event_at` first, the id breaking ties
/// (decision 8); pages follow an opaque cursor, and a session that moves to
/// the top meanwhile does not shift the next page.
#[test]
fn the_list_is_newest_first_and_pages_by_keyset() {
    let dir = tempfile::tempdir().unwrap();
    let at = |s: u8| format!("{T}{s}.000Z");
    let (a1, a2, a3, a5) = (at(1), at(2), at(3), at(5));
    let store = listed_store(
        dir.path(),
        &[
            ("s1", &a1, None, "/tmp"),
            ("s2", &a3, None, "/tmp"),
            ("s3", &a3, None, "/tmp"),
            ("s4", &a2, None, "/tmp"),
            ("s5", &a5, None, "/tmp"),
        ],
    );
    assert_eq!(
        ids(&store.list(&ListQuery::default()).unwrap()),
        ["s5", "s3", "s2", "s4", "s1"]
    );
    let first = store
        .list(&ListQuery {
            limit: 2,
            ..ListQuery::default()
        })
        .unwrap();
    assert_eq!(ids(&first), ["s5", "s3"]);
    let cursor = Cursor::decode(first.next_cursor.as_deref().unwrap()).unwrap();
    // s1 moves to the top: the next page is still the one after s3.
    store.ingest("s1", 1, &SessionBody::session_started("r", "a")).unwrap();
    let second = store
        .list(&ListQuery {
            after: Some(&cursor),
            limit: 2,
            ..ListQuery::default()
        })
        .unwrap();
    assert_eq!(ids(&second), ["s2", "s4"]);
    // s1 now sorts first, so nothing follows s4: no further page.
    assert_eq!(second.next_cursor, None);
    let all = store
        .list(&ListQuery {
            limit: 6,
            ..ListQuery::default()
        })
        .unwrap();
    assert_eq!(ids(&all), ["s1", "s5", "s3", "s2", "s4"]);
    assert_eq!(all.next_cursor, None);
}

/// Decision 8: `q` matches a substring of the title, cwd, branch or id, with
/// `%`, `_` and `\` taken literally and ASCII case ignored; while it is set,
/// the lifecycle filter is bypassed (frontend §5, F-10).
#[test]
fn search_matches_title_cwd_branch_and_id_literally_across_every_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let at = format!("{T}1.000Z");
    let store = listed_store(
        dir.path(),
        &[
            ("percent", &at, Some("100% done"), "/tmp"),
            ("plain", &at, Some("1000 done"), "/tmp"),
            ("under", &at, None, "/src/my_app"),
            ("nounder", &at, None, "/src/myXapp"),
            ("slash", &at, Some("a\\b"), "/tmp"),
            ("branchy", &at, None, "/tmp"),
        ],
    );
    rusqlite::Connection::open(dir.path().join("hennery.db"))
        .unwrap()
        .execute(
            "UPDATE sessions SET git_branch = 'feat/list-search' WHERE id = 'branchy'",
            [],
        )
        .unwrap();
    store.close_now("percent").unwrap();
    let found = |q: &str| {
        let active = ["starting"];
        let mut found = ids(&store
            .list(&ListQuery {
                search: Some(q),
                lifecycles: Some(&active),
                ..ListQuery::default()
            })
            .unwrap())
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        found.sort();
        found
    };
    assert_eq!(found("100%"), ["percent"]);
    assert_eq!(found("DONE"), ["percent", "plain"]);
    assert_eq!(found("my_app"), ["under"]);
    assert_eq!(found("a\\b"), ["slash"]);
    assert_eq!(found("LIST-SEARCH"), ["branchy"]);
    assert_eq!(found("nounde"), ["nounder"]);
    assert_eq!(found("%"), ["percent"]);
}

/// Without `q`, only the named lifecycles are listed ("Hide closed" names
/// all but `closed`); a presumed park is `parked`.
#[test]
fn the_lifecycle_filter_keeps_only_the_named_lifecycles() {
    let dir = tempfile::tempdir().unwrap();
    let at = format!("{T}1.000Z");
    let store = listed_store(dir.path(), &[("open", &at, None, "/tmp"), ("shut", &at, None, "/tmp")]);
    store.close_now("shut").unwrap();
    let only = |lifecycles: &[&str]| {
        ids(&store
            .list(&ListQuery {
                lifecycles: Some(lifecycles),
                ..ListQuery::default()
            })
            .unwrap())
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>()
    };
    assert_eq!(only(&["starting", "active", "parked", "failed"]), ["open"]);
    assert_eq!(only(&["closed"]), ["shut"]);
}

/// The list serves each item bounded (the review's A1); the detail's is as
/// stored.
#[test]
fn the_list_serves_items_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = format!("/home/someone/{}webapp", "deep/".repeat(40));
    let store = listed_store(dir.path(), &[("s1", &format!("{T}1.000Z"), None, &cwd)]);
    let listed = store.list(&ListQuery::default()).unwrap().sessions.remove(0);
    assert!(
        listed.cwd.starts_with('…') && listed.cwd.ends_with("deep/webapp"),
        "{}",
        listed.cwd
    );
    assert_eq!(store.session_item("s1").unwrap().unwrap().cwd, cwd);
}

```

In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

```rust
        include_str!("../../hennery-sessions/src/store.rs"),
        81,
```

with:

```rust
        include_str!("../../hennery-sessions/src/store.rs"),
        82,
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")), "{body}");
}

```

with:

```rust
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")), "{body}");
}

// Plan 6b: `GET /api/sessions` (ACP core §9).

/// Pages, searches and filters; every parameter it cannot honour is
/// refused, never ignored: `hat` until sessions have hats (plan 5).
#[tokio::test]
async fn the_session_list_pages_searches_filters_and_refuses_what_it_cannot_honour() {
    let collector = Collector::start().await;
    let store = &collector.state.store;
    for (id, cwd) in [("a", "/src/alpha"), ("b", "/src/beta"), ("c", "/src/gamma")] {
        store.create_session(id, HOST, "fake", cwd).unwrap();
        // One millisecond apart at least, so the order is known.
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    store.close_now("a").unwrap();
    let c = client(&collector);
    let list = async |query: &str| get(&c, collector.url(&format!("/api/sessions{query}"))).await;
    let ids = |page: &Value| -> Vec<String> {
        page["sessions"].as_array().unwrap().iter().map(|s| s["session_id"].as_str().unwrap().to_string()).collect()
    };

    let (status, page) = list("").await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(ids(&page), ["a", "c", "b"]);
    assert_eq!(page.get("next_cursor"), None);
    assert_eq!(page["sessions"][0]["lifecycle"], "closed");

    let (_, first) = list("?limit=2").await;
    assert_eq!(ids(&first), ["a", "c"]);
    let cursor = first["next_cursor"].as_str().unwrap().to_string();
    let (_, rest) = list(&format!("?limit=2&cursor={cursor}")).await;
    assert_eq!(ids(&rest), ["b"]);
    assert_eq!(rest.get("next_cursor"), None);
    // Clamped to at least one.
    assert_eq!(ids(&list("?limit=0").await.1), ["a"]);

    let (_, open) = list("?lifecycle=starting,active,parked,failed").await;
    assert_eq!(ids(&open), ["c", "b"]);
    // A search bypasses the lifecycle filter.
    let (_, found) = list("?lifecycle=starting&q=ALPHA").await;
    assert_eq!(ids(&found), ["a"]);
    let (_, blank) = list("?q=%20%20").await;
    assert_eq!(ids(&blank), ["a", "c", "b"]);

    for (query, code) in [
        ("?hat=work", "hat_filter_unavailable"),
        ("?hat=", "hat_filter_unavailable"),
        ("?cursor=zz", "invalid_cursor"),
        ("?limit=x", "invalid"),
        ("?limit=-1", "invalid"),
        ("?lifecycle=active,bogus", "invalid"),
    ] {
        let (status, body) = list(query).await;
        assert_eq!((status, body["code"].as_str()), (400, Some(code)), "{query}: {body}");
    }
    let (status, body) = list(&format!("?q={}", "q".repeat(201))).await;
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    // A control character would cut the pattern short (a NUL ends it).
    let (status, body) = list("?q=a%00b").await;
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    assert_eq!(list(&format!("?q={}", "q".repeat(200))).await.0, 200);
}

```

- [ ] **Step 2: Run the tests to see them fail**

Run: `nix develop -c cargo test -p hennery-sessions --locked`
Expected: FAIL to compile: ``cannot find type `Cursor` in this scope``, ``cannot find function `like_pattern` in this scope``, ``cannot find function `list_statement` in this scope``.

- [ ] **Step 3: The list**

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::LifecycleResponse,
        rest::SessionItem,
        rest::SessionDetail,
```

with:

```rust
        rest::LifecycleResponse,
        rest::SessionItem,
        rest::SessionPage,
        rest::SessionDetail,
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::SessionItem,
        rest::SessionDetail,
```

with:

```rust
        rest::SessionItem,
        rest::SessionPage,
        rest::SessionDetail,
```

In `crates/hennery-proto/src/rest.rs`, replace:

```rust
        self.host_id = cut(&self.host_id, usize::MAX, HOST_ID_MAX_JSON_BYTES);
        self
    }
}

```

with:

```rust
        self.host_id = cut(&self.host_id, usize::MAX, HOST_ID_MAX_JSON_BYTES);
        self
    }
}

/// `GET /api/sessions` (ACP core §9): one page of the session list, newest
/// `last_event_at` first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionPage {
    pub sessions: Vec<SessionItem>,
    /// Where the next page starts, for `cursor` (opaque); absent on the last
    /// page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub next_cursor: Option<String>,
}

```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
use crate::store::{AnswerSubmission, ResumeRequest, Store};
```

with:

```rust
use crate::store::{
    AnswerSubmission, Cursor, LIFECYCLES, LIST_DEFAULT_LIMIT, LIST_MAX_LIMIT, ListQuery, ResumeRequest, Store,
};
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        .route("/api/sessions", post(start_session))
```

with:

```rust
        .route("/api/sessions", post(start_session).get(list_sessions))
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        // code (`Undo::Start`).
        Err(err) => request_failed(err),
    }
```

with:

```rust
        // code (`Undo::Start`).
        Err(err) => request_failed(err),
    }
}

/// The longest search the list takes, in characters (plan 6b decision 8).
const SEARCH_MAX_CHARS: usize = 200;

/// `GET /api/sessions`' query (ACP core §9). Every value is read as text,
/// so a malformed one gets an `ApiError`.
#[derive(Deserialize)]
struct ListParams {
    cursor: Option<String>,
    limit: Option<String>,
    q: Option<String>,
    hat: Option<String>,
    lifecycle: Option<String>,
}

/// The session list (ACP core §9; plan 6b decision 8): newest
/// `last_event_at` first, in pages of `limit` (50 by default, clamped to
/// 1..=200) after `cursor`; only the comma-separated `lifecycle`s, unless
/// `q` searches the title, cwd, branch and id of every session.
async fn list_sessions(State(state): State<AppState>, Query(params): Query<ListParams>) -> Response {
    // Sessions have no hat until hats (plan 5) gives them `hat_id`; a
    // filter that cannot be honoured is refused, never ignored. The seam
    // hats fills.
    if params.hat.is_some() {
        return error(
            StatusCode::BAD_REQUEST,
            "hat_filter_unavailable",
            "sessions have no hat yet, so the list cannot be filtered by one",
        );
    }
    let limit = match params.limit.as_deref().map(str::parse::<u32>) {
        None => LIST_DEFAULT_LIMIT,
        Some(Ok(limit)) => limit.clamp(1, LIST_MAX_LIMIT),
        Some(Err(_)) => return error(StatusCode::BAD_REQUEST, "invalid", "limit must be a whole number"),
    };
    let cursor = match params.cursor.as_deref().map(Cursor::decode) {
        None => None,
        Some(Some(cursor)) => Some(cursor),
        Some(None) => {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_cursor",
                "not a cursor this list gave out",
            );
        }
    };
    let search = params.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    // A control character would cut the pattern short (a NUL ends it: the
    // second review's P4).
    if search.is_some_and(|q| q.chars().count() > SEARCH_MAX_CHARS || q.chars().any(char::is_control)) {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid",
            format!("a search is at most {SEARCH_MAX_CHARS} characters, with no control characters"),
        );
    }
    let mut lifecycles: Vec<&str> = Vec::new();
    for name in params
        .lifecycle
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
    {
        if name.is_empty() || lifecycles.contains(&name) {
            continue;
        }
        if !LIFECYCLES.contains(&name) {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid",
                format!("lifecycle is a list of {}", LIFECYCLES.join(", ")),
            );
        }
        lifecycles.push(name);
    }
    let query = ListQuery {
        after: cursor.as_ref(),
        limit,
        lifecycles: (!lifecycles.is_empty()).then_some(lifecycles.as_slice()),
        search,
    };
    match state.store.list(&query) {
        Ok(page) => Json(page).into_response(),
        Err(err) => internal(err),
    }
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    AnswerRequest, EventDto, PendingItem, PendingState, SessionCatalog, SessionItem, TITLE_MAX_CHARS,
```

with:

```rust
    AnswerRequest, EventDto, PendingItem, PendingState, SessionCatalog, SessionItem, SessionPage, TITLE_MAX_CHARS,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust

/// A session's `model`, `mode` and `config_axes` columns.
```

with:

```rust

/// Every lifecycle a session can be in (ACP core §4.2).
pub const LIFECYCLES: [&str; 5] = ["starting", "active", "parked", "closed", "failed"];

/// The session list's page size when none is asked for, and the largest
/// it serves (plan 6b decision 8).
pub const LIST_DEFAULT_LIMIT: u32 = 50;
pub const LIST_MAX_LIMIT: u32 = 200;

/// A position in the session list: the last item of a page, by its sort
/// key (decision 8). It goes out opaque, as hex, and is a position only, so
/// a cursor from another query is harmless.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub last_event_at: String,
    pub session_id: String,
}

impl Cursor {
    pub fn encode(&self) -> String {
        hex::encode(format!("{}\n{}", self.last_event_at, self.session_id))
    }

    /// `None` for anything `encode` did not make: not hex, not UTF-8, no
    /// separator, a part empty or longer than 64 bytes, or more than 256
    /// characters in all (the review's O4).
    pub fn decode(cursor: &str) -> Option<Self> {
        if cursor.len() > 256 {
            return None;
        }
        let text = String::from_utf8(hex::decode(cursor).ok()?).ok()?;
        let (at, id) = text.split_once('\n')?;
        let fits = |part: &str| (1..=64).contains(&part.len());
        (fits(at) && fits(id)).then(|| Self {
            last_event_at: at.to_string(),
            session_id: id.to_string(),
        })
    }
}

/// What a page of the session list holds (ACP core §9; plan 6b decision 8).
#[derive(Debug, Clone, Copy)]
pub struct ListQuery<'a> {
    /// Start after this position (the previous page's `next_cursor`).
    pub after: Option<&'a Cursor>,
    /// At most this many sessions, clamped to 1..=`LIST_MAX_LIMIT`.
    pub limit: u32,
    /// Only sessions in one of these lifecycles (names from `LIFECYCLES`);
    /// all when `None`. Ignored while `search` is set (frontend §5).
    pub lifecycles: Option<&'a [&'a str]>,
    /// Only sessions whose title, cwd, branch or id holds this text.
    pub search: Option<&'a str>,
}

impl Default for ListQuery<'_> {
    fn default() -> Self {
        Self {
            after: None,
            limit: LIST_DEFAULT_LIMIT,
            lifecycles: None,
            search: None,
        }
    }
}

/// `search` as a `LIKE` pattern that matches it anywhere, its `%`, `_` and
/// `\` taken literally (`ESCAPE '\'`). SQLite's `LIKE` ignores case for
/// ASCII letters only.
fn like_pattern(search: &str) -> String {
    let mut pattern = String::from("%");
    for c in search.chars() {
        if matches!(c, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern.push('%');
    pattern
}

/// The session list's one statement (decision 8): the owner's sessions
/// after the cursor `(?2, ?3)`, in one of the lifecycles `?4`…`?8` (a NULL
/// slot matches nothing), matching the pattern `?9` unless it is NULL, the
/// newest `last_event_at` first and the id breaking ties, at most `?10`. It
/// walks `sessions_by_recency` and sorts nothing (the review's A11).
fn list_statement() -> String {
    format!(
        "SELECT {SESSION_ITEM_COLUMNS} FROM sessions
         WHERE owner_id = ?1 AND (last_event_at, id) < (?2, ?3) AND lifecycle IN (?4, ?5, ?6, ?7, ?8)
             AND (?9 IS NULL OR title LIKE ?9 ESCAPE '\\' OR cwd LIKE ?9 ESCAPE '\\'
                  OR git_branch LIKE ?9 ESCAPE '\\' OR id LIKE ?9 ESCAPE '\\')
         ORDER BY last_event_at DESC, id DESC LIMIT ?10"
    )
}

/// A session's `model`, `mode` and `config_axes` columns.
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
                read_item,
            )
            .optional()?)
    }

```

with:

```rust
                read_item,
            )
            .optional()?)
    }

    /// One page of the session list (ACP core §9; decision 8), each item
    /// `bounded`, with where the next page starts if there is one.
    pub fn list(&self, query: &ListQuery<'_>) -> Result<SessionPage> {
        let limit = query.limit.clamp(1, LIST_MAX_LIMIT);
        // A page with no cursor starts above every stamp.
        let (at, id) = match query.after {
            Some(cursor) => (cursor.last_event_at.as_str(), cursor.session_id.as_str()),
            None => ("\u{10FFFF}", ""),
        };
        let pattern = query.search.map(like_pattern);
        let slots: [Option<&str>; 5] = match query.lifecycles.filter(|_| pattern.is_none()) {
            Some(named) => std::array::from_fn(|i| named.get(i).copied()),
            None => LIFECYCLES.map(Some),
        };
        let conn = self.conn();
        let mut stmt = conn.prepare(&list_statement())?;
        let rows = stmt.query_map(
            params![
                self.owner,
                at,
                id,
                slots[0],
                slots[1],
                slots[2],
                slots[3],
                slots[4],
                pattern,
                limit + 1
            ],
            read_item,
        )?;
        let mut sessions = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let next_cursor = if sessions.len() > limit as usize {
            sessions.truncate(limit as usize);
            sessions.last().map(|last| {
                Cursor {
                    last_event_at: last.last_event_at.clone(),
                    session_id: last.session_id.clone(),
                }
                .encode()
            })
        } else {
            None
        };
        Ok(SessionPage {
            sessions: sessions.into_iter().map(SessionItem::bounded).collect(),
            next_cursor,
        })
    }

```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
                params!["o", "~", "", "active", "parked", None::<String>, None::<String>, None::<String>, "%x%", 50],
```

with:

```rust
                params![
                    "o",
                    "~",
                    "",
                    "active",
                    "parked",
                    None::<String>,
                    None::<String>,
                    None::<String>,
                    "%x%",
                    50
                ],
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
        page["sessions"].as_array().unwrap().iter().map(|s| s["session_id"].as_str().unwrap().to_string()).collect()
```

with:

```rust
        page["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["session_id"].as_str().unwrap().to_string())
            .collect()
```

Run: `cargo run -p hennery-proto --bin gen`

- [ ] **Step 4: Run the tests to see them pass, then the checks**

Run: `nix develop -c cargo test -p hennery-sessions --locked`
Expected: PASS (`store`: 80 passed). Then the five checks: 559 tests.

- [ ] **Step 5: Revert-probes**

Each probe below was applied to the task's commit, its test run, and the change undone. Every one was caught (7 of 7):
- a search's wildcards not escaped (`store.rs`): `search_matches_title_cwd_branch_and_id_literally` fails.
- the lifecycle filter applied during a search (`store.rs`): `search_matches_title_cwd_branch_and_id_literally` fails.
- the list without its owner filter (`store.rs`): `another_owners_sessions_are_invisible_to_the_store` fails.
- the list ordered off the index (`store.rs`): `the_list_walks_the_recency_index` fails.
- a `hat` filter accepted and ignored (`api.rs`): `the_session_list_pages_searches_filters` fails.
- a cursor taken without its length check (`store.rs`): `a_cursor_round_trips_and_a_malformed_one_is_refused` fails.
- a search with a control character taken (`api.rs`): `the_session_list_pages_searches_filters` fails.

- [ ] **Step 6: Commit, push, and open PR 1**

`git -c commit.gpgsign=false commit -m "feat(sessions): list and search sessions"`

---

## 6b-ii: the git state (PR 2)

### Task 6: The `git_state` body, and the git columns

**Files:**
- Modify: `crates/hennery-proto/src/frames.rs` (`SessionBody::GitState`), generated files; `crates/hennery-sessions/src/store.rs` (the `git_state` arm, `SessionRow.git_worktree`, `SessionRow.base_commit`)
- Test: `crates/hennery-sessions/tests/store.rs`, `crates/hennery-proto/tests/frames.rs`, `crates/hennery-testkit/tests/host_session.rs` (`kinds` names it), `owner_filter.rs` (floor 83)

**Interfaces:**
- Produces: `SessionBody::GitState { branch, dirty, worktree, head, base_commit }`; `SessionRow.git_worktree: Option<bool>`, `SessionRow.base_commit: Option<String>`.
- Consumes: Task 1's columns, Task 3's `one_line`, Task 4's branch caps.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
        assert!(serde_json::from_value::<AnswerRequest>(bad.clone()).is_err(), "{bad}");
    }
}

```

with:

```rust
        assert!(serde_json::from_value::<AnswerRequest>(bad.clone()).is_err(), "{bad}");
    }
}

/// Plan 6b-ii: `git_state` on the wire, with its optionals left out.
#[test]
fn git_state_round_trips_and_leaves_out_absent_optionals() {
    let detached = SessionBody::GitState {
        branch: None,
        dirty: true,
        worktree: false,
        head: None,
        base_commit: None,
    };
    let expected = json!({"kind": "git_state", "dirty": true, "worktree": false});
    assert_eq!(serde_json::to_value(&detached).unwrap(), expected);
    assert_eq!(serde_json::from_value::<SessionBody>(expected).unwrap(), detached);
    let full = json!({
        "kind": "git_state", "branch": "main", "dirty": false, "worktree": true,
        "head": "c0ffee", "base_commit": "c0ffee"
    });
    let body: SessionBody = serde_json::from_value(full.clone()).unwrap();
    assert_eq!(serde_json::to_value(&body).unwrap(), full);
}

```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    assert_eq!(store.session_item("s1").unwrap().unwrap().cwd, cwd);
}

```

with:

```rust
    assert_eq!(store.session_item("s1").unwrap().unwrap().cwd, cwd);
}

// Plan 6b-ii: the git state (ACP core §3.2, §7, §8).

fn git(branch: Option<&str>, dirty: bool, base: Option<&str>) -> SessionBody {
    SessionBody::GitState {
        branch: branch.map(str::to_string),
        dirty,
        worktree: false,
        head: Some("c0ffee".into()),
        base_commit: base.map(str::to_string),
    }
}

/// Decision 11: a `git_state` fills the git columns, the branch on one
/// line and capped like the title; `base_commit` is recorded once; one
/// that changes nothing is stored but not listed (the review's O1), and
/// does not move the session up the list.
#[test]
fn a_git_state_fills_the_git_columns_and_records_the_base_commit_once() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    let created = store.ingest("s1", 2, &git(Some("main"), true, Some("c0ffee"))).unwrap();
    assert_eq!(kinds(&created), ["git_state"]);
    let row = store.session("s1").unwrap().unwrap();
    assert_eq!(
        (row.git_worktree, row.base_commit.as_deref()),
        (Some(false), Some("c0ffee"))
    );
    let item = store.session_item("s1").unwrap().unwrap();
    assert_eq!((item.git_branch.as_deref(), item.git_dirty), (Some("main"), Some(true)));

    let recency = store.session("s1").unwrap().unwrap().last_event_id;
    assert!(
        store
            .ingest("s1", 3, &git(Some("main"), true, Some("decade")))
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.session("s1").unwrap().unwrap().last_event_id, recency);
    assert_eq!(listed(&store, "s1"), ["session_started", "git_state"]);

    store
        .ingest(
            "s1",
            4,
            &git(
                Some(&format!("feat/\u{202E}{}", "x".repeat(200))),
                false,
                Some("decade"),
            ),
        )
        .unwrap();
    let row = store.session("s1").unwrap().unwrap();
    assert_eq!(row.base_commit.as_deref(), Some("c0ffee"));
    let item = store.session_item("s1").unwrap().unwrap();
    assert_eq!(item.git_branch, Some(format!("feat/{}", "x".repeat(115))));
    assert_eq!(item.git_dirty, Some(false));
    // Detached: no branch.
    store.ingest("s1", 5, &git(None, false, None)).unwrap();
    assert_eq!(store.session_item("s1").unwrap().unwrap().git_branch, None);
}

/// A git state for a closed session changes nothing; a base commit that is
/// not a commit id is not recorded.
#[test]
fn a_git_state_for_a_closed_session_or_with_a_strange_base_changes_nothing() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store
        .ingest("s1", 2, &git(Some("main"), false, Some("not a commit")))
        .unwrap();
    assert_eq!(store.session("s1").unwrap().unwrap().base_commit, None);
    store.close_now("s1").unwrap();
    assert!(
        store
            .ingest("s1", 3, &git(Some("other"), true, Some("c0ffee")))
            .unwrap()
            .is_empty()
    );
    let item = store.session_item("s1").unwrap().unwrap();
    assert_eq!(
        (item.git_branch.as_deref(), item.git_dirty),
        (Some("main"), Some(false))
    );
}

```

In `crates/hennery-testkit/tests/host_session.rs`, replace:

```rust
                SessionBody::AnswerResult { delivered, .. } => format!("answer_result:{delivered}"),
```

with:

```rust
                SessionBody::AnswerResult { delivered, .. } => format!("answer_result:{delivered}"),
                SessionBody::GitState { .. } => "git_state".to_string(),
```

In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

```rust
        include_str!("../../hennery-sessions/src/store.rs"),
        82,
```

with:

```rust
        include_str!("../../hennery-sessions/src/store.rs"),
        83,
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `nix develop -c cargo test -p hennery-sessions --locked --test store`
Expected: FAIL to compile: ``no variant named `GitState` found for enum `SessionBody` ``, ``no field `base_commit` on type `SessionRow` ``.

- [ ] **Step 3: The body and its columns**

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
        delivered: bool,
    },
}
```

with:

```rust
        delivered: bool,
    },
    /// The git state of the session's cwd (ACP core §3.2, §7): after the
    /// start and after each turn, when `cwd` is in a work tree and `git`
    /// answered within 3 s (plan 6b-ii decision 11). Never in place of, or
    /// ahead of, the `turn_ended` it follows.
    GitState {
        /// The branch checked out; absent when detached.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(type = "string | undefined", optional)]
        branch: Option<String>,
        /// Any staged, unstaged or untracked change.
        dirty: bool,
        /// `cwd` is in a linked work tree (`git worktree add`), not the
        /// repository's main one.
        worktree: bool,
        /// The commit checked out; absent on a branch with no commit yet.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(type = "string | undefined", optional)]
        head: Option<String>,
        /// On the first state after a new session's start: the commit it
        /// started from. The collector records it once.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(type = "string | undefined", optional)]
        base_commit: Option<String>,
    },
}
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    AnswerRequest, EventDto, PendingItem, PendingState, SessionCatalog, SessionItem, SessionPage, TITLE_MAX_CHARS,
    TITLE_MAX_JSON_BYTES, json_char_width,
```

with:

```rust
    AnswerRequest, BRANCH_MAX_CHARS, BRANCH_MAX_JSON_BYTES, EventDto, PendingItem, PendingState, SessionCatalog,
    SessionItem, SessionPage, TITLE_MAX_CHARS, TITLE_MAX_JSON_BYTES, json_char_width,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    pub title: Option<String>,
```

with:

```rust
    pub title: Option<String>,
    /// Whether `cwd` was in a linked work tree, as the host last reported.
    pub git_worktree: Option<bool>,
    /// The commit a new session started from, recorded once (ACP core §7).
    pub base_commit: Option<String>,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
                        presumed_parked, model, mode, config_axes, last_event_at, last_event_id, title
```

with:

```rust
                        presumed_parked, model, mode, config_axes, last_event_at, last_event_id, title,
                        git_worktree, base_commit
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
                        title: r.get(15)?,
```

with:

```rust
                        title: r.get(15)?,
                        git_worktree: r.get(16)?,
                        base_commit: r.get(17)?,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
            }
            // Diagnostics only, with no transition of their own: an
```

with:

```rust
            }
            SessionBody::GitState {
                branch,
                dirty,
                worktree,
                base_commit,
                ..
            } => {
                // The branch on one line and capped, like the title; the
                // base commit once, and only a commit id (plan 6b-ii
                // decision 11). A state that changes nothing is kept as the
                // idempotency key only, like a verdict that changes nothing
                // (the review's O1): not listed, and the session does not
                // move up the list.
                let branch = branch
                    .as_deref()
                    .and_then(|b| one_line(b, BRANCH_MAX_CHARS, BRANCH_MAX_JSON_BYTES));
                let base = base_commit
                    .as_deref()
                    .filter(|c| (4..=64).contains(&c.len()) && c.chars().all(|c| c.is_ascii_hexdigit()));
                let changed = if fact_applies(&tx, &self.owner, session_id, None)? {
                    tx.execute(
                        "UPDATE sessions SET git_branch = ?2, git_dirty = ?3, git_worktree = ?4,
                             base_commit = COALESCE(base_commit, ?5)
                         WHERE id = ?1 AND owner_id = ?6
                             AND (git_branch IS NOT ?2 OR git_dirty IS NOT ?3 OR git_worktree IS NOT ?4
                                  OR (base_commit IS NULL AND ?5 IS NOT NULL))",
                        params![session_id, branch, dirty, worktree, base, self.owner],
                    )?
                } else {
                    0
                };
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, &self.owner, fact_id)?;
                }
            }
            // Diagnostics only, with no transition of their own: an
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
        SessionBody::AnswerResult { .. } => "answer_result",
```

with:

```rust
        SessionBody::AnswerResult { .. } => "answer_result",
        SessionBody::GitState { .. } => "git_state",
```

Run: `cargo run -p hennery-proto --bin gen`

- [ ] **Step 4: Run the tests to see them pass, then the checks**

Run: `nix develop -c cargo test -p hennery-sessions --locked --test store`
Expected: PASS (82 passed). Then the five checks: 562 tests.

- [ ] **Step 5: Revert-probes**

Each probe below was applied to the task's commit, its test run, and the change undone. Every one was caught (4 of 4):
- `base_commit` overwritten by later states (`store.rs`): `a_git_state_fills_the_git_columns_and_records_the_base_commit_once` fails.
- a state that changes nothing listed (`store.rs`): `a_git_state_fills_the_git_columns_and_records_the_base_commit_once` fails.
- the branch stored as sent (`store.rs`): `a_git_state_fills_the_git_columns_and_records_the_base_commit_once` fails.
- a malformed base commit recorded (`store.rs`): `a_git_state_for_a_closed_session_or_with_a_strange_base_changes_nothing` fails.

- [ ] **Step 6: Commit**

`git -c commit.gpgsign=false commit -m "feat(sessions): the git_state body and the git columns"`

---

### Task 7: The host's git probe

**Files:**
- Create: `crates/hennery-host/src/git.rs`
- Modify: `crates/hennery-host/src/lib.rs`, `crates/hennery-host/src/session.rs` (`SessionOptions.git`, `Actor.probe`, `Inbound::Git`, `probe_git`, the two call sites), `crates/hennery-host/src/connection.rs` (`HostConfig.git`)
- Test: `git.rs`'s unit tests, `crates/hennery-testkit/tests/host_session.rs`

**Interfaces:**
- Produces: `hennery_host::git::{find_git() -> Option<PathBuf>, probe(&Path, &Path) -> Option<GitState>, GitState, PROBE_TIMEOUT}`; `SessionOptions.git: Option<PathBuf>` (default `None`: no probe); `HostConfig.git: Option<PathBuf>` (found at `HostConfig::new`).
- Consumes: Task 6's body.

- [ ] **Step 1: Write the failing tests**

`git.rs` starts as its unit tests, so they fail on the probe they name. They and the actor's run a real `git` (skipping, and saying so, without one) and, for a hung one, a script that records its pid and its child's.

Create `crates/hennery-host/src/git.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsStr;

    /// `git` run by a test to set a repository up, not by the probe.
    fn sh_git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new(find_git().unwrap())
            .current_dir(dir)
            .args(["-c", "user.name=test", "-c", "user.email=test@example.invalid"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// The review's A9 and decision 11: every `GIT_*` the host inherited is
    /// removed, and so is what an agent never inherits either (the second
    /// review's B2); `GIT_OPTIONAL_LOCKS=0` keeps `git status` off
    /// `index.lock`, and discovery stops below an absolute `HOME`.
    #[test]
    fn a_command_is_isolated_from_the_hosts_git_environment() {
        let host: Vec<(OsString, OsString)> = [
            ("GIT_DIR", "/elsewhere/.git"),
            ("GIT_INDEX_FILE", "/elsewhere/index"),
            ("GIT_CONFIG_PARAMETERS", "'core.fsmonitor=evil'"),
            ("HOME", "/home/someone"),
            ("PATH", "/usr/bin"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        let cmd = command(
            Path::new("/usr/bin/git"),
            Path::new("/tmp"),
            &["status"],
            host.into_iter(),
        );
        let envs: HashMap<OsString, Option<OsString>> = cmd
            .as_std()
            .get_envs()
            .map(|(k, v)| (k.to_owned(), v.map(|v| v.to_owned())))
            .collect();
        for removed in ["GIT_DIR", "GIT_INDEX_FILE", "GIT_CONFIG_PARAMETERS"] {
            assert_eq!(envs.get(OsStr::new(removed)), Some(&None), "{removed}");
        }
        for removed in crate::adapter::NESTING_VARS
            .iter()
            .chain(crate::adapter::HOST_SECRET_VARS)
        {
            assert_eq!(envs.get(OsStr::new(removed)), Some(&None), "{removed}");
        }
        assert_eq!(envs[OsStr::new("GIT_OPTIONAL_LOCKS")], Some("0".into()));
        assert_eq!(
            envs[OsStr::new("GIT_CEILING_DIRECTORIES")],
            Some("/home/someone".into())
        );
        assert!(!envs.contains_key(OsStr::new("PATH")));
        let args: Vec<&OsStr> = cmd.as_std().get_args().collect();
        assert_eq!(args, ["-c", "core.fsmonitor=false", "status"]);
        // A relative `HOME` sets no ceiling.
        let relative = [(OsString::from("HOME"), OsString::from("home"))];
        let cmd = command(Path::new("/usr/bin/git"), Path::new("/tmp"), &[], relative.into_iter());
        assert!(cmd.as_std().get_envs().all(|(k, _)| k != "GIT_CEILING_DIRECTORIES"));
    }

    /// The review's A9: only an absolute `PATH` entry is searched, so a
    /// `git` the agent writes into its cwd (a relative entry such as `.`)
    /// never runs.
    #[test]
    fn git_is_found_on_absolute_path_entries_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let git = bin.join("git");
        std::fs::write(&git, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = |entries: &[&Path]| Some(std::env::join_paths(entries).unwrap());
        // `bin` again, but relative to the working directory: it holds a
        // `git`, and is skipped.
        let here = std::env::current_dir().unwrap();
        let up: PathBuf = here.components().skip(1).map(|_| "..").collect();
        let relative = up.join(bin.strip_prefix("/").unwrap());
        assert!(
            relative.is_relative() && relative.join("git").exists(),
            "{}",
            relative.display()
        );
        assert_eq!(find_git_in(path(&[&relative])), None);
        assert_eq!(find_git_in(path(&[&relative, &bin])), Some(git.clone()));
        assert_eq!(find_git_in(Some("".into())), None);
        assert_eq!(find_git_in(None), None);
        // Not executable: skipped.
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(find_git_in(path(&[&bin])), None);
    }

    /// The second review's P5: a `git` the probe cannot use (older than
    /// 2.31, or one that does not answer `--version`, as a missing
    /// developer tool does) is not used at all.
    #[test]
    fn only_git_2_31_or_newer_is_used() {
        assert_eq!(parse_version("git version 2.39.5 (Apple Git-154)\n"), Some((2, 39)));
        assert_eq!(parse_version("git version 2.31.0"), Some((2, 31)));
        assert_eq!(parse_version("git version 2.30.2"), Some((2, 30)));
        assert_eq!(parse_version("git version 3.0"), Some((3, 0)));
        assert_eq!(parse_version("xcrun: error: invalid active developer path"), None);
        assert_eq!(parse_version(""), None);
        assert!(usable(Some((2, 31))) && usable(Some((3, 0))));
        assert!(!usable(Some((2, 30))) && !usable(Some((1, 99))) && !usable(None));
    }

    #[test]
    fn status_headers_give_the_branch_and_the_head() {
        let (mut branch, mut head) = (None, None);
        header("branch.oid c0ffee", &mut branch, &mut head);
        header("branch.head main", &mut branch, &mut head);
        header("branch.upstream origin/main", &mut branch, &mut head);
        assert_eq!((branch.as_deref(), head.as_deref()), (Some("main"), Some("c0ffee")));
        header("branch.oid (initial)", &mut branch, &mut head);
        header("branch.head (detached)", &mut branch, &mut head);
        assert_eq!((branch, head), (None, None));
    }

    /// Decision 11 and the review's A8: a linked work tree is reported as
    /// one, the main checkout is not, even probed from a subdirectory
    /// (where git would print a relative common dir); changes make it
    /// dirty; outside a work tree there is no state.
    #[tokio::test]
    async fn a_probe_reports_the_branch_changes_and_linked_work_trees() {
        let Some(git) = find_git() else {
            eprintln!("no git on PATH: skipped");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        std::fs::create_dir_all(main.join("sub")).unwrap();
        sh_git(&main, &["init", "-q", "-b", "trunk"]);
        sh_git(&main, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let head = sh_git(&main, &["rev-parse", "HEAD"]);
        let state = probe(&git, &main.join("sub")).await.unwrap();
        assert_eq!(
            state,
            GitState {
                branch: Some("trunk".into()),
                dirty: false,
                worktree: false,
                head: Some(head.clone()),
            }
        );
        std::fs::write(main.join("sub/new.txt"), "x").unwrap();
        assert!(probe(&git, &main).await.unwrap().dirty);
        sh_git(&main, &["worktree", "add", "-q", "-b", "side", "../linked"]);
        let linked = probe(&git, &dir.path().join("linked")).await.unwrap();
        assert_eq!(
            (linked.branch.as_deref(), linked.worktree, linked.dirty),
            (Some("side"), true, false)
        );
        sh_git(&main, &["checkout", "-q", "--detach"]);
        let detached = probe(&git, &main).await.unwrap();
        assert_eq!((detached.branch, detached.head), (None, Some(head)));
        let outside = tempfile::tempdir().unwrap();
        assert_eq!(probe(&git, outside.path()).await, None);
    }
}
```

In `crates/hennery-host/src/lib.rs`, replace:

```rust
pub mod connection;
```

with:

```rust
pub mod connection;
pub mod git;
```

In `crates/hennery-testkit/tests/host_session.rs`, replace:

```rust
            Some(vec![json!({"name": "review", "description": "Review the diff"})]),
            None,
            true
        )]
    );
}

```

with:

```rust
            Some(vec![json!({"name": "review", "description": "Review the diff"})]),
            None,
            true
        )]
    );
}

// Plan 6b-ii: the git probe (ACP core §3.2, §7).

/// `(branch, dirty, worktree, head, base_commit)` of every `git_state`.
type GitFields = (Option<String>, bool, bool, Option<String>, Option<String>);

fn git_states(frames: &[HostFrame]) -> Vec<GitFields> {
    frames
        .iter()
        .filter_map(|f| match f {
            HostFrame::Session {
                body:
                    SessionBody::GitState {
                        branch,
                        dirty,
                        worktree,
                        head,
                        base_commit,
                    },
                ..
            } => Some((branch.clone(), *dirty, *worktree, head.clone(), base_commit.clone())),
            _ => None,
        })
        .collect()
}

fn probing(git: Option<std::path::PathBuf>) -> SessionOptions {
    SessionOptions {
        git,
        ..SessionOptions::default()
    }
}

fn setup_git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=test", "-c", "user.email=test@example.invalid"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Decision 11: a `git_state` follows the start (with the commit a new
/// session started from) and each turn's end, never ahead of it.
#[tokio::test]
async fn the_git_state_follows_the_start_and_every_turn_end() {
    let Some(git) = hennery_host::git::find_git() else {
        eprintln!("no git on PATH: skipped");
        return;
    };
    let repo = tempfile::tempdir().unwrap();
    setup_git(repo.path(), &["init", "-q", "-b", "main"]);
    setup_git(repo.path(), &["commit", "-q", "--allow-empty", "-m", "first"]);
    let head = Some(setup_git(repo.path(), &["rev-parse", "HEAD"]));
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = session::spawn(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&FakeScript::default()),
        repo.path().to_path_buf(),
        probing(Some(git)),
    );
    let frames = wait_until(&uplink, has("git_state")).await;
    assert_eq!(kinds(&frames), ["session_started", "git_state"]);
    let main = Some("main".to_string());
    assert_eq!(
        git_states(&frames),
        [(main.clone(), false, false, head.clone(), head.clone())]
    );
    std::fs::write(repo.path().join("new.txt"), "x").unwrap();
    assert!(handle.send(prompt("r1", "t1")));
    let frames = wait_until(&uplink, |f| git_states(f).len() == 2).await;
    let kinds = kinds(&frames);
    let ended = kinds.iter().position(|k| k == "turn_ended").unwrap();
    assert_eq!(kinds[ended + 1..], ["git_state"]);
    assert_eq!(git_states(&frames)[1], (main, true, false, head, None));
}

/// The pids `pids` holds, one per line.
fn pids_in(pids: &Path) -> Vec<i32> {
    std::fs::read_to_string(pids)
        .unwrap_or_default()
        .lines()
        .map(|l| l.parse().unwrap())
        .collect()
}

/// Wait until `pids` holds at least `n` pids, and return them.
async fn wait_for_pids(pids: &Path, n: usize) -> Vec<i32> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let found = pids_in(pids);
        if found.len() >= n {
            return found;
        }
        assert!(tokio::time::Instant::now() < deadline, "only {found:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Whether `pid` is gone: no such process, or a zombie (killed, but not
/// reaped yet by its parent, which on Linux may take a while: fleet rule,
/// poll for a positive signal and never trust one read).
fn gone(pid: i32) -> bool {
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout);
    stat.trim().is_empty() || stat.trim_start().starts_with('Z')
}

/// Wait until none of `pids` runs, at most `within`.
async fn wait_dead_all(pids: &[i32], within: Duration) {
    let deadline = tokio::time::Instant::now() + within;
    while !pids.iter().all(|pid| gone(*pid)) {
        assert!(tokio::time::Instant::now() < deadline, "{pids:?} still run");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// An executable script at `path`, ready to run: until every descriptor a
/// concurrent fork inherited while it was written is gone, running it fails
/// (`ETXTBSY`). Run with no arguments, the script must only exit.
fn write_script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    for attempt in 0.. {
        match Command::new(path).status() {
            Ok(status) => {
                assert!(status.success());
                return;
            }
            Err(err) if err.raw_os_error() == Some(libc::ETXTBSY) && attempt < 100 => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(err) => panic!("{err}"),
        }
    }
}

/// The second review's B1: a first turn that ends while the start's probe
/// still runs does not cost the base commit. The start's probe is awaited,
/// not aborted, and the states still arrive in order.
#[tokio::test]
async fn a_quick_first_turn_still_gets_the_base_commit() {
    let Some(real) = hennery_host::git::find_git() else {
        eprintln!("no git on PATH: skipped");
        return;
    };
    let repo = tempfile::tempdir().unwrap();
    setup_git(repo.path(), &["init", "-q", "-b", "main"]);
    setup_git(repo.path(), &["commit", "-q", "--allow-empty", "-m", "first"]);
    let head = Some(setup_git(repo.path(), &["rev-parse", "HEAD"]));
    let dir = tempfile::tempdir().unwrap();
    let git = dir.path().join("git");
    let release = dir.path().join("release");
    // The first call (the start's probe) waits until the test releases it,
    // at most ten seconds; every other call is the real git.
    write_script(
        &git,
        &format!(
            "#!/bin/sh\n[ $# -eq 0 ] && exit 0\nif [ ! -e {mark} ]; then : > {mark}\n  n=0; while [ ! -e {release} ] && [ $n -lt 1000 ]; do sleep 0.01; n=$((n+1)); done\nfi\nexec {real} \"$@\"\n",
            mark = dir.path().join("mark").display(),
            release = release.display(),
            real = real.display()
        ),
    );
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = session::spawn(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&FakeScript::default()),
        repo.path().to_path_buf(),
        probing(Some(git)),
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    let frames = wait_until(&uplink, has("turn_ended")).await;
    assert!(git_states(&frames).is_empty(), "the start's probe is still held");
    std::fs::write(&release, "").unwrap();
    let frames = wait_until(&uplink, |f| git_states(f).len() == 2).await;
    let main = Some("main".to_string());
    assert_eq!(
        git_states(&frames),
        [
            (main.clone(), false, false, head.clone(), head.clone()),
            (main, false, false, head, None)
        ]
    );
}

/// Decision 11: a `git` that hangs never delays a turn's end. A probe that
/// a newer one replaces is killed with whatever it started (its process
/// group), and so is one past its 3 s bound; neither reports a state.
#[tokio::test]
async fn a_hung_git_never_delays_a_turn_end_and_is_killed_with_its_group() {
    let dir = tempfile::tempdir().unwrap();
    let pids = dir.path().join("pids");
    let git = dir.path().join("git");
    write_script(
        &git,
        &format!(
            "#!/bin/sh\n[ $# -eq 0 ] && exit 0\necho $$ >> {0}\nsleep 30 &\necho $! >> {0}\nwait\n",
            pids.display()
        ),
    );
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = session::spawn(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&FakeScript::default()),
        std::env::temp_dir(),
        probing(Some(git)),
    );
    // The start's probe hangs: its git and git's child run.
    let after_start = wait_for_pids(&pids, 2).await;
    let asked = tokio::time::Instant::now();
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_ended")).await;
    assert!(asked.elapsed() < Duration::from_secs(2), "{:?}", asked.elapsed());
    // The turn's probe replaced it, killing its whole group.
    wait_dead_all(&after_start, Duration::from_secs(5)).await;
    // The turn's probe hangs too, and is killed past its bound.
    let after_turn = wait_for_pids(&pids, 4).await;
    wait_dead_all(
        &after_turn[2..],
        hennery_host::git::PROBE_TIMEOUT + Duration::from_secs(3),
    )
    .await;
    assert!(git_states(&uplink.pending().unwrap()).is_empty());
}

/// Outside a work tree, or with no `git` at all, nothing is reported.
#[tokio::test]
async fn no_git_state_is_reported_outside_a_work_tree_or_without_git() {
    let outside = tempfile::tempdir().unwrap();
    for git in [hennery_host::git::find_git(), None] {
        let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
        let handle = session::spawn(
            uplink.clone(),
            "r0".into(),
            "s1".into(),
            fake_with(&FakeScript::default()),
            outside.path().to_path_buf(),
            probing(git),
        );
        wait_until(&uplink, has("session_started")).await;
        assert!(handle.send(prompt("r1", "t1")));
        wait_until(&uplink, has("turn_ended")).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(git_states(&uplink.pending().unwrap()).is_empty());
    }
}

```

- [ ] **Step 2: Run the tests to see them fail**

Run: `nix develop -c cargo test -p hennery-host --locked --lib git`
Expected: FAIL to compile: ``cannot find function `find_git_in` in this scope``, ``cannot find function `command` in this scope``, ``cannot find struct, variant or union type `GitState` in this scope``.

- [ ] **Step 3: The probe, and the actor's use of it**

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    pub healthy_after: Duration,
```

with:

```rust
    pub healthy_after: Duration,
    /// `git` for the sessions' git probe (ACP core §7), found on `PATH` once,
    /// when the host starts; `None`: no `git_state`.
    pub git: Option<PathBuf>,
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            healthy_after: Duration::from_secs(60),
```

with:

```rust
            healthy_after: Duration::from_secs(60),
            git: crate::git::find_git(),
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            idle_timeout: (!self.idle_timeout.is_zero()).then_some(self.idle_timeout),
```

with:

```rust
            idle_timeout: (!self.idle_timeout.is_zero()).then_some(self.idle_timeout),
            git: self.git.clone(),
```

In `crates/hennery-host/src/git.rs`, replace:

```rust
#[cfg(test)]
```

with:

```rust
//! The host's git probe (ACP core §3.2, §7; plan 6b-ii decision 11): the
//! branch, changes and work tree of a session's cwd, after its start and
//! after each turn.
//!
//! The session actor runs it on a task of its own, bounded to 3 s, and
//! hears its result through its ordered channel, so a probe never delays a
//! turn's end. Hardening (the review's A8–A10):
//! - `git` is the absolute path `find_git` found on `PATH` when the host
//!   started, never a relative entry: a `git` the agent wrote into its cwd
//!   never runs;
//! - every `GIT_*` variable the host inherited is removed, and
//!   `GIT_OPTIONAL_LOCKS=0` keeps `git status` from taking `index.lock`
//!   under the agent's own git;
//! - discovery stops below `$HOME` (`GIT_CEILING_DIRECTORIES`), so a cwd in
//!   no repository never scans a home directory kept in git;
//! - fsmonitor and submodules are off, and git runs in a process group of
//!   its own, killed whole when a probe is given up.
//!
//! A repository can still make `git status` run a command it configures (a
//! clean filter, which `.git/info/attributes` can name). That is an
//! accepted residual risk (umbrella §8.4): the agent working there can run
//! anything already.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

/// The whole probe's bound (ACP core §7).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// What `rev-parse` may print before the probe stops reading it.
const REV_PARSE_MAX_BYTES: u64 = 64 * 1024;

/// What `git status` may print before the probe stops reading it: its
/// headers are short, but a repository can name an upstream of any length
/// (the second review's P3).
const STATUS_MAX_BYTES: u64 = 64 * 1024;

/// What a probe found: the fields of a `git_state`, but `base_commit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitState {
    /// The branch checked out; `None` when detached.
    pub branch: Option<String>,
    /// Any staged, unstaged or untracked change.
    pub dirty: bool,
    /// The cwd is in a linked work tree, not the repository's main one.
    pub worktree: bool,
    /// The commit checked out; `None` on a branch with no commit yet.
    pub head: Option<String>,
}

/// `git` on the host's `PATH`, as an absolute path: found once, when the
/// host starts. Relative entries are skipped, and so is a `git` older than
/// 2.31 (no `--path-format`) or one that does not answer `--version` (the
/// second review's P5: on macOS, `/usr/bin/git` without the developer tools
/// would otherwise offer to install them at every probe). It is asked once
/// here, and the host says so if it is not used.
pub fn find_git() -> Option<PathBuf> {
    let git = find_git_in(std::env::var_os("PATH"))?;
    let found = version_of(&git);
    if usable(found) {
        Some(git)
    } else {
        tracing::info!(git = %git.display(), ?found, "git is older than 2.31 or does not run: sessions report no git state");
        None
    }
}

/// What `git --version` says, within `PROBE_TIMEOUT`: a git that hangs
/// must not hold the host's start (the second review, after B1–B4).
fn version_of(git: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut child = std::process::Command::new(git)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut out = String::new();
    child.stdout.take()?.take(4096).read_to_string(&mut out).ok()?;
    if status.success() { parse_version(&out) } else { None }
}

/// `git --version`'s major and minor version.
fn parse_version(text: &str) -> Option<(u32, u32)> {
    let mut numbers = text
        .trim()
        .strip_prefix("git version ")?
        .split(|c: char| !c.is_ascii_digit());
    Some((numbers.next()?.parse().ok()?, numbers.next()?.parse().ok()?))
}

/// Whether the probe can use a `git` of this version.
fn usable(version: Option<(u32, u32)>) -> bool {
    version.is_some_and(|version| version >= (2, 31))
}

fn find_git_in(path: Option<OsString>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(&path?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("git"))
        .find(|git| std::fs::metadata(git).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

/// `git <args>` in `cwd`, kept from the host's own git environment (`vars`,
/// the host's variables): see the module's doc.
fn command(git: &Path, cwd: &Path, args: &[&str], vars: impl Iterator<Item = (OsString, OsString)>) -> Command {
    let mut cmd = Command::new(git);
    cmd.current_dir(cwd)
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true);
    // What an agent never inherits, git does not either: a filter it runs
    // is the repository's code (the second review's B2).
    for var in crate::adapter::NESTING_VARS
        .iter()
        .chain(crate::adapter::HOST_SECRET_VARS)
    {
        cmd.env_remove(var);
    }
    let mut home = None;
    for (name, value) in vars {
        if name.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(&name);
        } else if name == "HOME" {
            home = Some(value);
        }
    }
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    if let Some(home) = home.filter(|home| Path::new(home).is_absolute()) {
        cmd.env("GIT_CEILING_DIRECTORIES", home);
    }
    cmd
}

/// Kills a git's process group when dropped, unless it finished: an
/// abandoned probe takes git and whatever it started with it.
struct Group(Option<i32>);

impl Drop for Group {
    fn drop(&mut self) {
        if let Some(pgid) = self.0 {
            // SAFETY: killpg(2) on the group of a git this probe spawned
            // and has not reaped. ESRCH (already gone) is harmless.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
}

/// The git state of `cwd`, if it is in a work tree and `git` answers
/// within `PROBE_TIMEOUT`; `None` otherwise, which is not an error.
pub async fn probe(git: &Path, cwd: &Path) -> Option<GitState> {
    tokio::time::timeout(PROBE_TIMEOUT, probe_unbounded(git, cwd))
        .await
        .ok()
        .flatten()
}

fn spawn(git: &Path, cwd: &Path, args: &[&str]) -> Option<(tokio::process::Child, Group)> {
    let child = command(git, cwd, args, std::env::vars_os()).spawn().ok()?;
    let group = Group(child.id().and_then(|id| i32::try_from(id).ok()));
    Some((child, group))
}

async fn probe_unbounded(git: &Path, cwd: &Path) -> Option<GitState> {
    // Absolute, so the main checkout's subdirectories, where git prints a
    // relative common dir, are not taken for linked work trees (the
    // review's A8). Needs git 2.31; an older one gives no state.
    let (mut child, mut group) = spawn(
        git,
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-dir", "--git-common-dir"],
    )?;
    let mut dirs = String::new();
    child
        .stdout
        .take()?
        .take(REV_PARSE_MAX_BYTES)
        .read_to_string(&mut dirs)
        .await
        .ok()?;
    let finished = child.wait().await.ok()?;
    group.0 = None;
    if !finished.success() {
        return None;
    }
    let mut dirs = dirs.lines();
    let worktree = dirs.next()? != dirs.next()?;

    // The headers come first; the first other line is a change, and the
    // probe stops reading there rather than buffering every change.
    let (mut child, mut group) = spawn(
        git,
        cwd,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "--ignore-submodules=all",
            "-unormal",
        ],
    )?;
    let mut status = BufReader::new(child.stdout.take()?.take(STATUS_MAX_BYTES));
    let (mut branch, mut head, mut dirty) = (None, None, false);
    let mut line = Vec::new();
    loop {
        line.clear();
        if status.read_until(b'\n', &mut line).await.ok()? == 0 {
            break;
        }
        match line.strip_prefix(b"# ") {
            Some(text) => header(String::from_utf8_lossy(text).trim_end(), &mut branch, &mut head),
            None => {
                dirty = true;
                break;
            }
        }
    }
    if !dirty {
        let finished = child.wait().await.ok()?;
        group.0 = None;
        if !finished.success() {
            return None;
        }
    }
    Some(GitState {
        branch,
        dirty,
        worktree,
        head,
    })
}

/// One `git status --porcelain=v2 --branch` header, without its `# `.
fn header(text: &str, branch: &mut Option<String>, head: &mut Option<String>) {
    if let Some(oid) = text.strip_prefix("branch.oid ") {
        *head = (oid != "(initial)").then(|| oid.to_string());
    } else if let Some(name) = text.strip_prefix("branch.head ") {
        *branch = (name != "(detached)").then(|| name.to_string());
    }
}

#[cfg(test)]
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    pub config_timeout: Duration,
```

with:

```rust
    pub config_timeout: Duration,
    /// `git` for the git probe (ACP core §7), found once when the host
    /// starts (`git::find_git`); `None`: no probe, no `git_state`.
    pub git: Option<PathBuf>,
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            config_timeout: CONFIG_TIMEOUT,
```

with:

```rust
            config_timeout: CONFIG_TIMEOUT,
            git: None,
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        inbound: OnceLock::new(),
```

with:

```rust
        inbound: OnceLock::new(),
        probe: Mutex::new(None),
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        pending_id: String,
    },
```

with:

```rust
        pending_id: String,
    },
    /// What a git probe found (ACP core §7); `base`: the first after a new
    /// session's start, so `head` is the commit it started from.
    Git {
        state: crate::git::GitState,
        base: bool,
    },
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
struct Watcher(tokio::task::JoinHandle<()>);

impl Drop for Watcher {
    fn drop(&mut self) {
        self.0.abort();
```

with:

```rust
struct Watcher(Option<tokio::task::JoinHandle<()>>);

impl Drop for Watcher {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            // No switch is sent, and no question is open, before the
            // actor's main loop starts.
            Inbound::SwitchAnswer { .. } | Inbound::QuestionWithdrawn { .. } => {}
```

with:

```rust
            // No switch is sent, no question is open and no probe runs
            // before the actor's main loop starts.
            Inbound::SwitchAnswer { .. } | Inbound::QuestionWithdrawn { .. } | Inbound::Git { .. } => {}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    /// The inbound channel, for the questions' withdrawal watchers.
    inbound: OnceLock<mpsc::UnboundedSender<Inbound>>,
```

with:

```rust
    /// The inbound channel, for the questions' withdrawal watchers and the
    /// git probe.
    inbound: OnceLock<mpsc::UnboundedSender<Inbound>>,
    /// The git probe running, if one is, and whether it (or one it waits
    /// for) records the base commit: a newer probe replaces it, which aborts
    /// it unless it does, and the actor's end aborts it.
    probe: Mutex<Option<(Watcher, bool)>>,
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        } = launch;
        let (mut adapter, io) = match Adapter::spawn(&agent, &cwd) {
```

with:

```rust
        } = launch;
        // For the git probe: `cwd` goes to the adapter's start.
        let probe_cwd = cwd.clone();
        let (mut adapter, io) = match Adapter::spawn(&agent, &cwd) {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                Ok(Inbound::SwitchAnswer { .. } | Inbound::QuestionWithdrawn { .. }) => {}
```

with:

```rust
                Ok(Inbound::SwitchAnswer { .. } | Inbound::QuestionWithdrawn { .. } | Inbound::Git { .. }) => {}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            });
        }

        // Prompts are deduplicated by turn_id: a retried delivery after a
```

with:

```rust
            });
        }
        // The git state after the start; a new session's names the commit
        // it started from (the review's O3: a resume's would name a later
        // one).
        self.probe_git(&probe_cwd, matches!(attach, Attach::New));

        // Prompts are deduplicated by turn_id: a retried delivery after a
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                            self.end_turn(ended.id, outcome, stop_reason(&response), None);
```

with:

```rust
                            self.end_turn(ended.id, outcome, stop_reason(&response), None);
                            self.probe_git(&probe_cwd, false);
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                            self.end_turn(ended.id, outcome, None, Some(err.to_string()));
```

with:

```rust
                            self.end_turn(ended.id, outcome, None, Some(err.to_string()));
                            self.probe_git(&probe_cwd, false);
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                self.withdraw_question(pending_id);
                true
            }
        }
    }

```

with:

```rust
                self.withdraw_question(pending_id);
                true
            }
            Inbound::Git { state, base } => {
                self.emit(SessionBody::GitState {
                    base_commit: state.head.clone().filter(|_| base),
                    branch: state.branch,
                    dirty: state.dirty,
                    worktree: state.worktree,
                    head: state.head,
                });
                false
            }
        }
    }

    /// Probe `cwd`'s git state on a task of its own (ACP core §7; plan
    /// 6b-ii decision 11): it never holds the actor, so it never delays or
    /// reorders a turn's end. Its result, if any, comes back on the ordered
    /// inbound channel as `Inbound::Git`. A newer probe aborts this one, and
    /// so does the actor's end; either kills git's process group. Only the
    /// probe that records the base commit is not aborted by a newer one (the
    /// second review's B1): the newer waits for it (it is bounded too), so a
    /// quick first turn cannot cost the base, and the states stay in order.
    fn probe_git(&self, cwd: &std::path::Path, base: bool) {
        let (Some(git), Some(inbound)) = (self.options.git.clone(), self.inbound.get().cloned()) else {
            return;
        };
        let mut slot = self.probe.lock().expect("probe lock");
        let earlier = match slot.as_mut() {
            Some((watcher, true)) if watcher.0.as_ref().is_some_and(|task| !task.is_finished()) => watcher.0.take(),
            _ => None,
        };
        let carries_base = base || earlier.is_some();
        let cwd = cwd.to_path_buf();
        let task = tokio::spawn(async move {
            if let Some(earlier) = earlier {
                // Aborted with this task, if it is.
                let mut earlier = Watcher(Some(earlier));
                if let Some(task) = earlier.0.as_mut() {
                    let _ = task.await;
                }
            }
            if let Some(state) = crate::git::probe(&git, &cwd).await {
                let _ = inbound.send(Inbound::Git { state, base });
            }
        });
        *slot = Some((Watcher(Some(task)), carries_base));
    }

```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            Watcher(tokio::spawn(async move {
                cancellation.cancelled().await;
                let _ = inbound.send(Inbound::QuestionWithdrawn { pending_id });
            }))
```

with:

```rust
            Watcher(Some(tokio::spawn(async move {
                cancellation.cancelled().await;
                let _ = inbound.send(Inbound::QuestionWithdrawn { pending_id });
            })))
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                    Ok(Inbound::SwitchAnswer { .. } | Inbound::QuestionWithdrawn { .. }) => {}
```

with:

```rust
                    Ok(Inbound::SwitchAnswer { .. } | Inbound::QuestionWithdrawn { .. } | Inbound::Git { .. }) => {}
```

- [ ] **Step 4: Run the tests to see them pass, then the checks**

Run: `nix develop -c cargo test -p hennery-host --locked --lib git` and `nix develop -c cargo test -p hennery-testkit --locked --test host_session -- git`
Expected: PASS (5 passed; 3 passed). Then the five checks: 571 tests.

- [ ] **Step 5: Revert-probes**

Each probe below was applied to the task's commit, its test run, and the change undone. Every one was caught (10 of 10):
- `GIT_OPTIONAL_LOCKS` not set (`git.rs`): `a_command_is_isolated_from_the_hosts_git_environment` fails.
- the host's `GIT_*` passed on (`git.rs`): `a_command_is_isolated_from_the_hosts_git_environment` fails.
- a relative `PATH` entry searched (`git.rs`): `git_is_found_on_absolute_path_entries_only` fails.
- relative git dirs compared (`git.rs`): `a_probe_reports_the_branch_changes_and_linked_work_trees` fails.
- only git killed, not its group (`git.rs`): `a_hung_git_never_delays_a_turn_end_and_is_killed_with_its_group` fails.
- the probe awaited on the actor (`session.rs`): `a_hung_git_never_delays_a_turn_end_and_is_killed_with_its_group` fails.
- `base_commit` on every state (`session.rs`): `the_git_state_follows_the_start_and_every_turn_end` fails.
- the base probe aborted by a quick first turn (`session.rs`): `a_quick_first_turn_still_gets_the_base_commit` fails.
- git inheriting what an agent may not (`git.rs`): `a_command_is_isolated_from_the_hosts_git_environment` fails.
- a git older than 2.31 used (`git.rs`): `only_git_2_31_or_newer_is_used` fails.

- [ ] **Step 6: Commit, push, and open PR 2**

`git -c commit.gpgsign=false commit -m "feat(host): report the git state after the start and each turn"`

---

## After this plan

**Hats 5c must slot in** (whichever of 6b and 5c lands second does it):
- `sessions.hat_id` in `SESSION_ITEM_COLUMNS`, `read_item` and `SessionItem` (`hat_id?`, an id within the 64 bytes decision 10 keeps);
- the `hat` filter: replace the 400 at the top of `list_sessions` with `ListQuery.hat`, and `AND (?11 IS NULL OR hat_id = ?11)` in `list_statement`, applied with or without `q` (frontend §5); keep the query-plan test (an index on `(owner_id, hat_id, last_event_at DESC, id DESC)` may be needed);
- this plan found no `sessions.hat_id` on `main` at `7f779e4` (checked again at the rebase before each merge).

**Hats 5c's `PATCH /api/sessions/{id}`** (rename): an operator's rename must win over later agent titles (a column of its own, or a flag that `store_state` honours). An early title already never overwrites a stored one (decision 2).

**The list stream** `GET /api/stream/sessions` (`session_upsert`):
- `last_event_id` is the `id:` of a session's last listed event, kept for it (decision 6);
- `mark_failed` and `mark_failed_if_starting` change list-visible state without an event (decision 6): the stream needs one, or an upsert of its own;
- keyset pages can miss a session that moves above the cursor between fetches (decision 8); the keyed store fixes it.

**The frontend (plan 4):**
- show each agent-controlled field (title, branch, cwd, model, mode, failure reason) in `<bdi>` or with `unicode-bidi: isolate` (the first review's A2, the second's B4);
- the host's name comes from `GET /api/hosts` by `host_id`;
- the "needs you" count: a keyset page cannot give the number of `blocked` sessions, and a session blocked for days sinks below the first page; add `blocked_count` to `SessionPage`, or count from the list stream;
- "Hide closed" is `lifecycle=starting,active,parked,failed`; a search ignores it;
- a title from before this plan, or of an agent that never reports one, is absent: fall back to the cwd's last component (frontend §5).

**Recorded, not done:**
- the `text_projection`, `usage` and `plan` extracts; `GET …/catalog` gains plan and usage with them;
- transcript full-text search (out of v1);
- `LIKE` ignores case for ASCII letters only (decision 8);
- O2: `last_event_at` is not clamped against a clock that steps back;
- O5: titles and branches are cut at character, not grapheme, boundaries;
- a session from before 6b-ii never gets a `base_commit` (O3: only a new session's first probe records one), nor does one whose first probe failed (a first turn no longer aborts it: B1);
- a title the agent cleared live comes back after a resume that replays it: the replayed "set" fills the empty column, and an early clear clears nothing (decision 2);
- P6: the detail's two reads (`session`, `session_item`) are not one transaction;
- P7: `PROTOCOL_VERSION` stays 1.0; an older collector logs a `git_state` it cannot read and drops it, and ignores the new `Indexed` fields;
- if git exits successfully, whatever it left running in the background is not killed;
- `find_git`'s `git --version` is bounded to 3 s, but its output is read after git exits: a wrapper that leaves a child holding standard output open would still hold the host's start (the second review's last note);
- `git` older than 2.31 gives no state (`--path-format`);
- the clean-filter residual risk (decision 11, A10);
- `SessionDetail` now carries `created_at` and `last_event_at` too (decision 9).

**Not tested here:**
- **Linux:** only macOS compiled and ran here; CI's ubuntu job is the first Linux run of `git.rs`'s process group and of the git tests.
- **A real adapter's `session_info_update`:** the fake sends it raw; the shapes follow agent-client-protocol-schema 1.9.1. Claude's and Codex's adapters were not run.
- **A clean filter during the probe** (A10): accepted, not exercised.
- **P3's 64 KiB cap on `git status`:** in the code, not pinned by a test.
- **macOS without the developer tools:** `find_git`'s `--version` check (P5) is unit-tested on its parsing only.
- **`GIT_CEILING_DIRECTORIES` with a home kept in git:** set and pinned by the unit test, not run against such a home.
- **A pre-existing timing test under load:** `replayed_history_never_leaks_under_a_multi_thread_runtime` failed once in 12 copies of `host_session` (four at once, three rounds): its 50 ms sleep after `session_started` was not enough for the replayed state update to be emitted. It passed 15 runs of 15 alone, and touches nothing this plan changed beyond the `early` flag on that update. Hold it with polling when it next needs touching.

**Carried, unchanged:** the "After this plan" of 3c, 3b-iii, 3b-ii, 3b-i, (2) and B2b, except what this plan did: "the rest of the catalogue" (commands; plan and usage remain), "`model` / `mode` in the list and detail items", and 3b-iii's "`start_session` stores whatever `host_id` it is given" (decision 9).

Then, in order (unchanged): **(4) Frontend shell**, **(5) Hats**, **(6) Gateway**, **(7) Distribution**.

---

_Generated with Claude AI — please review before distribution._
