"""Folded channel delivery; credentials are read only after the shared verifier."""
import base64
import importlib.util
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

import contract
import crates_delivery
import gate
import homebrew
import homebrew_publish as tap
import verifier_process
import verify

ROOT=Path(__file__).resolve().parents[2]
SECRETS=('HOMEBREW_TAP_TOKEN','GHCR_PUBLISH_TOKEN','CARGO_REGISTRY_TOKEN','RELEASE_CRATES_BOOTSTRAP_TOKEN')


def shared_verify(root,signatures,channel):
    import release_run
    # Authenticated API transport stays in this parent; verifier children use
    # verifier_process's credential-free allow-list without mutating os.environ.
    return release_run.verify(root,signatures,channel)

def crates_token(environment):
    bootstrap=environment.get('RELEASE_CRATES_BOOTSTRAP_TOKEN')
    oidc=environment.get('CARGO_REGISTRY_TOKEN')
    if environment.get('RELEASE_CRATES_BOOTSTRAP_ENABLED')=='true':
        gate.require(bootstrap and not oidc,'bootstrap needs only its explicit temporary token; OIDC cannot fall back')
        return bootstrap
    gate.require(oidc and not bootstrap,'trusted publishing token absent or stale bootstrap secret still present')
    return oidc


def homebrew_upload(root,signatures,value):
    gate.require(re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+',value['tag']),'Homebrew publication requires stable version')
    frozen=homebrew.check(root,value['tag'],value['targets'])
    previous=tap.tap_api('GET')
    payload={'message':'chore(formula): release hennery '+value['tag'],'branch':'main',
             'content':base64.b64encode(frozen).decode(),
             'committer':{'name':'Marcin Wadoń','email':'marcin.wadon@gmail.com'}}
    if previous is not None:
        raw=tap.remote_bytes(previous)
        if raw==frozen:return {'state':'already-present','sha':previous['sha']}
        gate.require(tap.version(raw)<tap.version(frozen),'tap same-version conflict or downgrade refused')
        payload['sha']=previous['sha']
    current=shared_verify(root,signatures,'homebrew')
    gate.require(current==value and homebrew.check(root,current['tag'],current['targets'])==frozen,'formula changed at write boundary')
    result=tap.tap_api('PUT',payload)
    gate.require(isinstance(result,dict) and result.get('content',{}).get('sha')==tap.blob_sha(frozen)
                 and re.fullmatch('[0-9a-f]{40}',result.get('commit',{}).get('sha','')),'tap write unconfirmed; reconcile on retry')
    gate.require(tap.remote_bytes(tap.tap_api('GET'))==frozen,'tap read-back differs; reconcile on retry')
    return {'state':'published','commit':result['commit']['sha']}


def image_helper():
    spec=importlib.util.spec_from_file_location('release_oci',ROOT/'packaging/ghcr-candidate.py')
    helper=importlib.util.module_from_spec(spec);spec.loader.exec_module(helper)
    return helper


def image_subjects(work,signatures):
    index=gate.strict_json(contract.regular(work/'verified.json'))
    for subject,digest in [('index',index['digest']),*index['platforms'].items()]:
        for kind in (['provenance'] if subject=='index' else ['provenance','image-sbom','binary-sbom']):
            yield subject,kind,digest,('https://slsa.dev/provenance/v1' if kind=='provenance' else 'https://spdx.dev/Document'),signatures/f'ghcr-{subject}-{kind}.json'


def check_images(root,signatures,value,work):
    image_helper().inspect_release(root,work,value)
    expected=gate.strict_json(contract.regular(root/'ghcr-index.json'))
    gate.require(gate.strict_json(contract.regular(work/'verified.json'))==expected,'working OCI descriptor differs')
    for _,_,digest,predicate,bundle in image_subjects(work,signatures):
        contract.regular(bundle)
        manifest=work/'layout/blobs/sha256'/digest.removeprefix('sha256:')
        gate.require('sha256:'+gate.digest(contract.regular(manifest))==digest,'local OCI manifest differs')
        command=contract.attestation_command(manifest,value['tag'],value['source_sha'],bundle)
        command[command.index('--predicate-type')+1]=predicate
        verifier_process.run(command,check=True,capture_output=True,text=True)
    return expected


def verify_images(root,signatures,value):
    with tempfile.TemporaryDirectory(prefix='release-oci-check-') as tmp:
        work=Path(tmp)/'image'
        return check_images(root,signatures,value,work)


def ghcr_upload(root,signatures,value):
    with tempfile.TemporaryDirectory(prefix='release-ghcr-') as tmp,verifier_process.environment() as transport:
        work=Path(tmp)/'image';index=image_helper().inspect_release(root,work,value)
        check_images(root,signatures,value,work)
        # No credential is read or registry contacted before all local subjects
        # and their exact-tag/source provenance have verified.
        token=os.environ.get('GHCR_PUBLISH_TOKEN');user=os.environ.get('GHCR_PUBLISH_USER','')
        gate.require(token and re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9-]*',user),'dedicated GHCR credential/user missing')
        auth=Path(tmp)/'config.json'
        subprocess.run(['skopeo','login','--authfile',str(auth),'--username',user,'--password-stdin','ghcr.io'],
                       input=token,text=True,check=True,capture_output=True,env=transport)
        auth.chmod(0o600)
        # Registry transport gets only its dedicated auth, never GitHub/OIDC
        # authority or operator configuration. Crypto uses a separate fresh env.
        registry_env=dict(transport,REGISTRY_AUTH_FILE=str(auth),DOCKER_CONFIG=tmp)
        remote=registry_helper()
        version=value['tag'].removeprefix('v');digest=index['digest']
        exists=remote.reconcile(version,digest,auth)==0
        if not exists:exists=remote.reconcile(digest,digest,auth)==0
        if not exists:
            shared_verify(root,signatures,'ghcr');check_images(root,signatures,value,work)
            # Reconcile again after a potentially slow cryptographic boundary.
            if remote.reconcile(version,digest,auth)!=0 and remote.reconcile(digest,digest,auth)!=0:
                subprocess.run(['skopeo','copy','--all','--preserve-digests','--authfile',str(auth),
                                'oci:'+str(work/'layout'),'docker://ghcr.io/marcinwadon/hennery@'+digest],check=True,env=registry_env)
            gate.require(remote.reconcile(digest,digest,auth)==0,'GHCR upload ambiguous; stop and reconcile')
        # Standard OCI attachments may duplicate valid bundles on retry. Their
        # content-addressed image subjects must still match the accepted digest.
        for _, _, subject, predicate, bundle in image_subjects(work, signatures):
            shared_verify(root, signatures, 'ghcr')
            attach_bundle(subject, predicate, bundle, auth, registry_env)
        # Fetch through the supported verifier, rather than matching a canonical
        # seven-referrer layout. Crypto children never inherit registry authority.
        for subject, predicate in {(s[2], s[3]) for s in image_subjects(work, signatures)}:
            verify.verify_image_subject('ghcr.io/marcinwadon/hennery@' + subject, value,
                                        predicate_type=predicate)
        return {'state':'published','digest':digest}


def attach_bundle(digest, predicate, bundle, auth, environment):
    media = 'application/vnd.dev.sigstore.bundle.v0.3+json'
    bundle = Path(bundle).resolve()
    contract.regular(bundle)
    subprocess.run(['oras', 'attach', '--registry-config', str(auth),
                    '--artifact-type', media,
                    '--annotation', 'dev.sigstore.bundle.predicateType=' + predicate,
                    '--annotation', 'dev.sigstore.bundle.content=dsse-envelope',
                    'ghcr.io/marcinwadon/hennery@' + digest, bundle.name + ':' + media],
                   cwd=bundle.parent, env=environment, check=True, capture_output=True)


def registry_helper():
    spec=importlib.util.spec_from_file_location('ghcr_remote',ROOT/'packaging/ghcr-remote.py')
    remote=importlib.util.module_from_spec(spec);spec.loader.exec_module(remote)
    return remote


def publish(root, signatures, channel):
    bootstrap=channel=='crates' and os.environ.get('RELEASE_CRATES_BOOTSTRAP_ENABLED')=='true'
    try:
        value=shared_verify(root,signatures,channel)
        if channel=='homebrew':return homebrew_upload(root,signatures,value)
        if channel=='ghcr':return ghcr_upload(root,signatures,value)
        gate.require(channel=='crates','unknown folded publisher')
        token=crates_token(os.environ)
        def authorize_crates():
            shared_verify(root,signatures,'crates')
            return gate.digest(contract.regular(Path(root)/'crates-inventory.json'))
        with verifier_process.environment() as env:
            return crates_delivery.run(root,signatures,token,authorize_crates,environment=env)
    finally:
        if bootstrap:
            print('BOOTSTRAP: revoke the bootstrap token and confirm revocation on success or abandonment; remove its release secret and disable bootstrap before any later run.')
