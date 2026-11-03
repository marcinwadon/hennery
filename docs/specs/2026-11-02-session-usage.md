# Session usage and cache retention

The session composer links its last reported context observation to Usage in
the details panel (a separate tab in the mobile sheet). Usage shows the last
completed turn, recorded session totals, reported model rows and the visible
family. Unknown values are absent, never zero. Input, cache reads and cache
writes are disjoint normalized categories. Output is used as reported, without
adding a separate reasoning counter.

## Observations and collection

The host captures optional end-of-turn ACP usage in `turn_ended.usage`. The
standard usage field is enabled explicitly in the pinned Rust ACP schema.
Bundled Claude/Codex profiles additionally interpret their pinned
`_meta.quota.model_usage` shape. Custom runtimes get standard counters only.
Counters must be nonnegative integers representable exactly by JavaScript,
with bounded model names and at most 32 model rows. Model names are redacted
like other host-owned text. Accumulation saturates at the wire limit and marks
the result partial.

The collector stores each owned turn's first valid observation and updates its
session projection in the same transaction. Session/sequence replay and a late
duplicate turn end cannot add it twice. A late response after a synthetic
detach can fill previously missing usage for the same ended turn without
reopening it or changing its lifecycle. Usage cannot name another session's
turn. Invalid observations and missing/cancelled responses do not become zero.
Old turns have no newly invented counts.

Live `usage_update` observations update context and optional reported cost.
Early history replay does not refresh these observations. Context usage is
the runtime's last observation; window sizes can be estimates. The UI shows
its age and unavailable-host state, without interpreting either as cache
residency or expiry. Cost is the latest cumulative runtime reading, never the
sum of cumulative snapshots. Its coverage across attachments is unconfirmed,
and it is not an invoice or remaining subscription allowance.

Session totals are durable, with known/ended turn counts and collection start.
Reported model rows can have a different accounting scope: Claude includes
native subagents, compaction and helper calls; Codex can attribute a prompt
tally to its current model. These rows are not added to the main-loop total.
The per-session display records up to 32 distinct models.

Family totals aggregate available sessions in the same owner's current hat,
including retained closed sessions. Reassigned, deleted or purged members do
not expose their data through another hat. Missing observations make totals
partial. The query is bounded to 256 members (one additional member detects
truncation); currencies are not mixed. Native subagent model rows are not
counted again as independent Hennery family sessions.

## API and presentation

- `GET /api/sessions/{id}/usage` returns the typed `SessionUsage` projection.
- `PATCH /api/sessions/{id}/cache-retention` accepts exactly
  `{"requested":"inherit"|"5m"|"1h"}`. Both routes use ordinary operator
  authentication and mutation-origin protections.
- The selected visible view refreshes usage at most once every five seconds,
  without a second transcript stream. Child usage therefore updates family
  totals even while the parent is idle. Background tabs suspend refreshes.
  Errors retain the last observation and show a visible stale/error state.
- The Compact action is offered only when the session advertises the command,
  and enabled for an idle attached session with settled configuration and no
  queued prompts. It uses ordinary prompt admission and an idempotency key;
  it preserves the operator's composer draft.

The token share is cached-read tokens divided by all reported input
categories. It is shown only when both cache categories are known. It is a
token share, not a request hit rate. There is no cache expiry countdown,
automatic warming, exact miss-cause diagnosis or claimed dollar savings.

## Requested retention

The operator may save Inherit, five minutes or one hour for a session's main
Claude conversation. It takes effect on the next attach, without editing global
settings or restarting the service. The host appends
`CLAUDE_CODE_PROMPT_CACHE_TTL` only for an explicit override and only for its
bundled Claude profile. Inherit leaves the operator's existing runtime policy
in place. Internal subagent retention is unaffected.

`session_started.indexed.cache_retention` acknowledges the request handed to a
supported runtime, including Inherit. It is not provider readback. Explicit
requests are refused by unsupported profiles, and the operator resume route
checks the host's `cache_retention` capability before changing session state.
The UI distinguishes a saved preference from the last attached request and
explains the higher one-hour cache-write rate. Existing environment/managed
controls can still take precedence.

Codex and custom-runtime retention controls remain unavailable until their
contract is established. Clearing an old override to Inherit is always allowed,
including after a runtime downgrade. Preferences survive restart and are copied
by New like this. Deleting a session scrubs observations and preferences along
with its transcript.

## Verification

Regression coverage exercises duplicate and late reports, missing and invalid
counts, restart/resume, replay timestamps, cumulative costs, family hat
boundaries and deletion. A fake ACP process reports known counts through the
real host transport and outbox. Browser acceptance reads the resulting
collector API and panel at desktop/mobile widths. These fixtures establish
integration behavior; live provider acceptance is recorded separately.

The current bundled contracts are Claude ACP 0.99.0 with Agent SDK 0.3.312,
and Codex ACP 2.3.0 with CLI 0.165.0. The opt-in
`pinned_runtime_usage_observations` test in `delegation_live.rs` runs one real
turn through an isolated collector, host and credential copy. For Claude it
also saves retention and checks the acknowledgement after park/resume; it
does not verify provider cache residency. Requalify these contracts when
updating the adapter pins.
