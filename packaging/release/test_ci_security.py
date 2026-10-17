"""Keep release qualification and fresh dependency checks runnable before tagging."""
from pathlib import Path
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[2]


class CiSecurity(unittest.TestCase):
    def workflow(self, name):
        return yaml.safe_load((ROOT / '.github/workflows' / name).read_bytes())

    def test_audits_run_on_lock_changes_and_schedule_without_publication_authority(self):
        data = self.workflow('nix.yml')
        trigger = data.get('on', data.get(True))
        self.assertTrue(trigger['schedule'][0]['cron'])
        self.assertEqual(data['permissions'], {'contents': 'read'})
        ci = self.workflow('ci.yml')['jobs']['checks']
        audit_steps = [s for s in ci['steps'] if 'Fresh ' in s.get('name', '')]
        self.assertEqual({s['if'] for s in audit_steps}, {
            "steps.paths.outputs.audit_rust == 'true'", "steps.paths.outputs.audit_web == 'true'"})
        jobs = {'audit': data['jobs']['audit'], 'checks': ci}
        commands = '\n'.join(s.get('run', '') for j in jobs.values() for s in j['steps'])
        self.assertIn('mktemp -d', commands)
        self.assertIn('cargo-audit audit --file Cargo.lock --db "$audit_db"', commands)
        self.assertNotIn('--no-fetch', commands)
        self.assertNotIn('--stale', commands)
        self.assertNotIn('--ignore', commands)
        self.assertIn('pnpm --dir web audit', commands)
        self.assertNotIn('--offline', commands)
        self.assertNotIn('continue-on-error', str(data))
        for job in jobs.values():
            self.assertGreater(job['timeout-minutes'], 0)
            self.assertNotIn('environment', job)
            for step in job['steps']:
                if step.get('uses', '').startswith('actions/checkout@'):
                    self.assertIs(step['with']['persist-credentials'], False)
