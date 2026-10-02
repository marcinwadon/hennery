# hennery — client view layer and swappable frontends (subsystem spec)

- **Date:** 2026-10-02
- **Status:** Draft, from a design session with the maintainer on 2026-10-02.
- **Amends:** [architecture spec](2026-09-25-hennery-architecture-design.md)
  §5.2 (who interprets ACP) and the [frontend spec](2026-09-26-frontend-design.md)
  (§1 stack, §4 data layer, §6.1 display fold). Both are edited in the same
  change as this spec.
- **Consumes:** the session store and streams of the
  [ACP core spec](2026-09-26-acp-core-design.md) §9.

hennery will have more than one client: a web UI first, then a terminal UI
written in Rust in this repository, and later an iPhone app written in Swift.
This spec places the interpretation of ACP so that each client only renders,
and it fixes how the first web UI is built: from the predecessor's frontend,
not from scratch.

---

## 1. Decisions

| # | Decision | Decided by |
|---|---|---|
| D1 | The interpretation of ACP for display ("the fold") moves out of the frontend into a Rust crate, `hennery-view`, run by the collector. Clients receive display items. | Maintainer, 2026-10-02 (option A of three) |
| D2 | Raw events stay as they are: stored verbatim, served by the existing events routes and session stream. The view layer only reads them. | Maintainer, 2026-10-02 |
| D3 | An update the fold does not recognise becomes a visible `unrecognised` item that carries the raw JSON. It is never dropped. | Maintainer, 2026-10-02 |
| D4 | The web UI reuses the predecessor's frontend: its stack, components and look. Only its data layer is replaced. | Maintainer, 2026-10-02 |
| D5 | Ported files are treated like every other file in the repository: no per-file licence or provenance headers. | Maintainer, 2026-10-02 |
| D6 | Passkey sign-in uses a "Sign in with passkey" button. Browser autofill (conditional UI, discoverable credentials) is deferred. | Maintainer, 2026-10-02 |

*Rejected:* each client folding ACP itself (three implementations in three
languages that drift apart), and one Rust fold compiled to WebAssembly for the
web and bound to Swift (one implementation, but a WASM toolchain in the web
build and foreign bindings for every client).

**Why the umbrella's rule changes.** Architecture §5.2 rejected a collector
that interprets ACP, because the predecessor's collector silently dropped an
update kind (the plan) for months. That failure was silence, not location: a
client-side fold that ignores an unknown kind fails the same way. D3 and the
golden tests (§7) answer it wherever the fold runs, and running it once keeps
three clients consistent.

---

## 2. Components

```
host ──raw ACP──▶ collector store (raw events, unchanged)
                        │
                 hennery-view: fold(events) → items       (pure, no I/O)
                        │
            view API: /api/view/… + /api/stream/…         (read only)
            ▲                  ▲                    ▲
         web UI          terminal UI (later)    iPhone app (later)
```

