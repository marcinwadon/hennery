#!/bin/sh
# Both native release and PR proof candidates carry the same current audits.
set -eu
nix develop .#release -c cargo-audit audit --file Cargo.lock --db "$RUNNER_TEMP/release-advisories" --json > release-materials/rust-advisories.json
git -C "$RUNNER_TEMP/release-advisories" rev-parse HEAD > release-materials/rust-advisory-revision.txt
nix develop .#release -c python3 packaging/audit_web.py > release-materials/web-advisories.json
nix develop .#web -c sh packaging/audit-adapters.sh
