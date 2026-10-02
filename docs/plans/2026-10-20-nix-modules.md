# The pinned adapters as Nix packages, and the NixOS and home-manager modules (plan 7e-ii-a) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** hennery's pinned adapters build with Nix, and NixOS and home-manager run hennery as services (distribution spec §4.3; 7e-i's hand-off):
- each adapter is a package made from the manifest the binary compiles in (`adapters/manifest.json`): the same tarballs as the managed runtime's, fetched at their `sha512` integrities, unpacked under the host installer's rules, wrapped with nixpkgs' Node 24. No npm runs, and no hash is kept by hand;
- the Claude adapter, whose CLI is Anthropic's and unfree, is never a package `nix flake check` evaluates. CI builds and runs it on Linux only and never caches or uploads it;
- a check runs every bundled native program and the adapter's ACP `initialize`, offline, and a set of fake adapters proves each of its verdicts;
- `services.hennery.{collector,host}` for NixOS (system units) and for home-manager (the user units `hennery service install` writes), with `adapters.source = "nix" | "managed"`, default `nix`;
- `hennery doctor` reads the `--agent` words of the NixOS module's system unit, so it judges a Nix host as a host given its agents;
- the NixOS module runs in a virtual machine in CI: a collector, a host paired to it, and doctor.

Nothing is published.

**Architecture:**
- `nix/adapters.nix` (new): one `stdenvNoCC.mkDerivation` per adapter. It `fetchurl`s each of the manifest's tarballs for the system (`hash = integrity`) and unpacks each at its lockfile path with `nix/unpack.sh` (new). On glibc CLIs (Claude's) on Linux, `autoPatchelfHook` runs; nothing is ever stripped. A `makeBinaryWrapper` runs Node on the adapter's entry.
- `nix/unpack-check.nix`, `nix/adapter-check.nix`, `nix/adapter-check-cases.nix` (new): the unpacker refuses what the installer refuses; an adapter runs; the run check gives each verdict it claims.
- `nix/modules/common.nix`, `nixos.nix`, `home-manager.nix` (new): the shared options and command lines, and each module's units.
- `nix/tests/modules.nix`, `nix/tests/nixos.nix` (new): the modules evaluated (Linux), home-manager's unit read back by the real `hennery doctor` (Linux), and the NixOS virtual machine (x86_64-linux, under KVM).
- `nix/outputs.nix` (new): one system's adapter packages and checks, so `flake.nix` names them in a few lines.
- `flake.nix`: `nixosModules.default`, `homeManagerModules.default`, `packages.codex-acp`, `legacyPackages.{claude-acp,claude-acp-runs}`, and the new checks.
- `crates/hennery`: `service::read_system_command_line` and `Doctor::agents_given_by_system_unit`; doctor's check 1 names the system unit.
- `.github/workflows/nix.yml`: KVM on ubuntu, the Claude adapter built and run on ubuntu, every system evaluated, a 60-minute timeout.

**Tech Stack:** Nix flakes on the locked `nixpkgs` (unchanged; `flake.lock` does not change): `fetchurl`, `autoPatchelfHook`, `makeBinaryWrapper`, `nodejs_24`, `testers.testBuildFailure`, `testers.runNixOSTest`; Rust (edition 2024) for the doctor change; GitHub Actions.

**Spec:** [`docs/specs/2026-09-26-distribution-design.md`](../specs/2026-09-26-distribution-design.md) §4.3:

> **Adapters:** the flake reads the same manifest (`builtins.fromJSON`), fetches each tarball for the system with `fetchurl { url; hash = integrity; }` and unpacks them, then wraps with nixpkgs' Node 24. On Linux the Claude CLI gets `autoPatchelfHook` (to be verified). The Claude adapter derivation is marked `unfree` so no public cache ever holds it. *(D-2: …)*
>
> NixOS/home-manager modules: `services.hennery.{collector,host}` with `adapters.source = "nix" | "managed"` (default `nix` on NixOS).

The earlier plans' obligations:
- 7e-i's "After this plan": the adapter derivations from the manifest (7b) and the NixOS and home-manager modules (7c's services).
- 7c: the service model the units keep (`HENNERY_SERVICE=systemd`, restart on failure under a start limit, `KillMode=mixed`, `--data-dir` on the command line, the PATH in `service.env`).
- 7d: doctor reads a service's `--agent` (`Doctor::given_agents`); a host given its agents installs no managed set, and check 1 says so.

