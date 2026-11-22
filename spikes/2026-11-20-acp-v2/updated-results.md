# Updated dependency experiment

Repository pins updated: Codex ACP 2.8.0 / Codex CLI 0.170.0;
Claude ACP 0.109.0 / Claude Agent SDK 0.3.328;
Rust ACP SDK 5.4.0 / schema 1.19.0. Managed Node unchanged at 24.22.3.
Lockfiles and host distribution manifest regenerated through hennery-pins.
No running host's installed adapter set was switched.

## Verified

- cargo check --workspace --locked: passed in the Nix dev shell.
- cargo test -p hennery-host -p hennery-pins --lib --locked:
  148 host tests and 11 pins tests passed.
- hennery-pins check: generated manifest matches pins, registry and lockfiles.
- packaging/audit-adapters.sh: zero reported vulnerabilities for both locks.
- Codex 0.170.0 generated JSON schema retains thread/delete with required string
  threadId. Live deletion and multi-platform acceptance remain unverified.

## Live background experiment

Installed exact regenerated npm locks in temporary directories. Used live.py
with ACP_PACKAGES, ACP_CODEX_VERSION and ACP_CLAUDE_VERSION overrides. Explicitly
requested native retained commands (no shell ampersands) ending with 0 and 7.
Raw traces are private temporary evidence; no credentials committed.

Codex ACP 2.8.0 FIXES the standard background-list gap observed in 2.7.0.
It emits terminal announcements, background lists growing to two entries, idle
with active background work, terminal exitStatus exitCode 0 and 7, then lists
shrinking to one and zero. Cancellation capabilities are cleared at completion.
Close/resume succeeded and resume emitted a current idle background=[] snapshot.

Claude ACP 0.109.0 reports the corresponding background sequence correctly and
close/resume succeeded. The missing numeric outcome remains: BOTH terminal
exitStatus objects are empty, despite test commands exiting 0 and 7. Thus host
logic can count completed tasks but cannot promise success/failure classification
from the terminal contract. Do not silently label unknown outcomes successful.

The Rust SDK upgrade was compiled and unit-tested, but live.py speaks raw JSON-RPC
and does not exercise Rust's v2 driver (Hennery still uses v1). This distinction
matters: successful adapter tests are not end-to-end Hennery v2 acceptance.

Existing unresolved scope: full steering/permission/config/MCP parity, resume
while background jobs remain active, cross-process replay, multi-platform live
acceptance. Completion followed by close/resume does not prove task survival
across an adapter restart.

Codex cancellation follow-up: session/cancel notification followed by an
advertised terminal/cancel request returned success; close/resume succeeded.
The same terminal emitted exitStatus twice: first null exitCode/signal, later
exitCode=-1. A host must deduplicate terminal completion and must not treat an
empty/null exit code as success. This immediate per-item cancellation does not
prove independent survival after foreground-only cancel.

Claude 0.109.0 cancellation follow-up also returned terminal/cancel success,
then close/resume success. Background removal and final idle were observed.
The same foreground-only-survival limitation applies.

## Product decision after audit

The user accepted limited outcome support for Claude: completed terminals with
missing exit outcome count as succeeded in the UI summary. This supersedes the
requirement above to introduce an unknown-outcome count before shipping. It does
not change the observed adapter behavior. Preserve raw evidence; do not invent
exitCode=0, and retain explicit failure/cancellation evidence when available.
Codex continues to use its reported numeric outcomes.
