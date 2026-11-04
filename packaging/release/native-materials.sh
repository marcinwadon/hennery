#!/bin/sh
# Consume prepared UI dependency material; this runner needs no node_modules.
set -eu
python3 packaging/release/supply.py materials --output release-materials \
  --source-sha "$RELEASE_SOURCE_SHA" --target "$TARGET" \
  --cargo-metadata cargo-metadata.json --frontend-materials inputs/frontend \
  --openssl-prefix "$OPENSSL_DIR"
cp inputs/advisories/* release-materials/