Anchors are `main` at `ecc50cd` (PR #93), which includes #82 (frontend 4b: the `nixpkgs-web` input, `HENNERY_WEB_DIST`) and #86 (`--shell`: `SHELL` recorded in the plist and `service.env`).

**Status:** not executed; amended after two security reviews, both binding on the maintainer's behalf.
- **The first** (opus, 2026-10-02) approved with four amendments: A1, stricter unpacking; A2, the collector's hardening; A3, the pairing documented in `host.dataDir`; A4, doctor naming the system unit in check 1. All were taken, and its scoped re-confirmation (opus) said "confirmed".
- **The second** (opus, 2026-10-02) reviewed what changed since: the maintainer's answers Q1 to Q4, the `service.env` without `SHELL=`, the `library-missing` case, and the offline environment of `home-manager-doctor`. It approved with A1 and A2 (the spec's wording on the checks the system unit feeds, and on caches), O1 (check 3's PATH, recorded), O2 (a missing loader is fatal in a patched adapter), N1 (`offline()` parity) and N2 (a comment). All were taken, and its scoped re-confirmation (the same reviewer) confirmed each, and that O2 is safe for the Claude adapter while CI stays green.
- Decisions 1 to 13 stand as those reviews confirmed them.

The code was built on `scratch/7e2a-build` (one commit per task) and its CI ran on draft PR #90:
- run 37054209788, on `719c79e` (before the second review): both `flake` jobs passed, ubuntu in 10m20s and macOS in 12m37s;
- run 37062042499, on `32480c7` (this plan's code, with a scratch job of Linux revert-probes): both `flake` jobs passed, ubuntu in 11m48s and macOS in 18m20s; the Claude adapter's step printed `2.1.280 (Claude Code)`.

**Revert-probes:** the 36 marked "this Mac" were run here, each ending as written. The 12 marked "Linux, in CI" ran in #90's scratch job: each made its check fail. Four of their patterns were then corrected to the output observed (nix's "Cannot build" wording; the virtual machine's assertion text), and those four were run again in #90's next run. The plan was replayed from its own text onto `ecc50cd`, task by task (8, 6, 8, 6 and 2 blocks), and after each task the tree matched the build branch's commit byte for byte.

## Execution status

Not executed yet.

## Scope

7e-ii is split in two:
- **7e-ii-a, this plan:** the adapter derivations from the manifest, the NixOS and home-manager modules, and doctor's reading of the NixOS host unit;
- **7e-ii-b:** the web UI's derivation, embedded in the package (PR #104).

That is **5 tasks:** (1) the adapters and their checks; (2) doctor reads the system unit; (3) the modules and their checks; (4) CI; (5) the spec write-back.

**Cross-lane:**
- **7e-ii-b (PR #104)** changes `flake.nix`'s outputs too. A trial merge gives adjacent-line conflicts there only (its `webUi` binding against `nixOutputs`, `packages.web` against `packages.codex-acp`); both resolve by keeping both sides. Whichever lands second merges them by hand. Its deferred `nix.yml` minor (the header and step name do not list `web-ui`) goes to whichever lands second too.
- **#102 (default data directories)** and **#96 (doctor check 18)** touch `crates/hennery/src/doctor/mod.rs`. Trial merges of this plan's branch with #102 at `c9c1b25` and #96 at `1c91215`, separately and together, are clean, and doctor's tests pass on the merge of all three (39).
- **Frontend 4d** builds on `Doctor::given_agents` and a `cli: given` source for Nix adapters: see "After this plan".

## Maintainer decisions (2026-10-02, binding)

The operator answered this plan's four questions through the fleet parent:

| # | Question | Decision | Where |
|---|---|---|---|
| Q1 | Does Claude stay in the default agents, so a host does not evaluate until its licence is accepted? | **Yes.** The option's description says how to accept it: `nixpkgs.config.allowUnfree`, or `allowUnfreePredicate` for `hennery-claude-acp` alone | decision 9; Task 3 |
| Q2 | `NoNewPrivileges=` on the host unit? | **No.** The host unit is not hardened; the comment in `nixos.nix` records it as the operator's decision | decision 6; Task 3 |
| Q3 | May CI run Anthropic's CLI on GitHub's runners? | **Yes,** to build and check only: never cached, uploaded or published | decision 2; Task 4 |
| Q4 | Should a revoked host (exit 78) stop restarting? | **Yes,** in this plan's units: `RestartPreventExitStatus = 78`. 7c's own unit (`service/unit.rs`) and launchd agent are the parent's, in a separate PR: this plan does not touch `unit.rs` | decision 6; Task 3 |

## Decisions this plan makes where the spec is silent

1. **The adapters come from the binary's own manifest, unpacked as the host installs them** (§3.1, §3.2, D-2).
   - Each tarball is a fixed-output `fetchurl` whose hash is the manifest's `integrity`. No hash is kept by hand, and a manifest bump changes the packages with no edit under `nix/`.
   - `nix/unpack.sh` judges the listing before it writes anything, under the rules of `crates/hennery-host/src/runtime/extract.rs`: regular files and directories only; no absolute name, no `.`, `..` or empty component; no name twice; a destination that exists is refused. It then strips the first component (`package/`), with no owner and no permissions taken from the archive. After extracting, anything but files and directories fails, as a backstop: with the listing's rule removed, the backstop alone refuses a link (probed). The listings are read whole and judged from here-strings, never piped into `grep -q`, whose early exit under `pipefail` could turn a refusal into a pass. (The first security review's A1: stricter unpacking.)
   - No npm and no install script runs. The packages are unpacked in the manifest's order, a package before the ones nested in it.
   - nixpkgs' `nodejs_24` runs the entry, and evaluation fails if its major is not the manifest's Node major.
2. **The Claude adapter is unfree, and is no `packages` output** (§3.3; Q3).
   - Its `meta.license` is `unfree`. `nix flake check` evaluates every `packages` and `checks` output, so the adapter and its run check live in `legacyPackages` (`claude-acp`, `claude-acp-runs`), which `nix flake check` does not evaluate. `nix build .#claude-acp` builds it once the operator accepts the licence (`NIXPKGS_ALLOW_UNFREE=1 --impure`, or their own nixpkgs configuration through the modules).
   - Hydra never builds it. The tarball of its CLI is a store path of its own, with no licence attached: a cache that uploads every path built (`cachix watch-store`, attic) would publish both. The comment at the top of `adapters.nix` says so.
   - CI builds and runs it on ubuntu, with the licence accepted for that one command, and nothing in the job writes to a cache.
3. **Patching and stripping.**
   - `autoPatchelfHook` runs on Linux for an adapter whose CLI is built against glibc (Claude's), with `libstdc++`/`libgcc_s` as build inputs. Codex's CLI is musl-static and runs as it is.
   - Nothing is stripped (`dontStrip`): a bun-compiled CLI (Claude's) carries its program after the ELF, and stripping leaves the bare bun runtime. nixpkgs' `claude-code` does the same.
   - Codex's voice host and its libraries are glibc builds left unpatched (patching them failed on Linux CI). On NixOS its voice feature does not load; the run check names it and passes over it.
4. **What the run check judges** (`nix/adapter-check.nix`).
   - Every native program of the bundled CLI's packages (executable ELF or Mach-O files that are not libraries, outside nested `node_modules`) runs `--version`. `claude`, `codex` and `rg` must exit 0 and name themselves (`(Claude Code)`, `codex-cli`, `ripgrep`): a stripped bun CLI answers with bun's version. Any other helper must have started: not 124 (the timeout), 126 to 255 (the loader's failures and signals).
   - Exit 127 is passed over only when the program's ELF interpreter does not exist on this system (Codex's voice host), and only in an adapter that is not patched: in one `autoPatchelfHook` patches (Claude's on Linux, `passthru.patched`), a missing loader means the hook missed that program, and fails (the second review's O2). A 127 with the loader present, a library missing, fails.
   - The adapter must answer ACP `initialize` (`"result"` with `"id":0`) within 120 seconds, on a pipe held open until then.
   - No program found fails.
   - Each outcome has its own fake adapter in `adapter-check-cases.nix`, compiled in the build: passes (good; a helper whose loader is missing, Linux) and fails, each with its reason (a CLI that fails; a CLI that does not name itself; a helper that cannot start, Linux; a helper missing a library, Linux; a helper whose loader is missing in a patched adapter, Linux; no program; no answer).
   - On macOS the check runs unsandboxed (`__noChroot`): the macOS sandbox refuses the CLIs what they need at their first run, as for nixpkgs' `claude-code`. A Mac with `sandbox = true` cannot build it.
5. **One set of options and command lines for both modules** (`common.nix`).
   - Every word of `ExecStart=` is quoted as `service/unit.rs`'s `systemd_word` quotes it: double-quoted, `\` and `"` escaped, `%` and `$` doubled. Doctor's `systemd_command_line` reads only words written that way, so it reads the module's units as it reads `service install`'s.
   - `host run --data-dir <dir>`, then with `source = "nix"` one `--agent name=<wrapper>` per agent, then `--workspace-root` per root. With `source = "managed"`, no `--agent`: the host installs the pinned set at its start.
6. **NixOS: system units** (the security review's A2; Q2, Q4).
   - **The collector** runs as a system user of its own, `hennery` (kernel spec §10: a separate OS user is what keeps its credentials from agents). Its data directory is made by tmpfiles, `hennery`'s, mode 0700. The unit is hardened: no capability (so a port below 1024 needs a proxy in front), `NoNewPrivileges`, `PrivateUsers`, `ProtectSystem=strict` with its data directory writable, `ProtectHome`, the kernel and clock protections, `ProtectProc=invisible`, `RestrictAddressFamilies` to IP and Unix sockets, `MemoryDenyWriteExecute`, `SystemCallFilter=@system-service ~@privileged @resources`, `UMask=0077`.
   - **The host** runs as `host.user`, a normal user whose home, credentials and projects its agents use; an assertion refuses a user the system does not have. Its data directory is made by tmpfiles, that user's, 0700, outside the home (tmpfiles would make missing parents as root). It is not hardened: no `NoNewPrivileges` (Q2), so its agents can run setuid programs (`sudo`) as at that user's login.
   - Both keep 7c's model: `HENNERY_SERVICE=systemd` (each process writes its own rotating log, here in a `LogsDirectory=` of mode 0700), `Restart=on-failure` after 3 seconds under a start limit of 10 in 300 seconds, `KillMode=mixed`, `TimeoutStopSec=30`.
   - A revoked host exits 78 and is not restarted (`RestartPreventExitStatus = 78`, Q4). A host whose directory holds no `host.key` is skipped (`ConditionPathExists`), not crash-looped.
   - The host's PATH: `host.path`, the user's and the system's profiles, `/run/wrappers`, then `bash` and `git`.
   - `managed` on NixOS downloads glibc programs, so an assertion asks for `programs.nix-ld.enable`.
7. **Pairing is the operator's, as the host's user** (the security review's A3). The `host.dataDir` option says how: `sudo -u <user> hennery host join <url> --data-dir <dir>`, with `--no-runtime` when `source = "nix"`, then `systemctl start hennery-host`. A join run as root leaves files the service cannot read. `hennery admin` runs as the collector's user: `sudo -u hennery hennery admin --data-dir /var/lib/hennery …`.
8. **home-manager: the units `service install` writes, Linux only.**
   - `hennery-collector` and `hennery-host` in `~/.config/systemd/user`, where doctor's check 10 finds them, with `EnvironmentFile=` `~/.config/hennery/service.env`, in `unit::env_file`'s quoting, which check 5 reads.
   - **No `SHELL=` line** (new since #86). home-manager does not know the account's login shell; the user manager gives every unit the account's shell, which is what `service install` records without `--shell`. Doctor reads a file without the line as one written before the record, and runs the account's shell. A `host.shell` option was ruled out: it would record a shell the units are not given.
   - The PATH: `host.path`, the home-manager profile, the user's and the system's profiles, `/run/wrappers/bin`, then `/nix/var/nix/profiles/default/bin`, `/usr/bin`, `/bin` (as 7c's captured PATH ends). It is not the login shell's, so check 5 may say it has drifted; the module's comment says to change it with `host.path`.
   - An assertion refuses the module on macOS: there, `hennery service install` writes the launchd agent. Use the module or `service install`, not both: the module's files are read-only links into the store.
9. **The default agents are both** (Q1). With `claude` in `adapters.agents`, the host evaluates only once the operator's nixpkgs accepts its licence. The option's description gives `nixpkgs.config.allowUnfree = true`, or `nixpkgs.config.allowUnfreePredicate = pkg: lib.getName pkg == "hennery-claude-acp"`; `modules-eval` checks both, and the refusal without either. The adapter packages default to the operator's own nixpkgs (`pkgs.callPackage ../adapters.nix { }`), whose configuration alone accepts the licence.
10. **Doctor reads the NixOS host unit for its `--agent` words, and nothing else** (the security review's A4).
    - `service::read_system_command_line(cx, Role::Host)` reads `/etc/systemd/system/hennery-host.service` (under the context's root), on Linux only, with `unit::systemd_command_line`. Drop-ins (`hennery-host.service.d/`) are not read.
    - `Doctor::given` looks at the user services first, then that unit; the first whose `--data-dir` is the directory checked and that gives an agent wins. `given_agents` keeps its meaning (the agents given, by whichever); `agents_given_by_system_unit` says whether a system unit gave them.
    - Its `--agent` words count as a user service's do, for every check that reads them: check 1 names the unit ("the system unit /etc/systemd/system/hennery-host.service gives its agents with --agent", as check 10 finds no user service to explain it); check 2 does not ask for nix-ld; checks 3 and 4 start those agents; check 9 needs room for no next set; check 12 says no managed set is needed. Nothing else of the unit is read, and no other check judges it (the second review's A1).
    - Check 3 starts them with doctor's own PATH, not the unit's: with no user service there is no service PATH to read. Their commands are absolute store paths; tools they look up (`sh`, `git`) may differ from the unit's (the second review's O1, under "After this plan").
    - Only root writes `/etc/systemd/system`, the unit counts only for the directory its `--data-dir` names, and user services are tried first, so it gives doctor nothing to run that root did not choose.
11. **Where each check runs.**
    - Everywhere: `codex-acp-runs`, `unpack-refuses`, `adapter-check-cases` (its loader cases on Linux only).
    - Linux: `modules-eval` (its results are computed at evaluation, so it also evaluates on a Mac: `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`), and `home-manager-doctor`, which needs a Linux `hennery`. Gated on the system's name, never on `pkgs.stdenv` (a module's structure gated on `stdenv` recurses).
    - x86_64-linux: `nixos`, the virtual machine, which needs KVM; only the x86_64 runner has it.
    - The home-manager module is evaluated against stand-ins for home-manager's own options: the flake takes no home-manager input.
12. **Tests never touch the building machine's services or data** (fleet rule).
    - `home-manager-doctor` runs the binary as `offline()` in `crates/hennery/tests/cli.rs` does (PR #102): `HOME` and `XDG_{CONFIG,DATA,STATE,CACHE}_HOME` in the build's own directory; `HENNERY_DATA_DIR`, `HENNERY_HOST_DATA_DIR`, `HENNERY_MASTER_KEY`, `CREDENTIALS_DIRECTORY`, `HENNERY_DEV_TOKEN`, `HENNERY_SERVICE`, `HENNERY_LOG_DIR`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME` and `CODEX_SQLITE_HOME` unset; the npm registry and Node mirror a port nothing listens on (the second review's N1). The build sandbox starts from a clean environment and has no network on Linux either.
    - The virtual machine's service manager, users and data directories are its own.
    - The Rust tests write only under their temporary roots.
13. **CI** (`nix.yml`).
    - On ubuntu, a udev rule opens `/dev/kvm` to the runner's user, and Nix gets the `kvm` and `nixos-test` features.
    - `nix flake check -L --keep-going` builds every check; then the package runs; then, on ubuntu only and even after a failure, `NIXPKGS_ALLOW_UNFREE=1 nix build --impure .#claude-acp-runs`; then `nix flake check --all-systems --no-build` evaluates aarch64-linux, which no runner builds.
    - The timeout is 60 minutes (the virtual machine and the adapters add about 10 minutes on ubuntu).
    - `actions/checkout` is pinned by commit (`11d5960`, v4.4.0).

## Global Constraints

- **After every task** `nix develop -c cargo fmt --all --check` passes; after Task 2, both clippy runs, the workspace tests and the codegen check pass as well.
- **`flake.lock` does not change.** Check: `git diff --stat origin/main -- flake.lock` is empty.
- **Flakes ignore untracked files:** `git add` a new file before any `nix` command reads it.
- **Nothing publishes:** no cache push, no token, no new permission. The fixed-output fetches read the npm registry's public tarballs at the manifest's integrities; no test does.
- **The unfree adapter is never cached or uploaded,** and no check `nix flake check` runs needs its licence.
- **`crates/hennery/src/service/unit.rs` is not touched** (Q4: the parent's).
- **Probes and tools run in scratch homes:** every probe below ran with `HOME`, `XDG_*`, `npm_config_userconfig`, the npm and pnpm caches and stores, and `CARGO_HOME` in a scratch directory.

## Review Focus

1. **A malicious or broken tarball** behind a manifest entry. Expected: the integrity check refuses another tarball; a link, a special file, an escaping or repeated name, or an existing destination is refused before anything is written. Each rule revert-probed (Task 1).
2. **A nixpkgs bump** that changes Node's major, `autoPatchelfHook`, or the strip phase. Expected: evaluation fails on Node; the run check fails on a CLI left unpatched (127) or stripped (bun's version). Both probed on Linux (Task 1).
3. **An operator who enables the host with the defaults.** Expected: evaluation stops at the Claude licence, with nixpkgs' message, until they accept it or leave `claude` out (Task 3, `modules-eval`).
4. **A NixOS host that is not paired, or revoked.** Expected: skipped, not failed; once revoked, not restarted (Task 3, both `modules-eval` and the virtual machine).
5. **Doctor on a NixOS host.** Expected: check 1 names the system unit, check 2 does not ask for nix-ld, check 3 starts the Nix adapter (the virtual machine), and on macOS no system unit is read (Task 2's tests).
6. **The collector's unit.** Expected: its own user, no capability, `ProtectSystem=strict`, its directory 0700 (the virtual machine).

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `nix/unpack.sh` (new) | One npm package at its lockfile path, under the installer's rules | 1 |
| `nix/unpack-check.nix` (new) | The unpacker refuses each bad tarball | 1 |
| `nix/adapters.nix` (new) | The adapter packages, from the manifest | 1 |
| `nix/adapter-check.nix` (new) | An adapter's programs run, and it answers `initialize` | 1 |
| `nix/adapter-check-cases.nix` (new) | Each verdict of the run check | 1 |
| `nix/outputs.nix` (new) | One system's adapters and checks | 1, 3 |
| `flake.nix` | The packages, checks and modules | 1, 3 |
| `crates/hennery/src/service/mod.rs` | `read_system_command_line` | 2 |
| `crates/hennery/src/doctor/mod.rs`, `runtime.rs`, `tests.rs` | Doctor reads the system unit; check 1 names it | 2 |
| `nix/modules/common.nix`, `nixos.nix`, `home-manager.nix` (new) | The modules | 3 |
| `nix/tests/modules.nix`, `nix/tests/nixos.nix` (new) | The modules' checks | 3 |
| `.github/workflows/nix.yml` | KVM, the Claude adapter, every system | 4 |
| `docs/specs/2026-09-26-distribution-design.md` | §4.3 and §7 | 5 |

**Reading the steps:** "Create `path`:" makes a new file with the block; "Replace the whole of `path` with:" overwrites it; "In `path`, replace:" is followed by a block that occurs exactly once, then "with:" and its replacement.

All commands run from the repository root. `<system>` is this machine's Nix system, `aarch64-darwin` here. **The revert-probes** are listed per task: each changes one line as shown, runs the one check named, must end as shown with output matching the pattern, and is then undone. They were run by a script that applies them one at a time and restores the file after each (kept on branch `scratch/7e2a-tools`, `probes.py`); the Linux ones ran in the scratch CI of draft PR #90.

---

### Task 1: The adapters, and their checks

**Files:**
- Create: `nix/unpack.sh`, `nix/unpack-check.nix`, `nix/adapters.nix`, `nix/adapter-check.nix`, `nix/adapter-check-cases.nix`, `nix/outputs.nix`
- Modify: `flake.nix`

**Interfaces:**
- Produces: `nix/adapters.nix`, `{ claude, codex }`, each with `passthru.cliPaths` (the bundled CLI's packages) and `meta.mainProgram = "hennery-<name>-acp"`; `nix/adapter-check.nix`, a function from an adapter to its run check. Task 3's modules take the adapters from `adapters.nix`.

- [ ] **Step 1: The unpacker and its check**

  Create `nix/unpack.sh`:

  ```sh
  # shellcheck shell=bash
  # unpack <package.tgz> <dest>: one npm package at its lockfile path, its
  # first component (`package/`) stripped, under the host installer's rules
  # (distribution spec §3.2; crates/hennery-host/src/runtime/extract.rs).
  # The listing is judged before anything is written: only regular files
  # and directories; no absolute name, no `.` or `..` component, no empty
  # one; no name twice. A destination that exists is refused. After
  # extraction, anything but files and directories fails as a backstop.
  unpack() {
    local tgz=$1 dest=$2 listing names
    refuse() {
      echo "hennery: $tgz: $1" >&2
      return 1
    }
    if [ -e "$dest" ]; then
      refuse "$dest exists"
      return 1
    fi
    # Listed whole first, and judged from here-strings: a `grep -q` reading a
    # pipe can stop its writer early, and under `pipefail` the writer's
    # SIGPIPE would turn a refusal into a pass.
    listing=$(tar -tvzPf "$tgz") || { refuse "cannot be listed"; return 1; }
    names=$(tar -tzPf "$tgz") || { refuse "cannot be listed"; return 1; }
    if grep -qv '^[-d]' <<< "$listing"; then
      refuse "an entry is neither a file nor a directory (a link, or a special file)"
      return 1
    fi
    if grep -qE '^/|(^|/)\.{1,2}(/|$)|//' <<< "$names"; then
      refuse "a name is absolute, or has a '.', '..' or empty component"
      return 1
    fi
    if [ -n "$(sed 's|/$||' <<< "$names" | sort | uniq -d)" ]; then
      refuse "a name appears twice"
      return 1
    fi
    mkdir -p "$dest"
    tar -xzf "$tgz" -C "$dest" --strip-components=1 --no-same-owner --no-same-permissions
    if [ -n "$(find "$dest" ! -type f ! -type d)" ]; then
      refuse "extracted an entry that is neither a file nor a directory"
      return 1
    fi
  }
  ```

  Create `nix/unpack-check.nix`:

  ```nix
  # `unpack.sh` refuses what the host's installer refuses (plan 7e-ii-a;
  # crates/hennery-host/src/runtime/extract.rs): each bad tarball below fails
  # with its own reason, and a good one lands without its first component.
  {
    runCommand,
    python3,
  }:
  runCommand "hennery-unpack-refuses" { nativeBuildInputs = [ python3 ]; } ''
    source ${./unpack.sh}
    python3 - <<'EOF'
    import io, tarfile

    def make(name, entries):
        with tarfile.open(f"{name}.tgz", "w:gz") as tar:
            for path, kind, extra in entries:
                info = tarfile.TarInfo(path)
                info.type = kind
                data = b""
                if kind == tarfile.REGTYPE:
                    data = extra or b"x"
                    info.size = len(data)
                elif kind in (tarfile.SYMTYPE, tarfile.LNKTYPE):
                    info.linkname = extra
                tar.addfile(info, io.BytesIO(data) if kind == tarfile.REGTYPE else None)

    R, D = tarfile.REGTYPE, tarfile.DIRTYPE
    make("good", [("package/", D, None), ("package/package.json", R, b"{}"), ("package/lib/a.js", R, None)])
    make("symlink", [("package/a", R, None), ("package/l", tarfile.SYMTYPE, "/etc/passwd")])
    make("hardlink", [("package/a", R, None), ("package/h", tarfile.LNKTYPE, "package/a")])
    make("fifo", [("package/f", tarfile.FIFOTYPE, None)])
    make("twice", [("package/a", R, b"x"), ("package/a", R, b"y")])
    make("dotdot", [("package/../escape", R, None)])
    make("absolute", [("/package/escape", R, None)])
    make("dot", [("package/./a", R, None)])
    make("empty-component", [("package//a", R, None)])
    EOF
    refused() {
      if (unpack "$1.tgz" "$TMPDIR/out-$1") 2> "$1.err"; then
        echo "FAIL: $1 was unpacked" >&2
        exit 1
      fi
      grep -q "$2" "$1.err" || { echo "FAIL: $1: $(cat "$1.err")" >&2; exit 1; }
      echo "ok: $1 refused: $(cat "$1.err")"
    }
    refused symlink "neither a file nor a directory (a link, or a special file)"
    refused hardlink "neither a file nor a directory (a link, or a special file)"
    refused fifo "neither a file nor a directory (a link, or a special file)"
    refused twice "appears twice"
    refused dotdot "has a '.', '..'"
    refused absolute "is absolute"
    refused dot "has a '.', '..'"
    refused empty-component "empty component"

    unpack good.tgz "$TMPDIR/good/node_modules/p"
    [ "$(cat "$TMPDIR/good/node_modules/p/package.json")" = "{}" ]
    [ -f "$TMPDIR/good/node_modules/p/lib/a.js" ]
    [ ! -e "$TMPDIR/good/node_modules/p/package" ]
    echo "ok: good unpacked without its first component"
    refused_again() {
      if (unpack good.tgz "$TMPDIR/good/node_modules/p") 2> again.err; then
        echo "FAIL: an existing destination was unpacked into" >&2
        exit 1
      fi
      grep -q "exists" again.err
      echo "ok: an existing destination refused"
    }
    refused_again
    touch "$out"
  ''
  ```

- [ ] **Step 2: The adapters**

  Create `nix/adapters.nix`:

  ```nix
  # The pinned adapters as Nix packages (plan 7e-ii-a, distribution spec
  # §4.3), read from the manifest the binary compiles in
  # (`adapters/manifest.json`, §3.1): the same tarballs as the managed
  # runtime's, each a fixed-output `fetchurl` whose hash is the manifest's own
  # `sha512` integrity, so no hash is kept by hand (D-2). Each tarball is
  # unpacked at its lockfile path with its first component (`package/`)
  # stripped, as the host's installer does, so Node finds the bundled CLI the
  # same way. No npm, and no install script, runs. Node is nixpkgs' Node 24,
  # the major the manifest pins.
  #
  # The Claude adapter's CLI is Anthropic's, "All rights reserved" (§3.3): its
  # package is `unfree`, so evaluating it needs the operator's own consent in
  # their nixpkgs configuration, and Hydra never builds it. The tarball
  # fetched for its CLI is a store path of its own, with no licence: a cache
  # that uploads every path built (`cachix watch-store`, attic) would publish
  # both, so keep such a cache off a machine that builds this adapter.
  {
    lib,
    stdenv,
    stdenvNoCC,
    fetchurl,
    nodejs_24,
    makeBinaryWrapper,
    autoPatchelfHook,
  }:
  let
    manifest = lib.importJSON ../adapters/manifest.json;
    # The manifest's platform names (Node's), for the v1 systems (§1).
    platforms = {
      x86_64-linux = "linux-x64";
      aarch64-linux = "linux-arm64";
      aarch64-darwin = "darwin-arm64";
    };
    inherit (stdenvNoCC.hostPlatform) system;
    platform = platforms.${system} or (throw "hennery: the adapter manifest pins nothing for ${system}");
    nodeMajor = lib.versions.major manifest.node.version;

    adapter =
      name:
      {
        license,
        description,
        glibc,
      }:
      let
        # A CLI built against glibc needs NixOS' loader and libraries put in;
        # one that is musl-static (Codex's) runs as it is. Codex's voice host
        # and its libraries are glibc builds left as they are (patching them
        # failed on Linux CI), so on NixOS its voice feature does not load.
        patchElf = glibc && stdenv.hostPlatform.isElf;
        pinned = manifest.adapters.${name};
        files = pinned.platforms.${platform};
        root = "lib/hennery/${name}";
        unpack = file: ''
          unpack ${
            fetchurl {
              inherit (file) url;
              hash = file.integrity;
            }
          } "$out/${root}/${file.path}"
        '';
      in
      assert lib.assertMsg (lib.versions.major nodejs_24.version == nodeMajor)
        "hennery: nixpkgs' nodejs_24 is ${nodejs_24.version}; the adapter manifest pins Node ${manifest.node.version}";
      stdenvNoCC.mkDerivation {
        pname = "hennery-${name}-acp";
        inherit (pinned) version;
        dontUnpack = true;
        dontConfigure = true;
        dontBuild = true;
        # A bun-compiled CLI (Claude's) carries its program after the ELF:
        # stripping it leaves the bare bun runtime (nixpkgs' `claude-code`).
        dontStrip = true;
        strictDeps = true;
        nativeBuildInputs = [ makeBinaryWrapper ] ++ lib.optionals patchElf [ autoPatchelfHook ];
        # What the bundled native programs link besides libc.
        buildInputs = lib.optionals patchElf [ (lib.getLib stdenv.cc.cc) ];
        installPhase = ''
          runHook preInstall
          # Each package at its lockfile path, under the host installer's
          # rules (`unpack.sh`), in the manifest's order (a package before
          # the ones nested in it).
          source ${./unpack.sh}
          ${lib.concatMapStrings unpack files}
          makeBinaryWrapper ${lib.getExe nodejs_24} "$out/bin/hennery-${name}-acp" \
            --add-flags "$out/${root}/${pinned.entry}"
          runHook postInstall
        '';
        passthru = {
          # What a check runs: the bundled CLI's packages, at their paths.
          cliPaths = map (file: "${root}/${file.path}") (lib.filter (file: file.cli or false) files);
          inherit (pinned) entry;
          # Whether `autoPatchelfHook` ran: then no program may be left with
          # a loader this system lacks.
          patched = patchElf;
        };
        meta = {
          inherit license description;
          mainProgram = "hennery-${name}-acp";
          platforms = lib.attrNames platforms;
          sourceProvenance = with lib.sourceTypes; [
            fromSource
            binaryNativeCode
          ];
        };
      };
  in
  {
    claude = adapter "claude" {
      license = lib.licenses.unfree;
      description = "The Claude ACP adapter hennery pins, with its bundled Claude CLI";
      glibc = true;
    };
    codex = adapter "codex" {
      license = lib.licenses.asl20;
      description = "The Codex ACP adapter hennery pins, with its bundled Codex CLI";
      glibc = false;
    };
  }
  ```

- [ ] **Step 3: The run check, and its cases**

  Create `nix/adapter-check.nix`:

  ```nix
  # An adapter package works (plan 7e-ii-a): every native program of its
  # bundled CLI runs (`--version`), which on Linux proves `autoPatchelfHook`
  # left each one working, and the adapter answers ACP `initialize` as the
  # host starts it. Nothing here needs the network.
  {
    lib,
    runCommand,
    coreutils,
    findutils,
    gnugrep,
    patchelf,
  }:
  adapter:
  runCommand "${adapter.pname}-runs"
    {
      nativeBuildInputs = [
        coreutils
        findutils
        gnugrep
      ]
      ++ lib.optionals adapter.stdenv.hostPlatform.isElf [ patchelf ];
      # The macOS sandbox refuses the CLIs what they need at their first run
      # (nixpkgs' `claude-code` builds its check the same way). So on a Mac
      # with `sandbox = true` (not `relaxed`) this check cannot be built, and
      # elsewhere on macOS the CLIs run with the network.
      __noChroot = adapter.stdenv.hostPlatform.isDarwin;
    }
    ''
      export HOME="$TMPDIR/home"
      mkdir -p "$HOME"
      programs=0
      for dir in ${lib.concatMapStringsSep " " (path: "${adapter}/${path}") adapter.cliPaths}; do
        # The native programs, as the host's installer finds them: executable
        # files that are neither scripts nor libraries (`.so.<n>` included),
        # outside nested `node_modules`.
        while IFS= read -r program; do
          # Native code only: ELF, or Mach-O (thin or universal).
          case "$(head -c 4 "$program" | od -An -tx1 | tr -d ' \n')" in
            7f454c46 | cffaedfe | cafebabe) ;;
            *) continue ;;
          esac
          echo "running $program --version"
          status=0
          said=$(timeout 120 "$program" --version < /dev/null 2>&1) || status=$?
          echo "$said"
          # Each CLI must name itself: a bun-compiled one that lost its
          # program (stripped) answers with bun's version instead.
          case "$(basename "$program")" in
            claude) name="(Claude Code)" ;;
            codex) name="codex-cli" ;;
            rg) name="ripgrep" ;;
            *) name="" ;;
          esac
          if [ "$status" -eq 0 ] && [ -n "$name" ] && ! grep -qF "$name" <<< "$said"; then
            echo "$program --version does not say $name" >&2
            exit 1
          fi
          # The CLIs themselves must answer; a helper that takes no
          # `--version` must still have started (not 126 or 127, the loader's
          # failures, nor a signal or the timeout's 124). A helper whose
          # loader this system does not have (Codex's voice host, left
          # unpatched: decision 3) is named and passed over, unless the
          # adapter was patched: then `autoPatchelfHook` missed it.
          case "$(basename "$program"):$status" in
            claude:0 | codex:0 | rg:0) ;;
            claude:* | codex:* | rg:*) echo "$program failed: $status" >&2; exit 1 ;;
            *:127)
              interpreter=$(patchelf --print-interpreter "$program" 2> /dev/null || true)
              if [ -n "$interpreter" ] && [ ! -e "$interpreter" ]; then
                if ${lib.boolToString (adapter.patched or false)}; then
                  echo "$program needs the loader $interpreter, which this system does not have: autoPatchelfHook left it unpatched" >&2
                  exit 1
                fi
                echo "passed over: $program needs the loader $interpreter, which this system does not have"
                continue
              fi
              echo "$program did not run: $status (loader: ''${interpreter:-none})" >&2
              exit 1
              ;;
            *:124 | *:12[6-9] | *:1[3-9]? | *:2??) echo "$program did not run: $status" >&2; exit 1 ;;
          esac
          programs=$((programs + 1))
        done < <(find "$dir" -name node_modules -prune -o -type f -perm -u+x \
          ! -name '*.node' ! -name '*.so' ! -name '*.so.*' ! -name '*.dylib' -print | sort)
      done
      if [ "$programs" -eq 0 ]; then
        echo "no native program found in ${adapter.pname}'s CLI" >&2
        exit 1
      fi
      # `initialize` on standard input, a pipe held open until the answer is
      # read; closing it then ends the adapter, as it ends at its host's end.
      request='{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}'
      mkfifo "$TMPDIR/in"
      timeout 120 ${lib.getExe adapter} < "$TMPDIR/in" > "$TMPDIR/answer" &
      adapter=$!
      exec 3> "$TMPDIR/in"
      echo "$request" >&3
      for _ in $(seq 1 120); do
        if grep -q '"result"' "$TMPDIR/answer"; then
          break
        fi
        sleep 1
      done
      exec 3>&-
      wait "$adapter" || true
      head -c 2000 "$TMPDIR/answer"
      echo
      if ! grep '"result"' "$TMPDIR/answer" | grep -q '"id":0'; then
        echo "${adapter.pname} did not answer initialize" >&2
        exit 1
      fi
      touch "$out"
    ''
  ```

  Create `nix/adapter-check-cases.nix`:

  ```nix
  # `adapter-check.nix` judges each outcome as it says (plan 7e-ii-a): fake
  # adapters, made of native programs compiled here and a shell script as
  # the adapter, each built through the check.
  # - Passes: a good one; one whose helper's loader this system lacks
  #   (Linux), passed over by name.
  # - Fails, each with its reason (`testers.testBuildFailure`): a CLI that
  #   fails; a CLI whose `--version` does not name it; a helper that cannot
  #   start (Linux); a helper whose loader is there but a library it needs is
  #   not, the loader's own 127 (Linux: what a bad `autoPatchelfHook` leaves);
  #   a helper whose loader is missing in an adapter that was patched
  #   (Linux); no native program; no answer to `initialize`.
  {
    lib,
    stdenv,
    runCommand,
    testers,
    coreutils,
    patchelf,
    writeShellScript,
    adapterCheck,
  }:
  let
    linux = stdenv.hostPlatform.isLinux;
    answers = writeShellScript "answers" ''
      read -r _
      echo '{"jsonrpc":"2.0","id":0,"result":{"protocolVersion":1}}'
      cat > /dev/null
    '';
    silent = writeShellScript "silent" ''
      read -r _
    '';
    # A fake adapter: `programs` (name = how to make it) in its CLI's package,
    # and `adapter` as its wrapper.
    fake =
      name:
      {
        programs,
        adapter ? answers,
        patched ? false,
      }:
      stdenv.mkDerivation {
        pname = "fake-${name}";
        version = "0";
        dontUnpack = true;
        dontFixup = true;
        nativeBuildInputs = lib.optionals linux [ patchelf ];
        installPhase = ''
          cli="$out/lib/cli"
          mkdir -p "$cli" "$out/bin"
          ${lib.concatStrings (lib.mapAttrsToList (program: make: "${make "$cli/${program}"}\n") programs)}
          cp ${adapter} "$out/bin/fake-acp"
        '';
        passthru = {
          cliPaths = [ "lib/cli" ];
          inherit patched;
        };
        meta.mainProgram = "fake-acp";
      };
    # A native program that prints `text` and exits with `code`, whatever its
    # arguments.
    says =
      text: code: target:
      "printf '%s\\n' '#include <stdio.h>' 'int main(void) { puts(\"${text}\"); return ${toString code}; }'"
      + " | $CC -x c -o ${target} -";
    claude = says "2.1.0 (Claude Code)" 0;
    # An ELF whose loader is somewhere else: missing, or not a loader at all.
    loader = path: target: "${says "" 0 target}; patchelf --set-interpreter ${path} ${target}";
    # An ELF that needs a library nothing provides.
    needs = library: target: "${says "" 0 target}; patchelf --add-needed ${library} ${target}";
    passes = name: drv: (adapterCheck drv).overrideAttrs { name = "adapter-check-passes-${name}"; };
    fails =
      name: drv: reason:
      runCommand "adapter-check-fails-${name}" { failed = testers.testBuildFailure (adapterCheck drv); } ''
        grep -qF ${lib.escapeShellArg reason} "$failed/testBuildFailure.log" || {
          echo "${name}: expected ${lib.escapeShellArg reason}, got:"; cat "$failed/testBuildFailure.log"; exit 1;
        }
        touch "$out"
      '';
    cases = {
      good = passes "good" (fake "good" { programs.claude = claude; });
      cli-fails = fails "cli-fails" (fake "cli-fails" { programs.codex = says "codex-cli 1.0" 1; }) "failed: 1";
      cli-unnamed = fails "cli-unnamed" (fake "cli-unnamed" {
        programs.claude = says "1.3.0" 0;
      }) "does not say (Claude Code)";
      no-program = fails "no-program" (fake "no-program" { programs = { }; }) "no native program found";
      no-answer = fails "no-answer" (fake "no-answer" {
        programs.rg = says "ripgrep 15.2.0" 0;
        adapter = silent;
      }) "did not answer initialize";
    }
    // lib.optionalAttrs linux {
      loader-missing = passes "loader-missing" (fake "loader-missing" {
        programs = {
          inherit claude;
          helper = loader "/nonexistent/ld-musl-x86_64.so.1";
        };
      });
      helper-cannot-start = fails "helper-cannot-start" (fake "helper-cannot-start" {
        programs = {
          inherit claude;
          helper = loader "${coreutils}/bin/true";
        };
      }) "did not run";
      library-missing = fails "library-missing" (fake "library-missing" {
        programs = {
          inherit claude;
          helper = needs "libhennery-missing.so.1";
        };
      }) "did not run: 127 (loader: /";
      loader-missing-patched = fails "loader-missing-patched" (fake "loader-missing-patched" {
        programs = {
          inherit claude;
          helper = loader "/nonexistent/ld-musl-x86_64.so.1";
        };
        patched = true;
      }) "autoPatchelfHook left it unpatched";
    };
  in
  runCommand "hennery-adapter-check-cases" { passthru = cases; } ''
    ${lib.concatMapStrings (c: "echo ok: ${c} ${cases.${c}}\n") (lib.attrNames cases)}
    touch "$out"
  ''
  ```

- [ ] **Step 4: The flake's outputs**

  Create `nix/outputs.nix`:

  ```nix
  # The flake's outputs for one system from plan 7e-ii-a: the pinned adapters
  # (`nix/adapters.nix`) and their checks. Kept apart from `flake.nix` so the
  # flake names them in three lines.
  {
    pkgs,
    nixpkgs,
  }:
  let
    inherit (nixpkgs) lib;
    adapters = pkgs.callPackage ./adapters.nix { };
    adapterCheck = pkgs.callPackage ./adapter-check.nix { };
  in
  {
    codex-acp = adapters.codex;
    # Not evaluated by `nix flake check`: the Claude adapter needs the
    # operator's consent to its licence.
    legacyPackages = {
      claude-acp = adapters.claude;
      claude-acp-runs = adapterCheck adapters.claude;
    };
    checks = {
      codex-acp-runs = adapterCheck adapters.codex;
      unpack-refuses = pkgs.callPackage ./unpack-check.nix { };
      adapter-check-cases = pkgs.callPackage ./adapter-check-cases.nix { inherit adapterCheck; };
    };
  }
  ```

  In `flake.nix`, replace:

  ```nix
          webTools = [ web.nodejs_24 web.pnpm ];
        in {
  ```

  with:

  ```nix
          webTools = [ web.nodejs_24 web.pnpm ];
          # The pinned adapters and their checks (plan 7e-ii-a).
          nixOutputs = import ./nix/outputs.nix { inherit pkgs nixpkgs; };
        in {
  ```

  In `flake.nix`, replace:

  ```nix
          packages.default = hennery.package;
          checks = hennery.checks;
          # Building the web UI only (CI's Rust and release jobs): no browsers.
  ```

  with:

  ```nix
          packages.default = hennery.package;
          # The free adapter. The Claude adapter is unfree (distribution spec
          # §3.3), so it is no package of the flake's, which `nix flake check`
          # would evaluate: `nix build .#claude-acp` builds it once the
          # operator accepts its licence (`NIXPKGS_ALLOW_UNFREE=1 --impure`).
          packages.codex-acp = nixOutputs.codex-acp;
          legacyPackages = nixOutputs.legacyPackages;
          checks = hennery.checks // nixOutputs.checks;
          # Building the web UI only (CI's Rust and release jobs): no browsers.
  ```

- [ ] **Step 5: Build the checks**

  Run:

  ```sh
  git add nix flake.nix
  nix build --no-link -L .#checks.<system>.unpack-refuses .#checks.<system>.adapter-check-cases .#checks.<system>.codex-acp-runs
  nix flake check --all-systems --no-build
  ```

  Expected: both exit 0. `unpack-refuses` prints `ok: <name> refused: …` for each of the eight bad tarballs, then `ok: good unpacked without its first component` and `ok: an existing destination refused`. `codex-acp-runs` prints each native program it runs and the adapter's answer, `{"jsonrpc":"2.0","id":0,"result":{…"agentInfo":{"name":"@agentclientprotocol/codex-acp"…`. On Linux (CI), `adapter-check-cases` builds its four Linux cases too, and `NIXPKGS_ALLOW_UNFREE=1 nix build --impure --no-link -L .#claude-acp-runs` prints `2.1.280 (Claude Code)` and the adapter's answer.

- [ ] **Step 6: The revert-probes**

  - **unpack-links** (this Mac), in `nix/unpack.sh`: `if grep -qv '^[-d]' <<< "$listing"; then` → `if false; then`. Run `nix build --no-link -L .#checks.aarch64-darwin.unpack-refuses`: it must fail, its output matching `symlink\.tgz:\ extracted\ an\ entry\ that\ is\ neither\ a\ file\ nor\ a\ directory`.
  - **unpack-names** (this Mac), in `nix/unpack.sh`: `if grep -qE '^/|(^|/)\.{1,2}(/|$)|//' <<< "$names"; then` → `if false; then`. Run `nix build --no-link -L .#checks.aarch64-darwin.unpack-refuses`: it must fail, its output matching `FAIL:\ dotdot\ was\ unpacked`.
  - **unpack-twice** (this Mac), in `nix/unpack.sh`: `if [ -n "$(sed 's|/$||' <<< "$names" | sort | uniq -d)" ]; then` → `if false; then`. Run `nix build --no-link -L .#checks.aarch64-darwin.unpack-refuses`: it must fail, its output matching `FAIL:\ twice\ was\ unpacked`.
  - **unpack-exists** (this Mac), in `nix/unpack.sh`: `if [ -e "$dest" ]; then` → `if false; then`. Run `nix build --no-link -L .#checks.aarch64-darwin.unpack-refuses`: it must fail, its output matching `an\ existing\ destination\ was\ unpacked\ into`.
  - **unpack-strip** (this Mac), in `nix/unpack.sh`: `--strip-components=1` → `(nothing)`. Run `nix build --no-link -L .#checks.aarch64-darwin.unpack-refuses`: it must fail, its output matching `hennery:\ good\.tgz:\`.
  - **unpack-backstop** (this Mac), in `nix/unpack.sh`: `if grep -qv '^[-d]' <<< "$listing"; then` → `if false; then`; and `if [ -n "$(find "$dest" ! -type f ! -type d)" ]; then` → `if false; then`. Run `nix build --no-link -L .#checks.aarch64-darwin.unpack-refuses`: it must fail, its output matching `FAIL:\ symlink\ was\ unpacked`.
  - **check-cli-status** (this Mac), in `nix/adapter-check.nix`: `claude:* | codex:* | rg:*) echo "$program failed: $status" >&2; exit 1 ;;` → `claude:* | codex:* | rg:*) ;;`. Run `nix build --no-link -L .#checks.aarch64-darwin.adapter-check-cases.cli-fails`: it must fail, its output matching `testBuildFailure:\ The\ builder\ did\ not\ fail,\ but\ a\ failure\ was\ expected!`.
  - **check-cli-name** (this Mac), in `nix/adapter-check.nix`: `if [ "$status" -eq 0 ] && [ -n "$name" ] && ! grep -qF "$name" <<< "$said"; then` → `if false; then`. Run `nix build --no-link -L .#checks.aarch64-darwin.adapter-check-cases.cli-unnamed`: it must fail, its output matching `testBuildFailure:\ The\ builder\ did\ not\ fail,\ but\ a\ failure\ was\ expected!`.
  - **check-no-program** (this Mac), in `nix/adapter-check.nix`: `if [ "$programs" -eq 0 ]; then` → `if false; then`. Run `nix build --no-link -L .#checks.aarch64-darwin.adapter-check-cases.no-program`: it must fail, its output matching `testBuildFailure:\ The\ builder\ did\ not\ fail,\ but\ a\ failure\ was\ expected!`.
  - **check-initialize** (this Mac), in `nix/adapter-check.nix`: `if ! grep '"result"' "$TMPDIR/answer" | grep -q '"id":0'; then` → `if false; then`. Run `nix build --no-link -L .#checks.aarch64-darwin.adapter-check-cases.no-answer`: it must fail, its output matching `testBuildFailure:\ The\ builder\ did\ not\ fail,\ but\ a\ failure\ was\ expected!`.
  - **check-loader-missing** (Linux, in CI), in `nix/adapter-check.nix`: `echo "passed over: $program needs the loader $interpreter, which this system does not have" ⏎               continue` → `exit 1`. Run `nix build --no-link -L .#checks.x86_64-linux.adapter-check-cases.loader-missing`: it must fail, its output matching `adapter-check-passes-loader-missing`.
  - **check-cannot-start** (Linux, in CI), in `nix/adapter-check.nix`: `*:124 | *:12[6-9] | *:1[3-9]? | *:2??) echo "$program did not run: $status" >&2; exit 1 ;;` → `*:124) exit 1 ;;`. Run `nix build --no-link -L .#checks.x86_64-linux.adapter-check-cases.helper-cannot-start`: it must fail, its output matching `testBuildFailure:\ The\ builder\ did\ not\ fail,\ but\ a\ failure\ was\ expected!`.
  - **check-patched-loader** (Linux, in CI), in `nix/adapter-check.nix`: `if ${lib.boolToString (adapter.patched or false)}; then ⏎                 echo "$program needs the loader $interpreter, which this system does not have: autoPatchelfHook left it unpatched" >&2 ⏎                 exit 1 ⏎               fi ⏎` → `(nothing)`. Run `nix build --no-link -L .#checks.x86_64-linux.adapter-check-cases.loader-missing-patched`: it must fail, its output matching `testBuildFailure:\ The\ builder\ did\ not\ fail,\ but\ a\ failure\ was\ expected!`.
  - **check-library-missing** (Linux, in CI), in `nix/adapter-check.nix`: `echo "$program did not run: $status (loader: ''${interpreter:-none})" >&2 ⏎             exit 1` → `continue`. Run `nix build --no-link -L .#checks.x86_64-linux.adapter-check-cases.library-missing`: it must fail, its output matching `testBuildFailure:\ The\ builder\ did\ not\ fail,\ but\ a\ failure\ was\ expected!`.
  - **codex-entry** (this Mac), in `nix/adapters.nix`: `--add-flags "$out/${root}/${pinned.entry}"` → `--add-flags "$out/${root}/nothing.js"`. Run `nix build --no-link -L .#checks.aarch64-darwin.codex-acp-runs`: it must fail, its output matching `did not answer initialize`.
  - **claude-strip** (Linux, in CI), in `nix/adapters.nix`: `dontStrip = true;` → `dontStrip = false;`. Run `NIXPKGS_ALLOW_UNFREE=1 nix build --impure --no-link -L .#legacyPackages.x86_64-linux.claude-acp-runs`: it must fail, its output matching `does not say \(Claude Code\)|claude failed`.
  - **claude-patchelf** (Linux, in CI), in `nix/adapters.nix`: `patchElf = glibc && stdenv.hostPlatform.isElf;` → `patchElf = false;`. Run `NIXPKGS_ALLOW_UNFREE=1 nix build --impure --no-link -L .#legacyPackages.x86_64-linux.claude-acp-runs`: it must fail, its output matching `claude failed: 127`.

- [ ] **Step 7: Commit**

  ```sh
  git add nix flake.nix
  git diff --cached --stat
  git commit -m "build(nix): the pinned adapters as packages from the manifest, and their checks"
  ```

---

### Task 2: Doctor reads the agents the NixOS host unit gives

**Files:**
- Modify: `crates/hennery/src/service/mod.rs`, `crates/hennery/src/doctor/mod.rs`, `crates/hennery/src/doctor/runtime.rs`, `crates/hennery/src/doctor/tests.rs`

**Interfaces:**
- Produces: `pub(crate) fn service::read_system_command_line(cx: &Context, role: Role) -> Option<Vec<String>>`; `pub fn Doctor::agents_given_by_system_unit(&self) -> bool`. `Doctor::given_agents` and `Doctor::agents_given` keep their signatures and now see the system unit too.
- Consumed by: Task 3's virtual machine (check 1's wording).

- [ ] **Step 1: The tests**

  In `crates/hennery/src/doctor/tests.rs`, replace:

  ```rust
      assert!(check.summary.contains("a rollback holds this host"), "{check:?}");
  }
  ```

  with:

  ```rust
      assert!(check.summary.contains("a rollback holds this host"), "{check:?}");
  }

  /// The NixOS module's host runs as a system unit (plan 7e-ii-a): its
  /// `--agent` words count as a user service's do, on Linux, for the
  /// directory it serves only. Nothing else of it is judged.
  #[test]
  fn the_nixos_modules_system_unit_gives_its_agents() {
      for platform in [Platform::Linux, Platform::MacOs] {
          let dir = tempfile::tempdir().unwrap();
          let fake = Fake::none();
          let cx = machine(dir.path(), platform, &fake);
          let host = paired(&dir.path().join("host"));
          let dirs = Dirs::by_contents(host.clone(), Found::Given);
          let check1 = || line(&checked(&cx, dirs.clone(), &nothing, &host), 1).clone();
          assert!(check1().summary.contains("no adapter set is installed"), "{platform:?}");

          // As the module writes it: every word quoted as `systemd_word`
          // quotes it, among the unit's other lines.
          let system = cx.root.join("etc/systemd/system");
          std::fs::create_dir_all(&system).unwrap();
          let unit_with = |data: &Path, agents: &[&str]| {
              let mut argv = unit::command_line(Role::Host, &cx.exe, data).unwrap();
              for agent in agents {
                  argv.extend(["--agent".to_string(), agent.to_string()]);
              }
              let exec = unit::systemd_unit(Role::Host, &argv, "/tmp/service.env").unwrap();
              let exec = exec.lines().find(|l| l.starts_with("ExecStart=")).unwrap().to_string();
              format!(
                  "[Unit]\nConditionPathExists={}/host.key\n\n[Service]\n{exec}\nUser=alice\n",
                  data.display()
              )
          };
          let unit = |data: &Path| unit_with(data, &["codex=/nix/store/x-codex/bin/hennery-codex-acp"]);
          std::fs::write(system.join("hennery-host.service"), unit(&host)).unwrap();
          let check = check1();
          assert_eq!(check.status, Status::Ok, "{check:?}");
          match platform {
              // Named, as check 10 finds no service.
              Platform::Linux => assert!(
                  check
                      .summary
                      .contains("the system unit /etc/systemd/system/hennery-host.service gives its agents with --agent"),
                  "{check:?}"
              ),
              Platform::MacOs => assert!(check.summary.contains("no adapter set is installed"), "{check:?}"),
          }
          // Not a user service: check 10 still finds none.
          assert!(cx.installed().is_empty());

          // A system unit of another directory says nothing of this one.
          let elsewhere = paired(&dir.path().join("elsewhere"));
          std::fs::write(system.join("hennery-host.service"), unit(&elsewhere)).unwrap();
          assert!(check1().summary.contains("no adapter set is installed"), "{platform:?}");
          // Nor does one that gives no agent (`adapters.source = "managed"`).
          std::fs::write(system.join("hennery-host.service"), unit_with(&host, &[])).unwrap();
          assert!(check1().summary.contains("no adapter set is installed"), "{platform:?}");
      }
  }

  /// A user service's `--agent` is named as the service's, not a system
  /// unit's, even with a system unit beside it.
  #[test]
  fn a_user_services_agents_are_the_services() {
      let dir = tempfile::tempdir().unwrap();
      let fake = Fake::none();
      let cx = machine(dir.path(), Platform::Linux, &fake);
      let host = paired(&dir.path().join("host"));
      let dirs = Dirs::by_contents(host.clone(), Found::Given);
      install_with_agents(&cx, Role::Host, &host);
      let system = cx.root.join("etc/systemd/system");
      std::fs::create_dir_all(&system).unwrap();
      std::fs::copy(cx.service_file(Role::Host), system.join("hennery-host.service")).unwrap();
      let check = line(&checked(&cx, dirs, &nothing, &host), 1).clone();
      assert_eq!(check.status, Status::Ok, "{check:?}");
      assert!(
          check.summary.contains("the service gives its agents with --agent"),
          "{check:?}"
      );
  }
  ```

  Run: `nix develop -c cargo test -p hennery --locked --bin hennery -- doctor::tests::the_nixos_modules_system_unit_gives_its_agents doctor::tests::a_user_services_agents_are_the_services`

  Expected: both compile; `the_nixos_modules_system_unit_gives_its_agents` fails at its first assertion after the unit is written (`assert_eq!(check.status, Status::Ok …)` passes, then on Linux check 1 still says "no adapter set is installed"); `a_user_services_agents_are_the_services` passes.

- [ ] **Step 2: The system unit's command line**

  In `crates/hennery/src/service/mod.rs`, replace:

  ```rust

  pub(crate) fn data_dir_of(argv: &[String]) -> Option<PathBuf> {
  ```

  with:

  ```rust

  /// The command line of `role`'s systemd *system* unit, as the NixOS module
  /// writes it (plan 7e-ii-a): `/etc/systemd/system/<unit>`, under `root`.
  /// Linux only. Read for its `--agent` words and nothing else: no check
  /// judges a system unit otherwise.
  pub(crate) fn read_system_command_line(cx: &Context, role: Role) -> Option<Vec<String>> {
      if cx.platform != Platform::Linux {
          return None;
      }
      let text = std::fs::read_to_string(cx.root.join("etc/systemd/system").join(role.unit())).ok()?;
      unit::systemd_command_line(&text)
  }

  pub(crate) fn data_dir_of(argv: &[String]) -> Option<PathBuf> {
  ```

- [ ] **Step 3: Doctor reads it, and check 1 names it**

  In `crates/hennery/src/doctor/mod.rs`, replace:

  ```rust
      pub fn given_agents(&self) -> Option<Vec<(String, hennery_host::AgentCommand)>> {
          use crate::service::unit::Role;
  ```

  with:

  ```rust
      pub fn given_agents(&self) -> Option<Vec<(String, hennery_host::AgentCommand)>> {
          self.given().map(|(_, agents)| agents)
      }

      /// Whether the agents are given by the NixOS module's system unit, not
      /// by a user service: no other check sees that unit.
      pub fn agents_given_by_system_unit(&self) -> bool {
          self.given().is_some_and(|(system, _)| system)
      }

      /// The agents given, and whether by a system unit. The service is a
      /// user service, or the NixOS module's system unit for a host
      /// (`/etc/systemd/system/hennery-host.service`, plan 7e-ii-a), read for
      /// its `ExecStart` alone (drop-ins in `hennery-host.service.d/` are not
      /// seen).
      fn given(&self) -> Option<(bool, Vec<(String, hennery_host::AgentCommand)>)> {
          use crate::service::unit::Role;
  ```

  In `crates/hennery/src/doctor/mod.rs`, replace:

  ```rust
          let host = self.dirs.host.as_ref().and_then(|h| h.canonicalize().ok())?;
          self.cx.installed().into_iter().find_map(|role| {
              let argv = crate::service::read_command_line(self.cx, role)?;
              let served = match (role, crate::service::data_dir_of(&argv)) {
  ```

  with:

  ```rust
          let host = self.dirs.host.as_ref().and_then(|h| h.canonicalize().ok())?;
          let user = self
              .cx
              .installed()
              .into_iter()
              .filter_map(|role| crate::service::read_command_line(self.cx, role).map(|argv| (false, role, argv)));
          let system = crate::service::read_system_command_line(self.cx, Role::Host).map(|argv| (true, Role::Host, argv));
          user.chain(system).find_map(|(system, role, argv)| {
              let served = match (role, crate::service::data_dir_of(&argv)) {
  ```

  In `crates/hennery/src/doctor/mod.rs`, replace:

  ```rust
              }
              (!given.is_empty()).then_some(given)
          })
  ```

  with:

  ```rust
              }
              (!given.is_empty()).then_some((system, given))
          })
  ```

  In `crates/hennery/src/doctor/runtime.rs`, replace:

  ```rust
          match Layout::new(host).and_then(|layout| layout.current()) {
              Ok(None) if doctor.agents_given() => verdict.ok("the service gives its agents with --agent"),
  ```

  with:

  ```rust
          match Layout::new(host).and_then(|layout| layout.current()) {
              Ok(None) if doctor.agents_given_by_system_unit() => {
                  verdict.ok("the system unit /etc/systemd/system/hennery-host.service gives its agents with --agent")
              }
              Ok(None) if doctor.agents_given() => verdict.ok("the service gives its agents with --agent"),
  ```

- [ ] **Step 4: Run the checks**

  Run:

  ```sh
  nix develop -c cargo fmt --all --check
  nix develop -c cargo clippy --workspace --all-targets --locked -- -D warnings
  nix develop -c cargo clippy -p hennery --locked -- -D warnings
  nix develop -c cargo test --workspace --locked
  nix develop -c cargo run -p hennery-proto --bin gen -- --check
  ```

  Expected: all exit 0; both new tests pass, and the workspace has two tests more than `main`.

- [ ] **Step 5: The revert-probes**

  - **rust-read** (this Mac), in `crates/hennery/src/service/mod.rs`: `let text = std::fs::read_to_string(cx.root.join("etc/systemd/system").join(role.unit())).ok()?;` → `let text = std::fs::read_to_string(cx.root.join("etc/systemd/system").join(role.unit())).ok().filter(|_| false)?;`. Run `nix develop -c cargo test -p hennery --locked --bin hennery -- doctor::tests::the_nixos_modules_system_unit_gives_its_agents`: it must fail, its output matching `doctor::tests::the_nixos_modules_system_unit_gives_its_agents \.\.\. FAILED`.
  - **rust-linux-only** (this Mac), in `crates/hennery/src/service/mod.rs`: `if cx.platform != Platform::Linux { ⏎         return None; ⏎     } ⏎     let text` → `let text`. Run `nix develop -c cargo test -p hennery --locked --bin hennery -- doctor::tests::the_nixos_modules_system_unit_gives_its_agents`: it must fail, its output matching `doctor::tests::the_nixos_modules_system_unit_gives_its_agents \.\.\. FAILED`.
  - **rust-named** (this Mac), in `crates/hennery/src/doctor/runtime.rs`: `Ok(None) if doctor.agents_given_by_system_unit() =>` → `Ok(None) if false =>`. Run `nix develop -c cargo test -p hennery --locked --bin hennery -- doctor::tests::the_nixos_modules_system_unit_gives_its_agents`: it must fail, its output matching `doctor::tests::the_nixos_modules_system_unit_gives_its_agents \.\.\. FAILED`.
  - **rust-user-first** (this Mac), in `crates/hennery/src/doctor/mod.rs`: `user.chain(system).find_map` → `system.into_iter().chain(user).find_map`. Run `nix develop -c cargo test -p hennery --locked --bin hennery -- doctor::tests::a_user_services_agents_are_the_services`: it must fail, its output matching `doctor::tests::a_user_services_agents_are_the_services \.\.\. FAILED`.
  - **rust-no-agent** (this Mac), in `crates/hennery/src/doctor/mod.rs`: `(!given.is_empty()).then_some((system, given))` → `Some((system, given))`. Run `nix develop -c cargo test -p hennery --locked --bin hennery -- doctor::tests::the_nixos_modules_system_unit_gives_its_agents`: it must fail, its output matching `doctor::tests::the_nixos_modules_system_unit_gives_its_agents \.\.\. FAILED`.
  - **rust-its-directory** (this Mac), in `crates/hennery/src/doctor/mod.rs`: `.map(|argv| (true, Role::Host, argv));` → `.map(|argv| (true, Role::Up, argv));`. Run `nix develop -c cargo test -p hennery --locked --bin hennery -- doctor::tests::the_nixos_modules_system_unit_gives_its_agents`: it must fail, its output matching `doctor::tests::the_nixos_modules_system_unit_gives_its_agents \.\.\. FAILED`.
  - **rust-system-flag** (this Mac), in `crates/hennery/src/doctor/mod.rs`: `self.given().is_some_and(|(system, _)| system)` → `self.given().is_some()`. Run `nix develop -c cargo test -p hennery --locked --bin hennery -- doctor::tests::a_user_services_agents_are_the_services`: it must fail, its output matching `doctor::tests::a_user_services_agents_are_the_services \.\.\. FAILED`.

- [ ] **Step 6: Commit**

  ```sh
  git add crates/hennery/src
  git diff --cached --stat
  git commit -m "feat(cli): doctor reads the agents the NixOS module's system unit gives"
  ```

---

### Task 3: The NixOS and home-manager modules, and their checks

**Files:**
- Create: `nix/modules/common.nix`, `nix/modules/nixos.nix`, `nix/modules/home-manager.nix`, `nix/tests/modules.nix`, `nix/tests/nixos.nix`
- Modify: `nix/outputs.nix`, `flake.nix`

**Interfaces:**
- Consumes: Task 1's `adapters.nix`; Task 2's check 1 wording (the virtual machine asserts it).
- Produces: `nixosModules.default`, `homeManagerModules.default`, each a function of `{ henneryFor }` (the flake's package for a system) applied in `flake.nix`.

- [ ] **Step 1: The shared options and command lines**

  Create `nix/modules/common.nix`:

  ```nix
  # What the NixOS and home-manager modules share (plan 7e-ii-a): the
  # options of `services.hennery.{collector,host}`, and the command lines they
  # run, written as `hennery service install` writes them (plan 7c) so
  # `hennery doctor` reads them back the same way.
  { lib }:
  let
    inherit (lib) mkOption types;
  in
  rec {
    # One word of a systemd command line, as `service/unit.rs`'s
    # `systemd_word`: double-quoted, `\` and `"` escaped, `%` and `$` doubled
    # so neither specifiers nor variables are expanded. Doctor's
    # `systemd_command_line` reads only words written this way.
    systemdWord =
      text:
      "\""
      + lib.replaceStrings
        [ "\\" "\"" "%" "$" ]
        [ "\\\\" "\\\"" "%%" "$$" ]
        text
      + "\"";

    # One `Environment=` assignment, double-quoted: `%` doubled, as systemd
    # expands specifiers there, but not variables.
    envWord =
      text: "\"" + lib.replaceStrings [ "\\" "\"" "%" ] [ "\\\\" "\\\"" "%%" ] text + "\"";

    # A command line as one `ExecStart=` value.
    execStart = argv: lib.concatMapStringsSep " " systemdWord argv;

    # A path in a unit's `Condition…=` or `…Paths=`: specifiers escaped.
    unitPath = lib.replaceStrings [ "%" ] [ "%%" ];

    # The agents a host can be given.
    agentNames = [
      "claude"
      "codex"
    ];

    # `hennery collector`'s command line.
    collectorArgv =
      exe: cfg:
      [
        exe
        "collector"
        "--data-dir"
        cfg.dataDir
      ]
      ++ lib.concatMap (address: [
        "--listen"
        address
      ]) cfg.listen
      ++ lib.optionals (cfg.publicUrl != null) [
        "--public-url"
        cfg.publicUrl
      ];

    # `hennery host run`'s command line. With the Nix adapters, each agent is
    # given with `--agent name=<its wrapper>`: the host then installs and runs
    # no managed set (distribution spec §4.3), and doctor, reading these
    # `--agent` words, judges it as such. With the managed runtime, no
    # `--agent`: the host installs the pinned set at its start.
    hostArgv =
      exe: cfg:
      [
        exe
        "host"
        "run"
        "--data-dir"
        cfg.dataDir
      ]
      ++ lib.optionals (cfg.adapters.source == "nix") (
        lib.concatMap (name: [
          "--agent"
          "${name}=${lib.getExe cfg.adapters.packages.${name}}"
        ]) cfg.adapters.agents
      )
      ++ lib.concatMap (root: [
        "--workspace-root"
        root
      ]) cfg.workspaceRoots;

    # The options both modules declare. `dataDirs` gives each role's default
    # directory and its description; `adapterPackages` the default Nix
    # adapters, built from the operator's own nixpkgs, whose configuration
    # alone accepts the Claude CLI's licence.
    options =
      {
        defaultPackage,
        adapterPackages,
        dataDirs,
        defaultSource,
        defaultSourceText,
      }:
      {
        package = mkOption {
          type = types.package;
          default = defaultPackage;
          defaultText = lib.literalExpression "hennery.packages.\${system}.default";
          description = "The hennery package both services run.";
        };
        collector = {
          enable = lib.mkEnableOption "the hennery collector";
          dataDir = mkOption {
            type = types.str;
            inherit (dataDirs.collector) default;
            description = "The collector's data directory. ${dataDirs.collector.text}";
          };
          listen = mkOption {
            type = types.nonEmptyListOf types.str;
            default = [ "127.0.0.1:7117" ];
            description = "The addresses the collector listens on (`--listen`), as `host:port`. ${dataDirs.collector.listen}";
          };
          publicUrl = mkOption {
            type = types.nullOr types.str;
            default = null;
            example = "https://hennery.example";
            description = "Where browsers reach the collector (`--public-url`), until setup stores its own.";
          };
        };
        host = {
          enable = lib.mkEnableOption "a hennery host";
          dataDir = mkOption {
            type = types.str;
            inherit (dataDirs.host) default;
            description = ''
              The host's data directory. ${dataDirs.host.text} The service is
              skipped until it holds a pairing (`host.key`), and systemd
              checks that only when the unit starts. Pair it as the user the
              host runs as, ${dataDirs.host.join}, adding `--no-runtime` when
              `adapters.source` is `nix`, then start the unit
              (${dataDirs.host.start}). A join run as another user (root)
              leaves files the service cannot read.
            '';
          };
          adapters = {
            source = mkOption {
              type = types.enum [
                "nix"
                "managed"
              ];
              default = defaultSource;
              defaultText = defaultSourceText;
              description = ''
                Where the host's adapters come from. `nix`: the pinned
                adapters built by Nix from the binary's own manifest, given
                with `--agent`. `managed`: the host downloads and installs
                the pinned set itself (distribution spec §3.2), which on
                NixOS needs `programs.nix-ld.enable`.
              '';
            };
            agents = mkOption {
              type = types.listOf (types.enum agentNames);
              default = agentNames;
              description = ''
                With `source = "nix"`, the agents the host runs. Both are in
                the default (the maintainer's decision of 2026-10-02). The
                Claude adapter bundles Anthropic's CLI under an unfree licence,
                so a host with `claude` evaluates only once you accept it in
                your nixpkgs configuration: `nixpkgs.config.allowUnfree = true`,
                or `nixpkgs.config.allowUnfreePredicate = pkg:
                lib.getName pkg == "hennery-claude-acp"` for this one package.
                Or leave `claude` out.
              '';
            };
            packages = mkOption {
              type = types.attrsOf types.package;
              default = adapterPackages;
              defaultText = lib.literalMD "the pinned adapters, built from the manifest with this nixpkgs";
              description = "The package of each agent, with `source = \"nix\"`.";
            };
          };
          workspaceRoots = mkOption {
            type = types.listOf types.str;
            default = [ ];
            example = [ "~/src" ];
            description = "Directories to find projects in (`--workspace-root`): absolute, or `~/…`.";
          };
          path = mkOption {
            type = types.listOf types.package;
            default = [ ];
            description = "Packages added to the PATH the host's agents run with, before the user's profiles.";
          };
        };
      };
  }
  ```

- [ ] **Step 2: The NixOS module**

  Create `nix/modules/nixos.nix`:

  ```nix
  # The NixOS module (plan 7e-ii-a, distribution spec §4.3):
  # `services.hennery.{collector,host}` as system services.
  #
  # - The collector runs as a system user of its own, `hennery` (kernel spec
  #   §10: a separate OS user is what protects its credentials from agents),
  #   in a hardened unit.
  # - The host runs as the login user it serves (`host.user`): its agents use
  #   that user's home, credentials and projects. Its unit is not hardened
  #   (no `NoNewPrivileges=`: the maintainer's decision of 2026-10-02), so
  #   its agents can run setuid programs (`sudo`) as at that user's login.
  #
  # Both units keep plan 7c's service model: `HENNERY_SERVICE=systemd` (each
  # process writes its own rotating log, here in a `LogsDirectory=`), a
  # restart on failure under a start limit, `KillMode=mixed` so the host stops
  # its adapters itself, and the binary's `--data-dir` on the command line.
  # A host revoked by its collector exits 78 and is not restarted (distribution
  # spec §5.2). A host without a pairing is skipped, not crash-looped.
  #
  # `hennery doctor` reads the host's `--agent` words from
  # `/etc/systemd/system/hennery-host.service`; it does not judge a system
  # unit otherwise (its check 10 looks for user services).
  { henneryFor }:
  {
    config,
    lib,
    pkgs,
    ...
  }:
  let
    common = import ./common.nix { inherit lib; };
    cfg = config.services.hennery;
    exe = lib.getExe cfg.package;
    # A user that is not there fails the assertion below, not an evaluation.
    hostUser =
      config.users.users.${cfg.host.user} or {
        isNormalUser = false;
        group = "nogroup";
        home = "/var/empty";
      };
    restart = {
      unitConfig = {
        StartLimitIntervalSec = 300;
        StartLimitBurst = 10;
      };
      serviceConfig = {
        Type = "exec";
        Restart = "on-failure";
        RestartSec = 3;
        KillMode = "mixed";
        TimeoutStopSec = 30;
      };
    };
  in
  {
    options.services.hennery = lib.recursiveUpdate (common.options {
        defaultPackage = henneryFor pkgs.stdenv.hostPlatform.system;
        adapterPackages = pkgs.callPackage ../adapters.nix { };
        dataDirs = {
          collector = {
            default = "/var/lib/hennery";
            text = "Owned by the `hennery` system user, 0700. `hennery admin` runs as that user: `sudo -u hennery hennery admin --data-dir <this> …`.";
            listen = "The unit keeps no capability: a port below 1024 needs a proxy in front.";
          };
          host = {
            default = "/var/lib/hennery-host";
            text = "Made by systemd-tmpfiles, owned by `host.user`, 0700: keep it outside that user's home, where tmpfiles would make missing parents as root.";
            join = "`sudo -u <user> hennery host join <url> --data-dir <this>`";
            start = "`systemctl start hennery-host`";
          };
        };
        defaultSource = "nix";
        defaultSourceText = lib.literalExpression ''"nix"'';
      }) {
        host.user = lib.mkOption {
          type = lib.types.str;
          example = "alice";
          description = "The login user the host runs as: its agents run with this user's home, credentials and projects.";
        };
      };

    config = lib.mkMerge [
      (lib.mkIf cfg.collector.enable {
        users.users.hennery = {
          isSystemUser = true;
          group = "hennery";
          home = cfg.collector.dataDir;
        };
        users.groups.hennery = { };
        systemd.tmpfiles.settings."10-hennery".${cfg.collector.dataDir}.d = {
          user = "hennery";
          group = "hennery";
          mode = "0700";
        };
        systemd.services.hennery-collector = lib.recursiveUpdate restart {
          description = "hennery collector";
          wantedBy = [ "multi-user.target" ];
          after = [ "network.target" ];
          environment = {
            HENNERY_SERVICE = "systemd";
            HENNERY_LOG_DIR = "/var/log/hennery-collector";
          };
          serviceConfig = {
            ExecStart = common.execStart (common.collectorArgv exe cfg.collector);
            User = "hennery";
            Group = "hennery";
            LogsDirectory = "hennery-collector";
            LogsDirectoryMode = "0700";
            UMask = "0077";
            ReadWritePaths = [ (common.unitPath cfg.collector.dataDir) ];
            # The collector runs no program (agents are the host's): it keeps
            # no capability, so a port below 1024 needs a proxy in front.
            CapabilityBoundingSet = "";
            AmbientCapabilities = "";
            NoNewPrivileges = true;
            PrivateUsers = true;
            PrivateTmp = true;
            PrivateDevices = true;
            ProtectSystem = "strict";
            ProtectHome = true;
            ProtectClock = true;
            ProtectHostname = true;
            ProtectKernelLogs = true;
            ProtectKernelTunables = true;
            ProtectKernelModules = true;
            ProtectControlGroups = true;
            ProtectProc = "invisible";
            ProcSubset = "pid";
            RestrictAddressFamilies = [
              "AF_INET"
              "AF_INET6"
              "AF_UNIX"
            ];
            RestrictNamespaces = true;
            RestrictSUIDSGID = true;
            RestrictRealtime = true;
            RemoveIPC = true;
            MemoryDenyWriteExecute = true;
            LockPersonality = true;
            SystemCallArchitectures = "native";
            SystemCallFilter = [
              "@system-service"
              "~@privileged @resources"
            ];
          };
        };
        environment.systemPackages = [ cfg.package ];
      })

      (lib.mkIf cfg.host.enable {
        assertions = [
          {
            assertion = hostUser.isNormalUser;
            message = "services.hennery.host.user: ${cfg.host.user} is not a normal user of this system; the host runs as the user whose agents it runs.";
          }
          {
            assertion = cfg.host.adapters.source != "managed" || config.programs.nix-ld.enable;
            message = ''services.hennery.host.adapters.source = "managed" downloads glibc programs, which NixOS runs only with programs.nix-ld.enable; or use source = "nix".'';
          }
        ];
        systemd.tmpfiles.settings."10-hennery-host".${cfg.host.dataDir}.d = {
          user = cfg.host.user;
          inherit (hostUser) group;
          mode = "0700";
        };
        systemd.services.hennery-host = lib.recursiveUpdate restart {
          description = "hennery host";
          wantedBy = [ "multi-user.target" ];
          wants = [ "network-online.target" ];
          after = [ "network-online.target" ];
          # The agents' tools: what this unit is given, then the user's and
          # the system's profiles, then a shell and git.
          path =
            cfg.host.path
            ++ [
              "/run/wrappers"
              "/etc/profiles/per-user/${cfg.host.user}"
              "${hostUser.home}/.nix-profile"
              "/run/current-system/sw"
            ]
            ++ (with pkgs; [
              bash
              git
            ]);
          environment = {
            HENNERY_SERVICE = "systemd";
            HENNERY_LOG_DIR = "/var/log/hennery-host";
          };
          unitConfig.ConditionPathExists = common.unitPath "${cfg.host.dataDir}/host.key";
          serviceConfig = {
            ExecStart = common.execStart (common.hostArgv exe cfg.host);
            User = cfg.host.user;
            Group = hostUser.group;
            # Revoked (distribution spec §5.2): pairing again is the
            # operator's, so the unit stops (the maintainer's decision of
            # 2026-10-02).
            RestartPreventExitStatus = 78;
            LogsDirectory = "hennery-host";
            LogsDirectoryMode = "0700";
          };
        };
        environment.systemPackages = [ cfg.package ];
      })
    ];
  }
  ```

- [ ] **Step 3: The home-manager module**

  Create `nix/modules/home-manager.nix`:

  ```nix
  # The home-manager module (plan 7e-ii-a, distribution spec §4.3):
  # `services.hennery.{collector,host}` as systemd user services, the units
  # `hennery service install` writes (plan 7c, §6.3): `hennery-collector`
  # and `hennery-host`, in `~/.config/systemd/user`, where `hennery doctor`
  # reads them, with their PATH in `~/.config/hennery/service.env`, which
  # doctor's check 5 reads (and, as the module's PATH is not the login
  # shell's, may say has drifted: change it with `host.path`). The file has
  # no `SHELL=` line: the user manager gives each unit the account's login
  # shell, which is what `service install` records without `--shell`, and
  # doctor reads a file without one as such. Use this
  # module or `hennery service install`, not both: the module's files are
  # read-only links into the store. Linux only: on macOS, `hennery service
  # install` writes the launchd agent.
  #
  # As there, a user service runs only while the user is logged in unless
  # linger is on (`loginctl enable-linger`). A host without a pairing is
  # skipped, not crash-looped; a revoked one (exit 78) is not restarted.
  { henneryFor }:
  {
    config,
    lib,
    pkgs,
    ...
  }:
  let
    common = import ./common.nix { inherit lib; };
    cfg = config.services.hennery;
    exe = lib.getExe cfg.package;
    dataHome = config.xdg.dataHome;
    # The agents' PATH: what `host.path` gives, the user's and the system's
    # profiles, then the base system's directories, as plan 7c's captured
    # PATH ends. `sh` and `git` come from these; doctor's check 5 says when
    # either is missing.
    path = lib.concatStringsSep ":" (
      map (p: "${p}/bin") cfg.host.path
      ++ [
        "${config.home.profileDirectory}/bin"
        "/etc/profiles/per-user/${config.home.username}/bin"
        "/run/wrappers/bin"
        "/run/current-system/sw/bin"
        "/nix/var/nix/profiles/default/bin"
        "/usr/bin"
        "/bin"
      ]
    );
    # Where `hennery service install` writes the PATH, in its format
    # (`unit::env_file`) but without its `SHELL=` line (see above), so
    # doctor's check 5 reads it back.
    envFile = "${config.xdg.configHome}/hennery/service.env";
    envQuoted =
      text: "\"" + lib.replaceStrings [ "\\" "\"" "$" "`" ] [ "\\\\" "\\\"" "\\$" "\\`" ] text + "\"";
    unit = role: argv: extra: {
      Unit = {
        Description = "hennery ${role}";
        StartLimitIntervalSec = 300;
        StartLimitBurst = 10;
      } // extra.Unit or { };
      Service = {
        Type = "exec";
        ExecStart = common.execStart argv;
        EnvironmentFile = common.unitPath envFile;
        Environment = common.envWord "HENNERY_SERVICE=systemd";
        Restart = "on-failure";
        RestartSec = 3;
        KillMode = "mixed";
        TimeoutStopSec = 30;
      } // extra.Service or { };
      Install.WantedBy = [ "default.target" ];
    };
  in
  {
    options.services.hennery = common.options {
      defaultPackage = henneryFor pkgs.stdenv.hostPlatform.system;
      adapterPackages = pkgs.callPackage ../adapters.nix { };
      dataDirs = {
        collector = {
          default = "${dataHome}/hennery/collector";
          text = "Made 0700 by the collector.";
          listen = "";
        };
        host = {
          default = "${dataHome}/hennery/host";
          text = "Made 0700 by `hennery host join`.";
          join = "`hennery host join <url> --data-dir <this>`";
          start = "`systemctl --user start hennery-host`";
        };
      };
      defaultSource = "nix";
      defaultSourceText = lib.literalExpression ''"nix"'';
    };

    config = lib.mkIf (cfg.collector.enable || cfg.host.enable) {
      assertions = [
        {
          assertion = pkgs.stdenv.hostPlatform.isLinux;
          message = "services.hennery: the home-manager module writes systemd user services, on Linux only; on macOS run `hennery service install`.";
        }
      ];
      home.packages = [ cfg.package ];
      xdg.configFile."hennery/service.env".text = ''
        # Written by the home-manager module of hennery: the services' PATH.
        PATH=${envQuoted path}
      '';
      systemd.user.services = lib.mkMerge [
        (lib.mkIf cfg.collector.enable {
          hennery-collector = unit "collector" (common.collectorArgv exe cfg.collector) { };
        })
        (lib.mkIf cfg.host.enable {
          hennery-host = unit "host" (common.hostArgv exe cfg.host) {
            Unit.ConditionPathExists = common.unitPath "${cfg.host.dataDir}/host.key";
            Service.RestartPreventExitStatus = 78;
          };
        })
      ];
    };
  }
  ```

- [ ] **Step 4: The checks**

  Create `nix/tests/modules.nix`:

  ```nix
  # The modules' evaluation checks (plan 7e-ii-a), Linux only:
  #
  # - `modules-eval`: the NixOS module's units for each `adapters.source`, the
  #   consent the Claude adapter needs, and the assertions, and the
  #   home-manager module's unit and PATH file, all evaluated without building
  #   a system;
  # - `home-manager-doctor`: the home-manager module evaluated against stand-ins
  #   for home-manager's own options (this flake takes no home-manager
  #   input), its host unit written where `hennery service install` writes
  #   one, and the real `hennery doctor` reading its `--agent` back, with a data
  #   directory holding every character the unit must escape.
  {
    pkgs,
    nixpkgs,
    hennery,
    nixosModule,
    homeManagerModule,
  }:
  let
    inherit (pkgs) lib;
    inherit (pkgs.stdenv.hostPlatform) system;

    nixos =
      config:
      (import "${nixpkgs}/nixos/lib/eval-config.nix" {
        inherit system;
        modules = [
          nixosModule
          {
            users.users.alice.isNormalUser = true;
            system.stateVersion = "26.05";
            boot.loader.grub.enable = false;
            fileSystems."/" = {
              device = "none";
              fsType = "tmpfs";
            };
          }
          config
        ];
      }).config;
    failedAssertions = config: map (a: a.message) (lib.filter (a: !a.assertion) config.assertions);
    hostExec = config: config.systemd.services.hennery-host.serviceConfig.ExecStart;
    host = extra: {
      services.hennery.host = {
        enable = true;
        user = "alice";
      } // extra;
    };

    nixNoClaude = nixos (host { adapters.agents = [ "codex" ]; });
    nixBoth = nixos (host { } // { nixpkgs.config.allowUnfree = true; });
    # The predicate the option's description gives, for this one package.
    nixPredicate = nixos (
      host { } // { nixpkgs.config.allowUnfreePredicate = pkg: lib.getName pkg == "hennery-claude-acp"; }
    );
    nixDefault = nixos (host { });
    managed = nixos (host { adapters.source = "managed"; });
    managedLd = nixos (host { adapters.source = "managed"; } // { programs.nix-ld.enable = true; });
    stranger = nixos {
      services.hennery.host = {
        enable = true;
        user = "nobody-here";
      };
    };
    collector = nixos {
      services.hennery.collector = {
        enable = true;
        listen = [ "127.0.0.1:7117" "[::1]:7117" ];
        publicUrl = "https://hennery.example";
      };
    };

    # home-manager's options this module uses, as stand-ins.
    homeStubs =
      { lib, ... }:
      {
        options = {
          assertions = lib.mkOption {
            type = lib.types.listOf lib.types.anything;
            default = [ ];
          };
          home.packages = lib.mkOption {
            type = lib.types.listOf lib.types.package;
            default = [ ];
          };
          home.username = lib.mkOption { type = lib.types.str; };
          home.profileDirectory = lib.mkOption { type = lib.types.str; };
          xdg.dataHome = lib.mkOption { type = lib.types.str; };
          xdg.configHome = lib.mkOption { type = lib.types.str; };
          xdg.configFile = lib.mkOption {
            type = lib.types.attrsOf (lib.types.submodule { options.text = lib.mkOption { type = lib.types.lines; }; });
            default = { };
          };
          systemd.user.services = lib.mkOption {
            type = lib.types.attrsOf lib.types.anything;
            default = { };
          };
        };
      };
    # `@DATA@` is the check's own temporary directory, put in at build time.
    data = "@DATA@/a dir %h $HOME \"quoted\" \\back";
    home =
      (lib.evalModules {
        modules = [
          homeStubs
          homeManagerModule
          {
            _module.args.pkgs = pkgs;
            home.username = "alice";
            home.profileDirectory = "/home/alice/.nix-profile";
            xdg.dataHome = "/home/alice/.local/share";
            xdg.configHome = "/home/alice/.config";
            services.hennery.package = hennery;
            services.hennery.host = {
              enable = true;
              dataDir = data;
              adapters.agents = [ "codex" ];
              # The sandbox has no profile: the shell and git the agents need.
              path = [
                pkgs.bash
                pkgs.git
              ];
            };
          }
        ];
      }).config;
    homeHost = home.systemd.user.services.hennery-host;
    codex = (pkgs.callPackage ../adapters.nix { }).codex;
    has = needle: haystack: lib.hasInfix needle haystack;
    # Each check, its name and whether it holds.
    results = {
      "nix: the agents given with --agent" =
        has ''"--agent" "codex=/nix/store/'' (hostExec nixNoClaude)
        && !has "claude=" (hostExec nixNoClaude)
        && has ''"host" "run" "--data-dir" "/var/lib/hennery-host"'' (hostExec nixNoClaude);
      "nix: claude given once its licence is accepted" =
        has ''"claude=/nix/store/'' (hostExec nixBoth) && has ''"codex=/nix/store/'' (hostExec nixBoth);
      "nix: claude given with the documented predicate" = has ''"claude=/nix/store/'' (hostExec nixPredicate);
      "nix: claude refused without the licence accepted" =
        !(builtins.tryEval (builtins.seq (hostExec nixDefault) true)).success;
      "managed: no --agent" = !has "--agent" (hostExec managed);
      "managed: refused without nix-ld" = lib.any (has "nix-ld") (failedAssertions managed);
      "managed: accepted with nix-ld" = failedAssertions managedLd == [ ];
      "nix: no assertion fails" = failedAssertions nixNoClaude == [ ];
      "a user who is not there is refused" = lib.any (has "not a normal user") (failedAssertions stranger);
      "host: skipped until paired" =
        nixNoClaude.systemd.services.hennery-host.unitConfig.ConditionPathExists or null == "/var/lib/hennery-host/host.key";
      "host: a revoked host is not restarted" =
        nixNoClaude.systemd.services.hennery-host.serviceConfig.RestartPreventExitStatus or null == 78;
      "host: runs as its user" = nixNoClaude.systemd.services.hennery-host.serviceConfig.User or null == "alice";
      "collector: its command line" =
        collector.systemd.services.hennery-collector.serviceConfig.ExecStart
        == ''"${lib.getExe hennery}" "collector" "--data-dir" "/var/lib/hennery" "--listen" "127.0.0.1:7117" "--listen" "[::1]:7117" "--public-url" "https://hennery.example"'';
      "collector: its own system user" =
        collector.systemd.services.hennery-collector.serviceConfig.User or null == "hennery"
        && collector.users.users.hennery.isSystemUser;
      "home-manager: every word quoted as unit.rs quotes it" =
        homeHost.Service.ExecStart
        == ''"${lib.getExe hennery}" "host" "run" "--data-dir" "@DATA@/a dir %%h $$HOME \"quoted\" \\back" "--agent" "codex=${lib.getExe codex}"'';
      "home-manager: the PATH in service.env, in its quoting" =
        home.xdg.configFile."hennery/service.env".text
        == "# Written by the home-manager module of hennery: the services' PATH.\nPATH=\"${pkgs.bash}/bin:${pkgs.git}/bin:/home/alice/.nix-profile/bin:/etc/profiles/per-user/alice/bin:/run/wrappers/bin:/run/current-system/sw/bin:/nix/var/nix/profiles/default/bin:/usr/bin:/bin\"\n";
      "home-manager: the unit reads it" =
        homeHost.Service.EnvironmentFile or null == "/home/alice/.config/hennery/service.env";
      "home-manager: skipped until paired, not restarted once revoked" =
        homeHost.Unit.ConditionPathExists or null == "@DATA@/a dir %%h $HOME \"quoted\" \\back/host.key"
        && homeHost.Service.RestartPreventExitStatus or null == 78;
      "home-manager: no assertion fails on Linux" = failedAssertions home == [ ];
    };
    failed = lib.attrNames (lib.filterAttrs (_: ok: !ok) results);

    # The unit as home-manager writes it: one `key=value` line per value.
    ini =
      unit:
      lib.concatStrings (
        lib.mapAttrsToList (
          section: keys:
          "[${section}]\n"
          + lib.concatStrings (
            lib.mapAttrsToList (
              key: value: lib.concatMapStrings (v: "${key}=${toString v}\n") (lib.toList value)
            ) keys
          )
          + "\n"
        ) unit
      );
    homeUnit = pkgs.writeText "hennery-host.service" (ini home.systemd.user.services.hennery-host);
    homeEnv = pkgs.writeText "service.env" home.xdg.configFile."hennery/service.env".text;
  in
  {
    modules-eval = pkgs.runCommand "hennery-modules-eval" { } ''
      ${lib.concatStrings (lib.mapAttrsToList (name: ok: "echo '${if ok then "ok  " else "FAIL"} ${name}'\n") results)}
      ${lib.optionalString (failed != [ ]) "exit 1"}
      touch "$out"
    '';

    home-manager-doctor =
      pkgs.runCommand "hennery-home-manager-doctor"
        {
          nativeBuildInputs = [ hennery ];
        }
        ''
          # As `offline()` in crates/hennery/tests/cli.rs runs the binary:
          # every directory doctor could default to is this check's own, no
          # data directory or secret is inherited, nothing logs to a file, and
          # the managed runtime's sources are a port nothing listens on.
          export HOME="$TMPDIR/home" XDG_CONFIG_HOME="$TMPDIR/config"
          export XDG_DATA_HOME="$TMPDIR/data" XDG_STATE_HOME="$TMPDIR/state" XDG_CACHE_HOME="$TMPDIR/cache"
          export HENNERY_NPM_REGISTRY=http://127.0.0.1:1/ HENNERY_NODE_MIRROR=http://127.0.0.1:1/
          unset HENNERY_DATA_DIR HENNERY_HOST_DATA_DIR HENNERY_MASTER_KEY CREDENTIALS_DIRECTORY HENNERY_DEV_TOKEN
          unset HENNERY_SERVICE HENNERY_LOG_DIR CLAUDE_CONFIG_DIR CODEX_HOME CODEX_SQLITE_HOME
          data="$TMPDIR/a dir %h \$HOME \"quoted\" \\back"
          mkdir -p "$HOME" "$data" "$XDG_CONFIG_HOME/systemd/user" "$XDG_CONFIG_HOME/hennery"
          # A host's directory, by its names alone.
          touch "$data/host.key"
          sed "s|@DATA@|$TMPDIR|g" ${homeUnit} > "$XDG_CONFIG_HOME/systemd/user/hennery-host.service"
          cp ${homeEnv} "$XDG_CONFIG_HOME/hennery/service.env"
          cat "$XDG_CONFIG_HOME/systemd/user/hennery-host.service" "$XDG_CONFIG_HOME/hennery/service.env"
          grep -q 'ExecStart=".*" "host" "run" "--data-dir" ".*/a dir %%h $$HOME \\"quoted\\" \\\\back" "--agent" "codex=' \
            "$XDG_CONFIG_HOME/systemd/user/hennery-host.service"
          hennery doctor --data-dir "$data" > report 2>&1 || true
          cat report
          grep -E '^ok +1 .*; the service gives its agents with --agent$' report
          grep -E '^ok +3 .*codex answers .*given by --agent' report
          # Check 5 reads the module's PATH, and finds sh and git on it.
          grep -E '^(ok|warn) +5 ' report
          if grep -E '^(ok|warn|fail) +5 .*(cannot be read|(sh|git) is not on the service)' report; then
            exit 1
          fi
          touch "$out"
        '';
  }
  ```

  Create `nix/tests/nixos.nix`:

  ```nix
  # The NixOS module in a virtual machine (plan 7e-ii-a): a collector and a
  # host on one machine, the host given the Nix Codex adapter (the free one:
  # the Claude adapter is unfree, and no check may need its licence).
  #
  # - The collector starts as its own system user and answers `/healthz`.
  # - The host is skipped while it holds no pairing, not failed.
  # - Paired with `host join --no-runtime`, it starts, connects, and logs to
  #   its `LogsDirectory=`.
  # - `hennery doctor`, run as the host's user, reads the system unit's
  #   `--agent` and judges no managed set: check 1 says so, check 2 does not
  #   fail for want of nix-ld, and check 3 starts the Nix adapter.
  # - The collector's unit keeps its hardening, and logs to its own directory.
  #
  # Everything runs inside the virtual machine: its service manager, users and
  # data directories are its own, never the machine building it.
  { pkgs, nixosModule }:
  pkgs.testers.runNixOSTest {
    name = "hennery-nixos";
    nodes.machine =
      { pkgs, ... }:
      {
        imports = [ nixosModule ];
        virtualisation.memorySize = 2048;
        users.users.alice = {
          isNormalUser = true;
          uid = 1000;
        };
        services.hennery = {
          collector.enable = true;
          host = {
            enable = true;
            user = "alice";
            adapters.agents = [ "codex" ];
          };
        };
        environment.systemPackages = [ pkgs.curl ];
      };
    testScript = ''
      import re

      machine.wait_for_unit("hennery-collector.service")
      machine.wait_for_open_port(7117)
      machine.succeed("curl -sf http://127.0.0.1:7117/healthz")
      machine.succeed("[ \"$(stat -c '%U %a' /var/lib/hennery)\" = 'hennery 700' ]")

      with subtest("the collector runs as its own user, hardened"):
          machine.succeed("[ \"$(systemctl show -P User hennery-collector.service)\" = hennery ]")
          machine.succeed("[ \"$(systemctl show -P NoNewPrivileges hennery-collector.service)\" = yes ]")
          machine.succeed("[ \"$(systemctl show -P ProtectSystem hennery-collector.service)\" = strict ]")
          machine.succeed("[ -z \"$(systemctl show -P CapabilityBoundingSet hennery-collector.service)\" ]")
          machine.succeed("[ \"$(systemctl show -P MemoryDenyWriteExecute hennery-collector.service)\" = yes ]")
          machine.succeed("[ \"$(stat -c '%U %a' /var/log/hennery-collector)\" = 'hennery 700' ]")
          machine.wait_until_succeeds("[ -s /var/log/hennery-collector/hennery-collector.log ]", timeout=30)

      with subtest("an unpaired host is skipped, not failed"):
          machine.succeed("[ \"$(stat -c '%U %a' /var/lib/hennery-host)\" = 'alice 700' ]")
          machine.succeed("systemctl start hennery-host.service")
          machine.succeed("[ \"$(systemctl show -P ConditionResult hennery-host.service)\" = no ]")
          machine.fail("systemctl is-failed hennery-host.service")

      with subtest("the unit gives the Nix adapter and keeps 7c's policy"):
          exec_start = machine.succeed("systemctl show -P ExecStart hennery-host.service")
          assert "--agent codex=/nix/store/" in exec_start, exec_start
          assert "hennery-codex-acp" in exec_start, exec_start
          machine.succeed("[ \"$(systemctl show -P RestartPreventExitStatus hennery-host.service)\" = 78 ]")
          machine.succeed("[ \"$(systemctl show -P KillMode hennery-host.service)\" = mixed ]")
          machine.succeed("[ \"$(systemctl show -P User hennery-host.service)\" = alice ]")

      def dump():
          # What a failure needs to be read from CI's log alone.
          print(machine.execute(
              "systemctl status hennery-host.service hennery-collector.service --no-pager -l; "
              "journalctl -u hennery-host -u hennery-collector --no-pager | tail -n 80; "
              "tail -n 40 /var/log/hennery-host/hennery-host.log; ls -la /var/lib/hennery-host"
          )[1])

      with subtest("paired, the host runs and connects"):
          try:
              # `admin pairing-code` asks on a terminal: `script` gives it one,
              # and the answer is typed ahead.
              said = machine.succeed(
                  "{ echo yes; sleep 5; } | script -qec"
                  " 'runuser -u hennery -- hennery admin --data-dir /var/lib/hennery pairing-code' /dev/null",
                  timeout=60,
              )
              found = re.search(r"\b[A-Za-z0-9]{4}-[A-Za-z0-9]{4}\b", said)
              assert found, said
              code = found.group(0)
              machine.succeed(
                  f"runuser -u alice -- hennery host join http://127.0.0.1:7117 {code} --no-runtime"
                  " --data-dir /var/lib/hennery-host < /dev/null",
                  timeout=60,
              )
              machine.succeed("systemctl start hennery-host.service", timeout=30)
              machine.wait_for_unit("hennery-host.service", timeout=60)
              machine.wait_until_succeeds(
                  "grep -q 'connected to collector' /var/log/hennery-host/hennery-host.log", timeout=60
              )
              hosts = machine.succeed("runuser -u hennery -- hennery admin --data-dir /var/lib/hennery hosts", timeout=30)
              assert "paired" in hosts, hosts
          except Exception:
              dump()
              raise

      with subtest("doctor sees the system unit's --agent"):
          report = machine.succeed(
              "runuser -u alice -- hennery doctor --data-dir /var/lib/hennery-host < /dev/null || true", timeout=180
          )
          print(report)
          lines = {
              int(m.group(2)): m.group(0)
              for m in re.finditer(r"^(ok|warn|fail)\s+(\d+) .*$", report, re.M)
          }
          assert (
              "the system unit /etc/systemd/system/hennery-host.service gives its agents with --agent" in lines[1]
          ), report
          assert not lines[2].startswith("fail"), report
          assert "codex answers" in lines[3] and lines[3].startswith("ok"), report
    '';
  }
  ```

- [ ] **Step 5: The flake's outputs**

  Replace the whole of `nix/outputs.nix` with:

  ```nix
  # The flake's outputs for one system from plan 7e-ii-a: the pinned adapters
  # (`nix/adapters.nix`), their checks, and the modules' checks. Kept apart
  # from `flake.nix` so the flake names them in three lines.
  {
    pkgs,
    nixpkgs,
    system,
    hennery,
    nixosModule,
    homeManagerModule,
  }:
  let
    inherit (nixpkgs) lib;
    adapters = pkgs.callPackage ./adapters.nix { };
    adapterCheck = pkgs.callPackage ./adapter-check.nix { };
    # Gated on the system's name, never on `pkgs.stdenv`.
    linux = lib.hasSuffix "-linux" system;
    modules = import ./tests/modules.nix {
      inherit
        pkgs
        nixpkgs
        hennery
        nixosModule
        homeManagerModule
        ;
    };
  in
  {
    codex-acp = adapters.codex;
    # Not evaluated by `nix flake check`: the Claude adapter needs the
    # operator's consent to its licence.
    legacyPackages = {
      claude-acp = adapters.claude;
      claude-acp-runs = adapterCheck adapters.claude;
    };
    checks = {
      codex-acp-runs = adapterCheck adapters.codex;
      unpack-refuses = pkgs.callPackage ./unpack-check.nix { };
      adapter-check-cases = pkgs.callPackage ./adapter-check-cases.nix { inherit adapterCheck; };
    }
    // lib.optionalAttrs linux {
      inherit (modules) modules-eval home-manager-doctor;
    }
    // lib.optionalAttrs (system == "x86_64-linux") {
      # Virtual machines need KVM, which only the x86_64 runner has.
      nixos = import ./tests/nixos.nix { inherit pkgs nixosModule; };
    };
  }
  ```

  In `flake.nix`, replace:

  ```nix

    outputs = { nixpkgs, nixpkgs-web, flake-utils, crane, advisory-db, ... }:
      # The v1 platforms (distribution spec §1): Intel Macs are not one.
  ```

  with:

  ```nix

    outputs = { self, nixpkgs, nixpkgs-web, flake-utils, crane, advisory-db, ... }:
      let
        # The modules run this flake's own package (plan 7e-ii-a).
        henneryFor = system: self.packages.${system}.default;
        nixosModule = import ./nix/modules/nixos.nix { inherit henneryFor; };
        homeManagerModule = import ./nix/modules/home-manager.nix { inherit henneryFor; };
      in
      {
        nixosModules.default = nixosModule;
        homeManagerModules.default = homeManagerModule;
      }
      //
      # The v1 platforms (distribution spec §1): Intel Macs are not one.
  ```

  In `flake.nix`, replace:

  ```nix
          webTools = [ web.nodejs_24 web.pnpm ];
          # The pinned adapters and their checks (plan 7e-ii-a).
          nixOutputs = import ./nix/outputs.nix { inherit pkgs nixpkgs; };
        in {
  ```

  with:

  ```nix
          webTools = [ web.nodejs_24 web.pnpm ];
          # The pinned adapters, their checks and the modules' (plan 7e-ii-a).
          nixOutputs = import ./nix/outputs.nix {
            inherit pkgs nixpkgs system nixosModule homeManagerModule;
            hennery = hennery.package;
          };
        in {
  ```

- [ ] **Step 6: Evaluate, and build what this machine can**

  Run:

  ```sh
  git add nix flake.nix
  nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand | grep -E "^echo '(ok|FAIL)"
  nix flake check --all-systems --no-build
  nix build --no-link -L .#checks.<system>.unpack-refuses
  ```

  Expected: 19 lines, each starting `echo 'ok`, none `FAIL`; the flake check exits 0. On ubuntu (CI): `modules-eval` prints the same 19 `ok` lines; `home-manager-doctor` prints doctor's report with `ok  1 …; the service gives its agents with --agent` and `ok  3 … codex answers …`; the virtual machine (`checks.x86_64-linux.nixos`) passes its six subtests, and its doctor report has check 1 `… the system unit /etc/systemd/system/hennery-host.service gives its agents with --agent`.

- [ ] **Step 7: The revert-probes**

  - **eval-agent** (this Mac), in `nix/modules/common.nix`: `"--agent" ⏎         "${name}=` → `"--agen" ⏎         "${name}=`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must succeed, its output matching `echo 'FAIL nix:\ the\ agents\ given\ with\ \-\-agent'`.
  - **eval-managed** (this Mac), in `nix/modules/common.nix`: `lib.optionals (cfg.adapters.source == "nix") (` → `lib.optionals true (`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must fail, its output matching `Refusing to evaluate package 'hennery-claude-acp-[0-9.]+' .* because it has an unfree license`.
  - **eval-percent** (this Mac), in `nix/modules/common.nix`: `[ "\\\\" "\\\"" "%%" "$$" ]` → `[ "\\\\" "\\\"" "%" "$$" ]`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must succeed, its output matching `echo 'FAIL home\-manager:\ every\ word\ quoted\ as\ unit\.rs\ quotes\ it'`.
  - **eval-dollar** (this Mac), in `nix/modules/common.nix`: `[ "\\\\" "\\\"" "%%" "$$" ]` → `[ "\\\\" "\\\"" "%%" "$" ]`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must succeed, its output matching `echo 'FAIL home\-manager:\ every\ word\ quoted\ as\ unit\.rs\ quotes\ it'`.
  - **eval-listen** (this Mac), in `nix/modules/common.nix`: `"--listen" ⏎       address` → `"--listn" ⏎       address`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must succeed, its output matching `echo 'FAIL collector:\ its\ command\ line'`.
  - **eval-public-url** (this Mac), in `nix/modules/common.nix`: `"--public-url" ⏎       cfg.publicUrl` → `"--public-ur" ⏎       cfg.publicUrl`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must succeed, its output matching `echo 'FAIL collector:\ its\ command\ line'`.
  - **eval-nixos-revoked** (this Mac), in `nix/modules/nixos.nix`: `RestartPreventExitStatus = 78;` → `(nothing)`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must succeed, its output matching `echo 'FAIL host:\ a\ revoked\ host\ is\ not\ restarted'`.
  - **eval-nixos-paired** (this Mac), in `nix/modules/nixos.nix`: `unitConfig.ConditionPathExists = common.unitPath "${cfg.host.dataDir}/host.key";` → `(nothing)`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must succeed, its output matching `echo 'FAIL host:\ skipped\ until\ paired'`.
  - **eval-nixos-user** (this Mac), in `nix/modules/nixos.nix`: `User = cfg.host.user;` → `(nothing)`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must succeed, its output matching `echo 'FAIL host:\ runs\ as\ its\ user'`.
  - **eval-nix-ld** (this Mac), in `nix/modules/nixos.nix`: `assertion = cfg.host.adapters.source != "managed" || config.programs.nix-ld.enable;` → `assertion = true;`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must succeed, its output matching `echo 'FAIL managed:\ refused\ without\ nix\-ld'`.
  - **eval-normal-user** (this Mac), in `nix/modules/nixos.nix`: `assertion = hostUser.isNormalUser;` → `assertion = true;`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must succeed, its output matching `echo 'FAIL a\ user\ who\ is\ not\ there\ is\ refused'`.
  - **eval-collector-user** (this Mac), in `nix/modules/nixos.nix`: `User = "hennery";` → `(nothing)`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must succeed, its output matching `echo 'FAIL collector:\ its\ own\ system\ user'`.
  - **eval-unfree** (this Mac), in `nix/adapters.nix`: `license = lib.licenses.unfree;` → `license = lib.licenses.mit;`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must succeed, its output matching `echo 'FAIL nix:\ claude\ refused\ without\ the\ licence\ accepted'`.
  - **eval-hm-revoked** (this Mac), in `nix/modules/home-manager.nix`: `Service.RestartPreventExitStatus = 78;` → `(nothing)`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must succeed, its output matching `echo 'FAIL home\-manager:\ skipped\ until\ paired,\ not\ restarted\ once\ revoked'`.
  - **eval-hm-paired** (this Mac), in `nix/modules/home-manager.nix`: `Unit.ConditionPathExists = common.unitPath "${cfg.host.dataDir}/host.key";` → `(nothing)`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must succeed, its output matching `echo 'FAIL home\-manager:\ skipped\ until\ paired,\ not\ restarted\ once\ revoked'`.
  - **eval-hm-envfile** (this Mac), in `nix/modules/home-manager.nix`: `EnvironmentFile = common.unitPath envFile;` → `(nothing)`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must succeed, its output matching `echo 'FAIL home\-manager:\ the\ unit\ reads\ it'`.
  - **eval-hm-path** (this Mac), in `nix/modules/home-manager.nix`: `"/usr/bin" ⏎` → `(nothing)`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must succeed, its output matching `echo 'FAIL home\-manager:\ the\ PATH\ in\ service\.env,\ in\ its\ quoting'`.
  - **eval-hm-linux** (this Mac), in `nix/modules/home-manager.nix`: `assertion = pkgs.stdenv.hostPlatform.isLinux;` → `assertion = false;`. Run `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`: it must succeed, its output matching `echo 'FAIL home\-manager:\ no\ assertion\ fails\ on\ Linux'`.
  - **eval-fail-fails** (Linux, in CI), in `nix/tests/modules.nix`: `"host: runs as its user" = nixNoClaude.systemd.services.hennery-host.serviceConfig.User or null == "alice";` → `"host: runs as its user" = false;`. Run `nix build --no-link -L .#checks.x86_64-linux.modules-eval`: it must fail, its output matching `FAIL host: runs as its user`.
  - **hm-doctor-dollar** (Linux, in CI), in `nix/modules/common.nix`: `[ "\\\\" "\\\"" "%%" "$$" ]` → `[ "\\\\" "\\\"" "%%" "$" ]`. Run `nix build --no-link -L .#checks.x86_64-linux.home-manager-doctor`: it must fail, its output matching `Cannot build '[^']*hennery-home-manager-doctor\.drv'`.
  - **hm-doctor-path** (Linux, in CI), in `nix/modules/home-manager.nix`: `PATH=${envQuoted path}` → `PATHS=${envQuoted path}`. Run `nix build --no-link -L .#checks.x86_64-linux.home-manager-doctor`: it must fail, its output matching `5 .*PATH cannot be read`.
  - **vm-host-mode** (Linux, in CI), in `nix/modules/nixos.nix`: `user = cfg.host.user; ⏎         inherit (hostUser) group; ⏎         mode = "0700";` → `user = cfg.host.user; ⏎         inherit (hostUser) group; ⏎         mode = "0750";`. Run `nix build --no-link -L .#checks.x86_64-linux.nixos`: it must fail, its output matching `` = 'alice 700' \]` failed ``.
  - **vm-host-log** (Linux, in CI), in `nix/modules/nixos.nix`: `HENNERY_LOG_DIR = "/var/log/hennery-host";` → `HENNERY_LOG_DIR = "/var/log/hennery-elsewhere";`. Run `nix build --no-link -L .#checks.x86_64-linux.nixos`: it must fail, its output matching `action timed out after`.
  - **vm-doctor** (Linux, in CI), in `crates/hennery/src/service/mod.rs`: `if cx.platform != Platform::Linux { ⏎         return None; ⏎     } ⏎     let text` → `if cx.platform == Platform::Linux { ⏎         return None; ⏎     } ⏎     let text`. Run `nix build --no-link -L .#checks.x86_64-linux.nixos`: it must fail, its output matching `ok +1 binary and adapter set: .*no adapter set is installed`.

- [ ] **Step 8: Commit**

  ```sh
  git add nix flake.nix
  git diff --cached --stat
  git commit -m "feat(nix): the NixOS and home-manager modules, and their checks"
  ```

---

### Task 4: CI

**Files:**
- Modify: `.github/workflows/nix.yml`

- [ ] **Step 1: The workflow**

  In `.github/workflows/nix.yml`, replace:

  ```yaml
  # The Nix flake (distribution spec §4.3, §9; plan 7e-i): `nix flake check`
  # builds the package and runs clippy, rustfmt and cargo-audit through crane,
  # on Linux and on macOS, and the package's binary runs. Nothing is pushed to
  # a binary cache: that would publish. The tests stay `ci.yml`'s.
  #
  ```

  with:

  ```yaml
  # The Nix flake (distribution spec §4.3, §9; plans 7e-i and 7e-ii-a):
  # `nix flake check` builds the package and runs clippy, rustfmt and
  # cargo-audit through crane, checks the pinned adapters and the modules, and
  # on ubuntu runs the NixOS module in a virtual machine; the package's binary
  # runs. Nothing is pushed to a binary cache: that would publish. The tests
  # stay `ci.yml`'s.
  #
  ```

  In `.github/workflows/nix.yml`, replace:

  ```yaml
      runs-on: ${{ matrix.os }}
      # A cold build takes about 9 minutes; a hung one should not run for hours.
      timeout-minutes: 30
      steps:
  ```

  with:

  ```yaml
      runs-on: ${{ matrix.os }}
      # A cold build takes about 9 minutes, and on ubuntu the NixOS test and
      # the adapters add more; a hung one should not run for hours.
      timeout-minutes: 60
      steps:
  ```

  In `.github/workflows/nix.yml`, replace:

  ```yaml
      steps:
        - uses: actions/checkout@v4
          with:
  ```

  with:

  ```yaml
      steps:
        - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4.4.0
          with:
  ```

  In `.github/workflows/nix.yml`, replace:

  ```yaml
            persist-credentials: false
        - uses: cachix/install-nix-action@13d8dd58da0234aa297dedd986986ccb8e7f3e24 # v31.11.1
  ```

  with:

  ```yaml
            persist-credentials: false
        # The NixOS test's virtual machines run under KVM, which the runner's
        # user may not open by default.
        - name: KVM for the NixOS test (ubuntu)
          if: runner.os == 'Linux'
          run: |
            echo 'KERNEL=="kvm", GROUP="kvm", MODE="0666", OPTIONS+="static_node=kvm"' | sudo tee /etc/udev/rules.d/99-kvm4all.rules
            sudo udevadm control --reload-rules
            sudo udevadm trigger --name-match=kvm
            ls -l /dev/kvm
        - uses: cachix/install-nix-action@13d8dd58da0234aa297dedd986986ccb8e7f3e24 # v31.11.1
  ```

  In `.github/workflows/nix.yml`, replace:

  ```yaml
            github_access_token: ${{ github.token }}
        - name: Flake checks (package, clippy, rustfmt, cargo-audit)
          run: nix flake check -L
        - name: The package runs
  ```

  with:

  ```yaml
            github_access_token: ${{ github.token }}
            extra_nix_config: |
              system-features = nixos-test benchmark big-parallel kvm
        - name: Flake checks (package, clippy, rustfmt, cargo-audit, adapters, modules)
          run: nix flake check -L --keep-going
        - name: The package runs
  ```

  In `.github/workflows/nix.yml`, replace:

  ```yaml
          run: nix run . -- --version
  ```

  with:

  ```yaml
          run: nix run . -- --version
        # The Claude adapter is unfree (distribution spec §3.3): built and run
        # here, with the licence accepted for this one command, and never
        # pushed anywhere (the maintainer's decision of 2026-10-02: build and
        # check only). Nothing in this job writes to a cache or uploads. On
        # Linux only, where `autoPatchelfHook` must leave its CLI working;
        # macOS runners are scarce, and there the CLI is unpatched.
        - name: The Claude adapter runs (unfree, built locally only)
          if: always() && runner.os == 'Linux'
          run: NIXPKGS_ALLOW_UNFREE=1 nix build --impure --no-link -L .#claude-acp-runs
        # The systems no runner builds: every output evaluates.
        - name: Every system evaluates
          if: runner.os == 'Linux'
          run: nix flake check --all-systems --no-build
  ```

- [ ] **Step 2: Check it**

  Run: `nix shell nixpkgs#yq-go -c yq '.jobs.flake.steps[].name' .github/workflows/nix.yml`

  Expected: the steps' names, in order: `KVM for the NixOS test (ubuntu)`, `Flake checks (package, clippy, rustfmt, cargo-audit, adapters, modules)`, `The package runs`, `The Claude adapter runs (unfree, built locally only)`, `Every system evaluates` (and `null` for the two `uses:` steps). On the PR, both `flake` jobs pass; the ubuntu job's log shows the virtual machine's subtests and `2.1.280 (Claude Code)`.

- [ ] **Step 3: Commit**

  ```sh
  git add .github/workflows/nix.yml
  git diff --cached --stat
  git commit -m "ci(nix): the NixOS test under KVM, the Claude adapter built locally, every system evaluated"
  ```

---

### Task 5: The spec write-back

**Files:**
- Modify: `docs/specs/2026-09-26-distribution-design.md`

- [ ] **Step 1: §4.3 and §7**

  In `docs/specs/2026-09-26-distribution-design.md`, replace:

  ```markdown
    fetches each tarball for the system with `fetchurl { url; hash = integrity; }`
    and unpacks them, then wraps with nixpkgs' Node 24. On Linux the Claude CLI
    gets `autoPatchelfHook` (to be verified). The Claude adapter derivation is
    marked `unfree` so no public cache ever holds it. *(D-2: the predecessor's
    adapters were network-fetching fixed-output derivations with one hand-kept
    hash per system and no lockfile, so a transitive release could break the hash
    at any time.)*
  - NixOS/home-manager modules: `services.hennery.{collector,host}` with
    `adapters.source = "nix" | "managed"` (default `nix` on NixOS).

  ```

  with:

  ```markdown
    fetches each tarball for the system with `fetchurl { url; hash = integrity; }`
    and unpacks each at its lockfile path under the host installer's rules (§3.2:
    files and directories only, no absolute, `.`, `..` or empty component, no
    name twice; no npm and no install script), then wraps with nixpkgs' Node 24.
    On Linux the Claude CLI gets `autoPatchelfHook` and is never stripped (a
    bun-compiled CLI keeps its program after the ELF); Codex's CLI is
    musl-static and runs as it is. A check runs every bundled native program's
    `--version` and the adapter's ACP `initialize`, offline. The Claude adapter
    derivation is marked `unfree`, so Hydra never builds it, and it is no flake
    package that `nix flake check` evaluates (`legacyPackages.claude-acp`). CI
    builds and runs it on Linux only, never caching or uploading it. Its CLI's
    fetched tarball is a store path with no licence: a cache that uploads every
    path built must stay off a machine that builds this adapter.
    *(D-2: the predecessor's adapters were network-fetching fixed-output
    derivations with one hand-kept hash per system and no lockfile, so a
    transitive release could break the hash at any time.)*
  - **NixOS/home-manager modules:** `services.hennery.{collector,host}` with
    `adapters.source = "nix" | "managed"` (default `nix`), plan 7e-ii-a.
    - `nix` gives each agent of `adapters.agents` (default both) with
      `--agent name=<wrapper>`, so the host installs no managed set. A host with
      `claude` evaluates only once the operator's nixpkgs accepts its licence
      (`allowUnfree`, or `allowUnfreePredicate` for `hennery-claude-acp`).
      `managed` on NixOS needs `programs.nix-ld.enable` (an assertion).
    - **NixOS:** system units. The collector runs as the `hennery` system user,
      in a hardened unit with no capability; the host runs as `host.user`, the
      login user whose agents it runs, unhardened. Each keeps §5.2's policy
      (restart on failure under a start limit; exit 78, revoked, is not
      restarted), `KillMode=mixed`, and logs in its own `LogsDirectory=`. The
      host is skipped until its directory holds `host.key`.
    - **home-manager (Linux):** the user units `service install` writes (§6.3),
      at its paths, with the PATH in its `service.env` (no `SHELL=` line: the
      user manager gives the account's shell).
    - Doctor reads the host's `--agent` from the NixOS system unit too (§7).

  ```

  In `docs/specs/2026-09-26-distribution-design.md`, replace:

  ```markdown
  them to the collector, so the Hosts view shows them without a terminal.

  ---

  ```

  with:

  ```markdown
  them to the collector, so the Hosts view shows them without a terminal.

  On Linux, doctor also reads the NixOS module's system unit,
  `/etc/systemd/system/hennery-host.service` (§4.3), for the `--agent` words its
  `ExecStart` gives the directory checked. They count as a user service's do,
  for every check that reads them (1, 2, 3–4, 9, 12); check 3 starts them with
  doctor's own PATH. No other check judges a system unit, drop-ins in
  `hennery-host.service.d/` are not read, and check 10 looks for user services
  only.

  ---

  ```

- [ ] **Step 2: Commit**

  ```sh
  git add docs/specs/2026-09-26-distribution-design.md
  git diff --cached --stat
  git commit -m "docs(spec): record the Nix adapters and the modules in distribution §4.3 and §7"
  ```

---

## After this plan

- **For frontend 4d** (it builds on doctor's `given_agents` and a `cli: given` source for Nix adapters):
  - `pub fn Doctor::given_agents(&self) -> Option<Vec<(String, hennery_host::AgentCommand)>>` is unchanged in signature and now includes the NixOS system unit's `--agent` words;
  - `pub fn Doctor::agents_given_by_system_unit(&self) -> bool` is new: whether those came from `/etc/systemd/system/hennery-host.service`;
  - `pub(crate) fn service::read_system_command_line(cx, role)` is new, Linux only.
- **7c's own unit and launchd agent** (the parent's PR, Q4): `RestartPreventExitStatus=78`, and launchd's equivalent, for `service install`. This plan's units already have it; `unit.rs` is untouched.
- **Doctor and the system unit:** check 10 still looks for user services only, so on a NixOS host it says no service is installed; check 5 does not read the system unit's PATH, and check 3 starts the unit's agents with doctor's own PATH (the second review's O1). Reading the unit's `Environment="PATH=…"`, and teaching check 10 about system units (active, pointing at this binary), are follow-ups. Drop-ins (`hennery-host.service.d/`) are not read.
- **aarch64-linux** is evaluated in CI, never built: no runner has it.
- **The macOS sandbox:** `adapter-check` runs unsandboxed on macOS; a Mac with `sandbox = true` cannot build `codex-acp-runs`.
- **Codex's voice host** stays unpatched on Linux, so its voice feature does not load on NixOS.
- **Caches:** no binary cache is set up. A cache that uploads every path built would publish the Claude CLI's tarball; the comment in `adapters.nix` says so.
- **home-manager's `SHELL`:** the module's `service.env` has no `SHELL=` line (decision 8). If doctor ever requires one, the module needs a shell option.
- **`nix.yml` and 7e-ii-b:** whichever lands second lists the `web-ui` check in the header and the step name, and rechecks the timeout.

_Generated with Claude AI — please review before distribution._
