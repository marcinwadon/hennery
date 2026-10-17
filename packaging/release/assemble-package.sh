#!/bin/sh
# Common read-only assembly: same transported native/crate/image bytes in PRs and tags.
set -eu
sh packaging/install-dist.sh "$RUNNER_TEMP/dist-bin"
python3 packaging/release/assemble.py candidates target/distrib --targets "$PACKAGING_TARGETS"
if [ "$PACKAGING_CRATES" = true ]; then
    nix develop -c node packaging/prepare-crates.mjs --stage crates-flat target/distrib
fi
# cargo-dist imports every *dist-manifest.json. Its stdout must stay outside
# the input directory until generation succeeds (alpha.3 regression).
# A PR has Linux inputs only; cargo-dist must offer only the archives built.
set --
for target in $(printf '%s' "$PACKAGING_TARGETS" | tr ',' ' '); do set -- "$@" --target "$target"; done
"$RUNNER_TEMP/dist-bin/dist" build --artifacts=global --output-format=json "$@" > "$RUNNER_TEMP/global-dist-manifest.json"
mv "$RUNNER_TEMP/global-dist-manifest.json" target/distrib/dist-manifest.json
python3 packaging/harden-installer.py target/distrib/hennery-installer.sh
sh packaging/test-installer.sh target/distrib
sh packaging/check-formula.sh target/distrib
nix develop .#release -c python3 packaging/release/homebrew.py prepare target/distrib --tag "$RELEASE_TAG" --targets "$PACKAGING_TARGETS"
git show HEAD:CHANGELOG.md > target/distrib/release-notes.md
python3 packaging/release/assets.py target/distrib "$GITHUB_SHA" --targets "$PACKAGING_TARGETS"
# Both architectures reuse the very binaries accepted in their archives.
python3 packaging/ghcr-candidate.py assemble target/distrib image-assembly "$GITHUB_SHA" "${RELEASE_TAG#v}" \
    images/release-image-amd64/layout images/release-image-arm64/layout
# Accept the serialized payload, rather than just its pre-serialization layout.
mkdir image-transport
tar -xf target/distrib/ghcr-oci-layout.tar -C image-transport
nix develop .#image -c sh packaging/test-image-registry.sh image-transport
# Do not change the release asset inventory just to exercise unselected channels.
python3 - <<'PY'
import json, os
from pathlib import Path
root = Path('target/distrib')
channels = json.loads(os.environ['RELEASE_CHANNELS'])
if 'crates' not in channels and (root / 'crates-inventory.json').exists():
    value = json.loads((root / 'crates-inventory.json').read_bytes())
    for name in ['crates-inventory.json', 'hennery-source-' + value['source']['sha'] + '.tar.gz',
                 *[crate['file'] for crate in value['crates']]]:
        (root / name).unlink()
if 'ghcr' not in channels:
    for name in ['ghcr-index.json', 'ghcr-oci-layout.tar']:
        (root / name).unlink()
PY
if [ "$PACKAGING_RELEASE" = true ]; then
    nix develop .#release -c python3 packaging/release/package_context.py --release freeze target/distrib
else
    nix develop .#release -c python3 packaging/release/package_context.py freeze target/distrib
fi
