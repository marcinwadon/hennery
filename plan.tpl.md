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

**Status:** {{status}}

## Execution status

{{execution}}

## Scope

7e-ii is split in two:
- **7e-ii-a, this plan:** the adapter derivations from the manifest, the NixOS and home-manager modules, and doctor's reading of the NixOS host unit;
- **7e-ii-b:** the web UI's derivation, embedded in the package (PR #104).

That is **5 tasks:** (1) the adapters and their checks; (2) doctor reads the system unit; (3) the modules and their checks; (4) CI; (5) the spec write-back.

**Cross-lane:**
- **7e-ii-b (PR #104)** changes `flake.nix`'s outputs too. A trial merge gives adjacent-line conflicts there only (its `webUi` binding against `nixOutputs`, `packages.web` against `packages.codex-acp`); both resolve by keeping both sides. Whichever lands second merges them by hand. Its deferred `nix.yml` minor (the header and step name do not list `web-ui`) goes to whichever lands second too.
- **#102 (default data directories)** and **#96 (doctor check 18)** touch `crates/hennery/src/doctor/mod.rs`. {{conflicts}}
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
   - `nix/unpack.sh` judges the listing before it writes anything, under the rules of `crates/hennery-host/src/runtime/extract.rs`: regular files and directories only; no absolute name, no `.`, `..` or empty component; no name twice; a destination that exists is refused. It then strips the first component (`package/`), with no owner and no permissions taken from the archive. After extracting, anything but files and directories fails, as a backstop. (The security review's A1: the first draft piped `tar -t` into `grep -q`, where under `pipefail` a SIGPIPE could turn a refusal into a pass; the listings are now read whole.)
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
   - Exit 127 is passed over only when the program's ELF interpreter does not exist on this system (Codex's voice host). A 127 with the loader present, a library missing, fails.
   - The adapter must answer ACP `initialize` (`"result"` with `"id":0`) within 120 seconds, on a pipe held open until then.
   - No program found fails.
   - Each outcome has its own fake adapter in `adapter-check-cases.nix`, compiled in the build: passes (good; a helper whose loader is missing, Linux) and fails, each with its reason (a CLI that fails; a CLI that does not name itself; a helper that cannot start, Linux; a helper missing a library, Linux; no program; no answer).
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
    - Check 1 then names the unit: "the system unit /etc/systemd/system/hennery-host.service gives its agents with --agent", as check 10 finds no user service to explain it. Check 3 starts those agents. No other check judges a system unit.
11. **Where each check runs.**
    - Everywhere: `codex-acp-runs`, `unpack-refuses`, `adapter-check-cases` (its loader cases on Linux only).
    - Linux: `modules-eval` (its results are computed at evaluation, so it also evaluates on a Mac: `nix eval --raw .#checks.x86_64-linux.modules-eval.buildCommand`), and `home-manager-doctor`, which needs a Linux `hennery`. Gated on the system's name, never on `pkgs.stdenv` (a module's structure gated on `stdenv` recurses).
    - x86_64-linux: `nixos`, the virtual machine, which needs KVM; only the x86_64 runner has it.
    - The home-manager module is evaluated against stand-ins for home-manager's own options: the flake takes no home-manager input.
