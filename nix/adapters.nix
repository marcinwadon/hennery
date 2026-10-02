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
