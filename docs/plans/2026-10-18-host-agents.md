# A host's agents (plan 4d-B1-i) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** the Hosts page and the New-session form can say which agents a host has, whether each can start and is logged in, and where they come from (ACP core §6, frontend §7 and §8):
- a host reports its agents in `hello` (the static view: as configured) and answers `probe_agents` (the live view: each adapter started and asked `initialize`, each CLI asked whether it is logged in);
- the collector stores the latest report per host and serves it at `GET /api/hosts/{id}/agents[?refresh=1]`, a refresh running one probe that concurrent refreshes share;
- the probe reuses doctor's knowledge of the CLIs (checks 3 and 4) but none of doctor's process handling: every program runs in the host's own guarded groups and environment, only exit codes are kept, one probe runs at a time, within 15 s on the host and 20 s on the collector.

**Architecture:**
- **Wire** (`hennery-proto`): a module `agents` with `AgentInfo {agent, available, auth: ok | missing | unknown, cli: bundled | override | given, adapter_version?, images?, note?}`, `RuntimeInfo {source: managed | given, set_id?, pinned?, held?}`, the bounds both ends apply (`bound_agents`, `RuntimeInfo::bounded`) and lenient readers (`AgentList`, `MaybeRuntime`). `rest::HostAgents {host_id, agents, runtime?, reported_at?, source: none | hello | probe, live}`. `hello` gains `agents` and `runtime`; `CollectorFrame::ProbeAgents {request_id}` and `HostFrame::Agents {request_id, agents, runtime?}`, behind the new capability `probe_agents`. The schema and TypeScript files are regenerated.
- **Kernel** (`hennery-kernel`): kernel migration 12 adds `hosts.agents`, `agents_reported_at` and `agents_source`; `Hosts::record_agents` (bounded again, never for a revoked host) and `Hosts::agents`.
- **Host** (`hennery-host`): `availability`: the static view, the probe (`initialize` through `Adapter::spawn`; the CLI's exit status through the new `adapter::exit_status`, a guarded group with its standard streams on `/dev/null`), `OneProbe`, and the trait `AgentChecks` through which the binary injects doctor's knowledge. `runtime::agents` records each agent's CLI, version and note (`Agents.infos`) and the runtime (`Prepared.runtime`). `connection` reports them in `hello` and answers `probe_agents` in a task of its own.
- **Sessions** (`hennery-sessions`): `agents`: the route and `Refreshes` (one probe per host, coalesced); `ws` stores a reconciled `hello`'s report.
- **Binary** (`hennery`): `host_agents`: `AgentSetup` (managed set or `--agent`) and `DoctorChecks`, which asks doctor which CLI and which question (`bundled_cli`, `status_of`, `writable_by_others`, re-exported `pub(crate)`).
- **Specs:** ACP core §3.3 and §6, kernel §1.1, §4.3 and §8, distribution §7 (Task 5).
- **Tests:** proto `tests/agents.rs` (new); kernel `tests/hosts.rs`, `tests/owner.rs`, the migration's unit test; host `tests/availability.rs` (new), `tests/adapter.rs`, `tests/runtime.rs`; testkit `tests/host_agents.rs` (new, a scripted host over a real WebSocket), `tests/host_connection.rs` (the real host), `auth.rs` (the route table), `owner_filter.rs` (`hosts.rs`: 18 statements); the binary's `tests/cli.rs` and `host_agents.rs`'s unit tests.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), rusqlite 0.40, tokio, axum 0.8, serde, schemars, ts-rs. No new crate; `Cargo.lock` does not change.

**Spec:**
- [ACP core](../specs/2026-09-26-acp-core-design.md) §3.3: "`probe_agents` | — | `agents{…}` (same shape as in `hello`)"; `hello.agents[]`, "per agent `{id, version, available, auth, catalog}`".
- ACP core §6: "`available` = the adapter can be launched; `auth` = `ok | missing | unknown`. Auth is taken, in order, from the adapter's `_auth/status_update` notification …, then from the bundled CLI (`claude auth status`, `codex login status`; exit 0 = logged in), then `unknown`. Account details … are never forwarded."
- [Kernel](../specs/2026-09-26-kernel-design.md) §1.1: `hosts(… agents JSON …)`; §4.3: "agent availability … updated from `hello` and `probe_agents`"; §8: the hosts routes; §11: owner filter.
- [Distribution](../specs/2026-09-26-distribution-design.md) §7: checks 3 and 4; "The host runs checks 3–4, 9, 12 and 13 on demand (`probe_agents`)".
- [Frontend](../specs/2026-09-26-frontend-design.md) §7: "Agent availability and auth state from the host (`auth: missing` disables the agent with instructions; `unknown` allows it)"; §8: the Hosts list shows "versions (host, adapters), agent availability".

It builds on plan 4 (frontend)'s split: 4c (the New-session form) and 4d-iii (the Hosts page) consume this route, and the shape was agreed with 4c before this plan ("B1-i shape v2"). It reuses doctor (plans 7c, 7d) under the conditions the distribution lane set for it (decision 4).

**Status:** not executed; amended after the security review. The security review of 2026-10-02 (an opus subagent, binding on the maintainer's behalf) approved after amendments: A1–A8 required and done, O1–O4 taken, O5 declined and recorded; its one product question was answered from the specs by the gateway lane (decision 9). Its scoped re-confirmation of the same day (a fresh opus subagent) confirmed A1, A3–A8, O1–O4 and decision 9, and asked one fix, R1 (A2's list of forbidden calls widened to `run(`, `run_bounded` and `spawn(`), applied exactly as it specified; the set is confirmed with it. The conditions on doctor's files were set and confirmed by the distribution lane (decision 4). A second scoped re-confirmation (2026-10-03, a fresh opus subagent) re-confirmed R1 as applied and the merges two rebases made with main (plans 8c, 9d-ii and #102); see After this plan.

Every code block below was built and tested in a scratch copy of `ecc50cd`, then rebased onto `c987ce7`, `b8cf8b3` and `d14f556` (4d-B2, whose kernel migration is 11, so this plan's is 12). The rebases merged this plan's `hello` fields with plan 8c's `Announce`, and its agent infos with 9d-ii's profiles and Codex app-server; `probe_agents` joined 8c's exhaustive `mcp_delivery`. The scratch was split again on `d14f556`, two commits per task (the tests alone, then the task), and the blocks were generated from those commits. The plan was then replayed from its own text, task by task, onto a fresh copy of `d14f556`: after each Step 1 the tree matched the tests-only commit, and after each task the task commit, byte for byte, the generated files aside (each task's Step regenerates them, and `gen -- --check` passed on every task commit). Every revert-probe below was run on the whole plan's tree: all of them on `ecc50cd`, and again on `c987ce7` the 25 in files the rebase merged (`connection.rs`, `runtime/agents.rs`, `host_agents.rs`, and those `cli.rs` checks), all caught; C1's anchor moved, as plan 8c's `McpServers` now precedes `ProbeAgents`.

One timing was measured before execution and is environmental. At Task 3, main's `a_host_without_agent_flags_runs_the_installed_set` (`cli.rs`, two waits of 20 s each) took 19–22 s alone, against main's 4–8 s, while the network was down; one interleaved run failed at its limit. It was re-measured on 2026-10-03 with both test binaries prebuilt (`d14f5563` and Task 3's tree, separate targets), run alternately with the network up, and timed phase by phase from the tracing timestamps of `up`, the host and the collector. After one warm-up run each (10.7 s and 7.5 s, ~2–4 s of it first execution, alike for both), the three alternating runs took 4.0, 1.1 and 2.5 s on main and 2.5, 1.3 and 1.3 s on Task 3; four copies in parallel took 2.9–4.5 s on main and 1.3–4.7 s on Task 3. Each phase matches main within noise: the collector listens 0.06–0.10 s after start and the host connects within 0.2 s. From `host connected` to `host reconciled`, the span that holds the new `UPDATE` of the host's agents, took 0.3–1.0 ms on Task 3 and 0.3–0.9 ms on main. The varying 0.7–3.7 s is the session start, in both. Task 3 adds no I/O, exec or probe on connect: `hello` clones and maps configured fields, and `probe_agents` runs only when a refresh asks for it. Task 3's full checks (1589 tests) and Tasks 4 and 5 passed this test. If it recurs during execution, report it with timestamps and do not rerun it.

## Execution status

Not executed yet.

## Scope

The hosts' half of ACP core §6's agent availability, and the route the frontend needs. That is **5 tasks**:
1. the wire types and their bounds, and the kernel's store;
2. the host's checks: the static view, the probe, one at a time;
3. `hello`, `probe_agents` and the route, both ends;
4. the binary: the managed set or `--agent`, and doctor's knowledge of the CLIs;
5. the spec write-back.

**In:**
- `hello.agents`, `hello.runtime`, `probe_agents` / `agents`, the `probe_agents` capability;
- `GET /api/hosts/{id}/agents[?refresh=1]`;
- checks 3 and 4 of doctor, run by the host on demand; check 12 as `runtime.pinned`.

**Out** (see "After this plan"):
- checks 9 and 13 and the stored doctor report (`last_doctor`), the Hosts page notices (plan 4d-B1-ii);
- the static catalogue in `hello` and `host_agent_catalog` (ACP core §8);
- reading `_auth/status_update` (decision 3);
- the UI (4c, 4d-iii).

**Where the hand-offs land:**

| Hand-off | Here |
|---|---|
| 4c: `GET /api/hosts/{id}/agents` for the New-session form | Decisions 1, 8; Task 3 |
| 4d: the Hosts page's agent availability and adapter versions | Decisions 1, 2; Tasks 2, 3 |
| the distribution lane's conditions on reusing doctor | Decision 4; Tasks 2, 4 |
| The images ruling (hide only on `images === false`) | Decision 1; Task 1 |

## Decisions this plan makes where the spec is silent

These were confirmed by a stronger-model security review on the maintainer's behalf (2026-10-02, "approve after amendments"); its scoped re-confirmation is recorded in "After this plan". Each gives the choice, the alternatives, and the cost if it is wrong. Decisions the review changed are marked "(amended after the security review of 2026-10-02)". Items marked **(amendment)** depart from explicit spec text and are written back into it in Task 5.

**What the review changed:**
- A1: `exit_status` kills its group on every path, a dropped future included (`GroupKill`; decision 4 (b); Task 2).
- A2: hazard (a) rests on a check, not on wording: a test forbids any process API in `host_agents.rs` (decision 4 (a); Task 4).
- A3–A6: tests that could not fail now can: another owner's host, connected, is never probed (Task 3); a CLI's directory and stripped environment (Task 2); the probe's directory (Task 3); the output caps (Task 2).
- A7: the client contract that stands in for a cooldown (decision 6; After this plan).
- A8: Review Focus and "Not tested here" name what no test proves.
- Optional, taken: O1 (a CLI ended by a signal is `unknown`, decision 3), O2 (`images` absent means no probe answered), O3 (the output caps have their own note), O4 (one list of format characters, the kernel's host names and the reports). Not taken: O5, a smaller limit for the first frame before `hello` is verified: it bounds every `hello`, not this plan's fields; recorded in "After this plan".
- **Who confirmed what:** the decisions and amendments, the security review (an opus subagent, 2026-10-02); the conditions on doctor's files (decision 4), the distribution lane (2026-10-02); the wire shape (decision 1), agreed with 4c through the frontend lane before this plan; the scoped re-confirmation, a fresh opus subagent (After this plan); a second scoped re-confirmation (a fresh opus subagent, 2026-10-03) of R1 as applied and of the two rebases' merges with plans 8c, 9d-ii and #102 (After this plan).

1. **The wire shape is the one agreed with 4c ("B1-i shape v2"), not ACP core §3.3's.**
   - **Choice:** `AgentInfo {agent, available, auth, cli, adapter_version?, images?, note?}`, `RuntimeInfo {source, set_id?, pinned?, held?}`, `HostAgents {host_id, agents, runtime?, reported_at?, source, live}`, spelled exactly as sent to 4c. `agent` is the profile name `StartSessionRequest.agent` takes (the spec's `id`); `adapter_version` is the adapter's, not the CLI's (the spec's `version`). `catalog` is not here (After this plan).
   - **The images ruling:** `AgentInfo.images`'s doc comment says it: a client hides images only when `images` is `false`; absent (no probe answered: none ran, or the adapter did not answer; the review's O2) it allows them, and the host's `error{images_unsupported}` (409 over the API) stays the guard.
   - **Alternatives:** the spec's names: 4c already builds against these, and `version` alone would not say whose.
   - **Cost if wrong:** a rename across the wire, the store's JSON and the generated files.
2. **Two views, one shape.**
   - **Choice:** `hello` carries the static view: every agent as configured, `available` meaning launchable as configured, `auth: unknown`, `images` absent, the version the set records. `probe_agents` answers the live view: `available` meaning the adapter started and answered `initialize`. `HostAgents.source` says which is stored (`none | hello | probe`) and `reported_at` when the collector received it; `live` is the hub's `is_ready` now.
   - **Stored only from a reconciled connection** (as the roots are, plan 6c), and only from a host with the `probe_agents` capability: an older host sends no list, which would otherwise erase a report.
   - **Alternatives:** a probe at every `hello`: every reconnect would start node and the CLIs, and on macOS each can raise a keychain prompt.
   - **Cost if wrong:** one condition in `ws.rs`.
3. **`auth` comes from the CLI alone, as doctor's check 4 asks it.** (amendment)
   - **Choice:** exit 0 is `ok`, any other exit code `missing`; ended by a signal, no answer in time, no program or no known CLI is `unknown` with a note (a signal: the review's O1, since Gatekeeper or a keychain prompt can end a logged-in CLI on macOS). Only the exit status is kept: the CLI's standard streams are `/dev/null`. ACP core §6 puts the adapter's `_auth/status_update` first; it is not read here, as doctor does not read it either, and is written back as "not read yet" (Task 5).
   - **Alternatives:** reading `_auth/status_update` after `initialize`: a wait for a notification that may never come, and a payload that names the account, which then must be parsed and dropped.
   - **Cost if wrong:** a later probe reads the notification first; the wire does not change.
4. **Doctor's knowledge, the host's process handling** (the distribution lane's conditions).
   - **Choice:** the wire types are in `hennery-proto`, mapped from what doctor knows; the trait (`AgentChecks`) is in `hennery-host`; the binary implements it (`DoctorChecks`) with doctor's `bundled_cli`, `status_of` and `writable_by_others`. The host never calls doctor to run anything. Hazards:
     - (a) **doctor's groups and signal handler:** never involved. (amended after the security review of 2026-10-02: A2) Nothing in `host_agents.rs` spawns; `Cli` is not re-exported, and `host_agents` holds the `Cli` that `bundled_cli` returns only to move its `program` and `args` out. Its `run` (doctor's static `GROUPS`) stays reachable on that value in principle, so a test, `nothing_here_reaches_a_process_api`, forbids `run(`, `run_bounded`, `spawn(`, `kill_all`, `spawn::`, `Command::new`, `std::process` and `tokio::process` in the module's code (the list widened by the re-confirmation's R1).
     - (b) **the host's guard:** each adapter starts through `Adapter::spawn`, each CLI through `adapter::exit_status`, both built by `guarded_command`: a group of its own led by `spawn_guard`'s guard (the death pipe), SIGKILLed with its group when the check ends. (amended after the security review of 2026-10-02: A1) `exit_status` kills the group through a one-shot `GroupKill`, made only once the CLI has spawned (on a failed spawn the guard is killed and may be reaped, and its id reused), which fires when the check ends and also when its future is dropped first.
     - (c) **exit codes only, never CLI output, with a size cap:** a CLI's streams are `/dev/null`; an adapter's output is read only for its `initialize` answer (at most 1 MiB and 1000 lines, past which it "wrote too much", the review's O3), and only `agentInfo.version` (if readable) and `promptCapabilities.image` are kept; its stderr drains into the adapter's existing bounded ring and is never read or sent. Notes are the host's own words: fixed sentences, `io::ErrorKind`'s fixed text, and the host's own configuration paths. Both ends bound a report (`bound_agents`, `RuntimeInfo::bounded`): at most 16 agents, names of 64 bytes, versions of 64, notes of 512, a set id of 128; the stored JSON stays under 32 KiB (`a_stored_report_is_bounded_in_bytes`).
     - (d) **the host's own agent env:** the probe's programs get what a session's adapter gets (`guarded_command`: the agent's variables, the host's secrets, log choice and nesting variables stripped), not doctor's built `agent_env`; they run in the host's home directory, never its working directory.
     - (e) **one probe in flight per host, refreshes coalesced, 20 s:** the host runs one probe at a time across its connections (`OneProbe`; another is answered `error{busy}`), within 15 s for every check at once; the collector shares one probe among concurrent refreshes of a host (`Refreshes`) and waits 20 s at most.
     - (f) **no check 5:** only checks 3 and 4.
   - **The conditions on doctor's files** (confirmed by the distribution lane, 2026-10-02): the re-exports are `pub(crate)` only; each re-exported item's doc comment names its second caller; the PR makes no logic or signature change in `doctor/agents.rs`, `runtime.rs` or `spawn.rs` (here: `status_of` goes from private to `pub(crate)`, and doc comments).
   - **Plan 7e-ii-a** (the distribution lane, 2026-10-03): it will make doctor's `given_agents` also return the agents of a NixOS system unit. This plan reports `cli: given` from the host's own parsed `--agent` list, never from unit files, so that changes nothing here. 7e-ii-a touches `doctor/mod.rs` (beside this plan's re-exports), `doctor/runtime.rs`, `doctor/tests.rs` and `service/mod.rs`, but not `agents.rs`, `spawn.rs`, `status_of`, `bundled_cli` or `Cli`. Whichever of the two merges second rebases.
   - **Cost if wrong:** the host's calls move into doctor; the wire does not change.
5. **The probe runs what the host is configured with, and nothing the collector names.**
   - **Choice:** `probe_agents` carries only a request id. The host checks the agents of its own `hello`; one not launchable as configured is reported as it is, with nothing started. An `--agent` agent is `cli: given`, with `runtime.source: given` and no managed field: hennery manages nothing of it and asks it no login.
   - **No step-up:** the probe is a fixed set of read-only checks (kernel §3.4 lists actions; reading is none).
6. **The route answers from the store, always 200 for the owner's host.**
   - **Choice:** `GET /api/hosts/{id}/agents` answers the stored report whatever its age and whether the host is connected (`live` says it). `refresh=1` first runs one probe, if the host is connected and has the capability, and waits for it; a probe that fails (offline, unsupported, busy, refused, timed out) leaves the last report and is logged, never an error answer. `refresh` other than `0`/`1` is 400 `invalid`. 404 `not_found` for a host that is not the owner's, decided from the registry (`Hosts::host`) before any report is read or any probe sent. The answer is `no-store`: a note can name a path on the host.
   - **An answer that arrives after its caller left is stored:** the probe runs in a task of its own, and its entry is removed before its waiters wake, so a refresh after it probes again.
   - **No cooldown between refreshes** (amended after the security review of 2026-10-02: A7): a refresh is the owner's own click; concurrent ones share a probe, and the host's own one-at-a-time bound holds across connections. The client contract stands in for a cooldown: `?refresh=1` only on an explicit action, never on mount, focus, a poll or a retry, and the control is disabled while one is in flight (After this plan).
   - **Alternatives:** 409 `host_offline` on a refresh of an offline host, as the projects routes answer: the client would then have to fetch again for the stored report it needs anyway.
7. **Storage: three columns on `hosts`.**
   - **Choice:** kernel migration 12 adds `agents` (JSON `{agents, runtime?}`), `agents_reported_at` and `agents_source` (`none` by default): columns added, never a rebuild. A report that cannot be read is `source: none`. A revoked host's report is not stored. The audit's floor for `hosts.rs` goes from 16 to 18.
8. **Leniency.** `hello.agents` and `agents.agents` are read entry by entry: one this build cannot read is skipped; a runtime it cannot read is absent; a list that is not a list is a malformed frame. As `capabilities` are (ACP core §3.3).
9. **`auth` is a host fact.**
   - **Choice:** agent logins are per OS user in v1 (ACP core §6, "Composed CODEX_HOME": a hat's composed `CODEX_HOME` links the user's `auth.json`; Claude's per-hat isolation is MCP servers only; maintainer decision 2, umbrella §8.4, ACP core §15), so one `auth` per agent per host is right for every hat. Confirmed by the gateway lane (2026-10-02), after the review raised it as a product question.
   - `AgentInfo.auth`'s doc comment says so: if plan 8h ever makes `auth.json` private per hat, `auth` must become per (host, hat).

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`.
- After every task these pass:
  - `nix develop -c cargo fmt --all --check` (`max_width = 120`);
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy -p hennery --locked -- -D warnings` (test hooks off);
  - `cargo test --workspace --locked`;
  - `cargo run -p hennery-proto --bin gen -- --check`.
- **Wire types change in Tasks 1 and 3.** The generated files (`schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`) are regenerated there with `cargo run -p hennery-proto --bin gen`; new root types go in both lists of `codegen.rs` (Task 1). The web checks then run: `pnpm --dir web install --frozen-lockfile && pnpm --dir web typecheck && pnpm --dir web test`.
- **Migrations:** this plan adds kernel migration 12 (index 11; 4d-B2 took 11). Its test finds it by its text, never by its index, so a renumbering on rebase changes no test.
- Storage (kernel §1): every statement names the owner. The audit (`owner_filter.rs`) reads every statement of `hosts.rs`; its floor is 18 after Task 1.
- Operator routes (kernel §3.3): the new route is behind `operator_only`; `auth.rs`'s route table lists it.
- Processes: every program the host starts for a probe runs in a group of its own led by the host's guard (`spawn_guard`), with the host's own agent environment, and is SIGKILLed with its group when the check ends; no test spawns the `hennery` binary except through the testkit's helper that scratches `HOME` and `XDG_*`.
- No Linux-only code. The tests that read another process's state poll for a positive signal; ubuntu ran them on the scratch CI PR (#101).
- Commits follow Conventional Commits and use the repository's own identity (gmail). Push the feature branch after every completed task; never push `main`.

## Review Focus

1. **The owner opens the Hosts page, then refreshes a host's agents.**
   - Expected: the stored report at once (`source: hello` after the host connected); with `refresh=1`, one `probe_agents`, its answer stored (`source: probe`) and served, `no-store`.
   - Tests: Task 3 `a_reconciled_hello_stores_the_agents_of_a_host_that_reports_them`, `concurrent_refreshes_share_one_probe`; `a_host_probes_its_agents_one_probe_at_a_time` (the real host).
2. **Every verdict.**
   - `auth` `ok`, `missing`, `unknown` (no answer, no program, not asked, ended by a signal): Task 2 `a_status_exit_of_zero_is_logged_in_and_any_other_exit_code_is_not`, `a_cli_ended_by_a_signal_is_unknown`, `a_silent_cli_is_unknown_at_the_deadline_and_leaves_nothing_running`, `a_cli_that_cannot_be_started_is_unknown`, `a_cli_not_asked_is_unknown_with_its_note`.
   - `cli` `bundled`, `override`, `given`: Task 2 `a_start_reports_each_agent_with_its_cli_and_version_and_the_pinned_set`, `an_override_beside_a_bundled_cli_is_reported_with_its_note`, `the_static_view_adds_every_given_agent_and_keeps_the_runtimes`; Task 4 `a_hosts_agents_are_reported_and_checked_through_the_binary`.
   - `source` `none`, `hello`, `probe`; `live` true and false: Task 3 `an_older_hosts_hello_stores_nothing`, `a_reconciled_hello_stores_the_agents_of_a_host_that_reports_them`, `concurrent_refreshes_share_one_probe`, `a_refresh_of_a_host_that_cannot_be_probed_answers_the_last_report`.
   - `RuntimeInfo.source` `managed`, `given`: Task 2 `a_start_reports_each_agent_with_its_cli_and_version_and_the_pinned_set`, `with_no_set_the_runtime_is_managed_and_empty`; Task 4 `given_agents_are_configured_as_given`.
3. **A host that lies, or an agent that misbehaves.**
   - Expected: a hostile answer bounded before it is stored; an adapter that never answers, or a CLI that hangs or floods its output, ends at the deadline with nothing of its group left; nothing an agent or a CLI prints reaches the report.
   - Tests: Task 3 `a_hostile_answer_is_bounded_before_it_is_stored`; Task 1 `a_stored_report_is_bounded_in_bytes`; Task 2 `a_silent_adapter_fails_at_the_deadline_and_leaves_nothing_running`, `a_cli_writes_to_nowhere_and_is_never_blocked_by_its_output`, `an_error_answer_fails_without_quoting_it`, `a_probe_keeps_to_one_budget_however_many_checks_hang`, `an_answer_after_too_many_bytes_is_never_read`, `an_answer_after_too_many_lines_is_never_read` (A6), `a_cli_ended_by_a_signal_is_unknown` (O1).
4. **The host dies uncleanly during a probe.**
   - Expected: no adapter or CLI of the probe survives it.
   - Tests: Task 2 `host_sigkill_kills_a_cli_s_whole_group`, `a_cli_runs_in_a_guarded_group_of_its_own`, `a_dropped_status_check_kills_its_group` (A1) (and the adapters' existing guard tests, since the probe starts them through `Adapter::spawn`).
   - And in the host's environment and directory (hazard (d)): Task 2 `an_adapter_is_started_in_its_directory_with_the_hosts_stripped_environment`, `a_cli_runs_in_its_directory_with_the_hosts_stripped_environment` (A4); Task 3 `a_host_probes_its_agents_one_probe_at_a_time` (the home directory, A5).
5. **Another owner, an unknown host, a bad query.**
   - Expected: 404 for both hosts, refreshed or not, nothing of theirs shown, and another owner's host never probed even if the hub held it (A3); 400 for a `refresh` other than `0` or `1`.
   - Tests: Task 3 `another_owners_host_is_not_found`, `an_unknown_host_is_not_found_and_a_bad_refresh_is_invalid`; Task 1 the kernel's `another_owners_hosts_and_codes_are_invisible_to_the_registry`.
6. **Doctor's code is reused, not its process handling** (decision 4).
   - Expected: the CLI asked is the override, else the bundled one, with doctor's question and the auto-updater off; a CLI others can write to is not run; nothing in `host_agents.rs` spawns.
   - Tests: Task 4 `a_bundled_cli_is_asked_its_status_with_the_auto_updater_off`, `an_overridden_cli_is_the_one_asked`, `with_no_known_cli_nothing_is_asked`, `a_cli_others_can_write_to_is_not_run`, `nothing_here_reaches_a_process_api` (A2).

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-proto/src/agents.rs` (new), `lib.rs`, `rest.rs`, `codegen.rs`, the generated files | `AgentInfo`, `RuntimeInfo`, the bounds, the lenient readers; `HostAgents`, `AgentsSource` | 1 |
| `crates/hennery-kernel/src/schema.rs`, `hosts.rs` | Kernel migration 12; `record_agents`, `agents` | 1 |
| `crates/hennery-proto/tests/agents.rs` (new), `crates/hennery-kernel/tests/hosts.rs`, `owner.rs`, `crates/hennery-testkit/tests/owner_filter.rs` | The bounds, the store, the owner | 1 |
| `crates/hennery-host/src/availability.rs` (new), `adapter.rs`, `lib.rs`, `runtime/agents.rs` | The static view, the probe, `exit_status`, `OneProbe`, `AgentChecks`; what a managed set reports | 2 |
| `crates/hennery-host/tests/availability.rs` (new), `adapter.rs`, `runtime.rs` | The checks, their groups and deadlines; the managed set's report | 2 |
| `crates/hennery-proto/src/frames.rs`, `crates/hennery-host/src/connection.rs`, `crates/hennery-sessions/src/agents.rs` (new), `hosts.rs`, `lib.rs`, `ws.rs`, the generated files | `hello`'s fields, `probe_agents`, the route, the coalesced refresh | 3 |
| `crates/hennery-testkit/tests/host_agents.rs` (new), `host_connection.rs`, `auth.rs`, and every other test that builds a `hello`; `crates/hennery-proto/tests/agents.rs`, `frames.rs` | Both ends | 3 |
| `crates/hennery/src/host_agents.rs` (new), `main.rs`, `runtime.rs`, `doctor/mod.rs`, `doctor/agents.rs`, `doctor/runtime.rs`, `doctor/spawn.rs` | `AgentSetup`, `DoctorChecks`; doctor's re-exports and their doc comments | 4 |
| `crates/hennery/tests/cli.rs` | Through the binary | 4 |
| `docs/specs/2026-09-26-acp-core-design.md`, `…-kernel-design.md`, `…-distribution-design.md` | The write-back | 5 |

All commands run from the repository root inside the dev shell (`nix develop -c …`). Work on a feature branch off `main`. Each task leaves the workspace compiling, clippy-clean and green.

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block (and a final newline).
- "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point, as whole lines (earlier blocks of the same task already applied, in order), then "with:" and its replacement.

Other "Run:" lines only check or regenerate: `cargo run -p hennery-proto --bin gen` rewrites the generated files. They change no other file.

---

### Task 1: The wire types, their bounds, and the kernel's store

**Files:**
- Create: `crates/hennery-proto/src/agents.rs`
- Modify: `crates/hennery-proto/src/lib.rs`, `rest.rs`, `codegen.rs`; `crates/hennery-kernel/src/schema.rs`, `hosts.rs`
- Regenerate: `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`
- Test: `crates/hennery-proto/tests/agents.rs` (new); `crates/hennery-kernel/tests/hosts.rs`, `tests/owner.rs`; `crates/hennery-testkit/tests/owner_filter.rs` (`hosts.rs`: 18 statements)

**Interfaces:**
- Produces: `hennery_proto::agents::{AgentAuth, AgentCli, AgentInfo, RuntimeSource, RuntimeInfo, AgentList, MaybeRuntime, bound_agents, bounded_note, is_readable_version, MAX_AGENTS, MAX_AGENT_NAME, MAX_AGENT_VERSION, MAX_SET_ID, MAX_AGENT_NOTE}`; `hennery_proto::rest::{AgentsSource, HostAgents}`; `hennery_kernel::hosts::{ReportedIn, AgentsRecord}`, `Hosts::record_agents(&self, host_id, ReportedIn, Vec<AgentInfo>, Option<RuntimeInfo>, now) -> Result<()>`, `Hosts::agents(&self, host_id) -> Result<Option<AgentsRecord>>`.

- [ ] **Step 1: Write the failing tests**

The bounds (names, versions, notes, the count, a `given` runtime), every wire name, a worst-case report under 32 KiB; the store: the latest report, bounded again, a revoked host's not stored, an unreadable one read as none, the stored bytes bounded; another owner's report invisible; the migration's columns in the owner audit.

In `crates/hennery-kernel/tests/hosts.rs`, replace:

  ```rust
      EnrollOutcome, Enrollment, HelloCheck, Hosts, MAX_LIVE_PAIRING_CODES, PAIRING_CODE_TTL_SECS, Registered, Revoke,
      TooManyPairingCodes, normalize_code, verify_proof,
  ```

with:

  ```rust
      EnrollOutcome, Enrollment, HelloCheck, Hosts, MAX_LIVE_PAIRING_CODES, PAIRING_CODE_TTL_SECS, Registered,
      ReportedIn, Revoke, TooManyPairingCodes, normalize_code, verify_proof,
  ```

In `crates/hennery-kernel/tests/hosts.rs`, replace:

  ```rust
              .enroll(&later.code, &enrollment(&key(2)), NOW + PAIRING_CODE_TTL_SECS + 2)
              .unwrap(),
      );
  }
  ```

with:

  ```rust
              .enroll(&later.code, &enrollment(&key(2)), NOW + PAIRING_CODE_TTL_SECS + 2)
              .unwrap(),
      );
  }

  // Plan 4d-B1-i: a host's agents.

  fn agent(name: &str) -> hennery_proto::agents::AgentInfo {
      hennery_proto::agents::AgentInfo {
          agent: name.into(),
          available: true,
          auth: hennery_proto::agents::AgentAuth::Unknown,
          cli: hennery_proto::agents::AgentCli::Bundled,
          adapter_version: Some("1.0.0".into()),
          images: None,
          note: None,
      }
  }

  fn managed() -> hennery_proto::agents::RuntimeInfo {
      hennery_proto::agents::RuntimeInfo {
          source: hennery_proto::agents::RuntimeSource::Managed,
          set_id: Some("abc".into()),
          pinned: Some(true),
          held: Some(false),
      }
  }

  /// Nothing until a report; then the latest report, `hello`'s or a probe's,
  /// with the collector's time. An unknown host has none at all.
  #[test]
  fn a_hosts_agents_are_its_latest_report() {
      use hennery_proto::rest::AgentsSource;
      let hosts = Hosts::open_in_memory().unwrap();
      hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap();
      assert_eq!(hosts.agents("host-9").unwrap(), None);
      let none = hosts.agents("host-1").unwrap().unwrap();
      assert_eq!(none.source, AgentsSource::None);
      assert_eq!((none.agents.len(), none.runtime, none.reported_at), (0, None, None));

      hosts
          .record_agents("host-1", ReportedIn::Hello, vec![agent("claude")], Some(managed()), NOW)
          .unwrap();
      let hello = hosts.agents("host-1").unwrap().unwrap();
      assert_eq!(hello.source, AgentsSource::Hello);
      assert_eq!(hello.agents, [agent("claude")]);
      assert_eq!(hello.runtime, Some(managed()));
      assert_eq!(hello.reported_at, Some(NOW));

      let mut live = agent("claude");
      live.auth = hennery_proto::agents::AgentAuth::Ok;
      hosts
          .record_agents("host-1", ReportedIn::Probe, vec![live.clone()], None, NOW + 9)
          .unwrap();
      let probe = hosts.agents("host-1").unwrap().unwrap();
      assert_eq!(probe.source, AgentsSource::Probe);
      assert_eq!(
          (probe.agents, probe.runtime, probe.reported_at),
          (vec![live], None, Some(NOW + 9))
      );
  }

  /// A host may lie, but only about itself, and only within the bounds.
  #[test]
  fn a_hosts_report_is_bounded_before_it_is_stored() {
      let hosts = Hosts::open_in_memory().unwrap();
      hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap();
      let mut noisy = agent("claude");
      noisy.note = Some("a\u{202E}b".into());
      noisy.adapter_version = Some("1.0 see https://x".into());
      let mut many = vec![noisy, agent("claude"), agent("bad name")];
      many.extend((0..30).map(|i| agent(&format!("a{i}"))));
      let mut runtime = managed();
      runtime.set_id = Some("../x".into());
      hosts
          .record_agents("host-1", ReportedIn::Probe, many, Some(runtime), NOW)
          .unwrap();
      let stored = hosts.agents("host-1").unwrap().unwrap();
      assert_eq!(stored.agents.len(), hennery_proto::agents::MAX_AGENTS);
      assert_eq!(stored.agents[0].note.as_deref(), Some("a\u{fffd}b"));
      assert_eq!(stored.agents[0].adapter_version, None);
      assert_eq!(
          stored.agents[1].agent, "a0",
          "the duplicate and the bad name are dropped"
      );
      assert_eq!(stored.runtime.unwrap().set_id, None);
  }

  /// What a host can make the collector store is bounded in bytes too,
  /// whatever it sends: `MAX_AGENTS` entries of at most `MAX_AGENT_NAME` +
  /// `MAX_AGENT_VERSION` + `MAX_AGENT_NOTE` bytes, which JSON's escaping at
  /// most doubles, and the set id. Here every field is at its limit and made
  /// of `"`, which escapes to two bytes: about 21 KiB, under 32 KiB.
  #[test]
  fn a_stored_report_is_bounded_in_bytes() {
      use hennery_proto::agents::{MAX_AGENT_NAME, MAX_AGENT_NOTE, MAX_AGENT_VERSION, MAX_AGENTS, MAX_SET_ID};
      let dir = tempfile::tempdir().unwrap();
      let db = dir.path().join("hennery.db");
      let hosts = Hosts::open(&db).unwrap();
      hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap();
      let worst: Vec<_> = (0..MAX_AGENTS * 4)
          .map(|i| {
              let mut a = agent(&format!("{i:03}{}", "\"".repeat(MAX_AGENT_NAME * 8)));
              a.agent.truncate(MAX_AGENT_NAME);
              a.adapter_version = Some("9".repeat(MAX_AGENT_VERSION));
              a.note = Some("\"".repeat(MAX_AGENT_NOTE * 100));
              a
          })
          .collect();
      let mut runtime = managed();
      runtime.set_id = Some("s".repeat(MAX_SET_ID));
      hosts
          .record_agents("host-1", ReportedIn::Probe, worst, Some(runtime), NOW)
          .unwrap();
      let conn = rusqlite::Connection::open(&db).unwrap();
      let bytes: i64 = conn
          .query_row(
              "SELECT length(CAST(agents AS BLOB)) FROM hosts WHERE id = 'host-1'",
              [],
              |r| r.get(0),
          )
          .unwrap();
      assert!(bytes > 16 * 1024, "every field at its limit: {bytes}");
      assert!(bytes <= 32 * 1024, "{bytes}");
  }

  /// A revoked host's report is not stored.
  #[test]
  fn a_revoked_hosts_report_is_not_stored() {
      let hosts = Hosts::open_in_memory().unwrap();
      hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap();
      hosts.revoke("host-1", NOW).unwrap();
      hosts
          .record_agents("host-1", ReportedIn::Hello, vec![agent("claude")], None, NOW)
          .unwrap();
      let record = hosts.agents("host-1").unwrap().unwrap();
      assert_eq!(record.source, hennery_proto::rest::AgentsSource::None);
  }

  /// A stored report that cannot be read is no report.
  #[test]
  fn an_unreadable_report_is_none() {
      let dir = tempfile::tempdir().unwrap();
      let db = dir.path().join("hennery.db");
      let hosts = Hosts::open(&db).unwrap();
      hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap();
      hosts
          .record_agents("host-1", ReportedIn::Hello, vec![agent("claude")], None, NOW)
          .unwrap();
      let conn = rusqlite::Connection::open(&db).unwrap();
      conn.execute("UPDATE hosts SET agents = 'not json' WHERE id = 'host-1'", [])
          .unwrap();
      let record = hosts.agents("host-1").unwrap().unwrap();
      assert_eq!(record.source, hennery_proto::rest::AgentsSource::None);
      assert!(record.agents.is_empty() && record.reported_at.is_none());
  }
  ```

In `crates/hennery-kernel/tests/owner.rs`, replace:

  ```rust
                  expected.insert("workspace_roots".into(), Value::Text("[]".into()));
              }
  ```

with:

  ```rust
                  expected.insert("workspace_roots".into(), Value::Text("[]".into()));
                  // Plan 4d-B1-i's: no agents until a host reports them.
                  expected.insert("agents".into(), Value::Null);
                  expected.insert("agents_reported_at".into(), Value::Null);
                  expected.insert("agents_source".into(), Value::Text("none".into()));
              }
  ```

In `crates/hennery-kernel/tests/owner.rs`, replace:

  ```rust
      hosts.record_workspace_roots("host-b2", &["/theirs".into()]).unwrap();
      assert_eq!(
  ```

with:

  ```rust
      hosts.record_workspace_roots("host-b2", &["/theirs".into()]).unwrap();
      hosts
          .record_agents(
              "host-b2",
              hennery_kernel::hosts::ReportedIn::Probe,
              Vec::new(),
              None,
              NOW,
          )
          .unwrap();
      assert_eq!(hosts.agents("host-b2").unwrap(), None);
      assert_eq!(
  ```

Create `crates/hennery-proto/tests/agents.rs`:

  ```rust
  //! Plan 4d-B1-i: a host's agents on the wire, and how both ends bound a
  //! report.

  use hennery_proto::agents::{
      AgentAuth, AgentCli, AgentInfo, MAX_AGENT_NOTE, MAX_AGENTS, RuntimeInfo, RuntimeSource, bound_agents, bounded_note,
  };
  use hennery_proto::rest::{AgentsSource, HostAgents};
  use serde_json::json;

  fn agent(name: &str) -> AgentInfo {
      AgentInfo {
          agent: name.into(),
          available: true,
          auth: AgentAuth::Unknown,
          cli: AgentCli::Bundled,
          adapter_version: None,
          images: None,
          note: None,
      }
  }

  #[test]
  fn every_auth_cli_and_source_value_has_its_wire_name() {
      for (auth, name) in [
          (AgentAuth::Ok, "ok"),
          (AgentAuth::Missing, "missing"),
          (AgentAuth::Unknown, "unknown"),
      ] {
          assert_eq!(serde_json::to_value(auth).unwrap(), json!(name));
      }
      for (cli, name) in [
          (AgentCli::Bundled, "bundled"),
          (AgentCli::Override, "override"),
          (AgentCli::Given, "given"),
      ] {
          assert_eq!(serde_json::to_value(cli).unwrap(), json!(name));
      }
      for (source, name) in [(RuntimeSource::Managed, "managed"), (RuntimeSource::Given, "given")] {
          assert_eq!(serde_json::to_value(source).unwrap(), json!(name));
      }
      for (source, name) in [
          (AgentsSource::None, "none"),
          (AgentsSource::Hello, "hello"),
          (AgentsSource::Probe, "probe"),
      ] {
          assert_eq!(serde_json::to_value(source).unwrap(), json!(name));
      }
      // An absent `auth` is `unknown`.
      let read: AgentInfo = serde_json::from_value(json!({"agent": "a", "available": false, "cli": "given"})).unwrap();
      assert_eq!(read.auth, AgentAuth::Unknown);
  }

  #[test]
  fn a_report_keeps_each_name_once_and_only_readable_names() {
      let mut second = agent("claude");
      second.available = false;
      let kept = bound_agents(vec![
          agent("claude"),
          second,
          agent(""),
          agent("has space"),
          agent("tab\tname"),
          agent("caf\u{e9}"),
          agent(&"a".repeat(65)),
          agent(&"a".repeat(64)),
      ]);
      let names: Vec<String> = kept.iter().map(|a| a.agent.clone()).collect();
      assert_eq!(names, ["claude".to_string(), "a".repeat(64)]);
      assert!(kept[0].available, "the first of a name is kept");
  }

  #[test]
  fn a_report_holds_at_most_max_agents() {
      let many: Vec<AgentInfo> = (0..MAX_AGENTS + 5).map(|i| agent(&format!("agent-{i}"))).collect();
      let kept = bound_agents(many);
      assert_eq!(kept.len(), MAX_AGENTS);
      assert_eq!(kept.last().unwrap().agent, format!("agent-{}", MAX_AGENTS - 1));
  }

  #[test]
  fn a_report_drops_an_unreadable_version() {
      let with = |v: &str| {
          bound_agents(vec![AgentInfo {
              adapter_version: Some(v.into()),
              ..agent("a")
          }])[0]
              .adapter_version
              .clone()
      };
      assert_eq!(with("0.81.0-beta+1"), Some("0.81.0-beta+1".into()));
      assert_eq!(with("1.2.3 see https://x"), None);
      assert_eq!(with("1_2"), None);
      assert_eq!(with(""), None);
      assert_eq!(with(&"9".repeat(64)), Some("9".repeat(64)));
      assert_eq!(with(&"9".repeat(65)), None);
  }

  #[test]
  fn a_note_is_escaped_and_cut_on_a_character_boundary() {
      assert_eq!(bounded_note("plain"), Some("plain".into()));
      assert_eq!(
          bounded_note("a\nb\u{1b}[31mc\u{202e}d"),
          Some("a\u{fffd}b\u{fffd}[31mc\u{fffd}d".into())
      );
      assert_eq!(bounded_note("   "), None);
      assert_eq!(bounded_note(""), None);
      let long = "é".repeat(MAX_AGENT_NOTE);
      let cut = bounded_note(&long).unwrap();
      assert!(
          cut.len() <= MAX_AGENT_NOTE && cut.len() > MAX_AGENT_NOTE - 2,
          "{}",
          cut.len()
      );
      assert!(cut.chars().all(|c| c == 'é'));
      let kept = bound_agents(vec![AgentInfo {
          note: Some("x\u{0}".into()),
          ..agent("a")
      }]);
      assert_eq!(kept[0].note.as_deref(), Some("x\u{fffd}"));
  }

  #[test]
  fn a_given_runtime_keeps_no_managed_field_and_a_bad_set_id_is_dropped() {
      let given = RuntimeInfo {
          source: RuntimeSource::Given,
          set_id: Some("abc".into()),
          pinned: Some(true),
          held: Some(true),
      };
      assert_eq!(
          given.bounded(),
          RuntimeInfo {
              source: RuntimeSource::Given,
              set_id: None,
              pinned: None,
              held: None,
          }
      );
      let managed = |id: &str| {
          RuntimeInfo {
              source: RuntimeSource::Managed,
              set_id: Some(id.into()),
              pinned: Some(false),
              held: None,
          }
          .bounded()
      };
      assert_eq!(managed("0123abcdef").set_id.as_deref(), Some("0123abcdef"));
      assert_eq!(managed("../etc").set_id, None);
      assert_eq!(managed(&"a".repeat(129)).set_id, None);
      assert_eq!(managed("a").pinned, Some(false));
  }

  /// Every field at its worst, every agent there: the stored report and the
  /// route's answer stay small. `"` is the worst character: JSON escapes it to
  /// two bytes, in a name as in a note.
  #[test]
  fn a_bounded_report_stays_under_32_kib() {
      let worst: Vec<AgentInfo> = (0..MAX_AGENTS + 1)
          .map(|i| AgentInfo {
              agent: format!("{i:0>2}{}", "\"".repeat(62)),
              available: true,
              auth: AgentAuth::Unknown,
              cli: AgentCli::Override,
              adapter_version: Some("9".repeat(64)),
              images: Some(true),
              note: Some("\"".repeat(4 * MAX_AGENT_NOTE)),
          })
          .collect();
      let agents = bound_agents(worst);
      assert_eq!(agents.len(), MAX_AGENTS);
      assert!(agents.iter().all(|a| a.note.as_ref().unwrap().len() == MAX_AGENT_NOTE));
      let answer = HostAgents {
          host_id: format!("host-{}", "f".repeat(16)),
          agents,
          runtime: Some(
              RuntimeInfo {
                  source: RuntimeSource::Managed,
                  set_id: Some("f".repeat(128)),
                  pinned: Some(true),
                  held: Some(true),
              }
              .bounded(),
          ),
          reported_at: Some("2026-10-02T00:00:00Z".into()),
          source: AgentsSource::Probe,
          live: true,
      };
      let bytes = serde_json::to_vec(&answer).unwrap().len();
      assert!(bytes > 16 * 1024, "every field at its limit: {bytes}");
      assert!(bytes <= 32 * 1024, "{bytes}");
  }
  ```

In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

  ```rust
          16,
  ```

with:

  ```rust
          18,
  ```

- [ ] **Step 2: Run them, and see them fail**

Run: `nix develop -c cargo test -p hennery-kernel --test hosts --test owner --locked; nix develop -c cargo test -p hennery-proto --test agents --locked`
Expected: FAIL to compile: `hennery_proto::agents`, `rest::AgentsSource`/`HostAgents`, `hosts::ReportedIn`, `Hosts::record_agents` and `Hosts::agents` do not exist yet (the `agents` tests of `hennery-proto`, the `hosts` and `owner` tests of `hennery-kernel`).

- [ ] **Step 3: Write the implementation**

In `crates/hennery-kernel/src/hosts.rs`, replace:

  ```rust
  use hennery_proto::frames::Capabilities;
  ```

with:

  ```rust
  use hennery_proto::agents::{AgentInfo, RuntimeInfo, bound_agents};
  use hennery_proto::frames::Capabilities;
  use hennery_proto::rest::AgentsSource;
  ```

In `crates/hennery-kernel/src/hosts.rs`, replace:

  ```rust

  /// Whether text a host sent can be shown (the review's A6): at most `max`
  ```

with:

  ```rust

  /// What a host's agents report came in (plan 4d-B1-i).
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum ReportedIn {
      /// A reconciled connection's `hello`.
      Hello,
      /// The answer to a `probe_agents`.
      Probe,
  }

  /// A host's latest report of its agents (plan 4d-B1-i), as stored.
  #[derive(Debug, Clone, PartialEq)]
  pub struct AgentsRecord {
      pub agents: Vec<AgentInfo>,
      pub runtime: Option<RuntimeInfo>,
      /// When the collector stored it (its own clock); `None` with `source:
      /// none`.
      pub reported_at: Option<i64>,
      pub source: AgentsSource,
  }

  /// The stored JSON of a report.
  #[derive(serde::Serialize, serde::Deserialize)]
  struct StoredAgents {
      agents: Vec<AgentInfo>,
      #[serde(default, skip_serializing_if = "Option::is_none")]
      runtime: Option<RuntimeInfo>,
  }

  /// Whether text a host sent can be shown (the review's A6): at most `max`
  ```

In `crates/hennery-kernel/src/hosts.rs`, replace:

  ```rust
  /// Invisible Unicode format characters (bidi overrides and isolates,
  /// zero-width characters, the byte-order mark): a host name holding one can
  /// display as another host's name.
  pub(crate) fn is_format_char(c: char) -> bool {
      matches!(c,
          '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
          | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
  }
  ```

with:

  ```rust
  // Invisible Unicode format characters (bidi overrides and isolates,
  // zero-width characters, the byte-order mark): a host name holding one can
  // display as another host's name. One list with the agents' reports.
  pub(crate) use hennery_proto::agents::is_format_char;
  ```

In `crates/hennery-kernel/src/hosts.rs`, replace:

  ```rust

      pub fn is_revoked(&self, host_id: &str) -> Result<bool> {
  ```

with:

  ```rust

      /// Store a report of `host_id`'s agents (plan 4d-B1-i), bounded again
      /// here whatever the host claims (`bound_agents`, `RuntimeInfo::bounded`):
      /// a host may lie, but only about itself. Not for a revoked host.
      pub fn record_agents(
          &self,
          host_id: &str,
          reported_in: ReportedIn,
          agents: Vec<AgentInfo>,
          runtime: Option<RuntimeInfo>,
          now: i64,
      ) -> Result<()> {
          let stored = StoredAgents {
              agents: bound_agents(agents),
              runtime: runtime.map(RuntimeInfo::bounded),
          };
          let source = match reported_in {
              ReportedIn::Hello => "hello",
              ReportedIn::Probe => "probe",
          };
          self.conn().execute(
              "UPDATE hosts SET agents = ?2, agents_reported_at = ?3, agents_source = ?4
               WHERE id = ?1 AND revoked_at IS NULL AND owner_id = ?5",
              params![host_id, serde_json::to_string(&stored)?, now, source, self.owner],
          )?;
          Ok(())
      }

      /// The latest report of `host_id`'s agents; `None` for an unknown host.
      /// A host that never reported, or whose report cannot be read, has
      /// `source: none`.
      pub fn agents(&self, host_id: &str) -> Result<Option<AgentsRecord>> {
          let row: Option<(Option<String>, Option<i64>, String)> = self
              .conn()
              .query_row(
                  "SELECT agents, agents_reported_at, agents_source FROM hosts WHERE id = ?1 AND owner_id = ?2",
                  [host_id, &self.owner],
                  |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
              )
              .optional()?;
          let Some((json, reported_at, source)) = row else {
              return Ok(None);
          };
          let source = match source.as_str() {
              "hello" => AgentsSource::Hello,
              "probe" => AgentsSource::Probe,
              _ => AgentsSource::None,
          };
          let stored = json.and_then(|json| serde_json::from_str::<StoredAgents>(&json).ok());
          Ok(Some(match (stored, reported_at, source) {
              (Some(stored), Some(at), AgentsSource::Hello | AgentsSource::Probe) => AgentsRecord {
                  agents: stored.agents,
                  runtime: stored.runtime,
                  reported_at: Some(at),
                  source,
              },
              _ => AgentsRecord {
                  agents: Vec::new(),
                  runtime: None,
                  reported_at: None,
                  source: AgentsSource::None,
              },
          }))
      }

      pub fn is_revoked(&self, host_id: &str) -> Result<bool> {
  ```

In `crates/hennery-kernel/src/schema.rs`, replace:

  ```rust
      ",
  ];
  ```

with:

  ```rust
      ",
      // A host's agents (kernel spec §1.1, §4.3; plan 4d-B1-i): its latest
      // report, as JSON (`{"agents": […], "runtime": {…}}`), when the
      // collector received it, and from what: `hello` or `probe` (`none`
      // until the first). Columns added, never a rebuild of `hosts`.
      "
      ALTER TABLE hosts ADD COLUMN agents TEXT;
      ALTER TABLE hosts ADD COLUMN agents_reported_at INTEGER;
      ALTER TABLE hosts ADD COLUMN agents_source TEXT NOT NULL DEFAULT 'none';
      ",
  ];
  ```

In `crates/hennery-kernel/src/schema.rs`, replace:

  ```rust
      }
  }
  ```

with:

  ```rust
      }

      /// Plan 4d-B1-i: the agents' columns are added to the hosts there are,
      /// which keep every other column and have reported nothing yet.
      #[test]
      fn a_paired_host_gains_the_agents_columns_with_nothing_reported() {
          let added = MIGRATIONS
              .iter()
              .position(|m| m.contains("ADD COLUMN agents_source"))
              .unwrap();
          let mut conn = Connection::open_in_memory().unwrap();
          crate::db::migrate_component(&mut conn, COMPONENT, &MIGRATIONS[..added]).unwrap();
          conn.execute(
              "INSERT INTO hosts(id, owner_id, name, public_key, platform, host_version, default_hat_id, created_at,
                                 workspace_roots)
               SELECT 'host-1', s.owner_id, 'n', 'k', 'p', 'v', s.value, 0, '[\"/srv/projects\"]'
               FROM settings s WHERE s.key = 'default_hat_id'",
              [],
          )
          .unwrap();
          crate::db::migrate_component(&mut conn, COMPONENT, MIGRATIONS).unwrap();
          let row: (Option<String>, Option<i64>, String, String) = conn
              .query_row(
                  "SELECT agents, agents_reported_at, agents_source, workspace_roots FROM hosts WHERE id = 'host-1'",
                  [],
                  |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
              )
              .unwrap();
          assert_eq!(row, (None, None, "none".into(), "[\"/srv/projects\"]".into()));
      }
  }
  ```

Create `crates/hennery-proto/src/agents.rs`:

  ```rust
  //! What a host says of its agents (ACP core §6 "Agent availability"; plan
  //! 4d-B1-i): in `hello`, a cheap static view, and in `agents`, the answer
  //! to `probe_agents`, a live one. Both ends bound a report the same way
  //! (`bound_agents`, `RuntimeInfo::bounded`): the host before it sends one,
  //! the collector again before it stores one, since a host may lie, but only
  //! about itself.

  use schemars::JsonSchema;
  use serde::{Deserialize, Serialize};
  use serde_json::Value;
  use ts_rs::TS;

  /// The most agents one report holds; the rest are dropped.
  pub const MAX_AGENTS: usize = 16;
  /// The longest agent (profile) name kept, in bytes.
  pub const MAX_AGENT_NAME: usize = 64;
  /// The longest adapter version kept, in bytes.
  pub const MAX_AGENT_VERSION: usize = 64;
  /// The longest adapter set id kept, in bytes.
  pub const MAX_SET_ID: usize = 128;
  /// The longest note kept, in bytes.
  pub const MAX_AGENT_NOTE: usize = 512;

  /// Whether an agent's CLI says it is logged in (ACP core §6): only the
  /// verdict, never the account. `unknown` until a probe asked, and when the
  /// host knows no CLI to ask, or it did not answer (in time, or at all: a CLI
  /// ended by a signal said nothing). A host fact: agent logins
  /// are per OS user in v1, every hat's included. If plan 8h ever makes
  /// `auth.json` private per hat, `auth` must become per (host, hat).
  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  #[serde(rename_all = "snake_case")]
  pub enum AgentAuth {
      /// The CLI's status command exited 0.
      Ok,
      /// It exited non-zero.
      Missing,
      #[default]
      Unknown,
  }

  /// Which CLI an agent runs (distribution spec §3.2, §13 decision 5).
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  #[serde(rename_all = "snake_case")]
  pub enum AgentCli {
      /// The CLI bundled in the host's adapter set.
      Bundled,
      /// The operator's own CLI (`--use-cli`, recorded in `host.toml`).
      Override,
      /// A command given to `host run --agent` (e.g. Nix-provided adapters):
      /// hennery manages nothing of it.
      Given,
  }

  /// One agent of a host (ACP core §6).
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  pub struct AgentInfo {
      /// The profile name: what `StartSessionRequest.agent` takes.
      pub agent: String,
      /// In `hello`: launchable as configured. After a probe: its adapter
      /// started and answered `initialize`.
      pub available: bool,
      #[serde(default)]
      pub auth: AgentAuth,
      pub cli: AgentCli,
      /// In `hello`: the version the adapter set records. After a probe: the
      /// version the adapter's `initialize` answered with (`agentInfo`), if
      /// it gave a readable one. Absent for a `given` agent until a probe.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "string | undefined", optional)]
      pub adapter_version: Option<String>,
      /// Whether the agent takes images in prompts (its `initialize`'s
      /// `promptCapabilities.image`). Clients hide images only when this is
      /// `false`. Absent means no probe answered (none has run, or the
      /// adapter did not answer): a client then allows images, and the
      /// server's 409 `images_unsupported` remains the guard.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "boolean | undefined", optional)]
      pub images: Option<bool>,
      /// Why the agent is unavailable, or a caveat, in the host's words: never
      /// anything an agent or its CLI printed.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "string | undefined", optional)]
      pub note: Option<String>,
  }

  /// Where a host's agents come from (distribution spec §3.2).
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  #[serde(rename_all = "snake_case")]
  pub enum RuntimeSource {
      /// An adapter set hennery installed and pins.
      Managed,
      /// `host run --agent` commands.
      Given,
  }

  /// The host's adapter runtime. `set_id`, `pinned` and `held` are a managed
  /// runtime's only.
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  pub struct RuntimeInfo {
      pub source: RuntimeSource,
      /// The adapter set the agents launch from; absent when none is
      /// installed.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "string | undefined", optional)]
      pub set_id: Option<String>,
      /// That set is the one this host's binary pins.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "boolean | undefined", optional)]
      pub pinned: Option<bool>,
      /// A rollback holds the host on its set (`hennery host adapters
      /// rollback`).
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "boolean | undefined", optional)]
      pub held: Option<bool>,
  }

  impl RuntimeInfo {
      /// Bounded as a report is (`bound_agents`): a `given` runtime keeps no
      /// managed field, and a set id that is not 1 to `MAX_SET_ID` of
      /// `[0-9A-Za-z.+-]` is dropped.
      pub fn bounded(self) -> Self {
          match self.source {
              RuntimeSource::Given => Self {
                  source: RuntimeSource::Given,
                  set_id: None,
                  pinned: None,
                  held: None,
              },
              RuntimeSource::Managed => Self {
                  set_id: self.set_id.filter(|id| is_token(id, MAX_SET_ID)),
                  ..self
              },
          }
      }
  }

  /// `s` is 1 to `max` bytes of `[0-9A-Za-z.+-]` (doctor's readable
  /// version).
  fn is_token(s: &str, max: usize) -> bool {
      !s.is_empty() && s.len() <= max && s.chars().all(|c| c.is_ascii_alphanumeric() || ".+-".contains(c))
  }

  /// Whether an adapter's version can be reported: 1 to
  /// `MAX_AGENT_VERSION` of `[0-9A-Za-z.+-]`.
  pub fn is_readable_version(version: &str) -> bool {
      is_token(version, MAX_AGENT_VERSION)
  }

  /// Invisible Unicode format characters (bidi overrides and isolates,
  /// zero-width characters, the byte-order mark). The kernel's host registry
  /// refuses them in a host's name with this same list (the review's O4).
  pub fn is_format_char(c: char) -> bool {
      matches!(c,
          '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
          | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
  }

  /// `note` as a report may carry it: control and format characters replaced
  /// by U+FFFD, then cut to `MAX_AGENT_NOTE` bytes on a character boundary;
  /// `None` if nothing is left.
  pub fn bounded_note(note: &str) -> Option<String> {
      let mut out = String::new();
      for c in note.chars() {
          let c = if c.is_control() || is_format_char(c) {
              '\u{FFFD}'
          } else {
              c
          };
          if out.len() + c.len_utf8() > MAX_AGENT_NOTE {
              break;
          }
          out.push(c);
      }
      (!out.trim().is_empty()).then_some(out)
  }

  /// `agents` as a report may carry them: each name 1 to `MAX_AGENT_NAME`
  /// bytes of printable ASCII, else the entry is dropped; the first of each
  /// name only; at most `MAX_AGENTS`; a version that is not 1 to
  /// `MAX_AGENT_VERSION` of `[0-9A-Za-z.+-]` dropped; the note bounded
  /// (`bounded_note`).
  pub fn bound_agents(agents: Vec<AgentInfo>) -> Vec<AgentInfo> {
      let mut out: Vec<AgentInfo> = Vec::new();
      for agent in agents {
          if out.len() == MAX_AGENTS {
              break;
          }
          let name_ok = !agent.agent.is_empty()
              && agent.agent.len() <= MAX_AGENT_NAME
              && agent.agent.chars().all(|c| c.is_ascii_graphic());
          if !name_ok || out.iter().any(|a| a.agent == agent.agent) {
              continue;
          }
          out.push(AgentInfo {
              adapter_version: agent.adapter_version.filter(|v| is_readable_version(v)),
              note: agent.note.as_deref().and_then(bounded_note),
              ..agent
          });
      }
      out
  }

  /// A report's agents, read leniently, as `Capabilities` is: an entry this
  /// build cannot read (a newer host's value, a missing field) is skipped,
  /// never a reason to refuse the whole frame. An absent field is an empty
  /// list.
  #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
  pub struct AgentList(pub Vec<AgentInfo>);

  impl<'de> Deserialize<'de> for AgentList {
      fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
          let raw: Vec<Value> = Deserialize::deserialize(deserializer)?;
          Ok(Self(
              raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect(),
          ))
      }
  }

  /// A report's runtime, read leniently: one this build cannot read is
  /// absent.
  #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
  pub struct MaybeRuntime(pub Option<RuntimeInfo>);

  impl MaybeRuntime {
      pub fn is_none(&self) -> bool {
          self.0.is_none()
      }
  }

  impl<'de> Deserialize<'de> for MaybeRuntime {
      fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
          let raw: Option<Value> = Deserialize::deserialize(deserializer)?;
          Ok(Self(raw.and_then(|v| serde_json::from_value(v).ok())))
      }
  }
  ```

In `crates/hennery-proto/src/codegen.rs`, replace:

  ```rust
  use crate::{frames, rest};
  ```

with:

  ```rust
  use crate::{agents, frames, rest};
  ```

In `crates/hennery-proto/src/codegen.rs`, replace:

  ```rust
          rest::SessionCatalog,
          rest::PendingItem,
          rest::AnswerRequest,
          rest::AnswerResponse,
          rest::PairingCodeResponse,
          rest::EnrollRequest,
          rest::EnrollResponse,
          rest::HostItem,
          rest::RecentProject,
          rest::HostProjects,
          rest::DirectoryListing,
          rest::SetupRequest,
          rest::SetupResponse,
          rest::LoginRequest,
          rest::StepUpRequest,
          rest::AuthSessionItem,
  ```

with:

  ```rust
          rest::SessionCatalog,
          rest::PendingItem,
          rest::AnswerRequest,
          rest::AnswerResponse,
          rest::PairingCodeResponse,
          rest::EnrollRequest,
          rest::EnrollResponse,
          rest::HostItem,
          rest::HostAgents,
          rest::RecentProject,
          rest::HostProjects,
          rest::DirectoryListing,
          rest::SetupRequest,
          rest::SetupResponse,
          rest::LoginRequest,
          rest::StepUpRequest,
          rest::AuthSessionItem,
  ```

In `crates/hennery-proto/src/codegen.rs`, replace:

  ```rust
          frames::ForgetOutcome,
          frames::HostFrame,
  ```

with:

  ```rust
          frames::ForgetOutcome,
          agents::AgentAuth,
          agents::AgentCli,
          agents::AgentInfo,
          agents::RuntimeSource,
          agents::RuntimeInfo,
          frames::HostFrame,
  ```

In `crates/hennery-proto/src/codegen.rs`, replace:

  ```rust
          rest::HostItem,
          rest::RecentProject,
  ```

with:

  ```rust
          rest::HostItem,
          rest::AgentsSource,
          rest::HostAgents,
          rest::RecentProject,
  ```

In `crates/hennery-proto/src/lib.rs`, replace:

  ```rust

  pub mod frames;
  ```

with:

  ```rust

  pub mod agents;
  pub mod frames;
  ```

In `crates/hennery-proto/src/rest.rs`, replace:

  ```rust
      pub revoked_at: Option<String>,
  }
  ```

with:

  ```rust
      pub revoked_at: Option<String>,
  }

  /// Where `HostAgents` comes from.
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  #[serde(rename_all = "snake_case")]
  pub enum AgentsSource {
      /// The host never reported its agents: an older host, or one not
      /// reconciled since it was paired.
      None,
      /// Its latest reconciled connection's `hello`: the static view.
      Hello,
      /// A `probe_agents`: the live view.
      Probe,
  }

  /// `GET /api/hosts/{id}/agents[?refresh=1]` (plan 4d-B1-i): the host's
  /// latest report of its agents, whatever its age; with `refresh=1`, after
  /// one probe of a connected host that can be probed (or the probe's
  /// budget, 20 s).
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
  pub struct HostAgents {
      pub host_id: String,
      pub agents: Vec<crate::agents::AgentInfo>,
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "RuntimeInfo | undefined", optional)]
      pub runtime: Option<crate::agents::RuntimeInfo>,
      /// When the collector received the report, RFC 3339; absent with
      /// `source: none`.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "string | undefined", optional)]
      pub reported_at: Option<String>,
      pub source: AgentsSource,
      /// The host is connected and reconciled now.
      pub live: bool,
  }
  ```

Run: `nix develop -c cargo run -p hennery-proto --bin gen`

- [ ] **Step 4: Run the tests, and see them pass**

Run: `nix develop -c cargo test -p hennery-proto --test agents --locked && nix develop -c cargo test -p hennery-kernel --locked && nix develop -c cargo test -p hennery-testkit --test owner_filter --locked`
Expected: all pass.

- [ ] **Step 5: Revert-probes**

Each on the whole plan's tree; each must make a test fail, then be restored. The lists name every test that failed (the commands: the task's test files, and the testkit's where the collector is involved).
- P1: in `crates/hennery-proto/src/agents.rs`, replace `if out.len() == MAX_AGENTS {` with `if out.len() == usize::MAX {`. Fails: `a_bounded_report_stays_under_32_kib`, `a_hosts_report_is_bounded_before_it_is_stored`, `a_report_holds_at_most_max_agents`, `a_stored_report_is_bounded_in_bytes`. Restore it.
- P2: in `crates/hennery-proto/src/agents.rs`, replace `let name_ok = !agent.agent.is_empty()` with `let name_ok = true || !agent.agent.is_empty()`. Fails: `a_report_keeps_each_name_once_and_only_readable_names`. Restore it.
- P3: in `crates/hennery-proto/src/agents.rs`, replace `if !name_ok || out.iter().any(|a| a.agent == agent.agent) {` with `if !name_ok {`. Fails: `a_report_keeps_each_name_once_and_only_readable_names`. Restore it.
- P4: in `crates/hennery-proto/src/agents.rs`, replace `adapter_version: agent.adapter_version.filter(|v| is_readable_version(v)),` with `adapter_version: agent.adapter_version,`. Fails: `a_hosts_report_is_bounded_before_it_is_stored`, `a_report_drops_an_unreadable_version`. Restore it.
- P5: in `crates/hennery-proto/src/agents.rs`, replace `note: agent.note.as_deref().and_then(bounded_note),` with `note: agent.note,`. Fails: `a_bounded_report_stays_under_32_kib`, `a_hosts_report_is_bounded_before_it_is_stored`, `a_note_is_escaped_and_cut_on_a_character_boundary`, `a_stored_report_is_bounded_in_bytes`. Restore it.
- P6: in `crates/hennery-proto/src/agents.rs`, replace `let c = if c.is_control() || is_format_char(c) {` with `let c = if false {`. Fails: `a_note_is_escaped_and_cut_on_a_character_boundary`. Restore it.
- P7: in `crates/hennery-proto/src/agents.rs`, replace `if out.len() + c.len_utf8() > MAX_AGENT_NOTE {` with `if false {`. Fails: `a_bounded_report_stays_under_32_kib`, `a_note_is_escaped_and_cut_on_a_character_boundary`, `a_stored_report_is_bounded_in_bytes`. Restore it.
- P8: in `crates/hennery-proto/src/agents.rs`, replace `source: RuntimeSource::Given, ⏎ set_id: None,` with `source: RuntimeSource::Given, ⏎ set_id: self.set_id.clone(),`. Fails: `a_given_runtime_keeps_no_managed_field_and_a_bad_set_id_is_dropped`. Restore it.
- P9: in `crates/hennery-proto/src/agents.rs`, replace `set_id: self.set_id.filter(|id| is_token(id, MAX_SET_ID)),` with `set_id: self.set_id,`. Fails: `a_given_runtime_keeps_no_managed_field_and_a_bad_set_id_is_dropped`, `a_hosts_report_is_bounded_before_it_is_stored`. Restore it.
- P10: in `crates/hennery-proto/src/agents.rs`, replace `/// It exited non-zero. ⏎ Missing, ⏎ #[default] ⏎ Unknown,` with `/// It exited non-zero. ⏎ #[default] ⏎ Missing, ⏎ Unknown,`. Fails: `an_agents_answer_reads_its_agents_leniently`, `every_auth_cli_and_source_value_has_its_wire_name`. Restore it.
- K1: in `crates/hennery-kernel/src/hosts.rs`, replace `agents: bound_agents(agents),` with `agents,`. Fails: `a_hosts_report_is_bounded_before_it_is_stored`, `a_stored_report_is_bounded_in_bytes`. Restore it.
- K2: in `crates/hennery-kernel/src/hosts.rs`, replace `runtime: runtime.map(RuntimeInfo::bounded),` with `runtime,`. Fails: `a_hosts_report_is_bounded_before_it_is_stored`. Restore it.
- K3: in `crates/hennery-kernel/src/hosts.rs`, replace `WHERE id = ?1 AND revoked_at IS NULL AND owner_id = ?5",` with `WHERE id = ?1 AND owner_id = ?5",`. Fails: `a_revoked_hosts_report_is_not_stored`. Restore it.
- K4: in `crates/hennery-kernel/src/hosts.rs`, replace `WHERE id = ?1 AND revoked_at IS NULL AND owner_id = ?5",` with `WHERE id = ?1 AND revoked_at IS NULL AND ?5 = ?5",`. Fails: `every_query_of_the_stores_filters_by_the_owner`. Restore it.
- K5: in `crates/hennery-kernel/src/hosts.rs`, replace `agents_source FROM hosts WHERE id = ?1 AND owner_id = ?2",` with `agents_source FROM hosts WHERE id = ?1 AND ?2 = ?2",`. Fails: `another_owners_hosts_and_codes_are_invisible_to_the_registry`, `every_query_of_the_stores_filters_by_the_owner`. Restore it.
- K6: in `crates/hennery-kernel/src/hosts.rs`, replace `ReportedIn::Hello => "hello",` with `ReportedIn::Hello => "probe",`. Fails: `a_hosts_agents_are_its_latest_report`. Restore it.
- K7: in `crates/hennery-kernel/src/hosts.rs`, replace `"hello" => AgentsSource::Hello,` with `"hello" => AgentsSource::Probe,`. Fails: `a_hosts_agents_are_its_latest_report`. Restore it.
- K8: in `crates/hennery-kernel/src/hosts.rs`, replace `reported_at: None, ⏎ source: AgentsSource::None,` with `reported_at, ⏎ source: AgentsSource::None,`. Fails: `an_unreadable_report_is_none`. Restore it.
- K9: in `crates/hennery-kernel/src/schema.rs`, replace `ADD COLUMN agents_source TEXT NOT NULL DEFAULT 'none';` with `ADD COLUMN agents_source TEXT NOT NULL DEFAULT 'hello';`. Fails: `a_3b_ii_database_keeps_its_owner_or_gets_one`, `a_paired_host_gains_the_agents_columns_with_nothing_reported`. Restore it.

- [ ] **Step 6: The full checks**

Run the five commands of "Global Constraints", and the web checks. Expected: all pass; **1541 tests**.

- [ ] **Step 7: Commit**

```bash
git add crates/hennery-proto crates/hennery-kernel crates/hennery-testkit/tests/owner_filter.rs schema web/src/generated
git commit -m "feat(kernel): store a host's latest report of its agents"
```

### Task 2: The host's checks

**Files:**
- Create: `crates/hennery-host/src/availability.rs`
- Modify: `crates/hennery-host/src/adapter.rs`, `lib.rs`, `runtime/agents.rs`
- Test: `crates/hennery-host/tests/availability.rs` (new), `tests/adapter.rs`, `tests/runtime.rs`

**Interfaces:**
- Consumes: Task 1's `hennery_proto::agents`.
- Produces: `hennery_host::availability::{PROBE_BUDGET, LoginCheck, AgentChecks, NoChecks, given_runtime, static_agents, Started, initialize, logged_in, probe, OneProbe, ProbeSlot}`; `hennery_host::adapter::exit_status(&AgentCommand, &Path, Duration) -> io::Result<Option<ExitInfo>>`; `runtime::agents::Agents.infos: Vec<AgentInfo>`; `runtime::agents::Prepared.runtime: Option<RuntimeInfo>`.
- Unchanged: `Adapter::spawn` and `spawn_stripped` (their command is built by the new `guarded_command`, which `exit_status` shares).

- [ ] **Step 1: Write the failing tests**

`initialize` against scripted adapters (an answer, no capabilities, an error, an exit, no program, silence until the deadline), each leaving nothing running; the adapter's directory and stripped environment; `logged_in` for every exit and for silence, a missing program and no question, its output going nowhere, in a guarded group; a host SIGKILLed while a CLI runs leaves none of its group; the whole probe, within one budget; the static view; one probe at a time; what a managed set reports of each agent and of itself.

In `crates/hennery-host/tests/adapter.rs`, replace:

  ```rust
      wait_dead(guard).await;
  }
  ```

with:

  ```rust
      wait_dead(guard).await;
  }

  /// Set for the copy of this test binary that
  /// `host_sigkill_kills_a_cli_s_whole_group` runs as a host: the directory
  /// its CLI writes its pids to.
  const STATUS_HOST_DIR_VAR: &str = "HENNERY_TEST_SIGKILLED_STATUS_HOST_DIR";

  /// Not a test of its own: a host that asks a CLI for its exit status
  /// (`exit_status`, plan 4d-B1-i), and waits to be killed. The CLI and its
  /// grandchild ignore SIGTERM and never end.
  #[tokio::test]
  async fn sigkilled_status_host() {
      let Some(dir) = std::env::var_os(STATUS_HOST_DIR_VAR) else {
          return;
      };
      let dir = Path::new(&dir);
      let cli = sh(&format!(
          "trap '' TERM; sleep 600 & echo $! > {g}.tmp; mv {g}.tmp {g}; echo $(ps -o pgid= -p $$) > {p}.tmp; mv {p}.tmp {p}; echo $$ > {l}; wait",
          g = dir.join("grandchild").display(),
          p = dir.join("pgid").display(),
          l = dir.join("leader").display(),
      ));
      let _ = hennery_host::adapter::exit_status(&cli, dir, Duration::from_secs(600)).await;
      std::future::pending::<()>().await;
  }

  /// Hazard (b): a CLI asked for its login status has the host's guard, as
  /// an adapter does: a host that dies uncleanly leaves no process of its
  /// group behind.
  #[tokio::test]
  async fn host_sigkill_kills_a_cli_s_whole_group() {
      let dir = tempfile::tempdir().unwrap();
      let mut command = std::process::Command::new(std::env::current_exe().unwrap());
      command
          .args(["--exact", "sigkilled_status_host", "--nocapture"])
          .env(STATUS_HOST_DIR_VAR, dir.path())
          .stdout(std::process::Stdio::null())
          .stderr(std::process::Stdio::null());
      let mut copy = HostCopy {
          host: command.spawn().unwrap(),
          pgid: None,
      };
      let pgid = read_pid(&dir.path().join("pgid")).await;
      copy.pgid = Some(pgid);
      let grandchild = read_pid(&dir.path().join("grandchild")).await;
      let leader = read_pid(&dir.path().join("leader")).await;
      assert_ne!(pgid, leader, "the CLI leads its own group: no guard");
      // SAFETY: kill(2) on the host this test started.
      assert_eq!(unsafe { libc::kill(copy.host.id() as i32, libc::SIGKILL) }, 0);
      copy.host.wait().unwrap();
      for pid in [grandchild, leader, pgid] {
          wait_dead(pid).await;
      }
      copy.pgid = None;
  }

  /// A CLI that ends by itself takes its guard with it: `exit_status` kills
  /// the group whatever way it ended.
  #[tokio::test]
  async fn a_cli_s_exit_takes_its_guard_with_it() {
      let dir = tempfile::tempdir().unwrap();
      let pgid_file = dir.path().join("pgid");
      let cli = sh(&format!("echo $(ps -o pgid= -p $$) > {}; exit 0", pgid_file.display()));
      let ended = hennery_host::adapter::exit_status(&cli, dir.path(), Duration::from_secs(10))
          .await
          .unwrap();
      assert_eq!(ended.map(|e| e.code), Some(Some(0)));
      wait_dead(read_pid(&pgid_file).await).await;
  }

  /// The review's A1: a caller that stops waiting (a task aborted, or a
  /// deadline of its own) still has the CLI's whole group killed, a
  /// grandchild that ignores SIGTERM included.
  #[tokio::test]
  async fn a_dropped_status_check_kills_its_group() {
      let dir = tempfile::tempdir().unwrap();
      let grandchild_file = dir.path().join("grandchild");
      let cli = sh(&format!(
          "trap '' TERM; sleep 600 & echo $! > {g}.tmp; mv {g}.tmp {g}; wait",
          g = grandchild_file.display()
      ));
      // Boxed, so that `drop` drops the future itself, not a pin of it.
      let mut check = Box::pin(hennery_host::adapter::exit_status(
          &cli,
          dir.path(),
          Duration::from_secs(600),
      ));
      // Polled until the grandchild runs, then dropped.
      let grandchild = tokio::select! {
          _ = &mut check => panic!("the check ended by itself"),
          pid = read_pid(&grandchild_file) => pid,
      };
      drop(check);
      wait_dead(grandchild).await;
  }
  ```

Create `crates/hennery-host/tests/availability.rs`:

  ```rust
  //! Plan 4d-B1-i: the host's agents, static and live, against plain shell
  //! adapters and CLIs: every outcome of `initialize` (check 3) and of a
  //! CLI's login status (check 4), what a probe makes of them, that it keeps
  //! to its budget, and that nothing it starts outlives it.

  use hennery_host::adapter::{AgentCommand, exit_status};
  use hennery_host::availability::{
      self, AgentChecks, LoginCheck, NoChecks, OneProbe, Started, initialize, logged_in, static_agents,
  };
  use hennery_proto::agents::{AgentAuth, AgentCli, AgentInfo, MAX_AGENTS};
  use std::collections::HashMap;
  use std::path::Path;
  use std::sync::Arc;
  use std::time::Duration;
  use tokio::time::Instant;

  fn sh(script: &str) -> AgentCommand {
      AgentCommand {
          program: "sh".into(),
          args: vec!["-c".into(), script.into()],
          env: Vec::new(),
      }
  }

  /// An adapter that reads `initialize` and answers it with `line`, then
  /// stays up, as a real one does.
  fn answering(line: &str) -> AgentCommand {
      sh(&format!("read line; printf '%s\\n' '{line}'; exec sleep 60"))
  }

  fn alive(pid: i32) -> bool {
      // SAFETY: signal 0 only probes for existence.
      unsafe { libc::kill(pid, 0) == 0 }
  }

  /// Poll until `pid` is gone.
  async fn wait_dead(pid: i32) {
      let deadline = Instant::now() + Duration::from_secs(5);
      while alive(pid) {
          assert!(Instant::now() < deadline, "pid {pid} is still alive");
          tokio::time::sleep(Duration::from_millis(20)).await;
      }
  }

  /// Poll until `path` holds a pid.
  async fn read_pid(path: &Path) -> i32 {
      let deadline = Instant::now() + Duration::from_secs(5);
      loop {
          if let Some(pid) = std::fs::read_to_string(path).ok().and_then(|s| s.trim().parse().ok()) {
              return pid;
          }
          assert!(Instant::now() < deadline, "no pid in {}", path.display());
          tokio::time::sleep(Duration::from_millis(20)).await;
      }
  }

  fn soon() -> Instant {
      Instant::now() + Duration::from_secs(10)
  }

  fn agent(name: &str, cli: AgentCli) -> AgentInfo {
      AgentInfo {
          agent: name.into(),
          available: true,
          auth: AgentAuth::Unknown,
          cli,
          adapter_version: None,
          images: None,
          note: None,
      }
  }

  // Check 3: `initialize`.

  #[tokio::test]
  async fn an_adapter_that_answers_gives_its_version_and_whether_it_takes_images() {
      let dir = tempfile::tempdir().unwrap();
      let started = initialize(
          &answering(
              r#"{"jsonrpc":"2.0","id":0,"result":{"agentInfo":{"name":"a","version":"1.2.3"},"agentCapabilities":{"promptCapabilities":{"image":true}}}}"#,
          ),
          dir.path(),
          soon(),
      )
      .await;
      assert_eq!(
          started,
          Started::Answered {
              version: Some("1.2.3".into()),
              images: true
          }
      );
  }

  /// An answer that names neither: no version, no images. Lines that are not
  /// its answer (noise, another id) are skipped.
  #[tokio::test]
  async fn an_answer_without_agent_info_or_prompt_capabilities_offers_no_images() {
      let dir = tempfile::tempdir().unwrap();
      let adapter = sh(
          r#"read line; echo 'starting up'; echo '{"jsonrpc":"2.0","id":7,"result":{}}'; echo '{"jsonrpc":"2.0","id":0,"result":{"agentCapabilities":{"promptCapabilities":{"image":false}}}}'; exec sleep 60"#,
      );
      assert_eq!(
          initialize(&adapter, dir.path(), soon()).await,
          Started::Answered {
              version: None,
              images: false
          }
      );
      let bare = answering(r#"{"jsonrpc":"2.0","id":0,"result":{}}"#);
      assert_eq!(
          initialize(&bare, dir.path(), soon()).await,
          Started::Answered {
              version: None,
              images: false
          }
      );
  }

  /// What the adapter printed never reaches the reason.
  #[tokio::test]
  async fn an_error_answer_fails_without_quoting_it() {
      let dir = tempfile::tempdir().unwrap();
      let adapter = answering(r#"{"jsonrpc":"2.0","id":0,"error":{"code":-1,"message":"canary-4d"}}"#);
      let Started::Failed(why) = initialize(&adapter, dir.path(), soon()).await else {
          panic!("an error answer is a failure")
      };
      assert!(why.contains("answered `initialize` with an error"), "{why}");
      assert!(!why.contains("canary"), "{why}");
  }

  #[tokio::test]
  async fn an_adapter_that_exits_without_answering_fails() {
      let dir = tempfile::tempdir().unwrap();
      let adapter = sh("read line; echo 'canary-4d'; exit 3");
      let Started::Failed(why) = initialize(&adapter, dir.path(), soon()).await else {
          panic!("an exit is a failure")
      };
      assert!(why.contains("exited without answering"), "{why}");
      assert!(!why.contains("canary"), "{why}");
  }

  /// The review's A6: an adapter's output is read up to `MAX_OUTPUT` bytes
  /// (1 MiB): an answer after 2 MiB with no newline is never read.
  #[tokio::test]
  async fn an_answer_after_too_many_bytes_is_never_read() {
      let dir = tempfile::tempdir().unwrap();
      let adapter = sh(
          r#"read line; head -c 2097152 /dev/zero | tr '\0' a; echo; echo '{"jsonrpc":"2.0","id":0,"result":{}}'; exec sleep 60"#,
      );
      let Started::Failed(why) = initialize(&adapter, dir.path(), soon()).await else {
          panic!("an answer past the byte cap is a failure")
      };
      assert!(why.contains("wrote too much"), "{why}");
  }

  /// The review's A6: and up to `MAX_LINES` lines (1000): an answer after
  /// 1001 other lines is never read.
  #[tokio::test]
  async fn an_answer_after_too_many_lines_is_never_read() {
      let dir = tempfile::tempdir().unwrap();
      let adapter = sh(
          r#"read line; i=0; while [ $i -lt 1001 ]; do echo '{}'; i=$((i+1)); done; echo '{"jsonrpc":"2.0","id":0,"result":{}}'; exec sleep 60"#,
      );
      let Started::Failed(why) = initialize(&adapter, dir.path(), soon()).await else {
          panic!("an answer past the line cap is a failure")
      };
      assert!(why.contains("wrote too much"), "{why}");
  }

  #[tokio::test]
  async fn an_adapter_that_cannot_be_started_fails() {
      let dir = tempfile::tempdir().unwrap();
      let missing = AgentCommand {
          program: "/nonexistent/adapter".into(),
          args: Vec::new(),
          env: Vec::new(),
      };
      let Started::Failed(why) = initialize(&missing, dir.path(), soon()).await else {
          panic!("a missing program is a failure")
      };
      assert!(why.contains("cannot be started"), "{why}");
  }

  /// A silent adapter fails at the deadline, and its whole group (a
  /// grandchild included) is gone afterwards.
  #[tokio::test]
  async fn a_silent_adapter_fails_at_the_deadline_and_leaves_nothing_running() {
      let dir = tempfile::tempdir().unwrap();
      let grandchild = dir.path().join("grandchild");
      let adapter = sh(&format!(
          "sleep 600 & echo $! > {}; read line; exec sleep 600",
          grandchild.display()
      ));
      let began = Instant::now();
      let started = initialize(&adapter, dir.path(), Instant::now() + Duration::from_millis(500)).await;
      let Started::Failed(why) = started else {
          panic!("a silence is a failure")
      };
      assert!(why.contains("did not answer `initialize` in the probe's time"), "{why}");
      assert!(why.contains("macOS"), "{why}");
      assert!(began.elapsed() < Duration::from_secs(5), "{:?}", began.elapsed());
      wait_dead(read_pid(&grandchild).await).await;
  }

  /// An answered adapter is killed too: nothing a probe starts stays.
  #[tokio::test]
  async fn an_answered_adapter_is_killed_with_its_group() {
      let dir = tempfile::tempdir().unwrap();
      let grandchild = dir.path().join("grandchild");
      let adapter = sh(&format!(
          r#"sleep 600 & echo $! > {}; read line; echo '{{"jsonrpc":"2.0","id":0,"result":{{}}}}'; exec sleep 600"#,
          grandchild.display()
      ));
      assert!(matches!(
          initialize(&adapter, dir.path(), soon()).await,
          Started::Answered { .. }
      ));
      wait_dead(read_pid(&grandchild).await).await;
  }

  /// Hazard (d): the adapter runs as a session's does, in the directory it
  /// is given, with its own variables, and without what the host strips
  /// even when the agent's own configuration names it.
  #[tokio::test]
  async fn an_adapter_is_started_in_its_directory_with_the_hosts_stripped_environment() {
      let dir = tempfile::tempdir().unwrap();
      let seen = dir.path().join("seen");
      let mut adapter = sh(&format!(
          r#"{{ pwd; env; }} > {}; read line; echo '{{"jsonrpc":"2.0","id":0,"result":{{}}}}'; exec sleep 60"#,
          seen.display()
      ));
      adapter.env = vec![
          ("AGENT_OWN".into(), "kept".into()),
          ("HENNERY_DEV_TOKEN".into(), "secret".into()),
          ("CLAUDECODE".into(), "1".into()),
      ];
      let cwd = std::fs::canonicalize(dir.path()).unwrap();
      assert!(matches!(
          initialize(&adapter, &cwd, soon()).await,
          Started::Answered { .. }
      ));
      let text = std::fs::read_to_string(&seen).unwrap();
      assert_eq!(text.lines().next(), Some(cwd.to_str().unwrap()), "{text}");
      assert!(text.contains("AGENT_OWN=kept"), "{text}");
      assert!(!text.contains("HENNERY_DEV_TOKEN"), "{text}");
      assert!(!text.contains("CLAUDECODE"), "{text}");
  }

  // Check 4: a CLI's login status, by its exit status alone.

  #[tokio::test]
  async fn a_status_exit_of_zero_is_logged_in_and_any_other_exit_code_is_not() {
      let dir = tempfile::tempdir().unwrap();
      let ask = |script: &str| logged_in(LoginCheck::Ask(sh(script)), dir.path(), soon());
      assert_eq!(ask("echo 'you@example.com'; exit 0").await, (AgentAuth::Ok, None));
      assert_eq!(ask("echo 'canary-4d'; exit 1").await, (AgentAuth::Missing, None));
      assert_eq!(ask("exit 2").await, (AgentAuth::Missing, None));
  }

  /// The review's O1: a CLI ended by a signal said nothing (Gatekeeper or a
  /// keychain prompt can end a logged-in CLI on macOS): `unknown`, with why.
  #[tokio::test]
  async fn a_cli_ended_by_a_signal_is_unknown() {
      let dir = tempfile::tempdir().unwrap();
      let (auth, note) = logged_in(LoginCheck::Ask(sh("kill -9 $$")), dir.path(), soon()).await;
      assert_eq!(auth, AgentAuth::Unknown);
      assert!(note.unwrap().contains("ended by a signal"));
  }

  /// Hazard (d), the review's A4: a CLI runs as an adapter does, in the
  /// directory it is given, with its own variables, and without what the
  /// host strips even when its own command names it.
  #[tokio::test]
  async fn a_cli_runs_in_its_directory_with_the_hosts_stripped_environment() {
      let dir = tempfile::tempdir().unwrap();
      let seen = dir.path().join("seen");
      let mut cli = sh(&format!("{{ pwd; env; }} > {}; exit 0", seen.display()));
      cli.env = vec![
          ("AGENT_OWN".into(), "kept".into()),
          ("HENNERY_DEV_TOKEN".into(), "secret".into()),
          ("CLAUDECODE".into(), "1".into()),
      ];
      let cwd = std::fs::canonicalize(dir.path()).unwrap();
      assert_eq!(
          logged_in(LoginCheck::Ask(cli), &cwd, soon()).await,
          (AgentAuth::Ok, None)
      );
      let text = std::fs::read_to_string(&seen).unwrap();
      assert_eq!(text.lines().next(), Some(cwd.to_str().unwrap()), "{text}");
      assert!(text.contains("AGENT_OWN=kept"), "{text}");
      assert!(!text.contains("HENNERY_DEV_TOKEN"), "{text}");
      assert!(!text.contains("CLAUDECODE"), "{text}");
  }

  #[tokio::test]
  async fn a_cli_not_asked_is_unknown_with_its_note() {
      let dir = tempfile::tempdir().unwrap();
      assert_eq!(
          logged_in(LoginCheck::NotAsked(None), dir.path(), soon()).await,
          (AgentAuth::Unknown, None)
      );
      assert_eq!(
          logged_in(LoginCheck::NotAsked(Some("why".into())), dir.path(), soon()).await,
          (AgentAuth::Unknown, Some("why".into()))
      );
  }

  #[tokio::test]
  async fn a_silent_cli_is_unknown_at_the_deadline_and_leaves_nothing_running() {
      let dir = tempfile::tempdir().unwrap();
      let grandchild = dir.path().join("grandchild");
      let cli = sh(&format!(
          "sleep 600 & echo $! > {}; exec sleep 600",
          grandchild.display()
      ));
      let began = Instant::now();
      let (auth, note) = logged_in(
          LoginCheck::Ask(cli),
          dir.path(),
          Instant::now() + Duration::from_millis(500),
      )
      .await;
      assert_eq!(auth, AgentAuth::Unknown);
      let note = note.unwrap();
      assert!(note.contains("did not say whether it is logged in"), "{note}");
      assert!(began.elapsed() < Duration::from_secs(5), "{:?}", began.elapsed());
      wait_dead(read_pid(&grandchild).await).await;
  }

  #[tokio::test]
  async fn a_cli_that_cannot_be_started_is_unknown() {
      let dir = tempfile::tempdir().unwrap();
      let missing = AgentCommand {
          program: "/nonexistent/claude".into(),
          args: vec!["auth".into(), "status".into()],
          env: Vec::new(),
      };
      let (auth, note) = logged_in(LoginCheck::Ask(missing), dir.path(), soon()).await;
      assert_eq!(auth, AgentAuth::Unknown);
      assert!(note.unwrap().contains("cannot be started"));
  }

  /// Hazard (c): a CLI's output goes nowhere. One that writes far more than a
  /// pipe holds still ends, and ends as it chose: it was never blocked, and
  /// never killed by a closed pipe.
  #[tokio::test]
  async fn a_cli_writes_to_nowhere_and_is_never_blocked_by_its_output() {
      let dir = tempfile::tempdir().unwrap();
      let loud = sh("head -c 1048576 /dev/zero; head -c 1048576 /dev/zero >&2; exit 0");
      let ended = exit_status(&loud, dir.path(), Duration::from_secs(10)).await.unwrap();
      assert_eq!(ended.map(|e| e.code), Some(Some(0)));
      // Its standard input is empty: a CLI that waits on it ends at once.
      let reads = sh("cat; exit 4");
      let ended = exit_status(&reads, dir.path(), Duration::from_secs(10)).await.unwrap();
      assert_eq!(ended.map(|e| e.code), Some(Some(4)));
  }

  /// Hazard (b): a CLI runs in a group of its own that it does not lead: the
  /// host's guard leads it (`Adapter::spawn`'s group).
  #[tokio::test]
  async fn a_cli_runs_in_a_guarded_group_of_its_own() {
      let dir = tempfile::tempdir().unwrap();
      let ids = dir.path().join("ids");
      let cli = sh(&format!("echo $$ $(ps -o pgid= -p $$) > {}; exit 0", ids.display()));
      exit_status(&cli, dir.path(), Duration::from_secs(10)).await.unwrap();
      let text = std::fs::read_to_string(&ids).unwrap();
      let mut parts = text.split_whitespace().map(|p| p.parse::<i32>().unwrap());
      let (pid, pgid) = (parts.next().unwrap(), parts.next().unwrap());
      assert_ne!(pid, pgid, "the CLI leads its own group: no guard");
      // SAFETY: getpgrp(2) has no failure mode.
      assert_ne!(pgid, unsafe { libc::getpgrp() }, "the CLI is in the test's group");
  }

  // The probe: every agent at once, within one budget.

  #[derive(Debug)]
  struct Logins(HashMap<String, LoginCheck>);

  impl AgentChecks for Logins {
      fn login(&self, agent: &str) -> LoginCheck {
          self.0.get(agent).cloned().unwrap_or(LoginCheck::NotAsked(None))
      }
  }

  #[tokio::test]
  async fn a_probe_reports_each_agent_live() {
      let dir = tempfile::tempdir().unwrap();
      let mut agents = HashMap::new();
      agents.insert(
          "claude".to_string(),
          answering(
              r#"{"jsonrpc":"2.0","id":0,"result":{"agentInfo":{"version":"2.0.0"},"agentCapabilities":{"promptCapabilities":{"image":true}}}}"#,
          ),
      );
      agents.insert(
          "codex".to_string(),
          answering(r#"{"jsonrpc":"2.0","id":0,"result":{"agentInfo":{"version":"see https://x"}}}"#),
      );
      agents.insert("broken".to_string(), sh("read line; exit 1"));
      let report = vec![
          AgentInfo {
              adapter_version: Some("9.9.9".into()),
              note: Some("a static note".into()),
              ..agent("broken", AgentCli::Bundled)
          },
          AgentInfo {
              adapter_version: Some("1.0.0".into()),
              ..agent("claude", AgentCli::Bundled)
          },
          AgentInfo {
              adapter_version: Some("1.0.0".into()),
              ..agent("codex", AgentCli::Override)
          },
          // Not launchable as configured: nothing is started for it.
          AgentInfo {
              available: false,
              note: Some("left out".into()),
              ..agent("left-out", AgentCli::Bundled)
          },
      ];
      let logins = Logins(HashMap::from([
          ("claude".to_string(), LoginCheck::Ask(sh("exit 0"))),
          ("codex".to_string(), LoginCheck::Ask(sh("exit 1"))),
          ("broken".to_string(), LoginCheck::NotAsked(Some("not asked".into()))),
      ]));
      let live = availability::probe(&report, &agents, Arc::new(logins), dir.path(), Duration::from_secs(10)).await;
      let by_name: HashMap<&str, &AgentInfo> = live.iter().map(|a| (a.agent.as_str(), a)).collect();
      assert_eq!(live.len(), 4);
      let claude = by_name["claude"];
      assert!(claude.available);
      assert_eq!(claude.auth, AgentAuth::Ok);
      assert_eq!(claude.adapter_version.as_deref(), Some("2.0.0"));
      assert_eq!(claude.images, Some(true));
      assert_eq!(claude.note, None);
      // An unreadable version: the set's stands.
      let codex = by_name["codex"];
      assert!(codex.available);
      assert_eq!((codex.auth, codex.cli), (AgentAuth::Missing, AgentCli::Override));
      assert_eq!(codex.adapter_version.as_deref(), Some("1.0.0"));
      assert_eq!(codex.images, Some(false));
      // Both notes, the static one replaced.
      let broken = by_name["broken"];
      assert!(!broken.available);
      assert_eq!(broken.images, None);
      let note = broken.note.as_deref().unwrap();
      assert!(
          note.contains("exited without answering") && note.ends_with("; not asked"),
          "{note}"
      );
      assert_eq!(*by_name["left-out"], report[3]);
  }

  /// Every check runs at once: two silent agents, each with a silent CLI,
  /// cost one budget, not four.
  #[tokio::test]
  async fn a_probe_keeps_to_one_budget_however_many_checks_hang() {
      let dir = tempfile::tempdir().unwrap();
      let silent = sh("read line; exec sleep 600");
      let agents = HashMap::from([("a".to_string(), silent.clone()), ("b".to_string(), silent)]);
      let report = vec![agent("a", AgentCli::Given), agent("b", AgentCli::Given)];
      let logins = Logins(HashMap::from([
          ("a".to_string(), LoginCheck::Ask(sh("exec sleep 600"))),
          ("b".to_string(), LoginCheck::Ask(sh("exec sleep 600"))),
      ]));
      let budget = Duration::from_secs(1);
      let began = Instant::now();
      let live = availability::probe(&report, &agents, Arc::new(logins), dir.path(), budget).await;
      let took = began.elapsed();
      assert!(took >= budget && took < 3 * budget, "{took:?}");
      for agent in &live {
          assert!(!agent.available);
          assert_eq!(agent.auth, AgentAuth::Unknown);
      }
  }

  /// A host given no checks asks no CLI.
  #[tokio::test]
  async fn without_checks_no_cli_is_asked() {
      let dir = tempfile::tempdir().unwrap();
      let agents = HashMap::from([("fake".to_string(), answering(r#"{"jsonrpc":"2.0","id":0,"result":{}}"#))]);
      let live = availability::probe(
          &[agent("fake", AgentCli::Given)],
          &agents,
          Arc::new(NoChecks),
          dir.path(),
          Duration::from_secs(10),
      )
      .await;
      assert_eq!((live[0].available, live[0].auth), (true, AgentAuth::Unknown));
  }

  // The static view.

  #[test]
  fn the_static_view_adds_every_given_agent_and_keeps_the_runtimes() {
      let agents = HashMap::from([("fake".to_string(), sh("true")), ("claude".to_string(), sh("true"))]);
      let infos = vec![
          AgentInfo {
              adapter_version: Some("1.0.0".into()),
              ..agent("claude", AgentCli::Bundled)
          },
          AgentInfo {
              available: false,
              note: Some("unavailable".into()),
              ..agent("codex", AgentCli::Bundled)
          },
      ];
      let view = static_agents(&agents, &infos);
      assert_eq!(
          view,
          [infos[0].clone(), infos[1].clone(), agent("fake", AgentCli::Given)]
      );
      // Bounded like any report.
      let many: HashMap<String, AgentCommand> = (0..MAX_AGENTS + 3).map(|i| (format!("a{i:02}"), sh("true"))).collect();
      assert_eq!(static_agents(&many, &[]).len(), MAX_AGENTS);
  }

  #[test]
  fn one_probe_at_a_time_until_the_first_ends() {
      let one = OneProbe::default();
      let first = one.try_start().expect("free");
      assert!(one.try_start().is_none(), "a second probe while the first runs");
      // Shared by every clone: the host keeps one across its connections.
      assert!(one.clone().try_start().is_none());
      drop(first);
      assert!(one.try_start().is_some(), "free again once the first ended");
  }
  ```

In `crates/hennery-host/tests/runtime.rs`, replace:

  ```rust
      assert!(ran.exists(), "{lines}");
  }
  ```

with:

  ```rust
      assert!(ran.exists(), "{lines}");
  }

  // Plan 4d-B1-i: what a host start reports of its agents and its runtime.

  fn info_of<'a>(prepared: &'a agents::Prepared, name: &str) -> &'a hennery_proto::agents::AgentInfo {
      prepared
          .agents
          .infos
          .iter()
          .find(|a| a.agent == name)
          .unwrap_or_else(|| panic!("no {name} in {:?}", prepared.agents.infos))
  }

  /// The pinned set, current: each agent bundled, launchable, with the
  /// version the set records; the runtime pinned and not held.
  #[tokio::test]
  async fn a_start_reports_each_agent_with_its_cli_and_version_and_the_pinned_set() {
      use hennery_proto::agents::{AgentAuth, AgentCli, RuntimeInfo, RuntimeSource};
      let server = Server::start().await;
      let fixture = Fixture::new("1.0.0");
      fixture.serve(&server);
      let (_dir, layout) = data_dir();
      let selection = selection(&fixture, &[]);
      let prepared = agents::prepare(&host_dir(&layout), Some(&selection), Some(&server.sources()), &quiet).await;
      let names: Vec<&str> = prepared.agents.infos.iter().map(|a| a.agent.as_str()).collect();
      assert_eq!(names, ["claude", "codex"]);
      for name in ["claude", "codex"] {
          let info = info_of(&prepared, name);
          assert!(info.available, "{info:?}");
          assert_eq!((info.cli, info.auth), (AgentCli::Bundled, AgentAuth::Unknown));
          assert_eq!(info.adapter_version.as_deref(), Some("1.0.0"));
          assert_eq!((info.images, info.note.as_deref()), (None, None));
      }
      assert_eq!(
          prepared.runtime,
          Some(RuntimeInfo {
              source: RuntimeSource::Managed,
              set_id: Some(selection.set_id()),
              pinned: Some(true),
              held: Some(false),
          })
      );
  }

  /// An older set runs while offline: not the pinned one. Rolled back to it:
  /// held too.
  #[tokio::test]
  async fn a_start_on_another_set_says_it_is_not_pinned_and_a_rollback_says_it_holds() {
      let server = Server::start().await;
      let (one, two) = (Fixture::new("1.0.0"), Fixture::new("2.0.0"));
      one.serve(&server);
      let (_dir, layout) = data_dir();
      install::install(&layout, &selection(&one, &[]), &server.sources(), &quiet)
          .await
          .unwrap();
      let prepared = agents::prepare(
          &host_dir(&layout),
          Some(&selection(&two, &[])),
          Some(&offline()),
          &quiet,
      )
      .await;
      let runtime = prepared.runtime.unwrap();
      assert_eq!(runtime.set_id, Some(selection(&one, &[]).set_id()));
      assert_eq!((runtime.pinned, runtime.held), (Some(false), Some(false)));
      two.serve(&server);
      install::install(&layout, &selection(&two, &[]), &server.sources(), &quiet)
          .await
          .unwrap();
      install::rollback(&layout, &quiet).await.unwrap();
      let prepared = agents::prepare(
          &host_dir(&layout),
          Some(&selection(&two, &[])),
          Some(&server.sources()),
          &quiet,
      )
      .await;
      let runtime = prepared.runtime.unwrap();
      assert_eq!((runtime.pinned, runtime.held), (Some(false), Some(true)));
      // With no pin on this platform, whether the set is pinned is not known.
      let prepared = agents::prepare(&host_dir(&layout), None, None, &quiet).await;
      assert_eq!(prepared.runtime.unwrap().pinned, None);
  }

  /// Every agent of the set is reported, those left out too, with why.
  #[tokio::test]
  async fn an_agent_left_out_is_reported_unavailable_with_its_note() {
      use hennery_proto::agents::AgentCli;
      let server = Server::start().await;
      let fixture = Fixture::new("1.0.0");
      fixture.serve(&server);
      let (_dir, layout) = data_dir();
      let dir = host_dir(&layout);
      std::fs::create_dir_all(&dir).unwrap();
      std::fs::write(dir.join("host.toml"), "[cli]\nclaude = \"/bin/sh\"\n").unwrap();
      let selection = selection(&fixture, &["claude"]);
      // A valid override: launchable, and its CLI is the operator's.
      let prepared = agents::prepare(&dir, Some(&selection), Some(&server.sources()), &quiet).await;
      let claude = info_of(&prepared, "claude");
      assert!(claude.available);
      assert_eq!(claude.cli, AgentCli::Override);
      assert_eq!(claude.note, None, "the set skipped the bundled CLI: nothing to say");
      assert_eq!(info_of(&prepared, "codex").cli, AgentCli::Bundled);
      // The override gone: no CLI at all.
      std::fs::write(dir.join("host.toml"), "").unwrap();
      let prepared = agents::prepare(&dir, Some(&selection), Some(&server.sources()), &quiet).await;
      let claude = info_of(&prepared, "claude");
      assert!(!claude.available);
      assert_eq!(claude.cli, AgentCli::Bundled);
      assert!(
          claude.note.as_deref().unwrap().contains("claude is unavailable"),
          "{claude:?}"
      );
      assert_eq!(claude.adapter_version.as_deref(), Some("1.0.0"));
      // An override that does not run: unavailable, and it says which.
      std::fs::write(dir.join("host.toml"), "[cli]\nclaude = \"/nonexistent/claude\"\n").unwrap();
      let prepared = agents::prepare(&dir, Some(&selection), Some(&server.sources()), &quiet).await;
      let claude = info_of(&prepared, "claude");
      assert!(!claude.available);
      assert_eq!(claude.cli, AgentCli::Override);
      assert!(
          claude.note.as_deref().unwrap().contains("/nonexistent/claude"),
          "{claude:?}"
      );
      assert!(prepared.agents.infos.iter().filter(|a| a.available).count() == 1);
  }

  /// An override beside the set's bundled CLI is launchable, and its note
  /// says so.
  #[tokio::test]
  async fn an_override_beside_a_bundled_cli_is_reported_with_its_note() {
      use hennery_proto::agents::AgentCli;
      let server = Server::start().await;
      let fixture = Fixture::new("1.0.0");
      fixture.serve(&server);
      let (_dir, layout) = data_dir();
      let dir = host_dir(&layout);
      install::install(&layout, &selection(&fixture, &[]), &server.sources(), &quiet)
          .await
          .unwrap();
      std::fs::write(dir.join("host.toml"), "[cli]\nclaude = \"/bin/sh\"\n").unwrap();
      let prepared = agents::prepare(&dir, Some(&selection(&fixture, &[])), None, &quiet).await;
      let claude = info_of(&prepared, "claude");
      assert!(claude.available);
      assert_eq!(claude.cli, AgentCli::Override);
      assert!(
          claude
              .note
              .as_deref()
              .unwrap()
              .contains("rather than the set's bundled CLI"),
          "{claude:?}"
      );
      assert_eq!(info_of(&prepared, "codex").note, None);
  }

  /// No set at all: a managed runtime with nothing in it.
  #[tokio::test]
  async fn with_no_set_the_runtime_is_managed_and_empty() {
      use hennery_proto::agents::{RuntimeInfo, RuntimeSource};
      let fixture = Fixture::new("1.0.0");
      let (_dir, layout) = data_dir();
      let prepared = agents::prepare(
          &host_dir(&layout),
          Some(&selection(&fixture, &[])),
          Some(&offline()),
          &quiet,
      )
      .await;
      assert!(prepared.agents.infos.is_empty());
      assert_eq!(
          prepared.runtime,
          Some(RuntimeInfo {
              source: RuntimeSource::Managed,
              set_id: None,
              pinned: None,
              held: Some(false),
          })
      );
  }

  /// A set whose Node is gone: each of its agents, none launchable.
  #[tokio::test]
  async fn a_set_without_its_node_reports_every_agent_unavailable() {
      let server = Server::start().await;
      let fixture = Fixture::new("1.0.0");
      fixture.serve(&server);
      let (_dir, layout) = data_dir();
      let selection = selection(&fixture, &[]);
      let set = install::install(&layout, &selection, &server.sources(), &quiet)
          .await
          .unwrap()
          .set()
          .clone();
      std::fs::remove_file(&set.node).unwrap();
      let prepared = agents::prepare(&host_dir(&layout), Some(&selection), Some(&offline()), &quiet).await;
      assert_eq!(prepared.agents.infos.len(), 2);
      for info in &prepared.agents.infos {
          assert!(!info.available, "{info:?}");
          assert!(info.note.as_deref().unwrap().contains("is missing"), "{info:?}");
      }
      assert_eq!(prepared.runtime.unwrap().set_id, Some(set.id));
  }
  ```

- [ ] **Step 2: Run them, and see them fail**

Run: `nix develop -c cargo test -p hennery-host --test availability --test adapter --test runtime --locked`
Expected: FAIL to compile: `hennery_host::availability` and `adapter::exit_status` do not exist yet (the host's `availability`, `adapter` and `runtime` tests).

- [ ] **Step 3: Write the implementation**

In `crates/hennery-host/src/adapter.rs`, replace:

  ```rust
          let mut command = tokio::process::Command::new(&agent.program);
          // Before `envs`: inherited, these are dropped; set by the agent's
          // own configuration, they pass.
          for var in INHERITED_OVERRIDE_VARS {
              command.env_remove(var);
          }
          command
              .args(&agent.args)
              .envs(agent.env.iter().cloned())
              .current_dir(cwd)
              .stdin(Stdio::piped())
              .stdout(Stdio::piped())
              .stderr(Stdio::piped())
              .process_group(pgid)
              .kill_on_drop(true);
          // After `envs`: a secret is stripped even if the agent's own
          // configuration names it.
          for var in NESTING_VARS
              .iter()
              .chain(HOST_SECRET_VARS)
              .chain(HOST_LOG_VARS)
              .chain(strip)
          {
              command.env_remove(var);
          }
          // SAFETY: the closure runs in the forked child before `exec` and
          // calls only `syscall(close_range)` (Linux), `fcntl` and `close`,
          // which are async-signal-safe; it allocates nothing.
          unsafe {
              command.pre_exec(move || {
                  close_inherited(limit);
                  Ok(())
              });
          }
  ```

with:

  ```rust
          let mut command = guarded_command(agent, cwd, strip, limit, pgid);
          command
              .stdin(Stdio::piped())
              .stdout(Stdio::piped())
              .stderr(Stdio::piped());
  ```

In `crates/hennery-host/src/adapter.rs`, replace:

  ```rust

  impl Drop for Adapter {
  ```

with:

  ```rust

  /// `agent` as `Adapter::spawn` starts it, in the group `pgid` that its
  /// guard leads: the inherited override variables dropped before the
  /// agent's own variables, the host's secrets, log choice, nesting
  /// variables and `strip` after them, every inherited descriptor closed.
  fn guarded_command(
      agent: &AgentCommand,
      cwd: &Path,
      strip: &[&str],
      limit: libc::c_int,
      pgid: i32,
  ) -> tokio::process::Command {
      let mut command = tokio::process::Command::new(&agent.program);
      // Before `envs`: inherited, these are dropped; set by the agent's
      // own configuration, they pass.
      for var in INHERITED_OVERRIDE_VARS {
          command.env_remove(var);
      }
      command
          .args(&agent.args)
          .envs(agent.env.iter().cloned())
          .current_dir(cwd)
          .process_group(pgid)
          .kill_on_drop(true);
      // After `envs`: a secret is stripped even if the agent's own
      // configuration names it.
      for var in NESTING_VARS
          .iter()
          .chain(HOST_SECRET_VARS)
          .chain(HOST_LOG_VARS)
          .chain(strip)
      {
          command.env_remove(var);
      }
      // SAFETY: the closure runs in the forked child before `exec` and
      // calls only `syscall(close_range)` (Linux), `fcntl` and `close`,
      // which are async-signal-safe; it allocates nothing.
      unsafe {
          command.pre_exec(move || {
              close_inherited(limit);
              Ok(())
          });
      }
      command
  }

  /// Run `command` to its end, or for at most `limit`, started as
  /// `Adapter::spawn` starts an adapter (a guarded group of its own, the same
  /// environment), with its standard input, output and error on `/dev/null`:
  /// only how it ended is kept, never a byte it wrote (plan 4d-B1-i, a CLI's
  /// login status). `Ok(None)`: it ran out of time. Its whole group is
  /// SIGKILLed afterwards, however it ended, and also if this future is
  /// dropped before it ends (the review's A1).
  pub async fn exit_status(command: &AgentCommand, cwd: &Path, limit: Duration) -> std::io::Result<Option<ExitInfo>> {
      let fds = fd_limit();
      let mut guard = spawn_guard(fds)?;
      let pgid = guard.id().expect("a just-spawned child has a pid") as i32;
      let mut cmd = guarded_command(command, cwd, &[], fds, pgid);
      cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
      let spawned = cmd.spawn();
      if spawned.is_err() {
          let _ = guard.start_kill();
      }
      tokio::spawn(async move {
          let _ = guard.wait().await;
      });
      let mut child = spawned?;
      // Only now: on a failed spawn the guard is killed above and may be
      // reaped, after which its id could be another process's.
      let mut group = GroupKill { pgid, done: false };
      let ended = tokio::time::timeout(limit, child.wait()).await;
      group.kill();
      match ended {
          Ok(status) => {
              let status = status?;
              Ok(Some(ExitInfo {
                  code: status.code(),
                  signal: status.signal(),
              }))
          }
          Err(_) => {
              let _ = child.wait().await;
              Ok(None)
          }
      }
  }

  /// SIGKILLs the group `exit_status` started, once: when it ends, or when
  /// its future is dropped first. The guard leads the group until the
  /// group's first SIGKILL, which is this one: its id cannot have been
  /// recycled.
  struct GroupKill {
      pgid: i32,
      done: bool,
  }

  impl GroupKill {
      fn kill(&mut self) {
          if !self.done {
              signal_group(self.pgid, libc::SIGKILL);
              self.done = true;
          }
      }
  }

  impl Drop for GroupKill {
      fn drop(&mut self) {
          self.kill();
      }
  }

  impl Drop for Adapter {
  ```

Create `crates/hennery-host/src/availability.rs`:

  ```rust
  //! The host's agents, as it reports them (plan 4d-B1-i; ACP core §6
  //! "Agent availability"): the static view in `hello`, and the live one,
  //! `probe_agents`.
  //!
  //! A probe runs a fixed set of read-only checks on the agents this host is
  //! configured with, and nothing the collector names: each adapter started
  //! as a session's is (`Adapter::spawn`: its own guarded group, the host's
  //! stripped environment) and sent `initialize` alone (doctor's check 3),
  //! then killed; and each agent's CLI asked whether it is logged in
  //! (check 4), of which only the exit status is kept. Which CLI and which
  //! question is doctor's knowledge, injected through `AgentChecks`: the
  //! binary implements it, this crate never depends on the binary. Every
  //! check of every agent runs at once, within one budget.

  use crate::adapter::{self, Adapter, AgentCommand};
  use hennery_proto::agents::{
      AgentAuth, AgentCli, AgentInfo, RuntimeInfo, RuntimeSource, bound_agents, is_readable_version,
  };
  use std::collections::HashMap;
  use std::path::{Path, PathBuf};
  use std::sync::Arc;
  use std::sync::atomic::{AtomicBool, Ordering};
  use std::time::Duration;
  use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
  use tokio::time::Instant;

  /// How long one probe takes at most, all its checks together: below the
  /// collector's 20 s wait, so an answer can still reach it.
  pub const PROBE_BUDGET: Duration = Duration::from_secs(15);

  /// What the probe sends each adapter: the protocol version and no
  /// capability, as doctor's check 3 does. No session, no prompt, no login.
  const INITIALIZE: &str =
      r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}"#;

  /// The most of an adapter's output read waiting for its answer.
  const MAX_OUTPUT: u64 = 1 << 20;
  /// The most lines read waiting for it.
  const MAX_LINES: usize = 1000;

  /// Said of an adapter that reached `MAX_OUTPUT` or `MAX_LINES` first.
  const TOO_MUCH: &str = "its adapter wrote too much before answering `initialize`";

  /// Said of a check that ran out of the probe's budget.
  const OUT_OF_TIME: &str = "(on macOS a program's first run can wait on an online check: refresh again)";

  /// How to ask an agent's CLI whether it is logged in (doctor's check 4).
  #[derive(Debug, Clone)]
  pub enum LoginCheck {
      /// Run this fixed command; exit 0 means logged in. Its output is never
      /// read.
      Ask(AgentCommand),
      /// Not asked: `auth` stays `unknown`, with this note, if any.
      NotAsked(Option<String>),
  }

  /// Doctor's knowledge of the agents, which a probe uses (distribution
  /// spec §7): implemented by the binary and given to the host in
  /// `HostConfig::checks`. Later checks come as methods with default bodies.
  pub trait AgentChecks: Send + Sync + std::fmt::Debug + 'static {
      /// How to ask `agent`'s CLI whether it is logged in. By default no CLI
      /// is known.
      fn login(&self, agent: &str) -> LoginCheck {
          let _ = agent;
          LoginCheck::NotAsked(None)
      }
  }

  /// No knowledge: every `auth` stays `unknown` (tests, and a host given no
  /// checks).
  #[derive(Debug, Default, Clone, Copy)]
  pub struct NoChecks;

  impl AgentChecks for NoChecks {}

  /// The runtime of a host given `--agent` commands: nothing managed.
  pub fn given_runtime() -> RuntimeInfo {
      RuntimeInfo {
          source: RuntimeSource::Given,
          set_id: None,
          pinned: None,
          held: None,
      }
  }

  /// The static view (`hello`): the managed runtime's `infos` as they are,
  /// and every other configured agent as given by `--agent`, launchable as
  /// configured. Sorted by name and bounded.
  pub fn static_agents(agents: &HashMap<String, AgentCommand>, infos: &[AgentInfo]) -> Vec<AgentInfo> {
      let mut out: Vec<AgentInfo> = infos.to_vec();
      for name in agents.keys() {
          if !out.iter().any(|a| &a.agent == name) {
              out.push(AgentInfo {
                  agent: name.clone(),
                  available: true,
                  auth: AgentAuth::Unknown,
                  cli: AgentCli::Given,
                  adapter_version: None,
                  images: None,
                  note: None,
              });
          }
      }
      out.sort_by(|a, b| a.agent.cmp(&b.agent));
      bound_agents(out)
  }

  /// What an adapter's `initialize` came to.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum Started {
      /// It answered: its `agentInfo.version` and `promptCapabilities.image`.
      Answered { version: Option<String>, images: bool },
      /// It did not: why, in words that quote nothing it printed.
      Failed(String),
  }

  /// Start `agent` as a session's adapter is started, in `cwd`, send
  /// `initialize` and wait for its answer until `deadline` (check 3). Its
  /// whole group is killed as this returns: `Adapter`'s drop does it.
  pub async fn initialize(agent: &AgentCommand, cwd: &Path, deadline: Instant) -> Started {
      let (_adapter, io) = match Adapter::spawn(agent, cwd) {
          Ok(spawned) => spawned,
          Err(err) => return Started::Failed(format!("its adapter cannot be started: {}", err.kind())),
      };
      let mut stdin = io.stdin;
      let answer = async {
          if stdin.write_all(format!("{INITIALIZE}\n").as_bytes()).await.is_err() || stdin.flush().await.is_err() {
              return Started::Failed("its adapter exited before it read `initialize`".into());
          }
          let mut lines = tokio::io::BufReader::new(io.stdout.take(MAX_OUTPUT)).lines();
          for _ in 0..MAX_LINES {
              let Ok(Some(line)) = lines.next_line().await else {
                  // The byte cap ends the output as an end of file would.
                  if lines.get_ref().get_ref().limit() == 0 {
                      return Started::Failed(TOO_MUCH.into());
                  }
                  return Started::Failed("its adapter exited without answering `initialize`".into());
              };
              let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
                  continue;
              };
              if message.get("id") != Some(&serde_json::json!(0)) {
                  continue;
              }
              return match message.get("result") {
                  Some(result) => Started::Answered {
                      version: result
                          .pointer("/agentInfo/version")
                          .and_then(|v| v.as_str())
                          .map(str::to_string),
                      images: result
                          .pointer("/agentCapabilities/promptCapabilities/image")
                          .and_then(|v| v.as_bool())
                          .unwrap_or(false),
                  },
                  None => Started::Failed("its adapter answered `initialize` with an error".into()),
              };
          }
          Started::Failed(TOO_MUCH.into())
      };
      match tokio::time::timeout_at(deadline, answer).await {
          Ok(started) => started,
          Err(_) => Started::Failed(format!(
              "its adapter did not answer `initialize` in the probe's time {OUT_OF_TIME}"
          )),
      }
  }

  /// What check 4 came to: `auth`, and a note when it says why it is
  /// `unknown`.
  pub async fn logged_in(check: LoginCheck, cwd: &Path, deadline: Instant) -> (AgentAuth, Option<String>) {
      let command = match check {
          LoginCheck::Ask(command) => command,
          LoginCheck::NotAsked(note) => return (AgentAuth::Unknown, note),
      };
      let left = deadline.saturating_duration_since(Instant::now());
      match adapter::exit_status(&command, cwd, left).await {
          Ok(Some(ended)) if ended.code == Some(0) => (AgentAuth::Ok, None),
          // Ended by a signal, it said nothing (the review's O1).
          Ok(Some(ended)) if ended.code.is_none() => (
              AgentAuth::Unknown,
              Some("its CLI was ended by a signal before it said whether it is logged in".into()),
          ),
          Ok(Some(_)) => (AgentAuth::Missing, None),
          Ok(None) => (
              AgentAuth::Unknown,
              Some(format!(
                  "its CLI did not say whether it is logged in in the probe's time (a keychain prompt may be waiting on the Mac's screen) {OUT_OF_TIME}"
              )),
          ),
          Err(err) => (
              AgentAuth::Unknown,
              Some(format!("its CLI cannot be started: {}", err.kind())),
          ),
      }
  }

  /// One agent, live: launchable only if its adapter answered; `auth` from
  /// its CLI; the version it answered with, else the one the set records.
  async fn probe_one(
      info: AgentInfo,
      command: Option<AgentCommand>,
      checks: Arc<dyn AgentChecks>,
      cwd: PathBuf,
      deadline: Instant,
  ) -> AgentInfo {
      // Not launchable as configured: nothing to start.
      let Some(command) = command else { return info };
      let login = checks.login(&info.agent);
      let (started, (auth, login_note)) =
          tokio::join!(initialize(&command, &cwd, deadline), logged_in(login, &cwd, deadline));
      let (available, version, images, start_note) = match started {
          Started::Answered { version, images } => (true, version.filter(|v| is_readable_version(v)), Some(images), None),
          Started::Failed(why) => (false, None, None, Some(why)),
      };
      let note = match (start_note, login_note) {
          (Some(a), Some(b)) => Some(format!("{a}; {b}")),
          (a, b) => a.or(b),
      };
      AgentInfo {
          available,
          auth,
          adapter_version: version.or(info.adapter_version),
          images,
          note,
          ..info
      }
  }

  /// The live view: every agent of `report` probed at once, within
  /// `budget` (`PROBE_BUDGET` but in tests), in `cwd`. Bounded like the
  /// static view.
  pub async fn probe(
      report: &[AgentInfo],
      agents: &HashMap<String, AgentCommand>,
      checks: Arc<dyn AgentChecks>,
      cwd: &Path,
      budget: Duration,
  ) -> Vec<AgentInfo> {
      let deadline = Instant::now() + budget;
      let each = report.iter().map(|info| {
          probe_one(
              info.clone(),
              agents.get(&info.agent).cloned(),
              checks.clone(),
              cwd.to_path_buf(),
              deadline,
          )
      });
      bound_agents(futures::future::join_all(each).await)
  }

  /// At most one probe at a time on a host, across its connections (hazard
  /// (e)): a probe still running when its connection dropped holds it.
  #[derive(Debug, Default, Clone)]
  pub struct OneProbe(Arc<AtomicBool>);

  /// Held while a probe runs.
  pub struct ProbeSlot(Arc<AtomicBool>);

  impl OneProbe {
      /// The slot, unless a probe holds it.
      pub fn try_start(&self) -> Option<ProbeSlot> {
          self.0
              .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
              .ok()
              .map(|_| ProbeSlot(self.0.clone()))
      }
  }

  impl Drop for ProbeSlot {
      fn drop(&mut self) {
          self.0.store(false, Ordering::Release);
      }
  }
  ```

In `crates/hennery-host/src/lib.rs`, replace:

  ```rust
  pub mod agent_home;
  pub mod connection;
  ```

with:

  ```rust
  pub mod agent_home;
  pub mod availability;
  pub mod connection;
  ```

In `crates/hennery-host/src/runtime/agents.rs`, replace:

  ```rust
  use anyhow::{Context, Result, bail};
  use std::collections::{BTreeMap, BTreeSet, HashMap};
  ```

with:

  ```rust
  use anyhow::{Context, Result, bail};
  use hennery_proto::agents::{AgentAuth, AgentCli, AgentInfo, RuntimeInfo, RuntimeSource};
  use std::collections::{BTreeMap, BTreeSet, HashMap};
  ```

In `crates/hennery-host/src/runtime/agents.rs`, replace:

  ```rust
      pub notes: Vec<String>,
  }
  ```

with:

  ```rust
      pub notes: Vec<String>,
      /// Every agent of the set, those left out too, as `hello` reports them
      /// (plan 4d-B1-i): its CLI, the version the set records, and the note
      /// that concerns it. In the set's order.
      pub infos: Vec<AgentInfo>,
  }

  /// An agent of a set, as `hello` reports it before any probe.
  fn info(name: &str, version: &str, cli: AgentCli, available: bool, note: Option<&String>) -> AgentInfo {
      AgentInfo {
          agent: name.to_string(),
          available,
          auth: AgentAuth::Unknown,
          cli,
          adapter_version: Some(version.to_string()),
          images: None,
          note: note.cloned(),
      }
  }
  ```

In `crates/hennery-host/src/runtime/agents.rs`, replace:

  ```rust
          let mut own_cli = false;
          match (overrides.get(name), cli_var(name)) {
  ```

with:

  ```rust
          let version = &adapter.version;
          let mut own_cli = false;
          let mut cli = AgentCli::Bundled;
          // This agent's note, if any: in `notes` and in its `infos` entry.
          let note = match (overrides.get(name), cli_var(name)) {
  ```

In `crates/hennery-host/src/runtime/agents.rs`, replace:

  ```rust
                      if !adapter.cli_skipped {
                          out.notes.push(format!(
                              "{name}: using {} rather than the set's bundled CLI",
                              path.display()
                          ));
                      }
                  }
                  Err(err) => {
                      out.notes
                          .push(format!("{name} is unavailable: its --use-cli CLI: {err:#}"));
  ```

with:

  ```rust
                      cli = AgentCli::Override;
                      (!adapter.cli_skipped)
                          .then(|| format!("{name}: using {} rather than the set's bundled CLI", path.display()))
                  }
                  Err(err) => {
                      let note = format!("{name} is unavailable: its --use-cli CLI: {err:#}");
                      out.infos
                          .push(info(name, version, AgentCli::Override, false, Some(&note)));
                      out.notes.push(note);
  ```

In `crates/hennery-host/src/runtime/agents.rs`, replace:

  ```rust
                  out.notes
                      .push(format!("{name}: no CLI override is possible; host.toml's is ignored"));
                  if adapter.cli_skipped {
                      continue;
                  }
              }
              (None, _) if adapter.cli_skipped => {
                  out.notes.push(format!(
                      "{name} is unavailable: its set has no bundled CLI and host.toml names none; \
                       run `hennery host adapters update`"
                  ));
  ```

with:

  ```rust
                  let note = format!("{name}: no CLI override is possible; host.toml's is ignored");
                  if adapter.cli_skipped {
                      out.infos.push(info(name, version, cli, false, Some(&note)));
                      out.notes.push(note);
                      continue;
                  }
                  Some(note)
              }
              (None, _) if adapter.cli_skipped => {
                  let note = format!(
                      "{name} is unavailable: its set has no bundled CLI and host.toml names none; \
                       run `hennery host adapters update`"
                  );
                  out.infos.push(info(name, version, cli, false, Some(&note)));
                  out.notes.push(note);
  ```

In `crates/hennery-host/src/runtime/agents.rs`, replace:

  ```rust
              }
          }
          out.profiles.insert(name.clone(), Profile::of_installed(name, own_cli));
  ```

with:

  ```rust
                  None
              }
          };
          out.profiles.insert(name.clone(), Profile::of_installed(name, own_cli));
          out.infos.push(info(name, version, cli, true, note.as_ref()));
          out.notes.extend(note);
  ```

In `crates/hennery-host/src/runtime/agents.rs`, replace:

  ```rust
      pub set: Option<InstalledSet>,
  }
  ```

with:

  ```rust
      pub set: Option<InstalledSet>,
      /// The runtime as `hello` reports it (plan 4d-B1-i): the set, whether
      /// it is the pinned one, and whether a rollback holds the host on it.
      /// `None` only when the host's data directory has no layout at all.
      pub runtime: Option<RuntimeInfo>,
  }

  /// A managed runtime running `set`, if any.
  fn managed(set: Option<&InstalledSet>, selection: Option<&Selection>, layout: &Layout) -> RuntimeInfo {
      RuntimeInfo {
          source: RuntimeSource::Managed,
          set_id: set.map(|set| set.id.clone()),
          pinned: set.zip(selection).map(|(set, selection)| set.id == selection.set_id()),
          held: Some(layout.held()),
      }
  }
  ```

In `crates/hennery-host/src/runtime/agents.rs`, replace:

  ```rust
          }
          if !set.node.is_file() {
  ```

with:

  ```rust
          }
          let runtime = Some(managed(Some(&set), selection, &layout));
          if !set.node.is_file() {
  ```

In `crates/hennery-host/src/runtime/agents.rs`, replace:

  ```rust
              ));
              return Prepared {
                  agents: Agents {
                      notes,
                      ..Agents::default()
  ```

with:

  ```rust
              ));
              // Every agent of the set, none launchable.
              let infos = set
                  .record
                  .adapters
                  .iter()
                  .map(|(name, adapter)| info(name, &adapter.version, AgentCli::Bundled, false, notes.last()))
                  .collect();
              return Prepared {
                  agents: Agents {
                      notes,
                      infos,
                      ..Agents::default()
  ```

In `crates/hennery-host/src/runtime/agents.rs`, replace:

  ```rust
                  set: Some(set),
              };
  ```

with:

  ```rust
                  set: Some(set),
                  runtime,
              };
  ```

In `crates/hennery-host/src/runtime/agents.rs`, replace:

  ```rust
              set: Some(set),
          };
  ```

with:

  ```rust
              set: Some(set),
              runtime,
          };
  ```

In `crates/hennery-host/src/runtime/agents.rs`, replace:

  ```rust
          },
          ..Prepared::default()
  ```

with:

  ```rust
          },
          runtime: Some(managed(None, selection, &layout)),
          ..Prepared::default()
  ```

- [ ] **Step 4: Run the tests, and see them pass**

Run: `nix develop -c cargo test -p hennery-host --locked`
Expected: all pass.

- [ ] **Step 5: Revert-probes**

Each on the whole plan's tree; each must make a test fail, then be restored. The lists name every test that failed (the commands: the task's test files, and the testkit's where the collector is involved).
- A1: in `crates/hennery-host/src/adapter.rs`, replace `cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());` with `cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());`. Fails: `a_cli_writes_to_nowhere_and_is_never_blocked_by_its_output`. Restore it.
- A2: in `crates/hennery-host/src/adapter.rs`, replace `group.kill(); ⏎ match ended {` with `std::mem::forget(group); ⏎ match ended {`. Fails: `a_cli_s_exit_takes_its_guard_with_it`, `a_probe_keeps_to_one_budget_however_many_checks_hang`, `a_silent_cli_is_unknown_at_the_deadline_and_leaves_nothing_running`. Restore it.
- A3: in `crates/hennery-host/src/adapter.rs`, replace `impl Drop for GroupKill { ⏎ fn drop(&mut self) { ⏎ self.kill();` with `impl Drop for GroupKill { ⏎ fn drop(&mut self) { ⏎ let _ = self.pgid;`. Fails: `a_dropped_status_check_kills_its_group`. Restore it.
- A4: in `crates/hennery-host/src/adapter.rs`, replace `.current_dir(cwd) ⏎ .process_group(pgid)` with `.current_dir(cwd) ⏎ .process_group(0)`. Fails: `a_cli_runs_in_a_guarded_group_of_its_own`, `a_dropped_status_check_kills_its_group`, `a_leaders_own_exit_reaps_its_grandchild_immediately`, `a_probe_keeps_to_one_budget_however_many_checks_hang`, `a_silent_adapter_fails_at_the_deadline_and_leaves_nothing_running`, `a_silent_cli_is_unknown_at_the_deadline_and_leaves_nothing_running`, `an_answered_adapter_is_killed_with_its_group`, `dropping_an_adapter_kills_its_group`, `host_sigkill_during_a_kill_grace_kills_the_whole_adapter_group`, `host_sigkill_kills_a_cli_s_whole_group`, `host_sigkill_kills_the_whole_adapter_group`. Restore it.
- A5: in `crates/hennery-host/src/adapter.rs`, replace `command.env_remove(var); ⏎ } ⏎ // SAFETY` with `let _ = var; ⏎ } ⏎ // SAFETY`. Fails: `a_cli_runs_in_its_directory_with_the_hosts_stripped_environment`, `an_adapter_is_started_in_its_directory_with_the_hosts_stripped_environment`, `nesting_variables_are_removed_from_the_adapter_environment`, `the_operator_token_is_removed_from_the_adapter_environment`, `the_service_logging_variables_are_removed_from_the_adapter_environment`. Restore it.
- A6: in `crates/hennery-host/src/adapter.rs`, replace `.args(&agent.args) ⏎ .envs(agent.env.iter().cloned()) ⏎ .current_dir(cwd)` with `.args(&agent.args) ⏎ .envs(agent.env.iter().cloned()) ⏎ .current_dir("/")`. Fails: `a_cli_runs_in_its_directory_with_the_hosts_stripped_environment`, `an_adapter_is_started_in_its_directory_with_the_hosts_stripped_environment`. Restore it.
- V1: in `crates/hennery-host/src/availability.rs`, replace `.pointer("/agentInfo/version")` with `.pointer("/agentInfo/name")`. Fails: `a_probe_reports_each_agent_live`, `an_adapter_that_answers_gives_its_version_and_whether_it_takes_images`. Restore it.
- V2: in `crates/hennery-host/src/availability.rs`, replace `.unwrap_or(false),` with `.unwrap_or(true),`. Fails: `a_probe_reports_each_agent_live`, `an_answer_without_agent_info_or_prompt_capabilities_offers_no_images`. Restore it.
- V3: in `crates/hennery-host/src/availability.rs`, replace `.pointer("/agentCapabilities/promptCapabilities/image")` with `.pointer("/agentCapabilities/image")`. Fails: `a_probe_reports_each_agent_live`, `an_adapter_that_answers_gives_its_version_and_whether_it_takes_images`. Restore it.
- V4: in `crates/hennery-host/src/availability.rs`, replace `` None => Started::Failed("its adapter answered `initialize` with an error".into()), `` with `None => Started::Answered { version: None, images: false },`. Fails: `an_error_answer_fails_without_quoting_it`. Restore it.
- V5: in `crates/hennery-host/src/availability.rs`, replace `io.stdout.take(MAX_OUTPUT)` with `io.stdout.take(u64::MAX)`. Fails: `an_answer_after_too_many_bytes_is_never_read`. Restore it.
- V6: in `crates/hennery-host/src/availability.rs`, replace `for _ in 0..MAX_LINES {` with `for _ in 0..usize::MAX {`. Fails: `an_answer_after_too_many_lines_is_never_read`. Restore it.
- V7: in `crates/hennery-host/src/availability.rs`, replace `if lines.get_ref().get_ref().limit() == 0 {` with `if false {`. Fails: `an_answer_after_too_many_bytes_is_never_read`. Restore it.
- V8: in `crates/hennery-host/src/availability.rs`, replace `match tokio::time::timeout_at(deadline, answer).await {` with `match tokio::time::timeout_at(deadline + Duration::from_secs(20), answer).await {`. Fails: `a_probe_keeps_to_one_budget_however_many_checks_hang`, `a_silent_adapter_fails_at_the_deadline_and_leaves_nothing_running`. Restore it.
- L1: in `crates/hennery-host/src/availability.rs`, replace `Ok(Some(ended)) if ended.code == Some(0) => (AgentAuth::Ok, None),` with `Ok(Some(ended)) if ended.code == Some(1) => (AgentAuth::Ok, None),`. Fails: `a_cli_runs_in_its_directory_with_the_hosts_stripped_environment`, `a_probe_reports_each_agent_live`, `a_status_exit_of_zero_is_logged_in_and_any_other_exit_code_is_not`. Restore it.
- L2: in `crates/hennery-host/src/availability.rs`, replace `Ok(Some(_)) => (AgentAuth::Missing, None),` with `Ok(Some(_)) => (AgentAuth::Unknown, None),`. Fails: `a_probe_reports_each_agent_live`, `a_status_exit_of_zero_is_logged_in_and_any_other_exit_code_is_not`. Restore it.
- L3: in `crates/hennery-host/src/availability.rs`, replace `Ok(Some(ended)) if ended.code.is_none() => ( ⏎ AgentAuth::Unknown,` with `Ok(Some(ended)) if ended.code.is_none() => ( ⏎ AgentAuth::Missing,`. Fails: `a_cli_ended_by_a_signal_is_unknown`. Restore it.
- L4: in `crates/hennery-host/src/availability.rs`, replace `Ok(None) => ( ⏎ AgentAuth::Unknown,` with `Ok(None) => ( ⏎ AgentAuth::Missing,`. Fails: `a_probe_keeps_to_one_budget_however_many_checks_hang`, `a_silent_cli_is_unknown_at_the_deadline_and_leaves_nothing_running`. Restore it.
- L5: in `crates/hennery-host/src/availability.rs`, replace `Err(err) => ( ⏎ AgentAuth::Unknown,` with `Err(err) => ( ⏎ AgentAuth::Missing,`. Fails: `a_cli_that_cannot_be_started_is_unknown`. Restore it.
- L6: in `crates/hennery-host/src/availability.rs`, replace `LoginCheck::NotAsked(note) => return (AgentAuth::Unknown, note),` with `LoginCheck::NotAsked(note) => return (AgentAuth::Unknown, note.filter(|_| false)),`. Fails: `a_cli_not_asked_is_unknown_with_its_note`, `a_probe_reports_each_agent_live`. Restore it.
- L7: in `crates/hennery-host/src/availability.rs`, replace `let left = deadline.saturating_duration_since(Instant::now());` with `let left = deadline.saturating_duration_since(Instant::now()) + Duration::from_secs(20);`. Fails: `a_probe_keeps_to_one_budget_however_many_checks_hang`, `a_silent_cli_is_unknown_at_the_deadline_and_leaves_nothing_running`. Restore it.
- Q1: in `crates/hennery-host/src/availability.rs`, replace `let Some(command) = command else { return info };` with `let Some(command) = command else { ⏎ return AgentInfo { available: true, ..info }; ⏎ };`. Fails: `a_probe_reports_each_agent_live`. Restore it.
- Q2: in `crates/hennery-host/src/availability.rs`, replace `Started::Answered { version, images } => (true,` with `Started::Answered { version, images } => (false,`. Fails: `a_probe_reports_each_agent_live`, `without_checks_no_cli_is_asked`. Restore it.
- Q3: in `crates/hennery-host/src/availability.rs`, replace `Started::Failed(why) => (false, None, None, Some(why)),` with `Started::Failed(why) => (true, None, None, Some(why)),`. Fails: `a_host_probes_its_agents_one_probe_at_a_time`, `a_probe_keeps_to_one_budget_however_many_checks_hang`, `a_probe_reports_each_agent_live`. Restore it.
- Q4: in `crates/hennery-host/src/availability.rs`, replace `adapter_version: version.or(info.adapter_version),` with `adapter_version: version,`. Fails: `a_probe_reports_each_agent_live`. Restore it.
- Q5: in `crates/hennery-host/src/availability.rs`, replace `(Some(a), Some(b)) => Some(format!("{a}; {b}")),` with `(Some(a), Some(_)) => Some(a),`. Fails: `a_probe_reports_each_agent_live`. Restore it.
- Q6: in `crates/hennery-host/src/availability.rs`, replace `let deadline = Instant::now() + budget;` with `let deadline = Instant::now() + budget + Duration::from_secs(20);`. Fails: `a_probe_keeps_to_one_budget_however_many_checks_hang`. Restore it.
- Q7: in `crates/hennery-host/src/availability.rs`, replace `.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)` with `.compare_exchange(self.0.load(Ordering::Acquire), true, Ordering::AcqRel, Ordering::Acquire)`. Fails: `a_host_probes_its_agents_one_probe_at_a_time`, `one_probe_at_a_time_until_the_first_ends`. Restore it.
- Q8: in `crates/hennery-host/src/availability.rs`, replace `self.0.store(false, Ordering::Release);` with `let _ = &self.0;`. Fails: `a_host_probes_its_agents_one_probe_at_a_time`, `one_probe_at_a_time_until_the_first_ends`. Restore it.
- Q9: in `crates/hennery-host/src/availability.rs`, replace `cli: AgentCli::Given,` with `cli: AgentCli::Bundled,`. Fails: `a_host_probes_its_agents_one_probe_at_a_time`, `a_host_reports_its_agents_and_runtime_in_hello`, `the_static_view_adds_every_given_agent_and_keeps_the_runtimes`. Restore it.
- Q10: in `crates/hennery-host/src/availability.rs`, replace `let mut out: Vec<AgentInfo> = infos.to_vec();` with `let mut out: Vec<AgentInfo> = Vec::new(); ⏎ let _ = infos;`. Fails: `a_host_reports_its_agents_and_runtime_in_hello`, `the_static_view_adds_every_given_agent_and_keeps_the_runtimes`. Restore it.
- Q11: in `crates/hennery-host/src/availability.rs`, replace `out.sort_by(|a, b| a.agent.cmp(&b.agent));` with `out.sort_by(|a, b| b.agent.cmp(&a.agent));`. Fails: `a_host_probes_its_agents_one_probe_at_a_time`, `a_host_reports_its_agents_and_runtime_in_hello`, `the_static_view_adds_every_given_agent_and_keeps_the_runtimes`. Restore it.
- Q12: in `crates/hennery-host/src/availability.rs`, replace `pub fn given_runtime() -> RuntimeInfo { ⏎ RuntimeInfo { ⏎ source: RuntimeSource::Given,` with `pub fn given_runtime() -> RuntimeInfo { ⏎ RuntimeInfo { ⏎ source: RuntimeSource::Managed,`. Fails: `given_agents_are_configured_as_given`. Restore it.
- R1: in `crates/hennery-host/src/runtime/agents.rs`, replace `cli = AgentCli::Override;` with `cli = AgentCli::Bundled;`. Fails: `an_agent_left_out_is_reported_unavailable_with_its_note`, `an_override_beside_a_bundled_cli_is_reported_with_its_note`. Restore it.
- R2: in `crates/hennery-host/src/runtime/agents.rs`, replace `out.infos.push(info(name, version, cli, true, note.as_ref()));` with `out.infos.push(info(name, version, cli, false, note.as_ref()));`. Fails: `a_start_reports_each_agent_with_its_cli_and_version_and_the_pinned_set`, `an_agent_left_out_is_reported_unavailable_with_its_note`, `an_override_beside_a_bundled_cli_is_reported_with_its_note`. Restore it.
- R3: in `crates/hennery-host/src/runtime/agents.rs`, replace `pinned: set.zip(selection).map(|(set, selection)| set.id == selection.set_id()),` with `pinned: set.zip(selection).map(|(set, selection)| set.id != selection.set_id()),`. Fails: `a_start_on_another_set_says_it_is_not_pinned_and_a_rollback_says_it_holds`, `a_start_reports_each_agent_with_its_cli_and_version_and_the_pinned_set`. Restore it.
- R4: in `crates/hennery-host/src/runtime/agents.rs`, replace `held: Some(layout.held()),` with `held: Some(false),`. Fails: `a_start_on_another_set_says_it_is_not_pinned_and_a_rollback_says_it_holds`. Restore it.
- R5: in `crates/hennery-host/src/runtime/agents.rs`, replace `.map(|(name, adapter)| info(name, &adapter.version, AgentCli::Bundled, false, notes.last()))` with `.map(|(name, adapter)| info(name, &adapter.version, AgentCli::Bundled, true, notes.last()))`. Fails: `a_set_without_its_node_reports_every_agent_unavailable`. Restore it.
- R6: in `crates/hennery-host/src/runtime/agents.rs`, replace `runtime: Some(managed(None, selection, &layout)),` with `runtime: None,`. Fails: `with_no_set_the_runtime_is_managed_and_empty`. Restore it.
- R7: in `crates/hennery-host/src/runtime/agents.rs`, replace `out.infos ⏎ .push(info(name, version, AgentCli::Override, false, Some(&note)));` with `out.infos ⏎ .push(info(name, version, AgentCli::Override, true, Some(&note)));`. Fails: `an_agent_left_out_is_reported_unavailable_with_its_note`. Restore it.

- [ ] **Step 6: Check the timing-sensitive tests under load**

Run the host's `availability` and `adapter` test binaries four at a time (`for i in 1 2 3 4; do target/debug/deps/availability-* & done; wait`, and the same for `adapter-*`). Expected: all pass, every run.

- [ ] **Step 7: The full checks**

Run the five commands of "Global Constraints". Expected: all pass; **1574 tests**.

- [ ] **Step 8: Commit**

```bash
git add crates/hennery-host
git commit -m "feat(host): check the host's own agents live, in guarded groups"
```

### Task 3: `hello`, `probe_agents` and the route

**Files:**
- Create: `crates/hennery-sessions/src/agents.rs`
- Modify: `crates/hennery-proto/src/frames.rs`; `crates/hennery-host/src/connection.rs`; `crates/hennery-sessions/src/hosts.rs`, `lib.rs`, `ws.rs`
- Regenerate: `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`
- Test: `crates/hennery-testkit/tests/host_agents.rs` (new), `host_connection.rs`, `auth.rs`; the tests that build a `hello` (`images.rs`, `projects.rs`, `purge.rs`, `reconcile.rs`, `resolve.rs`, `step_up.rs`, `ws_ingest_error.rs`); `crates/hennery-proto/tests/agents.rs`, `frames.rs`

**Interfaces:**
- Consumes: Tasks 1 and 2.
- Produces: `Capability::ProbeAgents`; `HostFrame::Hello { …, agents: AgentList, runtime: MaybeRuntime }`; `HostFrame::Agents { request_id, agents, runtime }`; `CollectorFrame::ProbeAgents { request_id }`; `HostConfig.{agent_infos, runtime, checks, probe_budget}`, `HostConfig::static_agents`, `reported_runtime`; `hennery_sessions::agents::{AGENTS_PROBE_TIMEOUT, Refreshes}`; `AppState.{agent_refreshes, agents_probe_timeout}`; `GET /api/hosts/{id}/agents[?refresh=1]` → `HostAgents`.

- [ ] **Step 1: Write the failing tests**

The frames' wire names and their lenient reading; against a scripted host: what a reconciled `hello` stores and what an older or unreconciled one does not, 404 for an unknown host and for another owner's, 400 for a bad `refresh`, concurrent refreshes sharing one probe, a host that cannot be probed, an unanswered or refused probe, an answer after its caller left, a hostile answer bounded; the real host's `hello` and its probe, one at a time; the route in the operator route table. Every other test that builds a `hello` gains the two fields.

In `crates/hennery-proto/tests/agents.rs`, replace:

  ```rust
  //! Plan 4d-B1-i: a host's agents on the wire, and how both ends bound a
  //! report.

  use hennery_proto::agents::{
      AgentAuth, AgentCli, AgentInfo, MAX_AGENT_NOTE, MAX_AGENTS, RuntimeInfo, RuntimeSource, bound_agents, bounded_note,
  };
  ```

with:

  ```rust
  //! Plan 4d-B1-i: a host's agents on the wire, in `hello` and in the answer
  //! to `probe_agents`, and how both ends bound a report.

  use hennery_proto::agents::{
      AgentAuth, AgentCli, AgentInfo, AgentList, MAX_AGENT_NOTE, MAX_AGENTS, MaybeRuntime, RuntimeInfo, RuntimeSource,
      bound_agents, bounded_note,
  };
  use hennery_proto::frames::{Capability, CollectorFrame, HostFrame};
  ```

In `crates/hennery-proto/tests/agents.rs`, replace:

  ```rust
      }
  }
  ```

with:

  ```rust
      }
  }

  fn hello(extra: serde_json::Value) -> HostFrame {
      let mut v = json!({
          "type": "hello", "protocol_version": "1.0", "host_version": "0", "host_id": "h",
          "proof": "p", "attached_sessions": []
      });
      v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
      serde_json::from_value(v).unwrap()
  }

  #[test]
  fn probe_agents_and_its_answer_use_the_plan_field_names() {
      let request = CollectorFrame::ProbeAgents { request_id: "r".into() };
      let wire = json!({"type": "probe_agents", "request_id": "r"});
      assert_eq!(serde_json::to_value(&request).unwrap(), wire);
      assert_eq!(serde_json::from_value::<CollectorFrame>(wire).unwrap(), request);
      assert_eq!(request.probe_capability(), Ok(Some(Capability::ProbeAgents)));
      assert_eq!(
          serde_json::to_value(Capability::ProbeAgents).unwrap(),
          json!("probe_agents")
      );

      let answer = HostFrame::Agents {
          request_id: "r".into(),
          agents: AgentList(vec![AgentInfo {
              auth: AgentAuth::Missing,
              adapter_version: Some("0.31.0".into()),
              images: Some(false),
              note: Some("a note".into()),
              ..agent("claude")
          }]),
          runtime: MaybeRuntime(Some(RuntimeInfo {
              source: RuntimeSource::Managed,
              set_id: Some("abc123".into()),
              pinned: Some(true),
              held: Some(false),
          })),
      };
      let wire = json!({
          "type": "agents", "request_id": "r",
          "agents": [{
              "agent": "claude", "available": true, "auth": "missing", "cli": "bundled",
              "adapter_version": "0.31.0", "images": false, "note": "a note"
          }],
          "runtime": {"source": "managed", "set_id": "abc123", "pinned": true, "held": false}
      });
      assert_eq!(serde_json::to_value(&answer).unwrap(), wire);
      assert_eq!(serde_json::from_value::<HostFrame>(wire).unwrap(), answer);
      assert_eq!(answer.probe_request_id(), Some("r"));
  }
  ```

In `crates/hennery-proto/tests/agents.rs`, replace:

  ```rust
      assert_eq!(read.auth, AgentAuth::Unknown);
  }
  ```

with:

  ```rust
      assert_eq!(read.auth, AgentAuth::Unknown);
  }

  /// An older host sends neither field; a newer one may send values this
  /// build does not know. Neither refuses the `hello`: an entry that cannot
  /// be read is skipped, and a runtime that cannot be read is absent.
  #[test]
  fn a_hello_reads_its_agents_leniently() {
      let HostFrame::Hello { agents, runtime, .. } = hello(json!({})) else {
          panic!("not a hello")
      };
      assert!(agents.0.is_empty());
      assert_eq!(runtime.0, None);

      let HostFrame::Hello { agents, runtime, .. } = hello(json!({
          "agents": [
              {"agent": "claude", "available": true, "auth": "unknown", "cli": "bundled", "adapter_version": "0.31.0"},
              {"agent": "codex", "available": true, "auth": "expired", "cli": "bundled"},
              {"agent": "gemini", "available": true, "cli": "borrowed"},
              "not an agent",
              {"agent": "fake", "available": false, "cli": "given", "note": "n"}
          ],
          "runtime": {"source": "managed", "set_id": "abc", "pinned": false, "held": true}
      })) else {
          panic!("not a hello")
      };
      let names: Vec<&str> = agents.0.iter().map(|a| a.agent.as_str()).collect();
      assert_eq!(names, ["claude", "fake"]);
      assert_eq!(
          runtime.0,
          Some(RuntimeInfo {
              source: RuntimeSource::Managed,
              set_id: Some("abc".into()),
              pinned: Some(false),
              held: Some(true),
          })
      );

      let HostFrame::Hello { runtime, .. } = hello(json!({"runtime": {"source": "borrowed"}})) else {
          panic!("not a hello")
      };
      assert_eq!(runtime.0, None);
      // A list that is not a list is a malformed frame, as for every field.
      let wrong = json!({
          "type": "hello", "protocol_version": "1.0", "host_version": "0", "host_id": "h",
          "proof": "p", "attached_sessions": [], "agents": "claude"
      });
      assert!(serde_json::from_value::<HostFrame>(wrong).is_err());
  }

  /// The probe's answer is read as leniently as `hello`.
  #[test]
  fn an_agents_answer_reads_its_agents_leniently() {
      let answer: HostFrame = serde_json::from_value(json!({
          "type": "agents", "request_id": "r",
          "agents": [{"agent": "claude", "available": true, "auth": "expired", "cli": "bundled"}, {"agent": "codex", "available": false, "cli": "override"}],
          "runtime": {"source": "elsewhere"}
      }))
      .unwrap();
      let HostFrame::Agents { agents, runtime, .. } = answer else {
          panic!("not an agents answer")
      };
      assert_eq!(
          agents.0,
          [AgentInfo {
              available: false,
              cli: AgentCli::Override,
              ..agent("codex")
          }]
      );
      assert_eq!(runtime.0, None);
  }
  ```

In `crates/hennery-proto/tests/frames.rs`, replace:

  ```rust
          mcp_isolation: Default::default(),
      };
  ```

with:

  ```rust
          mcp_isolation: Default::default(),
          agents: Default::default(),
          runtime: Default::default(),
      };
  ```

In `crates/hennery-proto/tests/mcp.rs`, replace:

  ```rust
          attached_sessions: vec![],
      };
  ```

with:

  ```rust
          attached_sessions: vec![],
          agents: Default::default(),
          runtime: Default::default(),
      };
  ```

In `crates/hennery-testkit/tests/auth.rs`, replace:

  ```rust
              mcp_isolation: Default::default(),
          })
  ```

with:

  ```rust
              mcp_isolation: Default::default(),
              agents: Default::default(),
              runtime: Default::default(),
          })
  ```

In `crates/hennery-testkit/tests/auth.rs`, replace:

  ```rust
      ("GET", "/api/hosts"),
      ("POST", "/api/hosts/pairing-codes"),
  ```

with:

  ```rust
      ("GET", "/api/hosts"),
      ("GET", "/api/hosts/host-9/agents"),
      ("POST", "/api/hosts/pairing-codes"),
  ```

Create `crates/hennery-testkit/tests/host_agents.rs`:

  ```rust
  //! A host's agents on the collector (plan 4d-B1-i): what a reconciled
  //! `hello` stores, and `GET /api/hosts/{id}/agents[?refresh=1]`, against a
  //! scripted host over a real WebSocket, so the test controls every frame,
  //! its timing and the connection.

  use futures::{SinkExt, StreamExt};
  use hennery_host::identity::HostKey;
  use hennery_kernel::hosts::{Enrollment, Hosts};
  use hennery_kernel::operator::Operator;
  use hennery_proto::agents::{AgentAuth, AgentCli, AgentInfo, AgentList, MaybeRuntime, RuntimeInfo, RuntimeSource};
  use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, HostFrame};
  use hennery_proto::rest::{AgentsSource, HostAgents};
  use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
  use hennery_sessions::{AppState, store::Store};
  use std::net::SocketAddr;
  use std::time::Duration;
  use tokio_tungstenite::tungstenite::Message;

  const HOST: &str = "host-1";

  fn host_key() -> HostKey {
      HostKey::from_seed([1; 32])
  }

  struct Collector {
      addr: SocketAddr,
      state: AppState,
      client: reqwest::Client,
      _dir: tempfile::TempDir,
  }

  impl Collector {
      async fn start() -> Self {
          Self::start_with(|_| {}).await
      }

      /// A collector whose state `tune` adjusts before it serves.
      async fn start_with(tune: impl FnOnce(&mut AppState)) -> Self {
          let dir = tempfile::tempdir().unwrap();
          let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
          let addr = listener.local_addr().unwrap();
          let db = dir.path().join("hennery.db");
          let hosts = Hosts::open(&db).unwrap();
          let enrollment = Enrollment {
              public_key: host_key().public_key_hex(),
              name: "test".into(),
              host_version: "test".into(),
              platform: "test".into(),
          };
          hosts.register(HOST, &enrollment, 0).unwrap();
          let operator = Operator::open(&db).unwrap();
          let client = hennery_testkit::operator_client(&operator);
          let mut state = AppState::new(Store::open(&db).unwrap(), hosts, operator);
          tune(&mut state);
          tokio::spawn(hennery_sessions::serve(listener, state.clone()));
          Self {
              addr,
              state,
              client,
              _dir: dir,
          }
      }

      fn url(&self, path: &str) -> String {
          format!("http://{}{path}", self.addr)
      }

      async fn get(&self, path: &str) -> reqwest::Response {
          self.client.get(self.url(path)).send().await.unwrap()
      }

      /// `GET /api/hosts/host-1/agents`, with `query`, as the API answers it:
      /// never cached, since a note can name a path on the host.
      async fn agents(&self, query: &str) -> HostAgents {
          let resp = self.get(&format!("/api/hosts/{HOST}/agents{query}")).await;
          assert_eq!(resp.status(), 200);
          assert_eq!(resp.headers()["cache-control"], "no-store");
          resp.json().await.unwrap()
      }
  }

  type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

  /// The test playing a host.
  struct ScriptedHost {
      ws: Ws,
  }

  impl ScriptedHost {
      /// `hello` with `capabilities`, `agents` and `runtime`, its
      /// `hello_ack`, and with `reconcile` its `resend_complete`, after which
      /// the host is waited for until it is listed as connected.
      async fn connect(
          collector: &Collector,
          capabilities: Vec<Capability>,
          agents: Vec<AgentInfo>,
          runtime: Option<RuntimeInfo>,
          reconcile: bool,
      ) -> Self {
          let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
              .await
              .unwrap();
          let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
          let mut host = Self { ws };
          host.send(&HostFrame::Hello {
              protocol_version: PROTOCOL_VERSION.into(),
              host_version: "test".into(),
              host_id: HOST.into(),
              proof: host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION),
              capabilities: Capabilities(capabilities),
              mcp_isolation: Default::default(),
              workspace_roots: vec![],
              attached_sessions: vec![],
              agents: AgentList(agents),
              runtime: MaybeRuntime(runtime),
          })
          .await;
          assert!(matches!(host.next().await, CollectorFrame::HelloAck { .. }));
          if reconcile {
              host.send(&HostFrame::ResendComplete).await;
              wait_for("host ready", || async {
                  collector.state.hub.is_ready(HOST).then_some(())
              })
              .await;
          }
          host
      }

      /// `connect`, reconciled, as a host of this build: it can be probed.
      async fn current(collector: &Collector) -> Self {
          Self::connect(
              collector,
              vec![Capability::ProbeAgents],
              vec![agent("claude", AgentAuth::Unknown)],
              Some(managed()),
              true,
          )
          .await
      }

      async fn send(&mut self, frame: &HostFrame) {
          self.ws
              .send(Message::text(serde_json::to_string(frame).unwrap()))
              .await
              .unwrap();
      }

      /// The next collector frame that is not an `ack`.
      async fn next(&mut self) -> CollectorFrame {
          tokio::time::timeout(Duration::from_secs(10), async {
              loop {
                  match self.ws.next().await {
                      Some(Ok(Message::Text(text))) => match serde_json::from_str(&text).unwrap() {
                          CollectorFrame::Ack { .. } => {}
                          frame => return frame,
                      },
                      Some(Ok(_)) => {}
                      other => panic!("collector connection ended: {other:?}"),
                  }
              }
          })
          .await
          .expect("a collector frame within 10s")
      }

      /// The request id of the `probe_agents` this host was sent next.
      async fn probed(&mut self) -> String {
          match self.next().await {
              CollectorFrame::ProbeAgents { request_id } => request_id,
              other => panic!("expected probe_agents, got {other:?}"),
          }
      }

      /// Nothing but pings reaches this host within `within`.
      async fn hears_nothing(&mut self, within: Duration) {
          let more = tokio::time::timeout(within, self.next()).await;
          assert!(more.is_err(), "the host was sent {more:?}");
      }

      async fn answer(&mut self, request_id: String, agents: Vec<AgentInfo>) {
          self.send(&HostFrame::Agents {
              request_id,
              agents: AgentList(agents),
              runtime: MaybeRuntime(Some(managed())),
          })
          .await;
      }

      /// Drop the connection and wait until the collector has noticed.
      async fn drop_connection(self, collector: &Collector) {
          drop(self.ws);
          wait_for("host gone", || async {
              (!collector.state.hub.is_ready(HOST)).then_some(())
          })
          .await;
      }
  }

  async fn wait_for<T, F, Fut>(what: &str, mut probe: F) -> T
  where
      F: FnMut() -> Fut,
      Fut: std::future::Future<Output = Option<T>>,
  {
      let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
      loop {
          if let Some(v) = probe().await {
              return v;
          }
          assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
          tokio::time::sleep(Duration::from_millis(20)).await;
      }
  }

  fn agent(name: &str, auth: AgentAuth) -> AgentInfo {
      AgentInfo {
          agent: name.into(),
          available: true,
          auth,
          cli: AgentCli::Bundled,
          adapter_version: Some("1.0.0".into()),
          images: None,
          note: None,
      }
  }

  fn managed() -> RuntimeInfo {
      RuntimeInfo {
          source: RuntimeSource::Managed,
          set_id: Some("abc".into()),
          pinned: Some(true),
          held: Some(false),
      }
  }

  fn source_of(collector: &Collector) -> AgentsSource {
      collector.state.hosts.agents(HOST).unwrap().unwrap().source
  }

  #[tokio::test]
  async fn a_reconciled_hello_stores_the_agents_of_a_host_that_reports_them() {
      let collector = Collector::start().await;
      let _host = ScriptedHost::current(&collector).await;
      let got = collector.agents("").await;
      assert_eq!(got.host_id, HOST);
      assert_eq!(got.source, AgentsSource::Hello);
      assert_eq!(got.agents, [agent("claude", AgentAuth::Unknown)]);
      assert_eq!(got.runtime, Some(managed()));
      assert!(got.live);
      let at = got.reported_at.unwrap();
      assert!(at.ends_with('Z') && at.contains('T'), "{at}");
  }

  /// A host of an older build has no `probe_agents`, and reports nothing.
  #[tokio::test]
  async fn an_older_hosts_hello_stores_nothing() {
      let collector = Collector::start().await;
      let _host = ScriptedHost::connect(
          &collector,
          vec![Capability::Projects],
          vec![agent("claude", AgentAuth::Unknown)],
          Some(managed()),
          true,
      )
      .await;
      let got = collector.agents("").await;
      assert_eq!(got.source, AgentsSource::None);
      assert!(got.agents.is_empty() && got.runtime.is_none() && got.reported_at.is_none());
      assert!(got.live);
  }

  /// `hennery host join`'s and doctor's probes send a `hello` and never
  /// reconcile: what it says is not stored.
  #[tokio::test]
  async fn a_hello_never_reconciled_stores_nothing() {
      let collector = Collector::start().await;
      let host = ScriptedHost::connect(
          &collector,
          vec![Capability::ProbeAgents],
          vec![agent("claude", AgentAuth::Unknown)],
          None,
          false,
      )
      .await;
      drop(host);
      let got = collector.agents("").await;
      assert_eq!(got.source, AgentsSource::None);
      assert!(!got.live);
  }

  #[tokio::test]
  async fn an_unknown_host_is_not_found_and_a_bad_refresh_is_invalid() {
      let collector = Collector::start().await;
      let resp = collector.get("/api/hosts/host-9/agents").await;
      assert_eq!(resp.status(), 404);
      let resp = collector.get("/api/hosts/host-9/agents?refresh=1").await;
      assert_eq!(resp.status(), 404);
      for query in ["?refresh=2", "?refresh=yes", "?refresh="] {
          let resp = collector.get(&format!("/api/hosts/{HOST}/agents{query}")).await;
          assert_eq!(resp.status(), 400, "{query}");
      }
      assert_eq!(collector.agents("?refresh=0").await.source, AgentsSource::None);
  }

  /// No existence oracle: another owner's host, with a report stored, answers
  /// as an unknown one, refreshed or not, and its report is never shown. Even
  /// connected (the hub, which never checks owners, is given it directly),
  /// it is never probed: the registry decides first (the review's A3).
  #[tokio::test]
  async fn another_owners_host_is_not_found() {
      const OTHER: &str = "owner-00000000000000b2";
      let collector = Collector::start().await;
      let conn = rusqlite::Connection::open(collector._dir.path().join("hennery.db")).unwrap();
      conn.execute(
          "INSERT INTO owners(id, created_at, set_up_at) VALUES (?1, 0, 0)",
          [OTHER],
      )
      .unwrap();
      conn.execute(
          "INSERT INTO hats(id, owner_id, name, colour, created_at) VALUES ('hat-00000000000000b2', ?1, 'Theirs', '#000000', 0)",
          [OTHER],
      )
      .unwrap();
      conn.execute(
          "INSERT INTO hosts(id, owner_id, name, public_key, platform, host_version, default_hat_id, created_at,
                             agents, agents_reported_at, agents_source)
           VALUES ('host-b2', ?1, 'theirs', 'b2', 'p', 'v', 'hat-00000000000000b2', 0,
                   '{\"agents\":[{\"agent\":\"theirs\",\"available\":true,\"cli\":\"bundled\"}]}', 1, 'probe')",
          [OTHER],
      )
      .unwrap();
      let (tx, mut sent) = tokio::sync::mpsc::unbounded_channel();
      let registration = collector
          .state
          .hub
          .register(
              "host-b2",
              tx,
              Capabilities(vec![Capability::ProbeAgents]),
              Default::default(),
          )
          .unwrap();
      collector.state.hub.mark_ready("host-b2", registration.conn_id);
      assert!(collector.state.hub.is_ready("host-b2"));
      for query in ["", "?refresh=1"] {
          let resp = collector.get(&format!("/api/hosts/host-b2/agents{query}")).await;
          assert_eq!(resp.status(), 404, "{query}");
          let body = resp.text().await.unwrap();
          assert!(!body.contains("theirs"), "{body}");
      }
      assert!(sent.try_recv().is_err(), "another owner's host was probed");
  }

  /// Concurrent refreshes share one probe; its answer is stored and each of
  /// them answers from the store. A refresh after them probes again.
  #[tokio::test]
  async fn concurrent_refreshes_share_one_probe() {
      let collector = std::sync::Arc::new(Collector::start().await);
      let mut host = ScriptedHost::current(&collector).await;
      let refresh = || {
          let collector = collector.clone();
          tokio::spawn(async move { collector.agents("?refresh=1").await })
      };
      let (first, second) = (refresh(), refresh());
      let request_id = host.probed().await;
      host.hears_nothing(Duration::from_millis(300)).await;
      assert!(!first.is_finished() && !second.is_finished());
      host.answer(request_id, vec![agent("claude", AgentAuth::Ok)]).await;
      for got in [first.await.unwrap(), second.await.unwrap()] {
          assert_eq!(got.source, AgentsSource::Probe);
          assert_eq!(got.agents, [agent("claude", AgentAuth::Ok)]);
      }
      let third = refresh();
      let request_id = host.probed().await;
      host.answer(request_id, vec![agent("claude", AgentAuth::Missing)]).await;
      assert_eq!(third.await.unwrap().agents, [agent("claude", AgentAuth::Missing)]);
  }

  /// The host that cannot be probed is not asked: one gone, one of an older
  /// build. Either answers at once, from the store.
  #[tokio::test]
  async fn a_refresh_of_a_host_that_cannot_be_probed_answers_the_last_report() {
      let collector = Collector::start().await;
      let host = ScriptedHost::current(&collector).await;
      host.drop_connection(&collector).await;
      let began = std::time::Instant::now();
      let got = collector.agents("?refresh=1").await;
      assert!(began.elapsed() < Duration::from_secs(2), "{:?}", began.elapsed());
      assert_eq!((got.source, got.live), (AgentsSource::Hello, false));
      assert_eq!(got.agents, [agent("claude", AgentAuth::Unknown)]);

      let mut older = ScriptedHost::connect(&collector, vec![Capability::Projects], vec![], None, true).await;
      let got = collector.agents("?refresh=1").await;
      assert_eq!((got.source, got.live), (AgentsSource::Hello, true));
      older.hears_nothing(Duration::from_millis(300)).await;
  }

  /// A probe the host does not answer in time, or refuses, changes nothing:
  /// the last report stands.
  #[tokio::test]
  async fn an_unanswered_or_refused_probe_leaves_the_last_report() {
      let collector = Collector::start_with(|state| state.agents_probe_timeout = Duration::from_millis(500)).await;
      let mut host = ScriptedHost::current(&collector).await;
      let began = std::time::Instant::now();
      let call = {
          let url = collector.url(&format!("/api/hosts/{HOST}/agents?refresh=1"));
          let client = collector.client.clone();
          tokio::spawn(async move {
              client
                  .get(url)
                  .send()
                  .await
                  .unwrap()
                  .json::<HostAgents>()
                  .await
                  .unwrap()
          })
      };
      let _ignored = host.probed().await;
      let got = call.await.unwrap();
      assert!(began.elapsed() < Duration::from_secs(5), "{:?}", began.elapsed());
      assert_eq!(got.source, AgentsSource::Hello);
      assert!(
          collector.state.hub.is_ready(HOST),
          "a probe's timeout keeps the connection"
      );

      let call = {
          let url = collector.url(&format!("/api/hosts/{HOST}/agents?refresh=1"));
          let client = collector.client.clone();
          tokio::spawn(async move {
              client
                  .get(url)
                  .send()
                  .await
                  .unwrap()
                  .json::<HostAgents>()
                  .await
                  .unwrap()
          })
      };
      let request_id = host.probed().await;
      host.send(&HostFrame::Error {
          request_id,
          code: "busy".into(),
          message: "a probe of the agents is running".into(),
      })
      .await;
      assert_eq!(call.await.unwrap().source, AgentsSource::Hello);
  }

  /// The probe outlives the request that started it: an answer that comes
  /// after its caller went away is still stored.
  #[tokio::test]
  async fn an_answer_after_its_caller_left_is_stored() {
      let collector = Collector::start().await;
      let mut host = ScriptedHost::current(&collector).await;
      let gone = tokio::time::timeout(Duration::from_millis(200), collector.agents("?refresh=1")).await;
      assert!(gone.is_err(), "the refresh waits for the probe");
      let request_id = host.probed().await;
      host.answer(request_id, vec![agent("claude", AgentAuth::Ok)]).await;
      wait_for("the probe's answer stored", || async {
          (source_of(&collector) == AgentsSource::Probe).then_some(())
      })
      .await;
  }

  /// A host's answer is bounded and escaped before it is stored: it may
  /// lie, but only about itself.
  #[tokio::test]
  async fn a_hostile_answer_is_bounded_before_it_is_stored() {
      let collector = std::sync::Arc::new(Collector::start().await);
      let mut host = ScriptedHost::current(&collector).await;
      let call = {
          let collector = collector.clone();
          tokio::spawn(async move { collector.agents("?refresh=1").await })
      };
      let request_id = host.probed().await;
      let mut loud = agent("claude", AgentAuth::Ok);
      loud.note = Some(format!("<script>\u{202E}{}", "x".repeat(10_000)));
      let mut many = vec![
          loud,
          agent("claude", AgentAuth::Missing),
          agent("bad name", AgentAuth::Ok),
      ];
      many.extend((0..40).map(|i| agent(&format!("a{i}"), AgentAuth::Ok)));
      host.answer(request_id, many).await;
      let got = call.await.unwrap();
      assert_eq!(got.agents.len(), hennery_proto::agents::MAX_AGENTS);
      let note = got.agents[0].note.clone().unwrap();
      assert!(note.len() <= hennery_proto::agents::MAX_AGENT_NOTE, "{}", note.len());
      assert!(note.starts_with("<script>\u{fffd}x"), "{note}");
      assert_eq!(got.agents[0].auth, AgentAuth::Ok, "the first of a name stands");
      assert_eq!(got.agents[1].agent, "a0");
  }
  ```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

  ```rust
              Capability::McpServers,
          ])
  ```

with:

  ```rust
              Capability::McpServers,
              Capability::ProbeAgents,
          ])
  ```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

  ```rust
      assert_eq!(logged[2].1["_meta"], strict, "{}", logged[2].1);
  }
  ```

with:

  ```rust
      assert_eq!(logged[2].1["_meta"], strict, "{}", logged[2].1);
  }

  // Plan 4d-B1-i: the host's agents, in `hello` and live.

  /// `hello` reports the agents as configured: the runtime's infos as they
  /// are, every other agent as given; and the runtime, bounded.
  #[tokio::test]
  async fn a_host_reports_its_agents_and_runtime_in_hello() {
      use hennery_proto::agents::{AgentAuth, AgentCli, AgentInfo, RuntimeInfo, RuntimeSource};
      let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
      let addr = listener.local_addr().unwrap();
      let mut cfg = host_with_fake(addr, "hello-agents", slow_fake());
      let codex = AgentInfo {
          agent: "codex".into(),
          available: false,
          auth: AgentAuth::Unknown,
          cli: AgentCli::Bundled,
          adapter_version: Some("1.0.0".into()),
          images: None,
          note: Some("codex is unavailable".into()),
      };
      cfg.agent_infos = vec![codex.clone()];
      cfg.runtime = Some(RuntimeInfo {
          source: RuntimeSource::Managed,
          set_id: Some("not a set id".into()),
          pinned: Some(true),
          held: Some(false),
      });
      tokio::spawn(run(cfg));
      let (tcp, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
          .await
          .expect("host connects")
          .unwrap();
      let (_sink, mut stream) = accept(tcp).await.unwrap().split();
      let HostFrame::Hello { agents, runtime, .. } = read_host_frame(&mut stream).await else {
          panic!("expected hello");
      };
      let fake = AgentInfo {
          agent: "fake".into(),
          available: true,
          auth: AgentAuth::Unknown,
          cli: AgentCli::Given,
          adapter_version: None,
          images: None,
          note: None,
      };
      assert_eq!(agents.0, [codex, fake]);
      assert_eq!(
          runtime.0,
          Some(RuntimeInfo {
              source: RuntimeSource::Managed,
              set_id: None,
              pinned: Some(true),
              held: Some(false),
          })
      );
  }

  /// A login check the test releases: the probe holds its slot until then.
  /// It writes the directory it runs in to `<release>.cwd` first.
  #[derive(Debug)]
  struct HeldLogin(std::path::PathBuf);

  impl hennery_host::availability::AgentChecks for HeldLogin {
      fn login(&self, agent: &str) -> hennery_host::availability::LoginCheck {
          use hennery_host::availability::LoginCheck;
          if agent != "fake" {
              return LoginCheck::NotAsked(None);
          }
          LoginCheck::Ask(hennery_host::AgentCommand {
              program: "sh".into(),
              args: vec![
                  "-c".into(),
                  format!(
                      "pwd > {r}.cwd; while [ ! -e {r} ]; do sleep 0.05; done; exit 0",
                      r = self.0.display()
                  ),
              ],
              env: Vec::new(),
          })
      }
  }

  /// `probe_agents` checks each agent live and answers on the connection;
  /// another probe while one runs is answered `busy`, and once the first is
  /// answered a new one runs. Its programs run in the host's home directory,
  /// never the host's own working directory (the review's A5).
  #[tokio::test]
  async fn a_host_probes_its_agents_one_probe_at_a_time() {
      use hennery_proto::agents::{AgentAuth, AgentCli};
      let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
      let addr = listener.local_addr().unwrap();
      let dir = tempfile::tempdir().unwrap();
      let release = dir.path().join("release");
      let mut cfg = host_with_fake(addr, "probe-agents", slow_fake());
      cfg.agents.insert(
          "gone".into(),
          hennery_host::AgentCommand {
              program: "/nonexistent/adapter".into(),
              args: Vec::new(),
              env: Vec::new(),
          },
      );
      cfg.checks = std::sync::Arc::new(HeldLogin(release.clone()));
      cfg.probe_budget = Duration::from_secs(10);
      // A host given `--agent` commands: the answer carries its runtime.
      cfg.runtime = Some(hennery_host::availability::given_runtime());
      let home = tempfile::tempdir().unwrap();
      let home = std::fs::canonicalize(home.path()).unwrap();
      cfg.home = Some(home.clone());
      tokio::spawn(run(cfg));
      let (mut sink, mut stream, _) = accept_host(&listener).await;
      let probe = |request_id: &str| CollectorFrame::ProbeAgents {
          request_id: request_id.into(),
      };
      send_frame(&mut sink, &probe("p1")).await;
      send_frame(&mut sink, &probe("p2")).await;
      let busy = read_until(&mut stream, error_for("p2")).await;
      assert!(
          matches!(&busy, HostFrame::Error { code, .. } if code == "busy"),
          "{busy:?}"
      );
      std::fs::write(&release, "").unwrap();
      let reply = read_until(&mut stream, |f| f.probe_request_id() == Some("p1")).await;
      let HostFrame::Agents { agents, runtime, .. } = reply else {
          panic!("expected agents, got {reply:?}");
      };
      assert_eq!(runtime.0, Some(hennery_host::availability::given_runtime()));
      let names: Vec<&str> = agents.0.iter().map(|a| a.agent.as_str()).collect();
      assert_eq!(names, ["fake", "gone"]);
      let (fake, gone) = (&agents.0[0], &agents.0[1]);
      assert!(fake.available, "{fake:?}");
      assert_eq!(
          (fake.cli, fake.auth, fake.images),
          (AgentCli::Given, AgentAuth::Ok, Some(true))
      );
      assert!(!gone.available);
      assert!(gone.note.as_deref().unwrap().contains("cannot be started"), "{gone:?}");
      let cwd = std::fs::read_to_string(release.with_extension("cwd")).unwrap();
      assert_eq!(cwd.trim_end(), home.to_str().unwrap());
      // The first answered: the slot is free again.
      send_frame(&mut sink, &probe("p3")).await;
      let reply = read_until(&mut stream, |f| {
          f.probe_request_id() == Some("p3") || error_for("p3")(f)
      })
      .await;
      assert!(matches!(reply, HostFrame::Agents { .. }), "{reply:?}");
  }
  ```

In `crates/hennery-testkit/tests/images.rs`, replace:

  ```rust
              mcp_isolation: Default::default(),
          })
  ```

with:

  ```rust
              mcp_isolation: Default::default(),
              agents: Default::default(),
              runtime: Default::default(),
          })
  ```

In `crates/hennery-testkit/tests/projects.rs`, replace:

  ```rust
              workspace_roots,
              attached_sessions: vec![],
              mcp_isolation: Default::default(),
          })
          .await;
          let ack = host.next().await;
  ```

with:

  ```rust
              workspace_roots,
              attached_sessions: vec![],
              mcp_isolation: Default::default(),
              agents: Default::default(),
              runtime: Default::default(),
          })
          .await;
          let ack = host.next().await;
  ```

In `crates/hennery-testkit/tests/projects.rs`, replace:

  ```rust
              mcp_isolation: Default::default(),
          })
  ```

with:

  ```rust
              mcp_isolation: Default::default(),
              agents: Default::default(),
              runtime: Default::default(),
          })
  ```

In `crates/hennery-testkit/tests/purge.rs`, replace:

  ```rust
              mcp_isolation: Default::default(),
          })
  ```

with:

  ```rust
              mcp_isolation: Default::default(),
              agents: Default::default(),
              runtime: Default::default(),
          })
  ```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
              attached_sessions: attached,
              mcp_isolation: Default::default(),
          })
          .await;
  ```

with:

  ```rust
              attached_sessions: attached,
              mcp_isolation: Default::default(),
              agents: Default::default(),
              runtime: Default::default(),
          })
          .await;
  ```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
              mcp_isolation: Default::default(),
          })
  ```

with:

  ```rust
              mcp_isolation: Default::default(),
              agents: Default::default(),
              runtime: Default::default(),
          })
  ```

In `crates/hennery-testkit/tests/resolve.rs`, replace:

  ```rust
              mcp_isolation,
          })
  ```

with:

  ```rust
              mcp_isolation,
              agents: Default::default(),
              runtime: Default::default(),
          })
  ```

In `crates/hennery-testkit/tests/step_up.rs`, replace:

  ```rust
      assert_eq!(resp.status(), 200);
      assert!(c.state.operator.authenticate(&other, unix_now()).unwrap().is_some());
  ```

with:

  ```rust
      assert_eq!(resp.status(), 200);
      // A host's agents are read, a refresh included (plan 4d-B1-i).
      let resp = send(&stale, "GET", "/api/hosts/host-9/agents?refresh=1", None)
          .await
          .unwrap();
      assert_eq!(code_of(resp).await, (404, "not_found".into()));
      assert!(c.state.operator.authenticate(&other, unix_now()).unwrap().is_some());
  ```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

  ```rust
              capabilities: Default::default(),
              workspace_roots: vec![],
              attached_sessions: vec![],
              mcp_isolation: Default::default(),
          })
          .unwrap(),
      ))
      .await
  ```

with:

  ```rust
              capabilities: Default::default(),
              workspace_roots: vec![],
              attached_sessions: vec![],
              mcp_isolation: Default::default(),
              agents: Default::default(),
              runtime: Default::default(),
          })
          .unwrap(),
      ))
      .await
  ```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

  ```rust
              mcp_isolation: Default::default(),
          })
  ```

with:

  ```rust
              mcp_isolation: Default::default(),
              agents: Default::default(),
              runtime: Default::default(),
          })
  ```

- [ ] **Step 2: Run them, and see them fail**

Run: `nix develop -c cargo test -p hennery-testkit --test host_agents --locked; nix develop -c cargo test -p hennery-proto --test agents --test frames --locked`
Expected: FAIL to compile: `HostFrame::Hello` has no `agents` or `runtime`, and `Capability::ProbeAgents`, `CollectorFrame::ProbeAgents` and `HostFrame::Agents` do not exist yet (`hennery-proto`'s `agents` and `frames` tests, the testkit's `host_agents`).

- [ ] **Step 3: Write the implementation**

In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
  use crate::agent_home::Registry;
  use crate::forget::{Forget, ForgetContext};
  ```

with:

  ```rust
  use crate::agent_home::Registry;
  use crate::availability::{self, AgentChecks, NoChecks, OneProbe};
  use crate::forget::{Forget, ForgetContext};
  ```

In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
  use futures::{SinkExt, StreamExt};
  use hennery_proto::frames::{
  ```

with:

  ```rust
  use futures::{SinkExt, StreamExt};
  use hennery_proto::agents::{AgentInfo, AgentList, MaybeRuntime, RuntimeInfo};
  use hennery_proto::frames::{
  ```

In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
      pub git: Option<PathBuf>,
  }
  ```

with:

  ```rust
      pub git: Option<PathBuf>,
      /// The managed runtime's agents as `hello` reports them (plan
      /// 4d-B1-i), those left out of `agents` too. An agent of `agents` not
      /// in it is reported as given by `--agent`.
      pub agent_infos: Vec<AgentInfo>,
      /// Where the agents come from, as `hello` and `probe_agents` report it.
      pub runtime: Option<RuntimeInfo>,
      /// Doctor's knowledge for `probe_agents` (the binary's): none unless
      /// set.
      pub checks: Arc<dyn AgentChecks>,
      /// How long one `probe_agents` takes at most.
      pub probe_budget: Duration,
  }
  ```

In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
              git: crate::git::find_git(),
          }
      }
  ```

with:

  ```rust
              git: crate::git::find_git(),
              agent_infos: Vec::new(),
              runtime: None,
              checks: Arc::new(NoChecks),
              probe_budget: availability::PROBE_BUDGET,
          }
      }

      /// `hello.agents`: the static view (plan 4d-B1-i).
      pub fn static_agents(&self) -> Vec<AgentInfo> {
          availability::static_agents(&self.agents, &self.agent_infos)
      }

      /// `hello.runtime` and the probe's, bounded.
      pub fn reported_runtime(&self) -> Option<RuntimeInfo> {
          self.runtime.clone().map(RuntimeInfo::bounded)
      }
  ```

In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
      // Outlives each connection, so its bounds hold across reconnects.
      let probes = Probes::default();
  ```

with:

  ```rust
      // Outlive each connection, so their bounds hold across reconnects.
      let probes = Probes::default();
      let one_probe = OneProbe::default();
  ```

In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
              if let Err(err) = connect_once(&cfg, &uplink, &sessions, &probes, &homes, &mut replies, &mut backoff).await
              {
  ```

with:

  ```rust
              let probes = (&probes, &one_probe);
              if let Err(err) = connect_once(&cfg, &uplink, &sessions, probes, &homes, &mut replies, &mut backoff).await {
  ```

In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
  /// Connect, send a `hello` signed over this connection's nonce (ACP core
  /// §3.5), and read the collector's answer. `attached` is read once the
  /// socket is up, right before the `hello` goes out.
  /// What a `hello` reports of the host's configuration: nothing for a
  /// probe.
  ```

with:

  ```rust
  /// What a `hello` reports of the host's configuration: nothing for a
  /// probe of the pairing (`probe`).
  ```

In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
      mcp_isolation: AgentIsolation,
  }

  async fn handshake(
  ```

with:

  ```rust
      mcp_isolation: AgentIsolation,
      agents: Vec<AgentInfo>,
      runtime: Option<RuntimeInfo>,
  }

  /// Connect, send a `hello` signed over this connection's nonce (ACP core
  /// §3.5), and read the collector's answer. `attached` is read once the
  /// socket is up, right before the `hello` goes out.
  async fn handshake(
  ```

In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
                  Capability::McpServers,
              ]),
  ```

with:

  ```rust
                  Capability::McpServers,
                  Capability::ProbeAgents,
              ]),
  ```

In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
              attached_sessions: attached()?,
          },
  ```

