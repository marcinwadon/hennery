# Operator authentication, part 2 (plan 3b-ii) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** Configure and recover the collector without a browser:
- It listens on several addresses. Every listener serves the same router, state and browser rules, and a start fails if any address is taken.
- `config.toml` sits under the environment and the flags.
- A private `admin.sock` answers `hennery admin`: print the setup link, reset the password, reset `public_url` (the way back from 3b-i's lock-out), list hosts, mint a pairing code. What changes state needs a confirmation on a terminal.
- Along the way:
  - agents stop inheriting the host's stray descriptors;
  - `--listen-fd` adopts only a listening TCP socket;
  - `/healthz` and `/readyz` exist.

**Architecture:**
- **Host** (`hennery-host`): `Adapter::spawn` closes every descriptor from 3 up that is not close-on-exec, in `pre_exec`.
- **Kernel** (`hennery-kernel`):
  - `health.rs`: the two health routes.
  - `operator.rs`: `reset_password` (a hashing permit, every session ended), `reset_public_url` (the row and the cached origin, every session ended) and `setup_link` (the live link or a fresh one). A session is opened only on the PHC string its password check verified, so a login in flight does not outlive a reset.
  - `admin.rs`: the socket. `bind` refuses a live socket, replaces a stale one and stops at anything else. `serve` checks the peer's user id and answers one JSON line with one JSON line. `request` is the client side, and checks the server's user id in turn.
- **Collector** (`hennery-sessions`): `serve_on` / `serve_all` run one `axum::serve` per listener, on one router.
- **Binary** (`hennery`):
  - `--listen` and `--listen-fd` are repeatable, and `HENNERY_LISTEN` is comma-separated. `up` binds every address and hands each over as a descriptor.
  - `config.rs` reads `listen` and `public_url` from `config.toml`.
  - `run_collector` binds `admin.sock` and serves it on the router's own `Operator` and `Hosts`. Its signal handlers exist from the first line.
  - `admin.rs` is `hennery admin …`, with its terminal confirmation.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), tokio, axum 0.8, rusqlite 0.40, clap 4, `toml` 1.1.6 and `serde` (both already workspace dependencies: the binary and the kernel now take them from there). No new crate, nothing new in `flake.nix`: `openpty`, `tcgetattr` and `TCP_CONNECTION_INFO` come from `libc` 0.2.189, already in `Cargo.lock`.

**Spec:** [`docs/specs/2026-09-26-kernel-design.md`](../specs/2026-09-26-kernel-design.md), these sections:
- §1: the data directory holds `config.toml` and `admin.sock`.
- §2: configuration precedence.
- §3.3: the health checks are exempt.
- §4.2: the admin socket, and the TTY confirmation of destructive commands.
- §7: several listeners, and browser access bound to `public_url`.
- §8: `/healthz` and `/readyz`.
- §10: the admin socket and same-user processes.
- §11: `Origin` rules on every listener, and a start that fails when one of several addresses is taken.

It builds on the executed [operator auth plan 3b-i](2026-10-02-operator-auth.md). Read its "Execution status" and "After this plan" first: the 3b-ii bullets there are this plan's scope. Every anchor below was taken from `main` at `c622c1e`, which merged PR #11 (the CLI tests' port-0 harness, and `up` binding its collector's socket and handing it over as `--listen-fd`). Where the code and a spec disagree, the code wins.

**Status:** not executed. Amended after the security review of 2026-10-03: required amendments A1–A11, and the optional hardening the coordinator took (see "Decisions"). **Execution ships as two PRs:** Tasks 1–4 (descriptors, listeners, health, `config.toml`), then Tasks 5–7 (the resets, the admin socket, `hennery admin`). The tasks and their replay are unchanged by the split.

Every code block below was built and tested in a scratch copy of `c622c1e`, one commit per task, and every block was generated from those commits. The plan was then replayed from its own text, task by task, onto a fresh copy of `c622c1e`:
- each block applied exactly as "Reading the steps" says;
- after every task the tree matched the scratch commit byte for byte, `Cargo.lock` included;
- after every task the replay ran fmt, both clippy runs (the second on the shipped binary, test hooks off), the workspace tests and the codegen check.

The replay ended with 472 tests, up from 441 (per task: 443, 443, 447, 454, 460, 468, 472). The timing-sensitive test binaries passed with four copies running at once, the CLI tests five rounds of four with macOS's default `TMPDIR` (`/var/folders/…`, 49 characters, as on CI) rather than the dev shell's short one. The security tests were revert-probed (each task's "Revert-probes" step). Every "Run:" of the failing-test and passing-test steps was run as written: before, on the parent commit with only the task's test files, and after, on the task's commit. Each "Expected:" is what it printed.

## Scope

3b-i's "After this plan" gives 3b-ii five pieces: several listeners, `config.toml`, the admin socket, `owner_id` on the older tables with every query filtering by it, and the health checks. With the three fold-ins the brief asks for, that is more than 7 to 9 right-sized tasks. The plan is **split** (decision 1):
- **3b-ii, this plan (7 tasks):**
  - the two descriptor fixes;
  - several listeners and the health checks;
  - `config.toml`;
  - the operator's resets;
  - the admin socket and `hennery admin`.
- **3b-iii, next (outlined in "After this plan"):** `owner_id` on `hosts`, `pairing_codes` and every sessions-store table, with every query filtering by it, the operator's own included.
- **3c, after it:** passkeys (unchanged).

**In:**
- Host: the adapter spawn closes inherited descriptors (Task 1).
- Binary:
  - `--listen-fd` checks the address family, and on macOS the TCP state (Task 2);
  - repeatable `--listen` / `--listen-fd`, `HENNERY_LISTEN`, and `up` handing every socket over (Task 3);
  - `config.toml` and `--public-url` / `HENNERY_PUBLIC_URL` (Task 4);
  - `admin.sock` bound and served, signals caught from the start (Task 6);
  - `hennery admin` (Task 7).
- Collector: `serve_on`, `serve_all`; the health routes merged outside the cookie (Task 3).
- Kernel:
  - `health.rs` and `Operator::ping` (Task 3);
  - `reset_password`, `reset_public_url`, `setup_link` and `Limiter::clear`; sessions bound to the verified PHC string (Task 5);
  - `admin.rs` (Task 6).
- Tests:
  - the route-table tests on two listeners (Task 3);
  - a pty for the confirmations (Task 7).

**Out** (see "After this plan"):
- the `owner_id` backfill (3b-iii);
- kernel §4.2's `restart pending`, and `hennery backup` / `restore` (§9), which also go through the socket;
- `PATCH /api/settings`;
- the absolute session cap (decision 16);
- a default data directory: `--data-dir` stays required, so no test can reach a real one.

**Where the hand-offs land:**

| Hand-off (3b-i "After this plan") | Here |
|---|---|
| Several listeners, the same router, `ConnectInfo`, browser rules and cookie; start fails if one is taken; route tables on every listener | Task 3 (decisions 2, 3) |
| `config.toml`, the precedence, a `public_url` there for the setup link | Task 4 (decision 4) |
| Admin socket: print the setup URL | Tasks 5, 6, 7 (decision 10) |
| Admin socket: reset the password, through the hashing permit, ending every session and its streams | Tasks 5, 6, 7 (decision 11) |
| Admin socket: reset `public_url`, replacing the cached one (3b-i decision 4) | Tasks 5, 6, 7 (decision 12) |
| Admin socket: list hosts; mint a pairing code; TTY confirmation | Tasks 6, 7 (decisions 8, 9) |
| `owner_id` on the older tables, the operator's queries included | 3b-iii (decision 1) |
| `/healthz` and `/readyz`, exempt, carrying no data | Task 3 (decision 15) |
| "`migrate_component` … matters for 3b-ii's listeners" | Not touched: the stores are still opened one after the other before any listener serves; several listeners share one `AppState` |
| Sessions have no absolute cap (an option) | Not taken (decision 16) |
| The brief's fold-ins: descriptors ≥ 3 to agents; `--listen-fd` hardening | Tasks 1, 2 (decisions 13, 14) |

## Decisions this plan makes where the spec is silent

These were confirmed by a stronger-model security review on the maintainer's behalf (2026-10-03, "after amendments"). Each gives the choice, the alternatives, and the cost if it is wrong, so a reviewer can confirm it on its own. Decisions the review changed are marked "(amended after the security review of 2026-10-03)". Items marked **(amendment)** depart from explicit spec text and should be written back into it.

**What the review changed:**
- A1 (blocking): a login in flight does not outlive a password reset; the session is bound to the PHC string its check verified (decision 11).
- A2: the descriptors that still survive are named (decision 13).
- A3: the admin socket is operator-equivalent for any same-user process (decision 5).
- A4: the client refuses a socket served by another user (decisions 5, 8).
- A5: only `ECONNREFUSED` marks a socket stale; the residuals and `flock` are recorded (decision 6).
- A6: a repeated `--listen-fd` is refused (decision 14), and `up` hands its `--public-url` on (decision 3).
- A7: `AdminResponse`'s `Debug` redacts the setup link and the pairing code (decision 8).
- A8: the echo goes off before the password prompt, and a test watches the terminal (decision 9).
- A9: the terminal check is not a barrier (decision 9).
- A10: the 3b-iii hand-off lists this plan's queries, and 3b-iii lands before the first tagged release (decision 1).
- A11: rebuilt and replayed.
- Optional hardening taken:
  - O1: state-changing admin commands logged at `warn`, with the peer's pid (decision 8);
  - O2: a pause after a failed accept (decision 8);
  - O3: descriptors closed up to the hard limit (decision 13);
  - O4: `/readyz` on a blocking thread with a 2 s timeout (decision 15);
  - O5: a `config.toml` others can write is refused (decision 4);
  - O6: dropping the `AdminSocket` removes it (decision 6);
  - O10: the answer's write is bounded too (decision 8).

  O7, O8, O9 and the residuals are recorded in "After this plan".
- D12 records the analogous `Origin` race.

1. **The split: `owner_id` goes to its own plan, 3b-iii.** (amended after the security review of 2026-10-03)
   - **Choice:** this plan leaves every older table as it is. 3b-iii adds `owner_id` to them and makes every query filter by it.
   - **Why:** two reasons.
     - Size: 3b-i put it at about 80 statements in the sessions store, plus the kernel's and the operator's own queries.
     - Design: a blocker that should be settled on its own. `hennery up` mints a pairing code, and its host enrolls, **before setup creates any owner** (`run_collector` mints right after listening; setup comes later, in the browser). The `hosts` and `pairing_codes` rows written then have no owner to carry, so the backfill needs a rule for them first. The options are in "After this plan".
   - **Alternatives:** do it here, which makes about 10 tasks and adds an unrelated design question to an already security-heavy review.
   - **Cost if wrong:** rows written until 3b-iii are backfilled by its migration. With one owner, that is one `UPDATE` per table.
   - **This plan's own queries join 3b-iii's list** (A10): `reset_password`'s and `reset_public_url`'s statements already name the owner (`WHERE owner_id = ?`), and `open_session`'s `EXISTS` on `password_credentials` does too. `Operator::ping` reads no table. 3b-iii must keep them so, and must include them in its test that every query filters.
   - **3b-iii lands before the first tagged release** (A10): no released schema may lack `owner_id`.
   - (The brief cites "decision 2 (`owner_id` everywhere)". In `docs/README.md`, decision 2 is per-hat agent config, as 3b-i noted. The `owner_id` rule is kernel §1 and umbrella §7.4.)
2. **Several listeners, one router.**
   - `hennery_sessions::serve_all` spawns one `axum::serve` per listener, each on a clone of the same `Router` with `ConnectInfo<SocketAddr>`, and every listener stops on the same `CancellationToken`. So the state, the browser rules and the cookie are one; there is nothing per listener to configure.
   - A listener whose serve fails cancels the token for the others, and the first error is returned.
   - At most `MAX_LISTENERS` (8).
   - Addresses are bound in the order given, with `std::net::TcpListener::bind`, before anything else is done: in `collector`, before the data directory is created; in `up`, before the data root is touched. A taken address fails the start and leaves nothing behind.
   - One `collector listening address=…` line is logged per listener, in that order. The setup link names the **first** listener's port, unless a `public_url` is configured (decision 4).
   - **Alternatives:** one accept loop over several sockets, behind a custom `axum::serve::Listener`: more code, and nothing gained.
   - **Cost if wrong:** 8 is arbitrary. More would need `pass_to_child`'s bounds raised (decision 3).
3. **`up` binds every address and hands each over.** (amended after the security review of 2026-10-03)
   - `up` binds all its addresses before the data root. The collector child gets them as `--listen-fd 4 --listen-fd 5 …` (`inherit::LISTENER_FD` onward), so a taken address still stops `up` before any child starts.
   - **The host child** connects over the first address that loopback reaches: the first for which `collector_ws_url(loopback_url(address))` is valid. With none, `up` fails with that check's own error ("…only allowed to a loopback address…"), as it did for a single address.
   - `pass_to_child` passes at most `MAX_PASSED` = 9 descriptors (the pairing pipe and 8 listeners), to targets 3 to 63. Each source is first moved to 64 or above, clear of every target. It still allocates nothing after the fork.
   - **`HENNERY_LISTEN` is removed from the collector child's environment.** Checked: clap counts a value that comes from the environment as present for `conflicts_with`, so a service definition that set `HENNERY_LISTEN` for `up` would make its child refuse `--listen-fd` (`the argument '--listen-fd' cannot be used with '--listen'`). The collector ignores `config.toml`'s `listen` whenever it gets `--listen-fd`.
   - **`up --public-url`** (A6): `up` takes `--public-url` and `HENNERY_PUBLIC_URL` like `collector` does, and hands a flag on to its collector child as `--public-url`. It was cheap, so it is done; `up_hands_its_public_url_to_its_collector` pins it.
   - **Alternatives:** `up` binds only the host's address and passes the others to the collector as `--listen`. A taken address would then fail after the children started, and a port-0 address could not be named to the host.
   - **Cost if wrong:** none beyond decision 2's cap.
4. **`config.toml` and the precedence (amendment to kernel §2).** (amended after the security review of 2026-10-03)
   - The file is `<data>/config.toml`, the collector's data directory (kernel §1). `up` reads `<data>/collector/config.toml` for its `listen`.
   - Two keys, `listen` (an array of addresses) and `public_url`, with `deny_unknown_fields`: a typo stops the start, naming the file. It is read, never created, before anything is bound.
   - **A file its group or others can write is refused** (O5), with an error naming the file and the fix (`chmod go-w <file>`). No secret belongs in it, but it says where the collector listens and which origin its setup link names. The mode is read from the opened file, not its name.
   - **Precedence**, as kernel §2 has it: flag, then `HENNERY_*`, then the file, then the default. Clap already takes a flag over its variable, and the file fills in what neither gave (`FileConfig::listen`, `FileConfig::public_url`). `public_url` also gets `--public-url` and `HENNERY_PUBLIC_URL`, so every setting follows the one rule.
   - **What `public_url` there does (the amendment):**
     - Kernel §2 says `public_url` lives in the database, not the file, and after setup that still holds: the stored one is in effect, and a configured one that differs is only named in a warning. That warning names `hennery admin reset-public-url`, which arrives in Task 7.
     - Before setup, the configured one is the base of the setup link, which is kernel §3.1's `<public_url or http://localhost:PORT>`. An invalid one stops the start.
   - An empty `HENNERY_LISTEN=` is refused ("an empty listen address"), not read as unset: clap hands it over as one empty value.
   - **Alternatives:**
     - The file overrides the stored `public_url`. That makes a second, silent source of truth for the `Origin` check, and moving hennery would then not end the sessions of the old origin.
     - No `public_url` in the file: behind a proxy, the setup link would name `localhost`, which the operator's browser may not reach.
   - **Cost if wrong:** an operator who edits `public_url` in the file after setup expects it to take effect and gets a warning instead. The warning says what to run.
5. **Who may use the admin socket.** (amended after the security review of 2026-10-03)
   - `admin.sock` is chmod 0600 right after it is bound. Each connection's peer user id (`peer_cred`: `SO_PEERCRED` on Linux, `getpeereid` on macOS) must equal the collector's effective user id. Otherwise it is dropped without an answer and a warning is logged.
   - Between the bind and the chmod, the socket has the umask's mode, usually one others cannot write to, and so cannot connect to. The uid check covers the rest.
   - **The client checks the server as well** (A4): `admin::request` reads the peer's user id right after connecting and refuses, sending nothing, when it is not its own effective user id ("`<socket>` is served by user id N, not by this user (M); nothing was sent"). A socket planted by another user at the path cannot collect a new password.
   - **The socket is operator-equivalent for any process of the collector's user** (A3), the agents of an all-in-one install included. Such a process can reset the password and `public_url`, mint pairing codes and read the setup link, with no password and no session. This is kernel §10's stated limit, and the terminal confirmation (decision 9) does not change it. An agent kept from the socket needs a sandbox that denies it connecting to `<data>/admin.sock`, or the collector as another OS user (kernel §10's recommendation); hennery itself does not confine a same-user agent.
   - **Not tested:** a second user id needs root. The checks are a few lines each, in `admin::answer` and `admin::request`.
   - **Alternatives:** the mode alone. It says nothing on a filesystem that ignores socket modes, and a directory left loose by the operator (the collector only warns about one) exposes the socket's name.
   - **Cost if wrong:** none known. The check only narrows who is answered.
6. **One collector per data directory, and a clean shutdown.** (amended after the security review of 2026-10-03)
   - `admin::bind` connects to an existing `admin.sock` first:
     - it answers: refused, "another collector serves this data directory";
     - it is absent: fine;
     - `ECONNREFUSED`, i.e. left by a collector that was killed: removed and bound afresh;
     - **any other error** (A5), for example `EACCES` or `ENOTSOCK` on macOS: the start fails, naming the path ("… is there and cannot be checked; remove it if no collector serves this data directory"), and the file is left alone.
   - `run_collector` binds it right after the data directory and before the database, the setup link and the pairing code. A second collector on the same directory therefore stops before it overwrites the first one's `setup-url`.
   - The socket is removed when the collector stops: `run_collector` awaits the admin task after the HTTP servers return. And dropping an `AdminSocket`, served or not, removes the file (O6), so a start that fails after the bind leaves none.
   - **Signals from the first line:** `run_collector` now creates `Signals` (`up`'s since #11) before anything else. With the old `terminated()`, whose handlers existed only once its task was first polled, a SIGTERM during the start killed the collector by the default action. That left `admin.sock` behind, and the four-copy stress run caught it: in 1 of 12 runs of the `cli` binary, `the_collectors_data_is_private_to_its_user` found the socket after the collector exited.
   - **Residuals** (A5):
     - macOS answers `ECONNREFUSED` when a live socket's listen backlog is full. A second collector started while the first is flooded with connections would take the socket for stale, remove it and bind its own.
     - Two collectors started at the same moment on an empty directory can both find no socket. The second's `bind` then fails (`EADDRINUSE`), unless the first's socket file is removed in between.
     - Linux answers `ECONNREFUSED` for a path that is not a socket, so there a stray regular file at `admin.sock` is removed.
   - **Alternatives:**
     - `flock(2)` (`libc::flock`) on a lock file in the data directory would close both residuals, since the lock dies with its process. Not taken: it adds a second file and a second rule for two narrow races that need a second collector started deliberately on the same data directory, and `flock` is unreliable on network filesystems. Recorded in "After this plan".
     - No check: two collectors on one database, each with its own setup token.
   - **Cost if wrong:** a collector that is killed leaves a stale socket, which the next start replaces.
7. **A data directory too deep for a Unix socket.**
   - A socket's path must fit `sockaddr_un.sun_path`, 104 bytes on macOS and 108 on Linux, NUL included. When `<data>/admin.sock` does not, `bind` logs a warning naming the path and returns `None`, and the collector runs without the socket.
   - **Alternatives:**
     - Fatal: a start that worked before this plan would stop over a recovery feature. CI's macOS `TMPDIR` alone is 49 characters.
     - A socket elsewhere (`/tmp`, `$XDG_RUNTIME_DIR`): a second discovery rule, and a shared directory to trust.
   - **Cost if wrong:** on such a directory, `hennery admin` says it cannot connect. The collector's log says why.
8. **The protocol.** (amended after the security review of 2026-10-03)
   - One JSON line each way; then the connection closes. The request is at most `MAX_REQUEST_BYTES` (16 KiB) and must arrive within `REQUEST_TIMEOUT` (10 s).
   - Anything else gets `Refused` with a reason and changes nothing: not JSON, an unknown command, or a line that runs past the limit.
   - Failures inside the collector answer `Failed` with the error.
   - The client (`request`, `read_answer`) reads **one line**, not to the end of the stream. On Linux, a Unix socket closed with input unread (the oversized request) resets its peer's connection, after the answer is delivered, and reading to the end would turn a delivered answer into an error there.
   - The types (`AdminRequest`, `AdminResponse`, `AdminHost`) live in `hennery_kernel::admin`, not `hennery-proto`: no browser or remote client speaks them, so there is no codegen.
   - The commands: `setup_url`, `reset_password`, `list_hosts`, `mint_pairing_code` and `reset_public_url`.
   - `AdminRequest`'s `Debug` is written by hand and leaves the password out. **So is `AdminResponse`'s** (A7), which leaves out `SetupUrl.url` and `PairingCode.code`. A `format!("{…:?}")` test pins each, as in 3b-i's A10.
   - **Logging** (O1): only the command's name is logged, never its arguments. The commands that change state or hand out a credential (`reset_password`, `reset_public_url`, `mint_pairing_code`; `AdminRequest::changes_state`) are logged at `warn` with the peer's process id, the others at `info`.
   - **Bounds:** a failed accept (out of descriptors, say) pauses 100 ms before the next, so it cannot spin (O2). Writing the answer is bounded by `REQUEST_TIMEOUT` too (O10), so a peer that never reads cannot hold a task.
   - **Cost if wrong:** a version skew between `hennery admin` and the collector answers `Refused` with serde's reason. Both come from one binary.
9. **Which commands need a confirmation, and how (kernel §4.2).** (amended after the security review of 2026-10-03)
   - `reset-password`, `reset-public-url` and `pairing-code` need a terminal on standard input and a typed `yes`. Without a terminal they refuse ("… asks for confirmation on a terminal … (nothing was sent)") and send nothing. `setup-url` and `hosts` need none.
   - **The terminal check is not a barrier** (A9): `script(1)`, `expect` or any pty passes it, as the tests do. It stops pipes, cron jobs and accidents, nothing more (decision 5).
   - `reset-public-url` counts: it ends every session, and it will change the passkey RP id in 3c (kernel §3.2).
   - **No `--yes` flag.** Kernel §4.2's confirmation exists to stop non-interactive use, and a flag would be the non-interactive path. Scripts mint pairing codes through the HTTP API, with step-up.
   - The tests drive the confirmation through a pty (`libc::openpty`, both ends close-on-exec at once), and pin the non-terminal refusal separately.
   - **The new password** is typed twice with echo off: `tcsetattr` with `TCSANOW`, so input typed ahead survives, where `TCSAFLUSH` would drop it (revert-probed: the typed-ahead pty test then hangs and is killed at 20 s).
   - **The echo goes off before the prompt is shown** (A8), so nothing typed in answer to it can be echoed. `a_password_reset_over_the_admin_socket_signs_everyone_out` types each answer only once its prompt is on standard error, reads everything the pty's master shows, and asserts the password is not there (revert-probed by leaving `ECHO` set, and by showing the prompt first).
   - The echo is restored by a `Drop` guard on every return. A SIGINT during the prompt kills the process with echo still off, and `stty echo` restores it. That is recorded, not handled (O8).
   - Two entries that differ change nothing. The password is never an argument (kernel §2).
   - **Cost if wrong:** an operator automating a reset has to script a pty. That is deliberate.
10. **`hennery admin setup-url`.**
    - It prints the live setup link. Once that link's hour is up, it issues a fresh one the way a start does: the file is rewritten and the old token dies (`Operator::setup_link`). Before this, an expired link needed a restart. Once set up, it fails with "set up already".
    - It is **printed to standard output whether or not that is a terminal.** Kernel §3.1's terminal rule keeps the token out of log collectors, and this command is an operator's explicit request, not a log line.
    - Holding it back from a pipe would not stop a same-user agent either: that agent can speak the protocol (decision 5).
    - **Cost if wrong:** a `hennery admin setup-url` run by a script whose output is logged puts the token in that log. It is single-use and dies at setup.
11. **The password reset** (`Operator::reset_password`). (amended after the security review of 2026-10-03)
    - It is validated like setup's (12 characters to 1024 bytes), and refused before setup (`NotSetUp`: setup is the way in).
    - It hashes on a blocking thread **under a `check_password` slot**, 3b-i's rule for any hash outside setup. A reset therefore cannot add a third Argon2 run to a login flood; a unit test holds both slots and sees it wait.
    - One `IMMEDIATE` transaction replaces the credential and deletes every `auth_sessions` row of the owner. Then the ending generation is bumped, so every open stream ends (3b-i decision 7).
    - **Both limiters are cleared.** The operator just proved local access. Otherwise a lockout through a shared proxy address (3b-i decision 6) would outlast the reset that was meant to fix it.
    - **A login in flight does not outlive the reset** (A1, the blocking item). A login whose Argon2 check passed on the old password just before the reset would otherwise open its session just after it, and keep a session the reset was meant to end.
      - `check_password` and `verify_password` return the PHC string they verified, not a `bool`.
      - `open_session(user_agent, verified, now)` inserts the session only `WHERE EXISTS (SELECT 1 FROM password_credentials WHERE owner_id = ? AND phc = <verified>)`, in the same statement. When the reset has replaced it, nothing is inserted and it returns `None`; login then answers 401 `invalid_password`, as for a wrong password.
      - Setup passes the PHC string it just wrote (`SetupOutcome::Done { owner_id, phc }`).
      - The testkit opens sessions the same way, through `hennery_testkit::owner_phc` (a check of the owner's password).
      - Test: `a_login_checked_before_a_password_reset_opens_no_session_after_it`, revert-probed by making the `EXISTS` always true.
      - Step-up needs no binding: the reset deletes the session a step-up would stamp.
    - **Cost if wrong:** clearing the limiters also clears a real guesser's lockout, once. The guesser needs a new password to guess.
12. **The `public_url` reset** (3b-i decision 4's recovery, `Operator::reset_public_url`). (amended after the security review of 2026-10-03)
    - It is parsed like setup's. The `settings` row is upserted and **the cached origin is replaced**, still under the connection's lock, so no request after the call sees the old origin. Every session ends and the generation is bumped.
    - **The admin server holds the router's own `Arc<Operator>`** (`Admin { operator: state.operator.clone(), … }`), never a second instance. The cached origin and the setup token live in that one; a second `Operator::open` would update the row and leave the router checking the old origin until a restart.
    - `a_moved_collector_is_recovered_over_the_admin_socket` proves it live: set up at one origin, locked out at another, reset over the socket, then signed in at the new origin, with no restart. Revert-probed by handing the admin server a second `Operator` (Task 7).
    - **Why it ends the sessions:** kernel §3.2 treats a `public_url` change as step-up-worthy (the passkey RP id and the OAuth redirect move with it), and the sessions were opened at the old origin.
    - **The analogous `Origin` race, recorded** (lower severity): `browser_rules` reads the cached origin once per request. A request that passed the check just before a reset still runs its handler with the old origin's approval, and its session is already past `require_operator`. The window is one in-flight request of a session the reset then deletes. An epoch on the cached origin, re-checked when the handler commits, would close it. Not taken; recorded in "After this plan".
    - **Cost if wrong:** a needless reset signs the owner in again.
13. **The adapter spawn closes inherited descriptors (the brief's fold-in).** (amended after the security review of 2026-10-03)
    - In `pre_exec`, every descriptor from 3 up to the first number not checked is closed if it lacks `FD_CLOEXEC`. That limit is the **hard** `RLIMIT_NOFILE` (O3), at most `MAX_CLOSED_FD` = 65 536, read before the fork, since getrlimit is not async-signal-safe. Not the soft limit: a descriptor opened while the soft limit was higher stays open once it is lowered. `a_descriptor_above_a_lowered_soft_limit_is_closed_too` pins it, in a copy of the test binary started with descriptor 60 open and the soft limit at 50.
    - Close-on-exec descriptors are left alone: std reports a failed `exec` over one of them. Only `fcntl` and `close` run after the fork.
    - This is what keeps from agents whatever the host inherits (from `up`, a service manager or a shell) and whatever another thread opens without close-on-exec meanwhile.
    - `ups_agents_never_see_the_operator_token_or_the_pairing_pipe` now starts `up` holding descriptor 7 open, and the agent must not see it.
    - **Alternatives:**
      - `closefrom`: not on macOS.
      - `close_range(CLOSE_RANGE_CLOEXEC)`: Linux 5.11 and later, in `libc` only for glibc, not musl. Linux-only code cannot be compiled here, let alone tested.
      - Listing `/dev/fd`: it allocates, which is not allowed after the fork.
    - **Cost if wrong:** up to 65 536 `fcntl` calls per adapter spawn when the hard limit is that high, as it is on macOS (unlimited) and under systemd (524 288): a few milliseconds.
    - **What still survives** (A2): a descriptor numbered at or above `min(hard limit, 65 536)`. Above the hard limit none can be opened. Between 65 536 and the hard limit one can, from a process that raised its soft limit that far. Linux's `close_range` would close them all; see "After this plan".
14. **`--listen-fd` hardening (the brief's fold-in).** (amended after the security review of 2026-10-03)
    - After `SO_TYPE`, the address family from `getsockname` must be `AF_INET` or `AF_INET6`. That refuses a listening Unix socket, which `SO_ACCEPTCONN` alone accepts on Linux.
    - On macOS, which has no `SO_ACCEPTCONN`, `TCP_CONNECTION_INFO`'s `tcpi_state` must be `TCPS_LISTEN` (1). This replaces the "bound, no peer" guess, which took a bound socket that never listened; the collector then hung on its first accept.
    - **A descriptor given twice is refused** (A6), "`--listen-fd N is given twice`": adopted twice, one socket would have two owners, and the second close would hit whatever reused the number.
    - Linux keeps `SO_ACCEPTCONN`. The non-Apple stand-in `listens_by_tcp_state` is a stub that is never reached, so the plan adds no Linux-only logic.
    - **Cost if wrong:** none known. It only refuses more.
15. **The health checks** (kernel §3.3, §8). (amended after the security review of 2026-10-03)
    - `GET /healthz` answers 200 `ok`.
    - `GET /readyz` answers 200 `ready` when `SELECT 1` answers on the operator's connection within `READY_TIMEOUT` (2 s), and 503 `not ready` otherwise, with the error logged, not shown (O4). The query runs on a blocking thread, so a database held busy does not hold up the runtime's workers or the probe.
    - **Residual (O4):** a timed-out ping keeps its blocking thread until the connection's lock frees. A prober hitting `/readyz` while the database is stuck adds a thread per probe, up to tokio's blocking pool (512).
    - Both are plain text, with no cookie and no data. They are merged outside the browser rules and the session cookie, on every listener. Other methods answer 405.
    - **Not tested:** the 503 path: making the database fail on demand needs a seam that does not exist.
    - **Cost if wrong:** `readyz` checks only the operator's connection, not the sessions store's.
16. **No absolute session cap.**
    - Considered: for example, 90 days from login, whatever the use.
    - **Not taken:**
      - This plan already gives the owner a way to end every session at once, both resets.
      - A cap changes 3b-i decision 7 and the cookie's `Max-Age` rule.
      - It is better decided with the frontend's session list, where a forced sign-out can be explained.
    - **Cost if wrong:** a stolen cookie in continuous use lives until it is revoked or a reset ends it.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`; crate names prefixed `hennery-`.
- After every task these pass:
  - `nix develop -c cargo fmt --all --check` (`max_width = 120`);
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy -p hennery --locked -- -D warnings` (test hooks off);
  - `cargo test --workspace --locked`;
  - `cargo run -p hennery-proto --bin gen -- --check`.
- A task that changes a `Cargo.toml` runs one `cargo build --workspace` **without** `--locked` first, and commits the updated `Cargo.lock`. Dependencies come from `[workspace.dependencies]` with `.workspace = true`; this plan adds no new crate.
- No wire type changes: the generated files stay as they are.
- Listeners (kernel §7): "`axum` on **one or more listeners** (`listen = ["127.0.0.1:7117"]` in `config.toml`; `--listen` repeatable; `HENNERY_LISTEN` comma-separated; default `127.0.0.1:7117`). Every listener serves the same router and the same authentication and `Origin` rules (§3.3). Start fails if any address cannot be bound." And: "**Browser access is bound to `public_url`, not to a listener.**"
- Configuration (kernel §2): "Precedence: CLI flags > environment (`HENNERY_*`) > `<data>/config.toml` > defaults. Settings that the operator edits in the UI (`public_url`, push policy) live in the database, not in the file. Secrets are never accepted as CLI flags". Decision 4 amends the `public_url` part.
- The admin socket (kernel §4.2): "The collector listens on `<data>/admin.sock` (Unix socket, mode 0600) for `hennery admin …` recovery commands (reset password, print setup URL, list hosts, restart pending) … **Destructive commands** (backup, restore, password reset, pairing-code minting) require interactive confirmation on a TTY. The confirmation is enforced by the CLI: it stops accidental and non-interactive use, not a process that speaks the socket protocol directly (§10)."
- Health (kernel §3.3): "`/healthz`, `/readyz` | None (no data) | Exempt"; §8: "Process up / database ready".
- Testing (kernel §11): "`Origin` rules per route class (§3.3), including a missing `Origin`, on every listener; start fails when one of several addresses is taken."
- **No Linux-only code:** CI runs `ubuntu-latest` and `macos-latest`, and only macOS could be compiled here. The one non-Apple item is decision 14's stub.
- No global installs: tooling comes from the flake dev shell. `/bin/sh` fd probes look only at 3 to 9: dash refuses multi-digit redirections.
- Commits follow Conventional Commits and use the repository's own identity (gmail, unsigned). Push the feature branch after every completed task; never push `main`.

## Review Focus

These are the inputs most likely to bite a real user that the obvious tests would not exercise, most likely first. Each is pinned by the named tests.

1. **A collector moved to another origin after setup** (3b-i decision 4): `public_url` is `http://127.0.0.1:7117` and the browser now comes as `http://localhost:7117`, or through a proxy.
   - Expected: every login from the new origin is refused until `hennery admin reset-public-url`, confirmed on a terminal, moves it without a restart. After that the new origin signs in, the old one is refused, and the old sessions are gone. Refusing the confirmation changes nothing.
   - Tests: Task 7 `a_moved_collector_is_recovered_over_the_admin_socket`; Task 5 `a_public_url_reset_replaces_the_cached_origin_and_ends_every_session`.
2. **A data directory deep enough to crowd the socket path:** CI's macOS `TMPDIR` is 49 characters before the test's own directories.
   - Expected: the collector still starts. Past the limit it runs without `admin.sock` and warns; within it, everything works.
   - Tests: Task 6 `a_path_too_long_for_a_socket_binds_nothing`, and every CLI test, which the replay ran with the long `TMPDIR` (five rounds of four copies).
3. **`HENNERY_LISTEN` set in `up`'s environment** (a service definition, or a Docker image's `ENV`), alongside flags.
   - Expected: `up` takes its flags, its collector child takes the handed-over sockets, and neither refuses the other's arguments.
   - Test: Task 3 `up_hands_every_listen_address_to_its_collector` (revert-probed by leaving the variable in the child's environment).
4. **A descriptor the host holds open across `exec`:** inherited from a shell, a service manager or `up`, or opened by another thread without close-on-exec.
   - Expected: no agent sees it; the agent's stdio is all it gets.
   - Tests: Task 1 `an_adapter_inherits_no_descriptor_but_its_stdio`, `a_descriptor_above_a_lowered_soft_limit_is_closed_too`, and `ups_agents_never_see_the_operator_token_or_the_pairing_pipe` with descriptor 7 held open by `up`.
5. **A stop during the collector's start, or a second collector on the same data directory.**
   - Expected: a SIGTERM at any point shuts down cleanly and leaves no `admin.sock`. A second collector stops at the socket that still answers, before it touches the first's setup link. A killed collector's socket is replaced at the next start.
   - Tests: Task 6 `a_live_socket_is_refused_and_a_stale_one_replaced`, `a_socket_that_cannot_be_checked_stops_the_start`, and `the_collectors_data_is_private_to_its_user`, which checks the socket is gone after the exit, under four parallel copies.
6. **A login racing a password reset** (added after the security review of 2026-10-03): the owner resets a leaked password while the thief's login is mid-check.
   - Expected: the thief's check passed on the old password, but no session is opened after the reset; the new password opens one.
   - Test: Task 5 `a_login_checked_before_a_password_reset_opens_no_session_after_it`.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-host/src/adapter.rs` | `close_inherited`, `fd_limit`, `MAX_CLOSED_FD` in the spawn's `pre_exec` | 1 |
| `crates/hennery/src/main.rs` | `inherited_listener`'s family and TCP-state checks; `listen_addresses`, `bind_all`, `MAX_LISTENERS`; `up`'s hand-over; `config.toml`; the admin socket's wiring; `Signals` from the start; the `admin` command | 2, 3, 4, 6, 7 |
| `crates/hennery/src/inherit.rs` | `pass_to_child` for up to `MAX_PASSED` descriptors | 3 |
| `crates/hennery/src/config.rs` | `FileConfig`: `config.toml`, and the file's place in the precedence | 4 |
| `crates/hennery/src/admin.rs` | `hennery admin …`: the confirmation, the password prompt, the output | 7 |
| `crates/hennery/Cargo.toml`, `crates/hennery-kernel/Cargo.toml`, `Cargo.lock` | `serde` and `toml` for the binary; `serde` for the kernel | 4, 6 |
| `crates/hennery-sessions/src/lib.rs` | `serve_on`, `serve_all`; the health routes in `router` | 3 |
| `crates/hennery-kernel/src/health.rs` | `/healthz`, `/readyz` | 3 |
| `crates/hennery-kernel/src/operator.rs` | `ping`; `Reset`, `reset_password`, `reset_public_url`, `setup_link`; `verify_password`/`check_password` return the PHC string; `open_session` takes it | 3, 5 |
| `crates/hennery-kernel/src/auth_api.rs` | Setup and login open their session on the verified PHC string | 5 |
| `crates/hennery-testkit/src/lib.rs` | `owner_phc`; `operator_client` opens its session on it | 5 |
| `crates/hennery-kernel/src/ratelimit.rs` | `Limiter::clear` | 5 |
| `crates/hennery-kernel/src/admin.rs` | The socket: `bind`, `serve`, `request`, the types | 6 |
| `crates/hennery-kernel/src/lib.rs` | `pub mod health`, `pub mod admin` | 3, 6 |
| Tests: `crates/hennery-host/tests/adapter.rs`, `crates/hennery-kernel/tests/{operator,admin,auth_sessions}.rs`, `crates/hennery-testkit/tests/{auth,step_up}.rs`, `crates/hennery/tests/cli.rs` | | all |

All commands run from the repository root inside the dev shell (`nix develop -c …`, or direnv). Work on a feature branch off `main` (e.g. `feat/operator-auth-2`). Each task leaves the workspace compiling, clippy-clean and green, and the binary working: `up` pairs and connects its host through every task.

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block (and a final newline).
- "Replace the whole of `path` with:" overwrites the file with the block (and a final newline).
- "Append to `path`:" adds a blank line, then the block, at the end of the file.
- "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point, as whole lines (earlier blocks of the same task already applied, in order), then "with:" and its replacement.

Other "Run:" lines only check or regenerate; they change no source file, except that `cargo build --workspace` updates `Cargo.lock`. The plan was replayed exactly this way, from its own text, onto `c622c1e`.

---

### Task 1: The adapter spawn closes every inherited descriptor

**Files:**
- Modify: `crates/hennery-host/src/adapter.rs` (`Adapter::spawn`, and two helpers above `signal_group`)
- Test: `crates/hennery-host/tests/adapter.rs`, `crates/hennery/tests/cli.rs` (`ups_agents_never_see_the_operator_token_or_the_pairing_pipe`)

**Interfaces:**
- Produces: `hennery_host::adapter::MAX_CLOSED_FD: libc::c_int = 65_536`. `fd_limit()` (the hard `RLIMIT_NOFILE`, capped) and `close_inherited(limit)` are private.
- Consumes: nothing new.

- [ ] **Step 1: Write the failing tests**

The adapter test holds a copy of `/dev/null` at descriptor 64 or above, without close-on-exec, as a leak would be, and runs `ls /dev/fd` as the agent. A plain `std::process::Command` is the control: it does see the descriptor. A second adapter test runs itself again in a copy of the test binary, with descriptor 60 open and the soft `RLIMIT_NOFILE` lowered to 50 (decision 13, O3): the limit is process-wide, so it cannot be lowered for one test in place. The CLI test starts `up` with descriptor 7 open (a `dup2` of its stderr), and the agent's probe of 3 to 9 must come back empty.

In `crates/hennery-host/tests/adapter.rs`, replace:

```rust
//! group, stderr is bounded and scrubbed, nesting variables are stripped.

use hennery_host::adapter::{Adapter, AgentCommand, STDERR_TAIL_BYTES, scrub};
use std::path::Path;
use std::time::{Duration, Instant};
```

with:

```rust
//! group, stderr is bounded and scrubbed, nesting variables are stripped,
//! and no descriptor but its stdio reaches the agent.

use hennery_host::adapter::{Adapter, AgentCommand, STDERR_TAIL_BYTES, scrub};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
```

In `crates/hennery-host/tests/adapter.rs`, replace:

```rust
        assert_eq!(scrub(input), expected, "input {input:?}");
    }
}
```

with:

```rust
        assert_eq!(scrub(input), expected, "input {input:?}");
    }
}

/// Closes a raw descriptor on drop, however the test ends.
struct CloseOnDrop(i32);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        // SAFETY: close(2) on a descriptor this test opened.
        unsafe { libc::close(self.0) };
    }
}

/// The descriptor numbers `ls /dev/fd` printed.
fn listed(out: &[u8]) -> Vec<i32> {
    String::from_utf8_lossy(out)
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// A descriptor the host holds open across `exec` (inherited from `hennery
/// up`, a service manager or a shell, or opened by another thread without
/// close-on-exec) never reaches an agent: it gets its stdio and nothing
/// else of the host's.
#[tokio::test]
async fn an_adapter_inherits_no_descriptor_but_its_stdio() {
    let file = std::fs::File::open("/dev/null").unwrap();
    // SAFETY: fcntl(2) on an open descriptor: a copy at 64 or above,
    // without close-on-exec, as a leaked one would be.
    let leaked = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD, 64) };
    assert!(leaked >= 64, "{}", std::io::Error::last_os_error());
    let _close = CloseOnDrop(leaked);
    // Control: a plain spawn passes it on.
    let control = std::process::Command::new("ls").arg("/dev/fd").output().unwrap();
    assert!(listed(&control.stdout).contains(&leaked), "{control:?}");

    let dir = tempfile::tempdir().unwrap();
    let ls = AgentCommand {
        program: "ls".into(),
        args: vec!["/dev/fd".into()],
        env: Vec::new(),
    };
    let (mut adapter, mut io) = Adapter::spawn(&ls, dir.path()).unwrap();
    let mut out = Vec::new();
    io.stdout.read_to_end(&mut out).await.unwrap();
    adapter.exited().await;
    let fds = listed(&out);
    assert!(fds.contains(&0) && fds.contains(&1), "{fds:?}");
    assert!(
        !fds.contains(&leaked),
        "the agent inherited descriptor {leaked}: {fds:?}"
    );
}

