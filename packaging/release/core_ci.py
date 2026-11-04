#!/usr/bin/env python3
"""Wait for successful main-push CI for the exact signed source commit."""
import os
import subprocess
import time
import uuid
from urllib.parse import urlencode

import gate


def main():
    source = os.environ['GITHUB_SHA']
    subprocess.run(['git', 'merge-base', '--is-ancestor', source, 'refs/remotes/origin/main'], check=True)
    api = gate.GitHub(os.environ['GH_TOKEN'])
    query = urlencode({'event': 'push', 'branch': 'main', 'head_sha': source})
    deadline = time.monotonic() + 45 * 60
    while True:
        runs = api.get('/actions/workflows/ci.yml/runs?' + query + '&_=' + uuid.uuid4().hex)['workflow_runs']
        exact = [run for run in runs if run.get('head_sha') == source and run.get('head_branch') == 'main'
                 and run.get('event') == 'push']
        if any(run.get('status') == 'completed' and run.get('conclusion') == 'success' for run in exact):
            print('Successful main CI verified for ' + source)
            return
        gate.require(not exact or any(run.get('status') != 'completed' for run in exact),
                     'core CI failed for signed source; resolve CI before releasing')
        gate.require(time.monotonic() < deadline, 'main CI is missing or pending; it must finish before release')
        print('Waiting for main CI for ' + source, flush=True)
        time.sleep(30)


if __name__ == '__main__':
    main()
