# The web UI's build (plan 7e-ii-b, distribution spec §4.3), from
# nixpkgs-web: the Node and pnpm the dev shell builds it with. The package
# embeds this output through `HENNERY_WEB_DIST`.
#
# The dependencies are one fixed-output derivation, fetched from the npm
# registry by `fetchPnpmDeps` at the lock's integrity hashes. Its own hash
# is the same on every system: the fetcher installs with `--force`, which
# takes every platform's optional packages, not only this system's. The
# build itself is offline, from that store.
#
# After any change to web/pnpm-lock.yaml: `sh packaging/update-web-hash.sh`.
{ pkgs }:
let
  inherit (pkgs) lib;
  # The major `web/pnpm-lock.yaml` was written by. A new major is a
  # deliberate change here, and a new dependency hash.
  pnpm = pkgs.pnpm_12;
  # Everything under `web/` that is not a build or test output, so a file
  # the frontend adds needs no change here.
  src = lib.fileset.toSource {
    root = ../web;
    fileset = lib.fileset.difference ../web (
      lib.fileset.unions [
        (lib.fileset.maybeMissing ../web/node_modules)
        (lib.fileset.maybeMissing ../web/dist)
        (lib.fileset.maybeMissing ../web/test-results)
        (lib.fileset.maybeMissing ../web/playwright-report)
        (lib.fileset.fileFilter (file: file.hasExt "tsbuildinfo") ../web)
      ]
    );
  };
in
pkgs.stdenvNoCC.mkDerivation (finalAttrs: {
  pname = "hennery-web";
  inherit ((lib.importJSON ../web/package.json)) version;
  inherit src;

  pnpmDeps = pkgs.fetchPnpmDeps {
    inherit (finalAttrs) pname version src;
    inherit pnpm;
    # The only version this nixpkgs' fetcher takes for pnpm 11 and newer
    # (D-1: a version nixpkgs removes fails evaluation, it is never kept).
    fetcherVersion = 4;
    # A workaround for the fetcher of nixpkgs-web at e158d9e with pnpm 12:
    # pnpm 12 keeps every package unpacked under `v11/links`, beside the
    # content-addressed `files` it is made from, and the fetcher's fixup
    # rewrites every `*.json` in the store with jq. It stops at the first
    # package file that is not strict JSON (a commented
    # `tsdoc-metadata.json`). The offline install rebuilds `links` from
    # `files`, so it is dropped before that fixup: half the size, and no
    # package file edited. Remove this once the fixup skips `links`; the
    # hash changes then.
    preFixup = ''
      [ -d "''${storePath:?}/v11" ] || { echo "no v11 store: revisit this workaround" >&2; exit 1; }
      rm -rf "''${storePath:?}/v11/links"
    '';
    # Changes with `web/pnpm-lock.yaml`: after a lock change, run
    # `sh packaging/update-web-hash.sh`. A stale hash fails CI with a hash
    # mismatch in `hennery-web-pnpm-deps`. A store that already holds the
    # old output never refetches it, so locally the build fails instead,
    # with ERR_PNPM_NO_OFFLINE_TARBALL.
    hash = "sha256-5p7sGn/gcxYo5HKokMqyUG0Y25POOVnJiNFyi8I4vxI=";
  };

  nativeBuildInputs = [
    pkgs.nodejs_24
    pnpm
    pkgs.pnpmConfigHook
  ];

  buildPhase = ''
    runHook preBuild
    pnpm build
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    # The page loads the entry script the build made, not a stale name. The
    # pattern is the tag Vite writes today: after a Vite upgrade, a failure
    # here may mean a new format rather than a bad build.
    entry=$(sed -n 's|.*<script type="module" crossorigin src="/\(assets/[^"]*\.js\)".*|\1|p' dist/index.html)
    if [ -z "$entry" ] || [ ! -f "dist/$entry" ]; then
      echo "dist/index.html loads no script the build made" >&2
      exit 1
    fi
    cp -r dist "$out"
    runHook postInstall
  '';

  # The build is embedded in the binary: it must name no store path.
  allowedReferences = [ ];

  # The dev shell takes the same pnpm (`flake.nix`).
  passthru = { inherit pnpm; };
})
