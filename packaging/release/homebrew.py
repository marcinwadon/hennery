#!/usr/bin/env python3
"""Add documentation installs and tests to cargo-dist's formula before freezing."""
import argparse
from pathlib import Path
import re
import shutil

import gate

DOC_INSTALL_BLOCK = '''    doc.install "CHANGELOG.md"
    doc.install "LICENSE"
    doc.install "README.md"
'''

TEST_BLOCK = '''  test do
    assert_match version.to_s, shell_output("#{bin}/hennery --version")
    refute_match "hennery-web-ui-not-embedded", File.binread(bin/"hennery")
    assert_path_exists pkgshare/"THIRD-PARTY-NOTICES.txt"
    assert_path_exists doc/"CHANGELOG.md"
    assert_path_exists doc/"LICENSE"
    assert_path_exists doc/"README.md"
  end
'''


def check(root, tag, targets, require_test=True):
    root = Path(root)
    gate.require(targets and len(set(targets)) == len(targets) and set(targets) <= gate.TARGETS,
                 'unqualified Homebrew targets')
    gate.require(re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?', tag), 'invalid Homebrew tag')
    path = root / 'hennery.rb'
    gate.require(path.is_file() and not path.is_symlink(), 'nonregular Homebrew formula')
    raw = path.read_bytes()
    try:
        formula = raw.decode('utf-8')
    except UnicodeError as error:
        raise gate.Refused('formula must be UTF-8') from error
    gate.require(formula.startswith('class Hennery < Formula\n') and formula.endswith('\nend\n'), 'unexpected formula structure')
    gate.require(formula.count('  def install\n') == 1, 'formula must have one install method')
    versions = re.findall(r'^\s*version (.+)$', formula, re.M)
    gate.require(versions == ['"' + tag[1:] + '"'], 'formula version differs from candidate')
    gate.require(not re.search(r'\b(service|post_install)\b', formula), 'formula must not manage services or run post-install hooks')
    pairs = re.findall(r'^\s*url "([^"\n]+)"\n\s*sha256 "([0-9a-f]{64})"$', formula, re.M)
    gate.require(len(pairs) == len(targets) == len(re.findall(r'^\s*url\b', formula, re.M))
                 == len(re.findall(r'^\s*sha256\b', formula, re.M)), 'formula URL/checksum set incomplete or extra')
    expected = {}
    for target in targets:
        name = 'hennery-' + target + '.tar.xz'
        archive = root / name
        gate.require(archive.is_file() and not archive.is_symlink(), 'nonregular Homebrew archive')
        expected[f'https://github.com/{gate.REPOSITORY}/releases/download/{tag}/{name}'] = gate.digest(archive.read_bytes())
    gate.require(len(dict(pairs)) == len(pairs) and dict(pairs) == expected, 'formula URL or digest differs from candidate archives')
    if require_test:
        gate.require(formula.count('  def install\n' + DOC_INSTALL_BLOCK) == 1,
                     'formula must explicitly install the tested documentation')
        gate.require(formula.count(TEST_BLOCK) == 1, 'formula must carry the version, UI, notices and changelog test')
    return raw


def prepare(root, tag, targets):
    check(root, tag, targets, require_test=False)
    path = Path(root) / 'hennery.rb'
    formula = path.read_text()
    if '  def install\n' + DOC_INSTALL_BLOCK not in formula:
        formula = formula.replace('  def install\n', '  def install\n' + DOC_INSTALL_BLOCK, 1)
    if TEST_BLOCK not in formula:
        formula = formula[:-4] + TEST_BLOCK + 'end\n'
    path.write_text(formula)
    check(root, tag, targets)


def freeze_proof(source, destination, tag, targets, source_sha):
    """Unsigned native PR fixture; it cannot authorize any publication."""
    gate.require(re.fullmatch('[0-9a-f]{40}', source_sha), 'invalid proof source SHA')
    check(source, tag, targets)
    destination = Path(destination)
    destination.mkdir(parents=True, exist_ok=False)
    for name in ['hennery.rb', *['hennery-' + t + '.tar.xz' for t in targets]]:
        shutil.copyfile(Path(source) / name, destination / name)
    import contract
    # This unsigned PR proof has no authority. The placeholder tag object
    # cannot satisfy the authenticated publishing context.
    contract.freeze(destination, {'schema': 3, 'repository': gate.REPOSITORY, 'source_sha': source_sha,
                    'tag': tag, 'tag_object': '0' * 40, 'channels': ['github'], 'targets': sorted(targets)})


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['prepare', 'check', 'freeze-proof'])
    parser.add_argument('candidate', type=Path)
    parser.add_argument('destination', nargs='?', type=Path)
    parser.add_argument('--source-sha')
    parser.add_argument('--tag', required=True)
    parser.add_argument('--targets', required=True)
    args = parser.parse_args()
    try:
        if args.operation == 'freeze-proof':
            if args.destination is None or args.source_sha is None:
                parser.error('freeze-proof needs a fresh destination and --source-sha')
            freeze_proof(args.candidate, args.destination, args.tag, args.targets.split(','), args.source_sha)
        else:
            (prepare if args.operation == 'prepare' else check)(args.candidate, args.tag, args.targets.split(','))
    except (gate.Refused, OSError) as error:
        raise SystemExit(f'Homebrew refused: {error}') from error
