#!/bin/sh
# Shared native release archive construction; caller supplies dist and materials.
set -eu
case "$TARGET" in
    aarch64-apple-darwin|aarch64-unknown-linux-musl|x86_64-unknown-linux-musl) ;;
    *) echo 'unqualified native release target' >&2; exit 1 ;;
esac
# Advisory inputs travel with the single prepared UI. Reject an incomplete
# transfer before spending a build or handing any archive to Homebrew.
for file in rust-advisories.json rust-advisory-revision.txt web-advisories.json; do
    [ -s "release-materials/$file" ] || { echo "missing advisory input: $file" >&2; exit 1; }
done
export HENNERY_WEB_REQUIRE=1
HENNERY_BUILD_SOURCE_SHA=$(python3 -c 'import json; print(json.load(open("release-materials/build-identity.json"))["source_sha"])')
HENNERY_BUILD_UI_SHA256=$(python3 -c 'import json; print(json.load(open("release-materials/build-identity.json"))["ui_sha256"])')
HENNERY_BUILD_MANIFEST_SHA256=$(python3 -c 'import json; print(json.load(open("release-materials/build-identity.json"))["adapter_manifest_sha256"])')
export HENNERY_BUILD_SOURCE_SHA HENNERY_BUILD_UI_SHA256 HENNERY_BUILD_MANIFEST_SHA256
dist build --artifacts=local --target="$TARGET" --print=linkage --output-format=json > target-manifest.json
archive="hennery-$TARGET"
mkdir -p unpacked
tar -xJf "target/distrib/$archive.tar.xz" -C unpacked
cp release-materials/THIRD-PARTY-NOTICES.txt "unpacked/$archive/"
tar -cJf "target/distrib/$archive.tar.xz" -C unpacked "$archive"
(cd target/distrib && shasum -a 256 "$archive.tar.xz" > "$archive.tar.xz.sha256")
sh packaging/check-archive.sh "target/distrib/$archive.tar.xz" "$TARGET" --notices
binary="$PWD/unpacked/$archive/hennery"
sh packaging/check-linkage.sh "$binary"
python3 packaging/release/openssl_source.py check "$binary"
sh packaging/check-web-ui.sh "$binary"
python3 packaging/release/check_source.py "$binary" release-materials/build-identity.json
"$binary" --version
cp target-manifest.json "target/distrib/$TARGET-dist-manifest.json"
python3 packaging/release/refresh_manifest.py "target/distrib/$TARGET-dist-manifest.json" "target/distrib/$archive.tar.xz"
cp release-materials/* target/distrib/
python3 - <<'PY' > target/distrib/builder.json
import json,os,platform,subprocess
print(json.dumps({'os':platform.platform(),'runner_image':os.environ.get('ImageVersion'),'rust':subprocess.check_output(['rustc','-vV'],text=True),'sdk':subprocess.getoutput('xcrun --show-sdk-version') if platform.system()=='Darwin' else subprocess.getoutput('ldd --version')}))
PY
# upload-artifact follows links. Refuse them before transport can erase type.
python3 packaging/release/assemble.py --check-native "$TARGET" target/distrib
