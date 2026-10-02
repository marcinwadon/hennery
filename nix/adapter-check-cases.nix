# `adapter-check.nix` judges each outcome as it says (plan 7e-ii-a): fake
# adapters, made of programs compiled here (one exits 0, one 1) and a shell
# script as the adapter, each built through the check. A good one passes, and so does one whose helper's
# loader this system lacks (Linux), by name. One whose CLI fails, one whose
# helper cannot start (Linux), one with no native program, and one that
# does not answer `initialize` each fail with their reason
# (`testers.testBuildFailure`).
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
  # A fake adapter: `programs` (name = command to make it) in its CLI's
  # package, and `adapter` as its wrapper.
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
  # A native program that exits with `code`, whatever its arguments.
  exits = code: target: "echo 'int main(void) { return ${toString code}; }' | $CC -x c -o ${target} -";
  # An ELF whose loader is somewhere else: missing, or not a loader at all.
  loader = path: target: "${exits 0 target}; patchelf --set-interpreter ${path} ${target}";
  passes = name: drv: (adapterCheck drv).overrideAttrs { name = "adapter-check-passes-${name}"; };
  fails =
    name: drv: reason:
    runCommand "adapter-check-fails-${name}" { failed = testers.testBuildFailure (adapterCheck drv); } ''
      grep -q ${lib.escapeShellArg reason} "$failed/testBuildFailure.log" || {
        echo "${name}: expected ${lib.escapeShellArg reason}, got:"; cat "$failed/testBuildFailure.log"; exit 1;
      }
      touch "$out"
    '';
  cases = {
    good = passes "good" (fake "good" { programs.claude = exits 0; });
    cli-fails = fails "cli-fails" (fake "cli-fails" { programs.codex = exits 1; }) "failed: 1";
    no-program = fails "no-program" (fake "no-program" { programs = { }; }) "no native program found";
    no-answer = fails "no-answer" (fake "no-answer" {
      programs.rg = exits 0;
      adapter = silent;
    }) "did not answer initialize";
  }
  // lib.optionalAttrs linux {
    loader-missing = passes "loader-missing" (fake "loader-missing" {
      programs = {
        claude = exits 0;
        helper = loader "/nonexistent/ld-musl-x86_64.so.1";
      };
    });
    helper-cannot-start = fails "helper-cannot-start" (fake "helper-cannot-start" {
      programs = {
        claude = exits 0;
        helper = loader "${coreutils}/bin/true";
      };
    }) "did not run";
  };
in
runCommand "hennery-adapter-check-cases" { passthru = cases; } ''
  ${lib.concatMapStrings (c: "echo ok: ${c} ${cases.${c}}\n") (lib.attrNames cases)}
  touch "$out"
''
