#!/bin/sh
# Install cargo-dist for a CI job: the pinned release's archive for this
# runner, checked against the SHA-256 written here (not one fetched beside
# it), so a replaced release asset fails the build instead of running.
#
# Usage: install-dist.sh <directory to put `dist` in>
set -eu

version=0.30.4 # dist-workspace.toml's `cargo-dist-version`
case "$(uname -s)-$(uname -m)" in
Linux-x86_64)
    target=x86_64-unknown-linux-gnu
    sha256=f7bd986e758d0d47c6995aaf92f26d093635c7cd69581ed9e2451b618ea98098
    ;;
Linux-aarch64)
    target=aarch64-unknown-linux-gnu
    sha256=79aa478537011e0cd4f5dd79e02f28b2b87788966d241fc605c6fe23b9e74e83
    ;;
Darwin-arm64)
    target=aarch64-apple-darwin
    sha256=c8b8f3163e5e4dd5a9cc5455957043a00bfee7d446489e4f8a4db6f2d5af1ab1
    ;;
*)
    echo "install-dist: no pinned cargo-dist for $(uname -s)-$(uname -m)" >&2
    exit 1
    ;;
esac

bin=$1
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
archive="$scratch/cargo-dist.tar.xz"
curl --proto '=https' --tlsv1.2 -sSfL -o "$archive" \
    "https://github.com/axodotdev/cargo-dist/releases/download/v$version/cargo-dist-$target.tar.xz"
echo "$sha256  $archive" | shasum -a 256 -c -
tar -xJf "$archive" -C "$scratch"
mkdir -p "$bin"
install -m 0755 "$scratch/cargo-dist-$target/dist" "$bin/dist"
"$bin/dist" --version
