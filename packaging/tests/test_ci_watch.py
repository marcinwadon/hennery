"""Exercise transitions, API pagination and failure handling without clock waits."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import subprocess
import sys
import unittest

spec = importlib.util.spec_from_file_location('ci_watch', Path(__file__).resolve().parents[1] / 'ci_watch.py')
watcher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(watcher)
SHA = 'a' * 40


class Watch(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.state = Path(self.temp.name) / 'nested/state.json'
        self.clock = 0
        self.events = []
        self.routes = []
        self.iteration = 0

    def request(self, repo, route):
        self.routes.append(route)
        if route == 'actions/runs/12':
            self.iteration += 1
            done = self.iteration >= 4
            return [{'id': 12, 'head_sha': SHA, 'run_attempt': 2,
                     'status': 'completed' if done else 'in_progress',
                     'conclusion': 'success' if done else None}]
        self.assertEqual(route, 'actions/runs/12/attempts/2/jobs?per_page=100')
        return [{'jobs': [{'id': 1, 'name': 'Rust', 'status': 'completed', 'conclusion': 'success'}]},
                {'jobs': [{'id': 2, 'name': 'Nix', 'status': 'in_progress', 'conclusion': None}]}]

    def sleep(self, seconds):
        self.clock += seconds

    def run_watch(self, request=None, **kwargs):
        return watcher.watch('owner/repo', SHA, [12], self.state, interval=10,
                             request=request or self.request, now=lambda: self.clock,
                             sleep=self.sleep, emit=self.events.append, **kwargs)

    def test_unchanged_polls_are_silent_and_success_is_saved(self):
        self.assertEqual(self.run_watch(), 0)
        self.assertEqual(self.iteration, 4)
        self.assertEqual(len(self.events), 2)
        state = json.loads(self.state.read_text())
        self.assertEqual(state['outcome'], 'success')
        self.assertEqual(len(state['runs'][0]['jobs']), 2)
        self.assertEqual(list(self.state.parent.iterdir()), [self.state])

    def test_cancelled_run_is_not_success(self):
        def request(repo, route):
            pages = self.request(repo, route)
            if route == 'actions/runs/12':
                pages[0].update(status='completed', conclusion='cancelled')
            return pages
        self.assertEqual(self.run_watch(request), 1)

    def test_failed_job_returns_before_other_jobs_finish(self):
        def request(repo, route):
            pages = self.request(repo, route)
            if '/jobs?' in route:
                pages[1]['jobs'][0].update(status='completed', conclusion='failure')
            return pages
        self.assertEqual(self.run_watch(request), 1)
        self.assertEqual(self.iteration, 1)

    def test_wrong_source_never_becomes_success(self):
        def request(repo, route):
            pages = self.request(repo, route)
            pages[0]['head_sha'] = 'b' * 40
            return pages
        self.assertEqual(self.run_watch(request), 2)
        self.assertEqual(json.loads(self.state.read_text())['outcome'], 'api_error')

    def test_transient_api_failure_recovers(self):
        def request(repo, route):
            if self.clock == 0:
                raise watcher.WatchError('temporary')
            return self.request(repo, route)
        self.assertEqual(self.run_watch(request), 0)
        self.assertEqual(json.loads(self.events[0])['outcome'], 'api_error')

    def test_stalled_job_wakes_the_caller_with_its_actual_state(self):
        self.assertEqual(self.run_watch(stall_after=15), 3)
        state = json.loads(self.state.read_text())
        self.assertEqual(state['outcome'], 'stalled')
        self.assertEqual(state['runs'][0]['id'], 12)

    def test_deadline_is_not_reset_by_status_changes(self):
        self.assertEqual(self.run_watch(timeout=15), 3)
        self.assertEqual(self.clock, 15)
        self.assertEqual(json.loads(self.state.read_text())['outcome'], 'timeout')

    def test_cli_reads_gh_pages_and_invokes_callback_once(self):
        root = Path(self.temp.name)
        gh = root / 'gh'
        gh.write_text('#!' + sys.executable + '\n' + '''import json, sys
if '/jobs?' in sys.argv[-1]:
    print(json.dumps([{'jobs': []}]))
else:
    print(json.dumps([{'id': 12, 'head_sha': 'a' * 40, 'run_attempt': 1,
                      'status': 'completed', 'conclusion': 'success'}]))
''')
        gh.chmod(0o700)
        callback = root / 'callback.py'
        callback.write_text('import json, pathlib, sys\n'
                            'assert json.loads(pathlib.Path(sys.argv[2]).read_text())["outcome"] == "success"\n'
                            'with pathlib.Path(sys.argv[1]).open("a") as f: f.write("called\\n")\n')
        called = root / 'called'
        result = subprocess.run([sys.executable, str(Path(watcher.__file__)), '--repo', 'owner/repo',
                                 '--sha', SHA, '--run', '12', '--state', str(self.state), '--notify',
                                 sys.executable, str(callback), str(called)],
                                env=dict(os.environ, PATH=str(root) + os.pathsep + os.environ['PATH']),
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(called.read_text(), 'called\n')
