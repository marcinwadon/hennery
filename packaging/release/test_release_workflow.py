"""Small static authority checks; actionlint checks Actions syntax and dependencies."""
from pathlib import Path
import re
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[2]


class ReleaseWorkflow(unittest.TestCase):
    def workflows(self):
        return {p.name: yaml.safe_load(p.read_bytes()) for p in (ROOT / '.github/workflows').glob('*.yml')}

    def test_release_trigger_is_tags_only(self):
        release = self.workflows()['release.yml']
        self.assertEqual(release.get('on', release.get(True)), {'push': {'tags': ['v*']}})

    def test_one_protected_publisher_and_scoped_permissions(self):
        publishers = []
        for name, workflow in self.workflows().items():
            self.assertEqual(workflow['permissions'], {'contents': 'read'}, name)
            for key, job in workflow['jobs'].items():
                permissions = job.get('permissions', workflow['permissions'])
                protected = job.get('environment') == 'release'
                if protected:
                    publishers.append((name, key))
                    self.assertEqual(permissions, {'contents': 'write', 'actions': 'read',
                                                   'attestations': 'read', 'id-token': 'write'})
                elif any(v == 'write' for v in permissions.values()):
                    self.assertEqual((name, key), ('release.yml', 'attest'))
                    self.assertEqual(permissions, {'contents': 'read', 'attestations': 'write', 'id-token': 'write'})
                if 'secrets.' in str(job):
                    self.assertTrue(protected, (name, key))
        self.assertEqual(publishers, [('release.yml', 'publish')])

    def test_remote_actions_are_commit_pinned(self):
        for name, workflow in self.workflows().items():
            for job in workflow['jobs'].values():
                for item in [job, *job.get('steps', [])]:
                    uses = item.get('uses')
                    if uses and not uses.startswith('./'):
                        self.assertRegex(uses, r'^[^@]+@[0-9a-f]{40}$', (name, uses))
