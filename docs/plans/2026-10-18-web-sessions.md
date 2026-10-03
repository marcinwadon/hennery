# The session list, transcript, cards and composer (plan 4c) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** the web UI's sessions, on 4a's display items (client view spec §5, §8 part 4c; frontend spec §4–§7, §10–§12). It comes in two parts, one PR each:
- **Part 1 (PR 4c-i, Tasks 1–5):**
  - the data layer: a typed view client, the SSE helper's deferred fixes, and item and session-list stores fed by their streams;
  - the session list: day buckets, rows, filters, search, hats and the waiting count;
  - the read-only session view: the header, a transcript of every item kind, safe Markdown, and a window on long transcripts;
  - push links: `/sessions/<id>` restores its view.
- **Part 2 (PR 4c-ii, Tasks 6–10):** the composer, cards (answering questions), New Session, footers and the header menu, and the full Playwright flow. Part 2's tasks are appended to this plan in PR 4c-ii; Part 1 is complete without them (see "Scope").

**Architecture:**
- **Data** (`web/src/api/view.ts`, `web/src/store/*`):
  - `view.ts` holds the view routes and the stream paths, over 4b's `Client`;
  - `items.ts` is a pure reducer of one session's items; `useSessionItems` feeds it from the first page, older pages and the item stream;
  - `sessionList.ts` is a pure reducer of the summaries; `useSessionList` feeds it from pages and the list stream;
  - `sse.ts` (4b's) gets the fixes 4b deferred to 4c.
- **The list** (`components/SessionList.tsx`, `SessionRow.tsx`, `SessionScope.tsx`, `HatSwitch.tsx`; `lib/status.ts`, `hats.ts`, `agent.ts`): in the rail's `.rail-scroll` from 768 px, the whole `/sessions` screen below it. `SessionScope` holds the hat, the filters, the query and the selection for the list and the badge.
- **The view** (`screens/Session.tsx`; `components/Transcript.tsx`, `SessionHeader.tsx`, `StepList.tsx`, `Markdown.tsx`, `ItemBoundary.tsx`, `items/*`): `/sessions/:id`, side by side with the list on a desktop, full screen with a back control on a phone.
- **Tests:** Vitest against stubs (`test-server.ts`, `test-stream.ts`); a gated Chromium measurement of the transcript's cost (`e2e/windowing.spec.ts`).

**Tech Stack:** 4b's (React 19.3.0, TypeScript 7.0.2, Vite 8.3.1, Tailwind 4.3.3, Vitest 5.0.3, jsdom 30.1.1, `@playwright/test` 1.63.0 with the flake's Chromium). New, pinned exactly in `web/package.json` (Task 4): `react-markdown` 10.1.0, `remark-gfm` 4.0.1, `remark-breaks` 4.0.0, `rehype-sanitize` 6.0.0, `rehype-highlight` 7.0.2. No `rehype-raw`; no package with a build script.

**Spec:**
- the [client view spec](../specs/2026-10-02-client-view-design.md): D1–D6, §3 (items), §4 (the view API), §5 (the web UI), §6 (errors), §8's part 4c and §9's open question 1 (windowing);
- the [frontend spec](../specs/2026-09-26-frontend-design.md): §4 (data), §5 (the list), §6 (the session view), §7 (New Session), §10 (accessibility), §11 (budgets), §12 (testing), and the F-findings it cites;
- the [ACP core spec](../specs/2026-09-26-acp-core-design.md) §4.6 and §9 (the routes Part 2 calls).

It builds on [plan 4a-i](2026-10-16-view-fold.md) (the items), [plan 4a-ii](2026-10-17-view-api.md) (the view API and the list stream) and [plan 4b](2026-10-16-web-shell.md) (the shell): their "After this plan" lists what 4c must do (see "Where the hand-offs land"). The code was built on `main` at `b046e28` plus plan 4a-ii's five commits (the view API, not yet merged; the branch rebases onto it when it merges), where every anchor was taken.

**Status:** written 2026-10-02. Part 1 (Tasks 1–5) built, replayed and executed. The security review on the maintainer's behalf (a stronger model, opus) approved it after amendments A1–A4; a scoped re-confirmation (a fresh opus subagent, 2026-10-03) found A1–A3 and O1–O5 in the code, A4 superseded by the server's count, and re-confirmed after three more amendments, all taken: R1 (a refused search keeps the waiting count; the search box takes at most 200 characters), R2 (consecutive resyncs back off) and R3 (the hats and hosts are read again after `/hats` or `/hosts`). The task reviews and the whole-branch review (opus) followed; every finding was taken (see "Execution status").

**How the code blocks were made and checked:**
- Task 1 is an import commit, like 4b's Task 1 (decision 0): styles and three helpers taken from the predecessor's frontend, described by a manifest, not code blocks. Its patch id (`git show <commit> | git patch-id --stable`) is `54dfd4be7f80f153fcad942e50344e5723a610a2`. Every later task replays onto it.
- The code was built and tested first, then cut into each task's commits: tests first, then code. Every block below was generated from those commits.
- The plan was then replayed from its own text onto the import commit, step by step. After each step the tree matched the matching commit byte for byte, `pnpm-lock.yaml` included (it comes from `pnpm install`).
- Every guard was revert-probed by a script that mutates one line, runs the test named, expects a failure and restores the file byte for byte. The counts are in each task's "Revert-probes" step.

## Execution status

Executed 2026-10-03 with subagent-driven development. The code was built first (one implementer per task), reviewed for security and re-confirmed (above), then cut into the task commits below, and the plan was generated from them and replayed from its own text onto the import: every step matched its commit byte for byte. Each task then had a review by a stronger model (opus), and a whole-branch review (opus) closed the run. The fixes were folded into their tasks' commits, the plan regenerated and replayed again. Commits are named here by subject.

| Task | Commit | Review |
|---|---|---|
| Plan | `docs(plan): plan 4c-i, the session list and the read-only transcript` | — |
| 1 | `feat(web): the transcript, card, composer and New Session styles, and the time, image and highlight helpers` | Approved: the patch id matches; every CSS variable is declared; the DST cases are covered. |
| 2 | `feat(web): a typed view client, an SSE helper that never loses its place, and item and session-list stores fed by their streams` | Approved with minors, all taken: rows tied on `last_event_at` now ordered by id newest first, as the server orders them; a further page never overwrites a stream's row on a tie; a session removed while a further page was on its way never comes back with it. |
| 3 | `feat(web): the session list: buckets, rows, filters, search, hats and the waiting count` | Changes requested, taken: day headings and relative times froze on an idle list (they now move each minute and when the tab is shown again). Minors taken: an exact locator, a refused search's test waits for its alert, an id that needs encoding through selection and a hat switch, the row limit reset by a new query, "Resynced" on the list, "Resume" worded as a label. |
| 4 | `feat(web): a read-only session view: header, transcript of every item kind, safe Markdown`; `test(web): a gated Chromium measurement of the transcript's render and scroll cost` | Changes requested, taken: a header read from the session's detail never said "Waiting on a question" for a question outside a turn. Minors taken: a card's and a marker's reason looked up by own key. |
| 5 | `feat(web): a session's link opens it beside the list, its header fed by the list, its transcript opening at the newest 200 items`; `test(web): give tests a loaded machine's time: 20 s per test, 5 s per wait`; `test(web): guard against loose text locators in the web tests` | Changes requested, taken: the tail window had broken the gated measurement (it now times the window's first render, then every "Load earlier" until all N rows are in). Minors taken: a summary newer than the detail keeps the header; the tests' list stub filters by hat; the guard takes backtick literals and the `$` anchor, and records what it does not scan yet (Playwright's `getByRole(…, { name })`). The subject no longer says the transcript renders "at most" 200 items: the window grows at the end while the reader is there. |
| — | (whole branch) | Approved with minors, all taken: a stale Deferred bullet; a comment on the window; a desktop at `/sessions` with no row to open now says "Pick a session from the list, or start a new one." rather than that the screen comes later; the file table; Part 2's references marked as such. |

**Checks on the final branch** (on `main` at `b046e28` with 4a-ii's five commits): every task commit typechecks and passes Vitest (307, 436, 534, 729, 749, then 769 with the guard); the build (199 kB gzip); the browser checks the guard edited, in Chromium (17 passed); the windowing measurement at N = 500 (both rates). Revert-probes on the final code: every guard of Part 1 has one that fails as it should, but two of the list store's defence lines that no test can reach (see Task 2), from 500 scripted probes plus the amendments' 32, the task reviews' fixes' 23 and the guard's 8. The Rust workspace is untouched; CI runs its checks.

## Scope

The client view spec's part 4c (§8), with the obligations 4a-i, 4a-ii and 4b hand the frontend. **10 tasks in two parts:**

Part 1, PR 4c-i:
1. the import: the transcript, card, composer and New Session styles, and the time, image and highlight helpers;
2. the data layer: the view client, the SSE helper's fixes, the item and list stores;
3. the session list;
4. the read-only session view and Markdown;
5. the list and the view together: push links, the header from the list, the transcript's tail window; two flaky 4b tests hardened; a guard on the tests' text locators.

Part 2, PR 4c-ii (appended to this plan there): 6. the composer; 7. cards; 8. New Session; 9. footers and the header menu; 10. Playwright's full flow and CI.

Two PRs, not one (the spec's §8 has one per part): Part 1 alone gives the first tester build a usable read-only cockpit with push links, and it is reviewable on its own. Part 2 needs Part 1's view to mount into.

**In (Part 1):** the list above, and their tests.

**Out** (see "After this plan"):
- everything that acts on a session (Part 2): answering, sending, cancelling, config, resume, park, close, delete, New Session;
- `catalog_changed` on the item stream (4a-ii): handled when it comes, and the catalogue is fetched on open (decision 4);
- the PWA and its service worker (4d): a notification's link is `/sessions/<id>`, which Task 5 restores;
- hosts, hats and settings screens (4d), the gateway screens (4e).

**Where the hand-offs land:**

| Hand-off | Here |
|---|---|
| 4b: bind streams to items; "Reconnecting…"; a resync refetches, closes and reopens | Decisions 2–4; Task 2 |
| 4b deferred: the SSE helper's abort listener, late `closed`, uncancelled bodies, a throwing handler losing its event, an id-only block; the untested delay reset, NUL id, lone CR | Decision 3; Task 2 |
| 4b: mount the list in `.rail-scroll`; screens replace `Placeholder` | Tasks 3, 4 |
| 4b: the session styles, with the Markdown container restoring list markers (client view §5.3) | Decision 18; Tasks 1, 4 |
| 4b: `saveTheme` with the hat's colours | Decision 11; Task 3 |
| 4a-i O-6: an item of a turn not loaded is ignored | Decision 4; Task 2 |
| 4a-i: ids are opaque; `turn_id` may be `event-<n>`, never parsed; `version` is a change marker | Decision 4; Task 2 |
| 4a-ii O-4: a resync replaces the store and reopens from the new anchor | Decision 4; Task 2 |
| 4a-ii O-5: the list stream sends every session of the owner; filters are the client's | Decision 5; Task 2 |
| 4a-ii O-7: a phone's first page is 8 groups | Decision 4; Task 2 |
| 4a-ii B-3 (N-1): `session_removed` on the item stream | Decision 4; Tasks 2, 4 |
| 4a-ii: `question_waits` (state = open) | Decision 8; Task 3 |
| 4a-ii: the server's waiting count (`SummaryPage.waiting`, `waiting_changed`, the list stream's `?hat=`), and a `session_removed` that may come twice | Decisions 5, 8; Tasks 2, 3 |
| frontend F-4, F-7–F-11, F-14, F-19 | Decisions 5–11, 14; Tasks 2–5 |
| client view §9 OQ1: the windowing threshold | Decision 20; Tasks 4, 5 |
| 4b deferred: `inert` behind dialogs, a failed sign-out, `MESSAGES` own keys | A failed sign-out and own keys: done by plan 4d-i (#109). `inert` only partly: the step-up dialog makes the page inert, 4d-i's `ConfirmDialog` does not. Part 2 reuses `ConfirmDialog` and `SignOut`, and amends brief item 36 instead (Part 2's decision 36, PR 4c-ii; `inert` behind every `ConfirmDialog` is a follow-up for 4d) |

## Decisions this plan makes where the specs are silent

Two reviews by a stronger model (opus) on the maintainer's behalf shaped these: the security review of Part 1 (2026-10-02), which approved after amendments A1–A4 and ruled on decisions 8, 11 and 20, and its scoped re-confirmation (2026-10-03), which added R1–R3. Each amendment is marked where it lands.

Decisions are numbered as the build brief numbered them, so a code comment's "plan 4c decision N" names one of these. Part 1 holds 0–20; Part 2 adds 21–46.

0. **Task 1 imports; it does not embed (as 4b's decision 1).**
   - **Choice:** one commit brings the predecessor's transcript, card, composer and New Session styles and three small helpers (`time.ts`, `image.ts`, `highlight.ts`, with their tests) into `web/`, scrubbed. A manifest describes it; it is not retyped here.
   - **Why:** the CSS is 215 lines of look the maintainer chose to keep (D4); retyping it adds length, not safety. The import's diff in the PR is the reviewable surface, and its patch id pins it.
   - **Licence:** the operator confirmed sole ownership of the predecessor and its publication under AGPL-3.0 (2026-10-02, recorded in 4b). No per-file header (D5).
1. **The view client** (`api/view.ts`): `itemPage`, `summaries`, `undeliveredTurn`, `catalog`, `sessionDetail`, the item stream's path and `listStreamPath(hat)` (the hat only when set: the server refuses an empty `hat=`). Every id in a path is `encodeURIComponent`ed; empty filters are not sent. `anchorOf(page)` is `<epoch>:<revision>`, and is otherwise opaque.
2. **Streams** use 4b's `openStream` with `lastEventId` = the page's anchor.
3. **The SSE helper's fixes** (4b deferred them to 4c):
   - a handler that throws stops reading that connection, keeps the id of the last event handled whole, and calls `onResync`, so the event that threw is never lost;
   - consumers never throw: they parse in try/catch, and resync on a malformed message;
   - each connection removes its abort listener; `close()` during a wait reports `closed` at once, and the default wait ends with it;
   - an answer's body that is not read is cancelled;
   - an id-only block keeps its id; an empty id clears it; an id with NUL is ignored; a lone CR at the end of a chunk ends a line; the wait goes back to 1 s on each open;
   - **a resync closes and reopens** (the brief's choice between a `reset(lastEventId)` and close-and-reopen): the helper gets no new method.
4. **The item store** (`store/items.ts`, `useSessionItems`):
   - the first page gives the items in order, `older`, and the anchor;
   - an upsert replaces an item in place by id, only if its `version` is at least the one held;
   - a new id joins its group (`turn_id`, or `start` before the first turn) at the group's end when that group is loaded; it starts a new group at the end when it is newer than every loaded group (nothing older left, or its `ts` is at least the first loaded item's); otherwise it is an item of a turn not loaded, and is ignored (4a-i O-6). The reducer never throws;
   - an older page (`before_turn` = the first loaded item's `turn_id`, only while `older`) is prepended, skipping ids held;
   - **a resync** (`resync_required`, a malformed message) closes the stream at once, refetches the first page and the catalogue, REPLACES the store, and reopens from the new anchor; "Resynced" shows for 3 s. A resync or a removal starts a new run, so a load still in flight can neither open a second stream nor reopen a removed session (amended: A1);
   - **consecutive resyncs back off** (amended at re-confirmation, R2): the first is immediate, then 1, 2, 4, 8, 16 and 30 s, and the count goes back to zero once a stream has stayed open 30 s without one. Neither "the stream opened" nor "a well-formed message came" can be the reset: a resume burst opens the stream and may carry good messages before the bad one, so a server that always sends a malformed message would otherwise loop at full speed. Closing, unmounting or switching the hat during a wait cancels it. The list store shares the same backoff;
   - **removal:** `session_removed`, or a 404 from the page or the stream, closes the stream and marks the session removed ("This session was deleted");
   - **the catalogue** is fetched on open and replaced by `catalog_changed` (when 4a-ii sends it) or by Part 2's config answer; there is no refetch on idle;
   - a phone's first page asks for 8 groups (4a-ii O-7); a desktop's takes the server's default.
5. **The list store** (`store/sessionList.ts`, `useSessionList`):
   - the first page seeds it and gives the stream's anchor; further pages come by `next_cursor`;
   - the list stream's `session_upsert` and `session_removed` are applied; **no event ever fetches** (F-4);
   - `all` holds every summary received, and the rows shown are `all` filtered on the client by the hat, the lifecycle filter and Hide closed (4a-ii O-5: the stream sends every session of the owner);
   - **during a search** the rows are the server's results: an upsert updates a row shown and never inserts one;
   - a resync refetches the first page with the current query and reopens;
   - **the server's waiting count:** the first page's `waiting` (a whole number from 0; a first page with any other `waiting` is unreadable, as a malformed page is), then each `waiting_changed {count}` (anything but such a count is a malformed message: a resync). A further page never changes it. The stream is opened with the query's hat (`/api/stream/sessions?hat=`, never an empty `hat=`), which scopes only the count; a new hat is a new query, so a new page and a new stream;
   - `session_removed` is a keyed delete, so the second of a repeated one changes nothing;
   - rows are ordered by `last_event_at`, newest first, ties by id (F-7).
6. **Day buckets:** Today, Yesterday, This week, This month, Earlier, from calendar-local midnights with `Math.round` across a DST change (F-8); empty or unparseable is Earlier, a future time Today. The headings are real `<h2>`s, sticky. The tests run in `Europe/Warsaw` (`vite.config.ts`), so a 23 h and a 25 h day are both tested on every machine.
7. **A row:** status marker, agent, title (else the directory's name, else the id), relative time; then the branch (else the directory, never repeating the title) and the host. Parked rows show "Resume"; Part 2 acts on it. A failed row shows its reason on hover and on a tap. The status reads lifecycle × activity as they are, with no staleness heuristic (F-9).
8. **The badge** (the lane asked this plan's review to decide the wording): a session with `activity = blocked` or `question_waits` gets the strongest marker and the words **"Waiting on a question"**, never "needs you": a summary cannot tell an answer in flight. The count (the tab title `(N) hennery`, the rail's and the top bar's badge) is the server's: 4a-ii's `SummaryPage.waiting` and `waiting_changed`, the sessions in the owner's and hat's scope that are `blocked` or hold an open question, whatever the search, the lifecycle filter or the page. While a new query's first page loads, the hat's last server count holds, never another hat's. **The fallback**, for a page without `waiting`, counts the loaded rows (`waitingCount`), and keeps the hat's unfiltered count through a search or a lifecycle filter (amended: A4).
9. **Search** runs on the server across every lifecycle; while it is active, Hide closed and the lifecycle filter are set aside, and only the hat applies (F-10). The box takes at most 200 characters, the server's limit. A refused first page (a search the server rejects, a pasted tab) keeps the hat's last count on the badge and the tab title, never 0 (amended at re-confirmation, R1).
10. **Filters:** Hide closed (on by default, kept as `hennery.hideClosed`) never hides a parked session; a lifecycle filter; the hat selector from `GET /api/hats`. The hats and the hosts' names are read on mount and again whenever the route leaves `/hats` or `/hosts`, where plan 4d-i's screens change them; the list itself is never refetched (amended at re-confirmation, R3).
11. **Selection (F-11):** the address's session (`/sessions/<id>`: a click, a push link, a reload) is the selection, honoured outside the hat and outside the loaded rows. A hat switch always clears it, even when the session is in the new hat. A desktop at `/sessions` with no id opens the newest visible row. The hat is kept as `hennery.hat`; its colour goes to `saveTheme` when it is `#rrggbb`.
12. **The list renders 50 rows at a time;** "Load more" shows held rows first and fetches the next page only once every row held is shown. The list is not windowed, although frontend §11 says it is: 50 rows a page keep it light, and real windowing waits for a measurement that needs it (a follow-up).
13. **The route** `/sessions/:id`: list and session side by side from 768 px, the session full screen with a back control below it. One tree (F-3).
14. **The header:** title, agent, host, branch, status in words, model, mode and the newest plan's steps, from the list's summary when it holds the session (following its upserts), else `GET /api/sessions/{id}` once. Speakers are the session's agent: `claude` "Claude", `codex` "Codex", anything else as given; the user is "You" (F-14).
15. **The transcript** opens at its end and follows it while the reader is there. "Load earlier" and reaching the top load earlier turns, keeping the reader's place.
16. **Each item kind** has a renderer:
    - `user_turn`: text exactly as sent (`pre-wrap`, no Markdown); images from `/api/attachments/<sha256>` only for PNG, JPEG, GIF and WebP, and only for a hash of 64 lowercase hex digits (O5);
    - `message`: Markdown; `thinking`: collapsed, Markdown inside;
    - `tool_call`: one line, expandable to its input (pretty-printed, or as given when cut), output and content blocks (text; diff as text; images as `data:` URLs of the four types only, else a note; terminal; other as raw text), its locations, a loud "Possible fabricated tool call" banner quoting the fabrication as text (F-13), rendered outside the item's error boundary so no render failure hides it (amended: A2, O2), and a note with a "Raw events" link when cut. Its status words are looked up by own key only (A2: a status `__proto__` threw);
    - `plan`: the step list; `question`: a read-only card in Part 1 (its request and its state);
    - `marker`: a divider in plain words for each of the 18 kinds, the server's reason codes in words, `text` in a `<details>`, and hat names for a reassignment;
    - `unrecognised`: a collapsed "Unsupported update (<kind>)" with its raw JSON as text.
17. **An error boundary per item,** keyed by id and reset by a new version: "Could not render this item". A row renders again only when its item changes. The header and the list read one status rule (`lib/status.ts`; O3).
18. **Markdown** (`components/Markdown.tsx`): `react-markdown` with `remark-gfm`, `remark-breaks`, a plugin that turns every raw HTML node into text (there is no `rehype-raw`, and nothing is dropped), `rehype-sanitize` with the GitHub schema, and `rehype-highlight` (`detect: false`; highlight.js' common languages; an unknown language stays plain, never throws).
    - Links open in a new tab with `noopener noreferrer` (a footnote's stays on the page); react-markdown's default `urlTransform` stays, so a `javascript:` link has no href at all.
    - An image never loads: it is a link with its alt text.
    - **The schema's `clobberPrefix` is empty (amended: A3):** remark-rehype prefixes the footnotes' ids with `user-content-`, and a second prefix breaks every footnote link. One id is not prefixed: the footnotes section's heading, `footnote-label`. The empty prefix is safe only because raw HTML never becomes elements, so Markdown can make no other id and no `name`: a test renders hostile input and checks every id and name, and a guard test keeps `rehype-raw` and `hast-util-raw` out of `package.json` and the lockfile. Two messages with `[^1]` share ids (a footnote link then jumps to the first).
    - The containers (`.bubble`, `.think-body`) restore list markers, spacing and code styles under Tailwind's reset (client view §5.3).
19. **The bundle:** the initial JS stays under 350 KiB gzip (§11). Measured after Task 5: 199 kB gzip (647 kB raw), one chunk.
20. **Windowing** (client view §9 OQ1): measured in Chromium (Task 4's `e2e/windowing.spec.ts`), then a **tail window** (Task 5): the transcript opens with at most the newest 200 items, and grows at the end while the reader stays there (trimming the top is not built; see "Deferred"); "Load earlier" first reveals 200 more held ones, and fetches an older page only when none is held. See "The windowing measurement" below.

**The windowing measurement** (Task 4; method and numbers in its spec's header):
- `page.route` serves the built UI and a synthetic session: the golden fixtures' 66 items repeated to N = 200, 500, 1000, 2000 and 5000, all in the first page. Chromium from the flake, at 1280×800, at desktop speed and at CPU ×4 (a phone's proxy). First render: from the page's answer to two frames after the Nth row is in the DOM. Scrolling: 240 frames of 400 px.
- The thresholds were set before measuring: a first render within 1 s (desktop) or 3 s (CPU ×4); scrolling p95 within 25 ms (desktop) or 50 ms (×4).
- The shared machine never got quiet (a load of 34–74 on 18 cores), so the numbers are upper bounds: two passes, desktop first render 338/1595 ms at N=200 and 2849/2383 ms at N=500; ×4 1952/1355 ms at 200 and 17.6/6.8 s at 500. Scrolling cost no more than standing still up to 2000 items.
- **Threshold:** between 200 and 500 items under that load. A first page plus three older pages reaches about 280 items at the fixtures' density (3.5 items a turn) on a desktop and 110 on a phone; real tool-heavy turns are denser, and one turn may hold up to 2000 items before the server elides. The threshold is therefore not safely above the reach, and the tail window bounds the first render at 200 items whatever the density.
- Since Task 5 the spec measures the windowed first render (the newest 200 rows and "Load earlier"; it asserts both, so a change of the window fails it), then the reveal: "Load earlier" clicked until none is left, each click timed, and all N rows asserted at the end; then scrolling over all N. Taken again at N = 500 (a load of 14 on 18 cores): first render 132 ms (×4: 961 ms), the whole reveal 298 ms in 2 clicks (×4: 1326 ms). The scroll numbers of that run are not valid by the spec's own rule (the browser held about 7.5 frames a second standing still).
- The spec stays in the tree, skipped unless `HENNERY_WINDOWING=1`, to take the numbers again on a quiet machine.

## Global Constraints

- After every task, in `web/`: `pnpm typecheck`, `pnpm test` and `pnpm build` pass (`nix develop -c sh -c 'cd web && pnpm …'` from the repository root). The Rust workspace is untouched by Part 1: its checks pass as on the base.
- pnpm only: no npm or yarn, and no global install. Every tool comes from the flake. Never `playwright install`.
- **Regenerating:** `pnpm-lock.yaml` changes only in Task 4, by `pnpm install`. CI runs `--frozen-lockfile`.
- **New dependencies** are pinned exactly, from packages published at least one release before the newest (pnpm 12's minimum release age), with no build scripts (`pnpm install` reports none ignored).
- **Safety of text:** no `dangerouslySetInnerHTML`, `innerHTML` or the other sinks `security.test.ts` forbids. Every server or agent string is React text; ids and errors are in `<bdi>`.
- **Storage keys** start with `hennery.`.
- **Repository hygiene** (public repository), before every commit:
  - the contract's two greps, over tracked text and over `web/` including binary files, find none of the predecessor's name, the two company names, the commercial font's name, or home-directory paths, in this plan's diff;
  - test fixtures use `/srv/work/…` or `~/work/…`; no one's initials stand for the user;
  - commit subjects say what the code does.

## Review Focus

1. **Agent text is text** (decisions 16, 18).
   - Expected: no item renders a server or agent string as HTML; raw HTML in Markdown is its characters; a `javascript:` link has no href; an image never loads from Markdown; tool images are `data:` URLs of four types only.
   - Tests: Task 4 `Markdown.test.tsx`, `items.test.tsx`; `security.test.ts`.
2. **A stream never loses its place** (decisions 3, 4).
   - Expected: a throwing handler resyncs and loses nothing; a resync replaces the store and reopens from the new anchor; an item of a turn not loaded is ignored; a version never goes back.
   - Tests: Task 2 `sse.test.ts`, `items.test.ts`, `useSessionItems.test.ts`.
3. **No event fetches** (decision 5, F-4).
   - Expected: list events update the store only; a search never gains rows from the stream.
   - Tests: Task 2 `useSessionList.test.ts` ("no event ever fetches"); Task 3 `SessionList.test.tsx`.
4. **Isolation per session** (decisions 4, 13).
   - Expected: switching sessions remounts the view (`key`), and nothing of one session's stream reaches another's store.
   - Tests: Task 4 `Session.test.tsx`; Task 5.
5. **The badge's words** (decision 8).
   - Expected: "Waiting on a question" for blocked or `question_waits`; never "needs you".
   - Tests: Task 3 `status.test.ts`, `SessionList.test.tsx`.
6. **Push links** (decision 11, F-19).
   - Expected: `/sessions/<id>` shows that session outside the hat and outside the loaded rows, and auto-select never overrides it.
   - Tests: Task 5.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `web/src/index.css`, `web/src/lib/{time,image,highlight}.ts` and tests, `web/vite.config.ts` | The import | 1 |
| `web/src/api/{view,sse}.ts`, `web/src/store/*`, `web/src/test-stream.ts`, tests | The data layer | 2 |
| `web/src/components/{SessionList,SessionRow,SessionScope,HatSwitch,Shell}.tsx`, `web/src/lib/{status,hats,agent}.ts`, `web/src/hooks/useNow.ts`, `index.css`, tests; `web/src/screens/Hosts.test.tsx` (the list's mount read of `GET /api/hosts`) | The list | 3 |
| `web/package.json`, `pnpm-lock.yaml`, `web/src/screens/Session.tsx`, `web/src/components/{Transcript,SessionHeader,StepList,Markdown,ItemBoundary}.tsx`, `components/items/*`, `web/src/api/names.ts`, `index.css`, tests; `web/e2e/windowing.spec.ts` | The view | 4 |
| `web/src/components/{Shell,SessionLink.test}.tsx`, `web/src/screens/Session.tsx`, `web/src/store/useSessionItems.ts`, tests; `web/vite.config.ts`, `web/src/test-setup.ts`; `web/src/test/{textLocators,text-locators.test}.ts`, `web/e2e/{manage,shell}.spec.ts` | Links, the header, the window; the tests' time; the locator guard | 5 |

All commands run from the repository root inside the dev shell (`nix develop -c …`). Work on a feature branch off `main` (`plan/frontend-4c`).

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block, and a final newline.
- "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point, as whole lines (earlier blocks of the same task already applied, in order). Then comes "with:" and its replacement.
- "Delete `path`." removes the file.

"Run:" lines that regenerate (`pnpm install`) change only the files they name. The plan was replayed exactly this way, from its own text.

---

### Task 1: Import the session styles and three helpers

The import commit, `feat(web): the transcript, card, composer and New Session styles, and the time, image and highlight helpers`. It changes 8 files under `web/` (+470 lines, nothing removed) and nothing else. Its patch id is `54dfd4be7f80f153fcad942e50344e5723a610a2` (`git show <commit> | git patch-id --stable`), which a rebase does not change. Execution cherry-picks the reviewed commit from the scratch branch; a replay starts from the PR's first commit. Nothing in it is retyped (decision 0).

**Manifest:** imported, then scrubbed. Paths on the left are under `web/`; on the right, under the predecessor's frontend root.

| Path in `web/` | Lines | From the predecessor's frontend | What was cut or changed |
|---|---|---|---|
| `src/index.css` | 546 (+215) | `src/index.css` (762): the transcript to the elicitation form (lines 224–379), the six `tk-*` tokens the JSON highlighter emits (635–640), the step list (684–696), the New Session form's inner parts (412–440), one mobile `.seg-item` rule (524) and the 400 px rule (548–550) | **Placement:** above 4b's `@media (max-width:767px)` block, so 4b's mobile overrides still win under 768 px; the `.seg-item` rule went into that block, and the 400 px rule follows it. **Tokens renamed** as in 4b (`--accent`, `--accent-wash`, `--accent-grad`, `--sh-accent`, `--ink`); one never-declared `var(--sh-pop, …)` became its literal fallback. **Comments rewritten** where they named cut features (the config viewer, the workflow picker, an ACP update name, an emoji). **Cut:** the board, the config explorer (but the six `tk-*` tokens), the changes tab, workflows, memory, the runtime mark, the mobile drawer, the welcome mark. **Already in 4b** and not copied again: session rows, groups, LEDs, the conversation header, the mobile transcript and composer rules. |
| `src/lib/time.ts` | 73 | the same (73) | None. |
| `src/lib/time.test.ts` | 88 | the same (72) | Added: "6 days ago is still this week" and "29 days ago is still this month" (without them two boundary mutations survived), and "runs in a zone with daylight saving time" (a precondition: the offsets of local midnight on 2026-03-29 and 2026-03-30 differ). |
| `src/lib/image.ts` | 23 | the same (23) | None. |
| `src/lib/image.test.ts` | 33 | the same (23) | Added: each allowed type and an image of exactly the cap are accepted; an SVG is refused. |
| `src/lib/highlight.ts` | 19 | `src/lib/highlight.ts` (47), lines 1–19 | Only `Seg` and `tokJSON`; the config explorer's Markdown tokenizer cut. |
| `src/lib/highlight.test.ts` | 14 | the same (25), lines 1–14 | The `tokJSON` tests only. |
| `vite.config.ts` | 15 (+5) | new | `process.env.TZ = 'Europe/Warsaw'` before `defineConfig`: the test workers inherit it, so the day buckets' DST tests run across a 23 h and a 25 h day on every machine and in CI. |

**Not imported:** the predecessor's project ranking (it reads the old data layer; Part 2's New Session writes its own against `HostProjects`), every component, hook and other helper, and the CSS sections listed as cut.

**Literal colours** kept as they were (the code surfaces, the highlight palette, the fabricated-call warning, the divider): they do not follow the hat. Recorded for 4d's theming.

**The scrub, run on the commit:**
- the contract's grep of tracked text, over this commit's patch and over `web/`: empty (on the whole tree it finds only the base's fictional home-directory fixtures in Rust tests and plans, the same before and after);
- the contract's grep of `web/`, binary files included: empty;
- over the 8 files: ticket-like ids, the old storage-key prefix, `url(`: empty; `@import`: only 4b's `@import "tailwindcss"`;
- every added comment read by hand.

**Variables:** every `var(--…)` in `web/src/index.css` is declared in its `:root`, checked mechanically.

**Revert-probes** (each run; each fails its test):
- `time.ts`: empty or unparseable as Today instead of Earlier; `Math.round` as `Math.floor` (the spring-forward test); `days <= 0` as `< 0` and as `=== 0` (a future time); `days === 1` as `=== 2`; `< 7` as `< 6`; `< 30` as `< 31` and `< 29`;
- `vite.config.ts`: the zone line removed, run under `TZ=UTC` ("runs in a zone with daylight saving time" fails);
- `image.ts`: the allowlist inverted, a type dropped (WebP, JPEG), SVG added; `size > MAX` as `<` and `>=`;
- `highlight.ts`: a key classed as a string.

Checks on the commit: `pnpm install --frozen-lockfile` (no dependency change), then in `web/` `pnpm typecheck`, `pnpm test` (307 tests on `main` with 4a-ii's commits) and `pnpm build` pass.

---

### Task 2: The data layer

The view client, the SSE helper's deferred fixes, and the two stores (decisions 1–5). Nothing renders yet.

**Files:**
- Create: `web/src/api/view.ts`, `web/src/store/items.ts`, `web/src/store/useSessionItems.ts`, `web/src/store/sessionList.ts`, `web/src/store/useSessionList.ts`, `web/src/test-stream.ts` (a scripted stream for the hooks' tests), and their tests.
- Modify: `web/src/api/sse.ts`, `web/src/api/sse.test.ts`.

- [ ] **Step 1: Write the tests**

Test files: `web/src/api/sse.test.ts`, `web/src/api/view.test.ts`, `web/src/store/items.test.ts`, `web/src/store/resyncBackoff.test.ts`, `web/src/store/sessionList.test.ts`, `web/src/store/useSessionItems.test.ts`, `web/src/store/useSessionList.test.ts`, `web/src/test-stream.ts`.

In `web/src/api/sse.test.ts`, replace:

  ```ts
    const open = (lastEventId?: string) =>
      openStream(client, '/api/stream/x', {
        onEvent: (e) => events.push(e),
  ```

with:

  ```ts
    const open = (lastEventId?: string, onEvent?: (e: StreamEvent) => void) =>
      openStream(client, '/api/stream/x', {
        onEvent: onEvent ?? ((e) => events.push(e)),
  ```

In `web/src/api/sse.test.ts`, replace:

  ```ts
    })
  })
  ```

with:

  ```ts
    })

    it('starts each wait at 1 s again once a connection opens', async () => {
      const t = setup([
        () => new Response(null, { status: 503 }),
        () => new Response(null, { status: 503 }),
        () => sse([]),
        () => new Response(null, { status: 503 }),
      ])
      const stream = t.open()
      await vi.waitFor(() => expect(t.sleeps).toHaveLength(4))
      expect(t.sleeps).toEqual([1000, 2000, 1000, 2000])
      stream.close()
    })

    it('ignores an id holding NUL, keeping the last good one', async () => {
      const t = setup([() => sse(['id: 1:2\ndata: a\n\nid: 1:3\u0000x\ndata: b\n\n'], true)])
      const stream = t.open()
      await vi.waitFor(() => expect(t.events).toHaveLength(2))
      expect(t.events[1].id).toBeUndefined()
      expect(stream.lastEventId()).toBe('1:2')
      stream.close()
    })

    it('ends a line at a lone CR, across chunks and at the end of the stream', async () => {
      const t = setup([() => sse(['event: item\rdata: a\r', '\rdata: b\r\r'])])
      const stream = t.open()
      await vi.waitFor(() => expect(t.events).toHaveLength(2))
      expect(t.events[0]).toEqual({ event: 'item', data: 'a', id: undefined })
      expect(t.events[1]).toEqual({ event: 'message', data: 'b', id: undefined })
      stream.close()
    })

    it('keeps the id of a block with no data, which dispatches nothing', async () => {
      const t = setup([() => sse(['id: 2:4\n\n', 'event: item\nid: 2:5\n\n'], true)])
      const stream = t.open('2:1')
      await vi.waitFor(() => expect(stream.lastEventId()).toBe('2:5'))
      expect(t.events).toHaveLength(0)
      stream.close()
    })

    it('an empty id clears it, so no Last-Event-ID is sent', async () => {
      const t = setup([() => sse(['id\n\n']), () => sse([], true)])
      const stream = t.open('2:1')
      await vi.waitFor(() => expect(t.sent).toHaveLength(2))
      expect(t.sent[1]['Last-Event-ID']).toBeUndefined()
      stream.close()
    })

    it('a handler that throws stops the burst, keeps the last good id and asks for a resync', async () => {
      const t = setup([
        () =>
          sse(
            [
              'event: item\ndata: a\nid: 1:6\n\n',
              'event: item\ndata: b\n\nevent: item\ndata: c\n\nevent: item\ndata: d\nid: 1:9\n\n',
            ],
            true,
          ),
        () => sse([], true),
      ])
      const seen: string[] = []
      const stream = t.open(undefined, (e) => {
        seen.push(e.data)
        if (e.data === 'b') throw new Error('boom')
      })
      await vi.waitFor(() => expect(t.onResync).toHaveBeenCalledTimes(1))
      expect(seen).toEqual(['a', 'b'])
      expect(stream.lastEventId()).toBe('1:6')
      // Not closed by the consumer: the reconnect resumes before the event that threw.
      await vi.waitFor(() => expect(t.sent).toHaveLength(2))
      expect(t.sent[1]['Last-Event-ID']).toBe('1:6')
      stream.close()
    })

    it('dispatches nothing more once closed from inside a handler', async () => {
      const t = setup([])
      const seen: string[] = []
      const client = new Client({
        fetch: (async () =>
          sse(['event: session_removed\ndata: {}\n\nevent: item\ndata: x\nid: 1:2\n\n'], true)) as unknown as typeof fetch,
        navigate: vi.fn(),
        here: () => ({ pathname: '/', search: '' }),
        stepUp: async () => {},
      })
      const stream: { s?: ReturnType<typeof openStream> } = {}
      stream.s = openStream(client, '/x', {
        lastEventId: '1:1',
        onEvent: (e) => {
          seen.push(e.event)
          if (e.event === 'session_removed') stream.s?.close()
        },
        onState: (s) => t.states.push(s),
      })
      await vi.waitFor(() => expect(t.states).toContain('closed'))
      await new Promise((r) => setTimeout(r, 10))
      expect(seen).toEqual(['session_removed'])
      expect(stream.s.lastEventId()).toBe('1:1')
    })

    it('reports closed at once when closed during a wait, and connects no more', async () => {
      const answers = [() => new Response(null, { status: 503 })]
      let calls = 0
      const states: StreamState[] = []
      let wake: () => void = () => {}
      const client = new Client({
        fetch: (async () => {
          calls++
          return answers.shift()?.() ?? sse([], true)
        }) as unknown as typeof fetch,
        navigate: vi.fn(),
        here: () => ({ pathname: '/', search: '' }),
        stepUp: async () => {},
      })
      const stream = openStream(client, '/x', {
        onEvent: () => {},
        onState: (s) => states.push(s),
        sleep: () => new Promise<void>((resolve) => (wake = resolve)),
      })
      await vi.waitFor(() => expect(states.at(-1)).toBe('reconnecting'))
      stream.close()
      expect(states.at(-1)).toBe('closed')
      wake()
      await new Promise((r) => setTimeout(r, 10))
      expect(calls).toBe(1)
      expect(states).toEqual(['connecting', 'reconnecting', 'closed'])
    })

    it('the default wait ends when the stream is closed', async () => {
      vi.useFakeTimers()
      try {
        const states: StreamState[] = []
        let calls = 0
        const client = new Client({
          fetch: (async () => {
            calls++
            return new Response(null, { status: 503 })
          }) as unknown as typeof fetch,
          navigate: vi.fn(),
          here: () => ({ pathname: '/', search: '' }),
          stepUp: async () => {},
        })
        const stream = openStream(client, '/x', { onEvent: () => {}, onState: (s) => states.push(s) })
        await vi.waitFor(() => expect(states.at(-1)).toBe('reconnecting'))
        expect(vi.getTimerCount()).toBe(1)
        stream.close()
        // At once: no timer is left to fire.
        expect(vi.getTimerCount()).toBe(0)
        expect(calls).toBe(1)
      } finally {
        vi.useRealTimers()
      }
    })

    it('cancels the body of an answer it does not read', async () => {
      const cancelled = vi.fn()
      const failing = (status: number) =>
        new Response(new ReadableStream({ cancel: cancelled }), { status })
      const t = setup([() => failing(503), () => failing(404)])
      t.open()
      await vi.waitFor(() => expect(t.onError).toHaveBeenCalledWith(404))
      await vi.waitFor(() => expect(cancelled).toHaveBeenCalledTimes(2))
    })

    it('removes its abort listener after each connection', async () => {
      const added: unknown[] = []
      const removed: unknown[] = []
      // The prototype that owns a signal's listener methods (jsdom's or Node's).
      let owner: object = new AbortController().signal
      while (!Object.prototype.hasOwnProperty.call(owner, 'addEventListener')) owner = Object.getPrototypeOf(owner)
      const target = owner as EventTarget
      const realAdd = target.addEventListener
      const realRemove = target.removeEventListener
      const addSpy = vi.spyOn(target, 'addEventListener').mockImplementation(function (
        this: EventTarget,
        type: string,
        listener: EventListenerOrEventListenerObject | null,
        opts?: boolean | AddEventListenerOptions,
      ) {
        if (type === 'abort') added.push(listener)
        return realAdd.call(this, type, listener, opts)
      })
      const removeSpy = vi.spyOn(target, 'removeEventListener').mockImplementation(function (
        this: EventTarget,
        type: string,
        listener: EventListenerOrEventListenerObject | null,
        opts?: boolean | EventListenerOptions,
      ) {
        if (type === 'abort') removed.push(listener)
        return realRemove.call(this, type, listener, opts)
      })
      try {
        const t = setup([() => sse(['data: a\n\n']), () => sse(['data: b\n\n']), () => sse(['data: c\n\n'])])
        const stream = t.open()
        await vi.waitFor(() => expect(t.sleeps).toHaveLength(3))
        stream.close()
        expect(added.length).toBeGreaterThanOrEqual(3)
        expect(added.filter((l) => !removed.includes(l))).toEqual([])
      } finally {
        addSpy.mockRestore()
        removeSpy.mockRestore()
      }
    })

    it('closed while connecting: reports nothing of the answer that comes after', async () => {
      let answer: (r: Response) => void = () => {}
      const states: StreamState[] = []
      const onError = vi.fn()
      const client = new Client({
        fetch: (() => new Promise<Response>((r) => (answer = r))) as unknown as typeof fetch,
        navigate: vi.fn(),
        here: () => ({ pathname: '/', search: '' }),
        stepUp: async () => {},
      })
      const stream = openStream(client, '/x', { onEvent: () => {}, onState: (s) => states.push(s), onError })
      await vi.waitFor(() => expect(states).toEqual(['connecting']))
      stream.close()
      answer(new Response(null, { status: 404 }))
      await new Promise((r) => setTimeout(r, 10))
      expect(states).toEqual(['connecting', 'closed'])
      expect(onError).not.toHaveBeenCalled()
    })
  })
  ```

Create `web/src/api/view.test.ts`:

  ```ts
  import { describe, expect, it } from 'vitest'
  import { json, routed } from '../test-stream'
  import {
    anchorOf,
    catalog,
    itemPage,
    itemStreamPath,
    listStreamPath,
    sessionDetail,
    summaries,
    undeliveredTurn,
  } from './view'

  // Ids that would change a path's meaning if not encoded.
  const ID = 'a/b?c#d'
  const ENC = 'a%2Fb%3Fc%23d'
  const TURN = 't/1%2'

  function server() {
    return routed(() => json({}))
  }

  describe('the view client', () => {
    it('itemPage encodes the id and sends before_turn and limit', async () => {
      const s = server()
      await itemPage(s.client, ID)
      await itemPage(s.client, ID, { beforeTurn: TURN, limit: 8 })
      expect(s.calls.map((c) => c.path)).toEqual([
        `/api/view/sessions/${ENC}`,
        `/api/view/sessions/${ENC}?before_turn=t%2F1%252&limit=8`,
      ])
    })

    it('undeliveredTurn encodes both ids', async () => {
      const s = server()
      await undeliveredTurn(s.client, ID, TURN)
      expect(s.calls[0].path).toBe(`/api/view/sessions/${ENC}/turns/t%2F1%252`)
    })

    it('catalog and sessionDetail encode the id', async () => {
      const s = server()
      await catalog(s.client, ID)
      await sessionDetail(s.client, ID)
      expect(s.calls.map((c) => c.path)).toEqual([`/api/sessions/${ENC}/catalog`, `/api/sessions/${ENC}`])
    })

    it('the item stream path encodes the id', () => {
      expect(itemStreamPath(ID)).toBe(`/api/stream/view/sessions/${ENC}`)
    })

    it('the list stream path sends the hat encoded, and never an empty one', () => {
      expect(listStreamPath('h/1&x=2#y')).toBe('/api/stream/sessions?hat=h%2F1%26x%3D2%23y')
      for (const none of ['', null, undefined]) expect(listStreamPath(none)).toBe('/api/stream/sessions')
    })

    it('summaries sends what is set, and never an empty hat or search', async () => {
      const s = server()
      await summaries(s.client, { hat: '', q: '', lifecycle: [] })
      await summaries(s.client, { cursor: 'c 1', limit: 50, q: 'fix', hat: 'h/1', lifecycle: ['active', 'parked'] })
      expect(s.calls.map((c) => c.path)).toEqual([
        '/api/view/sessions',
        '/api/view/sessions?cursor=c+1&limit=50&q=fix&hat=h%2F1&lifecycle=active%2Cparked',
      ])
    })

    it('anchorOf is <epoch>:<revision>', () => {
      expect(anchorOf({ epoch: 'e1', revision: 42 })).toBe('e1:42')
    })
  })
  ```

Create `web/src/store/items.test.ts`:

  ```ts
  import { describe, expect, it } from 'vitest'
  import type { Item, ItemPage } from '../generated/view'
  import { EMPTY_ITEMS, beforeTurnOf, isItem, itemsReducer, type ItemsState } from './items'

  function message(id: string, turn: string | undefined, version = 1, ts = '2026-10-02T10:00:00.000Z'): Item {
    return { id, version, ts, ...(turn === undefined ? {} : { turn_id: turn }), kind: 'message', text: id } as Item
  }

  function page(items: Item[], older = false, revision = 10): ItemPage {
    return { items, older, epoch: 'e1', revision }
  }

  const ids = (state: ItemsState) => state.items.map((item) => item.id)

  function loaded(items: Item[], older = false): ItemsState {
    return itemsReducer(EMPTY_ITEMS, { type: 'loaded', page: page(items, older) })
  }

  describe('itemsReducer', () => {
    it('takes a first page whole, with its anchor', () => {
      const state = loaded([message('a', 't1'), message('b', 't2')], true)
      expect(ids(state)).toEqual(['a', 'b'])
      expect(state).toMatchObject({ older: true, epoch: 'e1', revision: 10 })
    })

    it('a resync page replaces everything, older pages included', () => {
      let state = loaded([message('c', 't3')], true)
      state = itemsReducer(state, { type: 'prepended', page: page([message('a', 't1')], false) })
      state = itemsReducer(state, {
        type: 'loaded',
        page: { items: [message('x', 't9')], older: true, epoch: 'e2', revision: 99 },
      })
      expect(ids(state)).toEqual(['x'])
      expect(state).toMatchObject({ older: true, epoch: 'e2', revision: 99 })
    })

    it('replaces an item in place by a version at least the one held', () => {
      let state = loaded([message('a', 't1', 5), message('b', 't1', 5)])
      const newer = { ...message('a', 't1', 7), text: 'new' } as Item
      state = itemsReducer(state, { type: 'upsert', item: newer })
      expect(ids(state)).toEqual(['a', 'b'])
      expect(state.items[0]).toBe(newer)
      const same = { ...message('a', 't1', 7), text: 'again' } as Item
      state = itemsReducer(state, { type: 'upsert', item: same })
      expect(state.items[0]).toBe(same)
    })

    it('never goes back to an older version', () => {
      const state = loaded([message('a', 't1', 5)])
      const after = itemsReducer(state, { type: 'upsert', item: message('a', 't1', 4) })
      expect(after).toBe(state)
    })

    it('a new id of a loaded group goes at that group’s end', () => {
      let state = loaded([message('a', 't1'), message('b', 't1'), message('c', 't2')])
      state = itemsReducer(state, { type: 'upsert', item: message('n', 't1') })
      expect(ids(state)).toEqual(['a', 'b', 'n', 'c'])
    })

    it('the start group (no turn) is a group too', () => {
      let state = loaded([message('a', undefined), message('b', 't1')])
      state = itemsReducer(state, { type: 'upsert', item: message('n', undefined) })
      expect(ids(state)).toEqual(['a', 'n', 'b'])
    })

    it('a new group with nothing older left goes at the end', () => {
      let state = loaded([message('a', 't1')], false)
      state = itemsReducer(state, { type: 'upsert', item: message('n', 't2', 1, '2000-01-01T00:00:00.000Z') })
      expect(ids(state)).toEqual(['a', 'n'])
    })

    it('a new group newer than the first loaded item goes at the end, older groups or not', () => {
      let state = loaded([message('a', 't5', 1, '2026-10-02T10:00:00.000Z')], true)
      state = itemsReducer(state, { type: 'upsert', item: message('n', 't6', 1, '2026-10-02T10:00:00.000Z') })
      expect(ids(state)).toEqual(['a', 'n'])
    })

    it('ignores an item of a turn not loaded', () => {
      const state = loaded([message('a', 't5', 1, '2026-10-02T10:00:00.000Z')], true)
      const after = itemsReducer(state, { type: 'upsert', item: message('old', 't1', 1, '2026-10-01T10:00:00.000Z') })
      expect(after).toBe(state)
    })

    it('ignores a new group whose time cannot be read while older groups exist', () => {
      const state = loaded([message('a', 't5')], true)
      const after = itemsReducer(state, { type: 'upsert', item: message('n', 't6', 1, 'not a time') })
      expect(after).toBe(state)
    })

    it('removes an item by id, and ignores an id not held', () => {
      let state = loaded([message('a', 't1'), message('b', 't1')])
      state = itemsReducer(state, { type: 'removed', id: 'a' })
      expect(ids(state)).toEqual(['b'])
      expect(itemsReducer(state, { type: 'removed', id: 'zzz' })).toBe(state)
    })

    it('prepends an older page, skipping ids already held', () => {
      let state = loaded([message('c', 't3'), message('d', 't3')], true)
      state = itemsReducer(state, {
        type: 'prepended',
        page: page([message('a', 't1'), message('b', 't2'), message('c', 't3', 1)], false, 3),
      })
      expect(ids(state)).toEqual(['a', 'b', 'c', 'd'])
      expect(state.older).toBe(false)
      // The anchor is the first page's.
      expect(state.revision).toBe(10)
    })

    it('takes before_turn from the first loaded item, and none at the start', () => {
      expect(beforeTurnOf(loaded([message('a', 't4'), message('b', 't5')], true))).toBe('t4')
      expect(beforeTurnOf(loaded([message('a', undefined), message('b', 't5')], true))).toBeUndefined()
      expect(beforeTurnOf(loaded([message('a', 't4')], false))).toBeUndefined()
      expect(beforeTurnOf(EMPTY_ITEMS)).toBeUndefined()
    })

    it('never crashes on a malformed item or page', () => {
      const state = loaded([message('a', 't1')])
      const bad: unknown[] = [
        null,
        42,
        'item',
        {},
        { id: 'x' },
        { id: 'x', version: '2', ts: 't', kind: 'message' },
        { id: 'x', version: 1, ts: 't', kind: 'message', turn_id: 7 },
        { id: 1, version: 1, ts: 't', kind: 'message' },
        { id: 'x', version: Number.NaN, ts: 't', kind: 'message' },
      ]
      for (const item of bad) {
        expect(itemsReducer(state, { type: 'upsert', item: item as Item })).toBe(state)
      }
      expect(itemsReducer(state, { type: 'loaded', page: { items: 'no' } as unknown as ItemPage })).toBe(state)
      expect(itemsReducer(state, { type: 'prepended', page: null as unknown as ItemPage })).toBe(state)
      expect(itemsReducer(state, { type: 'removed', id: undefined as unknown as string })).toBe(state)
      expect(itemsReducer(state, { type: 'nope' } as unknown as never)).toBe(state)
      // A page's bad entries are dropped, the good ones kept.
      const mixed = itemsReducer(EMPTY_ITEMS, {
        type: 'loaded',
        page: page([null, message('a', 't1'), { id: 'b' }, message('a', 't1')] as unknown as Item[]),
      })
      expect(ids(mixed)).toEqual(['a'])
    })

    it('isItem accepts every kind the server sends', () => {
      for (const kind of ['user_turn', 'message', 'thinking', 'tool_call', 'plan', 'question', 'marker', 'unrecognised']) {
        expect(isItem({ id: 'x', version: 1, ts: 't', kind })).toBe(true)
      }
    })
  })
  ```

Create `web/src/store/resyncBackoff.test.ts`:

  ```ts
  // Resyncs one after another wait longer each time, in both stores
  // (`ResyncBackoff`): a server that keeps sending a message the client
  // refuses cannot make it refetch and reopen in a loop.
  import { act, renderHook } from '@testing-library/react'
  import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
  import type { Item, ItemPage, SessionSummary, SummaryPage } from '../generated/view'
  import { json, liveStream, routed, type LiveStream } from '../test-stream'
  import type { ListFilters } from './sessionList'
  import { SessionItemsController, useSessionItems } from './useSessionItems'
  import { SessionListController, useSessionList } from './useSessionList'

  const LIST = '/api/view/sessions'
  const LIST_STREAM = '/api/stream/sessions'
  const ITEMS = '/api/view/sessions/s1'
  const ITEM_STREAM = '/api/stream/view/sessions/s1'
  const CATALOG = '/api/sessions/s1/catalog'
  // The real waits (1 s, 2 s, 4 s …, at most 30 s) and the real settle time.
  const TIMING = { resyncedMs: 400 }

  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  /** Let fetches, bodies and streams move on, with no time passing. */
  async function flush() {
    for (let i = 0; i < 10; i++) {
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0)
      })
    }
  }

  async function advance(ms: number) {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(ms)
    })
    await flush()
  }

  function summary(id: string): SessionSummary {
    return {
      session_id: id,
      host_id: 'h1',
      agent: 'claude',
      cwd: '/srv/work/app',
      hat_id: 'hat-a',
      lifecycle: 'active',
      presumed_parked: false,
      created_at: '2026-10-01T00:00:00.000Z',
      last_event_at: '2026-10-02T10:00:00.000Z',
      question_waits: false,
    }
  }

  function server(paths: { page: string; stream: string }, page: () => unknown) {
    const streams: LiveStream[] = []
    const t = routed((call) => {
      const url = new URL(call.path, 'http://h')
      if (url.pathname === paths.page) return json(page())
      if (url.pathname === CATALOG) return json({ session_id: 's1', config_options: [], commands: [] })
      if (url.pathname === paths.stream) {
        const live = liveStream()
        streams.push(live)
        return live.response
      }
      return json({ code: 'not_found', message: 'no' }, 404)
    })
    const of = (path: string) => t.calls.filter((c) => new URL(c.path, 'http://h').pathname === path)
    return { ...t, streams, of }
  }

  const listServer = () =>
    server({ page: LIST, stream: LIST_STREAM }, (): SummaryPage => ({ sessions: [summary('a')], epoch: 'e1', revision: 10, waiting: 1 }))

  const BAD_COUNT = 'event: waiting_changed\ndata: {"count":-1}\n\n'

  /** A resume burst: a good upsert first, then a count that does not parse. */
  function badBurst(stream: LiveStream) {
    act(() => {
      stream.event('session_upsert', summary('b'), 'e1:11')
      stream.send(BAD_COUNT)
    })
  }

  describe('the session list’s resyncs', () => {
    it('refetch at once the first time, then wait 1 s, 2 s, 4 s … up to 30 s, even after a good message in the burst', async () => {
      const s = listServer()
      renderHook((f: ListFilters) => useSessionList(f, TIMING), { wrapper: s.wrapper, initialProps: { hideClosed: false } })
      await flush()
      expect(s.of(LIST)).toHaveLength(1)
      expect(s.streams).toHaveLength(1)

      badBurst(s.streams[0])
      await flush()
      expect(s.streams[0].cancelled).toBe(true)
      expect(s.of(LIST)).toHaveLength(2)
      expect(s.streams).toHaveLength(2)

      let pages = 2
      for (const wait of [1000, 2000, 4000, 8000, 16000, 30000, 30000]) {
        badBurst(s.streams[pages - 1])
        await flush()
        expect(s.streams[pages - 1].cancelled).toBe(true)
        await advance(wait - 1)
        expect(s.of(LIST)).toHaveLength(pages)
        expect(s.streams).toHaveLength(pages)
        await advance(1)
        pages++
        expect(s.of(LIST)).toHaveLength(pages)
        expect(s.streams).toHaveLength(pages)
      }
    })

    it('refetch at once again after the stream stayed open 30 s', async () => {
      const s = listServer()
      renderHook((f: ListFilters) => useSessionList(f, TIMING), { wrapper: s.wrapper, initialProps: { hideClosed: false } })
      await flush()
      badBurst(s.streams[0])
      await flush()
      expect(s.streams).toHaveLength(2)
      // Open 29 s: the next resync still follows this one.
      await advance(29_000)
      badBurst(s.streams[1])
      await advance(999)
      expect(s.streams).toHaveLength(2)
      await advance(1)
      expect(s.streams).toHaveLength(3)
      // Open 30 s: the next resync is a first one again.
      await advance(30_000)
      badBurst(s.streams[2])
      await flush()
      expect(s.of(LIST)).toHaveLength(4)
      expect(s.streams).toHaveLength(4)
    })

    it('a wait ends with the hook: nothing is fetched or opened after unmount', async () => {
      const s = listServer()
      const { unmount } = renderHook((f: ListFilters) => useSessionList(f, TIMING), {
        wrapper: s.wrapper,
        initialProps: { hideClosed: false },
      })
      await flush()
      badBurst(s.streams[0])
      await flush()
      badBurst(s.streams[1])
      await flush()
      // Waiting 1 s for the third page.
      unmount()
      await advance(60_000)
      expect(s.of(LIST)).toHaveLength(2)
      expect(s.streams).toHaveLength(2)
      expect(s.streams.every((live) => live.cancelled)).toBe(true)
      expect(vi.getTimerCount()).toBe(0)
    })

    it('a start after a stop begins a new run of resyncs: the first refetches at once', async () => {
      const s = listServer()
      const controller = new SessionListController(s.client, {}, TIMING)
      controller.start()
      await flush()
      badBurst(s.streams[0])
      await flush()
      badBurst(s.streams[1])
      await flush()
      // Waiting 1 s; hidden, then shown again.
      controller.stop()
      controller.start()
      await flush()
      expect(s.of(LIST)).toHaveLength(3)
      badBurst(s.streams[2])
      await flush()
      expect(s.of(LIST)).toHaveLength(4)
      expect(s.streams).toHaveLength(4)
      controller.stop()
    })

    it('a hat switch during a wait drops it: only the new hat’s page and stream come', async () => {
      const s = listServer()
      const { rerender } = renderHook((f: ListFilters) => useSessionList(f, TIMING), {
        wrapper: s.wrapper,
        initialProps: { hat: 'hat-a', hideClosed: false } as ListFilters,
      })
      await flush()
      badBurst(s.streams[0])
      await flush()
      badBurst(s.streams[1])
      await flush()
      rerender({ hat: 'hat-b', hideClosed: false })
      await flush()
      await advance(60_000)
      expect(s.of(LIST).map((c) => c.path)).toEqual([`${LIST}?hat=hat-a`, `${LIST}?hat=hat-a`, `${LIST}?hat=hat-b`])
      expect(s.of(LIST_STREAM).map((c) => c.path)).toEqual([`${LIST_STREAM}?hat=hat-a`, `${LIST_STREAM}?hat=hat-a`, `${LIST_STREAM}?hat=hat-b`])
      expect(s.streams.slice(0, 2).every((live) => live.cancelled)).toBe(true)
      expect(s.streams[2].cancelled).toBe(false)
    })
  })

  function message(id: string): Item {
    return { id, version: 1, ts: '2026-10-02T10:00:00.000Z', turn_id: 't1', kind: 'message', text: id } as Item
  }

  const itemServer = () => server({ page: ITEMS, stream: ITEM_STREAM }, (): ItemPage => ({ items: [message('a')], older: false, epoch: 'e1', revision: 10 }))

  /** A resume burst: a good item first, then one that does not parse. */
  function badItems(stream: LiveStream) {
    act(() => {
      stream.event('item', message('b'), 'e1:11')
      stream.send('event: item\ndata: nope\n\n')
    })
  }

  describe('a session’s resyncs', () => {
    it('refetch at once the first time, then wait 1 s, 2 s, 4 s, even after a good item in the burst', async () => {
      const s = itemServer()
      renderHook(() => useSessionItems('s1', TIMING), { wrapper: s.wrapper })
      await flush()
      expect(s.of(ITEMS)).toHaveLength(1)
      badItems(s.streams[0])
      await flush()
      expect(s.streams[0].cancelled).toBe(true)
      expect(s.of(ITEMS)).toHaveLength(2)
      expect(s.streams).toHaveLength(2)

      let pages = 2
      for (const wait of [1000, 2000, 4000]) {
        badItems(s.streams[pages - 1])
        await flush()
        // Closed at once: the stream is not read through the wait.
        expect(s.streams[pages - 1].cancelled).toBe(true)
        await advance(wait - 1)
        expect(s.of(ITEMS)).toHaveLength(pages)
        expect(s.streams).toHaveLength(pages)
        await advance(1)
        pages++
        expect(s.of(ITEMS)).toHaveLength(pages)
        expect(s.streams).toHaveLength(pages)
      }
    })

    it('refetch at once again after the stream stayed open 30 s', async () => {
      const s = itemServer()
      renderHook(() => useSessionItems('s1', TIMING), { wrapper: s.wrapper })
      await flush()
      badItems(s.streams[0])
      await flush()
      await advance(30_000)
      badItems(s.streams[1])
      await flush()
      expect(s.of(ITEMS)).toHaveLength(3)
      expect(s.streams).toHaveLength(3)
    })

    it('a start after a stop begins a new run of resyncs: the first refetches at once', async () => {
      const s = itemServer()
      const controller = new SessionItemsController(s.client, 's1', () => undefined, TIMING)
      controller.start()
      await flush()
      badItems(s.streams[0])
      await flush()
      badItems(s.streams[1])
      await flush()
      controller.stop()
      controller.start()
      await flush()
      expect(s.of(ITEMS)).toHaveLength(3)
      badItems(s.streams[2])
      await flush()
      expect(s.of(ITEMS)).toHaveLength(4)
      expect(s.streams).toHaveLength(4)
      controller.stop()
    })

    it('a wait ends with the hook: nothing is fetched or opened after unmount', async () => {
      const s = itemServer()
      const { unmount } = renderHook(() => useSessionItems('s1', TIMING), { wrapper: s.wrapper })
      await flush()
      badItems(s.streams[0])
      await flush()
      badItems(s.streams[1])
      await flush()
      unmount()
      await advance(60_000)
      expect(s.of(ITEMS)).toHaveLength(2)
      expect(s.streams).toHaveLength(2)
      expect(s.streams.every((live) => live.cancelled)).toBe(true)
      expect(vi.getTimerCount()).toBe(0)
    })
  })
  ```

Create `web/src/store/sessionList.test.ts`:

  ```ts
  import { describe, expect, it } from 'vitest'
  import type { SessionSummary, SummaryPage } from '../generated/view'
  import {
    EMPTY_LIST,
    compareSummaries,
    isFirstPage,
    isShown,
    listReducer,
    serverQuery,
    shownOf,
    waitingCount,
    type ListFilters,
    type ListState,
  } from './sessionList'

  function summary(id: string, patch: Partial<SessionSummary> = {}): SessionSummary {
    return {
      session_id: id,
      host_id: 'h1',
      agent: 'claude',
      cwd: '/srv/work/app',
      hat_id: 'hat-a',
      lifecycle: 'active',
      presumed_parked: false,
      created_at: '2026-10-01T00:00:00.000Z',
      last_event_at: '2026-10-02T10:00:00.000Z',
      question_waits: false,
      ...patch,
    }
  }

  /** A page; with no `waiting`, a server that does not count (the rows are
   *  counted instead). */
  function page(sessions: SessionSummary[], next?: string, revision = 10, waiting?: number): SummaryPage {
    return { sessions, next_cursor: next, epoch: 'e1', revision, ...(waiting === undefined ? {} : { waiting }) } as SummaryPage
  }

  const NONE: ListFilters = { hideClosed: false }
  const ids = (list: SessionSummary[]) => list.map((s) => s.session_id)

  function first(sessions: SessionSummary[], search = false): ListState {
    return listReducer(EMPTY_LIST, { type: 'first', page: page(sessions, 'c1'), search })
  }

  describe('serverQuery', () => {
    it('sends the hat, and Hide closed as every lifecycle but closed', () => {
      expect(serverQuery({ hat: 'hat-a', hideClosed: true })).toEqual({
        hat: 'hat-a',
        lifecycle: ['starting', 'active', 'parked', 'failed'],
      })
    })

    it('a lifecycle filter alone decides', () => {
      expect(serverQuery({ hideClosed: true, lifecycle: ['closed'] })).toEqual({ hat: undefined, lifecycle: ['closed'] })
    })

    it('a search sends q and the hat only', () => {
      expect(serverQuery({ hat: 'hat-a', hideClosed: true, lifecycle: ['active'], q: '  fix ' })).toEqual({
        q: 'fix',
        hat: 'hat-a',
      })
    })

    it('no hat sends none (the server refuses an empty one)', () => {
      expect(serverQuery({ hat: '', hideClosed: false })).toEqual({ hat: undefined })
      expect(serverQuery({ hat: null, hideClosed: false })).toEqual({ hat: undefined })
    })
  })

  describe('the shown list', () => {
    it('sorts by last_event_at, newest first, then by id, highest first (the server’s order)', () => {
      const state = first([
        summary('a', { last_event_at: '2026-10-02T10:00:00.000Z' }),
        summary('c', { last_event_at: '2026-10-02T11:00:00.000Z' }),
        summary('b', { last_event_at: '2026-10-02T10:00:00.000Z' }),
      ])
      expect(ids(shownOf(state, NONE))).toEqual(['c', 'b', 'a'])
      expect(compareSummaries(summary('a'), summary('a'))).toBe(0)
    })

    it('keeps the server’s order for rows of the same millisecond that come across two pages', () => {
      // The server pages by `last_event_at DESC, id DESC`: `b` on the first
      // page, `a` on the next, both at the same instant.
      const at = '2026-10-02T10:00:00.123Z'
      let state = first([summary('b', { last_event_at: at })])
      state = listReducer(state, { type: 'more', page: page([summary('a', { last_event_at: at })]) })
      expect(ids(shownOf(state, NONE))).toEqual(['b', 'a'])
    })

    it('filters by hat', () => {
      const state = first([summary('a'), summary('b', { hat_id: 'hat-b' })])
      expect(ids(shownOf(state, { hat: 'hat-b', hideClosed: false }))).toEqual(['b'])
    })

    it('Hide closed hides closed sessions and never parked ones', () => {
      const state = first([
        summary('a', { lifecycle: 'closed' }),
        summary('b', { lifecycle: 'parked' }),
        summary('c', { lifecycle: 'failed' }),
      ])
      expect(ids(shownOf(state, { hideClosed: true }))).toEqual(['c', 'b'])
      expect(ids(shownOf(state, { hideClosed: false }))).toEqual(['c', 'b', 'a'])
    })

    it('a lifecycle filter shows only its lifecycles', () => {
      const state = first([summary('a', { lifecycle: 'closed' }), summary('b', { lifecycle: 'parked' })])
      expect(ids(shownOf(state, { hideClosed: true, lifecycle: ['closed'] }))).toEqual(['a'])
    })

    it('a search shows what the server found, in the hat, whatever the lifecycle', () => {
      const state = first([summary('a', { lifecycle: 'closed' }), summary('b', { hat_id: 'hat-b' })], true)
      expect(ids(shownOf(state, { hideClosed: true, lifecycle: ['active'], q: 'x' }))).toEqual(['b', 'a'])
      expect(ids(shownOf(state, { hat: 'hat-a', hideClosed: true, q: 'x' }))).toEqual(['a'])
    })
  })

  describe('listReducer', () => {
    it('an upsert of a new session inserts it', () => {
      let state = first([summary('a')])
      state = listReducer(state, { type: 'upsert', summary: summary('n', { last_event_at: '2026-10-02T12:00:00.000Z' }) })
      expect(ids(shownOf(state, NONE))).toEqual(['n', 'a'])
    })

    it('an upsert updates a session in place, and moves it by its time', () => {
      let state = first([summary('a', { last_event_at: '2026-10-02T09:00:00.000Z' }), summary('b')])
      state = listReducer(state, {
        type: 'upsert',
        summary: summary('a', { title: 'new', last_event_at: '2026-10-02T12:00:00.000Z' }),
      })
      const shown = shownOf(state, NONE)
      expect(ids(shown)).toEqual(['a', 'b'])
      expect(shown[0].title).toBe('new')
    })

    it('an upsert out of the filter takes the row off the shown list, and keeps it in all', () => {
      let state = first([summary('a'), summary('b')])
      state = listReducer(state, { type: 'upsert', summary: summary('a', { lifecycle: 'closed' }) })
      expect(ids(shownOf(state, { hideClosed: true }))).toEqual(['b'])
      expect(state.all.get('a')?.lifecycle).toBe('closed')
    })

    it('while searching, an upsert updates a found row and never adds one', () => {
      let state = first([summary('a')], true)
      state = listReducer(state, { type: 'upsert', summary: summary('a', { title: 't2' }) })
      state = listReducer(state, { type: 'upsert', summary: summary('n') })
      const shown = shownOf(state, { hideClosed: false, q: 'x' })
      expect(ids(shown)).toEqual(['a'])
      expect(shown[0].title).toBe('t2')
      // The header still sees it.
      expect(state.all.has('n')).toBe(true)
    })

    it('session_removed takes a session out of all, the shown list and the search', () => {
      let state = first([summary('a'), summary('b')], true)
      state = listReducer(state, { type: 'removed', id: 'a' })
      expect(state.all.has('a')).toBe(false)
      expect(ids(shownOf(state, { hideClosed: false, q: 'x' }))).toEqual(['b'])
      // An id not held changes no row.
      expect(listReducer(state, { type: 'removed', id: 'zzz' }).all).toBe(state.all)
    })

    it('a further page read before a removal never puts the removed session back', () => {
      // `loadMore` in flight; `x` is removed before its page lands.
      let state = first([summary('a')])
      state = listReducer(state, { type: 'removed', id: 'x' })
      state = listReducer(state, { type: 'more', page: page([summary('x', { last_event_at: '2026-10-01T00:00:00.000Z' })]) })
      expect(state.all.has('x')).toBe(false)
      expect(ids(shownOf(state, NONE))).toEqual(['a'])
    })

    it('a search’s further page never finds a removed session', () => {
      let state = first([summary('a')], true)
      state = listReducer(state, { type: 'removed', id: 'x' })
      state = listReducer(state, { type: 'more', page: page([summary('x')]) })
      expect(state.found?.has('x')).toBe(false)
      expect(ids(shownOf(state, { hideClosed: false, q: 'x' }))).toEqual(['a'])
    })

    it('a first page forgets the removals: the new query’s pages hold what the server says', () => {
      let state = first([summary('a')])
      state = listReducer(state, { type: 'removed', id: 'x' })
      state = listReducer(state, { type: 'first', page: page([summary('a')], 'c2'), search: false })
      state = listReducer(state, { type: 'more', page: page([summary('x', { last_event_at: '2026-10-01T00:00:00.000Z' })]) })
      expect(ids(shownOf(state, NONE))).toEqual(['a', 'x'])
    })

    it('a session_removed sent twice is a keyed delete: the second changes nothing', () => {
      let state = first([summary('a'), summary('b')])
      state = listReducer(state, { type: 'removed', id: 'a' })
      expect(listReducer(state, { type: 'removed', id: 'a' })).toBe(state)
      expect([...state.all.keys()]).toEqual(['b'])
    })

    it('a further page adds rows and keeps the first page’s anchor', () => {
      let state = first([summary('a')])
      state = listReducer(state, {
        type: 'more',
        page: page([summary('b', { last_event_at: '2026-10-01T00:00:00.000Z' })], undefined, 99),
      })
      expect(ids(shownOf(state, NONE))).toEqual(['a', 'b'])
      expect(state.revision).toBe(10)
      expect(state.nextCursor).toBeUndefined()
    })

    it('a further page never puts back an older row than the stream sent', () => {
      let state = first([summary('x')])
      state = listReducer(state, { type: 'upsert', summary: summary('a', { title: 'live', last_event_at: '2026-10-02T12:00:00.000Z' }) })
      state = listReducer(state, { type: 'more', page: page([summary('a', { title: 'old' })]) })
      expect(state.all.get('a')?.title).toBe('live')
    })

    it('a further page never overwrites what the stream sent at the same last_event_at', () => {
      // A change that leaves the time as it was: the stream's copy came after
      // the page was read, so it wins the tie.
      let state = first([summary('x')])
      state = listReducer(state, { type: 'upsert', summary: summary('a', { presumed_parked: true }) })
      state = listReducer(state, { type: 'more', page: page([summary('a', { presumed_parked: false })]) })
      expect(state.all.get('a')?.presumed_parked).toBe(true)
    })

    it('a search’s further page adds to what it found', () => {
      let state = first([summary('a')], true)
      state = listReducer(state, { type: 'more', page: page([summary('b', { last_event_at: '2026-10-01T00:00:00.000Z' })]) })
      expect(ids(shownOf(state, { hideClosed: false, q: 'x' }))).toEqual(['a', 'b'])
    })

    it('a first page replaces everything', () => {
      let state = first([summary('a'), summary('b')])
      state = listReducer(state, { type: 'first', page: page([summary('c')], undefined, 50), search: false })
      expect([...state.all.keys()]).toEqual(['c'])
      expect(state.revision).toBe(50)
    })

    it('never crashes on a malformed summary or page', () => {
      const state = first([summary('a')])
      for (const bad of [null, 1, {}, { session_id: 'x' }, { ...summary('x'), question_waits: 'yes' }]) {
        expect(listReducer(state, { type: 'upsert', summary: bad as SessionSummary })).toBe(state)
      }
      expect(listReducer(state, { type: 'more', page: { sessions: 3 } as unknown as SummaryPage })).toBe(state)
      expect(listReducer(state, { type: 'first', page: null as unknown as SummaryPage, search: false })).toBe(state)
      expect(listReducer(state, { type: 'removed', id: 4 as unknown as string })).toBe(state)
    })
  })

  describe('the server’s waiting count', () => {
    const blocked = [summary('a', { activity: 'blocked' })]

    it('a first page holds its waiting; a later first page replaces it, or clears it when it has none', () => {
      let state = listReducer(EMPTY_LIST, { type: 'first', page: page(blocked, 'c1', 10, 4), search: false })
      expect(state.waiting).toBe(4)
      state = listReducer(state, { type: 'first', page: page(blocked, undefined, 20, 0), search: true })
      expect(state.waiting).toBe(0)
      state = listReducer(state, { type: 'first', page: page(blocked, undefined, 30), search: false })
      expect(state.waiting).toBeUndefined()
    })

    it('a further page never changes it, whatever it carries', () => {
      let state = listReducer(EMPTY_LIST, { type: 'first', page: page(blocked, 'c1', 10, 2), search: false })
      state = listReducer(state, { type: 'waiting', count: 3 })
      state = listReducer(state, { type: 'more', page: page([summary('b', { question_waits: true })], undefined, 15, 9) })
      expect(state.waiting).toBe(3)
      expect(state.all.has('b')).toBe(true)
    })

    it('waiting_changed sets it; a count that is not a whole number from 0 changes nothing', () => {
      let state = listReducer(EMPTY_LIST, { type: 'first', page: page(blocked, undefined, 10, 2), search: false })
      state = listReducer(state, { type: 'waiting', count: 0 })
      expect(state.waiting).toBe(0)
      for (const bad of [-1, 1.5, Number.NaN, '3', null]) {
        expect(listReducer(state, { type: 'waiting', count: bad as number })).toBe(state)
      }
      expect(listReducer(state, { type: 'waiting', count: 0 })).toBe(state)
    })

    it('a first page whose waiting is not a count is unreadable; a further page’s is never read', () => {
      for (const bad of [-1, 2.5, '1', null]) {
        const bogus = { ...page(blocked), waiting: bad } as unknown as SummaryPage
        expect(isFirstPage(bogus)).toBe(false)
        expect(listReducer(EMPTY_LIST, { type: 'first', page: bogus, search: false })).toBe(EMPTY_LIST)
      }
      expect(isFirstPage(page(blocked))).toBe(true)
      expect(isFirstPage(page(blocked, undefined, 10, 0))).toBe(true)
      const state = first([summary('x')])
      const more = listReducer(state, { type: 'more', page: { ...page(blocked), waiting: -1 } as unknown as SummaryPage })
      expect(more.all.has('a')).toBe(true)
    })

    it('waitingCount is the server’s count when it sent one, else the loaded rows’', () => {
      const rows = [summary('a', { activity: 'blocked' }), summary('b', { question_waits: true })]
      const counted = listReducer(EMPTY_LIST, { type: 'first', page: page(rows, undefined, 10, 7), search: false })
      expect(waitingCount(counted, 'hat-a')).toBe(7)
      expect(waitingCount(listReducer(counted, { type: 'waiting', count: 0 }), 'hat-a')).toBe(0)
      expect(waitingCount(first(rows), 'hat-a')).toBe(2)
    })
  })

  describe('waitingCount', () => {
    it('counts the hat’s sessions that are blocked or have a question open', () => {
      const state = first([
        summary('a', { activity: 'blocked' }),
        summary('b', { question_waits: true, lifecycle: 'closed' }),
        summary('c', { activity: 'running' }),
        summary('d', { activity: 'blocked', hat_id: 'hat-b' }),
      ])
      expect(waitingCount(state, 'hat-a')).toBe(2)
      expect(waitingCount(state, null)).toBe(3)
      expect(isShown(state, summary('d', { hat_id: 'hat-b' }), { hat: 'hat-a', hideClosed: false })).toBe(false)
    })
  })
  ```

Create `web/src/store/useSessionItems.test.ts`:

  ```ts
  import { act, renderHook, waitFor } from '@testing-library/react'
  import { afterEach, describe, expect, it, vi } from 'vitest'
  import type { SessionCatalog } from '../generated/protocol'
  import type { Item, ItemPage } from '../generated/view'
  import { json, liveStream, routed, type Call, type LiveStream } from '../test-stream'
  import { useSessionItems } from './useSessionItems'

  // A session id that needs encoding in a path.
  const ID = 's/1 x'
  const PAGE_PATH = '/api/view/sessions/s%2F1%20x'
  const STREAM_PATH = '/api/stream/view/sessions/s%2F1%20x'
  const CATALOG_PATH = '/api/sessions/s%2F1%20x/catalog'

  function message(id: string, turn: string | undefined, version = 1, ts = '2026-10-02T10:00:00.000Z'): Item {
    return { id, version, ts, ...(turn === undefined ? {} : { turn_id: turn }), kind: 'message', text: id } as Item
  }

  function page(items: Item[], revision: number, older = false): ItemPage {
    return { items, older, epoch: 'e1', revision }
  }

  function catalog(model: string): SessionCatalog {
    return { session_id: ID, config_options: [], commands: [], model }
  }

  const FAST = { retryMs: () => 5, resyncedMs: 400 }

  /** A server for one session: pages served in turn (the last repeats), a
   *  catalogue, and a new live stream per connection. */
  function server(pages: (ItemPage | number)[], options: { older?: Record<string, ItemPage | number> } = {}) {
    const streams: LiveStream[] = []
    const catalogs: SessionCatalog[] = [catalog('m1')]
    const answer = (value: ItemPage | number) => (typeof value === 'number' ? json({ code: 'x', message: 'x' }, value) : json(value))
    const t = routed((call: Call) => {
      const url = new URL(call.path, 'http://h')
      if (url.pathname === PAGE_PATH) {
        const before = url.searchParams.get('before_turn')
        if (before !== null) return answer(options.older?.[before] ?? 500)
        return answer(pages.length > 1 ? pages.shift()! : pages[0])
      }
      if (url.pathname === CATALOG_PATH) return json(catalogs.length > 1 ? catalogs.shift()! : catalogs[0])
      if (url.pathname === STREAM_PATH) {
        const live = liveStream()
        streams.push(live)
        return live.response
      }
      return json({ code: 'not_found', message: 'no' }, 404)
    })
    const of = (path: string) => t.calls.filter((c) => new URL(c.path, 'http://h').pathname === path)
    return { ...t, streams, catalogs, of }
  }

  afterEach(() => {
    // @ts-expect-error clear the stub between tests
    delete window.matchMedia
  })

  describe('useSessionItems', () => {
    it('opens: the first page, the catalogue, then the stream from the page’s anchor', async () => {
      const s = server([page([message('a', 't1')], 10)])
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      expect(result.current.loading).toBe(true)
      await waitFor(() => expect(result.current.stream).toBe('open'))
      expect(result.current.items.map((i) => i.id)).toEqual(['a'])
      expect(result.current.loading).toBe(false)
      expect(result.current.catalog?.model).toBe('m1')
      expect(s.of(PAGE_PATH)[0].path).toBe(PAGE_PATH)
      expect(s.of(STREAM_PATH)).toHaveLength(1)
      expect(s.of(STREAM_PATH)[0].headers['Last-Event-ID']).toBe('e1:10')
      expect(s.of(STREAM_PATH)[0].path).toBe(STREAM_PATH)
    })

    it('asks for 8 groups under 768 px', async () => {
      window.matchMedia = vi.fn().mockImplementation((query: string) => ({
        matches: query === '(max-width: 767px)',
        addEventListener: () => {},
        removeEventListener: () => {},
      })) as unknown as typeof window.matchMedia
      const s = server([page([message('a', 't1')], 10)])
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      expect(s.of(PAGE_PATH)[0].path).toBe(`${PAGE_PATH}?limit=8`)
    })

    it('applies upserts and removals from the stream', async () => {
      const s = server([page([message('a', 't1', 1)], 10)])
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      await waitFor(() => expect(s.streams).toHaveLength(1))
      act(() => {
        s.streams[0].event('item', { ...message('a', 't1', 2), text: 'changed' })
        s.streams[0].event('item', message('b', 't1', 3), 'e1:12')
      })
      await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['a', 'b']))
      expect((result.current.items[0] as Item & { text: string }).text).toBe('changed')
      act(() => s.streams[0].event('item_removed', { id: 'a' }, 'e1:13'))
      await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['b']))
    })

    it('says reconnecting while the stream is lost', async () => {
      const s = server([page([], 10)])
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      act(() => {
        s.streams[0].event('item', message('a', 't1'), 'e1:11')
        s.streams[0].end()
      })
      await waitFor(() => expect(result.current.stream).toBe('reconnecting'))
      await waitFor(() => expect(s.streams).toHaveLength(2), { timeout: 3000 })
      expect(s.of(STREAM_PATH)[1].headers['Last-Event-ID']).toBe('e1:11')
      await waitFor(() => expect(result.current.stream).toBe('open'))
      // No page was fetched again.
      expect(s.of(PAGE_PATH)).toHaveLength(1)
    })

    it('on resync_required: closes at once, refetches, replaces the store and reopens from the new anchor', async () => {
      const s = server([page([message('c', 't3')], 10, true), page([message('x', 't9')], 20, true)], {
        older: { t3: page([message('a', 't1')], 10, false) },
      })
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      await act(() => result.current.loadOlder())
      expect(s.of(PAGE_PATH)[1].path).toBe(`${PAGE_PATH}?before_turn=t3`)
      expect(result.current.items.map((i) => i.id)).toEqual(['a', 'c'])
      act(() => s.streams[0].send('event: resync_required\ndata: {}\n\n'))
      await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['x']))
      expect(result.current.older).toBe(true)
      expect(result.current.resynced).toBe(true)
      await waitFor(() => expect(result.current.stream).toBe('open'))
      // The old stream was closed, never reconnected with the old anchor.
      expect(s.streams[0].cancelled).toBe(true)
      expect(s.of(STREAM_PATH).map((c) => c.headers['Last-Event-ID'])).toEqual(['e1:10', 'e1:20'])
      // The catalogue too: a gap may have hidden a change of it.
      expect(s.of(CATALOG_PATH)).toHaveLength(2)
      await waitFor(() => expect(result.current.resynced).toBe(false))
    })

    it.each([
      ['item', 'not json'],
      ['item', '{"id":"a"}'],
      ['item_removed', '{"nope":1}'],
      ['catalog_changed', '{"model":"x"}'],
    ])('a malformed %s message (%s) resyncs', async (event, data) => {
      const s = server([page([message('a', 't1')], 10), page([message('y', 't2')], 30)])
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      act(() => s.streams[0].send(`event: ${event}\ndata: ${data}\nid: e1:15\n\n`))
      await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['y']))
      await waitFor(() => expect(s.streams).toHaveLength(2))
      expect(s.of(STREAM_PATH)[1].headers['Last-Event-ID']).toBe('e1:30')
      expect(s.streams[0].cancelled).toBe(true)
    })

    it('on session_removed: marks the session removed and closes the stream', async () => {
      const s = server([page([message('a', 't1')], 10)])
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      act(() => {
        s.streams[0].event('session_removed', { session_id: ID })
        s.streams[0].event('item', message('late', 't1'), 'e1:99')
      })
      await waitFor(() => expect(result.current.removed).toBe(true))
      expect(result.current.stream).toBe('closed')
      expect(s.streams[0].cancelled).toBe(true)
      expect(result.current.items.map((i) => i.id)).toEqual(['a'])
      await new Promise((r) => setTimeout(r, 30))
      expect(s.of(STREAM_PATH)).toHaveLength(1)
    })

    it('a 404 from the page marks the session removed and opens no stream', async () => {
      const s = server([404])
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      await waitFor(() => expect(result.current.removed).toBe(true))
      expect(result.current.loading).toBe(false)
      expect(result.current.stream).toBe('closed')
      expect(s.of(STREAM_PATH)).toHaveLength(0)
    })

    it('a 404 from the stream marks the session removed', async () => {
      const t = routed((call) => {
        if (call.path.startsWith('/api/stream/')) return json({ code: 'not_found', message: 'no' }, 404)
        if (call.path.endsWith('/catalog')) return json(catalog('m1'))
        return json(page([message('a', 't1')], 10))
      })
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: t.wrapper })
      await waitFor(() => expect(result.current.removed).toBe(true))
      expect(result.current.stream).toBe('closed')
    })

    it('another refusal from the stream ends it with an error, not removed', async () => {
      const t = routed((call) => {
        if (call.path.startsWith('/api/stream/')) return json({ code: 'invalid', message: 'no' }, 400)
        if (call.path.endsWith('/catalog')) return json(catalog('m1'))
        return json(page([], 10))
      })
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: t.wrapper })
      await waitFor(() => expect(result.current.error).not.toBeNull())
      expect(result.current.removed).toBe(false)
      expect(result.current.stream).toBe('closed')
    })

    it('on catalog_changed: the catalogue is replaced, with no fetch', async () => {
      const s = server([page([], 10)])
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      await waitFor(() => expect(result.current.catalog?.model).toBe('m1'))
      const fetches = s.fetch.mock.calls.length
      act(() => s.streams[0].event('catalog_changed', catalog('m2')))
      await waitFor(() => expect(result.current.catalog?.model).toBe('m2'))
      expect(s.fetch.mock.calls.length).toBe(fetches)
    })

    it('a catalogue fetched before a catalog_changed never replaces it', async () => {
      let release: (r: Response) => void = () => {}
      const streams: LiveStream[] = []
      const t = routed((call) => {
        if (call.path.endsWith('/catalog')) return new Promise<Response>((r) => (release = r))
        if (call.path.startsWith('/api/stream/')) {
          const live = liveStream()
          streams.push(live)
          return live.response
        }
        return json(page([], 10))
      })
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: t.wrapper })
      await waitFor(() => expect(streams).toHaveLength(1))
      act(() => streams[0].event('catalog_changed', catalog('new')))
      await waitFor(() => expect(result.current.catalog?.model).toBe('new'))
      await act(async () => release(json(catalog('old'))))
      expect(result.current.catalog?.model).toBe('new')
    })

    it('setCatalog (a config answer) replaces the catalogue', async () => {
      const s = server([page([], 10)])
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      await waitFor(() => expect(result.current.catalog?.model).toBe('m1'))
      act(() => result.current.setCatalog(catalog('m3')))
      expect(result.current.catalog?.model).toBe('m3')
    })

    it('loadOlder does nothing when nothing is older', async () => {
      const s = server([page([message('a', 't1')], 10, false)])
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      await act(() => result.current.loadOlder())
      expect(s.of(PAGE_PATH)).toHaveLength(1)
    })

    it('an older page that comes after a resync is dropped', async () => {
      let release: (r: Response) => void = () => {}
      const pages = [page([message('c', 't3')], 10, true), page([message('x', 't9')], 20, false)]
      const streams: LiveStream[] = []
      const t = routed((call) => {
        if (call.path.includes('before_turn')) return new Promise<Response>((r) => (release = r))
        if (call.path.endsWith('/catalog')) return json(catalog('m1'))
        if (call.path.startsWith('/api/stream/')) {
          const live = liveStream()
          streams.push(live)
          return live.response
        }
        return json(pages.length > 1 ? pages.shift()! : pages[0])
      })
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: t.wrapper })
      await waitFor(() => expect(streams).toHaveLength(1))
      let older: Promise<void> = Promise.resolve()
      act(() => {
        older = result.current.loadOlder()
      })
      act(() => streams[0].send('event: resync_required\ndata: {}\n\n'))
      await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['x']))
      await act(async () => {
        release(json(page([message('a', 't1')], 10, false)))
        await older
      })
      expect(result.current.items.map((i) => i.id)).toEqual(['x'])
      expect(result.current.loadingOlder).toBe(false)
    })

    it('fetches a failed first page again, then opens', async () => {
      const s = server([503, page([message('a', 't1')], 10)])
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      expect(s.of(PAGE_PATH)).toHaveLength(2)
      expect(result.current.error).toBeNull()
    })

    it('closes the stream on unmount', async () => {
      const s = server([page([], 10)])
      const { result, unmount } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      unmount()
      await waitFor(() => expect(s.streams[0].cancelled).toBe(true))
    })

    it('a refused first page shows its error and is not fetched again', async () => {
      const s = server([400, page([], 10)])
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      await waitFor(() => expect(result.current.error).not.toBeNull())
      expect(result.current.stream).toBe('closed')
      expect(result.current.removed).toBe(false)
      await new Promise((r) => setTimeout(r, 30))
      expect(s.of(PAGE_PATH)).toHaveLength(1)
    })

    it('a first page that is not a page is fetched again', async () => {
      let n = 0
      const t = routed((call) => {
        if (call.path.endsWith('/catalog')) return json(catalog('m1'))
        if (call.path.startsWith('/api/stream/')) return liveStream().response
        return json(n++ === 0 ? { nope: true } : page([message('a', 't1')], 10))
      })
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: t.wrapper })
      await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['a']))
      expect(n).toBe(2)
    })

    it('the wait before a refetch starts short again after a page comes', async () => {
      const retryMs = vi.fn(() => 5)
      const s = server([503, page([message('a', 't1')], 10), 503, page([message('b', 't1')], 20)])
      const { result } = renderHook(() => useSessionItems(ID, { ...FAST, retryMs }), { wrapper: s.wrapper })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      act(() => s.streams[0].send('event: resync_required\ndata: {}\n\n'))
      await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['b']))
      expect(retryMs.mock.calls).toEqual([[0], [0]])
    })

    it('an older page answered 404 marks the session removed', async () => {
      const s = server([page([message('c', 't3')], 10, true)], { older: { t3: 404 } })
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      await act(() => result.current.loadOlder())
      expect(result.current.removed).toBe(true)
      expect(s.streams[0].cancelled).toBe(true)
    })

    it('an older page answered 400 (the turn is gone) resyncs', async () => {
      const s = server([page([message('c', 't3')], 10, true), page([message('x', 't9')], 20)], { older: { t3: 400 } })
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      await act(() => result.current.loadOlder())
      await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['x']))
      expect(result.current.error).toBeNull()
    })

    /** A server whose first page comes at once and whose later first pages
     *  and older pages are held until the test answers them. */
    function heldServer(first: ItemPage) {
      const pages: ((r: Response) => void)[] = []
      const older: ((r: Response) => void)[] = []
      const streams: LiveStream[] = []
      let firstPages = 0
      const t = routed((call) => {
        if (call.path.includes('before_turn')) return new Promise<Response>((r) => older.push(r))
        if (call.path.endsWith('/catalog')) return json(catalog('m1'))
        if (call.path.startsWith('/api/stream/')) {
          const live = liveStream()
          streams.push(live)
          return live.response
        }
        firstPages++
        return firstPages === 1 ? json(first) : new Promise<Response>((r) => pages.push(r))
      })
      const live = () => streams.filter((s) => !s.cancelled)
      return { ...t, pages, older, streams, live, firstPages: () => firstPages }
    }

    const settle = () => new Promise((r) => setTimeout(r, 50))

    it('a resync while an older page is answered 400 leaves one stream open, and none after unmount', async () => {
      const s = heldServer(page([message('c', 't3')], 10, true))
      const { result, unmount } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      let older: Promise<void> = Promise.resolve()
      act(() => {
        older = result.current.loadOlder()
      })
      await waitFor(() => expect(s.older).toHaveLength(1))
      act(() => s.streams[0].send('event: resync_required\ndata: {}\n\n'))
      await waitFor(() => expect(s.pages).toHaveLength(1))
      // The 400 belongs to the run the resync replaced: it must not resync again.
      await act(async () => {
        s.older[0](json({ code: 'invalid', message: 'gone' }, 400))
        await older
      })
      await act(settle)
      await act(async () => {
        for (const answer of s.pages.splice(0)) answer(json(page([message('x', 't9')], 20, true)))
      })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      await act(settle)
      expect(s.firstPages()).toBe(2)
      expect(s.streams).toHaveLength(2)
      expect(s.live()).toHaveLength(1)
      expect(result.current.items.map((i) => i.id)).toEqual(['x'])
      expect(result.current.loadingOlder).toBe(false)
      unmount()
      await waitFor(() => expect(s.live()).toHaveLength(0))
    })

    it('an older page answered 400 after session_removed opens nothing', async () => {
      const s = heldServer(page([message('c', 't3')], 10, true))
      const { result } = renderHook(() => useSessionItems(ID, FAST), { wrapper: s.wrapper })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      let older: Promise<void> = Promise.resolve()
      act(() => {
        older = result.current.loadOlder()
      })
      await waitFor(() => expect(s.older).toHaveLength(1))
      act(() => s.streams[0].event('session_removed', { session_id: ID }))
      await waitFor(() => expect(result.current.removed).toBe(true))
      await act(async () => {
        s.older[0](json({ code: 'invalid', message: 'gone' }, 400))
        await older
      })
      await act(settle)
      expect(s.firstPages()).toBe(1)
      expect(s.streams).toHaveLength(1)
      expect(s.live()).toHaveLength(0)
      expect(result.current.stream).toBe('closed')
      expect(result.current.loadingOlder).toBe(false)
    })

    it('a resync drops a refetch already waiting to run', async () => {
      // A resync's first page fails (503) and waits to be fetched again; an
      // older page answered 400 then resyncs. The wait is dropped: one fetch.
      let n = 0
      const streams: LiveStream[] = []
      const t = routed((call) => {
        if (call.path.includes('before_turn')) return json({ code: 'invalid', message: 'gone' }, 400)
        if (call.path.endsWith('/catalog')) return json(catalog('m1'))
        if (call.path.startsWith('/api/stream/')) {
          const live = liveStream()
          streams.push(live)
          return live.response
        }
        n++
        if (n === 1) return json(page([message('c', 't3')], 10, true))
        if (n === 2) return json({ code: 'x', message: 'x' }, 503)
        return json(page([message('x', 't9')], 20, true))
      })
      const { result } = renderHook(() => useSessionItems(ID, { ...FAST, retryMs: () => 1000 }), { wrapper: t.wrapper })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      act(() => streams[0].send('event: resync_required\ndata: {}\n\n'))
      await waitFor(() => expect(n).toBe(2))
      await waitFor(() => expect(result.current.error).not.toBeNull())
      await act(() => result.current.loadOlder())
      // This resync follows the first with no stream opened between: it waits
      // `retryMs(0)` too (`ResyncBackoff`), after the dropped refetch's wait.
      await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['x']), { timeout: 3000 })
      // Past the dropped refetch's wait: it never fetches.
      await act(() => new Promise((r) => setTimeout(r, 1200)))
      expect(n).toBe(3)
    })
  })
  ```

Create `web/src/store/useSessionList.test.ts`:

  ```ts
  import { act, renderHook, waitFor } from '@testing-library/react'
  import { describe, expect, it, vi } from 'vitest'
  import type { SessionSummary, SummaryPage } from '../generated/view'
  import { json, liveStream, routed, type LiveStream } from '../test-stream'
  import type { ListFilters } from './sessionList'
  import { useSessionList } from './useSessionList'

  const LIST = '/api/view/sessions'
  const STREAM = '/api/stream/sessions'
  const FAST = { retryMs: () => 5, resyncedMs: 400 }

  function summary(id: string, patch: Partial<SessionSummary> = {}): SessionSummary {
    return {
      session_id: id,
      host_id: 'h1',
      agent: 'claude',
      cwd: '/srv/work/app',
      hat_id: 'hat-a',
      lifecycle: 'active',
      presumed_parked: false,
      created_at: '2026-10-01T00:00:00.000Z',
      last_event_at: '2026-10-02T10:00:00.000Z',
      question_waits: false,
      ...patch,
    }
  }

  /** A page; with no `waiting`, a server that does not count (the rows are
   *  counted instead). */
  function page(sessions: SessionSummary[], revision: number, next?: string, waiting?: number): SummaryPage {
    return { sessions, next_cursor: next, epoch: 'e1', revision, ...(waiting === undefined ? {} : { waiting }) } as SummaryPage
  }

  /** First pages served in turn (the last repeats), further pages by cursor,
   *  and a new live stream per connection. */
  function server(firsts: SummaryPage[], more: Record<string, SummaryPage> = {}) {
    const streams: LiveStream[] = []
    const t = routed((call) => {
      const url = new URL(call.path, 'http://h')
      if (url.pathname === LIST) {
        const cursor = url.searchParams.get('cursor')
        if (cursor !== null) return json(more[cursor])
        return json(firsts.length > 1 ? firsts.shift()! : firsts[0])
      }
      if (url.pathname === STREAM) {
        const live = liveStream()
        streams.push(live)
        return live.response
      }
      return json({ code: 'not_found', message: 'no' }, 404)
    })
    const of = (path: string) => t.calls.filter((c) => new URL(c.path, 'http://h').pathname === path)
    return { ...t, streams, of }
  }

  const ids = (list: SessionSummary[]) => list.map((s) => s.session_id)

  function render(s: ReturnType<typeof server>, filters: ListFilters) {
    return renderHook((f: ListFilters) => useSessionList(f, FAST), { wrapper: s.wrapper, initialProps: filters })
  }

  describe('useSessionList', () => {
    it('opens: the first page for the query, then the stream from its anchor', async () => {
      const s = server([page([summary('a'), summary('b', { lifecycle: 'closed' })], 10, 'c1')])
      const { result } = render(s, { hat: 'hat-a', hideClosed: true })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      expect(s.of(LIST)[0].path).toBe(`${LIST}?hat=hat-a&lifecycle=starting%2Cactive%2Cparked%2Cfailed`)
      expect(s.of(STREAM)[0].path).toBe(`${STREAM}?hat=hat-a`)
      expect(s.of(STREAM)[0].headers['Last-Event-ID']).toBe('e1:10')
      expect(ids(result.current.shown)).toEqual(['a'])
      expect(result.current.all.size).toBe(2)
      expect(result.current.hasMore).toBe(true)
    })

    it('upserts insert, update, and move a session off the shown list when it leaves the filter', async () => {
      const s = server([page([summary('a'), summary('b', { last_event_at: '2026-10-02T09:00:00.000Z' })], 10)])
      const { result } = render(s, { hideClosed: true })
      await waitFor(() => expect(s.streams).toHaveLength(1))
      act(() => s.streams[0].event('session_upsert', summary('n', { last_event_at: '2026-10-02T12:00:00.000Z' }), 'e1:11'))
      await waitFor(() => expect(ids(result.current.shown)).toEqual(['n', 'a', 'b']))
      act(() => s.streams[0].event('session_upsert', summary('b', { title: 'T', last_event_at: '2026-10-02T13:00:00.000Z' }), 'e1:12'))
      await waitFor(() => expect(ids(result.current.shown)).toEqual(['b', 'n', 'a']))
      expect(result.current.shown[0].title).toBe('T')
      act(() => s.streams[0].event('session_upsert', summary('n', { lifecycle: 'closed' }), 'e1:13'))
      await waitFor(() => expect(ids(result.current.shown)).toEqual(['b', 'a']))
      expect(result.current.all.has('n')).toBe(true)
    })

    it('during a search an upsert never inserts a row', async () => {
      const s = server([page([summary('a')], 10)])
      const { result } = render(s, { hideClosed: true, q: 'fix' })
      await waitFor(() => expect(s.streams).toHaveLength(1))
      expect(s.of(LIST)[0].path).toBe(`${LIST}?q=fix`)
      expect(result.current.searching).toBe(true)
      act(() => {
        s.streams[0].event('session_upsert', summary('n'))
        s.streams[0].event('session_upsert', summary('a', { title: 'found' }), 'e1:11')
      })
      await waitFor(() => expect(result.current.shown[0].title).toBe('found'))
      expect(ids(result.current.shown)).toEqual(['a'])
    })

    it('session_removed takes the session away', async () => {
      const s = server([page([summary('a'), summary('b')], 10)])
      const { result } = render(s, { hideClosed: false })
      await waitFor(() => expect(s.streams).toHaveLength(1))
      act(() => s.streams[0].event('session_removed', { session_id: 'a' }, 'e1:11'))
      await waitFor(() => expect(ids(result.current.shown)).toEqual(['b']))
      expect(result.current.all.has('a')).toBe(false)
    })

    it('on resync_required: closes at once, refetches the first page with the current query, replaces and reopens', async () => {
      const s = server([page([summary('a')], 10, 'c1'), page([summary('z')], 40)], { c1: page([summary('b', { last_event_at: '2026-10-01T00:00:00.000Z' })], 15) })
      const { result } = render(s, { hat: 'hat-a', hideClosed: false })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      await act(() => result.current.loadMore())
      expect(ids(result.current.shown)).toEqual(['a', 'b'])
      act(() => s.streams[0].send('event: resync_required\ndata: {}\n\n'))
      await waitFor(() => expect(ids(result.current.shown)).toEqual(['z']))
      expect(result.current.resynced).toBe(true)
      await waitFor(() => expect(s.streams).toHaveLength(2))
      expect(s.streams[0].cancelled).toBe(true)
      expect(s.of(LIST).map((c) => c.path)).toEqual([`${LIST}?hat=hat-a`, `${LIST}?cursor=c1&hat=hat-a`, `${LIST}?hat=hat-a`])
      expect(s.of(STREAM).map((c) => c.headers['Last-Event-ID'])).toEqual(['e1:10', 'e1:40'])
      await waitFor(() => expect(result.current.resynced).toBe(false))
    })

    it.each([
      ['session_upsert', 'nope'],
      ['session_upsert', '{"session_id":"a"}'],
      ['session_removed', '{}'],
      ['waiting_changed', 'nope'],
      ['waiting_changed', '{}'],
      ['waiting_changed', '{"count":-1}'],
      ['waiting_changed', '{"count":1.5}'],
      ['waiting_changed', '{"count":"2"}'],
    ])('a malformed %s (%s) resyncs', async (event, data) => {
      const s = server([page([summary('a')], 10), page([summary('y')], 30)])
      const { result } = render(s, { hideClosed: false })
      await waitFor(() => expect(s.streams).toHaveLength(1))
      act(() => s.streams[0].send(`event: ${event}\ndata: ${data}\n\n`))
      await waitFor(() => expect(ids(result.current.shown)).toEqual(['y']))
      await waitFor(() => expect(s.streams).toHaveLength(2))
      expect(s.of(STREAM)[1].headers['Last-Event-ID']).toBe('e1:30')
    })

    it('a further page never moves the stream’s anchor', async () => {
      const s = server([page([summary('a')], 10, 'c1')], { c1: page([summary('b')], 77) })
      const { result } = render(s, { hideClosed: false })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      await act(() => result.current.loadMore())
      expect(result.current.hasMore).toBe(false)
      act(() => s.streams[0].end())
      await waitFor(() => expect(s.streams).toHaveLength(2), { timeout: 3000 })
      expect(s.of(STREAM)[1].headers['Last-Event-ID']).toBe('e1:10')
    })

    it('no event ever fetches (F-4)', async () => {
      const s = server([page([summary('a'), summary('b')], 10)])
      const { result } = render(s, { hideClosed: true })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      const before = s.fetch.mock.calls.length
      act(() => {
        s.streams[0].event('session_upsert', summary('n'))
        s.streams[0].event('session_upsert', summary('a', { lifecycle: 'closed' }))
        s.streams[0].event('session_upsert', summary('b', { activity: 'blocked' }))
        s.streams[0].event('session_removed', { session_id: 'n' })
        s.streams[0].event('something_new', { x: 1 }, 'e1:20')
      })
      await waitFor(() => expect(result.current.counts.waiting).toBe(1))
      act(() => {
        // The server may send a session_removed twice (a delete racing its read).
        s.streams[0].event('session_removed', { session_id: 'n' })
        s.streams[0].event('session_removed', { session_id: 'a' })
        s.streams[0].event('session_removed', { session_id: 'a' }, 'e1:21')
        s.streams[0].event('waiting_changed', { count: 6 }, 'e1:22')
      })
      await waitFor(() => expect(result.current.counts.waiting).toBe(6))
      expect(result.current.all.has('a')).toBe(false)
      await new Promise((r) => setTimeout(r, 30))
      expect(s.fetch.mock.calls.length).toBe(before)
      expect(s.streams).toHaveLength(1)
      expect(result.current.stream).toBe('open')
    })

    it('the stream is opened with the hat, encoded, and with none for every hat', async () => {
      for (const [hat, path] of [
        ['h/1 x', `${STREAM}?hat=h%2F1+x`],
        ['', STREAM],
        [null, STREAM],
      ] as const) {
        const s = server([page([summary('a')], 10)])
        const { result, unmount } = render(s, { hat, hideClosed: false })
        await waitFor(() => expect(result.current.stream).toBe('open'))
        expect(s.of(STREAM)[0].path).toBe(path)
        unmount()
      }
    })

    it('holds the server’s count from the first page, and waiting_changed sets it, with no fetch', async () => {
      // One blocked row, but the server counts 4: the server's number wins.
      const s = server([page([summary('a', { activity: 'blocked' })], 10, undefined, 4)])
      const { result } = render(s, { hat: 'hat-a', hideClosed: false })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      expect(result.current.counts.waiting).toBe(4)
      const before = s.fetch.mock.calls.length
      act(() => s.streams[0].event('session_upsert', summary('a', { activity: 'running' }), 'e1:11'))
      await new Promise((r) => setTimeout(r, 20))
      expect(result.current.counts.waiting).toBe(4)
      act(() => s.streams[0].event('waiting_changed', { count: 1 }, 'e1:12'))
      await waitFor(() => expect(result.current.counts.waiting).toBe(1))
      act(() => s.streams[0].event('waiting_changed', { count: 0 }, 'e1:13'))
      await waitFor(() => expect(result.current.counts.waiting).toBe(0))
      expect(s.fetch.mock.calls.length).toBe(before)
    })

    it('a further page never changes the server’s count', async () => {
      const s = server([page([summary('a')], 10, 'c1', 2)], { c1: page([summary('b', { question_waits: true, last_event_at: '2026-10-01T10:00:00.000Z' })], 15, undefined, 9) })
      const { result } = render(s, { hideClosed: false })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      act(() => s.streams[0].event('waiting_changed', { count: 3 }, 'e1:11'))
      await waitFor(() => expect(result.current.counts.waiting).toBe(3))
      await act(() => result.current.loadMore())
      expect(ids(result.current.shown)).toEqual(['a', 'b'])
      expect(result.current.counts.waiting).toBe(3)
    })

    it('a resync takes the count from the new first page', async () => {
      const s = server([
        page([summary('a')], 10, undefined, 5),
        page([summary('z')], 40, undefined, 1),
        page([summary('y', { activity: 'blocked' }), summary('x', { activity: 'blocked' })], 50),
      ])
      const { result } = render(s, { hideClosed: false })
      await waitFor(() => expect(result.current.counts.waiting).toBe(5))
      act(() => s.streams[0].send('event: resync_required\ndata: {}\n\n'))
      await waitFor(() => expect(ids(result.current.shown)).toEqual(['z']))
      expect(result.current.counts.waiting).toBe(1)
      await waitFor(() => expect(s.streams).toHaveLength(2))
      // A first page with no count: the rows are counted, never the old count.
      act(() => s.streams[1].send('event: resync_required\ndata: {}\n\n'))
      await waitFor(() => expect(ids(result.current.shown)).toEqual(['y', 'x']))
      expect(result.current.counts.waiting).toBe(2)
    })

    it('a first page whose count is not a count is fetched again', async () => {
      const answers = [() => json({ ...page([summary('a')], 10), waiting: -1 }), () => json(page([summary('b')], 20, undefined, 2))]
      const streams: LiveStream[] = []
      const t = routed((call) => {
        if (call.path.startsWith(STREAM)) {
          const live = liveStream()
          streams.push(live)
          return live.response
        }
        return answers.shift()!()
      })
      const { result } = renderHook(() => useSessionList({ hideClosed: false }, FAST), { wrapper: t.wrapper })
      await waitFor(() => expect(ids(result.current.shown)).toEqual(['b']))
      expect(result.current.counts.waiting).toBe(2)
      expect(streams).toHaveLength(1)
    })

    it('counts the hat’s sessions waiting on a question', async () => {
      const s = server([
        page(
          [
            summary('a', { activity: 'blocked' }),
            summary('b', { question_waits: true }),
            summary('c', { activity: 'blocked', hat_id: 'hat-b' }),
          ],
          10,
        ),
      ])
      const { result } = render(s, { hat: 'hat-a', hideClosed: false })
      await waitFor(() => expect(result.current.counts.waiting).toBe(2))
    })

    it('a new query starts over; the same query in a new object does not', async () => {
      const s = server([page([summary('a')], 10), page([summary('b', { hat_id: 'hat-b' })], 20)])
      const { result, rerender } = render(s, { hat: 'hat-a', hideClosed: false })
      await waitFor(() => expect(result.current.stream).toBe('open'))
      rerender({ hat: 'hat-a', hideClosed: false })
      await new Promise((r) => setTimeout(r, 20))
      expect(s.of(LIST)).toHaveLength(1)
      rerender({ hat: 'hat-b', hideClosed: false })
      await waitFor(() => expect(ids(result.current.shown)).toEqual(['b']))
      expect(s.of(LIST)[1].path).toBe(`${LIST}?hat=hat-b`)
      expect(s.streams[0].cancelled).toBe(true)
      await waitFor(() => expect(s.of(STREAM)[1]?.headers['Last-Event-ID']).toBe('e1:20'))
      // The new stream counts the new hat's sessions.
      expect(s.of(STREAM).map((c) => c.path)).toEqual([`${STREAM}?hat=hat-a`, `${STREAM}?hat=hat-b`])
    })

    it('a further page that comes after a resync is dropped', async () => {
      let release: (r: Response) => void = () => {}
      const firsts = [page([summary('a')], 10, 'c1'), page([summary('z')], 20)]
      const streams: LiveStream[] = []
      const t = routed((call) => {
        if (call.path.includes('cursor=')) return new Promise<Response>((r) => (release = r))
        if (call.path.startsWith(STREAM)) {
          const live = liveStream()
          streams.push(live)
          return live.response
        }
        return json(firsts.length > 1 ? firsts.shift()! : firsts[0])
      })
      const { result } = renderHook(() => useSessionList({ hideClosed: false }, FAST), { wrapper: t.wrapper })
      await waitFor(() => expect(streams).toHaveLength(1))
      let more: Promise<void> = Promise.resolve()
      act(() => {
        more = result.current.loadMore()
      })
      act(() => streams[0].send('event: resync_required\ndata: {}\n\n'))
      await waitFor(() => expect(ids(result.current.shown)).toEqual(['z']))
      await act(async () => {
        release(json(page([summary('b')], 15)))
        await more
      })
      expect(ids(result.current.shown)).toEqual(['z'])
      expect(result.current.loadingMore).toBe(false)
    })

    it('after a resync dropped a further page, the next page can still be loaded', async () => {
      let release: (r: Response) => void = () => {}
      const firsts = [page([summary('a')], 10, 'c1'), page([summary('z')], 20, 'c2')]
      const streams: LiveStream[] = []
      const t = routed((call) => {
        if (call.path.includes('cursor=c1')) return new Promise<Response>((r) => (release = r))
        if (call.path.includes('cursor=c2')) return json(page([summary('y', { last_event_at: '2026-10-02T09:00:00.000Z' })], 25))
        if (call.path.startsWith(STREAM)) {
          const live = liveStream()
          streams.push(live)
          return live.response
        }
        return json(firsts.length > 1 ? firsts.shift()! : firsts[0])
      })
      const { result } = renderHook(() => useSessionList({ hideClosed: false }, FAST), { wrapper: t.wrapper })
      await waitFor(() => expect(streams).toHaveLength(1))
      let more: Promise<void> = Promise.resolve()
      act(() => {
        more = result.current.loadMore()
      })
      act(() => streams[0].send('event: resync_required\ndata: {}\n\n'))
      await waitFor(() => expect(ids(result.current.shown)).toEqual(['z']))
      await act(async () => {
        release(json(page([summary('b')], 15)))
        await more
      })
      await act(() => result.current.loadMore())
      expect(ids(result.current.shown)).toEqual(['z', 'y'])
      expect(t.calls.filter((c) => c.path.includes('cursor=c2'))).toHaveLength(1)
    })

    it('a refused first page shows its error and is not fetched again', async () => {
      const t = routed(() => json({ code: 'invalid', message: 'bad' }, 400))
      const { result } = renderHook(() => useSessionList({ hideClosed: false }, FAST), { wrapper: t.wrapper })
      await waitFor(() => expect(result.current.error).toBe('bad'))
      expect(result.current.stream).toBe('closed')
      await new Promise((r) => setTimeout(r, 30))
      expect(t.calls).toHaveLength(1)
    })

    it('a refusal from the stream ends it with an error', async () => {
      const t = routed((call) =>
        call.path.startsWith(STREAM) ? json({ code: 'x', message: 'x' }, 403) : json(page([summary('a')], 10)),
      )
      const { result } = renderHook(() => useSessionList({ hideClosed: false }, FAST), { wrapper: t.wrapper })
      await waitFor(() => expect(result.current.error).not.toBeNull())
      expect(result.current.stream).toBe('closed')
    })

    it('a failed or unreadable first page is fetched again, the wait starting short each time', async () => {
      const retryMs = vi.fn(() => 5)
      const answers = [
        () => json({ code: 'x', message: 'x' }, 503),
        () => json(page([summary('a')], 10)),
        () => json({ sessions: 'no' }),
        () => json(page([summary('b')], 20)),
      ]
      const streams: LiveStream[] = []
      const t = routed((call) => {
        if (call.path.startsWith(STREAM)) {
          const live = liveStream()
          streams.push(live)
          return live.response
        }
        return answers.shift()!()
      })
      const { result } = renderHook(() => useSessionList({ hideClosed: false }, { ...FAST, retryMs }), { wrapper: t.wrapper })
      await waitFor(() => expect(streams).toHaveLength(1))
      act(() => streams[0].send('event: resync_required\ndata: {}\n\n'))
      await waitFor(() => expect(ids(result.current.shown)).toEqual(['b']))
      expect(retryMs.mock.calls).toEqual([[0], [0]])
    })
  })
  ```

Create `web/src/test-stream.ts`:

  ```ts
  // Test helpers for the stores: a `fetch` that answers by path and records
  // every call with its headers, and SSE bodies a test writes to as it goes.
  import { createElement, type ReactNode } from 'react'
  import { vi } from 'vitest'
  import { Client } from './api/client'
  import { ClientContext } from './app-client'

  export interface Call {
    method: string
    path: string
    headers: Record<string, string>
  }

  export function json(body: unknown, status = 200): Response {
    return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
  }

  /** An SSE body that stays open until `end()`. */
  export interface LiveStream {
    response: Response
    /** Write raw SSE text. */
    send(text: string): void
    /** One message: `event`, `data` (JSON unless a string), and an `id`. */
    event(event: string, data: unknown, id?: string): void
    end(): void
    cancelled: boolean
  }

  export function liveStream(): LiveStream {
    const encoder = new TextEncoder()
    let controller!: ReadableStreamDefaultController<Uint8Array>
    const live: LiveStream = {
      cancelled: false,
      response: new Response(
        new ReadableStream<Uint8Array>({
          start(c) {
            controller = c
          },
          cancel() {
            live.cancelled = true
          },
        }),
        { status: 200, headers: { 'Content-Type': 'text/event-stream' } },
      ),
      send(text) {
        if (!live.cancelled) controller.enqueue(encoder.encode(text))
      },
      event(event, data, id) {
        const text = typeof data === 'string' ? data : JSON.stringify(data)
        live.send(`event: ${event}\ndata: ${text}\n${id === undefined ? '' : `id: ${id}\n`}\n`)
      },
      end() {
        if (!live.cancelled) controller.close()
      },
    }
    return live
  }

  /** A client over a `fetch` that `answer` serves; `calls` records each. */
  export function routed(answer: (call: Call) => Response | Promise<Response>) {
    const calls: Call[] = []
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const call = {
        method: init?.method ?? 'GET',
        path: String(input),
        headers: { ...(init?.headers as Record<string, string>) },
      }
      calls.push(call)
      return answer(call)
    })
    const client = new Client({
      fetch: fetch as unknown as typeof globalThis.fetch,
      navigate: vi.fn(),
      here: () => ({ pathname: '/sessions', search: '' }),
      stepUp: async () => {},
    })
    const wrapper = ({ children }: { children: ReactNode }) => createElement(ClientContext.Provider, { value: client }, children)
    return { calls, fetch, client, wrapper }
  }
  ```

- [ ] **Step 2: Run them, and see them fail**

Run: `nix develop -c sh -c 'cd web && pnpm vitest run src/api src/store'`
Expected: FAIL. The new test files cannot resolve `./view`, `./items`, `./useSessionItems`, `./sessionList` and `./useSessionList`, and `sse.test.ts`'s new cases fail against 4b's helper.

- [ ] **Step 3: The view client, the helper's fixes and the stores**

In `web/src/api/sse.ts`, replace:

  ```ts
  // - `resync_required` asks the consumer to refetch its first page.
  ```

with:

  ```ts
  // - `resync_required` asks the consumer to refetch its first page. The
  //   server ends the stream after it: the consumer closes this stream at
  //   once, inside `onResync`, and opens a new one from the new page's anchor
  //   (close and reopen; the helper has no way to reset its id).
  // - A handler that throws is a resync too: the helper stops reading that
  //   connection, keeps the id of the last event handled whole, and calls
  //   `onResync`. If the consumer does not close, the reconnect resumes from
  //   that id, so the event that threw is sent again, never lost. Consumers
  //   are not meant to throw: they parse in try/catch and resync on a
  //   malformed message themselves.
  ```

In `web/src/api/sse.ts`, replace:

  ```ts
    /** The server can no longer fill the gap: refetch the first page. */
  ```

with:

  ```ts
    /** The server can no longer fill the gap, or a handler threw: close this
     *  stream, refetch the first page and open a new one. */
  ```

In `web/src/api/sse.ts`, replace:

  ```ts
    sleep?: (ms: number) => Promise<void>
  ```

with:

  ```ts
    sleep?: (ms: number, signal: AbortSignal) => Promise<void>
  ```

In `web/src/api/sse.ts`, replace:

  ```ts
  export function openStream(client: Client, path: string, options: StreamOptions): Stream {
    const abort = new AbortController()
    const sleep = options.sleep ?? ((ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)))
    let lastId = options.lastEventId
  ```

with:

  ```ts
  /** `setTimeout` as a promise that also settles when `signal` aborts, so a
   *  closed stream leaves no timer behind. */
  function abortableSleep(ms: number, signal: AbortSignal): Promise<void> {
    return new Promise<void>((resolve) => {
      if (signal.aborted) return resolve()
      const done = () => {
        clearTimeout(timer)
        signal.removeEventListener('abort', done)
        resolve()
      }
      const timer = setTimeout(done, ms)
      signal.addEventListener('abort', done, { once: true })
    })
  }

  export function openStream(client: Client, path: string, options: StreamOptions): Stream {
    const abort = new AbortController()
    const sleep = options.sleep ?? abortableSleep
    let lastId = options.lastEventId === '' ? undefined : options.lastEventId
  ```

In `web/src/api/sse.ts`, replace:

  ```ts
          if (response.ok && response.body) {
            set('open')
            delay = FIRST_DELAY_MS
            await read(response.body, abort.signal, (event) => {
              if (event.id !== undefined) lastId = event.id
              if (event.event === 'resync_required') options.onResync?.()
              else options.onEvent(event)
            })
          } else if (status >= 400 && status < 500) {
            set('closed')
            options.onError?.(status)
            return
  ```

with:

  ```ts
          // Closed while connecting: nothing of this answer is reported.
          if (abort.signal.aborted) {
            void response.body?.cancel().catch(() => {})
            break
          }
          if (response.ok && response.body) {
            set('open')
            delay = FIRST_DELAY_MS
            const threw = await read(response.body, abort.signal, (event) => {
              if (event.event === 'resync_required') {
                options.onResync?.()
                return
              }
              if (event.data !== undefined) options.onEvent(event as StreamEvent)
            }, (id) => {
              lastId = id
            })
            if (threw && !abort.signal.aborted) options.onResync?.()
          } else {
            // Nobody reads it: give the connection back.
            void response.body?.cancel().catch(() => {})
            if (status >= 400 && status < 500) {
              set('closed')
              options.onError?.(status)
              return
            }
  ```

In `web/src/api/sse.ts`, replace:

  ```ts
        await sleep(delay)
  ```

with:

  ```ts
        await sleep(delay, abort.signal)
  ```

In `web/src/api/sse.ts`, replace:

  ```ts
      close: () => abort.abort(),
  ```

with:

  ```ts
      close: () => {
        abort.abort()
        set('closed')
      },
  ```

In `web/src/api/sse.ts`, replace:

  ```ts
  /** Read `body` to its end, calling `dispatch` per event (the SSE format:
   *  `event:`, `data:` lines joined by newlines, `id:`; `:` comments). An
   *  event with no data is dropped, as the format says, except
   *  `resync_required`, which needs none. */
  async function read(
    body: ReadableStream<Uint8Array>,
    signal: AbortSignal,
    dispatch: (event: StreamEvent) => void,
  ): Promise<void> {
    const reader = body.getReader()
    const decoder = new TextDecoder()
    signal.addEventListener('abort', () => void reader.cancel().catch(() => {}), { once: true })
  ```

with:

  ```ts
  interface Block {
    event: string
    data?: string
    id?: string
  }

  /** Read `body` to its end, calling `dispatch` per event (the SSE format:
   *  `event:`, `data:` lines joined by newlines, `id:`; `:` comments; a line
   *  ends at CRLF, LF or a lone CR). An event with no data dispatches nothing,
   *  as the format says, except `resync_required`, which needs none; its `id`
   *  still counts. `commit` gets each block's id once its handler returned
   *  (an empty id clears it). Whether a handler threw: the reading stops
   *  there, and no later id is committed. */
  async function read(
    body: ReadableStream<Uint8Array>,
    signal: AbortSignal,
    dispatch: (event: Block) => void,
    commit: (id: string | undefined) => void,
  ): Promise<boolean> {
    const reader = body.getReader()
    const decoder = new TextDecoder()
    const cancel = () => void reader.cancel().catch(() => {})
    signal.addEventListener('abort', cancel, { once: true })
  ```

In `web/src/api/sse.ts`, replace:

  ```ts
    for (;;) {
      const { value, done } = await reader.read()
      if (done || signal.aborted) return
      buffer += decoder.decode(value, { stream: true })
      let newline: number
      while ((newline = buffer.search(/\r\n|\r|\n/)) >= 0) {
        // A `\r` that ends the chunk may be the first half of `\r\n`.
        if (newline === buffer.length - 1 && buffer[newline] === '\r') break
        const line = buffer.slice(0, newline)
        buffer = buffer.slice(newline + (buffer.startsWith('\r\n', newline) ? 2 : 1))
        if (line === '') {
          if (data.length > 0 || event === 'resync_required') dispatch({ event: event || 'message', data: data.join('\n'), id })
          event = ''
          data = []
          id = undefined
          continue
        }
        if (line.startsWith(':')) continue
        const colon = line.indexOf(':')
        const field = colon < 0 ? line : line.slice(0, colon)
        let value = colon < 0 ? '' : line.slice(colon + 1)
        if (value.startsWith(' ')) value = value.slice(1)
        if (field === 'event') event = value
        else if (field === 'data') data.push(value)
        else if (field === 'id' && !value.includes('\0')) id = value
      }
  ```

with:

  ```ts
    let hasId = false

    /** One line; `false` once a handler threw. */
    const line = (text: string): boolean => {
      if (text === '') {
        const block: Block = { event: event || 'message', id }
        if (data.length > 0) block.data = data.join('\n')
        const takesId = hasId
        const blockId = id
        event = ''
        data = []
        id = undefined
        hasId = false
        if (signal.aborted) return true
        if (block.data !== undefined || block.event === 'resync_required') {
          try {
            dispatch(block)
          } catch {
            return false
          }
        }
        if (takesId) commit(blockId === '' ? undefined : blockId)
        return true
      }
      if (text.startsWith(':')) return true
      const colon = text.indexOf(':')
      const field = colon < 0 ? text : text.slice(0, colon)
      let value = colon < 0 ? '' : text.slice(colon + 1)
      if (value.startsWith(' ')) value = value.slice(1)
      if (field === 'event') event = value
      else if (field === 'data') data.push(value)
      else if (field === 'id' && !value.includes('\0')) {
        id = value
        hasId = true
      }
      return true
    }

    /** Every whole line in the buffer; at the end, a trailing CR ends one. */
    const lines = (atEnd: boolean): boolean => {
      let newline: number
      while ((newline = buffer.search(/\r\n|\r|\n/)) >= 0) {
        // A `\r` that ends the chunk may be the first half of `\r\n`.
        if (!atEnd && newline === buffer.length - 1 && buffer[newline] === '\r') break
        const text = buffer.slice(0, newline)
        buffer = buffer.slice(newline + (buffer.startsWith('\r\n', newline) ? 2 : 1))
        if (!line(text)) return false
      }
      return true
    }

    try {
      for (;;) {
        const { value, done } = await reader.read()
        if (done) {
          buffer += decoder.decode()
          // An event not ended by an empty line is dropped, as the format says.
          return !lines(true)
        }
        buffer += decoder.decode(value, { stream: true })
        if (!lines(false)) {
          cancel()
          return true
        }
      }
    } finally {
      signal.removeEventListener('abort', cancel)
  ```

Create `web/src/api/view.ts`:

  ```ts
  // The view API, typed (client view spec §4): a session's items a page of
  // whole turns at a time, the session list's summaries, an undelivered turn,
  // and the streams' paths. Every id in a path is encoded: ids are server
  // data, never trusted to be path-safe.
  import type { Client } from './client'
  import type { ItemPage, SummaryPage, TurnContent } from '../generated/view'
  import type { SessionCatalog, SessionDetail } from '../generated/protocol'

  const enc = encodeURIComponent

  /** `?a=1&b=2` of the params that are set, or nothing. */
  function query(params: Record<string, string | number | undefined>): string {
    const search = new URLSearchParams()
    for (const [key, value] of Object.entries(params)) {
      if (value !== undefined) search.set(key, String(value))
    }
    const text = search.toString()
    return text ? `?${text}` : ''
  }

  export interface ItemPageQuery {
    /** The groups before this turn's (the first loaded item's `turn_id`). */
    beforeTurn?: string
    /** Groups per page; the server's default (20) when absent. */
    limit?: number
  }

  /** `GET /api/view/sessions/{id}`: the last `limit` groups, or those before
   *  `beforeTurn`. */
  export function itemPage(client: Client, id: string, q: ItemPageQuery = {}): Promise<ItemPage> {
    return client.request<ItemPage>(
      'GET',
      `/api/view/sessions/${enc(id)}${query({ before_turn: q.beforeTurn, limit: q.limit })}`,
    )
  }

  export interface SummaryQuery {
    cursor?: string
    limit?: number
    q?: string
    /** A hat's id; never empty (the server refuses `hat=`). */
    hat?: string
    /** Lifecycle names (`starting`, `active`, `parked`, `closed`, `failed`). */
    lifecycle?: readonly string[]
  }

  /** `GET /api/view/sessions`: one page of summaries, newest first. */
  export function summaries(client: Client, q: SummaryQuery = {}): Promise<SummaryPage> {
    return client.request<SummaryPage>(
      'GET',
      `/api/view/sessions${query({
        cursor: q.cursor,
        limit: q.limit,
        q: q.q || undefined,
        hat: q.hat || undefined,
        lifecycle: q.lifecycle && q.lifecycle.length > 0 ? q.lifecycle.join(',') : undefined,
      })}`,
    )
  }

  /** `GET /api/view/sessions/{id}/turns/{turn_id}`: a prompt that was not
   *  delivered, whole, to send again. */
  export function undeliveredTurn(client: Client, id: string, turnId: string): Promise<TurnContent> {
    return client.request<TurnContent>('GET', `/api/view/sessions/${enc(id)}/turns/${enc(turnId)}`)
  }

  /** `GET /api/sessions/{id}/catalog`: the config options and slash commands. */
  export function catalog(client: Client, id: string): Promise<SessionCatalog> {
    return client.request<SessionCatalog>('GET', `/api/sessions/${enc(id)}/catalog`)
  }

  /** `GET /api/sessions/{id}`: the session, its open turn and pending requests. */
  export function sessionDetail(client: Client, id: string): Promise<SessionDetail> {
    return client.request<SessionDetail>('GET', `/api/sessions/${enc(id)}`)
  }

  /** The item stream of one session. It resumes from `Last-Event-ID`. */
  export function itemStreamPath(id: string): string {
    return `/api/stream/view/sessions/${enc(id)}`
  }

  /** The session list's stream. It sends every session's summaries; `hat`
   *  scopes only its `waiting_changed` count. Never sent empty: the server
   *  refuses `hat=`. */
  export function listStreamPath(hat?: string | null): string {
    return `/api/stream/sessions${query({ hat: hat || undefined })}`
  }

  /** Where a stream resumes after a page: `<epoch>:<revision>`, opaque. */
  export function anchorOf(page: { epoch: string; revision: number }): string {
    return `${page.epoch}:${page.revision}`
  }
  ```

Create `web/src/store/items.ts`:

  ```ts
  // One session's items, as a pure reducer (plan 4c decision 4): a first page,
  // older pages prepended, and the item stream's upserts and removals.
  //
  // - An item is replaced in place by id, only by a version at least the
  //   one held.
  // - A new id joins its group (`turn_id`, or `start` before the first turn)
  //   at the group's end when that group is loaded; it starts a new group at
  //   the end when it is newer than every loaded group (no older groups left,
  //   or its `ts` is at least the first loaded item's); otherwise it belongs
  //   to a turn not loaded, and is ignored.
  // - `turn_id` is an opaque key: never parsed.
  // - Anything that is not an item is ignored: the reducer never throws.
  import type { Item, ItemPage } from '../generated/view'

  export interface ItemsState {
    items: Item[]
    /** Groups exist before the first loaded one. */
    older: boolean
    /** The first page's anchor: where the item stream resumes. */
    epoch: string
    revision: number
  }

  export type ItemsAction =
    /** The first page, or a resync's: replaces everything. */
    | { type: 'loaded'; page: ItemPage }
    /** An older page: its items go first, skipping ids already held. */
    | { type: 'prepended'; page: ItemPage }
    | { type: 'upsert'; item: Item }
    | { type: 'removed'; id: string }

  export const EMPTY_ITEMS: ItemsState = { items: [], older: false, epoch: '', revision: 0 }

  /** Whether `value` has what the store reads of an item. */
  export function isItem(value: unknown): value is Item {
    if (!value || typeof value !== 'object') return false
    const v = value as Record<string, unknown>
    return (
      typeof v.id === 'string' &&
      typeof v.version === 'number' &&
      Number.isFinite(v.version) &&
      typeof v.ts === 'string' &&
      typeof v.kind === 'string' &&
      (v.turn_id === undefined || typeof v.turn_id === 'string')
    )
  }

  /** Whether `value` is a page the store can take. */
  export function isItemPage(value: unknown): value is ItemPage {
    if (!value || typeof value !== 'object') return false
    const v = value as Record<string, unknown>
    return (
      Array.isArray(v.items) &&
      typeof v.older === 'boolean' &&
      typeof v.epoch === 'string' &&
      typeof v.revision === 'number'
    )
  }

  const groupOf = (item: Item): string => item.turn_id ?? 'start'

  /** The items of `list` that are items, each id once (the first kept). */
  function distinct(list: readonly unknown[], held: ReadonlySet<string> = new Set()): Item[] {
    const seen = new Set(held)
    const out: Item[] = []
    for (const value of list) {
      if (isItem(value) && !seen.has(value.id)) {
        seen.add(value.id)
        out.push(value)
      }
    }
    return out
  }

  /** `before_turn` for the next older page: the first loaded item's turn.
   *  Absent when nothing is older, or the first group is the session's start. */
  export function beforeTurnOf(state: ItemsState): string | undefined {
    if (!state.older) return undefined
    return state.items[0]?.turn_id
  }

  /** Whether a new item of a group not loaded comes after every loaded one. */
  function isNewer(state: ItemsState, item: Item): boolean {
    if (!state.older || state.items.length === 0) return true
    // An unreadable time compares false: not newer.
    return Date.parse(item.ts) >= Date.parse(state.items[0].ts)
  }

  export function itemsReducer(state: ItemsState, action: ItemsAction): ItemsState {
    switch (action.type) {
      case 'loaded': {
        if (!isItemPage(action.page)) return state
        const { page } = action
        return { items: distinct(page.items), older: page.older, epoch: page.epoch, revision: page.revision }
      }
      case 'prepended': {
        if (!isItemPage(action.page)) return state
        const held = new Set(state.items.map((item) => item.id))
        const before = distinct(action.page.items, held)
        return { ...state, items: [...before, ...state.items], older: action.page.older }
      }
      case 'upsert': {
        const { item } = action
        if (!isItem(item)) return state
        const at = state.items.findIndex((held) => held.id === item.id)
        if (at >= 0) {
          if (item.version < state.items[at].version) return state
          const items = state.items.slice()
          items[at] = item
          return { ...state, items }
        }
        const group = groupOf(item)
        let last = -1
        for (let i = 0; i < state.items.length; i++) if (groupOf(state.items[i]) === group) last = i
        if (last >= 0) {
          const items = state.items.slice()
          items.splice(last + 1, 0, item)
          return { ...state, items }
        }
        if (isNewer(state, item)) return { ...state, items: [...state.items, item] }
        // A turn not loaded (4a-i O-6): its page brings it.
        return state
      }
      case 'removed': {
        if (typeof action.id !== 'string') return state
        const items = state.items.filter((item) => item.id !== action.id)
        return items.length === state.items.length ? state : { ...state, items }
      }
      default:
        return state
    }
  }
  ```

Create `web/src/store/sessionList.ts`:

  ```ts
  // The session list, as a pure reducer and the functions that read it
  // (plan 4c decision 5; frontend spec §5).
  //
  // - `all` holds every summary received, from pages and from the list
  //   stream, which sends every session of the owner's (O-5). The header
  //   reads it.
  // - The shown list is `all` filtered here by hat, lifecycle filter and
  //   Hide closed, newest `last_event_at` first, ties by id, highest first,
  //   as the server's pages are ordered (F-7).
  // - While a search is active the shown list is the server's result: an
  //   upsert updates a row already found and never adds one; only the hat
  //   applies (F-10).
  // - No event ever refetches (F-4): the stream's summaries are whole.
  // - `waiting` is the server's count, the hat's whatever the query: from
  //   the first page, then each `waiting_changed`; a further page never
  //   changes it.
  import type { SummaryQuery } from '../api/view'
  import type { SessionSummary, SummaryPage } from '../generated/view'

  /** The server's lifecycle names. */
  export const LIFECYCLES = ['starting', 'active', 'parked', 'closed', 'failed'] as const

  export interface ListFilters {
    /** A hat's id, or none for every hat. */
    hat?: string | null
    /** Only these lifecycles. When set, it alone decides: Hide closed applies
     *  only with no lifecycle filter (so picking `closed` shows closed). */
    lifecycle?: readonly string[]
    /** Hide `closed` sessions (never parked ones). */
    hideClosed: boolean
    /** A search over every lifecycle: the volume filters are bypassed. */
    q?: string
  }

  export interface ListState {
    all: Map<string, SessionSummary>
    /** While searching, the ids the server found; otherwise `null`. */
    found: Set<string> | null
    /** Where the next page starts; absent on the last one. */
    nextCursor?: string
    /** The first page's anchor: where the list stream resumes. */
    epoch: string
    revision: number
    /** The ids removed since this query's first page: a further page read
     *  before a removal never puts one back. A first page starts it over. */
    removed: ReadonlySet<string>
    /** The server's count of the hat's sessions waiting on a question,
     *  whatever the query: the first page's, then each `waiting_changed`'s.
     *  Absent when the first page had none: the loaded rows are counted. */
    waiting?: number
  }

  export type ListAction =
    /** The first page, or a resync's: replaces everything. */
    | { type: 'first'; page: SummaryPage; search: boolean }
    /** A further page: added; the anchor stays the first page's. */
    | { type: 'more'; page: SummaryPage }
    | { type: 'upsert'; summary: SessionSummary }
    | { type: 'removed'; id: string }
    /** `waiting_changed`: the server's new count. */
    | { type: 'waiting'; count: number }

  export const EMPTY_LIST: ListState = { all: new Map(), found: null, removed: new Set(), epoch: '', revision: 0 }

  export function isSummary(value: unknown): value is SessionSummary {
    if (!value || typeof value !== 'object') return false
    const v = value as Record<string, unknown>
    return (
      typeof v.session_id === 'string' &&
      typeof v.hat_id === 'string' &&
      typeof v.lifecycle === 'string' &&
      typeof v.last_event_at === 'string' &&
      typeof v.question_waits === 'boolean'
    )
  }

  export function isSummaryPage(value: unknown): value is SummaryPage {
    if (!value || typeof value !== 'object') return false
    const v = value as Record<string, unknown>
    return (
      Array.isArray(v.sessions) &&
      typeof v.epoch === 'string' &&
      typeof v.revision === 'number' &&
      (v.next_cursor === undefined || v.next_cursor === null || typeof v.next_cursor === 'string')
    )
  }

  /** A count the server could have sent: a whole number, never negative. */
  export function isWaitingCount(value: unknown): value is number {
    return typeof value === 'number' && Number.isInteger(value) && value >= 0
  }

  /** A first page: a page whose `waiting`, when there, is a count. A further
   *  page's `waiting` is never read, so it is never checked. */
  export function isFirstPage(value: unknown): value is SummaryPage {
    if (!isSummaryPage(value)) return false
    const waiting = (value as { waiting?: unknown }).waiting
    return waiting === undefined || isWaitingCount(waiting)
  }

  export function searching(filters: ListFilters): boolean {
    return (filters.q ?? '').trim() !== ''
  }

  /** The page query for `filters`: a search sends `q` and the hat only. */
  export function serverQuery(filters: ListFilters): SummaryQuery {
    const hat = filters.hat || undefined
    if (searching(filters)) return { q: filters.q!.trim(), hat }
    if (filters.lifecycle && filters.lifecycle.length > 0) return { hat, lifecycle: filters.lifecycle }
    if (filters.hideClosed) return { hat, lifecycle: LIFECYCLES.filter((name) => name !== 'closed') }
    return { hat }
  }

  /** Newest `last_event_at` first (a fixed RFC 3339 form: text order is time
   *  order), then by id, highest first: the server's order
   *  (`last_event_at DESC, id DESC`), so tied rows keep their place across
   *  pages. */
  export function compareSummaries(a: SessionSummary, b: SessionSummary): number {
    if (a.last_event_at !== b.last_event_at) return a.last_event_at < b.last_event_at ? 1 : -1
    return a.session_id < b.session_id ? 1 : a.session_id > b.session_id ? -1 : 0
  }

  const inHat = (s: SessionSummary, hat: string | null | undefined) => !hat || s.hat_id === hat

  export function isShown(state: ListState, s: SessionSummary, filters: ListFilters): boolean {
    if (!inHat(s, filters.hat)) return false
    if (state.found) return state.found.has(s.session_id)
    if (filters.lifecycle && filters.lifecycle.length > 0) return filters.lifecycle.includes(s.lifecycle)
    return !(filters.hideClosed && s.lifecycle === 'closed')
  }

  export function shownOf(state: ListState, filters: ListFilters): SessionSummary[] {
    return [...state.all.values()].filter((s) => isShown(state, s, filters)).sort(compareSummaries)
  }

  /** The sessions of the hat waiting on a question: the tab title's and the
   *  header badge's number. The server's count when it sent one
   *  (`state.waiting`, already the hat's: `state` is the hat's query's);
   *  otherwise the loaded rows'. */
  export function waitingCount(state: ListState, hat: string | null | undefined): number {
    if (state.waiting !== undefined) return state.waiting
    let n = 0
    for (const s of state.all.values()) {
      if (inHat(s, hat) && (s.activity === 'blocked' || s.question_waits)) n++
    }
    return n
  }

  /** `held` with a page's rows: a row the stream sent later is kept. */
  function merge(held: Map<string, SessionSummary>, rows: readonly unknown[]): { all: Map<string, SessionSummary>; ids: string[] } {
    const all = new Map(held)
    const ids: string[] = []
    for (const row of rows) {
      if (!isSummary(row)) continue
      ids.push(row.session_id)
      const before = all.get(row.session_id)
      // Strictly newer only: the stream's copy came after the page was read,
      // so it wins a tie (a title or `presumed_parked` changed, the time not).
      if (!before || row.last_event_at > before.last_event_at) all.set(row.session_id, row)
    }
    return { all, ids }
  }

  export function listReducer(state: ListState, action: ListAction): ListState {
    switch (action.type) {
      case 'first': {
        if (!isFirstPage(action.page)) return state
        const { all, ids } = merge(new Map(), action.page.sessions)
        return {
          all,
          found: action.search ? new Set(ids) : null,
          // A new query's pages hold what the server says.
          removed: new Set(),
          nextCursor: action.page.next_cursor ?? undefined,
          epoch: action.page.epoch,
          revision: action.page.revision,
          // The new page's, or none: never the page's before.
          waiting: action.page.waiting,
        }
      }
      case 'more': {
        if (!isSummaryPage(action.page)) return state
        // A row read before its session was removed is never put back.
        const rows = action.page.sessions.filter((row) => !(isSummary(row) && state.removed.has(row.session_id)))
        const { all, ids } = merge(state.all, rows)
        const found = state.found ? new Set([...state.found, ...ids]) : null
        return { ...state, all, found, nextCursor: action.page.next_cursor ?? undefined }
      }
      case 'upsert': {
        if (!isSummary(action.summary)) return state
        const all = new Map(state.all)
        all.set(action.summary.session_id, action.summary)
        return { ...state, all }
      }
      case 'removed': {
        if (typeof action.id !== 'string') return state
        const held = state.all.has(action.id)
        if (!held && state.removed.has(action.id)) return state
        // Out of `all` is out of the shown list, searching or not. Kept in
        // `removed` even when not held: a further page in flight, read
        // before the removal, may still carry it.
        let all = state.all
        if (held) {
          all = new Map(state.all)
          all.delete(action.id)
        }
        return { ...state, all, removed: new Set(state.removed).add(action.id) }
      }
      case 'waiting': {
        if (!isWaitingCount(action.count) || action.count === state.waiting) return state
        return { ...state, waiting: action.count }
      }
      default:
        return state
    }
  }
  ```

Create `web/src/store/useSessionItems.ts`:

  ```ts
  // One session's items, fed by its stream (plan 4c decision 4).
  //
  // - Open: the first page (8 groups under 768 px, else the server's
  //   default) and the catalogue, then the item stream from the page's
  //   anchor, `<epoch>:<revision>`.
  // - `item` upserts, `item_removed` removes, `catalog_changed` replaces the
  //   catalogue.
  // - A resync (`resync_required`, or any message that does not parse):
  //   close the stream at once, refetch the first page and the catalogue,
  //   REPLACE the store (older pages dropped), and open a new stream from the
  //   new anchor. "Resynced" shows for a moment. A resync that follows
  //   another waits before refetching, longer each time (`ResyncBackoff`).
  // - `session_removed`, or a 404 from the page or the stream: the stream
  //   closes and the session is marked removed.
  // - A page that fails otherwise is fetched again, waiting longer each time.
  import { useEffect, useMemo, useRef, useSyncExternalStore } from 'react'
  import { useClient } from '../app-client'
  import type { Client } from '../api/client'
  import { ApiFailure, Unauthenticated, messageOf } from '../api/errors'
  import { openStream, type Stream, type StreamEvent, type StreamState } from '../api/sse'
  import { anchorOf, catalog as fetchCatalog, itemPage, itemStreamPath } from '../api/view'
  import type { SessionCatalog } from '../generated/protocol'
  import type { Item } from '../generated/view'
  import { useMediaQuery } from '../hooks/useMediaQuery'
  import { EMPTY_ITEMS, beforeTurnOf, isItem, isItemPage, itemsReducer, type ItemsAction, type ItemsState } from './items'

  /** Groups on a phone's first page (O-7). */
  export const NARROW_PAGE_GROUPS = 8
  const NARROW = '(max-width: 767px)'

  export interface Timing {
    /** The wait before fetching a failed page again, by attempt (0, 1, …). */
    retryMs: (attempt: number) => number
    /** How long "Resynced" shows. */
    resyncedMs: number
    /** How long a stream stays open with no resync before the next resync
     *  is a first one again (`ResyncBackoff`). */
    settleMs: number
  }

  const DEFAULT_TIMING: Timing = {
    retryMs: (attempt) => Math.min(1000 * 2 ** attempt, 30000),
    resyncedMs: 3000,
    settleMs: 30000,
  }

  /**
   * The wait before a resync's refetch. The first resync refetches at once;
   * each one after it waits `retryMs(0)`, `retryMs(1)` … (1 s, 2 s, 4 s …,
   * at most 30 s), so a server that keeps sending a message this client
   * refuses cannot make it refetch and reopen in a loop.
   *
   * The run of resyncs ends once a stream has stayed open for `settleMs`
   * with no resync. Neither of the obvious signals bounds the loop:
   * - "the stream opened": it opens before the bad message comes, so every
   *   resync of the loop would look like a first one;
   * - "a well-formed message came": a resume burst may bring good messages
   *   before the bad one (the list's `waiting_changed` ends every burst),
   *   so again every resync would look like a first one.
   * A stream that stayed open long enough has had its burst, so a resync
   * after it is a new one, and refetches at once.
   */
  export class ResyncBackoff {
    private resyncs = 0
    private openedAt: number | undefined
    private readonly timing: Timing

    constructor(timing: Timing) {
      this.timing = timing
    }

    /** A new start (`start` after `stop`, as when a hidden view is shown
     *  again): no resync before it. */
    reset(): void {
      this.resyncs = 0
      this.openedAt = undefined
    }

    /** The stream opened (or reopened by itself). */
    opened(): void {
      this.openedAt = Date.now()
    }

    /** A resync: how long to wait before refetching. */
    next(): number {
      if (this.openedAt !== undefined && Date.now() - this.openedAt >= this.timing.settleMs) this.resyncs = 0
      const attempt = this.resyncs++
      return attempt === 0 ? 0 : this.timing.retryMs(attempt - 1)
    }
  }

  export interface SessionItemsSnapshot {
    store: ItemsState
    /** The first page has not come yet. */
    loading: boolean
    loadingOlder: boolean
    /** The session was deleted, or is not the operator's: nothing more comes. */
    removed: boolean
    error: string | null
    stream: StreamState
    /** A resync replaced the items a moment ago. */
    resynced: boolean
    catalog: SessionCatalog | null
  }

  const INITIAL: SessionItemsSnapshot = {
    store: EMPTY_ITEMS,
    loading: true,
    loadingOlder: false,
    removed: false,
    error: null,
    stream: 'connecting',
    resynced: false,
    catalog: null,
  }

  /** `JSON.parse`, or `undefined` for text that is not JSON. */
  function parse(text: string): unknown {
    try {
      return JSON.parse(text)
    } catch {
      return undefined
    }
  }

  function isCatalog(value: unknown): value is SessionCatalog {
    if (!value || typeof value !== 'object') return false
    const v = value as Record<string, unknown>
    return typeof v.session_id === 'string' && Array.isArray(v.config_options) && Array.isArray(v.commands)
  }

  export class SessionItemsController {
    private snapshot: SessionItemsSnapshot = INITIAL
    private readonly listeners = new Set<() => void>()
    /** Bumped by `start`, `stop`, a resync and `gone`: a continuation of
     *  another run is dropped. */
    private run = 0
    /** Bumped by each first page: an older page of the one before is dropped. */
    private generation = 0
    /** Bumped by each catalogue from the stream or a config answer: a fetch
     *  begun before it is older, and dropped. */
    private catalogSeq = 0
    private stream: Stream | null = null
    private attempt = 0
    private retryTimer: ReturnType<typeof setTimeout> | undefined
    private resyncedTimer: ReturnType<typeof setTimeout> | undefined
    private readonly client: Client
    private readonly id: string
    private readonly limit: () => number | undefined
    private readonly timing: Timing
    private readonly backoff: ResyncBackoff

    constructor(client: Client, id: string, limit: () => number | undefined, timing: Partial<Timing> = {}) {
      this.client = client
      this.id = id
      this.limit = limit
      this.timing = { ...DEFAULT_TIMING, ...timing }
      this.backoff = new ResyncBackoff(this.timing)
    }

    subscribe = (listener: () => void): (() => void) => {
      this.listeners.add(listener)
      return () => this.listeners.delete(listener)
    }

    getSnapshot = (): SessionItemsSnapshot => this.snapshot

    start(): void {
      this.stop()
      this.snapshot = INITIAL
      this.attempt = 0
      this.backoff.reset()
      this.notify()
      void this.load(this.run, false)
    }

    stop(): void {
      this.run++
      this.closeStream()
      clearTimeout(this.retryTimer)
      clearTimeout(this.resyncedTimer)
    }

    /** The groups before the first loaded one, prepended. */
    loadOlder = async (): Promise<void> => {
      const before = beforeTurnOf(this.snapshot.store)
      if (before === undefined || this.snapshot.loadingOlder || this.snapshot.removed) return
      const run = this.run
      const generation = this.generation
      this.update({ loadingOlder: true })
      try {
        const page = await itemPage(this.client, this.id, { beforeTurn: before, limit: this.limit() })
        if (run !== this.run || generation !== this.generation) return
        this.apply({ type: 'prepended', page })
      } catch (err) {
        if (run !== this.run || generation !== this.generation) return
        if (err instanceof ApiFailure && err.status === 404) return this.gone()
        // The turn is no longer this session's first loaded one: start over.
        if (err instanceof ApiFailure && err.status === 400) return this.resync()
        if (!(err instanceof Unauthenticated)) this.update({ error: messageOf(err) })
      } finally {
        if (run === this.run) this.update({ loadingOlder: false })
      }
    }

    /** A catalogue from a config answer (202) replaces the one held. */
    setCatalog = (catalog: SessionCatalog): void => {
      this.catalogSeq++
      this.update({ catalog })
    }

    private notify(): void {
      for (const listener of this.listeners) listener()
    }

    private update(patch: Partial<SessionItemsSnapshot>): void {
      this.snapshot = { ...this.snapshot, ...patch }
      this.notify()
    }

    private apply(action: ItemsAction): void {
      const store = itemsReducer(this.snapshot.store, action)
      if (store !== this.snapshot.store) this.update({ store })
    }

    private closeStream(): void {
      this.stream?.close()
      this.stream = null
    }

    private async loadCatalog(run: number): Promise<void> {
      const seq = this.catalogSeq
      try {
        const catalog = await fetchCatalog(this.client, this.id)
        if (run === this.run && seq === this.catalogSeq && isCatalog(catalog)) this.update({ catalog })
      } catch {
        // The page's own answer says what is wrong.
      }
    }

    private async load(run: number, resync: boolean): Promise<void> {
      // Every (re)load: a gap may have hidden a `catalog_changed`.
      void this.loadCatalog(run)
      try {
        const page = await itemPage(this.client, this.id, { limit: this.limit() })
        if (run !== this.run) return
        if (!isItemPage(page)) throw new Error('The session’s items could not be read.')
        this.generation++
        this.attempt = 0
        this.update({
          store: itemsReducer(this.snapshot.store, { type: 'loaded', page }),
          loading: false,
          error: null,
          resynced: resync || this.snapshot.resynced,
        })
        if (resync) {
          clearTimeout(this.resyncedTimer)
          this.resyncedTimer = setTimeout(() => this.update({ resynced: false }), this.timing.resyncedMs)
        }
        this.open(anchorOf(page))
      } catch (err) {
        if (run !== this.run) return
        if (err instanceof ApiFailure && err.status === 404) return this.gone()
        if (err instanceof Unauthenticated) return this.update({ stream: 'closed' })
        if (err instanceof ApiFailure && err.status >= 400 && err.status < 500) {
          return this.update({ error: messageOf(err), loading: false, stream: 'closed' })
        }
        this.update({ error: messageOf(err), stream: 'reconnecting' })
        this.retryTimer = setTimeout(() => void this.load(run, resync), this.timing.retryMs(this.attempt++))
      }
    }

    private open(anchor: string): void {
      // One stream at a time, whatever opened the one before.
      this.closeStream()
      this.stream = openStream(this.client, itemStreamPath(this.id), {
        lastEventId: anchor,
        // Nothing comes after `close()`: the helper drops it.
        onState: (stream) => {
          if (stream === 'open') this.backoff.opened()
          this.update({ stream })
        },
        onEvent: (event) => this.onEvent(event),
        onResync: () => this.resync(),
        onError: (status) => {
          if (status === 404) return this.gone()
          this.update({ error: `The session’s updates stopped (${status}).`, stream: 'closed' })
        },
      })
    }

    private onEvent(event: StreamEvent): void {
      switch (event.event) {
        case 'item': {
          const item = parse(event.data)
          if (!isItem(item)) return this.resync()
          return this.apply({ type: 'upsert', item: item as Item })
        }
        case 'item_removed': {
          const removed = parse(event.data) as { id?: unknown } | undefined
          if (!removed || typeof removed !== 'object' || typeof removed.id !== 'string') return this.resync()
          return this.apply({ type: 'removed', id: removed.id })
        }
        case 'session_removed':
          return this.gone()
        case 'catalog_changed': {
          const catalog = parse(event.data)
          if (!isCatalog(catalog)) return this.resync()
          return this.setCatalog(catalog)
        }
        default:
          // A message this client does not know: a newer server's, ignored.
          return
      }
    }

    /** Close now (the server ends the stream after `resync_required`), then
     *  refetch, replace and reopen from the new anchor, at once or after
     *  `ResyncBackoff`'s wait. A new run: whatever was in flight (an older
     *  page, a load, a retry, a wait) is dropped, so it can neither resync
     *  again nor open a second stream. */
    private resync(): void {
      this.run++
      this.closeStream()
      clearTimeout(this.retryTimer)
      this.update({ stream: 'reconnecting', loadingOlder: false })
      const run = this.run
      const wait = this.backoff.next()
      if (wait === 0) void this.load(run, true)
      // `stop`, `gone` and the next resync clear it, and bump the run.
      else this.retryTimer = setTimeout(() => void this.load(run, true), wait)
    }

    /** A new run too: a load in flight cannot reopen a removed session. */
    private gone(): void {
      this.run++
      this.closeStream()
      clearTimeout(this.retryTimer)
      this.update({ removed: true, loading: false, loadingOlder: false, stream: 'closed' })
    }
  }

  export interface SessionItems extends Omit<SessionItemsSnapshot, 'store'> {
    items: Item[]
    /** Groups exist before the first loaded one. */
    older: boolean
    loadOlder: () => Promise<void>
    setCatalog: (catalog: SessionCatalog) => void
  }

  export function useSessionItems(id: string, timing?: Partial<Timing>): SessionItems {
    const client = useClient()
    const narrow = useMediaQuery(NARROW)
    // Read when a page is fetched: a resize does not reload the session.
    const narrowRef = useRef(narrow)
    narrowRef.current = narrow
    const timingRef = useRef(timing)
    const controller = useMemo(
      () =>
        new SessionItemsController(client, id, () => (narrowRef.current ? NARROW_PAGE_GROUPS : undefined), timingRef.current),
      [client, id],
    )
    useEffect(() => {
      controller.start()
      return () => controller.stop()
    }, [controller])
    const snapshot = useSyncExternalStore(controller.subscribe, controller.getSnapshot)
    const { store, ...rest } = snapshot
    return { ...rest, items: store.items, older: store.older, loadOlder: controller.loadOlder, setCatalog: controller.setCatalog }
  }
  ```

Create `web/src/store/useSessionList.ts`:

  ```ts
  // The session list, fed by the list stream (plan 4c decision 5).
  //
  // - Seeded from the first page of `GET /api/view/sessions` for the
  //   filters' query; further pages by `next_cursor` (`loadMore`).
  // - The list stream resumes from the FIRST page's anchor only: a later
  //   page never moves it.
  // - `session_upsert`, `session_removed` and `waiting_changed` change the
  //   store and nothing else: no event ever refetches (F-4). A repeated
  //   `session_removed` (the server may send one twice) is a keyed delete,
  //   so harmless.
  // - The stream is opened with the query's hat (never an empty one): it
  //   scopes `waiting_changed`'s count only. A new hat is a new query, so a
  //   new page and a new stream.
  // - A resync (`resync_required`, or a message that does not parse): close
  //   the stream at once, refetch the first page with the current query,
  //   replace the store and reopen from the new anchor. A resync that
  //   follows another waits first, longer each time (`ResyncBackoff`).
  // - A change of the query (hat, lifecycle filter, Hide closed, search)
  //   starts over the same way.
  import { useEffect, useMemo, useRef, useSyncExternalStore } from 'react'
  import { useClient } from '../app-client'
  import type { Client } from '../api/client'
  import { ApiFailure, Unauthenticated, messageOf } from '../api/errors'
  import { openStream, type Stream, type StreamEvent, type StreamState } from '../api/sse'
  import { anchorOf, listStreamPath, summaries, type SummaryQuery } from '../api/view'
  import type { SessionSummary } from '../generated/view'
  import {
    EMPTY_LIST,
    isFirstPage,
    isSummary,
    isWaitingCount,
    listReducer,
    searching,
    serverQuery,
    shownOf,
    waitingCount,
    type ListAction,
    type ListFilters,
    type ListState,
  } from './sessionList'
  import { ResyncBackoff, type Timing } from './useSessionItems'

  const DEFAULT_TIMING: Timing = {
    retryMs: (attempt) => Math.min(1000 * 2 ** attempt, 30000),
    resyncedMs: 3000,
    settleMs: 30000,
  }

  export interface SessionListSnapshot {
    store: ListState
    loading: boolean
    loadingMore: boolean
    error: string | null
    stream: StreamState
    resynced: boolean
  }

  const INITIAL: SessionListSnapshot = {
    store: EMPTY_LIST,
    loading: true,
    loadingMore: false,
    error: null,
    stream: 'connecting',
    resynced: false,
  }

  function parse(text: string): unknown {
    try {
      return JSON.parse(text)
    } catch {
      return undefined
    }
  }

  export class SessionListController {
    private snapshot: SessionListSnapshot = INITIAL
    private readonly listeners = new Set<() => void>()
    private run = 0
    private generation = 0
    private stream: Stream | null = null
    private attempt = 0
    private retryTimer: ReturnType<typeof setTimeout> | undefined
    private resyncedTimer: ReturnType<typeof setTimeout> | undefined
    private readonly client: Client
    private readonly query: SummaryQuery
    private readonly search: boolean
    private readonly timing: Timing
    private readonly backoff: ResyncBackoff

    constructor(client: Client, query: SummaryQuery, timing: Partial<Timing> = {}) {
      this.client = client
      this.query = query
      this.search = query.q !== undefined && query.q !== ''
      this.timing = { ...DEFAULT_TIMING, ...timing }
      this.backoff = new ResyncBackoff(this.timing)
    }

    subscribe = (listener: () => void): (() => void) => {
      this.listeners.add(listener)
      return () => this.listeners.delete(listener)
    }

    getSnapshot = (): SessionListSnapshot => this.snapshot

    start(): void {
      this.stop()
      this.snapshot = INITIAL
      this.attempt = 0
      this.backoff.reset()
      this.notify()
      void this.load(this.run, false)
    }

    stop(): void {
      this.run++
      this.closeStream()
      clearTimeout(this.retryTimer)
      clearTimeout(this.resyncedTimer)
    }

    /** The next page, by `next_cursor`. */
    loadMore = async (): Promise<void> => {
      const cursor = this.snapshot.store.nextCursor
      if (cursor === undefined || this.snapshot.loadingMore) return
      const run = this.run
      const generation = this.generation
      this.update({ loadingMore: true })
      try {
        const page = await summaries(this.client, { ...this.query, cursor })
        if (run !== this.run || generation !== this.generation) return
        this.apply({ type: 'more', page })
      } catch (err) {
        if (run !== this.run || generation !== this.generation) return
        if (!(err instanceof Unauthenticated)) this.update({ error: messageOf(err) })
      } finally {
        if (run === this.run) this.update({ loadingMore: false })
      }
    }

    private notify(): void {
      for (const listener of this.listeners) listener()
    }

    private update(patch: Partial<SessionListSnapshot>): void {
      this.snapshot = { ...this.snapshot, ...patch }
      this.notify()
    }

    private apply(action: ListAction): void {
      const store = listReducer(this.snapshot.store, action)
      if (store !== this.snapshot.store) this.update({ store })
    }

    private closeStream(): void {
      this.stream?.close()
      this.stream = null
    }

    private async load(run: number, resync: boolean): Promise<void> {
      try {
        const page = await summaries(this.client, this.query)
        if (run !== this.run) return
        if (!isFirstPage(page)) throw new Error('The session list could not be read.')
        this.generation++
        this.attempt = 0
        this.update({
          store: listReducer(this.snapshot.store, { type: 'first', page, search: this.search }),
          loading: false,
          error: null,
          resynced: resync || this.snapshot.resynced,
        })
        if (resync) {
          clearTimeout(this.resyncedTimer)
          this.resyncedTimer = setTimeout(() => this.update({ resynced: false }), this.timing.resyncedMs)
        }
        this.open(anchorOf(page))
      } catch (err) {
        if (run !== this.run) return
        if (err instanceof Unauthenticated) return this.update({ stream: 'closed' })
        if (err instanceof ApiFailure && err.status >= 400 && err.status < 500) {
          return this.update({ error: messageOf(err), loading: false, stream: 'closed' })
        }
        this.update({ error: messageOf(err), stream: 'reconnecting' })
        this.retryTimer = setTimeout(() => void this.load(run, resync), this.timing.retryMs(this.attempt++))
      }
    }

    private open(anchor: string): void {
      // One stream at a time, whatever opened the one before.
      this.closeStream()
      this.stream = openStream(this.client, listStreamPath(this.query.hat), {
        lastEventId: anchor,
        // Nothing comes after `close()`: the helper drops it.
        onState: (stream) => {
          if (stream === 'open') this.backoff.opened()
          this.update({ stream })
        },
        onEvent: (event) => this.onEvent(event),
        onResync: () => this.resync(),
        onError: (status) => this.update({ error: `The session list’s updates stopped (${status}).`, stream: 'closed' }),
      })
    }

    private onEvent(event: StreamEvent): void {
      switch (event.event) {
        case 'session_upsert': {
          const summary = parse(event.data)
          if (!isSummary(summary)) return this.resync()
          return this.apply({ type: 'upsert', summary })
        }
        case 'session_removed': {
          const removed = parse(event.data) as { session_id?: unknown } | undefined
          if (!removed || typeof removed !== 'object' || typeof removed.session_id !== 'string') return this.resync()
          return this.apply({ type: 'removed', id: removed.session_id })
        }
        case 'waiting_changed': {
          const changed = parse(event.data) as { count?: unknown } | undefined
          if (!changed || typeof changed !== 'object' || !isWaitingCount(changed.count)) return this.resync()
          return this.apply({ type: 'waiting', count: changed.count })
        }
        default:
          return
      }
    }

    /** A new run, as in the items' store: a further page, a refetch or a
     *  wait in flight is dropped, and "load more" is free again. The refetch
     *  waits as `ResyncBackoff` says. */
    private resync(): void {
      this.run++
      this.closeStream()
      clearTimeout(this.retryTimer)
      this.update({ stream: 'reconnecting', loadingMore: false })
      const run = this.run
      const wait = this.backoff.next()
      if (wait === 0) void this.load(run, true)
      // `stop` and the next resync clear it, and bump the run.
      else this.retryTimer = setTimeout(() => void this.load(run, true), wait)
    }
  }

  export interface SessionList extends Omit<SessionListSnapshot, 'store'> {
    /** Every summary received, by id (the header reads it). */
    all: ReadonlyMap<string, SessionSummary>
    /** What the list shows, in order. */
    shown: SessionSummary[]
    /** `waiting`: the sessions of the hat waiting on a question (blocked, or
     *  a question open): the tab title's and the header badge's number. */
    counts: { waiting: number }
    /** More pages exist. */
    hasMore: boolean
    loadMore: () => Promise<void>
    searching: boolean
  }

  export function useSessionList(filters: ListFilters, timing?: Partial<Timing>): SessionList {
    const client = useClient()
    const query = serverQuery(filters)
    const key = JSON.stringify(query)
    const timingRef = useRef(timing)
    // A new query starts over; a new object with the same query does not.
    const controller = useMemo(() => new SessionListController(client, query, timingRef.current), [client, key])
    useEffect(() => {
      controller.start()
      return () => controller.stop()
    }, [controller])
    const snapshot = useSyncExternalStore(controller.subscribe, controller.getSnapshot)
    const { store, ...rest } = snapshot
    const { hat, lifecycle, hideClosed, q } = filters
    const shown = useMemo(
      () => shownOf(store, { hat, lifecycle, hideClosed, q }),
        [store, hat, hideClosed, q, lifecycle?.join(',')],
    )
    const waiting = useMemo(() => waitingCount(store, hat), [store, hat])
    const counts = useMemo(() => ({ waiting }), [waiting])
    return {
      ...rest,
      all: store.all,
      shown,
      counts,
      hasMore: store.nextCursor !== undefined,
      loadMore: controller.loadMore,
      searching: searching(filters),
    }
  }
  ```

- [ ] **Step 4: Run the checks**

Run: `nix develop -c sh -c 'cd web && pnpm typecheck && pnpm test && pnpm build'`
Expected: PASS, 436 tests (307 before).

- [ ] **Step 5: Revert-probes** (each must fail the test named; restore after each)

106 probes, run by script, all fail as they should. Two earlier survivors were fixed before the final run: a redundant `found.delete` on `session_removed` was removed (the rows read `all`), and the default-wait test asserts the timer count synchronously after `close()` (`vi.waitFor` advanced the fake timers).
- **The helper** (13): the abort listener removed per connection; `close()` reports `closed` at once; the default wait ends on close; an unread body cancelled; closed while connecting reports nothing; an id-only block keeps its id; an empty id clears it; a throwing handler stops the burst and asks for a resync; nothing dispatched after a close inside a handler; a lone CR at the end ends a line; an id with NUL ignored; the wait reset on open.
- **The item reducer** (12): versions never go back; replace in place; a new id joins its loaded group; the `start` group key; nothing older: a new group at the end; newer by `ts` (`>=`); an item of a turn not loaded ignored; `before_turn` only while `older`; a prepend skips held ids; a malformed item ignored; removed by id; a loaded page replaces everything.
- **The item hook** (30): the catalogue fetched on open; the stream from the page's anchor; 8 groups on a phone; upserts and removals applied; "Reconnecting…"; a resync closes at once, resyncs on `resync_required`, sets and ends "Resynced"; a malformed `item`, `item_removed` or `catalog_changed` resyncs; `session_removed`, a page 404 and a stream 404 mark it removed and close; another refusal is an error; `catalog_changed` and `setCatalog` replace the catalogue, and a stale catalogue fetch is dropped; an older page after a resync is dropped; nothing older, an older page's 404 (removed) and 400 (resync); a failed or unreadable page fetched again with a growing wait, a refused one not; `stop` closes the stream.
- **The list reducer** (21): Hide closed as lifecycles; the lifecycle filter alone decides the query; a search sends `q` and the hat only; an empty hat not sent; newest first, ties by id; the hat filter; a search shows found rows only; the lifecycle filter on rows shown; Hide closed hides closed only; upserts insert and update; a page keeps a newer streamed row; a search's first and further pages; removed from `all`; a further page keeps the first anchor; a first page replaces everything; a malformed summary ignored; the count's blocked, `question_waits` and hat scope.
- **The list hook** (19): the stream from the first anchor; upserts and removals applied; a resync closes at once, resyncs, and ends "Resynced"; malformed upserts and removals resync; search mode; **no event ever fetches** (F-4); the same query keeps its controller; a further page after a resync dropped; further pages use the cursor; a refused page not fetched again; a stream refusal is an error; a failed or unreadable page fetched again; the retry wait reset; the count in the hook.
- **The view client** (11): every id encoded (page, undelivered turn's session and turn, catalogue, detail, stream path); `before_turn` and `limit` sent; empty search, hat and lifecycle not sent; the anchor's form.

Load: 4 parallel copies of `vitest run src/store src/api`, 3 rounds: 12 of 12 green.


**Re-run on the final code** (after the rebase onto `main` and the re-confirmation's amendments, 2026-10-03): the probes above again, the server count's 35 and the amendments' 32 (Tasks 2 and 3 together). Each probe whose line had moved was retargeted, or superseded by a probe of the same guard; two guards no test caught got a new assertion (a resync closes the old stream before its wait; the 20-character boundary of Task 5's guard). Two of the list store's defence lines survive every test and are kept as defence in depth: the new run in `resync()` and the close in `open()` (review O1), because the list never has a stream open while a load runs. The task review's fixes added five more, all failing as they should: rows tied on `last_event_at` ordered by id, newest first, as the server orders them; a further page never overwriting a stream's row on a tie; a session removed while a further page was on its way never coming back with it.

- [ ] **Step 6: Commit**

```bash
git add web/src
git commit -m "feat(web): a typed view client, an SSE helper that never loses its place, and item and session-list stores fed by their streams"
```

---

### Task 3: The session list

The list in the rail from 768 px and as the `/sessions` screen below it, with the hat, the filters, search, the selection and the waiting count (decisions 6–12).

**Files:**
- Create: `web/src/components/SessionList.tsx`, `SessionRow.tsx`, `SessionScope.tsx`, `HatSwitch.tsx`; `web/src/lib/status.ts`, `hats.ts`, `agent.ts`; and their tests.
- Modify: `web/src/components/Shell.tsx` (the list in `.rail-scroll`, the screen under 768 px, the badges), `web/src/index.css` (the list's rules), `web/src/store/sessionList.ts` (`waitingCount`, the seam for the server's count).

- [ ] **Step 1: Write the tests**

Test files: `web/src/components/SessionList.test.tsx`, `web/src/components/SessionRow.test.tsx`, `web/src/hooks/useNow.test.ts`, `web/src/lib/agent.test.ts`, `web/src/lib/hats.test.ts`, `web/src/lib/status.test.ts`, `web/src/lib/time.test.ts`, `web/src/screens/Hosts.test.tsx`, `web/src/store/sessionList.test.ts`, `web/src/store/useSessionList.test.ts`.

Create `web/src/components/SessionList.test.tsx`:

  ```tsx
  // The session list in the app: day headings, search, filters, hats,
  // selection (F-10, F-11), the waiting count and the host names.
  import { act, render, screen, waitFor, within } from '@testing-library/react'
  import userEvent from '@testing-library/user-event'
  import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
  import App from '../App'
  import type { HatItem, HostItem } from '../generated/protocol'
  import type { SessionSummary, SummaryPage } from '../generated/view'
  import { THEME_KEY } from '../lib/theme'
  import { json, liveStream, type LiveStream } from '../test-stream'
  import { PAGE_ROWS, byDay } from './SessionList'
  import { navigate } from '../router'
  import { host } from '../test-fixtures'
  import { DESKTOP } from './Shell'

  const LIST = '/api/view/sessions'
  const STREAM = '/api/stream/sessions'

  function summary(id: string, patch: Partial<SessionSummary> = {}): SessionSummary {
    return {
      session_id: id,
      host_id: 'h1',
      agent: 'claude',
      cwd: `/srv/work/${id}`,
      hat_id: 'hat-a',
      lifecycle: 'active',
      activity: 'idle',
      presumed_parked: false,
      created_at: '2026-10-01T00:00:00.000Z',
      last_event_at: '2026-10-02T10:00:00.000Z',
      question_waits: false,
      ...patch,
    }
  }

  /** A page; with no `waiting`, a server that does not count (the rows are
   *  counted instead: the fallback). */
  const page = (sessions: SessionSummary[], next?: string, waiting?: number): SummaryPage =>
    ({ sessions, next_cursor: next, epoch: 'e1', revision: 1, ...(waiting === undefined ? {} : { waiting }) }) as SummaryPage

  function hat(id: string, name: string, colour: string): HatItem {
    return { id, name, colour, created_at: '2026-10-01T00:00:00Z', default_for_new_hosts: false, purging: false }
  }

  const HATS = [hat('hat-a', 'Work', '#112233'), hat('hat-b', 'Home', 'not-a-colour')]
  const HOSTS = [{ host_id: 'h1', name: 'laptop' } as HostItem]

  interface Options {
    /** The list page for a query (the search params as a string), or the
     *  server's whole answer. */
    list?: (params: URLSearchParams) => SummaryPage | Response
    hats?: HatItem[] | null
    /** Read at each request: a test may change it. */
    hosts?: HostItem[]
  }

  function server({ list = () => page([]), hats = HATS, hosts = HOSTS }: Options = {}) {
    const calls: URL[] = []
    const streams: LiveStream[] = []
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(String(input), 'http://h')
      calls.push(url)
      switch (url.pathname) {
        case '/api/capabilities':
          return json({ mode: 'full', features: [] })
        case '/api/hats':
          return hats ? json(hats) : json({ code: 'internal', message: 'no' }, 500)
        case '/api/hosts':
          return json(hosts)
        case LIST: {
          const answer = list(url.searchParams)
          return answer instanceof Response ? answer : json(answer)
        }
        case STREAM: {
          const live = liveStream()
          streams.push(live)
          return live.response
        }
        default:
          return json({ code: 'not_found', message: 'no' }, 404)
      }
    })
    const of = (path: string) => calls.filter((u) => u.pathname === path)
    return { fetch: fetch as unknown as typeof globalThis.fetch, calls, streams, of }
  }

  function width(desktop: boolean) {
    window.matchMedia = ((query: string) => ({
      matches: query === DESKTOP && desktop,
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    })) as unknown as typeof window.matchMedia
  }

  function at(path: string) {
    history.replaceState(null, '', path)
  }

  const names = (scope: HTMLElement) =>
    within(scope)
      .queryAllByRole('link')
      .filter((a) => a.classList.contains('sess'))
      .map((a) => a.querySelector('.sess-name')?.textContent)

  async function list() {
    return screen.findByRole('region', { name: 'Session list' })
  }

  beforeEach(() => {
    localStorage.clear()
    document.title = 'hennery'
    width(false)
  })

  afterEach(() => {
    at('/')
    // @ts-expect-error jsdom has none; each test sets its own
    delete window.matchMedia
    vi.useRealTimers()
  })

  describe('day headings', () => {
    it('are headings, in order, from calendar-local midnights', async () => {
      vi.useFakeTimers({ toFake: ['Date'] })
      vi.setSystemTime(new Date(2026, 9, 2, 12, 0))
      const local = (d: number, h = 10) => new Date(2026, 9, d, h, 0).toISOString()
      at('/sessions')
      const s = server({
        list: () =>
          page([
            summary('today', { last_event_at: local(2, 0) }),
            summary('yesterday', { last_event_at: local(1, 23) }),
            summary('week', { last_event_at: new Date(2026, 8, 29, 10).toISOString() }),
            summary('earlier', { last_event_at: '2026-06-01T10:00:00.000Z' }),
            summary('month', { last_event_at: new Date(2026, 8, 20, 10).toISOString() }),
            summary('broken', { last_event_at: 'not a time' }),
          ]),
      })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(within(region).getAllByRole('heading', { level: 2 }).length).toBeGreaterThan(0))
      const headings = within(region)
        .getAllByRole('heading', { level: 2 })
        .map((h) => h.querySelector('.grp-title')?.textContent)
      expect(headings).toEqual(['Today', 'Yesterday', 'This week', 'This month', 'Earlier'])
    })

    it('move on with the clock on an idle list: across midnight Today becomes Yesterday, and 29m ago 31m ago', async () => {
      // No list event and no fetch after the first page: only the clock moves.
      vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'clearInterval'] })
      vi.setSystemTime(new Date(2026, 9, 2, 23, 59))
      at('/sessions')
      const s = server({ list: () => page([summary('late', { last_event_at: new Date(2026, 9, 2, 23, 30).toISOString() })]) })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(names(region)).toEqual(['late']))
      const headings = () =>
        within(region)
          .getAllByRole('heading', { level: 2 })
          .map((h) => h.querySelector('.grp-title')?.textContent)
      const time = () => region.querySelector('.sess-time')?.textContent
      expect(headings()).toEqual(['Today'])
      expect(time()).toBe('29m ago')
      act(() => vi.advanceTimersByTime(2 * 60_000))
      expect(headings()).toEqual(['Yesterday'])
      expect(time()).toBe('31m ago')
      expect(s.of(LIST)).toHaveLength(1)
    })

    it('put an empty or unparseable time under Earlier, and a future one under Today', () => {
      const now = new Date(2026, 9, 2, 12, 0).getTime()
      const days = byDay(
        [
          summary('future', { last_event_at: new Date(2026, 9, 5, 9, 0).toISOString() }),
          summary('empty', { last_event_at: '' }),
          summary('broken', { last_event_at: 'not a time' }),
        ],
        now,
      )
      expect(days.map(([b, rows]) => [b, rows.map((s) => s.session_id)])).toEqual([
        ['today', ['future']],
        ['earlier', ['empty', 'broken']],
      ])
    })

    it('put a session from before a 23 h day under Yesterday', () => {
      // Europe/Warsaw (vite.config.ts): 2026-03-29 has 23 hours.
      const now = new Date(2026, 2, 30, 0, 30).getTime()
      const days = byDay([summary('a', { last_event_at: new Date(2026, 2, 29, 0, 10).toISOString() })], now)
      expect(days.map(([b]) => b)).toEqual(['yesterday'])
    })
  })

  describe('filters', () => {
    it('Hide closed is on by default and never hides a parked session', async () => {
      at('/sessions')
      const s = server({
        list: () =>
          page([
            summary('live'),
            summary('parked', { lifecycle: 'parked', activity: undefined }),
            summary('closed', { lifecycle: 'closed', activity: undefined }),
          ]),
      })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(names(region)).toEqual(['parked', 'live']))
      expect(s.of(LIST)[0].searchParams.get('lifecycle')).toBe('starting,active,parked,failed')
      await userEvent.click(within(region).getByRole('button', { name: 'Hide closed' }))
      await waitFor(() => expect(names(region)).toContain('closed'))
      expect(localStorage.getItem('hennery.hideClosed')).toBe('false')
      expect(s.of(LIST).at(-1)!.searchParams.get('lifecycle')).toBeNull()
    })

    it('a lifecycle filter shows that lifecycle only', async () => {
      at('/sessions')
      const s = server({
        list: () => page([summary('live'), summary('closed', { lifecycle: 'closed', activity: undefined })]),
      })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await userEvent.selectOptions(within(region).getByRole('combobox', { name: 'Lifecycle' }), 'closed')
      await waitFor(() => expect(names(region)).toEqual(['closed']))
      expect(s.of(LIST).at(-1)!.searchParams.get('lifecycle')).toBe('closed')
    })

    it('a search runs on the server across every lifecycle: only the hat applies (F-10)', async () => {
      localStorage.setItem('hennery.hat', 'hat-a')
      at('/sessions')
      const s = server({
        list: (params) =>
          params.get('q')
            ? page([
                summary('found-closed', { lifecycle: 'closed', activity: undefined }),
                summary('other-hat', { hat_id: 'hat-b' }),
              ])
            : page([summary('live')]),
      })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await userEvent.selectOptions(within(region).getByRole('combobox', { name: 'Lifecycle' }), 'active')
      await userEvent.type(within(region).getByRole('searchbox', { name: 'Search sessions' }), 'fix')
      await waitFor(() => expect(names(region)).toEqual(['found-closed']))
      const sent = s.of(LIST).at(-1)!.searchParams
      expect(sent.get('q')).toBe('fix')
      expect(sent.get('hat')).toBe('hat-a')
      expect(sent.get('lifecycle')).toBeNull()
      expect(within(region).getByRole('button', { name: 'Hide closed' })).toBeDisabled()
      expect(within(region).getByRole('combobox', { name: 'Lifecycle' })).toBeDisabled()
    })

    it('a search is at most 200 characters, the most the server takes', async () => {
      at('/sessions')
      const s = server({ list: () => page([summary('live')]) })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      const search = within(region).getByRole('searchbox', { name: 'Search sessions' })
      expect(search).toHaveAttribute('maxlength', '200')
      await userEvent.click(search)
      await userEvent.paste('q'.repeat(201))
      expect(search).toHaveValue('q'.repeat(200))
      await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('q')).toBe('q'.repeat(200)))
    })

    it('Hide closed is read back from storage', async () => {
      localStorage.setItem('hennery.hideClosed', 'false')
      at('/sessions')
      const s = server({ list: () => page([summary('live'), summary('closed', { lifecycle: 'closed', activity: undefined })]) })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(names(region)).toEqual(['live', 'closed']))
      expect(s.of(LIST)[0].searchParams.get('lifecycle')).toBeNull()
      expect(within(region).getByRole('button', { name: 'Hide closed' })).toHaveAttribute('aria-pressed', 'false')
    })

    it('loads the next page by its cursor', async () => {
      at('/sessions')
      const s = server({
        list: (params) => (params.get('cursor') === 'c1' ? page([summary('older', { last_event_at: '2026-10-01T10:00:00.000Z' })]) : page([summary('newer')], 'c1')),
      })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await userEvent.click(await within(region).findByRole('button', { name: 'Load more' }))
      await waitFor(() => expect(names(region)).toEqual(['newer', 'older']))
      expect(within(region).queryByRole('button', { name: 'Load more' })).not.toBeInTheDocument()
    })
  })

  describe('rows', () => {
    it('show the host name, fetched once, and no refetch on events', async () => {
      at('/sessions')
      const s = server({ list: () => page([summary('a')]) })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      expect(await within(region).findByText('laptop')).toBeInTheDocument()
      await waitFor(() => expect(s.streams).toHaveLength(1))
      act(() => s.streams[0].event('session_upsert', summary('b', { last_event_at: '2026-10-02T11:00:00.000Z' }), 'e1:2'))
      await waitFor(() => expect(names(region)).toEqual(['b', 'a']))
      expect(s.of('/api/hosts')).toHaveLength(1)
      expect(s.of(LIST)).toHaveLength(1)
    })

    it('render a hostile title as text', async () => {
      at('/sessions')
      const s = server({ list: () => page([summary('a', { title: '<img src=x onerror=alert(1)>' })]) })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      expect(await within(region).findByText('<img src=x onerror=alert(1)>')).toBeInTheDocument()
      expect(region.querySelector('img')).toBeNull()
    })
  })

  describe('the waiting count', () => {
    it('shows on a phone in the top bar', async () => {
      at('/sessions')
      const s = server({ list: () => page([summary('asks', { question_waits: true }), summary('calm')]) })
      const { container } = render(<App fetchImpl={s.fetch} />)
      await waitFor(() => expect(document.title).toBe('(1) hennery'))
      const bar = container.querySelector('.mtopbar') as HTMLElement
      expect(within(bar).getByRole('status', { name: '1 waiting on a question' })).toHaveTextContent('1')
    })


    it('is the tab title and the badge: the hat’s blocked or question-waiting sessions', async () => {
      localStorage.setItem('hennery.hat', 'hat-a')
      width(true)
      at('/sessions/x')
      const s = server({
        list: () =>
          page([
            summary('blocked', { activity: 'blocked' }),
            summary('asks', { question_waits: true }),
            summary('calm'),
            summary('elsewhere', { hat_id: 'hat-b', activity: 'blocked' }),
          ]),
      })
      render(<App fetchImpl={s.fetch} />)
      await waitFor(() => expect(document.title).toBe('(2) hennery'))
      const rail = await screen.findByRole('complementary', { name: 'Views' })
      expect(within(rail).getByRole('status', { name: '2 waiting on a question' })).toHaveTextContent('2')
      await waitFor(() => expect(s.streams).toHaveLength(1))
      act(() => s.streams[0].event('session_upsert', summary('blocked', { activity: 'running' }), 'e1:2'))
      act(() => s.streams[0].event('session_upsert', summary('asks'), 'e1:3'))
      await waitFor(() => expect(document.title).toBe('hennery'))
      expect(within(rail).queryByRole('status', { name: /waiting on a question/ })).not.toBeInTheDocument()
    })

    // The fallback (pages with no `waiting`): the hat's rows, a search's
    // (none) and the closed filter's (one, calm).
    const hatsList = (params: URLSearchParams) =>
      params.get('q')
        ? page([])
        : params.get('lifecycle') === 'closed'
          ? page([summary('done', { lifecycle: 'closed', activity: undefined })])
          : page([summary('asks', { activity: 'blocked' }), summary('calm')])

    it('stays the hat’s, never the query’s, while a search or a lifecycle filter is set', async () => {
      at('/sessions')
      const s = server({ list: hatsList })
      const { container } = render(<App fetchImpl={s.fetch} />)
      const region = await list()
      const bar = container.querySelector('.mtopbar') as HTMLElement
      const counted = () => {
        expect(document.title).toBe('(1) hennery')
        expect(within(bar).getByRole('status', { name: '1 waiting on a question' })).toHaveTextContent('1')
      }
      await waitFor(() => expect(names(region)).toEqual(['calm', 'asks']))
      counted()
      const search = within(region).getByRole('searchbox', { name: 'Search sessions' })
      await userEvent.type(search, 'zz')
      await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('q')).toBe('zz'))
      await within(region).findByText('No matches')
      counted()
      await userEvent.clear(search)
      await waitFor(() => expect(names(region)).toEqual(['calm', 'asks']))
      counted()
      await userEvent.selectOptions(within(region).getByRole('combobox', { name: 'Lifecycle' }), 'closed')
      await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('lifecycle')).toBe('closed'))
      await waitFor(() => expect(names(region)).toEqual(['done']))
      counted()
    })

    it('follows the stream while a search is set', async () => {
      at('/sessions')
      const s = server({ list: hatsList })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(document.title).toBe('(1) hennery'))
      await userEvent.type(within(region).getByRole('searchbox', { name: 'Search sessions' }), 'zz')
      await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('q')).toBe('zz'))
      await within(region).findByText('No matches')
      await waitFor(() => expect(s.streams.filter((x) => !x.cancelled)).toHaveLength(1))
      const live = s.streams.filter((x) => !x.cancelled)[0]
      act(() => live.event('session_upsert', summary('asks', { activity: 'running', last_event_at: '2026-10-02T11:00:00.000Z' }), 'e1:5'))
      await waitFor(() => expect(document.title).toBe('hennery'))
      act(() => live.event('session_upsert', summary('calm', { question_waits: true, last_event_at: '2026-10-02T11:00:00.000Z' }), 'e1:6'))
      await waitFor(() => expect(document.title).toBe('(1) hennery'))
    })

    // The server's count: 3, whatever the rows say (one blocked), and the
    // same on every page of the hat, searched or filtered.
    const countedList = (params: URLSearchParams) =>
      params.get('q')
        ? page([], undefined, 3)
        : params.get('lifecycle') === 'closed'
          ? page([summary('done', { lifecycle: 'closed', activity: undefined })], undefined, 3)
          : page([summary('asks', { activity: 'blocked' }), summary('calm')], undefined, 3)

    it('is the server’s count, kept while a search or a lifecycle filter is set', async () => {
      at('/sessions')
      const s = server({ list: countedList })
      const { container } = render(<App fetchImpl={s.fetch} />)
      const region = await list()
      const bar = container.querySelector('.mtopbar') as HTMLElement
      const counted = () => {
        expect(document.title).toBe('(3) hennery')
        expect(within(bar).getByRole('status', { name: '3 waiting on a question' })).toHaveTextContent('3')
      }
      await waitFor(() => expect(names(region)).toEqual(['calm', 'asks']))
      counted()
      const search = within(region).getByRole('searchbox', { name: 'Search sessions' })
      await userEvent.type(search, 'zz')
      await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('q')).toBe('zz'))
      await within(region).findByText('No matches')
      counted()
      await userEvent.clear(search)
      await waitFor(() => expect(names(region)).toEqual(['calm', 'asks']))
      counted()
      await userEvent.selectOptions(within(region).getByRole('combobox', { name: 'Lifecycle' }), 'closed')
      await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('lifecycle')).toBe('closed'))
      await waitFor(() => expect(names(region)).toEqual(['done']))
      counted()
    })

    it('is kept when the server refuses a search (a control character)', async () => {
      at('/sessions')
      const s = server({
        list: (params) =>
          params.get('q')?.includes('\t')
            ? json({ code: 'invalid', message: 'a search is at most 200 characters, with no control characters' }, 400)
            : countedList(params),
      })
      const { container } = render(<App fetchImpl={s.fetch} />)
      const region = await list()
      const bar = container.querySelector('.mtopbar') as HTMLElement
      await waitFor(() => expect(names(region)).toEqual(['calm', 'asks']))
      expect(document.title).toBe('(3) hennery')
      await userEvent.click(within(region).getByRole('searchbox', { name: 'Search sessions' }))
      await userEvent.paste('a\tb')
      await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('q')).toBe('a\tb'))
      // The refusal itself, not a page still loading (which also shows no rows).
      expect(await within(region).findByRole('alert')).toHaveTextContent(
        'a search is at most 200 characters, with no control characters',
      )
      expect(names(region)).toEqual([])
      expect(document.title).toBe('(3) hennery')
      expect(within(bar).getByRole('status', { name: '3 waiting on a question' })).toHaveTextContent('3')
    })

    it('follows waiting_changed, never an upsert, while a search is set', async () => {
      at('/sessions')
      const s = server({ list: countedList })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(document.title).toBe('(3) hennery'))
      await userEvent.type(within(region).getByRole('searchbox', { name: 'Search sessions' }), 'zz')
      await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('q')).toBe('zz'))
      await within(region).findByText('No matches')
      await waitFor(() => expect(s.streams.filter((x) => !x.cancelled)).toHaveLength(1))
      const live = s.streams.filter((x) => !x.cancelled)[0]
      act(() => live.event('session_upsert', summary('asks', { activity: 'running', last_event_at: '2026-10-02T11:00:00.000Z' }), 'e1:5'))
      await new Promise((r) => setTimeout(r, 30))
      expect(document.title).toBe('(3) hennery')
      act(() => live.event('waiting_changed', { count: 2 }, 'e1:6'))
      await waitFor(() => expect(document.title).toBe('(2) hennery'))
      act(() => live.event('waiting_changed', { count: 0 }, 'e1:7'))
      await waitFor(() => expect(document.title).toBe('hennery'))
    })

    it('is the chosen hat’s: a new hat opens a new page and a new stream with it', async () => {
      localStorage.setItem('hennery.hat', 'hat-a')
      at('/sessions')
      const counts: Record<string, number> = { 'hat-a': 3, 'hat-b': 1 }
      const s = server({ list: (params) => page([], undefined, counts[params.get('hat') ?? ''] ?? 7) })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(document.title).toBe('(3) hennery'))
      await waitFor(() => expect(s.streams).toHaveLength(1))
      await userEvent.click(await within(region).findByRole('button', { name: 'Home' }))
      await waitFor(() => expect(document.title).toBe('(1) hennery'))
      await userEvent.click(within(region).getByRole('button', { name: 'All hats' }))
      await waitFor(() => expect(document.title).toBe('(7) hennery'))
      await waitFor(() => expect(s.of(STREAM)).toHaveLength(3))
      // Never `hat=` empty: the server refuses it.
      expect(s.of(STREAM).map((u) => u.search)).toEqual(['?hat=hat-a', '?hat=hat-b', ''])
      expect(s.streams.slice(0, 2).every((x) => x.cancelled)).toBe(true)
    })
  })

  describe('selection (F-11)', () => {
    const rows = () =>
      page([
        summary('closed-newest', { lifecycle: 'closed', activity: undefined, last_event_at: '2026-10-02T12:00:00.000Z' }),
        summary('a', { last_event_at: '2026-10-02T11:00:00.000Z' }),
        summary('b', { hat_id: 'hat-b', last_event_at: '2026-10-02T10:00:00.000Z' }),
        summary('a-old', { last_event_at: '2026-10-02T09:00:00.000Z' }),
      ])

    it('on a desktop, /sessions opens the newest visible row, never a hidden one', async () => {
      width(true)
      at('/sessions')
      const s = server({ list: rows })
      render(<App fetchImpl={s.fetch} />)
      await waitFor(() => expect(location.pathname).toBe('/sessions/a'))
      const rail = await screen.findByRole('complementary', { name: 'Views' })
      await waitFor(() => expect(names(rail)).toContain('a'))
      const current = within(rail)
        .getAllByRole('link', { current: 'page' })
        .filter((l) => l.classList.contains('sess'))
      expect(current.map((l) => l.querySelector('.sess-name')?.textContent)).toEqual(['a'])
    })

    it('on a phone, /sessions stays the list', async () => {
      at('/sessions')
      const s = server({ list: rows })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(names(region)).toEqual(['a', 'b', 'a-old']))
      expect(location.pathname).toBe('/sessions')
    })

    it('honours a session outside the hat that the address names', async () => {
      localStorage.setItem('hennery.hat', 'hat-a')
      width(true)
      at('/sessions/b')
      const s = server({ list: rows })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(names(region)).toEqual(['a', 'a-old']))
      expect(location.pathname).toBe('/sessions/b')
    })

    it('a click opens the session', async () => {
      at('/sessions')
      const s = server({ list: rows })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(names(region)).toEqual(['a', 'b', 'a-old']))
      await userEvent.click(within(region).getAllByRole('link')[1])
      expect(location.pathname).toBe('/sessions/b')
    })

    /** The app on a desktop at `start`, with `hatBefore` chosen. */
    function openAt(start: string, hatBefore = '') {
      width(true)
      localStorage.setItem('hennery.hat', hatBefore)
      at(start)
      const s = server({ list: rows })
      render(<App fetchImpl={s.fetch} />)
      return s
    }

    it('switching hats clears a selection outside the new hat', async () => {
      openAt('/sessions/a')
      const rail = await screen.findByRole('complementary', { name: 'Views' })
      await waitFor(() => expect(names(rail)).toContain('a'))
      await userEvent.click(await within(rail).findByRole('button', { name: 'Home' }))
      // Cleared, then the newest visible row of the new hat is opened.
      await waitFor(() => expect(location.pathname).toBe('/sessions/b'))
      expect(localStorage.getItem('hennery.hat')).toBe('hat-b')
    })

    it('switching hats clears a selection inside the new hat too', async () => {
      // Not the hat's newest: kept, it would stay on a-old.
      const s = openAt('/sessions/a-old')
      const rail = await screen.findByRole('complementary', { name: 'Views' })
      await waitFor(() => expect(names(rail)).toContain('a-old'))
      const before = s.of(LIST).length
      await userEvent.click(await within(rail).findByRole('button', { name: 'Work' }))
      await waitFor(() => expect(s.of(LIST).length).toBeGreaterThan(before))
      await waitFor(() => expect(names(rail)).toEqual(['a', 'a-old']))
      await waitFor(() => expect(location.pathname).toBe('/sessions/a'))
    })

    it('switching to every hat clears the selection', async () => {
      const s = openAt('/sessions/a-old', 'hat-a')
      const rail = await screen.findByRole('complementary', { name: 'Views' })
      await waitFor(() => expect(names(rail)).toEqual(['a', 'a-old']))
      await userEvent.click(within(rail).getByRole('button', { name: 'All hats' }))
      await waitFor(() => expect(names(rail)).toEqual(['a', 'b', 'a-old']))
      await waitFor(() => expect(location.pathname).toBe('/sessions/a'))
      expect(s.of(LIST).at(-1)!.searchParams.get('hat')).toBeNull()
    })

    it('switching hats clears a session the list does not hold', async () => {
      openAt('/sessions/unknown')
      const rail = await screen.findByRole('complementary', { name: 'Views' })
      await waitFor(() => expect(names(rail)).toContain('a'))
      await userEvent.click(await within(rail).findByRole('button', { name: 'Work' }))
      await waitFor(() => expect(location.pathname).toBe('/sessions/a'))
    })

    it('selects a session whose id needs encoding, through the address, and a hat switch clears it', async () => {
      width(true)
      localStorage.setItem('hennery.hat', '')
      at('/sessions/x%2Fy%20z')
      const odd = summary('x/y z', { title: 'Odd id', last_event_at: '2026-10-02T11:30:00.000Z' })
      const s = server({ list: () => page([odd, ...rows().sessions]) })
      render(<App fetchImpl={s.fetch} />)
      const rail = await screen.findByRole('complementary', { name: 'Views' })
      await waitFor(() => expect(names(rail)).toContain('Odd id'))
      const current = within(rail)
        .getAllByRole('link', { current: 'page' })
        .filter((l) => l.classList.contains('sess'))
      expect(current.map((l) => l.querySelector('.sess-name')?.textContent)).toEqual(['Odd id'])
      expect(current[0]).toHaveAttribute('href', '/sessions/x%2Fy%20z')
      expect(location.pathname).toBe('/sessions/x%2Fy%20z')
      await userEvent.click(within(rail).getByRole('button', { name: 'Home' }))
      // Cleared, then the new hat's newest row is opened.
      await waitFor(() => expect(location.pathname).toBe('/sessions/b'))
      expect(s.of(LIST).at(-1)!.searchParams.get('hat')).toBe('hat-b')
    })

    it('choosing the hat already chosen keeps the selection', async () => {
      const s = openAt('/sessions/a-old', 'hat-a')
      const rail = await screen.findByRole('complementary', { name: 'Views' })
      await waitFor(() => expect(names(rail)).toEqual(['a', 'a-old']))
      await userEvent.click(within(rail).getByRole('button', { name: 'Work' }))
      expect(location.pathname).toBe('/sessions/a-old')
      expect(s.of(LIST)).toHaveLength(1)
    })
  })

  describe('hats', () => {
    it('the chosen hat’s colour becomes the theme, a bad one the default', async () => {
      at('/sessions')
      const s = server({ list: () => page([summary('a')]) })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await userEvent.click(await within(region).findByRole('button', { name: 'Work' }))
      await waitFor(() => expect(JSON.parse(localStorage.getItem(THEME_KEY)!)['--accent']).toBe('#112233'))
      expect(within(region).getByRole('button', { name: 'Work' })).toHaveAttribute('aria-pressed', 'true')
      await userEvent.click(within(region).getByRole('button', { name: 'Home' }))
      await waitFor(() => expect(localStorage.getItem(THEME_KEY)).toBeNull())
    })

    it('keeps the saved colours when the hats cannot be read', async () => {
      localStorage.setItem(THEME_KEY, JSON.stringify({ '--accent': '#445566', '--accent-2': '#334455' }))
      localStorage.setItem('hennery.hat', 'hat-a')
      at('/sessions')
      const s = server({ list: () => page([summary('a')]), hats: null })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(names(region)).toEqual(['a']))
      await waitFor(() => expect(s.of('/api/hats')).toHaveLength(1))
      expect(localStorage.getItem(THEME_KEY)).not.toBeNull()
    })

    it('a remembered hat that is gone falls back to every hat', async () => {
      localStorage.setItem('hennery.hat', 'hat-gone')
      at('/sessions')
      const s = server({ list: () => page([summary('a')]) })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(within(region).getByRole('button', { name: 'All hats' })).toHaveAttribute('aria-pressed', 'true'))
      expect(localStorage.getItem('hennery.hat')).toBe('')
    })
  })

  describe('hats and hosts, changed on their screens', () => {
    /** A server whose hats and hosts change when `change()` is called: a new
     *  hat, and the host renamed. */
    function changing() {
      const hats = [...HATS]
      // Whole hosts: the hosts screen renders them.
      const hosts = [host({ host_id: 'h1', name: 'laptop' })]
      const s = server({ list: () => page([summary('a')]), hats, hosts })
      const change = () => {
        hats.push(hat('hat-c', 'Clients', '#445566'))
        hosts[0] = host({ host_id: 'h1', name: 'build box' })
      }
      return { s, change }
    }

    /** The rail, once the session list's own hats and host names are read. */
    async function railRead() {
      const rail = await screen.findByRole('complementary', { name: 'Views' })
      await within(rail).findByRole('button', { name: 'Work' })
      await within(rail).findByText('laptop')
      return rail
    }

    it.each([
      ['/hats', '/sessions'],
      ['/hosts', '/sessions'],
      ['/hats', '/hosts'],
      ['/hosts', '/hats'],
    ])('are read again on leaving %s (for %s)', async (from, to) => {
      width(true)
      at(from)
      const { s, change } = changing()
      render(<App fetchImpl={s.fetch} />)
      const rail = await railRead()
      change()
      act(() => navigate(to))
      expect(await within(rail).findByRole('button', { name: 'Clients' })).toBeInTheDocument()
      expect(await within(rail).findByText('build box')).toBeInTheDocument()
      expect(s.of(LIST)).toHaveLength(1)
    })

    it('are read again after a visit to /hosts from a session, and not before leaving it', async () => {
      width(true)
      at('/sessions/a')
      const { s, change } = changing()
      render(<App fetchImpl={s.fetch} />)
      const rail = await railRead()
      act(() => navigate('/hosts'))
      await screen.findByRole('heading', { name: 'laptop' })
      change()
      await new Promise((r) => setTimeout(r, 100))
      expect(within(rail).getByText('laptop')).toBeInTheDocument()
      act(() => navigate('/sessions/a'))
      expect(await within(rail).findByText('build box')).toBeInTheDocument()
      expect(within(rail).getByRole('button', { name: 'Clients' })).toBeInTheDocument()
    })

    it('are not read again between the session list and a session', async () => {
      width(true)
      at('/sessions/a')
      const { s, change } = changing()
      render(<App fetchImpl={s.fetch} />)
      const rail = await railRead()
      change()
      act(() => navigate('/sessions'))
      await waitFor(() => expect(location.pathname).toBe('/sessions/a'))
      act(() => navigate('/sessions/b'))
      // The session view reads the hats for itself: the rail shows the
      // session list's own.
      await new Promise((r) => setTimeout(r, 100))
      expect(within(rail).queryByRole('button', { name: 'Clients' })).toBeNull()
      expect(within(rail).getByText('laptop')).toBeInTheDocument()
    })
  })

  describe('windowed rows', () => {
    it(`renders ${PAGE_ROWS} rows at a time, and fetches only past the rows held`, async () => {
      at('/sessions')
      const many = Array.from({ length: 120 }, (_, i) =>
        summary(`s${String(i).padStart(3, '0')}`, { last_event_at: new Date(Date.UTC(2026, 9, 2, 10, 0, 120 - i)).toISOString() }),
      )
      const s = server({ list: () => page(many) })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(names(region)).toHaveLength(50))
      expect(names(region)[0]).toBe('s000')
      await userEvent.click(within(region).getByRole('button', { name: 'Load more' }))
      await waitFor(() => expect(names(region)).toHaveLength(100))
      await userEvent.click(within(region).getByRole('button', { name: 'Load more' }))
      await waitFor(() => expect(names(region)).toHaveLength(120))
      expect(within(region).queryByRole('button', { name: 'Load more' })).not.toBeInTheDocument()
      expect(s.of(LIST)).toHaveLength(1)
    })

    it('opens a new query at one page of rows again', async () => {
      at('/sessions')
      const many = Array.from({ length: 120 }, (_, i) =>
        summary(`s${String(i).padStart(3, '0')}`, { last_event_at: new Date(Date.UTC(2026, 9, 2, 10, 0, 120 - i)).toISOString() }),
      )
      const s = server({ list: () => page(many) })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(names(region)).toHaveLength(50))
      await userEvent.click(within(region).getByRole('button', { name: 'Load more' }))
      await waitFor(() => expect(names(region)).toHaveLength(100))
      await userEvent.click(within(region).getByRole('button', { name: 'Work' }))
      await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('hat')).toBe('hat-a'))
      await waitFor(() => expect(names(region)).toHaveLength(PAGE_ROWS))
      expect(names(region)[0]).toBe('s000')
    })

    it('fetches the next page only once every row held is shown', async () => {
      at('/sessions')
      const many = Array.from({ length: 120 }, (_, i) =>
        summary(`s${String(i).padStart(3, '0')}`, { last_event_at: new Date(Date.UTC(2026, 9, 2, 10, 0, 120 - i)).toISOString() }),
      )
      const older = summary('older', { last_event_at: '2026-10-01T10:00:00.000Z' })
      const s = server({ list: (params) => (params.get('cursor') === 'c1' ? page([older]) : page(many, 'c1')) })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(names(region)).toHaveLength(50))
      const more = () => userEvent.click(within(region).getByRole('button', { name: 'Load more' }))
      await more()
      await waitFor(() => expect(names(region)).toHaveLength(100))
      await more()
      await waitFor(() => expect(names(region)).toHaveLength(120))
      expect(s.of(LIST)).toHaveLength(1)
      await more()
      await waitFor(() => expect(names(region)).toHaveLength(121))
      expect(s.of(LIST)).toHaveLength(2)
      expect(s.of(LIST)[1].searchParams.get('cursor')).toBe('c1')
      expect(within(region).queryByRole('button', { name: 'Load more' })).not.toBeInTheDocument()
    })
  })

  describe('the list stream, as the list shows it', () => {
    async function open(rows: SessionSummary[], hatChosen = '') {
      localStorage.setItem('hennery.hat', hatChosen)
      at('/sessions')
      const s = server({ list: (params) => (params.get('q') ? page([summary('found', { title: 'fix it' })]) : page(rows)) })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(s.streams).toHaveLength(1))
      await waitFor(() => expect(names(region)).toHaveLength(rows.length))
      return { s, region }
    }

    it('an upsert of a new session inserts its row in order', async () => {
      const { s, region } = await open([summary('a')])
      act(() => s.streams[0].event('session_upsert', summary('b', { last_event_at: '2026-10-02T11:00:00.000Z' }), 'e1:2'))
      await waitFor(() => expect(names(region)).toEqual(['b', 'a']))
    })

    it('an upsert updates a row in place', async () => {
      const { s, region } = await open([summary('a'), summary('b', { last_event_at: '2026-10-02T09:00:00.000Z' })])
      act(() => s.streams[0].event('session_upsert', summary('b', { title: 'Renamed', last_event_at: '2026-10-02T09:00:00.000Z' }), 'e1:2'))
      await waitFor(() => expect(names(region)).toEqual(['a', 'Renamed']))
    })

    it('an upsert that closes a session takes its row out while Hide closed is on', async () => {
      const { s, region } = await open([summary('a'), summary('b', { last_event_at: '2026-10-02T09:00:00.000Z' })])
      act(() => s.streams[0].event('session_upsert', summary('b', { lifecycle: 'closed', activity: undefined }), 'e1:2'))
      await waitFor(() => expect(names(region)).toEqual(['a']))
    })

    it('an upsert that moves a session to another hat takes its row out', async () => {
      const { s, region } = await open([summary('a'), summary('b', { last_event_at: '2026-10-02T09:00:00.000Z' })], 'hat-a')
      act(() => s.streams[0].event('session_upsert', summary('b', { hat_id: 'hat-b' }), 'e1:2'))
      await waitFor(() => expect(names(region)).toEqual(['a']))
    })

    it('during a search an upsert updates a found row and never adds one', async () => {
      const { s, region } = await open([summary('a')])
      await userEvent.type(within(region).getByRole('searchbox', { name: 'Search sessions' }), 'fix')
      await waitFor(() => expect(names(region)).toEqual(['fix it']), { timeout: 3000 })
      await waitFor(() => expect(s.streams).toHaveLength(2))
      const live = s.streams[1]
      act(() => live.event('session_upsert', summary('new', { title: 'fix that', last_event_at: '2026-10-02T12:00:00.000Z' }), 'e1:2'))
      act(() => live.event('session_upsert', summary('found', { title: 'fix it now' }), 'e1:3'))
      await waitFor(() => expect(names(region)).toEqual(['fix it now']))
    })

    it('session_removed takes the row out', async () => {
      const { s, region } = await open([summary('a'), summary('b', { last_event_at: '2026-10-02T09:00:00.000Z' })])
      act(() => s.streams[0].event('session_removed', { session_id: 'a' }, 'e1:2'))
      await waitFor(() => expect(names(region)).toEqual(['b']))
    })

    it('a resync fetches the first page again with the same query and replaces the rows', async () => {
      const pages = [page([summary('a')]), page([summary('z')])]
      let n = 0
      at('/sessions')
      const s = server({ list: () => pages[Math.min(n++, 1)] })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(names(region)).toEqual(['a']))
      await waitFor(() => expect(s.streams).toHaveLength(1))
      act(() => s.streams[0].send('event: resync_required\ndata: {}\n\n'))
      await waitFor(() => expect(names(region)).toEqual(['z']))
      const [first, again] = s.of(LIST)
      expect(again.search).toBe(first.search)
      await waitFor(() => expect(s.streams).toHaveLength(2))
    })

    it('says “Resynced” for a moment once a resync replaced the rows, and nothing before', async () => {
      const pages = [page([summary('a')]), page([summary('z')])]
      let n = 0
      at('/sessions')
      const s = server({ list: () => pages[Math.min(n++, 1)] })
      render(<App fetchImpl={s.fetch} />)
      const region = await list()
      await waitFor(() => expect(names(region)).toEqual(['a']))
      await waitFor(() => expect(s.streams).toHaveLength(1))
      expect(within(region).queryByText('Resynced')).toBeNull()
      act(() => s.streams[0].send('event: resync_required\ndata: {}\n\n'))
      await waitFor(() => expect(names(region)).toEqual(['z']))
      // One status line: "Reconnecting…" before, "Resynced" after.
      expect(await within(region).findByRole('status')).toHaveTextContent('Resynced')
    })

    it('says it is reconnecting while the stream is down', async () => {
      const { s, region } = await open([summary('a')])
      act(() => s.streams[0].end())
      expect(await within(region).findByRole('status')).toHaveTextContent('Reconnecting…')
      expect(names(region)).toEqual(['a'])
    })
  })
  ```

Create `web/src/components/SessionRow.test.tsx`:

  ```tsx
  import { render, screen, within } from '@testing-library/react'
  import userEvent from '@testing-library/user-event'
  import { describe, expect, it } from 'vitest'
  import type { SessionSummary } from '../generated/view'
  import SessionRow, { rowText } from './SessionRow'

  /** The list's clock, 15 minutes after the fixture's last event. */
  const NOW = Date.parse('2026-10-02T10:15:00.000Z')

  function summary(patch: Partial<SessionSummary> = {}): SessionSummary {
    return {
      session_id: 's1',
      host_id: 'h1',
      agent: 'claude',
      cwd: '/srv/work/app',
      hat_id: 'hat-a',
      lifecycle: 'active',
      activity: 'idle',
      presumed_parked: false,
      created_at: '2026-10-01T00:00:00.000Z',
      last_event_at: '2026-10-02T10:00:00.000Z',
      question_waits: false,
      ...patch,
    }
  }

  describe('rowText', () => {
    it('titles a row by its title, else its directory, else its id', () => {
      expect(rowText(summary({ title: 'Fix the parser' })).title).toBe('Fix the parser')
      expect(rowText(summary()).title).toBe('app')
      expect(rowText(summary({ cwd: '' })).title).toBe('s1')
    })

    it('puts the branch on line two, else the directory name', () => {
      expect(rowText(summary({ title: 'Fix', git_branch: 'fix/parser' })).where).toBe('fix/parser')
      expect(rowText(summary({ title: 'Fix' })).where).toBe('app')
    })

    it('never repeats the title on line two', () => {
      expect(rowText(summary()).where).toBe('')
      expect(rowText(summary({ title: 'main', git_branch: 'main' })).where).toBe('')
    })
  })

  describe('SessionRow', () => {
    it('reads its relative time from the list’s clock, not from the time it renders', () => {
      const { container, rerender } = render(<SessionRow now={NOW} session={summary()} selected={false} />)
      expect(container.querySelector('.sess-time')?.textContent).toBe('15m ago')
      rerender(<SessionRow now={NOW + 60 * 60_000} session={summary()} selected={false} />)
      expect(container.querySelector('.sess-time')?.textContent).toBe('1h ago')
    })

    it('links to the session, and shows agent, title, branch and host', () => {
      render(<SessionRow now={NOW} session={summary({ session_id: 'a b/c', title: 'Fix', git_branch: 'fix/x' })} selected hostName="laptop" />)
      const link = screen.getByRole('link')
      expect(link).toHaveAttribute('href', '/sessions/a%20b%2Fc')
      expect(link).toHaveAttribute('aria-current', 'page')
      expect(within(link).getByText('Claude')).toBeInTheDocument()
      expect(within(link).getByText('fix/x')).toBeInTheDocument()
      expect(within(link).getByText('laptop')).toBeInTheDocument()
    })

    it('shows the agent it is given, not a fixed one', () => {
      render(<SessionRow now={NOW} session={summary({ agent: 'codex' })} selected={false} />)
      expect(screen.getByText('Codex')).toBeInTheDocument()
      expect(screen.queryByText('Claude')).not.toBeInTheDocument()
    })

    it('renders a hostile title as text', () => {
      const { container } = render(<SessionRow now={NOW} session={summary({ title: '<img src=x onerror=alert(1)>' })} selected={false} />)
      expect(screen.getByText('<img src=x onerror=alert(1)>')).toBeInTheDocument()
      expect(container.querySelector('img')).toBeNull()
    })

    it('marks a session waiting on a question with words', () => {
      render(<SessionRow now={NOW} session={summary({ question_waits: true })} selected={false} />)
      expect(screen.getByRole('img', { name: 'Waiting on a question' })).toHaveClass('led-attn')
      expect(screen.getByText('Waiting on a question')).toBeInTheDocument()
    })

    it('animates a running session', () => {
      render(<SessionRow now={NOW} session={summary({ activity: 'running' })} selected={false} />)
      expect(screen.getByRole('img', { name: 'Running' })).toHaveClass('led-run')
    })

    it('offers Resume on a parked row, inside its link', () => {
      render(<SessionRow now={NOW} session={summary({ lifecycle: 'parked', activity: undefined })} selected={false} />)
      expect(within(screen.getByRole('link')).getByText('Resume')).toBeInTheDocument()
    })

    it('says a presumed-parked host is offline', () => {
      render(<SessionRow now={NOW} session={summary({ lifecycle: 'parked', activity: undefined, presumed_parked: true })} selected={false} />)
      expect(screen.getByRole('img', { name: 'Host offline' })).toBeInTheDocument()
      expect(screen.getByText('Resume')).toBeInTheDocument()
    })

    it('shows a failure reason on hover and on a tap, without opening the session', async () => {
      render(
        <SessionRow now={NOW} session={summary({ lifecycle: 'failed', activity: undefined, failure_reason: 'agent_not_logged_in' })} selected={false} />,
      )
      expect(screen.getByRole('img', { name: 'Failed' })).toHaveAttribute('title', 'Failed: agent_not_logged_in')
      expect(screen.queryByText('agent_not_logged_in')).not.toBeInTheDocument()
      const why = screen.getByRole('button', { name: 'Why it failed' })
      expect(screen.getByRole('link')).not.toContainElement(why)
      await userEvent.click(why)
      expect(screen.getByText('agent_not_logged_in')).toBeInTheDocument()
      expect(why).toHaveAttribute('aria-expanded', 'true')
    })

    it('shows no reason control on a session that did not fail', () => {
      render(<SessionRow now={NOW} session={summary({ failure_reason: 'left over' })} selected={false} />)
      expect(screen.queryByRole('button')).not.toBeInTheDocument()
      expect(screen.getByRole('img', { name: 'Idle' })).toHaveAttribute('title', 'Idle')
    })
  })
  ```

Create `web/src/hooks/useNow.test.ts`:

  ```ts
  import { act, renderHook } from '@testing-library/react'
  import { afterEach, describe, expect, it, vi } from 'vitest'
  import { useNow } from './useNow'

  afterEach(() => {
    vi.useRealTimers()
  })

  describe('useNow', () => {
    it('reads the time again at each interval, with nothing else rendering', () => {
      vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'clearInterval'] })
      vi.setSystemTime(new Date(2026, 9, 2, 23, 59))
      const { result } = renderHook(() => useNow(60_000))
      const start = result.current
      expect(start).toBe(new Date(2026, 9, 2, 23, 59).getTime())
      act(() => vi.advanceTimersByTime(59_999))
      expect(result.current).toBe(start)
      act(() => vi.advanceTimersByTime(1))
      expect(result.current).toBe(start + 60_000)
    })

    it('reads the time again when the page’s visibility changes, before the interval', () => {
      vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'clearInterval'] })
      vi.setSystemTime(new Date(2026, 9, 2, 23, 0))
      const { result } = renderHook(() => useNow(60_000))
      // The browser held the timers back while the tab slept: only the
      // clock moved.
      vi.setSystemTime(new Date(2026, 9, 3, 7, 0))
      expect(result.current).toBe(new Date(2026, 9, 2, 23, 0).getTime())
      act(() => void document.dispatchEvent(new Event('visibilitychange')))
      expect(result.current).toBe(new Date(2026, 9, 3, 7, 0).getTime())
    })

    it('stops reading once unmounted', () => {
      vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'clearInterval'] })
      const remove = vi.spyOn(document, 'removeEventListener')
      const { unmount } = renderHook(() => useNow(60_000))
      unmount()
      expect(vi.getTimerCount()).toBe(0)
      expect(remove).toHaveBeenCalledWith('visibilitychange', expect.any(Function))
      remove.mockRestore()
    })
  })
  ```

Create `web/src/lib/agent.test.ts`:

  ```ts
  import { describe, expect, it } from 'vitest'
  import { agentLabel, avatarLetter } from './agent'

  describe('agentLabel', () => {
    it.each([
      ['claude', 'Claude'],
      ['codex', 'Codex'],
      ['gemini-cli', 'gemini-cli'],
      ['constructor', 'constructor'],
      [undefined, 'Agent'],
      ['', 'Agent'],
    ])('%s → %s', (agent, label) => {
      expect(agentLabel(agent)).toBe(label)
    })
  })

  describe('avatarLetter', () => {
    it('is the first letter of the label', () => {
      expect(avatarLetter('You')).toBe('Y')
      expect(avatarLetter('codex')).toBe('C')
      expect(avatarLetter('')).toBe('?')
    })
  })
  ```

Create `web/src/lib/hats.test.ts`:

  ```ts
  import { beforeEach, describe, expect, it } from 'vitest'
  import { HAT_KEY, readHat, themeOf, writeHat } from './hats'

  beforeEach(() => localStorage.clear())

  describe('the selected hat', () => {
    it('is every hat until one is chosen, then remembered', () => {
      expect(readHat()).toBe('')
      writeHat('hat-a')
      expect(localStorage.getItem(HAT_KEY)).toBe('hat-a')
      expect(readHat()).toBe('hat-a')
    })

    it('keys its storage under hennery.', () => {
      expect(HAT_KEY).toBe('hennery.hat')
    })
  })

  describe('themeOf', () => {
    it('takes a #rrggbb colour, with a darker second stop', () => {
      expect(themeOf('#4C5FD5')).toEqual({ accent: '#4c5fd5', accent2: '#3b49a4' })
    })

    it('refuses anything else: the default colours', () => {
      for (const bad of [undefined, '', 'red', '#abc', '#12345g', '#1234567', 'url(x)', '#112233;color:red']) {
        expect(themeOf(bad)).toBeNull()
      }
    })
  })
  ```

Create `web/src/lib/status.test.ts`:

  ```ts
  import { describe, expect, it } from 'vitest'
  import { statusOf, type StatusInput } from './status'

  const base: StatusInput = { lifecycle: 'active', activity: 'idle', question_waits: false, presumed_parked: false }
  const of = (patch: Partial<StatusInput>) => statusOf({ ...base, ...patch })

  describe('statusOf', () => {
    it('blocked: waiting on a question, the strongest marker', () => {
      expect(of({ activity: 'blocked' })).toEqual({ tone: 'attn', label: 'Waiting on a question', resume: false })
    })

    it('a question open outside a turn: waiting on a question too', () => {
      expect(of({ activity: 'idle', question_waits: true })).toEqual({
        tone: 'attn',
        label: 'Waiting on a question',
        resume: false,
      })
    })

    it('waiting wins over the lifecycle, so the marker agrees with the count', () => {
      expect(of({ lifecycle: 'parked', question_waits: true }).tone).toBe('attn')
    })

    it('never says "needs you"', () => {
      for (const s of [of({ activity: 'blocked' }), of({ question_waits: true })]) expect(s.label).not.toMatch(/needs you/i)
    })

    it('running: animates', () => {
      expect(of({ activity: 'running' })).toEqual({ tone: 'run', label: 'Running', resume: false })
    })

    it('active and idle', () => {
      expect(of({ activity: 'idle' })).toEqual({ tone: 'idle', label: 'Idle', resume: false })
      expect(of({ activity: undefined })).toEqual({ tone: 'idle', label: 'Idle', resume: false })
    })

    it('starting', () => {
      expect(of({ lifecycle: 'starting', activity: undefined })).toEqual({ tone: 'wait', label: 'Starting', resume: false })
    })

    it('parked: offers Resume', () => {
      expect(of({ lifecycle: 'parked', activity: undefined })).toEqual({ tone: 'idle', label: 'Parked', resume: true })
    })

    it('presumed parked: the host is offline, Resume still offered', () => {
      expect(of({ lifecycle: 'parked', activity: undefined, presumed_parked: true })).toEqual({
        tone: 'idle',
        label: 'Host offline',
        resume: true,
      })
    })

    it('failed', () => {
      expect(of({ lifecycle: 'failed', activity: undefined })).toEqual({ tone: 'fail', label: 'Failed', resume: false })
    })

    it('closed: no Resume on the row', () => {
      expect(of({ lifecycle: 'closed', activity: undefined })).toEqual({ tone: 'idle', label: 'Closed', resume: false })
    })

    it('a lifecycle it does not know: shown by name', () => {
      expect(of({ lifecycle: 'archived', activity: undefined })).toEqual({ tone: 'idle', label: 'archived', resume: false })
      expect(of({ lifecycle: '', activity: undefined }).label).toBe('Unknown')
    })
  })
  ```

In `web/src/lib/time.test.ts`, replace:

  ```ts
    it('empty -> empty', () => expect(relTime('')).toBe(''))
  })
  ```

with:

  ```ts
    it('empty -> empty', () => expect(relTime('')).toBe(''))
    it('counts from the `now` it is given', () =>
      expect(relTime('2026-06-09T11:45:00Z', Date.parse('2026-06-09T13:45:00Z'))).toBe('2h ago'))
  })
  ```

In `web/src/screens/Hosts.test.tsx`, replace:

  ```tsx
        // The list, the read before the mint, then a poll that never answers.
        'GET /api/hosts': [json(200, [host()]), json(200, [host()]), () => new Promise<Response>(() => {})],
  ```

with:

  ```tsx
        // The list and the session list's host names (both at mount, in
        // either order), the read before the mint, then a poll that never
        // answers.
        'GET /api/hosts': [json(200, [host()]), json(200, [host()]), json(200, [host()]), () => new Promise<Response>(() => {})],
  ```

In `web/src/screens/Hosts.test.tsx`, replace:

  ```tsx
      expect(sent(server, 'GET', '/api/hosts')).toHaveLength(3)
  ```

with:

  ```tsx
      expect(sent(server, 'GET', '/api/hosts')).toHaveLength(4)
  ```

In `web/src/screens/Hosts.test.tsx`, replace:

  ```tsx
        // The list, the read before the mint, then the polls.
        'GET /api/hosts': [json(200, [host()]), json(200, [host()]), json(200, [host(), host({ host_id: 'host-9', name: 'new box' })])],
  ```

with:

  ```tsx
        // The list and the session list's host names (both at mount, in
        // either order), the read before the mint, then the polls.
        'GET /api/hosts': [
          json(200, [host()]),
          json(200, [host()]),
          json(200, [host()]),
          json(200, [host(), host({ host_id: 'host-9', name: 'new box' })]),
        ],
  ```

In `web/src/screens/Hosts.test.tsx`, replace:

  ```tsx
        'GET /api/hosts': [json(500, { code: 'internal', message: 'm' }), json(200, [host()])],
  ```

with:

  ```tsx
        // The list and the session list's host names both fail at mount.
        'GET /api/hosts': [json(500, { code: 'internal', message: 'm' }), json(500, { code: 'internal', message: 'm' }), json(200, [host()])],
  ```

In `web/src/store/sessionList.test.ts`, replace:

  ```ts
    EMPTY_LIST,
    compareSummaries,
  ```

with:

  ```ts
    EMPTY_LIST,
    carryWaiting,
    compareSummaries,
  ```

In `web/src/store/sessionList.test.ts`, replace:

  ```ts
      expect(isShown(state, summary('d', { hat_id: 'hat-b' }), { hat: 'hat-a', hideClosed: false })).toBe(false)
    })
  })
  ```

with:

  ```ts
      expect(isShown(state, summary('d', { hat_id: 'hat-b' }), { hat: 'hat-a', hideClosed: false })).toBe(false)
    })
  })

  describe('carryWaiting', () => {
    it('keeps the ids held and applies every summary received since', () => {
      const held = new Set(['kept', 'answered', 'moved'])
      const since = first([
        summary('answered', { activity: 'running' }),
        summary('moved', { activity: 'blocked', hat_id: 'hat-b' }),
        summary('asks', { question_waits: true }),
        summary('calm'),
      ])
      expect([...carryWaiting(held, since, 'hat-a')].sort()).toEqual(['asks', 'kept'])
      expect([...held].sort()).toEqual(['answered', 'kept', 'moved'])
    })
  })
  ```

In `web/src/store/useSessionList.test.ts`, replace:

  ```ts
    })
  })
  ```

with:

  ```ts
    })

    it('keeps the hat’s waiting count through a search and while the hat’s list loads again', async () => {
      let held: ((r: Response) => void) | null = null
      let unfilteredPages = 0
      const t = routed((call) => {
        if (call.path.startsWith(STREAM)) return liveStream().response
        if (call.path.includes('q=')) return json(page([], 30))
        unfilteredPages++
        const first = page([summary('asks', { activity: 'blocked' }), summary('calm')], 10)
        return unfilteredPages === 1 ? json(first) : new Promise<Response>((r) => (held = r))
      })
      const { result, rerender } = renderHook((f: ListFilters) => useSessionList(f, FAST), {
        wrapper: t.wrapper,
        initialProps: { hideClosed: false } as ListFilters,
      })
      await waitFor(() => expect(result.current.loading).toBe(false))
      expect(result.current.counts.waiting).toBe(1)
      rerender({ hideClosed: false, q: 'zz' })
      await waitFor(() => expect(result.current.loading).toBe(false))
      expect(result.current.shown).toEqual([])
      expect(result.current.counts.waiting).toBe(1)
      rerender({ hideClosed: false })
      await waitFor(() => expect(held).not.toBeNull())
      expect(result.current.loading).toBe(true)
      expect(result.current.counts.waiting).toBe(1)
      await act(async () => held!(json(page([summary('asks'), summary('calm')], 40))))
      await waitFor(() => expect(result.current.loading).toBe(false))
      expect(result.current.counts.waiting).toBe(0)
    })

    // The server's count: rows that disagree with it show which one is read.
    it('the server’s count holds through a search and while the hat’s list loads again', async () => {
      let held: ((r: Response) => void) | null = null
      let unfilteredPages = 0
      const t = routed((call) => {
        if (call.path.startsWith(STREAM)) return liveStream().response
        // The search finds nothing; the hat's count went up to 5 meanwhile.
        if (call.path.includes('q=')) return json(page([], 30, undefined, 5))
        unfilteredPages++
        const first = page([summary('asks', { activity: 'blocked' }), summary('calm')], 10, undefined, 4)
        return unfilteredPages === 1 ? json(first) : new Promise<Response>((r) => (held = r))
      })
      const { result, rerender } = renderHook((f: ListFilters) => useSessionList(f, FAST), {
        wrapper: t.wrapper,
        initialProps: { hat: 'hat-a', hideClosed: false } as ListFilters,
      })
      await waitFor(() => expect(result.current.loading).toBe(false))
      expect(result.current.counts.waiting).toBe(4)
      rerender({ hat: 'hat-a', hideClosed: false, q: 'zz' })
      expect(result.current.loading).toBe(true)
      expect(result.current.counts.waiting).toBe(4)
      await waitFor(() => expect(result.current.loading).toBe(false))
      expect(result.current.counts.waiting).toBe(5)
      rerender({ hat: 'hat-a', hideClosed: false })
      await waitFor(() => expect(held).not.toBeNull())
      expect(result.current.loading).toBe(true)
      expect(result.current.counts.waiting).toBe(5)
      await act(async () => held!(json(page([summary('asks', { activity: 'blocked' })], 40, undefined, 0))))
      await waitFor(() => expect(result.current.loading).toBe(false))
      expect(result.current.counts.waiting).toBe(0)
    })

    it('the server’s count of one hat is never shown for another while its list loads', async () => {
      let held: ((r: Response) => void) | null = null
      const t = routed((call) => {
        if (call.path.startsWith(STREAM)) return liveStream().response
        if (call.path.includes('hat=hat-b')) return new Promise<Response>((r) => (held = r))
        return json(page([summary('asks', { activity: 'blocked' })], 10, undefined, 4))
      })
      const { result, rerender } = renderHook((f: ListFilters) => useSessionList(f, FAST), {
        wrapper: t.wrapper,
        initialProps: { hat: 'hat-a', hideClosed: false } as ListFilters,
      })
      await waitFor(() => expect(result.current.counts.waiting).toBe(4))
      rerender({ hat: 'hat-b', hideClosed: false })
      await waitFor(() => expect(held).not.toBeNull())
      expect(result.current.loading).toBe(true)
      expect(result.current.counts.waiting).toBe(0)
      await act(async () => held!(json(page([], 20, undefined, 2))))
      await waitFor(() => expect(result.current.counts.waiting).toBe(2))
      rerender({ hat: 'hat-a', hideClosed: false })
      await waitFor(() => expect(result.current.loading).toBe(false))
      expect(result.current.counts.waiting).toBe(4)
    })

    // A refused first page (4xx) never loads: the count it would replace holds.
    const REFUSED = json({ code: 'invalid', message: 'a search is at most 200 characters, with no control characters' }, 400)

    it('a search the server refuses (over 200 characters) keeps the server’s count', async () => {
      const long = 'q'.repeat(201)
      const t = routed((call) => {
        if (call.path.startsWith(STREAM)) return liveStream().response
        if (call.path.includes('q=')) return REFUSED.clone()
        return json(page([summary('asks', { activity: 'blocked' })], 10, undefined, 4))
      })
      const { result, rerender } = renderHook((f: ListFilters) => useSessionList(f, FAST), {
        wrapper: t.wrapper,
        initialProps: { hat: 'hat-a', hideClosed: false } as ListFilters,
      })
      await waitFor(() => expect(result.current.counts.waiting).toBe(4))
      rerender({ hat: 'hat-a', hideClosed: false, q: long })
      await waitFor(() => expect(result.current.error).not.toBeNull())
      expect(new URL(t.calls.at(-1)!.path, 'http://h').searchParams.get('q')).toBe(long)
      expect(result.current.loading).toBe(false)
      expect(result.current.stream).toBe('closed')
      expect(result.current.counts.waiting).toBe(4)
    })

    it('an unfiltered first page the server refuses keeps the server’s count', async () => {
      const t = routed((call) => {
        if (call.path.startsWith(STREAM)) return liveStream().response
        if (call.path.includes('lifecycle=')) return REFUSED.clone()
        return json(page([summary('asks', { activity: 'blocked' })], 10, undefined, 4))
      })
      const { result, rerender } = renderHook((f: ListFilters) => useSessionList(f, FAST), {
        wrapper: t.wrapper,
        initialProps: { hat: 'hat-a', hideClosed: false } as ListFilters,
      })
      await waitFor(() => expect(result.current.counts.waiting).toBe(4))
      rerender({ hat: 'hat-a', hideClosed: true })
      await waitFor(() => expect(result.current.error).not.toBeNull())
      expect(result.current.loading).toBe(false)
      expect(result.current.counts.waiting).toBe(4)
    })

    it('with no server count, an unfiltered first page the server refuses keeps the rows counted before', async () => {
      const t = routed((call) => {
        if (call.path.startsWith(STREAM)) return liveStream().response
        if (call.path.includes('lifecycle=')) return REFUSED.clone()
        return json(page([summary('asks', { activity: 'blocked' }), summary('calm')], 10))
      })
      const { result, rerender } = renderHook((f: ListFilters) => useSessionList(f, FAST), {
        wrapper: t.wrapper,
        initialProps: { hat: 'hat-a', hideClosed: false } as ListFilters,
      })
      await waitFor(() => expect(result.current.counts.waiting).toBe(1))
      rerender({ hat: 'hat-a', hideClosed: true })
      await waitFor(() => expect(result.current.error).not.toBeNull())
      expect(result.current.loading).toBe(false)
      expect(result.current.counts.waiting).toBe(1)
    })
  })
  ```

- [ ] **Step 2: Run them, and see them fail**

Run: `nix develop -c sh -c 'cd web && pnpm vitest run src/components src/lib'`
Expected: FAIL: `SessionList`, `SessionRow`, `status`, `hats` and `agent` do not exist yet.

- [ ] **Step 3: The list**

Create `web/src/components/HatSwitch.tsx`:

  ```tsx
  // The hat selector (frontend spec §5): every hat, or one. The hats are the
  // server's; the app never computes a session's hat.
  import type { HatItem } from '../generated/protocol'

  interface Props {
    hats: readonly HatItem[] | null
    /** The selected hat's id, `''` for every hat. */
    hat: string
    onHat: (hat: string) => void
  }

  export default function HatSwitch({ hats, hat, onHat }: Props) {
    const choices = [{ id: '', name: 'All hats' }, ...(hats ?? []).map((h) => ({ id: h.id, name: h.name }))]
    return (
      <div className="hat-switch list-hats" role="group" aria-label="Hat">
        {choices.map((c) => (
          <button
            key={c.id || '*'}
            type="button"
            className={'chip' + (hat === c.id ? ' on' : '')}
            aria-pressed={hat === c.id}
            onClick={() => onHat(c.id)}
          >
            <bdi>{c.name}</bdi>
          </button>
        ))}
      </div>
    )
  }
  ```

Create `web/src/components/SessionList.tsx`:

  ```tsx
  // The session list (frontend spec §5; plan 4c decisions 6–12): the hat
  // selector, a search box, Hide closed and a lifecycle filter, then the rows
  // newest first under sticky day headings. In the rail from 768 px; the
  // whole screen at `/sessions` under it.
  //
  // - A search runs on the server across every lifecycle: while it is active
  //   Hide closed and the lifecycle filter are set aside, and only the hat
  //   applies (F-10).
  // - Rows render 50 at a time; past the rows held, "Load more" fetches the
  //   next page by `next_cursor`. A new server query (hat, filter, search)
  //   opens at 50 rows again.
  // - Day headings and relative times follow a clock read every minute and
  //   when the tab comes back (`useNow`), not only the list's events: an idle
  //   list left open overnight moves today's rows under "Yesterday" (F-8).
  // - The list stream's state shows as "Reconnecting…", and "Resynced" for a
  //   moment after the rows were replaced (frontend spec §4.1).
  import { useState } from 'react'
  import { useNow } from '../hooks/useNow'
  import type { SessionSummary } from '../generated/view'
  import { BUCKET_LABEL, BUCKET_ORDER, bucketOf, type Bucket } from '../lib/time'
  import { Icon } from '../lib/ui'
  import { LIFECYCLES } from '../store/sessionList'
  import HatSwitch from './HatSwitch'
  import SessionRow from './SessionRow'
  import { SEARCH_MAX_CHARS, useSessionScope } from './SessionScope'

  export const PAGE_ROWS = 50

  const LIFECYCLE_LABEL: Record<(typeof LIFECYCLES)[number], string> = {
    starting: 'Starting',
    active: 'Active',
    parked: 'Parked',
    closed: 'Closed',
    failed: 'Failed',
  }

  /** How often the day headings and relative times are read again. */
  export const CLOCK_MS = 60_000

  /** `rows` (newest first) under their day headings relative to `now`, in
   *  display order; an empty day has no heading. */
  export function byDay(rows: readonly SessionSummary[], now: number): [Bucket, SessionSummary[]][] {
    const days = new Map<Bucket, SessionSummary[]>()
    for (const s of rows) {
      const b = bucketOf(s.last_event_at, now)
      const list = days.get(b)
      if (list) list.push(s)
      else days.set(b, [s])
    }
    return BUCKET_ORDER.filter((b) => days.has(b)).map((b) => [b, days.get(b)!])
  }

  export default function SessionList({ screen = false }: { screen?: boolean }) {
    const scope = useSessionScope()
    const now = useNow(CLOCK_MS)
    // The rows shown, for one server query: a new query starts at a page.
    const [shownRows, setShownRows] = useState({ query: '', n: PAGE_ROWS })
    if (!scope) return null
    const { list, hats, hat, chooseHat, hostNames, hideClosed, setHideClosed, lifecycle, setLifecycle, query, setQuery, selectedId } =
      scope
    const searching = list.searching
    const limit = shownRows.query === list.queryKey ? shownRows.n : PAGE_ROWS
    const rows = list.shown.slice(0, limit)
    const more = list.shown.length > limit || list.hasMore
    const showMore = () => {
      setShownRows({ query: list.queryKey, n: limit + PAGE_ROWS })
      if (list.shown.length <= limit) void list.loadMore()
    }
    const streamNote = list.loading ? '' : list.stream === 'reconnecting' ? 'Reconnecting…' : list.resynced ? 'Resynced' : ''

    return (
      <section className={'slist' + (screen ? ' slist-screen' : '')} aria-label="Session list">
        {screen && <h1 className="slist-title">Sessions</h1>}
        <HatSwitch hats={hats} hat={hat} onHat={chooseHat} />
        <div className="rail-search">
          <Icon.Search size={15} />
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            maxLength={SEARCH_MAX_CHARS}
            placeholder="Search sessions…"
            aria-label="Search sessions"
          />
        </div>
        <div className="list-filters">
          <button
            type="button"
            className={'toggle' + (hideClosed && !searching ? ' on' : '')}
            aria-pressed={hideClosed}
            disabled={searching}
            onClick={() => setHideClosed(!hideClosed)}
          >
            Hide closed
          </button>
          <select
            className="list-select"
            aria-label="Lifecycle"
            value={lifecycle}
            disabled={searching}
            onChange={(e) => setLifecycle(e.target.value)}
          >
            <option value="">Any lifecycle</option>
            {LIFECYCLES.map((l) => (
              <option key={l} value={l}>
                {LIFECYCLE_LABEL[l]}
              </option>
            ))}
          </select>
        </div>
        {searching && <p className="list-note">Searching every session in this hat.</p>}
        {streamNote && (
          <p className="list-note" role="status">
            {streamNote}
          </p>
        )}
        {list.error && (
          <p className="form-error list-note" role="alert">
            <bdi>{list.error}</bdi>
          </p>
        )}
        {list.loading ? (
          <p className="list-note" role="status">
            Loading…
          </p>
        ) : rows.length === 0 ? (
          <p className="list-note">{searching ? 'No matches' : 'No sessions'}</p>
        ) : (
          byDay(rows, now).map(([bucket, day]) => (
            <div className="grp" key={bucket}>
              <h2 className="grp-head">
                <span className="grp-title">{BUCKET_LABEL[bucket]}</span>
                <span className="grp-count">{day.length}</span>
              </h2>
              {day.map((s) => (
                <SessionRow
                  key={s.session_id}
                  session={s}
                  selected={s.session_id === selectedId}
                  hostName={hostNames.get(s.host_id)}
                  now={now}
                />
              ))}
            </div>
          ))
        )}
        {!list.loading && more && (
          <button type="button" className="btn btn-ghost btn-sm list-more" disabled={list.loadingMore} onClick={showMore}>
            {list.loadingMore ? 'Loading…' : 'Load more'}
          </button>
        )}
      </section>
    )
  }
  ```

Create `web/src/components/SessionRow.tsx`:

  ```tsx
  // One row of the session list (frontend spec §5; plan 4c decision 7).
  //
  // Line 1: status marker, agent, title (fallback: the project directory's
  // name), relative time. Line 2: the branch (fallback: the directory's name,
  // never repeated when it is the title), the host's name, and "Resume" for a
  // parked session. The row is a link to the session, so a new tab works.
  // A failed session's reason is on the marker's hover, and a tap on "Why"
  // beside the link shows it (a button inside the link would open the session).
  import { useState } from 'react'
  import type { SessionSummary } from '../generated/view'
  import { agentLabel } from '../lib/agent'
  import { statusOf } from '../lib/status'
  import { basename, relTime } from '../lib/time'
  import { Link } from '../router'

  export function sessionPath(id: string): string {
    return `/sessions/${encodeURIComponent(id)}`
  }

  /** The row's title and its second line's place. */
  export function rowText(s: SessionSummary): { title: string; where: string } {
    const dir = basename(s.cwd)
    const title = s.title || dir || s.session_id
    const where = s.git_branch || dir
    return { title, where: where === title ? '' : where }
  }

  interface Props {
    session: SessionSummary
    selected: boolean
    hostName?: string
    /** The list's clock (`useNow`): the relative time moves with it. */
    now: number
  }

  export default function SessionRow({ session, selected, hostName, now }: Props) {
    const [why, setWhy] = useState(false)
    const status = statusOf(session)
    const { title, where } = rowText(session)
    const reason = status.tone === 'fail' ? session.failure_reason : undefined
    const markerTitle = reason ? `${status.label}: ${reason}` : status.label
    return (
      <div className="sess-item">
        <Link
          to={sessionPath(session.session_id)}
          className={'sess' + (selected ? ' active' : '')}
          aria-current={selected ? 'page' : undefined}
        >
          <div className="sess-row1">
            <span className={'sess-led led-' + status.tone} role="img" aria-label={status.label} title={markerTitle} />
            <span className="sess-kind sess-agent">{agentLabel(session.agent)}</span>
            <span className="sess-name">
              <bdi>{title}</bdi>
            </span>
            <span className="sess-time">{relTime(session.last_event_at, now)}</span>
          </div>
          <div className="sess-row2">
            {where && (
              <span className="sess-where">
                <bdi>{where}</bdi>
              </span>
            )}
            {hostName && (
              <span className="sess-machine">
                <bdi>{hostName}</bdi>
              </span>
            )}
            {(status.tone === 'attn' || status.tone === 'fail' || status.label === 'Host offline') && (
              <span className={'sess-flag sess-flag-' + status.tone}>{status.label}</span>
            )}
            {status.resume && <span className="sess-resume">Resume</span>}
          </div>
        </Link>
        {reason && (
          <>
            <button type="button" className="sess-why" aria-expanded={why} onClick={() => setWhy((open) => !open)}>
              Why it failed
            </button>
            {why && (
              <p className="sess-reason">
                <bdi>{reason}</bdi>
              </p>
            )}
          </>
        )}
      </div>
    )
  }
  ```

Create `web/src/components/SessionScope.tsx`:

  ```tsx
  // The session list's state for the whole signed-in app (frontend spec §5;
  // plan 4c decisions 8–11): one list store and one list stream, the filters,
  // the hats and the host names. The rail, the mobile list screen and the
  // session view's header all read it through `useSessionScope()`.
  //
  // It also owns what the list decides on its own:
  // - the tab title `(N) hennery`, N = the hat's sessions waiting on a question;
  // - selection (F-11): the route's id is the selection, honoured even
  //   outside the hat; a hat switch clears it; on a desktop, `/sessions` with no id opens the newest VISIBLE row.
  import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
  import { useClient } from '../app-client'
  import type { HatItem, HostItem } from '../generated/protocol'
  import { usePersistentToggle } from '../hooks/usePersistentToggle'
  import { readHat, themeOf, writeHat } from '../lib/hats'
  import { saveTheme } from '../lib/theme'
  import { navigate, type Route } from '../router'
  import { useSessionList, type SessionList } from '../store/useSessionList'

  export const HIDE_CLOSED_KEY = 'hennery.hideClosed'
  /** How long typing rests before a search goes to the server. */
  export const SEARCH_DEBOUNCE_MS = 250
  /** The longest search the server takes (it refuses a longer one with 400).
   *  An input's `maxLength` counts UTF-16 units, never fewer than the
   *  server's characters, and the query is trimmed: a capped search is never
   *  too long. */
  export const SEARCH_MAX_CHARS = 200

  export interface SessionScopeValue {
    list: SessionList
    /** The hats, or `null` until (or unless) they load. */
    hats: HatItem[] | null
    /** The selected hat's id, `''` for every hat. */
    hat: string
    chooseHat: (hat: string) => void
    /** Host names by host id, for the rows' second line. */
    hostNames: ReadonlyMap<string, string>
    hideClosed: boolean
    setHideClosed: (on: boolean) => void
    /** One lifecycle, or `''` for any. */
    lifecycle: string
    setLifecycle: (lifecycle: string) => void
    /** The search box as typed (the server gets it after a pause). */
    query: string
    setQuery: (q: string) => void
    /** The route's session id: the selection. */
    selectedId: string | null
  }

  const ScopeContext = createContext<SessionScopeValue | null>(null)

  /** The session list's state, or `null` outside a deployment with sessions. */
  export function useSessionScope(): SessionScopeValue | null {
    return useContext(ScopeContext)
  }

  function useDebounced(value: string, ms: number): string {
    const [settled, setSettled] = useState(value)
    useEffect(() => {
      const timer = setTimeout(() => setSettled(value), ms)
      return () => clearTimeout(timer)
    }, [value, ms])
    return settled
  }

  export function titleOf(waiting: number): string {
    return waiting > 0 ? `(${waiting}) hennery` : 'hennery'
  }

  export default function SessionScope({ route, desktop, children }: { route: Route; desktop: boolean; children: ReactNode }) {
    const client = useClient()
    const [hat, setHat] = useState(readHat)
    const [hats, setHats] = useState<HatItem[] | null>(null)
    const [hostNames, setHostNames] = useState<ReadonlyMap<string, string>>(new Map())
    const [hideClosed, setHideClosed] = usePersistentToggle(HIDE_CLOSED_KEY, true)
    const [lifecycle, setLifecycle] = useState('')
    const [query, setQuery] = useState('')
    const q = useDebounced(query, SEARCH_DEBOUNCE_MS)
    const list = useSessionList({ hat: hat || null, hideClosed, lifecycle: lifecycle ? [lifecycle] : undefined, q })
    const selectedId = route.name === 'session' && route.id !== undefined ? route.id : null

    // Hats and hosts: on mount, and again on leaving /hats or /hosts, the
    // screens that change them (a new or recoloured hat, a renamed or revoked
    // host), even for the other one (the rail shows the hats on both). No
    // event refetches them, and nothing here refetches the list (F-4).
    const [reads, setReads] = useState(0)
    const routeBefore = useRef(route.name)
    useEffect(() => {
      const before = routeBefore.current
      routeBefore.current = route.name
      if (before !== route.name && (before === 'hats' || before === 'hosts')) setReads((n) => n + 1)
    }, [route.name])
    useEffect(() => {
      let live = true
      client.request<HatItem[]>('GET', '/api/hats').then(
        (items) => live && Array.isArray(items) && setHats(items),
        () => {
          // Without the hats the switch offers every hat only, and the
          // colours stay as saved.
        },
      )
      client.request<HostItem[]>('GET', '/api/hosts').then(
        (items) => {
          if (!live || !Array.isArray(items)) return
          setHostNames(new Map(items.map((h) => [h.host_id, h.name])))
        },
        () => {
          // The rows show no host name.
        },
      )
      return () => {
        live = false
      }
    }, [client, reads])

    // A remembered hat that no longer exists falls back to every hat.
    useEffect(() => {
      if (hats && hat !== '' && !hats.some((h) => h.id === hat)) {
        setHat('')
        writeHat('')
      }
    }, [hats, hat])

    // The hat's colour, once the hats are known (never before: that would
    // wipe the saved colours on every load).
    useEffect(() => {
      if (!hats) return
      saveTheme(themeOf(hats.find((h) => h.id === hat)?.colour))
    }, [hats, hat])

    // The hat's count, kept through a search or a lifecycle filter: the
    // server's, or the rows' that the list store carries (`waitingCount`).
    const waiting = list.counts.waiting
    useEffect(() => {
      document.title = titleOf(waiting)
    }, [waiting])
    useEffect(() => () => void (document.title = 'hennery'), [])

    // A hat switch clears the selection, whatever hat the session is in: back
    // to `/sessions`, where a desktop then opens the new hat's newest row.
    const chooseHat = useCallback(
      (next: string) => {
        if (next === hat) return
        if (selectedId !== null) navigate('/sessions')
        setHat(next)
        writeHat(next)
      },
      [hat, selectedId],
    )

    // Desktop auto-select: only on `/sessions` with no id, only a visible row.
    const first = list.shown[0]?.session_id
    useEffect(() => {
      if (!desktop || route.name !== 'sessions' || first === undefined) return
      navigate(`/sessions/${encodeURIComponent(first)}`, { replace: true })
    }, [desktop, route.name, first])

    const value = useMemo<SessionScopeValue>(
      () => ({
        list,
        hats,
        hat,
        chooseHat,
        hostNames,
        hideClosed,
        setHideClosed,
        lifecycle,
        setLifecycle,
        query,
        setQuery,
        selectedId,
      }),
      [list, hats, hat, chooseHat, hostNames, hideClosed, setHideClosed, lifecycle, query, selectedId],
    )
    return <ScopeContext.Provider value={value}>{children}</ScopeContext.Provider>
  }

  /** The rail's and the top bar's count of sessions waiting on a question. */
  export function WaitingBadge() {
    const scope = useSessionScope()
    const n = scope?.list.counts.waiting ?? 0
    if (n === 0) return null
    const text = `${n} waiting on a question`
    return (
      <span className="badge badge-attn" role="status" aria-label={text} title={text}>
        {n}
      </span>
    )
  }
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
  import SignOut from './SignOut'

  ```

with:

  ```tsx
  import SignOut from './SignOut'
  import { useMediaQuery } from '../hooks/useMediaQuery'
  import SessionList from './SessionList'
  import SessionScope, { WaitingBadge } from './SessionScope'

  /** The rail's width and up (frontend spec §2). */
  export const DESKTOP = '(min-width: 768px)'

  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
    const [attempt, setAttempt] = useState(0)

  ```

with:

  ```tsx
    const [attempt, setAttempt] = useState(0)
    const desktop = useMediaQuery(DESKTOP)

  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx

    return (
      <div className="app">
  ```

with:

  ```tsx

    const frame = (
      <div className="app">
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
            <span className="brand-name">hennery</span>
          </div>
  ```

with:

  ```tsx
            <span className="brand-name">hennery</span>
            <WaitingBadge />
          </div>
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
          <div className="rail-scroll" />
  ```

with:

  ```tsx
          <div className="rail-scroll">{desktop && <SessionList />}</div>
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
            </div>
          </header>
  ```

with:

  ```tsx
            </div>
            <WaitingBadge />
          </header>
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
              <Hats />
            ) : route.name === 'session' ? (
  ```

with:

  ```tsx
              <Hats />
            ) : route.name === 'sessions' && !desktop ? (
              <SessionList screen />
            ) : route.name === 'session' ? (
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
    )
  }
  ```

with:

  ```tsx
    )
    return views.includes('sessions') ? (
      <SessionScope route={route} desktop={desktop}>
        {frame}
      </SessionScope>
    ) : (
      frame
    )
  }
  ```

Create `web/src/hooks/useNow.ts`:

  ```ts
  import { useEffect, useState } from 'react'

  /**
   * The time now, read again every `everyMs` and whenever the page's
   * visibility changes (a tab brought back after a night asleep, whose timers
   * the browser held back). What depends on the clock alone, a day heading or
   * "5m ago", then moves on an idle page too, with no other render to wait
   * for (F-8: a "Today" heading must not lie the morning after).
   */
  export function useNow(everyMs: number): number {
    const [now, setNow] = useState(() => Date.now())
    useEffect(() => {
      const read = () => setNow(Date.now())
      const timer = setInterval(read, everyMs)
      // Any change: a tab that becomes hidden reads once more, harmlessly.
      document.addEventListener('visibilitychange', read)
      return () => {
        clearInterval(timer)
        document.removeEventListener('visibilitychange', read)
      }
    }, [everyMs])
    return now
  }
  ```

In `web/src/index.css`, replace:

  ```css
  .dialog .modal-body p { margin:0 0 14px; }
  ```

with:

  ```css
  .dialog .modal-body p { margin:0 0 14px; }

  /* ── The session list (4c): in the rail from 768px, the whole screen under it ── */
  .slist { display:flex; flex-direction:column; min-width:0; }
  .slist-screen { padding:8px 8px 16px; }
  .slist-title { font-family:var(--font-display); font-weight:800; font-size:20px; margin:8px 10px 10px; }
  .slist .rail-search { margin:6px 6px 10px; }
  .list-hats { flex-wrap:wrap; margin:4px 6px 8px; }
  .list-filters { display:flex; gap:8px; align-items:center; margin:0 6px 8px; }
  .list-filters .toggle:disabled, .list-select:disabled { opacity:.5; }
  .list-select { flex:1; min-width:0; border:1px solid var(--border); background:var(--surface); color:var(--fg-2); border-radius:var(--r-full); padding:7px 12px; font-size:13px; font-weight:600; }
  .list-note { font-size:12.5px; color:var(--fg-muted); margin:6px 10px; }
  .list-more { margin:10px 6px 0; }
  h2.grp-head { margin:0; font-size:inherit; font-weight:inherit; }
  .slist-screen .grp-head { background:var(--bg); }
  .sess { text-decoration:none; color:inherit; }
  .sess-agent { font-size:10.5px; font-weight:600; }
  .sess-flag { font-size:10px; font-weight:700; letter-spacing:.04em; text-transform:uppercase; flex:none; }
  .sess-flag-attn { color:var(--st-attn); }
  .sess-flag-fail { color:var(--st-attn); }
  .sess-flag-idle { color:var(--fg-muted); }
  .sess-why { margin:0 10px 4px; border:none; background:none; padding:2px 0; font-size:11px; font-weight:600; color:var(--st-attn); text-decoration:underline; }
  .sess-reason { margin:0 10px 6px; font-size:12px; color:var(--fg-2); overflow-wrap:anywhere; }
  .led-fail { background:transparent; box-shadow:inset 0 0 0 2px var(--st-attn); }
  .rail-head .badge, .mtopbar .badge { margin-left:auto; }
  ```

Create `web/src/lib/agent.ts`:

  ```ts
  // Who speaks in a transcript (F-14): the session's agent, never a name
  // assumed for every session. The user is "You".

  const KNOWN: Record<string, string> = { claude: 'Claude', codex: 'Codex' }

  /** The agent's name as a transcript shows it: a known agent by its proper
   *  name, anything else as given, and "Agent" while the session is unknown. */
  export function agentLabel(agent: string | undefined): string {
    if (!agent) return 'Agent'
    return Object.hasOwn(KNOWN, agent) ? KNOWN[agent] : agent
  }

  export const USER_LABEL = 'You'

  /** An avatar's one letter: the label's first character, never anyone's
   *  initials. */
  export function avatarLetter(label: string): string {
    return Array.from(label.trim())[0]?.toUpperCase() ?? '?'
  }
  ```

Create `web/src/lib/hats.ts`:

  ```ts
  // The selected hat (frontend spec §5): server data, picked by the operator,
  // remembered in this browser. `''` means every hat.
  import type { Theme } from './theme'

  export const HAT_KEY = 'hennery.hat'

  export function readHat(): string {
    try {
      return localStorage.getItem(HAT_KEY) ?? ''
    } catch {
      return ''
    }
  }

  export function writeHat(hat: string): void {
    try {
      localStorage.setItem(HAT_KEY, hat)
    } catch {
      // No storage: the hat holds for this load only.
    }
  }

  const COLOUR = /^#[0-9a-fA-F]{6}$/

  /** `colour` darkened, for the gradient's second stop (the default pair is
   *  about this far apart). */
  function shade(colour: string): string {
    const n = parseInt(colour.slice(1), 16)
    const channel = (shift: number) => Math.round(((n >> shift) & 0xff) * 0.77)
    return '#' + [16, 8, 0].map((shift) => channel(shift).toString(16).padStart(2, '0')).join('')
  }

  /** The theme a hat's colour gives, or `null` (the default colours) when it
   *  is not `#rrggbb`. */
  export function themeOf(colour: string | undefined): Theme | null {
    if (colour === undefined || !COLOUR.test(colour)) return null
    const accent = colour.toLowerCase()
    return { accent, accent2: shade(accent) }
  }
  ```

Create `web/src/lib/status.ts`:

  ```ts
  // A session's status marker in the list (frontend spec §5; plan 4c
  // decisions 7 and 8). It reads the server's two axes, lifecycle and
  // activity, plus `question_waits` and `presumed_parked`, directly: there is
  // no staleness heuristic and no "actionable" guess (F-9).

  /** The marker's colour: `attn` is the strongest and means only "waiting on
   *  a question"; `fail` is a hollow red ring. */
  export type Tone = 'attn' | 'run' | 'wait' | 'idle' | 'fail'

  export interface Marker {
    tone: Tone
    label: string
    /** The row offers "Resume" (a parked session). */
    resume: boolean
  }

  export interface StatusInput {
    lifecycle: string
    activity?: string
    question_waits: boolean
    presumed_parked: boolean
  }

  /**
   * The marker for a session.
   *
   * Waiting on a question wins over every lifecycle, so a row's marker always
   * agrees with the waiting count (tab title, header badge), which counts
   * `blocked || question_waits` whatever the lifecycle. The wording is never
   * "needs you": the summary cannot tell an answer already in flight.
   */
  export function statusOf(s: StatusInput): Marker {
    if (s.activity === 'blocked' || s.question_waits) return { tone: 'attn', label: 'Waiting on a question', resume: false }
    switch (s.lifecycle) {
      case 'failed':
        return { tone: 'fail', label: 'Failed', resume: false }
      case 'closed':
        return { tone: 'idle', label: 'Closed', resume: false }
      case 'parked':
        // Parked only because its host went quiet: it may still be running
        // there. It is still parked, so Resume stays (the footer adds the
        // "host offline" note, decision 35).
        return s.presumed_parked
          ? { tone: 'idle', label: 'Host offline', resume: true }
          : { tone: 'idle', label: 'Parked', resume: true }
      case 'starting':
        return { tone: 'wait', label: 'Starting', resume: false }
      case 'active':
        return s.activity === 'running'
          ? { tone: 'run', label: 'Running', resume: false }
          : { tone: 'idle', label: 'Idle', resume: false }
      default:
        // A lifecycle from a newer server: shown by its name, never hidden.
        return { tone: 'idle', label: s.lifecycle || 'Unknown', resume: false }
    }
  }
  ```

In `web/src/lib/time.ts`, replace:

  ```ts
  export function relTime(iso: string): string {
    if (!iso) return ''
    const s = Math.floor((Date.now() - new Date(iso).getTime()) / 1000)
  ```

with:

  ```ts
  /** How long before `now` `iso` was, in words. A list passes its own ticking
   *  `now` (`useNow`), so its rows move on while nothing else renders. */
  export function relTime(iso: string, now: number = Date.now()): string {
    if (!iso) return ''
    const s = Math.floor((now - new Date(iso).getTime()) / 1000)
  ```

In `web/src/store/sessionList.ts`, replace:

  ```ts
  /** The sessions of the hat waiting on a question: the tab title's and the
   *  header badge's number. The server's count when it sent one
   *  (`state.waiting`, already the hat's: `state` is the hat's query's);
   *  otherwise the loaded rows'. */
  export function waitingCount(state: ListState, hat: string | null | undefined): number {
    if (state.waiting !== undefined) return state.waiting
    let n = 0
    for (const s of state.all.values()) {
      if (inHat(s, hat) && (s.activity === 'blocked' || s.question_waits)) n++
    }
    return n
  }
  ```

with:

  ```ts
  /**
   * The sessions of the hat waiting on a question (`blocked || question_waits`):
   * the tab title's and the header badge's number, read by both through
   * `useSessionList().counts.waiting`. It is the hat's number, never a query's:
   * a search or a lifecycle filter leaves it as it was.
   *
   * The server's count (`SummaryPage.waiting`, then `waiting_changed`) is used
   * whenever it was sent (`waitingCount`): it is the hat's whatever the query.
   * THE FALLBACK, for a page with no `waiting`, counts the rows loaded, and
   * the security review allowed that only with its A4: a search or a lifecycle
   * filter starts `all` over with its own rows only, so while one is set
   * `useSessionList` carries the ids counted from the hat's unfiltered list,
   * brought up to date by every summary received since (`carryWaiting`). A
   * session removed meanwhile stays counted until the filter is cleared.
   */
  export function waitingIds(state: ListState, hat: string | null | undefined): Set<string> {
    const ids = new Set<string>()
    for (const s of state.all.values()) {
      if (inHat(s, hat) && isWaiting(s)) ids.add(s.session_id)
    }
    return ids
  }

  /** The server's count when it sent one (`state.waiting`, already the
   *  hat's: `state` is the hat's query's); otherwise the loaded rows'. */
  export function waitingCount(state: ListState, hat: string | null | undefined): number {
    return state.waiting ?? waitingIds(state, hat).size
  }

  /** `held`, the ids counted from the hat's unfiltered list, with every
   *  summary `state` holds applied: each came after the count. */
  export function carryWaiting(held: ReadonlySet<string>, state: ListState, hat: string | null | undefined): Set<string> {
    const ids = new Set(held)
    for (const s of state.all.values()) {
      if (inHat(s, hat) && isWaiting(s)) ids.add(s.session_id)
      else ids.delete(s.session_id)
    }
    return ids
  }

  const isWaiting = (s: SessionSummary) => s.activity === 'blocked' || s.question_waits
  ```

In `web/src/store/useSessionList.ts`, replace:

  ```ts
    waitingCount,
  ```

with:

  ```ts
    carryWaiting,
    waitingCount,
    waitingIds,
  ```

In `web/src/store/useSessionList.ts`, replace:

  ```ts
    searching: boolean
  }
  ```

with:

  ```ts
    searching: boolean
    /** The server query, as a key: a new one is a new list (its pages start
     *  over), whatever filter changed. */
    queryKey: string
  }
  ```

In `web/src/store/useSessionList.ts`, replace:

  ```ts
    const waiting = useMemo(() => waitingCount(store, hat), [store, hat])
  ```

with:

  ```ts
    // The hat's waiting count, never the query's (see `waitingCount`):
    // - the server's, once a page of this query is loaded: it is the hat's
    //   whatever the query; while a new query's page loads, the hat's last
    //   server count holds (never another hat's);
    // - THE FALLBACK, when the page had no `waiting`: counted from the hat's
    //   unfiltered list once it is loaded, and carried, with the summaries
    //   received since, while a search or a lifecycle filter is set or while
    //   the unfiltered list loads again (the security review's A4).
    // A query whose first page was refused (a 4xx: a search over 200
    // characters, or with a control character) never loads one: its store
    // stays `EMPTY_LIST`, the same object, as no stream opens without a page
    // (an `epoch` of '' would not tell, a server may send one). It is held
    // like a page still loading, so the count does not fall to 0. Neither
    // `store.waiting === undefined` (a page with no count is counted, not
    // held) nor `error` (a failed further page sets it while a page is held)
    // tells it.
    const unfiltered = !searching(filters) && !(lifecycle && lifecycle.length > 0)
    const held = useRef<{ hat: string | null; ids: ReadonlySet<string>; server?: number }>({ hat: null, ids: new Set() })
    const waiting = useMemo(() => {
      const scope = hat ?? null
      const before = held.current.hat === scope ? held.current : { hat: scope, ids: new Set<string>() }
      const unloaded = snapshot.loading || store === EMPTY_LIST
      if (!unloaded && store.waiting !== undefined) {
        held.current = { hat: scope, ids: before.ids, server: store.waiting }
        return waitingCount(store, hat)
      }
      if (unloaded && before.server !== undefined) return before.server
      const ids = unfiltered && !unloaded ? waitingIds(store, hat) : carryWaiting(before.ids, store, hat)
      held.current = { hat: scope, ids }
      return ids.size
    }, [store, hat, unfiltered, snapshot.loading])
  ```

In `web/src/store/useSessionList.ts`, replace:

  ```ts
      searching: searching(filters),
    }
  ```

with:

  ```ts
      searching: searching(filters),
      queryKey: key,
    }
  ```

- [ ] **Step 4: Run the checks**

Run: `nix develop -c sh -c 'cd web && pnpm typecheck && pnpm test && pnpm build'`
Expected: PASS, 534 tests.

- [ ] **Step 5: Revert-probes** (each must fail the test named; restore after each)

75 probes, run by script, all fail as they should. One draft guard (desktop auto-select waiting for the list to load) was removed rather than probed: the list empties on every new query, so while it loads there is no row to pick.
- **Status** (11): blocked and `question_waits` wait; the words are never "needs you"; waiting wins over the lifecycle; failed's tone; parked offers Resume; presumed parked; running animates; starting; closed has no Resume; an unknown lifecycle by name.
- **The row** (14): the title falls back to the directory, then the id; the branch first on line two, else the directory, never repeating the title; the reason only when failed, on hover and on a tap; Resume on parked; the waiting flag in words; the path encodes the id; the agent from the session; the host's name; `aria-current` when selected.
- **Agent and hats** (8): known names; own keys only; no agent is "Agent"; the avatar's letter; the colour anchors; the accent lowercased; the `hennery.` key; every hat by default.
- **The scope** (18): Hide closed on by default, and its key; a hat switch clears the selection, a switch to every hat too, the same hat is no switch; the hat remembered; auto-select on a desktop only, only without an id, the newest visible row; colours only once hats load; a hat gone falls back; the hat's colour to `saveTheme`; the tab title's count; the hat, the search and the lifecycle filter reach the list; host names; the badge hidden at zero, and reading the count.
- **The list** (13): 50 rows at a time; held rows before a fetch; "Load more" for held rows; Hide closed and the lifecycle filter off while searching; headings are `<h2>`; buckets in order and from now; the selected row marked; "Reconnecting…"; the screen on a phone, the rail on a desktop, the badge in the rail.
- **Through the UI** (9): an upsert updates and inserts a row; `session_removed`; a resync refetches; a row out of the hat, and a closed one under Hide closed, leaves; a search never gains a row; the count of blocked, of `question_waits`, within the hat.

Load: 4 parallel copies of the five test files, 3 rounds: 12 of 12 green.


**Re-run on the final code:** the three count probes were retargeted to the fallback's `waitingIds` (with the server's count the hook's own line is an equivalent mutant: the count is the hat's already). The amendments' refused-search, cap and re-read probes are counted with Task 2's. The task review's fixes added nine: the list's clock (each minute, and when the tab is shown again), a row's time read from it, the row limit reset by a new query, "Resynced", an id that needs encoding through the address and a hat switch, the selected row, the refused search's alert.

- [ ] **Step 6: Commit**

```bash
git add web/src
git commit -m "feat(web): the session list: buckets, rows, filters, search, hats and the waiting count"
```

---

### Task 4: The read-only session view and Markdown

`/sessions/:id`: the header, the transcript of every item kind, safe Markdown, the stream's state; and the gated measurement that sets decision 20 (decisions 13–20).

**Files:**
- Create: `web/src/screens/Session.tsx`; `web/src/components/Transcript.tsx`, `SessionHeader.tsx`, `StepList.tsx`, `Markdown.tsx`, `ItemBoundary.tsx`; `web/src/components/items/{AgentMessage,Marker,PlanItem,QuestionCard,Thinking,ToolCall,Unrecognised,UserTurn,parts}.tsx` and `types.ts`; `web/src/api/names.ts`; their tests; `web/e2e/windowing.spec.ts`.
- Modify: `web/package.json` and `web/pnpm-lock.yaml` (the five Markdown packages), `web/src/components/Shell.tsx` (the view replaces the placeholder), `web/src/App.test.tsx`, `web/src/index.css` (the view's rules), `web/src/lib/time.ts` (one cached clock formatter) and its test.

- [ ] **Step 1: Write the tests**

Test files: `web/e2e/windowing.spec.ts`, `web/src/App.test.tsx`, `web/src/api/names.test.ts`, `web/src/components/Markdown.test.tsx`, `web/src/components/StepList.test.tsx`, `web/src/components/Transcript.rows.test.tsx`, `web/src/components/Transcript.test.tsx`, `web/src/components/items/items.test.tsx`, `web/src/deps.test.ts`, `web/src/lib/time.test.ts`, `web/src/screens/Session.test.tsx`.

Create `web/e2e/windowing.spec.ts`:

  ```ts
  // How long a transcript takes to render and to scroll as it grows (client
  // view spec §9 OQ1, frontend spec §15): the measurement that found the
  // threshold, between 200 and 500 items under load, which set the session
  // view's tail window at the newest 200 items (plan 4c decision 20). Kept so
  // it can be taken again on a quiet machine, and when the item renderers
  // change.
  //
  // - No server: the built UI (`pnpm build`, `dist/`) is served to Chromium
  //   from `page.route` on an origin of its own, and every `/api` route the
  //   session view asks for is answered here. An `/api` request with no answer
  //   fails the run, so a login screen is never what gets timed.
  // - The session is the golden fixtures' items (both pinned adapters),
  //   repeated with fresh ids and turn ids until it holds N items, all in its
  //   first page. Its stream is held open and never sends.
  // - First render: from the page's response to two frames after the Nth item
  //   is in the DOM. Scrolling: 240 frames of 400 px each from the end, the
  //   gaps between frames and any long task, beside 240 frames standing still
  //   (the frame rate the browser keeps on this machine anyway). A row whose
  //   still frames are slower than about one frame (p95 > 20 ms) is marked
  //   not valid: the machine was busy.
  // - Run at the desktop's speed and with the CPU slowed 4× (a phone).
  //
  // A timing on a shared CI runner says little, so this runs only when asked:
  // `HENNERY_WINDOWING=1 pnpm exec playwright test windowing` after
  // `pnpm build`, with the flake's browsers.
  import { expect, test, type Page, type Route } from '@playwright/test'
  import { existsSync, readFileSync, readdirSync } from 'node:fs'
  import { loadavg } from 'node:os'
  import { extname, join, resolve } from 'node:path'

  test.skip(!process.env.HENNERY_WINDOWING, 'a measurement: set HENNERY_WINDOWING=1 to take it')
  test.describe.configure({ mode: 'serial', timeout: 300_000 })

  const ORIGIN = 'http://hennery.test'
  const ID = 'long-session'
  const DIST = resolve(process.cwd(), 'dist')
  const FIXTURES = resolve(process.cwd(), '../crates/hennery-view/tests/fixtures')
  const SIZES = [200, 500, 1000, 2000, 5000]
  const SCROLL_FRAMES = 240
  const SCROLL_STEP = 400
  const PIXEL = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='

  type Raw = Record<string, unknown> & { id: string; turn_id?: string }

  const GOLDEN: Raw[] = readdirSync(FIXTURES)
    .filter((f) => f.endsWith('.items.json'))
    .sort()
    .flatMap((f) => JSON.parse(readFileSync(join(FIXTURES, f), 'utf8')) as Raw[])

  /** N items: the golden ones again and again, each pass with its own ids and
   *  turns, so the store holds every one in its own group. */
  function longSession(n: number): Raw[] {
    const out: Raw[] = []
    for (let pass = 0; out.length < n; pass++) {
      for (const item of GOLDEN) {
        if (out.length === n) break
        out.push({ ...item, id: `p${pass}:${item.id}`, turn_id: `p${pass}:${item.turn_id ?? 'start'}` })
      }
    }
    return out
  }

  const TYPES: Record<string, string> = {
    '.html': 'text/html',
    '.js': 'text/javascript',
    '.css': 'text/css',
    '.svg': 'image/svg+xml',
    '.png': 'image/png',
    '.ico': 'image/x-icon',
    '.woff2': 'font/woff2',
  }

  interface Served {
    unanswered: string[]
    release: () => Promise<void>
  }

  async function serve(page: Page, items: Raw[]): Promise<Served> {
    const unanswered: string[] = []
    const held: Route[] = []
    const json = (route: Route, body: unknown) =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(body) })
    const detail = {
      session_id: ID,
      host_id: 'h1',
      agent: 'claude',
      cwd: '/srv/work/long',
      hat_id: 'hat1',
      title: 'A long session',
      lifecycle: 'active',
      activity: 'idle',
      presumed_parked: false,
      created_at: '2026-10-02T09:00:00.000Z',
      last_event_at: '2026-10-02T10:00:00.000Z',
      pending: [],
    }
    const page_ = JSON.stringify({ items, older: false, epoch: 'e1', revision: items.length })
    await page.route(`${ORIGIN}/**`, async (route) => {
      const { pathname } = new URL(route.request().url())
      if (pathname.startsWith('/api/')) {
        if (pathname === '/api/capabilities') return json(route, { mode: 'full', features: [] })
        if (pathname === `/api/view/sessions/${ID}`)
          return route.fulfill({ status: 200, contentType: 'application/json', body: page_ })
        if (pathname === `/api/stream/view/sessions/${ID}`) {
          held.push(route)
          return
        }
        if (pathname === `/api/sessions/${ID}`) return json(route, detail)
        if (pathname === `/api/sessions/${ID}/catalog`) return json(route, { session_id: ID, config_options: [], commands: [] })
        if (pathname === '/api/hosts') return json(route, [{ host_id: 'h1', name: 'build-box' }])
        if (pathname === '/api/hats') return json(route, [])
        // A prompt's image: one pixel, whatever its hash.
        if (pathname.startsWith('/api/attachments/'))
          return route.fulfill({ status: 200, contentType: 'image/png', body: Buffer.from(PIXEL, 'base64') })
        unanswered.push(pathname)
        return route.fulfill({ status: 404, contentType: 'application/json', body: '{"code":"not_found","message":"no"}' })
      }
      const file = join(DIST, pathname)
      if (pathname !== '/' && existsSync(file) && !file.endsWith('/'))
        return route.fulfill({ status: 200, contentType: TYPES[extname(file)] ?? 'application/octet-stream', body: readFileSync(file) })
      return route.fulfill({ status: 200, contentType: 'text/html', body: readFileSync(join(DIST, 'index.html')) })
    })
    return {
      unanswered,
      release: async () => {
        for (const route of held.splice(0)) await route.abort().catch(() => {})
      },
    }
  }

  /** Installed before the app: the time the page's response came, the time
   *  the Nth item was painted, and every long task. */
  function probe(want: number) {
    const m = { longtasks: [] as [number, number][], t0: 0, t1: 0 }
    ;(window as unknown as { __m: typeof m }).__m = m
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) m.longtasks.push([e.startTime, e.duration])
    }).observe({ type: 'longtask', buffered: true })
    const original = window.fetch
    window.fetch = async (...args: Parameters<typeof fetch>) => {
      const response = await original(...args)
      const url = String(args[0] instanceof Request ? args[0].url : args[0])
      if (!m.t0 && /\/api\/view\/sessions\/[^/?]+(\?|$)/.test(url)) m.t0 = performance.now()
      return response
    }
    const tick = () => {
      const inner = document.querySelector('.transcript-inner')
      if (m.t0 && inner && inner.children.length >= want) {
        requestAnimationFrame(() => requestAnimationFrame(() => (m.t1 = performance.now())))
        return
      }
      requestAnimationFrame(tick)
    }
    requestAnimationFrame(tick)
  }

  async function scrollCost(page: Page) {
    return page.evaluate(
      async ({ frames, step }) => {
        const m = (window as unknown as { __m: { longtasks: [number, number][] } }).__m
        const frame = () => new Promise<number>((r) => requestAnimationFrame(r))
        const el = document.querySelector('.transcript') as HTMLElement
        el.scrollTop = el.scrollHeight
        await frame()
        await frame()
        // The same number of frames standing still first: the browser's own
        // frame rate on this machine, which the scrolling is read against.
        const run = async (move: boolean) => {
          const from = performance.now()
          const gaps: number[] = []
          let last = await frame()
          for (let i = 0; i < frames; i++) {
            if (move) el.scrollTop -= step
            const now = await frame()
            gaps.push(now - last)
            last = now
          }
          gaps.sort((a, b) => a - b)
          const long = m.longtasks.filter(([start]) => start >= from)
          return {
            mean: gaps.reduce((a, b) => a + b, 0) / gaps.length,
            p95: gaps[Math.floor(gaps.length * 0.95)],
            max: gaps[gaps.length - 1],
            longTasks: long.length,
            longestTask: Math.max(0, ...long.map(([, d]) => d)),
          }
        }
        const still = await run(false)
        const moving = await run(true)
        return { still, moving, height: el.scrollHeight }
      },
      { frames: SCROLL_FRAMES, step: SCROLL_STEP },
    )
  }

  for (const slowdown of [1, 4]) {
    for (const n of SIZES) {
      test(`${n} items, CPU ×${slowdown}`, async ({ browser }) => {
        const context = await browser.newContext({ viewport: { width: 1280, height: 800 } })
        const page = await context.newPage()
        const served = await serve(page, longSession(n))
        try {
          if (slowdown > 1) {
            const cdp = await context.newCDPSession(page)
            await cdp.send('Emulation.setCPUThrottlingRate', { rate: slowdown })
          }
          await page.addInitScript(probe, n)
          await page.goto(`${ORIGIN}/sessions/${ID}`)
          await page.waitForFunction(() => (window as unknown as { __m: { t1: number } }).__m.t1 > 0, undefined, {
            timeout: 240_000,
            polling: 100,
          })
          const first = await page.evaluate(() => {
            const m = (window as unknown as { __m: { t0: number; t1: number; longtasks: [number, number][] } }).__m
            const during = m.longtasks.filter(([start]) => start >= m.t0 && start <= m.t1)
            return {
              firstRenderMs: m.t1 - m.t0,
              longestTaskMs: Math.max(0, ...during.map(([, d]) => d)),
              domNodes: document.getElementsByTagName('*').length,
              heapMB: ((performance as unknown as { memory?: { usedJSHeapSize: number } }).memory?.usedJSHeapSize ?? 0) / 2 ** 20,
            }
          })
          const scroll = await scrollCost(page)
          expect(served.unanswered).toEqual([])
          expect(await page.locator('.transcript-inner > *').count()).toBe(n)
          // A row counts only if the browser kept its frame rate standing
          // still: else it measured the machine, not the transcript.
          const row = { n, slowdown, load1: loadavg()[0], valid: scroll.still.p95 <= 20, ...first, scroll }
          console.log(`WINDOWING ${JSON.stringify(row)}`)
          test.info().annotations.push({ type: 'windowing', description: JSON.stringify(row) })
        } finally {
          await served.release()
          await context.close()
        }
      })
    }
  }
  ```

In `web/src/App.test.tsx`, replace:

  ```tsx
      expect(await screen.findByText('<b>x</b>')).toBeInTheDocument()
  ```

with:

  ```tsx
      // The view shows the id while it loads, and with the deleted notice
      // (this stub has no session): text either way.
      await waitFor(() => expect(screen.getByText('<b>x</b>')).toBeInTheDocument())
      expect(document.querySelector('b')).toBeNull()
      // The session view, not a placeholder: this stub knows no session.
      expect(await screen.findByRole('heading', { name: 'This session was deleted' })).toBeInTheDocument()
  ```

Create `web/src/api/names.test.ts`:

  ```ts
  import { describe, expect, it } from 'vitest'
  import { namesOf } from './names'

  describe('namesOf', () => {
    it('maps each entry’s id to its name', () => {
      const hosts = namesOf([{ host_id: 'h1', name: 'build-box' }, { host_id: 'h2', name: 'laptop' }], 'host_id')
      expect([...hosts]).toEqual([['h1', 'build-box'], ['h2', 'laptop']])
      expect([...namesOf([{ id: 'hat1', name: 'Work' }], 'id')]).toEqual([['hat1', 'Work']])
    })

    it.each([null, undefined, {}, { code: 'not_found' }, 'hosts', 3])('gives no names for %o, which is not a list', (value) => {
      expect(namesOf(value, 'id').size).toBe(0)
    })

    it('skips entries without a string id and name', () => {
      const names = namesOf([null, 'x', { id: 1, name: 'n' }, { id: 'a' }, { id: 'b', name: 'B' }], 'id')
      expect([...names]).toEqual([['b', 'B']])
    })
  })
  ```

Create `web/src/components/Markdown.test.tsx`:

  ````tsx
  import { render, screen } from '@testing-library/react'
  import { readFileSync } from 'node:fs'
  import { join } from 'node:path'
  import { describe, expect, it, vi } from 'vitest'
  import Markdown, { MarkdownImage } from './Markdown'

  describe('Markdown', () => {
    it('highlights a fenced block in a common language', () => {
      const { container } = render(<Markdown>{'```js\nconst x = 1\n```'}</Markdown>)
      expect(container.querySelector('code.hljs')).not.toBeNull()
      expect(container.querySelector('.hljs-keyword, .hljs-number')).not.toBeNull()
    })

    it('leaves a fence in an unknown language plain, without throwing', () => {
      const errors = vi.spyOn(console, 'error').mockImplementation(() => {})
      try {
        const { container } = render(<Markdown>{'```nix\npkgs.hello\n```\n\n```no-such-language\nx\n```'}</Markdown>)
        expect(container.querySelectorAll('code')).toHaveLength(2)
        expect(container.textContent).toContain('pkgs.hello')
        expect(container.querySelector('[class*="hljs-"]')).toBeNull()
        expect(errors).not.toHaveBeenCalled()
      } finally {
        errors.mockRestore()
      }
    })

    it('never guesses a language for a fence without one', () => {
      const { container } = render(<Markdown>{'```\nfunction f() { return 1 }\nconst x = f()\n```'}</Markdown>)
      const code = container.querySelector('code') as HTMLElement
      // Guessing would mark the block and name the language it guessed.
      expect(code.className).toBe('')
      expect(container.querySelector('[class*="hljs"]')).toBeNull()
    })

    it('renders a single newline as a line break', () => {
      const { container } = render(<Markdown>{'line one\nline two'}</Markdown>)
      expect(container.querySelector('br')).not.toBeNull()
    })

    it('renders GFM tables, task lists and strikethrough, and lists', () => {
      const md = ['| a | b |', '| - | - |', '| 1 | 2 |', '', '- [x] done', '- [ ] todo', '', '~~gone~~', '', '1. one', '2. two'].join('\n')
      const { container } = render(<Markdown>{md}</Markdown>)
      expect(container.querySelector('table')).not.toBeNull()
      expect(container.querySelectorAll('input[type="checkbox"]')).toHaveLength(2)
      expect(container.querySelector('del')).not.toBeNull()
      expect(container.querySelector('ol')).not.toBeNull()
    })

    it('renders a blockquote', () => {
      const { container } = render(<Markdown>{'> quoted'}</Markdown>)
      expect(container.querySelector('blockquote')).not.toBeNull()
    })

    describe('raw HTML is text, never markup', () => {
      it.each([
        ['a script block', '<script>alert(1)</script>', 'script'],
        ['an image with a handler', '<img src=x onerror=alert(1)>', 'img'],
        ['inline bold', 'a <b>x</b> b', 'b'],
        ['a details block', '<details><summary>more</summary>body</details>', 'details'],
      ])('%s', (_name, source, tag) => {
        const { container } = render(<Markdown>{source}</Markdown>)
        expect(container.querySelector(tag)).toBeNull()
        expect(container.querySelector('[onerror]')).toBeNull()
        // The characters are all still there, as text.
        expect(container.textContent).toContain(source.replace(/^a | b$/g, ''))
      })
    })

    it('opens links in a new tab without an opener', () => {
      render(<Markdown>{'[docs](https://example.com/a)'}</Markdown>)
      const a = screen.getByRole('link', { name: 'docs' }) as HTMLAnchorElement
      expect(a.href).toBe('https://example.com/a')
      expect(a.target).toBe('_blank')
      expect(a.rel).toBe('noopener noreferrer')
    })

    it('opens an autolink in a new tab without an opener too', () => {
      render(<Markdown>{'see https://example.com/b'}</Markdown>)
      const a = screen.getByRole('link', { name: 'https://example.com/b' }) as HTMLAnchorElement
      expect(a.rel).toBe('noopener noreferrer')
      expect(a.target).toBe('_blank')
    })

    it.each(['javascript:alert(1)', 'JaVaScRiPt:alert(1)', 'vbscript:x', 'data:text/html,hi'])('gives a %s link no href at all', (url) => {
      const { container } = render(<Markdown>{`[click](${url})`}</Markdown>)
      const a = container.querySelector('a')
      expect(a?.textContent).toBe('click')
      // Not even an empty one, which would link to this page.
      expect(a?.hasAttribute('href')).toBe(false)
    })

    it('links a footnote to its note, on the same page', () => {
      const { container } = render(<Markdown>{'a claim[^1]\n\n[^1]: the source'}</Markdown>)
      const ref = container.querySelector('a[data-footnote-ref]') as HTMLAnchorElement
      const target = ref.getAttribute('href')!.slice(1)
      expect(container.querySelector(`[id="${target}"]`)?.textContent).toContain('the source')
      expect(ref.hasAttribute('target')).toBe(false)
      // Every id Markdown makes carries the prefix that keeps it off `window`.
      for (const el of Array.from(container.querySelectorAll('[id]'))) {
        if (el.id !== 'footnote-label') expect(el.id).toMatch(/^user-content-/)
      }
    })

    it('makes no id but a footnote’s, and no name, from hostile input', () => {
      const source = [
        '# Heading one',
        '## user-content-x',
        'a claim[^note] and another[^2]',
        '<a name="x" id="y">anchor</a> and <a name=bare>b</a>',
        '<form id="login"><input name="password"></form>',
        '<div id="footnote-label">fake label</div>',
        '<img id="z" name="z" src="x">',
        '',
        '[^note]: the source',
        '[^2]: another <span id="inner">source</span>',
      ].join('\n')
      const { container } = render(<Markdown>{source}</Markdown>)
      const ids = Array.from(container.querySelectorAll('[id]')).map((el) => el.id)
      // The footnotes are there, so their ids were made.
      expect(ids.filter((id) => id.startsWith('user-content-fn')).length).toBeGreaterThan(0)
      for (const id of ids) expect(id === 'footnote-label' || /^user-content-/.test(id)).toBe(true)
      expect(ids.filter((id) => id === 'footnote-label')).toHaveLength(1)
      expect(container.querySelector('[name]')).toBeNull()
      expect(container.querySelector('form, input')).toBeNull()
    })

    it('never loads an image: it is a link with its alt text', () => {
      const { container } = render(<Markdown>{'![a chart](https://example.com/c.png)'}</Markdown>)
      expect(container.querySelector('img')).toBeNull()
      const a = screen.getByRole('link', { name: 'Image: a chart' }) as HTMLAnchorElement
      expect(a.href).toBe('https://example.com/c.png')
      expect(a.rel).toBe('noopener noreferrer')
      expect(a.target).toBe('_blank')
    })

    it.each([[''], [undefined]])('shows an image whose source is %o as its alt text, with no link', (src) => {
      const { container } = render(<MarkdownImage src={src} alt="a chart" />)
      expect(container.querySelector('a')).toBeNull()
      expect(container.textContent).toBe('Image: a chart')
    })

    it('shows an image with an unsafe source as its alt text, with no link', () => {
      const { container } = render(<Markdown>{'![x](javascript:alert(1))'}</Markdown>)
      expect(container.querySelector('img')).toBeNull()
      expect(container.querySelector('a')).toBeNull()
      expect(container.textContent).toContain('Image: x')
    })

    // jsdom computes no cascade (the browser checks do), so this reads the
    // rules themselves: Tailwind's preflight sets lists to no markers, and the
    // two places Markdown renders into must put them back (client view §5.3).
    it('has its containers restore list markers over the reset', () => {
      const css = readFileSync(join(process.cwd(), 'src/index.css'), 'utf8').replace(/\s+/g, ' ')
      for (const scope of ['.bubble', '.think-body']) {
        expect(css).toMatch(new RegExp(`${scope.replace('.', '\\.')} ul[^{]*\\{[^}]*list-style:disc`))
        expect(css).toMatch(new RegExp(`${scope.replace('.', '\\.')} ol[^{]*\\{[^}]*list-style:decimal`))
        expect(css).toMatch(new RegExp(`${scope.replace('.', '\\.')} ol, [^{]*\\{[^}]*padding-left:22px`))
      }
    })
  })
  ````

Create `web/src/components/StepList.test.tsx`:

  ```tsx
  import { render, screen, within } from '@testing-library/react'
  import { describe, expect, it } from 'vitest'
  import StepList from './StepList'

  describe('StepList', () => {
    it('renders nothing for no steps', () => {
      const { container } = render(<StepList entries={[]} />)
      expect(container.firstChild).toBeNull()
    })

    it('shows each step and marks the current and the finished ones', () => {
      const { container } = render(
        <StepList
          entries={[
            { content: 'Read the spec', status: 'completed' },
            { content: 'Write the test', status: 'in_progress' },
            { content: 'Ship it', status: 'pending' },
          ]}
        />,
      )
      const list = within(container.querySelector('ol') as HTMLElement)
      expect(list.getByText('Ship it')).toBeTruthy()
      expect(list.getByText('Write the test').closest('li')?.className).toBe('on')
      expect(list.getByText('Read the spec').closest('li')?.className).toBe('done')
      expect(list.getByText('Ship it').closest('li')?.className).toBe('')
    })

    it('counts finished steps over all', () => {
      render(<StepList entries={[{ content: 'a', status: 'completed' }, { content: 'b', status: 'pending' }]} />)
      expect(screen.getByText(/1\s*\/\s*2/)).toBeTruthy()
    })

    it('names the step in progress in the summary', () => {
      const { container } = render(
        <StepList entries={[{ content: 'a', status: 'completed' }, { content: 'building the thing', status: 'in_progress' }]} />,
      )
      expect(within(container.querySelector('summary') as HTMLElement).getByText('building the thing')).toBeTruthy()
    })

    it('still shows a step the agent sent without text', () => {
      const { container } = render(<StepList entries={[{ content: '' }]} />)
      expect(within(container.querySelector('ol') as HTMLElement).getByText('Untitled step')).toBeTruthy()
    })

    it('is open while work is in progress and closed once all is done', () => {
      const { container: running } = render(<StepList entries={[{ content: 'a', status: 'in_progress' }]} />)
      expect(running.querySelector('details')?.hasAttribute('open')).toBe(true)
      const { container: finished } = render(<StepList entries={[{ content: 'a', status: 'completed' }]} />)
      expect(finished.querySelector('details')?.hasAttribute('open')).toBe(false)
    })

    it('stays closed when the caller says so', () => {
      const { container } = render(<StepList entries={[{ content: 'a', status: 'in_progress' }]} open={false} />)
      expect(container.querySelector('details')?.hasAttribute('open')).toBe(false)
    })

    it('says when steps were cut', () => {
      render(<StepList entries={[{ content: 'a' }]} truncated />)
      expect(screen.getByText(/cut or left out/)).toBeTruthy()
    })
  })
  ```

Create `web/src/components/Transcript.rows.test.tsx`:

  ```tsx
  import { render } from '@testing-library/react'
  import { describe, expect, it, vi } from 'vitest'
  import type { Item } from '../generated/view'
  import Transcript from './Transcript'
  import type { ItemEnv } from './items/types'

  // Counts how often each message renders: the Markdown of a held item must
  // not be parsed again when another item changes.
  const renders = vi.hoisted(() => new Map<string, number>())
  vi.mock('./items/AgentMessage', () => ({
    default: ({ item }: { item: { id: string; text: string } }) => {
      renders.set(item.id, (renders.get(item.id) ?? 0) + 1)
      return <p>{item.text}</p>
    },
  }))

  const env: ItemEnv = { sessionId: 's1', agent: 'Codex' }
  const message = (id: string, text: string, version = 1) =>
    ({ id, version, ts: '2026-10-02T10:00:00.000Z', turn_id: 't1', kind: 'message', text }) as Item

  describe('Transcript rows', () => {
    it('renders only the item that changed, or the one that came', () => {
      renders.clear()
      const a = message('a', 'first')
      const b = message('b', 'second')
      const { rerender } = render(<Transcript items={[a, b]} env={env} />)
      rerender(<Transcript items={[a, message('b', 'second, longer', 2)]} env={env} />)
      rerender(<Transcript items={[a, message('b', 'second, longer', 2), message('c', 'third')]} env={env} />)
      expect(renders.get('a')).toBe(1)
      expect(renders.get('c')).toBe(1)
      expect(renders.get('b')).toBeGreaterThanOrEqual(2)
    })

    it('renders every item again when the session’s environment changes', () => {
      renders.clear()
      const a = message('a', 'first')
      const { rerender } = render(<Transcript items={[a]} env={env} />)
      rerender(<Transcript items={[a]} env={{ ...env, agent: 'Claude' }} />)
      expect(renders.get('a')).toBe(2)
    })
  })
  ```

Create `web/src/components/Transcript.test.tsx`:

  ```tsx
  import { fireEvent, render, screen } from '@testing-library/react'
  import { readFileSync, readdirSync } from 'node:fs'
  import { join } from 'node:path'
  import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
  import type { Item } from '../generated/view'
  import ItemBoundary from './ItemBoundary'
  import Transcript from './Transcript'
  import type { ItemEnv } from './items/types'

  const env: ItemEnv = { sessionId: 's1', agent: 'Claude' }
  const TS = '2026-10-02T10:00:00.000Z'

  function message(id: string, text: string, version = 1): Item {
    return { id, version, ts: TS, turn_id: 't1', kind: 'message', text } as Item
  }

  // A plan whose entries are not a list: the store's check (id, version, ts,
  // kind) lets it through, and the step list throws on it.
  function brokenPlan(id: string, version: number): Item {
    return { id, version, ts: TS, turn_id: 't1', kind: 'plan', entries: null } as unknown as Item
  }

  let quiet: ReturnType<typeof vi.spyOn>
  beforeEach(() => {
    // React reports a caught render error on the console.
    quiet = vi.spyOn(console, 'error').mockImplementation(() => {})
  })
  afterEach(() => quiet.mockRestore())

  describe('Transcript', () => {
    it('says when there is nothing yet', () => {
      render(<Transcript items={[]} env={env} />)
      expect(screen.getByText('Nothing in this session yet.')).toBeInTheDocument()
    })

    it('renders the items in the order given', () => {
      const { container } = render(<Transcript items={[message('a', 'first'), message('b', 'second')]} env={env} />)
      expect(Array.from(container.querySelectorAll('.bubble')).map((b) => b.textContent)).toEqual(['first', 'second'])
    })

    it('keeps an item that throws to itself: the rest still renders', () => {
      render(<Transcript items={[message('a', 'before'), brokenPlan('p', 2), message('b', 'after')]} env={env} />)
      expect(screen.getByText('Could not render this item')).toBeInTheDocument()
      expect(screen.getByText('before')).toBeInTheDocument()
      expect(screen.getByText('after')).toBeInTheDocument()
    })

    it('keeps a possible fabrication’s banner when its tool call fails to render', () => {
      // A title that is not text: the tool call's renderer throws on it.
      const tool = { id: 'tc', version: 1, ts: TS, turn_id: 't1', kind: 'tool_call', tool_call_id: 'x', title: { not: 'text' }, fabricated: '<b>made</b> up' } as unknown as Item
      const { container } = render(<Transcript items={[tool]} env={env} />)
      expect(screen.getByText('Could not render this item')).toBeInTheDocument()
      const alert = screen.getByRole('alert')
      expect(alert).toHaveTextContent('Possible fabricated tool call')
      expect(alert).toHaveTextContent('<b>made</b> up')
      expect(container.querySelector('.fab-warn b')).toBeNull()
    })

    it('keeps the banner whatever throws inside the boundary', () => {
      const Throws = () => {
        throw new Error('broken')
      }
      render(
        <ItemBoundary version={1} fabricated="">
          <Throws />
        </ItemBoundary>,
      )
      expect(screen.getByText('Could not render this item')).toBeInTheDocument()
      expect(screen.getByRole('alert')).toHaveTextContent('Possible fabricated tool call')
    })

    it('shows no banner for a failed item that is not a tool call, whatever it carries', () => {
      const plan = { ...brokenPlan('p', 2), fabricated: 'not a tool call' } as unknown as Item
      render(<Transcript items={[brokenPlan('q', 2), plan]} env={env} />)
      expect(screen.getAllByText('Could not render this item')).toHaveLength(2)
      expect(screen.queryByRole('alert')).toBeNull()
    })

    it('renders the item again when a new version of it comes', () => {
      const { rerender } = render(<Transcript items={[brokenPlan('p', 2)]} env={env} />)
      expect(screen.getByText('Could not render this item')).toBeInTheDocument()
      const fixed = { id: 'p', version: 3, ts: TS, turn_id: 't1', kind: 'plan', entries: [{ content: 'step one' }] } as Item
      rerender(<Transcript items={[fixed]} env={env} />)
      expect(screen.queryByText('Could not render this item')).toBeNull()
      expect(screen.getAllByText('step one').length).toBeGreaterThan(0)
    })

    it('keeps a failed item failed while its version stays', () => {
      const { rerender } = render(<Transcript items={[brokenPlan('p', 2)]} env={env} />)
      rerender(<Transcript items={[brokenPlan('p', 2), message('b', 'later')]} env={env} />)
      expect(screen.getByText('Could not render this item')).toBeInTheDocument()
    })

    it('keeps an item’s state (an open tool call) across a new version of it', () => {
      const tool = (version: number, status: string) =>
        ({ id: 'tc', version, ts: TS, turn_id: 't1', kind: 'tool_call', tool_call_id: 'x', title: 'Run', status, output: 'out' }) as Item
      const { container, rerender } = render(<Transcript items={[tool(1, 'in_progress')]} env={env} />)
      fireEvent.click(screen.getByRole('button'))
      rerender(<Transcript items={[tool(2, 'completed')]} env={env} />)
      expect(screen.getByText('Done')).toBeInTheDocument()
      expect(container.querySelector('.tool-output')).not.toBeNull()
    })

    it('keeps an item’s state when items come in before it', () => {
      const tool = { id: 'tc', version: 1, ts: TS, turn_id: 't2', kind: 'tool_call', tool_call_id: 'x', title: 'Run', output: 'out' } as Item
      const { container, rerender } = render(<Transcript items={[tool]} env={env} />)
      fireEvent.click(screen.getByRole('button'))
      rerender(<Transcript items={[message('a', 'earlier'), tool]} env={env} />)
      expect(container.querySelector('.tool-output')).not.toBeNull()
    })

    // The golden fixtures: what the collector's fold makes of two recorded
    // sessions, one per pinned adapter.
    const FIXTURES = join(process.cwd(), '../crates/hennery-view/tests/fixtures')
    const files = readdirSync(FIXTURES).filter((f) => f.endsWith('.items.json'))

    it('has golden fixtures to render', () => {
      expect(files.length).toBeGreaterThanOrEqual(2)
    })

    it.each(files)('renders every item of %s', (file) => {
      const items = JSON.parse(readFileSync(join(FIXTURES, file), 'utf8')) as Item[]
      const { container } = render(<Transcript items={items} env={env} />)
      expect(screen.queryByText('Could not render this item')).toBeNull()
      expect(container.querySelector('.unrec')).toBeNull()
      expect(quiet).not.toHaveBeenCalled()
    })
  })
  ```

Create `web/src/components/items/items.test.tsx`:

  ```tsx
  import { fireEvent, render, screen, within } from '@testing-library/react'
  import { describe, expect, it, vi } from 'vitest'
  import type { Item, MarkerKind, ToolContent } from '../../generated/view'
  import { ItemView } from '../Transcript'
  import { MARKER_LABEL } from './Marker'
  import type { ItemEnv, ItemOf } from './types'

  const TS = '2026-10-02T10:00:00.000Z'
  const env: ItemEnv = { sessionId: 's/1', agent: 'Codex' }

  function item<K extends Item['kind']>(kind: K, body: Omit<ItemOf<K>, 'id' | 'version' | 'ts' | 'kind'>): ItemOf<K> {
    return { id: `i-${kind}`, version: 1, ts: TS, turn_id: 't1', kind, ...body } as unknown as ItemOf<K>
  }

  function show(value: Item, e: ItemEnv = env) {
    return render(<ItemView item={value} env={e} />)
  }

  describe('user_turn', () => {
    it('shows the text exactly as sent: no Markdown, line breaks kept', () => {
      const text = '  indented\n# not a heading\n**not bold** <b>x</b>\nsecond line\n'
      const { container } = show(item('user_turn', { content: [{ type: 'text', text }] }))
      const p = container.querySelector('.user-text') as HTMLElement
      expect(p.textContent).toBe(text)
      expect(container.querySelector('h1, strong, b')).toBeNull()
      expect(screen.getByText('You')).toBeInTheDocument()
    })

    const HASH = '57cda64cead0869cd5f90dfebb024f4bd9a922aaea91d513de6fa3e949d88921'

    it('shows an image of an allowed type from the attachment store, by its hash', () => {
      const { container } = show(item('user_turn', { content: [{ type: 'image', mimeType: 'image/png', sha256: HASH, size: 3 }] }))
      expect(container.querySelector('img')?.getAttribute('src')).toBe(`/api/attachments/${HASH}`)
    })

    it.each([
      ['a path', 'ab/c?d'],
      ['a traversal', `../${HASH.slice(3)}`],
      ['upper case', HASH.toUpperCase()],
      ['too short', HASH.slice(1)],
      ['too long', `${HASH}0`],
      ['a trailing newline', `${HASH}\n`],
      ['empty', ''],
      ['not text', 7],
    ])('loads nothing for a hash that is %s, and says so', (_name, sha256) => {
      const { container } = show(
        item('user_turn', { content: [{ type: 'image', mimeType: 'image/png', sha256: sha256 as string, size: 3 }] }),
      )
      expect(container.querySelector('img')).toBeNull()
      expect(container.querySelector('.item-note')?.textContent).toContain('not shown')
    })

    it.each(['image/jpeg', 'image/gif', 'image/webp'])('shows %s', (mimeType) => {
      const { container } = show(item('user_turn', { content: [{ type: 'image', mimeType, sha256: HASH, size: 1 }] }))
      expect(container.querySelector('img')).not.toBeNull()
    })

    it('names an image of another type instead of loading it', () => {
      const { container } = show(
        item('user_turn', { content: [{ type: 'image', mimeType: 'image/svg+xml', sha256: HASH, size: 1 }] }),
      )
      expect(container.querySelector('img')).toBeNull()
      expect(container.textContent).toContain('image/svg+xml')
    })

    it('says when the prompt was cut, linking the raw events', () => {
      show(item('user_turn', { content: [{ type: 'text', text: 'x' }], truncated: true }))
      expect(screen.getByText(/This prompt was cut/)).toBeInTheDocument()
      expect(screen.getByRole('link', { name: 'Raw events' })).toHaveAttribute('href', '/api/sessions/s%2F1/events')
    })
  })

  describe('message', () => {
    it('renders Markdown under the agent’s label', () => {
      const { container } = show(item('message', { text: 'some **bold** text' }))
      expect(container.querySelector('.bubble strong')?.textContent).toBe('bold')
      expect(screen.getByText('Codex')).toBeInTheDocument()
    })

    it('says when it was cut', () => {
      show(item('message', { text: 'x', truncated: true }))
      expect(screen.getByText(/This message was cut/)).toBeInTheDocument()
    })
  })

  describe('thinking', () => {
    it('is collapsed until opened, then Markdown', () => {
      const { container } = show(item('thinking', { text: 'first *thought*' }))
      expect(screen.getByText('Thinking')).toBeInTheDocument()
      expect(container.querySelector('.think-body')).toBeNull()
      fireEvent.click(screen.getByRole('button', { expanded: false }))
      expect(container.querySelector('.think-body em')?.textContent).toBe('thought')
    })

    it('says when it was cut', () => {
      show(item('thinking', { text: 'x', truncated: true }))
      expect(screen.getByText(/This reasoning was cut/)).toBeInTheDocument()
    })
  })

  describe('tool_call', () => {
    const base = { tool_call_id: 'tc1', title: 'Run a command', tool_kind: 'execute', status: 'completed' }

    it('is one line: name, the command, the status in words; the rest hidden', () => {
      const { container } = show(item('tool_call', { ...base, input: { command: 'echo hi' }, output: 'hi\n' }))
      expect(screen.getByText('Run a command')).toBeInTheDocument()
      expect(screen.getByText('echo hi')).toBeInTheDocument()
      expect(screen.getByText('Done')).toBeInTheDocument()
      expect(container.querySelectorAll('.tool-io')).toHaveLength(0)
    })

    it('falls back to the tool kind for a name', () => {
      show(item('tool_call', { tool_call_id: 'x', tool_kind: 'read' }))
      expect(screen.getByText('read')).toBeInTheDocument()
    })

    it('expands to the input as indented JSON and the output as text', () => {
      const { container } = show(item('tool_call', { ...base, input: { command: 'echo hi' }, output: 'hi <b>x</b>' }))
      fireEvent.click(screen.getByRole('button'))
      const input = container.querySelector('.tool-input') as HTMLElement
      expect(input.textContent).toContain('"command": "echo hi"')
      expect(input.querySelector('.tk-key')?.textContent).toBe('"command"')
      expect(container.querySelector('.tool-output')?.textContent).toBe('outputhi <b>x</b>')
      expect(container.querySelector('.tool-output b')).toBeNull()
    })

    it('never marks the tool’s own input as a redaction', () => {
      const { container } = show(item('tool_call', { ...base, input: { note: 'this was redacted' } }))
      fireEvent.click(screen.getByRole('button'))
      expect(container.querySelector('.tk-redact')).toBeNull()
      expect(container.querySelector('.tool-input .tk-str')?.textContent).toBe('"this was redacted"')
    })

    it('shows a cut input (a string) as it is', () => {
      const { container } = show(item('tool_call', { ...base, input: '{"command": "ech', truncated: true }))
      fireEvent.click(screen.getByRole('button'))
      expect(container.querySelector('.tool-input')?.textContent).toBe('input{"command": "ech')
      expect(screen.getByText(/Part of this tool call was cut/)).toBeInTheDocument()
    })

    it('is not expandable with nothing to show', () => {
      const { container } = show(item('tool_call', { tool_call_id: 'x', status: 'pending' }))
      fireEvent.click(screen.getByRole('button'))
      expect(container.querySelector('.tool-detail')).toBeNull()
      expect(screen.getByText('Pending')).toBeInTheDocument()
    })

    it('shows a status it does not know as sent', () => {
      show(item('tool_call', { tool_call_id: 'x', status: 'paused' }))
      expect(screen.getByText('paused')).toBeInTheDocument()
    })

    function expanded(content: ToolContent[]) {
      const r = show(item('tool_call', { ...base, content }))
      fireEvent.click(screen.getByRole('button'))
      return r.container
    }

    it('shows a text block as text', () => {
      const c = expanded([{ type: 'text', text: 'plain <i>x</i>' }])
      expect(c.textContent).toContain('plain <i>x</i>')
      expect(c.querySelector('i')).toBeNull()
    })

    it('shows a diff: its path, before and after', () => {
      const c = expanded([{ type: 'diff', path: '/srv/work/a.txt', old_text: 'old', new_text: 'new' }])
      expect(c.textContent).toContain('/srv/work/a.txt')
      expect(c.querySelector('.diff-old')?.textContent).toBe('old')
      expect(c.querySelector('.diff-new')?.textContent).toBe('new')
    })

    it('shows a new file’s diff without a before', () => {
      const c = expanded([{ type: 'diff', path: '/srv/work/b.txt', new_text: 'yes' }])
      expect(c.querySelector('.diff-old')).toBeNull()
      expect(c.textContent).toContain('new file')
    })

    it('shows an image of an allowed type as a data URL', () => {
      const c = expanded([{ type: 'image', mime_type: 'image/png', data: 'iVBORw0KGgo=' }])
      expect(c.querySelector('img')?.getAttribute('src')).toBe('data:image/png;base64,iVBORw0KGgo=')
    })

    it('names an image of a refused type instead of showing it', () => {
      const c = expanded([{ type: 'image', mime_type: 'image/svg+xml', data: 'PHN2Zz4=' }])
      expect(c.querySelector('img')).toBeNull()
      expect(c.textContent).toContain('image/svg+xml')
    })

    it('says an image was too large when it has no data', () => {
      const c = expanded([{ type: 'image', mime_type: 'image/png' }])
      expect(c.querySelector('img')).toBeNull()
      expect(c.textContent).toContain('too large')
    })

    it('shows a terminal by its id', () => {
      const c = expanded([{ type: 'terminal', terminal_id: 'term-7' }])
      expect(c.textContent).toContain('term-7')
    })

    it('shows an other block’s raw JSON as text', () => {
      const c = expanded([{ type: 'other', raw: '{"type":"audio"}' }])
      expect(c.textContent).toContain('{"type":"audio"}')
    })

    it('shows a block of an unknown type as its JSON', () => {
      const c = expanded([{ type: 'hologram', x: 1 } as unknown as ToolContent])
      expect(c.textContent).toContain('"hologram"')
    })

    it('lists the locations', () => {
      const c = expanded([])
      expect(c.querySelector('.tool-locations')).toBeNull()
      const { container } = show(item('tool_call', { ...base, locations: [{ path: '/srv/work/x.rs', line: 4 }, { path: '/srv/work/y.rs' }] }))
      fireEvent.click(within(container).getByRole('button'))
      expect(container.querySelector('.tool-locations')?.textContent).toBe('/srv/work/x.rs:4/srv/work/y.rs')
    })

    it('warns loudly of a possible fabrication, quoting it as text, without expanding', () => {
      const { container } = show(item('tool_call', { ...base, output: 'x', fabricated: '<tool_use><b>x</b></tool_use>' }))
      const alert = screen.getByRole('alert')
      expect(alert).toHaveTextContent('Possible fabricated tool call')
      expect(alert).toHaveTextContent('<tool_use><b>x</b></tool_use>')
      expect(container.querySelector('.fab-warn b')).toBeNull()
    })

    it.each(['__proto__', 'constructor', 'toString'])('shows a status named %s as sent, the fabrication banner too', (status) => {
      const { container } = show(item('tool_call', { ...base, status, fabricated: 'made up' }))
      expect(screen.getByRole('alert')).toHaveTextContent('Possible fabricated tool call')
      expect(container.querySelector('.tool-status')?.textContent).toBe(status)
      expect((container.querySelector('.tool-status') as HTMLElement).style.color).toBe('var(--fg-quiet)')
    })

    it('has no warning when nothing was fabricated', () => {
      show(item('tool_call', { ...base }))
      expect(screen.queryByRole('alert')).toBeNull()
    })
  })

  describe('plan', () => {
    it('shows its step list', () => {
      const { container } = show(item('plan', { entries: [{ content: 'step one', status: 'in_progress' }] }))
      expect(within(container.querySelector('ol') as HTMLElement).getByText('step one')).toBeInTheDocument()
    })
  })

  describe('question', () => {
    type Q = Omit<ItemOf<'question'>, 'id' | 'version' | 'ts' | 'kind'>
    const permission: Q = {
      pending_id: 'p1',
      question_kind: 'permission',
      request: {
        type: 'permission',
        title: 'Run rm -rf build?',
        options: [
          { option_id: 'a', name: 'Yes, always', option_kind: 'allow_always' },
          { option_id: 'b', name: 'Allow', option_kind: 'allow_once' },
          { option_id: 'c', name: 'Allow (really reject)', option_kind: 'reject_once' },
          { option_id: 'd', name: 'Never', option_kind: 'reject_always' },
        ],
      },
      answerable: true,
      state: 'open',
      answered: false,
    }

    it('shows the request, its options in order, styled by kind and never by name, and no buttons', () => {
      const { container } = show(item('question', permission))
      expect(screen.getByText('Run rm -rf build?')).toBeInTheDocument()
      expect(screen.getByText('Codex asks for permission')).toBeInTheDocument()
      const options = Array.from(container.querySelectorAll('.q-opt'))
      expect(options.map((o) => o.textContent)).toEqual(['Yes, always', 'Allow', 'Allow (really reject)', 'Never'])
      expect(options.map((o) => o.className)).toEqual([
        'q-opt q-opt-always',
        'q-opt q-opt-allow',
        'q-opt q-opt-reject',
        'q-opt q-opt-reject',
      ])
      expect(screen.queryByRole('button')).toBeNull()
      expect(screen.getByText('Needs your answer')).toBeInTheDocument()
    })

    it('renders the actions it is given (the seam for answering)', () => {
      show(item('question', permission), { ...env, questionActions: (q) => <button type="button">answer {q.pending_id}</button> })
      expect(screen.getByRole('button', { name: 'answer p1' })).toBeInTheDocument()
    })

    it('says a permission with no options cannot be answered here', () => {
      show(item('question', { ...permission, request: { type: 'permission', options: [] } }))
      expect(screen.getByText(/cannot be answered here: stop, park or close the session/)).toBeInTheDocument()
    })

    it.each([
      [{ answered: true, answerable: false }, 'Sent'],
      [{ answered: true, delivered: true, answerable: false }, 'Answered'],
      [{ answered: true, delivered: false, answerable: false }, 'Sent, but the agent was no longer waiting'],
      [{ state: 'delivered' as const, answerable: false }, 'Answered'],
      [{ state: 'cancelled' as const, reason: 'agent_withdrew' as const, answerable: false }, 'The agent stopped waiting (the agent withdrew the question)'],
      [{ state: 'cancelled' as const, reason: 'host_revoked' as const, answerable: false }, 'The agent stopped waiting (the host was revoked)'],
      [{ state: 'cancelled' as const, answered: true, delivered: true, answerable: false }, 'Answered'],
      [{ answerable: false }, 'Open'],
    ])('%o reads “%s”', (patch, text) => {
      show(item('question', { ...permission, ...patch }))
      expect(screen.getByText(text)).toBeInTheDocument()
    })

    it('shows a reason named like a prototype key as sent', () => {
      show(item('question', { ...permission, state: 'cancelled', reason: 'constructor' as never, answerable: false }))
      expect(screen.getByText('The agent stopped waiting (constructor)')).toBeInTheDocument()
    })

    it('shows an elicitation’s message, fields, hints and options', () => {
      show(
        item('question', {
          pending_id: 'p2',
          question_kind: 'elicitation',
          request: {
            type: 'elicitation',
            message: 'Tabs or spaces?',
            form_supported: false,
            fields: [
              { key: 'indent', label: 'Indent', hint: 'Pick one', field_kind: 'single', options: [{ value: 'tabs', label: 'Tabs', description: 'one char' }, { value: 'spaces' }] },
              { key: 'other', label: 'Other', field_kind: 'unsupported' },
            ],
          },
          answerable: false,
          state: 'open',
          answered: false,
        }),
      )
      expect(screen.getByText('Tabs or spaces?')).toBeInTheDocument()
      expect(screen.getByText('Codex asks')).toBeInTheDocument()
      expect(screen.getByText('Pick one')).toBeInTheDocument()
      expect(screen.getByText('Tabs')).toBeInTheDocument()
      expect(screen.getByText('one char')).toBeInTheDocument()
      expect(screen.getByText('spaces')).toBeInTheDocument()
      expect(screen.getByText(/This field cannot be filled in here/)).toBeInTheDocument()
      expect(screen.getByText(/can only be declined or cancelled/)).toBeInTheDocument()
    })
  })

  describe('marker', () => {
    // Written out, not read from the table under test: a changed label fails.
    const WORDS: [MarkerKind, string][] = [
      ['parked', 'Parked'],
      ['resumed', 'Resumed'],
      ['closed', 'Closed'],
      ['host_restarted', 'The host restarted'],
      ['host_offline', 'Host offline'],
      ['host_back', 'Host back'],
      ['turn_interrupted', 'The turn was interrupted'],
      ['turn_failed', 'The turn failed'],
      ['turn_cancelled', 'The turn was stopped'],
      ['turn_not_delivered', 'The turn was not delivered'],
      ['start_not_delivered', 'The start was not delivered'],
      ['start_failed', 'The session failed to start'],
      ['adapter_exited', 'The agent’s process exited'],
      ['transcript_gap', 'Part of the transcript is missing'],
      ['host_note', 'A note from the host'],
      ['conflict', 'The host sent a conflicting update'],
      ['hat_reassigned', 'Moved to another hat'],
      ['elided', 'This turn holds more than is shown here'],
    ]

    it('covers all 18 kinds, each once', () => {
      expect(WORDS).toHaveLength(18)
      expect(new Set(WORDS.map(([k]) => k))).toEqual(new Set(Object.keys(MARKER_LABEL)))
    })

    it.each(WORDS)('%s reads “%s”', (kind, words) => {
      const { container } = show(item('marker', { marker: kind }))
      const label = (container.querySelector('.divider > span')?.firstChild as Text | null)?.textContent
      expect(label).toBe(words)
      expect(container.textContent).not.toContain(`(${kind})`)
    })

    it.each([
      ['parked', 'idle', 'it was idle'],
      ['parked', 'operator', 'on request'],
      ['host_offline', 'host_revoked', 'the host was revoked'],
      ['start_failed', 'agent_has_no_record', 'the agent has no record of this session'],
      ['parked', 'adapter_exited', 'the agent’s process exited'],
      ['host_offline', 'host_offline', 'the host went offline'],
      ['start_failed', 'agent_not_logged_in', 'the agent is not logged in on the host'],
      ['start_failed', 'load_unsupported', 'the agent cannot open an earlier session'],
      ['start_failed', 'start_failed', 'the agent could not start'],
      ['host_note', 'config_failed', 'a setting did not take'],
      ['host_note', 'reapply_failed', 'a setting could not be applied again'],
      ['host_note', 'cancel_unanswered', 'a question was left unanswered when the turn stopped'],
      ['host_note', 'replay_unknown_dropped', 'history of an unknown kind was left out'],
      ['host_note', 'agent_home_moved', 'the agent’s data directory moved'],
      ['elided', 'items', 'too many items'],
      ['elided', 'questions', 'too many questions'],
      ['host_note', 'some_note', 'some_note'],
      ['adapter_exited', 'code 1', 'code 1'],
      ['transcript_gap', '4..9', 'host events 4..9'],
      ['conflict', '12', 'at host event 12'],
      ['elided', 'bytes', 'too much text'],
      ['start_failed', 'some_new_code', 'some_new_code'],
    ] as [MarkerKind, string, string][])('%s with reason %s reads “%s”', (kind, reason, words) => {
      const { container } = show(item('marker', { marker: kind, reason }))
      expect(container.querySelector('.divider')?.textContent).toContain(`(${words})`)
    })

    it('keeps its text behind a disclosure, as text', () => {
      const { container } = show(item('marker', { marker: 'adapter_exited', text: 'panic: <script>x</script>' }))
      const details = container.querySelector('details.marker-text') as HTMLDetailsElement
      expect(details.open).toBe(false)
      expect(details.querySelector('pre')?.textContent).toBe('panic: <script>x</script>')
      expect(container.querySelector('script')).toBeNull()
    })

    it('names the hats of a reassignment when they are known', () => {
      const names: Record<string, string> = { h1: 'Work', h2: 'Home' }
      const { container } = show(item('marker', { marker: 'hat_reassigned', from: 'h1', to: 'h2' }), { ...env, hatName: (id) => names[id] })
      expect(container.querySelector('.divider')?.textContent).toContain('from Work to Home')
    })

    it('shows the hats’ ids when their names are not known', () => {
      const { container } = show(item('marker', { marker: 'hat_reassigned', from: 'h1', to: 'h2' }))
      expect(container.querySelector('.divider')?.textContent).toContain('from h1 to h2')
    })

    it('offers “Send again” for an undelivered turn only through its seam', () => {
      const m = item('marker', { marker: 'turn_not_delivered', about_turn: 't9' })
      const { unmount } = show(m)
      expect(screen.getByText('The turn was not delivered')).toBeInTheDocument()
      expect(screen.queryByRole('button', { name: 'Send again' })).toBeNull()
      unmount()
      const onSendAgain = vi.fn()
      show(m, { ...env, onSendAgain })
      fireEvent.click(screen.getByRole('button', { name: 'Send again' }))
      expect(onSendAgain).toHaveBeenCalledWith(m)
    })

    it('points an elided turn to the raw events', () => {
      show(item('marker', { marker: 'elided', reason: 'items' }))
      expect(screen.getByText('This turn holds more than is shown here', { exact: false })).toBeInTheDocument()
      expect(screen.getByRole('link', { name: 'Raw events' })).toBeInTheDocument()
    })

    it('shows a reason as sent for a kind named like a prototype key', () => {
      const { container } = show(item('marker', { marker: '__proto__' as MarkerKind, reason: 'toString' }))
      expect(container.querySelector('.divider')?.textContent).toContain('(toString)')
    })

    it('names a kind it does not know rather than failing', () => {
      const { container } = show(item('marker', { marker: 'from_the_future' as MarkerKind }))
      expect(container.textContent).toContain('from_the_future')
    })
  })

  describe('unrecognised', () => {
    it('is collapsed, naming its kind, with its raw JSON as text', () => {
      const { container } = show(item('unrecognised', { update_kind: 'acp_update/new_thing', raw: '{"a":"<b>x</b>"}', truncated: true }))
      const details = container.querySelector('details') as HTMLDetailsElement
      expect(details.open).toBe(false)
      expect(details.querySelector('summary')?.textContent).toBe('Unsupported update (acp_update/new_thing)')
      expect(details.querySelector('pre')?.textContent).toBe('{"a":"<b>x</b>"}')
      expect(container.querySelector('pre b')).toBeNull()
      expect(screen.getByText(/This update was cut/)).toBeInTheDocument()
    })
  })

  describe('an item of a kind this client does not know', () => {
    it('is named, not dropped', () => {
      const { container } = show({ id: 'x', version: 1, ts: TS, kind: 'hologram' } as unknown as Item)
      expect(container.textContent).toContain('Unsupported item (hologram)')
    })
  })
  ```

Create `web/src/deps.test.ts`:

  ```ts
  // Raw HTML in an agent's Markdown stays text because nothing parses it
  // (Markdown.tsx). A raw-HTML parser in the dependencies would be one plugin
  // away from undoing that, and with it the reason ids need no prefix there.
  import { readFileSync } from 'node:fs'
  import { join } from 'node:path'
  import { describe, expect, it } from 'vitest'

  // Vitest runs from `web/`.
  const FILES = ['package.json', 'pnpm-lock.yaml']
  const PARSERS = ['rehype-raw', 'hast-util-raw']

  describe('the dependencies', () => {
    it.each(FILES)('%s names no raw-HTML parser', (file) => {
      const text = readFileSync(join(process.cwd(), file), 'utf8')
      expect(text.length).toBeGreaterThan(100)
      expect(PARSERS.filter((name) => text.includes(name))).toEqual([])
    })
  })
  ```

In `web/src/lib/time.test.ts`, replace:

  ```ts
    it('empty -> empty', () => expect(clockTime('')).toBe(''))
  })
  ```

with:

  ```ts
    it('empty -> empty', () => expect(clockTime('')).toBe(''))
    it('unparseable -> empty', () => expect(clockTime('not a time')).toBe(''))
    it('hours and minutes in the browser’s own form, the same on every call', () => {
      const iso = '2026-06-09T11:45:00Z'
      const expected = new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
      expect(clockTime(iso)).toBe(expected)
      expect(clockTime('2026-06-09T13:05:00Z')).toBe(new Date('2026-06-09T13:05:00Z').toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }))
      expect(clockTime(iso)).toBe(expected)
    })
  })
  ```

Create `web/src/screens/Session.test.tsx`:

  ```tsx
  import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
  import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from 'vitest'
  import type { SessionDetail } from '../generated/protocol'
  import type { Item, ItemPage, SessionSummary } from '../generated/view'
  import { json, liveStream, routed, type Call, type LiveStream } from '../test-stream'
  import SessionView from './Session'

  const ID = 's1'
  const PAGE = '/api/view/sessions/s1'
  const STREAM = '/api/stream/view/sessions/s1'
  const DETAIL = '/api/sessions/s1'
  const FAST = { retryMs: () => 5, resyncedMs: 300 }

  function message(id: string, turn: string, text = id): Item {
    return { id, version: 1, ts: '2026-10-02T10:00:00.000Z', turn_id: turn, kind: 'message', text } as Item
  }

  function page(items: Item[], older = false, revision = 10): ItemPage {
    return { items, older, epoch: 'e1', revision }
  }

  function detail(agent: string, patch: Partial<SessionDetail> = {}): SessionDetail {
    return {
      session_id: ID,
      host_id: 'h1',
      agent,
      cwd: '/srv/work/project',
      hat_id: 'hat1',
      lifecycle: 'active',
      activity: 'idle',
      presumed_parked: false,
      created_at: '2026-10-02T09:00:00.000Z',
      last_event_at: '2026-10-02T10:00:00.000Z',
      pending: [],
      ...patch,
    } as SessionDetail
  }

  type Answer = Response | Promise<Response>

  /** A server for one session. `pages` are served in turn (the last repeats);
   *  `older` by `before_turn`. */
  function server(opts: {
    pages: (() => Answer)[]
    older?: Record<string, () => Answer>
    detail?: () => Answer
    hats?: () => Answer
  }) {
    const streams: LiveStream[] = []
    const pages = [...opts.pages]
    const t = routed((call: Call) => {
      const url = new URL(call.path, 'http://h')
      if (url.pathname === PAGE) {
        const before = url.searchParams.get('before_turn')
        if (before !== null) return opts.older?.[before]?.() ?? json({ code: 'x', message: 'x' }, 500)
        return (pages.length > 1 ? pages.shift()! : pages[0])()
      }
      if (url.pathname === STREAM) {
        const live = liveStream()
        streams.push(live)
        return live.response
      }
      if (url.pathname === DETAIL) return opts.detail?.() ?? json(detail('claude'))
      if (url.pathname === `${DETAIL}/catalog`) return json({ session_id: ID, config_options: [], commands: [] })
      if (url.pathname === '/api/hosts') return json([{ host_id: 'h1', name: 'build-box' }])
      if (url.pathname === '/api/hats') return opts.hats?.() ?? json({ code: 'not_found', message: 'no' }, 404)
      return json({ code: 'not_found', message: 'no' }, 404)
    })
    const of = (path: string) => t.calls.filter((c) => new URL(c.path, 'http://h').pathname === path)
    return { ...t, streams, of }
  }

  // jsdom lays nothing out: the transcript's height is 100 px per row, its
  // window 300 px, and its scroll position is held and clamped as a browser
  // would.
  const ROW = 100
  const VIEW = 300
  const tops = new WeakMap<Element, number>()
  function isScroller(el: Element) {
    return el.classList.contains('transcript')
  }
  function heightOf(el: Element) {
    return el.querySelectorAll('.msg, .marker, .ask').length * ROW
  }
  const saved: Record<string, PropertyDescriptor | undefined> = {}
  beforeAll(() => {
    for (const key of ['scrollHeight', 'clientHeight', 'scrollTop']) {
      saved[key] = Object.getOwnPropertyDescriptor(Element.prototype, key)
    }
    Object.defineProperty(Element.prototype, 'scrollHeight', {
      configurable: true,
      get(this: Element) {
        return isScroller(this) ? heightOf(this) : 0
      },
    })
    Object.defineProperty(Element.prototype, 'clientHeight', {
      configurable: true,
      get(this: Element) {
        return isScroller(this) ? VIEW : 0
      },
    })
    Object.defineProperty(Element.prototype, 'scrollTop', {
      configurable: true,
      get(this: Element) {
        return tops.get(this) ?? 0
      },
      set(this: Element, value: number) {
        tops.set(this, Math.max(0, Math.min(value, heightOf(this) - VIEW)))
      },
    })
  })
  afterAll(() => {
    for (const [key, d] of Object.entries(saved)) if (d) Object.defineProperty(Element.prototype, key, d)
  })

  afterEach(() => {
    // @ts-expect-error clear a stub between tests
    delete window.matchMedia
  })

  const scroller = () => document.querySelector('.transcript') as HTMLElement

  function scrollTo(top: number) {
    const el = scroller()
    el.scrollTop = top
    fireEvent.scroll(el)
  }

  const rows = (n: number, turn: string, prefix = turn) => Array.from({ length: n }, (_, i) => message(`${prefix}-${i}`, turn))

  describe('SessionView', () => {
    it('opens at the end of the transcript', async () => {
      const s = server({ pages: [() => json(page(rows(10, 't5')))] })
      render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findByText('t5-9')
      expect(scroller().scrollTop).toBe(10 * ROW - VIEW)
    })

    it('loads earlier turns from a button, keeping the reader’s place', async () => {
      const s = server({
        pages: [() => json(page(rows(10, 't5'), true))],
        older: { t5: () => json(page(rows(4, 't4'), false)) },
      })
      render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findByText('t5-9')
      scrollTo(300)
      fireEvent.click(screen.getByRole('button', { name: /Load earlier/ }))
      await screen.findByText('t4-0')
      expect(s.of(PAGE).map((c) => new URL(c.path, 'http://h').searchParams.get('before_turn'))).toEqual([null, 't5'])
      // The rows on screen stay where they were: four rows went in above them.
      expect(scroller().scrollTop).toBe(300 + 4 * ROW)
      // Nothing older is left: no button.
      expect(screen.queryByRole('button', { name: /Load earlier/ })).toBeNull()
    })

    it('loads earlier turns on scrolling to the top', async () => {
      const s = server({
        pages: [() => json(page(rows(10, 't5'), true))],
        older: { t5: () => json(page(rows(4, 't4'), false)) },
      })
      render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findByText('t5-9')
      scrollTo(0)
      await screen.findByText('t4-0')
      expect(scroller().scrollTop).toBe(4 * ROW)
    })

    it('does not load earlier turns while scrolling below the top', async () => {
      const s = server({ pages: [() => json(page(rows(10, 't5'), true))] })
      render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findByText('t5-9')
      scrollTo(400)
      expect(s.of(PAGE)).toHaveLength(1)
    })

    it('follows new items while the reader is at the end', async () => {
      const s = server({ pages: [() => json(page(rows(5, 't5')))] })
      render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findByText('t5-4')
      await waitFor(() => expect(s.streams).toHaveLength(1))
      act(() => s.streams[0].event('item', message('t5-new', 't5')))
      await screen.findByText('t5-new')
      expect(scroller().scrollTop).toBe(6 * ROW - VIEW)
    })

    it('leaves the reader where they are when scrolled up', async () => {
      const s = server({ pages: [() => json(page(rows(8, 't5')))] })
      render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findByText('t5-7')
      await waitFor(() => expect(s.streams).toHaveLength(1))
      scrollTo(200)
      act(() => s.streams[0].event('item', message('t5-new', 't5')))
      await screen.findByText('t5-new')
      expect(scroller().scrollTop).toBe(200)
    })

    it('says “Reconnecting…” while it resyncs, then “Resynced” for a moment', async () => {
      let release!: (r: Response) => void
      const s = server({
        pages: [() => json(page(rows(2, 't5'))), () => new Promise<Response>((r) => (release = r))],
      })
      // "Resynced" shows long enough to be seen on a loaded machine.
      render(<SessionView id={ID} timing={{ ...FAST, resyncedMs: 1500 }} />, { wrapper: s.wrapper })
      await screen.findByText('t5-1')
      await waitFor(() => expect(s.streams).toHaveLength(1))
      expect(screen.queryByText('Reconnecting…')).toBeNull()
      act(() => s.streams[0].event('resync_required', {}))
      expect(await screen.findByText('Reconnecting…')).toBeInTheDocument()
      await act(async () => release(json(page(rows(3, 't6')))))
      expect(await screen.findByText('Resynced')).toBeInTheDocument()
      expect(screen.queryByText('Reconnecting…')).toBeNull()
      expect(screen.getByText('t6-2')).toBeInTheDocument()
      await waitFor(() => expect(screen.queryByText('Resynced')).toBeNull(), { timeout: 5000 })
    })

    it('says a deleted session was deleted, and opens no stream', async () => {
      const s = server({ pages: [() => json({ code: 'not_found', message: 'gone' }, 404)] })
      render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      expect(await screen.findByRole('heading', { name: 'This session was deleted' })).toBeInTheDocument()
      expect(s.of(STREAM)).toHaveLength(0)
    })

    it('says so when the stream says the session was removed', async () => {
      const s = server({ pages: [() => json(page(rows(2, 't5')))] })
      render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findByText('t5-1')
      await waitFor(() => expect(s.streams).toHaveLength(1))
      act(() => s.streams[0].event('session_removed', { session_id: ID }))
      expect(await screen.findByRole('heading', { name: 'This session was deleted' })).toBeInTheDocument()
      expect(screen.queryByText('t5-1')).toBeNull()
    })

    it.each([
      ['claude', 'Claude'],
      ['codex', 'Codex'],
      ['my-agent', 'my-agent'],
    ])('labels the %s agent “%s”, and the user “You”', async (agent, label) => {
      const user = { id: 'u', version: 1, ts: '2026-10-02T10:00:00.000Z', turn_id: 't5', kind: 'user_turn', content: [{ type: 'text', text: 'hello' }] } as Item
      const s = server({ pages: [() => json(page([user, message('m', 't5')]))], detail: () => json(detail(agent)) })
      const { container } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      await waitFor(() => expect(Array.from(container.querySelectorAll('.msg-who')).map((e) => e.textContent)).toEqual(['You', label]))
      // The avatars carry a letter of the label, never anyone's initials.
      expect(Array.from(container.querySelectorAll('.avatar')).map((e) => e.textContent)).toEqual(['Y', label[0].toUpperCase()])
    })

    it('reads the header from the session’s detail when no summary is given', async () => {
      const s = server({
        pages: [() => json(page(rows(1, 't5')))],
        detail: () => json(detail('codex', { title: 'Fix the build', git_branch: 'main', model: 'gpt-x', mode: 'auto', activity: 'running' })),
      })
      render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      const head = (await screen.findByRole('heading', { name: 'Fix the build' })).closest('header') as HTMLElement
      const h = within(head)
      await waitFor(() => expect(h.getByLabelText('Host')).toHaveTextContent('build-box'))
      expect(h.getByLabelText('Agent')).toHaveTextContent('Codex')
      expect(h.getByLabelText('Branch')).toHaveTextContent('main')
      expect(h.getByLabelText('Model')).toHaveTextContent('gpt-x')
      expect(h.getByLabelText('Mode')).toHaveTextContent('auto')
      expect(h.getByText('Running')).toBeInTheDocument()
      expect(s.of(DETAIL)).toHaveLength(1)
    })

    it('says “Waiting on a question” from the detail for a question outside a turn', async () => {
      // The detail has no `question_waits` (the real type has none), and a
      // question outside a turn leaves `activity` idle: its pending request
      // says it.
      const pending = {
        pending_id: 'p1',
        session_id: ID,
        kind: 'permission',
        state: 'open',
        payload: {},
        answered: false,
      } as SessionDetail['pending'][number]
      const s = server({ pages: [() => json(page(rows(1, 't5')))], detail: () => json(detail('claude', { activity: 'idle', pending: [pending] })) })
      const { container } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      await waitFor(() => expect(container.querySelector('header .badge')?.textContent).toBe('Waiting on a question'))
      expect(container.querySelector('header .badge')?.className).toBe('badge badge-attn')
      expect(s.of(DETAIL)).toHaveLength(1)
    })

    it('reads an idle detail with no pending request as “Idle”', async () => {
      const s = server({ pages: [() => json(page(rows(1, 't5')))], detail: () => json(detail('claude', { activity: 'idle' })) })
      const { container } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      await waitFor(() => expect(container.querySelector('header .badge')?.textContent).toBe('Idle'))
    })

    it('reads the header from the summary it is given, without fetching the detail', async () => {
      const summary = { ...detail('claude'), pending: undefined, title: undefined, question_waits: true } as unknown as SessionSummary
      const s = server({ pages: [() => json(page(rows(1, 't5')))] })
      render(<SessionView id={ID} summary={summary} timing={FAST} />, { wrapper: s.wrapper })
      // No title: the project directory's name.
      expect(await screen.findByRole('heading', { name: 'project' })).toBeInTheDocument()
      expect(screen.getByText('Waiting on a question')).toBeInTheDocument()
      await screen.findByText('t5-0')
      expect(s.of(DETAIL)).toHaveLength(0)
    })

    it('follows the summary it is given as it changes', async () => {
      const base = { ...detail('claude'), pending: undefined, title: 'A task' } as unknown as SessionSummary
      const s = server({ pages: [() => json(page(rows(1, 't5')))] })
      const { rerender } = render(<SessionView id={ID} summary={base} timing={FAST} />, { wrapper: s.wrapper })
      const head = (await screen.findByRole('heading', { name: 'A task' })).closest('header') as HTMLElement
      expect(within(head).getByText('Idle')).toBeInTheDocument()
      rerender(<SessionView id={ID} summary={{ ...base, activity: 'running', title: 'A renamed task' }} timing={FAST} />)
      expect(within(head).getByText('Running')).toBeInTheDocument()
      expect(within(head).getByRole('heading', { name: 'A renamed task' })).toBeInTheDocument()
      expect(s.of(DETAIL)).toHaveLength(0)
    })

    it('takes the summary over a detail fetched before it came', async () => {
      const s = server({ pages: [() => json(page(rows(1, 't5')))], detail: () => json(detail('codex', { title: 'From the detail' })) })
      const { rerender } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findByRole('heading', { name: 'From the detail' })
      const summary = { ...detail('codex'), pending: undefined, title: 'From the list' } as unknown as SessionSummary
      rerender(<SessionView id={ID} summary={summary} timing={FAST} />)
      expect(await screen.findByRole('heading', { name: 'From the list' })).toBeInTheDocument()
    })

    it.each([
      [{ lifecycle: 'starting' }, 'Starting', 'wait'],
      [{ activity: 'blocked' }, 'Waiting on a question', 'attn'],
      [{ lifecycle: 'parked' }, 'Parked', 'idle'],
      [{ lifecycle: 'parked', presumed_parked: true }, 'Host offline', 'idle'],
      [{ lifecycle: 'closed' }, 'Closed', 'idle'],
      [{ lifecycle: 'failed', failure_reason: 'agent_not_logged_in' }, 'Failed', 'fail'],
      // Waiting on a question wins over every lifecycle, as on the row.
      [{ lifecycle: 'parked', question_waits: true }, 'Waiting on a question', 'attn'],
      [{ lifecycle: 'closed', activity: 'blocked' }, 'Waiting on a question', 'attn'],
      [{ lifecycle: 'starting', question_waits: true }, 'Waiting on a question', 'attn'],
    ] as [Partial<SessionDetail> & { question_waits?: boolean }, string, string][])('reads %o as “%s”, as the row does', async (patch, text, tone) => {
      const summary = { ...detail('claude', patch), pending: undefined } as unknown as SessionSummary
      const s = server({ pages: [() => json(page(rows(1, 't5')))] })
      const { container } = render(<SessionView id={ID} summary={summary} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findByText('t5-0')
      expect(container.querySelector('header .badge')?.textContent).toBe(text)
      expect(container.querySelector('header .badge')?.className).toBe(`badge badge-${tone}`)
      if (patch.failure_reason) expect(container.querySelector('header')?.textContent).toContain(`Reason: ${patch.failure_reason}`)
    })

    it('shows the newest plan’s steps in the header', async () => {
      const plan = (id: string, step: string) =>
        ({ id, version: 1, ts: '2026-10-02T10:00:00.000Z', turn_id: 't5', kind: 'plan', entries: [{ content: step, status: 'in_progress' }] }) as Item
      const s = server({ pages: [() => json(page([plan('p1', 'old step'), plan('p2', 'new step')]))] })
      const { container } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findAllByText('new step')
      const header = container.querySelector('.session > .steps') as HTMLElement
      expect(within(header).getAllByText('new step').length).toBeGreaterThan(0)
      expect(within(header).queryByText('old step')).toBeNull()
    })

    it('names the hats of a reassignment from the hat list', async () => {
      const moved = { id: 'mv', version: 1, ts: '2026-10-02T10:00:00.000Z', turn_id: 't5', kind: 'marker', marker: 'hat_reassigned', from: 'h-a', to: 'h-b' } as Item
      const s = server({
        pages: [() => json(page([moved]))],
        hats: () => json([{ id: 'h-a', name: 'Work' }, { id: 'h-b', name: 'Home' }]),
      })
      const { container } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      await waitFor(() => expect(container.querySelector('.marker')?.textContent).toContain('from Work to Home'))
    })

    it('fetches no hats when no item names one', async () => {
      const s = server({ pages: [() => json(page(rows(1, 't5')))] })
      render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findByText('t5-0')
      expect(s.of('/api/hats')).toHaveLength(0)
    })

    it('has a way back to the list', async () => {
      const s = server({ pages: [() => json(page(rows(1, 't5')))] })
      render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      expect(await screen.findByRole('link', { name: 'Back to sessions' })).toHaveAttribute('href', '/sessions')
    })

    it('starts the header’s steps closed on a phone', async () => {
      window.matchMedia = vi.fn().mockImplementation((query: string) => ({
        matches: query === '(max-width: 767px)',
        addEventListener: () => {},
        removeEventListener: () => {},
      })) as unknown as typeof window.matchMedia
      const plan = { id: 'p', version: 1, ts: '2026-10-02T10:00:00.000Z', turn_id: 't5', kind: 'plan', entries: [{ content: 'a', status: 'in_progress' }] } as Item
      const s = server({ pages: [() => json(page([plan]))] })
      const { container } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findAllByText('a')
      expect((container.querySelector('.session > .steps') as HTMLDetailsElement).open).toBe(false)
      expect((container.querySelector('.transcript .steps') as HTMLDetailsElement).open).toBe(true)
    })
  })
  ```

- [ ] **Step 2: Run them, and see them fail**

Run: `nix develop -c sh -c 'cd web && pnpm vitest run'`
Expected: FAIL: the view's modules and the Markdown packages do not exist yet, and `App.test.tsx`'s deep link finds the placeholder.

- [ ] **Step 3: The view**

In `nix/web.nix`, replace:

  ```nix
      hash = "sha256-5p7sGn/gcxYo5HKokMqyUG0Y25POOVnJiNFyi8I4vxI=";
  ```

with:

  ```nix
      hash = "sha256-iLpOElKP64wYOWB5Fvv6iUEnTZ2Z1jFbbrFHFdXfYi0=";
  ```

In `web/package.json`, replace:

  ```json
      "react-dom": "19.3.0"
  ```

with:

  ```json
      "react-dom": "19.3.0",
      "react-markdown": "10.1.0",
      "rehype-highlight": "7.0.2",
      "rehype-sanitize": "6.0.0",
      "remark-breaks": "4.0.0",
      "remark-gfm": "4.0.1"
  ```

Create `web/src/api/names.ts`:

  ```ts
  // The names a session view shows in place of ids: hosts (`GET /api/hosts`)
  // and hats (`GET /api/hats`). Both answer a bare array.
  import type { HatItem, HostItem } from '../generated/protocol'
  import type { Client } from './client'

  export function hostList(client: Client): Promise<HostItem[]> {
    return client.request<HostItem[]>('GET', '/api/hosts')
  }

  export function hatList(client: Client): Promise<HatItem[]> {
    return client.request<HatItem[]>('GET', '/api/hats')
  }

  /** `id → name` of a list that may not be one: anything else is no names. */
  export function namesOf(list: unknown, key: 'host_id' | 'id'): Map<string, string> {
    const names = new Map<string, string>()
    if (!Array.isArray(list)) return names
    for (const entry of list) {
      if (!entry || typeof entry !== 'object') continue
      const e = entry as Record<string, unknown>
      if (typeof e[key] === 'string' && typeof e.name === 'string') names.set(e[key] as string, e.name)
    }
    return names
  }
  ```

Create `web/src/components/ItemBoundary.tsx`:

  ```tsx
  // One item's error boundary (frontend spec §6.4): an item that throws while
  // rendering shows a line in its place, and the rest of the transcript
  // stays. It is keyed by the item's id; a new version of the item renders
  // it again. A tool call's possible fabrication (F-13) outlives any failure
  // of its renderer: the fallback still shows the banner, as text.
  import { Component, type ReactNode } from 'react'

  interface Props {
    version: number
    /** A tool call's `fabricated`, as text: shown even when it fails. */
    fabricated?: string
    children: ReactNode
  }

  interface State {
    failed: boolean
    version: number
  }

  export default class ItemBoundary extends Component<Props, State> {
    state: State = { failed: false, version: this.props.version }

    static getDerivedStateFromError(): Partial<State> {
      return { failed: true }
    }

    static getDerivedStateFromProps(props: Props, state: State): Partial<State> | null {
      return props.version !== state.version ? { failed: false, version: props.version } : null
    }

    render() {
      if (this.state.failed) {
        const { fabricated } = this.props
        return (
          <>
            {fabricated !== undefined && (
              <div className="fab-warn" role="alert">
                <span className="fab-warn-tag">Possible fabricated tool call</span>
                <span>{fabricated}</span>
              </div>
            )}
            <div className="divider item-failed">Could not render this item</div>
          </>
        )
      }
      return this.props.children
    }
  }
  ```

Create `web/src/components/Markdown.tsx`:

  ```tsx
  // The one place an agent's Markdown becomes DOM (frontend spec §6.4; client
  // view spec §5.3): messages and thinking share this plugin stack.
  //
  //   remark-gfm       tables, task lists, strikethrough, autolinks
  //   remark-breaks    a single newline is a line break, as in a terminal
  //   htmlAsText       raw HTML in the text stays text: it is never parsed
  //                    (there is no rehype-raw), and it is never dropped
  //   rehype-sanitize  the default (GitHub) schema over what Markdown made,
  //                    without a second id prefix (see `schema`)
  //   rehype-highlight fenced code, highlight.js' common languages only, no
  //                    guessing; an unknown language is left plain, never thrown.
  //                    Version 7's options are `detect`, `languages` (default:
  //                    lowlight's `common`), `plainText`, `prefix` and `subset`;
  //                    it has no `ignoreMissing`: a fence in a language it does
  //                    not have becomes a message on the file, not an error.
  //
  // Links open in a new tab without an opener (a footnote's stays on the page). An image never loads on its own:
  // it is a link with its alt text. react-markdown's default `urlTransform`
  // stays, so a `javascript:` URL loses its href.
  import type { ComponentProps } from 'react'
  import ReactMarkdown, { type Components } from 'react-markdown'
  import rehypeHighlight from 'rehype-highlight'
  import rehypeSanitize, { defaultSchema, type Options as SanitizeSchema } from 'rehype-sanitize'
  import remarkBreaks from 'remark-breaks'
  import remarkGfm from 'remark-gfm'

  interface MdNode {
    type: string
    value?: string
    children?: MdNode[]
  }

  /** A remark plugin: every raw HTML node (block or inline) becomes a text
   *  node with the same source, so it renders as the characters it is. */
  export function htmlAsText() {
    const visit = (node: MdNode) => {
      if (node.type === 'html') node.type = 'text'
      node.children?.forEach(visit)
    }
    return visit
  }

  // rehype-highlight registers its languages each time it is set up, and
  // react-markdown sets its plugins up for every message it renders. Set up
  // once, its transformer is shared by every message instead.
  const highlight = rehypeHighlight({ detect: false })
  function sharedHighlight() {
    return highlight
  }

  // The GitHub schema, but no second prefix on ids. The ids Markdown makes
  // here are all footnotes' (remark-rehype): a note's and its reference's
  // carry `user-content-`, and their links point at those (prefixed again,
  // every footnote link would miss); the notes' heading is
  // `id="footnote-label"`, unprefixed. No other id or `name` can come in,
  // and that, not the prefix, is what makes `clobberPrefix: ''` safe: raw HTML
  // never becomes elements (`htmlAsText`, no rehype-raw).
  const schema: SanitizeSchema = { ...defaultSchema, clobberPrefix: '' }

  /** An image in Markdown: never loaded, a link with its alt text. `src` has
   *  been through `urlTransform` (an unsafe one is empty) and the sanitizer
   *  (an empty one is gone); either way there is no link. */
  export function MarkdownImage({ src, alt }: ComponentProps<'img'> & { node?: unknown }) {
    const label = alt ? `Image: ${alt}` : 'Image'
    return typeof src === 'string' && src ? (
      <a href={src} target="_blank" rel="noopener noreferrer" className="md-img-link">
        {label}
      </a>
    ) : (
      <span className="md-img-link">{label}</span>
    )
  }

  const components: Components = {
    // A link within the message (a footnote) stays on the page.
    a: ({ node: _node, ...props }: ComponentProps<'a'> & { node?: unknown }) =>
      typeof props.href === 'string' && props.href.startsWith('#') ? (
        <a {...props} />
      ) : (
        <a {...props} target="_blank" rel="noopener noreferrer" />
      ),
    img: MarkdownImage,
  }

  export default function Markdown({ children }: { children: string }) {
    return (
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkBreaks, htmlAsText]}
        rehypePlugins={[[rehypeSanitize, schema], sharedHighlight]}
        components={components}
      >
        {children}
      </ReactMarkdown>
    )
  }
  ```

Create `web/src/components/SessionHeader.tsx`:

  ```tsx
  // A session's header (frontend spec §6.2): its title, agent, host, branch,
  // where it stands in words, model and mode, and the newest step list. On a
  // phone it carries the way back to the list, and the steps start closed.
  import type { SessionItem } from '../generated/protocol'
  import type { PlanEntry } from '../generated/view'
  import { agentLabel } from '../lib/agent'
  import { statusOf } from '../lib/status'
  import { basename } from '../lib/time'
  import { Icon } from '../lib/ui'
  import { Link } from '../router'
  import StepList from './StepList'

  /** What the header reads: a list summary or a session's detail. */
  export type HeaderInfo = SessionItem & { question_waits?: boolean }

  export function titleOf(info: HeaderInfo | undefined, id: string): string {
    return info?.title || basename(info?.cwd ?? '') || id
  }

  interface Props {
    id: string
    info?: HeaderInfo
    hostName?: string
    plan?: { entries: PlanEntry[]; truncated?: boolean }
    /** Under 768 px: the steps start closed. */
    narrow: boolean
  }

  export default function SessionHeader({ id, info, hostName, plan, narrow }: Props) {
    // The list's marker, word for word (plan 4c decision 8): the header and
    // the row never disagree.
    const status = info
      ? statusOf({
          lifecycle: info.lifecycle,
          activity: info.activity,
          question_waits: !!info.question_waits,
          presumed_parked: !!info.presumed_parked,
        })
      : null
    return (
      <>
      <header className="conv-head session-head">
        <div className="conv-title-row">
          <Link to="/sessions" className="back-btn" aria-label="Back to sessions">
            <Icon.Chevron size={18} className="rot-180" />
          </Link>
          {status && (
            <span className={'badge badge-' + status.tone}>
              <span className="led" />
              {status.label}
            </span>
          )}
          <h1 className="conv-name">
            <bdi>{titleOf(info, id)}</bdi>
          </h1>
        </div>
        {info && (
          <div className="conv-meta">
            <span className="meta-pill" aria-label="Agent">
              {agentLabel(info.agent)}
            </span>
            {hostName && (
              <span className="meta-pill" aria-label="Host">
                <Icon.Cpu size={12} /> <bdi>{hostName}</bdi>
              </span>
            )}
            {info.git_branch && (
              <span className="meta-pill" aria-label="Branch">
                <Icon.Branch size={12} /> <bdi>{info.git_branch}</bdi>
                {info.git_dirty && <b className="meta-dirty"> (changed)</b>}
              </span>
            )}
            {info.model && (
              <span className="meta-pill" aria-label="Model">
                <bdi>{info.model}</bdi>
              </span>
            )}
            {info.mode && (
              <span className="meta-pill" aria-label="Mode">
                <bdi>{info.mode}</bdi>
              </span>
            )}
            {info.lifecycle === 'failed' && info.failure_reason && (
              <span className="meta-item">
                Reason: <bdi>{info.failure_reason}</bdi>
              </span>
            )}
          </div>
        )}
      </header>
        {plan && <StepList entries={plan.entries} truncated={plan.truncated} open={narrow ? false : undefined} />}
      </>
    )
  }
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
  import SignOut from './SignOut'
  import { useMediaQuery } from '../hooks/useMediaQuery'
  ```

with:

  ```tsx
  import SignOut from './SignOut'
  import SessionView from '../screens/Session'
  import { useMediaQuery } from '../hooks/useMediaQuery'
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
              <Placeholder title="Session" detail={route.id} text="The session view arrives with the transcript." />
  ```

with:

  ```tsx
              <SessionView key={route.id} id={route.id ?? ''} />
  ```

Create `web/src/components/StepList.tsx`:

  ```tsx
  // The session's own step list, as the agent keeps it (ACP `plan`; frontend
  // spec §6.2): the newest snapshot only. The adapter shows its plan instead
  // of the tool calls that make it, so this is the only place the steps are.
  //
  // Open while work is in progress and closed once every step is done, unless
  // the caller says (the header keeps it closed on a phone).
  import type { PlanEntry } from '../generated/view'

  interface Props {
    entries: PlanEntry[]
    /** Steps were left out or cut. */
    truncated?: boolean
    /** Overrides the open-while-in-progress default. */
    open?: boolean
    className?: string
  }

  export default function StepList({ entries, truncated, open, className }: Props) {
    if (entries.length === 0) return null
    const done = entries.filter((e) => e.status === 'completed').length
    const current = entries.find((e) => e.status === 'in_progress')
    return (
      <details className={'steps' + (className ? ` ${className}` : '')} open={open ?? done < entries.length}>
        <summary>
          <span className="steps-count">
            {done} / {entries.length}
          </span>
          <span className="steps-current">{current ? current.content || 'Untitled step' : 'Steps'}</span>
        </summary>
        <ol>
          {entries.map((e, i) => (
            <li key={i} className={e.status === 'in_progress' ? 'on' : e.status === 'completed' ? 'done' : ''}>
              {e.content || 'Untitled step'}
            </li>
          ))}
        </ol>
        {truncated && <p className="item-note">Some steps were cut or left out.</p>}
      </details>
    )
  }
  ```

Create `web/src/components/Transcript.tsx`:

  ```tsx
  // A session's items, in the order the store holds them (the collector folds
  // the events, D1): one renderer per kind, each in its own error boundary
  // keyed by the item's id.
  //
  // A row renders again only when its item (a new object for each version the
  // store takes) or the session's environment changes: an upsert at the tail
  // does not parse every message's Markdown again.
  import { memo } from 'react'
  import type { Item } from '../generated/view'
  import ItemBoundary from './ItemBoundary'
  import AgentMessage from './items/AgentMessage'
  import Marker from './items/Marker'
  import PlanItem from './items/PlanItem'
  import QuestionCard from './items/QuestionCard'
  import Thinking from './items/Thinking'
  import ToolCall from './items/ToolCall'
  import Unrecognised from './items/Unrecognised'
  import UserTurn from './items/UserTurn'
  import type { ItemEnv } from './items/types'

  export function ItemView({ item, env }: { item: Item; env: ItemEnv }) {
    switch (item.kind) {
      case 'user_turn':
        return <UserTurn item={item} env={env} />
      case 'message':
        return <AgentMessage item={item} env={env} />
      case 'thinking':
        return <Thinking item={item} env={env} />
      case 'tool_call':
        return <ToolCall item={item} env={env} />
      case 'plan':
        return <PlanItem item={item} env={env} />
      case 'question':
        return <QuestionCard item={item} agent={env.agent} actions={env.questionActions?.(item)} />
      case 'marker':
        return <Marker item={item} env={env} />
      case 'unrecognised':
        return <Unrecognised item={item} env={env} />
      default: {
        // A kind from a newer server: named, never dropped.
        const kind = String((item as { kind?: unknown }).kind)
        return (
          <details className="unrec fade-in">
            <summary>
              Unsupported item (<bdi>{kind}</bdi>)
            </summary>
          </details>
        )
      }
    }
  }

  /** A tool call's `fabricated` as text, read outside its renderer so the
   *  boundary can show it whatever the renderer does. */
  export function fabricatedOf(item: Item): string | undefined {
    if (item.kind !== 'tool_call') return undefined
    const fabricated = (item as { fabricated?: unknown }).fabricated
    if (fabricated === undefined) return undefined
    return typeof fabricated === 'string' ? fabricated : ''
  }

  const Row = memo(function Row({ item, env }: { item: Item; env: ItemEnv }) {
    return (
      <ItemBoundary version={item.version} fabricated={fabricatedOf(item)}>
        <ItemView item={item} env={env} />
      </ItemBoundary>
    )
  })

  export default function Transcript({ items, env }: { items: Item[]; env: ItemEnv }) {
    if (items.length === 0) return <p className="transcript-empty">Nothing in this session yet.</p>
    return (
      <>
        {items.map((item) => (
          <Row key={item.id} item={item} env={env} />
        ))}
      </>
    )
  }
  ```

Create `web/src/components/items/AgentMessage.tsx`:

  ```tsx
  // The agent's reply, as Markdown.
  import Markdown from '../Markdown'
  import { CutNote, Speaker } from './parts'
  import type { ItemEnv, ItemOf } from './types'

  export default function AgentMessage({ item, env }: { item: ItemOf<'message'>; env: ItemEnv }) {
    return (
      <Speaker who="agent" label={env.agent} ts={item.ts}>
        <div className="bubble">
          <Markdown>{item.text}</Markdown>
        </div>
        {item.truncated && <CutNote sessionId={env.sessionId}>This message was cut.</CutNote>}
      </Speaker>
    )
  }
  ```

Create `web/src/components/items/Marker.tsx`:

  ```tsx
  // A divider for what happened to the session rather than in its
  // conversation (frontend spec §6.1): one plain label per marker kind, the
  // reason in words where it is known (else the code as sent), and any text
  // behind a disclosure, as text.
  import type { MarkerKind } from '../../generated/view'
  import { clockTime } from '../../lib/time'
  import { CutNote } from './parts'
  import type { ItemEnv, ItemOf } from './types'

  type MarkerItem = ItemOf<'marker'>

  export const MARKER_LABEL: Record<MarkerKind, string> = {
    parked: 'Parked',
    resumed: 'Resumed',
    closed: 'Closed',
    host_restarted: 'The host restarted',
    host_offline: 'Host offline',
    host_back: 'Host back',
    turn_interrupted: 'The turn was interrupted',
    turn_failed: 'The turn failed',
    turn_cancelled: 'The turn was stopped',
    turn_not_delivered: 'The turn was not delivered',
    start_not_delivered: 'The start was not delivered',
    start_failed: 'The session failed to start',
    adapter_exited: 'The agent’s process exited',
    transcript_gap: 'Part of the transcript is missing',
    host_note: 'A note from the host',
    conflict: 'The host sent a conflicting update',
    hat_reassigned: 'Moved to another hat',
    elided: 'This turn holds more than is shown here',
  }

  /** Known reason codes, in words, by marker kind. */
  const REASON: Partial<Record<MarkerKind, Record<string, string>>> = {
    parked: {
      idle: 'it was idle',
      operator: 'on request',
      adapter_exited: 'the agent’s process exited',
    },
    host_offline: {
      host_offline: 'the host went offline',
      host_revoked: 'the host was revoked',
    },
    start_failed: {
      agent_has_no_record: 'the agent has no record of this session',
      agent_not_logged_in: 'the agent is not logged in on the host',
      load_unsupported: 'the agent cannot open an earlier session',
      start_failed: 'the agent could not start',
    },
    host_note: {
      config_failed: 'a setting did not take',
      reapply_failed: 'a setting could not be applied again',
      cancel_unanswered: 'a question was left unanswered when the turn stopped',
      replay_unknown_dropped: 'history of an unknown kind was left out',
      agent_home_moved: 'the agent’s data directory moved',
    },
    elided: {
      items: 'too many items',
      bytes: 'too much text',
      questions: 'too many questions',
    },
  }

  /** The reason in words; an unknown code is shown as sent. */
  export function reasonWords(kind: MarkerKind, reason: string): string {
    // Own keys only, at both levels: a kind of `__proto__` must not reach
    // the prototype's `toString`.
    const known = Object.hasOwn(REASON, kind) ? REASON[kind] : undefined
    if (known && Object.hasOwn(known, reason)) return known[reason]
    if (kind === 'transcript_gap') return `host events ${reason}`
    if (kind === 'conflict') return `at host event ${reason}`
    return reason
  }

  export function markerLabel(kind: string): string {
    return Object.hasOwn(MARKER_LABEL, kind) ? MARKER_LABEL[kind as MarkerKind] : 'Something happened'
  }

  export default function Marker({ item, env }: { item: MarkerItem; env: ItemEnv }) {
    const clock = clockTime(item.ts)
    const known = Object.hasOwn(MARKER_LABEL, item.marker)
    const hat = (id: string | undefined) => (id ? (env.hatName?.(id) ?? id) : 'no hat')
    return (
      <div className={'marker fade-in marker-' + (known ? item.marker : 'unknown')}>
        <div className="divider">
          <span>
            {markerLabel(item.marker)}
            {!known && (
              <>
                {' '}
                (<bdi>{item.marker}</bdi>)
              </>
            )}
            {item.marker === 'hat_reassigned' && (
              <>
                : from <bdi>{hat(item.from)}</bdi> to <bdi>{hat(item.to)}</bdi>
              </>
            )}
            {item.reason && (
              <>
                {' '}
                (<bdi>{reasonWords(item.marker, item.reason)}</bdi>)
              </>
            )}
            {clock && <span className="marker-when"> · {clock}</span>}
          </span>
          {item.marker === 'turn_not_delivered' && env.onSendAgain && item.about_turn && (
            <button type="button" className="btn btn-ghost btn-sm" onClick={() => env.onSendAgain?.(item)}>
              Send again
            </button>
          )}
        </div>
        {item.text && (
          <details className="marker-text">
            <summary>Details</summary>
            <pre>{item.text}</pre>
          </details>
        )}
        {item.marker === 'elided' && <CutNote sessionId={env.sessionId}>The rest is in the session’s events.</CutNote>}
      </div>
    )
  }
  ```

Create `web/src/components/items/PlanItem.tsx`:

  ```tsx
  // A plan update in the transcript: the step list as it stood then.
  import StepList from '../StepList'
  import { Speaker } from './parts'
  import type { ItemEnv, ItemOf } from './types'

  export default function PlanItem({ item, env }: { item: ItemOf<'plan'>; env: ItemEnv }) {
    return (
      <Speaker who="agent" label={env.agent} ts={item.ts}>
        <StepList entries={item.entries} truncated={item.truncated} className="steps-item" />
      </Speaker>
    )
  }
  ```

Create `web/src/components/items/QuestionCard.tsx`:

  ```tsx
  // A question the agent asked (frontend spec §6.3), read-only here: the
  // request, its options or fields, and where it stands, in words. The
  // answering controls come in through `actions`; without them the card only
  // shows. Options are styled by their kind, never by their agent-chosen names.
  import type { ReactNode } from 'react'
  import type { PendingReason } from '../../generated/protocol'
  import type { Field } from '../../generated/view'
  import type { ItemOf } from './types'

  type Question = ItemOf<'question'>

  export const REASON_WORDS: Record<PendingReason, string> = {
    turn_cancelled: 'the turn was stopped',
    session_closed: 'the session was closed',
    session_parked: 'the session was parked',
    adapter_lost: 'the agent’s process was lost',
    host_restarted: 'the host restarted',
    agent_withdrew: 'the agent withdrew the question',
    host_revoked: 'the host was revoked',
  }

  /** Where a question stands, from the item alone (the table in §6.3 without
   *  its local states: an answer in flight, an answer refused). */
  export function questionStateText(q: Question): string {
    if (q.delivered === true) return 'Answered'
    if (q.state === 'cancelled') {
      // Own keys only: a reason of `constructor` must read as sent.
      const why = q.reason ? (Object.hasOwn(REASON_WORDS, q.reason) ? REASON_WORDS[q.reason] : q.reason) : undefined
      return why ? `The agent stopped waiting (${why})` : 'The agent stopped waiting'
    }
    if (q.delivered === false) return 'Sent, but the agent was no longer waiting'
    if (q.state === 'delivered') return 'Answered'
    if (q.answered) return 'Sent'
    if (q.answerable) return 'Needs your answer'
    return 'Open'
  }

  /** A permission option's style, from its kind alone. */
  export function optionClass(kind: string): string {
    if (kind.startsWith('reject')) return 'q-opt q-opt-reject'
    if (kind === 'allow_always') return 'q-opt q-opt-always'
    return 'q-opt q-opt-allow'
  }

  function FieldView({ field }: { field: Field }) {
    return (
      <div className="elic-field">
        <span className="elic-label">{field.label || field.key}</span>
        {field.hint && <span className="elic-hint">{field.hint}</span>}
        {field.options && field.options.length > 0 && (
          <ul className="elic-opts">
            {field.options.map((option, i) => (
              <li key={i} className="elic-opt">
                <span className="elic-opt-value">{option.label || option.value}</span>
                {option.description && <span className="elic-opt-desc">{option.description}</span>}
              </li>
            ))}
          </ul>
        )}
        {field.field_kind === 'unsupported' && <span className="elic-unsupported-msg">This field cannot be filled in here.</span>}
      </div>
    )
  }

  interface Props {
    item: Question
    /** The agent's label: who is asking. */
    agent: string
    /** Controls that answer it; absent, the card is read-only. */
    actions?: ReactNode
  }

  export default function QuestionCard({ item, agent, actions }: Props) {
    const { request } = item
    const state = questionStateText(item)
    const live = item.answerable
    return (
      <section className={'ask fade-in' + (live ? '' : ' stale')} aria-label={`Question from ${agent}`}>
        <div className="ask-eyebrow">
          <span className="e-tag">
            {request.type === 'permission' ? `${agent} asks for permission` : `${agent} asks`}
          </span>
        </div>
        {request.type === 'permission' ? (
          <>
            <p className="ask-q">{request.title || 'Permission'}</p>
            {request.options.length === 0 ? (
              <p className="item-note">This question cannot be answered here: stop, park or close the session.</p>
            ) : (
              <ul className="q-opts">
                {request.options.map((option) => (
                  <li key={option.option_id} className={optionClass(option.option_kind)}>
                    {option.name}
                  </li>
                ))}
              </ul>
            )}
          </>
        ) : (
          <>
            <p className="ask-q">{request.message}</p>
            {request.fields.length > 0 && (
              <div className="elic-fields">
                {request.fields.map((field) => (
                  <FieldView key={field.key} field={field} />
                ))}
              </div>
            )}
            {!request.form_supported && (
              <p className="item-note">This form cannot be filled in here: it can only be declined or cancelled.</p>
            )}
          </>
        )}
        <p className="q-state">{state}</p>
        {actions && <div className="ask-actions">{actions}</div>}
      </section>
    )
  }
  ```

Create `web/src/components/items/Thinking.tsx`:

  ```tsx
  // The agent's reasoning: collapsed until asked for, then Markdown.
  import { useState } from 'react'
  import { Icon } from '../../lib/ui'
  import Markdown from '../Markdown'
  import { CutNote, Speaker } from './parts'
  import type { ItemEnv, ItemOf } from './types'

  export default function Thinking({ item, env }: { item: ItemOf<'thinking'>; env: ItemEnv }) {
    const [open, setOpen] = useState(false)
    return (
      <Speaker who="agent" label={env.agent} ts={item.ts}>
        <button type="button" className="tool-line think-line" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
          <Icon.Sparkle size={16} />
          <span className="tool-name">Thinking</span>
          <span className="tool-summary">{open ? 'Hide' : 'Show reasoning'}</span>
        </button>
        {open && (
          <div className="think-body">
            <Markdown>{item.text}</Markdown>
          </div>
        )}
        {item.truncated && <CutNote sessionId={env.sessionId}>This reasoning was cut.</CutNote>}
      </Speaker>
    )
  }
  ```

Create `web/src/components/items/ToolCall.tsx`:

  ```tsx
  // One tool call (frontend spec §6.4): a line with its name, a summary and its
  // status, expanding to the input, the output, the content blocks and the
  // locations. Every one of them is text. A possible fabrication (F-13) is a
  // banner that shows whether or not the call is expanded.
  import { useState } from 'react'
  import type { Location, ToolContent } from '../../generated/view'
  import { tokJSON } from '../../lib/highlight'
  import { Icon } from '../../lib/ui'
  import { CutNote, Speaker } from './parts'
  import { SHOWN_IMAGE_TYPES } from './UserTurn'
  import type { ItemEnv, ItemOf } from './types'

  // Read with `Object.hasOwn` only: a status is the agent's text, and
  // `__proto__` or `constructor` must not reach the prototype.
  const STATUS_WORDS: Record<string, string> = {
    pending: 'Pending',
    in_progress: 'Running',
    completed: 'Done',
    failed: 'Failed',
  }

  const STATUS_COLOUR: Record<string, string> = {
    pending: 'var(--fg-quiet)',
    in_progress: 'var(--st-wait)',
    completed: 'var(--st-run)',
    failed: 'var(--st-attn)',
  }

  type Tool = ItemOf<'tool_call'>

  /** The line's name: the agent's title, else its kind of tool. */
  export function toolName(tool: Tool): string {
    return tool.title || tool.tool_kind || 'Tool'
  }

  /** The line's summary: a shell command, else the first place it touched. */
  export function toolSummary(tool: Tool): string {
    const input = tool.input
    if (input && typeof input === 'object' && !Array.isArray(input)) {
      const command = (input as Record<string, unknown>).command
      if (typeof command === 'string') return command
      if (Array.isArray(command) && command.every((part) => typeof part === 'string')) return command.join(' ')
    }
    const first = tool.locations?.[0]
    return first ? locationText(first) : ''
  }

  function locationText(location: Location): string {
    return location.line === undefined ? location.path : `${location.path}:${location.line}`
  }

  /** The input as it is shown: a string (a cut one is the start of its JSON)
   *  as it is, anything else as indented JSON. */
  export function inputText(input: unknown): string {
    if (typeof input === 'string') return input
    try {
      return JSON.stringify(input, null, 2) ?? String(input)
    } catch {
      return String(input)
    }
  }

  /** JSON, one line per element, in the token colours. A tool's input is the
   *  agent's words: it gets no redaction mark (tokJSON's `tk-redact`), which
   *  only the server's own redactions may wear. */
  function JsonLines({ text }: { text: string }) {
    return (
      <>
        {text.split('\n').map((line, i) => (
          <span key={i} className="io-json-ln">
            {tokJSON(line).map((seg, j) => (
              <span key={j} className={seg.c === 'tk-redact' ? 'tk-str' : seg.c}>
                {seg.t}
              </span>
            ))}
          </span>
        ))}
      </>
    )
  }

  function ContentBlock({ block }: { block: ToolContent }) {
    switch (block.type) {
      case 'text':
        return <div className="tool-io">{block.text}</div>
      case 'diff':
        return (
          <div className="tool-io tool-diff">
            <span className="io-label">
              {block.old_text === undefined ? 'new file' : 'diff'} <bdi>{block.path}</bdi>
            </span>
            {block.old_text !== undefined && (
              <>
                <span className="io-label">before</span>
                <pre className="diff-old">{block.old_text}</pre>
              </>
            )}
            <span className="io-label">after</span>
            <pre className="diff-new">{block.new_text}</pre>
          </div>
        )
      case 'image':
        if (!SHOWN_IMAGE_TYPES.has(block.mime_type)) {
          return (
            <p className="item-note">
              An image of type <bdi>{block.mime_type}</bdi>, not shown.
            </p>
          )
        }
        if (!block.data) return <p className="item-note">An image too large to show here.</p>
        return <img className="tool-image" src={`data:${block.mime_type};base64,${block.data}`} alt="Tool output" />
      case 'terminal':
        return (
          <div className="tool-io">
            <span className="io-label">terminal</span>
            <bdi>{block.terminal_id}</bdi>
          </div>
        )
      case 'other':
        return (
          <div className="tool-io">
            <span className="io-label">other</span>
            {block.raw}
          </div>
        )
      default: {
        // A block type this view does not know: its JSON, as text.
        return <div className="tool-io">{inputText(block)}</div>
      }
    }
  }

  export default function ToolCall({ item, env }: { item: Tool; env: ItemEnv }) {
    const [open, setOpen] = useState(false)
    const hasInput = item.input !== undefined && item.input !== null
    const content = item.content ?? []
    const locations = item.locations ?? []
    const expandable = hasInput || !!item.output || content.length > 0 || locations.length > 0
    const summary = toolSummary(item)
    const status = item.status
    return (
      <Speaker who="agent" label={env.agent} ts={item.ts}>
        {item.fabricated !== undefined && (
          <div className="fab-warn" role="alert">
            <span className="fab-warn-tag">Possible fabricated tool call</span>
            <span>{item.fabricated}</span>
          </div>
        )}
        <button
          type="button"
          className="tool-line"
          aria-expanded={expandable ? open : undefined}
          onClick={() => expandable && setOpen((o) => !o)}
        >
          <Icon.Wrench size={16} />
          <span className="tool-name">{toolName(item)}</span>
          {summary && <span className="tool-summary">{summary}</span>}
          {status && (
            <span className="tool-status" style={{ color: Object.hasOwn(STATUS_COLOUR, status) ? STATUS_COLOUR[status] : 'var(--fg-quiet)' }}>
              {Object.hasOwn(STATUS_WORDS, status) ? STATUS_WORDS[status] : status}
            </span>
          )}
        </button>
        {open && (
          <div className="tool-detail">
            {hasInput && (
              <div className="tool-io tool-input">
                <span className="io-label">input</span>
                <JsonLines text={inputText(item.input)} />
              </div>
            )}
            {item.output && (
              <div className="tool-io tool-output">
                <span className="io-label">output</span>
                {item.output}
              </div>
            )}
            {content.map((block, i) => (
              <ContentBlock key={i} block={block} />
            ))}
            {locations.length > 0 && (
              <ul className="tool-locations">
                {locations.map((location, i) => (
                  <li key={i}>
                    <bdi>{locationText(location)}</bdi>
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
        {item.truncated && <CutNote sessionId={env.sessionId}>Part of this tool call was cut.</CutNote>}
      </Speaker>
    )
  }
  ```

Create `web/src/components/items/Unrecognised.tsx`:

  ```tsx
  // An update this view does not know: never dropped, shown collapsed with its
  // raw JSON as text.
  import { CutNote } from './parts'
  import type { ItemEnv, ItemOf } from './types'

  export default function Unrecognised({ item, env }: { item: ItemOf<'unrecognised'>; env: ItemEnv }) {
    return (
      <details className="unrec fade-in">
        <summary>
          Unsupported update (<bdi>{item.update_kind}</bdi>)
        </summary>
        <pre className="tool-io">{item.raw}</pre>
        {item.truncated && <CutNote sessionId={env.sessionId}>This update was cut.</CutNote>}
      </details>
    )
  }
  ```

Create `web/src/components/items/UserTurn.tsx`:

  ```tsx
  // The operator's prompt, shown exactly as sent: plain text with its line
  // breaks (no Markdown), and its images from the attachment store when they
  // are of a type the page shows.
  import { USER_LABEL } from '../../lib/agent'
  import { CutNote, Speaker } from './parts'
  import type { ItemEnv, ItemOf } from './types'

  /** The image types a page shows; anything else is named, not loaded. */
  export const SHOWN_IMAGE_TYPES: ReadonlySet<string> = new Set(['image/png', 'image/jpeg', 'image/gif', 'image/webp'])

  /** The attachment store's address for an image, or `null` when the hash is
   *  not one (64 lowercase hex digits): then nothing is loaded. */
  export function attachmentHref(sha256: unknown): string | null {
    return typeof sha256 === 'string' && /^[0-9a-f]{64}$/.test(sha256) ? `/api/attachments/${sha256}` : null
  }

  function Attachment({ mimeType, sha256 }: { mimeType: string; sha256: unknown }) {
    const href = attachmentHref(sha256)
    if (href === null) {
      return (
        <p className="item-note">
          An image of type <bdi>{mimeType}</bdi>, not shown: its reference is not one the attachment store makes.
        </p>
      )
    }
    return <img className="user-image" src={href} alt="An attached image" />
  }

  export default function UserTurn({ item, env }: { item: ItemOf<'user_turn'>; env: ItemEnv }) {
    return (
      <Speaker who="user" label={USER_LABEL} ts={item.ts}>
        <div className="bubble user">
          {item.content.map((block, i) =>
            block.type === 'text' ? (
              <p key={i} className="user-text">
                {block.text}
              </p>
            ) : block.type === 'image' && SHOWN_IMAGE_TYPES.has(block.mimeType) ? (
              <Attachment key={i} mimeType={block.mimeType} sha256={block.sha256} />
            ) : (
              <p key={i} className="item-note">
                An attachment of type <bdi>{block.type === 'image' ? block.mimeType : String((block as { type?: unknown }).type)}</bdi>, not shown.
              </p>
            ),
          )}
        </div>
        {item.truncated && <CutNote sessionId={env.sessionId}>This prompt was cut.</CutNote>}
      </Speaker>
    )
  }
  ```

Create `web/src/components/items/parts.tsx`:

  ```tsx
  // The pieces items share: a speaker's row with its avatar, and the note a
  // cut or elided item carries.
  import type { ReactNode } from 'react'
  import { avatarLetter } from '../../lib/agent'
  import { clockTime } from '../../lib/time'
  import { rawEventsHref } from './types'

  interface SpeakerProps {
    /** Whose row: the operator's, or the agent's. */
    who: 'user' | 'agent'
    label: string
    ts?: string
    children: ReactNode
  }

  export function Speaker({ who, label, ts, children }: SpeakerProps) {
    const when = ts ? clockTime(ts) : ''
    return (
      <div className={'msg fade-in msg-' + who}>
        <div className={'avatar ' + (who === 'user' ? 'avatar-user' : 'avatar-ai')} aria-hidden="true">
          {avatarLetter(label)}
        </div>
        <div className="msg-body">
          <div className="msg-head">
            <span className="msg-who">{label}</span>
            {when && <span className="msg-when">{when}</span>}
          </div>
          {children}
        </div>
      </div>
    )
  }

  /** "Part of this was cut", with the raw events behind it. */
  export function CutNote({ sessionId, children }: { sessionId: string; children: ReactNode }) {
    return (
      <p className="item-note">
        {children}{' '}
        <a href={rawEventsHref(sessionId)} target="_blank" rel="noopener noreferrer">
          Raw events
        </a>
      </p>
    )
  }
  ```

Create `web/src/components/items/types.ts`:

  ```ts
  // What every item renderer is given besides its item: the session it is in
  // and the seams later tasks fill (answering a question, sending a turn
  // again).
  import type { ReactNode } from 'react'
  import type { Item } from '../../generated/view'

  export type ItemOf<K extends Item['kind']> = Extract<Item, { kind: K }>

  export interface ItemEnv {
    sessionId: string
    /** The agent's label (lib/agent.ts): who speaks for the session. */
    agent: string
    /** A hat's name by its id, when the hats are known. */
    hatName?: (id: string) => string | undefined
    /** A question card's actions; none makes the card read-only. */
    questionActions?: (item: ItemOf<'question'>) => ReactNode
    /** "Send again" for a turn that was not delivered; absent, none is offered. */
    onSendAgain?: (item: ItemOf<'marker'>) => void
  }

  /** The session's raw events, where a cut or elided item can be read whole. */
  export function rawEventsHref(sessionId: string): string {
    return `/api/sessions/${encodeURIComponent(sessionId)}/events`
  }
  ```

In `web/src/index.css`, replace:

  ```css
  .badge-idle { background:var(--surface-3); color:var(--fg-muted); }
  .conv-name { font-family:var(--font-display); font-weight:800; font-size:18px; letter-spacing:-.01em; flex:1; min-width:0; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
  ```

with:

  ```css
  .badge-idle { background:var(--surface-3); color:var(--fg-muted); }
  /* Failed: a hollow red ring, as the list's marker; red fill means a question. */
  .badge-fail { background:transparent; color:var(--st-attn); box-shadow:inset 0 0 0 1px var(--st-attn); }
  .conv-name { font-family:var(--font-display); font-weight:800; font-size:18px; letter-spacing:-.01em; flex:1; min-width:0; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
  ```

In `web/src/index.css`, replace:

  ```css
  .rail-head .badge, .mtopbar .badge { margin-left:auto; }
  ```

with:

  ```css
  .rail-head .badge, .mtopbar .badge { margin-left:auto; }

  /* ── The session view (frontend spec §6) ──
     The view fills .main and only .transcript scrolls. Scroll anchoring is
     off: the view keeps the reader's place itself when older turns are
     prepended, and the browser's own anchoring would move it twice. */
  .session { flex:1; min-height:0; display:flex; flex-direction:column; }
  .session .transcript { overflow-anchor:none; }
  .session .transcript-inner { max-width:900px; }
  .session-head .conv-name { margin:0; }
  .back-btn { display:none; }
  .rot-180 { transform:rotate(180deg); }
  .stream-state:empty { display:none; }
  .stream-state { flex:none; padding:6px 20px; font-size:12.5px; font-weight:600; color:var(--st-wait); background:var(--surface-2); border-bottom:1px solid var(--border); }
  .session-error { padding:8px 20px 0; margin:0; }
  .transcript-empty { color:var(--fg-quiet); padding:20px 0; }
  .load-earlier { display:flex; justify-content:center; margin:0 0 18px; }
  .item-note { font-size:12.5px; color:var(--fg-muted); margin:6px 0 0; }
  .item-failed { color:var(--st-attn); }
  .user-text { white-space:pre-wrap; overflow-wrap:anywhere; margin:0 0 8px; }
  .user-text:last-child { margin-bottom:0; }
  .user-image, .tool-image { display:block; max-width:min(100%, 480px); height:auto; border-radius:var(--r-sm); margin:6px 0; }
  .md-img-link { font-style:italic; }
  .tool-detail { display:flex; flex-direction:column; }
  .tool-diff pre { margin:0 0 6px; white-space:pre-wrap; font:inherit; }
  .tool-diff .diff-old { color:#E5766B; }
  .tool-diff .diff-new { color:#B7D88C; }
  .tool-locations { margin:6px 0 0; padding:0; list-style:none; font-family:var(--font-mono); font-size:12px; color:var(--fg-muted); }
  .marker { margin:0 0 22px; }
  .marker .divider { margin-bottom:4px; }
  .marker-when { font-weight:500; color:var(--fg-quiet); }
  .marker-text summary, .unrec summary { cursor:pointer; font-size:12.5px; color:var(--fg-muted); }
  .marker-text pre { white-space:pre-wrap; overflow-wrap:anywhere; font-family:var(--font-mono); font-size:12px; background:var(--surface-2); border:1px solid var(--border); border-radius:var(--r-sm); padding:9px 12px; margin:6px 0 0; }
  .unrec { margin:0 0 22px; }
  .ask { margin:0 0 22px; }
  .q-opts { display:flex; flex-wrap:wrap; gap:8px; margin:0; padding:0; list-style:none; }
  .q-opt { border-radius:var(--r-full); padding:6px 14px; font-size:13px; font-weight:600; border:1.5px solid var(--border-strong); background:var(--surface); }
  .q-opt-allow { border-color:var(--accent); color:var(--accent); }
  .q-opt-always { border-style:dashed; color:var(--fg-2); }
  .q-opt-reject { border-color:var(--st-attn); color:var(--st-attn); }
  .q-state { margin:12px 0 0; font-size:13px; font-weight:700; color:var(--fg-2); }
  .elic-opts { list-style:none; margin:0; padding:0; }
  .steps-item { border:1px solid var(--border); border-radius:var(--r-sm); }
  .meta-dirty { color:var(--st-wait); font-weight:600; }

  @media (max-width:767px) {
    /* The session's header is the bar: a way back, and its title shown. */
    .back-btn { display:flex; align-items:center; justify-content:center; width:44px; height:44px; flex:none; border-radius:11px; border:1px solid var(--border); background:var(--surface); color:var(--fg-1); }
    .session-head .conv-name { display:block; font-size:16px; }
    .q-opts { flex-direction:column; }
  }
  ```

In `web/src/lib/status.ts`, replace:

  ```ts
  // A session's status marker in the list (frontend spec §5; plan 4c
  // decisions 7 and 8). It reads the server's two axes, lifecycle and
  // activity, plus `question_waits` and `presumed_parked`, directly: there is
  // no staleness heuristic and no "actionable" guess (F-9).
  ```

with:

  ```ts
  // A session's status marker in the list and in the session's header
  // (frontend spec §5; plan 4c decisions 7 and 8): one rule for both. It
  // reads the server's two axes, lifecycle and activity, plus
  // `question_waits` and `presumed_parked`, directly: there is no staleness
  // heuristic and no "actionable" guess (F-9).
  ```

In `web/src/lib/time.ts`, replace:

  ```ts
  export function clockTime(iso: string): string {
    if (!iso) return ''
    const d = new Date(iso)
    return isNaN(d.getTime()) ? '' : d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  ```

with:

  ```ts
  // One formatter for every call: `toLocaleTimeString` builds a new one each
  // time, which a transcript of a thousand items pays a thousand times.
  let clock: Intl.DateTimeFormat | undefined
  export function clockTime(iso: string): string {
    if (!iso) return ''
    const d = new Date(iso)
    if (isNaN(d.getTime())) return ''
    clock ??= new Intl.DateTimeFormat([], { hour: '2-digit', minute: '2-digit' })
    return clock.format(d)
  ```

Create `web/src/screens/Session.tsx`:

  ```tsx
  // One session (frontend spec §6): its header and its transcript, read-only.
  //
  // - The items come from the item store (`useSessionItems`): the first page,
  //   then the stream. The header reads the list's summary when the caller has
  //   it, else the session's detail, fetched once.
  // - The transcript opens at its end and stays there while the reader is at
  //   the end. "Load earlier" (or scrolling to the top) prepends the turns
  //   before, keeping what the reader sees where it was.
  // - The stream's state shows as "Reconnecting…", and "Resynced" for a moment
  //   after the items were replaced.
  // - A deleted session says so, and nothing more is fetched.
  import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
  import { hatList, hostList, namesOf } from '../api/names'
  import { sessionDetail } from '../api/view'
  import { useClient } from '../app-client'
  import SessionHeader, { type HeaderInfo } from '../components/SessionHeader'
  import Transcript from '../components/Transcript'
  import type { ItemEnv } from '../components/items/types'
  import type { SessionSummary } from '../generated/view'
  import type { Item } from '../generated/view'
  import { useMediaQuery } from '../hooks/useMediaQuery'
  import { agentLabel } from '../lib/agent'
  import { Icon } from '../lib/ui'
  import { Link } from '../router'
  import { useSessionItems, type Timing } from '../store/useSessionItems'

  /** Within this many pixels of the end, the reader is at the end. */
  const STICK_PX = 80
  /** Within this many pixels of the top, older turns are fetched. */
  const TOP_PX = 120

  interface Props {
    id: string
    /** The list store's summary of the session, when it has one. */
    summary?: SessionSummary
    /** The item store's timing (tests). */
    timing?: Partial<Timing>
    /** Seams for later tasks: question actions and "Send again". */
    env?: Pick<ItemEnv, 'questionActions' | 'onSendAgain'>
  }

  /** The newest plan among the loaded items. */
  function latestPlan(items: Item[]): Extract<Item, { kind: 'plan' }> | undefined {
    for (let i = items.length - 1; i >= 0; i--) {
      const item = items[i]
      if (item.kind === 'plan') return item
    }
    return undefined
  }

  /** The session's detail, once, unless the caller has its summary. */
  function useHeaderInfo(id: string, summary: SessionSummary | undefined): HeaderInfo | undefined {
    const client = useClient()
    const [detail, setDetail] = useState<HeaderInfo | undefined>(undefined)
    const needed = summary === undefined
    useEffect(() => {
      if (!needed) return
      let live = true
      sessionDetail(client, id).then(
        // The detail has no `question_waits`: its open pending requests say
        // it, as the summary's does ("whether or not an answer is queued"),
        // so a question outside a turn reads "Waiting on a question" here
        // too (decision 8: one status rule for the row and the header).
        (d) => live && setDetail({ ...d, question_waits: Array.isArray(d.pending) && d.pending.length > 0 }),
        // The item store says when the session is gone; the header stays bare.
        () => {},
      )
      return () => {
        live = false
      }
    }, [client, id, needed])
    return summary ?? detail
  }

  /** `id → name` from a list fetched once, when `wanted`; on failure, none. */
  function useNames(fetch: 'hosts' | 'hats', wanted: boolean): Map<string, string> {
    const client = useClient()
    const [names, setNames] = useState<Map<string, string>>(() => new Map())
    useEffect(() => {
      if (!wanted) return
      let live = true
      const request = fetch === 'hosts' ? hostList(client) : hatList(client)
      request.then(
        (list) => live && setNames(namesOf(list, fetch === 'hosts' ? 'host_id' : 'id')),
        () => {},
      )
      return () => {
        live = false
      }
    }, [client, fetch, wanted])
    return names
  }

  export default function SessionView({ id, summary, timing, env: seams }: Props) {
    const s = useSessionItems(id, timing)
    const info = useHeaderInfo(id, summary)
    const narrow = useMediaQuery('(max-width: 767px)')
    const hosts = useNames('hosts', true)
    const wantsHats = useMemo(() => s.items.some((i) => i.kind === 'marker' && i.marker === 'hat_reassigned'), [s.items])
    const hats = useNames('hats', wantsHats)
    const plan = useMemo(() => latestPlan(s.items), [s.items])

    const env: ItemEnv = useMemo(
      () => ({
        sessionId: id,
        agent: agentLabel(info?.agent),
        hatName: (hat: string) => hats.get(hat),
        ...seams,
      }),
      [id, info?.agent, hats, seams],
    )

    if (s.removed) {
      return (
        <div className="session">
          <div className="welcome" role="status">
            <h1>This session was deleted</h1>
            <p>
              <bdi>{id}</bdi>
            </p>
            <Link to="/sessions" className="btn btn-ghost">
              Back to sessions
            </Link>
          </div>
        </div>
      )
    }

    return (
      <div className="session">
        <SessionHeader id={id} info={info} hostName={info ? hosts.get(info.host_id) : undefined} plan={plan} narrow={narrow} />
        <div className="stream-state" role="status">
          {s.stream === 'reconnecting' ? 'Reconnecting…' : s.resynced ? 'Resynced' : ''}
        </div>
        {s.error && (
          <p className="form-error session-error">
            <bdi>{s.error}</bdi>
          </p>
        )}
        <Scroller items={s.items} older={s.older} loadingOlder={s.loadingOlder} loadOlder={s.loadOlder}>
          {s.loading ? <p className="transcript-empty">Loading…</p> : <Transcript items={s.items} env={env} />}
        </Scroller>
      </div>
    )
  }

  interface ScrollerProps {
    items: Item[]
    older: boolean
    loadingOlder: boolean
    loadOlder: () => Promise<void>
    children: React.ReactNode
  }

  /** The transcript's scrolling box: opens at the end, follows the end while
   *  the reader is there, and keeps the reader's place when older turns are
   *  prepended. */
  function Scroller({ items, older, loadingOlder, loadOlder, children }: ScrollerProps) {
    const ref = useRef<HTMLDivElement>(null)
    const atEnd = useRef(true)
    const before = useRef<{ first?: string; height: number }>({ height: 0 })

    useLayoutEffect(() => {
      const el = ref.current
      if (!el) return
      const first = items[0]?.id
      const was = before.current
      const prepended = was.first !== undefined && first !== was.first && items.some((i) => i.id === was.first)
      if (prepended) el.scrollTop += el.scrollHeight - was.height
      else if (atEnd.current) el.scrollTop = el.scrollHeight
      before.current = { first, height: el.scrollHeight }
    }, [items])

    const onScroll = () => {
      const el = ref.current
      if (!el) return
      atEnd.current = el.scrollTop + el.clientHeight >= el.scrollHeight - STICK_PX
      if (el.scrollTop < TOP_PX && older && !loadingOlder) void loadOlder()
    }

    return (
      <div className="transcript" ref={ref} onScroll={onScroll}>
        <div className="transcript-inner">
          {older && (
            <div className="load-earlier">
              <button type="button" className="btn btn-ghost btn-sm" disabled={loadingOlder} onClick={() => void loadOlder()}>
                {loadingOlder ? (
                  'Loading…'
                ) : (
                  <>
                    <Icon.ArrowUp size={14} /> Load earlier
                  </>
                )}
              </button>
            </div>
          )}
          {children}
        </div>
      </div>
    )
  }
  ```

Run: `nix develop -c sh -c 'cd web && pnpm install'`
Expected: `pnpm-lock.yaml` gains the five packages and their dependencies; no build script is ignored.

Run: `nix develop -c sh packaging/update-web-hash.sh`
Expected: `ok: <old> -> <new> in nix/web.nix`. The Nix package's web dependencies are a fixed-output derivation (plan 7e-ii-b): a changed lockfile needs its new hash, the one in the block above on macOS. If the ubuntu job reports another, follow `nix/web.nix`'s note; never edit the hash by hand.

- [ ] **Step 4: Run the checks**

Run: `nix develop -c sh -c 'cd web && pnpm install --frozen-lockfile && pnpm typecheck && pnpm test && pnpm build'`
Expected: PASS, 729 tests. The build's initial JS is 190 kB gzip (under decision 19's 350 KiB).

The measurement (decision 20), on demand: `nix develop -c sh -c 'cd web && pnpm build && HENNERY_WINDOWING=1 pnpm exec playwright test windowing'`. It asserts only that it measured the session view (no unstubbed `/api` call; N rows in the DOM), never a time.

- [ ] **Step 5: Revert-probes** (each must fail the test named; restore after each)

147 probes, run by script, all fail as they should. The first run's 5 survivors became tests: a fence with no language is never guessed; an image with an empty source gets no link; a summary that comes after the detail wins; the name lookup ignores anything not a list; a prompt shows exactly as typed, outer spaces and line breaks included.
- **Markdown** (16): raw HTML becomes text, and the plugin is in the list; no guessing a fence's language; highlighting on; sanitizing on; no second id prefix; links in a new tab without an opener; a footnote's link stays on the page; an image never loads, is a link in a new tab, and has no link from an unsafe source; line breaks; GFM; the list markers restored in `.bubble` and `.think-body` (CSS text).
- **Boundary and rows** (7): the boundary catches, resets on a new version, keeps failing at one version; rows keyed by id and memoised; an unknown kind named; an empty transcript.
- **The view** (36): opens at the end, follows it, leaves a reader who scrolled up; the anchor kept on a prepend; older turns on reaching the top, only near it; "Load earlier", absent with nothing older; the summary skips the detail and wins over it, the detail when there is none; hats fetched only when a marker names one; hat and host names from the lists; the agent's label; "This session was deleted", "Reconnecting…", "Resynced"; the newest plan in the header, its steps closed on a phone; back to the list; the title's fallback; every status word (blocked, `question_waits`, running, presumed parked, starting, closed, failed) and the failure's reason; the header's model, branch and agent; the name lookups take only a list of checked entries; the shell mounts the view.
- **Speakers** (8): Claude, Codex, own keys only, an unknown agent as given, the avatar's letter, the user is "You", the label shown; the cut note links the raw events.
- **Items** (80): every kind's renderer and each guard in it: the clock's unparseable time; a user's text exactly as typed and never Markdown, its images' allowed types and encoded hash, a cut turn; a message's Markdown and cut note; thinking closed, Markdown inside, cut; a tool call closed by default, its name's fallback, its command summary, its status words (an unknown one as sent, own keys only), its input as given or indented, no redaction mark, expandable only with content, its output, each content block (text, diff before and new file, an image of a refused type, a `data:` URL, too large, terminal, other, an unknown block as JSON), its locations, the fabricated banner with its text and none without, the cut note; the plan's steps; the question card's options by kind, its seam for Part 2's actions, no options, its states (delivered first, cancelled with the reason in words, no longer waiting, delivered, sent, needs an answer), the form not supported, a field's hint, an unsupported field; a marker's label for parked, host back, turn not delivered, elided and conflict, its reasons in words (idle, the agent's home moved, load unsupported, elided items), a gap's range, a conflict's sequence, hat names and ids, its text in a disclosure, the Send again seam and none without it, the elided note's raw events, an unknown kind named; the unrecognised item's kind, raw JSON and cut note; the step list open while working, its marks, count, an untitled step, a cut list.

Load: 4 parallel copies of the new test files, 3 rounds: 12 of 12 green.


**Re-run on the final code:** 17 of the probes above no longer matched their lines after amendments A2 and O5 and the windowing step; each was retargeted, or superseded by the same guard's probe in Task 5's or the amendments' scripts. The task review's fixes added three: the header from the detail says "Waiting on a question" for a question outside a turn; a card's and a marker's reason looked up by own key.

- [ ] **Step 6: Commit**

```bash
git add web ':!web/e2e/windowing.spec.ts'
git commit -m "feat(web): a read-only session view: header, transcript of every item kind, safe Markdown"
git add web/e2e/windowing.spec.ts
git commit -m "test(web): a gated Chromium measurement of the transcript's render and scroll cost"
```

---

### Task 5: The list and the view together; two flaky tests

A session's link (a push notification's, a reload's, a click's) opens it beside the list; the header follows the list's summary; the transcript renders a tail window (decisions 11, 13, 14, 20). A second commit gives the tests a loaded machine's time.

**Files:**
- Create: `web/src/components/SessionLink.test.tsx`.
- Modify: `web/src/components/Shell.tsx` (the summary and `awaitSummary` from the list; the session route keyed by id), `web/src/screens/Session.tsx` (`TAIL`, the window pinned by id, reveal before fetch) and its test, `web/src/store/useSessionItems.ts` (`loads`: a count of first pages, so the view can tell a resync from a prepend) and its test.
- Then: `web/vite.config.ts` (`testTimeout`), `web/src/test-setup.ts` (`asyncUtilTimeout`), `web/src/components/SessionList.test.tsx` (one debounce race).

- [ ] **Step 1: Write the tests**

Test files: `web/e2e/windowing.spec.ts`, `web/src/components/SessionLink.test.tsx`, `web/src/components/SessionList.test.tsx`, `web/src/screens/Session.test.tsx`, `web/src/store/useSessionItems.test.ts`.

In `web/e2e/windowing.spec.ts`, replace:

  ```ts
  //   first page. Its stream is held open and never sends.
  // - First render: from the page's response to two frames after the Nth item
  //   is in the DOM. Scrolling: 240 frames of 400 px each from the end, the
  //   gaps between frames and any long task, beside 240 frames standing still
  //   (the frame rate the browser keeps on this machine anyway). A row whose
  //   still frames are slower than about one frame (p95 > 20 ms) is marked
  //   not valid: the machine was busy.
  ```

with:

  ```ts
  //   first page. Its stream is held open and never sends. The session list
  //   beside it holds that one session; its stream is held open too.
  // - First render: from the page's response to two frames after the window
  //   is in the DOM: the newest `TAIL` items and, past them, "Load earlier".
  // - Reveal: "Load earlier" clicked until none is left, each click timed
  //   from the click to two frames after its rows are in; the cost of
  //   showing every one of the N items, `TAIL` at a time.
  // - Scrolling, once all N rows are shown: 240 frames of 400 px each from
  //   the end, the gaps between frames and any long task, beside 240 frames
  //   standing still (the frame rate the browser keeps on this machine
  //   anyway). A row whose still frames are slower than about one frame
  //   (p95 > 20 ms) is marked not valid: the machine was busy.
  ```

In `web/e2e/windowing.spec.ts`, replace:

  ```ts
  const SCROLL_STEP = 400
  const PIXEL = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='
  ```

with:

  ```ts
  const SCROLL_STEP = 400
  /** The session view's window (`TAIL` in src/screens/Session.tsx): the first
   *  render is checked against it, so a change there fails here. */
  const TAIL = 200
  const PIXEL = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='
  ```

In `web/e2e/windowing.spec.ts`, replace:

  ```ts
    const page_ = JSON.stringify({ items, older: false, epoch: 'e1', revision: items.length })
    await page.route(`${ORIGIN}/**`, async (route) => {
  ```

with:

  ```ts
    const page_ = JSON.stringify({ items, older: false, epoch: 'e1', revision: items.length })
    const list = { sessions: [{ ...detail, pending: undefined, question_waits: false }], epoch: 'l1', revision: 1, waiting: 0 }
    await page.route(`${ORIGIN}/**`, async (route) => {
  ```

In `web/e2e/windowing.spec.ts`, replace:

  ```ts
        if (pathname === `/api/stream/view/sessions/${ID}`) {
          held.push(route)
          return
        }
  ```

with:

  ```ts
        if (pathname === `/api/stream/view/sessions/${ID}` || pathname === '/api/stream/sessions') {
          held.push(route)
          return
        }
        if (pathname === '/api/view/sessions') return json(route, list)
  ```

In `web/e2e/windowing.spec.ts`, replace:

  ```ts
   *  the Nth item was painted, and every long task. */
  ```

with:

  ```ts
   *  the `want`th child of the transcript (its window, and "Load earlier"
   *  past `TAIL`) was painted, and every long task. */
  ```

In `web/e2e/windowing.spec.ts`, replace:

  ```ts
    requestAnimationFrame(tick)
  }
  ```

with:

  ```ts
    requestAnimationFrame(tick)
  }

  /** "Load earlier" clicked until none is left: how long each click took to
   *  paint its rows, and the long tasks meanwhile. */
  async function revealAll(page: Page) {
    return page.evaluate(async () => {
      const m = (window as unknown as { __m: { longtasks: [number, number][] } }).__m
      const frame = () => new Promise<number>((r) => requestAnimationFrame(r))
      const inner = document.querySelector('.transcript-inner') as HTMLElement
      const rows = () => inner.querySelectorAll(':scope > :not(.load-earlier)').length
      const clicks: number[] = []
      const from = performance.now()
      for (;;) {
        const button = inner.querySelector('.load-earlier button') as HTMLButtonElement | null
        if (!button) break
        const before = rows()
        const t = performance.now()
        button.click()
        // More rows, or a reveal the scroll handler made itself: either way
        // the count grows. 600 frames without one is a failure.
        for (let i = 0; rows() <= before; i++) {
          if (i === 600) throw new Error(`"Load earlier" showed nothing more after ${before} rows`)
          await frame()
        }
        await frame()
        await frame()
        clicks.push(performance.now() - t)
      }
      const to = performance.now()
      const long = m.longtasks.filter(([start]) => start >= from && start <= to)
      return {
        clicks: clicks.length,
        totalMs: to - from,
        meanClickMs: clicks.length ? clicks.reduce((a, b) => a + b, 0) / clicks.length : 0,
        maxClickMs: Math.max(0, ...clicks),
        longTasks: long.length,
        longestTaskMs: Math.max(0, ...long.map(([, d]) => d)),
        rows: rows(),
      }
    })
  }
  ```

In `web/e2e/windowing.spec.ts`, replace:

  ```ts
          await page.addInitScript(probe, n)
  ```

with:

  ```ts
          const shown = Math.min(n, TAIL)
          await page.addInitScript(probe, shown + (n > TAIL ? 1 : 0))
  ```

In `web/e2e/windowing.spec.ts`, replace:

  ```ts
          })
          const scroll = await scrollCost(page)
  ```

with:

  ```ts
          })
          // The window: the newest TAIL rows, and "Load earlier" past them.
          expect(await page.locator('.transcript-inner > :not(.load-earlier)').count()).toBe(shown)
          expect(await page.locator('.transcript-inner > .load-earlier').count()).toBe(n > TAIL ? 1 : 0)
          const reveal = await revealAll(page)
          // Every row shown, `TAIL` at a time (a reveal the scroll handler
          // made itself saves a click).
          expect(reveal.rows).toBe(n)
          expect(reveal.clicks).toBeLessThanOrEqual(Math.ceil((n - shown) / TAIL))
          const scroll = await scrollCost(page)
  ```

In `web/e2e/windowing.spec.ts`, replace:

  ```ts
          const row = { n, slowdown, load1: loadavg()[0], valid: scroll.still.p95 <= 20, ...first, scroll }
  ```

with:

  ```ts
          const row = { n, slowdown, load1: loadavg()[0], valid: scroll.still.p95 <= 20, ...first, reveal, scroll }
  ```

Create `web/src/components/SessionLink.test.tsx`:

  ```tsx
  // A session's address in the app (F-11, F-19): a push link or a reload
  // opens `/sessions/<id>` as an explicit selection, its header fed by the
  // list store, beside the list on a desktop and full screen on a phone.
  import { act, render, screen, waitFor, within } from '@testing-library/react'
  import userEvent from '@testing-library/user-event'
  import { readFileSync } from 'node:fs'
  import { join } from 'node:path'
  import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
  import App from '../App'
  import type { HatItem, SessionDetail } from '../generated/protocol'
  import type { Item, SessionSummary, SummaryPage } from '../generated/view'
  import { json, liveStream, type LiveStream } from '../test-stream'
  import { DESKTOP } from './Shell'

  const LIST = '/api/view/sessions'
  const LIST_STREAM = '/api/stream/sessions'
  const NARROW = '(max-width: 767px)'

  function summary(id: string, patch: Partial<SessionSummary> = {}): SessionSummary {
    return {
      session_id: id,
      host_id: 'h1',
      agent: 'claude',
      cwd: `/srv/work/${id}`,
      hat_id: 'hat-a',
      lifecycle: 'active',
      activity: 'idle',
      presumed_parked: false,
      created_at: '2026-10-01T00:00:00.000Z',
      last_event_at: '2026-10-02T10:00:00.000Z',
      question_waits: false,
      title: `Task ${id}`,
      ...patch,
    }
  }

  const hat = (id: string, name: string): HatItem => ({
    id,
    name,
    colour: '#112233',
    created_at: '2026-10-01T00:00:00Z',
    default_for_new_hosts: false,
    purging: false,
  })

  const ROWS = [
    summary('a', { last_event_at: '2026-10-02T11:00:00.000Z' }),
    summary('b', { hat_id: 'hat-b', last_event_at: '2026-10-02T10:00:00.000Z' }),
    summary('a-old', { last_event_at: '2026-10-02T09:00:00.000Z' }),
  ]

  const listPage = (sessions: SessionSummary[]): SummaryPage => ({ sessions, epoch: 'e1', revision: 1, waiting: 0 })

  function item(id: string, turn: string): Item {
    return { id, version: 1, ts: '2026-10-02T10:00:00.000Z', turn_id: turn, kind: 'message', text: id } as Item
  }

  /** The list's page for a query, as the server answers it: the chosen hat's
   *  rows only. */
  const hatPage = (params: URLSearchParams) => {
    const chosen = params.get('hat')
    return json(listPage(ROWS.filter((row) => chosen === null || row.hat_id === chosen)))
  }

  /** A session's detail as the server sends it: a summary's fields with
   *  `pending`, and no `question_waits` (the real detail has none). */
  function detailOf(id: string): SessionDetail {
    const fields: Record<string, unknown> = { ...summary(id, { title: `Detail ${id}` }), pending: [] }
    delete fields.question_waits
    return fields as unknown as SessionDetail
  }

  /** A server with the list and, for any session id, a one-row transcript
   *  (`<id> says hello`), a detail titled `Detail <id>` and a catalogue. */
  function server({ list = hatPage }: { list?: (params: URLSearchParams) => Response | Promise<Response> } = {}) {
    const calls: URL[] = []
    const listStreams: LiveStream[] = []
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(String(input), 'http://h')
      calls.push(url)
      const path = url.pathname
      if (path === '/api/capabilities') return json({ mode: 'full', features: [] })
      if (path === '/api/hats') return json([hat('hat-a', 'Work'), hat('hat-b', 'Home')])
      if (path === '/api/hosts') return json([{ host_id: 'h1', name: 'laptop' }])
      if (path === LIST) return list(url.searchParams)
      if (path === LIST_STREAM) {
        const live = liveStream()
        listStreams.push(live)
        return live.response
      }
      const view = /^\/api\/view\/sessions\/([^/]+)$/.exec(path)
      if (view) {
        const id = decodeURIComponent(view[1])
        return json({ items: [item(`${id} says hello`, 't1')], older: false, epoch: 'e1', revision: 1 })
      }
      if (/^\/api\/stream\/view\/sessions\/[^/]+$/.test(path)) return liveStream().response
      const catalog = /^\/api\/sessions\/([^/]+)\/catalog$/.exec(path)
      if (catalog) return json({ session_id: decodeURIComponent(catalog[1]), config_options: [], commands: [] })
      const detail = /^\/api\/sessions\/([^/]+)$/.exec(path)
      if (detail) return json(detailOf(decodeURIComponent(detail[1])))
      return json({ code: 'not_found', message: 'no' }, 404)
    })
    const of = (path: string) => calls.filter((u) => u.pathname === path)
    return { fetch: fetch as unknown as typeof globalThis.fetch, calls, listStreams, of }
  }

  /** Both of the app's media queries answer as one width would. */
  function width(desktop: boolean) {
    window.matchMedia = ((query: string) => ({
      matches: query === DESKTOP ? desktop : query === NARROW ? !desktop : false,
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    })) as unknown as typeof window.matchMedia
  }

  function at(path: string) {
    history.replaceState(null, '', path)
  }

  const main = () => document.querySelector('main.main') as HTMLElement
  /** The signed-in frame, once the capabilities came. */
  const rail = () => screen.findByRole('complementary', { name: 'Views' })
  /** The session's header title. */
  const title = () => within(main()).getByRole('heading', { level: 1 })
  /** Waits for the frame, then for the header's title to read `text`. */
  async function titled(text: string) {
    await rail()
    await waitFor(() => expect(title()).toHaveTextContent(text))
  }

  beforeEach(() => {
    localStorage.clear()
    width(true)
  })

  afterEach(() => {
    at('/')
    // @ts-expect-error jsdom has none; each test sets its own
    delete window.matchMedia
  })

  describe('the session’s header, from the list (F-4)', () => {
    it('follows the list stream’s upserts, and fetches no detail when the list holds the session', async () => {
      at('/sessions/a')
      const s = server()
      render(<App fetchImpl={s.fetch} />)
      await titled('Task a')
      await within(main()).findByText('a says hello')
      await waitFor(() => expect(s.listStreams).toHaveLength(1))
      act(() =>
        s.listStreams[0].event('session_upsert', summary('a', { title: 'Task a, renamed', activity: 'running', last_event_at: '2026-10-02T11:30:00.000Z' }), 'e1:2'),
      )
      await titled('Task a, renamed')
      expect(within(main()).getByText('Running')).toBeInTheDocument()
      // The view opened before the list's first page came: it waited for it.
      expect(s.of('/api/sessions/a')).toHaveLength(0)
    })

    it('fetches the detail only for a session the list does not hold', async () => {
      at('/sessions/elsewhere')
      const s = server()
      render(<App fetchImpl={s.fetch} />)
      await titled('Detail elsewhere')
      expect(within(main()).getByText('elsewhere says hello')).toBeInTheDocument()
      expect(s.of('/api/sessions/elsewhere')).toHaveLength(1)
      expect(s.of('/api/sessions/a')).toHaveLength(0)
    })

    it('keeps the header while the list starts over for a search', async () => {
      at('/sessions/a')
      // The search's page never comes.
      const s = server({ list: (params) => (params.get('q') ? new Promise<Response>(() => {}) : json(listPage(ROWS))) })
      render(<App fetchImpl={s.fetch} />)
      await titled('Task a')
      const views = await rail()
      await userEvent.type(within(views).getByRole('searchbox', { name: 'Search sessions' }), 'zzz')
      await waitFor(() => expect(s.of(LIST).some((u) => u.searchParams.get('q') === 'zzz')).toBe(true), { timeout: 3000 })
      expect(title()).toHaveTextContent('Task a')
      expect(s.of('/api/sessions/a')).toHaveLength(0)
    })

    it('fetches the detail when the list cannot be read', async () => {
      at('/sessions/a')
      const s = server({ list: () => json({ code: 'internal', message: 'down' }, 500) })
      render(<App fetchImpl={s.fetch} />)
      await titled('Detail a')
      expect(s.of('/api/sessions/a')).toHaveLength(1)
    })
  })

  describe('a session’s link (F-11, F-19)', () => {
    it('shows a session outside the chosen hat', async () => {
      localStorage.setItem('hennery.hat', 'hat-a')
      at('/sessions/b')
      const s = server()
      render(<App fetchImpl={s.fetch} />)
      // The hat's list does not hold it: its header is the detail's, as a
      // push link's is.
      await titled('Detail b')
      expect(await within(main()).findByText('b says hello')).toBeInTheDocument()
      const views = await rail()
      await waitFor(() => expect(within(views).queryAllByRole('link').filter((l) => l.classList.contains('sess'))).toHaveLength(2))
      expect(within(views).queryByText('Task b')).toBeNull()
      expect(location.pathname).toBe('/sessions/b')
      expect(s.of('/api/sessions/b')).toHaveLength(1)
      expect(s.of(LIST).every((u) => u.searchParams.get('hat') === 'hat-a')).toBe(true)
    })

    it('shows a session the loaded list does not hold', async () => {
      at('/sessions/not-in-the-list')
      const s = server()
      render(<App fetchImpl={s.fetch} />)
      await rail()
      expect(await within(main()).findByText('not-in-the-list says hello')).toBeInTheDocument()
      await waitFor(() => expect(s.listStreams).toHaveLength(1))
      expect(location.pathname).toBe('/sessions/not-in-the-list')
    })

    it('is never replaced by the desktop’s auto-select, even when a newer session comes', async () => {
      at('/sessions/a-old')
      const s = server()
      render(<App fetchImpl={s.fetch} />)
      await titled('Task a-old')
      await waitFor(() => expect(s.listStreams).toHaveLength(1))
      act(() => s.listStreams[0].event('session_upsert', summary('newest', { last_event_at: '2026-10-02T12:00:00.000Z' }), 'e1:2'))
      const views = await rail()
      await within(views).findByText('Task newest')
      expect(location.pathname).toBe('/sessions/a-old')
      expect(title()).toHaveTextContent('Task a-old')
    })

    it('is restored by a reload', async () => {
      at('/sessions/a')
      const s = server()
      const first = render(<App fetchImpl={s.fetch} />)
      await userEvent.click(await within(await rail()).findByText('Task a-old'))
      await within(main()).findByText('a-old says hello')
      first.unmount()
      // A reload: a new app at the same address.
      render(<App fetchImpl={server().fetch} />)
      await rail()
      expect(await within(main()).findByText('a-old says hello')).toBeInTheDocument()
      expect(location.pathname).toBe('/sessions/a-old')
    })

    it('shows the list in the rail and the session beside it on a desktop', async () => {
      at('/sessions/a')
      const s = server()
      render(<App fetchImpl={s.fetch} />)
      expect(await within(await rail()).findByRole('region', { name: 'Session list' })).toBeInTheDocument()
      expect(await within(main()).findByText('a says hello')).toBeInTheDocument()
      expect(within(main()).queryByRole('region', { name: 'Session list' })).toBeNull()
    })

    it('on a phone shows the session alone, with a way back to the list', async () => {
      width(false)
      at('/sessions/b')
      const s = server()
      render(<App fetchImpl={s.fetch} />)
      await rail()
      expect(await within(main()).findByText('b says hello')).toBeInTheDocument()
      // A phone's first page: 8 groups.
      expect(s.of('/api/view/sessions/b')[0].searchParams.get('limit')).toBe('8')
      expect(screen.queryByRole('region', { name: 'Session list' })).toBeNull()
      const back = within(main()).getByRole('link', { name: 'Back to sessions' })
      expect(back).toHaveAttribute('href', '/sessions')
      await userEvent.click(back)
      expect(location.pathname).toBe('/sessions')
      expect(await screen.findByRole('region', { name: 'Session list' })).toBeInTheDocument()
      // A phone stays on the list: no auto-select.
      expect(location.pathname).toBe('/sessions')
    })

    // jsdom applies no media query: the rules themselves. The way back is
    // hidden by default and shown under 768 px by a later rule.
    it('has its way back shown under 768 px', () => {
      const css = readFileSync(join(process.cwd(), 'src/index.css'), 'utf8').replace(/\s+/g, ' ')
      const hidden = css.indexOf('.back-btn { display:none; }')
      expect(hidden).toBeGreaterThan(-1)
      const mobile = /@media \(max-width:767px\) \{((?:[^{}]*\{[^}]*\})*)[^{}]*\}/g
      const shownAfter = [...css.matchAll(mobile)].some(
        (m) => (m.index ?? 0) > hidden && /\.back-btn \{[^}]*display:flex/.test(m[1]),
      )
      expect(shownAfter).toBe(true)
    })
  })
  ```

In `web/src/components/SessionList.test.tsx`, replace:

  ```tsx

    it('on a phone, /sessions stays the list', async () => {
  ```

with:

  ```tsx

    it('on a desktop, /sessions with no row to open says to pick one, not that the screen comes later', async () => {
      width(true)
      at('/sessions')
      const s = server()
      render(<App fetchImpl={s.fetch} />)
      expect(await screen.findByText('Pick a session from the list, or start a new one.')).toBeInTheDocument()
      expect(screen.queryByText('This screen arrives in a later part of the web UI.')).toBeNull()
      expect(location.pathname).toBe('/sessions')
    })

    it('on a phone, /sessions stays the list', async () => {
  ```

In `web/src/screens/Session.test.tsx`, replace:

  ```tsx
  import SessionView from './Session'
  ```

with:

  ```tsx
  import SessionView, { TAIL } from './Session'
  ```

In `web/src/screens/Session.test.tsx`, replace:

  ```tsx
    })

    it.each([
      [{ lifecycle: 'starting' }, 'Starting', 'wait'],
  ```

with:

  ```tsx
    })

    it('keeps a summary that went over the detail fetched before it, until the detail comes again', async () => {
      let release: (() => void) | undefined
      let calls = 0
      const s = server({
        pages: [() => json(page(rows(1, 't5')))],
        detail: () => {
          calls++
          if (calls === 1) return json(detail('codex', { title: 'From the detail' }))
          // The refetch, once the summary went: held until released.
          return new Promise<Response>((resolve) => {
            release = () => resolve(json(detail('codex', { title: 'From the detail, again' })))
          })
        },
      })
      const { rerender } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findByRole('heading', { name: 'From the detail' })
      const summary = { ...detail('codex'), pending: undefined, title: 'From the list' } as unknown as SessionSummary
      rerender(<SessionView id={ID} summary={summary} timing={FAST} />)
      await screen.findByRole('heading', { name: 'From the list' })
      // The summary goes (a new search's results) while the refetch is held.
      rerender(<SessionView id={ID} timing={FAST} />)
      await waitFor(() => expect(s.of(DETAIL)).toHaveLength(2))
      expect(screen.getByRole('heading', { name: 'From the list' })).toBeInTheDocument()
      act(() => release!())
      expect(await screen.findByRole('heading', { name: 'From the detail, again' })).toBeInTheDocument()
    })

    it.each([
      [{ lifecycle: 'starting' }, 'Starting', 'wait'],
  ```

In `web/src/screens/Session.test.tsx`, replace:

  ```tsx
      expect((container.querySelector('.transcript .steps') as HTMLDetailsElement).open).toBe(true)
    })
  })
  ```

with:

  ```tsx
      expect((container.querySelector('.transcript .steps') as HTMLDetailsElement).open).toBe(true)
    })
  })

  /** The transcript's rows, by the text of each message (its id). */
  const shown = () => Array.from(document.querySelectorAll('.transcript .bubble')).map((b) => b.textContent)

  describe('the tail window', () => {
    it(`renders at most the newest ${TAIL} items on open`, async () => {
      // The windowing measurement's threshold under load.
      expect(TAIL).toBe(200)
      // Markers: a cheap row, so the real window size stays quick under load.
      const markers = Array.from(
        { length: TAIL + 30 },
        (_, i) => ({ id: `m${i}`, version: 1, ts: '2026-10-02T10:00:00.000Z', turn_id: 't5', kind: 'marker', marker: 'host_back' }) as Item,
      )
      const s = server({ pages: [() => json(page(markers))] })
      const { container } = render(<SessionView id={ID} timing={FAST} />, { wrapper: s.wrapper })
      await waitFor(() => expect(container.querySelectorAll('.marker')).toHaveLength(TAIL))
      // Nothing older on the server, but rows are held: the way up stays.
      expect(screen.getByRole('button', { name: /Load earlier/ })).toBeInTheDocument()
      expect(scroller().scrollTop).toBe(TAIL * ROW - VIEW)
    })

    it('reveals held rows before fetching, and fetches only once none is held', async () => {
      const s = server({
        pages: [() => json(page(rows(11, 't5'), true))],
        older: { t5: () => json(page(rows(6, 't4'), false)) },
      })
      render(<SessionView id={ID} tail={4} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findByText('t5-10')
      expect(shown()).toEqual(['t5-7', 't5-8', 't5-9', 't5-10'])
      const earlier = () => fireEvent.click(screen.getByRole('button', { name: /Load earlier/ }))
      earlier()
      await screen.findByText('t5-3')
      expect(shown()).toHaveLength(8)
      earlier()
      await screen.findByText('t5-0')
      expect(shown()).toHaveLength(11)
      // Two reveals, no fetch.
      expect(s.of(PAGE)).toHaveLength(1)
      earlier()
      // Nothing held: now the turns before, and up to a window of them shown.
      await screen.findByText('t4-2')
      expect(s.of(PAGE).map((c) => new URL(c.path, 'http://h').searchParams.get('before_turn'))).toEqual([null, 't5'])
      expect(shown().slice(0, 5)).toEqual(['t4-2', 't4-3', 't4-4', 't4-5', 't5-0'])
      expect(screen.queryByText('t4-1')).toBeNull()
    })

    it('keeps the reader’s place when held rows are revealed, from the button or the top', async () => {
      const s = server({ pages: [() => json(page(rows(15, 't5')))] })
      render(<SessionView id={ID} tail={6} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findByText('t5-14')
      // Below the top: scrolling there reveals nothing by itself.
      scrollTo(200)
      expect(shown()).toHaveLength(6)
      fireEvent.click(screen.getByRole('button', { name: /Load earlier/ }))
      await screen.findByText('t5-3')
      expect(scroller().scrollTop).toBe(200 + 6 * ROW)
      scrollTo(0)
      await screen.findByText('t5-0')
      expect(scroller().scrollTop).toBe(3 * ROW)
      expect(s.of(PAGE)).toHaveLength(1)
    })

    it('pins its first row by id: a new item at the end leaves it, and shows while at the end', async () => {
      const s = server({ pages: [() => json(page(rows(6, 't5')))] })
      render(<SessionView id={ID} tail={4} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findByText('t5-5')
      await waitFor(() => expect(s.streams).toHaveLength(1))
      expect(shown()).toEqual(['t5-2', 't5-3', 't5-4', 't5-5'])
      act(() => s.streams[0].event('item', message('t5-new', 't5')))
      await screen.findByText('t5-new')
      expect(shown()).toEqual(['t5-2', 't5-3', 't5-4', 't5-5', 't5-new'])
      expect(scroller().scrollTop).toBe(5 * ROW - VIEW)
    })

    it('shows nothing new for an upsert of a held row, or a new row of a held turn', async () => {
      const s = server({ pages: [() => json(page([...rows(3, 't4'), ...rows(4, 't5')]))] })
      render(<SessionView id={ID} tail={4} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findByText('t5-3')
      await waitFor(() => expect(s.streams).toHaveLength(1))
      act(() => s.streams[0].event('item', { ...message('t4-0', 't4', 'changed'), version: 2 }))
      act(() => s.streams[0].event('item', message('t4-new', 't4')))
      act(() => s.streams[0].event('item', message('t5-new', 't5')))
      await screen.findByText('t5-new')
      expect(shown()).toEqual(['t5-0', 't5-1', 't5-2', 't5-3', 't5-new'])
      expect(screen.queryByText('changed')).toBeNull()
      expect(screen.queryByText('t4-new')).toBeNull()
    })

    it('goes back to the tail, and to the end, on a resync', async () => {
      // The same ids come back: the resync itself, not a missing row, resets.
      const s = server({ pages: [() => json(page(rows(10, 't5'))), () => json(page(rows(10, 't5'), false, 20))] })
      render(<SessionView id={ID} tail={5} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findByText('t5-9')
      await waitFor(() => expect(s.streams).toHaveLength(1))
      fireEvent.click(screen.getByRole('button', { name: /Load earlier/ }))
      await screen.findByText('t5-0')
      scrollTo(150)
      act(() => s.streams[0].event('resync_required', {}))
      await waitFor(() => expect(s.streams).toHaveLength(2))
      await waitFor(() => expect(shown()).toEqual(['t5-5', 't5-6', 't5-7', 't5-8', 't5-9']))
      expect(scroller().scrollTop).toBe(5 * ROW - VIEW)
    })

    it('goes to the end on a resync even when the new rows hold the old first one', async () => {
      const again = [...rows(2, 't4'), message('t5-5', 't5'), message('t5-6', 't5')]
      const s = server({ pages: [() => json(page(rows(10, 't5'))), () => json(page(again, false, 20))] })
      render(<SessionView id={ID} tail={5} timing={FAST} />, { wrapper: s.wrapper })
      await screen.findByText('t5-9')
      await waitFor(() => expect(s.streams).toHaveLength(1))
      scrollTo(150)
      act(() => s.streams[0].event('resync_required', {}))
      await waitFor(() => expect(shown()).toEqual(['t4-0', 't4-1', 't5-5', 't5-6']))
      // Not rows prepended above the reader: the end.
      expect(scroller().scrollTop).toBe(4 * ROW - VIEW)
    })
  })
  ```

In `web/src/store/useSessionItems.test.ts`, replace:

  ```ts
      await act(() => result.current.loadOlder())
      expect(s.of(PAGE_PATH)[1].path).toBe(`${PAGE_PATH}?before_turn=t3`)
      expect(result.current.items.map((i) => i.id)).toEqual(['a', 'c'])
      act(() => s.streams[0].send('event: resync_required\ndata: {}\n\n'))
      await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['x']))
  ```

with:

  ```ts
      expect(result.current.loads).toBe(1)
      await act(() => result.current.loadOlder())
      expect(s.of(PAGE_PATH)[1].path).toBe(`${PAGE_PATH}?before_turn=t3`)
      expect(result.current.items.map((i) => i.id)).toEqual(['a', 'c'])
      // An older page is not a first page.
      expect(result.current.loads).toBe(1)
      act(() => s.streams[0].send('event: resync_required\ndata: {}\n\n'))
      await waitFor(() => expect(result.current.items.map((i) => i.id)).toEqual(['x']))
      // The items were replaced: the view's window goes back to the tail.
      expect(result.current.loads).toBe(2)
  ```

- [ ] **Step 2: Run them, and see them fail**

Run: `nix develop -c sh -c 'cd web && pnpm vitest run src/components/SessionLink.test.tsx src/screens src/store/useSessionItems.test.ts'`
Expected: FAIL: the header is not fed by the list, every item renders, `TAIL` and `loads` do not exist.

- [ ] **Step 3: Links, the header and the window**

In `web/src/components/Shell.tsx`, replace:

  ```tsx
  import SessionScope, { WaitingBadge } from './SessionScope'
  ```

with:

  ```tsx
  import SessionScope, { WaitingBadge, useSessionScope } from './SessionScope'
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
    settings: Icon.Shield,
  }
  ```

with:

  ```tsx
    settings: Icon.Shield,
  }

  /** `/sessions/:id`: the session, its header fed by the list store's summary
   *  (kept current by the list stream) when the list holds it. While the
   *  list's first page is on its way the header waits for it; only a session
   *  the list does not hold fetches its detail. The address is the selection
   *  (F-11, F-19): shown whatever the hat, and whether or not the list holds it. */
  function SessionRoute({ id }: { id: string }) {
    const list = useSessionScope()?.list
    return <SessionView id={id} summary={list?.all.get(id)} awaitSummary={!!list && list.loading && !list.error} />
  }
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
              <SessionView key={route.id} id={route.id ?? ''} />
  ```

with:

  ```tsx
              <SessionRoute key={route.id} id={route.id ?? ''} />
            ) : route.name === 'sessions' ? (
              // A desktop at /sessions with no row to open: an empty hat, a
              // search with no match, or before the first page has come.
              <Placeholder title={title} text="Pick a session from the list, or start a new one." />
  ```

In `web/src/screens/Session.tsx`, replace:

  ```tsx
  //   it, else the session's detail, fetched once.
  // - The transcript opens at its end and stays there while the reader is at
  //   the end. "Load earlier" (or scrolling to the top) prepends the turns
  //   before, keeping what the reader sees where it was.
  ```

with:

  ```tsx
  //   it, else the session's detail, fetched once. While the list's first page
  //   is on its way, the header waits for it rather than fetching the detail.
  // - The transcript opens at its end and stays there while the reader is at
  //   the end. It opens with at most the newest `TAIL` items, and grows at the
  //   end while the reader is there: "Load earlier" (or
  //   scrolling to the top) first shows `TAIL` more of the items held, and
  //   only when none is held fetches the turns before. Either way what the
  //   reader sees stays where it was.
  // - The window's first row is pinned by item id: an upsert, or an item
  //   joining a turn above it or at the end, never moves it. A resync (the
  //   items replaced) goes back to the tail, and to the end.
  ```

In `web/src/screens/Session.tsx`, replace:

  ```tsx
  const TOP_PX = 120

  ```

with:

  ```tsx
  const TOP_PX = 120
  /** The most items the transcript renders until the reader asks for more
   *  (the windowing measurement: a first render under load stays within
   *  budget up to about 200 items). */
  export const TAIL = 200

  ```

In `web/src/screens/Session.tsx`, replace:

  ```tsx
    summary?: SessionSummary
    /** The item store's timing (tests). */
  ```

with:

  ```tsx
    summary?: SessionSummary
    /** The list's first page is on its way: wait for it before fetching the
     *  session's detail. */
    awaitSummary?: boolean
    /** The transcript's window (tests); `TAIL` by default. */
    tail?: number
    /** The item store's timing (tests). */
  ```

In `web/src/screens/Session.tsx`, replace:

  ```tsx
  /** The session's detail, once, unless the caller has its summary. */
  function useHeaderInfo(id: string, summary: SessionSummary | undefined): HeaderInfo | undefined {
    const client = useClient()
    const [detail, setDetail] = useState<HeaderInfo | undefined>(undefined)
    const needed = summary === undefined
  ```

with:

  ```tsx
  /** The session's detail, once, unless the caller has its summary (or is
   *  about to). A summary seen once stays shown when the list starts over
   *  (a new search, a filter) until the detail or the summary comes back.
   *  With no summary given, the newer of the detail and the last summary
   *  seen is shown: a detail fetched before a summary came never wins over
   *  it once that summary goes. */
  function useHeaderInfo(id: string, summary: SessionSummary | undefined, awaitSummary: boolean): HeaderInfo | undefined {
    const client = useClient()
    // When each source came, in one order.
    const clock = useRef(0)
    const [detail, setDetail] = useState<{ info: HeaderInfo; at: number } | undefined>(undefined)
    const last = useRef<{ info: SessionSummary; at: number } | undefined>(undefined)
    if (summary && last.current?.info !== summary) last.current = { info: summary, at: ++clock.current }
    const needed = summary === undefined && !awaitSummary
  ```

In `web/src/screens/Session.tsx`, replace:

  ```tsx
        (d) => live && setDetail({ ...d, question_waits: Array.isArray(d.pending) && d.pending.length > 0 }),
  ```

with:

  ```tsx
        (d) =>
          live &&
          setDetail({ info: { ...d, question_waits: Array.isArray(d.pending) && d.pending.length > 0 }, at: ++clock.current }),
  ```

In `web/src/screens/Session.tsx`, replace:

  ```tsx
    return summary ?? detail
  ```

with:

  ```tsx
    if (summary) return summary
    const held = last.current
    if (detail && (!held || detail.at > held.at)) return detail.info
    return held?.info ?? detail?.info
  ```

In `web/src/screens/Session.tsx`, replace:

  ```tsx
  export default function SessionView({ id, summary, timing, env: seams }: Props) {
    const s = useSessionItems(id, timing)
    const info = useHeaderInfo(id, summary)
  ```

with:

  ```tsx
  /** Where the transcript's window starts: the pinned item's index, or the
   *  newest `size` items when nothing is pinned for these `loads`. */
  function windowStart(items: readonly Item[], pin: string | null, size: number): number {
    const tail = Math.max(0, items.length - size)
    if (pin === null) return tail
    const at = items.findIndex((item) => item.id === pin)
    return at < 0 ? tail : at
  }

  /** The transcript's window over `items`: its first row pinned by id, `TAIL`
   *  rows at first, more revealed on demand; back to the tail when `loads`
   *  changes (a resync replaced the items). */
  function useTailWindow(items: Item[], loads: number, size: number) {
    const [pinned, setPinned] = useState<{ id: string; loads: number } | null>(null)
    const pin = pinned && pinned.loads === loads ? pinned.id : null
    const start = windowStart(items, pin, size)
    const visible = useMemo(() => (start === 0 ? items : items.slice(start)), [items, start])
    const firstHeld = useRef<string | undefined>(undefined)

    useLayoutEffect(() => {
      const was = firstHeld.current
      firstHeld.current = items[0]?.id
      if (items.length === 0) return
      // Older turns came in above a window that showed everything held: the
      // reader asked for them, so up to `size` of them are shown.
      const prepended = was !== undefined && was !== items[0].id && pin === was && items.some((i) => i.id === was)
      const next = prepended ? Math.max(0, start - size) : start
      if (pin === null || prepended || items[start]?.id !== pin) setPinned({ id: items[next].id, loads })
    }, [items, loads, pin, start, size])

    /** Show `size` more of the held items; `false` when none is held. */
    const reveal = (): boolean => {
      if (start === 0) return false
      setPinned({ id: items[Math.max(0, start - size)].id, loads })
      return true
    }
    return { visible, held: start, reveal }
  }

  export default function SessionView({ id, summary, awaitSummary = false, tail = TAIL, timing, env: seams }: Props) {
    const s = useSessionItems(id, timing)
    const info = useHeaderInfo(id, summary, awaitSummary)
    const win = useTailWindow(s.items, s.loads, tail)
  ```

In `web/src/screens/Session.tsx`, replace:

  ```tsx
        <Scroller items={s.items} older={s.older} loadingOlder={s.loadingOlder} loadOlder={s.loadOlder}>
          {s.loading ? <p className="transcript-empty">Loading…</p> : <Transcript items={s.items} env={env} />}
  ```

with:

  ```tsx
        <Scroller
          items={win.visible}
          loads={s.loads}
          earlier={win.held > 0 || s.older}
          loadingOlder={s.loadingOlder}
          loadEarlier={() => {
            if (!win.reveal() && s.older && !s.loadingOlder) void s.loadOlder()
          }}
        >
          {s.loading ? <p className="transcript-empty">Loading…</p> : <Transcript items={win.visible} env={env} />}
  ```

In `web/src/screens/Session.tsx`, replace:

  ```tsx
    items: Item[]
    older: boolean
    loadingOlder: boolean
    loadOlder: () => Promise<void>
  ```

with:

  ```tsx
    /** The rows rendered. */
    items: Item[]
    /** The item store's first pages taken: a new one (a resync) goes to the end. */
    loads: number
    /** Rows are held above the window, or turns before the first loaded. */
    earlier: boolean
    loadingOlder: boolean
    loadEarlier: () => void
  ```

In `web/src/screens/Session.tsx`, replace:

  ```tsx
   *  the reader is there, and keeps the reader's place when older turns are
   *  prepended. */
  function Scroller({ items, older, loadingOlder, loadOlder, children }: ScrollerProps) {
    const ref = useRef<HTMLDivElement>(null)
    const atEnd = useRef(true)
    const before = useRef<{ first?: string; height: number }>({ height: 0 })
  ```

with:

  ```tsx
   *  the reader is there, and keeps the reader's place when rows go in above
   *  (held rows revealed, or older turns prepended). */
  function Scroller({ items, loads, earlier, loadingOlder, loadEarlier, children }: ScrollerProps) {
    const ref = useRef<HTMLDivElement>(null)
    const atEnd = useRef(true)
    const before = useRef<{ first?: string; height: number; loads: number }>({ height: 0, loads })
  ```

In `web/src/screens/Session.tsx`, replace:

  ```tsx
      const prepended = was.first !== undefined && first !== was.first && items.some((i) => i.id === was.first)
      if (prepended) el.scrollTop += el.scrollHeight - was.height
      else if (atEnd.current) el.scrollTop = el.scrollHeight
      before.current = { first, height: el.scrollHeight }
    }, [items])
  ```

with:

  ```tsx
      // A resync replaced the items: the window went back to the tail.
      if (loads !== was.loads) atEnd.current = true
      const prepended =
        loads === was.loads && was.first !== undefined && first !== was.first && items.some((i) => i.id === was.first)
      if (prepended) el.scrollTop += el.scrollHeight - was.height
      else if (atEnd.current) el.scrollTop = el.scrollHeight
      before.current = { first, height: el.scrollHeight, loads }
    }, [items, loads])
  ```

In `web/src/screens/Session.tsx`, replace:

  ```tsx
      if (el.scrollTop < TOP_PX && older && !loadingOlder) void loadOlder()
  ```

with:

  ```tsx
      if (el.scrollTop < TOP_PX && earlier && !loadingOlder) loadEarlier()
  ```

In `web/src/screens/Session.tsx`, replace:

  ```tsx
          {older && (
            <div className="load-earlier">
              <button type="button" className="btn btn-ghost btn-sm" disabled={loadingOlder} onClick={() => void loadOlder()}>
  ```

with:

  ```tsx
          {earlier && (
            <div className="load-earlier">
              <button type="button" className="btn btn-ghost btn-sm" disabled={loadingOlder} onClick={loadEarlier}>
  ```

In `web/src/store/useSessionItems.ts`, replace:

  ```ts
    catalog: SessionCatalog | null
  }
  ```

with:

  ```ts
    catalog: SessionCatalog | null
    /** First pages taken: bumped by the first load and by every resync (the
     *  items were replaced). */
    loads: number
  }
  ```

In `web/src/store/useSessionItems.ts`, replace:

  ```ts
    catalog: null,
  }
  ```

with:

  ```ts
    catalog: null,
    loads: 0,
  }
  ```

In `web/src/store/useSessionItems.ts`, replace:

  ```ts
          resynced: resync || this.snapshot.resynced,
        })
  ```

with:

  ```ts
          resynced: resync || this.snapshot.resynced,
          loads: this.snapshot.loads + 1,
        })
  ```

- [ ] **Step 4: Run the checks**

Run: `nix develop -c sh -c 'cd web && pnpm typecheck && pnpm test && pnpm build'`
Expected: PASS, 749 tests. The initial JS is 199 kB gzip (647 kB raw), one chunk.

- [ ] **Step 5: Revert-probes** (each must fail the test named; restore after each)

33 probes, run by script, all fail as they should:
- **Links and the header** (12): the summary from the list; the header waits for the list, and stops waiting on its error; `awaitSummary` honoured; the detail only for a session the list lacks; the last summary kept while the list starts over; the shell mounts the session route; desktop auto-select only on `/sessions` (a link is never replaced); the view on the session route (a reload restores it); the list in the rail on a desktop and not beside the session on a phone; the back control shown under 768 px (CSS text).
- **The window** (21): `TAIL` is 200 and is the default; the transcript and the scroller use the window; a reveal before a fetch, a fetch only when nothing is held; a reveal steps by the window's size; a fetched page revealed, one window at a time; the button while rows are held or older turns exist; reaching the top reveals; the window pinned by id; a resync goes back to the tail and to the end, and is no prepend; the anchor kept on a reveal; sticking to the end; a hidden upsert stays hidden; `loads` counted (store and view).


**Re-run on the final code:** 33 of 33 fail as they should. The task review's fixes added two, and one in Chromium: a summary newer than the detail keeps the header; the list stub filters by hat, so a push link outside the hat takes the detail; the measurement's window size.

- [ ] **Step 6: Commit**

```bash
git add web/src
git commit -m "feat(web): a session's link opens it beside the list, its header fed by the list, its transcript opening at the newest 200 items"
```

- [ ] **Step 7: Give the tests a loaded machine's time**

Four parallel copies of the whole suite, 9 rounds at a load of 68–92 on 18 cores, failed 4b's tests by vitest's 5 s timeout while they imported and rendered the whole app: **Setup "takes the token from the fragment and out of the address bar at once"** (9 times) and **StepUpDialog "confirms with the password, then the request goes once more, with the form as it was"** (4 times; once in a lighter run too); also Setup "sends the token, password, this origin and the hat name, then offers a passkey" (4) and, once, Login "offers the passkey first, signs in with it, and returns to next" (Testing Library's 1 s `findBy`).

The fix is the time, not the tests: 20 s per test and 5 s per async wait. One search test waited for a debounce that a loaded machine overran mid-word; it now waits for the whole word's request.

In `web/src/components/SessionList.test.tsx`, replace:

  ```tsx
      await userEvent.type(within(region).getByRole('searchbox', { name: 'Search sessions' }), 'fix')
      await waitFor(() => expect(names(region)).toEqual(['found-closed']))
  ```

with:

  ```tsx
      await userEvent.type(within(region).getByRole('searchbox', { name: 'Search sessions' }), 'fix')
      // A loaded machine may pause past the debounce mid-word: a search for
      // `f` can go first, with the same rows. Wait for the whole word's.
      await waitFor(() => expect(s.of(LIST).at(-1)!.searchParams.get('q')).toBe('fix'))
      await waitFor(() => expect(names(region)).toEqual(['found-closed']))
  ```

In `web/src/test-setup.ts`, replace:

  ```ts
  import '@testing-library/jest-dom/vitest'
  ```

with:

  ```ts
  import '@testing-library/jest-dom/vitest'
  import { configure } from '@testing-library/react'

  // `findBy*` and `waitFor` give up after 1 s by default: on a loaded machine
  // a whole app's first render can take longer. Kept well below the test
  // timeout (vite.config.ts), so a wait that never ends fails with its own
  // message, not as a timed-out test.
  configure({ asyncUtilTimeout: 5000 })
  ```

In `web/vite.config.ts`, replace:

  ```ts
    test: { environment: 'jsdom', globals: true, setupFiles: './src/test-setup.ts', include: ['src/**/*.test.{ts,tsx}'] },
  ```

with:

  ```ts
    test: {
      environment: 'jsdom',
      globals: true,
      setupFiles: './src/test-setup.ts',
      include: ['src/**/*.test.{ts,tsx}'],
      // A loaded machine (CI's 2–4 vCPUs, or parallel runs) takes seconds to
      // import a test's module graph and render a whole app: the 5 s default
      // timed out tests that pass. A real hang still fails, only later.
      testTimeout: 20000,
    },
  ```

**Revert-probe:** with the fix removed, the same 9 × 4 load failed Setup "takes the token from the fragment…" once; with it, 9 × 4 at a load of 20–84 failed none of them. A timing failure is not reproducible on demand: the probe shows the failure exists without the fix, not its rate.

Run: `nix develop -c sh -c 'cd web && pnpm test'`
Expected: PASS, 749 tests.

```bash
git add web/vite.config.ts web/src/test-setup.ts web/src/components/SessionList.test.tsx
git commit -m "test(web): give tests a loaded machine's time: 20 s per test, 5 s per wait"
```

- [ ] **Step 8: Guard the tests' text locators**

Plan 4d-i's browser check found "Saved." by `getByText('Saved.')`, which in Playwright is a case-insensitive substring: it also matched a hint ending "…as saved." and turned `main` red (#110). A Vitest test now reads every browser check and every unit test and fails on a Playwright `getByText`/`getByLabel`/`getByTitle`/`getByPlaceholder` with a string and no `{ exact: true }`, on an unanchored regex there, and on a unit test's `{ exact: false }` or short unanchored regex. It fixes what it found in 4b's and 4d-i's checks and in this plan's tests.

In `web/e2e/manage.spec.ts`, replace:

  ```ts
        const command = (await page.getByLabel('Pairing command').textContent())!
        expect(command).toMatch(new RegExp(`^hennery host join ${collector.origin} [0-9A-Z]{4}-[0-9A-Z]{4}$`))
        const code = command.split(' ').at(-1)!
        expect(await host.join(command, ['--name', 'e2e host', '--no-runtime'])).toBe(0)
        await expect(page.getByText('Paired: e2e host')).toBeVisible({ timeout: 15_000 })
  ```

with:

  ```ts
        const command = (await page.getByLabel('Pairing command', { exact: true }).textContent())!
        expect(command).toMatch(new RegExp(`^hennery host join ${collector.origin} [0-9A-Z]{4}-[0-9A-Z]{4}$`))
        const code = command.split(' ').at(-1)!
        expect(await host.join(command, ['--name', 'e2e host', '--no-runtime'])).toBe(0)
        await expect(page.getByText('Paired: e2e host', { exact: true })).toBeVisible({ timeout: 15_000 })
  ```

In `web/e2e/manage.spec.ts`, replace:

  ```ts
        const tester = page.getByLabel('Test a path')
        await tester.fill(join(project, 'work'))
        const resolution = page.getByLabel('Resolution')
  ```

with:

  ```ts
        const tester = page.getByLabel('Test a path', { exact: true })
        await tester.fill(join(project, 'work'))
        const resolution = page.getByLabel('Resolution', { exact: true })
  ```

In `web/e2e/manage.spec.ts`, replace:

  ```ts
        await page.getByLabel('Path 1').fill(join(project, 'work'))
        await page.getByLabel('Hat 1').selectOption({ label: 'Work' })
  ```

with:

  ```ts
        await page.getByLabel('Path 1', { exact: true }).fill(join(project, 'work'))
        // The label holds the select, so its name is "Hat 1" and the options'
        // names: anchored, and never "Hat 10".
        await page.getByLabel(/^Hat 1(?!\d)/).selectOption({ label: 'Work' })
  ```

In `web/e2e/manage.spec.ts`, replace:

  ```ts
        await expect(stepUp.getByLabel('Your password')).toBeFocused()
        // The confirmation itself, not its buttons: they are disabled while
        // its action waits, and a disabled button takes no focus anyway.
        await confirm.evaluate((d: HTMLElement) => d.focus())
        await expect(stepUp.getByLabel('Your password')).toBeFocused()
  ```

with:

  ```ts
        await expect(stepUp.getByLabel('Your password', { exact: true })).toBeFocused()
        // The confirmation itself, not its buttons: they are disabled while
        // its action waits, and a disabled button takes no focus anyway.
        await confirm.evaluate((d: HTMLElement) => d.focus())
        await expect(stepUp.getByLabel('Your password', { exact: true })).toBeFocused()
  ```

In `web/e2e/manage.spec.ts`, replace:

  ```ts
        await stepUp.getByLabel('Your password').focus()
        await stepUp.getByLabel('Your password').fill(PASSWORD)
  ```

with:

  ```ts
        await stepUp.getByLabel('Your password', { exact: true }).focus()
        await stepUp.getByLabel('Your password', { exact: true }).fill(PASSWORD)
  ```

In `web/e2e/shell.spec.ts`, replace:

  ```ts
    await expect(page.getByLabel('Public URL')).toHaveValue(collector.origin)
    await page.getByLabel(/^Password at least/).fill(PASSWORD)
    await page.getByLabel('Password again').fill(PASSWORD)
  ```

with:

  ```ts
    await expect(page.getByLabel(/^Public URL/)).toHaveValue(collector.origin)
    await page.getByLabel(/^Password at least/).fill(PASSWORD)
    await page.getByLabel('Password again', { exact: true }).fill(PASSWORD)
  ```

In `web/e2e/shell.spec.ts`, replace:

  ```ts
    await page.getByLabel('Passkey name').fill('Test authenticator')
    await page.getByRole('button', { name: 'Add a passkey' }).click()
    await expect(page.getByText('Your passkey is added')).toBeVisible()
  ```

with:

  ```ts
    await page.getByLabel('Passkey name', { exact: true }).fill('Test authenticator')
    await page.getByRole('button', { name: 'Add a passkey' }).click()
    await expect(page.getByText(/^Your passkey is added/)).toBeVisible()
  ```

In `web/e2e/shell.spec.ts`, replace:

  ```ts
    await page.getByLabel('Password').fill(PASSWORD)
  ```

with:

  ```ts
    await page.getByLabel('Password', { exact: true }).fill(PASSWORD)
  ```

In `web/src/components/StepList.test.tsx`, replace:

  ```tsx
      expect(screen.getByText(/1\s*\/\s*2/)).toBeTruthy()
  ```

with:

  ```tsx
      expect(screen.getByText(/^1\s*\/\s*2$/)).toBeTruthy()
  ```

In `web/src/components/StepList.test.tsx`, replace:

  ```tsx
      expect(screen.getByText(/cut or left out/)).toBeTruthy()
  ```

with:

  ```tsx
      expect(screen.getByText(/cut or left out\.$/)).toBeTruthy()
  ```

In `web/src/components/items/items.test.tsx`, replace:

  ```tsx
      expect(screen.getByText(/This prompt was cut/)).toBeInTheDocument()
  ```

with:

  ```tsx
      expect(screen.getByText('This prompt was cut.')).toBeInTheDocument()
  ```

In `web/src/components/items/items.test.tsx`, replace:

  ```tsx
      expect(screen.getByText('This turn holds more than is shown here', { exact: false })).toBeInTheDocument()
  ```

with:

  ```tsx
      expect(screen.getByText(/^This turn holds more than is shown here/)).toBeInTheDocument()
  ```

In `web/src/components/items/items.test.tsx`, replace:

  ```tsx
      expect(screen.getByText(/This update was cut/)).toBeInTheDocument()
  ```

with:

  ```tsx
      expect(screen.getByText('This update was cut.')).toBeInTheDocument()
  ```

Create `web/src/test/text-locators.test.ts`:

  ```ts
  // The loose-text-locator guard: Playwright's `getByText('x')` is a
  // case-insensitive SUBSTRING match by default, so `getByText('Saved.')` also
  // matched "...rules as saved." and turned main red only when that other text
  // happened to be on screen (#109 -> #110). Testing Library's matchers are
  // exact by default, so `web/src` tests get only the narrower checks.
  //
  // The scanners are in `./textLocators.ts`. The snippet tests below give
  // each verdict its own case; the last block runs the scanners over the
  // real `web/e2e` and `web/src` trees, which is what makes `pnpm test` fail
  // on a new loose locator.
  import { readFileSync, readdirSync, statSync } from 'node:fs'
  import { join, relative } from 'node:path'
  import { describe, expect, it } from 'vitest'
  import { scanPlaywrightLocators, scanTestingLibraryLocators } from './textLocators'

  // Vitest runs from `web/`.
  const ROOT = process.cwd()

  function filesUnder(dir: string, test: RegExp): string[] {
    return readdirSync(dir).flatMap((name) => {
      const path = join(dir, name)
      if (statSync(path).isDirectory()) return filesUnder(path, test)
      return test.test(name) ? [path] : []
    })
  }

  describe('scanPlaywrightLocators (web/e2e/**/*.ts)', () => {
    it('flags a string literal with no { exact: true }', () => {
      const v = scanPlaywrightLocators('f.ts', "await expect(page.getByText('Saved.')).toBeVisible()")
      expect(v).toHaveLength(1)
      expect(v[0].line).toBe(1)
    })

    it('passes a string literal with { exact: true }', () => {
      expect(scanPlaywrightLocators('f.ts', "await expect(page.getByText('Saved.', { exact: true })).toBeVisible()")).toEqual([])
    })

    it('flags an unanchored regex literal', () => {
      const v = scanPlaywrightLocators('f.ts', "await page.getByLabel(/Password at least/).fill(x)")
      expect(v).toHaveLength(1)
    })

    it('passes a regex literal anchored with ^', () => {
      expect(scanPlaywrightLocators('f.ts', "await page.getByLabel(/^Password at least/).fill(x)")).toEqual([])
    })

    it('passes a regex literal anchored with $', () => {
      expect(scanPlaywrightLocators('f.ts', 'await expect(page.getByText(/Paired: e2e host$/)).toBeVisible()')).toEqual([])
    })

    it('flags a regex literal whose only $ is escaped (a dollar sign, not the end)', () => {
      expect(scanPlaywrightLocators('f.ts', String.raw`await page.getByText(/costs \$/).click()`)).toHaveLength(1)
    })

    it('passes a regex literal ending in an escaped backslash and then $ (the end)', () => {
      expect(scanPlaywrightLocators('f.ts', String.raw`await page.getByText(/C:\\$/).click()`)).toEqual([])
    })

    it('flags a template literal with no { exact: true }, plain or with a ${} hole', () => {
      expect(scanPlaywrightLocators('f.ts', 'await page.getByText(`Saved.`).click()')).toHaveLength(1)
      expect(scanPlaywrightLocators('f.ts', 'await page.getByLabel(`Hat ${n}`).click()')).toHaveLength(1)
    })

    it('passes a template literal with { exact: true }', () => {
      expect(scanPlaywrightLocators('f.ts', 'await page.getByLabel(`Hat ${n}`, { exact: true }).click()')).toEqual([])
    })

    it('does not judge a locator built from a variable', () => {
      expect(scanPlaywrightLocators('f.ts', 'await page.getByText(label).click()')).toEqual([])
    })
  })

  describe('scanTestingLibraryLocators (web/src/**/*.test.ts(x))', () => {
    it('flags an explicit { exact: false }', () => {
      const v = scanTestingLibraryLocators('f.tsx', "screen.getByText('This turn holds more than is shown here', { exact: false })")
      expect(v).toHaveLength(1)
    })

    it('passes a plain string literal (Testing Library matches exactly by default)', () => {
      expect(scanTestingLibraryLocators('f.tsx', "screen.getByText('Saved.')")).toEqual([])
    })

    it('flags a short, unanchored regex literal', () => {
      const v = scanTestingLibraryLocators('f.tsx', 'screen.getByText(/cut or left out/)')
      expect(v).toHaveLength(1)
    })

    it('passes a short regex literal anchored with ^', () => {
      expect(scanTestingLibraryLocators('f.tsx', 'screen.getByLabelText(/^Public URL/)')).toEqual([])
    })

    it('passes a short regex literal anchored with $', () => {
      expect(scanTestingLibraryLocators('f.tsx', 'screen.getByText(/e2e host$/)')).toEqual([])
    })

    it('flags a short regex literal whose only $ is escaped', () => {
      expect(scanTestingLibraryLocators('f.tsx', String.raw`screen.getByText(/costs \$/)`)).toHaveLength(1)
    })

    it('passes a long unanchored regex literal (20 chars of pattern or more)', () => {
      expect(scanTestingLibraryLocators('f.tsx', 'screen.getByText(/This field cannot be filled in here/)')).toEqual([])
      // Exactly 20 characters of pattern is long enough.
      expect(scanTestingLibraryLocators('f.tsx', 'screen.getByText(/this field is filled/)')).toEqual([])
    })
  })

  describe('the web test sources', () => {
    // This file's own snippets above are deliberately-violating fixtures (data,
    // not real locators): it is the guard's test, not something it guards.
    const SELF = join(ROOT, 'src', 'test', 'text-locators.test.ts')
    const e2eFiles = filesUnder(join(ROOT, 'e2e'), /\.ts$/)
    const srcTestFiles = filesUnder(join(ROOT, 'src'), /\.test\.tsx?$/).filter((f) => f !== SELF)

    it('are all read', () => {
      expect(e2eFiles.length).toBeGreaterThan(4)
      expect(srcTestFiles.length).toBeGreaterThan(30)
    })

    it('use exact or anchored text locators in web/e2e', () => {
      const violations = e2eFiles.flatMap((f) => scanPlaywrightLocators(relative(ROOT, f), readFileSync(f, 'utf8')))
      expect(violations.map((v) => `${v.file}:${v.line}: ${v.snippet}`)).toEqual([])
    })

    it('never opt into substring text matching in web/src tests', () => {
      const violations = srcTestFiles.flatMap((f) => scanTestingLibraryLocators(relative(ROOT, f), readFileSync(f, 'utf8')))
      expect(violations.map((v) => `${v.file}:${v.line}: ${v.snippet}`)).toEqual([])
    })
  })
  ```

Create `web/src/test/textLocators.ts`:

  ```ts
  // A cheap guard against loose text locators in the web tests.
  //
  // The lesson it keeps: Playwright's `page.getByText('Saved.')` is a
  // case-insensitive SUBSTRING match by default. It also matched "...rules as
  // saved." elsewhere on the page, and turned main red only when that other
  // text happened to be on screen (#109 -> #110). A text locator is therefore
  // a full string with `{ exact: true }`, an anchored regex, or a role.
  //
  // These are pure string scanners (no filesystem access), so each verdict
  // has its own snippet test; `text-locators.test.ts`, beside this file,
  // also points them at the real `web/e2e` and `web/src` trees.
  //
  // What they assume (regex and text scanning over type-checked TypeScript,
  // not a JS parser):
  //   - brackets are balanced outside strings, and a template literal's
  //     `${...}` holds no unbalanced bracket and no nested template literal;
  //   - a bare `/` outside a string always starts a regex literal: true for
  //     call arguments here, which never divide numbers;
  //   - only the first argument is judged, and only the other arguments' raw
  //     text is searched for `exact: true` / `exact: false`.
  //
  // Known limits (they let a loose locator through; none is a false alarm):
  //   - A first argument that is not a bare literal is not judged: a variable
  //     (`getByText(text)`), a matcher function, a wrapped or cast literal
  //     (`('x')`, `'x' as const`).
  //   - A `//` comment holding an apostrophe or a quote inside a call's
  //     arguments makes the scanner read on as if in a string. It then throws
  //     (unbalanced; the test fails loudly) or splits the arguments wrongly,
  //     with no message.
  //   - Playwright's `getByRole(role, { name: '<string>' })` is ALSO a
  //     case-insensitive substring match unless `exact: true` is passed, and
  //     it is NOT scanned yet (25 such calls in web/e2e when this was
  //     written, some loose, e.g. `{ name: 'Create' }`). Nor are
  //     `filter({ hasText: '...' })`, `getByAltText`, or `locator('text=...')`.
  //     Scanning them is a follow-up.

  export interface Violation {
    file: string
    line: number
    snippet: string
  }

  function lineOf(text: string, index: number): number {
    let line = 1
    for (let i = 0; i < index; i++) if (text[i] === '\n') line++
    return line
  }

  // Splits the arguments of the call whose `(` is at `openParenIdx` into their
  // raw (untrimmed-of-inner-content, trimmed-of-surrounding-space) top-level
  // source text, skipping over nested brackets, quoted strings and regex
  // literals so a comma or bracket inside one of those never splits early.
  function splitTopLevelArgs(text: string, openParenIdx: number): { args: string[]; endIdx: number } {
    let i = openParenIdx + 1
    let depth = 0
    let start = i
    const args: string[] = []
    while (i < text.length) {
      const c = text[i]
      if (c === "'" || c === '"' || c === '`') {
        const quote = c
        i++
        while (i < text.length && text[i] !== quote) {
          if (text[i] === '\\') i++
          i++
        }
        i++
        continue
      }
      if (c === '/') {
        // See the file-level comment: treated as a regex literal start.
        i++
        let inClass = false
        while (i < text.length && (inClass || text[i] !== '/')) {
          if (text[i] === '\\') i++
          else if (text[i] === '[') inClass = true
          else if (text[i] === ']') inClass = false
          i++
        }
        i++ // closing '/'
        while (i < text.length && /[a-z]/i.test(text[i])) i++ // flags
        continue
      }
      if (c === '(' || c === '{' || c === '[') {
        depth++
        i++
        continue
      }
      if (c === ')' && depth === 0) {
        args.push(text.slice(start, i).trim())
        return { args, endIdx: i + 1 }
      }
      if (c === ')' || c === '}' || c === ']') {
        depth--
        i++
        continue
      }
      if (c === ',' && depth === 0) {
        args.push(text.slice(start, i).trim())
        start = i + 1
        i++
        continue
      }
      i++
    }
    throw new Error(`textLocators: unbalanced parens scanning a call at index ${openParenIdx} of ${JSON.stringify(text.slice(openParenIdx, openParenIdx + 40))}`)
  }

  // A quoted string or a template literal (with or without `${...}`): to
  // Playwright all three are a string, matched as a substring.
  const STRING_LITERAL = /^'(?:[^'\\]|\\.)*'$|^"(?:[^"\\]|\\.)*"$|^`(?:[^`\\]|\\.)*`$/
  // Captures the pattern body of a /.../flags regex literal.
  const REGEX_LITERAL = /^\/((?:[^/\\[\]]|\\.|\[(?:[^\]\\]|\\.)*\])*)\/[a-z]*$/

  function isStringLiteral(arg: string): boolean {
    return STRING_LITERAL.test(arg)
  }

  function regexBody(arg: string): string | null {
    const m = REGEX_LITERAL.exec(arg)
    return m ? m[1] : null
  }

  /** Anchored at its start (`^`) or at its end: a `$` after an even number
   *  of backslashes. In `/costs \$/` the `$` is a dollar sign, matched
   *  anywhere; in `/C:\\$/` it is the end. */
  function isAnchored(body: string): boolean {
    if (body.startsWith('^')) return true
    if (!body.endsWith('$')) return false
    let slashes = 0
    for (let i = body.length - 2; i >= 0 && body[i] === '\\'; i--) slashes++
    return slashes % 2 === 0
  }

  function hasExact(rest: string, value: 'true' | 'false'): boolean {
    return new RegExp(`exact\\s*:\\s*${value}\\b`).test(rest)
  }

  function scanCalls(file: string, source: string, callRe: RegExp, judge: (arg0: string, rest: string) => boolean): Violation[] {
    const out: Violation[] = []
    let m: RegExpExecArray | null
    while ((m = callRe.exec(source))) {
      const openParen = m.index + m[0].length - 1
      const { args, endIdx } = splitTopLevelArgs(source, openParen)
      const arg0 = args[0] ?? ''
      const rest = args.slice(1).join(', ')
      if (judge(arg0, rest)) {
        out.push({ file, line: lineOf(source, m.index), snippet: source.slice(m.index, Math.min(endIdx, m.index + 160)) })
      }
      callRe.lastIndex = endIdx
    }
    return out
  }

  // web/e2e/**/*.ts: Playwright's `getByText`/`getByLabel`/`getByTitle`/
  // `getByPlaceholder` do a case-insensitive SUBSTRING match on a string by
  // default. A string literal must carry `{ exact: true }`; a regex literal
  // must be anchored with `^` or `$` (otherwise it is just as loose a
  // substring match).
  export function scanPlaywrightLocators(file: string, source: string): Violation[] {
    const callRe = /\b(?:getByText|getByLabel|getByTitle|getByPlaceholder)\(/g
    return scanCalls(file, source, callRe, (arg0, rest) => {
      if (isStringLiteral(arg0)) return !hasExact(rest, 'true')
      const body = regexBody(arg0)
      if (body === null) return false // not a literal (e.g. a variable): not ours to judge
      return !isAnchored(body)
    })
  }

  // web/src/**/*.test.ts(x): Testing Library's `*ByText`/`*ByLabelText`/
  // `*ByTitle` matchers are exact-string matches by default, so a string
  // literal is already safe. Only flag an explicit `{ exact: false }` (opts
  // back into substring matching), or a regex literal that is both short
  // (under 20 characters of pattern - long enough to rarely collide) and
  // unanchored.
  export function scanTestingLibraryLocators(file: string, source: string): Violation[] {
    const callRe = /\b(?:get|query|find|getAll|queryAll|findAll)By(?:Text|LabelText|Title)\(/g
    return scanCalls(file, source, callRe, (arg0, rest) => {
      if (hasExact(rest, 'false')) return true
      const body = regexBody(arg0)
      if (body === null) return false
      return body.length < 20 && !isAnchored(body)
    })
  }
  ```

**Revert-probes:** eight, run: each of the four rules removed in turn (a Playwright string without `{ exact: true }`, an unanchored Playwright regex, a unit test's `{ exact: false }`, a short unanchored regex), the 20-character boundary moved, a backtick literal not taken as a string, the `$` anchor not taken, and an escaped `\$` taken as one: each fails its test. The browser checks it changed were run: 17 passed (one fix of its own was wrong, an exact name for a label that wraps a `<select>`, and the run caught it). Playwright's `getByRole(…, { name })` is a substring match too and is not scanned yet (a follow-up, in the file's header).

Run: `nix develop -c sh -c 'cd web && pnpm test'`
Expected: PASS, 769 tests.

```bash
git add web
git commit -m "test(web): guard against loose text locators in the web tests"
```


## After this plan

**What Part 2 (Tasks 6–10, PR 4c-ii) must know:**
- **The seams Part 1 leaves:**
  - `QuestionCard` takes the card's actions through its environment (`components/items/types.ts`); without them it is read-only;
  - the `turn_not_delivered` marker shows "Send again" only when the environment gives it a handler;
  - `useSessionItems(id).setCatalog(catalog)` replaces the catalogue (a config change's 202);
  - the item renderers skip a render when their item and environment are the same objects: an environment passed down must be memoised;
  - one agent label, `agentLabel` in `lib/agent.ts`.
- **Where Part 2 mounts:** the composer and the footers inside `screens/Session.tsx` under the transcript; New Session replaces `/new`'s placeholder in `Shell.tsx`.
- **4b's deferred items:** a failed sign-out and `MESSAGES` read by own keys were done by plan 4d-i (#109). `inert` behind a modal only partly: the step-up dialog makes the page inert (`App.tsx`), but 4d-i's `ConfirmDialog`, which the delete dialog reuses, does not. Part 2 (PR 4c-ii) amends brief item 36 instead (its review's MUST-3, option (b)): a card cannot take the focus from the dialog, and a test shows it; `inert` behind every `ConfirmDialog` (Hosts, Hats, the session menu) is a follow-up for 4d. The Markdown list-markers check in a real browser is Part 2's (Task 10).

**Obligations this plan hands on:**
- **4a-ii, when it merges:**
  - `catalog_changed` on the item stream is already handled (`useSessionItems`); its test fixture follows 4a-ii's final shape.
  - The branch rebases onto it before the PR merges; the plan's anchors are taken again then.
- **4d:**
  - the service worker's `notificationclick` navigates to the payload's `url`, `/sessions/<id>`, which Task 5 restores in a running app and on a cold start;
  - `GET /api/hosts/{id}/agents` replaces Part 2's fallback in `agentsFor` only;
  - the literal colours of the imported styles (code surfaces, the highlight palette) do not follow the hat.
- **4f:** the quickstart can show the list and a session from the first tester build of Part 1.

**Deferred** (owner named):
- **Windowing, again on a quiet machine** (any lane, before 4f's release notes): `HENNERY_WINDOWING=1 pnpm exec playwright test windowing` after `pnpm build`. Every number here was taken at a load of 34–74. If the quiet desktop threshold is well above the reach, `TAIL` can grow; it never needs to shrink.
- **The tail window** (Part 2 or a later polish): it grows at the tail while the reader stays at the end, so a session open for hours renders more than 200 rows (trimming the top while at the end is not built); a resync jumps to the end even when the reader had scrolled up; a removed pinned item falls back to the tail.
- **The phone's top bar** still says "Sessions" above the session's own header; hiding it on the session route would also hide the waiting badge there (4d's layout pass).
- **Footnotes:** two messages that both use `[^1]` share ids, so a footnote link may jump to another message's note (decision 18).

**Not tested here:**
- the list and the view in a real browser against the binary: Part 2's Task 10 (Playwright at 1280 and 390 px, the computed-style checks, no CSP violation);
- the service worker's link: 4d;
- the windowing threshold on a quiet machine (above);
- Linux, except by CI.

**Spec amendments** (to write back):
- **frontend §5:** the badge's words are "Waiting on a question" for `blocked` or `question_waits`, not "needs you" (decision 8); a hat switch always clears the selection (decision 11).
- **frontend §6 and client view §9 OQ1:** the transcript renders a tail window of 200 items, revealed before fetching older pages; the threshold was measured under load only (decision 20).
- **client view §8:** 4c ships as two PRs, 4c-i and 4c-ii.

Generated with Claude AI — please review before distribution.