with:

  ```rust
              attached_sessions: attached()?,
              agents: AgentList(announce.agents),
              runtime: MaybeRuntime(announce.runtime),
          },
  ```

In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
      uplink: &Uplink,
      sessions: &Sessions,
      probes: &Probes,
      homes: &Arc<Registry>,
      replies: &mut mpsc::UnboundedReceiver<HostFrame>,
  ```

with:

  ```rust
      uplink: &Uplink,
      sessions: &Sessions,
      probes: (&Probes, &OneProbe),
      homes: &Arc<Registry>,
      replies: &mut mpsc::UnboundedReceiver<HostFrame>,
  ```

In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
  ) -> Result<()> {
      let (mut sink, mut stream, answer) = handshake(
  ```

with:

  ```rust
  ) -> Result<()> {
      let announce = Announce {
          workspace_roots: cfg.reported_roots(),
          mcp_isolation: cfg.mcp_isolation(),
          agents: cfg.static_agents(),
          runtime: cfg.reported_runtime(),
      };
      let (mut sink, mut stream, answer) = handshake(
  ```

In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
          Announce {
              workspace_roots: cfg.reported_roots(),
              mcp_isolation: cfg.mcp_isolation(),
          },
  ```

with:

  ```rust
          announce,
  ```

In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
      probes: &Probes,
  ```

