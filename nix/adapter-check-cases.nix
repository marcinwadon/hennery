# `adapter-check.nix` judges each outcome as it says (plan 7e-ii-a): fake
# adapters, made of native programs compiled here and a shell script as
# the adapter, each built through the check.
# - Passes: a good one; one whose helper's loader this system lacks
#   (Linux), passed over by name.
# - Fails, each with its reason (`testers.testBuildFailure`): a CLI that
#   fails; a CLI whose `--version` does not name it; a helper that cannot
#   start (Linux); no native program; no answer to `initialize`.
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
      passthru.cliPaths = [ "lib/cli" ];
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
  };
in
runCommand "hennery-adapter-check-cases" { passthru = cases; } ''
  ${lib.concatMapStrings (c: "echo ok: ${c} ${cases.${c}}\n") (lib.attrNames cases)}
  touch "$out"
''
