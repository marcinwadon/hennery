"""Prevent the build-tool exception from hiding production or new findings."""
from copy import deepcopy
from datetime import date
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('audit_web', Path(__file__).resolve().parents[1] / 'audit_web.py')
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)
TODAY = date(2026, 11, 18)


class AuditWeb(unittest.TestCase):
    def setUp(self):
        self.advisory = {'github_advisory_id': audit.ADVISORY, 'module_name': 'picomatch',
                         'patched_versions': None, 'patched_versions_unpublished': True, 'severity': 'high',
                         'findings': [{'dev': True, 'version': '4.0.7',
                                       'paths': ['.>vitest>picomatch', '.>vite>tinyglobby>picomatch']}]}
        self.report = {'advisories': {'1247710': self.advisory},
                       'metadata': {'vulnerabilities': dict.fromkeys(audit.SEVERITIES, 0)}}
        self.report['metadata']['vulnerabilities']['high'] = 1

    def test_accepts_only_current_dev_tool_finding_until_expiry(self):
        self.assertTrue(audit.accepted(self.report, 1, TODAY))
        self.assertFalse(audit.accepted(self.report, 1, audit.EXPIRES))

    def test_rejects_production_and_unrecognised_paths(self):
        for changes in ({'dev': False}, {'paths': ['.>react>picomatch']}, {'paths': []}, {'version': '4.0.8'}):
            with self.subTest(changes=changes):
                report = deepcopy(self.report)
                report['advisories']['1247710']['findings'][0].update(changes)
                self.assertFalse(audit.accepted(report, 1, TODAY))

    def test_rejects_available_patch_and_other_advisories(self):
        for changes in ({'patched_versions': '>=4.0.8'}, {'patched_versions_unpublished': False},
                        {'github_advisory_id': 'GHSA-new'}, {'module_name': 'other'}, {'findings': []}):
            with self.subTest(changes=changes):
                report = deepcopy(self.report)
                report['advisories']['1247710'].update(changes)
                self.assertFalse(audit.accepted(report, 1, TODAY))
        self.report['advisories']['new'] = deepcopy(self.advisory)
        self.assertFalse(audit.accepted(self.report, 1, TODAY))

    def test_rejects_transport_errors_and_incomplete_reports(self):
        self.assertFalse(audit.accepted(self.report, 2, TODAY))
        self.assertFalse(audit.accepted(self.report, 0, TODAY))
        self.assertFalse(audit.accepted({}, 0, TODAY))
        self.report['error'] = 'registry unavailable'
        self.assertFalse(audit.accepted(self.report, 1, TODAY))

    def test_cli_retains_the_original_high_finding(self):
        raw = json.dumps(self.report) + '\n'
        output = io.StringIO()
        with patch.object(audit.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1, raw, '')), \
                patch.object(audit, 'date') as clock, patch.object(audit.sys, 'stdout', output), \
                patch.object(audit.sys, 'stderr', io.StringIO()):
            clock.today.return_value = TODAY
            self.assertEqual(audit.main(), 0)
        self.assertEqual(output.getvalue(), raw)
        self.assertEqual(json.loads(output.getvalue())['metadata']['vulnerabilities']['high'], 1)

    def test_clean_report_requires_success_and_zero_counts(self):
        self.report['advisories'] = {}
        self.assertFalse(audit.accepted(self.report, 0, TODAY))
        self.report['metadata']['vulnerabilities']['high'] = 0
        self.assertTrue(audit.accepted(self.report, 0, TODAY))
        self.assertFalse(audit.accepted(self.report, 1, TODAY))


if __name__ == '__main__':
    unittest.main()
