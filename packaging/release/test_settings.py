import copy
from datetime import datetime, timezone
import unittest

import gate
import settings_audit


def settings():
    return {
        'owner': {'login': 'marcinwadon', 'id': 123, 'type': 'User'},
        'environment': {'id': 7, 'name': 'release', 'can_admins_bypass': False,
                        'protection_rules': [{'type': 'required_reviewers', 'reviewers': [{'type': 'User', 'reviewer': {'id': 123}}]}],
                        'deployment_branch_policy': {'protected_branches': False, 'custom_branch_policies': True}},
        'deployment_policies': [{'name': 'v*', 'type': 'tag'}],
        'rulesets': settings_audit.tag_rulesets(123),
        'repository_secrets': [], 'release_secrets': ['GHCR_PUBLISH_TOKEN'],
        'immutable_releases': True,
        'release_vars': {'GHCR_PUBLISH_USER': 'marcinwadon'},
        'ghcr': {'repository': gate.REPOSITORY, 'actions_access': 'read', 'dedicated_credential': 'GHCR_PUBLISH_TOKEN',
                 'negative_push': {'read_status': 200, 'push_status': 403, 'credential': 'GITHUB_TOKEN'}},
        'crates': [{'crate': name, 'repository': gate.REPOSITORY, 'workflow': 'release.yml', 'environment': 'release'} for name in settings_audit.CRATES],
    }


