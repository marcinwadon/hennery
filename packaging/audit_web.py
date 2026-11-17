"""Keep raw web audit evidence; bound one unpatched build-tool exception."""
from datetime import date
import json
from pathlib import Path
import subprocess
import sys


ADVISORY = 'GHSA-fc5q-39xc-vjpf'
EXPIRES = date(2026, 12, 2)
TOOL_ROOTS = {'vitest', 'vite', '@testing-library/jest-dom',
              '@vitejs/plugin-react', '@tailwindcss/vite'}
SEVERITIES = ('info', 'low', 'moderate', 'high', 'critical')


def accepted(report, status, today):
    """Fail closed on new findings, production paths, fixes, expiry or errors."""
    if status not in (0, 1) or report.get('error') or report.get('ignoredAdvisories'):
        return False
    advisories = report.get('advisories')
    counts = report.get('metadata', {}).get('vulnerabilities', {})
    if not isinstance(advisories, dict) or any(type(counts.get(k)) is not int for k in SEVERITIES):
        return False
    if not advisories:
        return status == 0 and all(counts[k] == 0 for k in SEVERITIES)
    if status != 1 or today >= EXPIRES or len(advisories) != 1:
        return False
    advisory, = advisories.values()
    if (advisory.get('github_advisory_id') != ADVISORY
            or advisory.get('module_name') != 'picomatch'
            or advisory.get('patched_versions') is not None
            or advisory.get('patched_versions_unpublished') is not True
            or advisory.get('severity') != 'high'
            or any(counts[k] != (1 if k == 'high' else 0) for k in SEVERITIES)):
        return False
    findings = advisory.get('findings')
    if not isinstance(findings, list) or not findings:
        return False
    for finding in findings:
        paths = finding.get('paths')
        if (finding.get('dev') is not True or finding.get('version') != '4.0.7'
                or not isinstance(paths, list) or not paths):
            return False
        for path in paths:
            if not isinstance(path, str):
                return False
            parts = path.split('>')
            if len(parts) < 3 or parts[0] != '.' or parts[1] not in TOOL_ROOTS or parts[-1] != 'picomatch':
                return False
    return True


def main():
    result = subprocess.run(['nix', 'develop', '.#web', '-c', 'pnpm', '--dir', 'web', 'audit', '--json'],
                            cwd=Path(__file__).resolve().parents[1], capture_output=True, text=True)
    # Preserve the original finding and severity in release evidence, even when accepted.
    sys.stdout.write(result.stdout)
    sys.stderr.write(result.stderr)
    try:
        report = json.loads(result.stdout)
        ok = isinstance(report, dict) and accepted(report, result.returncode, date.today())
    except (ValueError, TypeError, AttributeError):
        ok = False
    if ok and report['advisories']:
        print(f'Accepted until {EXPIRES}: {ADVISORY}, picomatch4.0.7 in trusted build/test globs only; '
              'no patched release. Original high-severity finding retained.', file=sys.stderr)
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
