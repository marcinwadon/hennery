# Rotating logs and children that die with up (plan 7c-iii) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** close the two gaps plan 7c recorded (its decisions 14 and 15), so a service-run hennery neither fills the disk nor blocks its own relaunch (distribution spec §5, §6.2, §8):
- **Children that die with `up`.** If `up` is killed outright (launchd's SIGKILL after `ExitTimeOut`, the OOM killer, a panic), both children notice and stop, as on SIGTERM: the host stops its adapters and parks its sessions, the collector closes its port and admin socket. A relaunch is no longer refused on the port or `host.lock` they held.
- **Rotating logs.** Run by a service, `up`, the collector and `host run` each write a size-capped rotating log of their own (10 MiB × 5 files) in the per-OS logs directory, or one `HENNERY_LOG_DIR` names. The service manager's capture (launchd's `<role>.log`, systemd's journal) gets crash output only. A terminal run, a container and every other command log to standard output as today.

**Architecture:**
- **CLI** (`hennery`):
  - `inherit.rs`: `PARENT_FD` (12), `MAX_PASSED` one more, `watch_parent` (a detached thread reading the parent pipe until end-of-file, and ending the process 20 s later if it is still there) and `parent_gone`.
  - `main.rs`:
    - `run_up` makes the parent pipe before the first child; `UpChildren` holds both ends for `up`'s life and hands the reading end to every child it starts, first starts and restarts, as the hidden `--parent-fd 12`;
    - `run_collector` and `run_host` take `--parent-fd`, and stop when `up` is gone as they stop on SIGTERM;
    - `main` parses the command line first, then sets logging up for it (`Command::log_name`, `log::init`).
  - `log.rs` (new): `target` (where to log, from the environment), `init`, and `Rotating`, the size-capped writer.
- **Host** (`hennery-host`): `adapter::HOST_LOG_VARS`, stripped from every agent's environment and every `git` the host runs, like the host's secrets.
- **Tests:** `crates/hennery/tests/cli.rs` (a killed `up`, a restart and a relaunch; a service run's files; the descriptor checks), `log.rs`'s unit tests, `crates/hennery-host/tests/adapter.rs`.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), tokio, tracing-subscriber 0.3.23. **No new crate; `Cargo.lock` does not change** (decision 5).

**Spec:** [`docs/specs/2026-09-26-distribution-design.md`](../specs/2026-09-26-distribution-design.md):
- §5.1–§5.2: `up`'s two children in their own process groups; SIGTERM to the host first;
- §6.2: launchd's stdout/stderr go to `~/Library/Logs/hennery/<role>.log`, "crash output only; hennery writes its own size-capped rotating log";
- §6.3: the systemd unit sets `HENNERY_SERVICE=systemd`, `KillMode=mixed`, `TimeoutStopSec=30`;
- §8: logs in `$XDG_STATE_HOME/hennery/log` (Linux) and `~/Library/Logs/hennery` (macOS); "Logs rotate at 10 MiB × 5 files".

It builds on the executed [services, supervisor and host lock (7c)](2026-10-09-services.md). Its "After this plan" hands on:
- "**Rotating logs** (decision 14): a size-capped writer (10 MiB × 5) under `~/Library/Logs/hennery` and `$XDG_STATE_HOME/hennery/log`, when run by a service (`HENNERY_SERVICE`). Under launchd the plist's log then gets crash output only. A release blocker for macOS."
- "**The parent-death pipe** (decision 15): each child of `up` exits when a pipe from `up` reaches end of file, so a killed `up` leaves nothing running and launchd's relaunch can bind. The review suggests it ahead of 7d."

Every anchor below was taken from `main` at `3117743`, which merged PR #51 (7b) after PR #43 (7c-ii), 6b, PR #49 (the probe sockets' close-on-exec) and PR #45 (7e-i). 7b's `git.rs` strips what an agent never inherits from every `git` the host runs; the log variables join that list too (decision 9). Where the code and a spec disagree, the code wins, and the plan says so.

**Status:** not executed; amended after security review and plan review. The security review of 2026-10-02 (opus, binding on the maintainer's behalf) approved after amendments: A-1 to A-6 required and taken; O-1 to O-5, O-7 and O-8 taken; O-6 recorded (see "Decisions", "What the review changed"). It found no open product question. A plan review (opus, 2026-10-02) asked for changes, all taken: the log variables stripped from 7b's `git` commands too (A-3's rest), the plan re-anchored on `3117743` with its counts, no colour codes in the crash log, a failed reopen after a rotation moving the file back, the service-run test reading standard error only once both children are gone, A-2 described as structural, and four wording fixes.

**How this plan was checked.**

- **The code was built first,** on a scratch branch, then carved into the two tasks by a generator. Each task's blocks were generated from its before and after files, with every anchor grown until it occurs exactly once where it applies; the generator checks its blocks reproduce the after file.
- **A scratch CI run** (draft PR #47, closed unmerged) passed on `ubuntu-latest` and `macos-latest`, the new tests included: run 36954292372 on the first scratch tree, and run 36960414880 on the final one, the code of this plan on `3117743`.
- **The plan was replayed from its own text** onto a fresh checkout of `3117743`, task by task, by a harness that parses "Reading the steps" (Task 1: Step 1 7 instructions, its `cargo fmt` among them, and Step 3 8; Task 2: 8 and 14). After each task's Step 1 the test files matched the generator's snapshot, and after each task every file did, byte for byte. Each Step 2 and Step 4 command ran on its replay commit and printed what its "Expected" says. After each task the replay ran fmt, both clippy runs, the workspace tests and the codegen check: 840, then 841 tests, up from 831. (An earlier replay, onto `c05d642` before the plan review, matched too: 742 and 743 there.)
- **Revert-probes:** 12, run on the replay's final commit (each task's Step 5). Every one was caught.
- **Under load:** four copies of the CLI test binary at once, on each task's replay commit (each task's Step 6).

## Scope

**2 tasks, one PR** (`feat/logs-parent-death`):
1. the rotating logs;
2. the parent-death pipe.

The logs come first: they bring `hennery()`, which every `hennery` the CLI tests start goes through, so Task 2's tests are written with it; and Task 2 extends Task 1's service-run test to kill `up` and read each child's "gone" line in its own file.

**In:** `inherit.rs`, `log.rs` (new), and in `main.rs` `main`'s logging, `run_up`/`UpChildren`, and one hook each at the start of `run_collector` and `run_host`; `adapter::HOST_LOG_VARS` in `hennery-host`, chained where `adapter.rs` and `git.rs` strip the host's secrets (decision 9); the tests.

**Out** (see "After this plan"):
- `service status` and `uninstall` still name only `<role>.log` (7d);
- `doctor`'s checks of the log locations and of a child given up on (7d);
- a log directory resolved through the passwd entry when `HOME` is unset (O-6, recorded).

## Decisions this plan makes where the spec is silent

These were confirmed by a stronger-model security review on the maintainer's behalf (2026-10-02, "approve after amendments"). Each gives the choice, the alternatives, and the cost if it is wrong. Decisions the review changed are marked "(amended after the security review of 2026-10-02)". Items marked **(amendment)** depart from explicit spec text and should be written back into it.

**What the review changed:**
- **A-1:** a child whose parent is gone ends itself 20 s later if its shutdown hangs (decision 7). Under `up`, `STOP_GRACE` and `kill_on_drop` bound a stuck shutdown; with `up` gone nothing would.
- **A-2:** one `pass_to_child` per command, so the host's pairing pipe (3) and parent pipe (12) are moved together, never by two `pre_exec`s that could overwrite each other's source (decision 6). This is structural: no test can tell it apart, as `up`'s own descriptor numbers never sit at 3 or 12. Task 2's service-run test kills `up` on its first start, when the host holds both, and checks it stops.
- **A-3:** `HENNERY_SERVICE` and `HENNERY_LOG_DIR` are stripped from every agent's environment, from every `git` the host runs (7b's `git.rs`, found by the plan review), and from every `hennery` the CLI tests start (decision 9).
- **A-4:** the log directory must be the user's and not group- or world-writable; the file a regular file of the user's with one link; otherwise standard error (decision 3).
- **A-5:** a line break inside one event is escaped, so text from an agent (an error message) cannot forge a log line (decision 3).
- **A-6:** a panic is written to the file too, but never waited for: `try_lock`, so a panic while the file is being written cannot deadlock (decision 4).
- **Optional, taken:** O-1 (`log_internal_errors(false)`; a failed write said once), O-2 (`PARENT_FD` derived, with a compile-time check), O-3 (the comment on macOS's non-atomic `pipe`), O-4 (a zombie counts as gone in the tests; the proof is the positive signals), O-5 (a rotation that cannot open the new file empties the old one), O-7 (one line on standard error naming the file), O-8 (recorded below: `RUST_LOG=debug` keeps third-party detail on disk).
- **Optional, recorded:** O-6 (the passwd entry's home when `HOME` is unset; both service managers set `HOME`).
- **The three questions:** a relative `HENNERY_LOG_DIR` refuses the start, any other trouble falls back to standard error at `warn` (decision 2); the panic hook, yes (decision 4); journald under systemd, file only (decision 1).

1. **Who logs where.**
   - **Choice:** only the long-running commands log to a file, and only when run by a service: `up`, `collector` (not `collector healthcheck`) and `host run`. `admin`, `service`, `host join` and `collector healthcheck` keep standard output. Each of the three writes a file of its own: `hennery-up.log`, `hennery-collector.log`, `hennery-host.log`.
     - Not `<role>.log`: launchd holds that file open as each role's crash output, and a writer renaming it away would lose what launchd writes.
     - One file per process: two processes rotating one file by size lose lines. `up`'s children inherit the variables and open their own file.
     - One role per machine (7c decision 11) keeps a service-run collector and `up`'s collector child apart. Two processes forced into one directory by hand interleave and may drop lines at a rotation; that is not prevented.
     - **Under systemd too:** spec §8 names `$XDG_STATE_HOME/hennery/log` for Linux, so the journal gets crash output only, as launchd's file does. 7c's decision 14 ("journald rotates on Linux") described the code then, not a requirement (the review's answer to A9).
   - **Alternatives:** `warn` and above to the journal as well (two places to look); one file for all three (lines lost at rotation).
   - **Cost if wrong:** `journalctl --user -u hennery` shows only crashes; the startup line (decision 2) names the file.
2. **How a service run is recognised.** (amended after the security review of 2026-10-02: A7's answer)
   - **Choice:** `log::target`, in this order:
     - `HENNERY_LOG_DIR` set and not empty: that directory. Relative, it is a configuration error: the start fails, exit 1, before anything else.
     - Else `HENNERY_SERVICE` set and not empty (both 7c units set it: `launchd`, `systemd`): spec §8's directory, from the environment: macOS `$HOME/Library/Logs/hennery`; Linux `$XDG_STATE_HOME/hennery/log` when absolute, else `$HOME/.local/state/hennery/log`.
     - Else standard output, as today.

     No unit changes: installed units select it already. A directory that cannot be used (no absolute `HOME`, not creatable, not private, A-4's checks) falls back to standard error, with one line saying why, at `warn` unless `RUST_LOG` says otherwise: under a service that is the crash log, which must not grow with every line. When the file is in use, one line on standard error names it (O-7).
   - **Alternatives:**
     - **A `--log-dir` flag in the units:** rewrites 7c's units and every installed one.
     - **"Standard output is not a terminal":** would move a container's log out of `docker logs`.
     - **Refusing to start on any log trouble:** the cockpit down for a log directory's permissions; under launchd, a crash loop.
   - **Cost if wrong:** a service whose log directory is unusable logs warnings to its crash log; `doctor` (7d) is to say so.
3. **The writer.** (amended after the security review of 2026-10-02: A-4, A-5)
   - **Choice:**
     - "10 MiB × 5 files" is the file being written and four rotated ones, `.1` (newest) to `.4`: at most 50 MiB per process. A write that would take the file past 10 MiB first rotates (`.3`→`.4`, …, the file→`.1`) and opens a new one. A line longer than the cap is written whole, alone. The count starts from the file's length at open.
     - A rotation that fails empties the file instead, so the cap holds; it is said once on standard error, as is a failed write (O-1, O-5).
     - The directory is made 0700 if missing; it must be the user's, and not group- or world-writable. The file is opened `O_APPEND | O_NOFOLLOW | O_NONBLOCK` (a planted FIFO fails the open), 0600, close-on-exec; it must be a regular file of the user's with one link, and is made 0600 again at every open.
     - No colour codes. A line break inside one event is written as `\n` or `\r` (A-5): tracing-subscriber's escaping leaves CR and LF alone, and an adapter's error message is logged as a field.
     - Each event is one `write_all` from tracing-subscriber 0.3.23's fmt layer (`fmt_layer.rs`, one buffer per event), and `Rotating`'s writer holds a lock for it, so a rotation falls between lines. Writes are synchronous, as standard output's are: nothing is lost at exit.
   - **Alternatives:** `tracing-appender` 0.2.5, the latest, rotates by time only (`MINUTELY` … `WEEKLY`, `NEVER`, `max_log_files`): no size cap, which §8 asks for. A non-blocking writer would lose the last lines of a crash.
   - **Cost if wrong:** a few lines lost in a rotation race between hand-started processes (decision 1).
4. **Panics are written to the file too.** (amended after the security review of 2026-10-02: A-6)
   - **Choice:** while a file is in use, a panic hook writes `PANIC <message>` to it, then runs the previous hook (standard error, the crash log). It takes the file with `try_lock`: if it is being written, by this thread or another, the line is left out rather than waited for.
   - **Alternatives:** no hook (the crash only in the crash log); a blocking lock (deadlocks a panic inside the writer).
   - **Cost if wrong:** a panic during a log write is only in the crash log.
5. **No new crate.** The writer is about 150 lines in `log.rs` (decision 3); `Cargo.lock` does not change.
6. **The parent pipe.** (amended after the security review of 2026-10-02: A-2, O-2, O-3)
   - **Choice:**
     - `run_up` makes one pipe (`std::io::pipe`, both ends close-on-exec) before the first child, and `UpChildren` holds both ends as long as `up` runs. Every child it starts, first starts and restarts alike, gets the reading end at descriptor 12 (`PARENT_FD`, after the pairing pipe at 3 and up to eight listeners at 4–11) and `--parent-fd 12`. Each command's descriptors go through one `pass_to_child`.
     - Only `up` holds the writing end, and nothing writes to it. However `up` dies, the kernel closes it, and each child's read returns end-of-file.
     - On macOS `pipe` sets close-on-exec after making the pipe; nothing else spawns while `up` starts, and the pipe is never made again (a comment says so).
   - **Alternatives:**
     - **`PR_SET_PDEATHSIG`:** Linux only (macOS needs the pipe anyway), and it fires when the *thread* that forked the child ends, not the process; `up` forks from tokio's threads.
     - **kqueue's `EVFILT_PROC`, `pidfd`:** per platform; a parent pid check alone races pid reuse.
   - **Cost if wrong:** a child holding the writing end would never see end-of-file: today's behaviour. A reading end leaked to an agent gives it nothing to read and cannot keep `up` "alive".
7. **A child whose parent is gone.** (amended after the security review of 2026-10-02: A-1)
   - **Choice:**
     - `--parent-fd` is checked first thing, as `--pairing-code-fd` and `--join-code-fd` are (an open pipe, else a refusal naming the flag), and made close-on-exec at once, so no agent inherits it.
     - A detached thread blocks in `read`: end-of-file, or any error but `EINTR`, means `up` is gone; bytes are ignored. Not `spawn_blocking` (the runtime's shutdown waits for it), not a non-blocking read (`O_NONBLOCK` would land on the pipe both children share).
     - The child logs `hennery up is gone; stopping` and leaves by its SIGTERM path: the host through `run_until`'s shutdown future (it stops its adapters' process groups and parks its sessions), the collector through the task that waits on its signals (the listeners and the admin socket close). Exit 0, as after SIGTERM.
     - Both stop at once, not host first: no supervisor is left to order them. Park facts the host cannot send stay in its outbox, which is durable, and go after the relaunch.
     - **A-1:** 20 s after the end-of-file (`PARENT_GONE_DEADLINE`) the same thread says so on standard error and calls `_exit(1)`. That is past the host's bound on stopping its adapters (5 s and one more) and within systemd's `TimeoutStopSec` (30 s). An admin socket left behind is harmless: `admin::bind` checks for a stale one and removes it.
     - A child started by hand (no `--parent-fd`) behaves as before.
   - **Alternatives:** exiting at once on end-of-file (adapters orphaned, sessions not parked).
   - **Cost if wrong:** a relaunch that comes before the children are gone fails at the port or `host.lock` and ends within the startup grace; launchd retries after 10 s, systemd after 3 s.
8. **The relaunch converges.** No new code: a new `up` that finds the port taken exits 1; a new host that finds `host.lock` taken ends within the startup grace, which ends `up`; the service manager retries. `std` binds with `SO_REUSEADDR`, so connections left in `TIME_WAIT` do not hold the port.
9. **The log variables stay out of agents and tests.** (amended after the security review of 2026-10-02: A-3)
   - **Choice:** `hennery_host::adapter::HOST_LOG_VARS` (`HENNERY_SERVICE`, `HENNERY_LOG_DIR`) is removed from every agent's environment and from every `git` the host runs (`git::command`, whose filters run the repository's code), next to `NESTING_VARS` and `HOST_SECRET_VARS`. An agent that runs `hennery`, its test suite say, would otherwise log into the host's log directory. The CLI tests start every `hennery` without them (`hennery()`).
   - **Alternatives:** leaving them (an agent's `up` would write into the host's logs).
   - **Cost if wrong:** an agent that wants them must set them itself.
10. **Tests read real processes and poll for positive signals.**
    - A killed `up`: each child's "gone" line, the port bindable again, `host.lock` lockable with `flock(LOCK_EX | LOCK_NB)`, the adapter's grandchild gone, and a relaunch on the same port and data directory whose host connects. A pid's absence is checked last, a zombie counting as gone (O-4).
    - A restart is covered: the host is killed once past the startup grace and started again by `up`, then `up` is killed (revert-probed: a restart without the pipe fails it).
    - No test touches a real log directory: `HOME` and `XDG_STATE_HOME` are scratch directories, and `HENNERY_LOG_DIR` and `HENNERY_SERVICE` are removed from every other `hennery` the tests start.
11. **`RUST_LOG=debug` under a service** keeps third-party detail (HTTP headers, say) on disk, up to 50 MiB per process (O-8). The default, `info`, logs no secret: the setup link goes only to a terminal, and the pairing code never.

**Spec gaps put to the review, and its answers** (2026-10-02):
1. **Refuse or fall back when the log cannot be written:** "a split rule": a relative `HENNERY_LOG_DIR` refuses, any environment trouble falls back to standard error at `warn`.
2. **A panic hook:** "yes", with A-6's conditions.
3. **Journald under systemd:** "file only. Spec §8 decides this."

The review found no open product question.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`; crate names prefixed `hennery-`.
- After every task these pass:
  - `nix develop -c cargo fmt --all --check` (`max_width = 120`);
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy -p hennery --locked -- -D warnings` (the shipped binary: nothing may be unused outside the tests);
  - `cargo test --workspace --locked`;
  - `cargo run -p hennery-proto --bin gen -- --check`.
- **No new crate, and `Cargo.lock` does not change.**
- **Session runtime is unchanged.** Plans 5 (hats) and 6 (session features) change it in parallel. This plan touches `main.rs`'s `main`, `run_up` and `UpChildren`, one hook each at the start of `run_collector` and `run_host` (and their shutdown futures), `inherit.rs`, `log.rs`, and the strip lists in `hennery-host`'s `adapter.rs` and `git.rs`.
- **Nothing in a test touches a real log or data directory** (decision 10).
- **Linux-only behaviour:** none in the code; the pipe and the writer behave alike on both systems. Only macOS compiles here; CI's ubuntu job is the only Linux run (a scratch CI run did, before this plan: see "How this plan was checked" above).
- **Timing:** the CLI tests of both tasks must pass with four copies of their binary at once.
- No global installs: tooling comes from the flake dev shell.
- Commits follow Conventional Commits and use the repository's own identity (gmail, unsigned). Push the feature branch after every completed task; never push `main`.

## Review Focus

These are the inputs most likely to bite a real user that the obvious tests would not exercise, most likely first. Each is pinned by the named tests.

1. **launchd SIGKILLs `up`, after a child was restarted.**
   - Expected: both children stop as on SIGTERM; the adapter's process group is stopped; the port and `host.lock` are free; a relaunch on the same port serves, its host connected.
   - Tests: Task 2 `ups_children_stop_when_up_is_killed`.
2. **`up` dies on its first start, while the host is still pairing.**
   - Expected: the host holds the pairing pipe and the parent pipe together, and stops.
   - Tests: Task 2 `a_service_run_logs_to_its_own_files_and_not_to_its_output` (as Task 2 extends it), `ups_agents_never_see_the_operator_token_or_the_pairing_pipe` (a first start pairs with both descriptors passed).
3. **A service run, over weeks.**
   - Expected: three files of their own, private, none past 10 MiB, four rotated ones each; the crash log gets one line per process at each start (three for `up`), and one more per restarted child.
   - Tests: Task 1 `a_service_run_logs_to_its_own_files_and_not_to_its_output`, `the_log_rotates_at_its_cap_and_keeps_five_files`, `a_reopened_log_counts_what_it_holds`, `a_failed_rotation_still_keeps_the_cap`.
4. **A hostile log directory** (a shared `HENNERY_LOG_DIR`, a planted link).
   - Expected: refused, and logged to standard error instead; nothing written through a link.
   - Tests: Task 1 `planted_links_and_a_shared_directory_are_refused`.
5. **An agent's error message with a line break in it.**
   - Expected: one line in the file.
   - Tests: Task 1 `line_breaks_inside_an_event_are_escaped`.
6. **An agent that runs `hennery`.**
   - Expected: it neither inherits the parent pipe nor the log variables.
   - Tests: Task 2 `ups_agents_never_see_the_operator_token_or_the_pairing_pipe` (descriptor 12); Task 1 `the_service_logging_variables_are_removed_from_the_adapter_environment`.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery/src/log.rs` (new) | `target`, `init`, `Rotating`, the panic hook | 1 |
| `crates/hennery/src/main.rs` | `Command::log_name` and `log::init` in `main`; `--parent-fd` on `collector` and `host run`; `UpChildren::parent` | 1, 2 |
| `crates/hennery-host/src/adapter.rs` | `HOST_LOG_VARS` | 1 |
| `crates/hennery-host/tests/adapter.rs` | the variables stripped | 1 |
| `crates/hennery-host/src/git.rs` | `HOST_LOG_VARS` stripped from `git` too, and its unit test | 1 |
| `crates/hennery/src/inherit.rs` | `PARENT_FD`, `MAX_PASSED`, `PARENT_GONE_DEADLINE`, `watch_parent`, `parent_gone` | 2 |
| `crates/hennery/tests/cli.rs` | `hennery()`; a service run's files; a killed `up`; the descriptor checks | 1, 2 |

All commands run from the repository root inside the dev shell (`nix develop -c …`, or direnv). Each task leaves the workspace compiling, clippy-clean and green, and the binary working.

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block (and a final newline).
- "Append to `path`:" adds a blank line, then the block, at the end of the file.
- "In `path`, replace:" is followed by a block of whole lines that occurs **exactly once** in the file at that point (earlier blocks of the same task already applied, in order), then "with:" and its replacement.
- "In `path`, replace every occurrence of:" is followed by a one-line block, then "with:" and a one-line block: every occurrence of the first text, anywhere in a line, becomes the second.

"Run:" lines only check, but for Task 1's `cargo fmt --all`, which rewraps the lines the replacement shortened. The plan was replayed exactly this way, from its own text, onto `3117743`.

---

### Task 1: Rotating logs

**Files:**
- Create: `crates/hennery/src/log.rs`
- Modify: `crates/hennery/src/main.rs` (`mod log`, `Command::log_name`, `main`), `crates/hennery-host/src/adapter.rs` (`HOST_LOG_VARS`), `crates/hennery-host/src/git.rs` (the list chained, and its unit test)
- Test: `crates/hennery/tests/cli.rs` (`hennery()` everywhere; `a_service_run_logs_to_its_own_files_and_not_to_its_output`), `crates/hennery-host/tests/adapter.rs`, `log.rs`'s unit tests

**Interfaces:**
- Produces:
  - `log::target(var, macos) -> Result<Target, String>`, `log::init(Option<&str>) -> Result<(), String>`, `log::Rotating` (a `MakeWriter`), `LOG_DIR_VAR`, `SERVICE_VAR`, `MAX_BYTES`, `FILES`;
  - `Command::log_name(&self) -> Option<&'static str>`;
  - `hennery_host::adapter::HOST_LOG_VARS`;
  - in the CLI tests: `LOG_VARS`, `hennery()`, `text_of`.
- Consumes: nothing new.

- [ ] **Step 1: Write the failing tests**

Every `hennery` the CLI tests start goes through `hennery()`, without the two log variables (decision 9, A-3). The replacement shortens three chains that `cargo fmt` then puts on one line. The service-run test starts `up` with `HENNERY_SERVICE` and scratch `HOME` and `XDG_STATE_HOME`, and reads each process's file; Task 2 extends it. The adapter test gives an agent both variables and checks they are gone.

In `crates/hennery/tests/cli.rs`, replace every occurrence of:

```rust
Command::new(env!("CARGO_BIN_EXE_hennery"))
```

with:

```rust
hennery()
```

Run: `nix develop -c cargo fmt --all`
Expected: it rewraps `help_lists_the_skeleton_commands`'s, `host_help_lists_join_and_run`'s and `service_help_lists_install_uninstall_and_status`'s chains onto one line each; nothing else changes.

In `crates/hennery/tests/cli.rs`, replace:

```rust
use std::time::{Duration, Instant};

```

with:

```rust
use std::time::{Duration, Instant};

/// The variables that send a long-running `hennery` to a log file (plan
/// 7c-iii): a test run from a service-run host's agent, or a shell that set
/// them, must not log into a real log directory, nor away from the output
/// these tests read.
const LOG_VARS: [&str; 2] = ["HENNERY_SERVICE", "HENNERY_LOG_DIR"];

/// The `hennery` binary under test, without `LOG_VARS`.
fn hennery() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hennery"));
    for var in LOG_VARS {
        cmd.env_remove(var);
    }
    cmd
}

```

In `crates/hennery/tests/cli.rs`, replace:

```rust
    cmd.args(["-c", "umask 022; exec \"$0\" \"$@\"", env!("CARGO_BIN_EXE_hennery")]);
    cmd
```

with:

```rust
    cmd.args(["-c", "umask 022; exec \"$0\" \"$@\"", env!("CARGO_BIN_EXE_hennery")]);
    for var in LOG_VARS {
        cmd.env_remove(var);
    }
    cmd
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        .env_remove("HENNERY_DEV_TOKEN")
        .stdin(std::process::Stdio::null())
```

with:

```rust
        .env_remove("HENNERY_DEV_TOKEN")
        .env_remove(LOG_VARS[0])
        .env_remove(LOG_VARS[1])
        .stdin(std::process::Stdio::null())
```

Append to `crates/hennery/tests/cli.rs`:

```rust
/// `path`'s text, or nothing while it is not there.
fn text_of(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Plan 7c-iii: run by a service (`HENNERY_SERVICE` set, as both units set
/// it), `up` and its two children each log to a file of their own in
/// distribution spec §8's directory, private. Standard output, which launchd
/// keeps as `<role>.log`, gets nothing; standard error one line from each
/// naming its file. HOME and XDG_STATE_HOME are scratch directories: no real
/// log directory is touched.
#[test]
fn a_service_run_logs_to_its_own_files_and_not_to_its_output() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch_dir("svclog");
    let _cleanup = RemoveDir(dir.clone());
    let (home, state) = (dir.join("home"), dir.join("state"));
    let logs = if cfg!(target_os = "macos") {
        home.join("Library/Logs/hennery")
    } else {
        state.join("hennery/log")
    };
    let mut command = hennery();
    command
        .env("HENNERY_SERVICE", "systemd")
        .env("HOME", &home)
        .env("XDG_STATE_HOME", &state)
        .env("RUST_LOG", "info");
    let log = dir.join("up.log");
    let data = dir.join("data");
    let mut up = up_logging_to_with(command, &data, &log, &[]);
    let collector_log = logs.join("hennery-collector.log");
    let host_log = logs.join("hennery-host.log");
    up.wait_until("the collector's line in its own file", || {
        text_of(&collector_log).contains("collector listening")
    });
    up.wait_until("the host's line in its own file", || {
        text_of(&host_log).contains("connected to collector")
    });
    unsafe { libc::kill(up.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut up.up, Duration::from_secs(30)).is_some_and(|s| s.success()));
    assert!(
        !text_of(&host_log).contains("collector listening"),
        "one file per process"
    );
    assert!(
        !text_of(&collector_log).contains("connected to collector"),
        "one file per process"
    );
    let mode = |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&logs), 0o700);
    let names = ["hennery-up.log", "hennery-collector.log", "hennery-host.log"];
    for name in names {
        assert_eq!(mode(&logs.join(name)), 0o600, "{name}");
    }
    // No colour codes in a file.
    assert!(!text_of(&collector_log).contains('\u{1b}'));
    let stdout = text_of(&log);
    assert!(stdout.is_empty(), "stdout:\n{stdout}");
    let stderr = text_of(&log.with_extension("err"));
    let mut lines: Vec<&str> = stderr.lines().collect();
    lines.sort_unstable();
    let mut expected: Vec<String> = names
        .iter()
        .map(|name| format!("hennery: logging to {}", logs.join(name).display()))
        .collect();
    expected.sort_unstable();
    assert_eq!(lines, expected, "stderr:\n{stderr}");
}
```

In `crates/hennery-host/tests/adapter.rs`, replace:

```rust
    assert!(!env.contains("operator-token-never-for-agents"), "{env}");
}
```

with:

```rust
    assert!(!env.contains("operator-token-never-for-agents"), "{env}");
}

/// Plan 7c-iii: how a service-run host picks its own log
/// (`HENNERY_SERVICE`, `HENNERY_LOG_DIR`) is not passed to an agent, which
/// could otherwise run `hennery` into the host's log directory.
#[tokio::test]
async fn the_service_logging_variables_are_removed_from_the_adapter_environment() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("env.txt");
    let mut cmd = sh(&format!("env > {}", out.display()));
    cmd.env.push(("HENNERY_SERVICE".into(), "launchd".into()));
    cmd.env.push(("HENNERY_LOG_DIR".into(), "/var/log/elsewhere".into()));
    cmd.env.push(("HENNERY_KEEP".into(), "yes".into()));
    let (mut adapter, _io) = Adapter::spawn(&cmd, dir.path()).unwrap();
    adapter.exited().await;
    let env = std::fs::read_to_string(&out).unwrap();
    assert!(env.contains("HENNERY_KEEP=yes"), "{env}");
    assert!(!env.contains("HENNERY_SERVICE="), "{env}");
    assert!(!env.contains("HENNERY_LOG_DIR="), "{env}");
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `nix develop -c cargo test -p hennery --locked --test cli a_service_run_logs_to_its_own_files_and_not_to_its_output`
Expected: FAIL: `timed out waiting for the collector's line in its own file` (`up` logs to standard output as before).

Run: `nix develop -c cargo test -p hennery-host --locked --test adapter the_service_logging_variables_are_removed_from_the_adapter_environment`
Expected: FAIL: the assertion `!env.contains("HENNERY_SERVICE=")` (the agent's environment printed).

- [ ] **Step 3: The log**

`log.rs` holds the choice of target (decision 2), the writer (decision 3) and the panic hook (decision 4). `main` parses the command line first and sets logging up for the command. The adapter spawn and `git::command` strip the two variables (decision 9); `git.rs`'s unit test checks its list.

Create `crates/hennery/src/log.rs`:

```rust
//! hennery's own log (distribution spec §6.2, §8). Run by a service, `up`,
//! the collector and `host run` each write a size-capped rotating file of
//! their own in the logs directory, and the service manager's capture of
//! standard output and error gets crash output only. Anywhere else (a
//! terminal, a container) they log to standard output, as every other
//! command always does.

use std::ffi::OsString;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};

/// Names the directory to log to, in place of the service default.
pub const LOG_DIR_VAR: &str = "HENNERY_LOG_DIR";

/// Set by the launchd agent and the systemd unit `hennery service install`
/// writes (`launchd`, `systemd`): a process run by a service logs to its
/// file.
pub const SERVICE_VAR: &str = "HENNERY_SERVICE";

/// Distribution spec §8: "Logs rotate at 10 MiB × 5 files": the file
/// being written and four rotated ones.
pub const MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const FILES: usize = 5;

/// Where a long-running process logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Stdout,
    Dir(PathBuf),
    /// Run by a service, but with no directory to log to (no absolute
    /// HOME): standard error, and why.
    Unavailable(String),
}

/// Where to log, from the environment (`var`) on a macOS host or not:
/// `HENNERY_LOG_DIR` if set and not empty; else, under a service
/// (`HENNERY_SERVICE`), §8's directory for the platform; else standard
/// output. A `HENNERY_LOG_DIR` that is not absolute is a configuration
/// error, refused.
pub fn target(var: impl Fn(&str) -> Option<OsString>, macos: bool) -> Result<Target, String> {
    let set = |name: &str| var(name).filter(|v| !v.is_empty());
    let absolute = |name: &str| set(name).map(PathBuf::from).filter(|p| p.is_absolute());
    if let Some(dir) = set(LOG_DIR_VAR) {
        let dir = PathBuf::from(dir);
        if !dir.is_absolute() {
            return Err(format!("{LOG_DIR_VAR} is not an absolute path: {}", dir.display()));
        }
        return Ok(Target::Dir(dir));
    }
    if set(SERVICE_VAR).is_none() {
        return Ok(Target::Stdout);
    }
    let Some(home) = absolute("HOME") else {
        return Ok(Target::Unavailable("HOME is not an absolute path".into()));
    };
    Ok(Target::Dir(if macos {
        home.join("Library/Logs/hennery")
    } else {
        absolute("XDG_STATE_HOME")
            .unwrap_or_else(|| home.join(".local/state"))
            .join("hennery/log")
    }))
}

/// Set up logging for `process` (`up`, `collector`, `host`), or for a
/// command that always logs to standard output (`None`). `RUST_LOG`
/// filters, `info` by default.
///
/// A file that cannot be used (the directory not creatable, not private,
/// the disk full) falls back to standard error, saying why there, at `warn`
/// unless `RUST_LOG` says otherwise: under a service, that is the crash log,
/// which must not grow with every line. Only a configuration error
/// (`HENNERY_LOG_DIR` relative) is returned, to stop the start.
pub fn init(process: Option<&str>) -> Result<(), String> {
    let filter = |default: &str| {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default))
    };
    let why = match process.map(|_| target(|name| std::env::var_os(name), cfg!(target_os = "macos"))) {
        None | Some(Ok(Target::Stdout)) => {
            tracing_subscriber::fmt().with_env_filter(filter("info")).init();
            return Ok(());
        }
        Some(Err(why)) => return Err(why),
        Some(Ok(Target::Unavailable(why))) => why,
        Some(Ok(Target::Dir(dir))) => {
            let name = format!("hennery-{}.log", process.unwrap_or_default());
            match Rotating::open(&dir, &name, MAX_BYTES, FILES) {
                Ok(log) => {
                    record_panics(log.clone());
                    // One line where the service manager keeps output, so
                    // its log points at this one.
                    say(&format!("logging to {}", dir.join(&name).display()));
                    tracing_subscriber::fmt()
                        .with_env_filter(filter("info"))
                        .with_ansi(false)
                        // A failed write is said once by the writer, not
                        // once per event on standard error.
                        .log_internal_errors(false)
                        .with_writer(log)
                        .init();
                    return Ok(());
                }
                Err(err) => format!("cannot write the log in {}: {err}", dir.display()),
            }
        }
    };
    say(&format!("{why}; logging warnings and errors to standard error"));
    // Colour only on a terminal: under a service this is the crash log.
    tracing_subscriber::fmt()
        .with_env_filter(filter("warn"))
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_writer(std::io::stderr)
        .init();
    Ok(())
}

/// `line` on standard error, prefixed, in one `write`: `up` and its
/// children share it, and `eprintln!` writes a line in pieces, between
/// which another process's line can land (seen under load).
fn say(line: &str) {
    let _ = std::io::stderr().write_all(format!("hennery: {line}\n").as_bytes());
}

/// Write each panic to `log` too, then to standard error as before. Never
/// waits for the file: a panic while it is being written (on this thread
/// or another) leaves it out rather than deadlock.
fn record_panics(log: Rotating) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        record_panic(&log, &info.to_string());
        previous(info);
    }));
}

/// `text` as one line of `log`, if it is free now.
fn record_panic(log: &Rotating, text: &str) {
    let inner = match log.0.try_lock() {
        Ok(inner) => Some(inner),
        Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    };
    if let Some(mut inner) = inner {
        let _ = inner.write_all(format!("PANIC {text}\n").as_bytes());
    }
}

/// A log file that never grows past `max` bytes (but for one longer line):
/// before a write would take it past, it becomes `<name>.1`, `.1` becomes
/// `.2`, and so on; `<name>.<files - 1>` is dropped. One process writes it:
/// tracing formats each event whole and writes it with one `write_all`, so
/// a rotation falls between lines. A clone writes the same file.
#[derive(Clone)]
pub struct Rotating(Arc<Mutex<Inner>>);

struct Inner {
    path: PathBuf,
    file: std::fs::File,
    /// The file's length, counted from its length at open.
    len: u64,
    max: u64,
    files: usize,
    /// A failed write or rotation was said once on standard error; no
    /// other is.
    complained: bool,
}

impl Rotating {
    /// Open `dir/name` to append to, making `dir` (0700) if it is missing.
    /// `dir` must be this user's and writable by nobody else; the file must
    /// be a regular file of this user's, with no other link to it.
    pub fn open(dir: &Path, name: &str, max: u64, files: usize) -> std::io::Result<Self> {
        assert!(files >= 1 && max > 0);
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        let meta = std::fs::metadata(dir)?;
        if meta.uid() != euid() || meta.mode() & 0o022 != 0 {
            return Err(std::io::Error::other(format!(
                "{} is not this user's own directory, or others can write to it",
                dir.display()
            )));
        }
        let path = dir.join(name);
        let file = open_file(&path)?;
        let len = file.metadata()?.len();
        Ok(Self(Arc::new(Mutex::new(Inner {
            path,
            file,
            len,
            max,
            files,
            complained: false,
        }))))
    }
}

fn euid() -> u32 {
    // SAFETY: geteuid(2) cannot fail.
    unsafe { libc::geteuid() }
}

/// `path` for appending, private, never through a symbolic link, and only
/// if it is a regular file of this user's that nothing else links to: one
/// planted in a shared directory is refused, not written to. `O_NONBLOCK`
/// makes a FIFO planted there fail the open rather than block it; it does
/// nothing to a regular file.
fn open_file(path: &Path) -> std::io::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != euid() || meta.nlink() != 1 {
        return Err(std::io::Error::other(format!(
            "{} is not a regular file of this user's alone",
            path.display()
        )));
    }
    // One left by an older run may be broader: it is ours, so it is made
    // private again.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

/// `path` with `.n` appended.
fn numbered(path: &Path, n: usize) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{n}"));
    PathBuf::from(name)
}

/// `buf` with every line break but a final one escaped (`\n`, `\r`), so
/// text from an agent (an error's message, say) cannot forge a line.
fn one_line(buf: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let body = buf.strip_suffix(b"\n").unwrap_or(buf);
    if !body.iter().any(|&b| b == b'\n' || b == b'\r') {
        return buf.into();
    }
    let mut out = Vec::with_capacity(buf.len() + 8);
    for &b in body {
        match b {
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b => out.push(b),
        }
    }
    if body.len() < buf.len() {
        out.push(b'\n');
    }
    out.into()
}

impl Inner {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let buf = one_line(buf);
        if self.len > 0 && self.len.saturating_add(buf.len() as u64) > self.max {
            self.rotate();
        }
        let written = self.file.write_all(&buf);
        // Counted also when it failed: part of it may be in the file.
        self.len = self.len.saturating_add(buf.len() as u64);
        if let Err(err) = &written {
            self.complain(&format!("cannot write {}: {err}", self.path.display()));
        }
        written
    }

    fn rotate(&mut self) {
        // Whether the file being written was renamed to `.1`: should the new
        // one then fail to open, it is moved back, so the log keeps its name
        // and the rotated files keep what they hold.
        let mut renamed = false;
        let opened = (|| -> std::io::Result<std::fs::File> {
            if self.files == 1 {
                std::fs::remove_file(&self.path)?;
            } else {
                for n in (1..self.files - 1).rev() {
                    match std::fs::rename(numbered(&self.path, n), numbered(&self.path, n + 1)) {
                        Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
                        _ => {}
                    }
                }
                std::fs::rename(&self.path, numbered(&self.path, 1))?;
                renamed = true;
            }
            open_file(&self.path)
        })();
        match opened {
            Ok(file) => self.file = file,
            Err(err) => {
                if renamed {
                    let _ = std::fs::rename(numbered(&self.path, 1), &self.path);
                }
                // The cap holds whatever happens: the file being written
                // starts again empty.
                let _ = self.file.set_len(0);
                self.complain(&format!(
                    "cannot rotate {}: {err}; it is emptied instead",
                    self.path.display()
                ));
            }
        }
        self.len = 0;
    }

    /// Say `what` on standard error, the first time only: under a service
    /// that is the crash log.
    fn complain(&mut self, what: &str) {
        if !self.complained {
            self.complained = true;
            say(what);
        }
    }
}

/// The writer one event is written with: the file, locked.
pub struct Writer<'a>(MutexGuard<'a, Inner>);

impl Write for Writer<'_> {
    /// Writes all of `buf` at once, so a rotation never splits it.
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Rotating {
    type Writer = Writer<'a>;

    fn make_writer(&'a self) -> Writer<'a> {
        Writer(self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tracing_subscriber::fmt::MakeWriter;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: BTreeMap<String, OsString> = pairs.iter().map(|(k, v)| (k.to_string(), v.into())).collect();
        move |name| map.get(name).cloned()
    }

    /// Distribution spec §8's directories, under a service only; the
    /// override first; an empty one is unset, a relative one refused.
    #[test]
    fn the_target_follows_the_service_and_the_override() {
        let dir = |p: &str| Ok(Target::Dir(PathBuf::from(p)));
        assert_eq!(target(env(&[("HOME", "/h")]), true), Ok(Target::Stdout));
        assert_eq!(
            target(env(&[("HOME", "/h"), (SERVICE_VAR, "")]), false),
            Ok(Target::Stdout)
        );
        let service = [("HOME", "/h"), (SERVICE_VAR, "launchd")];
        assert_eq!(target(env(&service), true), dir("/h/Library/Logs/hennery"));
        assert_eq!(target(env(&service), false), dir("/h/.local/state/hennery/log"));
        let state = [("HOME", "/h"), (SERVICE_VAR, "systemd"), ("XDG_STATE_HOME", "/s")];
        assert_eq!(target(env(&state), false), dir("/s/hennery/log"));
        let relative_state = [("HOME", "/h"), (SERVICE_VAR, "systemd"), ("XDG_STATE_HOME", "s")];
        assert_eq!(target(env(&relative_state), false), dir("/h/.local/state/hennery/log"));
        for no_home in [
            &[(SERVICE_VAR, "systemd"), ("HOME", "h")][..],
            &[(SERVICE_VAR, "launchd")],
        ] {
            assert!(matches!(target(env(no_home), true), Ok(Target::Unavailable(_))));
        }
        let custom = [("HOME", "/h"), (SERVICE_VAR, "systemd"), (LOG_DIR_VAR, "/l")];
        assert_eq!(target(env(&custom), false), dir("/l"));
        assert_eq!(target(env(&[(LOG_DIR_VAR, "/l")]), true), dir("/l"));
        assert_eq!(target(env(&[(LOG_DIR_VAR, "")]), true), Ok(Target::Stdout));
        assert!(target(env(&[(LOG_DIR_VAR, "l")]), true).is_err());
    }

    fn write(log: &Rotating, line: &str) {
        log.make_writer().write_all(line.as_bytes()).unwrap();
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// Lines move down `.1` … `.4` as the file fills; the oldest are dropped;
    /// no file passes the cap; every file is private.
    #[test]
    fn the_log_rotates_at_its_cap_and_keeps_five_files() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        let log = Rotating::open(&logs, "x.log", 100, 5).unwrap();
        // 40 bytes each: two fit in a file, a third rotates it.
        let line = |n: usize| format!("{n:039}\n");
        for n in 0..12 {
            write(&log, &line(n));
        }
        let path = logs.join("x.log");
        assert_eq!(read(&path), line(10) + &line(11));
        assert_eq!(read(&numbered(&path, 1)), line(8) + &line(9));
        assert_eq!(read(&numbered(&path, 4)), line(2) + &line(3));
        let mut names: Vec<_> = std::fs::read_dir(&logs)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["x.log", "x.log.1", "x.log.2", "x.log.3", "x.log.4"]);
        for name in &names {
            let meta = std::fs::metadata(logs.join(name)).unwrap();
            assert!(meta.len() <= 100, "{name}");
            assert_eq!(meta.permissions().mode() & 0o777, 0o600, "{name}");
        }
        assert_eq!(std::fs::metadata(&logs).unwrap().permissions().mode() & 0o777, 0o700);
    }

    /// A restart counts what the file holds already, and a line longer than
    /// the cap is written whole, alone in its file.
    #[test]
    fn a_reopened_log_counts_what_it_holds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.log");
        std::fs::write(&path, "a".repeat(90)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let log = Rotating::open(dir.path(), "x.log", 100, 3).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        write(&log, &"b".repeat(20));
        assert_eq!(read(&numbered(&path, 1)), "a".repeat(90));
        assert_eq!(read(&path), "b".repeat(20));
        write(&log, &"c".repeat(150));
        assert_eq!(read(&path), "c".repeat(150));
        assert_eq!(read(&numbered(&path, 2)), "a".repeat(90));
    }

    /// Neither a symbolic link nor a hard link planted at the log's name is
    /// written through, and a directory others can write to is refused.
    #[test]
    fn planted_links_and_a_shared_directory_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::fs::write(&elsewhere, "kept").unwrap();
        let soft = dir.path().join("soft");
        std::fs::create_dir(&soft).unwrap();
        std::os::unix::fs::symlink(&elsewhere, soft.join("x.log")).unwrap();
        assert!(Rotating::open(&soft, "x.log", 100, 5).is_err());
        let hard = dir.path().join("hard");
        std::fs::create_dir(&hard).unwrap();
        std::fs::hard_link(&elsewhere, hard.join("x.log")).unwrap();
        assert!(Rotating::open(&hard, "x.log", 100, 5).is_err());
        assert_eq!(read(&elsewhere), "kept");
        let shared = dir.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(Rotating::open(&shared, "x.log", 100, 5).is_err());
        assert!(!shared.join("x.log").exists());
    }

    /// A rotation that cannot rename (the directory made read-only) empties
    /// the file instead, so the cap still holds.
    #[test]
    fn a_failed_rotation_still_keeps_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        let log = Rotating::open(&logs, "x.log", 100, 5).unwrap();
        write(&log, &"a".repeat(80));
        std::fs::set_permissions(&logs, std::fs::Permissions::from_mode(0o500)).unwrap();
        write(&log, &"b".repeat(40));
        std::fs::set_permissions(&logs, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(read(&logs.join("x.log")), "b".repeat(40));
        assert!(!numbered(&logs.join("x.log"), 1).exists());
    }

    /// A line break inside an event cannot start a forged line; the final
    /// one stays.
    #[test]
    fn line_breaks_inside_an_event_are_escaped() {
        let dir = tempfile::tempdir().unwrap();
        let log = Rotating::open(dir.path(), "x.log", 1000, 5).unwrap();
        write(&log, "WARN error=boom\nINFO forged\r\n");
        write(&log, "INFO plain\n");
        assert_eq!(
            read(&dir.path().join("x.log")),
            "WARN error=boom\\nINFO forged\\r\nINFO plain\n"
        );
    }

    /// A panic is written to the file when it is free, and left out, not
    /// waited for, when it is being written: by this very thread, say.
    #[test]
    fn a_panic_is_recorded_without_waiting_for_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let log = Rotating::open(dir.path(), "x.log", 1000, 5).unwrap();
        let held = log.make_writer();
        record_panic(&log, "while held");
        drop(held);
        record_panic(&log, "at line 7");
        assert_eq!(read(&dir.path().join("x.log")), "PANIC at line 7\n");
    }
}
```

In `crates/hennery/src/main.rs`, replace:

```rust
mod lock;
mod service;
```

with:

```rust
mod lock;
mod log;
mod service;
```

In `crates/hennery/src/main.rs`, replace:

```rust

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    // Every arm returns a `Result<ExitCode>` (not just `Result<()>`), and
```

with:

```rust

impl Command {
    /// The name of the log this command writes when run by a service
    /// (`log::init`): only the long-running ones have one.
    fn log_name(&self) -> Option<&'static str> {
        match self {
            Self::Up(_) => Some("up"),
            Self::Collector(CollectorCli {
                run: Some(_),
                command: None,
            }) => Some("collector"),
            Self::Host {
                command: HostCommand::Run(_),
            } => Some("host"),
            _ => None,
        }
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    if let Err(why) = log::init(cli.command.log_name()) {
        eprintln!("Error: {why}");
        return std::process::ExitCode::FAILURE;
    }
    // Every arm returns a `Result<ExitCode>` (not just `Result<()>`), and
```

In `crates/hennery/src/main.rs`, replace:

```rust
    // did; only then does the process actually exit with that code.
    let result = match Cli::parse().command {
        Command::Collector(CollectorCli {
```

with:

```rust
    // did; only then does the process actually exit with that code.
    let result = match cli.command {
        Command::Collector(CollectorCli {
```

In `crates/hennery-host/src/adapter.rs`, replace:

```rust
pub const HOST_SECRET_VARS: &[&str] = &["HENNERY_DEV_TOKEN"];

```

with:

```rust
pub const HOST_SECRET_VARS: &[&str] = &["HENNERY_DEV_TOKEN"];

/// How a service-run host chooses its own log (plan 7c-iii): not an
/// agent's. An agent that runs `hennery` (its test suite, say) must not
/// log into the host's log directory.
pub const HOST_LOG_VARS: &[&str] = &["HENNERY_SERVICE", "HENNERY_LOG_DIR"];

```

In `crates/hennery-host/src/adapter.rs`, replace:

```rust
        // configuration names it.
        for var in NESTING_VARS.iter().chain(HOST_SECRET_VARS) {
            command.env_remove(var);
```

with:

```rust
        // configuration names it.
        for var in NESTING_VARS.iter().chain(HOST_SECRET_VARS).chain(HOST_LOG_VARS) {
            command.env_remove(var);
```

In `crates/hennery-host/src/git.rs`, replace:

```rust
    // What an agent never inherits, git does not either: a filter it runs
    // is the repository's code (the second review's B2).
    for var in crate::adapter::NESTING_VARS
        .iter()
        .chain(crate::adapter::HOST_SECRET_VARS)
    {
```

with:

```rust
    // What an agent never inherits, git does not either: a filter it runs
    // is the repository's code (the second review's B2), and the host's log
    // variables (plan 7c-iii) are not its own.
    for var in crate::adapter::NESTING_VARS
        .iter()
        .chain(crate::adapter::HOST_SECRET_VARS)
        .chain(crate::adapter::HOST_LOG_VARS)
    {
```

In `crates/hennery-host/src/git.rs`, replace:

```rust
            .chain(crate::adapter::HOST_SECRET_VARS)
        {
```

with:

```rust
            .chain(crate::adapter::HOST_SECRET_VARS)
            .chain(crate::adapter::HOST_LOG_VARS)
        {
```

- [ ] **Step 4: Run them to see them pass**

Run the two commands of Step 2, then `nix develop -c cargo test -p hennery --locked --bin hennery log::` and `nix develop -c cargo test -p hennery-host --locked --lib git::tests::a_command_is_isolated_from_the_hosts_git_environment`.
Expected: PASS: 1, 1, 7 and 1 tests.

- [ ] **Step 5: Revert-probes**

Each on the replay's final commit; restore the line after each.
- In `target`, make `if set(SERVICE_VAR).is_none() {` `if set(SERVICE_VAR).is_none() || true {`. `a_service_run_logs_to_its_own_files_and_not_to_its_output` fails: `timed out waiting for the collector's line in its own file`.
- In `one_line`, make the early return unconditional (`if true || …`). `line_breaks_inside_an_event_are_escaped` fails: `left: "WARN error=boom\nINFO forged\r\nINFO plain\n"`.
- In `Adapter::spawn`, drop `HOST_LOG_VARS` from the chain (`.chain(HOST_LOG_VARS.iter().take(0))`). `the_service_logging_variables_are_removed_from_the_adapter_environment` fails at `!env.contains("HENNERY_SERVICE=")`.
- The same in `git::command`. `a_command_is_isolated_from_the_hosts_git_environment` fails: `HENNERY_SERVICE`, `left: None`.
- In `record_panic`, take the lock with `lock().unwrap()` in place of `try_lock()`. `a_panic_is_recorded_without_waiting_for_the_file` hangs (killed after 300 s): the deadlock A-6 is about.
- In `Rotating::open`, drop `|| meta.mode() & 0o022 != 0`. `planted_links_and_a_shared_directory_are_refused` fails: `Rotating::open(&shared, …).is_err()`.
- In `open_file`, drop `|| meta.nlink() != 1`. The same test fails: `Rotating::open(&hard, …).is_err()`.
- In `Rotating::open`, count from 0 (`len() * 0`). `a_reopened_log_counts_what_it_holds` fails: `left: ""`.

- [ ] **Step 6: Check the timing-sensitive tests under load**

Run four copies of the CLI test binary at once (its path from `cargo test -p hennery --locked --test cli --no-run`), each with `a_service_run_logs_to_its_own_files_and_not_to_its_output`, three rounds.
Expected: 0 of 12 copies failed.

- [ ] **Step 7: The full checks**

Run the five commands of "Global Constraints". Expected: all pass; **840 tests** in the workspace.

- [ ] **Step 8: Commit**

```bash
git add crates/hennery/src/log.rs crates/hennery/src/main.rs crates/hennery/tests/cli.rs crates/hennery-host/src/adapter.rs crates/hennery-host/tests/adapter.rs crates/hennery-host/src/git.rs
git diff --cached --stat
git -c commit.gpgsign=false commit -m "feat(cli): rotating log files of their own under a service"
git push origin feat/logs-parent-death
```

### Task 2: Children that die with `up`

**Files:**
- Modify: `crates/hennery/src/inherit.rs` (`PARENT_FD`, `MAX_PASSED`, `PARENT_GONE_DEADLINE`, `watch_parent`, `parent_gone`), `crates/hennery/src/main.rs` (`--parent-fd`, `run_collector`, `run_host`, `UpChildren`, `run_up`)
- Test: `crates/hennery/tests/cli.rs` (`ups_children_stop_when_up_is_killed`; the service-run test kills `up`; the descriptor checks)

**Interfaces:**
- Produces:
  - `inherit::PARENT_FD` (12), `inherit::PARENT_GONE_DEADLINE` (20 s), `inherit::watch_parent(RawFd) -> Result<oneshot::Receiver<()>>`, `inherit::parent_gone(Option<oneshot::Receiver<()>>)`; `MAX_PASSED` is `2 + MAX_LISTENERS`;
  - the hidden `--parent-fd` on `collector` and `host run`;
  - in the CLI tests: `running`, `poll_until`, `ups_children`, `lockable`.
- Consumes: Task 1's `hennery()` and `text_of`, and its service-run test.

- [ ] **Step 1: Write the failing tests**

The new test kills `up` after one restart of its host, and polls for the positive signals of decision 10. The service-run test now kills `up` on its first start, when the host holds both pipes (A-2), and reads each child's "gone" line in its own file. The descriptor checks cover `--parent-fd` like the pairing descriptors, and the agent's probe looks for descriptor 12 too, at `/dev/fd/12`: dash takes no descriptor above 9 in a redirection. The control holds 12 and shows the probe sees it.

In `crates/hennery/tests/cli.rs`, replace:

```rust
/// The agent is a shell script that dumps its environment, and which of the
/// descriptors 3 to 9 it holds (3 is the pairing pipe's number in the host
/// child, 4 the listening socket's in the collector child), then exits.
///
```

with:

```rust
/// The agent is a shell script that dumps its environment, and which of the
/// descriptors 3 to 9 and 12 it holds (3 is the pairing pipe's number in the
/// host child, 4 the listening socket's in the collector child, 12 the
/// parent pipe's in both), then exits. 12 is looked for at `/dev/fd/12`
/// first thing, before the shell itself opens anything: a shell such as dash
/// takes no descriptor above 9 in a redirection.
///
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        format!(
            "for n in 3 4 5 6 7 8 9; do if ( eval \": <&$n\" ) 2>/dev/null; then echo $n; fi; done > {fd}.tmp\n\
             env > {env}.tmp\nmv {fd}.tmp {fd}\nmv {env}.tmp {env}\n",
```

with:

```rust
        format!(
            "if [ -e /dev/fd/12 ]; then twelve=12; fi\n\
             {{ for n in 3 4 5 6 7 8 9; do if ( eval \": <&$n\" ) 2>/dev/null; then echo $n; fi; done; \
             if [ -n \"$twelve\" ]; then echo 12; fi; }} > {fd}.tmp\n\
             env > {env}.tmp\nmv {fd}.tmp {fd}\nmv {env}.tmp {env}\n",
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
            hennery_testkit::place_fd(1, 4)?;
            Ok(())
        });
    }
    assert!(control.status().unwrap().success());
    assert_eq!(std::fs::read_to_string(report("fds.txt")).unwrap(), "3\n4\n");
    std::fs::remove_file(report("fds.txt")).unwrap();
```

with:

```rust
            hennery_testkit::place_fd(1, 4)?;
            hennery_testkit::place_fd(1, 12)?;
            Ok(())
        });
    }
    assert!(control.status().unwrap().success());
    assert_eq!(std::fs::read_to_string(report("fds.txt")).unwrap(), "3\n4\n12\n");
    std::fs::remove_file(report("fds.txt")).unwrap();
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
    };
    type Make<'a> = &'a dyn Fn(&std::path::Path, &str) -> Command;
    for (flag, make) in [("--pairing-code-fd", &collector as Make), ("--join-code-fd", &host)] {
        for (what, fd, expected) in [
```

with:

```rust
    };
    // Plan 7c-iii: the parent pipe's end, `--parent-fd`, alike.
    let collector_parent = |data: &std::path::Path, fd: &str| -> Command {
        let mut cmd = hennery();
        cmd.args(["collector", "--listen", "127.0.0.1:0", "--parent-fd", fd])
            .arg("--data-dir")
            .arg(data);
        cmd
    };
    let host_parent = |data: &std::path::Path, fd: &str| -> Command {
        let mut cmd = hennery();
        cmd.args(["host", "run", "--parent-fd", fd]).arg("--data-dir").arg(data);
        cmd
    };
    type Make<'a> = &'a dyn Fn(&std::path::Path, &str) -> Command;
    for (flag, make) in [
        ("--pairing-code-fd", &collector as Make),
        ("--join-code-fd", &host),
        ("--parent-fd", &collector_parent),
        ("--parent-fd", &host_parent),
    ] {
        for (what, fd, expected) in [
```

In `crates/hennery/tests/cli.rs`, replace:

```rust

/// Plan 7c-iii: run by a service (`HENNERY_SERVICE` set, as both units set
```

with:

```rust

/// Whether `pid` runs: alive, and not a zombie its new parent has yet to
/// reap (`kill(pid, 0)` succeeds on one).
fn running(pid: i32) -> bool {
    pid_alive(pid)
        && Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .is_ok_and(|out| {
                let stat = String::from_utf8_lossy(&out.stdout);
                let stat = stat.trim();
                !stat.is_empty() && !stat.starts_with('Z')
            })
}

/// Poll `probe` until it holds, failing after 40 seconds: for when the
/// process that would explain a failure is gone.
fn poll_until(what: &str, mut probe: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(40);
    while !probe() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `up`'s two children once both run, polled: the host's pid from
/// `host.lock`, the collector's from `pgrep`. Both are recorded in `up`, so
/// they are killed should the test fail.
fn ups_children(up: &mut KillTree, lock: &std::path::Path) -> (i32, i32) {
    let up_pid = up.up.id() as i32;
    let (mut host, mut collector) = (0, 0);
    up.wait_until("up's two children", || {
        host = pid_from(lock).unwrap_or(0);
        let children = children_of(up_pid);
        collector = children.iter().copied().find(|&pid| pid != host).unwrap_or(0);
        children.len() == 2 && children.contains(&host) && collector != 0
    });
    up.children.extend([host, collector]);
    (host, collector)
}

/// Plan 7c-iii: run by a service (`HENNERY_SERVICE` set, as both units set
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
/// log directory is touched.
#[test]
```

with:

```rust
/// log directory is touched.
///
/// `up` is then killed outright on its first start, when its host holds the
/// pairing pipe as well as the parent pipe: each child says, in its own
/// file, that `up` is gone.
#[test]
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
    });
    unsafe { libc::kill(up.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut up.up, Duration::from_secs(30)).is_some_and(|s| s.success()));
    assert!(
```

with:

```rust
    });
    let (host, collector) = ups_children(&mut up, &data.join("host").join("host.lock"));

    unsafe { libc::kill(up.up.id() as i32, libc::SIGKILL) };
    let _ = up.up.wait();
    for file in [&collector_log, &host_log] {
        poll_until("each child saying up is gone", || {
            text_of(file).contains("hennery up is gone; stopping")
        });
    }
    // Gone, so nothing more can reach standard error after it is read.
    poll_until("both children gone", || !running(host) && !running(collector));
    assert!(
```

Append to `crates/hennery/tests/cli.rs`:

```rust
/// Whether `path` can be locked as `lock::acquire` locks it, now: nobody
/// holds it.
fn lockable(path: &std::path::Path) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(file) = std::fs::OpenOptions::new().read(true).write(true).open(path) else {
        return false;
    };
    // SAFETY: flock(2) on a descriptor this function owns; closing it below
    // releases the lock.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

/// Plan 7c-iii: `up` killed outright (launchd's SIGKILL after
/// `ExitTimeOut`, the OOM killer) leaves nothing running. Each child reads
/// end-of-file on the pipe whose writing end only `up` held and stops as on
/// SIGTERM: the host stops its adapter, and the port and `host.lock` are
/// free, so a relaunch on the same port and data directory serves again,
/// its host connected. The host is one `up` started again after a crash, so
/// a restart hands the pipe on too.
#[test]
fn ups_children_stop_when_up_is_killed() {
    let dir = scratch_dir("parentdeath");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("data");
    // An adapter that never answers, with a grandchild, as in
    // `a_revoked_hosts_still_starting_adapter_is_reaped_past_shut_downs_bound`.
    let adapter_pid_file = dir.join("adapter.pid");
    let grandchild_pid_file = dir.join("grandchild.pid");
    let script = dir.join("slow.sh");
    std::fs::write(
        &script,
        format!(
            "echo $$ > {}\nsleep 7117 &\necho $! > {}\nwait\n",
            adapter_pid_file.display(),
            grandchild_pid_file.display()
        ),
    )
    .unwrap();
    let _kill_adapter = KillAdapter(adapter_pid_file.clone());
    let log = dir.join("up.log");
    let mut up = up_logging_to_with(
        hennery(),
        &data,
        &log,
        &["--agent", &format!("slow=/bin/sh {}", script.display())],
    );
    let listen = up.listening();
    let session = sign_in(&mut up, &listen, &data.join("collector"));
    let connections = |log: &std::path::Path| text_of(log).matches("connected to collector").count();
    up.wait_until("the host connected", || connections(&log) >= 1);

    // Past the startup grace (5 s), then the host killed: `up` starts it
    // again, and that host must get the pipe as well.
    std::thread::sleep(Duration::from_secs(6));
    let lock = data.join("host").join("host.lock");
    let first = pid_from(&lock).expect("the host's pid");
    up.children.push(first);
    unsafe { libc::kill(first, libc::SIGKILL) };
    up.wait_until("the host started again", || {
        pid_from(&lock).is_some_and(|pid| pid != first && pid_alive(pid))
    });
    up.wait_until("the host connected again", || connections(&log) >= 2);
    let (host, collector) = ups_children(&mut up, &lock);
    let hosts = get_json(&listen, "/api/hosts", &session).unwrap();
    let host_id = hosts[0]["host_id"].as_str().unwrap().to_string();
    post_json(
        &listen,
        "/api/sessions",
        &session,
        &serde_json::json!({ "host_id": host_id, "agent": "slow", "cwd": dir }).to_string(),
    );
    let mut grandchild = None;
    up.wait_until("the adapter started", || {
        grandchild = pid_from(&grandchild_pid_file);
        grandchild.is_some()
    });
    let grandchild = grandchild.unwrap();

    unsafe { libc::kill(up.up.id() as i32, libc::SIGKILL) };
    let _ = up.up.wait();
    poll_until("both children saying up is gone", || {
        text_of(&log).matches("hennery up is gone; stopping").count() == 2
    });
    // The positive signals: the port and the lock taken back, and the
    // relaunch below. A pid alone could be a zombie not yet reaped.
    poll_until("the port free", || std::net::TcpListener::bind(&listen).is_ok());
    poll_until("host.lock free", || lockable(&lock));
    poll_until("the adapter stopped", || !running(grandchild));
    poll_until("both children gone", || !running(host) && !running(collector));

    // The same port: another test binding port 0 could take it meanwhile,
    // which would fail the relaunch; not seen in the load runs.
    let again_log = dir.join("again.log");
    let again = hennery()
        .args(["up", "--listen", &listen, "--data-dir"])
        .arg(&data)
        .stdout(std::fs::File::create(&again_log).unwrap())
        .stderr(std::fs::File::create(again_log.with_extension("err")).unwrap())
        .spawn()
        .unwrap();
    let mut again = KillTree::new(again, &again_log);
    again.wait_until("the relaunched host connected", || {
        get_json(&listen, "/api/hosts", &session)
            .is_some_and(|hosts| hosts[0]["host_id"] == host_id.as_str() && hosts[0]["connected"] == true)
    });
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `nix develop -c cargo test -p hennery --locked --test cli -- ups_children_stop_when_up_is_killed a_service_run_logs_to_its_own_files_and_not_to_its_output the_code_descriptors_must_be_open_pipes ups_agents_never_see_the_operator_token_or_the_pairing_pipe`
Expected: FAIL: 1 passed (`ups_agents_never_see_…`: before this task no child holds descriptor 12; its control shows the probe sees one), 3 failed: `the_code_descriptors_must_be_open_pipes` (`--parent-fd, a closed descriptor: error: unexpected argument '--parent-fd' found`), and the other two `timed out waiting for each child saying up is gone` and `timed out waiting for both children saying up is gone`. Without the pipe, each test's guard kills the children `up` left running.

- [ ] **Step 3: The parent pipe**

`inherit.rs` gains the descriptor, the watcher and its deadline (decisions 6, 7). `run_up` makes the pipe before its first child, `UpChildren` hands its reading end to every child through one `pass_to_child` per command, and `run_collector` and `run_host` stop when `up` is gone as on SIGTERM.

In `crates/hennery/src/inherit.rs`, replace:

```rust
//! child, each end inherited as a file descriptor, so the code is never on a
//! command line or in the environment; and the collector child inherits the
//! listening sockets `up` bound for it.

```

with:

```rust
//! child, each end inherited as a file descriptor, so the code is never on a
//! command line or in the environment; the collector child inherits the
//! listening sockets `up` bound for it; and each child inherits the reading
//! end of a pipe whose end-of-file says `up` is gone.

```

In `crates/hennery/src/inherit.rs`, replace:

```rust

/// The most descriptors one child is handed: the pairing pipe's end and a
/// listening socket for each of up to `MAX_LISTENERS` addresses.
pub const MAX_PASSED: usize = 1 + crate::MAX_LISTENERS;

```

with:

```rust

/// The descriptor number each child finds the reading end of `up`'s parent
/// pipe at: past the pairing pipe's and every listening socket's.
pub const PARENT_FD: RawFd = LISTENER_FD + crate::MAX_LISTENERS as RawFd;

/// The most descriptors one child is handed: the pairing pipe's end, a
/// listening socket for each of up to `MAX_LISTENERS` addresses, and the
/// parent pipe's reading end.
pub const MAX_PASSED: usize = 2 + crate::MAX_LISTENERS;

const _: () = assert!(PARENT_FD < MOVE_FLOOR);

/// How long a child may take to stop once `up` is gone, before it exits
/// outright: past the host's bound on stopping its adapters (5 s and one
/// more), within systemd's `TimeoutStopSec` (30 s). With `up` gone nothing
/// else would end a shutdown that hangs.
pub const PARENT_GONE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);

```

In `crates/hennery/src/inherit.rs`, replace:

```rust

/// Close an inherited descriptor that is not needed after all.
```

with:

```rust

/// Watch `--parent-fd`, the reading end of a pipe whose only writing end
/// `hennery up` holds: the receiver resolves once `up` is gone, however it
/// died, as the kernel then closes that end. The descriptor is checked as
/// the other inherited pipes are, and made close-on-exec at once, so no
/// agent inherits it.
///
/// A thread of its own blocks in `read`, detached, so it never holds the
/// process's exit back: not `spawn_blocking`, whose tasks the runtime waits
/// for as it shuts down, and not a non-blocking read, whose `O_NONBLOCK`
/// would be set on the pipe both children share. Once `up` is gone, the same
/// thread ends the process outright if it is still there after
/// `PARENT_GONE_DEADLINE`.
pub fn watch_parent(fd: RawFd) -> Result<tokio::sync::oneshot::Receiver<()>> {
    check_pipe("--parent-fd", fd)?;
    // SAFETY: fcntl(2) on the descriptor just checked; it only sets its
    // flags.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0 {
            return Err(std::io::Error::last_os_error()).context("make --parent-fd close-on-exec");
        }
    }
    // SAFETY: `fd` was inherited for exactly this and nothing else owns it.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let (gone, receiver) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("parent-watch".into())
        .spawn(move || {
            let mut buf = [0u8; 64];
            // `up` never writes; anything read is ignored. End-of-file, or
            // any error but an interruption, means it is gone.
            loop {
                match file.read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            let _ = gone.send(());
            std::thread::sleep(PARENT_GONE_DEADLINE);
            // One `write`, as `log::say` does: the other child shares the
            // descriptor.
            let _ = std::io::stderr().write_all(
                format!(
                    "hennery: still running {} s after `hennery up` is gone; exiting\n",
                    PARENT_GONE_DEADLINE.as_secs()
                )
                .as_bytes(),
            );
            // SAFETY: _exit(2) ends the process at once, running nothing
            // else: no other thread's state is touched.
            unsafe { libc::_exit(1) }
        })
        .context("start the thread that watches --parent-fd")?;
    Ok(receiver)
}

/// Resolves once `up` is gone (`watch_parent`), and says so; never without
/// a parent pipe (a child started by hand).
pub async fn parent_gone(watch: Option<tokio::sync::oneshot::Receiver<()>>) {
    match watch {
        // A watcher that stopped without a word is as good as `up` gone.
        Some(watch) => {
            let _ = watch.await;
            tracing::warn!("hennery up is gone; stopping");
        }
        None => std::future::pending().await,
    }
}

/// Close an inherited descriptor that is not needed after all.
```

In `crates/hennery/src/main.rs`, replace:

```rust
    listen_fd: Vec<i32>,
}
```

with:

```rust
    listen_fd: Vec<i32>,
    /// `hennery up` only: the reading end of a pipe only `up` writes to.
    /// At its end-of-file `up` is gone, and the collector stops.
    #[arg(long, hide = true, value_parser = clap::value_parser!(i32).range(3..))]
    parent_fd: Option<i32>,
}
```

In `crates/hennery/src/main.rs`, replace:

```rust
    collector_url: Option<String>,
    /// A directory to find projects in and to allow browsing under (ACP
```

with:

```rust
    collector_url: Option<String>,
    /// `hennery up` only: as for `collector`; at its end-of-file the host
    /// stops, as on SIGTERM.
    #[arg(long, hide = true, value_parser = clap::value_parser!(i32).range(3..))]
    parent_fd: Option<i32>,
    /// A directory to find projects in and to allow browsing under (ACP
```

In `crates/hennery/src/main.rs`, replace:

```rust
        inherit::check_pipe("--pairing-code-fd", fd)?;
    }
    warn_if_dev_token();
    let file = config::FileConfig::load(&args.data_dir)?;
```

with:

```rust
        inherit::check_pipe("--pairing-code-fd", fd)?;
    }
    let parent = args.parent_fd.map(inherit::watch_parent).transpose()?;
    warn_if_dev_token();
    let file = config::FileConfig::load(&args.data_dir)?;