/// Set in the copy of this binary that
/// `a_descriptor_above_a_lowered_soft_limit_is_closed_too` runs.
const LOWERED_SOFT_LIMIT: &str = "HENNERY_TEST_LOWERED_SOFT_LIMIT";

/// Held open across `exec` above the lowered soft limit.
const ABOVE_THE_LIMIT: i32 = 60;

/// A descriptor opened while the soft `RLIMIT_NOFILE` was higher stays open
/// once it is lowered, so the spawn closes up to the hard limit. The test
/// runs itself again in a copy of this binary, holding descriptor 60 open
/// with the soft limit lowered to 50: the limit is process-wide, and would
/// starve the other tests here.
#[tokio::test]
async fn a_descriptor_above_a_lowered_soft_limit_is_closed_too() {
    if std::env::var_os(LOWERED_SOFT_LIMIT).is_some() {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: getrlimit(2) and fcntl(2) into and on local values.
        unsafe {
            assert_eq!(libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit), 0);
            assert!(libc::fcntl(ABOVE_THE_LIMIT, libc::F_GETFD) >= 0, "not inherited");
        }
        assert!(limit.rlim_cur < ABOVE_THE_LIMIT as libc::rlim_t, "{}", limit.rlim_cur);
        let dir = tempfile::tempdir().unwrap();
        let ls = AgentCommand {
            program: "ls".into(),
            args: vec!["/dev/fd".into()],
            env: Vec::new(),
        };
        let (mut adapter, mut io) = Adapter::spawn(&ls, dir.path()).unwrap();
        let mut out = Vec::new();
        io.stdout.read_to_end(&mut out).await.unwrap();
        adapter.exited().await;
        let fds = listed(&out);
        assert!(fds.contains(&1), "{fds:?}");
        assert!(!fds.contains(&ABOVE_THE_LIMIT), "the agent inherited it: {fds:?}");
        return;
    }
    let file = std::fs::File::open("/dev/null").unwrap();
    let fd = file.as_raw_fd();
    let mut hard = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit(2) into a local struct.
    assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut hard) }, 0);
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", "a_descriptor_above_a_lowered_soft_limit_is_closed_too"])
        .env(LOWERED_SOFT_LIMIT, "1");
    // SAFETY: dup2(2) and setrlimit(2) in the forked child, before exec;
    // nothing is allocated.
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(&mut child, move || {
            if libc::dup2(fd, ABOVE_THE_LIMIT) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let lowered = libc::rlimit {
                rlim_cur: 50,
                rlim_max: hard.rlim_max,
            };
            if libc::setrlimit(libc::RLIMIT_NOFILE, &lowered) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let out = child.output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("1 passed"), "{text}");
}
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
/// `up` and the control start with no inherited descriptor above 2: on
/// macOS, std makes a pipe or socket close-on-exec only after creating it,
/// so under parallel tests one another thread of this binary is making can
/// leak into a spawn (in 4 of 96 runs with four copies at once, the
/// control included). Those are this binary's, not `up`'s, and would pass
/// on to the agent. A fresh data directory, so this run pairs through that pipe.
```

with:

```rust
/// `up` itself is started holding descriptor 7 open across `exec`, as a
/// service manager or a shell can leave one: the host's adapter spawn must
/// close it too. The control starts with no inherited descriptor above 2:
/// on macOS, std makes a pipe or socket close-on-exec only after creating
/// it, so under parallel tests one another thread of this binary is making
/// can leak into a spawn. A fresh data directory, so this run pairs through
/// that pipe.
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
            close_leaked_descriptors();
            Ok(())
```

with:

```rust
            close_leaked_descriptors();
            if libc::dup2(2, 7) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
```

- [ ] **Step 2: Run them to see them fail**

Run: `nix develop -c cargo test -p hennery-host --test adapter -- an_adapter_inherits_no_descriptor_but_its_stdio a_descriptor_above_a_lowered_soft_limit_is_closed_too`
Expected: FAIL, both: `the agent inherited descriptor 64: [0, 1, 2, 3, 64]`, and the copy's `the agent inherited it: [0, 1, 2, 3, 60]`.

Run: `nix develop -c cargo test -p hennery --test cli ups_agents_never_see_the_operator_token_or_the_pairing_pipe`
Expected: FAIL, `left: "7\n"`, `right: ""`.

- [ ] **Step 3: Close them in the spawn**

In `crates/hennery-host/src/adapter.rs`, replace:

```rust
//! group with a scrubbed environment, capture a bounded stderr tail, watch
//! for exit, and kill the whole group — never just the direct child.
```

with:

```rust
//! group with a scrubbed environment and no inherited descriptor but its
//! stdio, capture a bounded stderr tail, watch for exit, and kill the whole
//! group — never just the direct child.
```

In `crates/hennery-host/src/adapter.rs`, replace:

```rust
            command.env_remove(var);
        }
```

with:

```rust
            command.env_remove(var);
        }
        // Read before the fork: getrlimit is not async-signal-safe.
        let limit = fd_limit();
        // SAFETY: the closure runs in the forked child before `exec` and
        // calls only `fcntl` and `close`, which are async-signal-safe; it
        // allocates nothing.
        unsafe {
            command.pre_exec(move || {
                close_inherited(limit);
                Ok(())
            });
        }
```

In `crates/hennery-host/src/adapter.rs`, replace:

```rust

fn signal_group(pgid: i32, signal: i32) {
```

with:

```rust

/// `close_inherited` checks no descriptor at or above this, whatever the
/// soft limit: a soft `RLIMIT_NOFILE` of a million (a container's default)
/// would cost every adapter spawn a million `fcntl` calls.
pub const MAX_CLOSED_FD: libc::c_int = 65_536;

/// The first descriptor number `close_inherited` does not check: the hard
/// `RLIMIT_NOFILE`, at most `MAX_CLOSED_FD`. Not the soft limit: a
/// descriptor opened while it was higher stays open once it is lowered.
fn fd_limit() -> libc::c_int {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit(2) into a local struct of the right type.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return MAX_CLOSED_FD;
    }
    limit.rlim_max.min(MAX_CLOSED_FD as libc::rlim_t) as libc::c_int
}

/// In the forked child, before `exec`: close every descriptor from 3 up to
/// `limit` that would stay open in the agent. Whatever the host inherited
/// without close-on-exec (from `hennery up`, a service manager or a shell)
/// and whatever another thread opened without it would otherwise reach
/// every agent. Descriptors that are close-on-exec already are left alone:
/// std reports a failed `exec` over one of them.
fn close_inherited(limit: libc::c_int) {
    for fd in 3..limit {
        // SAFETY: fcntl(2) and close(2) on a descriptor number of this
        // (forked) process; both are async-signal-safe.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                libc::close(fd);
            }
        }
    }
}

