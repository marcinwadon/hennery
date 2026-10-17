"""Real cargo-dist output crosses the shared archive/advisory/staging boundary."""
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import assemble
import gate
import supply
import test_assemble

ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(os.environ.get('HENNERY_TEST_DIST'), 'requires pinned cargo-dist (packaging CI)')
class NativeDist(unittest.TestCase):
    def run_archive(self, root, missing=None, *, host_only=False):
        native_target = {('Darwin', 'arm64'): 'aarch64-apple-darwin',
                         ('Linux', 'x86_64'): 'x86_64-unknown-linux-musl',
                         ('Linux', 'aarch64'): 'aarch64-unknown-linux-musl'}[(platform.system(), platform.machine())]
        target = native_target
        if host_only:
            # Linked Rustup fixtures have host std only and cannot add components.
            # Cross-target release qualification belongs to the other archive test.
            version = subprocess.check_output(['rustc', '-vV'], cwd=root, text=True)
            target = next(line.removeprefix('host: ') for line in version.splitlines() if line.startswith('host: '))
        (root / 'src').mkdir()
        (root / 'src/main.rs').write_text('fn main() { println!("hennery 0.1.0-alpha.3"); }\n')
        (root / 'Cargo.toml').write_text(
            '[package]\nname="hennery"\nversion="0.1.0-alpha.3"\nedition="2024"\n'
            'repository="https://github.com/marcinwadon/hennery"\nlicense="AGPL-3.0-only"\n'
            '[profile.dist]\ninherits="release"\n')
        (root / 'dist-workspace.toml').write_text(
            '[workspace]\nmembers=["cargo:."]\n[dist]\ncargo-dist-version="0.30.4"\n'
            'targets=' + json.dumps([target]) + '\nci="github"\nallow-dirty=["ci"]\n'
            'checksum="sha256"\ninstallers=["shell","homebrew"]\n')
        for name in ('README.md', 'CHANGELOG.md', 'LICENSE'):
            (root / name).write_text('fixture documentation\n')
        (root / 'packaging').symlink_to(ROOT / 'packaging', target_is_directory=True)
        materials = root / 'release-materials'; materials.mkdir()
        seed = test_assemble.candidates(root / 'seed') / ('candidate-' + native_target)
        for name in ('THIRD-PARTY-NOTICES.txt', 'licence-findings.json', 'third-party-source.tar.xz',
                     'rust-advisories.json', 'rust-advisory-revision.txt', 'web-advisories.json',
                     'runtime-inventory.json', 'hennery-' + native_target + '.spdx.json'):
            shutil.copyfile(seed / name, materials / name)
        (materials / 'build-identity.json').write_text(json.dumps({
            'source_sha': 'a' * 40, 'ui_sha256': 'b' * 64, 'adapter_manifest_sha256': 'c' * 64}))
        if missing:
            (materials / missing).unlink()
        (root / 'home').mkdir()
        env = dict(os.environ, HOME=str(root / 'home'), CARGO_HOME=str(root / 'home/cargo'),
                   RUSTUP_HOME=os.environ.get('RUSTUP_HOME', str(Path.home() / '.rustup')),
                   XDG_CONFIG_HOME=str(root / 'home/config'), XDG_CACHE_HOME=str(root / 'home/cache'),
                   GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_SYSTEM=os.devnull, TARGET=target,
                   PATH=str(Path(os.environ['HENNERY_TEST_DIST']).resolve().parent) + os.pathsep + os.environ['PATH'])
        for args in (['init', '-q'], ['add', '.'],
                     ['-c', 'commit.gpgsign=false', '-c', 'user.name=Marcin Wadoń',
                      '-c', 'user.email=marcin.wadon@gmail.com', 'commit', '-qm', 'fixture']):
            subprocess.run(['git', *args], cwd=root, env=env, check=True, capture_output=True)
        command = (['dist', 'build', '--artifacts=local', '--target=' + target] if host_only
                   else ['sh', str(ROOT / 'packaging/release/native-archive.sh')])
        result = subprocess.run(command,
                                cwd=root, env=env, capture_output=True, text=True)
        return result, target

    def test_real_native_output_preserves_the_transport_inventory_and_expanded_staging(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result, target = self.run_archive(root)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            assemble.check_native(root / 'target/distrib', target)
            self.assertEqual({p.name for p in (root / 'target/distrib').iterdir()},
                             assemble.native_files(target) | {'hennery-' + target})

    def test_corresponding_source_uses_real_cargo_normal_and_build_closure(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'src').mkdir(); (root / 'src/main.rs').write_text('fn main() {}')
            (root / 'Cargo.toml').write_text('[package]\nname="hennery"\nversion="0.1.0"\nedition="2024"\n'
                '[dependencies]\nruntime={path="runtime"}\n[build-dependencies]\nbuilder={path="builder"}\n'
                '[dev-dependencies]\ntesttool={path="testtool"}\n')
            for name in ('runtime', 'builder', 'testtool'):
                (root / name / 'src').mkdir(parents=True)
                (root / name / 'src/lib.rs').write_text('pub fn fixture() {}')
                (root / name / 'Cargo.toml').write_text('[package]\nname="' + name + '"\nversion="0.1.0"\nedition="2024"\n')
            cargo = json.loads(subprocess.check_output(['cargo', 'metadata', '--offline', '--format-version=1'], cwd=root))
            self.assertEqual({p['name'] for p in supply.corresponding_packages(cargo)}, {'hennery', 'runtime', 'builder'})

    def test_missing_advisory_input_stops_before_any_archive_can_reach_homebrew(self):
        for name in ('rust-advisories.json', 'rust-advisory-revision.txt', 'web-advisories.json'):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                result, target = self.run_archive(root, missing=name)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse((root / 'target/distrib' / ('hennery-' + target + '.tar.xz')).exists(),
                                 result.stdout + result.stderr)

    def test_real_rustup_toolchain_survives_isolated_home_without_ambient_override(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            runner_home = root / 'runner-home'; runner_home.mkdir()
            toolchain = root / 'toolchain'; (toolchain / 'bin').mkdir(parents=True)
            sysroot = Path(subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip())
            for binary in ('cargo', 'rustc', 'rustdoc'):
                installed = subprocess.run(['rustup', 'which', binary], capture_output=True, text=True,
                                           env=dict(os.environ, RUSTUP_AUTO_INSTALL='0'))
                (toolchain / 'bin' / binary).symlink_to(
                    installed.stdout.strip() if installed.returncode == 0 else shutil.which(binary))
            (toolchain / 'lib').symlink_to(sysroot / 'lib', target_is_directory=True)
            setup = dict(os.environ, HOME=str(runner_home), RUSTUP_HOME=str(runner_home / '.rustup'),
                         CARGO_HOME=str(runner_home / '.cargo'))
            for command in (['toolchain', 'link', 'fixture-native', str(toolchain)], ['default', 'fixture-native']):
                subprocess.run(['rustup', *command], env=setup, check=True, capture_output=True)
            shims = runner_home / '.cargo/bin'; shims.mkdir(parents=True)
            for binary in ('cargo', 'rustc', 'rustdoc'):
                (shims / binary).symlink_to(shutil.which('rustup'))
            environment = setup | {'PATH': str(shims) + os.pathsep + os.environ['PATH']}
            environment.pop('RUSTUP_HOME'); environment.pop('RUSTUP_TOOLCHAIN', None)
            project = root / 'project'; project.mkdir()
            with patch.dict(os.environ, environment, clear=True):
                result, target = self.run_archive(project, host_only=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertTrue((project / 'target/distrib' / ('hennery-' + target + '.tar.xz')).is_file())
