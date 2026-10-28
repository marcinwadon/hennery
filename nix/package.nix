# The `hennery` package and its flake checks (plan 7e-i, distribution spec
# §4.3), built with crane on nixpkgs' own Rust toolchain: the one the dev
# shell has, so `nix flake check` and the dev shell's `cargo clippy` agree.
# OpenSSL is nixpkgs', not the release build's vendored copy: a Nix build
# links its dependencies from the store.
{ pkgs, crane, advisory-db, webUi, sourceRevision ? "unrecorded (development build)" }:
let
  inherit (pkgs) lib stdenv;
  craneLib = crane.mkLib pkgs;
  workspace = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).workspace.package;
  # Only what cargo reads: the crates, the manifests and the lock, and the
  # files a crate compiles in with `include_str!`. The adapter manifest
  # is included at either layout across the R3 crate-packaging transition.
  # The shared release helper rejects differing bytes when both are present.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      (craneLib.fileset.commonCargoSources ../.)
      # Crane's common set omits SQL read by Rust's include_str!.
      ../crates/hennery-sessions/src/delegation_schema.sql
      ../crates/hennery-sessions/src/delegation_delivery.sql
      ../crates/hennery-sessions/src/delegation_attention.sql
      ../crates/hennery-sessions/src/family.sql
      ../crates/hennery-sessions/src/idle_hold.sql
      ../crates/hennery-sessions/src/runtime_catalog.sql
      ../crates/hennery-sessions/src/prompt_queue.sql
      ../crates/hennery-sessions/src/review.sql
      ../crates/hennery-sessions/src/review_child_backfill.sql
      ../crates/hennery-sessions/tests/fixtures/undo_family.sql
      ../crates/hennery-sessions/tests/fixtures/session_tool_contract.json
      ../crates/hennery-memory/src/schema.sql
      ../crates/hennery-memory/src/schema2.sql
      ../crates/hennery-memory/src/schema3.sql
      ../crates/hennery-memory/src/schema4.sql
      ../crates/hennery-memory/src/schema5.sql
      ../crates/hennery-memory/src/ui-client.mjs
      ../crates/hennery-memory/tests/ui-client.test.mjs
      ../crates/hennery-sessions/tests/fixtures/undo_delegation.sql
      ../crates/hennery-sessions/tests/fixtures/undo_delegation_limits.sql
      ../crates/hennery-testkit/tests/fixtures/delegation-codex-acp-1.13.0.jsonl
      ../crates/hennery-testkit/tests/support/codex_mcp_probe.mjs
      ../crates/hennery-view/tests/fixtures
      (lib.fileset.maybeMissing ../adapters/manifest.json)
      (lib.fileset.maybeMissing ../crates/hennery-host/manifest.json)
    ];
  };
  common = {
    inherit src;
    strictDeps = true;
    cargoExtraArgs = "--workspace --no-default-features";
    pname = "hennery";
    inherit (workspace) version;
    nativeBuildInputs = [ pkgs.pkg-config ];
    buildInputs = [ pkgs.openssl ] ++ lib.optionals stdenv.hostPlatform.isDarwin [ pkgs.libiconv ];
  };
  # Match the shipped package's feature graph. Tests and workspace lint run
  # outside Nix in CI; compiling their dependencies here duplicates that work.
  packageArgs = "--locked -p hennery --no-default-features";
  cargoArtifacts = craneLib.buildDepsOnly (common // {
    cargoExtraArgs = packageArgs;
    buildPhaseCargoCommand = "cargoWithProfile build ${packageArgs}";
    doCheck = false;
  });
  package = craneLib.buildPackage (
    common
    // {
      inherit cargoArtifacts;
      cargoExtraArgs = packageArgs;
      # The web UI's build (`nix/web.nix`, plan 7e-ii-b), embedded by the
      # kernel's build script. Without it the build fails rather than embed
      # the placeholder page. The checks below build without it.
      HENNERY_WEB_DIST = "${webUi}";
      HENNERY_WEB_REQUIRE = "1";
      HENNERY_BUILD_SOURCE_SHA = sourceRevision;
      nativeBuildInputs = common.nativeBuildInputs ++ [ pkgs.python3 ];
      # The same tree digest as R1's candidate metadata, with no run-time
      # reference to Python or the UI derivation left in the executable.
      preBuild = ''
        export HENNERY_BUILD_UI_SHA256="$(${pkgs.python3}/bin/python3 - '${webUi}' <<'PY'
        import hashlib, json, pathlib, sys
        root = pathlib.Path(sys.argv[1])
        files = [{'name': str(p.relative_to(root)), 'sha256': hashlib.sha256(p.read_bytes()).hexdigest()}
                 for p in sorted(root.rglob('*')) if p.is_file()]
        raw = (json.dumps(files, sort_keys=True, separators=(',', ':'), ensure_ascii=True) + '\n').encode()
        print(hashlib.sha256(raw).hexdigest())
        PY
        )"
        HENNERY_BUILD_MANIFEST_SHA256="$(${pkgs.python3}/bin/python3 ${../packaging/release/adapter_manifest.py} --sha256 ${src})"
        export HENNERY_BUILD_MANIFEST_SHA256
      '';
      # The UI is in the binary, so the package never needs it at run time.
      disallowedReferences = [ webUi ];
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
    memory-ui = pkgs.runCommand "hennery-memory-ui-check" { nativeBuildInputs = [ pkgs.nodejs_24 ]; } ''
      node --check ${src}/crates/hennery-memory/src/ui-client.mjs
      node --test ${src}/crates/hennery-memory/tests/ui-client.test.mjs
      touch "$out"
    '';
    inherit package;
    source-inputs = pkgs.runCommand "hennery-source-inputs" { nativeBuildInputs = [ pkgs.python3 ]; } ''
      python ${../packaging}/test-nix-sources.py
      python ${../packaging}/check-nix-sources.py --root ${../.} --filtered ${src}
      touch "$out"
    '';
    clippy = craneLib.cargoClippy (
      common
      // {
        inherit cargoArtifacts;
        # Crane appends these to common.cargoExtraArgs; scope/features occur once.
        cargoClippyExtraArgs = "--all-targets -- -D warnings";
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