```

In `crates/hennery/src/main.rs`, replace:

```rust
    tokio::spawn(async move {
        signals.recv().await;
        shutdown.cancel();
```

with:

```rust
    tokio::spawn(async move {
        tokio::select! {
            () = signals.recv() => {}
            () = inherit::parent_gone(parent) => {}
        }
        shutdown.cancel();
```

In `crates/hennery/src/main.rs`, replace:

```rust
    }
    warn_if_dev_token();
```

with:

```rust
    }
    let parent = args.parent_fd.map(inherit::watch_parent).transpose()?;
    warn_if_dev_token();
```

In `crates/hennery/src/main.rs`, replace:

```rust
    // `std::process::exit` here would skip all of that.
    match hennery_host::run_until(cfg, terminated()).await {
        // Its own exit code, so `hennery up` can tell a revoke apart.
```

with:

```rust
    // `std::process::exit` here would skip all of that.
    // `up` gone (its pipe's end-of-file) stops the host as a signal does.
    let shutdown = async {
        tokio::select! {
            () = terminated() => {}
            () = inherit::parent_gone(parent) => {}
        }
    };
    match hennery_host::run_until(cfg, shutdown).await {
        // Its own exit code, so `hennery up` can tell a revoke apart.
```

In `crates/hennery/src/main.rs`, replace:

```rust
    listeners: Vec<std::net::TcpListener>,
}
```

with:

```rust
    listeners: Vec<std::net::TcpListener>,
    /// Made before the first child and held until `up` ends: every child
    /// gets the reading end (`--parent-fd`), and only `up` holds the
    /// writing end, so however `up` dies, each child reads end-of-file and
    /// stops (plan 7c-iii).
    parent: (std::io::PipeReader, std::io::PipeWriter),
}
```

In `crates/hennery/src/main.rs`, replace:

```rust
        }
        inherit::pass_to_child(&mut cmd, &fds);
```

with:

```rust
        }
        fds.push((self.parent.0.as_raw_fd(), inherit::PARENT_FD));
        cmd.arg("--parent-fd").arg(inherit::PARENT_FD.to_string());
        inherit::pass_to_child(&mut cmd, &fds);
