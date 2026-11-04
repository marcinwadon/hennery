#!/usr/bin/env python3
"""Select CI work from Git's actual changed paths, including deletions/renames."""
import argparse
import fnmatch
import json
import os
import subprocess


def matches(path, patterns):
    return any(fnmatch.fnmatchcase(path, pattern) for pattern in patterns)


def classify(paths, event="push"):
    recipes = any(matches(path, (
        'README.md', 'packaging/readme-blocks.sh', 'packaging/check-web-ui.sh',
        'Cargo.*', 'flake.*', 'nix/*', '.cargo/*', 'rust-toolchain*',
        'web/package.json', 'web/pnpm-lock.yaml', '.github/workflows/ci.yml',
        '.github/workflows/nix.yml',
    )) for path in paths)
    def documentation(path):
        return (path == 'LICENSE'
                or path.endswith('.md') and ('/' not in path or path.startswith('docs/'))
                or matches(path, ('docs/*.svg', 'docs/*.png')))
    # These files are exercised by checks, or by the reusable Nix job below.
    # Keep the allowlist exact: unrecognised inputs still select product tests.
    infrastructure = {
        'packaging/ci-paths.py', 'packaging/test_ci_paths.py',
        'packaging/ci_watch.py', 'packaging/tests/test_ci_watch.py',
        'packaging/runtime_acceptance.py', 'packaging/tests/test_runtime_acceptance.py',
        '.github/workflows/nix-package.yml',
    }
    code = any(not documentation(path) and path not in infrastructure for path in paths)
    nix_workflow = '.github/workflows/nix-package.yml' in paths
    platforms = any(matches(path, (
        'flake.*', 'nix/*', '.cargo/*', 'rust-toolchain*',
        '.github/workflows/ci.yml', '.github/workflows/nix.yml',
    )) for path in paths) or nix_workflow
    result = {name: str(value).lower() for name, value in {
        'rust': code, 'macos': code and event != 'pull_request', 'web': code,
        'nix': nix_workflow or (event != 'pull_request' and (code or recipes)) or any(matches(path, (
            'flake.nix', 'flake.lock', 'Cargo.lock', 'nix/*',
            'web/package.json', 'web/pnpm-lock.yaml', 'web/.npmrc',
        )) for path in paths),
        'recipes': recipes,
        'audit_rust': any(path == 'Cargo.lock' for path in paths),
        'audit_web': any(path == 'web/pnpm-lock.yaml' for path in paths),
    }.items()}
    matrix = [{'os': 'ubuntu-24.04', 'system': 'x86_64-linux'}]
    if platforms:
        matrix += [{'os': 'ubuntu-24.04-arm', 'system': 'aarch64-linux'},
                   {'os': 'macos-15', 'system': 'aarch64-darwin'}]
    if event == 'pull_request':
        matrix = [item for item in matrix if item['system'].endswith('-linux')]
    result['recipe_matrix'] = json.dumps(['ubuntu-latest'] if event == 'pull_request' else ['ubuntu-latest', 'macos-latest'])
    result['nix_matrix'] = json.dumps({'include': matrix}, separators=(',', ':'))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', required=True)
    parser.add_argument('--head', default='HEAD')
    args = parser.parse_args()
    if args.base and set(args.base) != {'0'}:
        # --no-renames exposes both the removed and added name, so moving a
        # source file into docs cannot disguise its deletion as documentation.
        command = ['git', 'diff', '--no-renames', '--name-only', '-z', args.base, args.head, '--']
    else:
        command = ['git', 'ls-tree', '--full-tree', '--name-only', '-r', '-z', args.head]
    paths = subprocess.check_output(command).decode('utf-8', errors='surrogateescape').split('\0')
    for name, value in classify([path for path in paths if path], os.environ["GITHUB_EVENT_NAME"]).items():
        print(f'{name}={value}')


if __name__ == '__main__':
    main()
