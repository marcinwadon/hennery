#!/usr/bin/env python3
"""Revert-probes of plan 7e-ii-a: each removes or breaks one guarded line,
runs the one check that guards it, and must see that check fail with its
reason. The file is restored after each probe, whatever happens.

Usage: probes.py <repo> mac|linux [name ...]

Every command runs with HOME, XDG_*, npm, pnpm and cargo directories in a
scratch directory of its own (fleet rule, 2026-10-02).
"""

import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

LINUX = "x86_64-linux"
MAC = "aarch64-darwin"


def build(system, attr, unfree=False):
    if unfree:
        return ["nix", "build", "--impure", "--no-link", "-L", f".#legacyPackages.{system}.{attr}"]
    return ["nix", "build", "--no-link", "-L", f".#checks.{system}.{attr}"]


EVAL = ["nix", "eval", "--raw", f".#checks.{LINUX}.modules-eval.buildCommand"]


def rust(*tests):
    return ["nix", "develop", "-c", "cargo", "test", "-p", "hennery", "--locked", "--bin", "hennery", "--", *tests]


SYSTEM_UNIT_TEST = "doctor::tests::the_nixos_modules_system_unit_gives_its_agents"
USER_SERVICE_TEST = "doctor::tests::a_user_services_agents_are_the_services"

# name, where it runs, file, old, new, command, how the run must end
# ("fail" or "pass"), and a regex its output must match.
P = []


def probe(name, where, path, old, new, cmd, ends, expect):
    P.append(dict(name=name, where=where, path=path, old=old, new=new, cmd=cmd, ends=ends, expect=expect))


def evalfail(name, path, old, new, result):
    probe(name, "mac", path, old, new, EVAL, "pass", r"echo 'FAIL " + re.escape(result) + "'")


# --- Task 1: unpack.sh, against unpack-refuses (portable) ---------------
U = "nix/unpack.sh"
for name, old, new, expect in [
    # With the listing's rule gone, the backstop after extraction refuses.
    ("unpack-links", "if grep -qv '^[-d]' <<< \"$listing\"; then", "if false; then", "symlink.tgz: extracted an entry that is neither a file nor a directory"),
    ("unpack-names", "if grep -qE '^/|(^|/)\\.{1,2}(/|$)|//' <<< \"$names\"; then", "if false; then", "FAIL: dotdot was unpacked"),
    ("unpack-twice", "if [ -n \"$(sed 's|/$||' <<< \"$names\" | sort | uniq -d)\" ]; then", "if false; then", "FAIL: twice was unpacked"),
    ("unpack-exists", "if [ -e \"$dest\" ]; then", "if false; then", "an existing destination was unpacked into"),
    ("unpack-strip", " --strip-components=1", "", "hennery: good.tgz: "),
]:
    probe(name, "mac", U, old, new, build(MAC, "unpack-refuses"), "fail", re.escape(expect))
# Both the listing's rule and the backstop gone: a link is unpacked.
probe("unpack-backstop", "mac", U,
      ["if grep -qv '^[-d]' <<< \"$listing\"; then", "if [ -n \"$(find \"$dest\" ! -type f ! -type d)\" ]; then"],
      ["if false; then", "if false; then"], build(MAC, "unpack-refuses"), "fail", re.escape("FAIL: symlink was unpacked"))

# --- Task 1: adapter-check.nix, against adapter-check-cases -------------
NOT_FAILED = re.escape("testBuildFailure: The builder did not fail, but a failure was expected!")
A = "nix/adapter-check.nix"
for name, old, new, case in [
    ("check-cli-status", "claude:* | codex:* | rg:*) echo \"$program failed: $status\" >&2; exit 1 ;;", "claude:* | codex:* | rg:*) ;;", "cli-fails"),
    ("check-cli-name", "if [ \"$status\" -eq 0 ] && [ -n \"$name\" ] && ! grep -qF \"$name\" <<< \"$said\"; then", "if false; then", "cli-unnamed"),
    ("check-no-program", "if [ \"$programs\" -eq 0 ]; then", "if false; then", "no-program"),
    ("check-initialize", "if ! grep '\"result\"' \"$TMPDIR/answer\" | grep -q '\"id\":0'; then", "if false; then", "no-answer"),
]:
    probe(name, "mac", A, old, new, build(MAC, f"adapter-check-cases.{case}"), "fail", NOT_FAILED)
