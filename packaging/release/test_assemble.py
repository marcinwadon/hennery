"""The hosted download layout includes cargo-dist's expanded staging tree."""
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
import subprocess
import sys

import assemble
import gate


def candidates(root, targets=None):
    for target in sorted(gate.TARGETS if targets is None else targets):
        directory = root / ('candidate-' + target)
        directory.mkdir(parents=True)
        base = 'hennery-' + target
        files = {'hennery': b'binary-' + target.encode(), 'CHANGELOG.md': b'changes',
                 'LICENSE': b'licence', 'README.md': b'readme'}
        staging = directory / base
        staging.mkdir()
        for name, data in files.items():
            (staging / name).write_bytes(data)
        with tarfile.open(directory / (base + '.tar.xz'), 'w:xz') as archive:
            entry = tarfile.TarInfo(base); entry.type = tarfile.DIRTYPE
            archive.addfile(entry)
            for name, data in (files | {'THIRD-PARTY-NOTICES.txt': b'notices'}).items():
                entry = tarfile.TarInfo(base + '/' + name); entry.size = len(data)
                archive.addfile(entry, io.BytesIO(data))
        for name in ('builder.json', 'third-party-source.tar.xz', 'rust-advisories.json',
                     'rust-advisory-revision.txt', 'web-advisories.json', 'build-identity.json',
                     'runtime-inventory.json', base + '.spdx.json', target + '-dist-manifest.json'):
            (directory / name).write_bytes(b'{}')
        (directory / 'THIRD-PARTY-NOTICES.txt').write_bytes(b'notices')
        (directory / 'licence-findings.json').write_bytes(b'{"findings":[]}')
        (directory / (base + '.tar.xz.sha256')).write_text(
            gate.digest((directory / (base + '.tar.xz')).read_bytes()) + '  ' + base + '.tar.xz\n')
    return root


class Assemble(unittest.TestCase):
    def test_native_archive_members_are_unique_regular_and_match_staging(self):
        target = sorted(gate.TARGETS)[0]; base = 'hennery-' + target
        for kind in ('missing', 'duplicate', 'link'):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as tmp:
                directory = candidates(Path(tmp) / 'candidates') / ('candidate-' + target)
                path = directory / (base + '.tar.xz')
                with tarfile.open(path, 'r:xz') as archive:
                    members = [(m, archive.extractfile(m).read() if m.isfile() else None)
                               for m in archive.getmembers()]
                with tarfile.open(path, 'w:xz') as archive:
                    for member, data in members:
                        if member.name == base + '/hennery':
                            if kind == 'missing': continue
                            if kind == 'duplicate': archive.addfile(member, io.BytesIO(data))
                            if kind == 'link':
                                member.type = tarfile.SYMTYPE; member.linkname = 'README.md'; member.size = 0; data = None
                        archive.addfile(member, io.BytesIO(data) if data is not None else None)
                with self.assertRaisesRegex(gate.Refused, 'nonregular native archive member'):
                    assemble.check_native(directory, target)

    def test_preupload_check_is_fatal_and_bound_to_native_archive_script(self):
        root = Path(__file__).resolve().parents[2]
        script = (root / 'packaging/release/native-archive.sh').read_text()
        self.assertIn('set -eu\n', script)
        self.assertEqual(script.splitlines()[-1],
                         'python3 packaging/release/assemble.py --check-native "$TARGET" target/distrib')
        target = sorted(gate.TARGETS)[0]
        with tempfile.TemporaryDirectory() as tmp:
            source = candidates(Path(tmp) / 'candidates') / ('candidate-' + target)
            command = [sys.executable, str(root / 'packaging/release/assemble.py'), '--check-native', target, str(source)]
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            path = source / 'builder.json'
            path.unlink(); path.symlink_to(source / 'build-identity.json')
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('nonregular', result.stderr)

    def test_real_download_layout_merges_final_files_without_expanded_staging(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); source = candidates(root / 'candidates')
            output = root / 'output'
            assemble.assemble(source, output)
            self.assertTrue(all(p.is_file() for p in output.iterdir()))
            for target in gate.TARGETS:
                self.assertFalse((output / ('hennery-' + target)).exists())
                self.assertTrue((output / ('hennery-' + target + '.tar.xz.sha256')).is_file())
                self.assertTrue((output / (target + '-builder.json')).is_file())
                self.assertTrue((output / (target + '-dist-manifest.json')).is_file())

    def test_input_closure_refuses_missing_extra_linked_or_conflicting_materials(self):
        for mutation in ('extra-file', 'extra-directory', 'missing-file', 'symlink-file',
                         'symlink-candidate', 'symlink-root', 'symlink-staging', 'staging-extra',
                         'staging-link', 'staging-directory', 'staging-changed', 'conflict'):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp); source = candidates(root / 'candidates')
                directory = source / ('candidate-' + sorted(gate.TARGETS)[0])
                staging = directory / ('hennery-' + sorted(gate.TARGETS)[0])
                if mutation == 'extra-file': (directory / 'unexpected').write_bytes(b'x')
                if mutation == 'extra-directory': (directory / 'unexpected').mkdir()
                if mutation == 'missing-file': (directory / 'runtime-inventory.json').unlink()
                if mutation == 'symlink-file':
                    (directory / 'builder.json').unlink()
                    (directory / 'builder.json').symlink_to(directory / 'build-identity.json')
                if mutation in ('symlink-candidate', 'symlink-root', 'symlink-staging'):
                    path = {'symlink-candidate': directory, 'symlink-root': source,
                            'symlink-staging': staging}[mutation]
                    saved = root / 'saved'; path.rename(saved); path.symlink_to(saved)
                if mutation == 'staging-extra': (staging / 'extra').write_bytes(b'x')
                if mutation == 'staging-link':
                    (staging / 'hennery').unlink(); (staging / 'hennery').symlink_to(staging / 'README.md')
                if mutation == 'staging-directory':
                    (staging / 'hennery').unlink(); (staging / 'hennery').mkdir()
                if mutation == 'staging-changed': (staging / 'README.md').write_bytes(b'changed')
                if mutation == 'conflict': (directory / 'build-identity.json').write_bytes(b'changed')
                with self.assertRaises(gate.Refused): assemble.assemble(source, root / 'output')
