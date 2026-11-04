"""Check the acceptance driver without downloading agents or using credentials."""
import importlib.util
import json
import os
import signal
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('runtime_acceptance',
        Path(__file__).resolve().parents[1] / 'runtime_acceptance.py')
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


class RuntimeAcceptance(unittest.TestCase):
    def test_external_reports_require_all_platforms_cases_and_exact_source_inputs(self):
        rows = [{'case': name, 'exit': 0} for name in ('build', 'install', 'mcp-source-policy')]
        rows += [{'case': '-'.join(part for part in row if part), 'exit': 0} for row in driver.cases()]
        with tempfile.TemporaryDirectory() as directory:
            paths = []
            for platform in ('darwin-arm64', 'linux-arm64', 'linux-x64'):
                path = Path(directory) / platform
                path.write_text(json.dumps({'source': 'a' * 40, 'inputs': {'manifest': 'digest'},
                                            'platform': platform, 'outcome': 'success', 'results': rows}))
                paths.append(path)
            self.assertEqual(driver.verify_reports(paths, 'a' * 40, {'manifest': 'digest'})['outcome'], 'success')
            for candidates, source, inputs in ((paths[:2], 'a' * 40, {'manifest': 'digest'}),
                                               (paths, 'b' * 40, {'manifest': 'digest'}),
                                               (paths, 'a' * 40, {'manifest': 'changed'}),
                                               ([paths[0]] * 3, 'a' * 40, {'manifest': 'digest'})):
                with self.subTest(source=source, reports=len(candidates)):
                    with self.assertRaises(ValueError):
                        driver.verify_reports(candidates, source, inputs)
            broken = json.loads(paths[0].read_text())
            broken['results'] = rows[:-1]
            paths[0].write_text(json.dumps(broken))
            with self.assertRaises(ValueError):
                driver.verify_reports(paths, 'a' * 40, {'manifest': 'digest'})

    def test_complete_fixed_matrix_has_sixteen_distinct_invocations(self):
        cases = driver.cases()
        self.assertEqual(len(cases), 16)
        self.assertEqual(len(set(cases)), 16)
        matrix = {(parent, child) for test, parent, child in cases if child}
        self.assertEqual(matrix, {('claude', 'claude'), ('claude', 'codex'),
                                  ('codex', 'claude'), ('codex', 'codex')})
        self.assertEqual(sum('delete' in test for test, _, _ in cases), 1)

    def test_cargo_output_must_produce_both_real_test_executables(self):
        lines = '\n'.join(json.dumps({'reason': 'compiler-artifact', 'target': {'name': name},
                                      'executable': '/target/' + name})
                          for name in ('delegation_live', 'forget_codex'))
        self.assertEqual(set(driver.artifacts('noise\n' + lines)), {'delegation_live', 'forget_codex'})
        with self.assertRaises(ValueError):
            driver.artifacts(lines.splitlines()[0])

    def test_a_missing_exact_test_cannot_pass_as_zero_tests(self):
        success = {'exit': 0, 'seconds': 0}
        self.assertEqual(driver.qualify_case(success, 'test result: ok. 0 passed;')['exit'], 1)
        self.assertEqual(driver.qualify_case(success, 'test result: ok. 1 passed;')['exit'], 0)
        self.assertEqual(driver.qualify_case({'exit': 37}, 'test result: ok. 1 passed;')['exit'], 37)

    def test_auth_and_data_overrides_do_not_enter_test_environment(self):
        env = driver.isolated_env({'PATH': '/bin', 'HOME': '/real', 'HENNERY_DATA_DIR': '/live',
                                   'ANTHROPIC_API_KEY': 'sensitive', 'CODEX_HOME': '/real-codex',
                                   'CLAUDE_CONFIG_DIR': '/real-claude'},
                                  Path('/scratch'), Path('/runtime/bin/node'), Path('/managed'),
                                  Path('/auth/claude'), Path('/auth/codex'))
        for name in ('HENNERY_DATA_DIR', 'ANTHROPIC_API_KEY', 'CODEX_HOME', 'CLAUDE_CONFIG_DIR'):
            self.assertNotIn(name, env)
        self.assertEqual(env['HOME'], '/scratch')
        self.assertTrue(env['PATH'].startswith('/runtime/bin:'))

    def test_real_child_output_is_redacted_and_retained_privately(self):
        secret = 'fixture-provider-token'
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'case.log'
            result, text = driver.execute([sys.executable, '-c', 'print("fixture-provider-token")'],
                                          dict(os.environ), log, 10, [secret])
            self.assertEqual(result['exit'], 0)
            self.assertEqual(text.strip(), '[REDACTED]')
            self.assertEqual(log.stat().st_mode & 0o777, 0o600)
            self.assertEqual(log.read_text(), text)

    def test_child_failure_and_timeout_are_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            result, _ = driver.execute([sys.executable, '-c', 'raise SystemExit(37)'],
                                      dict(os.environ), Path(directory) / 'failed', 10)
            self.assertEqual(result['exit'], 37)
            result, _ = driver.execute([sys.executable, '-c', 'import time; time.sleep(30)'],
                                      dict(os.environ), Path(directory) / 'timeout', .1)
            self.assertEqual(result['exit'], 124)

    def test_timeout_cleanup_follows_owned_descendants_across_process_groups(self):
        process = type('Process', (), {'pid': 10, 'wait': lambda self: None})()
        with patch.object(driver.os, 'kill') as kill, patch.object(driver.subprocess, 'check_output',
                side_effect=['10 1\n11 10\n99 1\n', '10 1\n11 10\n12 11\n99 1\n',
                             '10 1\n11 10\n12 11\n99 1\n']):
            driver.terminate_owned_tree(process)
        killed = {call.args[0] for call in kill.call_args_list if call.args[1] == signal.SIGKILL}
        self.assertEqual(killed, {10, 11, 12})

    def test_platform_must_match_requested_host_before_any_work(self):
        with patch.object(driver, 'host_platform', return_value='darwin-arm64'):
            with self.assertRaises(ValueError):
                driver.run(type('Args', (), {'platform': 'linux-x64', 'allow_real_agents': True})())

    def test_real_agents_need_operator_opt_in_and_are_refused_in_ci(self):
        with self.assertRaises(ValueError):
            driver.run(type('Args', (), {'allow_real_agents': False})())
        with patch.dict(os.environ, {'CI': 'true'}):
            with self.assertRaises(ValueError):
                driver.run(type('Args', (), {'allow_real_agents': True})())
