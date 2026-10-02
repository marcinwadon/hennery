# The web UI's build (plan 7e-ii-b, distribution spec §4.3), from
# nixpkgs-web: the Node and pnpm the dev shell builds it with. The package
# embeds this output through `HENNERY_WEB_DIST`.
#
# The dependencies are one fixed-output derivation, fetched from the npm
# registry by `fetchPnpmDeps` at the lock's integrity hashes. Its own hash
# is the same on every system: the fetcher installs with `--force`, which
# takes every platform's optional packages, not only this system's. The
# build itself is offline, from that store.
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
    # pnpm 12 keeps every package unpacked under `v11/links`, beside the
    # content-addressed `files` it is made from. The fetcher's fixup then
    # rewrites every `*.json` there with jq, and stops at the first package
    # file that is not strict JSON (a commented `tsdoc-metadata.json`).
    # `links` is rebuilt from `files` by the offline install, so it is
    # dropped before that fixup: half the size, and no package file edited.
    preFixup = ''
      rm -rf "''${storePath:?}/v11/links"
    '';
    # Changes with `web/pnpm-lock.yaml`. After a lock change, set it to
    # `lib.fakeHash`, run `nix build .#web.pnpmDeps`, and copy the hash it
    # got. A store that already holds the old output never refetches it,
    # so the offline build is what fails there, with
    # ERR_PNPM_NO_OFFLINE_TARBALL.
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
    cp -r dist "$out"
    runHook postInstall
  '';
})
