"""Main artifacts are inputs, never unsigned publication authority."""
import copy
import io
from pathlib import Path
import stat
import tempfile
import unittest
from unittest.mock import Mock, patch
import zipfile

import contract
import gate
import qualified_main as qualified


class Selection(unittest.TestCase):
    def setUp(self):
        self.run = {'id': 42, 'workflow_id': 9, 'path': '.github/workflows/packaging.yml',
                    'head_sha': 'a' * 40, 'head_branch': 'main', 'event': 'push',
                    'repository': {'full_name': gate.REPOSITORY}, 'status': 'completed',
                    'conclusion': 'success', 'run_attempt': 1}
        self.artifacts = [{'id': i, 'name': name, 'digest': 'sha256:' + 'c' * 64,
                           'expired': False, 'workflow_run': {'head_sha': 'a' * 40}}
                          for i, name in enumerate(['release-payload-42', *qualified.PROOFS], 1)]

    def select(self, runs=None):
        api = Mock()
        api.get.side_effect = [{'id': 9}, {'workflow_runs': [self.run] if runs is None else runs}]
        api.pages.return_value = self.artifacts
        return qualified.select(api, 'a' * 40)

    def test_selects_only_complete_immutable_main_inputs(self):
        value = self.select()
        self.assertEqual(value['run_id'], 42)
        self.assertEqual(len(value['artifacts']), 4)

    def test_failed_pending_fork_wrong_workflow_source_branch_or_event_is_refused(self):
        for key, value in [('head_sha', 'b' * 40), ('head_branch', 'feature'), ('event', 'pull_request'),
                           ('status', 'in_progress'), ('conclusion', 'failure'), ('workflow_id', 10),
                           ('path', '.github/workflows/other.yml'), ('repository', {'full_name': 'fork/repo'})]:
            with self.subTest(key=key), self.assertRaises(gate.Refused):
                self.select([self.run | {key: value}])
        for runs in ([], [self.run, self.run]):
            with self.assertRaises(gate.Refused):
                self.select(runs)

    def test_missing_expired_ambiguous_undigested_or_wrong_source_artifacts_are_refused(self):
        original = copy.deepcopy(self.artifacts)
        for index in range(4):
            for changes in ({'expired': True}, {'digest': ''}, {'workflow_run': {'head_sha': 'b' * 40}}):
                self.artifacts = copy.deepcopy(original)
                self.artifacts[index].update(changes)
                with self.subTest(index=index, changes=changes), self.assertRaises(gate.Refused):
                    self.select()
            self.artifacts = original[:index] + original[index + 1:]
            with self.assertRaises(gate.Refused):
                self.select()
        self.artifacts = original + [original[0]]
        with self.assertRaises(gate.Refused):
            self.select()