fn signal_group(pgid: i32, signal: i32) {
```

- [ ] **Step 4: Run them to see them pass**

Run the two commands of Step 2. Expected: PASS.

- [ ] **Step 5: Revert-probe**

- Replace `close_inherited(limit);` in the `pre_exec` closure with `let _ = limit;`. The tests of Step 2 fail as shown there. Restore it.
- In `fd_limit`, take `limit.rlim_cur` in place of `limit.rlim_max` (the soft limit). `a_descriptor_above_a_lowered_soft_limit_is_closed_too` fails: `the agent inherited it: [0, 1, 2, 3, 60]`. Restore it.

- [ ] **Step 6: The full checks**

Run the five commands of "Global Constraints". Expected: all pass; **443 tests** in the workspace.

- [ ] **Step 7: Commit**

```bash
git add crates/hennery-host crates/hennery/tests/cli.rs
git commit -m "fix(host): close every inherited descriptor above stdio in the adapter spawn"
```

### Task 2: `--listen-fd` adopts only a listening TCP socket

**Files:**
- Modify: `crates/hennery/src/main.rs` (`inherited_listener`, and the new `socket_family` and `listens_by_tcp_state` after it)
- Test: `crates/hennery/tests/cli.rs` (`the_collector_refuses_a_listen_fd_that_is_not_a_listening_socket`)

**Interfaces:**
- Produces: no new public item. `inherited_listener(fd)` refuses a non-IPv4/IPv6 socket with `--listen-fd {fd} is not a TCP socket (address family …)`, and on macOS a socket whose TCP state is not `TCPS_LISTEN` with `--listen-fd {fd} is not a listening socket`.
- Consumes: `libc::tcp_connection_info`, `libc::TCP_CONNECTION_INFO` (Apple targets only).

- [ ] **Step 1: Write the failing test**

Two new cases join the table:
- a TCP socket bound to port 0 (the system picks the port, so nothing races for it) and never listened on;
- a listening Unix socket.

In `crates/hennery/tests/cli.rs`, replace:

```rust
/// closed descriptor, a file, a UDP socket or a TCP socket that does not
/// listen is refused at once with a message naming it, before the data
/// directory is made, not adopted to abort or hang later. Also refused: a
/// standard stream's number, and `--listen` with it.
```

with:

```rust
/// closed descriptor, a file, a UDP socket, a TCP socket that does not
/// listen (never bound, or bound and never listened on) and a listening
/// Unix socket are refused at once with a message naming it, before the
/// data directory is made, not adopted to abort, hang or serve later. Also
/// refused: a standard stream's number, and `--listen` with it.
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
    let tcp: OwnedFd = unsafe { std::os::fd::FromRawFd::from_raw_fd(tcp) };
    let listening: OwnedFd = std::net::TcpListener::bind("127.0.0.1:0").unwrap().into();
```

with:

```rust
    let tcp: OwnedFd = unsafe { std::os::fd::FromRawFd::from_raw_fd(tcp) };
    // Bound to a port (port 0: the system picks one), never listened on:
    // macOS used to take this one.
    // SAFETY: socket(2) and bind(2) on a local address, owned at once.
    let bound: OwnedFd = unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
        assert!(fd >= 0);
        let mut addr: libc::sockaddr_in = std::mem::zeroed();
        addr.sin_family = libc::AF_INET as libc::sa_family_t;
        addr.sin_addr.s_addr = u32::from(std::net::Ipv4Addr::LOCALHOST).to_be();
        let rc = libc::bind(
            fd,
            (&raw const addr).cast(),
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        );
        assert_eq!(rc, 0, "{}", std::io::Error::last_os_error());
        std::os::fd::FromRawFd::from_raw_fd(fd)
    };
    let unix: OwnedFd = std::os::unix::net::UnixListener::bind(dir.join("s")).unwrap().into();
    let listening: OwnedFd = std::net::TcpListener::bind("127.0.0.1:0").unwrap().into();
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        ),
    ] {
```

with:

```rust
        ),
        (
            "a TCP socket bound and never listened on",
            Some(&bound),
            "is not a listening socket",
        ),
        ("a listening Unix socket", Some(&unix), "is not a TCP socket"),
    ] {
```

- [ ] **Step 2: Run it to see it fail**

Run: `nix develop -c cargo test -p hennery --test cli the_collector_refuses_a_listen_fd_that_is_not_a_listening_socket`
Expected on macOS: FAIL after about 15 s: `a TCP socket bound and never listened on: the collector hung`. The old guess adopts it, and the collector waits on an accept that never comes. On Linux, `SO_ACCEPTCONN` refuses that one, and the listening Unix socket is adopted instead: `a listening Unix socket: the collector hung`.

- [ ] **Step 3: Check the family and, on macOS, the TCP state**

In `crates/hennery/src/main.rs`, replace:

```rust
/// on first use, and a UDP or unconnected socket would hang it. The socket
/// is made close-on-exec and non-blocking.
```

with:

```rust
/// on first use, a UDP or unconnected socket would hang it, and a Unix
/// socket would be served as if it were TCP. The socket is made
/// close-on-exec and non-blocking.
```

In `crates/hennery/src/main.rs`, replace:

```rust
    let listening = match option(libc::SO_ACCEPTCONN) {
        Ok(value) => value == 1,
        // macOS has no `SO_ACCEPTCONN` to read. There, a socket bound to a
        // port and without a peer is taken to listen: that refuses one never
        // bound and a connected one, but not one bound and never listened on.
        Err(err) if err.raw_os_error() == Some(libc::ENOPROTOOPT) => {
            // SAFETY: getsockname/getpeername(2) into local storage of the
            // size they are told.
            unsafe {
                let mut addr: libc::sockaddr_storage = std::mem::zeroed();
                let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
                let bound = libc::getsockname(fd, (&raw mut addr).cast(), &mut len) == 0
                    && match libc::c_int::from(addr.ss_family) {
                        libc::AF_INET => (*(&raw const addr).cast::<libc::sockaddr_in>()).sin_port != 0,
                        libc::AF_INET6 => (*(&raw const addr).cast::<libc::sockaddr_in6>()).sin6_port != 0,
                        _ => false,
                    };
                let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
                let peer = libc::getpeername(fd, (&raw mut addr).cast(), &mut len) == 0;
                bound && !peer
            }
```

with:

```rust
    let family = socket_family(fd).with_context(|| format!("--listen-fd {fd}: getsockname"))?;
    if family != libc::AF_INET && family != libc::AF_INET6 {
        bail!("--listen-fd {fd} is not a TCP socket (address family {family}, not IPv4 or IPv6)");
    }
    let listening = match option(libc::SO_ACCEPTCONN) {
        Ok(value) => value == 1,
        // macOS has no `SO_ACCEPTCONN` to read; its TCP state tells.
        Err(err) if err.raw_os_error() == Some(libc::ENOPROTOOPT) => {
            listens_by_tcp_state(fd).with_context(|| format!("--listen-fd {fd}: TCP_CONNECTION_INFO"))?
```

In `crates/hennery/src/main.rs`, replace:

```rust
    Ok(listener)
}
```

with:

```rust
    Ok(listener)
}

/// The address family of socket `fd`, from getsockname(2).
fn socket_family(fd: i32) -> std::io::Result<libc::c_int> {
    // SAFETY: getsockname(2) into local storage of the size it is told;
    // all-zero bytes are a valid `sockaddr_storage`.
    unsafe {
        let mut addr: libc::sockaddr_storage = std::mem::zeroed();
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        if libc::getsockname(fd, (&raw mut addr).cast(), &mut len) < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(libc::c_int::from(addr.ss_family))
    }
}

/// Whether TCP socket `fd` listens, from its TCP state (`TCPS_LISTEN`):
/// macOS's stand-in for `SO_ACCEPTCONN`. A socket bound and never listened
/// on is refused, as is one never bound and a connected one.
#[cfg(target_vendor = "apple")]
fn listens_by_tcp_state(fd: i32) -> std::io::Result<bool> {
    /// `TCPS_LISTEN` in `<netinet/tcp_fsm.h>`.
    const TCPS_LISTEN: u8 = 1;
    // SAFETY: all-zero bytes are a valid `tcp_connection_info` (integers).
    let mut info: libc::tcp_connection_info = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::tcp_connection_info>() as libc::socklen_t;
    // SAFETY: getsockopt(2) into a local struct of the size it is told.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_CONNECTION_INFO,
            (&raw mut info).cast(),
            &mut len,
        )
    };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(info.tcpi_state == TCPS_LISTEN)
}

/// Elsewhere `SO_ACCEPTCONN` answers, so this is never reached.
#[cfg(not(target_vendor = "apple"))]
fn listens_by_tcp_state(_fd: i32) -> std::io::Result<bool> {
    Err(std::io::Error::from_raw_os_error(libc::ENOPROTOOPT))
}
```

- [ ] **Step 4: Run it to see it pass**

Run the command of Step 2. Expected: PASS.

- [ ] **Step 5: Revert-probe**

- Let `AF_UNIX` through: add `&& family != libc::AF_UNIX` to the family check. The test fails at `a listening Unix socket`. On macOS it fails because `TCP_CONNECTION_INFO` refuses it with the wrong message; on Linux because it is adopted. Restore it.
- Put the old guess back in the `ENOPROTOOPT` arm, "bound to a port and no peer", in place of `listens_by_tcp_state(fd)`. On macOS the test fails at `a TCP socket bound and never listened on: the collector hung`. Restore it.

- [ ] **Step 6: The full checks**

Run the five commands of "Global Constraints". Expected: all pass; **443 tests**.

- [ ] **Step 7: Commit**

```bash
git add crates/hennery/src/main.rs crates/hennery/tests/cli.rs
git commit -m "fix(cli): adopt --listen-fd only as a listening TCP socket, by its TCP state on macOS"
```

### Task 3: Several listeners, and `/healthz` and `/readyz`

**Files:**
- Create: `crates/hennery-kernel/src/health.rs`
- Modify: `crates/hennery-kernel/src/lib.rs`, `crates/hennery-kernel/src/operator.rs` (`ping`), `crates/hennery-sessions/src/lib.rs` (`serve`, `serve_on`, `serve_all`, `router`), `crates/hennery/src/inherit.rs` (`pass_to_child`), `crates/hennery/src/main.rs` (the args, `listen_addresses`, `bind_all`, `run_collector`, `run_up`, the unit test's `UpArgs`)
- Test: `crates/hennery-testkit/tests/auth.rs`, `crates/hennery/tests/cli.rs`

**Interfaces:**
- Produces:
  - `hennery_sessions::serve_on(listeners: Vec<tokio::net::TcpListener>, state: AppState) -> std::io::Result<()>`.
  - `hennery_sessions::serve_all(listeners: Vec<tokio::net::TcpListener>, app: Router, shutdown: CancellationToken) -> std::io::Result<()>`.
  - `serve(listener, state)` is now `serve_on(vec![listener], state)`.
  - `hennery_kernel::health::router(Arc<Operator>) -> Router`, and `health::READY_TIMEOUT` (2 s).
  - `Operator::ping(&self) -> anyhow::Result<()>`.
  - In the binary: `MAX_LISTENERS = 8`, `DEFAULT_LISTEN`, `listen_addresses(&[String]) -> Result<Vec<String>>`, `bind_all(&[String]) -> Result<Vec<std::net::TcpListener>>`, and `inherit::MAX_PASSED = 9`.
  - `CollectorArgs.listen` and `UpArgs.listen` become `Vec<String>`, and `CollectorArgs.listen_fd` becomes `Vec<i32>`; a descriptor given twice is refused.
- Consumes: Task 2's `inherited_listener`.

- [ ] **Step 1: Write the failing tests**

The testkit's `Collector` listens on two addresses. The three route-table tests run every route on both, and a new test pins the health checks there.

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
//! pins that every operator route is actually covered.
```

with:

```rust
//! pins that every operator route is actually covered, on every listener
//! (kernel spec §7, §11): the test collector listens on two.
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
    addr: SocketAddr,
```

with:

```rust
    /// The first of `addrs`.
    addr: SocketAddr,
    /// Every address it listens on, the same router on each.
    addrs: Vec<SocketAddr>,
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
```

with:

```rust
        let mut listeners = Vec::new();
        for _ in 0..2 {
            listeners.push(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap());
        }
        let addrs: Vec<SocketAddr> = listeners.iter().map(|l| l.local_addr().unwrap()).collect();
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
        let task = tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self {
            addr,
```

with:

```rust
        let task = tokio::spawn(hennery_sessions::serve_on(listeners, state.clone()));
        Self {
            addr: addrs[0],
            addrs,
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
/// Bounded, so a route that escaped the layer and streams (SSE) fails the
/// test instead of hanging it.
fn request(client: &reqwest::Client, collector: &Collector, method: &str, path: &str) -> reqwest::RequestBuilder {
    client
        .request(method.parse().unwrap(), collector.url(path))
```

with:

```rust
/// Every operator route on every listener of `collector`.
fn every_route(collector: &Collector) -> Vec<(SocketAddr, &'static str, &'static str)> {
    let on = |addr: SocketAddr| OPERATOR_ROUTES.iter().map(move |&(method, path)| (addr, method, path));
    collector.addrs.iter().copied().flat_map(on).collect()
}

/// Bounded, so a route that escaped the layer and streams (SSE) fails the
/// test instead of hanging it. To the collector's listener at `addr`.
fn request(client: &reqwest::Client, addr: SocketAddr, method: &str, path: &str) -> reqwest::RequestBuilder {
    client
        .request(method.parse().unwrap(), format!("http://{addr}{path}"))
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
    for &(method, path) in OPERATOR_ROUTES {
        let none = request(&plain, &collector, method, path)
            .header("origin", hennery_testkit::PUBLIC_URL)
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(none).await, (401, "unauthenticated".into()), "{method} {path}");
        let wrong = request(&plain, &collector, method, path)
```

with:

```rust
    for (addr, method, path) in every_route(&collector) {
        let none = request(&plain, addr, method, path)
            .header("origin", hennery_testkit::PUBLIC_URL)
            .send()
            .await
            .unwrap();
        assert_eq!(
            code_of(none).await,
            (401, "unauthenticated".into()),
            "{addr} {method} {path}"
        );
        let wrong = request(&plain, addr, method, path)
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
        assert_eq!(code_of(wrong).await, (401, "unauthenticated".into()), "{method} {path}");
        // Sanity: the owner's session gets through, proving the 401s above
        // are about the cookie and not a broken harness.
        let status = request(&signed_in, &collector, method, path)
            .send()
            .await
            .unwrap()
            .status();
        assert!(status != 401 && status != 403, "{method} {path}: {status}");
```

with:

```rust
        assert_eq!(
            code_of(wrong).await,
            (401, "unauthenticated".into()),
            "{addr} {method} {path}"
        );
        // Sanity: the owner's session gets through, proving the 401s above
        // are about the cookie and not a broken harness.
        let status = request(&signed_in, addr, method, path).send().await.unwrap().status();
        assert!(status != 401 && status != 403, "{addr} {method} {path}: {status}");
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
    for &(method, path) in OPERATOR_ROUTES {
        let send = |origin: Option<&str>, site: Option<&str>| {
            let mut req = request(&plain, &collector, method, path).header("cookie", &cookie);
```

with:

```rust
    for (addr, method, path) in every_route(&collector) {
        let send = |origin: Option<&str>, site: Option<&str>| {
            let mut req = request(&plain, addr, method, path).header("cookie", &cookie);
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
        assert_eq!(evil, (403, "origin_mismatch".into()), "{method} {path}");
```

with:

```rust
        assert_eq!(evil, (403, "origin_mismatch".into()), "{addr} {method} {path}");
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
                    "{method} {path} {site}"
                );
            }
            for site in [Some("same-origin"), Some("none"), None] {
                let status = send(None, site).await.unwrap().status();
                assert!(status != 401 && status != 403, "{method} {path} {site:?}: {status}");
            }
        } else {
            let missing = code_of(send(None, None).await.unwrap()).await;
            assert_eq!(missing, (403, "origin_mismatch".into()), "{method} {path}");
            // From the right origin, the body must still be JSON: the rules
            // refuse any other type (the code proves it is them, not an
            // extractor), and accept `application/json` with parameters.
            let with_body = |content_type: &'static str, body: &'static str| {
                request(&plain, &collector, method, path)
```

with:

```rust
                    "{addr} {method} {path} {site}"
                );
            }
            for site in [Some("same-origin"), Some("none"), None] {
                let status = send(None, site).await.unwrap().status();
                assert!(
                    status != 401 && status != 403,
                    "{addr} {method} {path} {site:?}: {status}"
                );
            }
        } else {
            let missing = code_of(send(None, None).await.unwrap()).await;
            assert_eq!(missing, (403, "origin_mismatch".into()), "{addr} {method} {path}");
            // From the right origin, the body must still be JSON: the rules
            // refuse any other type (the code proves it is them, not an
            // extractor), and accept `application/json` with parameters.
            let with_body = |content_type: &'static str, body: &'static str| {
                request(&plain, addr, method, path)
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
            assert_eq!(text, (415, "unsupported_media_type".into()), "{method} {path}");
```

with:

```rust
            assert_eq!(text, (415, "unsupported_media_type".into()), "{addr} {method} {path}");
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
                "{method} {path} application/json; charset=utf-8: {status}"
```

with:

```rust
                "{addr} {method} {path} application/json; charset=utf-8: {status}"
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
    for &(method, path) in OPERATOR_ROUTES {
        let evil = request(&plain, &collector, method, path)
            .header("origin", "https://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(evil).await, (403, "origin_mismatch".into()), "{method} {path}");
        if method == "GET" {
            let cross = request(&plain, &collector, method, path)
                .header("sec-fetch-site", "cross-site")
                .send()
                .await
                .unwrap();
            assert_eq!(code_of(cross).await, (403, "cross_site".into()), "{method} {path}");
```

with:

```rust
    for (addr, method, path) in every_route(&collector) {
        let evil = request(&plain, addr, method, path)
            .header("origin", "https://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(
            code_of(evil).await,
            (403, "origin_mismatch".into()),
            "{addr} {method} {path}"
        );
        if method == "GET" {
            let cross = request(&plain, addr, method, path)
                .header("sec-fetch-site", "cross-site")
                .send()
                .await
                .unwrap();
            assert_eq!(
                code_of(cross).await,
                (403, "cross_site".into()),
                "{addr} {method} {path}"
            );
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
    ));
    collector.stop().await;
```

with:

```rust
    ));
    collector.stop().await;
}

/// `/healthz` and `/readyz` (kernel spec §3.3, §8) answer on every listener
/// without a session, from any origin, and say nothing but a fixed word:
/// no cookie, no data.
#[tokio::test]
async fn the_health_checks_are_exempt_and_carry_no_data() {
    let collector = Collector::start().await;
    hennery_testkit::operator_client(&collector.state.operator);
    let plain = reqwest::Client::new();
    for &addr in &collector.addrs {
        for (path, word) in [("/healthz", "ok"), ("/readyz", "ready")] {
            let resp = request(&plain, addr, "GET", path)
                .header("origin", "https://evil.example")
                .header("sec-fetch-site", "cross-site")
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 200, "{addr} {path}");
            assert!(resp.headers().get("set-cookie").is_none(), "{addr} {path}");
            assert_eq!(resp.text().await.unwrap(), word, "{addr} {path}");
            let post = request(&plain, addr, "POST", path).send().await.unwrap();
            assert_eq!(post.status(), 405, "{addr} POST {path}");
        }
    }
    collector.stop().await;
```

The CLI's harness learns to read every listener's line (`listening_on(n)`). Three new tests:
- the same collector, and `public_url`'s origin, on each of two listeners;
- a taken address stopping `collector` and `up` before their data directories;
- `up` handing two sockets over, with a `HENNERY_LISTEN` in its environment that must not reach the child.

The `--listen-fd` test gains one more refused case: the same descriptor twice.

In `crates/hennery/tests/cli.rs`, replace:

```rust
        let log = self.log.clone().expect("the process's output is captured");
        let mut address = None;
        self.wait_until("the collector listening", || {
            address = std::fs::read_to_string(&log)
                .ok()
                .and_then(|text| listening_address(&text));
            address.is_some()
        });
        address.unwrap()
    }
}

/// The `address` of the first complete "collector listening" line in `log`.
fn listening_address(log: &str) -> Option<String> {
    // Complete lines only: a line still being written could end mid-port.
    let complete = &log[..log.rfind('\n')?];
    complete.lines().map(strip_ansi).find_map(|line| {
        let fields = line.split_once("collector listening")?.1;
        let address = fields
            .split_whitespace()
            .find_map(|field| field.strip_prefix("address="))?;
        Some(address.to_string())
    })
```

with:

```rust
        self.listening_on(1).remove(0)
    }

    /// The addresses of the collector's first `n` "collector listening"
    /// lines, one per listener, in the order it was given them.
    fn listening_on(&mut self, n: usize) -> Vec<String> {
        let log = self.log.clone().expect("the process's output is captured");
        let mut addresses = Vec::new();
        self.wait_until("the collector listening", || {
            addresses = std::fs::read_to_string(&log)
                .map(|text| listening_addresses(&text))
                .unwrap_or_default();
            addresses.len() >= n
        });
        addresses.truncate(n);
        addresses
    }
}

/// The `address` of every complete "collector listening" line in `log`.
fn listening_addresses(log: &str) -> Vec<String> {
    // Complete lines only: a line still being written could end mid-port.
    let Some(end) = log.rfind('\n') else {
        return Vec::new();
    };
    log[..end]
        .lines()
        .map(strip_ansi)
        .filter_map(|line| {
            let fields = line.split_once("collector listening")?.1;
            let address = fields
                .split_whitespace()
                .find_map(|field| field.strip_prefix("address="))?;
            Some(address.to_string())
        })
        .collect()
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
/// refused: a standard stream's number, and `--listen` with it.
```

with:

```rust
/// refused: a standard stream's number, `--listen` with it, and one
/// descriptor given twice.
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        &["--listen-fd", "50", "--listen", "127.0.0.1:0"],
    ] {
```

with:

```rust
        &["--listen-fd", "50", "--listen", "127.0.0.1:0"],
        &["--listen-fd", "50", "--listen-fd", "50"],
    ] {
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
    assert!(String::from_utf8_lossy(&out.stdout).contains("paired as"));
}
```

with:

```rust
    assert!(String::from_utf8_lossy(&out.stdout).contains("paired as"));
}

/// `POST path` with no body on the collector at `listen`, with the owner's
/// `session` and `Origin: origin`: the status.
fn post_from(listen: &str, path: &str, session: &str, origin: &str) -> Option<u16> {
    let mut stream = TcpStream::connect(listen).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(15))).ok()?;
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {listen}\r\nCookie: hennery_session={session}\r\nOrigin: {origin}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    response.split(' ').nth(1)?.parse().ok()
}

/// `GET path` on the collector at `listen`, without a session: the status
/// and the body.
fn get_plain(listen: &str, path: &str) -> Option<(u16, String)> {
    let mut stream = TcpStream::connect(listen).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(15))).ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {listen}\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    let (head, body) = response.split_once("\r\n\r\n")?;
    Some((head.split(' ').nth(1)?.parse().ok()?, body.to_string()))
}

/// Kernel spec §7: a collector given several addresses serves the same
/// routes and the same state on each, with the same browser rules, and
/// browser access stays bound to `public_url`: a state-changing request on
/// the second listener needs the first's origin, the one set up as
/// `public_url`, not the second's own.
#[test]
fn every_listen_address_serves_the_same_collector() {
    let dir = scratch_dir("listeners");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    let log = dir.join("collector.log");
    let collector = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["collector", "--listen", "127.0.0.1:0", "--listen", "127.0.0.1:0"])
        .arg("--data-dir")
        .arg(&data)
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
        .spawn()
        .unwrap();
    let mut collector = KillTree::new(collector, &log);
    let addresses = collector.listening_on(2);
    assert_ne!(addresses[0], addresses[1]);
    let (first, second) = (&addresses[0], &addresses[1]);
    // `public_url` is `http://<first>`.
    let session = sign_in(&mut collector, first, &data);
    for address in &addresses {
        assert_eq!(get_plain(address, "/healthz"), Some((200, "ok".into())), "{address}");
        assert!(get_json(address, "/api/hosts", &session).is_some(), "{address}");
    }
    assert_eq!(
        post_from(second, "/api/auth/logout", &session, &format!("http://{second}")),
        Some(403),
        "the second listener's own origin was taken for public_url"
    );
    assert_eq!(
        post_from(second, "/api/auth/logout", &session, &format!("http://{first}")),
        Some(204)
    );
    // The logout on the second listener ended the session on the first.
    assert!(get_json(first, "/api/hosts", &session).is_none());
}

