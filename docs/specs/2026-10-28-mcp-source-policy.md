# Per-hat, per-ACP MCP sources

The operator selected modes per `(hat, actual agent/ACP ID)`:
`managed_only` and `managed_plus_native`, with `legacy` (existing adapter
behavior) always selectable. The same hat can use Claude managed-only and
Codex legacy/native without one agent blocking the other. Agent identity is
the stored/configured ID, not the selected model, profile alias or executable.
The internal `legacy` value preserves existing behavior; it is not an implicit
choice of either new mode. Managed MCP includes gateway connections, built-in
session tools and Hennery-registered stdio servers running on the host.

## Persistence and migration

Existing hats and Create requests without new fields preserve their scalar
policy/revision (`legacy` before source-policy configuration). Sparse overrides
are stored in `hats.mcp_source_overrides`; migration adds an empty JSON map
without backfilling or changing runtime behavior. New-hat UI sends only a
Claude `managed_only` override; Codex and other agents retain legacy defaults.

Create/PATCH accept `mcp_source_overrides: {agent_id: policy | null}`. Omitted
field/keys and an empty map preserve settings. `null` resets that agent to the
scalar fallback; explicit `legacy` overrides even an explicit scalar fallback.
Unknown policy values, invalid IDs and more than 128 stored agent entries are
refused. GET returns an optional map of `{policy: policy | null, revision}`.
Reset retains its per-agent revision tombstone; repeated save/reset is a no-op.
Edits increment only that pair's revision. Legacy scalar updates advance the
fallback above stored revisions and affect only agents inheriting that fallback;
an explicit override keeps its revision. A reset resolves to the fallback with
the greater of its reset revision and the fallback revision. This avoids both
cross-agent pending changes and revision rollback on reset. Changes require
the existing operator step-up; purging hats cannot change settings. Owner
filters apply to every read and write. No update restarts a running process.

## Attach contract

The collector resolves policy and revision from the hat and the session's
stored actual agent ID in the start/resume transaction. Probes use their
actual requested agent ID. Manual, queued and delegated new/load paths use
the same selection; reassignment invalidates the old attach and the next
attach resolves this session's agent in the new hat. Editing another agent
neither invalidates its ACK nor changes its requested/applied/pending state.
Every actual attach, including delegated and queued resume, carries that
snapshot even when there are no managed servers. A live host capability and
the agent's verified profile must support the request; otherwise admission
fails. A reconnect to an older host is checked against that connection, not a
cached prediction. An older collector's omitted field means legacy behavior.

The sequenced `SessionStarted` acknowledgement identifies the request and
echoes the applied snapshot. A stale request, revision or delegated generation
cannot confirm a newer launch. Generation belongs to its request: a later
ordinary resume uses generation zero instead of inheriting the previous queued
or delegated attach's generation. Pending startup MCP discovery remains available
under the committed session identity; it does not authorize mutation by an
unattached session. A policy acknowledgement describes the mechanism used,
not the health or complete inventory of native servers.

If an API client returns a previously explicit session to `legacy`, its
collector-only snapshot still fences late facts. The legacy wire field stays
omitted, so an old host can acknowledge the correlated request without claiming
an explicit source guarantee. Previously legacy-only sessions keep their
existing wire behavior.

Reassignment keeps the historical source snapshot for correlation and invalidates
its acknowledgement eligibility, including after returning to the previous hat.
Only the next real attach creates a fresh eligible request. The existing
reassignment transaction revokes the old token, and the next attach resolves
delivery and policy in the new hat.

Saving a setting does not restart an active/starting process. Its applied
snapshot remains visible. The next real attach reads the new setting;
presumed parking and reconnect of a living process do not reapply it. Native
content already present in history is not erased. Agent homes and forget roots
are not moved. Children resolve the current policy of their own hat at attach,
rather than copying their parent's environment.

## Profiles and boundaries

Bundled Claude uses strict MCP configuration for managed-only on every new/load.
Combined uses the controlled session option to permit its normal native MCP
sources. Catalog probes retain strict configuration independently of this
choice. Own-CLI Claude, Codex and arbitrary commands cannot claim verified
support merely from their name. In particular, Codex's configured-name filter
can omit an ACP server; disabling that filter can deep-merge incompatible
definitions. Neither behavior qualifies its combined mode. Explicit unsupported
requests fail even when the managed list is empty.

| Profile / live host | `legacy` | `managed_only` | `managed_plus_native` |
|---|---|---|---|
| Qualified bundled Claude | Previous strict behavior | Strict new/load, including zero managed servers | Managed definitions plus native sources; whole-definition Hennery precedence |
| Codex | Previous default-hat fallback | Refused | Refused pending source/merge qualification |
| Own-CLI Claude / generic command | Previous fallback, without a new isolation claim | Refused | Refused |
| Older host, missing source capability | Previous behavior | Refused even with zero managed servers | Refused even with zero managed servers |

Capability prediction comes from the latest accepted host report; admission
rechecks the live connection. Active process settings remain unchanged by a
hat edit or a presumed reconnect. A conflicting request to reuse that actor
is refused rather than acknowledging a policy it never applied.

Future runtime profiles (including OMP) need independent capability and pin
qualification. External MCP configuration continues to use the same managed
mount/auth contract; source permission cannot widen those grants.

Combined support requires pin-specific evidence that the managed definition
is preserved in a native-name collision, without merging credentials or sending
them to the native endpoint. Deterministic whole-definition Hennery precedence
is acceptable; otherwise the profile must refuse the collision or remain
unsupported. A namespace prefix alone is insufficient. Native credentials and
account connectors remain subject to the runtime's existing consent, disabled
settings and administrative policy; Hennery does not rewrite global configs.

Managed delivery and native-source permission are separate. Combined is never
reported as isolated or implemented by widening the legacy `isolation_waived`
path. Scope and token revocation remain unchanged. The UI distinguishes the
current hat setting, the last requested snapshot and its actual acknowledgement,
including a setting waiting for the next attach. Source revisions are compared
only within the same hat; reassignment refreshes the detail and shows the new
hat's setting even if its revision is lower or equal. The last confirmed attach
remains historical until a fresh attach is acknowledged.

## Acceptance

**Qualified Claude composition.** ACP 0.93.0 with bundled SDK/CLI package
0.3.300 was exercised against real local MCP endpoints through the actual ACP
adapter. Hennery's explicit definition wins over colliding user and project
HTTP definitions; their endpoints receive no connection and their extra headers
do not reach the managed endpoint. Eight cases cover new and fresh-process
load, both source modes, changed mode at resume, and zero managed entries.
The fixture uses nonce-bearing real `tools/call` requests with a synthetic local
model provider, without account credentials. It does not establish cloud
account-connector availability. In-process hot replacement is outside this
contract: Hennery applies policy only at a new process attach.

Run `nix develop -c node packaging/test-mcp-source-policy.mjs <node_modules>
<scratch-output>` with modules installed from the committed Claude adapter
lockfile in a scratch directory. The fixture rejects different adapter/SDK
versions and builds a fresh HOME/XDG/config tree. Repeat it for pin changes;
do not replace it with CLI argument snapshots or model prose.

Use two hats concurrently, synthetic managed HTTP/stdio and native servers with
distinct nonce tools, and scratch HOME/XDG/agent configuration. Check actual
MCP calls, zero-managed cases, conflicting names, new/load, delegated/queued
resume, stale acknowledgements, old hosts and cross-hat denial. Fake adapter
tests prove protocol handling; they do not replace real-runtime evidence.
