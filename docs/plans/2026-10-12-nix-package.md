# The Nix package and its flake checks (plan 7e-i) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** The flake builds `hennery` and checks it (distribution spec §4.3, §9): `nix build` gives the binary, and `nix flake check` builds it and runs clippy, rustfmt and cargo-audit. CI runs both on Linux and macOS on every pull request. Nothing is pushed to a binary cache.

**Architecture:**
- `nix/package.nix` (new): crane on nixpkgs' own Rust toolchain. `buildDepsOnly` once, shared by the package and the clippy check. The source filter is crane's Cargo sources, plus the files a crate compiles in (`adapters/manifest.json` once plan 7b adds it).
- `flake.nix`: two inputs, `crane` (v0.24.0) and `advisory-db`, and `packages.default` and `checks` beside the unchanged dev shell. `flake.lock` gains exactly those two nodes.
- `.github/workflows/nix.yml` (new): `nix flake check` and `nix run . -- --version`, on ubuntu and macOS, with the Nix installer pinned by commit.

**Tech Stack:** Nix flakes; crane 0.24.0; nixpkgs (the locked `nixos-unstable`, unchanged): cargo and rustc 1.98, OpenSSL 3.6; RustSec's advisory database; `cachix/install-nix-action` 31.11.1.

**Spec:** [`docs/specs/2026-09-26-distribution-design.md`](../specs/2026-09-26-distribution-design.md):
- §4.3: "**Rust:** `crane` (`buildDepsOnly` for caching; clippy/test/audit as flake checks; crate hashes come from `Cargo.lock`, so no per-system vendor hash), toolchain pinned through an overlay." The frontend derivation, the adapters from the manifest, and the NixOS/home-manager modules are the rest of §4.3: 7e-ii's, after plans 4 and 7b.
- §9: "Nix: flake checks build the binary and the adapter derivation on Linux and macOS in CI."
- §3.3: the Claude adapter is never in a public cache; this plan builds no adapter and pushes no cache.