/// Kernel spec §7, §11: start fails when any one of several addresses is
/// taken, for `collector` and for `up`, before either touches its data
/// directory.
#[test]
fn a_taken_listen_address_fails_the_start_before_the_data_dir_is_touched() {
    let dir = scratch_dir("taken");
    let _cleanup = RemoveDir(dir.clone());
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let taken = taken.local_addr().unwrap().to_string();
    for command in ["collector", "up"] {
        let data = dir.join(command);
        let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
            .args([command, "--listen", "127.0.0.1:0", "--listen", &taken])
            .arg("--data-dir")
            .arg(&data)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{command} started: {stderr}");
        assert!(stderr.contains(&format!("bind {taken}")), "{command}: {stderr}");
        assert!(!data.exists(), "{command} made its data directory");
    }
}

/// `up` binds every address and hands each to its collector child (kernel
/// spec §7): its host connects over the first, and the second serves the
/// same collector. A `HENNERY_LISTEN` in `up`'s environment (which its
/// flags override) does not reach the child, which would otherwise refuse
/// it beside `--listen-fd`.
#[test]
fn up_hands_every_listen_address_to_its_collector() {
    let dir = scratch_dir("uplisteners");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("data");
    let log = dir.join("up.log");
    let mut command = Command::new(env!("CARGO_BIN_EXE_hennery"));
    command.env("HENNERY_LISTEN", "127.0.0.1:0");
    let mut up = up_logging_to_with(command, &data, &log, &["--listen", "127.0.0.1:0"]);
    let addresses = up.listening_on(2);
    let session = sign_in(&mut up, &addresses[0], &data.join("collector"));
    up.wait_until("the host connected, seen on the second listener", || {
        matches!(
            get_json(&addresses[1], "/api/hosts", &session),
            Some(serde_json::Value::Array(hosts)) if hosts.iter().any(|h| h["connected"] == true)
        )
    });
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `nix develop -c cargo test -p hennery-testkit --test auth`
Expected: FAIL to compile: `cannot find function serve_on in crate hennery_sessions`.

Run: `nix develop -c cargo test -p hennery --test cli`
Expected: FAIL, three tests: `every_listen_address_serves_the_same_collector`, `a_taken_listen_address_fails_the_start_before_the_data_dir_is_touched` and `up_hands_every_listen_address_to_its_collector`, each with `error: the argument '--listen <LISTEN>' cannot be used multiple times`.

- [ ] **Step 3: The health routes**

Create `crates/hennery-kernel/src/health.rs`:

```rust
//! `/healthz` and `/readyz` (kernel spec §8): whether the process is up, and
//! whether its database answers. For a service manager or a load balancer:
//! they need no session, sit outside the browser rules (kernel spec §3.3),
//! and carry no data, only a fixed word.

use crate::operator::Operator;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use std::sync::Arc;
use std::time::Duration;

/// `/readyz` answers 503 when the database has not answered by then.
pub const READY_TIMEOUT: Duration = Duration::from_secs(2);

pub fn router(operator: Arc<Operator>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(ready))
        .with_state(operator)
}

/// 200 `ready` when the database answers a query within `READY_TIMEOUT`,
/// 503 `not ready` when it does not (the error is logged, not shown). The
/// query runs on a blocking thread: a database held busy must not hold up
/// the runtime's workers, nor the probe past its timeout.
async fn ready(State(operator): State<Arc<Operator>>) -> (StatusCode, &'static str) {
    let ping = tokio::task::spawn_blocking(move || operator.ping());
    let why = match tokio::time::timeout(READY_TIMEOUT, ping).await {
        Ok(Ok(Ok(()))) => return (StatusCode::OK, "ready"),
        Ok(Ok(Err(err))) => format!("{err:#}"),
        Ok(Err(err)) => err.to_string(),
        Err(_) => format!("no answer within {READY_TIMEOUT:?}"),
    };
    tracing::warn!(error = %why, "readyz: the database does not answer");
    (StatusCode::SERVICE_UNAVAILABLE, "not ready")
}
```

In `crates/hennery-kernel/src/lib.rs`, replace:

```rust
pub mod db;
pub mod hosts;
```

with:

```rust
pub mod db;
pub mod health;
pub mod hosts;
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
        Ok(self.owner_id()?.is_some())
    }
```

with:

```rust
        Ok(self.owner_id()?.is_some())
    }

    /// Whether the database answers a query (`/readyz`).
    pub fn ping(&self) -> Result<()> {
        self.conn().query_row("SELECT 1", [], |_| Ok(()))?;
        Ok(())
    }
```

- [ ] **Step 4: Serve one router on every listener**

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
    offline::after_startup(&state);
    let shutdown = state.shutdown.clone();
    // The peer address is what enrollment rate-limits on (kernel spec §4.1).
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown.cancelled_owned())
    .await
}

/// Every session, host and operator route plus the host WebSocket. Serve it with
```

with:

```rust
    serve_on(vec![listener], state).await
}

/// Serve `router(state)` on every listener until `state.shutdown` is
/// cancelled (kernel spec §7).
pub async fn serve_on(listeners: Vec<tokio::net::TcpListener>, state: AppState) -> std::io::Result<()> {
    offline::after_startup(&state);
    serve_all(listeners, router(state.clone()), state.shutdown.clone()).await
}

/// Serve `app` on every listener, the same router and the same state on
/// each (kernel spec §7), each with its peer's address: enrollment and
/// login rate-limit on it. Until `shutdown` is cancelled; a listener that
/// fails cancels it for the others too.
pub async fn serve_all(
    listeners: Vec<tokio::net::TcpListener>,
    app: Router,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    let mut serving = tokio::task::JoinSet::new();
    for listener in listeners {
        let app = app.clone().into_make_service_with_connect_info::<SocketAddr>();
        let shutdown = shutdown.clone();
        serving.spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
        });
    }
    let mut result = Ok(());
    while let Some(done) = serving.join_next().await {
        if let Err(err) = done.map_err(std::io::Error::other).and_then(|served| served) {
            shutdown.cancel();
            if result.is_ok() {
                result = Err(err);
            }
        }
    }
    result
}

/// Every session, host and operator route, the health checks and the host
/// WebSocket. Serve it with
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
        .merge(hennery_kernel::auth_api::router(state.operator.clone()))
        .merge(ws::router(state))
```

with:

```rust
        .merge(hennery_kernel::auth_api::router(state.operator.clone()))
        .merge(hennery_kernel::health::router(state.operator.clone()))
        .merge(ws::router(state))
```

- [ ] **Step 5: Hand up to nine descriptors to a child**

In `crates/hennery/src/inherit.rs`, replace:

```rust
//! listening socket `up` bound for it.
```

with:

```rust
//! listening sockets `up` bound for it.
```

In `crates/hennery/src/inherit.rs`, replace:

```rust
/// The descriptor number the collector child finds its listening socket at.
pub const LISTENER_FD: RawFd = 4;
```

with:

```rust
/// The descriptor number the collector child finds its first listening
/// socket at; the others follow it, in order.
pub const LISTENER_FD: RawFd = 4;

/// The most descriptors one child is handed: the pairing pipe's end and a
/// listening socket for each of up to `MAX_LISTENERS` addresses.
pub const MAX_PASSED: usize = 1 + crate::MAX_LISTENERS;

/// Each source is first copied to a number at or above this one, clear of
/// every target.
const MOVE_FLOOR: RawFd = 64;
```

In `crates/hennery/src/inherit.rs`, replace:

```rust
    assert!(fds.len() <= 2 && fds.iter().all(|&(_, to)| to < 10));
```

with:

```rust
    assert!(fds.len() <= MAX_PASSED && fds.iter().all(|&(_, to)| (3..MOVE_FLOOR).contains(&to)));
```

In `crates/hennery/src/inherit.rs`, replace:

```rust
            let mut moved = [0; 2];
            for (slot, &(fd, _)) in moved.iter_mut().zip(&fds) {
                *slot = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10);
```

with:

```rust
            let mut moved = [0; MAX_PASSED];
            for (slot, &(fd, _)) in moved.iter_mut().zip(&fds) {
                *slot = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, MOVE_FLOOR);
```

- [ ] **Step 6: Repeatable `--listen` and `--listen-fd`, bound before anything else**

In `crates/hennery/src/main.rs`, replace:

```rust
struct CollectorArgs {
    #[arg(long, default_value = "127.0.0.1:7117")]
    listen: String,
    #[arg(long, env = "HENNERY_DATA_DIR")]
```

with:

```rust
struct CollectorArgs {
    /// An address to listen on, e.g. `127.0.0.1:7117`. Repeatable, or
    /// comma-separated in `HENNERY_LISTEN`; the collector serves the same
    /// routes on each (kernel spec §7). Default: 127.0.0.1:7117.
    #[arg(long = "listen", env = "HENNERY_LISTEN", value_delimiter = ',')]
    listen: Vec<String>,
    #[arg(long, env = "HENNERY_DATA_DIR")]
```

In `crates/hennery/src/main.rs`, replace:

```rust
    /// `up` bound, in place of binding `--listen`.
    #[arg(long, hide = true, conflicts_with = "listen", value_parser = clap::value_parser!(i32).range(3..))]
    listen_fd: Option<i32>,
```

with:

```rust
    /// `up` bound, in place of binding `--listen`. Repeatable.
    #[arg(long = "listen-fd", hide = true, conflicts_with = "listen", value_parser = clap::value_parser!(i32).range(3..))]
    listen_fd: Vec<i32>,
```

In `crates/hennery/src/main.rs`, replace:

```rust
    #[arg(long, default_value = "127.0.0.1:7117")]
    listen: String,
```

with:

```rust
    /// An address to listen on, as for `collector`; repeatable. The host
    /// child connects over the first one that loopback reaches.
    #[arg(long = "listen", env = "HENNERY_LISTEN", value_delimiter = ',')]
    listen: Vec<String>,
```

In `crates/hennery/src/main.rs`, replace:

```rust
    Ok((name.to_string(), command))
}
```

with:

```rust
    Ok((name.to_string(), command))
}

/// Where the collector listens when nothing says otherwise (kernel spec §7).
const DEFAULT_LISTEN: &str = "127.0.0.1:7117";

/// The most addresses one collector listens on: `up` hands each to its
/// collector child as a descriptor of its own.
pub(crate) const MAX_LISTENERS: usize = 8;

/// The addresses to listen on: those given, else `DEFAULT_LISTEN`. An empty
/// one, or more than `MAX_LISTENERS`, is refused.
fn listen_addresses(given: &[String]) -> Result<Vec<String>> {
    let addresses: Vec<String> = if given.is_empty() {
        vec![DEFAULT_LISTEN.to_string()]
    } else {
        given.iter().map(|a| a.trim().to_string()).collect()
    };
    if addresses.iter().any(String::is_empty) {
        bail!("an empty listen address: give each as host:port");
    }
    if addresses.len() > MAX_LISTENERS {
        bail!(
            "{} listen addresses; at most {MAX_LISTENERS} are supported",
            addresses.len()
        );
    }
    Ok(addresses)
}

/// Bind every address, in order (kernel spec §7): one that cannot be bound
/// fails the start, before anything else is done.
fn bind_all(addresses: &[String]) -> Result<Vec<std::net::TcpListener>> {
    addresses
        .iter()
        .map(|address| {
            let listener = std::net::TcpListener::bind(address).with_context(|| format!("bind {address}"))?;
            listener.set_nonblocking(true)?;
            Ok(listener)
        })
        .collect()
}
```

In `crates/hennery/src/main.rs`, replace:

```rust
    // Checked before anything is created: a descriptor that is not a
    // listening TCP socket must fail here, and clearly.
    let inherited = args.listen_fd.map(inherited_listener).transpose()?;
```

with:

```rust
    // Before anything is created: a descriptor that is not a listening TCP
    // socket, or an address that is taken, must fail here, and clearly.
    let listeners = if args.listen_fd.is_empty() {
        bind_all(&listen_addresses(&args.listen)?)?
    } else {
        if args.listen_fd.len() > MAX_LISTENERS {
            bail!("more than {MAX_LISTENERS} --listen-fd");
        }
        // One socket adopted twice would be two owners of one descriptor.
        for (i, fd) in args.listen_fd.iter().enumerate() {
            if args.listen_fd[..i].contains(fd) {
                bail!("--listen-fd {fd} is given twice");
            }
        }
        args.listen_fd
            .iter()
            .map(|&fd| inherited_listener(fd))
            .collect::<Result<_>>()?
    };
```

In `crates/hennery/src/main.rs`, replace:

```rust
    let listener = match inherited {
        Some(listener) => {
            tokio::net::TcpListener::from_std(listener).context("the listening socket `up` handed over")?
        }
        None => tokio::net::TcpListener::bind(&args.listen)
            .await
            .with_context(|| format!("bind {}", args.listen))?,
    };
    let address = listener.local_addr()?;
    tracing::info!(%address, "collector listening");
    // Only once listening: the link names the port (kernel spec §3.1).
    let base_url = format!("http://localhost:{}", address.port());
```

with:

```rust
    let listeners = listeners
        .into_iter()
        .map(tokio::net::TcpListener::from_std)
        .collect::<std::io::Result<Vec<_>>>()
        .context("serve the listening sockets")?;
    // One line per listener, in order: the first names the setup link's port.
    let mut addresses = Vec::new();
    for listener in &listeners {
        let address = listener.local_addr()?;
        tracing::info!(%address, "collector listening");
        addresses.push(address);
    }
    // Only once listening: the link names the port (kernel spec §3.1).
    let base_url = format!("http://localhost:{}", addresses[0].port());
```

In `crates/hennery/src/main.rs`, replace:

```rust
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(state.shutdown.clone().cancelled_owned())
        .await?;
```

with:

```rust
    hennery_sessions::serve_all(listeners, app, state.shutdown.clone()).await?;
```

In `crates/hennery/src/main.rs`, replace:

```rust
    // Validated before any child starts: a non-loopback `--listen` (or
    // another scheme it cannot make sense of) must fail here, not after the
    // collector is already up and serving.
    hennery_host::pairing::collector_ws_url(&loopback_url(&args.listen))?;
    // Bound here and handed to the collector child, so the host's URL names
    // the port the collector serves on, also for `--listen` port 0. Before
    // the data root is touched: a busy port leaves nothing behind.
    let listener = std::net::TcpListener::bind(&args.listen).with_context(|| format!("bind {}", args.listen))?;
    let collector_url = loopback_url(&listener.local_addr()?.to_string());
```

with:

```rust
    let addresses = listen_addresses(&args.listen)?;
    // Validated before any child starts: with no address that loopback
    // reaches (or only schemes it cannot make sense of) `up` fails here,
    // not after the collector is already up and serving.
    let Some(host_listener) = addresses
        .iter()
        .position(|address| hennery_host::pairing::collector_ws_url(&loopback_url(address)).is_ok())
    else {
        let err = hennery_host::pairing::collector_ws_url(&loopback_url(&addresses[0]))
            .expect_err("no address passed the check");
        return Err(err.context("the all-in-one host reaches its collector over loopback"));
    };
    // Bound here and handed to the collector child, so the host's URL names
    // the port the collector serves on, also for `--listen` port 0. Before
    // the data root is touched: a busy port leaves nothing behind.
    let listeners = bind_all(&addresses)?;
    let collector_url = loopback_url(&listeners[host_listener].local_addr()?.to_string());
```

In `crates/hennery/src/main.rs`, replace:

```rust
    collector_cmd
        .args(["collector", "--listen-fd", &inherit::LISTENER_FD.to_string()])
        .arg("--data-dir")
        .arg(args.data_dir.join("collector"))
        // `up` has warned about it already; the collector has no use for it.
        .env_remove(DEV_TOKEN_VAR)
        .kill_on_drop(true)
        .process_group(0);
    let mut fds = vec![(listener.as_raw_fd(), inherit::LISTENER_FD)];
```

with:

```rust
    collector_cmd.arg("collector");
    let mut fds = Vec::new();
    for (listener, to) in listeners.iter().zip(inherit::LISTENER_FD..) {
        collector_cmd.arg("--listen-fd").arg(to.to_string());
        fds.push((listener.as_raw_fd(), to));
    }
    collector_cmd
        .arg("--data-dir")
        .arg(args.data_dir.join("collector"))
        // `up` has warned about it already; the collector has no use for it.
        .env_remove(DEV_TOKEN_VAR)
        // `up` bound these addresses already; the child takes the sockets.
        .env_remove("HENNERY_LISTEN")
        .kill_on_drop(true)
        .process_group(0);
```

In `crates/hennery/src/main.rs`, replace:

```rust
    // The collector holds the socket now. Kept open here, it would hold the
    // port after the collector exits.
    drop(listener);
```

with:

```rust
    // The collector holds the sockets now. Kept open here, they would hold
    // the ports after the collector exits.
    drop(listeners);
```

In `crates/hennery/src/main.rs`, replace:

```rust
            listen: "127.0.0.1:7117".into(),
```

with:

```rust
            listen: vec!["127.0.0.1:7117".into()],
```

- [ ] **Step 7: Run them to see them pass**

Run the two commands of Step 2. Expected: PASS.

- [ ] **Step 8: Revert-probes**

- In `serve_all`, serve only the first listener (`for listener in listeners.into_iter().take(1)`). The four route-table and health tests in `auth.rs` fail on the second address. Restore it.
- Comment out `.env_remove("HENNERY_LISTEN")` in `run_up`. `up_hands_every_listen_address_to_its_collector` fails: the child logs `the argument '--listen-fd <LISTEN_FD>' cannot be used with '--listen <LISTEN>'`. Restore it.
- In `run_collector`, replace `bail!("--listen-fd {fd} is given twice");` with `let _ = fd;`. `the_collector_refuses_a_listen_fd_that_is_not_a_listening_socket` fails: the collector adopts descriptor 50 twice and serves, so `wait_with_timeout` gives up after 15 s (`called Option::unwrap() on a None value`). Restore it.

- [ ] **Step 9: The full checks**

Run the five commands of "Global Constraints". Expected: all pass; **447 tests**.

- [ ] **Step 10: Commit**

```bash
git add crates/hennery-kernel crates/hennery-sessions crates/hennery-testkit crates/hennery
git commit -m "feat(collector): listen on several addresses, and serve /healthz and /readyz"
```

### Task 4: `config.toml`, under the environment and the flags

**Files:**
- Create: `crates/hennery/src/config.rs` (with its unit tests)
- Modify: `crates/hennery/Cargo.toml`, `Cargo.lock`, `crates/hennery/src/main.rs` (`mod config`, `--public-url`, `run_collector`, `warn_if_public_url_differs`, `run_up`)
- Test: `crates/hennery/tests/cli.rs`

**Interfaces:**
- Produces:
  - `config::CONFIG_FILE = "config.toml"`.
  - `config::FileConfig { listen: Option<Vec<String>>, public_url: Option<String> }`, with `parse(&str)`, `load(&Path) -> Result<FileConfig>` (which refuses a file its group or others can write), `listen(&self, given: &[String]) -> Vec<String>` and `public_url(&self, given: Option<&str>) -> Option<String>`.
  - `CollectorArgs.public_url` and `UpArgs.public_url`: `Option<String>` (`--public-url`, `HENNERY_PUBLIC_URL`). `up` hands its flag on to the collector child.
- Consumes: Task 3's `listen_addresses` and `bind_all`; `hennery_kernel::operator::PublicUrl`.

- [ ] **Step 1: Write the failing tests**

The CLI tests count the collector's listeners (the file names three, the environment two, a flag one) and read the setup link's origin. Each run is stopped after its setup link is written, since that link is written after every listener's line. A second test pins a typo, an invalid `public_url` and a file others can write (0666) stopping the start before the database is made. A third starts `up --public-url` and reads its collector's setup link.

In `crates/hennery/tests/cli.rs`, replace:

```rust
    });
}
```

with:

```rust
    });
}

/// Kernel spec §2: `config.toml` gives `listen` and `public_url` when
/// neither a flag nor the environment does; `HENNERY_LISTEN` and
/// `HENNERY_PUBLIC_URL` win over the file, and flags over both. Counted by
/// the collector's listeners (the file names three, the environment two, a
/// flag one) and by the setup link, which names `public_url`.
#[test]
fn config_toml_yields_to_the_environment_and_the_environment_to_flags() {
    let dir = scratch_dir("config");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("config.toml"),
        "listen = [\"127.0.0.1:0\", \"127.0.0.1:0\", \"127.0.0.1:0\"]\npublic_url = \"https://file.example\"\n",
    )
    .unwrap();
    // Private whatever the umask: a file others can write is refused.
    std::fs::set_permissions(
        data.join("config.toml"),
        std::os::unix::fs::PermissionsExt::from_mode(0o644),
    )
    .unwrap();
    let run = |env: &[(&str, &str)], args: &[&str], name: &str| {
        let log = dir.join(format!("{name}.log"));
        let collector = Command::new(env!("CARGO_BIN_EXE_hennery"))
            .arg("collector")
            .args(args)
            .arg("--data-dir")
            .arg(&data)
            .envs(env.iter().copied())
            .stdout(std::fs::File::create(&log).unwrap())
            .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
            .spawn()
            .unwrap();
        let mut collector = KillTree::new(collector, &log);
        let file = data.join("setup-url");
        // Written after every listener's line is logged.
        collector.wait_until("the setup link", || file.exists());
        let link = std::fs::read_to_string(&file).unwrap();
        let listeners = listening_addresses(&std::fs::read_to_string(&log).unwrap()).len();
        unsafe { libc::kill(collector.up.id() as i32, libc::SIGTERM) };
        assert!(wait_with_timeout(&mut collector.up, Duration::from_secs(15)).is_some());
        let _ = std::fs::remove_file(&file);
        let origin = link.split("/setup#").next().unwrap().to_string();
        (listeners, origin)
    };
    assert_eq!(run(&[], &[], "file"), (3, "https://file.example".to_string()));
    let env = [
        ("HENNERY_LISTEN", "127.0.0.1:0,127.0.0.1:0"),
        ("HENNERY_PUBLIC_URL", "https://env.example"),
    ];
    assert_eq!(run(&env, &[], "env"), (2, "https://env.example".to_string()));
    assert_eq!(
        run(
            &env,
            &["--listen", "127.0.0.1:0", "--public-url", "https://flag.example"],
            "flag"
        ),
        (1, "https://flag.example".to_string())
    );
}

/// A `config.toml` that does not parse, names a key hennery does not know,
/// or that other users can write, stops the start with the file's name,
/// before anything is bound or created.
#[test]
fn a_bad_config_toml_stops_the_start() {
    let dir = scratch_dir("badconfig");
    let _cleanup = RemoveDir(dir.clone());
    for (name, text, mode, expected) in [
        ("typo", "listens = [\"127.0.0.1:0\"]\n", 0o644, "unknown field"),
        (
            "public_url",
            "public_url = \"http://hennery.example\"\n",
            0o644,
            "public_url must be https://",
        ),
        ("writable", "listen = [\"127.0.0.1:0\"]\n", 0o666, "chmod go-w"),
    ] {
        use std::os::unix::fs::PermissionsExt;
        let data = dir.join(name);
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("config.toml"), text).unwrap();
        std::fs::set_permissions(data.join("config.toml"), std::fs::Permissions::from_mode(mode)).unwrap();
        // On port 0, and bounded: a collector that ignored the file would
        // serve rather than stop, and never on the default port.
        let mut child = Command::new(env!("CARGO_BIN_EXE_hennery"))
            .args(["collector", "--listen", "127.0.0.1:0", "--data-dir"])
            .arg(&data)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let status = wait_with_timeout(&mut child, Duration::from_secs(15));
        let _ = child.kill();
        let _ = child.wait();
        let mut stderr = String::new();
        child.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
        assert!(status.is_some_and(|s| !s.success()), "{name}: started: {stderr}");
        assert!(stderr.contains(expected), "{name}: {stderr}");
        assert!(!data.join("hennery.db").exists(), "{name}: the database was made");
    }
}

/// `up --public-url` reaches its collector child, whose setup link names it
/// (kernel spec §3.1).
#[test]
fn up_hands_its_public_url_to_its_collector() {
    let dir = scratch_dir("uppublicurl");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("data");
    let mut up = up_logging_to_with(
        Command::new(env!("CARGO_BIN_EXE_hennery")),
        &data,
        &dir.join("up.log"),
        &["--public-url", "https://up.example"],
    );
    let file = data.join("collector").join("setup-url");
    up.wait_until("the setup link", || file.exists());
    let link = std::fs::read_to_string(&file).unwrap();
    assert!(link.starts_with("https://up.example/setup#"), "{link}");
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `nix develop -c cargo test -p hennery --test cli -- config_toml up_hands_its_public_url_to_its_collector`
Expected: FAIL, all three.
- `config_toml_yields_…` fails with `left: (1, "http://localhost:7117")`, `right: (3, "https://file.example")`. The file is ignored, so its run without flags listens on the default 127.0.0.1:7117; if that port is taken, it fails waiting for the setup link instead.
- `a_bad_config_toml_stops_the_start` fails after 15 s, `typo: started`: the collector serves instead of stopping. It runs on port 0 and is killed, so it never hangs.
- `up_hands_its_public_url_to_its_collector` fails: `up` does not know `--public-url` (`unexpected argument '--public-url'`), and the wait for the setup link finds `up` exited.

- [ ] **Step 3: The file and its place in the precedence**

In `crates/hennery/Cargo.toml`, replace:

```toml
tokio.workspace = true
```

with:

```toml
serde.workspace = true
tokio.workspace = true
toml.workspace = true
```

Run: `nix develop -c cargo build --workspace` (without `--locked`): `Cargo.lock` gains `serde` and `toml` in `hennery`'s dependencies.

Create `crates/hennery/src/config.rs`:

```rust
//! `<data>/config.toml` (kernel spec §1, §2): the collector's settings that
//! are not the operator's to edit in the UI. Precedence, per setting:
//! command-line flags, then the environment (`HENNERY_*`), then this file,
//! then the defaults. Flags and the environment are clap's: it takes a
//! flag over its variable. This file fills in what neither gave.
//!
//! ```toml
//! listen = ["127.0.0.1:7117", "100.64.0.7:7117"]
//! public_url = "https://hennery.example"
//! ```
//!
//! An unknown key is refused, so a typo fails the start instead of being
//! ignored. No secret belongs here, but the file says where the collector
//! listens and which origin its setup link names: one that other users can
//! write is refused.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// The file's name in the collector's data directory.
pub const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    /// The addresses to listen on (kernel spec §7).
    pub listen: Option<Vec<String>>,
    /// Where browsers reach the collector, until setup stores its own
    /// (kernel spec §3.1): the setup link names it.
    pub public_url: Option<String>,
}

