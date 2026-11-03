# Automatic compaction budgets

Follow-up to [session usage](2026-11-02-session-usage.md). Expose an optional,
durable per-session budget for the runtime's own automatic compaction. Presets
are runtime default, 100k, 200k, 500k and 1M tokens. Existing sessions retain
runtime defaults. A budget is applied at the next attachment and is copied by
New like this. It does not initiate synthetic user prompts or change transcript
admission, pending questions, child coordination, permissions or model choice.

The bundled Claude profile receives `CLAUDE_CODE_AUTO_COMPACT_WINDOW`; its
native engine reserves output space and clamps the requested window to the
model's actual window. Codex ACP receives only the numeric
`model_auto_compact_token_limit` through its documented `CODEX_CONFIG` launch
override. This is per process; Hennery does not edit shared Codex configuration.
The native engines choose safe compaction boundaries, including inside long
tool loops. A budget is not a hard billing cap or a precise memory guarantee.

The collector persists requested and last-attached values separately. A host
capability gates explicit requests; runtime support is established by the
host's bundled profile, not an executable name. Custom and own-CLI profiles
remain unsupported. Restoring runtime defaults is always allowed, including
after downgrade. Acknowledgement proves launch delivery, not provider readback.
Deletion removes the policy together with session data. No summary is invented
from historical usage, and no busy session is restarted to change its policy.

`PATCH /api/sessions/{id}/context-budget` accepts, for example,
`{"requested":"500k"}`. Allowed values are `inherit`, `100k`, `200k`, `500k`
and `1m`. The Usage response carries
`budget.supported`, `budget.requested` and `budget.attached_request`. Explicit
start/resume requests require the host's `context_budget` capability. Native
automatic compaction must be enabled; a budget does not override an existing
decision to disable it.

Compaction discards some detail and can itself consume tokens or rewrite cache.
The UI states this tradeoff and does not promise net savings. Native defaults
remain available. Broad automatic defaults require a deliberate product choice.

## Cache warming boundary

The Anthropic Messages API supports a no-output replay of an identical request
to refresh cache. Hennery's pinned ACP contract does not expose that full
provider request and headers. The Agent SDK's `prewarm` starts a spare process;
it is not prompt-cache refresh. Cache warming therefore needs an upstream
adapter/SDK capability before Hennery can schedule it without changing the
conversation. Do not inject hidden user turns, intercept provider credentials,
add a traffic proxy or claim cache residency from elapsed time.

## Contract references

- [Claude auto-compact window](https://code.claude.com/docs/en/model-config#context-window-and-auto-compaction)
- [Claude SDK reference](https://code.claude.com/docs/en/agent-sdk/typescript)
- [Codex configuration](https://developers.openai.com/codex/config-reference)
- [Anthropic cache refresh](https://platform.claude.com/docs/en/about-claude/models/optimizing-for-cost-and-intelligence)

Pinned acceptance must cover Claude SDK 0.3.312 / ACP 0.99.0 and
Codex CLI 0.165.0 / ACP 2.3.0. Test both request delivery and real native behavior
in isolated homes before treating documentation as qualification.

## Local acceptance

The pinned Claude SDK's no-inference context-usage readback reported a default
threshold of 967,000 tokens for the tested model, 67,000 for a requested 100k
window and 467,000 for 500k. The reserved margin is native runtime behavior,
not a Hennery-derived limit. Codex CLI's config/read confirmed the 100,000 token
override. Both runtimes completed a real turn, parked, resumed with the saved
100k request and completed another real turn. Those small turns did not trigger
compaction; they qualify policy delivery and continuation, not summary quality
or realized monetary savings. Source auth remained untouched and copied auth
was removed after the probes.