for name, old, new, case, expect in [
    ("check-loader-missing", "echo \"passed over: $program needs the loader $interpreter, which this system does not have\"\n              continue", "exit 1", "loader-missing", r"adapter-check-passes-loader-missing"),
    ("check-cannot-start", "*:124 | *:12[6-9] | *:1[3-9]? | *:2??) echo \"$program did not run: $status\" >&2; exit 1 ;;", "*:124) exit 1 ;;", "helper-cannot-start", NOT_FAILED),
    ("check-patched-loader", "              if ${lib.boolToString (adapter.patched or false)}; then\n                echo \"$program needs the loader $interpreter, which this system does not have: autoPatchelfHook left it unpatched\" >&2\n                exit 1\n              fi\n", "", "loader-missing-patched", NOT_FAILED),
    ("check-library-missing", "echo \"$program did not run: $status (loader: ''${interpreter:-none})\" >&2\n            exit 1", "continue", "library-missing", NOT_FAILED),
]:
    probe(name, "linux", A, old, new, build(LINUX, f"adapter-check-cases.{case}"), "fail", expect)

# --- Task 1: the adapters themselves -----------------------------------
D = "nix/adapters.nix"
probe("codex-entry", "mac", D, '--add-flags "$out/${root}/${pinned.entry}"', '--add-flags "$out/${root}/nothing.js"',
      build(MAC, "codex-acp-runs"), "fail", r"did not answer initialize")
probe("claude-strip", "linux", D, "dontStrip = true;", "dontStrip = false;",
      build(LINUX, "claude-acp-runs", unfree=True), "fail", r"does not say \(Claude Code\)|claude failed")
probe("claude-patchelf", "linux", D, "patchElf = glibc && stdenv.hostPlatform.isElf;", "patchElf = false;",
      build(LINUX, "claude-acp-runs", unfree=True), "fail", r"claude failed: 127")

# --- Task 2: doctor reads the system unit (Rust) -----------------------
S = "crates/hennery/src/service/mod.rs"
M = "crates/hennery/src/doctor/mod.rs"
R = "crates/hennery/src/doctor/runtime.rs"
probe("rust-read", "mac", S, 'let text = std::fs::read_to_string(cx.root.join("etc/systemd/system").join(role.unit())).ok()?;',
      'let text = std::fs::read_to_string(cx.root.join("etc/systemd/system").join(role.unit())).ok().filter(|_| false)?;',
      rust(SYSTEM_UNIT_TEST), "fail", re.escape(SYSTEM_UNIT_TEST) + r" \.\.\. FAILED")
probe("rust-linux-only", "mac", S, "if cx.platform != Platform::Linux {\n        return None;\n    }\n    let text",
      "let text", rust(SYSTEM_UNIT_TEST), "fail", re.escape(SYSTEM_UNIT_TEST) + r" \.\.\. FAILED")
probe("rust-named", "mac", R, "Ok(None) if doctor.agents_given_by_system_unit() =>", "Ok(None) if false =>",
      rust(SYSTEM_UNIT_TEST), "fail", re.escape(SYSTEM_UNIT_TEST) + r" \.\.\. FAILED")
probe("rust-user-first", "mac", M, "user.chain(system).find_map", "system.into_iter().chain(user).find_map",
      rust(USER_SERVICE_TEST), "fail", re.escape(USER_SERVICE_TEST) + r" \.\.\. FAILED")
probe("rust-no-agent", "mac", M, "(!given.is_empty()).then_some((system, given))", "Some((system, given))",
      rust(SYSTEM_UNIT_TEST), "fail", re.escape(SYSTEM_UNIT_TEST) + r" \.\.\. FAILED")
probe("rust-its-directory", "mac", M, ".map(|argv| (true, Role::Host, argv));", ".map(|argv| (true, Role::Up, argv));",
      rust(SYSTEM_UNIT_TEST), "fail", re.escape(SYSTEM_UNIT_TEST) + r" \.\.\. FAILED")
probe("rust-system-flag", "mac", M, "self.given().is_some_and(|(system, _)| system)", "self.given().is_some()",
      rust(USER_SERVICE_TEST), "fail", re.escape(USER_SERVICE_TEST) + r" \.\.\. FAILED")

