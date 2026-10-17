#!/usr/bin/env python3
"""Read-only packaging context; PR payloads never acquire release authority."""
import argparse
import fnmatch
import os
from pathlib import Path
import subprocess
import tomllib

import gate
import release_run


def context(release):
    source = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    gate.require(source == os.environ['GITHUB_SHA'], 'packaging source differs from checkout')
    version = tomllib.loads(Path('Cargo.toml').read_text())['workspace']['package']['version']
    value = release_run.configuration() if release else {'channels': ['github'], 'tag_object': '0' * 40}
    return value | {'tag': 'v' + version, 'source': source}


def extended(release, base):
    if release or not base or set(base) == {'0'}:
        return True
    paths = subprocess.check_output(['git', 'diff', '--no-renames', '--name-only', '-z', base, 'HEAD', '--']).decode().split('\0')
    return any(fnmatch.fnmatchcase(path, pattern) for path in paths for pattern in
               ('Cargo.*', 'crates/*/Cargo.toml', 'flake.*', 'nix/*', '.cargo/*', 'rust-toolchain*', '.github/workflows/*'))


def selection(release, base):
    pr = os.environ.get('GITHUB_EVENT_NAME') == 'pull_request'
    archives = [
        {'target': 'x86_64-unknown-linux-musl', 'runner': 'ubuntu-24.04', 'image': 'amd64'},
        {'target': 'aarch64-unknown-linux-musl', 'runner': 'ubuntu-24.04-arm', 'image': 'arm64'},
    ]
    if not pr:
        archives.append({'target': 'aarch64-apple-darwin', 'runner': 'macos-15'})
    crates = release
    if not crates and pr and base and set(base) != {'0'}:
        paths = subprocess.check_output(['git', 'diff', '--no-renames', '--name-only', '-z', base, 'HEAD', '--']).decode().split('\0')
        scripts = ('dist-workspace.toml', 'packaging/prepare-crates.mjs', 'packaging/crate-release.mjs',
                   'packaging/crate-registry.mjs', 'packaging/ci-crates.sh', 'packaging/check-crates.mjs',
                   'packaging/check-package-build.mjs', 'packaging/check-native-crate.mjs',
                   'packaging/check-binstall.mjs', 'packaging/smoke-crate-ui.mjs',
                   'packaging/check-offline.mjs', 'packaging/offline-build.sh', 'packaging/check-dist-plan.mjs')
        for path in paths:
            if any(fnmatch.fnmatchcase(path, pattern) for pattern in scripts):
                crates = True
            if path == 'Cargo.toml' or fnmatch.fnmatchcase(path, 'crates/*/Cargo.toml'):
                def metadata(revision):
                    result = subprocess.run(['git', 'show', revision + ':' + path], capture_output=True, text=True)
                    if result.returncode:
                        return None
                    value = tomllib.loads(result.stdout)
                    return value.get('package'), value.get('workspace', {}).get('package')
                if metadata(base) != metadata('HEAD'):
                    crates = True
    elif pr and (not base or set(base) == {'0'}):
        crates = True
    return {'crates': str(crates).lower(), 'targets': ','.join(sorted(item['target'] for item in archives)),
            'archive_matrix': gate.canonical({'include': archives}).decode().strip(),
            'native_matrix': gate.canonical({'runner': [item['runner'] for item in archives]}).decode().strip()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['prepare', 'freeze'])
    parser.add_argument('--release', action='store_true')
    parser.add_argument('--base', default='')
    parser.add_argument('root', nargs='?', type=Path)
    args = parser.parse_args()
    value = context(args.release)
    if args.operation == 'prepare':
        for key, data in {'tag': value['tag'], 'source': value['source'],
                          'channels': gate.canonical(value['channels']).decode().strip(),
                          'extended': str(extended(args.release, args.base)).lower(), **selection(args.release, args.base)}.items():
            print(key + '=' + data)
    else:
        gate.require(args.root is not None, 'payload directory required')
        release_run.freeze_payload(args.root, value, value['tag'], value['source'], os.environ['PACKAGING_TARGETS'].split(','))


if __name__ == '__main__':
    main()
