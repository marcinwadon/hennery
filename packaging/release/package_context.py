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
                          'extended': str(extended(args.release, args.base)).lower()}.items():
            print(key + '=' + data)
    else:
        gate.require(args.root is not None, 'payload directory required')
        release_run.freeze_payload(args.root, value, value['tag'], value['source'])


if __name__ == '__main__':
    main()