with:

  ```rust
      (probes, one_probe): (&Probes, &OneProbe),
  ```

In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
          CollectorFrame::HelloAck { .. } | CollectorFrame::HelloError { .. } => {}
      }
      Ok(())
  ```

with:

  ```rust
          CollectorFrame::ProbeAgents { request_id } => probe_agents(cfg, uplink, one_probe, request_id),
          CollectorFrame::HelloAck { .. } | CollectorFrame::HelloError { .. } => {}
      }
      Ok(())
  }

  /// `probe_agents` (plan 4d-B1-i): the fixed checks on this host's own
  /// agents, in a task of its own, answered on the reply channel. One at a
  /// time: another is answered `busy`.
  fn probe_agents(cfg: &HostConfig, uplink: &Uplink, one_probe: &OneProbe, request_id: String) {
      let Some(slot) = one_probe.try_start() else {
          return uplink.reply(HostFrame::Error {
              request_id,
              code: "busy".into(),
              message: "a probe of the agents is running".into(),
          });
      };
      let report = cfg.static_agents();
      let runtime = cfg.reported_runtime();
      let agents = cfg.agents.clone();
      let checks = cfg.checks.clone();
      // The home directory, never the host's own working directory: a
      // repository there could configure an agent with code of its own.
      let cwd = cfg.home.clone().unwrap_or_else(|| PathBuf::from("/"));
      let budget = cfg.probe_budget;
      let uplink = uplink.clone();
      tokio::spawn(async move {
          let agents = availability::probe(&report, &agents, checks, &cwd, budget).await;
          uplink.reply(HostFrame::Agents {
              request_id,
              agents: AgentList(agents),
              runtime: MaybeRuntime(runtime),
          });
          drop(slot);
      });
  ```

In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
  use schemars::JsonSchema;
  ```

with:

  ```rust
  use crate::agents::{AgentList, MaybeRuntime};
  use schemars::JsonSchema;
  ```

In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
      McpServers,
  }
  ```

with:

  ```rust
      McpServers,
      /// Checking its agents live (`probe_agents`, plan 4d-B1-i); such a host
      /// reports its agents in `hello` too.
      ProbeAgents,
  }
  ```

In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
          attached_sessions: Vec<AttachedSession>,
      },
  ```

