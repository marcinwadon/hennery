# Changelog

## Unreleased

## 0.3.4

- Show the full session ID as selectable text in the session details panel,
  on desktop and mobile (#262).

## 0.3.3

- Fix the managed Claude adapter's body-parser and fast-uri security advisories (#258)
  without changing the adapter or bundled CLI versions. Audit both managed
  adapter lockfiles in pins CI and before release packaging.

- Security: update the Web Push arithmetic stack to p256 0.14 and
  crypto-bigint 0.7.6 (RUSTSEC-2026-0387). Replace web-push-native's older
  dependency with a bounded RFC 8291 adapter and the existing record library.
  Saved VAPID keys and browser subscriptions remain compatible (#257).

- New like this (#245). Start an empty, standalone session using another
  session's last reported host, directory, agent, model, mode and options,
  including from a closed session. Its history and family are not copied;
  the new composer opens empty and focused.
- Unseen activity (#249). A bold title and accent dot show unread agent
  activity during work and after completion, with an Unseen filter. Reading
  the transcript's end in a focused, visible window clears only the activity
  already shown, and synchronizes across devices. Finished results alone no
  longer enter Needs you; that queue is reserved for operator attention.
- Pin sessions to the sidebar (#248). Persistent shortcuts across hats and
  session states, independent of the "Do not park" idle hold.
- Recent keeps its visible positions when switching sessions or receiving
  updates (#246). Closed sessions leave Recent, its shortcuts and the command
  palette's Recent section (#250).
- Clearer Needs you cards and primary "Open to answer" actions for questions
  (#252). "Mark as read" also appears beside the completed transcript result.
  Session rename controls stay aligned on desktop (#244).
- Security: ctutils 0.4.5 includes fixes for non-constant-time mask generation
  and `u128` comparisons (RUSTSEC-2026-0385, #251).
- Repository workflow: private intake and a scoped work queue (#247).
- Database migrations 32–33 run on first start. An older binary cannot open
  the upgraded database: back it up before upgrading if you may roll back.

## 0.3.2

Not published: the final release audit caught RUSTSEC-2026-0387 before any
channel published. The signed tag is retained. Its intended changes ship in 0.3.3.

## 0.3.1

- Results to review (#241). A finished operator-driven turn stays in Needs
  you as "To review" until you read it: Mark as read, or it is read once the
  result's end has been in view in a focused window. A prompt you send
  acknowledges the result shown when you began; delegated children's
  completions never enter the inbox. Queued prompts and wakes that start the
  next turn at once no longer hide the result.
- Security: Node 24.22.1 for ACP adapters (the managed runtime) and in the Nix
  toolchain, fixing CVE-2026-111497 in the bundled Brotli decoder (#242).
- Child sessions in the family panel show what happened ("Start child: Done",
  "Close: Done"); a child its parent closed reads Closed, not
  operator-driven and needing attention (#239).
- Sessions in a project section are indented under its heading (#238).
- Database migration 31 runs on first start. An older binary cannot open the
  upgraded database: back it up before upgrading if you may roll back.

## 0.3.0

- Message queue (#233). Send while a turn is running: messages wait in a
  durable, editable queue and go out in order when the turn ends; a pending
  question or permission is never answered by a queued message. On a parked
  session Send becomes "Resume and send" (keyboard shortcut too), and the
  parked notice sits above the input. An explicit Park holds the queue until
  you resume. Failed messages stay available to copy; deleting a session
  deletes its unsent messages and images.
- Session switching (#230). Recent in the sidebar (up to six visited sessions
  with status and hat colour), Option/Alt+1–5 to open them, backtick for this
  window's previous session, and Recent first in the command palette. The
  selected hat now scopes everything: sessions, Needs you, Recent and counts.
- Sessions overview (#235). Group by Hat or Host, then by Project or Nothing;
  collapsible project sections with path, hosts, running count and sessions
  needing you; families stay together. Sort by Needs you first, Last activity
  or Started (stable). Hosts report a repository root, so worktrees and
  subdirectories group with their repository. The header stays on one line at
  1024 px.
- Runtime catalog cache (#229). New session and Defaults show the last known
  models and options instantly, with Refresh to probe again; Start no longer
  waits for a preview. "Agent default" is the adapter's own default, never a
  previous session's choice. Hat overrides on permission mode are always sent;
  unavailable model/effort overrides fall back with a notice.
- Local OAuth callback (#234). Connect an MCP server's OAuth when the collector
  is remote: pick a local callback and run the shown `hennery oauth-callback`
  command on the browser's computer. Tokens never pass through it.
- The MCP gateway accepts a body-less 202/204 that names a non-JSON content
  type, as some servers send for `notifications/initialized` (#231).
- The project picker's outside-tap choice no longer races the listing (#232).
- Database migrations 28–30 run on first start. An older binary cannot open
  the upgraded database.

## 0.2.2

- Updates replace immediately (#224). A new version takes over at once and
  every open hennery window reloads onto it, including windows still on 0.2.1;
  no button, no waiting for other windows. Saved message drafts survive, and a
  draft whose send was interrupted notes that it may already have been sent.
  Unsent images and other unsaved input may be lost.
- Delegation (#225, #226):
  - Session tools say plainly that a child may use any model, mode and option
    its runtime offers, including modes that never ask for permission;
    permission requests raised by a child go to you.
  - A child inherits the parent's permission mode when none is given: the same
    mode on the same runtime, and Claude `bypassPermissions` ↔ Codex
    `agent-full-access` across runtimes. An explicit mode always wins.
  - Delegation limits are global settings (Settings → Delegation limits): live
    children per parent, child starts and automatic parent wake-ups per message
    you send. Defaults stay 8, 32 and 20; each is 1–64.
- The page and collector versions show quietly in the sidebar footer and at the
  bottom of Settings and More, with a small hint when an older page is open
  (#223).
- New session directory browsing (#227): "Use this directory" stays beside the
  current path; Enter or a tap/click outside chooses the shown directory, and
  Escape closes without choosing. Starting while browsing starts in the shown
  directory.
- Database migration 27 runs on first start. An older binary cannot open the
  upgraded database.

## 0.2.1

- Delegation is strictly on or off per hat (#218). On means a child may use any
  model, mode and option its runtime offers at that moment: a runtime signed in
  later, or models added by an adapter update, work without toggling again.
  Enabling no longer probes hosts, so one signed-out runtime cannot block it.
  - Upgrading drops the execution grants of 0.2.0: hats with Delegation on now
    allow everything their runtimes offer.
  - A runtime that cannot be used fails only that child, with a readable reason
    (for example "codex on laptop: not signed in"). Your own sessions still
    show the login steps.
  - Cheaper and steadier child starts: limits are checked before any probe,
    probes are shared, and queued work settles with a reason instead of waiting.
- "Reload to update" in the update banner (#217). Every open hennery window is
  asked first; a busy or silent window defers it, stored drafts survive, and
  unsent images or answers need a confirmation. All windows reload once onto the
  new version, and old offline caches are removed.
- Typography follows the design system (#219): Roboto for headings, text,
  buttons and labels; Inter for form values and table cells; paths stay
  monospace. Roboto italic is bundled.
- Database migration 26 runs on first start. An older binary cannot open the
  upgraded database.

## 0.2.0

- Redesigned web UI. Light and dark themes, the hat's colour as the accent, and
  self-hosted Inter and Roboto. A new shell with a hat rail and a phone tab
  bar. All sessions grouped by hat, host, project or time, with children under
  their parent. A "Needs you" queue with short previews, dismiss and one-click
  permission answers. A session view with message bubbles, a compact composer
  with a settings chip, and Family / Plan / Details panels. A ⌘K command palette
  and keyboard shortcuts (#202–#214).
- Per-hat session defaults (model, permission mode, effort): edited in a
  Defaults matrix, applied in New session. New session has path autocomplete
  (#208, #212).
- Delegation is one per-hat switch in Settings. Children start with any model
  and mode and cannot start their own children (#210, #212).
- Rename sessions. Untitled sessions get a provisional name from their first
  prompt (#213, #214).
- "Do not park": parents stay awake while their children work, and any session
  can be pinned (#205, #207).
- Reliability:
  - Event streams reconnect after silent connections, and SSE responses ask
    reverse proxies not to buffer (`X-Accel-Buffering: no`) (#211).
  - The requested model is applied after the mode and confirmed. A
    configuration that cannot be confirmed fails with a readable reason
    instead of waiting forever.
  - Catalog previews work without `workspace_roots`.
  - Closed sessions no longer appear as needing you (#213).
- Security: rustls-webpki 0.103.16 and webpki-roots 1.0.10
  (RUSTSEC-2026-0381, RUSTSEC-2026-0382) (#211).
- Database migrations 23–25 run on first start. An older binary cannot open the
  upgraded database.

## 0.1.4

- Managed adapters: Node 24.22.0, claude-agent-acp 0.93.0 and codex-acp 2.2.2
  with bundled Codex 0.160.1. New picker models: Codex gpt-6.1-sol, Claude
  Sonnet 5.5 and Haiku 5.5.
- GHCR images now tagged `:<version>` and `:latest` (#192).
- crates.io trusted publishing through OIDC, with no long-lived token.
- Faster CI and releases (#179–#183).
- Automatic upstream watch (pins-watch) and Dependabot (#184, #185).
- Dependency updates (#186, #187, #189).

## 0.1.3

- First complete release on all four channels: GitHub Releases, Homebrew,
  crates.io and GHCR, following the partial crates.io publication of 0.1.2.
- Verify the whole release once before the crates writer starts, rather than
  at every gate call (#179), so publishing can finish within its deadline.
- Faster qualification and CI through one cached locked install of the frozen
  crate tarballs, weekly unlocked/MSRV checks, parallel tests with nextest and
  fewer redundant Nix builds (#180, #181).

## 0.1.2

- Published on GitHub Releases and Homebrew. On crates.io, only
  `hennery-proto`, `hennery-kernel`, `hennery-view` and `hennery-host` 0.1.2
  were published. The tag's crates writer re-verified the whole release at
  every gate call and exhausted its deadline before publishing the rest.
- Check crate existence through the crates.io version API before uploading.
  Missing downloads return a CDN 403, which must not decide existence. Send an
  identifying User-Agent and wait for API/CDN propagation after upload before
  checking the remote digest and index.

## 0.1.1

- 0.1.0 was tagged but never published: the GHCR SBOM predicate check failed
  before any channel wrote a release. Verify official `actions/attest-sbom`
  output with its exact `https://spdx.dev/Document/v2.3` predicate in both the
  publisher and consumer CLI. The stable release includes the changes below.

## 0.1.0

First stable release, building on the published alpha.3.

- Stable distribution through all four channels: GitHub Releases, Homebrew,
  crates.io and GHCR. Seven crates publish in dependency order from frozen
  tarballs, with registry/index propagation checks before dependent uploads.
- Six GitHub release assets: three native archives (Linux musl x86_64/arm64
  and macOS arm64), the installer, `SHA256SUMS` and one combined SBOM. Licence
  notices inside each archive link exact MPL dependency sources; hennery source
  remains public at the signed tag and GitHub's automatic source downloads.
- Shared packaging builds the UI once and reuses accepted archives for installers,
  Homebrew and images. Linux-only PR checks avoid macOS queues; main retains
  macOS smoke and full macOS/ARM suites run weekly, alongside scheduled audits.
- Simpler release verification binds attested bytes to the maintainer-signed tag,
  exact tagged workflow and source digest. Official attestation actions and the
  standard GitHub publisher retain protected approval, immutable uploads and
  explicit recovery for partial releases, without producing-run/name policies.

## 0.1.0-alpha.3

- Assemble the exact native release material while validating cargo-dist staging
  directories and refusing unexpected files, symlinks and conflicting inputs.
- Rehearse the production release graph and artifact transport on pull requests
  and main, through archive assembly, freezing, native installers and local
  verification, without signed tags, attestation or publication authority.
- Alpha.2 failed assembly before publication and was never published.

## 0.1.0-alpha.2

- Update rustls to fix RUSTSEC-2026-0346, RUSTSEC-2026-0347,
  RUSTSEC-2026-0348 and RUSTSEC-2026-0349.
- Align release qualification with CI's Linux test isolation and file descriptor
  limits, and check fresh Rust and web advisories on pull requests and daily.
- Alpha.1 failed pre-publication qualification and was never published.

## 0.1.0-alpha.1

First alpha for early testers. Expect rough edges and back up your data.

- Self-hosted cockpit for Claude and Codex sessions, with transcripts, tools,
  questions and session controls in the browser.
- Hats group projects, hosts and MCP access; the MCP gateway supports local
  servers, remote connections and OAuth.
- HTTPS/WSS transport and reverse-proxy deployment, phone answers and push
  notifications, and delegation between agent sessions.
- Optional memory plug-in with scoped search, management and import.
- Session deletion and per-hat purge, with host transcript cleanup receipts.
- GitHub-only alpha: Linux musl (x86_64 and arm64) and macOS arm64 archives,
  a verified installer, corresponding source, licence notices and SBOMs.
  Homebrew, crates.io and GHCR are prepared for the stable release.

Generated with Claude AI — please review before distribution.
