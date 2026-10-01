# Release build (plan 7a) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** What a hennery release ships is built and checked on every pull request, and published nowhere (distribution spec §1, §1.1, §2, §4.1):
- one `hennery` binary per platform: Linux x86_64 and aarch64 musl-static, macOS arm64, with OpenSSL (which `webauthn-rs-core` links) compiled in;
- cargo-dist's archives with their `.sha256`, `sha256.sum`, the shell installer and the Homebrew formula;
- the installer refusing an archive it cannot check, as §2 asks and cargo-dist's own does not;
- the collector's container image, from the same musl binary, started and found healthy;
- `hennery collector healthcheck`, which the image's `HEALTHCHECK` runs;
- macOS CI's OpenSSL named, not left to the runner image.

Nothing here publishes. No tag trigger, no release, no registry push, no attestation: those need the operator (packaging/README.md).

**Architecture:**
- **Cargo** (`Cargo.toml`, `crates/hennery/Cargo.toml`): the `hennery` crate's `vendored-openssl` feature turns on `openssl/vendored`, which builds OpenSSL from source (`openssl-src`) and links it statically. Only release builds enable it; development, tests and `ci.yml` keep the system OpenSSL. The `dist` profile (`release`, thin LTO) and the metadata cargo-dist needs.
- **CLI** (`crates/hennery`): `collector` gains a subcommand, `collector healthcheck`, in `healthcheck.rs`: `GET /healthz` over plain TCP to the first listen address (a wildcard one over loopback), 2 s per step, exit 0 on a 200, else 1 with the reason on standard error.
- **cargo-dist** (`dist-workspace.toml`): targets, the feature, the installers and the tap configured; only its build half wired up.
- **CI** (`.github/workflows/build.yml`, new): per target, on a native runner, `dist build`, then the archive's contents and the binary's linkage checked, the binary run, and on Linux run on Alpine and built into the image, which is started and checked. Then the installers: the installer hardened and tested on Linux and on macOS, the formula's checksums checked. Everything uploaded as workflow artifacts, kept a week.
- **Packaging scripts** (`packaging/`): `check-linkage.sh`, `check-archive.sh`, `install-dist.sh` (cargo-dist at a pinned SHA-256), `harden-installer.py`, `test-installer.sh`, `check-formula.sh`, `smoke-image.sh`, and a README.
- **Image** (`Dockerfile`, `.dockerignore`): the spec's Dockerfile, plus the data directory owned by the image's user.

**Tech Stack:** Rust (edition 2024, MSRV 1.88); cargo-dist 0.30.4; GitHub Actions (`ubuntu-24.04`, `ubuntu-24.04-arm`, `macos-latest`); `musl-tools`; Docker with BuildKit; distroless `static-debian13`. One new crate edge, no new crate version: `openssl = "=0.10.81"` (already in the tree through `webauthn-rs-core`) as an optional dependency of `hennery`, and its `vendored` feature pulls in `openssl-src` 300.6.1+3.6.3 (OpenSSL 3.6.3). `Cargo.lock` changes once, in Task 1.

**Spec:** [`docs/specs/2026-09-26-distribution-design.md`](../specs/2026-09-26-distribution-design.md), these sections:
- §1: "`hennery collector healthcheck` | Exit 0/1 against `/healthz` (containers)"; "Platforms v1: Linux x86_64 and aarch64, macOS aarch64."
- §1.1: "The `hennery` binary is musl-static on Linux (rustls, no system OpenSSL; `webauthn-rs` needs OpenSSL, built vendored and static). One Linux binary per architecture runs anywhere, including Alpine."
- §2: cargo-dist, "version pinned in `dist-workspace.toml`", producing "per-archive `.sha256`, a combined `sha256.sum`, a `dist-manifest.json`, a shell installer and a Homebrew formula"; "macOS arm64 builds on a native runner; Linux musl builds for both architectures"; the installer "refuses to continue when no SHA-256 tool is available rather than skipping the check"; attestations; the tap.
- §3.3: the Claude CLI is never redistributed, "not in release archives, container images, Homebrew bottles or public Nix caches".
- §4.1: the Dockerfile; "The volume must be local storage"; `/healthz` and `/readyz`; "The image contains no Node and no adapters."
- §9: "Installer: checksum mismatch aborts; missing SHA-256 tool aborts; musl/glibc detection."

And the umbrella [`docs/specs/2026-09-25-hennery-architecture-design.md`](../specs/2026-09-25-hennery-architecture-design.md) §12.1–§12.2: one binary; "GitHub Releases with signed checksums; `curl | sh` installer; Homebrew tap; Docker image for the **collector only**; Nix flake"; no self-update.

It builds on the "After this plan" sections of the executed plans, [passkeys 3c](2026-10-05-passkeys.md) above all. Every anchor below was taken from `main` at `fbacde8`, which merged PR #20. Where the code and a spec disagree, the code wins, and the plan says so.