# --- Task 3: the modules, by evaluation (runs on the Mac) ---------------
C = "nix/modules/common.nix"
N = "nix/modules/nixos.nix"
H = "nix/modules/home-manager.nix"
O = "nix/outputs.nix"
evalfail("eval-agent", C, '"--agent"\n        "${name}=', '"--agen"\n        "${name}=', "nix: the agents given with --agent")
# With `managed` given the Nix adapters, its default agents need the
# Claude licence: the evaluation itself fails.
probe("eval-managed", "mac", C, 'lib.optionals (cfg.adapters.source == "nix") (', 'lib.optionals true (', EVAL, "fail",
      r"Refusing to evaluate package 'hennery-claude-acp-[0-9.]+' .* because it has an unfree license")
evalfail("eval-percent", C, '[ "\\\\\\\\" "\\\\\\"" "%%" "$$" ]', '[ "\\\\\\\\" "\\\\\\"" "%" "$$" ]', "home-manager: every word quoted as unit.rs quotes it")
evalfail("eval-dollar", C, '[ "\\\\\\\\" "\\\\\\"" "%%" "$$" ]', '[ "\\\\\\\\" "\\\\\\"" "%%" "$" ]', "home-manager: every word quoted as unit.rs quotes it")
evalfail("eval-listen", C, '"--listen"\n      address', '"--listn"\n      address', "collector: its command line")
evalfail("eval-public-url", C, '"--public-url"\n      cfg.publicUrl', '"--public-ur"\n      cfg.publicUrl', "collector: its command line")
evalfail("eval-nixos-revoked", N, "RestartPreventExitStatus = 78;", "", "host: a revoked host is not restarted")
evalfail("eval-nixos-paired", N, 'unitConfig.ConditionPathExists = common.unitPath "${cfg.host.dataDir}/host.key";', "", "host: skipped until paired")
evalfail("eval-nixos-user", N, "User = cfg.host.user;", "", "host: runs as its user")
evalfail("eval-nix-ld", N, 'assertion = cfg.host.adapters.source != "managed" || config.programs.nix-ld.enable;', "assertion = true;", "managed: refused without nix-ld")
evalfail("eval-normal-user", N, "assertion = hostUser.isNormalUser;", "assertion = true;", "a user who is not there is refused")
evalfail("eval-collector-user", N, 'User = "hennery";', "", "collector: its own system user")
evalfail("eval-unfree", D, "license = lib.licenses.unfree;", "license = lib.licenses.mit;", "nix: claude refused without the licence accepted")
evalfail("eval-hm-revoked", H, "Service.RestartPreventExitStatus = 78;", "", "home-manager: skipped until paired, not restarted once revoked")
evalfail("eval-hm-paired", H, 'Unit.ConditionPathExists = common.unitPath "${cfg.host.dataDir}/host.key";', "", "home-manager: skipped until paired, not restarted once revoked")
evalfail("eval-hm-envfile", H, "EnvironmentFile = common.unitPath envFile;", "", "home-manager: the unit reads it")
evalfail("eval-hm-path", H, '      "/usr/bin"\n', "", "home-manager: the PATH in service.env, in its quoting")
evalfail("eval-hm-linux", H, "assertion = pkgs.stdenv.hostPlatform.isLinux;", "assertion = false;", "home-manager: no assertion fails on Linux")
# The check itself: a FAIL ends its build (the `exit 1` line's guard).
probe("eval-fail-fails", "linux", "nix/tests/modules.nix",
      '"host: runs as its user" = nixNoClaude.systemd.services.hennery-host.serviceConfig.User or null == "alice";',
      '"host: runs as its user" = false;', build(LINUX, "modules-eval"), "fail", r"FAIL host: runs as its user")

# --- Task 3: Linux builds of the modules' checks -----------------------
HM = "home-manager-doctor"
probe("hm-doctor-dollar", "linux", C, '[ "\\\\\\\\" "\\\\\\"" "%%" "$$" ]', '[ "\\\\\\\\" "\\\\\\"" "%%" "$" ]',
      build(LINUX, HM), "fail", r"Cannot build '[^']*hennery-home-manager-doctor\.drv'")
probe("hm-doctor-path", "linux", H, "PATH=${envQuoted path}", "PATHS=${envQuoted path}",
      build(LINUX, HM), "fail", r"5 .*PATH cannot be read")
