# Repeatable managed-runtime acceptance

Decision, 2026-11-04: use owned native hosts with local provider credentials.
Routine coding sessions and CI use unit/contract tests and fake agents. They
must not start real agent sessions, including the synthetic-provider MCP probe.
Only the operator or separately configured automation in the operator's Hennery
runs real acceptance. Keep credentials off GitHub Actions. Each host runs the same driver from its
own clean checkout of the candidate commit, with its own Cargo target directory.
Use macOS arm64, glibc Linux arm64 and glibc Linux x64 hosts; the driver does not
provision containers or emulators. Keep the host/toolchain prepared between pin
updates so a release does not become an environment-repair exercise.

Inspect the fixed plan without credentials or downloads:

```sh
nix develop .#release -c python3 packaging/runtime_acceptance.py plan
```

On each host, supply explicit source credential files (JSON objects) and a new
private output directory. For example, on Linux x64:

```sh
nix develop .#release -c python3 packaging/runtime_acceptance.py run \
  --allow-real-agents \
  --platform linux-x64 \
  --claude-auth /private/credentials/claude.json \
  --codex-auth /private/credentials/codex.json \
  --output .superpowers/runtime-acceptance/candidate-linux-x64
```

The driver requires explicit real-agent opt-in and refuses CI execution,
a mismatched host platform, dirty/untracked source or an
existing output directory. It compiles both integration test binaries once,
installs the embedded candidate with the production installer into a new
scratch runtime, verifies the manifest/platform and Node version, and runs:

- the eight synthetic-provider ACP/CLI MCP source-policy cases;
- sixteen opt-in invocations, including all four delegation pairs with their
  existing lifecycle assertions, provider/usage/tool tests and real Codex deletion.

No test limits or assertions are relaxed. Tests execute directly from Cargo's
reported binary paths, without rebuilding for each row. A successful invocation
must report one actual passing test; a renamed or missing test cannot pass with
zero tests. The first failing case stops the driver and retains prior results.
There is no automatic retry hiding a first failure.

Each invocation uses a temporary HOME/XDG environment. The existing Rust harness
copies explicit source credentials into its own private test homes; the driver
never writes the originals. Local raw test output is temporary and credential
strings are redacted before retaining logs. The output root is mode 0700 and
logs/report are mode 0600. Captures remain private; do not upload this directory
or agent payloads to CI artifacts. Delete test-owned runtime payloads after
inspection; nothing in that output directory is needed by a real host service.

`report.json` binds results to the source commit, platform and SHA-256 digests
of the manifest, both adapter locks, Cargo.lock and flake.lock. A running or
failed report is not acceptance. Require successful reports for the same source
and inputs from all three hosts before merging a pin bump. The Codex source and
deletion-contract audit in `packaging/README.md` remains a separate requirement.
No report authorizes release publication or reuses qualification for new inputs.

Pin bumps move through code-ready and externally accepted states separately.
The coding session reports a missing external result and hands off; it neither
starts real agents nor polls indefinitely for the operator's acceptance. The
existing pin-bump acceptance requirements remain, but execution belongs to the
operator's separate workflow. This amends earlier instructions that assigned
real-agent execution to the coding session or pins CI.

Coding sessions can consume the operator's three private reports without
starting agents. The verifier reads expected input digests from the requested
Git commit and requires every required case on all three distinct platforms:

```sh
nix develop .#release -c python3 packaging/runtime_acceptance.py verify \
  --sha <full-candidate-SHA> \
  --report /private/acceptance/macos/report.json \
  --report /private/acceptance/linux-arm64/report.json \
  --report /private/acceptance/linux-x64/report.json
```

The reports are trusted operator evidence, not cryptographically attested host
execution. Only distribute the compact reports through the operator's private
workflow; full logs, captures, installed runtimes and credentials stay on hosts.

This PR supplies the driver, not host provisioning or a live three-host result.
Each owner must run it where the intended provider credentials are available.
Run its offline regressions with `nix develop .#release -c python3 -m unittest
discover -s packaging/tests -p test_runtime_acceptance.py`.