impl FileConfig {
    pub fn parse(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }

    /// `dir/config.toml`, or the defaults when there is none. Reading it
    /// creates nothing. A file that its group or others can write is
    /// refused: checked on the file that is read, not on its name.
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(CONFIG_FILE);
        let mut file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(err).with_context(|| format!("read {}", path.display())),
        };
        let mode = file.metadata()?.permissions().mode() & 0o777;
        if mode & 0o022 != 0 {
            bail!(
                "{} is writable by other users (mode {mode:o}); run `chmod go-w {}`",
                path.display(),
                path.display()
            );
        }
        let mut text = String::new();
        file.read_to_string(&mut text)
            .with_context(|| format!("read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("{}", path.display()))
    }

    /// The listen addresses: `given` (a flag or `HENNERY_LISTEN`) if any,
    /// else the file's. Empty means the default.
    pub fn listen(&self, given: &[String]) -> Vec<String> {
        if given.is_empty() {
            self.listen.clone().unwrap_or_default()
        } else {
            given.to_vec()
        }
    }

    /// `public_url`: `given` (a flag or `HENNERY_PUBLIC_URL`) if any, else
    /// the file's.
    pub fn public_url(&self, given: Option<&str>) -> Option<String> {
        given.map(str::to_string).or_else(|| self.public_url.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_holds_listen_and_public_url_and_nothing_else() {
        let config = FileConfig::parse(
            "listen = [\"127.0.0.1:7117\", \"[::1]:7117\"]\npublic_url = \"https://hennery.example\"\n",
        )
        .unwrap();
        assert_eq!(
            config,
            FileConfig {
                listen: Some(vec!["127.0.0.1:7117".into(), "[::1]:7117".into()]),
                public_url: Some("https://hennery.example".into()),
            }
        );
        assert_eq!(FileConfig::parse("").unwrap(), FileConfig::default());
        let typo = FileConfig::parse("listens = [\"127.0.0.1:7117\"]\n").unwrap_err();
        assert!(format!("{typo:#}").contains("unknown field"), "{typo:#}");
        assert!(FileConfig::parse("listen = \"127.0.0.1:7117\"\n").is_err());
    }

    #[test]
    fn a_missing_file_is_the_defaults() {
        let dir = std::env::temp_dir().join(format!("hennery-config-missing-{}", std::process::id()));
        assert_eq!(FileConfig::load(&dir).unwrap(), FileConfig::default());
        assert!(!dir.exists(), "loading made the directory");
    }

    #[test]
    fn a_file_others_can_write_is_refused() {
        let dir = std::env::temp_dir().join(format!("hennery-config-mode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CONFIG_FILE);
        std::fs::write(&path, "listen = [\"127.0.0.1:7117\"]\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(FileConfig::load(&dir).unwrap().listen.is_some());
        for mode in [0o664, 0o646] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            let err = FileConfig::load(&dir).unwrap_err();
            assert!(format!("{err:#}").contains("chmod go-w"), "{mode:o}: {err:#}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn flags_and_the_environment_win_over_the_file() {
        let file = FileConfig {
            listen: Some(vec!["127.0.0.1:1".into()]),
            public_url: Some("https://file.example".into()),
        };
        assert_eq!(file.listen(&[]), ["127.0.0.1:1"]);
        assert_eq!(file.listen(&["127.0.0.1:2".into()]), ["127.0.0.1:2"]);
        assert_eq!(file.public_url(None).as_deref(), Some("https://file.example"));
        assert_eq!(
            file.public_url(Some("https://flag.example")).as_deref(),
            Some("https://flag.example")
        );
        let empty = FileConfig::default();
        assert!(empty.listen(&[]).is_empty());
        assert_eq!(empty.public_url(None), None);
    }
}
```

In `crates/hennery/src/main.rs`, replace:

```rust

mod inherit;
```

with:

```rust

mod config;
mod inherit;
```

In `crates/hennery/src/main.rs`, replace:

```rust
use hennery_kernel::operator::{Operator, SetupLink};
```

with:

```rust
use hennery_kernel::operator::{Operator, PublicUrl, SetupLink};
```

In `crates/hennery/src/main.rs`, replace:

```rust
    /// routes on each (kernel spec §7). Default: 127.0.0.1:7117.
    #[arg(long = "listen", env = "HENNERY_LISTEN", value_delimiter = ',')]
    listen: Vec<String>,
    #[arg(long, env = "HENNERY_DATA_DIR")]
    data_dir: PathBuf,
```

with:

```rust
    /// routes on each (kernel spec §7). Else `listen` in `config.toml`, else
    /// 127.0.0.1:7117.
    #[arg(long = "listen", env = "HENNERY_LISTEN", value_delimiter = ',')]
    listen: Vec<String>,
    #[arg(long, env = "HENNERY_DATA_DIR")]
    data_dir: PathBuf,
    /// Where browsers reach the collector, for the setup link until setup
    /// stores its own (kernel spec §3.1). Else `public_url` in
    /// `config.toml`.
    #[arg(long, env = "HENNERY_PUBLIC_URL")]
    public_url: Option<String>,
```

In `crates/hennery/src/main.rs`, replace:

```rust
    /// An address to listen on, as for `collector`; repeatable. The host
    /// child connects over the first one that loopback reaches.
    #[arg(long = "listen", env = "HENNERY_LISTEN", value_delimiter = ',')]
    listen: Vec<String>,
```

with:

```rust
    /// An address to listen on, as for `collector`; repeatable, else
    /// `listen` in the collector's `config.toml` (`<data-dir>/collector`).
    /// The host child connects over the first one that loopback reaches.
    #[arg(long = "listen", env = "HENNERY_LISTEN", value_delimiter = ',')]
    listen: Vec<String>,
    /// As for `collector`, handed on to the collector child.
    #[arg(long, env = "HENNERY_PUBLIC_URL")]
    public_url: Option<String>,
```

In `crates/hennery/src/main.rs`, replace:

```rust
    // Before anything is created: a descriptor that is not a listening TCP
    // socket, or an address that is taken, must fail here, and clearly.
    let listeners = if args.listen_fd.is_empty() {
        bind_all(&listen_addresses(&args.listen)?)?
```

with:

```rust
    let file = config::FileConfig::load(&args.data_dir)?;
    let public_url = file
        .public_url(args.public_url.as_deref())
        .map(|url| PublicUrl::parse(&url).map_err(|why| anyhow::anyhow!("{why}")))
        .transpose()?;
    // Before anything is created: a descriptor that is not a listening TCP
    // socket, or an address that is taken, must fail here, and clearly.
    let listeners = if args.listen_fd.is_empty() {
        bind_all(&listen_addresses(&file.listen(&args.listen))?)?
```

In `crates/hennery/src/main.rs`, replace:

```rust
    // Only once listening: the link names the port (kernel spec §3.1).
    let base_url = format!("http://localhost:{}", addresses[0].port());
```

with:

```rust
    // Only once listening: the link names the first listener's port, unless
    // there is a `public_url` to name (kernel spec §3.1).
    let base_url = match &public_url {
        Some(url) => url.origin().to_string(),
        None => format!("http://localhost:{}", addresses[0].port()),
    };
    warn_if_public_url_differs(state.operator.public_url().as_ref(), public_url.as_ref());
```

In `crates/hennery/src/main.rs`, replace:

```rust
             the setup link and a password; remove it from the environment"
        );
```

with:

```rust
             the setup link and a password; remove it from the environment"
        );
    }
}

/// Once set up, the stored `public_url` is the one in effect (kernel spec
/// §2): a configured one that differs is named in a warning, not used.
fn warn_if_public_url_differs(stored: Option<&PublicUrl>, configured: Option<&PublicUrl>) {
    if let (Some(stored), Some(configured)) = (stored, configured)
        && stored != configured
    {
        tracing::warn!(
            stored = stored.origin(),
            configured = configured.origin(),
            "the configured public_url is not the one setup stored, which stays in effect; \
             to move hennery, reset it with `hennery admin reset-public-url`"
        );
```

In `crates/hennery/src/main.rs`, replace:

```rust
    let addresses = listen_addresses(&args.listen)?;
```

with:

```rust
    let file = config::FileConfig::load(&args.data_dir.join("collector"))?;
    let addresses = listen_addresses(&file.listen(&args.listen))?;
```

In `crates/hennery/src/main.rs`, replace:

```rust
        .process_group(0);
    if let Some((_, writer)) = &pairing {
```

with:

```rust
        .process_group(0);
    if let Some(public_url) = &args.public_url {
        collector_cmd.arg("--public-url").arg(public_url);
    }
    if let Some((_, writer)) = &pairing {
```

In `crates/hennery/src/main.rs`, replace:

```rust
            listen: vec!["127.0.0.1:7117".into()],
            data_dir: "/nonexistent".into(),
```

with:

```rust
            listen: vec!["127.0.0.1:7117".into()],
            public_url: None,
            data_dir: "/nonexistent".into(),
```

- [ ] **Step 4: Run them to see them pass**

Run: `nix develop -c cargo test -p hennery`
Expected: PASS, the four unit tests in `config.rs` included.

- [ ] **Step 5: Revert-probe**

In `FileConfig::load`, make the mode check `if mode & 0o022 != 0 && false`. `a_file_others_can_write_is_refused` fails (and `a_bad_config_toml_stops_the_start` at `writable`). Restore it.

- [ ] **Step 6: The full checks**

Run the five commands of "Global Constraints". Expected: all pass; **454 tests**.

- [ ] **Step 7: Commit**

```bash
git add Cargo.lock crates/hennery
git commit -m "feat(cli): read listen and public_url from config.toml, under the environment and flags"
```

### Task 5: The operator's resets, and the setup link again

**Files:**
- Modify: `crates/hennery-kernel/src/ratelimit.rs` (`Limiter::clear`), `crates/hennery-kernel/src/operator.rs` (`Reset`, `announced`, `setup_link`, `reset_password`, `reset_public_url`, `sessions_ended`, the PHC string through `verify_password`, `check_password`, `set_up` and `open_session`, and a unit test), `crates/hennery-kernel/src/auth_api.rs` (setup and login open their session on the PHC string), `crates/hennery-testkit/src/lib.rs` (`owner_phc`)
- Test: `crates/hennery-kernel/tests/operator.rs`, `crates/hennery-kernel/tests/auth_sessions.rs`, `crates/hennery-testkit/tests/step_up.rs`, `crates/hennery-testkit/tests/auth.rs` (every session opened on the owner's PHC string)

**Interfaces:**
- Produces (`hennery_kernel::operator`):
  - `enum Reset { Done { sessions_ended: usize }, NotSetUp, Invalid(String) }`;
  - `async fn reset_password(self: &Arc<Self>, password: String, now: i64) -> Result<Reset>`;
  - `fn reset_public_url(&self, input: &str) -> Result<Reset>`;
  - `fn setup_link(&self, dir: &Path, base_url: &str, now: i64) -> Result<Option<SetupLink>>`;
  - `Limiter::clear(&self)`.
  - Changed (decision 11, A1): `verify_password(&self, &str) -> Result<Option<String>>` and `check_password(self: &Arc<Self>, String) -> Result<Option<String>>` return the PHC string they verified; `open_session(&self, user_agent: &str, verified: &str, now: i64) -> Result<Option<String>>` opens a session only while `verified` is still the owner's; `SetupOutcome::Done { owner_id, phc }`.
  - `hennery_testkit::owner_phc(&Operator) -> String`: the owner's PHC string, from a check of `OWNER_PASSWORD`.

  `Operator.setup_file` becomes `announced: Mutex<Option<SetupLink>>`.
- Consumes: 3b-i's `hashing` semaphore, `password_problem`, `PublicUrl`, `announce_setup`, the `ended` watch.

- [ ] **Step 1: Write the failing tests**

The tests of 3b-i that open sessions now pass the PHC string a password check returned, and a new one pins A1: a check, then a reset, then `open_session` with the stale PHC string.

In `crates/hennery-kernel/tests/operator.rs`, replace:

```rust
//! one-time setup token, the owner's password and the `public_url`.

use hennery_kernel::operator::{
    MAX_PASSWORD_BYTES, Operator, PublicUrl, SETUP_TOKEN_TTL_SECS, SETUP_URL_FILE, SetupOutcome,
```

with:

```rust
//! one-time setup token, the owner's password and the `public_url`, and
//! the admin socket's recoveries (kernel spec §4.2).

use hennery_kernel::operator::{
    MAX_PASSWORD_BYTES, Operator, PublicUrl, Reset, SETUP_TOKEN_TTL_SECS, SETUP_URL_FILE, SetupOutcome,
```

In `crates/hennery-kernel/tests/operator.rs`, replace:

```rust
    let SetupOutcome::Done { owner_id } = op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap() else {
```

with:

```rust
    let SetupOutcome::Done { owner_id, .. } = op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap()
    else {
```

In `crates/hennery-kernel/tests/operator.rs`, replace:

```rust
    assert!(!op.verify_password(PASSWORD).unwrap());
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap();
    assert!(op.verify_password(PASSWORD).unwrap());
    assert!(!op.verify_password("correct horse battery!").unwrap());
    assert!(!op.verify_password(&"x".repeat(MAX_PASSWORD_BYTES + 1)).unwrap());
```

with:

```rust
    assert_eq!(op.verify_password(PASSWORD).unwrap(), None);
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap() else {
        panic!("setup failed");
    };
    assert_eq!(op.verify_password(PASSWORD).unwrap(), Some(phc));
    assert_eq!(op.verify_password("correct horse battery!").unwrap(), None);
    assert_eq!(op.verify_password(&"x".repeat(MAX_PASSWORD_BYTES + 1)).unwrap(), None);
```

In `crates/hennery-kernel/tests/operator.rs`, replace:

```rust
        results.push(check.await.unwrap());
```

with:

```rust
        results.push(check.await.unwrap().is_some());
```

In `crates/hennery-kernel/tests/operator.rs`, replace:

```rust
    assert!(!shown.contains(token) && shown.contains(SETUP_URL_FILE), "{shown}");
}
```

with:

```rust
    assert!(!shown.contains(token) && shown.contains(SETUP_URL_FILE), "{shown}");
}

/// The admin socket's password reset (kernel spec §4.2): the new password
/// verifies and the old does not, every session ends (and the ending is
/// announced, so their streams end), and the login and step-up lockouts
/// are lifted. A password setup would refuse changes nothing, and there is
/// nothing to reset before setup.
#[tokio::test]
async fn a_password_reset_replaces_the_password_and_ends_every_session() {
    let op = Arc::new(Operator::open_in_memory().unwrap());
    assert_eq!(
        op.reset_password("a new long password".into(), NOW).await.unwrap(),
        Reset::NotSetUp
    );
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap() else {
        panic!("setup failed");
    };
    let sessions = [
        op.open_session("a", &phc, NOW).unwrap().unwrap(),
        op.open_session("b", &phc, NOW).unwrap().unwrap(),
    ];
    let peer: std::net::IpAddr = "192.0.2.1".parse().unwrap();
    for _ in 0..5 {
        let _ = op.login_limiter.attempt(peer, std::time::Instant::now());
        let _ = op.step_up_limiter.attempt(peer, std::time::Instant::now());
    }
    assert!(op.login_limiter.attempt(peer, std::time::Instant::now()).is_err());
    let ends = op.session_ends();

    assert_eq!(
        op.reset_password("short".into(), NOW).await.unwrap(),
        Reset::Invalid("the password must be at least 12 characters".into())
    );
    assert!(op.verify_password(PASSWORD).unwrap().is_some());
    assert!(op.authenticate(&sessions[0], NOW).unwrap().is_some());

    assert_eq!(
        op.reset_password("a new long password".into(), NOW + 1).await.unwrap(),
        Reset::Done { sessions_ended: 2 }
    );
    assert!(op.verify_password("a new long password").unwrap().is_some());
    assert!(op.verify_password(PASSWORD).unwrap().is_none());
    for session in &sessions {
        assert!(op.authenticate(session, NOW + 1).unwrap().is_none());
    }
    assert!(ends.has_changed().unwrap(), "the ending was not announced");
    assert!(op.login_limiter.attempt(peer, std::time::Instant::now()).is_ok());
    assert!(op.step_up_limiter.attempt(peer, std::time::Instant::now()).is_ok());
}

/// The admin socket's `public_url` reset (3b decision 4's recovery): the
/// origin every browser request is checked against changes at once, not
/// only the stored row (a new `Operator` on the file reads it back), and
/// every session ends. An invalid URL changes nothing.
#[test]
fn a_public_url_reset_replaces_the_cached_origin_and_ends_every_session() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let op = Operator::open(&db).unwrap();
    assert_eq!(op.reset_public_url("https://moved.example").unwrap(), Reset::NotSetUp);
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = op.set_up(&token, PASSWORD, "http://localhost:7117", NOW).unwrap() else {
        panic!("setup failed");
    };
    let session = op.open_session("browser", &phc, NOW).unwrap().unwrap();
    let ends = op.session_ends();

    let refused = op.reset_public_url("http://moved.example").unwrap();
    assert!(matches!(refused, Reset::Invalid(_)), "{refused:?}");
    assert_eq!(op.public_url().unwrap().origin(), "http://localhost:7117");
    assert!(op.authenticate(&session, NOW).unwrap().is_some());

    assert_eq!(
        op.reset_public_url("https://Moved.Example/").unwrap(),
        Reset::Done { sessions_ended: 1 }
    );
    assert_eq!(op.public_url().unwrap().origin(), "https://moved.example");
    assert!(op.public_url().unwrap().is_https());
    assert!(op.authenticate(&session, NOW).unwrap().is_none());
    assert!(ends.has_changed().unwrap(), "the ending was not announced");
    drop(op);
    let reopened = Operator::open(&db).unwrap();
    assert_eq!(reopened.public_url().unwrap().origin(), "https://moved.example");
}

/// The admin socket's `setup-url` (kernel spec §4.2): the link announced
/// at start while its token is live, a fresh one (and a fresh file) once it
/// has expired, and none once set up.
#[test]
fn the_setup_link_is_the_live_one_or_a_fresh_one() {
    let dir = tempfile::tempdir().unwrap();
    let op = Operator::open_in_memory().unwrap();
    let base = "http://localhost:7117";
    let first = op.announce_setup(dir.path(), base, NOW).unwrap().unwrap();
    let again = op.setup_link(dir.path(), base, NOW + 1).unwrap().unwrap();
    assert_eq!(again, first);
    let later = NOW + SETUP_TOKEN_TTL_SECS;
    let fresh = op.setup_link(dir.path(), base, later).unwrap().unwrap();
    assert_ne!(fresh.url, first.url);
    assert_eq!(
        std::fs::read_to_string(dir.path().join(SETUP_URL_FILE))
            .unwrap()
            .trim_end(),
        fresh.url
    );
    let token = fresh.url.rsplit_once('#').unwrap().1;
    assert!(matches!(
        op.set_up(token, PASSWORD, "https://hennery.example", later).unwrap(),
        SetupOutcome::Done { .. }
    ));
    assert_eq!(op.setup_link(dir.path(), base, later).unwrap(), None);
    assert!(!dir.path().join(SETUP_URL_FILE).exists());
}

/// 3b-ii decision 11, amended (A1): a login whose password check ran just
/// before a reset gets no session afterwards. The session is opened on the
/// PHC string the check verified, and the reset replaced it; a check
/// against the new password opens one.
#[tokio::test]
async fn a_login_checked_before_a_password_reset_opens_no_session_after_it() {
    let op = Arc::new(Operator::open_in_memory().unwrap());
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap();
    let stale = op
        .check_password(PASSWORD.into())
        .await
        .unwrap()
        .expect("the old password");
    assert_eq!(
        op.reset_password("a new long password".into(), NOW).await.unwrap(),
        Reset::Done { sessions_ended: 0 }
    );
    assert_eq!(op.open_session("browser", &stale, NOW).unwrap(), None);
    assert!(op.sessions(NOW).unwrap().is_empty());
    let fresh = op
        .check_password("a new long password".into())
        .await
        .unwrap()
        .expect("the new password");
    assert!(op.open_session("browser", &fresh, NOW).unwrap().is_some());
}
```

In `crates/hennery-kernel/tests/auth_sessions.rs`, replace:

```rust
    MAX_SESSION_COOKIES, Operator, SESSION_SLIDE_SECS, SESSION_TTL_SECS, STEP_UP_SECS, cleared_cookie, session_cookie,
    session_tokens,
};

const NOW: i64 = 1_800_000_000;

fn set_up(op: &Operator) {
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    op.set_up(&token, "correct horse battery", "https://hennery.example", NOW)
        .unwrap();
```

with:

```rust
    MAX_SESSION_COOKIES, Operator, SESSION_SLIDE_SECS, SESSION_TTL_SECS, STEP_UP_SECS, SetupOutcome, cleared_cookie,
    session_cookie, session_tokens,
};

const NOW: i64 = 1_800_000_000;

/// Set `op` up; the stored PHC string, which a session is opened on.
fn set_up(op: &Operator) -> String {
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = op
        .set_up(&token, "correct horse battery", "https://hennery.example", NOW)
        .unwrap()
    else {
        panic!("setup failed");
    };
    phc
```

In `crates/hennery-kernel/tests/auth_sessions.rs`, replace:

```rust
    assert_eq!(op.open_session("browser", NOW).unwrap(), None);
```

with:

```rust
    assert_eq!(op.open_session("browser", "", NOW).unwrap(), None);
```

In `crates/hennery-kernel/tests/auth_sessions.rs`, replace:

```rust
    let op = Operator::open_in_memory().unwrap();
    set_up(&op);
    let token = op.open_session("browser", NOW).unwrap().unwrap();
    assert_eq!(token.len(), 64);
```

with:

```rust
    let op = Operator::open_in_memory().unwrap();
    let phc = set_up(&op);
    let token = op.open_session("browser", &phc, NOW).unwrap().unwrap();
    assert_eq!(token.len(), 64);
```

In `crates/hennery-kernel/tests/auth_sessions.rs`, replace:

```rust
    let op = Operator::open(&db).unwrap();
    set_up(&op);
    let token = op.open_session("browser", NOW).unwrap().unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
```

with:

```rust
    let op = Operator::open(&db).unwrap();
    let phc = set_up(&op);
    let token = op.open_session("browser", &phc, NOW).unwrap().unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
```

In `crates/hennery-kernel/tests/auth_sessions.rs`, replace:

```rust
    set_up(&op);
    let token = op.open_session("browser", NOW).unwrap().unwrap();
```

with:

```rust
    let phc = set_up(&op);
    let token = op.open_session("browser", &phc, NOW).unwrap().unwrap();
```

In `crates/hennery-kernel/tests/auth_sessions.rs`, replace:

```rust
    set_up(&op);
    let phone = op.open_session("phone\u{7}", NOW).unwrap().unwrap();
    let laptop = op.open_session(&"L".repeat(300), NOW + 10).unwrap().unwrap();
```

with:

```rust
    let phc = set_up(&op);
    let phone = op.open_session("phone\u{7}", &phc, NOW).unwrap().unwrap();
    let laptop = op.open_session(&"L".repeat(300), &phc, NOW + 10).unwrap().unwrap();
```

In `crates/hennery-testkit/tests/step_up.rs`, replace:

```rust
        self.state
            .operator
            .open_session("test", unix_now() - age)
```

with:

```rust
        let phc = hennery_testkit::owner_phc(&self.state.operator);
        self.state
            .operator
            .open_session("test", &phc, unix_now() - age)
```

In `crates/hennery-testkit/tests/step_up.rs`, replace:

```rust

/// A logged request must never show the password.
```

with:

```rust

/// The admin socket's resets (kernel spec §4.2) end every session, and so
/// every stream a session holds open: a password reset, then a
/// `public_url` reset.
#[tokio::test]
async fn a_reset_ends_every_session_and_its_streams() {
    let c = Collector::start().await;
    let streams = [
        open_stream(&c, &c.session(0)).await,
        open_stream(&c, &c.session(0)).await,
    ];
    let reset = c
        .state
        .operator
        .reset_password("a new long password".into(), unix_now())
        .await
        .unwrap();
    assert_eq!(reset, hennery_kernel::operator::Reset::Done { sessions_ended: 3 });
    for stream in streams {
        assert!(
            ends(stream, Duration::from_secs(1)).await,
            "a stream outlived the password reset"
        );
    }

    // A session opened on the new password.
    let phc = c
        .state
        .operator
        .verify_password("a new long password")
        .unwrap()
        .unwrap();
    let session = c
        .state
        .operator
        .open_session("test", &phc, unix_now())
        .unwrap()
        .unwrap();
    let stream = open_stream(&c, &session).await;
    let reset = c.state.operator.reset_public_url(PUBLIC_URL).unwrap();
    assert_eq!(reset, hennery_kernel::operator::Reset::Done { sessions_ended: 1 });
    assert!(
        ends(stream, Duration::from_secs(1)).await,
        "a stream outlived the public_url reset"
    );
}

/// A logged request must never show the password.
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
        format!("http://{}{path}", self.addr)
    }
```

with:

```rust
        format!("http://{}{path}", self.addr)
    }

    /// A new session of the owner's, last checked at `at`, opened as a
    /// login would.
    fn session_at(&self, at: i64) -> String {
        let phc = hennery_testkit::owner_phc(&self.state.operator);
        self.state.operator.open_session("test", &phc, at).unwrap().unwrap()
    }
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
    let token = collector
        .state
        .operator
        .open_session("test", hennery_kernel::secret::unix_now())
        .unwrap()
        .unwrap();
```

with:

```rust
    let token = collector.session_at(hennery_kernel::secret::unix_now());
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
    let fresh = collector.state.operator.open_session("test", now).unwrap().unwrap();
```

with:

```rust
    let fresh = collector.session_at(now);
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
    assert!(resp.headers().get("set-cookie").is_none());

    let stale = collector
        .state
        .operator
        .open_session("test", now - 120)
        .unwrap()
        .unwrap();
    let resp = reqwest::Client::new()
        .get(&url)
```

with:

```rust
    assert!(resp.headers().get("set-cookie").is_none());

    let stale = collector.session_at(now - 120);
    let resp = reqwest::Client::new()
        .get(&url)
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
    let real = collector.state.operator.open_session("test", now).unwrap().unwrap();
```

with:

```rust
    let real = collector.session_at(now);
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
    let stale = collector
        .state
        .operator
        .open_session("test", now - 120)
        .unwrap()
        .unwrap();
```

with:

```rust
    let stale = collector.session_at(now - 120);
```

- [ ] **Step 2: Run them to see them fail**

Run: `nix develop -c cargo test -p hennery-kernel --test operator`
Expected: FAIL to compile: `unresolved import hennery_kernel::operator::Reset`, `no method named reset_password`, `reset_public_url`, `setup_link`; `this method takes 2 arguments but 3 arguments were supplied` (`open_session`); `can't compare bool with Option<_>` (`verify_password`); `Done` has no field `phc`.

- [ ] **Step 3: The resets**

In `crates/hennery-kernel/src/ratelimit.rs`, replace:

```rust
        self.state.lock().expect("limiter lock").entries.remove(&key(addr));
    }
```

with:

```rust
        self.state.lock().expect("limiter lock").entries.remove(&key(addr));
    }

    /// Forget every address and the overflow budget: a lockout ends now.
    pub fn clear(&self) {
        let mut state = self.state.lock().expect("limiter lock");
        state.entries.clear();
        state.overflow = None;
    }
```

The unit test at the end holds both hashing slots and checks that a reset does not hash until one is free. Its 200 ms wait can only make it pass falsely under load, never fail falsely.

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
//!   on a blocking thread and at most `MAX_CONCURRENT_HASHES` at once.
//!
```

with:

```rust
//!   on a blocking thread and at most `MAX_CONCURRENT_HASHES` at once.
//! - **Recovery** (the admin socket, kernel spec §4.2): a password reset
//!   and a `public_url` reset each end every session, and so its streams.
//!
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
        owner_id: String,
    },
```

with:

```rust
        owner_id: String,
        /// The PHC string just stored: the session setup opens is bound to
        /// it (`open_session`).
        phc: String,
    },
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
    /// not used up.
    Invalid(String),
```

with:

```rust
    /// not used up.
    Invalid(String),
}

/// The outcome of `Operator::reset_password` and `reset_public_url`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reset {
    /// Done; this many signed-in sessions were ended.
    Done { sessions_ended: usize },
    /// There is no owner to reset yet: setup is the way in.
    NotSetUp,
    /// The new password or `public_url` is not acceptable (why); nothing
    /// changed.
    Invalid(String),
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
    /// The `setup-url` file, removed once setup is done.
    setup_file: Mutex<Option<PathBuf>>,
```

with:

```rust
    /// The link last written to `setup-url` (and so the file), removed
    /// once setup is done.
    announced: Mutex<Option<SetupLink>>,
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
    max_in_flight: std::sync::atomic::AtomicUsize,
}
```

with:

```rust
    max_in_flight: std::sync::atomic::AtomicUsize,
    /// Password resets that have started hashing (`reset_password`'s
    /// permit, pinned by the unit tests below).
    #[cfg(test)]
    reset_hashes: AtomicU64,
}
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
            setup_file: Mutex::new(None),
```

with:

```rust
            announced: Mutex::new(None),
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
            max_in_flight: Default::default(),
        })
