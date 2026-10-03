# hennery — frontend (subsystem spec)

- **Date:** 2026-09-26
- **Status:** Draft. Amended 2026-10-01 where the APIs it consumes changed as
  built (plans A to 3c); rendering is still the design, untouched by any plan.
  Amended 2026-10-02 by the [client view spec](2026-10-02-client-view-design.md):
  the display fold (§6.1) runs on the collector, the data layer (§4) consumes
  display items, and the stack (§1) is the predecessor frontend's, reused.
- **Refines:** [architecture spec](2026-09-25-hennery-architecture-design.md) §13;
  consumes the APIs of the [ACP core](2026-09-26-acp-core-design.md) and
  [MCP gateway](2026-09-26-mcp-gateway-design.md) specs.
- **Evidence:** a behaviour catalogue of the predecessor's dashboard and its
  tests. "F-n" marks a predecessor incident or defect (§14). Items the
  catalogue found only by reading code, never reproduced, are marked "(read)".

The frontend is a single React application embedded in the `hennery` binary. It
is mobile-first and installable as a PWA. It renders display items that the
collector's view layer folds from ACP (client view spec); it is one of several
clients of that view API.

---

## 1. Stack

| Concern | Choice | Why |
|---|---|---|
| Language / build | TypeScript (strict), Vite, pnpm | Umbrella §13; the predecessor frontend's build, reused (client view spec D4). |
| UI | React 19 | The predecessor frontend, reused. |
| Routing | A small router for real links (`/sessions/<id>` and the routes of §2) | A push link or a reload restores the view (F-19). |
| Server state | A typed client for the view API: snapshot pages plus SSE upserts into a keyed store (§4) | Items arrive folded; no component parses event JSON. |
| Styling | Tailwind v4 plus the predecessor's CSS custom properties, with the guards of client view spec §5.3 | Reuse of the predecessor's look. F-1 and F-2 are guarded by a scoped markdown container and Playwright computed-style checks. |
| hennery types | TypeScript generated from Rust (`ts-rs`), including the view items | Umbrella §5.4. |
| ACP types | Not needed for rendering: the view layer folds ACP | Client view spec D1. |
| Markdown | `react-markdown` + `remark-gfm` → `remark-breaks` → `rehype-sanitize` → `rehype-highlight {ignoreMissing}` | Order is load-bearing; raw HTML is not parsed (§6.4). |
| Tests | Vitest + Testing Library; Playwright (desktop and mobile viewports) | §12. |

*Amended 2026-10-02:* this table first chose CSS Modules and rejected a
utility-CSS framework with a global reset, after F-1 and F-2. The maintainer
chose to reuse the predecessor frontend (client view spec D4); those two
failures are now guarded directly (client view spec §5.3).

---

## 2. Routes and views

| Route | View | Shown in `gateway` mode |
|---|---|---|
| `/setup` (token in the URL fragment, kernel spec §3.1) | Owner account creation (one-time link) | yes |
| `/login` | Password or passkey | yes |
| `/sessions` | Session list | no |
| `/sessions/:id` | Session | no |
| `/new` | New session | no |
| `/hosts` | Hosts: pair, rename, revoke, versions, doctor, online state | no |
| `/mcp` | Connections, OAuth, mounts (clients in `gateway` mode) | yes |
| `/hats` | Hats, themes, default hat per host, path rules, purge | no |
| `/settings` | Account, passkeys, push devices, `public_url`, per-hat push policy | yes |

- `GET /api/capabilities` tells the app which views exist; routes not offered
  are not rendered. One app, two modes.
- **Layout:** mobile-first. Under 768 px: one column, bottom tab bar
  (Sessions, New, Hosts/MCP, Settings), session view full screen with a back
  control. From 768 px: list and session side by side. **One component tree for
  both**, never a parallel mobile header. *(F-3: the predecessor had two
  headers with different feature sets and a back button that never rendered.)*
- Safe areas: `viewport-fit=cover`, `env(safe-area-inset-*)` padding, `dvh`
  units for full-height layouts (iOS `100vh` ignores the collapsing browser
  chrome and pushes a bottom composer off screen).

---

## 3. Auth screens