- **`hennery-view`** (new crate). A pure function from one session's stored
  events to an ordered list of items, plus an incremental form that applies
  one more event to a folded state. No I/O, no clock, no database. It holds
  the rules of [frontend spec §6.1](2026-09-26-frontend-design.md#61-display-fold),
  which move here unchanged.
- **The view API** (in `hennery-sessions`). Read-only routes and streams that
  serve items and session summaries. Every write (prompt, answer, cancel,
  park, resume, close, config, hat re-assignment, delete) stays on the
  existing routes.
- **Generated types.** Item and summary types are defined in Rust and
  generated as TypeScript (`ts-rs`, as today) and JSON Schema (for Swift and
  any other client). The JSON contract is what makes a client swappable.
- **Clients.** The web UI (§5) renders items. The terminal UI links
  `hennery-view`'s types and uses the same API. The iPhone app uses the API
  and the JSON Schema.

---

## 3. Items

One session is an ordered list of items. Each item has:

- `id` — stable for the life of the session: derived from what ACP gives
  (`toolCallId`, `pending_id`) or from the turn and the item's order in it,
  never from an array index;
- `version` — raised each time the item changes;
- `turn_id` — the turn it belongs to, when it belongs to one;
- `kind` and its fields.

| Kind | What it carries |
|---|---|
| `user_turn` | Content blocks, with image attachments as attachment ids |
| `message` | Merged assistant text (consecutive chunks of one run) |
| `thinking` | Merged thought text |
| `tool_call` | Title, kind, status, input, output (every known output shape, frontend §6.1), fabrication warning flag |
| `plan` | The latest step list |
| `question` | A permission or elicitation: the `pending_id`, the parsed request, `answerable` (from the pending set), delivery state |
| `marker` | A collector or host event shown as a divider: parked, resumed, host restarted, host offline/back, not delivered, conflict, hat re-assigned, config not applied, adapter exited |
| `unrecognised` | The raw JSON of an update the fold does not know, and its update kind |

- A tool call's later updates change the same item; a sparser update never
  erases a field (frontend §6.1).
- `answerable` comes from the server's pending set, never from position
  (frontend F-15).

---

## 4. View API

### 4.1 Session items

- `GET /api/view/sessions/{id}?before_turn=&limit=` returns
  `{items, revision, older: bool}`. Items are paged by whole turns, newest
  first; the first page is the tail of the session. Opening a long session
  never folds or sends all of it at once to the client.
- `GET /api/stream/view/sessions/{id}` is an SSE stream of changes:
  - `item` — an upsert of one whole item (`id`, `version`, the item);
  - `item_removed` — an item id;
  - `resync_required` — the client refetches the first page.

  Each message carries the session's `revision` as its SSE id. A client
  reconnects with `Last-Event-ID` and gets only what changed since. A gap the
  collector can no longer fill answers `resync_required`.
- An unknown session, or one the operator may not see, answers 404 on both
  routes. Neither route loads a session's whole history before answering
  (live smoke test, finding F2).

### 4.2 Session list

- `GET /api/view/sessions?cursor&limit&q&hat&lifecycle` returns session
  summaries: title, agent, host, hat, lifecycle, activity, whether a question
  waits, branch, last activity.
- `GET /api/stream/sessions` is the list stream already named in ACP core §9:
  `session_upsert` with one full summary and `session_removed`. No list
  refetch is ever triggered by an event (frontend F-4).

### 4.3 How folding runs

- The collector folds a session when a client first asks for it, and keeps
  the folded state in memory while any client watches it. Each new event is
  applied incrementally and turned into upserts.
- Nothing new is stored. A better fold applies to old sessions on its next
  load.
- A table that caches folded items is deferred until folding is measured to
  be slow.
- The view API is versioned through its types. A breaking change to an item
  gets a new item kind or a new route, so an older client keeps working.

---

## 5. The web UI

### 5.1 What is reused

The predecessor's frontend: React 19, strict
TypeScript, Vite, Tailwind v4, pnpm, Vitest. Its components and look carry
over:

- the transcript and turns, thinking, tool calls, plan steps;
- permission and elicitation cards, the composer (images, slash commands,
  config bar);
- the sidebar, session rows, the project picker, the mobile drawer layout;
- markdown and code highlighting, the push subscription;
- its MCP screen, as the starting point of the gateway screens.

### 5.2 What changes

- **Data layer:** its fetch and SSE code (`api.ts`, `acp.ts`, `useSessions`,
  `useTimeline`) is replaced by a typed client for the view API. Components
  take items as props; no component parses event JSON.
- **Cards:** actionability from `answerable`, not the last card (F-15); digit
  shortcuts scoped to the focused card, inert in editable targets (F-16).
- **Markdown:** no `rehype-raw`; raw HTML from an agent renders as text
  (frontend §6.4).
- **Build and serving:** a normal multi-file Vite build with hashed assets,
  embedded in the `hennery` binary. The single-file build with inline scripts
  is dropped: the CSP is `script-src 'self'`.
- **Routes:** a small router for real links (`/sessions/<id>` and the routes
  of frontend §2), so a push opens its session (F-19).
- **New screens:** setup, login with the passkey button (D6), the step-up
  dialog, hosts and pairing, hats and path rules, passkeys, settings with push
  devices and policies, and the gateway's connections, OAuth consent and
  credential status.
- **Removed:** its commercially licensed fonts, its client logos and brand
  palettes (replaced by a freely licensed font and neutral tokens); hat
  colours come from each hat's settings on the server. Board, workflows,
  memory, config explorer, changes and quick chat stay out (frontend §13).

### 5.3 Guarding the cascade

Frontend §1 rejected a utility framework with a global reset because of F-1
and F-2. Tailwind v4 returns with D4, so those two failures are guarded
directly:

- the markdown container restores list markers, spacing and code styles
  explicitly, under its own scope;
- Playwright checks computed styles in a real browser (list markers present,
  the mobile controls visible at 390 px) — jsdom cannot see the cascade.

---

## 6. Errors

The same contract for every client:

| Answer | Client behaviour |
|---|---|
| 401 | Go to login, then return to where the operator was |
| 403 `step_up_required` | Step-up dialog (passkey, then password), then retry the request once |
| 409 with a code (`hat_ambiguous`, `cwd_moved`, `hat_mismatch`, `presumed_parked`, `images_unsupported`, `host_offline`, `host_installing`, …) | A plain message per code |
| Stream lost | "Reconnecting…", then resume from the last revision, or one resync |
| `unrecognised` item | A collapsed "unsupported update" block with the raw JSON |

---

## 7. Testing

- **The fold:** golden tests over recorded real sessions of Claude and Codex
  (from the live smoke test of 2026-10-02, redacted), for every pinned adapter
  version. A pin bump that changes shapes fails them. Generated sequences with
  unknown update kinds must produce `unrecognised` items, never fewer items.
- **The view API:**
  - its statements are in the owner audit (`owner_filter.rs`);
  - one owner never sees another's items or summaries;
  - an unknown session answers 404 without loading history;
  - reconnect with `Last-Event-ID` yields exactly the missed upserts;
  - every side-effect line has its revert-probe.
- **The web UI:** the predecessor's component tests, ported to items; frontend
  §12's list; one Playwright run against `up` with the fake adapter, at
  1280 px and 390 px.

---

## 8. Delivery

One pull request each:

| Part | Content | Depends on |
|---|---|---|
| 4a | `hennery-view`, the view API, the list stream, generated types and JSON Schema | — |
| 4b | The web shell ported from the predecessor: branding removed, multi-file build embedded in the binary, router, setup, login, step-up, a typed client against a stub | — (parallel with 4a) |
| 4c | Session list, transcript, cards and composer on items | 4a, 4b |
| 4d | Hosts and pairing, hats and path rules, passkeys, settings and push | 4b |
| 4e | The gateway screens | 4b, plan 8's API merged |
| 4f | A README quickstart: build from source, `hennery up`, setup, pair | 4c |

The terminal UI and the iPhone app are later plans; they need nothing from
the collector beyond 4a.

---

## 9. Open questions

1. **Transcript windowing threshold** (frontend §15) — measured with real long
   sessions in 4c.
2. **History growth:** the live smoke test stored about one event per streamed
   chunk. Whether the host should coalesce chunks before they are stored is a
   question for the ACP core, not this spec; the fold handles either.

---

_Generated with Claude AI — please review before distribution._