12. **Tests never touch the building machine's services or data** (fleet rule).
    - `home-manager-doctor` runs the binary as `offline()` in `crates/hennery/tests/cli.rs` does (PR #102): `HOME` and `XDG_{CONFIG,DATA,STATE}_HOME` in the build's own directory, `HENNERY_DATA_DIR`, `HENNERY_HOST_DATA_DIR`, `HENNERY_MASTER_KEY`, `CREDENTIALS_DIRECTORY`, `HENNERY_SERVICE` and `HENNERY_LOG_DIR` unset, the npm registry and Node mirror a port nothing listens on. The build sandbox has no network on Linux either.
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

  {{file:B1:nix/unpack.sh:sh}}

  Create `nix/unpack-check.nix`:

  {{file:B1:nix/unpack-check.nix:nix}}

- [ ] **Step 2: The adapters**

  Create `nix/adapters.nix`:

  {{file:B1:nix/adapters.nix:nix}}

- [ ] **Step 3: The run check, and its cases**

  Create `nix/adapter-check.nix`:

  {{file:B1:nix/adapter-check.nix:nix}}

  Create `nix/adapter-check-cases.nix`:

  {{file:B1:nix/adapter-check-cases.nix:nix}}

- [ ] **Step 4: The flake's outputs**

  Create `nix/outputs.nix`:

  {{file:B1:nix/outputs.nix:nix}}

  {{hunks:BASE:B1:flake.nix:nix}}

- [ ] **Step 5: Build the checks**

  Run:

  ```sh
  git add nix flake.nix
  nix build --no-link -L .#checks.<system>.unpack-refuses .#checks.<system>.adapter-check-cases .#checks.<system>.codex-acp-runs
  nix flake check --all-systems --no-build
  ```

  Expected: both exit 0. `unpack-refuses` prints `ok: <name> refused: …` for each of the eight bad tarballs, then `ok: good unpacked without its first component` and `ok: an existing destination refused`. `codex-acp-runs` prints each native program it runs and the adapter's answer, `{"jsonrpc":"2.0","id":0,"result":{…"agentInfo":{"name":"@agentclientprotocol/codex-acp"…`. On Linux (CI), `adapter-check-cases` builds its three Linux cases too, and `NIXPKGS_ALLOW_UNFREE=1 nix build --impure --no-link -L .#claude-acp-runs` prints `2.1.280 (Claude Code)` and the adapter's answer.

- [ ] **Step 6: The revert-probes**

  {{probes:1}}

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

  {{hunks:B1:B2:crates/hennery/src/doctor/tests.rs:rust}}

  Run: `nix develop -c cargo test -p hennery --locked --bin hennery -- doctor::tests::the_nixos_modules_system_unit_gives_its_agents doctor::tests::a_user_services_agents_are_the_services`

  Expected: both compile; `the_nixos_modules_system_unit_gives_its_agents` fails at its first assertion after the unit is written (`assert_eq!(check.status, Status::Ok …)` passes, then on Linux check 1 still says "no adapter set is installed"); `a_user_services_agents_are_the_services` passes.

- [ ] **Step 2: The system unit's command line**

  {{hunks:B1:B2:crates/hennery/src/service/mod.rs:rust}}

- [ ] **Step 3: Doctor reads it, and check 1 names it**

  {{hunks:B1:B2:crates/hennery/src/doctor/mod.rs:rust}}

  {{hunks:B1:B2:crates/hennery/src/doctor/runtime.rs:rust}}

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

  {{probes:2}}

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

  {{file:B3:nix/modules/common.nix:nix}}

- [ ] **Step 2: The NixOS module**

  Create `nix/modules/nixos.nix`:

  {{file:B3:nix/modules/nixos.nix:nix}}

- [ ] **Step 3: The home-manager module**

  Create `nix/modules/home-manager.nix`:

  {{file:B3:nix/modules/home-manager.nix:nix}}

- [ ] **Step 4: The checks**

  Create `nix/tests/modules.nix`:

  {{file:B3:nix/tests/modules.nix:nix}}

  Create `nix/tests/nixos.nix`:

  {{file:B3:nix/tests/nixos.nix:nix}}

- [ ] **Step 5: The flake's outputs**

  Replace the whole of `nix/outputs.nix` with:

  {{file:B3:nix/outputs.nix:nix}}

  {{hunks:B2:B3:flake.nix:nix}}

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

  {{probes:3}}

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

  {{hunks:B3:B4:.github/workflows/nix.yml:yaml}}

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

  {{hunks:B4:B5:docs/specs/2026-09-26-distribution-design.md:markdown}}

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
- **Doctor and the system unit:** check 10 still looks for user services only, so on a NixOS host it says no service is installed; check 5 does not read the system unit's PATH. Teaching check 10 about system units (active, pointing at this binary) is a follow-up. Drop-ins (`hennery-host.service.d/`) are not read.
- **aarch64-linux** is evaluated in CI, never built: no runner has it.
- **The macOS sandbox:** `adapter-check` runs unsandboxed on macOS; a Mac with `sandbox = true` cannot build `codex-acp-runs`.
- **Codex's voice host** stays unpatched on Linux, so its voice feature does not load on NixOS.
- **Caches:** no binary cache is set up. A cache that uploads every path built would publish the Claude CLI's tarball; the comment in `adapters.nix` says so.
- **home-manager's `SHELL`:** the module's `service.env` has no `SHELL=` line (decision 8). If doctor ever requires one, the module needs a shell option.
- **`nix.yml` and 7e-ii-b:** whichever lands second lists the `web-ui` check in the header and the step name, and rechecks the timeout.

_Generated with Claude AI — please review before distribution._
