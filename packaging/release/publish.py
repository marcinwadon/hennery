#!/usr/bin/env python3
"""GitHub channel reconciliation/upload. Never overwrites or rebuilds payloads.

Inactive legacy transport, retained with publication.py until operator-authorized
hosted qualification. The active publisher uses hosting.py and standard gh release.
"""
import os
import urllib.error
import urllib.parse
import urllib.request

import gate


def require_enabled():
    gate.require(os.environ.get('RELEASE_PUBLICATION_ENABLED') == 'true', 'publication is disabled')


def api(path, method='GET', data=None, content_type='application/json', binary=False):
    gate.require(path.startswith('/repos/' + gate.REPOSITORY + '/') and '..' not in path, 'invalid publication API path')
    host = 'https://uploads.github.com' if method == 'POST' and '/assets?' in path else 'https://api.github.com'
    request = urllib.request.Request(host + path, method=method, data=data,
                                     headers={'Authorization': 'Bearer ' + os.environ['GH_TOKEN'], 'Content-Type': content_type,
                                              'Accept': 'application/octet-stream' if binary else 'application/vnd.github+json',
                                              'X-GitHub-Api-Version': '2022-11-28'})
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *args, **kwargs):
            return None
    try:
        with urllib.request.build_opener(NoRedirect).open(request, timeout=60) as response:
            body = response.read()
            return body if binary else gate.strict_json(body)
    except urllib.error.HTTPError as error:
        if error.code == 404 and method == 'GET' and not binary:
            return None
        if error.code in (301, 302, 303, 307, 308) and method == 'GET' and binary:
            location = error.headers.get('Location', '')
            url = urllib.parse.urlsplit(location)
            gate.require(url.scheme == 'https' and url.hostname in ('release-assets.githubusercontent.com', 'objects.githubusercontent.com')
                         and url.username is None and url.port in (None, 443), 'untrusted asset redirect')
            # No Authorization header goes to the signed object-store URL.
            with urllib.request.build_opener(NoRedirect).open(urllib.request.Request(location), timeout=60) as response:
                return response.read()
        raise gate.Refused(f'publication API failed with HTTP {error.code}; reconcile before retry') from error
    except urllib.error.URLError as error:
        raise gate.Refused('ambiguous publication response; reconcile before retry') from error


def asset_bytes(asset):
    url = urllib.parse.urlsplit(asset['url'])
    gate.require(url.scheme == 'https' and url.netloc == 'api.github.com', 'invalid asset API URL')
    return api(url.path, binary=True)


def plan(expected, assets):
    gate.require(len({a['name'] for a in assets}) == len(assets), 'duplicate remote assets')
    gate.require({a['name'] for a in assets} <= set(expected), 'unapproved remote release assets')
    complete = []
    for asset in assets:
        data = asset_bytes(asset)
        gate.require(data is not None and gate.digest(data) == expected[asset['name']], f'remote artifact mismatch/unverifiable: {asset["name"]}')
        complete.append(asset['name'])
    return {'complete': sorted(complete), 'missing': sorted(set(expected) - set(complete))}


def release_assets(release):
    if release is None:
        return []
    assets = []
    for page in range(1, 101):
        result = api(f'/repos/{gate.REPOSITORY}/releases/{release["id"]}/assets?per_page=100&page={page}')
        gate.require(isinstance(result, list), 'release asset listing unavailable')
        assets.extend(result)
        if len(result) < 100:
            return assets
    raise gate.Refused('incomplete release asset pagination')