```

with:

```rust
            max_in_flight: Default::default(),
            #[cfg(test)]
            reset_hashes: Default::default(),
        })
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
        *self.setup_file.lock().expect("setup file lock") = Some(file.clone());
        Ok(Some(SetupLink { url, file }))
```

with:

```rust
        let link = SetupLink { url, file };
        *self.announced.lock().expect("announced lock") = Some(link.clone());
        Ok(Some(link))
    }

    /// The setup link for the admin socket's `setup-url` (kernel spec
    /// §4.2): the one last announced while its token is live, else a fresh
    /// one, announced as `announce_setup` does (the old token dies). `None`
    /// once set up.
    pub fn setup_link(&self, dir: &Path, base_url: &str, now: i64) -> Result<Option<SetupLink>> {
        if self.is_set_up()? {
            return Ok(None);
        }
        let live = self
            .setup
            .lock()
            .expect("setup lock")
            .as_ref()
            .is_some_and(|t| t.expires_at > now);
        if live && let Some(link) = self.announced.lock().expect("announced lock").clone() {
            return Ok(Some(link));
        }
        self.announce_setup(dir, base_url, now)
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
        if let Some(file) = self.setup_file.lock().expect("setup file lock").take() {
            remove_setup_file(&file);
        }
        *self.public_url.write().expect("public_url lock") = Some(public_url);
        Ok(SetupOutcome::Done { owner_id })
    }

    /// Whether `password` is the owner's. Before setup it is checked
    /// against a dummy hash, so the answer takes as long either way.
    /// Blocking: prefer `check_password`.
    pub fn verify_password(&self, password: &str) -> Result<bool> {
```

with:

```rust
        if let Some(link) = self.announced.lock().expect("announced lock").take() {
            remove_setup_file(&link.file);
        }
        *self.public_url.write().expect("public_url lock") = Some(public_url);
        Ok(SetupOutcome::Done { owner_id, phc })
    }

    /// Whether `password` is the owner's: the PHC string it verified
    /// against, or `None`. Before setup it is checked against a dummy hash,
    /// so the answer takes as long either way. A session opened on the
    /// strength of it passes the PHC to `open_session`, so a reset landing
    /// in between wins (3b-ii decision 11). Blocking: prefer
    /// `check_password`.
    pub fn verify_password(&self, password: &str) -> Result<Option<String>> {
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
            return Ok(false);
```

with:

```rust
            return Ok(None);
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
        Ok(password_auth::verify_password(password, &phc).is_ok())
```

with:

```rust
        Ok(password_auth::verify_password(password, &phc).is_ok().then_some(phc))
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
    pub async fn check_password(self: &Arc<Self>, password: String) -> Result<bool> {
```

with:

```rust
    pub async fn check_password(self: &Arc<Self>, password: String) -> Result<Option<String>> {
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
        self.verifications.load(Ordering::Relaxed)
    }
```

with:

```rust
        self.verifications.load(Ordering::Relaxed)
    }

    /// Replace the owner's password (the admin socket's reset, kernel spec
    /// §4.2) and end every signed-in session, so their streams end too. The
    /// hash runs on a blocking thread and takes a `check_password` slot like
    /// any other, so a reset cannot add a third Argon2 run to a login
    /// flood. The limiters are cleared: the operator proved local access.
    pub async fn reset_password(self: &Arc<Self>, password: String, now: i64) -> Result<Reset> {
        if let Some(problem) = password_problem(&password) {
            return Ok(Reset::Invalid(problem));
        }
        let Some(owner_id) = self.owner_id()? else {
            return Ok(Reset::NotSetUp);
        };
        let permit = self
            .hashing
            .clone()
            .acquire_owned()
            .await
            .context("the hashing semaphore is closed")?;
        #[cfg(test)]
        let this = self.clone();
        let phc = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            #[cfg(test)]
            this.reset_hashes.fetch_add(1, Ordering::SeqCst);
            password_auth::generate_hash(password)
        })
        .await?;
        let ended = {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE password_credentials SET phc = ?2, updated_at = ?3 WHERE owner_id = ?1",
                params![owner_id, phc, now],
            )?;
            let ended = tx.execute("DELETE FROM auth_sessions WHERE owner_id = ?1", [&owner_id])?;
            tx.commit()?;
            ended
        };
        self.sessions_ended();
        self.login_limiter.clear();
        self.step_up_limiter.clear();
        Ok(Reset::Done { sessions_ended: ended })
    }

    /// Replace `public_url` (the admin socket's recovery when the collector
    /// moved, 3b decision 4): the stored row and the origin every browser
    /// request is checked against, which is cached here. Every signed-in
    /// session ends: they were opened at the old origin, and the new one
    /// signs in afresh.
    pub fn reset_public_url(&self, input: &str) -> Result<Reset> {
        let public_url = match PublicUrl::parse(input) {
            Ok(url) => url,
            Err(problem) => return Ok(Reset::Invalid(problem)),
        };
        let Some(owner_id) = self.owner_id()? else {
            return Ok(Reset::NotSetUp);
        };
        let ended = {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO settings(owner_id, key, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT(owner_id, key) DO UPDATE SET value = excluded.value",
                params![owner_id, PUBLIC_URL_KEY, public_url.origin()],
            )?;
            let ended = tx.execute("DELETE FROM auth_sessions WHERE owner_id = ?1", [&owner_id])?;
            tx.commit()?;
            // Still under the connection's lock: no other reset lands
            // between the row and the cache.
            *self.public_url.write().expect("public_url lock") = Some(public_url);
            ended
        };
        self.sessions_ended();
        Ok(Reset::Done { sessions_ended: ended })
    }

    /// Wake every stream held open by a session: some have ended.
    fn sessions_ended(&self) {
        self.ended.send_modify(|generation| *generation += 1);
    }
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
    /// checked, so the session starts stepped up. `None` before setup.
    pub fn open_session(&self, user_agent: &str, now: i64) -> Result<Option<String>> {
```

with:

```rust
    /// checked, so the session starts stepped up. `verified` is the PHC
    /// string that check verified against (`verify_password`): the session
    /// is opened only while it is still the owner's, so a login whose check
    /// raced a password reset gets no session (`None`), like one before
    /// setup.
    pub fn open_session(&self, user_agent: &str, verified: &str, now: i64) -> Result<Option<String>> {
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
        conn.execute(
            "INSERT INTO auth_sessions(id_hash, owner_id, user_agent, created_at, last_seen_at, last_step_up_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?4, ?4, ?5)",
```

with:

```rust
        let opened = conn.execute(
            "INSERT INTO auth_sessions(id_hash, owner_id, user_agent, created_at, last_seen_at, last_step_up_at, expires_at)
             SELECT ?1, ?2, ?3, ?4, ?4, ?4, ?5
             WHERE EXISTS (SELECT 1 FROM password_credentials WHERE owner_id = ?2 AND phc = ?6)",
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
                now + SESSION_TTL_SECS
            ],
        )?;
        Ok(Some(token))
```

with:

```rust
                now + SESSION_TTL_SECS,
                verified
            ],
        )?;
        Ok((opened > 0).then_some(token))
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
            self.ended.send_modify(|generation| *generation += 1);
        }
        Ok(changed > 0)
    }

    /// Changes whenever a session ends (`revoke_session`, and so logout).
```

with:

```rust
            self.sessions_ended();
        }
        Ok(changed > 0)
    }

    /// Changes whenever a session ends (`revoke_session`, and so logout,
    /// and the resets).
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
            assert!(check.await.unwrap());
        }
        assert_eq!(op.verifications(), 8);
        assert!(op.max_in_flight.load(Ordering::SeqCst) <= MAX_CONCURRENT_HASHES);
```

with:

```rust
            assert!(check.await.unwrap().is_some());
        }
        assert_eq!(op.verifications(), 8);
        assert!(op.max_in_flight.load(Ordering::SeqCst) <= MAX_CONCURRENT_HASHES);
    }

    /// A password reset hashes only once it holds a slot: with both taken
    /// (two verifies of a login flood, say) it waits, and runs once one is
    /// free.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_password_reset_hashes_only_with_a_hashing_slot() {
        let op = set_up();
        let held = op
            .hashing
            .clone()
            .acquire_many_owned(MAX_CONCURRENT_HASHES as u32)
            .await
            .unwrap();
        let reset = {
            let op = op.clone();
            tokio::spawn(async move { op.reset_password("a new long password".into(), NOW).await })
        };
        // Load can only make this pass falsely (the reset not yet started),
        // never fail falsely.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(op.reset_hashes.load(Ordering::SeqCst), 0, "hashed without a slot");
        drop(held);
        assert_eq!(reset.await.unwrap().unwrap(), Reset::Done { sessions_ended: 0 });
        assert_eq!(op.reset_hashes.load(Ordering::SeqCst), 1);
```

Setup and login open their session on the PHC string, and the testkit does the same:

In `crates/hennery-kernel/src/auth_api.rs`, replace:

```rust
    match outcome {
        Ok(Ok(SetupOutcome::Done { owner_id })) => {
            tracing::info!(%owner_id, public_url = %public_url.origin(), "hennery set up");
```

with:

```rust
    let phc = match outcome {
        Ok(Ok(SetupOutcome::Done { owner_id, phc })) => {
            tracing::info!(%owner_id, public_url = %public_url.origin(), "hennery set up");
            phc
```

In `crates/hennery-kernel/src/auth_api.rs`, replace:

```rust
    }
    let token = match operator.open_session(&user_agent(&headers), unix_now()) {
        Ok(Some(token)) => token,
        Ok(None) => return internal(anyhow::anyhow!("no owner right after setup")),
```

with:

```rust
    };
    let token = match operator.open_session(&user_agent(&headers), &phc, unix_now()) {
        Ok(Some(token)) => token,
        Ok(None) => return internal(anyhow::anyhow!("the password changed right after setup")),
```

In `crates/hennery-kernel/src/auth_api.rs`, replace:

```rust
    match operator.check_password(req.password).await {
        Ok(true) => {}
        Ok(false) => return error(StatusCode::UNAUTHORIZED, "invalid_password", "wrong password"),
        Err(err) => return internal(err),
    }
    operator.login_limiter.succeeded(peer.ip());
    match operator.open_session(&user_agent(&headers), unix_now()) {
```

with:

```rust
    let phc = match operator.check_password(req.password).await {
        Ok(Some(phc)) => phc,
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "invalid_password", "wrong password"),
        Err(err) => return internal(err),
    };
    operator.login_limiter.succeeded(peer.ip());
    // Bound to the password just checked: a reset since then wins.
    match operator.open_session(&user_agent(&headers), &phc, unix_now()) {
```

In `crates/hennery-kernel/src/auth_api.rs`, replace:

```rust
        Ok(true) => {}
        Ok(false) => return error(StatusCode::UNAUTHORIZED, "invalid_password", "wrong password"),
```

with:

```rust
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "invalid_password", "wrong password"),
```

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust

/// A client signed in as the owner of `operator`'s collector (kernel spec
```

with:

```rust

/// The owner's PHC string, from a check of `OWNER_PASSWORD` as a login
/// makes: what `Operator::open_session` opens a session on.
pub fn owner_phc(operator: &hennery_kernel::operator::Operator) -> String {
    operator
        .verify_password(OWNER_PASSWORD)
        .unwrap()
        .expect("the owner's password")
}

/// A client signed in as the owner of `operator`'s collector (kernel spec
```

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust
    let token = operator.open_session("hennery-testkit", now).unwrap().unwrap();
```

with:

```rust
    let token = operator
        .open_session("hennery-testkit", &owner_phc(operator), now)
        .unwrap()
        .unwrap();
```

- [ ] **Step 4: Run them to see them pass**

Run: `nix develop -c cargo test -p hennery-kernel -p hennery-testkit --test operator --test step_up --test auth_sessions --test auth` and `nix develop -c cargo test -p hennery-kernel --lib operator`
Expected: PASS.

- [ ] **Step 5: Revert-probes**

- In `reset_password`, replace the `acquire_owned` statement with `let permit = ();`. `a_password_reset_hashes_only_with_a_hashing_slot` fails: `hashed without a slot`, `left: 1`. Restore it.
- In `reset_public_url`, replace the cache write with `let _ = public_url;`. `a_public_url_reset_replaces_the_cached_origin_and_ends_every_session` fails: `left: "http://localhost:7117"`. Restore it.
- In `reset_password`, remove `self.sessions_ended();`. `a_reset_ends_every_session_and_its_streams` fails: `a stream outlived the password reset`. Restore it.
- In `open_session`, make the guard always true: `WHERE ?6 = ?6 OR EXISTS (…)` (dropping the clause outright leaves `?6` unbound, which rusqlite refuses for another reason). `a_login_checked_before_a_password_reset_opens_no_session_after_it` fails: `left: Some("…")`, `right: None`. Restore it.

- [ ] **Step 6: The full checks**

Run the five commands of "Global Constraints". Expected: all pass; **460 tests**.

- [ ] **Step 7: Commit**

```bash
git add crates/hennery-kernel crates/hennery-testkit
git commit -m "feat(kernel): reset the password and public_url, ending every session, and re-show the setup link"
```

### Task 6: The admin socket

**Files:**
- Create: `crates/hennery-kernel/src/admin.rs`
- Modify: `crates/hennery-kernel/Cargo.toml`, `Cargo.lock`, `crates/hennery-kernel/src/lib.rs`, `crates/hennery/src/main.rs` (`run_collector`: `Signals` first, the socket bound after the data directory and served on the router's own `Operator` and `Hosts`, and awaited at shutdown; `Signals`' doc)
- Test: `crates/hennery-kernel/tests/admin.rs`, `crates/hennery/tests/cli.rs` (`the_collectors_data_is_private_to_its_user`)

**Interfaces:**
- Produces (`hennery_kernel::admin`):
  - consts `ADMIN_SOCKET = "admin.sock"`, `MAX_REQUEST_BYTES = 16 * 1024`, `REQUEST_TIMEOUT = 10 s`;
  - `enum AdminRequest { SetupUrl, ResetPassword { password }, ListHosts, MintPairingCode, ResetPublicUrl { public_url } }`, with `name()`, `changes_state()` and a hand-written `Debug`;
  - `enum AdminResponse { SetupUrl { url }, AlreadySetUp, NotSetUp, PasswordReset { sessions_ended }, PublicUrlReset { public_url, sessions_ended }, Hosts { hosts: Vec<AdminHost> }, PairingCode { code, expires_at }, Refused { message }, Failed { message } }`, with a hand-written `Debug` that leaves out the link and the code;
  - `struct AdminHost { id, name, platform, host_version, created_at, last_seen_at, revoked_at }`;
  - `struct Admin { pub operator: Arc<Operator>, pub hosts: Arc<Hosts>, pub dir: PathBuf, pub base_url: String }`;
  - `struct AdminSocket` (with `path()`), whose drop removes the socket file;
  - `fn bind(dir: &Path) -> Result<Option<AdminSocket>>`: only `ECONNREFUSED` marks an existing socket stale;
  - `async fn serve(AdminSocket, Admin, shutdown: impl Future<Output = ()>)`;
  - `async fn request(socket: &Path, &AdminRequest) -> Result<AdminResponse>`, which refuses a socket served by another user;
  - `async fn read_answer(&mut tokio::net::UnixStream) -> Result<AdminResponse>`: one line.
- Consumes: Task 5's `setup_link`, `reset_password`, `reset_public_url` and `Reset`; `Hosts::list`, `Hosts::mint_pairing_code`.

- [ ] **Step 1: Write the failing tests**

Create `crates/hennery-kernel/tests/admin.rs`:

```rust
//! The admin socket (kernel spec §4.2): private to its user, one JSON line
//! each way, every command against the collector's own operator and host
//! registry, and one collector per data directory.

use hennery_kernel::admin::{
    ADMIN_SOCKET, Admin, AdminRequest, AdminResponse, MAX_REQUEST_BYTES, bind, read_answer, request, serve,
};
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_kernel::secret::unix_now;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

const PASSWORD: &str = "correct horse battery";
const BASE_URL: &str = "http://localhost:7117";

/// An admin socket served in `dir` on `operator` and `hosts`, until the
/// returned sender is dropped.
fn start(dir: &Path, operator: &Arc<Operator>, hosts: &Arc<Hosts>) -> tokio::sync::oneshot::Sender<()> {
    let socket = bind(dir).unwrap().expect("a short path");
    let admin = Admin {
        operator: operator.clone(),
        hosts: hosts.clone(),
        dir: dir.to_path_buf(),
        base_url: BASE_URL.into(),
    };
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(serve(socket, admin, async move {
        let _ = stopped.await;
    }));
    stop
}

fn set_up(operator: &Operator) {
    let now = unix_now();
    let token = operator.issue_setup_token(now).unwrap().unwrap();
    operator
        .set_up(&token, PASSWORD, "https://hennery.example", now)
        .unwrap();
}

#[tokio::test]
async fn the_admin_socket_is_private_and_carries_out_each_command() {
    let dir = tempfile::tempdir().unwrap();
    let operator = Arc::new(Operator::open_in_memory().unwrap());
    let hosts = Arc::new(Hosts::open_in_memory().unwrap());
    let _stop = start(dir.path(), &operator, &hosts);
    let socket = dir.path().join(ADMIN_SOCKET);
    let mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let ask = |req: AdminRequest| {
        let socket = socket.clone();
        async move { request(&socket, &req).await }
    };

    // Before setup.
    let AdminResponse::SetupUrl { url } = ask(AdminRequest::SetupUrl).await.unwrap() else {
        panic!("no setup link");
    };
    assert!(url.starts_with(&format!("{BASE_URL}/setup#")), "{url}");
    assert_eq!(
        ask(AdminRequest::SetupUrl).await.unwrap(),
        AdminResponse::SetupUrl { url: url.clone() },
        "the live link was not the one shown"
    );
    let reset = AdminRequest::ResetPassword {
        password: "a new long password".into(),
    };
    assert_eq!(ask(reset.clone()).await.unwrap(), AdminResponse::NotSetUp);

    set_up(&operator);
    assert_eq!(ask(AdminRequest::SetupUrl).await.unwrap(), AdminResponse::AlreadySetUp);
    let phc = operator.verify_password(PASSWORD).unwrap().unwrap();
    operator.open_session("browser", &phc, unix_now()).unwrap().unwrap();
    assert_eq!(
        ask(reset).await.unwrap(),
        AdminResponse::PasswordReset { sessions_ended: 1 }
    );
    assert!(operator.verify_password("a new long password").unwrap().is_some());
    assert!(matches!(
        ask(AdminRequest::ResetPassword {
            password: "short".into()
        })
        .await
        .unwrap(),
        AdminResponse::Refused { .. }
    ));

    // The router's own operator: its cached origin changes at once.
    assert_eq!(
        ask(AdminRequest::ResetPublicUrl {
            public_url: "https://Moved.Example".into()
        })
        .await
        .unwrap(),
        AdminResponse::PublicUrlReset {
            public_url: "https://moved.example".into(),
            sessions_ended: 0
        }
    );
    assert_eq!(operator.public_url().unwrap().origin(), "https://moved.example");

    assert_eq!(
        ask(AdminRequest::ListHosts).await.unwrap(),
        AdminResponse::Hosts { hosts: Vec::new() }
    );
    let AdminResponse::PairingCode { code, expires_at } = ask(AdminRequest::MintPairingCode).await.unwrap() else {
        panic!("no pairing code");
    };
    assert!(expires_at > unix_now());
    let enrollment = Enrollment {
        public_key: hex_key(),
        name: "laptop".into(),
        host_version: "0.0.0".into(),
        platform: "test".into(),
    };
    assert!(matches!(
        hosts.enroll(&code, &enrollment, unix_now()).unwrap(),
        hennery_kernel::hosts::EnrollOutcome::Enrolled { .. }
    ));
    let AdminResponse::Hosts { hosts: listed } = ask(AdminRequest::ListHosts).await.unwrap() else {
        panic!("no host list");
    };
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "laptop");
}

/// A valid Ed25519 public key in hex (the base point's encoding).
fn hex_key() -> String {
    "5866666666666666666666666666666666666666666666666666666666666666".into()
}

/// A request that is not one line of JSON, or runs past
/// `MAX_REQUEST_BYTES`, is refused with an answer and changes nothing.
#[tokio::test]
async fn a_malformed_or_oversized_request_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let operator = Arc::new(Operator::open_in_memory().unwrap());
    let hosts = Arc::new(Hosts::open_in_memory().unwrap());
    let _stop = start(dir.path(), &operator, &hosts);
    let socket = dir.path().join(ADMIN_SOCKET);
    let send = |bytes: Vec<u8>| {
        let socket = socket.clone();
        async move {
            let mut stream = tokio::net::UnixStream::connect(&socket).await.unwrap();
            // Written whole even when the collector stops reading early.
            let writer = tokio::spawn(async move {
                let _ = stream.write_all(&bytes).await;
                stream
            });
            let mut stream = writer.await.unwrap();
            read_answer(&mut stream).await.unwrap()
        }
    };
    for (what, bytes) in [
        ("not JSON", b"reset everything\n".to_vec()),
        ("an unknown command", b"{\"command\":\"drop_tables\"}\n".to_vec()),
        ("too long", vec![b'x'; MAX_REQUEST_BYTES as usize + 10]),
    ] {
        let answer = send(bytes).await;
        assert!(matches!(answer, AdminResponse::Refused { .. }), "{what}: {answer:?}");
    }
    assert!(!operator.is_set_up().unwrap());
    assert!(hosts.list().unwrap().is_empty());
}

/// One collector per data directory: a socket that answers is refused; a
/// stale one (its collector killed) is replaced; the socket is removed at
/// shutdown.
#[tokio::test]
async fn a_live_socket_is_refused_and_a_stale_one_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let operator = Arc::new(Operator::open_in_memory().unwrap());
    let hosts = Arc::new(Hosts::open_in_memory().unwrap());
    let socket = dir.path().join(ADMIN_SOCKET);
    let stop = start(dir.path(), &operator, &hosts);
    let err = bind(dir.path()).err().expect("a second socket was bound");
    assert!(format!("{err:#}").contains("another collector"), "{err:#}");
    drop(stop);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while socket.exists() {
        assert!(std::time::Instant::now() < deadline, "the socket outlived its server");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // A killed collector's socket: still there, nobody listening.
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    assert!(socket.exists());
    let _stop = start(dir.path(), &operator, &hosts);
    assert!(matches!(
        request(&socket, &AdminRequest::ListHosts).await.unwrap(),
        AdminResponse::Hosts { .. }
    ));
}

/// A data directory whose `admin.sock` path does not fit a Unix socket
/// address leaves the collector without one, rather than stopping it.
#[test]
fn a_path_too_long_for_a_socket_binds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let long = dir.path().join("d".repeat(120));
    std::fs::create_dir(&long).unwrap();
    assert!(bind(&long).unwrap().is_none());
    assert!(!long.join(ADMIN_SOCKET).exists());
}

#[test]
fn a_reset_request_does_not_show_its_password_in_debug() {
    let req = AdminRequest::ResetPassword {
        password: "hunter2-hunter2".into(),
    };
    let shown = format!("{req:?}");
    assert!(!shown.contains("hunter2"), "{shown}");
    assert!(shown.contains("ResetPassword"), "{shown}");
}

#[test]
fn a_setup_link_or_pairing_code_answer_does_not_show_it_in_debug() {
    let link = AdminResponse::SetupUrl {
        url: "http://localhost:7117/setup#0123456789abcdef".into(),
    };
    let code = AdminResponse::PairingCode {
        code: "ABCD-EFGH".into(),
        expires_at: 1,
    };
    for (answer, secret) in [(link, "0123456789abcdef"), (code, "ABCD-EFGH")] {
        let shown = format!("{answer:?}");
        assert!(!shown.contains(secret), "{shown}");
        assert!(shown.contains("<redacted>"), "{shown}");
    }
}

/// Only a socket that refuses the connection is taken for stale: one that
/// cannot be checked (here, a socket its own user may not connect to) stops
/// the start, named, and is left where it is.
#[test]
fn a_socket_that_cannot_be_checked_stops_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join(ADMIN_SOCKET);
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o000)).unwrap();
    let err = bind(dir.path()).err().expect("an unchecked socket was replaced");
    assert!(format!("{err:#}").contains("cannot be checked"), "{err:#}");
    assert!(socket.exists());
}

