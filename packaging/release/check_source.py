"""Refuse a native archive whose executable lost the frozen source identity."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile


def version_output(binary):
    # Even --version uses the same disposable home/data boundary as other
    # executable acceptance checks; never inherit operator configuration.
    with tempfile.TemporaryDirectory(prefix='hennery-source-') as home:
        environment = {key: os.environ[key] for key in ('PATH', 'LANG', 'LC_ALL', 'LC_CTYPE', 'TMPDIR') if key in os.environ}
        environment.update(HOME=home, **{key: str(Path(home) / key.lower()) for key in
                           ('XDG_DATA_HOME', 'XDG_CONFIG_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME')})
        return subprocess.check_output([str(Path(binary).resolve()), '--version'], text=True, env=environment, timeout=120)


def check(binary, identity):
    expected = json.loads(Path(identity).read_text())['source_sha']
    if not isinstance(expected, str) or re.fullmatch(r'(?:[a-fA-F0-9]{40}|[a-fA-F0-9]{64})', expected) is None:
        raise ValueError('build identity has no hexadecimal source revision')
    output = version_output(binary)
    sources = re.findall(r'^source: (.*)$', output, re.MULTILINE)
    if sources != [expected]:
        raise ValueError('native binary source revision differs from build identity')


if __name__ == '__main__':
    try:
        check(*sys.argv[1:])
    except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print(f'FAIL: native source identity: {error}', file=sys.stderr)
        raise SystemExit(1)
    print('ok: native binary source matches build identity')
