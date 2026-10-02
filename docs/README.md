# hennery design documents

Status: the walking skeleton is implemented (Rust workspace, end-to-end host ↔ collector
slice); everything else is design. The maintainer's decisions are recorded below.

## Reading order

1. [Architecture (umbrella spec)](specs/2026-09-25-hennery-architecture-design.md)
   — product, topology, protocol principles, sessions, auth, hats, module
   boundaries, testing. Authoritative where subsystem specs disagree.
2. Subsystem specs:
   - [ACP core](specs/2026-09-26-acp-core-design.md) — host, adapters,
     host↔collector protocol, session state machine, storage, session API.
   - [Kernel](specs/2026-09-26-kernel-design.md) — storage, config, operator
     auth, pairing, hats, push, HTTP security.
   - [MCP gateway](specs/2026-09-26-mcp-gateway-design.md) — connections,
     OAuth, per-session tokens, streaming proxy, renderers.
   - [Frontend](specs/2026-09-26-frontend-design.md) — views, data layer,
     transcript fold, cards, PWA.
   - [Distribution](specs/2026-09-26-distribution-design.md) — binary,
     managed runtime and adapter manifest, release pipeline, services, doctor.
3. Evidence:
   - [Spike: per-session MCP over ACP](spikes/2026-09-25-per-session-mcp.md)
     and its [harness](../spikes/2026-09-25-mcp-per-session/).