/// Dropping a bound socket, served or not, removes its file.
#[tokio::test]
async fn a_dropped_socket_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let socket = bind(dir.path()).unwrap().expect("a short path");
    assert!(socket.path().exists());
    drop(socket);
    assert!(!dir.path().join(ADMIN_SOCKET).exists());
}
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
    for file in ["hennery.db", "hennery.db-wal", "hennery.db-shm"] {
        assert_eq!(mode_of(&data.join(file)), 0o600, "{file}");
    }
    unsafe { libc::kill(collector.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut collector.up, Duration::from_secs(15)).is_some());
```

with:

```rust
    for file in ["hennery.db", "hennery.db-wal", "hennery.db-shm", "admin.sock"] {
        assert_eq!(mode_of(&data.join(file)), 0o600, "{file}");
    }
    unsafe { libc::kill(collector.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut collector.up, Duration::from_secs(15)).is_some());
    assert!(
        !data.join("admin.sock").exists(),
        "the admin socket outlived the collector"
    );
```

- [ ] **Step 2: Run them to see them fail**

Run: `nix develop -c cargo test -p hennery-kernel --test admin`
Expected: FAIL to compile: `unresolved import hennery_kernel::admin`. (`the_collectors_data_is_private_to_its_user` fails too: there is no `admin.sock` yet.)

- [ ] **Step 3: The socket**

In `crates/hennery-kernel/Cargo.toml`, replace:

```toml
rusqlite.workspace = true
serde_json.workspace = true
```

with:

```toml
rusqlite.workspace = true
serde.workspace = true
serde_json.workspace = true
```

Run: `nix develop -c cargo build --workspace` (without `--locked`): `Cargo.lock` gains `serde` in `hennery-kernel`'s dependencies.

Create `crates/hennery-kernel/src/admin.rs`:

```rust
//! The admin socket (kernel spec §4.2): `<data>/admin.sock`, a Unix socket
//! only the collector's own user may use, for `hennery admin …` recovery
//! commands that work without a browser or a session: print the setup link,
//! reset the password or `public_url`, list the hosts, mint a pairing code.
//!
//! - **Who may connect:** the socket is 0600, and a peer whose user id is
//!   not the collector's is dropped without an answer. The client refuses,
//!   in turn, a socket served by another user. **Any process of the
//!   collector's own user is the operator here**, agents of the all-in-one
//!   install included (kernel spec §10): it can reset the password or
//!   `public_url` and mint pairing codes. The CLI's confirmation on a
//!   terminal stops accidents, not a process that speaks this protocol.
//! - **The protocol:** one JSON `AdminRequest` on one line, at most
//!   `MAX_REQUEST_BYTES`, within `REQUEST_TIMEOUT`; one JSON `AdminResponse`
//!   on one line back; then the connection closes.
//! - **One collector per data directory:** binding refuses a socket that
//!   still answers, replaces one that refuses the connection (a collector
//!   that was killed), and stops at any other error. The socket is removed
//!   when its `AdminSocket` is dropped.

use crate::hosts::Hosts;
use crate::operator::{Operator, Reset};
use crate::secret::unix_now;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// The socket's name in the collector's data directory (kernel spec §1).
pub const ADMIN_SOCKET: &str = "admin.sock";

/// The longest request line read, newline included.
pub const MAX_REQUEST_BYTES: u64 = 16 * 1024;

/// A connection that has not sent its whole request by then, or not taken
/// its whole answer, is dropped.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the server waits after a failed accept (out of descriptors,
/// say) before it tries again, so it does not spin.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// The longest response the client reads: a long host list fits.
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum AdminRequest {
    /// The live setup link, or a fresh one (`Operator::setup_link`).
    SetupUrl,
    /// A new password for the owner; every session ends.
    ResetPassword {
        password: String,
    },
    ListHosts,
    MintPairingCode,
    /// A new `public_url`; every session ends (3b decision 4's recovery).
    ResetPublicUrl {
        public_url: String,
    },
}

impl AdminRequest {
    /// Whether it changes state or hands out a credential: logged at
    /// `warn`, with the peer's process id.
    pub fn changes_state(&self) -> bool {
        !matches!(self, Self::SetupUrl | Self::ListHosts)
    }

    /// The command's name, for the log: never its arguments.
    pub fn name(&self) -> &'static str {
        match self {
            Self::SetupUrl => "setup_url",
            Self::ResetPassword { .. } => "reset_password",
            Self::ListHosts => "list_hosts",
            Self::MintPairingCode => "mint_pairing_code",
            Self::ResetPublicUrl { .. } => "reset_public_url",
        }
    }
}

/// Written by hand: the password is left out.
impl std::fmt::Debug for AdminRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ResetPassword { .. } => f
                .debug_struct("ResetPassword")
                .field("password", &format_args!("<redacted>"))
                .finish(),
            Self::ResetPublicUrl { public_url } => f
                .debug_struct("ResetPublicUrl")
                .field("public_url", public_url)
                .finish(),
            other => f.write_str(other.name()),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum AdminResponse {
    SetupUrl {
        url: String,
    },
    /// `setup_url` once there is an owner.
    AlreadySetUp,
    /// A reset before there is an owner: setup is the way in.
    NotSetUp,
    PasswordReset {
        sessions_ended: usize,
    },
    PublicUrlReset {
        public_url: String,
        sessions_ended: usize,
    },
    Hosts {
        hosts: Vec<AdminHost>,
    },
    PairingCode {
        code: String,
        expires_at: i64,
    },
    /// The request is not acceptable (why); nothing changed.
    Refused {
        message: String,
    },
    /// The collector failed to carry it out.
    Failed {
        message: String,
    },
}

/// Written by hand: the setup link and the pairing code are credentials,
/// and are left out.
impl std::fmt::Debug for AdminResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = format_args!("<redacted>");
        match self {
            Self::SetupUrl { .. } => f.debug_struct("SetupUrl").field("url", &redacted).finish(),
            Self::AlreadySetUp => f.write_str("AlreadySetUp"),
            Self::NotSetUp => f.write_str("NotSetUp"),
            Self::PasswordReset { sessions_ended } => f
                .debug_struct("PasswordReset")
                .field("sessions_ended", sessions_ended)
                .finish(),
            Self::PublicUrlReset {
                public_url,
                sessions_ended,
            } => f
                .debug_struct("PublicUrlReset")
                .field("public_url", public_url)
                .field("sessions_ended", sessions_ended)
                .finish(),
            Self::Hosts { hosts } => f.debug_struct("Hosts").field("hosts", hosts).finish(),
            Self::PairingCode { expires_at, .. } => f
                .debug_struct("PairingCode")
                .field("code", &redacted)
                .field("expires_at", expires_at)
                .finish(),
            Self::Refused { message } => f.debug_struct("Refused").field("message", message).finish(),
            Self::Failed { message } => f.debug_struct("Failed").field("message", message).finish(),
        }
    }
}

/// One paired host, as `list_hosts` shows it. Times are seconds since the
/// Unix epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminHost {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub host_version: String,
    pub created_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// What the admin commands act on: the router's own `Operator` and
/// `Hosts`, never a second instance, since the cached `public_url` and the
/// setup token live in that one.
#[derive(Clone)]
pub struct Admin {
    pub operator: Arc<Operator>,
    pub hosts: Arc<Hosts>,
    /// The data directory, where a fresh setup link is written.
    pub dir: PathBuf,
    /// The setup link's base (kernel spec §3.1).
    pub base_url: String,
}

/// A bound admin socket. Dropping it removes the socket file, however the
/// collector stops.
pub struct AdminSocket {
    listener: tokio::net::UnixListener,
    path: PathBuf,
}

impl AdminSocket {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for AdminSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The longest path a Unix socket address holds, its terminating NUL
/// included (104 bytes on macOS, 108 on Linux).
fn max_socket_path() -> usize {
    // SAFETY: all-zero bytes are a valid `sockaddr_un`.
    let addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_path.len()
}

/// Bind `dir/admin.sock`, mode 0600. `Ok(None)`, with a warning, when that
/// path is too long for a Unix socket: the collector then runs without it.
/// Refused when the socket there still answers: another collector serves
/// this data directory. One that refuses the connection is replaced; any
/// other error (not a socket, not ours to connect to) stops the start and
/// is named.
pub fn bind(dir: &Path) -> Result<Option<AdminSocket>> {
    let path = dir.join(ADMIN_SOCKET);
    if path.as_os_str().len() >= max_socket_path() {
        tracing::warn!(
            path = %path.display(),
            "the data directory's path is too long for a Unix socket: `hennery admin` cannot reach this collector"
        );
        return Ok(None);
    }
    match std::os::unix::net::UnixStream::connect(&path) {
        Ok(_) => bail!(
            "{} answers: another collector serves this data directory",
            path.display()
        ),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        // Left by a collector that did not shut down cleanly.
        Err(err) if err.raw_os_error() == Some(libc::ECONNREFUSED) => {
            std::fs::remove_file(&path).with_context(|| format!("remove the stale {}", path.display()))?
        }
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "{} is there and cannot be checked; remove it if no collector serves this data directory",
                    path.display()
                )
            });
        }
    }
    let listener = std::os::unix::net::UnixListener::bind(&path).with_context(|| format!("bind {}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("make {} private", path.display()))?;
    listener.set_nonblocking(true)?;
    let listener = tokio::net::UnixListener::from_std(listener)?;
    Ok(Some(AdminSocket { listener, path }))
}

/// Answer admin commands until `shutdown` resolves, then remove the socket
/// (by dropping it).
pub async fn serve(socket: AdminSocket, admin: Admin, shutdown: impl std::future::Future<Output = ()>) {
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            accepted = socket.listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let admin = admin.clone();
                    tokio::spawn(async move {
                        if let Err(err) = answer(stream, &admin).await {
                            tracing::warn!(error = %format!("{err:#}"), "admin socket connection");
                        }
                    });
                }
                Err(err) => {
                    tracing::warn!(error = %err, "admin socket accept");
                    tokio::time::sleep(ACCEPT_BACKOFF).await;
                }
            },
            () = &mut shutdown => break,
        }
    }
    drop(socket);
}

/// One connection: check the peer, read its request, answer it.
async fn answer(mut stream: tokio::net::UnixStream, admin: &Admin) -> Result<()> {
    let peer = stream.peer_cred().context("the peer's credentials")?;
    // SAFETY: geteuid(2) cannot fail.
    let own = unsafe { libc::geteuid() };
    if peer.uid() != own {
        tracing::warn!(
            peer = peer.uid(),
            "admin socket: refused a connection from another user"
        );
        return Ok(());
    }
    let peer_pid = peer.pid();
    let (read, mut write) = stream.split();
    let mut line = String::new();
    let mut reader = BufReader::new(read.take(MAX_REQUEST_BYTES));
    let response = match tokio::time::timeout(REQUEST_TIMEOUT, reader.read_line(&mut line)).await {
        Err(_) => bail!("no request within {REQUEST_TIMEOUT:?}"),
        Ok(Err(err)) => refused(format!("the request is not a line of UTF-8: {err}")),
        Ok(Ok(_)) if !line.ends_with('\n') => refused(format!(
            "the request must be one line of at most {MAX_REQUEST_BYTES} bytes"
        )),
        Ok(Ok(_)) => match serde_json::from_str::<AdminRequest>(&line) {
            Ok(request) => {
                if request.changes_state() {
                    tracing::warn!(command = request.name(), ?peer_pid, "admin command");
                } else {
                    tracing::info!(command = request.name(), ?peer_pid, "admin command");
                }
                carry_out(request, admin).await
            }
            Err(err) => refused(format!("not an admin request: {err}")),
        },
    };
    let mut out = serde_json::to_vec(&response)?;
    out.push(b'\n');
    let sent = tokio::time::timeout(REQUEST_TIMEOUT, async {
        write.write_all(&out).await?;
        write.shutdown().await
    });
    match sent.await {
        Ok(result) => result?,
        Err(_) => bail!("the answer was not taken within {REQUEST_TIMEOUT:?}"),
    }
    Ok(())
}

fn refused(message: String) -> AdminResponse {
    AdminResponse::Refused { message }
}

async fn carry_out(request: AdminRequest, admin: &Admin) -> AdminResponse {
    let now = unix_now();
    let outcome = match request {
        AdminRequest::SetupUrl => admin
            .operator
            .setup_link(&admin.dir, &admin.base_url, now)
            .map(|link| match link {
                Some(link) => AdminResponse::SetupUrl { url: link.url },
                None => AdminResponse::AlreadySetUp,
            }),
        AdminRequest::ResetPassword { password } => {
            admin
                .operator
                .reset_password(password, now)
                .await
                .map(|reset| match reset {
                    Reset::Done { sessions_ended } => AdminResponse::PasswordReset { sessions_ended },
                    Reset::NotSetUp => AdminResponse::NotSetUp,
                    Reset::Invalid(message) => AdminResponse::Refused { message },
                })
        }
        AdminRequest::ListHosts => admin.hosts.list().map(|records| AdminResponse::Hosts {
            hosts: records
                .into_iter()
                .map(|r| AdminHost {
                    id: r.id,
                    name: r.name,
                    platform: r.platform,
                    host_version: r.host_version,
                    created_at: r.created_at,
                    last_seen_at: r.last_seen_at,
                    revoked_at: r.revoked_at,
                })
                .collect(),
        }),
        AdminRequest::MintPairingCode => admin
            .hosts
            .mint_pairing_code(now)
            .map(|code| AdminResponse::PairingCode {
                code: code.code,
                expires_at: code.expires_at,
            }),
        AdminRequest::ResetPublicUrl { public_url } => {
            admin.operator.reset_public_url(&public_url).map(|reset| match reset {
                Reset::Done { sessions_ended } => AdminResponse::PublicUrlReset {
                    public_url: admin
                        .operator
                        .public_url()
                        .map(|url| url.origin().to_string())
                        .unwrap_or_default(),
                    sessions_ended,
                },
                Reset::NotSetUp => AdminResponse::NotSetUp,
                Reset::Invalid(message) => AdminResponse::Refused { message },
            })
        }
    };
    outcome.unwrap_or_else(|err| AdminResponse::Failed {
        message: format!("{err:#}"),
    })
}

/// The client's side: send `request` to the collector at `socket` and read
/// its answer, one line. Not to the end of the stream: a collector that
/// closes with part of a request unread resets the connection on Linux,
/// after its answer.
pub async fn request(socket: &Path, request: &AdminRequest) -> Result<AdminResponse> {
    let mut stream = tokio::net::UnixStream::connect(socket)
        .await
        .with_context(|| format!("connect to {} (is the collector running?)", socket.display()))?;
    // The other way round too: a password is sent only to a collector of
    // this user's.
    let server = stream.peer_cred().context("the collector's credentials")?.uid();
    // SAFETY: geteuid(2) cannot fail.
    let own = unsafe { libc::geteuid() };
    if server != own {
        bail!(
            "{} is served by user id {server}, not by this user ({own}); nothing was sent",
            socket.display()
        );
    }
    let mut line = serde_json::to_vec(request)?;
    line.push(b'\n');
    stream.write_all(&line).await?;
    read_answer(&mut stream).await
}

/// One `AdminResponse` line from `stream`, at most `MAX_RESPONSE_BYTES`.
pub async fn read_answer(stream: &mut tokio::net::UnixStream) -> Result<AdminResponse> {
    let mut answer = String::new();
    BufReader::new(stream.take(MAX_RESPONSE_BYTES))
        .read_line(&mut answer)
        .await
        .context("read the collector's answer")?;
    serde_json::from_str(&answer).context("the collector's answer")
}
```

In `crates/hennery-kernel/src/lib.rs`, replace:

```rust
//! the operator and their setup, and host identity and pairing.

```

with:

```rust
//! the operator and their setup, host identity and pairing, and the admin
//! socket.

pub mod admin;
```

- [ ] **Step 4: The collector serves it**

In `crates/hennery/src/main.rs`, replace:

```rust
async fn run_collector(args: CollectorArgs) -> Result<()> {
    warn_if_dev_token();
```

with:

```rust
async fn run_collector(args: CollectorArgs) -> Result<()> {
    // First: a SIGINT or SIGTERM from here on shuts down cleanly, removing
    // the admin socket, instead of killing the collector by the default
    // action while it starts.
    let mut signals = Signals::new()?;
    warn_if_dev_token();
```

In `crates/hennery/src/main.rs`, replace:

```rust
    private_data_dir(&args.data_dir)?;
    let db = args.data_dir.join("hennery.db");
```

with:

```rust
    private_data_dir(&args.data_dir)?;
    // Before the database and the setup link: a second collector on this
    // data directory stops here, while the first still answers on it.
    let admin_socket = hennery_kernel::admin::bind(&args.data_dir)?;
    let db = args.data_dir.join("hennery.db");
```

In `crates/hennery/src/main.rs`, replace:

```rust
        terminated().await;
        shutdown.cancel();
    });
    let app = hennery_sessions::router(state.clone()).route("/", get(|| async { Html(PLACEHOLDER) }));
    hennery_sessions::serve_all(listeners, app, state.shutdown.clone()).await?;
```

with:

```rust
        signals.recv().await;
        shutdown.cancel();
    });
    // Served on the router's own operator and registry (kernel spec §4.2).
    let admin = admin_socket.map(|socket| {
        let admin = hennery_kernel::admin::Admin {
            operator: state.operator.clone(),
            hosts: state.hosts.clone(),
            dir: args.data_dir.clone(),
            base_url,
        };
        tokio::spawn(hennery_kernel::admin::serve(
            socket,
            admin,
            state.shutdown.clone().cancelled_owned(),
        ))
    });
    let app = hennery_sessions::router(state.clone()).route("/", get(|| async { Html(PLACEHOLDER) }));
    let served = hennery_sessions::serve_all(listeners, app, state.shutdown.clone()).await;
    // Also when serving failed: the admin socket is removed once it stops.
    state.shutdown.cancel();
    if let Some(admin) = admin {
        let _ = admin.await;
    }
    served?;
```

In `crates/hennery/src/main.rs`, replace:

```rust
/// `up`'s SIGINT and SIGTERM, caught from the moment it is made: unlike
/// `terminated`, whose handlers exist only once it is first polled.
```

with:

```rust
/// SIGINT and SIGTERM for `up` and the collector, caught from the moment it
/// is made: unlike `terminated`, whose handlers exist only once it is first
/// polled.
```

- [ ] **Step 5: Run them to see them pass**

Run: `nix develop -c cargo test -p hennery-kernel --test admin` and `nix develop -c cargo test -p hennery --test cli the_collectors_data_is_private_to_its_user`
Expected: PASS.

- [ ] **Step 6: Revert-probes**

- In `bind`, remove the `set_permissions` statement. `the_admin_socket_is_private_and_carries_out_each_command` fails: `left: 493` (0755), `right: 384` (0600). Restore it.
- Derive `Debug` for `AdminRequest`, or print the password in the hand-written one. `a_reset_request_does_not_show_its_password_in_debug` fails. Restore it.
- In `bind`, take any connect error for stale: `Err(_) if true =>` in place of the `ECONNREFUSED` arm. `a_socket_that_cannot_be_checked_stops_the_start` fails: the unconnectable socket is removed and replaced. Restore it.
- Print `url` in `AdminResponse`'s `Debug` for `SetupUrl`. `a_setup_link_or_pairing_code_answer_does_not_show_it_in_debug` fails. Restore it.
- Remove `impl Drop for AdminSocket`. `a_dropped_socket_is_removed` fails, and so does `a_live_socket_is_refused_and_a_stale_one_replaced` (it waits for the served socket to go). Restore it.
- In `run_collector`, go back to `terminated().await` in the shutdown task (no `Signals` up front). Run `the_collectors_data_is_private_to_its_user` with four copies of the `cli` test binary at once, a few rounds. It fails now and then: `the admin socket outlived the collector` (1 in 12 runs here). Restore it.

- [ ] **Step 7: The full checks**

Run the five commands of "Global Constraints". Expected: all pass; **468 tests**.

- [ ] **Step 8: Commit**

```bash
git add Cargo.lock crates/hennery-kernel crates/hennery
git commit -m "feat(kernel): serve admin commands on a private admin.sock"
```

### Task 7: `hennery admin`, confirmed on a terminal

**Files:**
- Create: `crates/hennery/src/admin.rs`
- Modify: `crates/hennery/src/main.rs` (`mod admin`, `Command::Admin`)
- Test: `crates/hennery/tests/cli.rs`

**Interfaces:**
- Produces:
  - `hennery admin --data-dir <dir> setup-url | reset-password | hosts | pairing-code | reset-public-url <url>`. The data directory is the collector's, or `up`'s, whose `collector/admin.sock` is used when `<dir>/admin.sock` does not exist.
  - In the binary: `admin::AdminArgs` and `admin::run(AdminArgs) -> Result<()>`.
- Consumes: Task 6's `hennery_kernel::admin::{ADMIN_SOCKET, AdminRequest, AdminResponse, request}`.

- [ ] **Step 1: Write the failing tests**

Test helpers:
- `collector_on` starts a collector and waits for its socket.
- `admin` runs a command with an empty standard input.
- `admin_on_a_terminal` runs one with a pty as standard input, types into it ahead, and kills the command if it still waits after 20 seconds.
- `admin_conversation` types each answer only once its prompt is on the command's standard error, and returns everything the pty's master showed: the successful password reset uses it, and asserts the password never appears there (decision 9, A8).
- `login` posts a password from an origin.

In `crates/hennery/tests/cli.rs`, replace:

```rust
    for cmd in ["collector", "host", "up"] {
```

with:

```rust
    for cmd in ["collector", "host", "up", "admin"] {
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
    assert!(link.starts_with("https://up.example/setup#"), "{link}");
}
```

with:

```rust
    assert!(link.starts_with("https://up.example/setup#"), "{link}");
}

