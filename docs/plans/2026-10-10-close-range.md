# Closing inherited descriptors with `close_range` (plan 7a-ii) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** On Linux, an adapter inherits none of the host's descriptors, however high their numbers. Today the spawn's loop checks descriptors 3 up to the hard `RLIMIT_NOFILE`, capped at `MAX_CLOSED_FD` (65536), so one above the cap reaches the agent. A container's hard limit of a million makes that real. One `close_range(3, ~0, CLOSE_RANGE_CLOEXEC)` covers them all, with the loop kept as the fallback for older kernels and other systems.

**Architecture:** `crates/hennery-host/src/adapter.rs`'s `close_inherited`, which runs in the forked child before `exec`, first makes the raw `close_range` system call on Linux, and falls back to its loop when the kernel refuses it. Two tests in `crates/hennery-host/tests/adapter.rs`, and the Linux CI job's test step raising its hard descriptor limit so the Linux test can run.

**Tech Stack:** Rust (edition 2024, MSRV 1.88); `libc` 0.2.189 (already a dependency), whose `SYS_close_range` exists for Linux gnu and musl on x86_64 and aarch64, and `CLOSE_RANGE_CLOEXEC` for Linux. No new crate.

**Spec:** the kernel spec and ACP core §2.3 say an agent gets its stdio and nothing else of the host's; plan 3b-ii's "After this plan" carries the follow-up: "`close_range(3, ~0, CLOSE_RANGE_CLOEXEC)` would reach every descriptor, past `MAX_CLOSED_FD` and the hard limit's cap, once Linux-only code can be tested". The debt lane handed it to the distribution lane on 2026-10-02, the first lane whose CI checks Linux behaviour (plan 7a). Anchors are `main` at `2d9d3b3` (PR #41).

**Status:** executed 2026-10-02 (see "Execution status"); amended after the security review. The security review of 2026-10-02 (binding on the maintainer's behalf) approved with one amendment, A1: the `pre_exec` SAFETY comment names the new system call. Taken, with O2 (the loop's docs say it is the fallback on Linux 5.11 and later); O1 and O3 recorded under "After this plan"; decisions 1–5 confirmed. The code was built on `feat/close-range`, CI-proven and revert-probed there (runs below); the plan was replayed from its own text onto `2d9d3b3`, and the tree matched the branch byte for byte. The checks passed on macOS: fmt, both clippy runs, the workspace tests and the codegen check.

## Execution status (2026-10-02)

**Executed** on branch `exec/close-range`, pushed as `feat/close-range` (PR #40), on `main` at `2d9d3b3`. One implementer applied the task; the opus task review, which is also this one-task plan's whole-branch review, approved it ("ready to merge"), with no Critical or Important finding. The code is byte-identical to the CI-proven and revert-probed branch. The CI revert-probes (Step 6) were not repeated: runs 36949287213 and 36949540092 ran them on this code.

As built beyond the task: the ACP core spec's §2.3 "Descriptors" line is amended here rather than later (the review's Minor 1), since this plan closes the gap it named.

Deferred minors: on Linux the fallback loop is tested by nothing (O1); the `close_inherited` doc's "close-on-exec descriptors are left alone" now describes the loop only (the `close_range` path re-marks them, harmlessly).

Tests: 682 in the workspace on macOS (one more than `2d9d3b3`); on Linux, two more.

## Scope

One task: the system call, its two tests, and the CI step.

**Out:** macOS (it has no `close_range`; the loop stays); any change to which descriptors count as inherited.

## Decisions this plan makes where the spec is silent

1. **`CLOSE_RANGE_CLOEXEC`, not 0.** The handoff named `close_range(3, ~0, 0)`, which closes every descriptor at once, std's own among them: the close-on-exec pipe through which the forked child reports a failed `exec`. With it closed, a spawn of a program that does not exist "succeeds" and the agent is never there. Revert-probed in CI: with flag 0, `a_program_that_cannot_be_executed_fails_the_spawn` fails on Linux (run 36949540092). The flag marks every descriptor close-on-exec instead; `exec` closes them, after std's pipe has done its work. The loop skips close-on-exec descriptors for the same reason.
2. **The kernel's refusal falls back to the loop.** `CLOSE_RANGE_CLOEXEC` needs Linux 5.11: before 5.9 the call is `ENOSYS`, on 5.9 and 5.10 the flag is `EINVAL`. Any non-zero return runs the loop as before, so an old kernel behaves as today.
3. **A raw `syscall`, not glibc's `close_range` wrapper.** glibc has it from 2.34 and musl not at all; the release binary is musl-static (plan 7a). `libc::syscall` is async-signal-safe and allocates nothing, as `pre_exec` requires.
4. **The test needs a descriptor above 65536, so CI raises its hard limit.** The ubuntu runner's hard `RLIMIT_NOFILE` is exactly 65536, `MAX_CLOSED_FD`, so there the loop already reaches every possible descriptor and the test cannot tell the two apart. The test step runs `sudo prlimit --pid $$ --nofile=65536:1048576` on Linux before `cargo test`: root may raise its shell's hard limit, which `cargo test` and the tests inherit. The test refuses to skip under `CI` (it asserts it ran), so losing that step fails the build rather than quietly testing nothing. On a developer's machine with a low hard limit it skips, and says so.
5. **The test re-runs itself in a copy of its binary,** as `a_descriptor_above_a_lowered_soft_limit_is_closed_too` does: the soft limit it raises and the descriptor it places are process-wide. The descriptor is placed in the forked child with `place_fd`, after `setrlimit`, and the copy checks it holds it before spawning.

## Global Constraints

- After the task these pass: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; `cargo clippy -p hennery --locked -- -D warnings`; `cargo test --workspace --locked`; `cargo run -p hennery-proto --bin gen -- --check`.
- **Linux is CI's** (fleet rule): the Linux test cannot be compiled here (macOS). Its proof is the `rust (ubuntu-latest)` job's log: `test a_descriptor_past_the_loops_cap_is_closed_on_linux ... ok`. A test that reads another process's state polls for a positive signal: here the agent's own `ls /dev/fd`, read to its end after it exits.
- `pre_exec` code calls only async-signal-safe functions and allocates nothing.

## Review Focus

1. **A host whose hard descriptor limit is a million** (a container), holding a descriptor above 65536 without close-on-exec. Expected: the agent does not see it. Pinned by `a_descriptor_past_the_loops_cap_is_closed_on_linux`, revert-probed in CI without the call (run 36949287213: it fails).
2. **An adapter path that does not exist.** Expected: the spawn fails with `NotFound`, as before. Pinned by `a_program_that_cannot_be_executed_fails_the_spawn` (every system), revert-probed with flag 0.
3. **A kernel older than 5.11.** Expected: the loop, as today. Not testable on CI's kernel; the fallback is the unchanged loop, reached on any non-zero return.

## File structure

| Path | Responsibility |
|---|---|
| `crates/hennery-host/src/adapter.rs` | `close_inherited`: `close_range` first on Linux |
| `crates/hennery-host/tests/adapter.rs` | The failed-`exec` test, and the Linux test past the cap |
| `.github/workflows/ci.yml` | The test step, raising Linux's hard descriptor limit |

**Reading the steps:** as in plan 7a: "Append to `path`:" adds a blank line, then the block; "In `path`, replace:" is followed by a block that occurs exactly once, then "with:" and its replacement.

---

### Task 1: `close_range` for the adapter's inherited descriptors

**Files:**
- Modify: `crates/hennery-host/src/adapter.rs`, `.github/workflows/ci.yml`
- Test: `crates/hennery-host/tests/adapter.rs`

**Interfaces:** none new. `close_inherited(limit)` keeps its signature; `MAX_CLOSED_FD` stays `pub`.

- [ ] **Step 1: The tests**

  Append to `crates/hennery-host/tests/adapter.rs`:

  ```rust
  /// A failed `exec` still fails the spawn: the descriptor closing before
  /// `exec` leaves std's close-on-exec pipe, which reports it, alone.
  #[tokio::test]
  async fn a_program_that_cannot_be_executed_fails_the_spawn() {
      let dir = tempfile::tempdir().unwrap();
      let missing = AgentCommand {
          program: dir.path().join("no-such-adapter").display().to_string(),
          args: Vec::new(),
          env: Vec::new(),
      };
      let err = Adapter::spawn(&missing, dir.path()).err().expect("the spawn failed");
      assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "{err}");
  }

  /// Set in the copy of this binary that
  /// `a_descriptor_past_the_loops_cap_is_closed_on_linux` runs.
  #[cfg(target_os = "linux")]
  const PAST_THE_CAP: &str = "HENNERY_TEST_PAST_THE_CAP";

  /// Held open across `exec` above `MAX_CLOSED_FD`, where the closing loop
  /// never looks.
  #[cfg(target_os = "linux")]
  const BEYOND_THE_LOOP: i32 = hennery_host::adapter::MAX_CLOSED_FD + 64;

  /// On Linux, `close_range` reaches every descriptor, also one above
  /// `MAX_CLOSED_FD` that the loop would leave open. The test runs itself
  /// again in a copy of this binary, with the soft `RLIMIT_NOFILE` raised past
  /// that descriptor and the descriptor held open there: the limit is
  /// process-wide. A machine whose hard limit is too low skips it, except in
  /// CI (`CI` set), where it must run.
  #[cfg(target_os = "linux")]
  #[tokio::test]
  async fn a_descriptor_past_the_loops_cap_is_closed_on_linux() {
      if std::env::var_os(PAST_THE_CAP).is_some() {
          // SAFETY: fcntl(2) on a descriptor number.
          assert!(
              unsafe { libc::fcntl(BEYOND_THE_LOOP, libc::F_GETFD) } >= 0,
              "not inherited"
          );
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
          assert!(!fds.contains(&BEYOND_THE_LOOP), "the agent inherited it: {fds:?}");
          return;
      }
      let mut hard = libc::rlimit {
          rlim_cur: 0,
          rlim_max: 0,
      };
      // SAFETY: getrlimit(2) into a local struct.
      assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut hard) }, 0);
      let needed = BEYOND_THE_LOOP as libc::rlim_t + 1;
      if hard.rlim_max < needed {
          assert!(
              std::env::var_os("CI").is_none(),
              "the hard RLIMIT_NOFILE ({}) is below {needed}: CI must run this test",
              hard.rlim_max
          );
          eprintln!("skipped: the hard RLIMIT_NOFILE ({}) is below {needed}", hard.rlim_max);
          return;
      }
      let file = std::fs::File::open("/dev/null").unwrap();
      let fd = file.as_raw_fd();
      let mut child = std::process::Command::new(std::env::current_exe().unwrap());
      child
          .args(["--exact", "a_descriptor_past_the_loops_cap_is_closed_on_linux"])
          .env(PAST_THE_CAP, "1");
      // SAFETY: setrlimit(2) and dup2(2) in the forked child, before exec;
      // nothing is allocated.
      unsafe {
          std::os::unix::process::CommandExt::pre_exec(&mut child, move || {
              let raised = libc::rlimit {
                  rlim_cur: needed,
                  rlim_max: hard.rlim_max,
              };
              if libc::setrlimit(libc::RLIMIT_NOFILE, &raised) != 0 {
                  return Err(std::io::Error::last_os_error());
              }
              hennery_testkit::place_fd(fd, BEYOND_THE_LOOP)
          });
      }
      let out = child.output().unwrap();
      let text = String::from_utf8_lossy(&out.stdout);
      assert!(out.status.success(), "{text}{}", String::from_utf8_lossy(&out.stderr));
      assert!(text.contains("1 passed"), "{text}");
  }
  ```

  Run: `nix develop -c cargo test -p hennery-host --locked --test adapter cannot_be_executed`
  Expected: PASS already (macOS has no `close_range`; on Linux it pins that the call below keeps std's pipe). The Linux test is not compiled here.

- [ ] **Step 2: The call**

  In `crates/hennery-host/src/adapter.rs`, replace:

  ```rust
  /// In the forked child, before `exec`: close every descriptor from 3 up to
  /// `limit` that would stay open in the agent. Whatever the host inherited
  /// without close-on-exec (from `hennery up`, a service manager or a shell)
  /// and whatever another thread opened without it would otherwise reach
  /// every agent. Descriptors that are close-on-exec already are left alone:
  /// std reports a failed `exec` over one of them.
  fn close_inherited(limit: libc::c_int) {
  ```

  with:

  ```rust
  /// In the forked child, before `exec`: close every descriptor from 3 up to
  /// `limit` that would stay open in the agent. Whatever the host inherited
  /// without close-on-exec (from `hennery up`, a service manager or a shell)
  /// and whatever another thread opened without it would otherwise reach
  /// every agent. Descriptors that are close-on-exec already are left alone:
  /// std reports a failed `exec` over one of them.
  ///
  /// On Linux 5.11 and later, one `close_range(3, ~0, CLOSE_RANGE_CLOEXEC)`
  /// marks every descriptor from 3 up close-on-exec instead, past `limit`
  /// and `MAX_CLOSED_FD` too, and `exec` closes them. Marked, not closed:
  /// std's pipe for a failed `exec` must stay open until then. An older
  /// kernel refuses the call (`ENOSYS` before 5.9, `EINVAL` for the flag
  /// before 5.11), and the loop below does the work.
  fn close_inherited(limit: libc::c_int) {
      #[cfg(target_os = "linux")]
      {
          // SAFETY: a raw close_range(2) on this (forked) process's own
          // descriptor table: a system call, async-signal-safe, allocating
          // nothing.
          let marked = unsafe {
              libc::syscall(
                  libc::SYS_close_range,
                  3 as libc::c_uint,
                  libc::c_uint::MAX,
                  libc::CLOSE_RANGE_CLOEXEC,
              )
          };
          if marked == 0 {
              return;
          }
      }
  ```

  In `crates/hennery-host/src/adapter.rs`, replace:

  ```rust
          // SAFETY: the closure runs in the forked child before `exec` and
          // calls only `fcntl` and `close`, which are async-signal-safe; it
          // allocates nothing.
  ```

  with:

  ```rust
          // SAFETY: the closure runs in the forked child before `exec` and
          // calls only `syscall(close_range)` (Linux), `fcntl` and `close`,
          // which are async-signal-safe; it allocates nothing.
  ```

  In `crates/hennery-host/src/adapter.rs`, replace:

  ```rust
  /// `close_inherited` checks no descriptor at or above this, whatever the
  /// soft limit: a soft `RLIMIT_NOFILE` of a million (a container's default)
  /// would cost every adapter spawn a million `fcntl` calls.
  ```

  with:

  ```rust
  /// `close_inherited`'s loop checks no descriptor at or above this, whatever
  /// the soft limit: a soft `RLIMIT_NOFILE` of a million (a container's
  /// default) would cost every adapter spawn a million `fcntl` calls. On
  /// Linux 5.11 and later the loop is only the fallback: `close_range`
  /// reaches past it.
  ```

  In `crates/hennery-host/src/adapter.rs`, replace:

  ```rust
  /// The first descriptor number `close_inherited` does not check: the hard
  ```

  with:

  ```rust
  /// The first descriptor number `close_inherited`'s loop does not check: the hard
  ```

- [ ] **Step 3: CI can hold a descriptor past the cap**

  In `.github/workflows/ci.yml`, replace:

  ```yaml
        - run: cargo test --workspace --locked
  ```

  with:

  ```yaml
        # On Linux, a hard RLIMIT_NOFILE above the adapter's closing loop's cap
        # (`MAX_CLOSED_FD`, 65536, the runner's own hard limit), so the test of
        # `close_range` can hold a descriptor past it, as a container's
        # million-descriptor limit would (it asserts it ran, under `CI`).
        - name: Tests
          run: |
            if [ "$RUNNER_OS" = Linux ]; then sudo prlimit --pid $$ --nofile=65536:1048576; fi
            cargo test --workspace --locked
  ```

- [ ] **Step 4: The checks**

  Run the five checks of "Global Constraints".
  Expected: all pass; one more test in the workspace on macOS (two on Linux).

- [ ] **Step 5: Commit, push, and read CI**

  ```bash
  git add crates/hennery-host/src/adapter.rs crates/hennery-host/tests/adapter.rs .github/workflows/ci.yml
  git diff --cached --stat
  git -c commit.gpgsign=false commit -m "fix(host): close every inherited descriptor with close_range on Linux"
  git push origin feat/close-range
  ```

  In the `rust (ubuntu-latest)` job's log: the `prlimit` line, then `test a_descriptor_past_the_loops_cap_is_closed_on_linux ... ok` and `test a_program_that_cannot_be_executed_fails_the_spawn ... ok`.

- [ ] **Step 6: Revert-probes, in CI**

  Each as one probe commit, dropped after its run:
  - the `#[cfg(target_os = "linux")]` block disabled (`#[cfg(any())]`): `a_descriptor_past_the_loops_cap_is_closed_on_linux ... FAILED` (run 36949287213);
  - `libc::CLOSE_RANGE_CLOEXEC` replaced by `0 as libc::c_uint`: `a_program_that_cannot_be_executed_fails_the_spawn ... FAILED` (run 36949540092).

---

## After this plan

- **Not tested here:** a kernel older than 5.11 (the fallback); `close_range` on aarch64 Linux (the musl build compiles it; no arm64 test job runs the tests). On Linux the fallback loop is now tested by nothing: the two older descriptor tests go through `close_range` too. Its code is the one macOS runs and tests. A test-only switch forcing the fallback would close the gap (the review's O1).
- **A seccomp filter that kills on unknown system calls** (the review's O3): the child then dies by `SIGSYS` before `exec`, std's pipe closes empty, the spawn reports success, and the agent exits at once by signal. It fails closed, visibly. Filters that answer `EPERM`, Docker's default among them, fall back to the loop.
- **Spec amendment:** ACP core §2.3, applied in this PR.

---

_Generated with Claude AI — please review before distribution._
