"""Exercise the source-policy harness with a fake ACP process, never real agents."""
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class McpSourcePolicy(unittest.TestCase):
    def run_fixture(self, wrong_mode_on=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            package = root / 'modules/@agentclientprotocol/claude-agent-acp'
            (package / 'dist').mkdir(parents=True)
            manifest = json.loads((ROOT / 'crates/hennery-host/manifest.json').read_text())
            (package / 'package.json').write_text(json.dumps({
                'type': 'module', 'version': manifest['adapters']['claude']['version']}))
            lock = json.loads((ROOT / 'adapters/claude/package-lock.json').read_text())
            sdk = root / 'modules/@anthropic-ai/claude-agent-sdk'
            sdk.mkdir(parents=True)
            (sdk / 'package.json').write_text(json.dumps({
                'version': lock['packages']['node_modules/@anthropic-ai/claude-agent-sdk']['version']}))
            (package / 'scenario.json').write_text(json.dumps({'wrongModeOn': wrong_mode_on}))
            shutil.copyfile(Path(__file__).with_name('fixtures') / 'mcp-source-adapter.mjs',
                            package / 'dist/index.js')
            result = subprocess.run([shutil.which('node'), str(ROOT / 'packaging/test-mcp-source-policy.mjs'),
                                     str(root / 'modules'), str(root / 'output')],
                                    text=True, capture_output=True, timeout=20)
            trace = [json.loads(line) for line in (package / 'trace.jsonl').read_text().splitlines()]
            matrix = root / 'output/runtime-matrix.json'
            return result, trace, json.loads(matrix.read_text()) if matrix.exists() else None

    def test_manual_permissions_work_for_every_new_and_loaded_session(self):
        result, trace, matrix = self.run_fixture()
        self.assertEqual(result.returncode, 0, result.stderr)
        sessions = [row for row in trace if row.get('method') in ('session/new', 'session/load')]
        self.assertEqual(len(sessions), 8)
        self.assertEqual({row['mode'] for row in sessions}, {'default'})
        self.assertEqual(sum(row.get('permission') == 'allow_once' for row in trace), 8)
        self.assertEqual(len(matrix['results']), 8)

    def test_a_nonmanual_readback_refuses_before_the_affected_prompt(self):
        for method, earlier_prompts in [('session/new', 0), ('session/load', 1)]:
            with self.subTest(method=method):
                result, trace, matrix = self.run_fixture(method)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('Manual (default) permission mode', result.stderr)
                self.assertEqual(sum(row.get('method') == 'session/prompt' for row in trace), earlier_prompts)
                self.assertIsNone(matrix)