4. Plans:
   - [Walking skeleton](plans/2026-09-26-walking-skeleton.md) — the first
     implementation plan (9 tasks) — executed 2026-09-27; its "Execution status" section
     lists where the built code deviates from the task text.
   - [Teardown and reconciliation](plans/2026-09-27-teardown-and-reconciliation.md) —
     plan A of the resume work (9 tasks): adapter supervisor and exit watcher, park/close,
     idle reaper, conflict events and reconciliation after reconnect. Executed 2026-09-27 (see its
     "Execution status").
   - [Resume (B1)](plans/2026-09-28-resume.md) — plan B1 (10 tasks): `resume_session` with
     `session/load` and replay suppression, presumed park while a host is offline, conn_id-keyed
     waiters, one visibility rule for unapplied facts. Executed 2026-09-29 (see its "Execution status").
   - [Cancel and capabilities (B2a)](plans/2026-09-29-cancel-capabilities.md) — `cancel_turn`,
     `hello.capabilities` with the park gate, and B1's two carry-overs. Executed 2026-09-29 (see its "Execution status").
   - [Session config (B2b)](plans/2026-09-30-session-config.md) — model, axes and mode on start
     and resume, `set_config`, the session catalogue, and B2a's two folds. Executed 2026-09-30 (see its "Execution status").
   - [Permission and elicitation](plans/2026-10-01-permissions.md) — plan (2) (7 tasks): the
     pending set and answer queue, teardown hooks, answer endpoint. Executed 2026-10-01 (see its "Execution status").
   - [Host pairing (3a)](plans/2026-10-01-host-pairing.md) — the first part of plan (3) (7 tasks):
     pairing codes and enrollment, `hennery host join`, the Ed25519 `hello` proof, `up` pairing its
     own host, and host revoke. Operator auth (3b) and passkeys (3c) follow. Reviewed and amended
     2026-10-01; executed 2026-10-01 (see its "Execution status").
   - [Operator auth (3b-i)](plans/2026-10-02-operator-auth.md) — the first part of plan 3b (7 tasks):
     the setup link, the owner's password and `public_url`, login and sessions behind the cookie, the
     `Origin` rules, step-up, and the development bearer removed. Listeners, `config.toml` and the admin
     socket (3b-ii), then `owner_id` everywhere (3b-iii), follow. Amended after the security review of
     2026-10-02; executed 2026-10-02 (see its "Execution status").
   - [Operator auth (3b-ii)](plans/2026-10-03-operator-auth-2.md) — the second part of plan 3b (7 tasks):
     several listeners and the health checks, `config.toml`, the admin socket with `hennery admin`
     (setup link, password and `public_url` resets, hosts, pairing codes), and agents no longer
     inheriting stray descriptors. `owner_id` everywhere (3b-iii) and passkeys (3c) follow. Amended after
     the security review of 2026-10-03; executed 2026-10-01 as PRs #14 and #15 (see its "Execution status").
   - [`owner_id` everywhere (3b-iii)](plans/2026-10-04-owner-id.md) — the third part of plan 3b (5 tasks): the owner from the first start, so `up`'s host paired before setup has one; `owner_id` on the older tables; every query of the operator, host registry and sessions store naming it, with a test that reads every statement. Amended after the security review of 2026-10-01; executed 2026-10-01 (see its "Execution status").
   - [Passkeys (3c)](plans/2026-10-05-passkeys.md) — plan 3c (6 tasks): `webauthn-rs` with the relying party from `public_url`, the `passkeys` table, ceremonies in memory, registration from a stepped-up session, login and step-up by passkey with the counter checked, and `admin reset-public-url` removing the passkeys of a host name it leaves. `admin reset-password` removes every passkey. The API only; the frontend is plan 4's. Amended after the security review of 2026-10-01; executed 2026-10-01 (see its "Execution status").
   - [Kernel hats (5a)](plans/2026-10-06-hats.md) — the first part of plan 5, hats (3 tasks): `hats` and `hat_path_rules` with `owner_id` and composite foreign keys, a default hat on every host from the first start (setup names it), the pure resolver (segment match, longest prefix, default fallback), and the hats, path-rules and `PATCH /api/hosts/{id}` API behind step-up. No session carries a hat yet: `resolve_path` (5b), the session's hat on start and resume (5c) and re-assignment (5d) follow. Amended after the security review of 2026-10-02; executed 2026-10-02 (see its "Execution status").
   - [`resolve_path` (5b)](plans/2026-10-07-resolve-path.md) — the second part of plan 5 (3 tasks): the host resolves a typed path (`~`, symlinks, a missing tail kept under its resolved ancestor, a `..` past it refused); `resolve_path`, a probe of its own capability on plan 6c's probe map; `POST /api/hats/resolve`; and rules saved only through their connected host. Amended after the security review of 2026-10-02 (it covered 5b–5d); executed 2026-10-02 and rebased onto 6c (see its "Execution status").
   - [Image attachments (6a)](plans/2026-10-07-images.md) — plan 6a (6 tasks): the collector checks a prompt's content before anything is written, stores its images as content-addressed files (`attachments`, `event_attachments`) referenced from the turn and its `user_turn`, serves them at `GET /api/attachments/{sha256}`, and reports their size at `GET /api/settings/attachments`; the host announces `images`, reads 32 MiB frames, and refuses images to an agent that takes none. The API only. Amended after the security review of 2026-10-02; executed 2026-10-02 (see its "Execution status").
   - [Session list and search (6b)](plans/2026-10-07-session-list.md) — plan 6b (7 tasks, two PRs): the `title` and `commands` extracts (a load's replay included, an early title only filling an empty one), commands in the catalogue and `catalog_changed` as the catalogue stands, fixed-width stamps and recency moved only by listed events, `GET /api/sessions` (keyset pages, a search over title, cwd, branch and id across every lifecycle, `hat` refused until hats), a list item under 1 KiB whatever the agent reports, the detail built from it with the model and mode; then the `git_state` body and the host's git probe, off the actor and bounded to 3 s (6b-ii). Amended after two security reviews of 2026-10-02; executed 2026-10-02 as PRs #46 and #48 (see its "Execution status").
   - [Release build (7a)](plans/2026-10-07-release-build.md) — the first part of plan 7, distribution (4 tasks): OpenSSL built into release binaries (`vendored-openssl`), `hennery collector healthcheck`, cargo-dist's archives and installers built and checked in CI (`build.yml`; the installer hardened to refuse what it cannot check), and the collector image started and checked. Nothing publishes; packaging/README.md lists what publishing needs. 7b (managed runtime), 7c (services), 7d (doctor) and 7e (Nix) follow. Amended after the security review of 2026-10-02; executed 2026-10-02 (see its "Execution status").
   - [`close_range` (7a-ii)](plans/2026-10-10-close-range.md) — one task: on Linux 5.11 and later, an adapter's spawn marks every inherited descriptor close-on-exec with one `close_range`, past the closing loop's 65536 cap, with the loop as the fallback; CI raises its hard descriptor limit so the test can hold one past the cap. Handed over by the debt lane. Amended after the security review of 2026-10-02; executed 2026-10-02 (see its "Execution status").
   - [Services, supervisor and host lock (7c)](plans/2026-10-09-services.md) — the third part of plan 7, distribution (4 tasks): `host.lock` and `up.lock`; `up` restarting a crashed child with a backoff (1 s to 60 s) and giving it up after ten crashes in five minutes, with `<data>/supervisor.json` for `status` and `doctor`; `hennery service install|uninstall|status` with the login shell's PATH, a launchd user agent or a systemd user unit, linger advised and WSL explained. Amended after the security review of 2026-10-01; executed 2026-10-02 as PR #42 and its stacked follow-up (see its "Execution status").
   - [Project picker (6c)](plans/2026-10-07-projects.md) — plan 6c (6 tasks): the probe map shared with hats' `resolve_path`, workspace roots in `host.toml` or `--workspace-root`, `list_projects` and `browse_directory` behind the browse fence, `GET /api/hosts/{id}/projects` and `…/browse` with the 60 s cache, and recents per hat, shown for the hat a path resolves to. Amended after the security review of 2026-10-01 and, for hats 5a, of 2026-10-02; executed 2026-10-02 as PRs #41 and B (see its "Execution status").
   - [Nix package (7e-i)](plans/2026-10-12-nix-package.md) — 2 tasks: `nix build` gives `hennery` (crane, on nixpkgs' own toolchain and OpenSSL), and `nix flake check` runs the package, clippy, rustfmt and cargo-audit against a pinned advisory database, on Linux and macOS in a new CI job on every PR. The adapter and frontend derivations and the NixOS modules are 7e-ii's. Amended after the security review of 2026-10-02; executed 2026-10-02 (see its "Execution status").
   - [Managed runtime and pinned adapters (7b)](plans/2026-10-08-managed-runtime.md) — plan 7b (7 tasks, two PRs): the adapter manifest (`adapters/manifest.json`, compiled in) and its generator `hennery-pins`, the pin-bump job, the installer (resumable verified downloads, safe extraction, sets with `current`/`previous`, rollback hold, collection), `hennery host adapters update|rollback`, `join` installing the runtime, `host run`'s default agents from the installed set, and `--use-cli`. Pins proposed, operator to confirm. Amended after the security review of 2026-10-01 and the plan review of 2026-10-02; executed 2026-10-02 as PR #51 and its stacked follow-up (see its "Execution status").

## Maintainer decisions (2026-09-27)

Every open design decision is resolved; the specs carry the details.

| # | Question | Decision | Where |
|---|---|---|---|
| 1 | `master.key` in the OS keystore | No — a 0600 file in v1; a separate OS user or container is the real protection | kernel §10 |
| 2 | Per-hat "isolate agent user config" | Documentation only in v1 | umbrella §8.4, ACP core §15 |
| 3 | Codex sharing a composed `CODEX_HOME` | Stays on the mixed-host fallback until measured | umbrella §8.5, ACP core §6 |
| 4 | Headless macOS hosts | A logged-in user is required in v1 | distribution §6.2, §13 |
| 5 | "Use my own CLI" | Advanced install option (`--use-cli`), not the default | distribution §13 |
| 6a | Attachment size cap | None in v1; disk usage shown in Settings | ACP core §15 |
| 6b | VAPID contact | Not asked at setup; derived from `public_url`, optional in Settings | kernel §6 |
| 6c | Multiple listeners | Supported; browser access stays bound to `public_url` | kernel §7 |
| 6d | Static-token header | Header name plus optional value prefix | gateway §1 |
| 6e | Windowing threshold, vendor token behaviour, `GET` SSE | Measured once the code exists | frontend §15, gateway §13 |

---

_Generated with Claude AI — please review before distribution._
