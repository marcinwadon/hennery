#!/usr/bin/env python3
"""Maintainer-local platform acceptance; never a publisher authorization token.

Inspect platform settings and explicitly confirmed registry observations. The
single tagged workflow uses its environment approval,
not an inventory signature, retained audit or digest-bearing review comment.
"""
import argparse
from datetime import datetime, timezone
import os
from pathlib import Path
import subprocess

import gate

CRATES = ('hennery-proto', 'hennery-view', 'hennery-gateway', 'hennery-sessions', 'hennery-host', 'hennery-kernel', 'hennery')


def tag_rulesets(owner_id):
    common = {'target': 'tag', 'enforcement': 'active', 'conditions': {'ref_name': {'include': ['refs/tags/v*'], 'exclude': []}}}
    return [common | {'name': 'release-tag-creation', 'bypass_actors': [{'actor_type': 'User', 'actor_id': owner_id, 'bypass_mode': 'always'}], 'rules': [{'type': 'creation'}]},
            common | {'name': 'release-tags-immutable', 'bypass_actors': [], 'rules': [{'type': 'update'}, {'type': 'deletion'}]}]


def check_tag_rules(rulesets, owner_id):
    creation, immutable = False, False
    for rule in rulesets:
        if rule.get('target') != 'tag' or rule.get('enforcement') != 'active':
            continue
        refs = rule.get('conditions', {}).get('ref_name', {})
        if refs != {'include': ['refs/tags/v*'], 'exclude': []}:
            continue
        operations = {r['type'] for r in rule.get('rules', [])}
        creation |= operations == {'creation'} and rule.get('bypass_actors') == [{'actor_type': 'User', 'actor_id': owner_id, 'bypass_mode': 'always'}]
        immutable |= {'update', 'deletion'} <= operations and rule.get('bypass_actors') == []
    gate.require(creation and immutable, 'release tags require owner-only creation and immutable update/deletion with no bypass')


def validate_settings(settings, channels):
    gate.require(channels == sorted(set(channels)) and 'github' in channels and set(channels) <= set(gate.CHANNELS), 'invalid selected channel set')
    owner = settings['owner']
    gate.require(owner.get('login') == gate.REPOSITORY.split('/')[0] and owner.get('type') == 'User' and type(owner.get('id')) is int and owner['id'] > 0, 'settings audit requires the personal repository owner')
    env = settings['environment']
    gate.require(env.get('name') == 'release' and env.get('can_admins_bypass') is False, 'release environment missing or bypass not explicitly disabled')
    reviewers = [r for rule in env.get('protection_rules', []) if rule.get('type') == 'required_reviewers' for r in rule.get('reviewers', [])]
    gate.require(len(reviewers) == 1 and reviewers[0].get('type') == 'User' and reviewers[0].get('reviewer', {}).get('id') == owner['id'], 'release reviewer must be exactly the maintainer')
    gate.require(env.get('deployment_branch_policy') == {'protected_branches': False, 'custom_branch_policies': True}, 'release environment deployment restrictions missing')
    gate.require(settings.get('deployment_policies') == [{'name': 'v*', 'type': 'tag'}], 'release deployment must permit protected v* tags only')
    check_tag_rules(settings.get('rulesets', []), owner['id'])
    gate.require(settings.get('repository_secrets') == [], 'repository secrets must be empty; publishing credentials belong only in release')
    gate.require(settings.get('immutable_releases') is True, 'immutable GitHub releases must be enabled')
    secrets = settings.get('release_secrets', [])
    allowed = {'RELEASE_HOMEBREW_TOKEN', 'GHCR_PUBLISH_TOKEN', 'RELEASE_CRATES_BOOTSTRAP_TOKEN'}
    gate.require(isinstance(secrets, list) and len(secrets) == len(set(secrets)) and set(secrets) <= allowed,
                 'only channel credentials belong in release; no stored GitHub write token, upload App or storage credentials')
    variables = settings.get('release_vars', {})
    switches = ('RELEASE_PUBLICATION_ENABLED', 'RELEASE_GITHUB_ENABLED', 'RELEASE_HOMEBREW_ENABLED',
                'RELEASE_CRATES_ENABLED', 'RELEASE_GHCR_ENABLED', 'RELEASE_CRATES_BOOTSTRAP_ENABLED')
    gate.require(set(variables) <= set(switches) | {'GHCR_PUBLISH_USER'}, 'obsolete or unknown release environment variable')
    gate.require(all(variables.get(name, 'false') in ('true', 'false') for name in switches), 'release switches must be explicit true/false strings')
    if 'homebrew' in channels:
        tap = settings.get('homebrew', {})
        gate.require('RELEASE_HOMEBREW_TOKEN' in secrets, 'Homebrew credential must exist only in release')
        gate.require(tap.get('repository') == 'marcinwadon/homebrew-tap' and tap.get('branch') == 'main'
                     and tap.get('environment') == 'release' and tap.get('credential') == 'RELEASE_HOMEBREW_TOKEN',
                     'Homebrew audit must name the protected tap credential and destination')
        gate.require(tap.get('kind') in ('fine-grained-pat', 'app-installation')
                     and tap.get('repositories') == ['marcinwadon/homebrew-tap']
                     and tap.get('permissions') == {'contents': 'write'} and tap.get('scope_confirmed') is True,
                     'maintainer must confirm tap-only contents scope; no broad token')
    if 'ghcr' in channels:
        ghcr = settings.get('ghcr', {})
        gate.require(ghcr.get('repository') == gate.REPOSITORY and ghcr.get('actions_access') == 'read', 'GHCR Actions access must be READ')
        gate.require(ghcr.get('dedicated_credential') == 'GHCR_PUBLISH_TOKEN' and 'GHCR_PUBLISH_TOKEN' in secrets, 'GHCR requires its dedicated release environment credential')
        gate.require(isinstance(variables.get('GHCR_PUBLISH_USER'), str) and bool(variables['GHCR_PUBLISH_USER'].strip()), 'GHCR publishing user must be configured')
        denial = ghcr.get('negative_push', {})
        gate.require(denial.get('credential') == 'GITHUB_TOKEN' and denial.get('read_status') == 200 and denial.get('push_status') == 403, 'post-setup GITHUB_TOKEN push denial must be observed with working read access')
    if 'crates' in channels:
        if variables.get('RELEASE_CRATES_BOOTSTRAP_ENABLED') == 'true':
            bootstrap = settings.get('crates_bootstrap', {})
            gate.require('RELEASE_CRATES_BOOTSTRAP_TOKEN' in secrets and bootstrap.get('publish_only') is True
                         and bootstrap.get('crate_names') == sorted(CRATES) and bootstrap.get('scope_confirmed') is True
                         and bootstrap.get('revocation_required') is True,
                         'explicit bootstrap requires temporary seven-crate publish-only scope and revocation on success or abandonment')
        else:
            gate.require('RELEASE_CRATES_BOOTSTRAP_TOKEN' not in secrets, 'remove/revoke abandoned or completed bootstrap token before trusted publishing')
            configs = settings.get('crates', [])
            gate.require(len(configs) == len(CRATES) and {c.get('crate') for c in configs} == set(CRATES), 'trusted publisher configuration missing for production crate closure')
            gate.require(all(c.get('repository') == gate.REPOSITORY and c.get('workflow') == 'release.yml' and c.get('environment') == 'release' for c in configs), 'crates.io trusted publishing must require exact workflow and release environment')


