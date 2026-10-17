#!/usr/bin/env python3
"""Release primitives and read-only fixed-origin platform transport."""
import hashlib
import json
import urllib.error
import urllib.request


REPOSITORY = 'marcinwadon/hennery'
CHANNELS = {'github', 'crates', 'ghcr', 'homebrew'}
TARGETS = {'aarch64-apple-darwin', 'aarch64-unknown-linux-musl', 'x86_64-unknown-linux-musl'}


class Refused(ValueError):
    pass


def require(condition, reason):
    if not condition:
        raise Refused(reason)


def canonical(value):
    return (json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True) + '\n').encode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def strict_json(raw):
    def pairs(items):
        result = {}
        seen = set()
        for key, value in items:
            require(key.casefold() not in seen, 'duplicate or case-ambiguous JSON key')
            seen.add(key.casefold())
            result[key] = value
        return result
    try:
        return json.loads(raw, object_pairs_hook=pairs)
    except (ValueError, UnicodeError) as error:
        raise Refused(f'invalid JSON: {error}') from error


class GitHub:
    """Only GET; fixed origin, bounded pagination, no credential-bearing redirects."""
    def __init__(self, token):
        self.token = token

    def get(self, suffix):
        require(suffix.startswith('/') and '..' not in suffix, 'invalid API path')
        request = urllib.request.Request('https://api.github.com/repos/' + REPOSITORY + ('' if suffix == '/' else suffix),
                                         headers={'Accept': 'application/vnd.github+json', 'Authorization': 'Bearer ' + self.token,
                                                  'X-GitHub-Api-Version': '2022-11-28'})
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *args, **kwargs):
                raise Refused('platform API redirected')
        try:
            with urllib.request.build_opener(NoRedirect).open(request, timeout=30) as response:
                return strict_json(response.read())
        except urllib.error.URLError as error:
            raise Refused(f'platform evidence unavailable: {error}') from error

    def pages(self, suffix, key=None):
        out = []
        for page in range(1, 101):
            data = self.get(suffix + ('&' if '?' in suffix else '?') + f'per_page=100&page={page}')
            batch = data[key] if key else data
            require(isinstance(batch, list), 'malformed platform page')
            out.extend(batch)
            if len(batch) < 100:
                return out
        raise Refused('platform pagination limit: incomplete evidence')