**Status:** not executed; amended after the security review. The security review of 2026-10-02 (binding on the maintainer's behalf) approved with amendments: A1–A5 and A7 taken, A6 taken but for the dependency-update bot (decision 14), O1–O4 taken, O5 and O6 recorded (see "Decisions", "After this plan").

Every code block below was built and tested in a scratch copy of `fbacde8`, one commit per task (and Task 2's tests alone), and generated from those commits. Linux, the image and the installers were built and checked by the `build` workflow on the draft PR #24. The plan was then replayed from its own text, task by task, onto a fresh copy of `fbacde8`:
- each block applied exactly as "Reading the steps" says, and `Cargo.lock` regenerated by Task 1's `cargo build`;
- after Task 2's Step 1 the tree matched the scratch's tests-only commit, and after each task the scratch's task commit, byte for byte;
- after every task the replay ran fmt, both clippy runs, the workspace tests and the codegen check.

The replay matched every tree and passed every check of every task. It ended with 546 tests, up from 538 on `fbacde8`. Per task: 538, 546, 546, 546. The timing-sensitive tests (the healthcheck's unit tests, with their 300 ms budgets, and the two CLI tests) ran as four copies of each binary at once, three rounds: all 24 passed.

The revert-probes were run, each on its task's code:
- Task 1: a build without the feature fails `check-linkage.sh` on its OpenSSL.
- Task 2: all five, each caught by its named test (the read-timeout one by a hang, stopped after 10 s; the trickle one after 39 s).
- Task 3: on an installer that `dist build --artifacts=global` made here from the three archives of CI run 36933449065. Each of the four hardening replacements left out alone, the `shasum` branch removed, an archive's embedded checksum removed, a `.sha256` missing, a one-archive formula, and the archive check given the wrong target: every one fails at its named check. The first round left the unknown-style replacement uncaught, and the `unknown` case was added.
- Task 4, in CI: without `-m 0700` (run 36938993887), both Linux jobs fail with `FAIL: the data directory is 65532:65532 755, not 65532:65532 700`.

A5's check found one bug before any probe. The Dockerfile as first written (`COPY --from=data --chmod=0700` of the directory itself) made a 0755 directory, which the collector warned about (run 36937844513). Decision 7 has the fix. The amended code then passed every job (run 36938522235): both archives' checks, the image's five checks on each architecture, and the installer's eight on Linux and on macOS.

## Execution status

Not executed yet.

## Scope

Plan 7 (distribution) is split into five parts, each its own PR:
- **7a, this plan:** the release build;
- **7b:** the managed runtime and pinned adapters (§3);
- **7c:** services, the supervisor's restart policy and `host.lock` (§5.2, §6, §8);
- **7d:** `doctor` (§7);
- **7e:** the Nix package and modules (§4.3).

7b and 7c run in parallel with this plan. None of them touches the files of another's tasks except `crates/hennery/src/main.rs`, where each adds its own subcommand.

The earlier plans hand 7a these:
- **3c, "After this plan":** "OpenSSL in the release build: `webauthn-rs-core` links it. Plan 7's static (musl) build needs it vendored (`openssl`'s `vendored` feature) or built in." (Task 1)
- **3c, the same:** "CI's OpenSSL: … macOS relies on Homebrew's `openssl@3` in the image; if a future image lacks it, add `brew install openssl@3` to the workflow." (Task 1)
- **3b-ii and 3c:** "The Linux side is compile-unverified here … CI's `ubuntu-latest` job is their first run." The musl build is a second Linux build of every crate, against another libc (Task 3).

That is **4 tasks**:
1. OpenSSL built in, and the linkage check;
2. `hennery collector healthcheck`;
3. the release archives and the installers, built and checked in CI;
4. the collector image.

**In:**
- the `vendored-openssl` feature and macOS CI's OpenSSL step (Task 1);
- `collector healthcheck` (Task 2);
- cargo-dist's configuration, the `build` workflow's archives and installers, the hardened installer and its test (Task 3);
- the `Dockerfile`, the image's smoke test, the packaging README (Task 4).

**Out** (see "After this plan"):
- **publishing:** GitHub Releases, attestations, the Homebrew tap, GHCR. Operator.
- the README's install instructions, which name a release;
- the frontend in the pre-build hook (§2): the frontend is plan 4's;
- a multi-arch image manifest, which only a registry push needs;
- the per-archive SBOM or any other artifact the spec does not name.

## Decisions this plan makes where the spec is silent

1. **cargo-dist's build half only; its publishing workflow is not committed.**
   - `dist generate` writes a `release.yml` that runs on every version tag, creates a GitHub Release, uploads to it and attests (`id-token: write`). The brief forbids all of that without the operator. So `dist-workspace.toml` holds the configuration a release would use, with `allow-dirty = ["ci"]` so that `dist` does not ask for its workflow. The repository's own `build.yml` runs the same `dist build` calls on the same runners.
   - Evidence: `dist plan --output-format=json` on this configuration gives the matrix `build.yml` copies: `macos-14` for `aarch64-apple-darwin`, `ubuntu-22.04-arm` for `aarch64-unknown-linux-musl` and `ubuntu-22.04` for `x86_64-unknown-linux-musl`, with `sudo apt-get install musl-tools` on both Linux runners. `build.yml` takes the 24.04 images and `macos-latest`, as `ci.yml` does.
   - Alternatives: commit `release.yml` with publishing turned off (cargo-dist has no setting that keeps it from creating the release on a tag); build with plain `cargo` (the archives, the installer and `sha256.sum` would then not be cargo-dist's, and the release would build them differently).
   - Cost if wrong: the operator runs `dist generate` and keeps the steps packaging/README.md lists.
2. **OpenSSL is vendored for macOS too, not only for musl** (§1.1 names Linux).
   - Built without it, the macOS binary links Homebrew's `libssl.3.dylib` in CI and Nix's here (Task 1, Step 1), and fails to start on a Mac without that file, which is every Mac the curl installer is for. macOS ships no OpenSSL to link instead.
   - Alternatives: Homebrew's `openssl@3` as a dependency of the formula (the curl installer would still ship a binary that does not start).
   - Cost if wrong: about 2 MB more binary, and three minutes more build.
3. **A feature of the `hennery` crate, not of the kernel or the workspace.**
   - Vendoring is a choice about how the shipped binary is packaged. The kernel keeps linking whatever OpenSSL `openssl-sys` finds. Every workspace build and test (`ci.yml`, the dev shell) is as before.
   - Because only `hennery` has the feature, `dist build` must build that package alone: `precise-builds = true`. Without it, `dist` runs `cargo build --workspace --features vendored-openssl`, which fails with "none of the selected packages contains this feature" (seen in the scratch).
   - Alternatives: `openssl-sys`'s environment (`OPENSSL_STATIC`, `OPENSSL_DIR`) in each job, which needs a static OpenSSL on every runner first.
   - Cost if wrong: one line moved.
4. **cargo-dist's installer is hardened by rewriting what it generates.** (amended after the security review of 2026-10-02: A1, A2, O2)
   - §2 and §9: the installer "refuses to continue when no SHA-256 tool is available rather than skipping the check". cargo-dist 0.30.4's does skip it (`skipping sha256 checksum verification (it requires the 'sha256sum' command)`, then `return 0`), and also when the archive has no checksum, an empty one, or a style it does not know.
   - `harden-installer.py` replaces those four branches with `err` (which exits 1). Where `sha256sum` is missing it first tries `shasum -a 256` (macOS's `/usr/bin/shasum`, and Perl's on Linux): the refusal is for a machine with neither (O2). Each replaced text must occur exactly once, or the script fails, and the build with it. A cargo-dist upgrade that changes them cannot quietly ship an installer that skips the check again. A failed run leaves the file as it was.
   - The checksums themselves are cargo-dist's: embedded in the installer when the global build is given each archive's `dist-manifest.json` (Task 3 keeps them in the artifact for that). Without a manifest, the installer embeds none for that archive. Seen in the scratch, where it then said "no checksums to verify" and installed. That case is now an error too.
   - `test-installer.sh` checks that every archive the installer offers has a `sha256` checksum embedded, equal to the archive's `.sha256` (A1). It then tests each refusal: a mismatch, no tool, an empty checksum, no checksum, an unknown style (A2); and installs with `sha256sum`, and with `shasum` alone. It runs on Linux x86_64 and on macOS (O4). Each refusal was revert-probed on its own.
   - Alternatives: an installer of our own (its platform detection, PATH editing and receipt to maintain); a cargo-dist fork; leaving the gap (§2 broken).
   - Cost if wrong: the hardening is undone, and §2's guarantee with it, by deleting one step.
5. **The installer puts `hennery` in `~/.local/bin`** (`install-path`).
   - cargo-dist's default is `CARGO_HOME` (`~/.cargo/bin`), odd for anyone who is not a Rust developer. §6.3's example unit runs `/home/me/.local/bin/hennery`. 7c's `service install` writes the absolute path it finds, wherever that is.
   - Cost if wrong: one configuration line, before the first release.
6. **`collector healthcheck` speaks HTTP/1.1 over a plain TCP stream, to the first listen address.**
   - The address: `--listen`, `HENNERY_LISTEN`, `listen` in the data directory's `config.toml` (when `--data-dir` or `HENNERY_DATA_DIR` names one), else `127.0.0.1:7117`, as for `collector`. Only the first is probed: every listener serves the same routes (kernel §7). A wildcard address (`0.0.0.0`, `[::]`, the image's) is probed over loopback.
   - The whole check (connect, write, every read) has one 2 s budget, inside the image's `--timeout=3s`: each call gets what is left of it (O1). Only the status line is read, up to its CRLF, at most 256 bytes. A collector that accepts and never answers fails the check, and so does a peer that trickles bytes, rather than Docker killing it. Only a `200` status line passes. `/healthz`, not `/readyz`, as §1 says: the database's health is `/readyz`'s.
   - Its flags are its own. `collector --data-dir x healthcheck` is refused (`args_conflicts_with_subcommands`), so a flag meant for the collector cannot look as if it was used.
   - No HTTP client: `hennery` has none of its own (`reqwest` is the host's), and one line read is all that counts. The request goes out in one `write_all`, the lesson of 3c's CLI failure under load.
   - Alternatives: `curl` in the image (distroless has none); a `/readyz` check (a slow database would restart a container that is up).
   - Cost if wrong: a flag or a path, behind the image's `HEALTHCHECK`.
7. **The image's data directory exists in the image, owned by 65532 and 0700.** (amended after the security review: A5)
   - §4.1's Dockerfile has `VOLUME ["/var/lib/hennery"]` and `USER 65532:65532`, but no such directory in distroless. Docker would then make the volume root's, and the collector could not write to it.
   - Distroless has no shell. A stage of the same base's `:debug` variant (busybox) makes the directory 0700 (`mkdir -m 0700`), and `COPY --from=data --chown=65532:65532 /out/var/lib/ /var/lib/` merges its parent into the image's `/var/lib`, so the directory keeps its mode. Docker copies an image directory's owner and mode into a new volume.
   - Copying the directory itself with `--chmod=0700`, as first written, does not work: `COPY` of a directory copies what it holds, and makes the target 0755 whatever `--chmod` says. A5's check caught it on the first CI run after the review (the collector warned "the data directory is readable by other users").
   - `smoke-image.sh` runs the image with the anonymous volume this makes. It checks that the volume's directory is `65532:65532 700` (`sudo stat` on the host), that the collector logged no warning about it, and that the collector serves and answers on its admin socket, which lives in that directory. `smoke-image.sh` also reads the image's own layer (`docker export`): `/var/lib/hennery` 65532's and 0700, `/var/lib` still root's. Revert-probed in CI without `-m 0700` (Task 4, Step 7).
   - Alternatives: an empty directory in the build context (the workflow would have to make it); `WORKDIR` (root's).
   - Cost if wrong: a volume the collector cannot write. The smoke test catches it.
8. **cargo-dist is installed in CI from its release archive, checked against a SHA-256 kept in the repository** (`install-dist.sh`; the parent's review of the draft PR, confirmed by the security review).
   - Not `curl … | sh` of its installer, which would run whatever script that URL serves that day. Not Nix in CI, which needs its own installer run first, and minutes per job.
   - The digests were read from the release's `.sha256` files on 2026-10-02 and checked against a download of the macOS archive here. A replaced asset now fails the build.
   - Cost if wrong: a cargo-dist bump changes three digests and one version.
9. **The `build` workflow runs on every pull request and every push to `main`, not only on demand.**
   - The musl build is the only build against another libc, and a lane can add a dependency or a `libc` call that breaks it (3b-ii's `close_range` note). It is caught on that lane's PR, not at the first release.
   - Cost: three runners for about five minutes per PR, beside `ci.yml`; `concurrency` cancels a superseded run.
10. **macOS CI's OpenSSL:** installed if missing, and pointed at with `OPENSSL_DIR`.
    - 3c's note asked for `brew install openssl@3` "if a future image lacks it". `brew list … || brew install …` costs nothing while the image has it. `OPENSSL_DIR` keeps `openssl-sys` from finding another one first.
11. **What counts as "runs anywhere":**
    - On Linux, no `NEEDED` entry and no program interpreter (`readelf`), and the binary runs on the runner (glibc) and on Alpine (musl).
    - On macOS, every library under `/usr/lib` or `/System/Library` (`otool -L`).
    - §9's "musl/glibc detection" in the installer is moot for the binary: cargo-dist offers each Linux runner only the musl-static archive (`x86_64-unknown-linux-musl-static` in the installer). The host's glibc requirement (§1.1) is `doctor`'s (7d).
12. **Workflow artifacts, kept seven days, are the only output.**
    - On a public repository, any signed-in user can download them. They hold the AGPL `hennery` binary, its installer and formula, never an adapter (§3.3).
13. **Third-party actions are pinned by commit, images by their multi-arch index digest.** (the security review: A3, A4)
    - `dtolnay/rust-toolchain@02cb101…` (its `master` on 2026-10-02; `@stable` is a branch, so `toolchain: stable` is named) and `Swatinem/rust-cache@6323deb…` (`v2.9.2`), in `build.yml` and in `ci.yml`, which this plan edits anyway. GitHub's own actions stay on their major tags, as before.
    - `gcr.io/distroless/static-debian13:debug` and `:nonroot`, `alpine:3` and the `docker/dockerfile:1` frontend, each `tag@sha256:<index digest>`. The digests were read on 2026-10-02 with `crane digest`. An index digest, not a platform's, so the one line serves both runners.
    - A cache written by a compromised action in a `main` run could be restored by a later workflow. That is why both cache-writing actions are pinned, and why a future `release.yml` restores no cache (After this plan).
    - The toolchain itself (`stable`) is not pinned in CI. A release pins it (After this plan).
14. **No dependency-update bot is switched on.** (the security review's A6, in part)
    - The review asked for Dependabot updates of `cargo` (`openssl-src` above all), the actions and the images. A bot opens pull requests on the operator's repository, on its own schedule, for every lane to rebase over. That is the operator's to switch on, so it is recorded (packaging/README.md, After this plan) rather than committed.
    - The re-confirmation accepted this on one condition, now a gate in "After this plan": before the first release, the operator switches on Dependabot alerts (no pull requests) or commits `dependabot.yml`. Until then the pins are bumped by hand.
    - The rest of A6 is taken: packaging/README.md says how an OpenSSL fix reaches users (an `openssl-src` bump and a release, never the system's OpenSSL), and that 3.6's end of support must be checked before the first release.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`.
- After every task these pass:
  - `nix develop -c cargo fmt --all --check` (`max_width = 120`);
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy -p hennery --locked -- -D warnings` (test hooks off);
  - `cargo test --workspace --locked`;
  - `cargo run -p hennery-proto --bin gen -- --check`.
- **Nothing publishes.** No workflow may have a tag trigger, `contents: write`, `id-token: write`, `attestations: write`, `packages: write`, a release, a registry push or an attestation. Check before every push: `grep -nE "tags:|contents: write|id-token|attestations:|packages:|gh release|docker push|attest" .github/workflows/*.yml` prints only `build.yml`'s header comment.
- **No adapter in any artifact** (§3.3): no job downloads, builds or uploads an adapter or an npm package.
- **Third-party tools in CI are pinned:** cargo-dist by version and SHA-256 (`install-dist.sh`); third-party actions by commit; images by multi-arch index digest; GitHub's own actions by major tag (decision 13).
- **Linux is CI's.** Only macOS can be built here, and there is no container runtime. The musl archives, the Alpine run and the image are proven by the `build` workflow's first run, not locally.
- **`Cargo.lock` changes once,** in Task 1, by one `cargo build -p hennery` without `--locked`. Every other command runs `--locked`.
- No global installs: tooling comes from the flake dev shell; cargo-dist locally from `nix shell nixpkgs#cargo-dist`.
- Commits follow Conventional Commits and use the repository's own identity (gmail, unsigned). Push the feature branch after every completed task; never push `main`.

## Review Focus

These are the inputs most likely to bite a real user that the obvious tests would not exercise, most likely first. Each is pinned by the named check.

1. **A Mac without Homebrew's OpenSSL** (every curl-installer user).
   - Expected: the macOS binary starts. It links only `/usr/lib` and `/System/Library`.
   - Pinned by: Task 1 `check-linkage.sh`, run on every archive by Task 3. Revert-probed on a build without the feature, which links `libssl.3.dylib`.
2. **A machine with no `sha256sum`** (a minimal container, an old Mac).
   - Expected: the installer checks with `shasum -a 256` if it has that, and refuses, installing nothing, if it has neither. cargo-dist's own skips the check with a message.
   - Pinned by: Task 3 `test-installer.sh`'s "installs with shasum alone" and "no SHA-256 tool aborts", on Linux and on macOS. Revert-probed: without the hardening it installs unchecked; without the `shasum` branch it refuses where it could check.
3. **The image with Docker's own anonymous volume** (`docker run` with no `-v`).
   - Expected: the collector, as 65532, writes its database there and answers. A volume Docker makes on a path the image lacks is root's.
   - Pinned by: Task 4 `smoke-image.sh`: `collector healthcheck`, an admin request over the socket in the volume, and Docker's health status.
4. **The healthcheck run as the image runs it:** only `HENNERY_LISTEN=0.0.0.0:8080` and `HENNERY_DATA_DIR` set, no flags.
   - Expected: it probes `127.0.0.1:8080`; 0 while the collector serves, 1 once it is gone or hangs, within Docker's 3 s.
   - Pinned by: Task 2 `the_healthcheck_passes_while_the_collector_serves_and_fails_once_it_stops` (both ways of naming the address), `a_wildcard_listen_address_is_probed_over_loopback` and `a_collector_that_never_answers_is_unhealthy_within_the_timeout`.
5. **A musl binary on a musl system** (Alpine) and on a glibc one.
   - Expected: it runs on both, with no loader and no shared library.
   - Pinned by: Task 3's "Runs" and "Runs on Alpine (musl)" steps, and `check-linkage.sh`'s `NEEDED` and interpreter checks.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `Cargo.toml`, `crates/hennery/Cargo.toml`, `Cargo.lock` | `openssl` and the `vendored-openssl` feature; the `dist` profile and metadata | 1, 3 |
| `.github/workflows/ci.yml` | macOS's OpenSSL step; its actions pinned | 1 |
| `packaging/check-linkage.sh` (new) | A release binary links nothing outside the system | 1 |
| `crates/hennery/src/healthcheck.rs` (new), `crates/hennery/src/main.rs` | `collector healthcheck` | 2 |
| `crates/hennery/tests/cli.rs` | The healthcheck against a real collector; the CLI's refusals | 2 |
| `dist-workspace.toml` (new) | cargo-dist's configuration | 3 |
| `packaging/install-dist.sh`, `harden-installer.py`, `test-installer.sh`, `check-archive.sh`, `check-formula.sh` (new) | cargo-dist pinned; the installer hardened and tested; the archives' contents and the formula's checksums | 3 |
| `.github/workflows/build.yml` (new) | The archives, the installers and the image, built and checked | 3, 4 |
| `Dockerfile`, `.dockerignore` (new), `.gitignore` | The collector image; its staged binary | 4 |
| `packaging/smoke-image.sh`, `packaging/README.md` (new) | The image's check; what is built and what publishing needs | 4 |

All commands run from the repository root inside the dev shell (`nix develop -c …`, or direnv). Work on a feature branch off `main` (e.g. `feat/release-build`). Each task leaves the workspace compiling, clippy-clean and green.

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block (and a final newline).
- "Replace the whole of `path` with:" overwrites the file with the block (and a final newline).
- "Append to `path`:" adds a blank line, then the block, at the end of the file.
- "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point, as whole lines (earlier blocks of the same task already applied, in order), then "with:" and its replacement.

Other "Run:" lines only check or regenerate: `cargo build -p hennery` rewrites `Cargo.lock`. They change no other file. Scripts under `packaging/` are made executable (`chmod +x`) where they are created. The plan was replayed exactly this way, from its own text, onto `fbacde8`.

---

### Task 1: OpenSSL built in, and the linkage check

**Files:**
- Modify: `Cargo.toml` (`openssl` in `[workspace.dependencies]`), `crates/hennery/Cargo.toml` (the feature), `Cargo.lock`, `.github/workflows/ci.yml` (macOS's OpenSSL step; the actions pinned)
- Create: `packaging/check-linkage.sh`

**Interfaces:**
- Produces:
  - the `hennery` crate's feature `vendored-openssl`, which Task 3's `dist-workspace.toml` enables for every archive;
  - `packaging/check-linkage.sh <binary>`: exit 0 when the binary links nothing outside the system, else 1 with what it links. Task 3's workflow runs it on every archive.

There is no Rust to test here. The check is the script, run on two builds: without the feature, which must fail it on its OpenSSL, and with it, which must not name OpenSSL. A release build made in the dev shell links Nix's `libiconv` on macOS (the toolchain's), so the script fails it too, on that line only. CI's toolchain is not Nix's, and Task 3's workflow is where the script must pass.

- [ ] **Step 1: The linkage check**

  Create `packaging/check-linkage.sh`:

  ```sh
  #!/bin/sh
  # A release binary must run on a machine without our build's libraries
  # (distribution spec §1.1): on Linux it is fully static (no `NEEDED` entry,
  # no interpreter); on macOS it links only the system's own libraries, not
  # Homebrew's or Nix's OpenSSL (plan 7a).
  #
  # Usage: check-linkage.sh <binary>
  set -eu

  binary=$1
  case "$(uname -s)" in
  Linux)
      if readelf -d "$binary" | grep -q NEEDED; then
          readelf -d "$binary"
          echo "FAIL: $binary links shared libraries" >&2
          exit 1
      fi
      if readelf -l "$binary" | grep -q "Requesting program interpreter"; then
          readelf -l "$binary"
          echo "FAIL: $binary needs a dynamic loader" >&2
          exit 1
      fi
      ;;
  Darwin)
      # Every line after the first names a library; only the system's count.
      foreign=$(otool -L "$binary" | tail -n +2 | grep -v -E '^[[:space:]]+/(usr/lib|System/Library)/' || true)
      if [ -n "$foreign" ]; then
          otool -L "$binary"
          echo "FAIL: $binary links libraries outside the system:" >&2
          echo "$foreign" >&2
          exit 1
      fi
      ;;
  *)
      echo "FAIL: no linkage check for $(uname -s)" >&2
      exit 1
      ;;
  esac
  echo "ok: $binary links nothing outside the system"
  ```

  Run: `chmod +x packaging/check-linkage.sh && nix develop -c cargo build -p hennery --locked && sh packaging/check-linkage.sh target/debug/hennery`
  Expected: FAIL. The output lists `libssl.3.dylib` and `libcrypto.3.dylib` from `/nix/store/…-openssl-3.6.4/lib/`, and `libiconv.2.dylib`, after `FAIL: target/debug/hennery links libraries outside the system:`.

- [ ] **Step 2: The feature**

  In `Cargo.toml`, replace:

  ```toml
  webauthn-authenticator-rs = { version = "=0.5.5", features = ["softpasskey"] }
  ```

  with:

  ```toml
  webauthn-authenticator-rs = { version = "=0.5.5", features = ["softpasskey"] }
  # The release build only (plan 7a): `webauthn-rs-core` links OpenSSL, built
  # from source and linked statically there (distribution spec §1.1).
  openssl = "=0.10.81"
  ```

  In `crates/hennery/Cargo.toml`, replace:

  ```toml
  publish.workspace = true

  [dependencies]
  ```

  with:

  ```toml
  publish.workspace = true

  [features]
  # Release builds (plan 7a): build OpenSSL, which `webauthn-rs-core` links,
  # from source and link it statically, so the binary needs no system OpenSSL
  # (distribution spec §1.1). Development and tests use the system one.
  vendored-openssl = ["dep:openssl", "openssl/vendored"]

  [dependencies]
  ```

  In `crates/hennery/Cargo.toml`, replace:

  ```toml
  libc = "0.2"
  ```

  with:

  ```toml
  libc = "0.2"
  openssl = { workspace = true, optional = true }
  ```

  Run: `nix develop -c cargo build -p hennery`
  Expected: `Finished`. `git diff --stat Cargo.lock` shows `11 +++++++++++`: `openssl` under `hennery`'s dependencies, `openssl-src` under `openssl-sys`'s, and the `openssl-src` 300.6.1+3.6.3 package.

  Run: `nix develop -c cargo tree --locked --target x86_64-unknown-linux-musl -p hennery --features vendored-openssl -i openssl-src -e normal,build | head -3`
  Expected: `openssl-src v300.6.1+3.6.3`, then `[build-dependencies]` and `└── openssl-sys v0.9.117`. The musl target, which cannot be built here, gets the vendored build.

- [ ] **Step 3: The feature takes OpenSSL out of the binary**

  Run: `nix develop -c cargo build --release -p hennery --locked --features vendored-openssl && otool -L target/release/hennery`
  Expected: two libraries, `/nix/store/…-libiconv-115.100.1/lib/libiconv.2.dylib` and `/usr/lib/libSystem.B.dylib`; no `libssl` or `libcrypto`. (Three minutes from a cold target: OpenSSL is compiled.)

  Run: `sh packaging/check-linkage.sh target/release/hennery`
  Expected: FAIL, naming `libiconv.2.dylib` alone: the dev shell's toolchain, not the feature (see above).

- [ ] **Step 4: macOS CI names its OpenSSL; its actions are pinned**

  In `.github/workflows/ci.yml`, replace:

  ```yaml
        - uses: dtolnay/rust-toolchain@stable
          with:
            components: clippy, rustfmt
        - uses: Swatinem/rust-cache@v2
        # `webauthn-rs` links OpenSSL (plan 3c). macOS finds Homebrew's
        # `openssl@3` on its own; ubuntu needs the headers and pkg-config.
        - name: OpenSSL headers (ubuntu)
          if: runner.os == 'Linux'
          run: sudo apt-get update && sudo apt-get install -y libssl-dev pkg-config
  ```

  with:

  ```yaml
        - uses: dtolnay/rust-toolchain@02cb101ec7c40f2c49e1d9714d64511d8e1b74de # master, 2026-10-02
          with:
            toolchain: stable
            components: clippy, rustfmt
        - uses: Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6 # v2.9.2
        # `webauthn-rs` links OpenSSL (plan 3c): the system's for these checks.
        # Release builds build their own (`vendored-openssl`, build.yml).
        - name: OpenSSL headers (ubuntu)
          if: runner.os == 'Linux'
          run: sudo apt-get update && sudo apt-get install -y libssl-dev pkg-config
        # Named rather than left to the image (plan 7a): installed if a future
        # image lacks it, and pointed at, so no other OpenSSL is picked up.
        - name: OpenSSL (macOS)
          if: runner.os == 'macOS'
          run: |
            brew list openssl@3 > /dev/null 2>&1 || brew install openssl@3
            echo "OPENSSL_DIR=$(brew --prefix openssl@3)" >> "$GITHUB_ENV"
  ```

  CI's macOS job is this step's proof: its log shows `OPENSSL_DIR: /opt/homebrew/opt/openssl@3` in the later steps' environment. The two third-party actions are pinned by commit (decision 13): `dtolnay/rust-toolchain`'s `stable` is a branch, so the toolchain is now named with `toolchain:`; `6323deb` is `Swatinem/rust-cache`'s `v2.9.2`, which its `v2` tag points at today.

- [ ] **Step 5: The checks**

  Run the five checks of "Global Constraints".
  Expected: all pass; the workspace's test count is unchanged (the clippy runs and the tests do not enable the feature).

  Run: `nix develop -c cargo clippy -p hennery --locked --features vendored-openssl -- -D warnings`
  Expected: `Finished`, no warning.

- [ ] **Step 6: Commit**

  ```bash
  git add Cargo.toml Cargo.lock crates/hennery/Cargo.toml .github/workflows/ci.yml packaging/check-linkage.sh
  git diff --cached --stat
  git -c commit.gpgsign=false commit -m "build(release): build OpenSSL into release binaries, and check what they link"
  ```

---

### Task 2: `hennery collector healthcheck`

**Files:**
- Create: `crates/hennery/src/healthcheck.rs`
- Modify: `crates/hennery/src/main.rs` (the subcommand)
- Test: `crates/hennery/tests/cli.rs`, and `healthcheck.rs`'s unit tests

**Interfaces:**
- Consumes: `config::FileConfig::load(&Path) -> Result<FileConfig>` and `FileConfig::listen(&self, &[String]) -> Result<Vec<String>>`; `main::listen_addresses(&[String]) -> Result<Vec<String>>` (the default `127.0.0.1:7117` when empty).
- Produces:
  - `hennery collector healthcheck [--listen ADDR]… [--data-dir DIR]`, with `HENNERY_LISTEN` and `HENNERY_DATA_DIR` as their variables: exit 0, or 1 with `unhealthy: <why>` on standard error. Task 4's `HEALTHCHECK` runs it with no flags.
  - `healthcheck::probe_address(&str) -> Result<SocketAddr>`, `healthcheck::check(SocketAddr, Duration) -> Result<()>` (the `Duration` is the whole check's budget), `healthcheck::TIMEOUT` (2 s).
  - `collector` with no subcommand is unchanged: it still requires `--data-dir`.

- [ ] **Step 1: The failing tests**

  Append to `crates/hennery/tests/cli.rs`:

  ```rust
  /// `hennery collector healthcheck` (distribution spec §1, §4.1), as the
  /// image's `HEALTHCHECK` runs it: the address from `--listen` or
  /// `HENNERY_LISTEN` (the image sets the variable), with `HENNERY_DATA_DIR`
  /// set too. 0 while the collector serves, 1 once it is gone.
  #[test]
  fn the_healthcheck_passes_while_the_collector_serves_and_fails_once_it_stops() {
      let dir = scratch_dir("healthcheck");
      let _cleanup = RemoveDir(dir.clone());
      let data = dir.join("collector");
      let log = dir.join("collector.log");
      let collector = Command::new(env!("CARGO_BIN_EXE_hennery"))
          .args(["collector", "--listen", "127.0.0.1:0"])
          .arg("--data-dir")
          .arg(&data)
          .stdout(std::fs::File::create(&log).unwrap())
          .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
          .spawn()
          .unwrap();
      let mut collector = KillTree::new(collector, &log);
      let address = collector.listening();
      let healthcheck = |how: &dyn Fn(&mut Command)| {
          let mut command = Command::new(env!("CARGO_BIN_EXE_hennery"));
          command
              .args(["collector", "healthcheck"])
              .env_remove("HENNERY_LISTEN")
              .env_remove("HENNERY_DATA_DIR");
          how(&mut command);
          command.output().unwrap()
      };
      let by_flag = |command: &mut Command| {
          command.arg("--listen").arg(&address);
      };
      let by_environment = |command: &mut Command| {
          command.env("HENNERY_LISTEN", &address).env("HENNERY_DATA_DIR", &data);
      };
      for (how, set) in [
          ("--listen", &by_flag as &dyn Fn(&mut Command)),
          ("HENNERY_LISTEN", &by_environment),
      ] {
          let out = healthcheck(set);
          assert!(
              out.status.success(),
              "{how}: {:?} {}",
              out.status,
              String::from_utf8_lossy(&out.stderr)
          );
      }
      drop(collector);
      for (how, set) in [
          ("--listen", &by_flag as &dyn Fn(&mut Command)),
          ("HENNERY_LISTEN", &by_environment),
      ] {
          let out = healthcheck(set);
          assert_eq!(
              out.status.code(),
              Some(1),
              "{how}: {}",
              String::from_utf8_lossy(&out.stderr)
          );
          assert!(
              String::from_utf8_lossy(&out.stderr).contains("unhealthy"),
              "{how}: {}",
              String::from_utf8_lossy(&out.stderr)
          );
      }
  }

  /// `collector` still needs its data directory; `collector healthcheck`
  /// takes none of the collector's own flags.
  #[test]
  fn the_collector_without_a_data_dir_or_with_the_healthcheck_and_its_flags_is_refused() {
      let run = |args: &[&str]| {
          Command::new(env!("CARGO_BIN_EXE_hennery"))
              .args(args)
              .env_remove("HENNERY_DATA_DIR")
              .env_remove("HENNERY_LISTEN")
              .output()
              .unwrap()
      };
      let out = run(&["collector"]);
      assert_eq!(out.status.code(), Some(2));
      assert!(String::from_utf8_lossy(&out.stderr).contains("--data-dir"));
      let out = run(&["collector", "--data-dir", "/nonexistent", "healthcheck"]);
      assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
  }
  ```

  Run: `nix develop -c cargo test -p hennery --locked --test cli healthcheck`
  Expected: FAIL. `the_healthcheck_passes_while_the_collector_serves_and_fails_once_it_stops` panics at `--listen: exit status: 2` (`error: unexpected argument 'healthcheck' found`). `the_collector_without_a_data_dir_or_with_the_healthcheck_and_its_flags_is_refused` already passes, for the wrong reason: `healthcheck` is an unexpected argument, which exits 2 too. From Step 4 it exits 2 because the subcommand cannot be used with `--data-dir`; Step 5's last probe shows the test holding that.

- [ ] **Step 2: The health check**

  Create `crates/hennery/src/healthcheck.rs`:

  ```rust
  //! `hennery collector healthcheck` (distribution spec §1, §4.1): exit 0 when
  //! the collector answers `GET /healthz` with 200, else 1. It is the
  //! container image's `HEALTHCHECK`: the image is distroless, with no shell,
  //! `curl` or `wget`, so the binary checks itself.

  use anyhow::{Context, Result, anyhow, bail};
  use std::io::{Read, Write};
  use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
  use std::time::{Duration, Instant};

  /// The whole check's budget, inside the image's `--timeout=3s`, so a hung
  /// collector is reported by this check and not by Docker killing it.
  pub const TIMEOUT: Duration = Duration::from_secs(2);

  /// The longest status line read: the answer's status line is all that counts.
  const MAX_STATUS_LINE: usize = 256;

  /// Where to reach a collector listening on `listen`: a wildcard address
  /// (`0.0.0.0`, `[::]`) is reached over loopback, as `up`'s host reaches it.
  pub fn probe_address(listen: &str) -> Result<SocketAddr> {
      let mut address = listen
          .to_socket_addrs()
          .with_context(|| format!("listen address {listen}"))?
          .next()
          .with_context(|| format!("listen address {listen} resolves to nothing"))?;
      match address.ip() {
          IpAddr::V4(ip) if ip.is_unspecified() => address.set_ip(Ipv4Addr::LOCALHOST.into()),
          IpAddr::V6(ip) if ip.is_unspecified() => address.set_ip(Ipv6Addr::LOCALHOST.into()),
          _ => {}
      }
      Ok(address)
  }

  /// `Ok` when `GET /healthz` at `address` answers 200 within `timeout`, all
  /// of it (connect, write and every read); else why not.
  pub fn check(address: SocketAddr, timeout: Duration) -> Result<()> {
      let deadline = Instant::now() + timeout;
      // What is left of the budget; none left is a failure, not a zero timeout
      // (which the socket calls refuse).
      let left = || {
          deadline
              .checked_duration_since(Instant::now())
              .filter(|left| !left.is_zero())
              .ok_or_else(|| anyhow!("{address} did not answer GET /healthz within {timeout:?}"))
      };
      let mut stream = TcpStream::connect_timeout(&address, left()?).with_context(|| format!("connect to {address}"))?;
      stream.set_write_timeout(Some(left()?))?;
      // One write: the whole request, so the collector never sees half of one.
      stream
          .write_all(format!("GET /healthz HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n").as_bytes())
          .with_context(|| format!("send to {address}"))?;
      // The status line only, up to its CRLF: a peer trickling bytes cannot
      // keep the check past its budget, since every read gets what is left.
      let mut line = Vec::new();
      let mut buf = [0u8; MAX_STATUS_LINE];
      loop {
          stream.set_read_timeout(Some(left()?))?;
          let n = stream.read(&mut buf).with_context(|| format!("read from {address}"))?;
          line.extend_from_slice(&buf[..n]);
          if let Some(end) = line.windows(2).position(|pair| pair == b"\r\n") {
              line.truncate(end);
              break;
          }
          if n == 0 || line.len() > MAX_STATUS_LINE {
              break;
          }
      }
      let status = String::from_utf8_lossy(&line);
      match status.split(' ').nth(1) {
          Some("200") => Ok(()),
          _ => bail!("{address} answered {status:?} to GET /healthz"),
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use std::net::TcpListener;

      #[test]
      fn a_wildcard_listen_address_is_probed_over_loopback() {
          assert_eq!(
              probe_address("0.0.0.0:8080").unwrap(),
              "127.0.0.1:8080".parse().unwrap()
          );
          assert_eq!(probe_address("[::]:8080").unwrap(), "[::1]:8080".parse().unwrap());
          assert_eq!(
              probe_address("100.64.0.7:7117").unwrap(),
              "100.64.0.7:7117".parse().unwrap()
          );
      }

      #[test]
      fn a_listen_address_without_a_port_is_refused() {
          assert!(probe_address("127.0.0.1").is_err());
      }

      /// A server that reads one request and answers `answer`, or nothing.
      fn answering(answer: &'static str) -> SocketAddr {
          let listener = TcpListener::bind("127.0.0.1:0").unwrap();
          let address = listener.local_addr().unwrap();
          std::thread::spawn(move || {
              let (mut stream, _) = listener.accept().unwrap();
              let mut request = [0u8; 1024];
              let _ = stream.read(&mut request);
              let _ = stream.write_all(answer.as_bytes());
          });
          address
      }

      #[test]
      fn only_a_200_is_healthy() {
          let ok = answering("HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok");
          check(ok, TIMEOUT).unwrap();
          let unavailable = answering("HTTP/1.1 503 Service Unavailable\r\n\r\n");
          let err = check(unavailable, TIMEOUT).unwrap_err();
          assert!(format!("{err:#}").contains("503"), "{err:#}");
          let garbage = answering("200 nonsense");
          assert!(check(garbage, TIMEOUT).is_err());
      }

      #[test]
      fn nothing_listening_is_unhealthy() {
          let address = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
          // The listener is dropped: nothing listens there now.
          assert!(check(address, TIMEOUT).is_err());
      }

      /// A peer that sends a byte at a time, each well inside any one read's
      /// timeout, still fails the check once the whole budget is spent.
      #[test]
      fn an_answer_trickled_byte_by_byte_is_unhealthy_within_the_timeout() {
          let listener = TcpListener::bind("127.0.0.1:0").unwrap();
          let address = listener.local_addr().unwrap();
          std::thread::spawn(move || {
              let (mut stream, _) = listener.accept().unwrap();
              let mut request = [0u8; 1024];
              let _ = stream.read(&mut request);
              for byte in b"HTTP/1.1 200 OK".iter().cycle() {
                  if stream.write_all(&[*byte]).is_err() {
                      return;
                  }
                  std::thread::sleep(Duration::from_millis(50));
              }
          });
          let started = std::time::Instant::now();
          assert!(check(address, Duration::from_millis(300)).is_err());
          assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
      }

      #[test]
      fn a_collector_that_never_answers_is_unhealthy_within_the_timeout() {
          let listener = TcpListener::bind("127.0.0.1:0").unwrap();
          let address = listener.local_addr().unwrap();
          // Accepted by the kernel's backlog, never read or answered.
          let started = std::time::Instant::now();
          assert!(check(address, Duration::from_millis(300)).is_err());
          assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
          drop(listener);
      }
  }
  ```

- [ ] **Step 3: The subcommand**

  In `crates/hennery/src/main.rs`, replace:

  ```rust
  mod config;
  mod inherit;
  ```

  with:

  ```rust
  mod config;
  mod healthcheck;
  mod inherit;
  ```

  In `crates/hennery/src/main.rs`, replace:

  ```rust
      /// Run the collector.
      Collector(CollectorArgs),
  ```

  with:

  ```rust
      /// Run the collector.
      Collector(CollectorCli),
  ```

  In `crates/hennery/src/main.rs`, replace:

  ```rust
  #[derive(Args, Clone)]
  struct CollectorArgs {
  ```

  with:

  ```rust
  /// `collector` runs the collector; `collector healthcheck` checks one.
  #[derive(Args)]
  #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
  struct CollectorCli {
      #[command(subcommand)]
      command: Option<CollectorCommand>,
      #[command(flatten)]
      run: Option<CollectorArgs>,
  }

  #[derive(Subcommand)]
  enum CollectorCommand {
      /// Exit 0 if the collector answers `/healthz`, else 1 (for containers).
      Healthcheck(HealthcheckArgs),
  }

  #[derive(Args)]
  struct HealthcheckArgs {
      /// The collector's listen address, as for `collector`: the first one
      /// given is checked, a wildcard one over loopback. Else `listen` in
      /// `config.toml`, else 127.0.0.1:7117.
      #[arg(long = "listen", env = "HENNERY_LISTEN", value_delimiter = ',')]
      listen: Vec<String>,
      /// Where `config.toml` is read from, when no address is given.
      #[arg(long, env = "HENNERY_DATA_DIR")]
      data_dir: Option<PathBuf>,
  }

  #[derive(Args, Clone)]
  struct CollectorArgs {
  ```

  In `crates/hennery/src/main.rs`, replace:

  ```rust
          Command::Collector(args) => run_collector(args).await.map(|()| std::process::ExitCode::SUCCESS),
  ```

  with:

  ```rust
          Command::Collector(CollectorCli {
              command: Some(CollectorCommand::Healthcheck(args)),
              ..
          }) => Ok(collector_healthcheck(args)),
          Command::Collector(CollectorCli {
              run: Some(args),
              command: None,
          }) => run_collector(args).await.map(|()| std::process::ExitCode::SUCCESS),
          Command::Collector(CollectorCli {
              run: None,
              command: None,
          }) => unreachable!("clap requires --data-dir when no subcommand is given"),
  ```

  In `crates/hennery/src/main.rs`, replace:

  ```rust
  async fn run_collector(args: CollectorArgs) -> Result<()> {
  ```

  with:

  ```rust
  /// `collector healthcheck`: 0 if healthy, else 1 and why, on standard
  /// error (Docker keeps it in the container's health log).
  fn collector_healthcheck(args: HealthcheckArgs) -> std::process::ExitCode {
      let checked = (|| {
          let file = match &args.data_dir {
              Some(dir) => config::FileConfig::load(dir)?,
              None => config::FileConfig::default(),
          };
          let addresses = listen_addresses(&file.listen(&args.listen)?)?;
          healthcheck::check(healthcheck::probe_address(&addresses[0])?, healthcheck::TIMEOUT)
      })();
      match checked {
          Ok(()) => std::process::ExitCode::SUCCESS,
          Err(err) => {
              eprintln!("unhealthy: {err:#}");
              std::process::ExitCode::FAILURE
          }
      }
  }

  async fn run_collector(args: CollectorArgs) -> Result<()> {
  ```

- [ ] **Step 4: The tests pass**

  Run: `nix develop -c cargo test -p hennery --locked healthcheck`
  Expected: PASS, 6 unit tests (`healthcheck::tests::…`) and the 2 CLI tests.

  Run: `nix develop -c cargo run -q -p hennery -- collector healthcheck --listen 127.0.0.1:1; echo $?`
  Expected: `unhealthy: connect to 127.0.0.1:1: Connection refused (os error 61)` (Linux: 111), then `1`.

- [ ] **Step 5: Revert-probes**

  Each, applied alone to the task's tree, must fail the named test; restore after each.
  - `probe_address` without its wildcard arms (`match` reduced to `_ => {}`): `a_wildcard_listen_address_is_probed_over_loopback`.
  - `check` accepting any status (`Some("200") => Ok(())` becomes `_ => Ok(())`, the `bail!` arm removed): `only_a_200_is_healthy`, and `the_healthcheck_passes_…_and_fails_once_it_stops` does not (nothing answers once the collector stops, so the connect fails first). That is why the unit test exists.
  - `check` without its read timeout (`stream.set_read_timeout(Some(left()?))?;` removed): `a_collector_that_never_answers_is_unhealthy_within_the_timeout` hangs, since the listener accepts and never answers. Stop it after 10 s.
  - Each read given the whole budget instead of what is left (`Some(left()?)` becomes `Some(timeout)` in the read loop): `an_answer_trickled_byte_by_byte_is_unhealthy_within_the_timeout` fails its elapsed-time assertion: each read then waits out a whole byte's arrival until 256 bytes have come (39 s here).
  - `args_conflicts_with_subcommands = true` removed: `the_collector_without_a_data_dir_or_with_the_healthcheck_and_its_flags_is_refused`.

- [ ] **Step 6: The checks**

  Run the five checks of "Global Constraints".
  Expected: all pass, 8 more tests in the workspace (546, from 538).

- [ ] **Step 7: Commit**

  ```bash
  git add crates/hennery/src/healthcheck.rs crates/hennery/src/main.rs crates/hennery/tests/cli.rs
  git diff --cached --stat
  git -c commit.gpgsign=false commit -m "feat(cli): add collector healthcheck for the container image"
  ```

---

### Task 3: The release archives and the installers, built and checked in CI

**Files:**
- Create: `dist-workspace.toml`, `packaging/install-dist.sh`, `packaging/harden-installer.py`, `packaging/test-installer.sh`, `packaging/check-archive.sh`, `packaging/check-formula.sh`, `.github/workflows/build.yml`
- Modify: `Cargo.toml` (`repository`, `homepage`, the `dist` profile), `crates/hennery/Cargo.toml` (the same, a `description`, `[package.metadata.dist]`)

**Interfaces:**
- Consumes: Task 1's `vendored-openssl` and `check-linkage.sh`.
- Produces:
  - the `build` workflow's `archive` job, per target (matrix `target`, `runner`), with the unpacked binary's path in `$BINARY` for later steps. Task 4 adds its image steps there, after "Runs on Alpine (musl)", and an `image` key to the two Linux entries.
  - workflow artifacts `archive-<target>` (the archive, its `.sha256`, its `dist-manifest.json`) and `installers` (`hennery-installer.sh`, hardened; `hennery.rb`; `sha256.sum`).
  - `test-installer.sh <dir>`, `check-formula.sh <dir>`, `check-archive.sh <archive> <target>`: exit 0 with `ok:` lines, else 1 with `FAIL:`.

The tests here are scripts that CI runs. Every archive exists only once CI has built it: the installer embeds the checksum of each archive whose `dist-manifest.json` the global build is given, and needs all three to pass `test-installer.sh`. So the revert-probes (Step 7) run here on the artifacts of the PR's run.

- [ ] **Step 1: cargo-dist's configuration**

  Create `dist-workspace.toml`:

  ```toml
  # cargo-dist (distribution spec §2). Only its build half is wired up: the
  # `build` workflow runs `dist build` and keeps what it makes as workflow
  # artifacts (plan 7a). Publishing (GitHub Releases, the Homebrew tap, the
  # attestations) is the operator's to switch on: `dist generate` would write
  # a workflow that publishes on every version tag, so that workflow is not
  # committed, and `allow-dirty` stops `dist` from asking for it.
  [workspace]
  members = ["cargo:."]

  [dist]
  cargo-dist-version = "0.30.4"
  ci = "github"
  allow-dirty = ["ci"]
  # Linux musl-static for both architectures, macOS arm64 (distribution §1).
  targets = ["aarch64-apple-darwin", "aarch64-unknown-linux-musl", "x86_64-unknown-linux-musl"]
  # OpenSSL built in, not linked from the system (distribution §1.1).
  features = ["vendored-openssl"]
  # Only the `hennery` package, so the feature applies to the crate that has it.
  precise-builds = true
  checksum = "sha256"
  installers = ["shell", "homebrew"]
  tap = "marcinwadon/homebrew-tap"
  install-path = "~/.local/bin"
  install-updater = false
  github-attestations = true
  ```

  In `Cargo.toml`, replace:

  ```toml
  publish = false
  ```

  with:

  ```toml
  publish = false
  repository = "https://github.com/marcinwadon/hennery"
  homepage = "https://github.com/marcinwadon/hennery"
  ```

  Append to `Cargo.toml`:

  ```toml
  # Release archives (plan 7a): `dist build` builds with this profile.
  [profile.dist]
  inherits = "release"
  lto = "thin"
  ```

  In `crates/hennery/Cargo.toml`, replace:

  ```toml
  publish.workspace = true
  ```

  with:

  ```toml
  publish.workspace = true
  repository.workspace = true
  homepage.workspace = true
  description = "Self-hosted cockpit for ACP coding agents"
  ```

  Append to `crates/hennery/Cargo.toml`:

  ```toml
  # cargo-dist (plan 7a) ships this crate's binary despite `publish = false`.
  [package.metadata.dist]
  dist = true
  ```

  Run: `nix shell nixpkgs#cargo-dist nixpkgs#cargo nixpkgs#rustc -c dist plan 2>&1 | grep -v WARN | grep -E "hennery-|installer|sha256.sum|\.rb"`
  Expected: the three archives (`hennery-aarch64-apple-darwin.tar.xz`, `hennery-aarch64-unknown-linux-musl.tar.xz`, `hennery-x86_64-unknown-linux-musl.tar.xz`), each with its `.sha256`, and `hennery-installer.sh`, `hennery.rb`, `sha256.sum`. It does not ask for a `release.yml` (`allow-dirty`).

- [ ] **Step 2: cargo-dist, pinned**

  Create `packaging/install-dist.sh`:

  ```sh
  #!/bin/sh
  # Install cargo-dist for a CI job: the pinned release's archive for this
  # runner, checked against the SHA-256 written here (not one fetched beside
  # it), so a replaced release asset fails the build instead of running.
  #
  # Usage: install-dist.sh <directory to put `dist` in>
  set -eu

  version=0.30.4 # dist-workspace.toml's `cargo-dist-version`
  case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)
      target=x86_64-unknown-linux-gnu
      sha256=f7bd986e758d0d47c6995aaf92f26d093635c7cd69581ed9e2451b618ea98098
      ;;
  Linux-aarch64)
      target=aarch64-unknown-linux-gnu
      sha256=79aa478537011e0cd4f5dd79e02f28b2b87788966d241fc605c6fe23b9e74e83
      ;;
  Darwin-arm64)
      target=aarch64-apple-darwin
      sha256=c8b8f3163e5e4dd5a9cc5455957043a00bfee7d446489e4f8a4db6f2d5af1ab1
      ;;
  *)
      echo "install-dist: no pinned cargo-dist for $(uname -s)-$(uname -m)" >&2
      exit 1
      ;;
  esac

  bin=$1
  scratch=$(mktemp -d)
  trap 'rm -rf "$scratch"' EXIT
  archive="$scratch/cargo-dist.tar.xz"
  curl --proto '=https' --tlsv1.2 -sSfL -o "$archive" \
      "https://github.com/axodotdev/cargo-dist/releases/download/v$version/cargo-dist-$target.tar.xz"
  echo "$sha256  $archive" | shasum -a 256 -c -
  tar -xJf "$archive" -C "$scratch"
  mkdir -p "$bin"
  install -m 0755 "$scratch/cargo-dist-$target/dist" "$bin/dist"
  "$bin/dist" --version
  ```

  Run: `chmod +x packaging/install-dist.sh && sh packaging/install-dist.sh "$(mktemp -d)"`
  Expected: `…/cargo-dist.tar.xz: OK`, then `cargo-dist 0.30.4`.

- [ ] **Step 3: The installer hardened, and the checks of what is built**

  Create `packaging/harden-installer.py`:

  ```python
  #!/usr/bin/env python3
  """Make cargo-dist's shell installer refuse to install what it cannot check.

  The distribution spec (§2) asks the installer to refuse to continue when no
  SHA-256 tool is available, rather than skipping the check. cargo-dist's
  installer (0.30.4) skips it, with a message, in four places: no `sha256sum`,
  no checksum for the archive, an archive with no checksum style, and a style
  it does not know. Each is replaced by an error. Where `sha256sum` is missing,
  `shasum -a 256` (macOS's, and Perl's) is tried before refusing. Every
  replaced text must occur exactly once: a cargo-dist upgrade that changes the
  installer fails this script, and so the build, instead of shipping an
  installer that skips the check again.

  Usage: harden-installer.py <path to hennery-installer.sh>  (edited in place)
  """

  import sys

  REFUSE_UNCHECKED = (
      'err "refusing to install an archive whose checksum cannot be checked; '
      'install sha256sum or shasum and run the installer again"'
  )

  REPLACEMENTS = [
      (
          # No `sha256sum`: `shasum`, else no SHA-256 tool at all.
          '''            if ! check_cmd sha256sum; then
                  say "skipping sha256 checksum verification (it requires the 'sha256sum' command)"
                  return 0
              fi
              _calculated_checksum="$(sha256sum -b "$_file" | awk '{printf $1}')"''',
          '''            if check_cmd sha256sum; then
                  _calculated_checksum="$(sha256sum -b "$_file" | awk '{printf $1}')"
              elif check_cmd shasum; then
                  _calculated_checksum="$(shasum -a 256 -b "$_file" | awk '{printf $1}')"
              else
                  '''
          + REFUSE_UNCHECKED
          + '''
              fi''',
      ),
      (
          # A checksum style it does not know.
          '''        *)
              say "skipping unknown checksum style: $_checksum_style"
              return 0
              ;;''',
          '''        *)
              '''
          + REFUSE_UNCHECKED
          + '''
              ;;''',
      ),
      (
          # An empty checksum for the archive.
          '''    if [ -z "$_checksum_value" ]; then
          return 0
      fi''',
          '''    if [ -z "$_checksum_value" ]; then
          '''
          + REFUSE_UNCHECKED
          + '''
      fi''',
      ),
      (
          # An archive with no checksum at all.
          '''    else
          say "no checksums to verify"
      fi''',
          '''    else
          '''
          + REFUSE_UNCHECKED
          + '''
      fi''',
      ),
  ]


  def harden(text: str) -> str:
      for old, new in REPLACEMENTS:
          count = text.count(old)
          if count != 1:
              raise SystemExit(
                  f"harden-installer: expected one copy of this text, found {count}:\n{old}"
              )
          text = text.replace(old, new)
      return text


  def main() -> None:
      if len(sys.argv) != 2:
          raise SystemExit(__doc__)
      path = sys.argv[1]
      with open(path, encoding="utf-8") as f:
          text = f.read()
      # Before the file is opened for writing: a failure leaves it as it was.
      hardened = harden(text)
      with open(path, "w", encoding="utf-8") as f:
          f.write(hardened)


  if __name__ == "__main__":
      main()
  ```

  Create `packaging/test-installer.sh`:

  ```sh
  #!/bin/sh
  # The hardened shell installer (distribution spec §2, §9), run against the
  # archives of this build, served from a directory (`file://`):
  #   - every archive's SHA-256 is embedded in it, and is the archive's;
  #   - it installs a binary that runs, with `sha256sum` or with `shasum`;
  #   - a checksum mismatch aborts, and installs nothing;
  #   - no SHA-256 tool, an empty checksum, no checksum at all, or a checksum
  #     style it does not know aborts, and installs nothing.
  # HOME and XDG_CONFIG_HOME point into a scratch directory, so nothing outside
  # it is written.
  #
  # Usage: test-installer.sh <directory holding hennery-installer.sh and every
  # archive with its .sha256>
  set -eu

  dist=$(cd "$1" && pwd)
  installer="$dist/hennery-installer.sh"
  scratch=$(mktemp -d)
  trap 'rm -rf "$scratch"' EXIT

  fail() {
      echo "FAIL: $*" >&2
      exit 1
  }

  # Every archive the installer offers has its checksum embedded, and it is the
  # one cargo-dist wrote beside that archive.
  arms=$(sed -n 's/^[[:space:]]*"\(hennery-.*\.tar\.xz\)")$/\1/p' "$installer")
  [ -n "$arms" ] || fail "the installer offers no archive"
  count=0
  for archive in $arms; do
      sum="$dist/$archive.sha256"
      [ -f "$sum" ] || fail "the installer offers $archive, and this build has no $archive.sha256"
      want=$(cut -d ' ' -f 1 "$sum")
      case "$want" in
      *[!0-9a-f]* | "") fail "$sum holds no SHA-256" ;;
      esac
      [ "${#want}" = 64 ] || fail "$sum holds no SHA-256"
      # The installer's case for this archive, up to its `;;`.
      arm=$(awk -v name="\"$archive\")" '$1 == name { on = 1 } on { print } on && $1 == ";;" { exit }' "$installer")
      printf '%s\n' "$arm" | grep -qx '[[:space:]]*_checksum_style="sha256"' ||
          fail "the installer has no sha256 checksum for $archive"
      printf '%s\n' "$arm" | grep -qx "[[:space:]]*_checksum_value=\"$want\"" ||
          fail "the installer's checksum for $archive is not the archive's"
      count=$((count + 1))
  done
  echo "ok: the installer embeds each of its $count archives' SHA-256"

  # run_installer <download dir> <install dir> <log>: the installer's exit status.
  run_installer() {
      mkdir -p "$scratch/home"
      status=0
      HOME="$scratch/home" XDG_CONFIG_HOME="$scratch/home/.config" \
          HENNERY_DOWNLOAD_URL="file://$1" HENNERY_UNMANAGED_INSTALL="$2" \
          sh "$1/hennery-installer.sh" >"$3" 2>&1 || status=$?
      return "$status"
  }

  # refused <name> <download dir> <message>: the installer, run on <download
  # dir>, fails with <message> and leaves no binary.
  refused() {
      if run_installer "$2" "$scratch/$1" "$scratch/$1.log"; then
          cat "$scratch/$1.log"
          fail "$1: installed"
      fi
      grep -q "$3" "$scratch/$1.log" || {
          cat "$scratch/$1.log"
          fail "$1: failed for another reason"
      }
      [ ! -e "$scratch/$1/hennery" ] || fail "$1: left a binary"
  }

  # tools <dir> <excluded>...: every command of /usr/bin and /bin but these.
  tools() {
      dir=$1
      shift
      mkdir "$dir"
      for from in /usr/bin /bin; do
          for tool in "$from"/*; do
              name=$(basename "$tool")
              for excluded in "$@"; do
                  [ "$name" = "$excluded" ] && continue 2
              done
              [ -e "$dir/$name" ] || ln -s "$tool" "$dir/$name"
          done
      done
  }

  # copy <name>: a download directory with the installer and the archives.
  copy() {
      mkdir "$scratch/$1"
      cp "$installer" "$dist"/*.tar.xz "$scratch/$1/"
      echo "$scratch/$1"
  }

  UNCHECKED="refusing to install an archive whose checksum cannot be checked"

  # A good install.
  run_installer "$dist" "$scratch/good" "$scratch/good.log" || {
      cat "$scratch/good.log"
      fail "the installer failed on its own archives"
  }
  "$scratch/good/hennery" --version || fail "the installed binary does not run"
  echo "ok: installs a binary that runs"

  # With `shasum` and no `sha256sum`.
  tools "$scratch/shasum-tools" sha256sum
  # In a subshell: an assignment before a function call may outlive it.
  (PATH="$scratch/shasum-tools" && run_installer "$dist" "$scratch/shasum" "$scratch/shasum.log") || {
      cat "$scratch/shasum.log"
      fail "the installer failed with shasum alone"
  }
  "$scratch/shasum/hennery" --version >/dev/null || fail "the binary installed with shasum does not run"
  echo "ok: installs with shasum alone"

  # A checksum mismatch: the same archives, one byte longer each.
  tampered=$(copy tampered-download)
  for archive in "$tampered"/*.tar.xz; do
      printf 'x' >>"$archive"
  done
  refused tampered "$tampered" "checksum mismatch"
  echo "ok: a checksum mismatch aborts"

  # No SHA-256 tool at all.
  tools "$scratch/no-tools" sha256sum shasum
  (PATH="$scratch/no-tools" && refused unchecked "$dist" "$UNCHECKED")
  echo "ok: no SHA-256 tool aborts"

  # An empty checksum.
  empty=$(copy empty-download)
  sed 's/^\([[:space:]]*_checksum_value=\)".*"$/\1""/' "$installer" >"$empty/hennery-installer.sh"
  refused empty "$empty" "$UNCHECKED"
  echo "ok: an empty checksum aborts"

  # No checksum at all.
  none=$(copy none-download)
  grep -v '^[[:space:]]*_checksum_style=' "$installer" >"$none/hennery-installer.sh"
  refused none "$none" "$UNCHECKED"
  echo "ok: no checksum aborts"

  # A checksum style the installer does not know.
  unknown=$(copy unknown-download)
  sed 's/^\([[:space:]]*_checksum_style=\)"sha256"$/\1"md5"/' "$installer" >"$unknown/hennery-installer.sh"
  refused unknown "$unknown" "$UNCHECKED"
  echo "ok: an unknown checksum style aborts"
  ```

  Create `packaging/check-archive.sh`:

  ```sh
  #!/bin/sh
  # A release archive holds the `hennery` binary, its licence and its README,
  # and nothing else (distribution spec §3.3): never an adapter, nor the Claude
  # CLI, which hennery does not redistribute.
  #
  # Usage: check-archive.sh <archive.tar.xz> <target>
  set -eu

  archive=$1
  dir="hennery-$2"
  listed=$(tar -tJf "$archive" | sort)
  expected=$(printf '%s\n' "$dir/" "$dir/LICENSE" "$dir/README.md" "$dir/hennery" | sort)
  if [ "$listed" != "$expected" ]; then
      echo "FAIL: $archive holds:" >&2
      echo "$listed" >&2
      echo "and should hold exactly:" >&2
      echo "$expected" >&2
      exit 1
  fi
  echo "ok: $archive holds the binary, its licence and its README alone"
  ```

  Create `packaging/check-formula.sh`:

  ```sh
  #!/bin/sh
  # The Homebrew formula names every archive of this build with the archive's
  # own SHA-256, on the line after its URL.
  #
  # Usage: check-formula.sh <directory holding hennery.rb and every archive's
  # .sha256>
  set -eu

  dist=$1
  formula="$dist/hennery.rb"
  count=0
  for sum in "$dist"/*.tar.xz.sha256; do
      archive=$(basename "$sum" .sha256)
      want=$(cut -d ' ' -f 1 "$sum")
      got=$(awk -v archive="/$archive\"" 'found { print; exit } $1 == "url" && index($2, archive) { found = 1 }' "$formula")
      case "$got" in
      *"sha256 \"$want\""*) ;;
      *)
          echo "FAIL: $formula does not give $archive its SHA-256 ($want): $got" >&2
          exit 1
          ;;
      esac
      count=$((count + 1))
  done
  urls=$(grep -c '^[[:space:]]*url "' "$formula")
  if [ "$urls" != "$count" ]; then
      echo "FAIL: $formula names $urls archives, this build made $count" >&2
      exit 1
  fi
  echo "ok: the formula gives each of its $count archives its SHA-256"
  ```

  Run: `chmod +x packaging/harden-installer.py packaging/test-installer.sh packaging/check-archive.sh packaging/check-formula.sh`

- [ ] **Step 4: The workflow**

  Create `.github/workflows/build.yml`:

  ```yaml
  # Release artifacts, built the way a release builds them (distribution spec
  # §2, plan 7a), and checked: Linux musl-static for both architectures and
  # macOS arm64, the shell installer and the Homebrew formula.
  #
  # This workflow publishes nothing. It has no tag trigger, no release, no
  # registry push, no attestation, and only `contents: read`. What it builds
  # stays a workflow artifact, kept for a week. Publishing is the operator's
  # to switch on (packaging/README.md).
  #
  # The archives hold the `hennery` binary alone, which `check-archive.sh`
  # asserts. No job here downloads, builds or uploads an adapter: the Claude CLI
  # is never redistributed (distribution spec §3.3).
  #
  # Third-party actions are pinned by commit, images by digest, cargo-dist by
  # SHA-256 (`install-dist.sh`); GitHub's own actions by major tag. Workflow
  # artifacts are never promoted to a release (packaging/README.md).
  name: build

  on:
    push:
      branches: [main]
    pull_request:
    workflow_dispatch:

  permissions:
    contents: read

  concurrency:
    group: build-${{ github.ref }}
    cancel-in-progress: true

  jobs:
    # One archive per target, each on a native runner.
    archive:
      strategy:
        fail-fast: false
        matrix:
          include:
            - target: x86_64-unknown-linux-musl
              runner: ubuntu-24.04
            - target: aarch64-unknown-linux-musl
              runner: ubuntu-24.04-arm
            - target: aarch64-apple-darwin
              runner: macos-latest
      runs-on: ${{ matrix.runner }}
      steps:
        - uses: actions/checkout@v4
          with:
            persist-credentials: false
        - uses: dtolnay/rust-toolchain@02cb101ec7c40f2c49e1d9714d64511d8e1b74de # master, 2026-10-02
          with:
            toolchain: stable
            targets: ${{ matrix.target }}
        - uses: Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6 # v2.9.2
          with:
            key: ${{ matrix.target }}
        # `musl-gcc` builds the C parts (OpenSSL, SQLite, ring) for musl.
        - name: musl tools (Linux)
          if: runner.os == 'Linux'
          run: sudo apt-get update && sudo apt-get install -y musl-tools
        - name: Install dist
          run: |
            sh packaging/install-dist.sh "$RUNNER_TEMP/dist-bin"
            echo "$RUNNER_TEMP/dist-bin" >> "$GITHUB_PATH"
        - name: Build the archive
          run: |
            dist build --artifacts=local --target=${{ matrix.target }} --print=linkage --output-format=json > dist-manifest.json
            cp dist-manifest.json "target/distrib/${{ matrix.target }}-dist-manifest.json"
        - name: Unpack the binary
          run: |
            mkdir -p unpacked
            tar -xJf "target/distrib/hennery-${{ matrix.target }}.tar.xz" -C unpacked
            echo "BINARY=$PWD/unpacked/hennery-${{ matrix.target }}/hennery" >> "$GITHUB_ENV"
        - name: Holds the binary alone
          run: sh packaging/check-archive.sh "target/distrib/hennery-${{ matrix.target }}.tar.xz" ${{ matrix.target }}
        - name: Links nothing outside the system
          run: sh packaging/check-linkage.sh "$BINARY"
        - name: Runs
          run: |
            "$BINARY" --version
            "$BINARY" collector --help > /dev/null
        - name: Runs on Alpine (musl)
          if: runner.os == 'Linux'
          run: docker run --rm -v "$BINARY:/hennery:ro" alpine:3@sha256:294b683cb724975bec92580e1e685676bd4b50bda910ddb8c51d4cabeaec77e6 /hennery --version
        - uses: actions/upload-artifact@v4
          with:
            name: archive-${{ matrix.target }}
            retention-days: 7
            if-no-files-found: error
            path: |
              target/distrib/hennery-${{ matrix.target }}.tar.xz
              target/distrib/hennery-${{ matrix.target }}.tar.xz.sha256
              target/distrib/${{ matrix.target }}-dist-manifest.json

    # What spans the archives: the installer (their checksums embedded), the
    # Homebrew formula, and `sha256.sum`.
    installers:
      needs: archive
      runs-on: ubuntu-24.04
      steps:
        - uses: actions/checkout@v4
          with:
            persist-credentials: false
        - name: Install dist
          run: |
            sh packaging/install-dist.sh "$RUNNER_TEMP/dist-bin"
            echo "$RUNNER_TEMP/dist-bin" >> "$GITHUB_PATH"
        - uses: actions/download-artifact@v4
          with:
            pattern: archive-*
            path: target/distrib/
            merge-multiple: true
        - name: Build the installers
          run: dist build --artifacts=global
        # cargo-dist's installer skips the check without `sha256sum`; the spec
        # wants it to refuse (packaging/harden-installer.py).
        - name: Harden the installer
          run: python3 packaging/harden-installer.py target/distrib/hennery-installer.sh
        - name: The installer installs, and refuses what it cannot check
          run: sh packaging/test-installer.sh target/distrib
        - name: The formula names every archive's SHA-256
          run: sh packaging/check-formula.sh target/distrib
        - uses: actions/upload-artifact@v4
          with:
            name: installers
            retention-days: 7
            if-no-files-found: error
            path: |
              target/distrib/hennery-installer.sh
              target/distrib/hennery.rb
              target/distrib/sha256.sum

    # The same installer, on macOS: its own `sha256sum` and `shasum`, its own
    # archive.
    installer-macos:
      needs: installers
      runs-on: macos-latest
      steps:
        - uses: actions/checkout@v4
          with:
            persist-credentials: false
        - uses: actions/download-artifact@v4
          with:
            pattern: archive-*
            path: target/distrib/
            merge-multiple: true
        - uses: actions/download-artifact@v4
          with:
            name: installers
            path: target/distrib/
        - name: The installer installs, and refuses what it cannot check
          run: sh packaging/test-installer.sh target/distrib
  ```

  Run: `grep -nE "tags:|contents: write|id-token|attestations:|packages:|gh release|docker push|attest" .github/workflows/*.yml`
  Expected: only `.github/workflows/build.yml:7:# registry push, no attestation, and only `contents: read`. What it builds`, its comment.

- [ ] **Step 5: The checks**

  Run the five checks of "Global Constraints".
  Expected: all pass; the test count is Task 2's.

- [ ] **Step 6: Commit, push, and the workflow's run**

  ```bash
  git add Cargo.toml crates/hennery/Cargo.toml dist-workspace.toml packaging/install-dist.sh packaging/harden-installer.py packaging/test-installer.sh packaging/check-archive.sh packaging/check-formula.sh .github/workflows/build.yml
  git diff --cached --stat
  git -c commit.gpgsign=false commit -m "build(release): build the release archives and installers in CI, and publish nothing"
  git push origin feat/release-build
  ```

  Open the PR (a draft until Task 4), and read the `build` run's job logs, not only its status:
  - each `archive` job prints `ok: …hennery-<target>.tar.xz holds the binary, its licence and its README alone`, `ok: …/hennery links nothing outside the system` and `hennery 0.0.0`, and the Linux ones `hennery 0.0.0` again from Alpine;
  - `installers` and `installer-macos` print `test-installer.sh`'s eight `ok:` lines, the first `ok: the installer embeds each of its 3 archives' SHA-256`; `installers` then `ok: the formula gives each of its 3 archives its SHA-256`.

- [ ] **Step 7: Revert-probes, on the run's artifacts**

  The run's archives, and an installer built from them here, unhardened:

  ```bash
  RUN=<the build run's id, from `gh run list --branch feat/release-build`>
  A=$(mktemp -d) && gh run download "$RUN" --repo marcinwadon/hennery -D "$A"
  rm -rf target/distrib && mkdir -p target/distrib && cp "$A"/archive-*/* target/distrib/
  nix shell nixpkgs#cargo-dist nixpkgs#cargo nixpkgs#rustc -c dist build --artifacts=global
  P=$(mktemp -d) && cp target/distrib/*.tar.xz target/distrib/*.tar.xz.sha256 target/distrib/hennery.rb "$P"/
  cp target/distrib/hennery-installer.sh "$P/unhardened.sh"
  ```

  (`gh` as fleet rules run it, with the personal account's token.) Each probe below, applied alone, must fail at the named check; `sh packaging/test-installer.sh "$P"` with the fully hardened installer (`cp "$P/unhardened.sh" "$P/hennery-installer.sh" && python3 packaging/harden-installer.py "$P/hennery-installer.sh"`) passes all eight first.
  - Each of `harden-installer.py`'s four replacements left out in turn (`REPLACEMENTS.pop(i)` in a copy of the script, run on `unhardened.sh`):
    - the first (no `sha256sum`): `FAIL: unchecked: installed`;
    - the second (an unknown style): `FAIL: unknown: installed`;
    - the third (an empty checksum): `FAIL: empty: installed`;
    - the fourth (no checksum): `FAIL: none: installed`.
  - The `elif check_cmd shasum` branch removed from the first replacement's text: `FAIL: the installer failed with shasum alone`.
  - One archive's `_checksum_style` and `_checksum_value` lines deleted from its case of the hardened installer: `FAIL: the installer has no sha256 checksum for hennery-x86_64-unknown-linux-musl.tar.xz`.
  - One archive's `.sha256` removed from `$P`: `FAIL: the installer offers hennery-aarch64-apple-darwin.tar.xz, and this build has no hennery-aarch64-apple-darwin.tar.xz.sha256`.
  - `check-formula.sh` on `$P` with the formula of a one-archive build (`dist build --artifacts=global` given only the macOS manifest): `FAIL: …hennery.rb does not give hennery-aarch64-apple-darwin.tar.xz its SHA-256`.
  - `check-archive.sh` given another target's name than the archive's: `FAIL: … holds:`.

---

### Task 4: The collector image

**Files:**
- Create: `Dockerfile`, `.dockerignore`, `packaging/smoke-image.sh`, `packaging/README.md`
- Modify: `.gitignore` (`/dist/`), `.github/workflows/build.yml` (the image steps)

**Interfaces:**
- Consumes: Task 2's `collector healthcheck`; Task 3's `archive` job and `$BINARY`.
- Produces: the image `hennery-collector:ci` inside the Linux `archive` jobs, built from `dist/linux-<amd64|arm64>/hennery` and checked by `packaging/smoke-image.sh <image>`. It is not pushed or uploaded.

No container runtime is available here, so the image is proven by the workflow alone: the `archive` job's "The image starts and is healthy" step, on both architectures.

- [ ] **Step 1: The image**

  Create `Dockerfile`:

  ```dockerfile
  # syntax=docker/dockerfile:1@sha256:4edf897a3ffa55b89f906fc8cc78afdb3f1834cc9c7083565e611a8a7d5fe99e
  # The collector's image (distribution spec §4.1): the released musl binary,
  # the same bytes as the archive, on distroless static (CA roots and a
  # non-root user, no shell). No Node and no adapters: hosts run next to the
  # repositories, never in this image. The `build` workflow stages the binary
  # at `dist/linux-<arch>/hennery` (plan 7a). Every image is pinned by its
  # multi-arch index digest, the tag beside it for the reader.

  # The data directory, owned by the image's user. A volume Docker creates on
  # a path the image lacks would be root's, and the collector, running as
  # 65532, could not write it. Distroless has no shell to make it with, so a
  # stage of the same base's debug variant (busybox) does, 0700. Its parent is
  # copied, not the directory: `COPY` of a directory copies what it holds, and
  # makes the target with a mode of its own, whatever `--chmod` says.
  FROM gcr.io/distroless/static-debian13:debug@sha256:07148a6899406df51906b183f581cf66e5b05fd51aca438bdf1d3998df566961 AS data
  RUN ["/busybox/mkdir", "-p", "-m", "0700", "/out/var/lib/hennery"]

  FROM gcr.io/distroless/static-debian13:nonroot@sha256:e2e927ec666bae08560abb3c55d0659eceabb657f56b6782ab500a9fc7f555e3
  ARG TARGETARCH
  COPY --chmod=0755 dist/linux-${TARGETARCH}/hennery /usr/local/bin/hennery
  COPY --from=data --chown=65532:65532 /out/var/lib/ /var/lib/
  ENV HENNERY_DATA_DIR=/var/lib/hennery HENNERY_LISTEN=0.0.0.0:8080
  # Local storage only: SQLite's locking is unreliable on network filesystems.
  VOLUME ["/var/lib/hennery"]
  USER 65532:65532
  EXPOSE 8080
  HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/hennery", "collector", "healthcheck"]
  ENTRYPOINT ["/usr/local/bin/hennery"]
  CMD ["collector"]
  ```

  Create `.dockerignore`:

  ```
  # The image takes only the staged binary (see Dockerfile).
  *
  !dist/
  ```

  In `.gitignore`, replace:

  ```
  /.superpowers/
  ```

  with:

  ```
  /.superpowers/
  /dist/
  ```

- [ ] **Step 2: The image's check**

  Create `packaging/smoke-image.sh`:

  ```sh
  #!/bin/sh
  # The collector image starts and is healthy (distribution spec §4.1, §9):
  # run with the anonymous volume its `VOLUME` makes, as its own user, the
  # collector must find that directory its own (65532, 0700), write it, answer
  # `collector healthcheck`, and be reported healthy by Docker's own
  # `HEALTHCHECK`. Reading the volume's directory on the host needs `sudo`.
  #
  # Usage: smoke-image.sh <image>
  set -eu

  image=$1
  name="hennery-smoke-$$"
  trap 'docker rm -f "$name" >/dev/null 2>&1 || true' EXIT

  fail() {
      docker logs "$name" >&2 || true
      echo "FAIL: $*" >&2
      exit 1
  }

  # Docker's checks start one interval after the start: shortened here.
  docker run -d --name "$name" --health-interval=2s "$image" >/dev/null

  # The healthcheck itself, as the HEALTHCHECK runs it (no shell in the image).
  healthy=0
  for _ in $(seq 1 30); do
      if docker exec "$name" /usr/local/bin/hennery collector healthcheck; then
          healthy=1
          break
      fi
      sleep 1
  done
  [ "$healthy" = 1 ] || fail "collector healthcheck never passed"
  echo "ok: collector healthcheck passes"

  # The database is in the volume, so the collector could write it.
  docker exec "$name" /usr/local/bin/hennery admin hosts >/dev/null ||
      fail "the admin socket in the data directory does not answer"
  volume=$(docker inspect --format '{{range .Mounts}}{{if eq .Destination "/var/lib/hennery"}}{{.Source}}{{end}}{{end}}' "$name")
  [ -n "$volume" ] || fail "no volume at /var/lib/hennery"
  mode=$(sudo stat -c '%u:%g %a' "$volume")
  [ "$mode" = "65532:65532 700" ] || fail "the data directory is $mode, not 65532:65532 700"
  # The collector warns when it finds its data directory open to others.
  if docker logs "$name" 2>&1 | grep -q "other users"; then
      fail "the collector warned about its data directory's permissions"
  fi
  echo "ok: the data directory is the collector's (65532:65532 700)"

  status=
  for _ in $(seq 1 30); do
      status=$(docker inspect --format '{{.State.Health.Status}}' "$name")
      [ "$status" = healthy ] && break
      sleep 1
  done
  [ "$status" = healthy ] || fail "Docker reports the container $status"
  echo "ok: Docker reports the container healthy"

  # In the image itself: the directory is 65532's and 0700, and its parent is
  # still root's (`COPY --from=data … /var/lib/` merges into the existing one).
  listing=$(docker export "$name" | tar -tvf - | grep -E ' var/lib/(hennery/)?$' || true)
  printf '%s\n' "$listing" | grep -Eq '^drwx------ +65532/65532 .* var/lib/hennery/$' ||
      fail "the image's /var/lib/hennery is not 65532's and 0700: $listing"
  printf '%s\n' "$listing" | grep -Eq '^drwxr-xr-x +0/0 .* var/lib/$' ||
      fail "the image's /var/lib is not root's and 0755: $listing"
  echo "ok: the image's /var/lib/hennery is 65532's and 0700, /var/lib root's"

  user=$(docker inspect --format '{{.Config.User}}' "$image")
  [ "$user" = 65532:65532 ] || fail "the image runs as $user"
  echo "ok: the image runs as 65532:65532"
  ```

  Run: `chmod +x packaging/smoke-image.sh`

- [ ] **Step 3: The workflow builds and checks it**

  In `.github/workflows/build.yml`, replace:

  ```yaml
  # macOS arm64, the shell installer and the Homebrew formula.
  ```

  with:

  ```yaml
  # macOS arm64, the shell installer, the Homebrew formula and the collector
  # image.
  ```

  In `.github/workflows/build.yml`, replace:

  ```yaml
            - target: x86_64-unknown-linux-musl
              runner: ubuntu-24.04
            - target: aarch64-unknown-linux-musl
              runner: ubuntu-24.04-arm
  ```

  with:

  ```yaml
            - target: x86_64-unknown-linux-musl
              runner: ubuntu-24.04
              image: amd64
            - target: aarch64-unknown-linux-musl
              runner: ubuntu-24.04-arm
              image: arm64
  ```

  In `.github/workflows/build.yml`, replace:

  ```yaml
          run: docker run --rm -v "$BINARY:/hennery:ro" alpine:3@sha256:294b683cb724975bec92580e1e685676bd4b50bda910ddb8c51d4cabeaec77e6 /hennery --version
  ```

  with:

  ```yaml
          run: docker run --rm -v "$BINARY:/hennery:ro" alpine:3@sha256:294b683cb724975bec92580e1e685676bd4b50bda910ddb8c51d4cabeaec77e6 /hennery --version
        - name: Build the collector image
          if: runner.os == 'Linux'
          run: |
            mkdir -p "dist/linux-${{ matrix.image }}"
            cp "$BINARY" "dist/linux-${{ matrix.image }}/hennery"
            docker build -t hennery-collector:ci .
        - name: The image starts and is healthy
          if: runner.os == 'Linux'
          run: sh packaging/smoke-image.sh hennery-collector:ci
  ```

- [ ] **Step 4: What is built, and what publishing needs**

  Create `packaging/README.md`:

  ````markdown
  # Release builds

  What a hennery release would ship is built by the `build` workflow
  (`.github/workflows/build.yml`) on every pull request and every push to
  `main` (distribution spec §2, plan 7a). It is checked, then kept as workflow
  artifacts for a week. **Nothing is published:** there are no releases yet.

  | Artifact | Built by | Checked by |
  |---|---|---|
  | `hennery-x86_64-unknown-linux-musl.tar.xz`, `hennery-aarch64-unknown-linux-musl.tar.xz` | `dist build`, natively on each architecture, with `musl-gcc` | `check-archive.sh` (the binary, its licence and README, nothing else); `check-linkage.sh` (no shared library, no loader); runs on the runner and on Alpine |
  | `hennery-aarch64-apple-darwin.tar.xz` | `dist build` on macOS arm64 | `check-archive.sh`; `check-linkage.sh` (system libraries only); runs |
  | `hennery-installer.sh` | `dist build --artifacts=global`, then `harden-installer.py` | `test-installer.sh`, on Linux x86_64 and on macOS |
  | `hennery.rb` (Homebrew formula) | `dist build --artifacts=global` | `check-formula.sh` |
  | `sha256.sum` | `dist build --artifacts=global` | — |
  | the collector image, one architecture per Linux runner | `docker build` of `Dockerfile` on the staged musl binary | `smoke-image.sh` |

  Every archive builds with the `vendored-openssl` feature
  (`dist-workspace.toml`). OpenSSL, which `webauthn-rs-core` links, is then
  compiled in from source. Without it, the macOS binary would load Homebrew's
  `libssl` and fail on a Mac without it, and a static musl build would not
  link at all (distribution §1.1). Development and `ci.yml` use the system
  OpenSSL.

  Third-party actions are pinned by commit, every image by its multi-arch
  digest, and cargo-dist by the SHA-256 in `install-dist.sh`.

  ## OpenSSL

  The release binaries carry their own OpenSSL: 3.6.3, from `openssl-src`
  300.6.1+3.6.3 in `Cargo.lock`. Updating the system's OpenSSL does not
  reach them. An OpenSSL fix reaches users only when `openssl-src` is bumped
  in `Cargo.lock` and a new release is made. `webauthn-rs` passes what a
  browser sends (attestation and assertion data) through it, so OpenSSL's
  advisories are this project's to follow.

  OpenSSL 3.6 is not a long-term-support line. Check its end of support
  against openssl.org's release strategy before the first release, and move
  to a supported line if it ends first.

  ## Locally

  The dev shell's toolchain links Nix's `libiconv` on macOS, so a local
  release build is for trying out, not for handing on; `check-linkage.sh` says
  so. Linux archives and the image are built in CI only.

  ```sh
  nix develop -c cargo build --profile dist -p hennery --features vendored-openssl
  sh packaging/check-linkage.sh target/dist/hennery
  ```

  ## Publishing is the operator's

  These need the operator's decision, credentials, or both. None is set up:

  - **GitHub Releases.** `dist generate` writes a `release.yml` that builds the
    same archives and creates a release on every version tag. It is not
    committed (`allow-dirty = ["ci"]` in `dist-workspace.toml`). Committed, it
    must:
    - keep this workflow's checks: `check-archive.sh`, `check-linkage.sh`,
      `test-installer.sh`, `check-formula.sh`;
    - run `harden-installer.py` before the installer is uploaded, and before
      it is attested;
    - pin every action by commit;
    - restore no Actions cache (no `rust-cache`), since anything a `main`
      workflow runs can write one;
    - build with a pinned toolchain (`rust-toolchain.toml`).

    The `build` workflow's artifacts are never promoted to a release: a pull
    request's are built from code nobody has reviewed yet.
  - **Artifact attestations** (`github-attestations = true`): they need
    `id-token: write` and `attestations: write`, and they write to the public
    Sigstore log. The README's "verified install" (`gh attestation verify
    hennery-installer.sh --repo marcinwadon/hennery`) is documented with the
    first release.
  - **The Homebrew tap** (`marcinwadon/homebrew-tap`): the repository and a
    token allowed to push to it.
  - **The image on GHCR:** `packages: write`, one multi-arch manifest from
    the per-architecture images, and its provenance attestation. Its docs
    should say: publish the port on loopback behind a TLS proxy
    (`-p 127.0.0.1:8080:8080`); read the setup link with
    `docker exec … hennery admin setup-url`, not from `docker logs` (with
    `docker run -t` the link itself is logged); a bind mount keeps the host's
    owner, so it must belong to 65532.
  - **Dependency updates** (Dependabot, or another bot): for `openssl-src`
    above all, the pinned actions and the image digests. It opens pull
    requests on its own schedule, so it is the operator's to switch on.
  - **Fork pull requests:** require approval before their workflows run.

  ---

  _Generated with Claude AI — please review before distribution._
  ````

  Run: `grep -nE "tags:|contents: write|id-token|attestations:|packages:|gh release|docker push|attest" .github/workflows/*.yml`
  Expected: only `build.yml`'s line 7, its comment.

- [ ] **Step 5: The checks**

  Run the five checks of "Global Constraints".
  Expected: all pass; the test count is Task 2's.

- [ ] **Step 6: Commit, push, and the workflow's run**

  ```bash
  git add Dockerfile .dockerignore .gitignore packaging/smoke-image.sh packaging/README.md .github/workflows/build.yml
  git diff --cached --stat
  git -c commit.gpgsign=false commit -m "build(release): build the collector image in CI and check it starts healthy"
  git push origin feat/release-build
  ```

  Read both Linux `archive` jobs' logs: after `naming to docker.io/library/hennery-collector:ci`, `smoke-image.sh` prints `ok: collector healthcheck passes`, `ok: the data directory is the collector's (65532:65532 700)`, `ok: Docker reports the container healthy`, `ok: the image's /var/lib/hennery is 65532's and 0700, /var/lib root's`, `ok: the image runs as 65532:65532`.

- [ ] **Step 7: Revert-probe, in CI**

  The image's `data` stage is the one fix §4.1's Dockerfile needed (decision 7), and only CI can build the image. Push one probe commit that removes `"-m", "0700", ` from the `data` stage's `mkdir`, and read the run:
  - Expected: both Linux `archive` jobs fail at "The image starts and is healthy" with `FAIL: the data directory is 65532:65532 755, not 65532:65532 700`.

  Then drop the probe commit (`git reset --hard HEAD~1`, `git push --force-with-lease origin feat/release-build`) and see the next run pass.

---

## After this plan

**Publishing, the operator's** (packaging/README.md has the list):
- **A version and a tag.** The workspace is `0.0.0`. The first release needs a version, and `dist generate`'s `release.yml`, which (A7):
  - keeps this plan's checks: `check-archive.sh`, `check-linkage.sh`, `test-installer.sh`, `check-formula.sh`;
  - runs `harden-installer.py` before the installer is uploaded or attested;
  - pins every action by commit;
  - restores no Actions cache;
  - builds with a pinned toolchain (`rust-toolchain.toml`).

  The `build` workflow's artifacts are never promoted to a release.
- **Attestations** (`github-attestations = true` is set): `id-token: write`, `attestations: write`, the public Sigstore log. The README's "verified install" with `gh attestation verify` comes with the first release.
- **The Homebrew tap:** the repository `marcinwadon/homebrew-tap` and a token that may push to it.
- **GHCR:** `packages: write`, a multi-arch manifest from the two per-architecture images, and its provenance attestation.
- **The toolchain:** CI builds with `stable` from `dtolnay/rust-toolchain`. A release should pin it (`rust-toolchain.toml`) so a rebuilt release is the same build.
- **OpenSSL:** 3.6.3 is vendored. Check 3.6's end of support before the first release (A6). Follow OpenSSL's advisories: a fix needs an `openssl-src` bump and a release.
- **Before the first release, a gate** (the re-confirmation's note on decision 14): Dependabot *alerts* switched on (a repository setting that opens no pull request), or a committed `dependabot.yml`. Until then nothing says when the vendored OpenSSL or a pinned image needs a security fix, and the pinned commits and digests are bumped by hand.
- **Approval for fork pull requests' workflows** (O6): a repository setting, the operator's.
- **The image's docs** (O5): loopback port behind a TLS proxy; the setup link from `docker exec … hennery admin setup-url`, not `docker logs`; a bind mount must belong to 65532.

**For the other parts of plan 7:**
- **7b (managed runtime):** no artifact of this workflow may ever hold an adapter set. The pin-bump job is 7b's, as is its live e2e gate, which needs logged-in agents in CI (an operator item).
- **7c (services):** the installer's path is `~/.local/bin/hennery` (decision 5); units resolve the absolute path at install time.
- **7d (doctor):** check 2's glibc ≥ 2.28 for a host (§1.1), since the musl binary runs on musl systems that cannot run a host (decision 11). Check 11, another `hennery` earlier on PATH: a Homebrew and a curl install can both exist.
- **7e (Nix):** the flake's package is built by crane, not cargo-dist; it needs `vendored-openssl` off and nixpkgs' OpenSSL instead, or the feature on and `perl` in the build.

**Plan 4 (frontend):** §2's "frontend build runs in the pre-build hook and is embedded". Once it exists, `dist build` must build it first: `build.rs` or a `github-build-setup` step, in `build.yml` and in a future `release.yml`.

**Not tested here:**
- **Linux, Docker and the image:** only CI's runs prove them, on the draft PR #24 (run of 2026-10-01: both `archive` jobs for Linux, the image's four checks on each architecture, `installers`' three).
- **The installer on Linux arm64:** `test-installer.sh` runs on x86_64 Linux and on macOS, not on an arm64 Linux runner.
- **The Homebrew formula:** `check-formula.sh` checks its checksums; it names release URLs that do not exist yet, so `brew install` of it is untested.
- **`collector healthcheck` against a TLS listener:** none exists; the collector serves plain HTTP behind a proxy (kernel §7).
- **A volume that is a bind mount:** Docker copies nothing into one, so its owner is the host's. The image's docs, with publishing, must say so (O5).

**Spec amendments** (distribution spec):
- decision 2: §1.1: OpenSSL is vendored for macOS too;
- decision 4: §2: the installer is cargo-dist's, hardened: the four cases that now refuse, and `shasum` where `sha256sum` is missing;
- decision 5: §4.2: the installer's directory is `~/.local/bin`;
- decision 6: §1: `collector healthcheck`'s address (flags, variables, `config.toml`), its 2 s budget for the whole check, and only a 200 passing;
- decision 7: §4.1: the Dockerfile's `data` stage;
- decision 13: §4.1: the Dockerfile's images pinned by digest;
- decision 11: §9: "musl/glibc detection" means the musl-static archive on every Linux;
- §2: the build half, without publishing, in `build.yml`; publishing is the operator's.

---

_Generated with Claude AI — please review before distribution._
