# The `hennery` package and its flake checks (plan 7e-i, distribution spec
# §4.3), built with crane on nixpkgs' own Rust toolchain: the one the dev
# shell has, so `nix flake check` and the dev shell's `cargo clippy` agree.
# OpenSSL is nixpkgs', not the release build's vendored copy: a Nix build
# links its dependencies from the store.
{ pkgs, crane, advisory-db, webUi }:
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
      # The web UI's build (`nix/web.nix`, plan 7e-ii-b), embedded by the
      # kernel's build script. Without it the build fails rather than embed
      # the placeholder page. The checks below build without it.
      HENNERY_WEB_DIST = "${webUi}";
      HENNERY_WEB_REQUIRE = "1";
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
    # The binary embeds the web UI, not the placeholder: the release
    # archives' own check (plan 4b), run on the Nix-built binary.
    web-ui = pkgs.runCommand "hennery-web-ui-check" { } ''
      sh ${../packaging/check-web-ui.sh} ${lib.getExe package}
      touch "$out"
    '';
    fmt = craneLib.cargoFmt { inherit (common) src pname version; };
    audit = craneLib.cargoAudit { inherit (common) src pname version; inherit advisory-db; };
  };
}
