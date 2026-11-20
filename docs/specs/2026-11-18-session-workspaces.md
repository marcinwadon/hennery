# Managed session workspaces

Implementation review of the direction discussed in proposal PR #351.

## Behavior

The launcher can create a new Git worktree or share the selected directory.
The browser remembers that preference independently from New like this.
New like this opens a workspace configuration dialog; new worktree is its
initial default and the operator can select shared cwd. Existing empty-body
API calls retain their shared-directory behavior.

`session_spawn` continues to share cwd unless `worktree_base` is specified.
That option names a commit or ref; the tool description explains that local
changes are excluded. Repeated delegated requests use a stable workspace ID
bound to caller, operation key and request digest, and the host also verifies
the complete persisted provisioning request before returning an existing tree.

The host creates a new `hennery/<id>` branch. Worktrees and private ownership
records live under the repository's common Git directory, in
`hennery-workspaces/`. Destination and source must be inside host roots.
Source subdirectories are preserved relative to the new checkout. Generated
paths are checked against hat rules; a path that would change the hat is refused.

## Preparation

The workspace form previews the resolved base commit and submits that SHA.
Tracked files start at this revision; local changes in the source are left alone.

A saved preparation profile is scoped to owner, host and canonical project
source. Managed workspaces retain the source profile identity, so an isolated
parent can use it when creating isolated children. The operator can save a
file list and optional shell command; saving requires fresh step-up. An inline
command also requires step-up. Profiles are not inferred from ignored files.

At most 64 regular files, totaling 16 MiB, are copied. Paths must be relative,
without traversal or symlinks. Existing checkout files are not overwritten.
Preparation files are private. The optional command has a 90-second deadline;
its process group is killed when preparation ends, including on failure.
Preparation errors prevent the adapter from starting. Provisioning ownership
is persisted before checkout/preparation and recovered after uncertain delivery.

## Membership and archival

A workspace has multiple users. Creating a session with its canonical root or
a subdirectory acquires membership in the session transaction. Starting,
active, parked, closed and failed sessions retain it. Archive/delete releases it.
An admission transaction cannot join a workspace while cleanup holds its fence.

`closed` still permits explicit resume. The new `archived` state retains the
transcript but prevents resume. Archive is available only for closed sessions.
Archive does not erase the agent transcript. Deletion retains existing erasure
behavior and separately retains outstanding workspace cleanup ownership.
Archived sessions are included in host close reconciliation after reconnect.

## Cleanup

Archive and delete record cleanup intent. The worker retries on connected
hosts, with fair attempt ordering. Host cleanup checks registered ownership,
repository identity, symlink replacement and live actors using that worktree.
Branches are retained. Uncommitted, untracked and ignored files block automatic
removal. Settings lists retained workspaces even after their sessions are deleted.

The operator can explicitly discard a blocked, unused workspace in Settings.
That action confirms loss of all local files and requires fresh step-up. It
still requires the atomic no-members fence and host ownership checks.

A timeout keeps the deletion fence: it does not prove host deletion stopped.
Retries are serialized and idempotent. Unused successful provisioning that
never reached session admission becomes eligible for cleanup after ten minutes.
Uncertain provisioning is inspected after three minutes without rerunning setup.

## API additions

- `POST /api/sessions`: optional `workspace: { base, files, command? }`.
- `POST /api/sessions/{id}/new-like-this`: optional workspace options body;
  no body shares the existing cwd.
- `POST /api/sessions/{id}/archive`.
- `GET /api/sessions/{id}/workspace`.
- `GET /api/workspaces`: bounded inventory, including outstanding deletion jobs.
- `POST /api/workspaces/{id}/discard`: explicit local-file loss, with step-up.
- `POST /api/workspace-preview`: source selection plus base ref.
- `GET/PUT /api/workspace-profile`: source selection in query; PUT needs step-up.

Profile/preview source selection is `host_id` plus `cwd`, or `session_id`.
New host protocol operations require the `workspaces` capability. Older hosts
refuse these requests rather than silently launching in the source checkout.

## Validation limits

Host tests exercise real temporary Git repositories, file preparation,
idempotence, failed preparation recovery, root restrictions, cleanup refusal,
branch preservation and source changes. Store tests cover membership and
archival fences. Existing host/session, MCP contract and launcher checks are
part of verification. The workspace lifecycle also passed real-provider acceptance
with both pinned adapters; see [live acceptance evidence](../spikes/2026-11-18-workspace-live-acceptance.md)
for the matrix, assertions, reproduction and the observed Codex transport fallback.
This is workspace acceptance, not a claim that all runtime release checks ran.