Anchors are `main` at `c05d642` (PR #43).

**Status:** executed 2026-10-02 (see "Execution status"); amended after the security review. The security review of 2026-10-02 (binding on the maintainer's behalf) approved with one amendment, A1: the audit check revert-probed (Task 1, Step 4). Taken, with O1–O4; O5 and O6 recorded under "After this plan"; decisions 1–8 confirmed.

The code was built on `feat/nix-package` and its CI run on PR #45 (run 36952694016, before the amendments): both `flake` jobs green, the audit loading 1279 advisories, `hennery 0.0.0` from the package. Every revert-probe of Task 1 was run here and failed as written. The plan was replayed from its own text onto `c05d642`, task by task, and the tree matched the branch byte for byte. The dev shell's checks are unaffected: no Rust file changes.

## Execution status (2026-10-02)

**Executed** on branch `exec/nix-package`, pushed as `feat/nix-package` (PR #45), on `main` at `b2aee2e` (PR #46). `flake.nix` and `flake.lock` are the same there as at the anchors' `c05d642`. Task 1 was done by one implementer and reviewed (opus): approved. Task 2 is a single new workflow, so its review was folded into the whole-branch review (opus), which found it "ready after fixes"; the fixes are below, and a scoped re-review followed. The code is byte-identical to the planning branch, whose CI run after the review's amendments (36953876775) passed both `flake` jobs: 1279 advisories loaded, `hennery 0.0.0` from the package. Every revert-probe of Task 1 was run again in execution, each failing as written; the audit's against an advisory database copied to `/private/tmp`.

| Area | As built | Why |
|---|---|---|
| `nix.yml`'s `concurrency` (final review) | `group: nix-${{ github.event_name == 'pull_request' && github.ref || github.sha }}` with `cancel-in-progress: true` | The planned `cancel-in-progress: false` for `main` keeps one running and one pending run per group, so a third quick merge cancels the second's pending run. A group per commit on `main` keeps every one |
| "`aarch64-linux` is evaluated, not built" (final review) | Corrected: neither evaluated nor built by CI | `nix flake check` without `--all-systems` skips other systems entirely: the logs say "omitted these incompatible systems" |
| Decision 9 (Task 1 review) | It says Intel Macs lose `nix develop` too | `eachSystem` limits the dev shell as well as the package |

Deferred minors: `dist-workspace.toml` rides in the build source (an edit to it rebuilds the dependency cache); the flake's clippy check mirrors only `ci.yml`'s workspace lane, not its `-p hennery` one; the token install-nix-action writes into `nix.conf` is readable by unsandboxed macOS builds (read-only, job-scoped, and `ci.yml`'s checkout exposes the same); `actions/checkout@v4`'s Node 20 deprecation, for all three workflows together.

Tests: none added; the workspace's are unchanged. CI on the final commit: PR #45's last run, both `flake` jobs green.

## Scope

7e is split in two:
- **7e-i, this plan:** the package and its checks, and CI;
- **7e-ii:** the adapter derivations from the manifest (7b), the frontend derivation (plan 4), and the NixOS and home-manager modules (7c's services).

That is **2 tasks:** (1) the package and its checks; (2) CI.

## Decisions this plan makes where the spec is silent

1. **nixpkgs' toolchain, not an overlay.** §4.3 says "toolchain pinned through an overlay" (a `rust-overlay` input). The locked nixpkgs already pins one toolchain, and it is the dev shell's: the clippy that every lane's `-D warnings` runs is the one the flake check runs. An overlay would add an input to bump and let the two drift, or move every lane's dev shell to a new clippy at once. `crane.mkLib pkgs` uses `pkgs.cargo` and `pkgs.rustc`.
2. **OpenSSL from nixpkgs, not vendored.** The `vendored-openssl` feature (plan 7a) is for the release archives, which must run without the store. A Nix build links its dependencies from the store, and nixpkgs patches OpenSSL. The package is built with `-p hennery` and no feature.
3. **No test check.** §4.3 lists tests among the flake checks. The suite spawns processes, binds loopback ports, makes Unix sockets whose paths must stay short, and has a Linux test that must run under `CI` (plan 7a-ii); a build sandbox promises none of that, and `ci.yml` runs the suite on both systems already. The package is built with `doCheck = false`.
4. **`cargo audit` with a pinned advisory database, yanked crates unchecked.** The audit is §4.3's. In the sandbox it has no network, so its check for yanked crates prints one error line per crate and does not fail. It reports vulnerabilities from the database `flake.lock` pins (`6de4455`, 2026-10-01). Silencing the yanked lines would need an `audit.toml` that turns the check off for developers too.
5. **The source filter names the compiled-in files.** crane's Cargo sources are `.rs`, `.toml` and the lock. `adapters/manifest.json`, which plan 7b's host crate `include_str!`s, is added with `lib.fileset.maybeMissing` (the review's O2), so this plan and 7b-i can land in either order. Today nothing but `.rs` files is compiled in (`git grep include_str!` on `main`: the testkit's `owner_filter.rs` reads `.rs` files only).
6. **The version comes from the workspace's `Cargo.toml`** (`workspace.package.version`), read by the flake, so it cannot drift from the binary's.
7. **A Nix CI job on every pull request, on both systems,** as §9 asks. A lane that adds a compiled-in file, or a clippy warning only nixpkgs' clippy sees, is caught on its own PR. About 9 minutes per system from a cold store, in parallel with the other jobs. No cache action: Cachix needs a token and pushes; GitHub's cache action is a third-party cache to pin and trust. Recorded under "After this plan". A pull request's superseded run is cancelled; on `main` every commit is its own concurrency group and keeps its run (the review's O3, as built after the final review). A job stops after 30 minutes (O4).
8. **The job's read-only token goes to Nix,** so fetching the locked GitHub inputs is not refused by the anonymous API limit that shared runners share. It has `contents: read` only, and nothing in the job writes.
9. **The three v1 platforms, not `eachDefaultSystem`** (the review's O1): `x86_64-linux`, `aarch64-linux`, `aarch64-darwin` (§1), and `meta.platforms` the same. An Intel Mac gets no package rather than an untested one, and no `nix develop` either: the dev shell is limited with the rest. CI builds `x86_64-linux` and `aarch64-darwin`; `aarch64-linux` is neither evaluated nor built (`nix flake check` skips other systems without `--all-systems`).

## Global Constraints

- After every task: `nix develop -c cargo fmt --all --check`, both clippy runs, the workspace tests and the codegen check pass, as before. The dev shell is unchanged.
- `flake.lock` changes once, in Task 1, by exactly the two new nodes (`crane`, `advisory-db`) and their entries in `root`; `nixpkgs`, `flake-utils` and `systems` are unchanged. Check it: the `nodes` key sets before and after differ by those two, and no other node's entry changes.
- Flakes ignore untracked files: `git add` a new file before any `nix` command reads it.
- **Nothing publishes:** no cache push, no `cachix` token, no write permission.
- **Third-party actions pinned by commit.**

## Review Focus

1. **A plan that adds a compiled-in file** (7b's `adapters/manifest.json`). Expected: the package still builds. Pinned by Task 1's revert-probe: a crate that `include_str!`s the manifest builds with the filter and fails without it ("couldn't read … adapters/manifest.json").
2. **A clippy warning or a misformatted line in any lane's PR.** Expected: `nix flake check` fails on it, as `ci.yml` does. Revert-probed (Task 1).
3. **A user running `nix run github:marcinwadon/hennery -- --version`.** Expected: the binary runs; its OpenSSL is nixpkgs'. Pinned by Task 2's CI step on both systems.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `nix/package.nix` (new) | The package, its source filter, and the checks | 1 |
| `flake.nix`, `flake.lock` | The two inputs; `packages.default`, `checks` | 1 |
| `.github/workflows/nix.yml` (new) | `nix flake check` and the package run, on both systems | 2 |

**Reading the steps:** "Create `path`:" makes a new file with the block; "Replace the whole of `path` with:" overwrites it; "In `path`, replace:" is followed by a block that occurs exactly once, then "with:" and its replacement.

---

### Task 1: The package and its checks

**Files:**
- Create: `nix/package.nix`
- Modify: `flake.nix`, `flake.lock`

**Interfaces:**
- Produces: `packages.<system>.default` (the `hennery` binary, `meta.mainProgram = "hennery"`) and `checks.<system>.{package,clippy,fmt,audit}`. Task 2 runs them.

- [ ] **Step 1: The package**

  Create `nix/package.nix`:

  ```nix
  # The `hennery` package and its flake checks (plan 7e-i, distribution spec
  # §4.3), built with crane on nixpkgs' own Rust toolchain: the one the dev
  # shell has, so `nix flake check` and the dev shell's `cargo clippy` agree.
  # OpenSSL is nixpkgs', not the release build's vendored copy: a Nix build
  # links its dependencies from the store.
  { pkgs, crane, advisory-db }:
  let
    inherit (pkgs) lib stdenv;
    craneLib = crane.mkLib pkgs;
    workspace = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).workspace.package;
    # Only what cargo reads: the crates, the manifests and the lock, and the
    # files a crate compiles in with `include_str!`. The adapter manifest
    # (`adapters/manifest.json`, distribution spec §3.1) is named once it
    # exists, so the build does not depend on which plan lands first.
    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.unions [
        (craneLib.fileset.commonCargoSources ../.)
        (lib.fileset.maybeMissing ../adapters/manifest.json)
      ];
    };
    common = {
      inherit src;
      strictDeps = true;
      pname = "hennery";
      inherit (workspace) version;
      nativeBuildInputs = [ pkgs.pkg-config ];
      buildInputs = [ pkgs.openssl ] ++ lib.optionals stdenv.hostPlatform.isDarwin [ pkgs.libiconv ];
    };
    # Every dependency of the workspace, built once and shared by the package
    # and the checks.
    cargoArtifacts = craneLib.buildDepsOnly common;
    package = craneLib.buildPackage (
      common
      // {
        inherit cargoArtifacts;
        cargoExtraArgs = "--locked -p hennery";
        # The tests run in CI's own jobs (`ci.yml`): they spawn processes and
        # bind loopback ports, which a build sandbox does not promise.
        doCheck = false;
        meta = {
          description = "Self-hosted cockpit for ACP coding agents";
          license = lib.licenses.agpl3Only;
          mainProgram = "hennery";
          # The v1 platforms (distribution spec §1).
          platforms = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
        };
      }
    );
  in
  {
    inherit package;
    checks = {
      inherit package;
      clippy = craneLib.cargoClippy (
        common
        // {
          inherit cargoArtifacts;
          cargoClippyExtraArgs = "--workspace --all-targets -- -D warnings";
        }
      );
      fmt = craneLib.cargoFmt { inherit (common) src pname version; };
      audit = craneLib.cargoAudit { inherit (common) src pname version; inherit advisory-db; };
    };
  }
  ```

- [ ] **Step 2: The flake**

  In `flake.nix`, replace:

  ```nix
      flake-utils.url = "github:numtide/flake-utils";
    };
  ```

  with:

  ```nix
      flake-utils.url = "github:numtide/flake-utils";
      # The package and its checks (plan 7e-i, distribution spec §4.3).
      crane.url = "github:ipetkov/crane/v0.24.0";
      advisory-db = {
        url = "github:rustsec/advisory-db";
        flake = false;
      };
    };
  ```

  In `flake.nix`, replace:

  ```nix
    outputs = { nixpkgs, flake-utils, ... }:
      flake-utils.lib.eachDefaultSystem (system:
        let pkgs = import nixpkgs { inherit system; };
        in {
  ```

  with:

  ```nix
    outputs = { nixpkgs, flake-utils, crane, advisory-db, ... }:
      # The v1 platforms (distribution spec §1): Intel Macs are not one.
      flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ] (system:
        let
          pkgs = import nixpkgs { inherit system; };
          hennery = import ./nix/package.nix { inherit pkgs crane advisory-db; };
        in {
          packages.default = hennery.package;
          checks = hennery.checks;
  ```

  The lock pins the two new nodes at the revisions the plan was tested with; `nix flake lock` would take today's `advisory-db`.

  Replace the whole of `flake.lock` with:

  ```json
  {
    "nodes": {
      "advisory-db": {
        "flake": false,
        "locked": {
          "lastModified": 1790886327,
          "narHash": "sha256-pf1LEfjD6m4Lfs44r1d2IKMCpwBM+KKfURzIMlixuXc=",
          "owner": "rustsec",
          "repo": "advisory-db",
          "rev": "6de4455103aced2cba86e3b86e5c090b22827cf1",
          "type": "github"
        },
        "original": {
          "owner": "rustsec",
          "repo": "advisory-db",
          "type": "github"
        }
      },
      "crane": {
        "locked": {
          "lastModified": 1787326676,
          "narHash": "sha256-lWhBbBvC05/xwivKBBiM2YNizpmgqCgyOIzomvRuwxs=",
          "owner": "ipetkov",
          "repo": "crane",
          "rev": "692f7e9ef2ece8125b466f66f2af532b3edaed0d",
          "type": "github"
        },
        "original": {
          "owner": "ipetkov",
          "ref": "v0.24.0",
          "repo": "crane",
          "type": "github"
        }
      },
      "flake-utils": {
        "inputs": {
          "systems": "systems"
        },
        "locked": {
          "lastModified": 1731533236,
          "narHash": "sha256-l0KFg5HjrsfsO/JpG+r7fRrqm12kzFHyUHqHCVpMMbI=",
          "owner": "numtide",
          "repo": "flake-utils",
          "rev": "11707dc2f618dd54ca8739b309ec4fc024de578b",
          "type": "github"
        },
        "original": {
          "owner": "numtide",
          "repo": "flake-utils",
          "type": "github"
        }
      },
      "nixpkgs": {
        "locked": {
          "lastModified": 1790463110,
          "narHash": "sha256-hKlVl12B1dF0Q5vd9dY3lIJM5mFGWYSlXwSLAqHZ1+s=",
          "owner": "NixOS",
          "repo": "nixpkgs",
          "rev": "e158d9ed9b51c98974c5e66e1ba1c9e0255fecaa",
          "type": "github"
        },
        "original": {
          "owner": "NixOS",
          "ref": "nixos-unstable",
          "repo": "nixpkgs",
          "type": "github"
        }
      },
      "root": {
        "inputs": {
          "advisory-db": "advisory-db",
          "crane": "crane",
          "flake-utils": "flake-utils",
          "nixpkgs": "nixpkgs"
        }
      },
      "systems": {
        "locked": {
          "lastModified": 1681028828,
          "narHash": "sha256-Vy1rq5AaRuLzOxct8nz4T6wlgyUR7zLU309k9mBC768=",
          "owner": "nix-systems",
          "repo": "default",
          "rev": "da67096a3b9bf56a91d16901293e51ba5b49a27e",
          "type": "github"
        },
        "original": {
          "owner": "nix-systems",
          "repo": "default",
          "type": "github"
        }
      }
    },
    "root": "root",
    "version": 7
  }
  ```

  Run: `git add nix/package.nix flake.nix flake.lock && nix flake metadata --json | python3 -c "import json,sys; print(sorted(json.load(sys.stdin)['locks']['nodes']))"`
  Expected: `['advisory-db', 'crane', 'flake-utils', 'nixpkgs', 'root', 'systems']`. `git diff --stat origin/main -- flake.lock` shows `34 +` and no deletion.

- [ ] **Step 3: Build and check**

  Run: `nix build .#default --no-link --print-out-paths -L && nix run . -- --version`
  Expected: a store path ending `-hennery-0.0.0`, then `hennery 0.0.0`. About 7 minutes from a cold store.

  Run: `nix flake check -L`
  Expected: exit 0. It builds `checks.<system>.{package,clippy,fmt,audit}` for this system and warns that it omitted the other two v1 systems. The audit prints `couldn't check if the package is yanked` for every crate: the sandbox has no network, and that part of `cargo audit` needs the crates.io index (decision 4). It reports no vulnerability.

- [ ] **Step 4: Revert-probes**

  Each applied alone, `git add`ed (the flake sees only the index), then undone:
  - `pub fn probe_unused() { let x = 1; }` added to `crates/hennery/src/healthcheck.rs`: `nix build .#checks.<system>.clippy` fails with an unused-variable error and "`-D unused-variables` implied by `-D warnings`".
  - A line of `healthcheck.rs` misformatted (an extra space): `nix build .#checks.<system>.fmt` fails with `Diff in …/healthcheck.rs`.
  - `adapters/manifest.json` created (`{"schema": 1}`) and `#[allow(dead_code)] pub const PROBE: &str = include_str!("../../../adapters/manifest.json");` added to `healthcheck.rs`: `nix build .#default` builds. Then, with the `(lib.fileset.maybeMissing ../adapters/manifest.json)` line removed from `nix/package.nix`, it fails with ``couldn't read `crates/hennery/src/../../../adapters/manifest.json` ``.
  - The audit, against a copy of the locked advisory database with one advisory added (security review A1). Copy the input's store path (`nix eval --raw --impure --expr '(builtins.getFlake (toString ./.)).inputs.advisory-db.outPath'`) to `/private/tmp/adb`, make it writable, and add `crates/anyhow/RUSTSEC-2026-9999.md`: a TOML block with `[advisory]` `id = "RUSTSEC-2026-9999"`, `package = "anyhow"`, `date`, `url`, empty `categories` and `keywords`, and `[versions]` `patched = []`, then a title line. `nix build .#checks.<system>.audit --no-link --override-input advisory-db path:/private/tmp/adb` loads 1280 advisories and fails with `ID: RUSTSEC-2026-9999` and `error: 1 vulnerability found!`; the locked build passes. (An advisory whose id is not a valid RustSec id, e.g. `RUSTSEC-0000-0000`, is skipped silently: 1279 loaded. Use a valid one.) `/tmp` is a symlink on macOS, which a `path:` input refuses: use `/private/tmp`.

- [ ] **Step 5: The checks, and commit**

  Run the five checks of "Global Constraints" (the dev shell's): all pass, the test count unchanged.

  ```bash
  git add nix/package.nix flake.nix flake.lock
  git diff --cached --stat
  git -c commit.gpgsign=false commit -m "build(nix): package hennery with crane, and check it with clippy, rustfmt and cargo-audit"
  ```

---

### Task 2: CI

**Files:**
- Create: `.github/workflows/nix.yml`

- [ ] **Step 1: The workflow**

  Create `.github/workflows/nix.yml`:

  ```yaml
  # The Nix flake (distribution spec §4.3, §9; plan 7e-i): `nix flake check`
  # builds the package and runs clippy, rustfmt and cargo-audit through crane,
  # on Linux and on macOS, and the package's binary runs. Nothing is pushed to
  # a binary cache: that would publish. The tests stay `ci.yml`'s.
  #
  # The Nix installer is pinned by commit. The job's own read-only token goes
  # to Nix only to fetch the locked GitHub inputs without hitting the
  # anonymous API rate limit that shared runners share.
  name: nix

  on:
    push:
      branches: [main]
    pull_request:
    workflow_dispatch:

  permissions:
    contents: read

  concurrency:
    group: nix-${{ github.ref }}
    # A pull request's superseded run only: every commit on `main` keeps its own.
    cancel-in-progress: ${{ github.event_name == 'pull_request' }}

  jobs:
    flake:
      strategy:
        fail-fast: false
        matrix:
          os: [ubuntu-24.04, macos-latest]
      runs-on: ${{ matrix.os }}
      # A cold build takes about 9 minutes; a hung one should not run for hours.
      timeout-minutes: 30
      steps:
        - uses: actions/checkout@v4
          with:
            persist-credentials: false
        - uses: cachix/install-nix-action@13d8dd58da0234aa297dedd986986ccb8e7f3e24 # v31.11.1
          with:
            github_access_token: ${{ github.token }}
        - name: Flake checks (package, clippy, rustfmt, cargo-audit)
          run: nix flake check -L
        - name: The package runs
          run: nix run . -- --version
  ```

  Run: `grep -nE "tags:|contents: write|id-token|attestations:|packages:|gh release|docker push|attest|cachix push|CACHIX" .github/workflows/*.yml`
  Expected: only `build.yml`'s line 7, its comment.

- [ ] **Step 2: Commit, push, and read the run**

  ```bash
  git add .github/workflows/nix.yml
  git diff --cached --stat
  git -c commit.gpgsign=false commit -m "ci(nix): run the flake checks and the package on Linux and macOS"
  git push origin feat/nix-package
  ```

  Both `flake` jobs pass. Their logs show `checking derivation checks.<system>.{package,clippy,fmt,audit}`, `running … flake checks`, and `hennery 0.0.0` from the package run.

---

## After this plan

- **7e-ii:** the adapter derivations from `adapters/manifest.json` (§4.3: `fetchurl` per tarball with the integrity as hash, unpacked, wrapped with nixpkgs' Node 24, `autoPatchelfHook` on Linux, the Claude adapter `unfree`); the frontend derivation (`fetchPnpmDeps`, `fetcherVersion = 4`) once plan 4 exists; the NixOS and home-manager modules `services.hennery.{collector,host}` with `adapters.source`, after 7c's services.
- **The advisory database is pinned:** bump it with `nix flake update advisory-db`, then check that only that node changed. A new advisory then fails the audit, as it should.
- **Yanked crates** are not checked (decision 4); `cargo audit` run with network, outside the sandbox, would.
- **A binary cache** for CI (`magic-nix-cache` or Cachix) would cut the cold build; Cachix pushes, so it is the operator's.
- **The toolchain overlay** (§4.3) is not used (decision 1); spec amendment.
- **`cargo audit` fails only on vulnerabilities;** unsound and unmaintained advisories only warn. `--deny unsound` would fail on those too: the maintainer's policy (the review's O5).
- **A new advisory shows only after a lock bump.** A scheduled job running a networked `cargo audit` against the live database, read-only and with no secret, would catch it, and yanked crates too (O6).
- **`aarch64-linux` is neither evaluated nor built** by CI. `nix flake check --all-systems` would evaluate it; building it needs an arm64 Linux runner.
- **Spec amendments:** §4.3: the toolchain is nixpkgs' (decision 1); the tests run in `ci.yml`, not as a flake check (decision 3); §9: the Nix CI job exists for the binary; the adapter derivation's check comes with 7e-ii.

---

_Generated with Claude AI — please review before distribution._
