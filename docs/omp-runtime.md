# External OMP configuration and runtime prerequisites

`hennery host adapters omp set /absolute/executable --agent-data-dir /private/root`
saves preparation in the paired host's `host.toml`. `remove` removes only this
configuration. Neither command installs OMP, restarts a host, or removes agent
data. The executable must be a regular executable without symlink components;
the existing root must belong to the host user and have mode `0700`.

This binary does **not** offer the configured OMP command as a runnable agent.
Startup reports that limitation. Explicit `--agent` commands retain their
existing precedence and generic, unverified security profile. No OMP MCP
isolation or source mode is advertised.

Doctor reads external configuration independently of managed adapter sets. It
checks the executable's `--version` exit status, a bounded versioned missing-file
handshake, and ACP `initialize`, using the configured literal
`<executable> --agent-dir <root> acp` arguments. External output is parsed privately.
Failures point to the external executable/configuration; a managed adapter
update does not install or repair OMP. Even a successful handshake and ACP reply
produce a warning that runtime integration remains unavailable.

The handshake asks `session lease hold` about a fresh absent JSONL path below
`<root>/sessions/<fresh-project>/`. It accepts only exit 1 and
`{"schemaVersion":1,"state":"error","code":"session_missing"}` within its output
limit. It also refuses creation of the fresh project path. This response proves
only recognition of that missing-file protocol. It does not prove that agent
roots are frozen, a writer is excluded, a lease is held, or deletion is safe.

## Exact session-file binding

The host library provides read-only discovery and verification for subsequent
lifecycle integration. Discovery reads direct project directories below
`<root>/sessions`, requiring exactly one JSONL whose logical `session` header
matches the ACP ID and canonical workspace. OMP's optional physical `title`
slot is skipped; filenames and message contents never establish identity.
Directories and files are opened relative to no-follow descriptors. Unsafe
ownership, writable metadata, ambiguous IDs, malformed/missing identity and
changed recorded paths are refused.

The registry can commit ACP ID, literal executable/root, canonical workspace,
exact JSONL path and agent home in one FULL-synchronous SQLite transaction. An
existing ID cannot be rebound to another file. Resume verification checks the
recorded file rather than substituting a new lookup result. These APIs are not
yet wired to `session_started`; this change does not claim a working OMP
create/prompt/cancel/park/resume/close/restart lifecycle.

## Upstream qualification before enabling sessions

Source checks on 2026-10-29 covered official release
[v18.14.0](https://github.com/can1357/oh-my-pi/tree/31147f9d1a76608549821c3759f8487b4d66c72d)
and [main at 338d2ce6](https://github.com/can1357/oh-my-pi/tree/338d2ce6658dfdbfba19efe0f006f0d5a8edd66d).
They provide ACP new/load/close, but not the proposed `--agent-dir` or
`session lease hold` CLI contracts. These source facts do not qualify later
releases or a locally patched build.

Required upstream work includes:

- Freeze auth, settings and session roots before loading `.env`; cover existing
  XDG directories, named profiles, default-root equality and child/worker
  inheritance. `PI_CODING_AGENT_DIR` alone does not establish this contract.
- Keep ACP's requested ID and exact session file stable during load and later
  writes. Current session persistence can recover an ownership/write conflict
  by moving to a sibling with a fresh ID. ACP load does not return an ID and
  must refuse such an unnoticed remap.
- Implement a real holder using the writer's ownership primitive; fail closed
  on lease errors. Current `claimSession` can return a no-op release callback
  after an acquisition error. That callback is not a held lease.
- Retain ownership until close has drained backing writes and artifact work;
  the inspected `SessionManager.close` releases its claim before that drain.
- Qualify contention across ownership namespaces and different XDG settings.
  A holder cannot exclude old or uncooperative writers merely by using a lock.

Tests must cover actual competing writers, stable ACP identity across a busy
load and prompt, errors during lease acquisition, close/drain interleavings,
process death, missing JSONL without mutation, and held/busy/error responses.
Only then can lifecycle integration require registry persistence before
`session_started`, use the exact recorded binding on resume/restart, and wait
for ACP close before stopping the process group.

OMP forget remains unimplemented. It requires a separately qualified held-lease
removal path, durable progress, JSONL removal last, and read-only reconciliation
of an already absent family after a lost reply. Neither a missing-file handshake
nor a stored binding authorizes unleased cleanup.