with:

  ```rust
          attached_sessions: Vec<AttachedSession>,
          /// Its agents, as configured (plan 4d-B1-i): `available` says each
          /// can be launched, `auth` is `unknown`, `images` absent. Read
          /// leniently: an entry this build cannot read is skipped. Absent
          /// from an older host.
          #[serde(default)]
          #[ts(as = "Vec<crate::agents::AgentInfo>")]
          agents: AgentList,
          /// Where its agents come from. Absent from an older host, or one
          /// this build cannot read.
          #[serde(default, skip_serializing_if = "MaybeRuntime::is_none")]
          #[ts(type = "RuntimeInfo | undefined", optional)]
          runtime: MaybeRuntime,
      },
  ```

In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
      },
      /// The answer to `browse_directory` (ACP core §3.3, §7): the
  ```

with:

  ```rust
      },
      /// The answer to `probe_agents` (plan 4d-B1-i): each agent started as
      /// the host starts it and asked `initialize`, and its CLI asked whether
      /// it is logged in. A probe reply, like `projects`; read leniently, like
      /// `hello`'s agents.
      Agents {
          request_id: String,
          #[serde(default)]
          #[ts(as = "Vec<crate::agents::AgentInfo>")]
          agents: AgentList,
          #[serde(default, skip_serializing_if = "MaybeRuntime::is_none")]
          #[ts(type = "RuntimeInfo | undefined", optional)]
          runtime: MaybeRuntime,
      },
      /// The answer to `browse_directory` (ACP core §3.3, §7): the
  ```

In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
              Self::ForgetSession { .. } => Ok(Some(Capability::ForgetSession)),
              Self::HelloAck { .. }
  ```