def report(settings, channels, now=None):
    """Local checklist evidence only: neither consumed nor ratified by publishing."""
    validate_settings(settings, channels)
    return {'schema': 2, 'purpose': 'maintainer-local-platform-acceptance',
            'repository': gate.REPOSITORY, 'channels': channels,
            'checked_at': (now or datetime.now(timezone.utc)).isoformat(), 'settings': settings}


def gh(path):
    # Deliberately local-only. Use the maintainer's own gh login, never a CI
    # administration token or a branch-supplied evidence collector.
    gate.require(os.environ.get('GITHUB_ACTIONS') != 'true', 'settings audit must run locally by the maintainer')
    return gate.strict_json(subprocess.run(['gh', 'api', '--hostname', 'github.com', path], check=True, capture_output=True).stdout)


def pages(path, key=None):
    values = []
    for page in range(1, 101):
        result = gh(path + ('&' if '?' in path else '?') + f'per_page=100&page={page}')
        batch = result[key] if key else result
        gate.require(isinstance(batch, list), 'incomplete settings audit page')
        values.extend(batch)
        if len(batch) < 100:
            return values
    raise gate.Refused('incomplete settings audit pagination')


def collect(observations):
    user = gh('/user')
    repo = gh('/repos/' + gate.REPOSITORY)
    owner = {k: repo['owner'][k] for k in ('login', 'id', 'type')}
    gate.require(user['id'] == owner['id'] and user['login'] == owner['login'], 'local gh account must be the personal repository owner')
    prefix = '/repos/' + gate.REPOSITORY
    environment = gh(prefix + '/environments/release')
    rulesets = [gh(prefix + '/rulesets/' + str(r['id'])) for r in pages(prefix + '/rulesets')]
    return {'owner': owner, 'environment': environment,
            'deployment_policies': [{k: p[k] for k in ('name', 'type')} for p in pages(prefix + '/environments/release/deployment-branch-policies', 'branch_policies')],
            'rulesets': rulesets, 'repository_secrets': [s['name'] for s in pages(prefix + '/actions/secrets', 'secrets')],
            'release_secrets': [s['name'] for s in pages(prefix + '/environments/release/secrets', 'secrets')],
            'immutable_releases': gh(prefix + '/immutable-releases')['enabled'],
            'release_vars': {v['name']: v['value'] for v in pages(prefix + '/environments/release/variables', 'variables')},
            'ghcr': observations.get('ghcr'), 'crates': observations.get('crates'),
            'crates_bootstrap': observations.get('crates_bootstrap'), 'homebrew': observations.get('homebrew')}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--payload', required=True, type=Path)
    parser.add_argument('--observations', required=True, type=Path, help='maintainer inspection: GHCR access, crates.io trusted publishers, Homebrew tap-only credential scope')
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    import contract
    import tag_signature
    metadata = contract.load(args.payload)
    signed_tag = tag_signature.verify(metadata['tag'], metadata['source_sha'], object_sha=metadata['tag_object'])
    value = report(collect(gate.strict_json(args.observations.read_bytes())), metadata['channels']) | {'signed_tag': signed_tag}
    print(gate.canonical(value).decode())
    gate.require(input('Confirm the displayed API settings AND personally checked registry observations by typing ' + gate.REPOSITORY + ': ') == gate.REPOSITORY, 'maintainer settings confirmation absent')
    with args.output.open('xb') as output:
        output.write(gate.canonical(value))
    print('Local acceptance recorded; this file is not publication authority.')


if __name__ == '__main__':
    main()