- **Setup:** the one-time link opens a form for the owner's password and
  `public_url` (pre-filled with `location.origin`); the token is read from
  `location.hash`, removed from the address bar and sent in the
  `POST /api/setup` body. It then offers to register a passkey: setup's
  session counts as stepped up, so registration needs no second password
  within 5 minutes.
- **Login:** a "Sign in with passkey" button first, password second. Browser
  autofill (conditional UI) is deferred (client view spec D6).
  The passkey button is hidden on 409 `no_passkeys` or `passkeys_unavailable`
  (a `public_url` at an IP address); a 429 honours `Retry-After`.
- A 401 from any API call routes to `/login` with the current URL as the return
  target.
- **Step-up:** a 403 `step_up_required` (kernel spec §3.4) opens a small
  dialog (passkey first, password second) and, on success, retries the
  original request once. Nothing else is lost; forms keep their input.

---

## 4. Data layer

### 4.1 Snapshots plus deltas

- **List:** `GET /api/sessions` (paginated) seeds a keyed store; the list SSE
  stream (`session_upsert`, `session_removed`) updates entries in place. **No
  list refetch is ever triggered by an event.** *(F-4: the predecessor
  refetched the full list on every SSE event, including every streamed message
  chunk, from every open tab; with ~600 sessions the list payload reached
  5.6 MB before catalogues were moved out.)*
- **Session:** opening a session fetches its detail and the first page of its
  **items** (`GET /api/view/sessions/{id}`, the newest turns), then opens the
  item stream from that page's `revision` and applies upserts by item id
  (client view spec §4.1). Scrolling up loads older pages with `before_turn`.
  No per-event refetch, and no fold in the browser.
- **Catalogue** (config options, commands, plan): fetched on open, refreshed on
  `catalog_changed`. Never shown for the wrong session while loading.
- **Resume:** both streams reconnect with `Last-Event-ID`. The UI shows a
  compact "reconnecting…" / "resynced" state. On `resync_required` the affected
  snapshot is refetched and the store replaced. *(F-5: the predecessor sent no
  ids and missed events silently after reconnects.)*
- **Connections:** at most two EventSources per tab (list + open session).
  Cross-tab sharing (a `BroadcastChannel` leader) is deferred; HTTP/2 via the
  supported TLS topologies removes the six-connections-per-host limit.
- Expensive derived fetches (git state, diffs in later features) are triggered
  by turn end, never per event. *(F-6 (read): a diff view refetched a host-side
  `git diff` per streamed chunk.)*

### 4.2 Keys

List items, transcript items and cards are keyed by stable ids
(`session_id`, `event_id`, ACP `toolCallId`, `pending_id`), never by array
index, so card-local state (a half-filled form) cannot move to another item.

---

## 5. Session list

- **One sort key:** `last_event_at` descending. Host and status are row fields,
  not sort keys. *(F-7: the predecessor sorted by machine, then staleness, then
  status, then recency; no position on screen meant "recent".)*
- **Day headers:** Today / Yesterday / This week / This month / Earlier,
  computed from **calendar-local midnights** (a rolling 24 h window mislabels
  "Today" at 00:30). Day deltas use `Math.round` because DST makes adjacent
  midnights 23 or 25 hours apart. *(F-8.)* Empty and unparseable timestamps go
  to Earlier, future ones to Today. Headers are sticky with their own
  background and are real headings for screen readers.
- **Row:** status marker, agent mark, title, relative time; line 2: branch
  (fallback: project directory name, never repeated when it equals the title),
  host, and a "resume" affordance for parked sessions.
- **Status display** reads the two axes directly: `blocked` gets the strongest
  marker ("needs you"); `running` animates; `parked` shows "resume"; `failed`
  shows the reason on hover/tap. There is no staleness heuristic. *(F-9: the
  predecessor's single status enum plus sessions that never reconciled needed
  `isStale`/`isActionable` patches; counts claimed 17 sessions needed attention
  when 2 were live.)*
- **Search** runs on the server (`q`) over title, cwd, branch and id across
  **all** sessions; while a query is active, the volume filters are bypassed and
  only the hat applies. *(F-10: filtering before querying made closed threads
  unfindable.)* Transcript full-text search is out of scope for v1.
- **Filters:** "Hide closed" (default on, persisted), lifecycle filter, hat.
  Parked sessions are never hidden by "Hide closed".