with:

  ```rust
              Self::ForgetSession { .. } => Ok(Some(Capability::ForgetSession)),
              Self::ProbeAgents { .. } => Ok(Some(Capability::ProbeAgents)),
              Self::HelloAck { .. }
  ```

In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
              | Self::ForgetSession { .. } => None,
  ```

with:

  ```rust
              | Self::ForgetSession { .. }
              | Self::ProbeAgents { .. } => None,
  ```

In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
              | Self::SessionForgotten { request_id, .. } => Some(request_id),
  ```

with:

  ```rust
              | Self::SessionForgotten { request_id, .. }
              | Self::Agents { request_id, .. } => Some(request_id),
  ```

In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
          fallback: bool,
      },
  }
  ```

with:

  ```rust
          fallback: bool,
      },
      /// Check the host's own agents live (plan 4d-B1-i): only to a host with
      /// the `probe_agents` capability. It names nothing: the host runs a
      /// fixed set of read-only checks on the agents it is configured with.
      /// Answered by `agents` | `error{busy}`.
      ProbeAgents {
          request_id: String,
      },
  }
  ```

Create `crates/hennery-sessions/src/agents.rs`:

  ```rust
  //! A host's agents on the collector (plan 4d-B1-i): the latest report, from
  //! a reconciled `hello` or a `probe_agents`, stored in the host registry;
  //! and `GET /api/hosts/{id}/agents[?refresh=1]`, which answers from the
  //! store, after one probe of the host when asked to refresh. Concurrent
  //! refreshes of a host share one probe, bounded by `AGENTS_PROBE_TIMEOUT`.

  use crate::AppState;
  use crate::api::{error, internal};
  use axum::Json;
  use axum::extract::rejection::QueryRejection;
  use axum::extract::{Path, Query, State};
  use axum::http::{HeaderValue, StatusCode, header};
  use axum::response::{IntoResponse, Response};
  use hennery_kernel::hosts::{AgentsRecord, ReportedIn};
  use hennery_kernel::secret::{rfc3339, unix_now};
  use hennery_proto::frames::{CollectorFrame, HostFrame};
  use hennery_proto::rest::HostAgents;
  use serde::Deserialize;
  use std::collections::HashMap;
  use std::sync::Mutex;
  use std::time::Duration;
  use tokio::sync::watch;

  /// How long a probe of a host's agents waits for its answer: the host's
  /// own budget (15 s) and some time for the answer to arrive.
  pub const AGENTS_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

  /// The probes of agents under way, by host: a refresh that finds one joins
  /// it.
  #[derive(Default)]
  pub struct Refreshes(Mutex<HashMap<String, watch::Receiver<bool>>>);

  /// Removes its host's entry when its probe is done, however it ends.
  struct Running<'a> {
      refreshes: &'a Refreshes,
      host_id: String,
  }

  impl Drop for Running<'_> {
      fn drop(&mut self) {
          self.refreshes.0.lock().expect("refreshes lock").remove(&self.host_id);
      }
  }

  impl Refreshes {
      /// Probe `host_id`'s agents and store the answer, or join the probe
      /// already under way; return once it is done. The probe runs in a task
      /// of its own: a caller that goes away does not stop its answer from
      /// being stored. A host that is not connected, or cannot be probed, is
      /// not asked (`Hub::probe`).
      pub async fn refresh(state: &AppState, host_id: &str) {
          let mut done = {
              let mut running = state.agent_refreshes.0.lock().expect("refreshes lock");
              match running.get(host_id) {
                  Some(done) => done.clone(),
                  None => {
                      let (tx, done) = watch::channel(false);
                      running.insert(host_id.to_string(), done.clone());
                      let (state, host_id) = (state.clone(), host_id.to_string());
                      tokio::spawn(async move {
                          let running = Running {
                              refreshes: &state.agent_refreshes,
                              host_id: host_id.clone(),
                          };
                          probe_and_store(&state, &host_id).await;
                          // Gone before the waiters wake: a refresh after
                          // this one probes again.
                          drop(running);
                          let _ = tx.send(true);
                      });
                      done
                  }
              }
          };
          // The probe is bounded itself; this only bounds a waiter whose task
          // was lost.
          let _ = tokio::time::timeout(state.agents_probe_timeout, done.wait_for(|done| *done)).await;
      }
  }

  /// One `probe_agents`, its answer stored.
  async fn probe_and_store(state: &AppState, host_id: &str) {
      let request_id = uuid::Uuid::now_v7().to_string();
      let frame = CollectorFrame::ProbeAgents {
          request_id: request_id.clone(),
      };
      match state
          .hub
          .probe(host_id, &request_id, frame, state.agents_probe_timeout)
          .await
      {
          Ok(HostFrame::Agents { agents, runtime, .. }) => {
              if let Err(err) = state
                  .hosts
                  .record_agents(host_id, ReportedIn::Probe, agents.0, runtime.0, unix_now())
              {
                  tracing::warn!(%host_id, error = %err, "storing the host's agents failed");
              }
          }
          Ok(_) => tracing::warn!(%host_id, "the host answered probe_agents with another frame"),
          Err(err) => tracing::info!(%host_id, ?err, "no answer to probe_agents"),
      }
  }

  /// A host's report as the API shows it.
  fn host_agents(state: &AppState, host_id: String, record: AgentsRecord) -> HostAgents {
      HostAgents {
          live: state.hub.is_ready(&host_id),
          host_id,
          agents: record.agents,
          runtime: record.runtime,
          reported_at: record.reported_at.map(rfc3339),
          source: record.source,
      }
  }

  #[derive(Deserialize)]
  pub(crate) struct AgentsQuery {
      refresh: Option<String>,
  }

  /// `GET /api/hosts/{id}/agents[?refresh=1]`: the host's latest report,
  /// whatever its age and whether the host is connected; with `refresh=1`,
  /// after one probe of it (or that probe's timeout). 404 only for a host
  /// that is not the owner's, decided from the registry before any report is
  /// read or any probe sent. Never cached: a note can name a path on the
  /// host.
  pub(crate) async fn get_agents(
      State(state): State<AppState>,
      Path(host_id): Path<String>,
      query: Result<Query<AgentsQuery>, QueryRejection>,
  ) -> Response {
      let refresh = match query.as_ref().map(|Query(q)| q.refresh.as_deref()) {
          Ok(None | Some("0")) => false,
          Ok(Some("1")) => true,
          _ => return error(StatusCode::BAD_REQUEST, "invalid", "`refresh`, if given, is 0 or 1"),
      };
      match state.hosts.host(&host_id) {
          Ok(Some(_)) => {}
          Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such host"),
          Err(err) => return internal(err),
      }
      if refresh {
          Refreshes::refresh(&state, &host_id).await;
      }
      match state.hosts.agents(&host_id) {
          Ok(Some(record)) => {
              let mut response = Json(host_agents(&state, host_id, record)).into_response();
              response
                  .headers_mut()
                  .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
              response
          }
          Ok(None) => error(StatusCode::NOT_FOUND, "not_found", "no such host"),
          Err(err) => internal(err),
      }
  }
  ```

In `crates/hennery-sessions/src/hosts.rs`, replace:

  ```rust
              .route("/api/hosts", get(list_hosts))
              .route(
  ```

with:

  ```rust
              .route("/api/hosts", get(list_hosts))
              // Reads only, so no step-up: the probe a refresh runs is a
              // fixed set of read-only checks (plan 4d-B1-i).
              .route("/api/hosts/{id}/agents", get(crate::agents::get_agents))
              .route(
  ```

In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust

  pub mod api;
  ```

with:

  ```rust

  pub mod agents;
  pub mod api;
  ```

In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
      pub forgets: Arc<forget::InFlight>,
  }
  ```

with:

  ```rust
      pub forgets: Arc<forget::InFlight>,
      /// The probes of hosts' agents under way (plan 4d-B1-i).
      pub agent_refreshes: Arc<agents::Refreshes>,
      /// How long a probe of a host's agents waits for its answer.
      pub agents_probe_timeout: Duration,
  }
  ```

In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
              forgets: Arc::new(forget::InFlight::default()),
          }
  ```