/// A collector on port 0 in `data`, logging to `log`, once it serves: the
/// guard, and the address it listens on.
fn collector_on(data: &std::path::Path, log: &std::path::Path) -> (KillTree, String) {
    let collector = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["collector", "--listen", "127.0.0.1:0"])
        .arg("--data-dir")
        .arg(data)
        .stdout(std::fs::File::create(log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
        .spawn()
        .unwrap();
    let mut guard = KillTree::new(collector, log);
    let listen = guard.listening();
    guard.wait_until("the admin socket", || data.join("admin.sock").exists());
    (guard, listen)
}

/// `hennery admin --data-dir <data> <args…>` with no terminal: standard
/// input is empty.
fn admin(data: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_hennery"))
        .arg("admin")
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap()
}

/// Like `admin`, with a terminal (a pty) as standard input, into which
/// `typed` is typed ahead: the confirmations' path. A command still
/// waiting for input after 20 seconds (typed-ahead input lost, say) is
/// killed and fails the test.
fn admin_on_a_terminal(data: &std::path::Path, args: &[&str], typed: &str) -> std::process::Output {
    let (master, slave) = open_pty();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .arg("admin")
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .stdin(std::process::Stdio::from(slave))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    (&master).write_all(typed.as_bytes()).unwrap();
    let Some(status) = wait_with_timeout(&mut child, Duration::from_secs(20)) else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("`hennery admin {args:?}` still waited for input");
    };
    drop(master);
    // Its output is a few lines: it fit the pipes while it ran.
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    child.stdout.take().unwrap().read_to_end(&mut stdout).unwrap();
    child.stderr.take().unwrap().read_to_end(&mut stderr).unwrap();
    std::process::Output { status, stdout, stderr }
}

/// Like `admin_on_a_terminal`, typing each `(prompt, typed)`'s `typed` only
/// once `prompt` is on the command's standard error, as a person would.
/// Also returns everything the terminal showed (its echo).
fn admin_conversation(data: &std::path::Path, args: &[&str], steps: &[(&str, &str)]) -> (std::process::Output, String) {
    use std::sync::{Arc, Mutex};
    let (master, slave) = open_pty();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .arg("admin")
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .stdin(std::process::Stdio::from(slave))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Read on throughout: what the terminal shows, and the prompts.
    let collect = |mut from: Box<dyn Read + Send>| {
        let text = Arc::new(Mutex::new(Vec::new()));
        let sink = text.clone();
        let thread = std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            while let Ok(n) = from.read(&mut buf) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
        (text, thread)
    };
    let (shown, shown_thread) = collect(Box::new(master.try_clone().unwrap()));
    let (stderr, stderr_thread) = collect(Box::new(child.stderr.take().unwrap()));
    for (prompt, typed) in steps {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !String::from_utf8_lossy(&stderr.lock().unwrap()).contains(prompt) {
            if Instant::now() > deadline || matches!(child.try_wait(), Ok(Some(_))) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("no {prompt:?}: {}", String::from_utf8_lossy(&stderr.lock().unwrap()));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        (&master).write_all(typed.as_bytes()).unwrap();
    }
    let Some(status) = wait_with_timeout(&mut child, Duration::from_secs(20)) else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("`hennery admin {args:?}` still waited for input");
    };
    drop(master);
    let mut stdout = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut stdout).unwrap();
    // Both end once the command has exited: the terminal with its last
    // holder, standard error with the command.
    stderr_thread.join().unwrap();
    shown_thread.join().unwrap();
    let stderr = stderr.lock().unwrap().clone();
    let shown = String::from_utf8_lossy(&shown.lock().unwrap()).into_owned();
    (std::process::Output { status, stdout, stderr }, shown)
}

/// A new pty: its master end, and its slave end for a child's standard
/// input.
fn open_pty() -> (std::fs::File, std::os::fd::OwnedFd) {
    use std::os::fd::{FromRawFd, OwnedFd};
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: openpty(3) into two local ints, with no name and the default
    // settings; both ends are made close-on-exec at once, so no other
    // test's child inherits them, and owned.
    unsafe {
        let rc = libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        assert_eq!(rc, 0, "{}", std::io::Error::last_os_error());
        for fd in [master, slave] {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        (std::fs::File::from_raw_fd(master), OwnedFd::from_raw_fd(slave))
    }
}

/// `POST /api/auth/login` with `password`, from `origin`: the status.
fn login(listen: &str, origin: &str, password: &str) -> Option<u16> {
    let body = serde_json::json!({ "password": password }).to_string();
    let mut stream = TcpStream::connect(listen).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(15))).ok()?;
    write!(
        stream,
        "POST /api/auth/login HTTP/1.1\r\nHost: {listen}\r\nOrigin: {origin}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    response.split(' ').nth(1)?.parse().ok()
}

/// Kernel spec §4.2: the commands that change state or hand out a
/// credential ask for confirmation on a terminal, and without one they
/// refuse and send nothing. Printing the setup link and listing hosts
/// need none; the setup link printed is the one in `setup-url`.
#[test]
fn admin_commands_that_change_state_need_a_terminal() {
    let dir = scratch_dir("adminnotty");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    let (mut collector, listen) = collector_on(&data, &dir.join("collector.log"));
    collector.wait_until("the setup link", || data.join("setup-url").exists());
    let out = admin(&data, &["setup-url"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        std::fs::read_to_string(data.join("setup-url")).unwrap()
    );
    let session = sign_in(&mut collector, &listen, &data);

    for args in [
        &["reset-password"][..],
        &["pairing-code"],
        &["reset-public-url", "https://moved.example"],
    ] {
        let out = admin(&data, args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?} ran without a terminal");
        assert!(stderr.contains("on a terminal"), "{args:?}: {stderr}");
        assert!(!contains_a_pairing_code_shape(&String::from_utf8_lossy(&out.stdout)));
    }
    // Nothing reached the collector: the session and the origin still hold.
    assert!(get_json(&listen, "/api/hosts", &session).is_some());
    assert_eq!(
        post_from(&listen, "/api/auth/logout", &session, &format!("http://{listen}")),
        Some(204)
    );

    let out = admin(&data, &["hosts"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let out = admin(&data, &["setup-url"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("set up already"));
}

/// 3b decision 4's lock-out, recovered live: a collector set up at one
/// origin and then reached at another refuses every login from the new one.
/// `hennery admin reset-public-url`, confirmed on a terminal, moves it
/// without a restart: the new origin signs in, the old one no longer does,
/// and the sessions of the old are gone.
#[test]
fn a_moved_collector_is_recovered_over_the_admin_socket() {
    let dir = scratch_dir("adminmove");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    let (mut collector, listen) = collector_on(&data, &dir.join("collector.log"));
    // Set up at `http://127.0.0.1:<port>`, then reached as `localhost`.
    let session = sign_in(&mut collector, &listen, &data);
    let old = format!("http://{listen}");
    let new = format!("http://localhost:{}", listen.rsplit(':').next().unwrap());
    assert_eq!(login(&listen, &new, PASSWORD), Some(403), "not locked out");

    let refused = admin_on_a_terminal(&data, &["reset-public-url", &new], "no\n");
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("not confirmed"));
    assert_eq!(login(&listen, &new, PASSWORD), Some(403), "moved without a yes");

    let out = admin_on_a_terminal(&data, &["reset-public-url", &new], "yes\n");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(&format!("public_url is now {new}")),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(login(&listen, &new, PASSWORD), Some(204));
    assert_eq!(login(&listen, &old, PASSWORD), Some(403));
    assert!(
        get_json(&listen, "/api/hosts", &session).is_none(),
        "an old session survived"
    );
}

/// `hennery admin reset-password` reads the new password twice from the
/// terminal and, confirmed, replaces it and signs every session out. Two
/// different entries change nothing.
#[test]
fn a_password_reset_over_the_admin_socket_signs_everyone_out() {
    const NEW: &str = "a new long password";
    let dir = scratch_dir("adminpassword");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    let (mut collector, listen) = collector_on(&data, &dir.join("collector.log"));
    let session = sign_in(&mut collector, &listen, &data);
    let origin = format!("http://{listen}");

    let differ = admin_on_a_terminal(&data, &["reset-password"], &format!("{NEW}\nsomething else\n"));
    assert!(!differ.status.success());
    assert!(String::from_utf8_lossy(&differ.stderr).contains("differ"));
    assert!(get_json(&listen, "/api/hosts", &session).is_some());

    // Typed only once each prompt is shown, as a person would: none of it
    // may be echoed.
    let line = format!("{NEW}\n");
    let (out, shown) = admin_conversation(
        &data,
        &["reset-password"],
        &[
            ("New password: ", &line),
            ("The same again: ", &line),
            ("Type yes", "yes\n"),
        ],
    );
    assert!(!shown.contains(NEW), "the terminal echoed the password: {shown:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("1 session(s) signed out"), "{stdout}");
    assert!(!stdout.contains(NEW) && !String::from_utf8_lossy(&out.stderr).contains(NEW));
    assert!(
        get_json(&listen, "/api/hosts", &session).is_none(),
        "a session survived"
    );
    assert_eq!(login(&listen, &origin, PASSWORD), Some(401));
    assert_eq!(login(&listen, &origin, NEW), Some(204));
}

/// `hennery admin pairing-code`, confirmed on a terminal, prints a code
/// that `host join` pairs with; `hennery admin hosts` then lists the host,
/// also given `hennery up`'s data directory rather than the collector's.
#[test]
fn a_pairing_code_from_the_admin_socket_pairs_a_host() {
    let dir = scratch_dir("admincode");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    let (_collector, listen) = collector_on(&data, &dir.join("collector.log"));
    let out = admin_on_a_terminal(&data, &["pairing-code"], "yes\n");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let code = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(contains_a_pairing_code_shape(&code), "{code}");
    let join = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["host", "join", &format!("http://{listen}"), &code, "--name", "laptop"])
        .arg("--data-dir")
        .arg(dir.join("host"))
        .output()
        .unwrap();
    assert!(join.status.success(), "{}", String::from_utf8_lossy(&join.stderr));
    let out = admin(&dir, &["hosts"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let listed = String::from_utf8_lossy(&out.stdout);
    assert!(listed.contains("\tlaptop\t") && listed.contains("\tpaired"), "{listed}");
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `nix develop -c cargo test -p hennery --test cli admin`
Expected: FAIL, all four: `error: unrecognized subcommand 'admin'`.

- [ ] **Step 3: The command**

Create `crates/hennery/src/admin.rs`:

```rust
//! `hennery admin …` (kernel spec §4.2): recovery commands, sent to the
//! running collector over its admin socket. Those that change state or
//! hand out a credential (a password reset, a `public_url` reset, a
//! pairing code) ask for confirmation, and so need a terminal on standard
//! input: without one they refuse and send nothing. That stops accidents
//! and scripts, not a process of the collector's user that speaks the
//! socket's protocol itself (kernel spec §10).

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use hennery_kernel::admin::{ADMIN_SOCKET, AdminRequest, AdminResponse};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct AdminArgs {
    /// The collector's data directory, or `hennery up`'s (whose collector
    /// keeps its own in `collector/`).
    #[arg(long, env = "HENNERY_DATA_DIR")]
    data_dir: PathBuf,
    #[command(subcommand)]
    command: AdminCommand,
}

#[derive(Subcommand)]
enum AdminCommand {
    /// Print the one-time setup link: the live one, or a fresh one once it
    /// has expired.
    SetupUrl,
    /// Set a new owner password, typed on the terminal. Signs out every
    /// session.
    ResetPassword,
    /// List the paired hosts.
    Hosts,
    /// Mint a pairing code for `hennery host join`, valid for ten minutes.
    PairingCode,
    /// Set where browsers reach hennery, after moving it. Signs out every
    /// session.
    ResetPublicUrl {
        /// The new public URL, e.g. https://hennery.example.
        public_url: String,
    },
}

/// The socket in `dir`, or in `dir/collector` when only that one exists
/// (`dir` is then `hennery up`'s data directory).
fn socket_path(dir: &Path) -> PathBuf {
    let own = dir.join(ADMIN_SOCKET);
    let ups = dir.join("collector").join(ADMIN_SOCKET);
    if !own.exists() && ups.exists() { ups } else { own }
}

pub async fn run(args: AdminArgs) -> Result<()> {
    let request = match args.command {
        AdminCommand::SetupUrl => AdminRequest::SetupUrl,
        AdminCommand::Hosts => AdminRequest::ListHosts,
        AdminCommand::PairingCode => {
            confirm(
                "pairing-code",
                "A pairing code pairs any machine it is given to, for ten minutes.",
            )?;
            AdminRequest::MintPairingCode
        }
        AdminCommand::ResetPassword => {
            require_terminal("reset-password")?;
            let password = read_secret("New password: ")?;
            if read_secret("The same again: ")? != password {
                bail!("the two passwords differ; nothing changed");
            }
            confirm("reset-password", "This signs out every session.")?;
            AdminRequest::ResetPassword { password }
        }
        AdminCommand::ResetPublicUrl { public_url } => {
            confirm(
                "reset-public-url",
                &format!("Browsers will have to reach hennery at {public_url}. This signs out every session."),
            )?;
            AdminRequest::ResetPublicUrl { public_url }
        }
    };
    let socket = socket_path(&args.data_dir);
    match hennery_kernel::admin::request(&socket, &request).await? {
        AdminResponse::SetupUrl { url } => println!("{url}"),
        AdminResponse::AlreadySetUp => bail!("hennery is set up already: there is no setup link"),
        AdminResponse::NotSetUp => {
            bail!("hennery is not set up yet: `hennery admin setup-url` prints the setup link")
        }
        AdminResponse::PasswordReset { sessions_ended } => {
            println!("The password is reset; {sessions_ended} session(s) signed out.")
        }
        AdminResponse::PublicUrlReset {
            public_url,
            sessions_ended,
        } => println!("public_url is now {public_url}; {sessions_ended} session(s) signed out."),
        AdminResponse::Hosts { hosts } => {
            for host in hosts {
                let state = if host.revoked_at.is_some() { "revoked" } else { "paired" };
                println!("{}\t{}\t{}\t{state}", host.id, host.name, host.platform);
            }
        }
        AdminResponse::PairingCode { code, .. } => {
            println!("{code}");
            eprintln!("Valid for ten minutes: `hennery host join <public_url>` on the machine, then type it.");
        }
        AdminResponse::Refused { message } => bail!("refused: {message}"),
        AdminResponse::Failed { message } => bail!("the collector failed: {message}"),
    }
    Ok(())
}

fn require_terminal(command: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() {
        bail!("`hennery admin {command}` asks for confirmation on a terminal; run it from one (nothing was sent)");
    }
    Ok(())
}

/// Ask on the terminal, and go on only when the answer is `yes`.
fn confirm(command: &str, what: &str) -> Result<()> {
    require_terminal(command)?;
    eprint!("{what}\nType yes to go on: ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .context("read the answer")?;
    if answer.trim() != "yes" {
        bail!("not confirmed; nothing changed");
    }
    Ok(())
}

/// One line typed on the terminal with its echo off, so the password is not
/// shown. The echo goes off before the prompt is shown, so nothing typed
/// in answer to it is echoed, and comes back however this returns.
fn read_secret(prompt: &str) -> Result<String> {
    let _echo = EchoOff::new()?;
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("read the password")?;
    eprintln!();
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// The terminal's settings with echo off, restored on drop.
struct EchoOff(libc::termios);

impl EchoOff {
    fn new() -> Result<Self> {
        // SAFETY: tcgetattr(3) into local storage; all-zero bytes are a
        // valid `termios`.
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut saved) } != 0 {
            return Err(std::io::Error::last_os_error()).context("read the terminal's settings");
        }
        let mut quiet = saved;
        quiet.c_lflag &= !libc::ECHO;
        // TCSANOW, not TCSAFLUSH: what was typed ahead is kept.
        // SAFETY: tcsetattr(3) with settings just read and changed.
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &quiet) } != 0 {
            return Err(std::io::Error::last_os_error()).context("turn the terminal's echo off");
        }
        Ok(Self(saved))
    }
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        // SAFETY: tcsetattr(3) with the settings read in `new`.
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.0);
        }
    }
}
```

In `crates/hennery/src/main.rs`, replace:

```rust

mod config;
```

with:

```rust

mod admin;
mod config;
```

In `crates/hennery/src/main.rs`, replace:

```rust
    Up(UpArgs),
}
```

with:

```rust
    Up(UpArgs),
    /// Recovery commands for a running collector, over its admin socket.
    Admin(admin::AdminArgs),
}
```

In `crates/hennery/src/main.rs`, replace:

```rust
        Command::Up(args) => run_up(args).await.map(|()| std::process::ExitCode::SUCCESS),
    };
```

with:

```rust
        Command::Up(args) => run_up(args).await.map(|()| std::process::ExitCode::SUCCESS),
        Command::Admin(args) => admin::run(args).await.map(|()| std::process::ExitCode::SUCCESS),
    };
```

- [ ] **Step 4: Run them to see them pass**

Run the command of Step 2, and `nix develop -c cargo test -p hennery --test cli help_lists_the_skeleton_commands`. Expected: PASS.

- [ ] **Step 5: Revert-probes**

- In `confirm`, remove `require_terminal(command)?;`. `admin_commands_that_change_state_need_a_terminal` fails: `pairing-code` answers `not confirmed` instead of naming the terminal. Restore it.
- In `EchoOff::new`, use `libc::TCSAFLUSH`. `a_password_reset_over_the_admin_socket_signs_everyone_out` fails at its typed-ahead run: `` `hennery admin ["reset-password"]` still waited for input ``, after 20 s. Restore it.
- In `EchoOff::new`, leave `ECHO` set (`quiet.c_lflag &= !0;`). The same test fails: `the terminal echoed the password: "a new long password\r\na new long password\r\nyes\r\n"`. Restore it.
- In `read_secret`, show the prompt first and turn the echo off after it, with a 300 ms sleep between to widen the window. The same test fails the same way. Restore it.
- In `run_collector`, give `Admin` a second operator, `operator: std::sync::Arc::new(Operator::open(&db).unwrap())`, in place of `state.operator.clone()` (decision 12's trap). `a_moved_collector_is_recovered_over_the_admin_socket` fails: the new origin's login still gets `left: Some(403)`, `right: Some(204)`, because the row moved and the router's cached origin did not. Restore it.

- [ ] **Step 6: The full checks**

Run the five commands of "Global Constraints". Expected: all pass; **472 tests**.

Then run the timing-sensitive test binaries four at a time: `cli`, `admin`, `operator`, `step_up`, `auth`, `adapter`, and the kernel's and the binary's unit tests. Find them with `cargo test --no-run`. Expected: all pass. Afterwards `ps -ax | grep -i hennery` shows no process left.

- [ ] **Step 7: Commit**

```bash
git add crates/hennery
git commit -m "feat(cli): hennery admin, with a confirmation on a terminal for what changes state"
```

## After this plan

**Plan 3b-iii, `owner_id` everywhere** (decision 1; kernel §1, umbrella §7.4):
- **The design question first: rows written before there is an owner.** `up` mints a pairing code and enrolls its host before setup, and `hosts` / `pairing_codes` rows need an owner. The options, for that plan to choose:
  - create the owner row at the first start, with setup filling in its password;
  - keep `owner_id` nullable on those tables and backfill it when setup commits;
  - a fixed single-owner id until teams exist.
- **The backfill:** `hosts`, `pairing_codes`, and the sessions store's `sessions`, `turns`, `events`, `session_catalog`, `pending` and `answer_queue` get `owner_id`, backfilled with the one owner in their migrations (kernel and sessions components each migrate their own).
- **Every query filters by it**, about 80 statements in `store.rs`, and the operator's own: `verify_password`, `authenticate`, `step_up`, `sessions`, `revoke_session`, `session_expires_at` and `load_public_url` (3b-i's A5).
- **This plan's queries, which already name the owner and must stay so** (A10): `reset_password`'s `UPDATE password_credentials … WHERE owner_id = ?` and `DELETE FROM auth_sessions WHERE owner_id = ?`; `reset_public_url`'s upsert of `settings (owner_id, key)` and its `DELETE FROM auth_sessions WHERE owner_id = ?`; `open_session`'s `EXISTS (… password_credentials WHERE owner_id = ? AND phc = ?)`. `admin`'s `list_hosts` and `mint_pairing_code` go through `Hosts`, and so join the `hosts` / `pairing_codes` backfill. `Operator::ping` reads no table.
- **3b-iii lands before the first tagged release** (A10): no released schema lacks `owner_id`, so no release needs a backfill migration kept for it.
- `migrate_component` still reads the version outside its transaction: open the stores one after the other (3b-i's note).

**Plan 3c, passkeys** (unchanged from 3b-i): `webauthn-rs`, with the RP id and origin from `public_url`. `admin reset-public-url` must then say that passkeys stop working (kernel §3.2). `openssl` and `pkg-config` go in `flake.nix`'s dev shell.

**Obligations this plan hands on:**
- **The rest of kernel §4.2's socket:** `restart pending`, and `hennery backup` / `restore` (§9), each with its terminal confirmation.
- **`doctor` check 16** (distribution §7): every configured listener bound, and `public_url` reaching one of them or a reverse proxy.
- **Not tested here:**
  - the admin socket's uid check (decision 5), since a second user needs root;
  - `readyz`'s 503 (decision 15);
  - the password prompt's echo actually off: the pty test types ahead, while echo is still on.
- **The Linux side is compile-unverified here:** `openpty`'s and `peer_cred`'s Linux paths, the `sockaddr_in` bind in the CLI test, and `SO_ACCEPTCONN` with the family check. CI's `ubuntu-latest` job is their first run.
- **Recorded from the security review of 2026-10-03:**
  - O7: `hennery admin` should say so when the data directory's socket path is too long, rather than only failing to connect (the collector's log says why).
  - O8: restore the terminal's settings on a SIGINT during the password prompt, which now leaves the echo off (decision 9).
  - O9: `reset_password` should check that it updated exactly one `password_credentials` row. This matters before 3c, when a passkey-only owner may have none.
  - O4's residual: a timed-out `/readyz` ping keeps a blocking thread until the database frees (decision 15).
  - The Linux `close_range` follow-up (decision 13): `close_range(3, ~0, CLOSE_RANGE_CLOEXEC)` would reach every descriptor, past `MAX_CLOSED_FD` and the hard limit's cap, once Linux-only code can be tested.
  - D12's `Origin` race: an epoch on the cached origin, re-checked when a handler commits.
  - D6's residuals (macOS's full backlog, two starts at once): an `flock` on a lock file closes them.
- **3a's hidden `--pairing-code-fd` / `--join-code-fd`** still take any descriptor unchecked (`from_raw_fd`); `--listen-fd`'s checks could be shared with them.
- **Spec amendments:**
  - decision 4, `public_url` in `config.toml` before setup (kernel §2);
  - decisions 6 and 7, one collector per data directory, and no socket on a path too long (kernel §4.2);
  - decision 9, which commands need confirmation;
  - and 3b-i's list (decisions 1, 14, 16, and every method but `GET`/`HEAD` state-changing).

**Carried from 3b-i, unchanged:**
- the frontend's setup page, login form, step-up prompt and session list;
- the default hat's name;
- the login limiter behind a proxy;
- the single writer thread;
- the CLI's port race, fixed by #11;
- the hardening the coordinator skipped: the `__Host-` cookie prefix, revoking a live session presented to login, and a dummy hash at start;
- `is_json_or_empty` over HTTP/2;
- the items "found in execution and the final review";
- everything 3b-i carried from 3a and earlier.

Then, in order:
- **(3b-iii) `owner_id` everywhere**
- **(3c) Passkeys**
- **(4) Frontend shell**
- **(5) Hats**
- **(6) Gateway**
- **(7) Distribution**

---

_Generated with Claude AI — please review before distribution._
