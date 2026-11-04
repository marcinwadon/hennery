#!/usr/bin/env python3
"""Watch exact Actions runs without a model; emit changes and an actionable result."""
import argparse
import datetime
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import uuid


class WatchError(Exception):
    pass


def api(repo, route):
    # Intermediary caches can retain a queued response after the run completes.
    route += ('&' if '?' in route else '?') + '_=' + uuid.uuid4().hex
    result = subprocess.run(
        ['gh', 'api', '--paginate', '--slurp', f'repos/{repo}/{route}'],
        capture_output=True, text=True, timeout=30,
    )
    if result.returncode:
        # CLI stderr may contain credentials or untrusted remote content.
        raise WatchError('GitHub API request failed; check authentication and connectivity')
    try:
        return json.loads(result.stdout)
    except ValueError as error:
        raise WatchError('GitHub returned invalid JSON') from error


def snapshot(repo, sha, run_ids, request=api):
    runs = []
    for run_id in run_ids:
        pages = request(repo, f'actions/runs/{run_id}')
        if len(pages) != 1:
            raise WatchError('Expected one workflow run')
        run = pages[0]
        if run.get('id') != run_id or run.get('head_sha') != sha:
            raise WatchError(f'Run {run_id} does not match the requested source')
        attempt = run.get('run_attempt')
        if not isinstance(attempt, int) or attempt < 1:
            raise WatchError('Workflow run has no valid attempt')
        pages = request(repo, f'actions/runs/{run_id}/attempts/{attempt}/jobs?per_page=100')
        jobs = []
        for page in pages:
            jobs.extend({key: job.get(key) for key in ('id', 'name', 'status', 'conclusion')}
                        for job in page['jobs'])
        runs.append({key: run.get(key) for key in ('id', 'name', 'status', 'conclusion', 'run_attempt')}
                    | {'jobs': sorted(jobs, key=lambda job: job['id'])})
    failed = any(
        run['status'] == 'completed' and run['conclusion'] != 'success'
        or any(job['status'] == 'completed' and job['conclusion'] not in ('success', 'skipped')
               for job in run['jobs'])
        for run in runs
    )
    complete = all(run['status'] == 'completed' and run['conclusion'] == 'success' for run in runs)
    return {'repo': repo, 'sha': sha, 'runs': runs,
            'outcome': 'failed' if failed else 'success' if complete else 'pending'}


def write_state(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=path.name + '.', dir=path.parent)
    try:
        with os.fdopen(fd, 'w') as stream:
            json.dump(value, stream, sort_keys=True)
            stream.write('\n')
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def watch(repo, sha, run_ids, state, interval=30, timeout=7200, stall_after=1800,
          request=api, now=time.monotonic, sleep=time.sleep, emit=print):
    started = changed = now()
    previous = None
    failures = 0
    while True:
        if now() - started >= timeout:
            current = {'repo': repo, 'sha': sha, 'outcome': 'timeout', 'runs': list(run_ids)}
        else:
            try:
                current = snapshot(repo, sha, run_ids, request)
                failures = 0
            except (WatchError, OSError, subprocess.TimeoutExpired, ValueError, KeyError, TypeError):
                failures += 1
                current = {'repo': repo, 'sha': sha, 'outcome': 'api_error',
                           'runs': list(run_ids), 'consecutive_errors': failures}
        if current != previous:
            changed = now()
            previous = current
            event = current | {'observed_at': datetime.datetime.now(datetime.timezone.utc).isoformat()}
            write_state(state, event)
            emit(json.dumps(event, sort_keys=True))
        outcome = current['outcome']
        if outcome in ('success', 'failed', 'timeout') or failures >= 3:
            return {'success': 0, 'failed': 1, 'timeout': 3, 'api_error': 2}[outcome]
        if outcome == 'pending' and now() - changed >= stall_after:
            current = current | {'outcome': 'stalled'}
            write_state(state, current)
            emit(json.dumps(current, sort_keys=True))
            return 3
        sleep(min(interval, max(0, timeout - (now() - started))))


def positive(value):
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError('must be a finite positive number')
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', required=True)
    parser.add_argument('--sha', required=True)
    parser.add_argument('--run', type=int, action='append', required=True)
    parser.add_argument('--state', type=Path, required=True)
    parser.add_argument('--interval', type=positive, default=30)
    parser.add_argument('--timeout', type=positive, default=7200)
    parser.add_argument('--stall-after', type=positive, default=1800)
    parser.add_argument('--notify', nargs=argparse.REMAINDER,
                        help='local callback argv, invoked once on exit with the state path appended')
    args = parser.parse_args()
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', args.repo):
        parser.error('repo must be owner/name')
    if not re.fullmatch(r'[0-9a-f]{40}', args.sha) or any(run < 1 for run in args.run):
        parser.error('supply a full source SHA and positive run IDs')
    code = watch(args.repo, args.sha, sorted(set(args.run)), args.state,
                 args.interval, args.timeout, args.stall_after)
    if args.notify:
        try:
            result = subprocess.run([*args.notify, str(args.state.resolve())], timeout=30)
            if result.returncode:
                return 4
        except (OSError, subprocess.TimeoutExpired):
            return 4
    return code


if __name__ == '__main__':
    sys.exit(main())