```

In `crates/hennery/src/main.rs`, replace:

```rust
        let mut cmd = host_command(&self.exe, &self.host_dir, &self.collector_ws_url, self.args);
        if let Some(reader) = pairing {
            inherit::pass_to_child(&mut cmd, &[(reader.as_raw_fd(), inherit::CHILD_FD)]);
            cmd.arg("--join-url")
```

with:

```rust
        let mut cmd = host_command(&self.exe, &self.host_dir, &self.collector_ws_url, self.args);
        let mut fds = vec![(self.parent.0.as_raw_fd(), inherit::PARENT_FD)];
        cmd.arg("--parent-fd").arg(inherit::PARENT_FD.to_string());
        if let Some(reader) = pairing {
            fds.push((reader.as_raw_fd(), inherit::CHILD_FD));
            cmd.arg("--join-url")
```

In `crates/hennery/src/main.rs`, replace:

```rust
        }
        cmd
```

with:

```rust
        }
        inherit::pass_to_child(&mut cmd, &fds);
        cmd
```

In `crates/hennery/src/main.rs`, replace:

```rust
        listeners,
    };
```

with:

```rust
        listeners,
        // Made here, before any child: on macOS `std::io::pipe` marks its
        // ends close-on-exec only after making them, so a spawn on another
        // thread meanwhile could inherit the writing end and keep it open.
        // Nothing else spawns while `up` starts, and it is never made again.
        parent: std::io::pipe()?,
    };