probe("vm-host-mode", "linux", N, 'user = cfg.host.user;\n        inherit (hostUser) group;\n        mode = "0700";',
      'user = cfg.host.user;\n        inherit (hostUser) group;\n        mode = "0750";', build(LINUX, "nixos"), "fail", r"= 'alice 700' \]` failed")
probe("vm-host-log", "linux", N, 'HENNERY_LOG_DIR = "/var/log/hennery-host";', 'HENNERY_LOG_DIR = "/var/log/hennery-elsewhere";',
      build(LINUX, "nixos"), "fail", r"action timed out after")
probe("vm-doctor", "linux", S, "if cx.platform != Platform::Linux {\n        return None;\n    }\n    let text",
      "if cx.platform == Platform::Linux {\n        return None;\n    }\n    let text", build(LINUX, "nixos"), "fail",
      r"ok +1 binary and adapter set: .*no adapter set is installed")


def scratch_env(root):
    env = dict(os.environ)
    home = root / "home"
    for d in ["home", "config", "data", "state", "cache", "cargo", "npm-cache", "pnpm-store"]:
        (root / d).mkdir(parents=True, exist_ok=True)
    env.update(
        HOME=str(home),
        XDG_CONFIG_HOME=str(root / "config"),
        XDG_DATA_HOME=str(root / "data"),
        XDG_STATE_HOME=str(root / "state"),
        XDG_CACHE_HOME=str(root / "cache"),
        npm_config_userconfig=str(root / "npmrc"),
        npm_config_cache=str(root / "npm-cache"),
        pnpm_config_store_dir=str(root / "pnpm-store"),
        pnpm_config_cache_dir=str(root / "cache" / "pnpm"),
        CARGO_HOME=str(root / "cargo"),
        CARGO_TARGET_DIR=os.environ.get("PROBE_TARGET_DIR", str(root / "target")),
    )
    for var in ["HENNERY_DATA_DIR", "HENNERY_HOST_DATA_DIR", "HENNERY_MASTER_KEY", "HENNERY_SERVICE", "HENNERY_LOG_DIR"]:
        env.pop(var, None)
    return env


def main():
    repo, where, names = Path(sys.argv[1]), sys.argv[2], sys.argv[3:]
    os.chdir(repo)
    if subprocess.run(["git", "status", "--porcelain"], capture_output=True, text=True).stdout.strip():
        raise SystemExit("the tree is not clean")
    root = Path(os.environ.get("PROBE_SCRATCH") or tempfile.mkdtemp(prefix="7e2a-probes-"))
    env = scratch_env(root)
    chosen = [p for p in P if p["where"] == where and (not names or p["name"] in names)]
    missed = []
    for p in chosen:
        path = Path(p["path"])
        original = path.read_text()
        olds = p["old"] if isinstance(p["old"], list) else [p["old"]]
        news = p["new"] if isinstance(p["new"], list) else [p["new"]]
        changed = original
        broken = [o for o in olds if original.count(o) != 1]
        if broken:
            print(f"BROKEN {p['name']}: a line to change is not in {path} exactly once: {broken}", flush=True)
            missed.append(p["name"])
            continue
        for o, n in zip(olds, news):
            changed = changed.replace(o, n)
        try:
            path.write_text(changed)
            # The unfree adapter's licence, accepted for its one command.
            run_env = dict(env, NIXPKGS_ALLOW_UNFREE="1") if "--impure" in p["cmd"] else env
            run = subprocess.run(p["cmd"], capture_output=True, text=True, env=run_env)
        finally:
            path.write_text(original)
        out = run.stdout + run.stderr
        ended = "pass" if run.returncode == 0 else "fail"
        ok = ended == p["ends"] and re.search(p["expect"], out)
        log = root / f"{p['name']}.log"
        log.write_text(out)
        print(f"{'CAUGHT' if ok else 'MISSED'} {p['name']} (exit {run.returncode}; log {log})", flush=True)
        if not ok:
            missed.append(p["name"])
            print(out[-3000:], flush=True)
    print(f"{len(chosen) - len(missed)} of {len(chosen)} caught; missed: {missed}", flush=True)
    sys.exit(1 if missed else 0)


if __name__ == "__main__":
    main()
