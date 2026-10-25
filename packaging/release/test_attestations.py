"""Real curl -> public API response -> genuine bundle -> pinned gh crypto.

Only transport origins and the offline trust root are replaced. No account,
remote API, minted signature or external attestation service participates.
"""
from http.server import BaseHTTPRequestHandler
import importlib.util
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import urlsplit

import attestations
import contract
import gate
import test_hosting
import test_verify_cli

ALPHA, TRUST = test_verify_cli.ALPHA, test_verify_cli.TRUST

ROOT = Path(__file__).resolve().parents[2]


class PublicAPI(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        self.server.requests.append(self.path)
        self.server.case.assertIsNone(self.headers.get('Authorization'))
        if urlsplit(self.path).path.startswith('/repos/marcinwadon/hennery/attestations/sha256:'):
            bundle = {} if self.server.case.tamper else json.loads((ALPHA / contract.BUNDLE).read_text())
            value = {'attestations': [{'bundle': bundle}]}
            raw = json.dumps(value).encode()
        else:
            raw = (ALPHA / contract.BUNDLE).read_bytes()
            if self.server.case.tamper:
                raw = b'{}'
        self.send_response(200)
        self.send_header('Content-Length', str(len(raw)))
        self.end_headers(); self.wfile.write(raw)


class AnonymousConsumer(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        test_verify_cli.PinnedCLI.setUpClass.__func__(cls)
        test_hosting.Hosting.setUpClass.__func__(cls)
        cls.server.RequestHandlerClass = PublicAPI

    def setUp(self):
        self.server.case = self
        self.server.requests = []
        self.tamper = False
        self.metadata = gate.strict_json((ALPHA / contract.METADATA).read_bytes())

    def test_anonymous_python_fetch_verifies_real_bytes_and_refuses_changed_bundle(self):
        real_run = subprocess.run
        def transport(command, **kwargs):
            command = list(command)
            env = kwargs['env']
            self.assertFalse(any(k in env for k in ('GH_TOKEN', 'GITHUB_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_TOKEN')))
            env.update(HTTPS_PROXY='http://127.0.0.1:9', HTTP_PROXY='http://127.0.0.1:9',
                       ALL_PROXY='http://127.0.0.1:9', NO_PROXY='localhost,127.0.0.1')
            if command[0] == 'curl':
                command = [arg.replace('https://api.github.com', self.origin) for arg in command]
                command += ['--cacert', str(self.certificate)]
            return real_run(command, **kwargs)
        for tamper in (False, True):
            self.tamper = tamper
            with patch.object(subprocess, 'run', side_effect=transport):
                with attestations.bundle(gate.digest((ALPHA / contract.CHECKSUMS).read_bytes())) as bundle:
                    command = contract.attestation_command(ALPHA / contract.CHECKSUMS, self.metadata['tag'],
                                                          self.metadata['source_sha'], bundle, TRUST)
                    result = real_run(command, env=self.env, capture_output=True, text=True)
            self.assertEqual(result.returncode == 0, not tamper, result.stderr)
        self.assertTrue(any('/attestations/sha256:04472994' in path for path in self.server.requests))

    def test_shared_shell_anonymous_fetch_uses_real_curl_jq_and_crypto(self):
        spec = importlib.util.spec_from_file_location('hardener', ROOT / 'packaging/harden-installer.py')
        hardener = importlib.util.module_from_spec(spec); spec.loader.exec_module(hardener)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); tools = root / 'tools'; tools.mkdir()
            real_curl, real_gh = shutil.which('curl'), shutil.which('gh')
            # Wrappers change transport/trust only, preserving producer parsing,
            # SHA calculation, official crypto and every production constraint.
            (tools / 'curl').write_text('#!' + shutil.which('python3') + '\n' +
                f'import os,sys\na=[s.replace("https://api.github.com",{self.origin!r}) for s in sys.argv[1:]]\n'
                f'os.execv({real_curl!r},[{real_curl!r},*a,"--cacert",{str(self.certificate)!r}])\n')
            (tools / 'gh').write_text('#!' + shutil.which('python3') + '\n' +
                f'import os,sys\nos.execv({real_gh!r},[{real_gh!r},*sys.argv[1:],"--custom-trusted-root",{str(TRUST.resolve())!r}])\n')
            for tool in tools.iterdir(): tool.chmod(0o755)
            script = hardener.CONSUMER_FUNCTIONS + '\nrelease_attest ' + shlex.quote(str((ALPHA / contract.CHECKSUMS).resolve()))
            env = self.env | {'PATH': str(tools) + os.pathsep + self.env['PATH'], 'release_dir': str(root),
                              'release_tag': self.metadata['tag'], 'release_source': self.metadata['source_sha'],
                              'NO_PROXY': 'localhost,127.0.0.1'}
            for tamper in (False, True):
                self.tamper = tamper
                result = subprocess.run(['sh', '-eu', '-c', script], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode == 0, not tamper, result.stderr)
