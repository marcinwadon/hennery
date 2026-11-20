# Managed workspace live acceptance

Date: 2026-11-18. Scope: the workspace lifecycle implemented in PR #359.
These were real provider turns through the pinned ACP adapters, not scripted
fake-agent responses. Only disposable repositories and copied scratch agent
homes were supplied to the adapters. Source credentials and user projects
were not used as agent working directories.

## Accepted matrix

| Runtime | Adapter | Underlying runtime pin | Result | Provider turns per accepted run |
| --- | --- | --- | --- | --- |
| Claude | `claude-agent-acp` 0.107.3 | Claude Agent SDK 0.3.326 | PASS, 11.05 seconds | 2 |
| Codex | `codex-acp` 2.7.0 | Codex CLI 0.169.0 | PASS, 26.34 seconds | 2 |

Adapter package versions are checked against the embedded manifest by the
existing live harness; the Codex native executable version is checked too.
The model choices were the harness's catalog-selected Sonnet and gpt-6-sol.
The disposable write sessions explicitly selected an offered non-asking mode.
Provider usage had to report both turns and positive token counts.

## Assertions

The ignored `pinned_runtime_workspace_lifecycle` test in
`crates/hennery-testkit/tests/delegation_live.rs` exercises:

1. Commit-only base preview and saved preparation profile, including local
   `AGENTS.md` and a command that writes a preparation marker before attach.
2. A real native tool writes a marker and its actual cwd in the new worktree;
   committed source contents are present there, while source-local changes
   remain unchanged and the source receives no model-written marker.
3. Park/load preserves the directory. A new provider turn recalls the marker
   without tools. Only assistant messages after this new turn began are used
   for the assertion; streamed chunks are assembled with `assistant_text`.
4. Close/load preserves cwd and the agent's own session identity.
5. New like this with workspace options creates a separate worktree and
   inherits the saved source preparation profile. Empty-body New like this
   shares the existing cwd.
6. Archiving the first session does not remove the workspace while the shared
   clone retains it. Archiving the last user requests cleanup.
7. Local files block automatic removal. Explicitly approved scratch discard
   removes both owned worktrees while preserving their branches and all
   original source files.

The harness's unrelated active parent session also remains attached during
cleanup: unrelated sessions must not block removal of an unused worktree.
No certificate verification or workspace ownership checks were disabled.

## Reproduction

Install the repository's adapter package/lockfiles in a disposable directory
with `npm ci --ignore-scripts`. Use private credential source paths and let the
harness copy them into its disposable homes. Do not point adapter HOME at a
real user home.

```sh
HENNERY_D4_PARENT=claude \
HENNERY_D4_PINNED_ADAPTER_ROOT=/scratch/pinned-adapters \
HENNERY_D4_CLAUDE_AUTH=/private/claude-credentials.json \
HENNERY_D4_CODEX_AUTH=/private/codex-auth.json \
cargo test -p hennery-testkit --test delegation_live \
  pinned_runtime_workspace_lifecycle -- --ignored --exact --nocapture
```

Repeat with `HENNERY_D4_PARENT=codex`. Each accepted run performs two provider
turns and can incur provider charges. The ordinary test suite leaves this test
ignored and does not read provider credentials.

## Observed transport limitation

A Codex run emitted a WebSocket certificate/UnknownIssuer warning and fell back
to HTTPS before returning the correct recalled marker. The provider turn still
completed with usage. The history assertion checks the exact marker line in
new assistant output; it does not require transport notices to be absent.
This acceptance does not claim that the environment's WebSocket trust issue is
resolved, or replace broader runtime release acceptance.
