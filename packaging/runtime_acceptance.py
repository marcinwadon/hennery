#!/usr/bin/env python3
"""Run the fixed exact-pin acceptance suite on one prepared host, without emulation."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
INPUTS = ('crates/hennery-host/manifest.json', 'adapters/claude/package-lock.json',
          'adapters/codex/package-lock.json', 'Cargo.lock', 'flake.lock')


def host_platform():
    return {('Darwin', 'arm64'): 'darwin-arm64', ('Linux', 'aarch64'): 'linux-arm64',
            ('Linux', 'x86_64'): 'linux-x64'}.get((platform.system(), platform.machine()))


def cases():
    rows = []
    for test in ('pinned_runtime_process_token_exposure_without_inference',
                 'pinned_runtime_provider_authentication',
                 'pinned_runtime_shell_executes_after_operator_approval',
                 'pinned_runtime_usage_observations'):
        rows.extend((test, parent, None) for parent in ('claude', 'codex'))
    rows.extend((test, parent, None) for test, parent in (
        ('pinned_codex_busy_tools_without_inference', 'codex'),
        ('pinned_claude_child_configuration_without_inference', 'claude'),
        ('live_claude_haiku_plan_configuration_survives_late_updates', 'claude'),
        ('pinned_codex_delete_removes_rollouts_and_private_database_rows', 'codex')))
    rows.extend(('pinned_parent_child_provider_matrix', parent, child)
                for parent in ('claude', 'codex') for child in ('claude', 'codex'))
    return rows


def secret_strings(value):
    if isinstance(value, dict):
        return [secret for child in value.values() for secret in secret_strings(child)]
    if isinstance(value, list):
        return [secret for child in value for secret in secret_strings(child)]
    return [value] if isinstance(value, str) and len(value) >= 8 else []


def private_write(path, text):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, 'w') as stream:
        stream.write(text)


def terminate_owned_tree(process):
    # Adapters can create separate process groups. Freeze owned processes before
    # discovering descendants so they cannot keep forking during timeout cleanup.
    owned = {process.pid}
    try:
        while True:
            for pid in owned:
                try:
                    os.kill(pid, signal.SIGSTOP)
                except ProcessLookupError:
                    pass
            listing = subprocess.check_output(['ps', '-axo', 'pid=,ppid='], text=True, timeout=10)
            pairs = [tuple(map(int, line.split())) for line in listing.splitlines()]
            descendants = {pid for pid, parent in pairs if parent in owned}
            if descendants <= owned:
                break
            owned |= descendants
    finally:
        for pid in owned:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        process.wait()


def execute(argv, env, log, timeout, secrets=()):
    started = time.monotonic()
    with tempfile.TemporaryFile(mode='w+b') as raw:
        process = subprocess.Popen(argv, cwd=ROOT, env=env, stdout=raw,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            terminate_owned_tree(process)
            code = 124
        raw.seek(0)
        text = raw.read().decode(errors='replace')
    for secret in sorted(secrets, key=len, reverse=True):
        text = text.replace(secret, '[REDACTED]')
    private_write(log, text)
    return {'exit': code, 'seconds': round(time.monotonic() - started, 2)}, text


def artifacts(text):
    result = {}
    for line in text.splitlines():
        try:
            item = json.loads(line)
        except ValueError:
            continue
        if item.get('reason') == 'compiler-artifact' and item.get('executable'):
            result[item['target']['name']] = item['executable']
    if not all(name in result for name in ('delegation_live', 'forget_codex')):
        raise ValueError('Cargo did not produce both required test executables')
    return result


def qualify_case(result, text):
    if result['exit'] == 0 and 'test result: ok. 1 passed;' not in text:
        return result | {'exit': 1}
    return result


def verify_reports(paths, source, inputs):
    expected = {'build', 'install', 'mcp-source-policy'} | {
        '-'.join(part for part in row if part) for row in cases()}
    platforms = set()
    for path in paths:
        report = json.loads(path.read_text())
        if (report.get('source') != source or report.get('inputs') != inputs
                or report.get('outcome') != 'success'):
            raise ValueError('Report does not accept the requested candidate')
        rows = report.get('results', [])
        if (len(rows) != len(expected) or {row.get('case') for row in rows} != expected
                or any(row.get('exit') != 0 for row in rows)):
            raise ValueError('Report is missing required passing cases')
        if report.get('platform') in platforms:
            raise ValueError('Duplicate platform report')
        platforms.add(report.get('platform'))
    if platforms != {'darwin-arm64', 'linux-arm64', 'linux-x64'}:
        raise ValueError('All three platform reports are required')
    return {'source': source, 'outcome': 'success', 'platforms': sorted(platforms)}


def isolated_env(base, scratch, node, installed, claude_auth, codex_auth):
    env = {key: value for key, value in base.items()
           if not key.startswith(('HENNERY_', 'ANTHROPIC_', 'OPENAI_', 'CODEX_', 'CLAUDE_'))}
    env.update(HOME=str(scratch), TMPDIR='/tmp',
               PATH=str(node.parent) + os.pathsep + base['PATH'],
               HENNERY_D4_PINNED_ADAPTER_ROOT=str(installed),
               HENNERY_D4_CLAUDE_AUTH=str(claude_auth), HENNERY_D4_CODEX_AUTH=str(codex_auth))
    for kind in ('DATA', 'CONFIG', 'CACHE', 'STATE'):
        env['XDG_' + kind + '_HOME'] = str(scratch / kind.lower())
    return env


def run(args):
    if not args.allow_real_agents:
        raise ValueError('Real agents require explicit operator-owned execution')
    if os.environ.get('CI', '').lower() not in ('', '0', 'false') or os.environ.get('GITHUB_ACTIONS') == 'true':
        raise ValueError('Real-agent acceptance must run outside CI')
    if host_platform() != args.platform:
        raise ValueError('Run on the requested host platform; this command does not provision emulators')
    if subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT).strip():
        raise ValueError('Commit tracked changes before recording source-bound acceptance')
    source = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    secrets = []
    for path in (args.claude_auth, args.codex_auth):
        value = json.loads(path.read_text())
        if not isinstance(value, dict) or not value:
            raise ValueError('Source credentials must be nonempty JSON objects')
        secrets.extend(secret_strings(value))
    args.output.mkdir(parents=True, mode=0o700, exist_ok=False)
    args.output.chmod(0o700)
    inputs = {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in INPUTS}
    report = {'source': source, 'platform': args.platform, 'inputs': inputs, 'results': [], 'outcome': 'running'}

    def record(label, result):
        report['results'].append({'case': label, **result})
        if result['exit']:
            report['outcome'] = 'failed'
        private_write(args.output / 'report.json', json.dumps(report, indent=2) + '\n')
        print(json.dumps(report['results'][-1]), flush=True)
        if result['exit']:
            raise ValueError('Acceptance stopped at ' + label + '; inspect private evidence')

    env = dict(os.environ, CARGO_TARGET_DIR=str(ROOT / 'target'),
               CARGO_INCREMENTAL='0', CARGO_PROFILE_DEV_DEBUG='line-tables-only')
    build, text = execute(['nix', 'develop', '-c', 'cargo', 'test', '-p', 'hennery-testkit',
                          '--test', 'delegation_live', '--test', 'forget_codex', '--locked',
                          '--no-run', '--message-format=json'], env, args.output / 'build.log', 1800)
    record('build', build)
    binaries = artifacts(text)
    result, text = execute(['nix', 'develop', '-c', 'cargo', 'run', '-p', 'hennery-testkit', '--locked',
                            '--example', 'acceptance_runtime', '--', str(args.output / 'runtime')],
                           env, args.output / 'install.log', 1800)
    record('install', result)
    installed = json.loads(text.splitlines()[-1])
    if (installed['record']['manifest_hash'] != inputs['crates/hennery-host/manifest.json']
            or installed['record']['platform'] != args.platform):
        raise ValueError('Installed runtime differs from the candidate manifest/platform')
    node, runtime = Path(installed['node']), Path(installed['set'])
    manifest = json.loads((ROOT / 'crates/hennery-host/manifest.json').read_text())
    version = subprocess.check_output([str(node), '--version'], text=True).strip()
    if version != 'v' + manifest['node']['version']:
        raise ValueError('Installed Node version differs from the manifest')
    arch = {'darwin-arm64': 'aarch64-apple-darwin', 'linux-arm64': 'aarch64-unknown-linux-musl',
            'linux-x64': 'x86_64-unknown-linux-musl'}[args.platform]
    codex = runtime / f'codex/node_modules/@openai/codex-{args.platform}/vendor/{arch}/bin/codex'
    with tempfile.TemporaryDirectory(prefix='h-accept-', dir='/tmp') as directory:
        scratch = Path(directory)
        env = isolated_env(env, scratch, node, runtime, args.claude_auth, args.codex_auth)
        result, _ = execute([str(node), str(ROOT / 'packaging/test-mcp-source-policy.mjs'),
                             str(runtime / 'claude/node_modules'), str(args.output / 'mcp')],
                            env, args.output / 'mcp.log', 600, secrets)
        if result['exit'] == 0:
            matrix = json.loads((args.output / 'mcp/runtime-matrix.json').read_text())
            if len(matrix['results']) != 8:
                result['exit'] = 1
        record('mcp-source-policy', result)
        for test, parent, child in cases():
            label = '-'.join(part for part in (test, parent, child) if part)
            case_env = dict(env, HENNERY_D4_PARENT=parent, HENNERY_PIN_CODEX=str(codex),
                            HENNERY_D4_CODEX_PARENT_EFFORT='medium',
                            HENNERY_D4_CAPTURE=str(args.output / (label + '.capture.json')))
            if child:
                case_env['HENNERY_D4_CHILD'] = child
            binary = binaries['forget_codex' if test.startswith('pinned_codex_delete_') else 'delegation_live']
            result, text = execute([binary, test, '--ignored', '--exact', '--nocapture'], case_env,
                                args.output / (label + '.log'), 1800 if child else 360, secrets)
            record(label, qualify_case(result, text))
    for name, digest in inputs.items():
        if hashlib.sha256((ROOT / name).read_bytes()).hexdigest() != digest:
            raise ValueError('Candidate inputs changed during acceptance')
    if (subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip() != source
            or subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT).strip()):
        raise ValueError('Candidate source changed during acceptance')
    report['outcome'] = 'success'
    private_write(args.output / 'report.json', json.dumps(report, indent=2) + '\n')
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('plan', 'run', 'verify'))
    parser.add_argument('--platform', choices=('darwin-arm64', 'linux-arm64', 'linux-x64'))
    parser.add_argument('--claude-auth', type=lambda path: Path(path).resolve())
    parser.add_argument('--codex-auth', type=lambda path: Path(path).resolve())
    parser.add_argument('--output', type=lambda path: Path(path).resolve())
    parser.add_argument('--allow-real-agents', action='store_true',
                        help='operator/owned-automation opt-in; never set by routine coding sessions or CI')
    parser.add_argument('--sha')
    parser.add_argument('--report', type=Path, action='append', default=[])
    args = parser.parse_args()
    if args.command == 'plan':
        print(json.dumps({'mcp_source_policy': True, 'cases': cases()}, indent=2))
        return 0
    if args.command == 'verify':
        if not args.sha or not re.fullmatch(r'[0-9a-f]{40}', args.sha):
            parser.error('verify requires a full --sha and three --report paths')
        try:
            inputs = {name: hashlib.sha256(subprocess.check_output(
                ['git', 'show', args.sha + ':' + name], cwd=ROOT, stderr=subprocess.DEVNULL)).hexdigest()
                      for name in INPUTS}
            print(json.dumps(verify_reports(args.report, args.sha, inputs)))
            return 0
        except (OSError, ValueError, TypeError, KeyError, subprocess.SubprocessError):
            print('External runtime acceptance is missing or does not match the candidate.', file=sys.stderr)
            return 1
    if not all((args.platform, args.claude_auth, args.codex_auth, args.output)):
        parser.error('run requires --platform, --claude-auth, --codex-auth and a new --output directory')
    try:
        return run(args)
    except (OSError, ValueError, subprocess.SubprocessError):
        report_path = args.output / 'report.json'
        if report_path.is_file():
            report = json.loads(report_path.read_text())
            report['outcome'] = 'failed'
            private_write(report_path, json.dumps(report, indent=2) + '\n')
        # Never echo exception strings that could contain source credential JSON.
        print('Runtime acceptance failed; inspect the private output directory and supplied inputs.', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
