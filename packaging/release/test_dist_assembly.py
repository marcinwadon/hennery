"""Exercise the production assembly command with the real pinned cargo-dist.

The release-tests workflow supplies HENNERY_TEST_DIST after checksum installation.
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

import assemble
import gate
import test_assemble

ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(os.environ.get('HENNERY_TEST_DIST'), 'requires pinned cargo-dist (release-tests CI)')
class DistAssembly(unittest.TestCase):
    def test_production_global_build_imports_all_native_manifests(self):
        dist = Path(os.environ['HENNERY_TEST_DIST']).resolve()
        version = subprocess.check_output([str(dist), '--version'], text=True).strip()
        self.assertEqual(version, 'cargo-dist 0.30.4')
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'home').mkdir(); (root / 'scratch').mkdir()
            env = dict(os.environ, HOME=str(root / 'home'), CARGO_HOME=str(root / 'home/cargo'),
                       RUSTUP_HOME=os.environ.get('RUSTUP_HOME', str(Path.home() / '.rustup')),
                       XDG_CONFIG_HOME=str(root / 'home/config'), XDG_CACHE_HOME=str(root / 'home/cache'),
                       GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_SYSTEM=os.devnull,
                       RUNNER_TEMP=str(root / 'scratch'))
            (root / 'scratch/dist-bin').mkdir()
            (root / 'scratch/dist-bin/dist').symlink_to(dist)
            (root / 'src').mkdir()
            (root / 'src/main.rs').write_text('fn main() {}\n')
            (root / 'Cargo.toml').write_text(
                '[package]\nname="hennery"\nversion="0.1.0-alpha.3"\nedition="2024"\n'
                'repository="https://github.com/marcinwadon/hennery"\n'
                '[profile.dist]\ninherits="release"\n')
            (root / 'dist-workspace.toml').write_text(
                '[workspace]\nmembers=["cargo:."]\n[dist]\ncargo-dist-version="0.30.4"\n'
                'targets=' + json.dumps(sorted(gate.TARGETS)) + '\n'
                'ci="github"\nallow-dirty=["ci"]\ninstallers=["shell", "homebrew"]\nchecksum="sha256"\n')
            for name in ('README.md', 'CHANGELOG.md', 'LICENSE'):
                (root / name).write_text('fixture\n')
            for args in (['init', '-q'], ['add', '.'],
                         ['-c', 'commit.gpgsign=false', '-c', 'user.name=Release Fixture',
                          '-c', 'user.email=release@example.invalid', 'commit', '-qm', 'fixture']):
                subprocess.run(['git', *args], cwd=root, env=env, check=True, capture_output=True)
            source = test_assemble.candidates(root / 'candidates')
            checksums, systems = {}, {}
            for target in sorted(gate.TARGETS):
                result = subprocess.run([str(dist), 'manifest', '--artifacts=local', '--target=' + target,
                                         '--output-format=json'], cwd=root, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                manifest = json.loads(result.stdout)
                self.assertTrue(manifest['systems'])
                systems.update(manifest['systems'])
                archive = 'hennery-' + target + '.tar.xz'
                checksum = gate.digest((source / ('candidate-' + target) / archive).read_bytes())
                manifest['artifacts'][archive]['checksums'] = {'sha256': checksum}
                checksums[archive] = checksum
                (source / ('candidate-' + target) / (target + '-dist-manifest.json')).write_text(json.dumps(manifest))
            assemble.assemble(source, root / 'target/distrib')
            # Execute the checked-in production shell, including stdout redirection
            # and any final move. PR and tag packaging both execute this same script.
            script = (ROOT / 'packaging/release/assemble-package.sh').read_text()
            command = script[script.index('"$RUNNER_TEMP/dist-bin/dist" build'):]
            command = command[:command.index('python3 packaging/harden-installer.py')]
            result = subprocess.run(['sh', '-eu', '-c', command], cwd=root, env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            manifest = json.loads((root / 'target/distrib/dist-manifest.json').read_bytes())
            for archive, checksum in checksums.items():
                self.assertEqual(manifest['artifacts'][archive]['checksums']['sha256'], checksum)
            # Global generation can recompute archive digests, but it cannot
            # recreate the local builders' system records without these inputs.
            for system, details in systems.items():
                self.assertEqual(manifest['systems'][system], details)
            for name in ('hennery-installer.sh', 'hennery.rb'):
                self.assertGreater((root / 'target/distrib' / name).stat().st_size, 0)


if __name__ == '__main__':
    unittest.main()
