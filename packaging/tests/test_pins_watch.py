"""Regression coverage for CLI-only drift and frozen npm resolutions."""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import tomllib
import unittest

SPEC = importlib.util.spec_from_file_location('pins_watch', Path(__file__).parents[1] / 'pins_watch.py')
watch = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(watch)


class PinsWatch(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        (self.root / 'adapters').mkdir()
        self.path = self.root / 'adapters/pins.toml'
        self.path.write_text('''npm_before = "2026-10-01T00:00:00Z"
[node]
version = "24.0.0"
[adapters.claude]
package = "@agentclientprotocol/claude-agent-acp"
version = "0.93.0"
[adapters.codex]
package = "@agentclientprotocol/codex-acp"
version = "2.2.2"
[codex_app_server]
codex_version = "0.160.1"
delete_method = "thread/delete"
''')
        self.docs = {}
        for agent, adapter, runtime in [('claude', '0.93.0', '0.3.300'), ('codex', '2.2.2', '0.160.1')]:
            package = '@agentclientprotocol/' + ('claude-agent-acp' if agent == 'claude' else 'codex-acp')
            self.docs[package] = {'name': package, 'version': adapter}
            name = watch.RUNTIMES[agent]
            self.docs[name] = {'name': name, 'version': runtime}
            directory = self.root / f'adapters/{agent}'
            directory.mkdir()
            (directory / 'package-lock.json').write_text(json.dumps({'packages': {
                f'node_modules/{name}': {'version': runtime}}}))
        self.nodes = [{'version': 'v24.0.0', 'lts': 'Krypton'}]

    def get(self, url):
        if url == 'https://nodejs.org/dist/index.json':
            return self.nodes
        return self.docs[url.removeprefix('https://registry.npmjs.org/').removesuffix('/latest')]

    def discover(self):
        return watch.discover(self.root, self.get)

    def test_no_drift_proposes_nothing(self):
        self.assertEqual(self.discover(), [])

    def test_cli_and_sdk_only_releases_advance_cutoff_and_stop_after_locking(self):
        for agent, latest in [('claude', '0.3.312'), ('codex', '0.165.0')]:
            with self.subTest(agent=agent):
                self.docs[watch.RUNTIMES[agent]]['version'] = latest
                update, = self.discover()
                self.assertEqual(update['pins']['adapters'][agent]['runtime'], latest)
                self.assertEqual(self.discover()[0]['branch'], update['branch'])
                watch.apply(self.root, update)
                pins = tomllib.loads(self.path.read_text())
                self.assertGreater(pins['npm_before'], '2026-10-01T00:00:00Z')
                self.assertEqual(pins['adapters'][agent]['overrides'][watch.RUNTIMES[agent]], latest)
                self.assertEqual(pins['codex_app_server']['codex_version'],
                                 update['pins']['adapters']['codex']['runtime'])
                self.assertEqual(pins['codex_app_server']['delete_method'], 'thread/delete')
                lock = self.root / f'adapters/{agent}/package-lock.json'
                doc = json.loads(lock.read_text())
                doc['packages'][f'node_modules/{watch.RUNTIMES[agent]}']['version'] = latest
                lock.write_text(json.dumps(doc))
                self.assertEqual(self.discover(), [])

    def test_adapter_and_node_changes_share_one_set(self):
        self.docs['@agentclientprotocol/codex-acp']['version'] = '2.3.0'
        self.nodes += [{'version': 'v24.0.2', 'lts': 'Krypton'}, {'version': 'v25.1.0', 'lts': False},
                       {'version': 'v24.1.0-rc.1', 'lts': False}]
        update, = self.discover()
        self.assertEqual(update['pins']['node'], '24.0.2')
        self.assertEqual(update['pins']['adapters']['codex']['adapter'], '2.3.0')
        watch.apply(self.root, update)
        self.assertEqual(tomllib.loads(self.path.read_text())['adapters']['codex']['version'], '2.3.0')

    def test_downgrades_and_prerelease_tags_do_not_change_pins(self):
        for package in self.docs:
            for value in ['0.0.1', '99.0.0-beta.1', 'latest', '1.2.3\ninvalid']:
                with self.subTest(package=package, value=value):
                    saved = self.docs[package]['version']
                    self.docs[package]['version'] = value
                    self.assertEqual(self.discover(), [])
                    self.docs[package]['version'] = saved

    def test_apply_refuses_invalid_or_older_pins_and_cutoff_without_writing(self):
        self.docs[watch.RUNTIMES['codex']]['version'] = '0.165.0'
        update, = self.discover()
        before = self.path.read_bytes()
        for value in ['0.0.1', '^0.165.0', '0.165.0\n[evil]']:
            candidate = copy.deepcopy(update)
            candidate['pins']['adapters']['codex']['runtime'] = value
            with self.assertRaises(ValueError):
                watch.apply(self.root, candidate)
            self.assertEqual(self.path.read_bytes(), before)
        update['npm_before'] = '2026-09-01T00:00:00Z'
        with self.assertRaises(ValueError):
            watch.apply(self.root, update)
        self.assertEqual(self.path.read_bytes(), before)

    def test_registry_failure_or_wrong_package_cannot_propose_a_partial_update(self):
        self.docs[watch.RUNTIMES['claude']]['version'] = '0.3.312'
        del self.docs[watch.RUNTIMES['codex']]
        with self.assertRaises(KeyError):
            self.discover()
        self.docs[watch.RUNTIMES['codex']] = {'name': 'foreign', 'version': '99.0.0'}
        with self.assertRaises(ValueError):
            self.discover()


if __name__ == '__main__':
    unittest.main()