class Settings(unittest.TestCase):
    def test_platform_settings_and_each_independent_refusal(self):
        valid = settings(); settings_audit.validate_settings(valid, ['crates', 'ghcr', 'github'])
        mutations = [
            lambda s: s['owner'].update(login='untrusted'),
            lambda s: s.update(deployment_policies=[{'name': 'main', 'type': 'branch'}]),
            lambda s: s['environment'].update(can_admins_bypass=True),
            lambda s: s['environment'].update(name='other'),
            lambda s: s['environment'].update(deployment_branch_policy=None),
            lambda s: s.update(release_secrets=[]),
            lambda s: s['release_vars'].update(GHCR_PUBLISH_USER=''),
            lambda s: s['ghcr'].update(dedicated_credential='GITHUB_TOKEN'),
            lambda s: s['crates'].pop(),
            lambda s: s['environment']['protection_rules'][0]['reviewers'][0]['reviewer'].update(id=999),
            lambda s: s['environment']['protection_rules'][0]['reviewers'][0].update(type='Team'),
            lambda s: s['rulesets'][0].update(bypass_actors=[{'actor_type': 'RepositoryRole', 'actor_id': 4, 'bypass_mode': 'always'}]),
            lambda s: s['rulesets'][1].update(bypass_actors=[{'actor_type': 'User', 'actor_id': 123, 'bypass_mode': 'always'}]),
            lambda s: s['rulesets'][1].update(rules=[{'type': 'update'}]),
            lambda s: s['rulesets'][0].update(enforcement='disabled'),
            lambda s: s['rulesets'][1].update(enforcement='disabled'),
            lambda s: s['rulesets'][1].update(target='branch'),
            lambda s: s['rulesets'][0]['conditions']['ref_name'].update(exclude=['refs/tags/v*']),
            lambda s: s.update(repository_secrets=['PUBLISH_TOKEN']),
            lambda s: s.update(immutable_releases=False),
            lambda s: s['ghcr'].update(actions_access='write'),
            lambda s: s['ghcr']['negative_push'].update(push_status=201),
            lambda s: s['ghcr']['negative_push'].update(read_status=401),
            lambda s: s['ghcr']['negative_push'].update(credential='dedicated-publisher'),
            lambda s: s['crates'][0].update(environment=None),
            lambda s: s['crates'][0].update(workflow='branch.yml'),
        ]
        for mutate in mutations:
            bad = copy.deepcopy(valid); mutate(bad)
            with self.subTest(settings=bad), self.assertRaises(gate.Refused):
                settings_audit.validate_settings(bad, ['crates', 'ghcr', 'github'])

    def test_local_acceptance_report_has_no_candidate_or_approval_authority(self):
        now = datetime(2026, 10, 16, tzinfo=timezone.utc)
        value = settings_audit.report(settings(), ['github'], now)
        self.assertEqual(value['schema'], 2)
        self.assertEqual(value['purpose'], 'maintainer-local-platform-acceptance')
        self.assertEqual(value['channels'], ['github'])
        self.assertEqual(value['checked_at'], now.isoformat())
        for obsolete in ('candidate', 'inventory_sha256', 'settings_audit_sha256'):
            self.assertNotIn(obsolete, value)


    def test_github_needs_no_uploaded_app_or_stored_write_token(self):
        valid = settings(); valid['release_secrets'] = []
        settings_audit.validate_settings(valid, ['github'])
        for obsolete in ('RELEASE_PUBLISH_APP_KEY', 'RELEASE_PUBLISH_APP_ID', 'GITHUB_TOKEN',
                         'AWS_ACCESS_KEY_ID', 'AWS_SECRET_ACCESS_KEY'):
            bad = copy.deepcopy(valid); bad['release_secrets'] = [obsolete]
            with self.subTest(secret=obsolete), self.assertRaises(gate.Refused):
                settings_audit.validate_settings(bad, ['github'])

    def test_crates_bootstrap_is_explicit_scoped_temporary_and_never_fallback(self):
        valid = settings(); valid['release_secrets'].append('RELEASE_CRATES_BOOTSTRAP_TOKEN')
        valid['release_vars']['RELEASE_CRATES_BOOTSTRAP_ENABLED'] = 'true'
        valid['crates'] = []
        valid['crates_bootstrap'] = {'publish_only': True, 'crate_names': sorted(settings_audit.CRATES),
                                    'scope_confirmed': True, 'revocation_required': True}
        settings_audit.validate_settings(valid, ['crates', 'github'])
        mutations = [lambda s: s['release_secrets'].remove('RELEASE_CRATES_BOOTSTRAP_TOKEN'),
                     lambda s: s.pop('crates_bootstrap'),
                     lambda s: s['crates_bootstrap'].update(publish_only=False),
                     lambda s: s['crates_bootstrap']['crate_names'].pop(),
                     lambda s: s['crates_bootstrap'].update(revocation_required=False),
                     lambda s: s['crates_bootstrap'].update(scope_confirmed=False)]
        for mutate in mutations:
            bad = copy.deepcopy(valid); mutate(bad)
            with self.subTest(settings=bad), self.assertRaises(gate.Refused):
                settings_audit.validate_settings(bad, ['crates', 'github'])
        valid['release_vars']['RELEASE_CRATES_BOOTSTRAP_ENABLED'] = 'false'
        with self.assertRaises(gate.Refused): settings_audit.validate_settings(valid, ['crates', 'github'])
        trusted = settings(); trusted['release_secrets'].append('RELEASE_CRATES_BOOTSTRAP_TOKEN')
        with self.assertRaises(gate.Refused): settings_audit.validate_settings(trusted, ['crates', 'github'])

    def test_local_acceptance_checks_signed_tag_before_owner_settings_auth(self):
        import contextlib, io, sys, tempfile
        from pathlib import Path
        from unittest.mock import patch
        import contract
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)/'payload'; root.mkdir(); (root/'example').write_bytes(b'bytes')
            metadata={'schema':3,'repository':gate.REPOSITORY,'source_sha':'a'*40,'tag':'v0.1.0',
                      'tag_object':'b'*40,'channels':['github'],'targets':sorted(gate.TARGETS)}
            contract.freeze(root, metadata)
            observations=Path(tmp)/'observations.json';observations.write_text('{}')
            argv=['settings_audit.py','--payload',str(root),'--observations',str(observations),'--output',str(Path(tmp)/'acceptance.json')]
            with patch.object(sys,'argv',argv),patch('tag_signature.verify',side_effect=gate.Refused('local signed tag refused')),patch.object(settings_audit,'collect',return_value=settings()) as collect,patch('builtins.input',return_value=gate.REPOSITORY),contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaisesRegex(gate.Refused,'local signed tag refused'): settings_audit.main()
                collect.assert_not_called()

    def test_switch_types_and_removed_storage_or_app_vars_refuse(self):
        for change in ({'RELEASE_GITHUB_ENABLED':True}, {'RELEASE_CRATES_ENABLED':'TRUE'},
                       {'RELEASE_PUBLISH_APP_ID':'old'}, {'RELEASE_CANDIDATE_BUCKET':'old'},
                       {'RELEASE_AUDIT_BUCKET':'old'}, {'AWS_REGION':'old'}):
            data=settings();data['release_vars'].update(change)
            with self.subTest(change=change),self.assertRaises(gate.Refused):
                settings_audit.validate_settings(data,['github'])


    def test_settings_access_is_local_only_and_wrong_account_refuses(self):
        from unittest.mock import patch
        with patch.dict('os.environ',{'GITHUB_ACTIONS':'true'}),patch.object(settings_audit.subprocess,'run',return_value=__import__('types').SimpleNamespace(stdout=b'{}')) as run:
            with self.assertRaises(gate.Refused):settings_audit.gh('/user')
            run.assert_not_called()
        owner=settings()['owner']
        calls=[]
        def fake(path):
            calls.append(path)
            return {'login':'other','id':999} if path=='/user' else {'owner':owner}
        with patch.object(settings_audit,'gh',side_effect=fake):
            with self.assertRaises(gate.Refused):settings_audit.collect({})
        self.assertEqual(calls,['/user','/repos/'+gate.REPOSITORY])

    def test_pages_refuse_malformed_or_unbounded_results(self):
        from unittest.mock import patch
        with patch.object(settings_audit,'gh',return_value={'variables':{}}):
            with self.assertRaises(gate.Refused):settings_audit.pages('/example','variables')
        with patch.object(settings_audit,'gh',return_value=[{}]*100) as gh:
            with self.assertRaises(gate.Refused):settings_audit.pages('/example')
            self.assertEqual(gh.call_count,100)
        with patch.object(settings_audit,'gh',side_effect=[[{}]*100,[{'name':'last'}]]):
            self.assertEqual(len(settings_audit.pages('/example')),101)

    def test_invalid_selection_refuses(self):
        for channels in ([],['homebrew'],['unknown','github'],['github','github'],['homebrew','github']):
            with self.subTest(channels=channels),self.assertRaises(gate.Refused):
                settings_audit.validate_settings(settings(),channels)

    def test_main_confirmation_and_create_only_report(self):
        import contextlib, io, sys, tempfile
        from pathlib import Path
        from unittest.mock import patch
        import contract
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)/'payload';root.mkdir();(root/'example').write_bytes(b'bytes')
            metadata={'schema':3,'repository':gate.REPOSITORY,'source_sha':'a'*40,'tag':'v0.1.0',
                      'tag_object':'b'*40,'channels':['github'],'targets':sorted(gate.TARGETS)}
            contract.freeze(root,metadata)
            observations=Path(tmp)/'observations.json';observations.write_text('{}')
            output=Path(tmp)/'acceptance.json'
            argv=['settings_audit.py','--payload',str(root),'--observations',str(observations),'--output',str(output)]
            with patch.object(sys,'argv',argv),patch('tag_signature.verify',autospec=True,return_value={'verified':True}),patch.object(settings_audit,'collect',return_value=settings()),contextlib.redirect_stdout(io.StringIO()):
                with patch('builtins.input',return_value='no'),self.assertRaises(gate.Refused):settings_audit.main()
                self.assertFalse(output.exists())
                with patch('builtins.input',return_value=gate.REPOSITORY):settings_audit.main()
                raw=output.read_bytes();self.assertEqual(gate.strict_json(raw)['purpose'],'maintainer-local-platform-acceptance')
                with patch('builtins.input',return_value=gate.REPOSITORY),self.assertRaises(FileExistsError):settings_audit.main()
                self.assertEqual(output.read_bytes(),raw)

    def test_observation_template_does_not_claim_unperformed_acceptance(self):
        from pathlib import Path
        data=gate.strict_json((Path(__file__).with_name('example-observations.json')).read_bytes())
        self.assertIs(data['homebrew']['scope_confirmed'],False)
        self.assertIsNone(data['ghcr']['negative_push']['read_status'])
        self.assertIsNone(data['ghcr']['negative_push']['push_status'])
        self.assertIs(data['crates_bootstrap']['scope_confirmed'],False)
        self.assertEqual({c['crate'] for c in data['crates']},set(settings_audit.CRATES))
        self.assertTrue(all(c['workflow']=='release.yml' and c['environment']=='release' for c in data['crates']))