```

- [ ] **Step 4: Run them to see them pass**

Run the command of Step 2.
Expected: PASS, 4 tests.

- [ ] **Step 5: Revert-probes**

Each on the replay's final commit; restore the line after each.
- In `UpChildren::host`, pass the parent pipe only with the pairing pipe (a first start), not on a restart. `ups_children_stop_when_up_is_killed` fails: `timed out waiting for both children saying up is gone`.
- In `run_host`'s shutdown, disable the parent arm (`() = inherit::parent_gone(parent), if false => {}`). The same test fails the same way.
- The same in `run_collector`'s signal task. The same test fails the same way.
- In `watch_parent`, ignore `check_pipe`'s result. `the_code_descriptors_must_be_open_pipes` fails: `--parent-fd, a closed descriptor` is taken.

- [ ] **Step 6: Check the timing-sensitive tests under load**

Run four copies of the CLI test binary at once, each with `ups_children_stop_when_up_is_killed` and `a_service_run_logs_to_its_own_files_and_not_to_its_output`, three rounds; then four copies of the whole CLI test binary, three rounds.
Expected: 0 of 12 copies failed, then 0 of 12 for the whole binary. An earlier round of the whole binary, on the first replay, failed once: `up`'s children each wrote their "logging to" line with `eprintln!`, which writes a line in pieces, and two lines interleaved on the shared standard error. `log::say` now writes each line in one `write` (as does the deadline's line in `inherit.rs`); the plan's code is the fixed one.

- [ ] **Step 7: The full checks**

Run the five commands of "Global Constraints". Expected: all pass; **841 tests** in the workspace.

- [ ] **Step 8: Commit**

```bash
git add crates/hennery/src/inherit.rs crates/hennery/src/main.rs crates/hennery/tests/cli.rs
git diff --cached --stat
git -c commit.gpgsign=false commit -m "feat(up): children stop when up dies, through a pipe only up writes to"
git push origin feat/logs-parent-death
```

---

## After this plan

**Obligations this plan hands on:**
- **`doctor` (7d):**
  - the log locations: `~/Library/Logs/hennery/hennery-{up,collector,host}.log` (macOS) and `$XDG_STATE_HOME/hennery/log/…` (Linux), or `HENNERY_LOG_DIR`; a process that fell back to standard error (decision 2) says so only there, in the crash log, which `doctor` should read;
  - the restart-gave-up state, from `supervisor::read_state`, judged as `service status` judges it (7c decision 6);
  - `service status` and `uninstall` name `<role>.log` as the service's "output" and "log": they should name hennery's own files as well.
- **Recorded from the review:** O-6, the passwd entry's home when `HOME` is unset, so `doctor` and the runtime agree whatever the environment.

**Not tested here:**
- **A rotation whose new file cannot be opened** after the rename: the file is moved back and emptied (the plan review's finding 4); no test can fail the open once the rename succeeded.
- **A-1's deadline:** no test makes a shutdown hang for 20 s after `up` is gone; `_exit` would end the test binary.
- **`watch_parent`'s close-on-exec:** the adapter spawn closes every inherited descriptor anyway (`close_inherited`), so the descriptor test cannot tell it apart; it is belt and braces.
- **A real launchd or systemd killing `up`:** the tests SIGKILL `up` themselves.
- **A full disk:** a failed write is said once (O-1), untested.
- **The Linux side** on this machine: CI's ubuntu job is its only Linux run.

**Spec amendments:**
- distribution §6.2 and §8: hennery's own files are `hennery-<process>.log`, one per process, beside launchd's `<role>.log`; "10 MiB × 5 files" is the file and four rotated ones; `HENNERY_LOG_DIR` overrides the directory; a service run is recognised by `HENNERY_SERVICE`;
- distribution §5.1: each child of `up` holds the reading end of a pipe only `up` writes to, and stops at its end-of-file.

Then, in order: **7d (doctor)**, **7e (Nix)**.

---

_Generated with Claude AI — please review before distribution._
