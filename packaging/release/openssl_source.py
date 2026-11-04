#!/usr/bin/env python3
"""Build checksum-pinned static OpenSSL, or check the CLI's linked-library version.

Cargo source installs use their platform's maintained OpenSSL development
package. The release builder deliberately does not depend on openssl-src.
"""
import argparse
from datetime import date
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile

import check_source

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / 'packaging/openssl.json'
TARGETS = {
    'aarch64-apple-darwin': ('Darwin', 'arm64', 'darwin64-arm64-cc', '/usr/bin/cc'),
    'aarch64-unknown-linux-musl': ('Linux', 'aarch64', 'linux-aarch64', '/usr/bin/musl-gcc'),
    'x86_64-unknown-linux-musl': ('Linux', 'x86_64', 'linux-x86_64', '/usr/bin/musl-gcc'),
}


def source():
    value = json.loads(SOURCE.read_text())
    if date.today() >= date.fromisoformat(value['supported_until']):
        raise ValueError('release OpenSSL support has expired; update the pinned source')
    return value


def verify_archive(path, expected):
    if hashlib.sha256(path.read_bytes()).hexdigest() != expected:
        raise ValueError('OpenSSL source SHA-256 differs from the reviewed pin')


def check_binary(binary):
    expected = 'OpenSSL ' + source()['version'] + ' '
    versions = re.findall(r'^crypto: (.*)$', check_source.version_output(binary), re.MULTILINE)
    if len(versions) != 1 or not versions[0].startswith(expected):
        raise ValueError('binary does not report the pinned OpenSSL version')


def build(target, prefix, archive=None, jobs=2):
    pin = source()
    system, machine, configuration, compiler = TARGETS[target]
    if (platform.system(), platform.machine()) != (system, machine):
        raise ValueError('OpenSSL release builds must run on the native target host')
    if not Path(compiler).is_file():
        raise ValueError(f'native compiler missing: {compiler}')
    prefix = prefix.resolve()
    prefix.mkdir(parents=True, exist_ok=False)
    with tempfile.TemporaryDirectory(prefix='hennery-openssl-', dir=prefix.parent) as tmp:
        work = Path(tmp)
        if archive is None:
            archive = work / 'source.tar.gz'
            subprocess.run(['curl', '--fail', '--location', '--proto', '=https', '--tlsv1.2',
                            '--connect-timeout', '20', '--max-time', '600', '--retry', '2',
                            '--output', str(archive), pin['url']], check=True)
        verify_archive(archive, pin['sha256'])
        with tarfile.open(archive, 'r:gz') as tar:
            tar.extractall(work, filter='data')
        tree = work / ('openssl-' + pin['version'])
        env = {**os.environ, 'CC': compiler, 'AR': '/usr/bin/ar', 'RANLIB': '/usr/bin/ranlib'}
        # Use the native platform toolchain, not any inherited Nix flags or
        # developer compiler settings, for the standalone distributed archives.
        for name in ('CFLAGS', 'CXXFLAGS', 'CPPFLAGS', 'LDFLAGS', 'CROSS_COMPILE'):
            env.pop(name, None)
        if system == 'Darwin':
            deployment = subprocess.check_output(
                ['rustc', '--print=deployment-target', '--target', target], env=env, text=True).strip()
            minimum = re.fullmatch(r'MACOSX_DEPLOYMENT_TARGET=(\d+(?:\.\d+){1,2})', deployment)
            if minimum is None:
                raise ValueError('Rust did not report the macOS deployment target')
            env['MACOSX_DEPLOYMENT_TARGET'] = minimum.group(1)
        # Match the previous source provider's musl configuration: libc lacks
        # the async context APIs; engines and secure-heap probing are not portable.
        musl = ['no-engine', 'no-async', '-DOPENSSL_NO_SECURE_MEMORY'] if target.endswith('-musl') else []
        if target == 'aarch64-unknown-linux-musl':
            # Ubuntu's musl-gcc shares glibc libgcc: outlined atomics pull in
            # __getauxval from libgcc.a. Use baseline ARMv8 atomics directly.
            musl.append('-mno-outline-atomics')
        subprocess.run(['perl', 'Configure', configuration, 'no-shared', 'no-module',
                        'no-tests', 'no-comp', 'no-zlib', *musl, '--openssldir=/etc/ssl',
                        '--prefix=' + str(prefix)], cwd=tree, env=env, check=True)
        subprocess.run(['make', '-s', '-j', str(jobs), 'build_sw'], cwd=tree, env=env, check=True)
        subprocess.run(['make', '-s', 'install_sw'], cwd=tree, env=env, check=True)
        version = subprocess.check_output([str(prefix / 'bin/openssl'), 'version'], text=True)
        if not version.startswith('OpenSSL ' + pin['version'] + ' '):
            raise ValueError('built OpenSSL version differs from pin')
        shutil.copyfile(tree / 'LICENSE.txt', prefix / 'LICENSE.txt')
        (prefix / 'hennery-source.json').write_text(json.dumps(pin, sort_keys=True) + '\n')
    return prefix


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='operation', required=True)
    builder = commands.add_parser('build')
    builder.add_argument('--target', choices=TARGETS, required=True)
    builder.add_argument('--prefix', type=Path, required=True)
    builder.add_argument('--archive', type=Path)
    builder.add_argument('--jobs', type=int, default=2)
    checker = commands.add_parser('check')
    checker.add_argument('binary', type=Path)
    args = parser.parse_args()
    if args.operation == 'build':
        if args.jobs < 1:
            parser.error('--jobs must be positive')
        build(args.target, args.prefix, args.archive, args.jobs)
    else:
        check_binary(args.binary)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        raise SystemExit(f'OpenSSL qualification refused: {error}') from error
