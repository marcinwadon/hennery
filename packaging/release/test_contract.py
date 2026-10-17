"""Release contract refusals; no production transport or private signing key."""
import copy
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import contract
import gate


class Contract(unittest.TestCase):
    def fixture(self, root):
        (root / 'installer.sh').write_bytes(b'exit 0\n')
        metadata = {'schema': 3, 'repository': gate.REPOSITORY, 'source_sha': 'a' * 40,
                    'tag': 'v0.1.0', 'tag_object': 'b' * 40, 'channels': ['github'],
                    'targets': sorted(gate.TARGETS)}
        contract.freeze(root, metadata)
        return metadata

    def test_freeze_binds_complete_payload_and_refuses_nonregular_or_changed_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            metadata = self.fixture(root)
            self.assertEqual(contract.load(root)['source_sha'], metadata['source_sha'])
            entries = contract.verify_files(root)
            self.assertEqual(set(entries), {'installer.sh', 'release-metadata.json'})
            with self.assertRaises(gate.Refused): contract.freeze(root, metadata)
            (root / 'installer.sh').write_bytes(b'evil')
            with self.assertRaises(gate.Refused): contract.verify_files(root)
        for kind in ['extra', 'missing', 'symlink', 'directory']:
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp); self.fixture(root)
                if kind == 'extra': (root / 'unlisted').write_bytes(b'x')
                if kind == 'missing': (root / 'installer.sh').unlink()
                if kind == 'symlink': (root / 'link').symlink_to('installer.sh')
                if kind == 'directory': (root / 'dir').mkdir()
                with self.assertRaises(gate.Refused): contract.verify_files(root)

    def test_verification_uses_tag_then_every_subject_and_rechecks_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); self.fixture(root)
            output = 'verified'
            events = []
            def crypto(command, **kwargs):
                events.append(Path(command[3]).name)
                return subprocess.CompletedProcess(command, 0, output, '')
            with patch('tag_signature.verify', side_effect=lambda *a: events.append('tag')), patch.object(contract.subprocess, 'run', side_effect=crypto):
                result = contract.verify(root, 'v0.1.0', 'a' * 40, Path(tmp) / 'bundle')
                self.assertEqual(result['source_sha'], 'a' * 40)
                self.assertEqual(events, ['tag', 'SHA256SUMS', 'installer.sh', 'release-metadata.json'])
                for tag, source in [('v0.2.0','a'*40),('v0.1.0','c'*40)]:
                    with self.subTest(tag=tag,source=source), self.assertRaises(gate.Refused):
                        contract.verify(root, tag, source, 'bundle')
            def mutate(command, **kwargs):
                (root / 'installer.sh').write_bytes(b'changed while verifying')
                return subprocess.CompletedProcess(command, 0, output, '')
            with patch('tag_signature.verify'), patch.object(contract.subprocess,'run',side_effect=mutate), self.assertRaises(gate.Refused):
                contract.verify(root, 'v0.1.0', 'a'*40, 'bundle')

    def test_a_listed_symlink_to_identical_bytes_is_still_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)/'release';root.mkdir();self.fixture(root)
            source=Path(tmp)/'outside';source.write_bytes((root/'installer.sh').read_bytes())
            (root/'installer.sh').unlink();(root/'installer.sh').symlink_to(source)
            with self.assertRaises(gate.Refused):contract.verify_files(root)

    def test_canonical_unique_safe_checksum_entries(self):
        raw = ('a' * 64 + '  archive.tar.xz\n').encode()
        self.assertEqual(contract.parse_checksums(raw), {'archive.tar.xz': 'a' * 64})
        for bad in [b'', raw + raw, raw + raw.replace(b'archive', b'ARCHIVE'),
                    raw.replace(b'archive', b'ARCHIVE') + raw,
                    raw.replace(b'archive.tar.xz', b'../archive'), raw.replace(b'  ', b' '),
                    raw.replace(b'archive.tar.xz', b'SHA256SUMS'),
                    raw.replace(b'archive.tar.xz', b'release.sigstore.jsonl'),
                    raw.replace(b'archive.tar.xz', b'sha256sums'),
                    raw.replace(b'\n', b'\r\n'), raw.replace(b'\n', b'\v\n'),
                    raw.rstrip(b'\n'), raw.replace(b'a', b'G', 1)]:
            with self.subTest(raw=bad), self.assertRaises(gate.Refused):
                contract.parse_checksums(bad)

    def test_metadata_refuses_every_malformed_binding_and_ambiguous_json(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); metadata = self.fixture(root)
        for field, value in [('schema', 1), ('repository', 'other/repository'), ('tag', 'main'),
                             ('source_sha', 'main'), ('tag_object', 'x'*40), ('channels', ['homebrew']),
                             ('channels', ['github','github']), ('targets', []), ('targets', ['unknown']),
                             ('artifacts', [])]:
            value_map = copy.deepcopy(metadata);value_map[field] = value
            with self.subTest(field=field,value=value), self.assertRaises(gate.Refused):
                contract.validate_metadata(value_map)
        for raw in [b'{"run_id":42,"RUN_ID":43}',b'{"run_id":42,"run_\\u0069d":43}']:
            with self.subTest(raw=raw),self.assertRaises(gate.Refused):gate.strict_json(raw)

    def test_command_uses_exact_identity_and_both_digest_constraints(self):
        command = contract.attestation_command('payload', 'v0.1.0', 'a' * 40, 'bundle', 'root')
        self.assertNotIn('--signer-workflow', command)
        for flag, value in [('--cert-identity', 'https://github.com/marcinwadon/hennery/.github/workflows/release.yml@refs/tags/v0.1.0'),
                            ('--source-ref', 'refs/tags/v0.1.0'), ('--source-digest', 'a' * 40),
                            ('--signer-digest', 'a' * 40), ('--bundle', 'bundle'),
                            ('--custom-trusted-root', 'root')]:
            self.assertIn(flag, command)
            self.assertEqual(command[command.index(flag) + 1], value)
        self.assertIn('--deny-self-hosted-runners', command)
        for tag, sha in [('main', 'a' * 40), ('v0.1.0', 'main'), ('v0.1.0;false', 'a' * 40)]:
            with self.subTest(tag=tag), self.assertRaises(gate.Refused):
                contract.attestation_command('payload', tag, sha, 'bundle')



if __name__ == '__main__':
    unittest.main()
