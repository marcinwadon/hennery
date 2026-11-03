"""Keep release qualification and fresh dependency checks runnable before tagging."""
from pathlib import Path
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[2]


class CiSecurity(unittest.TestCase):
    def workflow(self, name):
        return yaml.safe_load((ROOT / '.github/workflows' / name).read_bytes())

    def test_pin_generation_has_no_persisted_or_exported_write_token(self):
        workflow = self.workflow('pins-watch.yml')
        job = workflow['jobs']['generate']
        self.assertEqual(job['permissions'], {'contents': 'read'})
        self.assertNotIn('GH_TOKEN', job.get('env', {}))
        self.assertNotIn('GITHUB_TOKEN', job.get('env', {}))
        for step in job['steps']:
            if step.get('uses', '').startswith('actions/checkout@'):
                self.assertIs(step.get('with', {}).get('persist-credentials'), False)
            if step.get('uses', '').startswith('cachix/install-nix-action@'):
                self.assertNotIn('github_access_token', step.get('with', {}))
            if 'hennery-pins' in step.get('run', ''):
                self.assertNotIn('GH_TOKEN', step.get('env', {}))
                self.assertNotIn('GITHUB_TOKEN', step.get('env', {}))
        publisher = workflow['jobs']['bump']
        self.assertEqual(set(publisher['needs']), {'check', 'generate'})
        self.assertEqual(publisher['strategy']['matrix'], job['strategy']['matrix'])
        upload, = [s for s in job['steps'] if s.get('uses', '').startswith('actions/upload-artifact@')]
        download, = [s for s in publisher['steps'] if s.get('uses', '').startswith('actions/download-artifact@')]
        self.assertEqual(upload['with']['name'], download['with']['name'])
        self.assertEqual(download['with']['path'], '${{ runner.temp }}/pin-update')
        self.assertEqual(set(upload['with']['path'].splitlines()), {
            'adapters/pins.toml', 'adapters/claude/package.json', 'adapters/claude/package-lock.json',
            'adapters/codex/package.json', 'adapters/codex/package-lock.json', 'crates/hennery-host/manifest.json'})
        for step in publisher['steps']:
            if step.get('uses', '').startswith('actions/checkout@'):
                self.assertIs(step['with']['persist-credentials'], False)
            run = step.get('run', '')
            for executable in ['nix ', 'cargo ', 'npm ', 'node ']:
                self.assertNotIn(executable, run)
            if 'python' in run:
                self.assertEqual(run, 'python3 -I packaging/copy_pin_update.py "$RUNNER_TEMP/pin-update" "$GITHUB_WORKSPACE"')
            if 'git push ' in run or 'gh workflow run ' in run:
                self.assertEqual(step['env']['GH_TOKEN'], '${{ github.token }}')

    def test_scheduled_audit_checks_both_managed_adapter_locks(self):
        workflow = self.workflow('nix.yml')
        steps = workflow['jobs']['audit']['steps']
        audits = [step for step in steps
                  if 'packaging/audit-adapters.sh' in step.get('run', '')]
        self.assertEqual(len(audits), 1)
        self.assertEqual(audits[0]['run'],
                         'nix develop -c sh packaging/audit-adapters.sh')
        self.assertNotIn('if', audits[0])
        self.assertFalse(audits[0].get('continue-on-error', False))
        self.assertEqual(workflow['permissions'], {'contents': 'read'})

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