with:

  ```rust
              forgets: Arc::new(forget::InFlight::default()),
              agent_refreshes: Arc::new(agents::Refreshes::default()),
              agents_probe_timeout: agents::AGENTS_PROBE_TIMEOUT,
          }
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
  use hennery_kernel::hosts::HelloCheck;
  use hennery_kernel::lifecycle::LifecycleHooks;
  use hennery_kernel::secret::{random_bytes, unix_now};
  use hennery_proto::frames::{AttachedSession, CollectorFrame, HostFrame, SessionBody};
  ```

with:

  ```rust
  use hennery_kernel::hosts::{HelloCheck, ReportedIn};
  use hennery_kernel::lifecycle::LifecycleHooks;
  use hennery_kernel::secret::{random_bytes, unix_now};
  use hennery_proto::frames::{AttachedSession, Capability, CollectorFrame, HostFrame, SessionBody};
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
          attached_sessions,
      }) = hello
  ```

with:

  ```rust
          attached_sessions,
          agents,
          runtime,
      }) = hello
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
                              after_reconcile(&state, &host_id, &workspace_roots, done, &tx, &mut reconcile_closes);
                          reconciled = true;
  ```

with:

  ```rust
                              after_reconcile(&state, &host_id, &workspace_roots, done, &tx, &mut reconcile_closes);
                          // Like the roots, only a reconciled connection's
                          // agents are stored, before the host is listed as
                          // connected; and only from a host that reports them
                          // (plan 4d-B1-i): an older one sends none.
                          if capabilities.has(Capability::ProbeAgents)
                              && let Err(err) = state.hosts.record_agents(
                                  &host_id,
                                  ReportedIn::Hello,
                                  agents.0.clone(),
                                  runtime.0.clone(),
                                  unix_now(),
                              )
                          {
                              tracing::warn!(%host_id, error = %err, "recording the host's agents failed");
                          }
                          reconciled = true;
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
              | HostFrame::SessionForgotten { .. }) => {
  ```

