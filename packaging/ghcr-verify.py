#!/usr/bin/env python3
"""Read-only verification of release and OCI provenance/SPDX subjects."""
import argparse
from pathlib import Path
import sys

sys.path.insert(0,str(Path(__file__).parent/'release'))
import contract
import gate
import verify

PROVENANCE=verify.PROVENANCE
SPDX=verify.SPDX


def subjects(work,bundles):
    index=gate.strict_json(contract.regular(work/'verified.json'))
    for subject,digest in [('index',index['digest']),*index['platforms'].items()]:
        for kind in (['provenance'] if subject=='index' else ['provenance','image-sbom','binary-sbom']):
            yield subject,kind,digest,PROVENANCE if kind=='provenance' else SPDX,bundles/f'ghcr-{subject}-{kind}.json'


def check(root,work,signatures,source_sha,tag,remote=False):
    value=verify.verify(root,signatures/contract.BUNDLE,source_sha,tag)
    gate.require(gate.strict_json(contract.regular(work/'verified.json'))==verify.image_descriptor(root,value),
                 'working image must match the release descriptor')
    for _,_,digest,predicate,bundle in subjects(work,signatures):
        gate.require(bundle.is_file() and not bundle.is_symlink(),'missing/nonregular OCI verification bundle')
        manifest=None if remote else work/'layout/blobs/sha256'/digest.removeprefix('sha256:')
        verify.verify_image_subject('ghcr.io/marcinwadon/hennery@'+digest,value,bundle,predicate,manifest)
    gate.require(contract.load(root)==value,'release bytes changed during OCI verification')
    return value


def main():
    parser=argparse.ArgumentParser(description=__doc__,allow_abbrev=False)
    parser.add_argument('--release-directory',required=True,type=Path)
    parser.add_argument('--work',required=True,type=Path,help='materialized layout already checked by ghcr-candidate accept')
    parser.add_argument('--signatures',required=True,type=Path)
    parser.add_argument('--source-sha',required=True)
    parser.add_argument('--tag',required=True)
    parser.add_argument('--remote',action='store_true')
    args=parser.parse_args()
    check(args.release_directory,args.work,args.signatures,args.source_sha,args.tag,args.remote)


if __name__=='__main__':main()