class Promotion(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.candidate = self.root / 'main'; self.candidate.mkdir()
        self.output = self.root / 'tag'
        self.metadata = {'schema': 3, 'repository': gate.REPOSITORY, 'source_sha': 'a' * 40,
                         'tag': 'v0.1.0', 'tag_object': '0' * 40, 'channels': ['github'],
                         'targets': sorted(gate.TARGETS)}
        (self.candidate / 'archive.tar.xz').write_bytes(b'qualified archive fixture')
        contract.freeze(self.candidate, self.metadata)
        self.checksums = gate.digest((self.candidate / contract.CHECKSUMS).read_bytes())
        for name, target in qualified.PROOFS.items():
            directory = self.root / name; directory.mkdir()
            (directory / 'result.json').write_bytes(gate.canonical({
                'native': target, 'source_sha': 'a' * 40, 'checksums_sha256': self.checksums,
                'install': '0.1.0', 'upgrade': '0.1.0', 'uninstall': True,
                'binary_unchanged': True, 'data_and_service_preserved': True}))

    def promote(self, advisories=None):
        def freeze(root, value, tag, source):
            contract.freeze(root, self.metadata | {'tag_object': value['tag_object']})
        with patch.object(qualified.supply, 'require_contract') as supply, \
                patch.object(qualified.supply, 'verify_binary_inventory') as binary, \
                patch.object(qualified.release_run, 'freeze_payload', side_effect=freeze):
            qualified.promote(self.candidate, self.root, self.output, {'run_id': 42},
                              {'channels': ['github'], 'tag_object': 'b' * 40}, 'v0.1.0', 'a' * 40, advisories)
            supply.assert_called_once()
            binary.assert_called_once()

    def test_preserves_original_candidate_and_archive_bytes_binds_real_tag_and_receipt(self):
        before = {p.name: p.read_bytes() for p in self.candidate.iterdir()}
        self.promote()
        self.assertEqual(contract.load(self.output)['tag_object'], 'b' * 40)
        self.assertEqual((self.output / 'archive.tar.xz').read_bytes(), before['archive.tar.xz'])
        self.assertEqual({p.name: p.read_bytes() for p in self.candidate.iterdir()}, before)
        receipt = gate.strict_json((self.output / 'qualified-main-inputs.json').read_bytes())
        self.assertEqual(receipt, {'run_id': 42, 'checksums_sha256': self.checksums})

    def test_corrupt_main_bytes_cannot_be_refrozen(self):
        (self.candidate / 'archive.tar.xz').write_bytes(b'changed')
        with self.assertRaises(gate.Refused):
            self.promote()
        self.assertFalse(self.output.exists())

    def test_fresh_audit_reports_join_the_new_checksum_closure(self):
        advisories = self.root / 'audits'; advisories.mkdir()
        reports = {'rust-advisories.json': '{}', 'rust-advisory-revision.txt': 'e' * 40,
                   'web-advisories.json': '{}'}
        for name, data in reports.items(): (advisories / name).write_text(data)
        self.promote(advisories)
        contract.load(self.output)
        self.assertEqual(gate.strict_json((self.output / 'tag-advisories.json').read_bytes()), reports)

    def test_missing_fresh_audits_cannot_be_frozen(self):
        advisories = self.root / 'audits'; advisories.mkdir()
        with self.assertRaisesRegex(gate.Refused, 'audit reports missing'): self.promote(advisories)
        self.assertFalse((self.output / contract.CHECKSUMS).exists())

    def test_tag_source_signed_placeholder_channels_and_target_must_match(self):
        for key, value in [('source_sha', 'c' * 40), ('tag', 'v0.2.0'), ('tag_object', 'b' * 40),
                           ('channels', ['crates', 'github']), ('targets', [sorted(gate.TARGETS)[0]])]:
            (self.candidate / contract.METADATA).unlink()
            (self.candidate / contract.CHECKSUMS).unlink()
            contract.freeze(self.candidate, self.metadata | {key: value})
            with self.subTest(key=key), self.assertRaises(gate.Refused):
                self.promote()
            self.assertFalse(self.output.exists())

    def test_each_native_proof_must_bind_the_exact_candidate_and_success(self):
        for name in qualified.PROOFS:
            path = self.root / name / 'result.json'
            raw = path.read_bytes(); proof = gate.strict_json(raw)
            for key, value in [('source_sha', 'b' * 40), ('native', 'other'), ('checksums_sha256', 'c' * 64),
                               ('install', '0.0.0'), ('upgrade', '0.0.0'), ('uninstall', False),
                               ('binary_unchanged', False), ('data_and_service_preserved', False)]:
                path.write_bytes(gate.canonical(proof | {key: value}))
                with self.subTest(name=name, key=key), self.assertRaises(gate.Refused):
                    self.promote()
                self.assertFalse(self.output.exists())
            path.write_bytes(raw)


class Download(unittest.TestCase):
    def test_zip_digest_flat_regular_unique_entries_and_corruption_are_checked(self):
        for kind in ('valid', 'digest', 'traversal', 'symlink', 'duplicate'):
            raw = io.BytesIO()
            with zipfile.ZipFile(raw, 'w') as archive:
                entry = zipfile.ZipInfo('../outside' if kind == 'traversal' else 'result.json')
                if kind == 'symlink': entry.external_attr = (stat.S_IFLNK | 0o777) << 16
                archive.writestr(entry, b'{}')
                if kind == 'duplicate': archive.writestr('RESULT.JSON', b'{}')
            content = raw.getvalue()
            artifact = {'id': 5, 'digest': 'sha256:' + ('0' * 64 if kind == 'digest' else gate.digest(content))}
            def transport(command, stdout, **kwargs):
                self.assertIn('/actions/artifacts/5/zip?_', command[-1])
                stdout.write(content)
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as tmp, \
                    patch.object(qualified.subprocess, 'run', side_effect=transport):
                if kind == 'valid':
                    qualified.download(artifact, Path(tmp) / 'download')
                    self.assertEqual((Path(tmp) / 'download/result.json').read_bytes(), b'{}')
                else:
                    with self.assertRaises(gate.Refused): qualified.download(artifact, Path(tmp) / 'download')