with:

  ```rust
              | HostFrame::SessionForgotten { .. }
              | HostFrame::Agents { .. }) => {
  ```

Run: `nix develop -c cargo run -p hennery-proto --bin gen`

- [ ] **Step 4: Run the tests, and see them pass**

Run: `nix develop -c cargo test -p hennery-proto --locked && nix develop -c cargo test -p hennery-testkit --test host_agents --test host_connection --test auth --locked`
Expected: all pass.

- [ ] **Step 5: Revert-probes**

Each on the whole plan's tree; each must make a test fail, then be restored. The lists name every test that failed (the commands: the task's test files, and the testkit's where the collector is involved).
- C1: in `crates/hennery-host/src/connection.rs`, replace `Capability::McpServers, ⏎ Capability::ProbeAgents,` with `Capability::McpServers,`. Fails: `a_host_announces_that_it_can_park_take_images_and_serve_projects`. Restore it.
- C2: in `crates/hennery-host/src/connection.rs`, replace `let Some(slot) = one_probe.try_start() else {` with `let _ = one_probe; ⏎ let Some(slot) = OneProbe::default().try_start() else {`. Fails: `a_host_probes_its_agents_one_probe_at_a_time`. Restore it.
- C3: in `crates/hennery-host/src/connection.rs`, replace `drop(slot); ⏎ });` with `std::mem::forget(slot); ⏎ });`. Fails: `a_host_probes_its_agents_one_probe_at_a_time`. Restore it.
- C4: in `crates/hennery-host/src/connection.rs`, replace `agents: cfg.static_agents(), ⏎ runtime: cfg.reported_runtime(), ⏎ };` with `agents: Vec::new(), ⏎ runtime: cfg.reported_runtime(), ⏎ };`. Fails: `a_host_reports_its_agents_and_runtime_in_hello`. Restore it.
- C5: in `crates/hennery-host/src/connection.rs`, replace `agents: cfg.static_agents(), ⏎ runtime: cfg.reported_runtime(), ⏎ };` with `agents: cfg.static_agents(), ⏎ runtime: None, ⏎ };`. Fails: `a_host_reports_its_agents_and_runtime_in_hello`. Restore it.
- C6: in `crates/hennery-host/src/connection.rs`, replace `let cwd = cfg.home.clone().unwrap_or_else(|| PathBuf::from("/"));` with `let cwd = PathBuf::from("/");`. Fails: `a_host_probes_its_agents_one_probe_at_a_time`. Restore it.
- C7: in `crates/hennery-host/src/connection.rs`, replace `runtime: MaybeRuntime(runtime), ⏎ }); ⏎ drop(slot);` with `runtime: MaybeRuntime(None), ⏎ }); ⏎ let _ = runtime; ⏎ drop(slot);`. Fails: `a_host_probes_its_agents_one_probe_at_a_time`. Restore it.
- C8: in `crates/hennery-host/src/connection.rs`, replace `self.runtime.clone().map(RuntimeInfo::bounded)` with `self.runtime.clone()`. Fails: `a_host_reports_its_agents_and_runtime_in_hello`. Restore it.
- W1: in `crates/hennery-sessions/src/ws.rs`, replace `if capabilities.has(Capability::ProbeAgents)` with `if (capabilities.has(Capability::ProbeAgents) || true)`. Fails: `an_older_hosts_hello_stores_nothing`. Restore it.
- W2: in `crates/hennery-sessions/src/ws.rs`, replace `if capabilities.has(Capability::ProbeAgents)` with `if (capabilities.has(Capability::ProbeAgents) && false)`. Fails: `a_reconciled_hello_stores_the_agents_of_a_host_that_reports_them`, `a_refresh_of_a_host_that_cannot_be_probed_answers_the_last_report`, `an_unanswered_or_refused_probe_leaves_the_last_report`. Restore it.
- S1: in `crates/hennery-sessions/src/agents.rs`, replace `running.insert(host_id.to_string(), done.clone());` with `let _ = &running;`. Fails: `concurrent_refreshes_share_one_probe`. Restore it.
- S2: in `crates/hennery-sessions/src/agents.rs`, replace `.record_agents(host_id, ReportedIn::Probe, agents.0, runtime.0, unix_now())` with `.record_agents(host_id, ReportedIn::Hello, agents.0, runtime.0, unix_now())`. Fails: `an_answer_after_its_caller_left_is_stored`, `concurrent_refreshes_share_one_probe`. Restore it.
- S3: in `crates/hennery-sessions/src/agents.rs`, replace `Ok(HostFrame::Agents { agents, runtime, .. }) => {` with `Ok(HostFrame::Agents { agents, runtime, .. }) if false => {`. Fails: `a_hostile_answer_is_bounded_before_it_is_stored`, `an_answer_after_its_caller_left_is_stored`, `concurrent_refreshes_share_one_probe`. Restore it.
- S4: in `crates/hennery-sessions/src/agents.rs`, replace `live: state.hub.is_ready(&host_id),` with `live: true,`. Fails: `a_hello_never_reconciled_stores_nothing`, `a_refresh_of_a_host_that_cannot_be_probed_answers_the_last_report`. Restore it.
- S5: in `crates/hennery-sessions/src/agents.rs`, replace `.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));` with `.insert(header::CACHE_CONTROL, HeaderValue::from_static("private"));`. Fails: `a_hello_never_reconciled_stores_nothing`, `a_hostile_answer_is_bounded_before_it_is_stored`, `a_reconciled_hello_stores_the_agents_of_a_host_that_reports_them`, `a_refresh_of_a_host_that_cannot_be_probed_answers_the_last_report`, `an_older_hosts_hello_stores_nothing`, `an_unknown_host_is_not_found_and_a_bad_refresh_is_invalid`, `concurrent_refreshes_share_one_probe`. Restore it.
- S6: in `crates/hennery-sessions/src/agents.rs`, replace `match state.hosts.host(&host_id) { ⏎ Ok(Some(_)) => {}` with `match state.hosts.host(&host_id).map(|_| Some(())) { ⏎ Ok(Some(_)) => {}`. Fails: `another_owners_host_is_not_found`. Restore it.
- S7: in `crates/hennery-sessions/src/agents.rs`, replace `Ok(Some("1")) => true,` with `Ok(Some(_)) => true,`. Fails: `an_unknown_host_is_not_found_and_a_bad_refresh_is_invalid`. Restore it.
- S8: in `crates/hennery-sessions/src/agents.rs`, replace `if refresh { ⏎ Refreshes::refresh(&state, &host_id).await;` with `if refresh && false { ⏎ Refreshes::refresh(&state, &host_id).await;`. Fails: `a_hostile_answer_is_bounded_before_it_is_stored`, `an_answer_after_its_caller_left_is_stored`, `an_unanswered_or_refused_probe_leaves_the_last_report`, `concurrent_refreshes_share_one_probe`. Restore it.
- S9: in `crates/hennery-sessions/src/agents.rs`, replace `reported_at: record.reported_at.map(rfc3339),` with `reported_at: None,`. Fails: `a_reconciled_hello_stores_the_agents_of_a_host_that_reports_them`. Restore it.
- S10: in `crates/hennery-sessions/src/agents.rs`, replace `drop(running); ⏎ let _ = tx.send(true);` with `let _ = tx.send(true); ⏎ drop(running);`. not caught, as recorded ("Not tested here", the review's A8): the order decides only whether a refresh issued in that instant joins the finished probe. Restore it.

- [ ] **Step 6: Check the timing-sensitive tests under load**

Run the testkit's `host_agents` and `host_connection` test binaries four at a time, as in Task 2. Expected: all pass, every run.

- [ ] **Step 7: The full checks**

Run the five commands of "Global Constraints", and the web checks. Expected: all pass; **1589 tests**.

- [ ] **Step 8: Commit**

```bash
git add crates schema web/src/generated
git commit -m "feat(sessions): GET /api/hosts/{id}/agents, from hello and probe_agents"
```

### Task 4: The binary: the managed set or `--agent`, and doctor's knowledge

**Files:**
- Create: `crates/hennery/src/host_agents.rs`
- Modify: `crates/hennery/src/main.rs`, `runtime.rs`; `doctor/mod.rs` (re-exports), `doctor/agents.rs` (`status_of` `pub(crate)`; doc comments), `doctor/runtime.rs`, `doctor/spawn.rs` (doc comments only)
- Test: `crates/hennery/tests/cli.rs`; `host_agents.rs`'s unit tests

**Interfaces:**
- Consumes: Tasks 2 and 3.
- Produces: `host_agents::{AgentSetup, DoctorChecks}`; `runtime::default_agents(data_dir, mirrors) -> agents::Prepared`.
- Unchanged: every doctor check, signature and its behaviour (decision 4).

- [ ] **Step 1: Write the failing test**

Through the binary: a host given `--agent` commands reports them as `given` in `hello`, with a `given` runtime, and a probe asks no CLI.

In `crates/hennery/tests/cli.rs`, replace:

  ```rust

  /// `path`'s text, or nothing while it is not there.
  ```

with:

  ```rust

  /// Plan 4d-B1-i through the binary: a host on the set it pins reports its
  /// agents in `hello`, and a refresh checks them live: each adapter as a
  /// session's is started, asked `initialize`, and each bundled CLI asked
  /// whether it is logged in, by its exit status alone.
  #[test]
  fn a_hosts_agents_are_reported_and_checked_through_the_binary() {
      use std::os::unix::fs::PermissionsExt;
      let dir = scratch_dir("agents");
      let _cleanup = RemoveDir(dir.clone());
      let data = dir.join("data");
      let host = data.join("host");
      let selection = hennery_host::runtime::install::Selection::pinned(&Default::default()).unwrap();
      let entries: Vec<(&str, &str)> = selection
          .adapters
          .iter()
          .map(|a| (a.name.as_str(), a.entry.as_str()))
          .collect();
      // The runtime's `node`: an adapter answers `initialize`; Codex's
      // bundled CLI (`node codex.js login status`) is logged out.
      let node = r#"case "$2" in login) exit 1;; esac
  read line
  printf '%s\n' '{"jsonrpc":"2.0","id":0,"result":{"agentInfo":{"version":"9.9.9"},"agentCapabilities":{"promptCapabilities":{"image":true}}}}'
  exec sleep 30"#;
      let set = fabricate_set(&host, &selection.set_id(), &selection.runtime_name(), node, &entries);
      link_set(&host, "current", &selection.set_id());
      // Claude's bundled CLI is logged in.
      let platform = hennery_host::runtime::manifest::Platform::current()
          .unwrap()
          .key()
          .to_string();
      let claude = set.join(format!(
          "claude/node_modules/@anthropic-ai/claude-agent-sdk-{platform}/claude"
      ));
      std::fs::create_dir_all(claude.parent().unwrap()).unwrap();
      std::fs::write(&claude, "#!/bin/sh\nexit 0\n").unwrap();
      std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
      let codex = set.join("codex/node_modules/@openai/codex/bin/codex.js");
      std::fs::create_dir_all(codex.parent().unwrap()).unwrap();
      std::fs::write(&codex, "// codex\n").unwrap();

      // The probe runs each CLI in the host's home (the review's A5): a
      // scratch one that exists, never the account of whoever runs the tests.
      let home = dir.join("home");
      std::fs::create_dir_all(&home).unwrap();
      let mut command = hennery();
      command.env("HOME", &home);

      let log = dir.join("up.log");
      let mut up = up_logging_to_with(command, &data, &log, &[]);
      let listen = up.listening();
      let session = sign_in(&mut up, &listen, &data.join("collector"));
      let mut host_id = String::new();
      up.wait_until("the host connected", || {
          let Some(serde_json::Value::Array(hosts)) = get_json(&listen, "/api/hosts", &session) else {
              return false;
          };
          match hosts.first() {
              Some(h) if h["connected"] == true => {
                  host_id = h["host_id"].as_str().unwrap().to_string();
                  true
              }
              _ => false,
          }
      });
      let path = format!("/api/hosts/{host_id}/agents");
      let hello = get_json(&listen, &path, &session).expect("the agents");
      assert_eq!(hello["source"], "hello", "{hello}");
      assert_eq!(hello["live"], true, "{hello}");
      assert_eq!(
          hello["runtime"],
          serde_json::json!({"source": "managed", "set_id": selection.set_id(), "pinned": true, "held": false}),
          "{hello}"
      );
      let agent = |report: &serde_json::Value, name: &str| {
          report["agents"]
              .as_array()
              .unwrap()
              .iter()
              .find(|a| a["agent"] == name)
              .unwrap_or_else(|| panic!("no {name} in {report}"))
              .clone()
      };
      for name in ["claude", "codex"] {
          assert_eq!(
              agent(&hello, name),
              serde_json::json!({"agent": name, "available": true, "auth": "unknown", "cli": "bundled", "adapter_version": "9.9.9"}),
          );
      }
      let mut live = serde_json::Value::Null;
      up.wait_until("a probe's report", || {
          live = get_json(&listen, &format!("{path}?refresh=1"), &session).unwrap_or_default();
          live["source"] == "probe"
      });
      let claude = agent(&live, "claude");
      assert_eq!(
          (&claude["available"], &claude["auth"], &claude["images"]),
          (
              &serde_json::json!(true),
              &serde_json::json!("ok"),
              &serde_json::json!(true)
          ),
          "{live}"
      );
      let codex = agent(&live, "codex");
      assert_eq!(
          (&codex["available"], &codex["auth"]),
          (&serde_json::json!(true), &serde_json::json!("missing")),
          "{live}"
      );
  }

  /// `path`'s text, or nothing while it is not there.
  ```

- [ ] **Step 2: Run it, and see it fail**

Run: `nix develop -c cargo test -p hennery --test cli --locked a_hosts_agents`
Expected: FAIL: `a_hosts_agents_are_reported_and_checked_through_the_binary`. The host reports its agents as `given`, with no `runtime`, because `host run` does not configure them yet.

- [ ] **Step 3: Write the implementation**

In `crates/hennery/src/doctor/agents.rs`, replace:

  ```rust
  /// the set's Node.
  ```

with:

  ```rust
  /// the set's Node. Also called by `crate::host_agents`'s probe (plan
  /// 4d-B1-i), which keeps only its `program` and `args`.
  ```

In `crates/hennery/src/doctor/agents.rs`, replace:

  ```rust
  fn status_of(agent: &str) -> Option<(&'static [&'static str], &'static str)> {
  ```

with:

  ```rust
  /// Also called by `crate::host_agents`'s probe (plan 4d-B1-i).
  pub(crate) fn status_of(agent: &str) -> Option<(&'static [&'static str], &'static str)> {
  ```

In `crates/hennery/src/doctor/mod.rs`, replace:

  ```rust
  mod tests;

  ```

with:

  ```rust
  mod tests;

  // What `probe_agents` takes of doctor's knowledge (plan 4d-B1-i,
  // `crate::host_agents`): check 4's CLI and question. `pub(crate)` only;
  // `Cli` is not re-exported, so the host never holds one to `run`.
  pub(crate) use agents::{bundled_cli, status_of};
  pub(crate) use runtime::writable_by_others;

  ```

In `crates/hennery/src/doctor/runtime.rs`, replace:

  ```rust
  /// group or others may, or someone else owns it. Agents run what is there.
  pub fn writable_by_others(path: &Path, uid: u32) -> bool {
  ```

with:

  ```rust
  /// group or others may, or someone else owns it. Agents run what is there.
  /// Also called by `crate::host_agents`'s probe (plan 4d-B1-i).
  pub fn writable_by_others(path: &Path, uid: u32) -> bool {
  ```

In `crates/hennery/src/doctor/spawn.rs`, replace:

  ```rust
  /// its own (`node <set>/codex/…/codex.js`).
  ```

with:

  ```rust
  /// its own (`node <set>/codex/…/codex.js`). `crate::host_agents`'s probe
  /// (plan 4d-B1-i) reads the fields of the one `bundled_cli` returns and
  /// never calls `run`, which starts it in doctor's own groups.
  ```

Create `crates/hennery/src/host_agents.rs`:

  ```rust
  //! `host run`'s agents and what it reports of them (plan 4d-B1-i): the
  //! managed set's or the `--agent` commands, the static view `hello` gives,
  //! and doctor's knowledge as `probe_agents` uses it: which CLI each agent
  //! runs and how to ask it whether it is logged in (check 4,
  //! `doctor::status_of`). The host runs the question itself, in its own
  //! guarded groups (`hennery_host::adapter::exit_status`): nothing here
  //! spawns anything, so doctor's process groups and signal handler are never
  //! involved.

  use crate::doctor::{bundled_cli, status_of, writable_by_others};
  use crate::runtime::MirrorArgs;
  use hennery_host::availability::{AgentChecks, LoginCheck};
  use hennery_host::runtime::agents::{self, CLI_VARS};
  use hennery_host::runtime::install::InstalledSet;
  use hennery_host::{AgentCommand, HostConfig};
  use std::collections::{BTreeMap, HashMap};
  use std::path::{Path, PathBuf};
  use std::sync::Arc;

  /// Where `host run`'s agents come from.
  pub enum AgentSetup {
      /// No `--agent`: the installed set, as `prepare` left it.
      Managed(Box<agents::Prepared>),
      /// `--agent` commands, as parsed: nothing managed, no CLI known.
      Given(HashMap<String, AgentCommand>),
  }

  impl AgentSetup {
      /// `given` if there are any, else the pinned set, installed first if it
      /// is not current (distribution spec §3.2).
      pub async fn new(data_dir: &Path, given: Vec<(String, AgentCommand)>, mirrors: &MirrorArgs) -> Self {
          if given.is_empty() {
              Self::Managed(Box::new(crate::runtime::default_agents(data_dir, mirrors).await))
          } else {
              Self::Given(given.into_iter().collect())
          }
      }

      /// Give `cfg` its agents, what `hello` reports of them, and the checks
      /// `probe_agents` runs. The file returned holds the managed set in use
      /// for as long as it is kept.
      pub fn configure(self, cfg: &mut HostConfig) -> Option<std::fs::File> {
          match self {
              Self::Managed(prepared) => {
                  let agents::Prepared {
                      agents,
                      in_use,
                      set,
                      runtime,
                  } = *prepared;
                  cfg.checks = Arc::new(DoctorChecks::new(set, &agents.agents));
                  cfg.agents = agents.agents;
                  cfg.profiles = agents.profiles;
                  cfg.codex_app_server = agents.codex_app_server;
                  cfg.agent_infos = agents.infos;
                  cfg.runtime = runtime;
                  in_use
              }
              Self::Given(given) => {
                  cfg.checks = Arc::new(DoctorChecks::new(None, &given));
                  // A given command is a generic agent (ACP core §6): no
                  // profile, and no app-server, so a Codex forget takes the
                  // fallback.
                  cfg.agents = given;
                  cfg.profiles = HashMap::new();
                  cfg.codex_app_server = None;
                  cfg.agent_infos = Vec::new();
                  cfg.runtime = Some(hennery_host::availability::given_runtime());
                  None
              }
          }
      }
  }

  /// The CLIs of a host's agents, as the host was started.
  #[derive(Debug, Clone)]
  pub struct DoctorChecks {
      /// The set the managed agents launch from; `None` for `--agent`.
      set: Option<InstalledSet>,
      /// Each overridden agent's CLI, as its command passes it to the
      /// adapter (`CLAUDE_CODE_EXECUTABLE`, `CODEX_PATH`).
      overrides: BTreeMap<String, PathBuf>,
      /// The host's user: a CLI others can change is not run (doctor's O1).
      uid: u32,
  }

  impl DoctorChecks {
      /// For a managed host: its set, and the overrides its agents' commands
      /// carry. For a host given `--agent` commands: `set` is `None` and no
      /// CLI is known, as doctor knows none for them.
      pub fn new(set: Option<InstalledSet>, agents: &HashMap<String, AgentCommand>) -> Self {
          let overrides = match &set {
              Some(_) => agents
                  .iter()
                  .filter_map(|(name, command)| {
                      let var = CLI_VARS.iter().find(|(agent, _)| agent == name)?.1;
                      let (_, path) = command.env.iter().find(|(k, _)| k == var)?;
                      Some((name.clone(), PathBuf::from(path)))
                  })
                  .collect(),
              None => BTreeMap::new(),
          };
          Self {
              set,
              overrides,
              // SAFETY: geteuid(2) has no failure mode.
              uid: unsafe { libc::geteuid() },
          }
      }

      /// `agent`'s CLI, its program and the arguments before its own: its
      /// override, else the set's bundled one. The `Cli` doctor returns is
      /// held only to move its fields out: its `run` starts it in doctor's own
      /// groups (hazard (a)), and a test forbids any process API here.
      fn cli(&self, agent: &str) -> Option<(PathBuf, Vec<String>)> {
          let set = self.set.as_ref()?;
          match self.overrides.get(agent) {
              Some(path) => Some((path.clone(), Vec::new())),
              None => bundled_cli(set, agent).map(|cli| (cli.program, cli.args)),
          }
      }
  }

  impl AgentChecks for DoctorChecks {
      /// Check 4's question: `claude auth status` / `codex login status`,
      /// with the auto-updater off, as doctor asks it.
      fn login(&self, agent: &str) -> LoginCheck {
          let (Some((program, before)), Some((args, _))) = (self.cli(agent), status_of(agent)) else {
              return LoginCheck::NotAsked(None);
          };
          let program = program.as_path();
          if let Some(writable) = [program, program.parent().unwrap_or(program)]
              .into_iter()
              .find(|p| writable_by_others(p, self.uid))
          {
              return LoginCheck::NotAsked(Some(format!(
                  "its CLI is not run: other users can write to {}",
                  writable.display()
              )));
          }
          LoginCheck::Ask(AgentCommand {
              program: program.to_string_lossy().into_owned(),
              args: before.into_iter().chain(args.iter().map(|a| a.to_string())).collect(),
              env: vec![("DISABLE_AUTOUPDATER".to_string(), "1".to_string())],
          })
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use hennery_host::identity::HostKey;
      use hennery_host::runtime::install::{LAYOUT, RecordAdapter, SetRecord};
      use std::os::unix::fs::PermissionsExt;

      const PLATFORM: &str = "test-platform";

      /// A set under `dir` with both agents; `bundled` names the agents whose
      /// bundled CLI is there.
      fn set(dir: &Path, bundled: &[&str]) -> InstalledSet {
          let path = dir.join("sets/s1");
          let node = dir.join("runtimes/r/bin/node");
          let mut adapters = std::collections::BTreeMap::new();
          for agent in ["claude", "codex"] {
              adapters.insert(
                  agent.to_string(),
                  RecordAdapter {
                      version: "1.0.0".into(),
                      entry: "index.js".into(),
                      cli_skipped: false,
                  },
              );
          }
          let tree = |agent: &str| path.join(agent).join("node_modules");
          for agent in bundled {
              let file = match *agent {
                  "claude" => tree("claude").join(format!("@anthropic-ai/claude-agent-sdk-{PLATFORM}/claude")),
                  _ => tree("codex").join("@openai/codex/bin/codex.js"),
              };
              std::fs::create_dir_all(file.parent().unwrap()).unwrap();
              std::fs::write(&file, "").unwrap();
          }
          InstalledSet {
              id: "s1".into(),
              path: path.clone(),
              record: SetRecord {
                  layout: LAYOUT,
                  id: "s1".into(),
                  manifest_hash: "0".repeat(64),
                  platform: PLATFORM.into(),
                  runtime: "r".into(),
                  adapters,
              },
              node,
          }
      }

      fn command(env: &[(&str, &str)]) -> AgentCommand {
          AgentCommand {
              program: "/node".into(),
              args: vec!["/entry".into()],
              env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
          }
      }

      fn asked(check: LoginCheck) -> AgentCommand {
          match check {
              LoginCheck::Ask(command) => command,
              other => panic!("expected a question, got {other:?}"),
          }
      }

      fn autoupdater_off() -> Vec<(String, String)> {
          vec![("DISABLE_AUTOUPDATER".to_string(), "1".to_string())]
      }

      #[test]
      fn a_bundled_cli_is_asked_its_status_with_the_auto_updater_off() {
          let dir = tempfile::tempdir().unwrap();
          let set = set(dir.path(), &["claude", "codex"]);
          let agents = HashMap::from([
              ("claude".to_string(), command(&[])),
              ("codex".to_string(), command(&[])),
          ]);
          let checks = DoctorChecks::new(Some(set.clone()), &agents);
          let claude = asked(checks.login("claude"));
          assert_eq!(
              Path::new(&claude.program),
              set.path.join(format!(
                  "claude/node_modules/@anthropic-ai/claude-agent-sdk-{PLATFORM}/claude"
              ))
          );
          assert_eq!(claude.args, ["auth", "status"]);
          assert_eq!(claude.env, autoupdater_off());
          let codex = asked(checks.login("codex"));
          assert_eq!(Path::new(&codex.program), set.node);
          assert_eq!(
              codex.args,
              [
                  set.path
                      .join("codex/node_modules/@openai/codex/bin/codex.js")
                      .to_string_lossy()
                      .into_owned(),
                  "login".into(),
                  "status".into()
              ]
          );
          assert_eq!(codex.env, autoupdater_off());
      }

      /// The CLI the agent's command passes its adapter, not the bundled one.
      #[test]
      fn an_overridden_cli_is_the_one_asked() {
          let dir = tempfile::tempdir().unwrap();
          let set = set(dir.path(), &["claude"]);
          let agents = HashMap::from([(
              "claude".to_string(),
              command(&[("CLAUDE_CODE_EXECUTABLE", "/opt/claude/bin/claude")]),
          )]);
          let claude = asked(DoctorChecks::new(Some(set), &agents).login("claude"));
          assert_eq!(claude.program, "/opt/claude/bin/claude");
          assert_eq!(claude.args, ["auth", "status"]);
      }

      /// No CLI known: an `--agent` command, an agent doctor does not know,
      /// or no bundled CLI and no override.
      #[test]
      fn with_no_known_cli_nothing_is_asked() {
          let dir = tempfile::tempdir().unwrap();
          let agents = HashMap::from([
              ("claude".to_string(), command(&[])),
              ("gemini".to_string(), command(&[])),
          ]);
          let given = DoctorChecks::new(None, &agents);
          assert!(matches!(given.login("claude"), LoginCheck::NotAsked(None)));
          let managed = DoctorChecks::new(Some(set(dir.path(), &[])), &agents);
          assert!(matches!(managed.login("claude"), LoginCheck::NotAsked(None)));
          assert!(matches!(managed.login("codex"), LoginCheck::NotAsked(None)));
          assert!(matches!(managed.login("gemini"), LoginCheck::NotAsked(None)));
      }

      /// Doctor's O1: a CLI other users can change is not run, and the note
      /// names where.
      #[test]
      fn a_cli_others_can_write_to_is_not_run() {
          let dir = tempfile::tempdir().unwrap();
          let open = dir.path().join("open");
          std::fs::create_dir(&open).unwrap();
          std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
          let cli = open.join("claude");
          std::fs::write(&cli, "").unwrap();
          let agents = HashMap::from([(
              "claude".to_string(),
              command(&[("CLAUDE_CODE_EXECUTABLE", cli.to_str().unwrap())]),
          )]);
          let checks = DoctorChecks::new(Some(set(dir.path(), &[])), &agents);
          match checks.login("claude") {
              LoginCheck::NotAsked(Some(note)) => {
                  assert!(
                      note.contains("other users can write to") && note.contains("open"),
                      "{note}"
                  )
              }
              other => panic!("expected a refusal, got {other:?}"),
          }
      }

      fn host_config() -> HostConfig {
          HostConfig::new(
              "ws://127.0.0.1:1/api/hosts/ws",
              "host-1",
              HostKey::from_seed([1; 32]),
              PathBuf::from("/nonexistent"),
          )
      }

      /// Hazard (a), the review's A2: this module only says which program to
      /// run; the host runs it, in its own guarded group. Nothing here may
      /// reach doctor's `Cli::run` (doctor's own groups) or start a process.
      #[test]
      fn nothing_here_reaches_a_process_api() {
          let source = include_str!("host_agents.rs");
          let code = &source[..source.find("#[cfg(test)]").expect("a test module")];
          // The scan covers the module's code: a test module placed earlier
          // would shrink it silently (the second re-confirmation's note).
          assert!(code.contains("fn configure"), "the scan stops before the code");
          // `run(` catches `Cli::run(&cli, …)` and doctor's `run`, as a method
          // or a function; `run_bounded` and `spawn(` doctor's other spawners
          // (the re-confirmation's R1).
          for banned in [
              "run(",
              "run_bounded",
              "spawn(",
              "kill_all",
              "spawn::",
              "Command::new",
              "std::process",
              "tokio::process",
          ] {
              assert!(!code.contains(banned), "host_agents.rs uses {banned}");
          }
      }

      /// `--agent` commands: reported as given, nothing managed, no CLI asked.
      #[test]
      fn given_agents_are_configured_as_given() {
          let mut cfg = host_config();
          let given = HashMap::from([("claude".to_string(), command(&[]))]);
          let in_use = AgentSetup::Given(given).configure(&mut cfg);
          assert!(in_use.is_none());
          assert_eq!(cfg.agents.keys().collect::<Vec<_>>(), ["claude"]);
          assert!(cfg.agent_infos.is_empty());
          // As the wire says it: `given`, and no managed field.
          assert_eq!(
              serde_json::to_value(cfg.runtime.as_ref().unwrap()).unwrap(),
              serde_json::json!({"source": "given"})
          );
          assert!(matches!(cfg.checks.login("claude"), LoginCheck::NotAsked(None)));
      }

      /// The managed set: its agents, their infos, its runtime and its CLIs.
      #[test]
      fn a_managed_set_is_configured_with_what_it_reports() {
          let dir = tempfile::tempdir().unwrap();
          let set = set(dir.path(), &["claude"]);
          let mut cfg = host_config();
          let file = std::fs::File::open(dir.path()).unwrap();
          let mut prepared = agents::Prepared {
              set: Some(set.clone()),
              in_use: Some(file),
              ..Default::default()
          };
          prepared.agents.agents.insert("claude".into(), command(&[]));
          prepared.agents.infos = hennery_host::availability::static_agents(&prepared.agents.agents, &[]);
          let infos = prepared.agents.infos.clone();
          // Any runtime: it is passed on as it is.
          prepared.runtime = Some(hennery_host::availability::given_runtime());
          let runtime = prepared.runtime.clone();
          let in_use = AgentSetup::Managed(Box::new(prepared)).configure(&mut cfg);
          assert!(in_use.is_some());
          assert_eq!(cfg.agents.keys().collect::<Vec<_>>(), ["claude"]);
          assert_eq!(cfg.agent_infos, infos);
          assert_eq!(cfg.runtime, runtime);
          assert!(matches!(cfg.checks.login("claude"), LoginCheck::Ask(_)));
      }
  }
  ```

In `crates/hennery/src/main.rs`, replace:

  ```rust
  mod healthcheck;
  mod inherit;
  ```

with:

  ```rust
  mod healthcheck;
  mod host_agents;
  mod inherit;
  ```

In `crates/hennery/src/main.rs`, replace:

  ```rust
      let (agents, profiles, codex_app_server, _set_in_use) = if args.agents.is_empty() {
          runtime::default_agents(&data_dir, &args.mirrors).await
      } else {
          (args.agents.into_iter().collect(), Default::default(), None, None)
      };
  ```

with:

  ```rust
      let agents = host_agents::AgentSetup::new(&data_dir, args.agents, &args.mirrors).await;
  ```

In `crates/hennery/src/main.rs`, replace:

  ```rust
      cfg.agents = agents;
      cfg.profiles = profiles;
      cfg.codex_app_server = codex_app_server;
  ```

with:

  ```rust
      let _set_in_use = agents.configure(&mut cfg);
  ```

In `crates/hennery/src/runtime.rs`, replace:

  ```rust
  use hennery_host::AgentCommand;
  use hennery_host::runtime::agents::{self, UseCli};
  use hennery_host::runtime::download::Sources;
  use hennery_host::runtime::install::{self, Installed, InstalledSet, Layout, Selection};
  use std::collections::HashMap;
  ```

with:

  ```rust
  use hennery_host::runtime::agents::{self, UseCli};
  use hennery_host::runtime::download::Sources;
  use hennery_host::runtime::install::{self, Installed, InstalledSet, Layout, Selection};
  ```

In `crates/hennery/src/runtime.rs`, replace:

  ```rust
  /// the Codex CLI a forget runs `app-server` from (plan 9d decision 9). The
  /// file returned holds that set in use for as long as it is kept.
  pub async fn default_agents(
      data_dir: &Path,
      mirrors: &MirrorArgs,
  ) -> (
      HashMap<String, AgentCommand>,
      HashMap<String, hennery_host::profile::Profile>,
      Option<AgentCommand>,
      Option<std::fs::File>,
  ) {
  ```

with:

  ```rust
  /// the Codex CLI a forget runs `app-server` from (plan 9d decision 9). Its
  /// `in_use` holds that set in use for as long as it is kept; its `set`,
  /// `agents.infos` and `runtime` are what the host reports of them (plan
  /// 4d-B1-i).
  pub async fn default_agents(data_dir: &Path, mirrors: &MirrorArgs) -> agents::Prepared {
  ```

In `crates/hennery/src/runtime.rs`, replace:

  ```rust
      (
          prepared.agents.agents,
          prepared.agents.profiles,
          prepared.agents.codex_app_server,
          prepared.in_use,
      )
  ```

with:

  ```rust
      prepared
  ```

- [ ] **Step 4: Run the tests, and see them pass**

Run: `nix develop -c cargo test -p hennery --locked`
Expected: all pass.

- [ ] **Step 5: Revert-probes**

Each on the whole plan's tree; each must make a test fail, then be restored. The lists name every test that failed (the commands: the task's test files, and the testkit's where the collector is involved).
- B1: in `crates/hennery/src/host_agents.rs`, replace `env: vec![("DISABLE_AUTOUPDATER".to_string(), "1".to_string())],` with `env: Vec::new(),`. Fails: `a_bundled_cli_is_asked_its_status_with_the_auto_updater_off`. Restore it.
- B2: in `crates/hennery/src/host_agents.rs`, replace `.find(|p| writable_by_others(p, self.uid))` with `.find(|p| writable_by_others(p, self.uid) && false)`. Fails: `a_cli_others_can_write_to_is_not_run`. Restore it.
- B3: in `crates/hennery/src/host_agents.rs`, replace `Some(path) => Some((path.clone(), Vec::new())),` with `Some(_) => bundled_cli(set, agent).map(|cli| (cli.program, cli.args)),`. Fails: `a_cli_others_can_write_to_is_not_run`, `an_overridden_cli_is_the_one_asked`. Restore it.
- B4: in `crates/hennery/src/host_agents.rs`, replace `cfg.runtime = Some(hennery_host::availability::given_runtime());` with `cfg.runtime = None;`. Fails: `given_agents_are_configured_as_given`. Restore it.
- B5: in `crates/hennery/src/host_agents.rs`, replace `cfg.agent_infos = agents.infos;` with `cfg.agent_infos = Vec::new(); ⏎ let _ = agents.infos;`. Fails: `a_managed_set_is_configured_with_what_it_reports`. Restore it.
- B6: in `crates/hennery/src/host_agents.rs`, replace `cfg.runtime = runtime;` with `cfg.runtime = None; ⏎ let _ = runtime;`. Fails: `a_managed_set_is_configured_with_what_it_reports`. Restore it.
- B7: in `crates/hennery/src/host_agents.rs`, replace `.chain(args.iter().map(|a| a.to_string()))` with `.chain(args.iter().take(1).map(|a| a.to_string()))`. Fails: `a_bundled_cli_is_asked_its_status_with_the_auto_updater_off`, `an_overridden_cli_is_the_one_asked`. Restore it.
- B8: in `crates/hennery/src/host_agents.rs`, replace `pub fn new(set: Option<InstalledSet>, agents: &HashMap<String, AgentCommand>) -> Self {` with `pub fn new(set: Option<InstalledSet>, agents: &HashMap<String, AgentCommand>) -> Self { ⏎ let _ = std::process::id();`. Fails: `nothing_here_reaches_a_process_api`. Restore it.

- [ ] **Step 6: The full checks**

Run the five commands of "Global Constraints". Expected: all pass; **1597 tests**.

- [ ] **Step 7: Commit**

```bash
git add crates/hennery
git commit -m "feat(hennery): host run reports its agents and asks their CLIs as doctor does"
```

### Task 5: The spec write-back

**Files:**
- Modify: `docs/specs/2026-09-26-acp-core-design.md` (§3.3, §6), `docs/specs/2026-09-26-kernel-design.md` (§1.1, §4.3, §8), `docs/specs/2026-09-26-distribution-design.md` (§7)

- [ ] **Step 1: Write the specs' new text**

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

  ```markdown
  | `probe_agents` | — | `agents{…}` (same shape as in `hello`) |
  ```

with:

  ```markdown
  | `probe_agents` | — | `agents{agents[], runtime?}` (the shape of `hello`'s) \| `error{busy}`. Only to a host with the `probe_agents` capability; it names nothing: the host runs fixed read-only checks on its own agents (§6, plan 4d-B1-i). |
  ```

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

  ```markdown
  since plan 5b: probes, answered only by the connection they went out on.

  ```

with:

  ```markdown
  since plan 5b: probes, answered only by the connection they went out on.
  `hello.agents[]`, `hello.runtime` and `probe_agents` / `agents` are on the
  wire since plan 4d-B1-i.

  ```

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

  ```markdown
    (resolving typed paths, kernel spec §5.4). The collector
  ```

with:

  ```markdown
    (resolving typed paths, kernel spec §5.4), `probe_agents` (checking its
    agents live, §6; such a host reports them in `hello` too). The collector
  ```

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

  ```markdown
    New-session pickers work before the first session on a host exists.
  - `workspace_roots[]`: from the host's config (§7).
  ```

with:

  ```markdown
    New-session pickers work before the first session on a host exists.
    *Built so far (plan 4d-B1-i):* per agent `{agent, available, auth, cli,
    adapter_version?, images?, note?}`, with `runtime?` beside the list (§6);
    no `catalog` yet. Read leniently, as `capabilities` are: an entry this
    build cannot read is skipped, a runtime it cannot read is absent, and an
    older host sends neither. The collector stores them only from a
    reconciled connection of a host with the `probe_agents` capability.
  - `workspace_roots[]`: from the host's config (§7).
  ```

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

  ```markdown
  `available` = the adapter can be launched; `auth` = `ok | missing | unknown`.
  Auth is taken, in order, from the adapter's `_auth/status_update` notification
  ```

with:

  ```markdown
  `available` = the adapter can be launched; `auth` = `ok | missing | unknown`.
  *Built so far (plan 4d-B1-i):* `hello` gives the static view: `available`
  means launchable as configured, `auth` is `unknown` and `images` absent. A
  `probe_agents` gives the live one, within 15 s for every check at once:
  each adapter is started exactly as a session's is (its own guarded group,
  the host's environment) and sent `initialize` alone, then killed;
  `available` means it answered, `adapter_version` is its `agentInfo.version`
  when readable, and `images` its `promptCapabilities.image`. A client hides
  images only when `images` is `false`; absent, it allows them, and
  `error{images_unsupported}` stays the guard. `auth` comes from the CLI
  alone, as doctor's check 4 asks it (exit 0 `ok`, another exit code
  `missing`; ended by a signal, no answer, or no known CLI `unknown`); only the
  exit status is kept, never
  a byte the CLI wrote, and the `_auth/status_update` notification is not
  read yet. `cli` says which CLI the agent runs: `bundled` (the set's),
  `override` (`--use-cli`) or `given` (`host run --agent`, of which hennery
  manages nothing). `runtime` says where the agents come from: `managed`
  (with `set_id`, `pinned` and `held`) or `given`. `note` is the host's own
  words, never an agent's. Both ends bound a report (16 agents, names of 64
  bytes of printable ASCII, versions of 64 bytes of `[0-9A-Za-z.+-]`, notes
  of 512 bytes with control and format characters replaced). One probe runs
  at a time on a host; another is answered `error{busy}`.
  Auth is taken, in order, from the adapter's `_auth/status_update` notification
  ```

In `docs/specs/2026-09-26-distribution-design.md`, replace:

  ```markdown
  them to the collector, so the Hosts view shows them without a terminal.

  ```

with:

  ```markdown
  them to the collector, so the Hosts view shows them without a terminal.
  *Built so far (plan 4d-B1-i):* checks 3 and 4, in the host's own guarded
  groups and environment, never doctor's (ACP core §6); check 12 as
  `runtime.pinned`. Check 4 asks the CLI only. Checks 9 and 13 come with the
  doctor report (plan 4d-B1-ii).

  ```

In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
  `workspace_roots` (`probe_agents`) and `last_doctor`. `push_subscriptions` and
  ```

with:

  ```markdown
  `workspace_roots` (`probe_agents`) and `last_doctor`. Kernel migration 12
  (plan 4d-B1-i) adds `agents` (the latest report, `{agents, runtime?}`),
  `agents_reported_at` (the collector's clock) and `agents_source` (`none`,
  `hello` or `probe`). `push_subscriptions` and
  ```

In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
    is the last accepted `hello`, not liveness; no rename or default hat yet.
  - One live connection per host (ACP core §3.5).
  ```

with:

  ```markdown
    is the last accepted `hello`, not liveness; no rename or default hat yet.
    A reconciled connection's `hello` from a host with the `probe_agents`
    capability, and each `probe_agents` answer, replace the host's agent
    report, bounded again by the collector whatever the host sent (plan
    4d-B1-i); a revoked host's is not stored.
  - One live connection per host (ACP core §3.5).
  ```

In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
  | `GET /api/hosts/ws` | Host WebSocket (ACP core) |
  | `GET/POST /api/hats`, `PATCH /api/hats/{id}` | Hats |
  ```

with:

  ```markdown
  | `GET /api/hosts/ws` | Host WebSocket (ACP core) |
  | `GET /api/hosts/{id}/agents[?refresh=1]` | The host's latest agent report; with `refresh=1`, after one probe (ACP core §6) |
  | `GET/POST /api/hats`, `PATCH /api/hats/{id}` | Hats |
  ```

In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
  `HostItem`, or 404.

  ```

with:

  ```markdown
  `HostItem`, or 404.

  `GET /api/hosts/{id}/agents` answers `HostAgents {host_id, agents[],
  runtime?, reported_at?, source: none | hello | probe, live}` from the store,
  whatever the report's age, `no-store`: `source: none` until the host reports,
  and `live` when it is connected and reconciled now. With `refresh=1` it first
  sends one `probe_agents` to a connected host that has the capability and
  waits for it, at most 20 s; concurrent refreshes of a host share that probe,
  and an answer that arrives after its caller left is stored all the same. A
  probe that fails leaves the last report. No step-up: the probe is a fixed
  set of read-only checks. 404 `not_found` for a host that is not the
  owner's, decided before anything is probed; 400 `invalid` for a `refresh`
  other than `0` or `1`.

  ```

- [ ] **Step 2: Check nothing still says otherwise**

Run: `git grep -n "hello.agents\[\]\` and \`workspace_roots\[\]\`;" -- docs/specs`
Expected: one line, the "Not on the wire yet" list, followed by the new sentence that `hello.agents[]` is on the wire since plan 4d-B1-i.

- [ ] **Step 3: Commit**

```bash
git add docs/specs
git commit -m "docs(spec): write back a host's agents, hello and probe_agents"
```

## After this plan

**What the frontend must do:**
- **4c (New session):** read `GET /api/hosts/{id}/agents`. `auth: missing` disables the agent with its instructions; `unknown` allows it (frontend §7). Hide images only when `images === false`; on 409 `images_unsupported` say so. `available: false` disables the agent and shows its `note` as escaped, bidi-isolated text. A refresh is `?refresh=1`, only on an explicit user action, never on mount, focus, a poll or a retry; it can take up to 20 s, and the control is disabled until it answers (the review's A7). A failed refresh shows as a `source` other than `probe`, or a `reported_at` older than the click.
- **4d-iii (Hosts page):** per host, `runtime` (managed: the set and whether it is the pinned one, held by a rollback; given: "commands given to `host run --agent`"), each agent's `adapter_version`, `available`, `auth` and `cli`, and when the report is from (`reported_at`, `source`); `live: false` says the report may be old. Notes are the host's words: render them as text. The refresh rules are 4c's (A7).

**Obligations this plan hands on:**
- **B1-ii (doctor report and notices):** checks 9 and 13 on demand, the stored `last_doctor`, and the Hosts page notices (frontend §8). The probe's checks 3 and 4 are here; B1-ii adds to `AgentChecks` (methods with default bodies) or to the answer, and keeps hazards (a)–(f): fresh groups per run in the host's guard, the host's env, exit codes or bounded facts only, one probe at a time, no check 5. A parameter `status_of` or `bundled_cli` needs for the host goes in a separate commit for the distribution lane's review.
- **The static catalogue** (ACP core §3.3 `catalog`, §8 `host_agent_catalog`): the New-session pickers' axes before the first session on a host. Not in this shape; a later field.
- **`_auth/status_update`** (decision 3): a probe could read it after `initialize`, before the CLI.

**Not tested here:**
- **A real agent's `initialize` and a real CLI's login status:** scripted adapters and CLIs stand in; the smoke test covers the real ones.
- **`DoctorChecks` against a real set:** its unit tests build the set's tree on disk; the CLIs are not run.
- **The waiter's own timeout in `Refreshes::refresh`:** it bounds only a waiter whose probe task was lost, which a test cannot cause.
- **The order in `Refreshes`, the entry removed before the waiters wake** (A8): it decides only whether a refresh issued in that instant joins the finished probe or starts a new one; a test of it would rest on timing.
- **`bound_agents` in the host's `availability::probe`** (A8): masked by `probe_one`, which keeps each name, and by the collector's own bound, which every stored report passes.
- **`guard.start_kill` on a failed spawn in `exit_status`:** the group then holds only the guard, which no test observes dying.
- **The first frame's size before `hello` is verified** (the review's O5, not taken): every frame is read under the 32 MiB limit; a smaller limit for the first frame would bound unauthenticated memory, a debt item for the host socket, not this plan's fields.

**Open for the maintainer:** nothing. The review's one product question (is `auth` per host or per (host, hat)?) is answered by the specs: decision 9.

**The security review's answers** (2026-10-02, on the maintainer's behalf):
- (a) adequate, with A2's test; (b) right on both paths, with A1 for a dropped check; (c) no agent or CLI byte reaches a report, the stderr drains into the bounded ring; (d) correct, `exit_status` gets what a session's adapter gets, with A4's test; (e) sound, the map bounded by the owner's hosts, no slot held forever; (f) holds.
- `cli: given` and `RuntimeInfo`: no managed field for `given`; bounded on both ends.
- The images doc comment states the ruling (O2: wording).
- 32 KiB holds as the worst case (about 21 KiB); the lenient readers are bounded by the frame limit, as `capabilities` are (O5).
- Owner filtering: the 404 is decided from the registry before any load or probe; the hub cannot hold another owner's host (A3 makes the test able to fail). The audit's floor, 16 to 18, is right.
- Display safety, the spec-gap rulings (D3, no cooldown with A7, 200 on a failed probe, no step-up, `no-store`, the home directory, storing only from a reconciled capable `hello`): approved.
- Every verdict outcome has a positive test; differential decoding is not needed (nothing downstream re-decodes the host's bytes).

**The scoped re-confirmation** (2026-10-02, a fresh opus subagent, on the maintainer's behalf): A1 (the boxed future is really dropped, and the test fails without `GroupKill`), A3 (the test now fails without the registry check, after the probe's timeout), A4, A5, A6 (`limit() == 0` is exact; an adapter that exits before the cap is never called "too much"), A7, A8, O1–O4 (the two lists of format characters were identical) and decision 9: confirmed. A2: confirmed with R1, applied. Noted, not required: a line that is not UTF-8 is still reported as "exited without answering" (as before); `GroupKill` assumes the CLI does not kill its own group's leader (as `Adapter` does).

**The second scoped re-confirmation** (2026-10-03, a fresh opus subagent, read-only, on the scratch rebased onto `b8cf8b3`): **RE-CONFIRMED**. It covered R1 as applied, and the merges the two rebases made with main: `hello`'s fields in one `Announce` (plan 8c's `mcp_isolation` beside this plan's agents and runtime; the pairing probe sends none and cannot overwrite a stored report); `from_set` keeping 9d-ii's profiles and Codex app-server beside this plan's `cli`, notes and infos; `AgentSetup::configure` setting both (empty and none for `--agent`, as before); `probe_agents` in the `None` arm of 8c's exhaustive `mcp_delivery` (it carries a request id only); and #102's default data directory. A1–A8, O1–O4, distribution's conditions and hazards (a)–(f) all held. Its optional note, taken: the guard test also asserts that its scan reaches `fn configure`, so a test module moved earlier cannot shrink it silently. Its other notes: a host whose `HOME` is set but missing makes every probed agent unavailable, which fails closed; and `cli.rs`'s binary test, which ran the probe (cwd = home, A5) in the real home of whoever ran the tests, was given a scratch one, after main's unwritable-HOME guard (#102) exposed it.

---

_Generated with Claude AI — please review before distribution._