- **Hat scope:** hat is server data (`session.hat_id`); the app never computes
  it. The hat selector scopes the list and counts. An explicit selection (a
  click, a just-started session, a push link) is honoured even outside the
  current hat; switching hats clears the selection; desktop auto-select picks
  the most recent **visible** row. *(F-11: scoping the selected-session lookup
  stranded the operator on an empty screen after starting a session outside the
  hat.)*
- **Counts and badges** (tab title, header): number of `blocked` sessions.

---

## 6. Session view

### 6.1 Display fold

A pure function, `fold(events) → items`, over ACP payloads from the session
stream. *Amended 2026-10-02:* it lives in the collector's view layer
(`hennery-view`, client view spec §3), not in the frontend; the rules below
move there unchanged. It is the riskiest code in the product and is specified
and tested as such:

- Consecutive `agent_message_chunk` / `agent_thought_chunk` of the same kind
  merge into one turn, keeping the first chunk's timestamp; any other item
  breaks the run.
- `tool_call` / `tool_call_update` merge by `toolCallId` at the first-seen
  position; a sparser later update never erases a title, input or output.
  An empty `rawInput` object means "no input yet".
- **Tool output extraction** accepts every known shape: `rawOutput` as a
  string, `rawOutput` as `[{type, text}]`, `content[]` text blocks, and legacy
  `_meta` tool responses. *(F-12: a schema change between adapter versions made
  shell output render empty.)*
- User turns render from the collector's `user_turn` events, including stored
  image attachments.
- `plan` updates replace the step list (latest snapshot).
- Non-ACP session bodies and collector events render as dividers:
  `adapter_exited` (stderr excerpt behind a disclosure), `transcript_gap`,
  `turn_ended{interrupted}` and `turn_ended_synthesized` ("the turn was
  interrupted"), `turn_not_delivered` ("not delivered", with "send again"),
  `start_not_delivered`, `host_restarted`, `presumed_parked` / `reattached`
  ("host offline" / "host back"), `session_parked` with its reason,
  `host_note` (including `config_failed` / `reapply_failed`: a switch on start
  or resume that did not take), `conflict`, and operator actions (rename, hat
  re-assignment). Question events (`pending_*`, `answer_*`) drive the cards
  (§6.3), not dividers.
- **Fabrication warning:** tool output that contains tool-invocation syntax
  (closed tags only: `<tool_use>`, not `<tool_use`, so a legitimate
  `<tool_use_error>` does not trip it) gets a visible warning. *(F-13: a
  sub-agent without tools role-played tool calls and invented results,
  including fake commits.)*
- Unknown update kinds become `unrecognised` items carrying their raw JSON,
  rendered as a collapsed "unsupported update" block; they never disappear.
- Fixtures: captured adapter output for every pinned adapter version (Claude
  and Codex), table-driven. A pin bump that changes shapes fails these tests.

### 6.2 Header and plan

Title (click or tap to rename), agent, host, branch, lifecycle/activity,
context usage, and the plan's step list (collapsible on mobile). Speaker labels
come from the session's agent, never hardcoded. *(F-14 (read): the
predecessor labelled Codex sessions "Claude".)*

### 6.3 Cards

**Actionability comes from the collector's pending set** (`pending_changed`
on the session stream), not from position in the transcript. Cards and
answers are keyed by `pending_id`. *(F-15: "last card wins" assumed one
request at a time.)*

Permission card:

- Options in the adapter's order; reject kinds styled as destructive.
- **Keyboard shortcuts** (1–9) work only when the card itself has focus, never
  from an editable target, and only on desktop. *(F-16 (read): a window-level
  listener meant typing "1" in the composer approved option 1.)*

Elicitation card:

- Supported shapes: single select (`oneOf` of `const` + description), multi
  select (array of `anyOf` options), text. **Anything else makes the form
  unanswerable: decline/cancel only.** The card never invents a value.
- A field and its paired free-text `…_custom` field are mutually exclusive:
  choosing an option clears the text and typing text clears the option (the
  adapter gives the custom text precedence on the wire). Whitespace does not
  count as an answer. Send appears only once one real answer exists.
- Each field's hint (the question text in multi-question forms) is rendered.

Both cards, delivery states:

| State | Shown |
|---|---|
| pending, live | buttons / form |
| answer queued, no verdict yet | "Sent" (plus "delivers when the host reconnects" while the host is offline) |
| `delivered` | "Answered" |
| verdict `delivered: false` | "Sent, but the agent was no longer waiting" |
| `cancelled(reason)` | "The agent stopped waiting (<reason>)" plus "Answer as a new message" |
| `cancelled(reason)` with `delivered: true` | "Answered" (the agent took the answer before it stopped waiting) |
| a permission with no option ids | no option buttons; "This question cannot be answered here: stop, park or close the session" |
| 409 `already_answered` | "Already answered from another device" (the card then follows the other answer's verdict) |

The verdict fold is monotonic: `delivered` sticks even if a later
`delivered: false` arrives, or the question is cancelled after it (ACP core
§4.6). Reasons are shown in words, including `agent_withdrew` ("the agent
withdrew the question") and `host_revoked`. "Answer as a new message" resumes the session if
needed and sends "You asked: …. My answer: …" as a new turn, labelled as such.

### 6.4 Rendering

- Markdown pipeline as in §1. **Raw HTML in agent output is not parsed** (no
  `rehype-raw`): it renders as literal text. The page's Content-Security-Policy
  (kernel spec §7.2) blocks inline script regardless.
- Links open in a new tab with `rel="noopener noreferrer"`; list markers are
  styled explicitly and a Playwright check asserts they are visible.
- Unknown code fence languages never throw (`ignoreMissing`).
- **Every transcript item and card sits inside its own error boundary**; one
  bad item renders "could not render this item" and the rest of the transcript
  survives.
- Thinking is collapsed by default. Tool calls are one line (name, summary,
  status), expandable to pretty-printed input and plain output.

### 6.5 Composer

- Text area, ⌘/Ctrl+Enter sends, Enter inserts a newline (mobile: a Send
  button).
- **Images:** paste, drop or pick; png/jpeg/gif/webp ≤ 5 MiB each, ≤ 20 and
  ≤ 16 MiB in total per prompt; hidden when the host lacks the `images`
  capability.
  Each attachment inserts an `[Image #N]` marker at the cursor (all markers of
  one action spliced at once). The text is the source of truth: only images
  whose marker is still present are sent. Content is sent as ordered ACP
  ContentBlocks (text runs and images). Object URLs are revoked on remove, send
  and unmount.
- **Slash commands** from the catalogue: menu while the text starts with `/`
  and has no space; arrow keys, Enter to pick, Escape to dismiss.
- **Config bar:** one switcher per catalogue axis (model, mode, then others in
  a stable order); optimistic, then replaced by the `SessionCatalog` the 202
  returns (what the agent reports, which may differ from the pick, e.g. a mode
  clamped by a model switch); rolled back on 409, 400 or 502 `config_failed`.
- Send is disabled while a turn is in flight (`running`/`blocked`); a Cancel
  control replaces it. Cancel answers with the turn's real outcome
  (`CancelResponse`): a turn that finished first shows as completed, not
  cancelled; `no_open_turn` / `not_running` mean nothing was left to stop.
- A 409 `not_attached` (parked, closed or failed) keeps the draft and offers
  "Resume and send"; a 409 `host_offline` (active, but its host is not
  reachable yet) keeps the draft and waits for the host. A 503 "delivery unknown" keeps the draft
  and shows that the outcome will be known when the host reconnects; a turn
  later reported `not_delivered` offers to send its content again.
- **Per-session state is keyed by session id**: draft text, attachments,
  scroll anchor and rename draft. Drafts persist per session in
  `sessionStorage`. *(F-17 (read): the predecessor's composer kept its draft
  across a session switch and would send it to the newly selected session.)*

### 6.6 Parked, failed, closed

- Parked or closed: a footer with "Resume" (and "host offline" when presumed),
  and "Park" in the header menu for active sessions on hosts with the `park`
  capability. A resume refused with `hat_mismatch` names both hats and links to
  hat re-assignment (sessions with no running adapter, with a warning).
- Starting: a spinner. With the host offline, "Close" closes at once; on a
  reachable host Close waits for the start to settle (409 `starting`).
- Failed: the reason and, for `agent_has_no_record`, "Start a new session in
  this project"; for `agent_not_logged_in`, the host's login instructions.
- "Delete session" in the header menu (confirmation plus step-up).

---

## 7. New session

One form, one request (`POST /api/sessions`):

- **Host:** online hosts first; offline hosts listed but disabled. Agent
  availability and auth state from the host (`auth: missing` disables the agent
  with instructions; `unknown` allows it).
- **Project:** recents (filtered by the hat that the path resolves to on that
  host), enumerated repositories, and a directory browser. Ranking: exact,
  prefix, substring, subsequence, stable within a rank; colliding directory
  names shown as `parent/name`. Input starting with `/` or `~` is a literal
  path.
- **Resolved hat preview:** the form shows which hat the chosen path resolves
  to (server-side resolution), before the session starts.
- **Agent** and **config axes** from the host's agent catalogue
  (`GET /api/hosts/{id}/agents`; the profile's static defaults until the first
  session on that host refines them).
- **MCP note:** when the host falls back to default-hat mounts for the chosen
  agent and hat (umbrella §8.5), the form says the session will run without
  gateway MCP servers.
- **First prompt** (optional), same composer rules as §6.5.
- Changing the host resets agent, axes, path and browse state; a silently
  downgraded choice shows a notice.

---

## 8. Hosts, MCP, Hats, Settings

- **Hosts:** "Add host" (step-up) shows a one-time code and the exact command
  (`hennery host join <public_url> <code>`), with a countdown. Revoked hosts
  stay listed, as revoked. Host names are rendered as escaped text. The list shows
  online state, versions (host, adapters), agent availability, last doctor
  result, rename and revoke (confirmation plus step-up; the dialog says the
  host's running agents stop only when it next connects). Notices: "restart
  needed" after a binary upgrade, "adapter set differs from this release's
  pin", outbox over its bound, another connection refused.
- **MCP:** connection list with status and the connected account
  (`account_label`); add/edit form per `cred_kind` (static token write-only;
  pre-registered client id/secret; the redirect URI to register at the vendor;
  an "internal network" switch with a warning), step-up on the listed edits;
  **Connect** opens the consent popup blank inside the click handler, sets its
  `opener` to `null` and navigates it to the (`https`) consent URL after the
  authorize call returns, keeping the URL visible as a link if the popup is
  blocked *(F-18: passing `noopener` to `window.open` makes it return `null`,
  so every attempt looked blocked)*; a grid of **all hosts** per connection
  (mounts saved as a full set), marking hosts where an agent falls back to
  default-hat mounts; per (host, hat), the local stdio servers (command, args,
  env; step-up); a persistent note that changes apply to new and resumed
  sessions. Vendor error text renders as text.
- **Hats:** create, rename, theme (colour; logo uploaded as SVG or PNG,
  sanitised by the server and always rendered as `<img>`), default hat per
  host, path rules per host with a live "this path resolves to" tester, and
  "Purge hat" (lists what will be deleted; confirmation plus step-up).
- **Settings:** account and passkeys (label, created, last used; registering
  and removing need step-up), signed-in devices (`GET /api/auth/sessions`;
  revoke with step-up; `user_agent` rendered as escaped, bidi-isolated text),
  push devices (subscribe/unsubscribe per device), `public_url` (with a warning
  that OAuth registrations must be redone, and passkeys too if the host name
  changes; that every device signs out, this one included; and that a wrong
  value locks every browser out until `hennery admin reset-public-url` is run
  on the collector's machine. The new value is shown as an origin and typed
  twice, sent only when it changed, behind step-up; after the 200 the page
  goes to the new origin to sign in, kernel spec §3.2), optional owner
  contact for push (kernel spec
  §6), attachment store disk usage (ACP core §15), per-hat push policy (mute,
  include details, generic title), and the deployment warning when the collector shares its OS
  user with agents while holding credentials for several hats (kernel spec
  §10; `deployment_warning` in `GET /api/settings`, which the page shows as
  advice, with the spec's recommendation).

**Theme:** the selected hat's colours are applied as custom properties on
`<html>` before first render (from a small inline script reading the persisted
choice), so there is no flash of the default palette. This is the only inline
script; its hash is allowed by the Content-Security-Policy (kernel spec §7.2).

---

## 9. PWA and push

- Manifest: standalone, theme colour, 192/512 PNG, maskable icon,
  `apple-touch-icon`.
- **Service worker:** caches the app shell (so the installed app opens
  offline and shows "reconnecting"), never API responses. On `push`, shows the
  notification (`tag` = session id). On `notificationclick`, focuses an
  existing window **and navigates it** to the payload's `url`
  (`/sessions/<id>`), or opens one. *(F-19 (read): the predecessor always opened
  `/` and ignored the URL when a window existed.)*
- New service worker versions activate on next launch with an "update
  available" hint; no silent reload mid-edit.
- Subscription: service worker ready → `GET /api/push/vapid` → subscribe
  (`userVisibleOnly`) → `POST /api/push/subscriptions`. The Settings view and
  a first-run hint explain that on iOS push works only from the installed
  home-screen app.

---

## 10. Accessibility

- Day headers are headings; status markers have text alternatives; the agent
  mark uses `aria-label`; card options have their bare value as accessible name.
- All actions reachable by keyboard; focus moves to a newly opened pending card
  only when the composer is empty (never steal typing).
- Touch targets ≥ 44 px on mobile; card options stack vertically on narrow
  screens.

---

## 11. Performance budgets

- Session list item as served: < 1 KiB (enforced server-side, ACP core §8).
- Initial JS (gzip): < 350 KiB; highlight.js languages loaded lazily beyond a
  common set.
- The session list is windowed; the transcript is rendered incrementally and
  windowed once it exceeds a threshold (stable keys make this safe).

---

## 12. Testing

- **Fold:** table-driven over captured fixtures per pinned adapter version;
  every tool-output shape; merge semantics; unknown kinds visible.
- **Cards:** every elicitation shape incl. unsupported; mutual exclusion;
  delivery states incl. queued and `already_answered`; monotonic verdict;
  shortcuts inert in editable targets.
- **Rendering:** raw HTML in agent output appears as text; no inline script
  anywhere except the hashed theme bootstrap.
- **Step-up:** a `step_up_required` response prompts and retries once.
- **List:** day buckets across DST; search bypasses filters; parked never
  hidden; selection rules across hat switches.
- **Composer:** marker/attachment consistency; per-session draft isolation.
- **Data layer:** delta application, resync on `resync_required`, no refetch on
  events (spy on fetch); timeline opens at the tail and pages with `before=`.
- **Playwright** (desktop 1280 px and mobile 390 px): setup → login → pair host
  (with a test host) → start session against the fake adapter → answer a
  permission → receive a push (Chromium) → resume after host restart. Visual
  snapshots of the list, a transcript with markdown lists and code, both cards.
- **Installed-PWA checklist** per release (iOS home-screen push, safe areas),
  because no automated suite covers it.

---

## 13. Out of scope

Board view, workflows, memory, config explorer, changes/diff view, quick chat
(a preset in New session may come later), transcript full-text search,
cross-tab stream sharing.

---

## 14. Predecessor incidents referenced

| Id | Incident or defect | Rule |
|---|---|---|
| F-1 | Media-query cascade hid a control | §1 |
| F-2 | Global reset removed list markers | §1, §6.4 |
| F-3 | Parallel mobile header, dead back button | §2 |
| F-4 | List refetched per event; payload blew up | §4.1 |
| F-5 | No SSE ids; missed events after reconnect | §4.1 |
| F-6 | Diff refetched per chunk (read) | §4.1 |
| F-7 | Recency was the fourth sort key | §5 |
| F-8 | DST mislabelled days | §5 |
| F-9 | Staleness heuristics over a lying status | §5 |
| F-10 | Filters applied before search | §5 |
| F-11 | Hat filter stranded an explicit selection | §5 |
| F-12 | Tool output shape drift rendered empty | §6.1 |
| F-13 | Fabricated tool calls | §6.1 |
| F-14 | Runtime-blind speaker labels (read) | §6.2 |
| F-15 | Card actionability by position | §6.3 |
| F-16 | Global digit shortcut answered permissions (read) | §6.3 |
| F-17 | Draft leaked across session switch (read) | §6.5 |
| F-18 | OAuth popup always "blocked" | §8 |
| F-19 | Push opened `/`, not the session (read) | §9 |

## 15. Open questions

1. **Transcript windowing threshold** — measure with real long sessions before
   choosing a library.
