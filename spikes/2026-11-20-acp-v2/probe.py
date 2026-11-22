"""Throwaway initialize-only probe. No login, sessions, prompts, or real HOME."""
import glob
import json
import os
import subprocess
import tempfile
import selectors

for agent, version in [('codex-acp', '2.7.0'), ('claude-agent-acp', '0.107.3')]:
    packages = glob.glob(os.path.expanduser(
        '~/.local/share/hennery/adapters/sets/*/*/node_modules/@agentclientprotocol/'
        + agent + '/package.json'))
    package = next(p for p in packages if json.load(open(p))['version'] == version)
    entry = os.path.join(os.path.dirname(package), 'dist/index.js')
    for protocol in (1, 2):
        with tempfile.TemporaryDirectory(prefix='acpv2-probe-') as home:
            env = {k: v for k, v in os.environ.items()
                   if k in ('PATH', 'LANG', 'LD_LIBRARY_PATH')}
            env.update(HOME=home, XDG_CONFIG_HOME=home, XDG_CACHE_HOME=home)
            info = {'name': 'hennery-audit', 'version': '0.0.0'}
            params = ({'protocolVersion': 1, 'clientCapabilities': {}, 'clientInfo': info}
                      if protocol == 1 else
                      {'protocolVersion': 2, 'capabilities': {}, 'info': info})
            wire = json.dumps({'jsonrpc': '2.0', 'id': 1,
                               'method': 'initialize', 'params': params}) + '\n'
            proc = subprocess.Popen(['node', entry], stdin=subprocess.PIPE,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                    text=True, cwd=home, env=env)
            try:
                proc.stdin.write(wire)
                proc.stdin.flush()
                selector = selectors.DefaultSelector()
                selector.register(proc.stdout, selectors.EVENT_READ)
                ready = selector.select(timeout=15)
                out = proc.stdout.readline() if ready else ''
                selector.close()
                proc.terminate()
                _, err = proc.communicate(timeout=5)
                print(json.dumps({'agent': agent, 'version': version,
                                  'requested': protocol, 'exit': proc.returncode,
                                  'stdout': out, 'stderr': err}))
            except subprocess.TimeoutExpired:
                proc.kill()
                out, err = proc.communicate()
                print(json.dumps({'agent': agent, 'requested': protocol,
                                  'timeout': True, 'stdout': out, 'stderr': err}))
