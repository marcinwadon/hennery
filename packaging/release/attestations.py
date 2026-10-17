"""Anonymous public bundle lookup; trust comes only from gh's crypto verification."""
from contextlib import contextmanager
from pathlib import Path
import re
import tempfile

import gate
import verifier_process


@contextmanager
def bundle(digest, supplied=None):
    if supplied is not None:
        yield supplied
        return
    gate.require(re.fullmatch(r'[0-9a-f]{64}', digest), 'invalid attestation subject digest')
    with tempfile.TemporaryDirectory(prefix='hennery-attestation-') as temporary:
        def fetch(url):
            return verifier_process.run(['curl', '-q', '--proto', '=https', '--proto-redir', '=https',
                                         '--tlsv1.2', '-fsSL', '--url', url], check=True, capture_output=True).stdout
        # Public user attestations can be read without a token; gh online
        # lookup requires login even for public repositories. No stored account.
        response = gate.strict_json(fetch('https://api.github.com/users/marcinwadon/attestations/sha256:'
                                         + digest + '?per_page=100'))
        entries = response['attestations']
        gate.require(isinstance(entries, list) and entries, 'no public attestations for digest')
        path = Path(temporary) / 'bundle.jsonl'
        with path.open('wb') as output:
            for entry in entries:
                output.write(gate.canonical(gate.strict_json(fetch(entry['bundle_url']))))
        yield path
