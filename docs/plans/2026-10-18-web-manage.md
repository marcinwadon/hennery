# The management screens: hosts and hats (plan 4d-i) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** the first half of 4d, the web UI's management screens (frontend spec §8; client view spec §8, part 4d), over the API on `main`:
- **Hosts** (`/hosts`):
  - every paired host, revoked ones included and shown as revoked, with its online state, its platform, its hennery version, its default hat and when it was paired and last connected;
  - "Add host" mints a one-time pairing code behind a step-up and shows it with the exact `hennery host join <public_url> <code>` command and a countdown to its expiry;
  - rename, change the default hat, or revoke a host, each confirmed, then stepped up. The revoke dialog says the host's running agents stop when it next connects.
- **Hats** (`/hats`):
  - create a hat; rename and recolour it, or make it the default for new hosts (step-up);
  - each host's path rules, edited and saved as a whole set (step-up), with a live "this path resolves to" tester that asks the host;
  - "Purge hat", from the server's preview of what goes, confirmed, then stepped up, with what it deleted shown afterwards.
- **Two things 4b handed on:**
  - the page behind the step-up dialog is `inert` while it is open;
  - a failed sign-out stays where it is and says the browser is still signed in, rather than showing the login screen.
- Every server string these screens show (host and hat names, platforms, paths, the server's and the host's error messages) is escaped and bidi-isolated: each Unicode format, control, line or paragraph separator character is shown as `<U+XXXX>`.

**Architecture:** all of it is in `web/`. No Rust changes, and no new dependency.
- `src/lib/text.tsx`: `visible()` and `<Text>`, the one way these screens render server text.
- `src/components/`:
  - `ConfirmDialog.tsx`: a confirmation that runs its action and stays open while it runs, so a step-up dialog can open over it;
  - `SignOut.tsx`;
  - `When.tsx`;
  - `Pairing.tsx`: the code, the command, the countdown;
  - `PathRules.tsx`: the rules and the tester.
- `src/screens/{Hosts,Hats}.tsx`, with `src/manage.css` for both.
- `src/api/manage.ts`: the routes, typed.
- `src/lib/manage.ts`: the classifiers (a host's state, whether a hat can be purged, a safe colour).
- `src/hooks/useResource.ts`.
- `e2e/host.ts`, `e2e/manage.spec.ts`: a real host paired with the command the page shows.
- Shared files, touched minimally (4c edits them too): `App.tsx` (the `inert` wrapper), `components/Shell.tsx` (two routes and the sign-out button), `api/errors.ts` (nine messages and `Object.hasOwn`), `e2e/collector.ts` (exports `BIN` and `scratchEnv`, runs the collector in the scratch environment, listens for `'error'`).

**Tech Stack:** as plan 4b: React 19.3.0, TypeScript 7.0.2, Vite 8.3.1, Tailwind 4.3.3, Vitest 5.0.3, Testing Library, `@playwright/test` 1.63.0 with the flake's Chromium. pnpm only.

**Spec:**
- frontend spec §2 (routes), §3 (step-up), §8 (Hosts, Hats), §10 (accessibility: 44 px targets, keyboard), §12 (Playwright at 1280 and 390 px);
- kernel spec §3.4 (step-up), §4.1 (pairing codes), §4.3 (revoke), §5 (hats, resolution, purge), §8 (the routes);
- client view spec §6 (the transport) and §8 (part 4d).

It builds on [plan 4b](2026-10-16-web-shell.md) (the client, `useClient`, `ApiFailure`, `messageOf`, the router, the step-up dialog, `startCollector()`) and takes up the frontend hand-offs of plans 3a, 3b-i, 5a, 5b, 5c, 9c and 9d (see "Where the hand-offs land"). Every anchor was taken from `main` at `ecc50cd0`.

**Status:** written 2026-10-02. Amended after the security review of 2026-10-02 (A1–A6 and O1–O5 taken, O6 in part, O7 recorded). Its scoped re-confirmation (a fresh opus subagent, 2026-10-02) found every amendment in the code and re-confirmed with notes, both taken: R1 (required), the browser check's scripted focus aimed at a button that is disabled while the action waits, so it proved nothing; it now aims at the confirmation itself. R2, the Shift+Tab check first asserts no passkey is offered, so the password is the dialog's first field. It judged the Tab trap's wider selector sound for every dialog this plan renders. The per-task reviews of the execution then fixed focus, stale reads and the browser checks' environment (four `fix(web)` commits; see "Execution status"); the whole-branch review (opus) judged that they tighten what the security review ruled on and loosen nothing, so they need no further re-confirmation.

**How the code blocks were made and checked:**
- Nothing is imported from the predecessor (decision 1): every file is new or a change to 4b's, embedded verbatim.
- The code was built and tested first, then cut into each task's commits: tests first, then code. Every block below was generated from those commits, as diffs from `ecc50cd0`.
- The plan was then replayed from its own text onto `ecc50cd0`, step by step (`replay.py`). After each step the tree matched the matching commit byte for byte.
- Every guard was revert-probed; each task's "Revert-probes" step lists them, and every one was run.

## Execution status

Not executed yet.

## Scope

Part 4d-i of the frontend: **4 tasks.**
1. The shared parts: escaped server text, the confirmation dialog, the `inert` page, an honest sign-out.
2. Hosts: the list, pairing, rename, default hat, revoke.
3. Hats: create, edit, path rules and the tester, purge.
4. Playwright: pair a host, test a path, revoke with a step-up, at 1280 and 390 px.

**Out** (see "After this plan"):
- **4d-ii:** Settings (passkeys, signed-in devices, push devices, `public_url`, contact, attachments, per-hat push policy, host removals), push and the PWA. Settings' "Sign out" for narrow screens comes with it.
- **4d-iii** (after plans 4d-B1 and 4d-B2 merge): a host's agents, its last doctor result, its adapter versions and the notices of frontend §8, and hat logos.
- **4d-iv** (after plans 4d-B3 and 4d-B4): the deployment warning, and changing `public_url` from Settings.
- **4c:** the session list, a session, and new session; a revoked host's sessions.
- **4e:** the gateway screens.

**Where the hand-offs land:**

| Hand-off | Here |
|---|---|
| 3a: host names escaped fully, not only the format characters the collector refuses | Decision 2; Tasks 1, 2 |
| 3b-i: escaped and isolated server text | Decision 2 |
| 4b: the page behind the step-up dialog `inert` | Decision 6; Task 1 |
| 4b: a failed sign-out lands on `/login` while the cookie stays good | Decision 7; Task 1 |
| 4b: `MESSAGES[code]` reads inherited keys | Decision 10; Task 1 |
| 4b: the browser checks' collector has no `'error'` listener | Task 4 |
| 5a: list, create, rename, recolour hats; pick the default for new hosts; step-up on every change but creating; `name_taken` | Decision 11; Task 3 |
| 5a: rename a host and change its default hat (step-up), warning about parked sessions no rule covers | Decision 8; Task 2 |
| 5a: path rules sent whole, `verified` shown, prefixes escaped; a colour only after `^#[0-9a-f]{6}$` | Decisions 12, 13; Task 3 |
| 5b: the tester; `host_offline`, `resolve_unsupported`, `bad_host_answer`/`host_refused` as text, `no_answer`/`busy` | Decision 12; Task 3 |
| 5c: `hat_ambiguous` | Task 3 (the tester); New session is 4c's |
| 9c: the purge preview, its refusals (`hat_is_default`, `sessions_running`), "resume purge", `unconfirmed`, sessions with no hat | Decision 14; Task 3 |
| 9d: a purge's `host_transcripts` | Task 3 |
| 9d: pending host removals in Settings | Out: 4d-ii |

## Decisions this plan makes where the specs are silent

A stronger-model security review on the maintainer's behalf (opus, 2026-10-02) answered "approve after amendments". The decisions it changed are marked "(amended: …)".

**What the review changed:**
- A1: "Add host" read the hosts paired before it from the list on the screen, which can be empty when it failed to load: the first poll would then take an old host for the new one, and hide a code still valid. The hosts are now read just before the mint (decision 4).
- A2: `visible()` also escapes every `Default_Ignorable_Code_Point`: the Hangul fillers are letters (Lo), pass the server's display checks, and render as nothing (decision 2).
- A3: the source guard uses `visible()`'s own set, over `src/`, `public/`, `e2e/` and `index.html` (decision 2).
- A4: the confirmation traps Tab, and holds focus itself while its action runs, so a cancelled step-up returns focus into it (decision 3).
- A5: the test host's `join` is tracked and stopped with the rest (decision 15).
- A6: the path rules' step-up test asserts both bodies are the same.

Optional hardening taken: O1 (read before minting), O2 (count from arrival), O3 (`pagehide`), O4 (`HOME` and `XDG_*` in the test directory, a stand-in that exists everywhere, no wait on a process that never started, `inert` checked in Chromium, by focus since the re-confirmation), O5 (a purging hat kept in a rule's picker), O6 in part (`<When>` escapes what it shows, and a host's default hat id is shown escaped; 4b's step-up dialog still shows its error in a plain `<bdi>`, its messages being the auth routes' own: recorded). O7 (a cancelled step-up tested per action) is recorded: the transport's own test covers it once for all.

**What the re-confirmation changed** (decision 3; Task 4): the click under the step-up dialog is no longer checked, since Playwright refuses a click on a covered element whether or not it is inert; `inert` is checked by focus instead, by script on the confirmation itself (R1) and by Shift+Tab with no passkey offered (R2). The confirmation's Tab trap covers every focusable kind of control, with a test of its own (`ConfirmDialog.test.tsx`).

1. **Nothing is imported from the predecessor.**
   - **Choice:** unlike 4b, no import commit. Every file is written for hennery's API and embedded verbatim.
   - **Why:** what the predecessor had for these screens was hard-wired to its own clients and machines (its hats were a fixed list matched by path segments), and its push code used other routes and field names. Nothing of it fits. 4d-ii writes push and the service worker the same way.
   - **Cost if wrong:** none; the look is 4b's tokens and classes, extended in `manage.css`.
2. **Server text is shown escaped and isolated, by one helper (frontend §6.4, §8; 3a's hand-off).**
   - **Choice:** `visible(text)` writes each character of Unicode categories Cf (format: bidi controls, zero-width characters, the soft hyphen, the BOM, tag characters), Cc (control), Zl and Zp, and every other `Default_Ignorable_Code_Point` (the Hangul fillers, which are letters and pass the server's checks, the variation selectors, the combining grapheme joiner), as `<U+XXXX>` (amended: A2). `<Text>` renders the result in a `<bdi>`. Combining marks (Mn) and private-use characters stay: scripts need the first, and the second render as a visible glyph.
   - **Where:** every host and hat name, platform, version, path, session title, and every error message these screens show, the host's own (`host_refused`, `bad_host_answer`) included. In an `<option>`, which cannot hold a `<bdi>`, the text is `visible()`'s.
   - **Why the whole categories:** the collector refuses a hand-picked set in host names (plan 3a), and a path from a host's filesystem is refused by nothing. React already makes markup text; this makes invisible characters visible and keeps a right-to-left override from reordering the line around it.
   - **A source guard** (amended: A3): `security.test.ts` fails on any character `visible()` would escape, tab and newlines aside, written literally in `src/` (tests included), `public/`, `e2e/` or `index.html`. It uses `visible()`'s own set, so tests spell such characters as escapes.
3. **A confirmation runs its action, and stays open while it runs.**
   - **Choice:** `ConfirmDialog` takes `action`. The confirm button runs it; the dialog closes when it resolves and shows the refusal, as text, when it fails.
   - **With step-up:** a refused action opens the step-up dialog over the confirmation. Cancelling the step-up leaves the confirmation open with "Not confirmed.", and nothing is sent again.
   - **Focus** (amended: A4): it moves to "Cancel" (the safe choice) when the dialog opens, and back to the button that opened it when it closes. Tab and Shift+Tab cycle through the dialog's own controls (buttons, links, inputs, selects, text areas and anything with a non-negative `tabindex`; amended at the re-confirmation), so nothing behind it is reached. While the action runs, its buttons are disabled and the dialog itself holds focus, so a step-up opened over it returns focus into it, where Escape still cancels (except while the action runs). A click on its backdrop keeps focus in it. When what opened it is gone when it closes (a revoked host's card loses its buttons; a rename form hides as the confirmation opens), focus goes to a fallback the caller names (`returnFocus`): the card's title, or its Rename button (amended after the Task 1 and Task 2 reviews). Its title's id is its own (`useId`).
4. **The pairing code is shown once, and kept nowhere else (kernel §4.1; the review's focus).**
   - **Minted on the click**, never on mount: React's StrictMode runs effects twice in development, and two mints would spend two of the 16 live codes. `GET /api/settings` and `GET /api/hosts` are read first, then the code is minted (amended: O1, A1): a failed read spends no code, and the hosts paired before it are known even when the list on the screen never loaded.
   - **The command** is `hennery host join <public_url> <code>`, with `public_url` from `GET /api/settings`, not `location.origin`: hosts reach the collector at the URL setup stored.
   - **Where the code lives:** the Hosts screen's state and the panel only. It is never in the address bar, the title, storage, a log line or a request but the mint's answer. It leaves the page when it expires, when a new host pairs (it then leaves the Hosts screen's state too, and "Add host" is offered again; amended after the Task 2 review), when the panel closes or the screen unmounts, and on `pagehide`, so a page restored from the back-forward cache shows none (amended: O3). Switching tabs keeps it: the owner switches to a terminal to paste it.
   - **The countdown** runs 600 s, the code's lifetime, from the answer's arrival, on the monotonic clock (`performance.now()`), not from the server's `expires_at`: this browser's clock may be off by more than the lifetime (amended: O2). The code dies within the network's delay of what it shows. At 0 it says the code expired. A host that pairs meanwhile is noticed by reading `GET /api/hosts` every 3 s while the code is live, one read at a time, and told from the hosts read before the mint.
   - **Closing the panel** only hides the code: there is no route to revoke one, and the panel says it stays valid until it expires.
   - **Its hint:** the host keeps its pairing in `--data-dir` (or `HENNERY_HOST_DATA_DIR`), which `host join` requires today; and leaving the code out makes `host join` read it from standard input, which keeps it out of the shell's history.
5. **A host's state is one of three, revoked first.** `hostState`: `revoked` when `revoked_at` is set, whatever `connected` says; else `online` when `connected` (connected and reconciled); else `offline`. A revoked host is listed with no action.
6. **The page behind the step-up dialog is `inert` (4b's hand-off).** `App` wraps the routed screen in `<div class="page" inert={…} style="display: contents">`, `inert` while the dialog waits. Focus and clicks reach only the dialog. A confirmation under it is inert too, and is live again when the step-up closes.
7. **A failed sign-out says so, and stays (4b's hand-off; for the review).**
   - **Choice:** `SignOut` goes to `/login` only after `POST /api/auth/logout` answered 2xx. Any failure, a refusal (`origin_mismatch`) or a request that never left (offline), keeps the page and says "Not signed out: this browser is still signed in.", with the reason, and the button can be pressed again.
   - **Why:** the session cookie is `HttpOnly`, so the page cannot clear it; showing the login screen while it is still valid would tell someone on a shared device that they are signed out when they are not.
   - **Alternative:** going to `/login` with a warning there. Rejected: the login screen is where a signed-out user is, and nobody reads a warning on it.
8. **Changes to a host are confirmed, then stepped up (frontend §8, kernel §4.3; 5a's hand-off).**
   - **Rename:** an inline field and "Save", then a confirmation naming the old and the new name, then `PATCH /api/hosts/{id} {name}`.
   - **Default hat:** choosing another hat opens a confirmation that says which hat sessions no rule covers will get, and that parked or closed sessions there keep theirs, so resuming one is refused until it is re-assigned (`hat_mismatch`). 5a asks for their count; no route gives it, so the warning is general (After this plan).
   - **Revoke:** "can no longer connect, and its sessions are parked. Pairing it again needs a new code. Agents it is running stop only when it next connects: it is then told it is revoked."
   - The server's answer, the `HostItem` as it is now, replaces the card.
9. **The hosts list is read once per visit.** No stream of host changes exists. The pairing panel reads it again while a code is live, and every change replaces its card from the answer.
10. **The message table reads its own keys only (4b's hand-off).** `ApiFailure` looks a code up with `Object.hasOwn`, so a server code named `toString` or `__proto__` shows the server's message, not an inherited function's text. Nine codes gain plain messages: `too_many_codes`, `name_taken`, `hat_purging`, `hat_is_default`, `resolve_unsupported`, `no_answer`, `busy`, `too_many_subscriptions` and `endpoint_taken` (the last two for 4d-ii). `sessions_running`, `bad_host_answer` and `host_refused` keep the server's text, which names the sessions or is the host's own.
11. **Hats (kernel §5, 5a).**
    - Creating needs no step-up; renaming, recolouring and "make default for new hosts" do (`PATCH /api/hats/{id}`). Only purging is confirmed first (frontend §8); the others are undone as easily as done.
    - Only one hat is the default for new hosts: after one is made so, the others lose the mark.
    - Each answer is folded into the list as it is when it lands, not as it was when the action began, so two changes that land together both stay (amended after the Task 3 review). A name of spaces only cannot be saved (amended after the whole-branch review). Focus returns to "Edit" after an edit, to the card's title once "Make default for new hosts" is gone, and to the name after a hat is created.
    - A hat being purged (`purging`) can be neither edited nor made a default; its button reads "Resume purge".
12. **Path rules and the tester (kernel §5.2; 5a, 5b).**
    - **The set:** a host's rules are edited as rows and sent whole (`PUT …/path-rules`, step-up). The answer is the set as the host resolved and stored it, which replaces the rows: a typed `/tmp/x` comes back as `/private/tmp/x`, and the next edit starts from that.
    - **Unverified** rules (the path did not exist when saved) are marked.
    - A row with no path blocks "Save rules", and says so: the server refuses the whole set for one (amended after the Task 3 review). Removing a row moves focus to the next row's path, else to "Add rule".
    - After a purge is tried, finished or not, the rules are read again: the server deleted the purged hat's rules, and a set naming it would be refused (amended after the Task 3 review).
    - **Offline:** the server refuses a set while the host is away (`host_offline`); the card says so beforehand.
    - **The tester** asks `POST /api/hats/resolve` 400 ms after typing pauses, against the rules **as saved**, and says so. A newer path aborts the older question and clears the last answer at once, and an answer that comes back after it was aborted is dropped, so an answer never lands under another path. It asks again after the rules are saved or a purge is tried (amended after the Task 3 review). Its answer sits in one live region, announced once. It shows the canonical path, the hat, whether a rule or the host's default decided, and whether the path exists or is a directory. `host_offline`, `resolve_unsupported` and `hat_ambiguous` have plain words; a host's own refusal is shown as escaped text.
    - Only hosts that are not revoked are offered.
    - A rule whose hat is being purged keeps that hat in its picker, shown and disabled, so the picker says what a save sends (amended: O5).
13. **A hat's colour reaches a style only as `#rrggbb` (5a).** `safeColour` accepts `^#[0-9a-f]{6}$`, what the server guarantees, and nothing else; a swatch without one has no colour. The colour picker's value is lower-cased before it is sent.
14. **Purge is driven by the server's preview (kernel §5.5, 9c, 9d).**
    - **The preview** (`GET /api/hats/{id}/purge`, no step-up) gives the counts the dialog lists: sessions, path rules, recent projects, and the hat with its push policy.
    - **`purgeState`** decides, in the server's order:
      - `default`: the hat is the default for new hosts, or any host's default, revoked hosts included (409 `hat_is_default`);
      - `running`: sessions may run on a host the collector reaches; they are listed as links, to close first (409 `sessions_running`);
      - `resume`: a purge began and stopped;
      - `ready` otherwise.
      The first two disable the confirm button; the card's "Purge" is disabled for a default hat already.
    - **Sessions with no hat** are listed as links: no purge deletes them; the operator re-assigns or deletes them one by one (4c's screens).
    - **A tried purge**, finished or not, reads the hats again, so a hat the server froze offers "Resume purge" (amended after the Task 3 review).
    - **Afterwards** a notice, which takes focus (the purged card and its button are gone; amended after the whole-branch review), shows what was deleted, the sessions deleted while their host was away (`unconfirmed`, as text: they no longer exist), and the agents' transcripts on the hosts: removed, removed in part, still to remove.
15. **The browser checks run a real host, hermetically (frontend §12).**
    - **Pairing:** the test reads the command from the page and runs it, `hennery host join <url> <code>`, adding `--name` and `--no-runtime` (no adapter download), with `HENNERY_HOST_DATA_DIR`, `HOME` and the `XDG_*` directories in a fresh directory (amended: O4). It then runs `hennery host run` with one stand-in agent (`--agent stand-in=<the binary>`, a path that exists everywhere), which is never started, so the host connects without any adapter set. The tester resolves real paths through it.
    - **Step-up on revoke:** a session is stepped up for 5 minutes after setup, so no real 403 comes during the run. The test answers the first `DELETE /api/hosts/{id}` with 403 `step_up_required` (`page.route`) and lets the retry reach the server. That the server refuses an unstepped revoke is the Rust tests' (`step_up.rs`).
    - **Both widths** run the same three checks, each with a collector and a host of its own, then check that no page broke the Content-Security-Policy: by the console on every page, and by `securitypolicyviolation` events on the last page loaded (an init script's list starts again on each navigation), as 4b's checks do.
    - **Every binary the checks start** (the collector, `host join`, `host run`) runs in `scratchEnv(dir)`: the runner's environment with every `HENNERY_*` variable removed, and `HOME` and `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME` in the test's directory. A sentinel `HENNERY_LOG_DIR` set in the runner's own environment must stay empty (amended after the Task 4 review: the collector inherited the runner's environment, and with it a log directory or `HENNERY_SERVICE`'s home logs). `host run`'s standard error is shown when it fails.
    - Every process is stopped by its own id, SIGTERM then SIGKILL, the `join` included (amended: A5), and every directory is removed, on every path. A process that never started is not waited for.
    - **`inert` in Chromium** (amended: O4, then at the re-confirmation): while the step-up dialog is open, the page under it is `inert`; focus is in the dialog; a script's `focus()` on the confirmation itself leaves it there (not on its buttons: they are disabled while its action waits, and a disabled button takes no focus, `inert` or not); and Shift+Tab from the dialog's first field, the password while no passkey is offered (asserted), lands nowhere on the page. A click on the covered confirmation is not checked: Playwright refuses it because the overlay is on top, `inert` or not, so it proved nothing.

## Global Constraints

- After every task, from the repository root:
  - `nix develop -c pnpm --dir web typecheck`;
  - `nix develop -c pnpm --dir web test`;
  - from Task 4, `pnpm --dir web build`, then `HENNERY_WEB_REQUIRE=1 cargo build -p hennery --locked`, then `pnpm --dir web e2e`.
  No Rust file changes, so the Rust checks stand as on `main`; CI runs them.
- pnpm only; no new dependency, so `pnpm-lock.yaml` does not change.
- **Repository hygiene** (public repository), before every commit: the contract's two greps (tracked text, and `web/` with binary files) find none of the predecessor's name, the two company names, the commercial font's name, or home-directory paths. Commit subjects say what the code does.
- Shared files (`App.tsx`, `Shell.tsx`, `errors.ts`, `e2e/collector.ts`) change by small hunks only: 4c edits them too.

## Review Focus

1. **The pairing code** (decision 4).
   - Expected: shown once; never in the URL, title, storage or console; gone at expiry, on pairing, on close and on `pagehide`; the countdown 600 s from arrival whatever this clock says; the command from the stored `public_url`; nothing minted when a read fails; a host paired before the mint never taken for the new one.
   - Tests: Task 2 `Hosts.test.tsx` ("adding a host", "the countdown"); Task 4 "a host pairs with the command the page shows, and the code goes".
2. **Step-up on every listed action** (decisions 3, 8, 11, 12, 14).
   - Expected: minting, renaming, re-hatting and revoking a host; editing a hat; saving path rules; purging. Each sends the same request again after the step-up, once; a cancelled step-up sends nothing.
   - Tests: one per action in Tasks 2 and 3 (two identical bodies sent); Task 4's revoke.
3. **Escaped, isolated text** (decision 2).
   - Expected: no server string renders markup or reorders its line; every hidden character is visible.
   - Tests: Task 1 `text.test.tsx`; Task 2 "renders a host name as escaped, isolated text"; Task 3 "shows a host’s own refusal as escaped text"; Task 1 `security.test.ts`.
4. **The `inert` page and the honest sign-out** (decisions 6, 7).
   - Tests: Task 1 `StepUpDialog.test.tsx` ("leaves the page under it inert…"), `SignOut.test.tsx`; Task 4's revoke checks `inert` in Chromium.
5. **Purge** (decision 14).
   - Expected: each refusal state disables the action and says why; the result is shown as the server gave it.
   - Tests: Task 2 `manage.test.ts` (`purgeState`, each outcome); Task 3 "purging a hat".

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `web/src/lib/text.tsx`, `web/src/lib/text.test.tsx` (new) | `visible`, `<Text>` | 1 |
| `web/src/api/errors.ts`, `web/src/api/client.test.ts` | nine messages; `Object.hasOwn` | 1 |
| `web/src/components/When.test.tsx`, `web/src/components/SignOut.test.tsx` (new), `web/src/components/StepUpDialog.test.tsx` | A time, escaped; sign-out; the `inert` page | 1 |
| `web/src/components/{ConfirmDialog,SignOut,When}.tsx` (new), `web/src/components/ConfirmDialog.test.tsx` (new) | The confirmation, sign-out, a time | 1 |
| `web/src/hooks/useResource.ts`, `web/src/hooks/useResource.test.ts` (new) | One server read per screen | 1 |
| `web/src/App.tsx`, `web/src/components/Shell.tsx` | The `inert` page; the sign-out button; the two routes | 1, 2, 3 |
| `web/src/security.test.ts` | No literal hidden characters | 1 |
| `web/src/api/manage.ts`, `web/src/lib/manage.ts`, `web/src/lib/manage.test.ts` (new) | The routes; the classifiers | 2 |
| `web/src/components/Pairing.tsx`, `web/src/screens/Hosts.tsx`, `web/src/screens/Hosts.test.tsx`, `web/src/manage.css` (new) | Hosts | 2 |
| `web/src/test-fixtures.ts`, `web/src/test-targets.ts` (new) | One of each item, every field set; the narrow screen's 44 px rules read into jsdom | 2 |
| `web/src/components/PathRules.tsx`, `web/src/screens/Hats.tsx`, `web/src/screens/Hats.test.tsx` (new) | Hats | 3 |
| `web/e2e/host.ts`, `web/e2e/manage.spec.ts` (new), `web/e2e/collector.ts` | Playwright | 4 |

All commands run from the repository root inside the dev shell (`nix develop -c …`). Work on a feature branch off `main`.

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block, and a final newline.
- "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point, as whole lines (earlier blocks of the same task already applied, in order). Then comes "with:" and its replacement.

---

### Task 1: Escaped text, the confirmation, the inert page, sign-out

- [ ] **Step 1: Write the tests**

In `web/src/api/client.test.ts`, replace:

  ```ts
      ['unauthenticated', 'Sign in first.'],
    ])('explains %s in its own words', async (code, message) => {
  ```

with:

  ```ts
      ['unauthenticated', 'Sign in first.'],
      ['too_many_codes', 'Sixteen pairing codes are live already: wait for one to expire.'],
      ['name_taken', 'Another hat has this name.'],
      ['hat_purging', 'This hat is being purged.'],
      ['hat_is_default', 'This hat is the default for new hosts, or a host’s default hat: make another hat that default first.'],
      ['resolve_unsupported', 'This host cannot resolve paths yet: update hennery on it.'],
      ['no_answer', 'The host did not answer in time. Try again.'],
      ['busy', 'The host is busy. Try again.'],
      ['too_many_subscriptions', 'Remove a device before adding another: 32 at most.'],
      ['endpoint_taken', 'This browser receives notifications for another account.'],
    ])('explains %s in its own words', async (code, message) => {
  ```

In `web/src/api/client.test.ts`, replace:

  ```ts
      await expect(client.request('POST', '/api/x')).rejects.toMatchObject({ code: 'brand_new', message: 'server text' })
    })

    it('reads Retry-After on a 429', async () => {
  ```

with:

  ```ts
      await expect(client.request('POST', '/api/x')).rejects.toMatchObject({ code: 'brand_new', message: 'server text' })
    })

    it.each(['toString', 'constructor', '__proto__', 'hasOwnProperty'])(
      'shows the server’s message for %s, a name every object inherits',
      async (code) => {
        const { client } = stub([json(409, { code, message: 'server text' })])
        await expect(client.request('POST', '/api/x')).rejects.toMatchObject({ code, message: 'server text' })
      },
    )

    it('reads Retry-After on a 429', async () => {
  ```

Create `web/src/components/ConfirmDialog.test.tsx`:

  ```tsx
  import { render, screen } from '@testing-library/react'
  import userEvent from '@testing-library/user-event'
  import { useState } from 'react'
  import { describe, expect, it, vi } from 'vitest'
  import ConfirmDialog from './ConfirmDialog'

  describe('the confirmation', () => {
    it('keeps Tab among its own controls, whatever kind they are', async () => {
      render(
        <>
          <button type="button">Behind</button>
          <ConfirmDialog title="Pick one?" confirm="Go" action={async () => {}} onClose={() => {}}>
            <select aria-label="Choice">
              <option>a</option>
            </select>
            <textarea aria-label="Note" />
            <span tabIndex={0}>Focusable</span>
          </ConfirmDialog>
        </>,
      )
      const cancel = screen.getByRole('button', { name: 'Cancel' })
      const go = screen.getByRole('button', { name: 'Go' })
      const choice = screen.getByRole('combobox', { name: 'Choice' })
      expect(cancel).toHaveFocus()
      await userEvent.tab()
      expect(go).toHaveFocus()
      // Forward from the last control wraps to the first: the select.
      await userEvent.tab()
      expect(choice).toHaveFocus()
      await userEvent.tab()
      expect(screen.getByRole('textbox', { name: 'Note' })).toHaveFocus()
      await userEvent.tab()
      expect(screen.getByText('Focusable')).toHaveFocus()
      await userEvent.tab()
      expect(cancel).toHaveFocus()
      // Backward from the first control wraps to the last.
      choice.focus()
      await userEvent.tab({ shift: true })
      expect(go).toHaveFocus()
      expect(screen.getByRole('button', { name: 'Behind' })).not.toHaveFocus()
    })

    it('keeps focus when its backdrop is clicked, so Tab and Escape still work', async () => {
      const onClose = vi.fn()
      render(
        <>
          <button type="button">Behind</button>
          <ConfirmDialog title="Pick one?" confirm="Go" action={async () => {}} onClose={onClose}>
            <p>Body</p>
          </ConfirmDialog>
        </>,
      )
      const dialog = screen.getByRole('dialog', { name: 'Pick one?' })
      await userEvent.click(dialog.parentElement!)
      expect(dialog.contains(document.activeElement)).toBe(true)
      await userEvent.tab()
      expect(dialog.contains(document.activeElement)).toBe(true)
      await userEvent.keyboard('{Escape}')
      expect(onClose).toHaveBeenCalledOnce()
    })

    it('is named by its own title, whatever else is on the page', () => {
      render(
        <>
          <ConfirmDialog title="First?" confirm="Go" action={async () => {}} onClose={() => {}}>
            <p>One</p>
          </ConfirmDialog>
          <ConfirmDialog title="Second?" confirm="Go" action={async () => {}} onClose={() => {}}>
            <p>Two</p>
          </ConfirmDialog>
        </>,
      )
      expect(screen.getByRole('dialog', { name: 'First?' })).toHaveTextContent('One')
      expect(screen.getByRole('dialog', { name: 'Second?' })).toHaveTextContent('Two')
    })

    it('returns focus to a fallback when what opened it is gone', async () => {
      function Page() {
        const [open, setOpen] = useState(false)
        const [opener, setOpener] = useState(true)
        return (
          <>
            <h2 tabIndex={-1} id="fallback">
              Fallback
            </h2>
            {opener && (
              <button type="button" onClick={() => setOpen(true)}>
                Open
              </button>
            )}
            {open && (
              <ConfirmDialog
                title="Go?"
                confirm="Go"
                action={async () => setOpener(false)}
                onClose={() => setOpen(false)}
                returnFocus={() => document.getElementById('fallback')}
              >
                <p>Body</p>
              </ConfirmDialog>
            )}
          </>
        )
      }
      render(<Page />)
      await userEvent.click(screen.getByRole('button', { name: 'Open' }))
      await userEvent.click(screen.getByRole('button', { name: 'Go' }))
      expect(screen.queryByRole('dialog')).toBeNull()
      expect(screen.getByRole('heading', { name: 'Fallback' })).toHaveFocus()
    })
  })
  ```

Create `web/src/components/SignOut.test.tsx`:

  ```tsx
  import { render, screen, waitFor } from '@testing-library/react'
  import userEvent from '@testing-library/user-event'
  import { afterEach, describe, expect, it } from 'vitest'
  import App from '../App'
  import { FULL, json, stubServer } from '../test-server'

  afterEach(() => history.replaceState(null, '', '/'))

  function signedIn(logout: Response | (() => Response | Promise<Response>)) {
    history.replaceState(null, '', '/mcp')
    const server = stubServer({
      'GET /api/capabilities': json(200, FULL),
      'POST /api/auth/logout': logout,
      'POST /api/auth/passkeys/login/start': json(409, { code: 'no_passkeys', message: 'm' }),
    })
    render(<App fetchImpl={server.fetch} />)
    return server
  }

  describe('signing out', () => {
    it('goes to the login screen once the server has ended the session', async () => {
      signedIn(new Response(null, { status: 204 }))
      await userEvent.click(await screen.findByRole('button', { name: 'Sign out' }))
      await waitFor(() => expect(location.pathname).toBe('/login'))
    })

    it('stays, and says the browser is still signed in, when the server refused', async () => {
      signedIn(json(403, { code: 'origin_mismatch', message: 'm' }))
      await userEvent.click(await screen.findByRole('button', { name: 'Sign out' }))
      expect(await screen.findByRole('alert')).toHaveTextContent(
        'Not signed out: this browser is still signed in. The public URL must be the address this page is open at.',
      )
      expect(location.pathname).toBe('/mcp')
      expect(screen.getByRole('button', { name: 'Sign out' })).toBeEnabled()
    })

    it('stays when the request never reached the server', async () => {
      signedIn(() => {
        throw new TypeError('Failed to fetch')
      })
      await userEvent.click(await screen.findByRole('button', { name: 'Sign out' }))
      expect(await screen.findByRole('alert')).toHaveTextContent('Not signed out: this browser is still signed in.')
      expect(location.pathname).toBe('/mcp')
    })
  })
  ```

In `web/src/components/StepUpDialog.test.tsx`, replace:

  ```tsx
    history.replaceState(null, '', '/hosts')
  ```

with:

  ```tsx
    history.replaceState(null, '', '/mcp')
  ```

In `web/src/components/StepUpDialog.test.tsx`, replace:

  ```tsx

    it('closes on Escape, as a cancel', async () => {
  ```

with:

  ```tsx

    it('leaves the page under it inert while it is open, and only then', async () => {
      const server = stubServer({
        'GET /api/capabilities': json(200, FULL),
        'POST /api/hosts/pairing-codes': json(403, STEP_UP),
      })
      await open(server)
      const page = screen.getByLabelText('Name').closest('.page')!
      expect(page).toHaveAttribute('inert')
      expect(screen.getByRole('dialog').closest('.page')).toBeNull()
      await userEvent.click(screen.getByRole('button', { name: 'Cancel' }))
      await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
      expect(page).not.toHaveAttribute('inert')
    })

    it('closes on Escape, as a cancel', async () => {
  ```

Create `web/src/components/When.test.tsx`:

  ```tsx
  import { render, screen } from '@testing-library/react'
  import { describe, expect, it } from 'vitest'
  import When from './When'

  describe('a server time', () => {
    it('shows one it cannot read as the server sent it, escaped, in its text and its attributes', () => {
      render(<When at={'2026\u202e-10-02'} />)
      const time = screen.getByText('2026<U+202E>-10-02')
      expect(time.tagName).toBe('TIME')
      expect(time).toHaveAttribute('dateTime', '2026<U+202E>-10-02')
      expect(time).toHaveAttribute('title', '2026<U+202E>-10-02')
    })

    it('shows one it can read in this browser’s own words, the exact value a hover away', () => {
      render(<When at="2026-10-02T12:00:00Z" />)
      const time = screen.getByTitle('2026-10-02T12:00:00Z')
      expect(time).toHaveAttribute('dateTime', '2026-10-02T12:00:00Z')
      expect(time).toHaveTextContent(new Date('2026-10-02T12:00:00Z').toLocaleString())
    })
  })
  ```

Create `web/src/hooks/useResource.test.ts`:

  ```ts
  import { act, renderHook, waitFor } from '@testing-library/react'
  import { describe, expect, it } from 'vitest'
  import { useResource } from './useResource'

  /** A load that answers only when told to. */
  function deferred() {
    const answers: Array<(value: string) => void> = []
    const load = () => new Promise<string>((resolve) => answers.push(resolve))
    return { load, answer: (value: string) => answers.at(-1)!(value) }
  }

  describe('one server read', () => {
    it('drops what it read for the old key as the key changes', async () => {
      const server = deferred()
      const { result, rerender } = renderHook(({ id }) => useResource(server.load, [id]), { initialProps: { id: 'a' } })
      act(() => server.answer('rules of a'))
      await waitFor(() => expect(result.current.data).toBe('rules of a'))
      rerender({ id: 'b' })
      expect(result.current.data).toBeNull()
      act(() => server.answer('rules of b'))
      await waitFor(() => expect(result.current.data).toBe('rules of b'))
    })

    it('keeps what it shows while it reads the same key again', async () => {
      const server = deferred()
      const { result, rerender } = renderHook(({ id }) => useResource(server.load, [id]), { initialProps: { id: 'a' } })
      act(() => server.answer('first'))
      await waitFor(() => expect(result.current.data).toBe('first'))
      act(() => result.current.reload())
      rerender({ id: 'a' })
      expect(result.current.data).toBe('first')
      act(() => server.answer('second'))
      await waitFor(() => expect(result.current.data).toBe('second'))
    })

    it('takes a change as a function of what it holds', async () => {
      const server = deferred()
      const { result } = renderHook(() => useResource(server.load))
      act(() => server.answer('a'))
      await waitFor(() => expect(result.current.data).toBe('a'))
      act(() => {
        result.current.set((prev) => `${prev}b`)
        result.current.set((prev) => `${prev}c`)
      })
      expect(result.current.data).toBe('abc')
    })
  })
  ```

Create `web/src/lib/text.test.tsx`:

  ```tsx
  import { render } from '@testing-library/react'
  import { describe, expect, it } from 'vitest'
  import { Text, visible } from './text'

  describe('visible', () => {
    it('leaves ordinary text as it is, other scripts included', () => {
      expect(visible('build-box 2 · łódź · ビルド · مضيف')).toBe('build-box 2 · łódź · ビルド · مضيف')
    })

    it.each([
      ['soft hyphen', '\u00AD', '<U+00AD>'],
      ['zero-width space', '\u200B', '<U+200B>'],
      ['zero-width joiner', '\u200D', '<U+200D>'],
      ['byte order mark', '\uFEFF', '<U+FEFF>'],
      ['left-to-right embedding', '\u202A', '<U+202A>'],
      ['right-to-left override', '\u202E', '<U+202E>'],
      ['left-to-right isolate', '\u2066', '<U+2066>'],
      ['pop directional isolate', '\u2069', '<U+2069>'],
      ['Arabic letter mark', '\u061C', '<U+061C>'],
      ['a tag character', '\u{E0041}', '<U+E0041>'],
    ])('escapes the %s, a format character', (_, c, shown) => {
      expect(visible(`a${c}b`)).toBe(`a${shown}b`)
    })

    it.each([
      ['Hangul filler', '\u3164', '<U+3164>'],
      ['Hangul choseong filler', '\u115F', '<U+115F>'],
      ['halfwidth Hangul filler', '\uFFA0', '<U+FFA0>'],
      ['variation selector 16', '\uFE0F', '<U+FE0F>'],
      ['combining grapheme joiner', '\u034F', '<U+034F>'],
      ['unassigned ignorable U+2065', '\u2065', '<U+2065>'],
    ])('escapes the %s, which renders as nothing', (_, c, shown) => {
      expect(visible(`a${c}b`)).toBe(`a${shown}b`)
    })

    it('leaves combining marks and private-use characters, which render', () => {
      expect(visible('e\u0301 \uE000')).toBe('e\u0301 \uE000')
    })

    it('escapes control characters', () => {
      expect(visible('a\u0000b\nc\u001Bd')).toBe('a<U+0000>b<U+000A>c<U+001B>d')
    })

    it('escapes the line and paragraph separators', () => {
      expect(visible('a\u2028b\u2029c')).toBe('a<U+2028>b<U+2029>c')
    })
  })

  describe('Text', () => {
    it('renders escaped text in a <bdi>, never as markup', () => {
      const { container } = render(<Text>{'<img src=x onerror=alert(1)>\u202Eevil'}</Text>)
      const bdi = container.querySelector('bdi')!
      expect(bdi.textContent).toBe('<img src=x onerror=alert(1)><U+202E>evil')
      expect(container.querySelector('img')).toBeNull()
    })
  })
  ```

In `web/src/security.test.ts`, replace:

  ```ts
  import { describe, expect, it } from 'vitest'

  ```

with:

  ```ts
  import { describe, expect, it } from 'vitest'
  import { HIDDEN } from './lib/text'

  ```

In `web/src/security.test.ts`, replace:

  ```ts

    it('leave no inline script in the page', () => {
  ```

with:

  ```ts

    it('hold no character `visible()` would escape, but tab and newlines: tests write them as escapes', () => {
      const all = (dir: string): string[] =>
        readdirSync(dir).flatMap((name) => {
          const path = join(dir, name)
          if (statSync(path).isDirectory()) return all(path)
          return /\.(ts|tsx|css|js|html|webmanifest)$/.test(name) ? [path] : []
        })
      const root = process.cwd()
      const files = [...all(SRC), ...all(join(root, 'public')), ...all(join(root, 'e2e')), join(root, 'index.html')]
      const found = files.filter((f) => [...readFileSync(f, 'utf8')].some((c) => !'\t\n\r'.includes(c) && HIDDEN.test(c)))
      expect(files.length).toBeGreaterThan(30)
      expect(found).toEqual([])
    })

    it('leave no inline script in the page', () => {
  ```


- [ ] **Step 2: Run them, and see them fail**

  Run: `nix develop -c pnpm --dir web test`
  Expected: `text.test.tsx`, `ConfirmDialog.test.tsx`, `When.test.tsx`, `useResource.test.ts` and `SignOut.test.tsx` fail (their modules do not exist); the new rows of `client.test.ts` (the nine messages, the inherited names) and the `inert` test fail.

- [ ] **Step 3: The code**

In `web/src/App.tsx`, replace:

  ```tsx
        {route.name === 'setup' ? <Setup /> : route.name === 'login' ? <Login /> : <Shell route={route} />}
  ```

with:

  ```tsx
        {/* While the step-up dialog is open, nothing under it can be focused
            or clicked. */}
        <div className="page" inert={waiting !== null} style={{ display: 'contents' }}>
          {route.name === 'setup' ? <Setup /> : route.name === 'login' ? <Login /> : <Shell route={route} />}
        </div>
  ```

In `web/src/api/errors.ts`, replace:

  ```ts
    unauthenticated: 'Sign in first.',
  }
  ```

with:

  ```ts
    unauthenticated: 'Sign in first.',
    too_many_codes: 'Sixteen pairing codes are live already: wait for one to expire.',
    name_taken: 'Another hat has this name.',
    hat_purging: 'This hat is being purged.',
    hat_is_default:
      'This hat is the default for new hosts, or a host’s default hat: make another hat that default first.',
    resolve_unsupported: 'This host cannot resolve paths yet: update hennery on it.',
    no_answer: 'The host did not answer in time. Try again.',
    busy: 'The host is busy. Try again.',
    too_many_subscriptions: 'Remove a device before adding another: 32 at most.',
    endpoint_taken: 'This browser receives notifications for another account.',
  }
  ```

In `web/src/api/errors.ts`, replace:

  ```ts
      super(MESSAGES[code] ?? serverMessage)
  ```

with:

  ```ts
      super(Object.hasOwn(MESSAGES, code) ? MESSAGES[code] : serverMessage)
  ```

Create `web/src/components/ConfirmDialog.tsx`:

  ```tsx
  // A confirmation before an action that cannot be taken back (frontend spec
  // §8: renaming or revoking a host, purging a hat, removing a passkey or a
  // device). The action runs from here: while it runs the dialog stays open,
  // so a step-up dialog can open over it, and a refusal is shown in it, as
  // text. Focus moves in, stays in (Tab cycles within it, and a click on the
  // backdrop keeps it), and goes back where it was when it closes, or to
  // `returnFocus` when that is gone.
  import { useEffect, useId, useRef, useState, type KeyboardEvent, type MouseEvent, type ReactNode } from 'react'
  import { messageOf } from '../api/errors'
  import { Text } from '../lib/text'

  interface Props {
    title: string
    children: ReactNode
    /** The button that runs `action`. */
    confirm: string
    danger?: boolean
    /** The action cannot run now; the body says why. */
    disabled?: boolean
    /** Runs on the confirm button; the dialog closes when it resolves. */
    action: () => Promise<void>
    onClose: () => void
    /** Where focus goes on close when what opened the dialog is gone (the
     *  action removed it, or it was gone before the dialog opened). Read on
     *  close. */
    returnFocus?: () => HTMLElement | null
  }

  export default function ConfirmDialog({ title, children, confirm, danger, disabled, action, onClose, returnFocus }: Props) {
    const [busy, setBusy] = useState(false)
    const [error, setError] = useState<string | null>(null)
    const dialog = useRef<HTMLDivElement>(null)
    const live = useRef(true)
    const titleId = useId()
    const fallback = useRef(returnFocus)
    fallback.current = returnFocus

    useEffect(() => {
      live.current = true
      const before = document.activeElement as HTMLElement | null
      dialog.current?.querySelector<HTMLElement>('[data-autofocus]')?.focus()
      return () => {
        live.current = false
        // An opener removed as the dialog opened leaves focus on the body.
        const gone = !before || before === document.body || !before.isConnected
        const back = gone ? fallback.current?.() : before
        back?.focus?.()
      }
    }, [])

    // A click on the backdrop would move focus to the body, out of the trap
    // and out of reach of Escape: it is kept in the dialog.
    const holdFocus = (e: MouseEvent<HTMLDivElement>) => {
      if (e.target !== e.currentTarget) return
      e.preventDefault()
      dialog.current?.focus()
    }

    const run = async () => {
      // Its buttons are disabled while the action runs: focus is held by the
      // dialog itself, so a step-up opened over it returns focus here.
      dialog.current?.focus()
      setBusy(true)
      setError(null)
      try {
        await action()
        if (live.current) onClose()
      } catch (err) {
        if (live.current) setError(messageOf(err))
      } finally {
        if (live.current) setBusy(false)
      }
    }

    // Tab and Shift+Tab cycle through the dialog's own controls: nothing
    // behind the overlay can be reached while it is open.
    const trap = (e: KeyboardEvent<HTMLDivElement>) => {
      if (e.key === 'Escape' && !busy) {
        onClose()
        return
      }
      if (e.key !== 'Tab') return
      const controls = [
        ...(dialog.current?.querySelectorAll<HTMLElement>(
          'button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ) ?? []),
      ]
      if (controls.length === 0) {
        e.preventDefault()
        return
      }
      const first = controls[0]
      const last = controls[controls.length - 1]
      const at = document.activeElement
      if (e.shiftKey && (at === first || !controls.includes(at as HTMLElement))) {
        e.preventDefault()
        last.focus()
      } else if (!e.shiftKey && (at === last || !controls.includes(at as HTMLElement))) {
        e.preventDefault()
        first.focus()
      }
    }

    return (
      <div className="overlay" onKeyDown={trap} onMouseDown={holdFocus}>
        <div className="modal dialog" role="dialog" aria-modal="true" aria-labelledby={titleId} ref={dialog} tabIndex={-1}>
          <div className="modal-head">
            <h2 className="modal-title" id={titleId}>
              {title}
            </h2>
          </div>
          <div className="modal-body">
            {children}
            {error && (
              <p className="form-error" role="alert">
                <Text>{error}</Text>
              </p>
            )}
          </div>
          <div className="modal-foot">
            <button type="button" className="btn btn-ghost" onClick={onClose} disabled={busy} data-autofocus>
              Cancel
            </button>
            <span className="spacer" />
            <button type="button" className={danger ? 'btn btn-danger' : 'btn btn-primary'} onClick={run} disabled={busy || disabled}>
              {confirm}
            </button>
          </div>
        </div>
      </div>
    )
  }
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
  import { capabilities, logOut } from '../api/auth'
  ```

with:

  ```tsx
  import { capabilities } from '../api/auth'
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
  import { Link, navigate, type Route } from '../router'
  import Placeholder from '../screens/Placeholder'
  ```

with:

  ```tsx
  import { Link, type Route } from '../router'
  import Placeholder from '../screens/Placeholder'
  import SignOut from './SignOut'
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
    const signOut = async () => {
      try {
        await logOut(client)
      } finally {
        navigate('/login')
      }
    }

  ```

with:

  ```tsx
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
            <button type="button" className="btn btn-ghost btn-sm" onClick={signOut}>
              Sign out
            </button>
  ```

with:

  ```tsx
            <SignOut />
  ```

Create `web/src/components/SignOut.tsx`:

  ```tsx
  // Sign out (kernel spec §3.2): `POST /api/auth/logout` ends this browser's
  // session, and only once it has does the browser go to the login screen. A
  // sign-out that failed (no network, a refusal) says so and stays: the
  // cookie is still good, and showing the login screen would claim
  // otherwise.
  import { useState } from 'react'
  import { logOut } from '../api/auth'
  import { messageOf } from '../api/errors'
  import { Text } from '../lib/text'
  import { useClient } from '../app-client'
  import { navigate } from '../router'

  export default function SignOut({ className = 'btn btn-ghost btn-sm' }: { className?: string }) {
    const client = useClient()
    const [busy, setBusy] = useState(false)
    const [error, setError] = useState<string | null>(null)

    const signOut = async () => {
      setBusy(true)
      setError(null)
      try {
        await logOut(client)
      } catch (err) {
        setError(messageOf(err))
        setBusy(false)
        return
      }
      navigate('/login')
    }

    return (
      <>
        <button type="button" className={className} onClick={signOut} disabled={busy}>
          Sign out
        </button>
        {error && (
          <p className="form-error sign-out-error" role="alert">
            Not signed out: this browser is still signed in. <Text>{error}</Text>
          </p>
        )}
      </>
    )
  }
  ```

Create `web/src/components/When.tsx`:

  ```tsx
  // A server time (RFC 3339), shown in the browser's own locale and zone, with
  // the exact value a hover away. One the browser cannot read is shown as
  // the server sent it, escaped.
  import { visible } from '../lib/text'

  export default function When({ at }: { at: string }) {
    const date = new Date(at)
    const shown = visible(at)
    return (
      <time dateTime={shown} title={shown}>
        {Number.isNaN(date.getTime()) ? shown : date.toLocaleString()}
      </time>
    )
  }
  ```

Create `web/src/hooks/useResource.ts`:

  ```ts
  // One server read for a screen: loads on mount and when `key` changes, can
  // be loaded again, and can be replaced by what a change answered (the
  // server's own copy, never a local guess), as a function of what it holds
  // so that two changes landing together both stay. A new key drops what the
  // old one read; reading the same key again keeps it on screen meanwhile. A
  // 401 has sent the browser to sign in already, so it is no error here.
  import { useCallback, useEffect, useRef, useState, type Dispatch, type SetStateAction } from 'react'
  import { Unauthenticated, messageOf } from '../api/errors'

  export interface Resource<T> {
    data: T | null
    error: string | null
    reload: () => void
    set: Dispatch<SetStateAction<T | null>>
  }

  export function useResource<T>(load: () => Promise<T>, key: unknown[] = []): Resource<T> {
    const [data, setData] = useState<T | null>(null)
    const [error, setError] = useState<string | null>(null)
    const [attempt, setAttempt] = useState(0)
    const lastKey = useRef(key)

    useEffect(() => {
      let live = true
      const before = lastKey.current
      lastKey.current = key
      if (before.length !== key.length || before.some((k, i) => !Object.is(k, key[i]))) setData(null)
      setError(null)
      load().then(
        (value) => live && setData(value),
        (err) => {
          if (live && !(err instanceof Unauthenticated)) setError(messageOf(err))
        },
      )
      return () => {
        live = false
      }
      // `load` is a new closure each render; `key` names what it depends on.
    }, [attempt, ...key])

    const reload = useCallback(() => setAttempt((n) => n + 1), [])
    return { data, error, reload, set: setData }
  }
  ```

Create `web/src/lib/text.tsx`:

  ```tsx
  // Server text shown as text (frontend spec §6.4, §8): names, labels, user
  // agents and paths come from hosts, browsers and the operator, and may hold
  // characters that are invisible or that reorder what follows them. Each
  // such character is shown as a visible escape instead, and the result sits
  // in a <bdi> so it cannot reorder the text around it (plan 3a: the
  // collector refuses only a hand-picked set of format characters in host
  // names; the whole category is escaped here).

  /** Format (Cf: bidi controls, zero-width characters, the soft hyphen, the
   *  BOM, tag characters), control (Cc), line and paragraph separators (Zl,
   *  Zp), and every other character Unicode says renders as nothing
   *  (Default_Ignorable_Code_Point: the Hangul fillers, the variation
   *  selectors, the combining grapheme joiner). Combining marks (Mn) and
   *  private-use characters stay: scripts need the first, and the second
   *  render as a visible glyph. */
  export const HIDDEN = /[\p{Cf}\p{Cc}\p{Zl}\p{Zp}\p{Default_Ignorable_Code_Point}]/u
  const ALL_HIDDEN = new RegExp(HIDDEN.source, 'gu')

  /** `text` with every hidden character written as `<U+XXXX>`. */
  export function visible(text: string): string {
    return text.replace(ALL_HIDDEN, (c) => `<U+${c.codePointAt(0)!.toString(16).toUpperCase().padStart(4, '0')}>`)
  }

  /** Server text, escaped and isolated: the one way 4d renders it. */
  export function Text({ children, className }: { children: string; className?: string }) {
    return <bdi className={className}>{visible(children)}</bdi>
  }
  ```


- [ ] **Step 4: Run the checks**

  Run: `nix develop -c sh -c 'pnpm --dir web typecheck && pnpm --dir web test'`
  Expected: all pass. 193 Vitest tests.

- [ ] **Step 5: Revert-probes** (each must fail the named test file; restore after each; all were run)
  - `text.tsx`: drop `\p{Cf}`, then `\p{Cc}`, then `\p{Zl}\p{Zp}`, then `\p{Default_Ignorable_Code_Point}` from `HIDDEN` (each fails `text.test.tsx`); `<bdi>` → `<span>` (`text.test.tsx`); render `children` unescaped (`Hosts.test.tsx`'s escaped name).
  - `security.test.ts`'s guard: a literal zero-width space in a comment of `When.tsx` fails it.
  - `errors.ts`: `MESSAGES[code] ?? serverMessage` (the inherited names); each of the nine new entries dropped in turn (its row of "explains %s in its own words").
  - `App.tsx`: drop `inert={…}` (`StepUpDialog.test.tsx`).
  - `SignOut.tsx`: drop the `return` after a failure, so it navigates anyway; drop `navigate('/login')` (each fails `SignOut.test.tsx`).
  - `ConfirmDialog.tsx`: Cancel runs the action; close on a failure; drop the focus return; drop `|| disabled`; drop the Tab trap; drop the dialog's own focus while the action runs (each fails `Hosts.test.tsx` or `Hats.test.tsx`); the Tab trap's selector back to buttons, links and inputs only; drop the backdrop's focus hold; a fixed title id; drop the `returnFocus` fallback (each fails `ConfirmDialog.test.tsx`).
  - `When.tsx`: show the server's time unescaped (`When.test.tsx`).
  - `useResource.ts`: keep the old key's data; drop it on a reload of the same key; `set` taking only a value (each fails `useResource.test.ts`).

- [ ] **Step 6: Commit**

  `git add -A && git commit -m "feat(web): server text shown escaped, a confirm dialog, and a sign-out that says when it failed"`

---

### Task 2: Hosts

- [ ] **Step 1: Write the tests**

Create `web/src/lib/manage.test.ts`:

  ```ts
  import { describe, expect, it } from 'vitest'
  import { hat, host, preview } from '../test-fixtures'
  import { count, hostState, isDefault, purgeState, safeColour } from './manage'

  describe('hostState', () => {
    it('is online when connected', () => {
      expect(hostState(host())).toBe('online')
    })

    it('is offline when not connected', () => {
      expect(hostState(host({ connected: false }))).toBe('offline')
    })

    it('is revoked when revoked, connected or not', () => {
      expect(hostState(host({ revoked_at: '2026-10-02T11:00:00Z' }))).toBe('revoked')
      expect(hostState(host({ connected: false, revoked_at: '2026-10-02T11:00:00Z' }))).toBe('revoked')
    })
  })

  describe('purgeState', () => {
    it('is default for the default for new hosts', () => {
      expect(purgeState(hat({ default_for_new_hosts: true }), [], preview())).toBe('default')
    })

    it('is default for a host’s default hat, a revoked host’s included', () => {
      const revoked = host({ default_hat_id: 'hat-b', revoked_at: '2026-10-02T11:00:00Z' })
      expect(purgeState(hat(), [revoked], preview())).toBe('default')
    })

    it('is running while a session may run, after the default check', () => {
      expect(purgeState(hat(), [host()], preview({ running: ['s-1'] }))).toBe('running')
      expect(purgeState(hat({ default_for_new_hosts: true }), [], preview({ running: ['s-1'] }))).toBe('default')
    })

    it('is resume for a hat whose purge began', () => {
      expect(purgeState(hat({ purging: true }), [host()], preview({ purging: true }))).toBe('resume')
    })

    it('is ready otherwise', () => {
      expect(purgeState(hat(), [host()], preview())).toBe('ready')
    })
  })

  describe('isDefault', () => {
    it('is false for a hat no host and no setting names', () => {
      expect(isDefault(hat(), [host()])).toBe(false)
    })
  })

  describe('safeColour', () => {
    it('takes a lowercase #rrggbb', () => {
      expect(safeColour('#0a1b2c')).toBe('#0a1b2c')
    })

    it.each(['#0A1B2C', 'red', '#abc', '#0a1b2c;background:url(x)', 'url(x)', ''])('refuses %j', (value) => {
      expect(safeColour(value)).toBeUndefined()
    })
  })

  describe('count', () => {
    it('says one and many', () => {
      expect(count(1, 'session')).toBe('1 session')
      expect(count(0, 'session')).toBe('0 sessions')
      expect(count(2, 'entry', 'entries')).toBe('2 entries')
    })
  })
  ```

Create `web/src/screens/Hosts.test.tsx`:

  ```tsx
  import { act, render, screen, waitFor, within } from '@testing-library/react'
  import userEvent from '@testing-library/user-event'
  import { afterEach, describe, expect, it, vi } from 'vitest'
  import App from '../App'
  import { formatLeft } from '../components/Pairing'
  import { hat, host } from '../test-fixtures'
  import { FULL, json, stubServer, type Answer } from '../test-server'
  import { loadManageCss, shortTargets } from '../test-targets'

  const STEP_UP = json(403, { code: 'step_up_required', message: 'm' })
  const STEPPED_UP = new Response(null, { status: 204 })
  const HATS = [hat({ id: 'hat-a', name: 'Personal', default_for_new_hosts: true }), hat()]
  const SETTINGS = { public_url: 'https://hennery.example.com' }

  afterEach(() => {
    history.replaceState(null, '', '/')
    vi.useRealTimers()
  })

  function open(routes: Record<string, Answer | Answer[]>) {
    history.replaceState(null, '', '/hosts')
    const server = stubServer({
      'GET /api/capabilities': json(200, FULL),
      'GET /api/hats': json(200, HATS),
      'GET /api/settings': json(200, SETTINGS),
      'POST /api/auth/step-up/password': STEPPED_UP,
      ...routes,
    })
    render(<App fetchImpl={server.fetch} />)
    return server
  }

  async function stepUp() {
    const dialog = await screen.findByRole('dialog', { name: 'This needs a fresh confirmation' })
    await userEvent.type(within(dialog).getByLabelText('Your password'), 'correct horse battery')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Confirm' }))
  }

  function card(name: string) {
    return screen.getByRole('listitem', { name })
  }

  const sent = (server: ReturnType<typeof stubServer>, method: string, path: string) =>
    server.sent.filter((s) => s.method === method && s.path === path)

  describe('the host list', () => {
    it('shows each host’s state, versions and default hat, revoked ones as revoked and without actions', async () => {
      open({
        'GET /api/hosts': json(200, [
          host({ host_id: 'host-1', name: 'laptop' }),
          host({ host_id: 'host-2', name: 'build box', connected: false, host_version: '0.2.0', default_hat_id: 'hat-b' }),
          host({ host_id: 'host-3', name: 'old box', connected: false, revoked_at: '2026-10-02T11:00:00Z' }),
        ]),
      })
      const laptop = await screen.findByRole('listitem', { name: 'laptop' })
      expect(within(laptop).getByText('Online')).toBeInTheDocument()
      expect(within(laptop).getByText('linux-x64')).toBeInTheDocument()
      await waitFor(() => expect(within(laptop).getAllByText('Personal').length).toBeGreaterThan(0))
      const build = card('build box')
      expect(within(build).getByText('Offline')).toBeInTheDocument()
      expect(within(build).getByText('0.2.0')).toBeInTheDocument()
      const old = card('old box')
      expect(within(old).getByText('Revoked')).toBeInTheDocument()
      expect(within(old).queryByRole('button')).toBeNull()
      expect(within(laptop).getByRole('button', { name: 'Revoke' })).toBeInTheDocument()
    })

    it('renders a host name as escaped, isolated text', async () => {
      open({ 'GET /api/hosts': json(200, [host({ name: 'box\u202Etxt.exe<b>' })]) })
      const title = await screen.findByRole('heading', { name: 'box<U+202E>txt.exe<b>' })
      expect(title.querySelector('bdi')).not.toBeNull()
      expect(title.querySelector('b')).toBeNull()
    })

    it('says so when no host is paired', async () => {
      open({ 'GET /api/hosts': json(200, []) })
      expect(await screen.findByText('No host is paired yet.')).toBeInTheDocument()
    })

    it('says so when the hats cannot be read', async () => {
      open({ 'GET /api/hosts': json(200, [host()]), 'GET /api/hats': json(500, { code: 'internal', message: 'the hats are away' }) })
      expect(await screen.findByRole('alert')).toHaveTextContent('the hats are away')
    })

    it('has every button and picker at least 44 px tall under 768 px', async () => {
      const unload = loadManageCss()
      try {
        open({
          'GET /api/hosts': json(200, [host()]),
          'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: new Date(Date.now() + 600_000).toISOString() }),
        })
        const laptop = await screen.findByRole('listitem', { name: 'laptop' })
        await waitFor(() => expect(within(laptop).getByRole('option', { name: 'Work' })).toBeInTheDocument())
        const page = document.querySelector('.manage')!
        expect(shortTargets(page)).toEqual([])
        await userEvent.click(screen.getByRole('button', { name: 'Add host' }))
        expect(await screen.findByLabelText('Pairing command')).toBeInTheDocument()
        await userEvent.click(within(laptop).getByRole('button', { name: 'Rename' }))
        expect(shortTargets(page)).toEqual([])
      } finally {
        unload()
      }
    })
  })

  describe('adding a host', () => {
    it('steps up, then shows the code and the exact command with the public URL', async () => {
      const expires = new Date(Date.now() + 600_000).toISOString()
      const server = open({
        'GET /api/hosts': json(200, [host()]),
        'POST /api/hosts/pairing-codes': [STEP_UP, json(201, { code: 'ABCD-EFGH', expires_at: expires })],
      })
      await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
      await stepUp()
      const command = await screen.findByLabelText('Pairing command')
      expect(command).toHaveTextContent('hennery host join https://hennery.example.com ABCD-EFGH')
      expect(sent(server, 'POST', '/api/hosts/pairing-codes')).toHaveLength(2)
      expect(screen.getByRole('timer').textContent).toMatch(/^(10:00|9:5\d)$/)
    })

    it('drops the code at its expiry, and keeps it out of storage and the title', async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true })
      const expires = new Date(Date.now() + 600_000).toISOString()
      open({
        'GET /api/hosts': json(200, [host()]),
        'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: expires }),
      })
      await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
      expect(await screen.findByLabelText('Pairing command')).toBeInTheDocument()
      await act(async () => {
        vi.advanceTimersByTime(599_000)
      })
      expect(screen.getByRole('timer')).toHaveTextContent(/^0:0\d$/)
      await act(async () => {
        vi.advanceTimersByTime(2_000)
      })
      expect(await screen.findByText('The code has expired. Add a host again for a new one.')).toBeInTheDocument()
      expect(document.body.textContent).not.toContain('ABCD-EFGH')
      expect(JSON.stringify({ ...localStorage })).not.toContain('ABCD')
      expect(JSON.stringify({ ...sessionStorage })).not.toContain('ABCD')
      expect(document.title).not.toContain('ABCD')
      expect(location.href).not.toContain('ABCD')
    })

    it('offers a new code once the last one expired, counting from the start again', async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true })
      const expires = new Date(Date.now() + 600_000).toISOString()
      open({
        'GET /api/hosts': json(200, [host()]),
        'POST /api/hosts/pairing-codes': [
          json(201, { code: 'ABCD-EFGH', expires_at: expires }),
          json(201, { code: 'JKLM-NPQR', expires_at: expires }),
        ],
      })
      const add = await screen.findByRole('button', { name: 'Add host' })
      await userEvent.click(add)
      expect(await screen.findByLabelText('Pairing command')).toBeInTheDocument()
      expect(add).toBeDisabled()
      await act(async () => {
        vi.advanceTimersByTime(601_000)
      })
      expect(await screen.findByText('The code has expired. Add a host again for a new one.')).toBeInTheDocument()
      expect(add).toBeEnabled()
      await userEvent.click(add)
      expect(await screen.findByLabelText('Pairing command')).toHaveTextContent('JKLM-NPQR')
      expect(screen.getByRole('timer').textContent).toMatch(/^(10:00|9:5\d)$/)
      expect(document.body.textContent).not.toContain('ABCD-EFGH')
    })

    it('reads the hosts once at a time while it waits, however slow a read is', async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true })
      const server = open({
        // The list, the read before the mint, then a poll that never answers.
        'GET /api/hosts': [json(200, [host()]), json(200, [host()]), () => new Promise<Response>(() => {})],
        'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: new Date(Date.now() + 600_000).toISOString() }),
      })
      await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
      expect(await screen.findByLabelText('Pairing command')).toBeInTheDocument()
      await act(async () => {
        vi.advanceTimersByTime(3_100)
      })
      await act(async () => {
        vi.advanceTimersByTime(3_000)
      })
      await act(async () => {
        vi.advanceTimersByTime(3_000)
      })
      expect(sent(server, 'GET', '/api/hosts')).toHaveLength(3)
    })

    it('ends the code when the new host pairs, and lists it', async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true })
      const expires = new Date(Date.now() + 600_000).toISOString()
      open({
        // The list, the read before the mint, then the polls.
        'GET /api/hosts': [json(200, [host()]), json(200, [host()]), json(200, [host(), host({ host_id: 'host-9', name: 'new box' })])],
        'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: expires }),
      })
      await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
      expect(await screen.findByLabelText('Pairing command')).toBeInTheDocument()
      await act(async () => {
        vi.advanceTimersByTime(3_100)
      })
      expect(await screen.findByText(/^Paired:/)).toHaveTextContent('Paired: new box')
      expect(document.body.textContent).not.toContain('ABCD-EFGH')
      expect(await screen.findByRole('listitem', { name: 'new box' })).toBeInTheDocument()
      // The code is spent: another can be minted.
      expect(screen.getByRole('button', { name: 'Add host' })).toBeEnabled()
    })

    it('tells a new host from those before it even when the list never loaded', async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true })
      open({
        'GET /api/hosts': [json(500, { code: 'internal', message: 'm' }), json(200, [host()])],
        'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: new Date(Date.now() + 600_000).toISOString() }),
      })
      await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
      expect(await screen.findByLabelText('Pairing command')).toBeInTheDocument()
      await act(async () => {
        vi.advanceTimersByTime(3_100)
      })
      expect(screen.queryByText(/^Paired:/)).toBeNull()
      expect(screen.getByLabelText('Pairing command')).toHaveTextContent('ABCD-EFGH')
    })

    it('reads the URL and the hosts before minting, so a failed read spends no code', async () => {
      const server = open({
        'GET /api/hosts': json(200, [host()]),
        'GET /api/settings': json(500, { code: 'internal', message: 'm' }),
        'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: new Date(Date.now() + 600_000).toISOString() }),
      })
      await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
      expect(await screen.findByRole('alert')).toBeInTheDocument()
      expect(sent(server, 'POST', '/api/hosts/pairing-codes')).toHaveLength(0)
    })

    it('drops the code when the page is hidden for the back-forward cache', async () => {
      open({
        'GET /api/hosts': json(200, [host()]),
        'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: new Date(Date.now() + 600_000).toISOString() }),
      })
      await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
      expect(await screen.findByLabelText('Pairing command')).toBeInTheDocument()
      act(() => {
        window.dispatchEvent(new Event('pagehide'))
      })
      expect(document.body.textContent).not.toContain('ABCD-EFGH')
    })

    it('hides the code when the panel closes', async () => {
      open({
        'GET /api/hosts': json(200, [host()]),
        'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: new Date(Date.now() + 600_000).toISOString() }),
      })
      await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
      expect(await screen.findByLabelText('Pairing command')).toBeInTheDocument()
      await userEvent.click(screen.getByRole('button', { name: 'Close' }))
      expect(document.body.textContent).not.toContain('ABCD-EFGH')
    })

    it('shows a refusal, and no code', async () => {
      open({
        'GET /api/hosts': json(200, [host()]),
        'POST /api/hosts/pairing-codes': json(409, { code: 'too_many_codes', message: 'm' }),
      })
      await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
      expect(await screen.findByRole('alert')).toHaveTextContent('Sixteen pairing codes are live already')
      expect(screen.queryByLabelText('Pairing command')).toBeNull()
    })
  })

  describe('the countdown', () => {
    it('counts the code’s ten minutes from its arrival, whatever this clock says of its expiry', async () => {
      open({
        'GET /api/hosts': json(200, [host()]),
        // This browser's clock is an hour fast: the expiry is in its past.
        'POST /api/hosts/pairing-codes': json(201, { code: 'ABCD-EFGH', expires_at: new Date(Date.now() - 3_600_000).toISOString() }),
      })
      await userEvent.click(await screen.findByRole('button', { name: 'Add host' }))
      expect(await screen.findByRole('timer')).toHaveTextContent(/^(10:00|9:5\d)$/)
    })

    it('reads m:ss', () => {
      expect(formatLeft(600)).toBe('10:00')
      expect(formatLeft(59.9)).toBe('0:59')
      expect(formatLeft(0)).toBe('0:00')
    })
  })

  describe('changing a host', () => {
    it('renames it after a confirmation and a step-up, sending the same name again', async () => {
      const server = open({
        'GET /api/hosts': json(200, [host()]),
        'PATCH /api/hosts/host-1': [STEP_UP, json(200, host({ name: 'desk' }))],
      })
      await userEvent.click(await within(await screen.findByRole('listitem', { name: 'laptop' })).findByRole('button', { name: 'Rename' }))
      const input = screen.getByLabelText('New name')
      await userEvent.clear(input)
      await userEvent.type(input, 'desk')
      await userEvent.click(screen.getByRole('button', { name: 'Save' }))
      const confirm = await screen.findByRole('dialog', { name: 'Rename this host?' })
      expect(sent(server, 'PATCH', '/api/hosts/host-1')).toHaveLength(0)
      await userEvent.click(within(confirm).getByRole('button', { name: 'Rename' }))
      await stepUp()
      expect(await screen.findByRole('listitem', { name: 'desk' })).toBeInTheDocument()
      const patches = sent(server, 'PATCH', '/api/hosts/host-1')
      expect(patches.map((p) => p.body)).toEqual([{ name: 'desk' }, { name: 'desk' }])
      expect(screen.queryByRole('dialog')).toBeNull()
    })

    it('returns focus to Rename after a rename, done or cancelled', async () => {
      open({
        'GET /api/hosts': json(200, [host()]),
        'PATCH /api/hosts/host-1': json(200, host({ name: 'desk' })),
      })
      await userEvent.click(await within(await screen.findByRole('listitem', { name: 'laptop' })).findByRole('button', { name: 'Rename' }))
      await userEvent.type(screen.getByLabelText('New name'), ' 2')
      await userEvent.click(screen.getByRole('button', { name: 'Save' }))
      await userEvent.click(within(await screen.findByRole('dialog', { name: 'Rename this host?' })).getByRole('button', { name: 'Cancel' }))
      expect(within(card('laptop')).getByRole('button', { name: 'Rename' })).toHaveFocus()
      await userEvent.click(within(card('laptop')).getByRole('button', { name: 'Rename' }))
      await userEvent.type(screen.getByLabelText('New name'), ' 2')
      await userEvent.click(screen.getByRole('button', { name: 'Save' }))
      await userEvent.click(within(await screen.findByRole('dialog', { name: 'Rename this host?' })).getByRole('button', { name: 'Rename' }))
      await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
      expect(within(card('desk')).getByRole('button', { name: 'Rename' })).toHaveFocus()
    })

    it('sends no rename when the trimmed name is empty or unchanged', async () => {
      const server = open({ 'GET /api/hosts': json(200, [host()]) })
      const laptop = await screen.findByRole('listitem', { name: 'laptop' })
      for (const typed of ['laptop  ', '   ']) {
        await userEvent.click(within(laptop).getByRole('button', { name: 'Rename' }))
        const input = screen.getByLabelText('New name')
        await userEvent.clear(input)
        await userEvent.type(input, typed)
        await userEvent.click(screen.getByRole('button', { name: 'Save' }))
        expect(screen.queryByRole('dialog')).toBeNull()
        expect(within(laptop).getByRole('button', { name: 'Rename' })).toHaveFocus()
      }
      await userEvent.click(within(laptop).getByRole('button', { name: 'Rename' }))
      await userEvent.click(screen.getByRole('button', { name: 'Cancel' }))
      expect(within(laptop).getByRole('button', { name: 'Rename' })).toHaveFocus()
      expect(sent(server, 'PATCH', '/api/hosts/host-1')).toHaveLength(0)
    })

    it('puts focus on the card’s title once a revoke took its buttons away', async () => {
      open({
        'GET /api/hosts': json(200, [host()]),
        'DELETE /api/hosts/host-1': json(200, host({ connected: false, revoked_at: '2026-10-02T12:00:00Z' })),
      })
      await userEvent.click(await within(await screen.findByRole('listitem', { name: 'laptop' })).findByRole('button', { name: 'Revoke' }))
      await userEvent.click(within(await screen.findByRole('dialog', { name: 'Revoke this host?' })).getByRole('button', { name: 'Revoke' }))
      await waitFor(() => expect(within(card('laptop')).getByText('Revoked')).toBeInTheDocument())
      expect(within(card('laptop')).getByRole('heading', { name: 'laptop' })).toHaveFocus()
    })

    it('revokes it after a confirmation that says its agents stop when it next connects', async () => {
      const server = open({
        'GET /api/hosts': json(200, [host()]),
        'DELETE /api/hosts/host-1': [STEP_UP, json(200, host({ connected: false, revoked_at: '2026-10-02T12:00:00Z' }))],
      })
      await userEvent.click(await within(await screen.findByRole('listitem', { name: 'laptop' })).findByRole('button', { name: 'Revoke' }))
      const confirm = await screen.findByRole('dialog', { name: 'Revoke this host?' })
      expect(confirm).toHaveTextContent('Agents it is running stop only when it next connects')
      await userEvent.click(within(confirm).getByRole('button', { name: 'Revoke' }))
      await stepUp()
      await waitFor(() => expect(within(card('laptop')).getByText('Revoked')).toBeInTheDocument())
      expect(sent(server, 'DELETE', '/api/hosts/host-1')).toHaveLength(2)
    })

    it('sends nothing when the confirmation is cancelled', async () => {
      const server = open({ 'GET /api/hosts': json(200, [host()]) })
      await userEvent.click(await within(await screen.findByRole('listitem', { name: 'laptop' })).findByRole('button', { name: 'Revoke' }))
      await userEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Cancel' }))
      expect(screen.queryByRole('dialog')).toBeNull()
      expect(sent(server, 'DELETE', '/api/hosts/host-1')).toHaveLength(0)
      expect(within(card('laptop')).getByRole('button', { name: 'Revoke' })).toHaveFocus()
    })

    it('keeps focus inside the confirmation, Tab cycling through its buttons', async () => {
      open({ 'GET /api/hosts': json(200, [host()]) })
      await userEvent.click(await within(await screen.findByRole('listitem', { name: 'laptop' })).findByRole('button', { name: 'Revoke' }))
      const confirm = await screen.findByRole('dialog', { name: 'Revoke this host?' })
      expect(within(confirm).getByRole('button', { name: 'Cancel' })).toHaveFocus()
      await userEvent.tab()
      expect(within(confirm).getByRole('button', { name: 'Revoke' })).toHaveFocus()
      await userEvent.tab()
      expect(within(confirm).getByRole('button', { name: 'Cancel' })).toHaveFocus()
      await userEvent.tab({ shift: true })
      expect(within(confirm).getByRole('button', { name: 'Revoke' })).toHaveFocus()
    })

    it('keeps the confirmation open with the reason when the step-up is cancelled', async () => {
      const server = open({ 'GET /api/hosts': json(200, [host()]), 'DELETE /api/hosts/host-1': STEP_UP })
      await userEvent.click(await within(await screen.findByRole('listitem', { name: 'laptop' })).findByRole('button', { name: 'Revoke' }))
      await userEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Revoke' }))
      const stepUpDialog = await screen.findByRole('dialog', { name: 'This needs a fresh confirmation' })
      await userEvent.click(within(stepUpDialog).getByRole('button', { name: 'Cancel' }))
      expect(await screen.findByRole('alert')).toHaveTextContent('Not confirmed.')
      const confirm = screen.getByRole('dialog', { name: 'Revoke this host?' })
      expect(sent(server, 'DELETE', '/api/hosts/host-1')).toHaveLength(1)
      // Focus came back into the confirmation, so Escape still closes it.
      expect(confirm.contains(document.activeElement)).toBe(true)
      await userEvent.keyboard('{Escape}')
      expect(screen.queryByRole('dialog')).toBeNull()
    })

    it('changes its default hat after a confirmation that warns about parked sessions', async () => {
      const server = open({
        'GET /api/hosts': json(200, [host()]),
        'PATCH /api/hosts/host-1': [STEP_UP, json(200, host({ default_hat_id: 'hat-b' }))],
      })
      const laptop = await screen.findByRole('listitem', { name: 'laptop' })
      await waitFor(() => expect(within(laptop).getByRole('option', { name: 'Work' })).toBeInTheDocument())
      await userEvent.selectOptions(within(laptop).getByLabelText('Default hat'), 'hat-b')
      const confirm = await screen.findByRole('dialog', { name: 'Change this host’s default hat?' })
      expect(confirm).toHaveTextContent('resuming one is refused until it is re-assigned')
      await userEvent.click(within(confirm).getByRole('button', { name: 'Change' }))
      await stepUp()
      await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
      expect(sent(server, 'PATCH', '/api/hosts/host-1').map((p) => p.body)).toEqual([
        { default_hat_id: 'hat-b' },
        { default_hat_id: 'hat-b' },
      ])
    })
  })
  ```

Create `web/src/test-fixtures.ts`:

  ```ts
  // Server answers for the management screens' tests: one of each item, with
  // every field set (an absent optional field makes for false passes), and
  // overridable per test.
  import type { HatItem, HostItem, PurgePreview } from './generated/protocol'

  export function host(over: Partial<HostItem> = {}): HostItem {
    return {
      host_id: 'host-1',
      name: 'laptop',
      platform: 'linux-x64',
      host_version: '0.1.0',
      capabilities: ['resolve_path'],
      default_hat_id: 'hat-a',
      workspace_roots: [],
      connected: true,
      created_at: '2026-10-01T10:00:00Z',
      last_seen_at: '2026-10-02T10:00:00Z',
      ...over,
    }
  }

  export function hat(over: Partial<HatItem> = {}): HatItem {
    return {
      id: 'hat-b',
      name: 'Work',
      colour: '#336699',
      created_at: '2026-10-01T10:00:00Z',
      default_for_new_hosts: false,
      purging: false,
      ...over,
    }
  }

  export function preview(over: Partial<PurgePreview> = {}): PurgePreview {
    return {
      hat_id: 'hat-b',
      purging: false,
      sessions: 3,
      running: [],
      rules: 2,
      recents: 1,
      unassigned: [],
      unassigned_count: 0,
      ...over,
    }
  }
  ```

Create `web/src/test-targets.ts`:

  ```ts
  // The 44 px rule under 768 px (frontend spec §10), checked in jsdom: it does
  // no layout and applies no media query, so this reads `manage.css` itself
  // and asks which of its narrow-screen rules match an element.
  import { readFileSync } from 'node:fs'
  import { join } from 'node:path'

  /** Puts `manage.css` in the document (Vitest loads no CSS); returns a
   *  remover. */
  export function loadManageCss(): () => void {
    const style = document.createElement('style')
    style.textContent = readFileSync(join(process.cwd(), 'src/manage.css'), 'utf8')
    document.head.append(style)
    return () => style.remove()
  }

  /** Whether a narrow-screen rule gives `el` a `min-height` of 44 px or
   *  more. */
  export function tallEnough(el: Element): boolean {
    for (const sheet of document.styleSheets) {
      for (const rule of sheet.cssRules) {
        if (!(rule instanceof CSSMediaRule) || !/max-width:\s*767px/.test(rule.media.mediaText)) continue
        for (const inner of rule.cssRules) {
          if (!(inner instanceof CSSStyleRule) || !el.matches(inner.selectorText)) continue
          if (parseFloat(inner.style.minHeight) >= 44) return true
        }
      }
    }
    return false
  }

  /** The controls in `root` too short to tap under 768 px, named. */
  export function shortTargets(root: Element): string[] {
    return [...root.querySelectorAll('button, select, input[type="color"]')]
      .filter((el) => !tallEnough(el))
      .map((el) => el.getAttribute('aria-label') ?? (el.textContent?.trim() || el.closest('label')?.textContent || el.tagName))
  }
  ```


- [ ] **Step 2: Run them, and see them fail**

  Run: `nix develop -c pnpm --dir web test`
  Expected: `manage.test.ts` and `Hosts.test.tsx` fail: `./manage` and `../components/Pairing` do not exist.

- [ ] **Step 3: The code**

Create `web/src/api/manage.ts`:

  ```ts
  // The routes the Hosts and Hats screens use (kernel spec §4, §5, §8), typed.
  // Which of them need a fresh step-up is the server's to say: the client
  // opens the dialog on 403 `step_up_required` and retries once.
  import type {
    CreateHatRequest,
    HatItem,
    HatResolution,
    HostItem,
    PairingCodeResponse,
    PathRuleInput,
    PathRuleItem,
    PurgePreview,
    PurgeResult,
    SettingsResponse,
    UpdateHatRequest,
    UpdateHostRequest,
  } from '../generated/protocol'
  import type { Client } from './client'

  const id = (value: string) => encodeURIComponent(value)

  export function hosts(client: Client): Promise<HostItem[]> {
    return client.request('GET', '/api/hosts')
  }

  export function settings(client: Client): Promise<SettingsResponse> {
    return client.request('GET', '/api/settings')
  }

  /** Step-up. */
  export function mintPairingCode(client: Client): Promise<PairingCodeResponse> {
    return client.request('POST', '/api/hosts/pairing-codes')
  }

  /** Rename, or change the default hat. Step-up. */
  export function updateHost(client: Client, hostId: string, change: UpdateHostRequest): Promise<HostItem> {
    return client.request('PATCH', `/api/hosts/${id(hostId)}`, change)
  }

  /** Step-up. */
  export function revokeHost(client: Client, hostId: string): Promise<HostItem> {
    return client.request('DELETE', `/api/hosts/${id(hostId)}`)
  }

  export function hats(client: Client): Promise<HatItem[]> {
    return client.request('GET', '/api/hats')
  }

  export function createHat(client: Client, hat: CreateHatRequest): Promise<HatItem> {
    return client.request('POST', '/api/hats', hat)
  }

  /** Rename, recolour, or make the default for new hosts. Step-up. */
  export function updateHat(client: Client, hatId: string, change: UpdateHatRequest): Promise<HatItem> {
    return client.request('PATCH', `/api/hats/${id(hatId)}`, change)
  }

  export function pathRules(client: Client, hostId: string): Promise<PathRuleItem[]> {
    return client.request('GET', `/api/hosts/${id(hostId)}/path-rules`)
  }

  /** The host's whole set, replacing the one before; the answer is the set as
   *  stored. Step-up, and the host must be connected. */
  export function replacePathRules(client: Client, hostId: string, rules: PathRuleInput[]): Promise<PathRuleItem[]> {
    return client.request('PUT', `/api/hosts/${id(hostId)}/path-rules`, { rules })
  }

  /** Which hat `path` resolves to on the host, under the saved rules. */
  export function resolveHat(client: Client, hostId: string, path: string, signal?: AbortSignal): Promise<HatResolution> {
    return client.request('POST', '/api/hats/resolve', { host_id: hostId, path }, { signal })
  }

  export function purgePreview(client: Client, hatId: string): Promise<PurgePreview> {
    return client.request('GET', `/api/hats/${id(hatId)}/purge`)
  }

  /** Step-up. */
  export function purgeHat(client: Client, hatId: string): Promise<PurgeResult> {
    return client.request('POST', `/api/hats/${id(hatId)}/purge`)
  }
  ```

Create `web/src/components/Pairing.tsx`:

  ```tsx
  // "Add host" (frontend spec §8, kernel spec §4.1): a one-time pairing code,
  // minted behind a step-up, and the exact command that pairs a machine with
  // it, with a countdown to its expiry.
  //
  // The code is shown once. It lives in the Hosts screen's state and this
  // panel only: never in the address bar, the title, storage or the console.
  // It is dropped from the page when it expires, when a new host pairs, and
  // when the panel closes or unmounts. Expired or paired, the code is spent:
  // the panel tells Hosts, which drops it and offers "Add host" again while
  // the panel still says what happened.
  import { useEffect, useRef, useState } from 'react'
  import { hosts as listHosts } from '../api/manage'
  import { useClient } from '../app-client'
  import type { HostItem } from '../generated/protocol'
  import { Text } from '../lib/text'

  /** A code is dead 600 s after minting (kernel spec §4.1): the countdown
   *  never shows more, whatever the clocks say. */
  export const CODE_LIFETIME_S = 600
  /** How often the host list is read while a code waits for its host. */
  export const PAIRED_POLL_MS = 3000

  /** What "Add host" got: the minted code, where hosts reach the collector,
   *  and the hosts paired before it, read just before the mint. */
  export interface Minted {
    code: string
    publicUrl: string
    known: string[]
  }

  type Stage = { kind: 'live' } | { kind: 'expired' } | { kind: 'paired'; host: HostItem }

  interface Props {
    /** `null` once the code is spent. */
    minted: Minted | null
    onPaired: () => void
    /** The code expired or a host paired with it: it is of no use now. */
    onSpent: () => void
    onClose: () => void
  }

  export default function Pairing({ minted, onPaired, onSpent, onClose }: Props) {
    const client = useClient()
    const [stage, setStage] = useState<Stage>({ kind: 'live' })
    // Counted from the answer's arrival, on the monotonic clock: the server's
    // `expires_at` is minted on its clock, and this browser's may be off by
    // more than the lifetime. The code dies within the network's delay of
    // this.
    const [deadline] = useState(() => performance.now() + CODE_LIFETIME_S * 1000)
    const [left, setLeft] = useState(CODE_LIFETIME_S)
    const before = useRef(new Set(minted?.known))
    // The parent's callbacks, current at each tick, without restarting the
    // poll whenever the parent renders.
    const told = useRef({ onPaired, onSpent })
    told.current = { onPaired, onSpent }

    // The countdown, and the code's end at zero.
    const counting = stage.kind === 'live'
    useEffect(() => {
      if (!counting) return
      const tick = () => {
        const s = Math.max(0, Math.ceil((deadline - performance.now()) / 1000))
        setLeft(s)
        if (s === 0) {
          setStage({ kind: 'expired' })
          told.current.onSpent()
        }
      }
      tick()
      const timer = setInterval(tick, 1000)
      return () => clearInterval(timer)
    }, [counting, deadline])

    // A host that pairs while the code is live ends it here: the code is
    // spent. A tick while a read is still out is skipped, so reads never
    // pile up on a slow link.
    const waiting = stage.kind === 'live'
    useEffect(() => {
      if (!waiting) return
      let live = true
      let reading = false
      const timer = setInterval(() => {
        if (reading) return
        reading = true
        listHosts(client).then(
          (list) => {
            reading = false
            const fresh = list.find((h) => !before.current.has(h.host_id))
            if (live && fresh) {
              setStage({ kind: 'paired', host: fresh })
              told.current.onSpent()
              told.current.onPaired()
            }
          },
          () => {
            // A failed read is tried again at the next tick.
            reading = false
          },
        )
      }, PAIRED_POLL_MS)
      return () => {
        live = false
        clearInterval(timer)
      }
    }, [client, waiting])

    return (
      <section className="card pairing" aria-labelledby="pairing-title">
        <h2 id="pairing-title" className="card-title">
          Add a host
        </h2>
        {stage.kind === 'live' && minted && (
          <>
            <p>On the machine to pair, run:</p>
            <pre className="command" aria-label="Pairing command">
              <code>{`hennery host join ${minted.publicUrl} ${minted.code}`}</code>
            </pre>
            <p className="pairing-code">
              Code <code>{minted.code}</code>, valid for{' '}
              <span role="timer" aria-live="off">
                {formatLeft(left)}
              </span>
              , and once only.
            </p>
            <p className="hint">
              The host keeps its pairing in <code>--data-dir</code> (or <code>HENNERY_HOST_DATA_DIR</code>). Leave the
              code out and <code>host join</code> asks for it instead, which keeps it out of your shell history.
            </p>
            <p role="status" className="hint">
              Waiting for the host… Closing this hides the code; it stays valid until it expires.
            </p>
          </>
        )}
        {stage.kind === 'expired' && <p role="status">The code has expired. Add a host again for a new one.</p>}
        {stage.kind === 'paired' && (
          <p role="status">
            Paired: <Text>{stage.host.name}</Text>
          </p>
        )}
        <div className="card-actions">
          <button type="button" className="btn btn-ghost btn-sm" onClick={onClose}>
            Close
          </button>
        </div>
      </section>
    )
  }

  /** `m:ss`. */
  export function formatLeft(seconds: number): string {
    const s = Math.max(0, Math.floor(seconds))
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`
  }
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
  import { Link, type Route } from '../router'
  import Placeholder from '../screens/Placeholder'
  ```

with:

  ```tsx
  import { Link, type Route } from '../router'
  import Hosts from '../screens/Hosts'
  import Placeholder from '../screens/Placeholder'
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
              <Placeholder title={title} text="This view is not part of this deployment." />
            ) : route.name === 'session' ? (
  ```

with:

  ```tsx
              <Placeholder title={title} text="This view is not part of this deployment." />
            ) : route.name === 'hosts' ? (
              <Hosts />
            ) : route.name === 'session' ? (
  ```

Create `web/src/lib/manage.ts`:

  ```ts
  // What the Hosts and Hats screens decide from the server's answers. Each
  // function is a classifier with one outcome per case the screen shows.
  import type { HatItem, HostItem, PurgePreview } from '../generated/protocol'

  export type HostState = 'online' | 'offline' | 'revoked'

  /** Revoked wins over everything; then connected (and reconciled) or not. */
  export function hostState(host: HostItem): HostState {
    if (host.revoked_at !== undefined) return 'revoked'
    return host.connected ? 'online' : 'offline'
  }

  export const HOST_STATE_LABEL: Record<HostState, string> = {
    online: 'Online',
    offline: 'Offline',
    revoked: 'Revoked',
  }

  export type PurgeState = 'default' | 'running' | 'resume' | 'ready'

  /** Whether a purge can go ahead (kernel spec §5.5), in the server's order:
   *  a default hat is refused first (409 `hat_is_default`), then running
   *  sessions (409 `sessions_running`); a frozen hat resumes its purge. */
  export function purgeState(hat: HatItem, hosts: HostItem[], preview: PurgePreview): PurgeState {
    if (isDefault(hat, hosts)) return 'default'
    if (preview.running.length > 0) return 'running'
    if (preview.purging) return 'resume'
    return 'ready'
  }

  /** The default for new hosts, or the default hat of any host, revoked ones
   *  included: the server refuses to purge either. */
  export function isDefault(hat: HatItem, hosts: HostItem[]): boolean {
    return hat.default_for_new_hosts || hosts.some((h) => h.default_hat_id === hat.id)
  }

  const COLOUR = /^#[0-9a-f]{6}$/

  /** A hat's colour as the server guarantees it (`#rrggbb`, lowercase); the
   *  client trusts no more than that before it reaches a style. */
  export function safeColour(colour: string): string | undefined {
    return COLOUR.test(colour) ? colour : undefined
  }

  /** `n thing`, or `n things`. */
  export function count(n: number, one: string, many = `${one}s`): string {
    return `${n} ${n === 1 ? one : many}`
  }
  ```

Create `web/src/manage.css`:

  ```css
  /* The management screens (plan 4d): Hosts, Hats and Settings. Their own
     file, so the shell's index.css stays the shell's. Mobile-first: one
     column, every target at least 44 px under 768 px. */

  .manage { width:100%; max-width:860px; margin:0 auto; padding:24px 20px calc(32px + env(safe-area-inset-bottom)); display:flex; flex-direction:column; gap:16px; }
  .manage-head { display:flex; align-items:center; gap:12px; }
  .manage-head h1 { font-family:var(--font-display); font-weight:800; font-size:26px; letter-spacing:-.01em; margin:0; flex:1; }
  .cards { list-style:none; margin:0; padding:0; display:flex; flex-direction:column; gap:12px; }
  .card { background:var(--surface); border:1px solid var(--border); border-radius:var(--r-md); box-shadow:var(--sh-sm); padding:16px 18px; min-width:0; }
  .card-head { display:flex; align-items:center; gap:10px; flex-wrap:wrap; margin-bottom:10px; }
  .card-title { font-family:var(--font-display); font-weight:700; font-size:16px; margin:0; min-width:0; overflow-wrap:anywhere; flex:1; }
  .card-actions { display:flex; align-items:center; gap:8px; flex-wrap:wrap; margin-top:12px; }
  .card-actions .spacer { flex:1; }
  .card p { margin:0 0 10px; overflow-wrap:anywhere; }
  .notice { border-color:var(--accent); }

  .facts { display:grid; grid-template-columns:max-content minmax(0,1fr); gap:4px 14px; margin:0; font-size:14px; }
  .facts dt { color:var(--fg-muted); }
  .facts dd { margin:0; min-width:0; overflow-wrap:anywhere; }

  .state, .tag { font-size:11px; font-weight:700; letter-spacing:.06em; text-transform:uppercase; border-radius:var(--r-full); padding:3px 9px; white-space:nowrap; }
  .state-online { background:rgba(47,158,68,.13); color:#247A33; }
  .state-offline { background:var(--surface-3); color:var(--fg-muted); }
  .state-revoked { background:rgba(229,72,77,.12); color:var(--st-attn); }
  .host-revoked .card-title { color:var(--fg-muted); }
  .tag { background:var(--accent-wash); color:var(--accent); }
  .tag-warn { background:rgba(232,145,12,.14); color:#B26A00; }

  .swatch { width:18px; height:18px; border-radius:6px; flex:none; background:var(--fg-quiet); border:1px solid var(--border-strong); }
  .colour-input { width:52px; height:40px; padding:2px; border:1.5px solid var(--border-strong); border-radius:var(--r-sm); background:var(--surface-2); }

  .inline-form { display:flex; align-items:flex-end; gap:10px; flex-wrap:wrap; margin-top:8px; }
  .inline-form .field { margin:0; flex:1; min-width:180px; }
  .select-field { display:inline-flex; flex-direction:column; gap:4px; }
  .select-field .field-label { margin:0; font-size:12px; }
  .select-field select { min-height:36px; border:1.5px solid var(--border-strong); border-radius:var(--r-sm); background:var(--surface-2); color:var(--fg-1); padding:6px 8px; font:inherit; max-width:100%; }

  .command { background:var(--ink); color:#fff; border-radius:var(--r-sm); padding:12px 14px; margin:0 0 10px; overflow-x:auto; font-family:var(--font-mono); font-size:13px; }
  .pairing-code code { font-family:var(--font-mono); font-weight:700; }
  .hint { color:var(--fg-muted); font-size:13px; }
  .empty { color:var(--fg-muted); }
  .sign-out-error { margin-top:8px; font-size:13px; }

  .rules { margin-top:12px; }
  .rule-list { list-style:none; margin:0; padding:0; display:flex; flex-direction:column; gap:10px; }
  .rule { display:flex; align-items:flex-end; gap:10px; flex-wrap:wrap; }
  .rule .field { margin:0; }
  .rule-prefix { flex:1; min-width:200px; }
  .tester { margin-top:16px; border-top:1px solid var(--border); padding-top:14px; }
  .tester .field { margin:0 0 6px; }
  .purge-list { margin:0 0 12px; padding-left:20px; overflow-wrap:anywhere; }

  @media (max-width:767px) {
    .manage { padding:16px 14px calc(24px + env(safe-area-inset-bottom)); }
    .manage-head h1 { font-size:22px; }
    .manage-head .btn, .card-actions .btn, .inline-form .btn, .rule .btn { min-height:44px; }
    .select-field select, .colour-input { min-height:44px; }
  }
  ```

Create `web/src/screens/Hosts.tsx`:

  ```tsx
  // Hosts (frontend spec §8, kernel spec §4): every paired host, revoked ones
  // included, with its online state and versions; pairing a new one; and
  // renaming, re-hatting or revoking one, each confirmed, then stepped up.
  import { useEffect, useRef, useState } from 'react'
  import { messageOf } from '../api/errors'
  import { hats as listHats, hosts as listHosts, mintPairingCode, revokeHost, settings, updateHost } from '../api/manage'
  import { useClient } from '../app-client'
  import ConfirmDialog from '../components/ConfirmDialog'
  import Pairing, { type Minted } from '../components/Pairing'
  import type { HatItem, HostItem } from '../generated/protocol'
  import { useResource } from '../hooks/useResource'
  import { HOST_STATE_LABEL, hostState } from '../lib/manage'
  import { Text, visible } from '../lib/text'
  import When from '../components/When'
  import '../manage.css'

  /** Each carries where focus goes when the confirmation closes and what
   *  opened it is gone. */
  type Pending =
    | { kind: 'rename'; host: HostItem; name: string; back: () => HTMLElement | null }
    | { kind: 'default_hat'; host: HostItem; hat: HatItem; back: () => HTMLElement | null }
    | { kind: 'revoke'; host: HostItem; back: () => HTMLElement | null }

  /** The pairing panel: a new one per code (`n`), and the code itself until
   *  it is spent. */
  interface Panel {
    n: number
    minted: Minted | null
  }

  export default function Hosts() {
    const client = useClient()
    const list = useResource(() => listHosts(client))
    const hatList = useResource(() => listHats(client))
    const [panel, setPanel] = useState<Panel | null>(null)
    const minted = panel?.minted ?? null
    const [adding, setAdding] = useState(false)
    const [addError, setAddError] = useState<string | null>(null)
    const [pending, setPending] = useState<Pending | null>(null)

    // The URL and the hosts already paired are read first: a failed read then
    // spends no code, and a host that pairs is told from the ones before it
    // even when the list above never loaded.
    const add = async () => {
      setAdding(true)
      setAddError(null)
      try {
        const s = await settings(client)
        const known = (await listHosts(client)).map((h) => h.host_id)
        const code = await mintPairingCode(client)
        setPanel((p) => ({ n: (p?.n ?? 0) + 1, minted: { code: code.code, publicUrl: s.public_url, known } }))
      } catch (err) {
        setAddError(messageOf(err))
      } finally {
        setAdding(false)
      }
    }

    // A page restored from the back-forward cache must not show a code: it
    // is dropped as the page is hidden. Switching tabs to paste it keeps it.
    useEffect(() => {
      const drop = () => setPanel(null)
      window.addEventListener('pagehide', drop)
      return () => window.removeEventListener('pagehide', drop)
    }, [])

    const replace = (host: HostItem) => list.set((prev) => (prev ?? []).map((h) => (h.host_id === host.host_id ? host : h)))

    const hatName = (id: string) => hatList.data?.find((h) => h.id === id)?.name

    return (
      <div className="manage">
        <header className="manage-head">
          <h1>Hosts</h1>
          <button type="button" className="btn btn-primary btn-sm" onClick={add} disabled={adding || minted !== null}>
            Add host
          </button>
        </header>
        {addError && (
          <p className="form-error" role="alert">
            <Text>{addError}</Text>
          </p>
        )}
        {panel && (
          <Pairing
            key={panel.n}
            minted={panel.minted}
            onPaired={list.reload}
            onSpent={() => setPanel((p) => p && { ...p, minted: null })}
            onClose={() => setPanel(null)}
          />
        )}
        {list.error && (
          <p className="form-error" role="alert">
            <Text>{list.error}</Text>
          </p>
        )}
        {hatList.error && (
          <p className="form-error" role="alert">
            <Text>{hatList.error}</Text>
          </p>
        )}
        {list.data && list.data.length === 0 && <p className="empty">No host is paired yet.</p>}
        <ul className="cards" aria-label="Hosts">
          {(list.data ?? []).map((host) => (
            <HostCard
              key={host.host_id}
              host={host}
              hats={hatList.data ?? []}
              hatName={hatName(host.default_hat_id)}
              onRename={(name, back) => setPending({ kind: 'rename', host, name, back })}
              onDefaultHat={(hat, back) => setPending({ kind: 'default_hat', host, hat, back })}
              onRevoke={(back) => setPending({ kind: 'revoke', host, back })}
            />
          ))}
        </ul>
        {pending?.kind === 'rename' && (
          <ConfirmDialog
            title="Rename this host?"
            confirm="Rename"
            action={async () => replace(await updateHost(client, pending.host.host_id, { name: pending.name }))}
            onClose={() => setPending(null)}
            returnFocus={pending.back}
          >
            <p>
              <Text>{pending.host.name}</Text> becomes <Text>{pending.name}</Text>.
            </p>
          </ConfirmDialog>
        )}
        {pending?.kind === 'default_hat' && (
          <ConfirmDialog
            title="Change this host’s default hat?"
            confirm="Change"
            action={async () => replace(await updateHost(client, pending.host.host_id, { default_hat_id: pending.hat.id }))}
            onClose={() => setPending(null)}
            returnFocus={pending.back}
          >
            <p>
              Sessions on <Text>{pending.host.name}</Text> that no path rule covers will belong to{' '}
              <Text>{pending.hat.name}</Text>.
            </p>
            <p>
              Parked or closed sessions there that no rule covers keep their hat: resuming one is refused until it is
              re-assigned.
            </p>
          </ConfirmDialog>
        )}
        {pending?.kind === 'revoke' && (
          <ConfirmDialog
            title="Revoke this host?"
            confirm="Revoke"
            danger
            action={async () => replace(await revokeHost(client, pending.host.host_id))}
            onClose={() => setPending(null)}
            returnFocus={pending.back}
          >
            <p>
              <Text>{pending.host.name}</Text> can no longer connect, and its sessions are parked. Pairing it again
              needs a new code.
            </p>
            <p>Agents it is running stop only when it next connects: it is then told it is revoked.</p>
          </ConfirmDialog>
        )}
      </div>
    )
  }

  /** Where focus goes when a confirmation closes and what opened it is
   *  gone. */
  type Back = () => HTMLElement | null

  interface CardProps {
    host: HostItem
    hats: HatItem[]
    hatName?: string
    onRename: (name: string, back: Back) => void
    onDefaultHat: (hat: HatItem, back: Back) => void
    onRevoke: (back: Back) => void
  }

  function HostCard({ host, hats, hatName, onRename, onDefaultHat, onRevoke }: CardProps) {
    const state = hostState(host)
    const [renaming, setRenaming] = useState(false)
    const [name, setName] = useState(host.name)
    const titleId = `host-${host.host_id}`
    // The rename form hides as it closes, taking focus with it: focus goes
    // back to "Rename". A revoke takes every button away: focus goes to the
    // card's title.
    const title = useRef<HTMLHeadingElement>(null)
    const rename = useRef<HTMLButtonElement>(null)
    const refocus = useRef(false)
    const toRename = () => rename.current
    const toTitle = () => title.current

    useEffect(() => {
      if (!renaming && refocus.current) {
        refocus.current = false
        rename.current?.focus()
      }
    }, [renaming])

    const closeForm = () => {
      refocus.current = true
      setRenaming(false)
    }

    return (
      <li className={`card host host-${state}`} aria-labelledby={titleId}>
        <div className="card-head">
          <h2 className="card-title" id={titleId} ref={title} tabIndex={-1}>
            <Text>{host.name}</Text>
          </h2>
          <span className={`state state-${state}`}>{HOST_STATE_LABEL[state]}</span>
        </div>
        {/* One row per fact; the agents, the last doctor result and the
            host's notices join these when the host reports them. */}
        <dl className="facts">
          <dt>Platform</dt>
          <dd>
            <Text>{host.platform}</Text>
          </dd>
          <dt>hennery</dt>
          <dd>
            <Text>{host.host_version}</Text>
          </dd>
          <dt>Default hat</dt>
          <dd>{hatName !== undefined ? <Text>{hatName}</Text> : <Text className="mono">{host.default_hat_id}</Text>}</dd>
          <dt>Paired</dt>
          <dd>
            <When at={host.created_at} />
          </dd>
          <dt>Last connected</dt>
          <dd>{host.last_seen_at ? <When at={host.last_seen_at} /> : 'never'}</dd>
          {host.revoked_at && (
            <>
              <dt>Revoked on</dt>
              <dd>
                <When at={host.revoked_at} />
              </dd>
            </>
          )}
        </dl>
        {state !== 'revoked' && (
          <div className="card-actions">
            {renaming ? (
              <form
                className="inline-form"
                onSubmit={(e) => {
                  e.preventDefault()
                  const trimmed = name.trim()
                  if (trimmed === '' || trimmed === host.name) {
                    closeForm()
                    return
                  }
                  // The confirmation returns focus to "Rename".
                  setRenaming(false)
                  onRename(trimmed, toRename)
                }}
              >
                <label className="field">
                  <span className="field-label">New name</span>
                  <input
                    className="text-input"
                    value={name}
                    maxLength={64}
                    onChange={(e) => setName(e.target.value)}
                    autoFocus
                  />
                </label>
                <button type="submit" className="btn btn-primary btn-sm">
                  Save
                </button>
                <button type="button" className="btn btn-ghost btn-sm" onClick={closeForm}>
                  Cancel
                </button>
              </form>
            ) : (
              <>
                <button
                  type="button"
                  className="btn btn-ghost btn-sm"
                  ref={rename}
                  onClick={() => {
                    setName(host.name)
                    setRenaming(true)
                  }}
                >
                  Rename
                </button>
                <label className="select-field">
                  <span className="field-label">Default hat</span>
                  <select
                    value={host.default_hat_id}
                    onChange={(e) => {
                      const hat = hats.find((h) => h.id === e.target.value)
                      if (hat) onDefaultHat(hat, toTitle)
                    }}
                  >
                    {hats
                      .filter((h) => !h.purging || h.id === host.default_hat_id)
                      .map((h) => (
                        <option key={h.id} value={h.id}>
                          {visible(h.name)}
                        </option>
                      ))}
                  </select>
                </label>
                <span className="spacer" />
                <button type="button" className="btn btn-danger btn-sm" onClick={() => onRevoke(toTitle)}>
                  Revoke
                </button>
              </>
            )}
          </div>
        )}
      </li>
    )
  }
  ```


- [ ] **Step 4: Run the checks**

  Run: `nix develop -c sh -c 'pnpm --dir web typecheck && pnpm --dir web test'`
  Expected: all pass. 236 Vitest tests.

- [ ] **Step 5: Revert-probes** (each must fail the named test file; all were run)
  - `manage.ts`: drop the revoked line; `connected` always online; always offline (each an outcome of `hostState`; `manage.test.ts`).
  - `Pairing.tsx`:
    - a lifetime of 5 s ("drops the code at its expiry");
    - drop the expiry's `setStage` ("drops the code at its expiry");
    - drop the paired `setStage` ("ends the code when the new host pairs");
    - the hosts before the mint taken as none ("tells a new host from those before it even when the list never loaded");
    - the command from `location.origin` ("steps up, then shows the code and the exact command").
  - `Hosts.tsx`: mint before the reads ("reads the URL and the hosts before minting"); drop the `pagehide` listener; actions on a revoked host; drop the revoke dialog's sentence about running agents; drop the default-hat warning; drop each `returnFocus` (the revoked card's title, Rename) and the title's `tabIndex`; compare the untrimmed name; drop the hats' error; keep a spent code, at its expiry and on pairing; one panel for every code (each fails `Hosts.test.tsx`).
  - `Pairing.tsx`: drop the read-in-flight skip ("reads the hosts once at a time"); `onPaired` not reloading the list; `replace` not called after a revoke.
  - `manage.css`: drop `.manage-head .btn` from the 44 px rule ("every button and picker at least 44 px").
  - `manage.ts` (`purgeState`, `isDefault`, `safeColour`; tested here, used in Task 3): drop each of the `default`, `running` and `resume` lines, and answer `resume` for `ready` (four outcomes); `isDefault` without the hosts' defaults; `COLOUR` as `/^#/` (each fails `manage.test.ts`).

- [ ] **Step 6: Commit**

  `git add -A && git commit -m "feat(web): hosts: pair one with a one-time code, rename, re-hat or revoke one"`

---

### Task 3: Hats

- [ ] **Step 1: Write the tests**

Create `web/src/screens/Hats.test.tsx`:

  ```tsx
  import { render, screen, waitFor, within } from '@testing-library/react'
  import userEvent from '@testing-library/user-event'
  import { afterEach, describe, expect, it } from 'vitest'
  import App from '../App'
  import { hat, host, preview } from '../test-fixtures'
  import { FULL, json, stubServer, type Answer } from '../test-server'
  import { loadManageCss, shortTargets } from '../test-targets'

  const STEP_UP = json(403, { code: 'step_up_required', message: 'm' })
  const PERSONAL = hat({ id: 'hat-a', name: 'Personal', colour: '#4c5fd5', default_for_new_hosts: true })
  const WORK = hat()

  afterEach(() => history.replaceState(null, '', '/'))

  function open(routes: Record<string, Answer | Answer[]> = {}) {
    history.replaceState(null, '', '/hats')
    const server = stubServer({
      'GET /api/capabilities': json(200, FULL),
      'GET /api/hats': json(200, [PERSONAL, WORK]),
      'GET /api/hosts': json(200, [host()]),
      'GET /api/hosts/host-1/path-rules': json(200, []),
      'POST /api/auth/step-up/password': new Response(null, { status: 204 }),
      ...routes,
    })
    render(<App fetchImpl={server.fetch} />)
    return server
  }

  async function stepUp() {
    const dialog = await screen.findByRole('dialog', { name: 'This needs a fresh confirmation' })
    await userEvent.type(within(dialog).getByLabelText('Your password'), 'correct horse battery')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Confirm' }))
  }

  const sent = (server: ReturnType<typeof stubServer>, method: string, path: string) =>
    server.sent.filter((s) => s.method === method && s.path === path)

  describe('the hats', () => {
    it('lists each hat with its colour, the default for new hosts marked', async () => {
      open()
      const personal = await screen.findByRole('listitem', { name: 'Personal' })
      expect(within(personal).getByText('Default for new hosts')).toBeInTheDocument()
      expect(personal.querySelector<HTMLElement>('.swatch')!.style.background).toBe('rgb(76, 95, 213)')
    })

    it('applies no colour the server did not give as #rrggbb', async () => {
      open({ 'GET /api/hats': json(200, [hat({ name: 'Odd', colour: 'red;background:url(x)' })]) })
      const odd = await screen.findByRole('listitem', { name: 'Odd' })
      expect(odd.querySelector<HTMLElement>('.swatch')!.getAttribute('style')).toBeNull()
    })

    it('creates a hat with no step-up', async () => {
      const created = hat({ id: 'hat-c', name: 'Clients', colour: '#64748b' })
      const server = open({ 'POST /api/hats': json(201, created) })
      await userEvent.type(await screen.findByRole('textbox', { name: 'Name' }), 'Clients')
      await userEvent.click(screen.getByRole('button', { name: 'Create' }))
      expect(await screen.findByRole('listitem', { name: 'Clients' })).toBeInTheDocument()
      expect(sent(server, 'POST', '/api/hats')[0].body).toEqual({ name: 'Clients', colour: '#64748b' })
    })

    it('asks for another name when it is taken', async () => {
      open({ 'POST /api/hats': json(409, { code: 'name_taken', message: 'm' }) })
      await userEvent.type(await screen.findByRole('textbox', { name: 'Name' }), 'work')
      await userEvent.click(screen.getByRole('button', { name: 'Create' }))
      expect(await screen.findByRole('alert')).toHaveTextContent('Another hat has this name.')
    })

    it('renames and recolours a hat after a step-up, sending the same change again', async () => {
      const server = open({
        'PATCH /api/hats/hat-b': [STEP_UP, json(200, hat({ name: 'Day job', colour: '#112233' }))],
      })
      const work = await screen.findByRole('listitem', { name: 'Work' })
      await userEvent.click(within(work).getByRole('button', { name: 'Edit' }))
      const name = within(work).getByRole('textbox', { name: 'Name' })
      await userEvent.clear(name)
      await userEvent.type(name, 'Day job')
      await userEvent.click(within(work).getByRole('button', { name: 'Save' }))
      await stepUp()
      expect(await screen.findByRole('listitem', { name: 'Day job' })).toBeInTheDocument()
      expect(sent(server, 'PATCH', '/api/hats/hat-b').map((p) => p.body)).toEqual([{ name: 'Day job' }, { name: 'Day job' }])
    })

    it('makes a hat the default for new hosts, and the other one no longer', async () => {
      const server = open({
        'PATCH /api/hats/hat-b': [STEP_UP, json(200, hat({ default_for_new_hosts: true }))],
      })
      const work = await screen.findByRole('listitem', { name: 'Work' })
      await userEvent.click(within(work).getByRole('button', { name: 'Make default for new hosts' }))
      await stepUp()
      await waitFor(() => expect(within(screen.getByRole('listitem', { name: 'Work' })).getByText('Default for new hosts')).toBeInTheDocument())
      expect(within(screen.getByRole('listitem', { name: 'Personal' })).queryByText('Default for new hosts')).toBeNull()
      expect(sent(server, 'PATCH', '/api/hats/hat-b').map((p) => p.body)).toEqual([
        { default_for_new_hosts: true },
        { default_for_new_hosts: true },
      ])
    })

    it('keeps both of two changes that land together', async () => {
      let answerPatch: (r: Response) => void = () => {}
      open({
        'PATCH /api/hats/hat-b': () => new Promise<Response>((resolve) => (answerPatch = resolve)),
        'POST /api/hats': json(201, hat({ id: 'hat-c', name: 'Clients' })),
      })
      const work = await screen.findByRole('listitem', { name: 'Work' })
      await userEvent.click(within(work).getByRole('button', { name: 'Make default for new hosts' }))
      await userEvent.type(screen.getByRole('textbox', { name: 'Name' }), 'Clients')
      await userEvent.click(screen.getByRole('button', { name: 'Create' }))
      expect(await screen.findByRole('listitem', { name: 'Clients' })).toBeInTheDocument()
      answerPatch(json(200, hat({ default_for_new_hosts: true })))
      await waitFor(() => expect(within(screen.getByRole('listitem', { name: 'Work' })).getByText('Default for new hosts')).toBeInTheDocument())
      expect(screen.getByRole('listitem', { name: 'Clients' })).toBeInTheDocument()
      expect(within(screen.getByRole('listitem', { name: 'Personal' })).queryByText('Default for new hosts')).toBeNull()
    })

    it('keeps a change that lands while a new hat is being created', async () => {
      let answerCreate: (r: Response) => void = () => {}
      open({
        'PATCH /api/hats/hat-b': json(200, hat({ default_for_new_hosts: true })),
        'POST /api/hats': () => new Promise<Response>((resolve) => (answerCreate = resolve)),
      })
      await userEvent.type(await screen.findByRole('textbox', { name: 'Name' }), 'Clients')
      await userEvent.click(screen.getByRole('button', { name: 'Create' }))
      await userEvent.click(within(screen.getByRole('listitem', { name: 'Work' })).getByRole('button', { name: 'Make default for new hosts' }))
      await waitFor(() => expect(within(screen.getByRole('listitem', { name: 'Work' })).getByText('Default for new hosts')).toBeInTheDocument())
      answerCreate(json(201, hat({ id: 'hat-c', name: 'Clients' })))
      expect(await screen.findByRole('listitem', { name: 'Clients' })).toBeInTheDocument()
      expect(within(screen.getByRole('listitem', { name: 'Work' })).getByText('Default for new hosts')).toBeInTheDocument()
    })

    it('returns focus to Edit after an edit is saved', async () => {
      open({ 'PATCH /api/hats/hat-b': json(200, hat({ name: 'Day job' })) })
      const work = await screen.findByRole('listitem', { name: 'Work' })
      await userEvent.click(within(work).getByRole('button', { name: 'Edit' }))
      await userEvent.type(within(work).getByRole('textbox', { name: 'Name' }), ' 2')
      await userEvent.click(within(work).getByRole('button', { name: 'Save' }))
      const dayJob = await screen.findByRole('listitem', { name: 'Day job' })
      await waitFor(() => expect(within(dayJob).getByRole('button', { name: 'Edit' })).toHaveFocus())
    })

    it('cannot save a name of spaces only', async () => {
      const server = open({})
      const work = await screen.findByRole('listitem', { name: 'Work' })
      await userEvent.click(within(work).getByRole('button', { name: 'Edit' }))
      await userEvent.clear(within(work).getByRole('textbox', { name: 'Name' }))
      await userEvent.type(within(work).getByRole('textbox', { name: 'Name' }), '   ')
      expect(within(work).getByRole('button', { name: 'Save' })).toBeDisabled()
      expect(sent(server, 'PATCH', '/api/hats/hat-b')).toHaveLength(0)
    })

    it('puts focus on the card’s title once “Make default for new hosts” is gone', async () => {
      open({ 'PATCH /api/hats/hat-b': json(200, hat({ default_for_new_hosts: true })) })
      const work = await screen.findByRole('listitem', { name: 'Work' })
      await userEvent.click(within(work).getByRole('button', { name: 'Make default for new hosts' }))
      await waitFor(() => expect(within(work).queryByRole('button', { name: 'Make default for new hosts' })).toBeNull())
      expect(within(work).getByRole('heading', { name: 'Work' })).toHaveFocus()
    })

    it('puts focus back in the name after a hat is created', async () => {
      open({ 'POST /api/hats': json(201, hat({ id: 'hat-c', name: 'Clients' })) })
      const name = await screen.findByRole('textbox', { name: 'Name' })
      await userEvent.type(name, 'Clients')
      await userEvent.click(screen.getByRole('button', { name: 'Create' }))
      expect(await screen.findByRole('listitem', { name: 'Clients' })).toBeInTheDocument()
      expect(name).toHaveFocus()
    })

    it('says so when the hosts cannot be read', async () => {
      open({ 'GET /api/hosts': json(500, { code: 'internal', message: 'the hosts are away' }) })
      expect(await screen.findByRole('alert')).toHaveTextContent('the hosts are away')
    })

    it('has every button, picker and colour at least 44 px tall under 768 px', async () => {
      const unload = loadManageCss()
      try {
        open({ 'GET /api/hosts/host-1/path-rules': json(200, [{ id: 'r-1', prefix: '/home/me/work', hat_id: 'hat-b', verified: true }]) })
        await screen.findByDisplayValue('/home/me/work')
        const work = screen.getByRole('listitem', { name: 'Work' })
        await userEvent.click(within(work).getByRole('button', { name: 'Edit' }))
        expect(shortTargets(document.querySelector('.manage')!)).toEqual([])
      } finally {
        unload()
      }
    })
  })

  describe('purging a hat', () => {
    const result = { sessions: 3, rules: 2, unconfirmed: ['s-9'], host_transcripts: { removed: 2, partial: 0, pending: 1, pending_sessions: ['s-9'] } }

    it('shows what goes, then purges after a confirmation and a step-up', async () => {
      const server = open({
        'GET /api/hats/hat-b/purge': json(200, preview()),
        'POST /api/hats/hat-b/purge': [STEP_UP, json(200, result)],
      })
      await userEvent.click(within(await screen.findByRole('listitem', { name: 'Work' })).getByRole('button', { name: 'Purge' }))
      const dialog = await screen.findByRole('dialog', { name: 'Purge this hat?' })
      expect(dialog).toHaveTextContent('3 sessions of the hat')
      expect(dialog).toHaveTextContent('2 path rules')
      expect(dialog).toHaveTextContent('1 recent project')
      expect(sent(server, 'POST', '/api/hats/hat-b/purge')).toHaveLength(0)
      await userEvent.click(within(dialog).getByRole('button', { name: 'Purge' }))
      await stepUp()
      const outcome = await screen.findByRole('status', { name: 'Purged Work' })
      expect(outcome).toHaveTextContent('Deleted 3 sessions and 2 path rules.')
      expect(outcome).toHaveTextContent('s-9')
      expect(outcome).toHaveTextContent('2 removed, 0 removed in part, 1 still to remove')
      expect(sent(server, 'POST', '/api/hats/hat-b/purge')).toHaveLength(2)
    })

    it('puts focus on what the purge deleted, once the dialog has gone', async () => {
      open({
        'GET /api/hats/hat-b/purge': json(200, preview()),
        'POST /api/hats/hat-b/purge': json(200, result),
      })
      await userEvent.click(within(await screen.findByRole('listitem', { name: 'Work' })).getByRole('button', { name: 'Purge' }))
      await userEvent.click(within(await screen.findByRole('dialog', { name: 'Purge this hat?' })).getByRole('button', { name: 'Purge' }))
      const outcome = await screen.findByRole('status', { name: 'Purged Work' })
      await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
      await waitFor(() => expect(within(outcome).getByRole('heading', { name: 'Purged Work' })).toHaveFocus())
    })

    it('cannot purge while a session runs, and names it', async () => {
      open({ 'GET /api/hats/hat-b/purge': json(200, preview({ running: ['s-1'] })) })
      await userEvent.click(within(await screen.findByRole('listitem', { name: 'Work' })).getByRole('button', { name: 'Purge' }))
      const dialog = await screen.findByRole('dialog', { name: 'Purge this hat?' })
      expect(dialog).toHaveTextContent('Close these sessions first')
      expect(within(dialog).getByRole('link', { name: 's-1' })).toHaveAttribute('href', '/sessions/s-1')
      expect(within(dialog).getByRole('button', { name: 'Purge' })).toBeDisabled()
    })

    it('cannot purge a default hat', async () => {
      open({ 'GET /api/hosts': json(200, [host({ default_hat_id: 'hat-b' })]) })
      const work = await screen.findByRole('listitem', { name: 'Work' })
      await waitFor(() => expect(within(work).getByRole('button', { name: 'Purge' })).toBeDisabled())
      expect(work).toHaveTextContent('A default hat cannot be purged')
    })

    it('resumes a purge that began', async () => {
      const server = open({
        'GET /api/hats': json(200, [PERSONAL, hat({ purging: true })]),
        'GET /api/hats/hat-b/purge': json(200, preview({ purging: true })),
        'POST /api/hats/hat-b/purge': json(200, result),
      })
      await userEvent.click(within(await screen.findByRole('listitem', { name: 'Work' })).getByRole('button', { name: 'Resume purge' }))
      const dialog = await screen.findByRole('dialog', { name: 'Resume this hat’s purge?' })
      await userEvent.click(within(dialog).getByRole('button', { name: 'Resume purge' }))
      expect(await screen.findByRole('status', { name: 'Purged Work' })).toBeInTheDocument()
      expect(sent(server, 'POST', '/api/hats/hat-b/purge')).toHaveLength(1)
    })

    it('reads the hats again when a purge fails, so a hat it froze offers “Resume purge”', async () => {
      open({
        'GET /api/hats': [json(200, [PERSONAL, WORK]), json(200, [PERSONAL, hat({ purging: true })])],
        'GET /api/hats/hat-b/purge': json(200, preview()),
        'POST /api/hats/hat-b/purge': json(500, { code: 'internal', message: 'stopped half way' }),
      })
      await userEvent.click(within(await screen.findByRole('listitem', { name: 'Work' })).getByRole('button', { name: 'Purge' }))
      const dialog = await screen.findByRole('dialog', { name: 'Purge this hat?' })
      await userEvent.click(within(dialog).getByRole('button', { name: 'Purge' }))
      expect(await within(dialog).findByRole('alert')).toHaveTextContent('stopped half way')
      await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))
      expect(await within(screen.getByRole('listitem', { name: 'Work' })).findByRole('button', { name: 'Resume purge' })).toBeInTheDocument()
    })

    it('reads the rules again after a purge, which deleted the hat’s, and asks the tester again', async () => {
      const server = open({
        'GET /api/hosts/host-1/path-rules': [json(200, [{ id: 'r-1', prefix: '/home/me/work', hat_id: 'hat-b', verified: true }]), json(200, [])],
        'GET /api/hats/hat-b/purge': json(200, preview()),
        'POST /api/hats/hat-b/purge': json(200, result),
        'POST /api/hats/resolve': [
          json(200, { canonical: '/home/me/work/app', exists: true, is_dir: true, hat_id: 'hat-b', rule_id: 'r-1' }),
          json(200, { canonical: '/home/me/work/app', exists: true, is_dir: true, hat_id: 'hat-a' }),
        ],
      })
      await screen.findByDisplayValue('/home/me/work')
      await userEvent.type(screen.getByLabelText('Test a path'), '/home/me/work/app')
      expect(await screen.findByLabelText('Resolution', {}, { timeout: 2000 })).toHaveTextContent('a path rule')
      await userEvent.click(within(screen.getByRole('listitem', { name: 'Work' })).getByRole('button', { name: 'Purge' }))
      await userEvent.click(within(await screen.findByRole('dialog', { name: 'Purge this hat?' })).getByRole('button', { name: 'Purge' }))
      expect(await screen.findByText('No rules: every session on this host gets its default hat.')).toBeInTheDocument()
      expect(screen.queryByDisplayValue('/home/me/work')).toBeNull()
      await waitFor(() => expect(screen.getByLabelText('Resolution')).toHaveTextContent('the host’s default hat'), { timeout: 2000 })
      expect(sent(server, 'POST', '/api/hats/resolve')).toHaveLength(2)
    })

    it('lists the sessions of no hat, which no purge deletes', async () => {
      const lost = { session_id: 's-0', host_id: 'host-1', agent: 'claude', cwd: '/srv/old', hat_id: '', lifecycle: 'closed', presumed_parked: false, created_at: '2026-10-01T10:00:00Z', last_event_at: '2026-10-01T10:00:00Z' }
      open({ 'GET /api/hats/hat-b/purge': json(200, preview({ unassigned: [lost], unassigned_count: 1 })) })
      await userEvent.click(within(await screen.findByRole('listitem', { name: 'Work' })).getByRole('button', { name: 'Purge' }))
      const dialog = await screen.findByRole('dialog', { name: 'Purge this hat?' })
      expect(dialog).toHaveTextContent('1 session belong to no hat')
      expect(within(dialog).getByRole('link', { name: '/srv/old' })).toHaveAttribute('href', '/sessions/s-0')
    })
  })

  describe('path rules', () => {
    const RULES = [{ id: 'r-1', prefix: '/home/me/work', hat_id: 'hat-b', verified: true }]

    it('shows a host’s rules, unverified ones marked', async () => {
      open({
        'GET /api/hosts/host-1/path-rules': json(200, [...RULES, { id: 'r-2', prefix: '/home/me/later', hat_id: 'hat-b', verified: false }]),
      })
      expect(await screen.findByDisplayValue('/home/me/work')).toBeInTheDocument()
      expect(screen.getAllByText('Unverified')).toHaveLength(1)
    })

    it('sends the whole set after a step-up, and shows the set as the host stored it', async () => {
      const server = open({
        'GET /api/hosts/host-1/path-rules': json(200, RULES),
        'PUT /api/hosts/host-1/path-rules': [
          STEP_UP,
          json(200, [...RULES, { id: 'r-3', prefix: '/private/tmp/x', hat_id: 'hat-a', verified: true }]),
        ],
      })
      await screen.findByDisplayValue('/home/me/work')
      await userEvent.click(screen.getByRole('button', { name: 'Add rule' }))
      await userEvent.type(screen.getByLabelText('Path 2'), '/tmp/x')
      await userEvent.selectOptions(screen.getByLabelText('Hat 2'), 'hat-a')
      await userEvent.click(screen.getByRole('button', { name: 'Save rules' }))
      await stepUp()
      expect(await screen.findByDisplayValue('/private/tmp/x')).toBeInTheDocument()
      const puts = sent(server, 'PUT', '/api/hosts/host-1/path-rules')
      expect(puts).toHaveLength(2)
      expect(puts[0].body).toEqual(puts[1].body)
      expect(puts[1].body).toEqual({
        rules: [
          { prefix: '/home/me/work', hat_id: 'hat-b' },
          { prefix: '/tmp/x', hat_id: 'hat-a' },
        ],
      })
    })

    it('shows a rule’s hat that is being purged, rather than another', async () => {
      open({
        'GET /api/hats': json(200, [PERSONAL, hat({ purging: true })]),
        'GET /api/hosts/host-1/path-rules': json(200, RULES),
      })
      await screen.findByDisplayValue('/home/me/work')
      const picker = screen.getByLabelText('Hat 1') as HTMLSelectElement
      expect(picker.value).toBe('hat-b')
      expect(within(picker).getByRole('option', { name: 'Work' })).toBeDisabled()
    })

    it('cannot save a rule with no path, and says why', async () => {
      open({ 'GET /api/hosts/host-1/path-rules': json(200, RULES) })
      await screen.findByDisplayValue('/home/me/work')
      expect(screen.getByRole('button', { name: 'Save rules' })).toBeEnabled()
      await userEvent.click(screen.getByRole('button', { name: 'Add rule' }))
      await userEvent.type(screen.getByLabelText('Path 2'), '   ')
      expect(screen.getByRole('button', { name: 'Save rules' })).toBeDisabled()
      expect(screen.getByText('Every rule needs a path.')).toBeInTheDocument()
      await userEvent.type(screen.getByLabelText('Path 2'), '/srv')
      expect(screen.getByRole('button', { name: 'Save rules' })).toBeEnabled()
    })

    it('moves focus to the next rule when one is removed, and to “Add rule” after the last', async () => {
      open({
        'GET /api/hosts/host-1/path-rules': json(200, [...RULES, { id: 'r-2', prefix: '/srv', hat_id: 'hat-a', verified: true }]),
      })
      await screen.findByDisplayValue('/home/me/work')
      await userEvent.click(screen.getByRole('button', { name: 'Remove rule 1' }))
      expect(screen.getByLabelText('Path 1')).toHaveValue('/srv')
      expect(screen.getByLabelText('Path 1')).toHaveFocus()
      await userEvent.click(screen.getByRole('button', { name: 'Remove rule 1' }))
      expect(screen.getByRole('button', { name: 'Add rule' })).toHaveFocus()
    })

    it('says why a set was refused', async () => {
      open({
        'GET /api/hosts/host-1/path-rules': json(200, RULES),
        'PUT /api/hosts/host-1/path-rules': json(409, { code: 'host_offline', message: 'm' }),
      })
      await screen.findByDisplayValue('/home/me/work')
      await userEvent.click(screen.getByRole('button', { name: 'Save rules' }))
      expect(await screen.findByRole('alert')).toHaveTextContent('The host is offline.')
    })
  })

  describe('the path tester', () => {
    it('asks the host which hat a typed path resolves to, and says which rule decided', async () => {
      const server = open({
        'POST /api/hats/resolve': json(200, { canonical: '/home/me/work/app', exists: true, is_dir: true, hat_id: 'hat-b', rule_id: 'r-1' }),
      })
      await userEvent.type(await screen.findByLabelText('Test a path'), '~/work/app')
      const resolution = await screen.findByLabelText('Resolution', {}, { timeout: 2000 })
      expect(resolution).toHaveTextContent('/home/me/work/app')
      expect(resolution).toHaveTextContent('Work')
      expect(resolution).toHaveTextContent('a path rule')
      const asked = sent(server, 'POST', '/api/hats/resolve')
      expect(asked.at(-1)?.body).toEqual({ host_id: 'host-1', path: '~/work/app' })
      // Typing paused once: one question, not one per key.
      expect(asked).toHaveLength(1)
    })

    it('names the host’s default hat when no rule decided, and a path that is missing', async () => {
      open({ 'POST /api/hats/resolve': json(200, { canonical: '/srv/x', exists: false, is_dir: false, hat_id: 'hat-a' }) })
      await userEvent.type(await screen.findByLabelText('Test a path'), '/srv/x')
      const resolution = await screen.findByLabelText('Resolution', {}, { timeout: 2000 })
      expect(resolution).toHaveTextContent('the host’s default hat')
      expect(resolution).toHaveTextContent('This path does not exist on the host.')
    })

    it('shows the answer for the path typed last, never an older one', async () => {
      let answerFirst: (r: Response) => void = () => {}
      open({
        'POST /api/hats/resolve': [
          () => new Promise<Response>((resolve) => (answerFirst = resolve)),
          json(200, { canonical: '/srv/b', exists: true, is_dir: true, hat_id: 'hat-a' }),
        ],
      })
      const tester = await screen.findByLabelText('Test a path')
      await userEvent.type(tester, '/srv/a')
      await screen.findByText('Asking the host…', {}, { timeout: 2000 })
      await userEvent.clear(tester)
      await userEvent.type(tester, '/srv/b')
      const resolution = await screen.findByLabelText('Resolution', {}, { timeout: 2000 })
      expect(resolution).toHaveTextContent('/srv/b')
      // The first question's answer arrives late, and is not shown.
      answerFirst(json(200, { canonical: '/srv/a', exists: true, is_dir: true, hat_id: 'hat-b' }))
      await new Promise((r) => setTimeout(r, 50))
      expect(screen.getByLabelText('Resolution')).toHaveTextContent('/srv/b')
    })

    it('asks again after the rules are saved, since it answers under the rules as saved', async () => {
      open({
        'GET /api/hosts/host-1/path-rules': json(200, []),
        'PUT /api/hosts/host-1/path-rules': json(200, [{ id: 'r-1', prefix: '/srv', hat_id: 'hat-b', verified: true }]),
        'POST /api/hats/resolve': [
          json(200, { canonical: '/srv/x', exists: true, is_dir: true, hat_id: 'hat-a' }),
          json(200, { canonical: '/srv/x', exists: true, is_dir: true, hat_id: 'hat-b', rule_id: 'r-1' }),
        ],
      })
      await userEvent.type(await screen.findByLabelText('Test a path'), '/srv/x')
      expect(await screen.findByLabelText('Resolution', {}, { timeout: 2000 })).toHaveTextContent('the host’s default hat')
      await userEvent.click(screen.getByRole('button', { name: 'Add rule' }))
      await userEvent.type(screen.getByLabelText('Path 1'), '/srv')
      await userEvent.click(screen.getByRole('button', { name: 'Save rules' }))
      await waitFor(() => expect(screen.getByLabelText('Resolution')).toHaveTextContent('a path rule'), { timeout: 2000 })
      expect(screen.getByLabelText('Test a path')).toHaveValue('/srv/x')
    })

    it('shows no answer for a path typed after it', async () => {
      open({ 'POST /api/hats/resolve': json(200, { canonical: '/srv/x', exists: true, is_dir: true, hat_id: 'hat-a' }) })
      const tester = await screen.findByLabelText('Test a path')
      await userEvent.type(tester, '/srv/x')
      expect(await screen.findByLabelText('Resolution', {}, { timeout: 2000 })).toBeInTheDocument()
      await userEvent.type(tester, 'y')
      expect(screen.queryByLabelText('Resolution')).toBeNull()
    })

    it('announces each answer once: in one live region, nested in none', async () => {
      open({
        'POST /api/hats/resolve': [
          json(409, { code: 'host_offline', message: 'm' }),
          json(200, { canonical: '/srv/xy', exists: true, is_dir: true, hat_id: 'hat-a' }),
        ],
      })
      const LIVE = '[role="status"], [role="alert"], [aria-live]:not([aria-live="off"])'
      const nested = () => [...document.querySelectorAll(LIVE)].filter((r) => r.parentElement?.closest(LIVE))
      const tester = await screen.findByLabelText('Test a path')
      await userEvent.type(tester, '/srv/x')
      await screen.findByText('The host is offline: it resolves the path.', {}, { timeout: 2000 })
      expect(nested()).toEqual([])
      await userEvent.type(tester, 'y')
      const resolution = await screen.findByLabelText('Resolution', {}, { timeout: 2000 })
      expect(resolution.closest(LIVE)).not.toBeNull()
      expect(nested()).toEqual([])
    })

    it.each([
      ['host_offline', 'The host is offline: it resolves the path.'],
      ['resolve_unsupported', 'This host cannot resolve paths yet: update hennery on it.'],
      ['hat_ambiguous', 'This path matches more than one hat'],
    ])('says why it could not tell (%s)', async (code, message) => {
      open({ 'POST /api/hats/resolve': json(409, { code, message: 'm' }) })
      await userEvent.type(await screen.findByLabelText('Test a path'), '/srv/x')
      expect(await screen.findByRole('alert', {}, { timeout: 2000 })).toHaveTextContent(message)
    })

    it('shows a host’s own refusal as escaped text', async () => {
      open({ 'POST /api/hats/resolve': json(502, { code: 'host_refused', message: '<b>no</b>\u202E' }) })
      await userEvent.type(await screen.findByLabelText('Test a path'), '/srv/x')
      const alert = await screen.findByRole('alert', {}, { timeout: 2000 })
      expect(alert.textContent).toBe('<b>no</b><U+202E>')
      expect(alert.querySelector('b')).toBeNull()
    })
  })
  ```


- [ ] **Step 2: Run them, and see them fail**

  Run: `nix develop -c pnpm --dir web test`
  Expected: `Hats.test.tsx` fails: `/hats` still shows its placeholder.

- [ ] **Step 3: The code**

Create `web/src/components/PathRules.tsx`:

  ```tsx
  // A host's path rules (kernel spec §5.2, §8): the whole set, edited here and
  // sent back whole (step-up; the host must be connected, since it resolves
  // each prefix). Beside it, a live tester: which hat a path on that host
  // resolves to under the rules as SAVED, as a session started there would
  // get: it asks again whenever they are saved, or a purge deleted some.
  import { useEffect, useRef, useState } from 'react'
  import { ApiFailure, messageOf } from '../api/errors'
  import { pathRules, replacePathRules, resolveHat } from '../api/manage'
  import { useClient } from '../app-client'
  import type { HatItem, HatResolution, HostItem, PathRuleInput, PathRuleItem } from '../generated/protocol'
  import { useResource } from '../hooks/useResource'
  import { Text, visible } from '../lib/text'

  /** How long typing pauses before the tester asks the host. */
  export const TEST_DELAY_MS = 400

  /** `purges` counts the purges tried: each may have deleted rules here. */
  export default function PathRules({ hosts, hats, purges }: { hosts: HostItem[]; hats: HatItem[]; purges: number }) {
    const [hostId, setHostId] = useState<string | null>(null)
    const [saves, setSaves] = useState(0)
    const host = hosts.find((h) => h.host_id === hostId) ?? hosts[0]

    if (!host) {
      return (
        <section className="card" aria-labelledby="rules-title">
          <h2 className="card-title" id="rules-title">
            Path rules
          </h2>
          <p className="empty">Pair a host to give its paths a hat.</p>
        </section>
      )
    }
    return (
      <section className="card" aria-labelledby="rules-title">
        <h2 className="card-title" id="rules-title">
          Path rules
        </h2>
        <p className="hint">
          A session belongs to the hat of the longest rule whose path is its directory or a parent of it, by whole path
          segments; with no rule, to its host’s default hat.
        </p>
        <label className="select-field">
          <span className="field-label">Host</span>
          <select value={host.host_id} onChange={(e) => setHostId(e.target.value)}>
            {hosts.map((h) => (
              <option key={h.host_id} value={h.host_id}>
                {visible(h.name)}
              </option>
            ))}
          </select>
        </label>
        {/* After a purge the rules are read again; after a save, the answer
            is the set as stored already. The tester asks again after both. */}
        <Rules key={`${host.host_id}-${purges}`} host={host} hats={hats} onSaved={() => setSaves((n) => n + 1)} />
        <Tester key={`t-${host.host_id}`} host={host} hats={hats} rules={`${purges}.${saves}`} />
      </section>
    )
  }

  interface Row {
    key: number
    prefix: string
    hat_id: string
    verified?: boolean
  }

  function rowsOf(rules: PathRuleItem[], next: () => number): Row[] {
    return rules.map((r) => ({ key: next(), prefix: r.prefix, hat_id: r.hat_id, verified: r.verified }))
  }

  function Rules({ host, hats, onSaved }: { host: HostItem; hats: HatItem[]; onSaved: () => void }) {
    const client = useClient()
    const counter = useRef(0)
    const next = () => ++counter.current
    const [rows, setRows] = useState<Row[] | null>(null)
    const [busy, setBusy] = useState(false)
    const [error, setError] = useState<string | null>(null)
    const [saved, setSaved] = useState(false)
    const stored = useResource(() => pathRules(client, host.host_id), [host.host_id])
    const usable = hats.filter((h) => !h.purging)
    // A removed row takes its focused button away: focus goes to the next
    // row's path, or to "Add rule" after the last.
    const inputs = useRef(new Map<number, HTMLInputElement>())
    const addRule = useRef<HTMLButtonElement>(null)
    const refocus = useRef<number | 'add' | null>(null)
    useEffect(() => {
      if (refocus.current === null) return
      const to = refocus.current === 'add' ? addRule.current : inputs.current.get(refocus.current)
      refocus.current = null
      to?.focus()
    })

    useEffect(() => {
      if (stored.data) setRows(rowsOf(stored.data, next))
    }, [stored.data])

    const edit = (key: number, change: Partial<Row>) => {
      setSaved(false)
      setRows((rows ?? []).map((r) => (r.key === key ? { ...r, ...change, verified: undefined } : r)))
    }

    const save = async () => {
      if (!rows) return
      setBusy(true)
      setError(null)
      try {
        const body: PathRuleInput[] = rows.map((r) => ({ prefix: r.prefix.trim(), hat_id: r.hat_id }))
        // The answer is the set as stored, resolved by the host: edit from
        // it, never from what was typed.
        stored.set(await replacePathRules(client, host.host_id, body))
        setSaved(true)
        onSaved()
      } catch (err) {
        setError(messageOf(err))
      } finally {
        setBusy(false)
      }
    }

    if (stored.error) {
      return (
        <p className="form-error" role="alert">
          <Text>{stored.error}</Text>
        </p>
      )
    }
    if (!rows) return <p role="status">Loading the rules…</p>
    // The server refuses a whole set for one blank path.
    const blank = rows.some((r) => r.prefix.trim() === '')

    return (
      <div className="rules">
        {rows.length === 0 && <p className="empty">No rules: every session on this host gets its default hat.</p>}
        <ul className="rule-list" aria-label="Rules">
          {rows.map((row, i) => (
            <li key={row.key} className="rule">
              <label className="field rule-prefix">
                <span className="field-label">Path {i + 1}</span>
                <input
                  ref={(el) => {
                    if (el) inputs.current.set(row.key, el)
                    else inputs.current.delete(row.key)
                  }}
                  className="text-input mono"
                  value={row.prefix}
                  placeholder="/home/me/work"
                  onChange={(e) => edit(row.key, { prefix: e.target.value })}
                />
              </label>
              <label className="select-field">
                <span className="field-label">Hat {i + 1}</span>
                <select value={row.hat_id} onChange={(e) => edit(row.key, { hat_id: e.target.value })}>
                  {/* A rule's hat being purged is no choice, but it is the
                      rule's: shown, so the picker says what a save sends. */}
                  {!usable.some((h) => h.id === row.hat_id) && (
                    <option value={row.hat_id} disabled>
                      {visible(hats.find((h) => h.id === row.hat_id)?.name ?? row.hat_id)}
                    </option>
                  )}
                  {usable.map((h) => (
                    <option key={h.id} value={h.id}>
                      {visible(h.name)}
                    </option>
                  ))}
                </select>
              </label>
              {row.verified === false && (
                <span className="tag tag-warn" title="The path did not exist on the host when the rule was saved.">
                  Unverified
                </span>
              )}
              <button
                type="button"
                className="btn btn-ghost btn-sm"
                aria-label={`Remove rule ${i + 1}`}
                onClick={() => {
                  setSaved(false)
                  refocus.current = rows[i + 1]?.key ?? 'add'
                  setRows(rows.filter((r) => r.key !== row.key))
                }}
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
        <div className="card-actions">
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            ref={addRule}
            disabled={usable.length === 0}
            onClick={() => {
              setSaved(false)
              setRows([...rows, { key: next(), prefix: '', hat_id: usable[0].id }])
            }}
          >
            Add rule
          </button>
          <span className="spacer" />
          <button type="button" className="btn btn-primary btn-sm" onClick={save} disabled={busy || blank}>
            Save rules
          </button>
        </div>
        {blank && <p className="hint">Every rule needs a path.</p>}
        {!host.connected && <p className="hint">The host is offline: rules can be saved only while it is connected.</p>}
        {saved && <p role="status">Saved.</p>}
        {error && (
          <p className="form-error" role="alert">
            <Text>{error}</Text>
          </p>
        )}
      </div>
    )
  }

  type Verdict = { kind: 'idle' } | { kind: 'asking' } | { kind: 'resolved'; resolution: HatResolution } | { kind: 'failed'; error: string }

  /** `rules` names the saved set: a new value asks again. */
  function Tester({ host, hats, rules }: { host: HostItem; hats: HatItem[]; rules: string }) {
    const client = useClient()
    const [path, setPath] = useState('')
    const [verdict, setVerdict] = useState<Verdict>({ kind: 'idle' })

    // Each change waits for typing to pause, then asks; a newer path aborts
    // the older question, so an answer never lands on the wrong path, and
    // the last answer goes as soon as the path or the rules change.
    useEffect(() => {
      setVerdict({ kind: 'idle' })
      const typed = path.trim()
      if (typed === '') return
      const abort = new AbortController()
      const timer = setTimeout(() => {
        setVerdict({ kind: 'asking' })
        resolveHat(client, host.host_id, typed, abort.signal).then(
          (resolution) => {
            if (!abort.signal.aborted) setVerdict({ kind: 'resolved', resolution })
          },
          (err) => {
            if (abort.signal.aborted) return
            setVerdict({ kind: 'failed', error: testerMessage(err) })
          },
        )
      }, TEST_DELAY_MS)
      return () => {
        clearTimeout(timer)
        abort.abort()
      }
    }, [client, host.host_id, path, rules])

    const hatName = (id: string) => hats.find((h) => h.id === id)?.name ?? id

    return (
      <div className="tester">
        <label className="field">
          <span className="field-label">Test a path</span>
          <input
            className="text-input mono"
            value={path}
            placeholder="~/work/project"
            onChange={(e) => setPath(e.target.value)}
          />
        </label>
        <p className="hint">Resolved by the host, under the rules as saved.</p>
        {/* Each answer is its own live region, nested in none: announced
            once. */}
        <div className="tester-out">
          {verdict.kind === 'asking' && <p role="status">Asking the host…</p>}
          {verdict.kind === 'failed' && (
            <p className="form-error" role="alert">
              <Text>{verdict.error}</Text>
            </p>
          )}
          {verdict.kind === 'resolved' && (
            <div role="status">
              <dl className="facts" aria-label="Resolution">
                <dt>Resolves to</dt>
                <dd className="mono">
                  <Text>{verdict.resolution.canonical}</Text>
                </dd>
                <dt>Hat</dt>
                <dd>
                  <Text>{hatName(verdict.resolution.hat_id)}</Text>
                </dd>
                <dt>Decided by</dt>
                <dd>{verdict.resolution.rule_id ? 'a path rule' : 'the host’s default hat'}</dd>
                {!verdict.resolution.exists && (
                  <>
                    <dt>Note</dt>
                    <dd>This path does not exist on the host.</dd>
                  </>
                )}
                {verdict.resolution.exists && !verdict.resolution.is_dir && (
                  <>
                    <dt>Note</dt>
                    <dd>This is not a directory: no session can start in it.</dd>
                  </>
                )}
              </dl>
            </div>
          )}
        </div>
      </div>
    )
  }

  /** The tester's own words for the refusals a host's resolution can end in
   *  (plan 5b): the rest are the client's plain messages, or the host's text. */
  function testerMessage(err: unknown): string {
    if (err instanceof ApiFailure && err.code === 'host_offline') return 'The host is offline: it resolves the path.'
    return messageOf(err)
  }
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
  import { Link, type Route } from '../router'
  import Hosts from '../screens/Hosts'
  ```

with:

  ```tsx
  import { Link, type Route } from '../router'
  import Hats from '../screens/Hats'
  import Hosts from '../screens/Hosts'
  ```

In `web/src/components/Shell.tsx`, replace:

  ```tsx
              <Hosts />
            ) : route.name === 'session' ? (
  ```

with:

  ```tsx
              <Hosts />
            ) : route.name === 'hats' ? (
              <Hats />
            ) : route.name === 'session' ? (
  ```

Create `web/src/screens/Hats.tsx`:

  ```tsx
  // Hats (frontend spec §8, kernel spec §5): create, rename and recolour hats,
  // pick the default for new hosts, edit each host's path rules with a live
  // "this path resolves to" tester, and purge a hat after seeing what goes.
  // Every change but creating needs a fresh step-up, which the client asks
  // for when the server refuses.
  import { useEffect, useRef, useState, type FormEvent } from 'react'
  import { messageOf } from '../api/errors'
  import { createHat, hats as listHats, hosts as listHosts, purgeHat, purgePreview, updateHat } from '../api/manage'
  import { useClient } from '../app-client'
  import ConfirmDialog from '../components/ConfirmDialog'
  import PathRules from '../components/PathRules'
  import type { HatItem, HostItem, PurgePreview, PurgeResult } from '../generated/protocol'
  import { useResource } from '../hooks/useResource'
  import { count, isDefault, purgeState, safeColour } from '../lib/manage'
  import { Text } from '../lib/text'
  import { Link } from '../router'
  import '../manage.css'

  /** The colour a new hat starts with, as the server's own default. */
  const NEW_COLOUR = '#64748b'

  export default function Hats() {
    const client = useClient()
    const hats = useResource(() => listHats(client))
    const hosts = useResource(() => listHosts(client))
    const [purge, setPurge] = useState<{ hat: HatItem; preview: PurgePreview } | null>(null)
    const [purged, setPurged] = useState<{ name: string; result: PurgeResult } | null>(null)
    const [error, setError] = useState<string | null>(null)
    // A purge deletes the hat's path rules on the server: every purge tried,
    // whether it finished or not, has the rules read again.
    const [purges, setPurges] = useState(0)
    const purgeTried = useRef(false)

    // As functions of the list held, so two changes landing together both
    // stay.
    const replace = (hat: HatItem) =>
      hats.set((prev) =>
        (prev ?? []).map((h) => {
          if (h.id === hat.id) return hat
          // Only one hat is the default for new hosts.
          return hat.default_for_new_hosts ? { ...h, default_for_new_hosts: false } : h
        }),
      )

    const askPurge = async (hat: HatItem) => {
      setError(null)
      try {
        setPurge({ hat, preview: await purgePreview(client, hat.id) })
      } catch (err) {
        setError(messageOf(err))
      }
    }

    return (
      <div className="manage">
        <header className="manage-head">
          <h1>Hats</h1>
        </header>
        {[hats.error, hosts.error, error].map(
          (shown, i) =>
            shown && (
              <p key={i} className="form-error" role="alert">
                <Text>{shown}</Text>
              </p>
            ),
        )}
        {purged && (
          <PurgeOutcome name={purged.name} result={purged.result} onClose={() => setPurged(null)} />
        )}
        <ul className="cards" aria-label="Hats">
          {(hats.data ?? []).map((hat) => (
            <HatCard
              key={hat.id}
              hat={hat}
              isDefault={isDefault(hat, hosts.data ?? [])}
              onChanged={replace}
              onPurge={() => askPurge(hat)}
            />
          ))}
        </ul>
        <NewHat onCreated={(hat) => hats.set((prev) => [...(prev ?? []), hat])} />
        <PathRules
          hosts={(hosts.data ?? []).filter((h) => h.revoked_at === undefined)}
          hats={hats.data ?? []}
          purges={purges}
        />
        {purge && (
          <PurgeDialog
            hat={purge.hat}
            preview={purge.preview}
            hosts={hosts.data ?? []}
            onTried={() => (purgeTried.current = true)}
            onPurged={(result) => setPurged({ name: purge.hat.name, result })}
            onClose={() => {
              setPurge(null)
              // A purge that failed may have frozen the hat ("Resume purge")
              // and deleted its rules already: both are read again.
              if (purgeTried.current) {
                purgeTried.current = false
                hats.reload()
                setPurges((n) => n + 1)
              }
            }}
          />
        )}
      </div>
    )
  }

  function Swatch({ colour }: { colour: string }) {
    const safe = safeColour(colour)
    return <span className="swatch" aria-hidden="true" style={safe ? { background: safe } : undefined} />
  }

  function HatCard({
    hat,
    isDefault,
    onChanged,
    onPurge,
  }: {
    hat: HatItem
    isDefault: boolean
    onChanged: (hat: HatItem) => void
    onPurge: () => void
  }) {
    const client = useClient()
    const [editing, setEditing] = useState(false)
    const [name, setName] = useState(hat.name)
    const [colour, setColour] = useState(safeColour(hat.colour) ?? NEW_COLOUR)
    const [busy, setBusy] = useState(false)
    const [error, setError] = useState<string | null>(null)
    const titleId = `hat-${hat.id}`
    // A closed form or a done "Make default for new hosts" takes the focused
    // control away: focus goes to "Edit", or to the card's title.
    const title = useRef<HTMLHeadingElement>(null)
    const editButton = useRef<HTMLButtonElement>(null)
    const refocus = useRef<'edit' | 'title' | null>(null)
    useEffect(() => {
      if (refocus.current === null) return
      const to = refocus.current === 'edit' ? editButton.current : title.current
      refocus.current = null
      to?.focus()
    })

    const closeForm = () => {
      refocus.current = 'edit'
      setEditing(false)
    }

    const change = async (body: Parameters<typeof updateHat>[2], then: 'edit' | 'title') => {
      setBusy(true)
      setError(null)
      try {
        onChanged(await updateHat(client, hat.id, body))
        refocus.current = then
        setEditing(false)
      } catch (err) {
        setError(messageOf(err))
      } finally {
        setBusy(false)
      }
    }

    const save = (e: FormEvent) => {
      e.preventDefault()
      const body: Parameters<typeof updateHat>[2] = {}
      if (name.trim() !== hat.name) body.name = name.trim()
      if (colour !== hat.colour) body.colour = colour
      if (Object.keys(body).length === 0) closeForm()
      else change(body, 'edit')
    }

    return (
      <li className="card hat" aria-labelledby={titleId}>
        <div className="card-head">
          <Swatch colour={hat.colour} />
          <h2 className="card-title" id={titleId} ref={title} tabIndex={-1}>
            <Text>{hat.name}</Text>
          </h2>
          {hat.default_for_new_hosts && <span className="tag">Default for new hosts</span>}
          {hat.purging && <span className="tag tag-warn">Being purged</span>}
        </div>
        {editing ? (
          <form className="inline-form" onSubmit={save}>
            <label className="field">
              <span className="field-label">Name</span>
              <input className="text-input" value={name} maxLength={64} required onChange={(e) => setName(e.target.value)} />
            </label>
            <label className="field">
              <span className="field-label">Colour</span>
              <input type="color" className="colour-input" value={colour} onChange={(e) => setColour(e.target.value.toLowerCase())} />
            </label>
            <button type="submit" className="btn btn-primary btn-sm" disabled={busy || name.trim() === ''}>
              Save
            </button>
            <button type="button" className="btn btn-ghost btn-sm" onClick={closeForm} disabled={busy}>
              Cancel
            </button>
          </form>
        ) : (
          <div className="card-actions">
            <button
              type="button"
              className="btn btn-ghost btn-sm"
              ref={editButton}
              onClick={() => {
                setName(hat.name)
                setColour(safeColour(hat.colour) ?? NEW_COLOUR)
                setEditing(true)
              }}
              disabled={hat.purging}
            >
              Edit
            </button>
            {!hat.default_for_new_hosts && !hat.purging && (
              <button
                type="button"
                className="btn btn-ghost btn-sm"
                onClick={() => change({ default_for_new_hosts: true }, 'title')}
                disabled={busy}
              >
                Make default for new hosts
              </button>
            )}
            <span className="spacer" />
            <button type="button" className="btn btn-danger btn-sm" onClick={onPurge} disabled={isDefault && !hat.purging}>
              {hat.purging ? 'Resume purge' : 'Purge'}
            </button>
          </div>
        )}
        {isDefault && !hat.purging && (
          <p className="hint">A default hat cannot be purged: make another hat the default first.</p>
        )}
        {error && (
          <p className="form-error" role="alert">
            <Text>{error}</Text>
          </p>
        )}
      </li>
    )
  }

  function NewHat({ onCreated }: { onCreated: (hat: HatItem) => void }) {
    const client = useClient()
    const [name, setName] = useState('')
    const [colour, setColour] = useState(NEW_COLOUR)
    const [busy, setBusy] = useState(false)
    const [error, setError] = useState<string | null>(null)
    const nameInput = useRef<HTMLInputElement>(null)

    const submit = async (e: FormEvent) => {
      e.preventDefault()
      setBusy(true)
      setError(null)
      try {
        onCreated(await createHat(client, { name: name.trim(), colour }))
        setName('')
        // "Create" is disabled with the name empty: focus goes to the name,
        // ready for the next hat.
        nameInput.current?.focus()
      } catch (err) {
        setError(messageOf(err))
      } finally {
        setBusy(false)
      }
    }

    return (
      <section className="card" aria-labelledby="new-hat-title">
        <h2 className="card-title" id="new-hat-title">
          New hat
        </h2>
        <form className="inline-form" onSubmit={submit}>
          <label className="field">
            <span className="field-label">Name</span>
            <input
              ref={nameInput}
              className="text-input"
              value={name}
              maxLength={64}
              required
              onChange={(e) => setName(e.target.value)}
            />
          </label>
          <label className="field">
            <span className="field-label">Colour</span>
            <input type="color" className="colour-input" value={colour} onChange={(e) => setColour(e.target.value.toLowerCase())} />
          </label>
          <button type="submit" className="btn btn-primary btn-sm" disabled={busy || name.trim() === ''}>
            Create
          </button>
        </form>
        {error && (
          <p className="form-error" role="alert">
            <Text>{error}</Text>
          </p>
        )}
      </section>
    )
  }

  function PurgeDialog({
    hat,
    preview,
    hosts,
    onTried,
    onPurged,
    onClose,
  }: {
    hat: HatItem
    preview: PurgePreview
    hosts: HostItem[]
    onTried: () => void
    onPurged: (result: PurgeResult) => void
    onClose: () => void
  }) {
    const client = useClient()
    const state = purgeState(hat, hosts, preview)
    const blocked = state === 'default' || state === 'running'
    return (
      <ConfirmDialog
        title={state === 'resume' ? 'Resume this hat’s purge?' : 'Purge this hat?'}
        confirm={state === 'resume' ? 'Resume purge' : 'Purge'}
        danger
        disabled={blocked}
        action={async () => {
          onTried()
          onPurged(await purgeHat(client, hat.id))
        }}
        onClose={onClose}
      >
        <p>
          Purging <Text>{hat.name}</Text> deletes, for good:
        </p>
        <ul className="purge-list">
          <li>{count(preview.sessions, 'session')} of the hat, on every host, with their transcripts</li>
          <li>{count(preview.rules, 'path rule')}</li>
          <li>{count(preview.recents, 'recent project')}</li>
          <li>the hat itself, with its push policy</li>
        </ul>
        {state === 'default' && (
          <p className="form-error">
            This hat is the default for new hosts, or a host’s default hat. Make another hat that default first.
          </p>
        )}
        {state === 'running' && (
          <>
            <p className="form-error">Close these sessions first: they are running on a host hennery reaches.</p>
            <SessionIds ids={preview.running} link />
          </>
        )}
        {state === 'resume' && <p>A purge of this hat began and did not finish; this finishes it.</p>}
        {preview.unassigned_count > 0 && (
          <>
            <p>
              {count(preview.unassigned_count, 'session')} belong to no hat. No purge deletes them; re-assign or delete
              them one by one:
            </p>
            <ul className="purge-list">
              {preview.unassigned.map((s) => (
                <li key={s.session_id}>
                  <Link to={`/sessions/${encodeURIComponent(s.session_id)}`}>
                    <Text>{s.title ?? s.cwd}</Text>
                  </Link>
                </li>
              ))}
            </ul>
          </>
        )}
      </ConfirmDialog>
    )
  }

  /** Session ids, as links to the sessions when they still exist. */
  function SessionIds({ ids, link }: { ids: string[]; link?: boolean }) {
    return (
      <ul className="purge-list mono">
        {ids.map((id) => (
          <li key={id}>
            {link ? (
              <Link to={`/sessions/${encodeURIComponent(id)}`}>
                <Text>{id}</Text>
              </Link>
            ) : (
              <Text>{id}</Text>
            )}
          </li>
        ))}
      </ul>
    )
  }

  function PurgeOutcome({ name, result, onClose }: { name: string; result: PurgeResult; onClose: () => void }) {
    const t = result.host_transcripts
    // The purged hat's card, and the button that opened the dialog, are
    // gone: focus comes here.
    const title = useRef<HTMLHeadingElement>(null)
    useEffect(() => title.current?.focus(), [])
    return (
      <section className="card notice" aria-labelledby="purged-title" role="status">
        <h2 className="card-title" id="purged-title" ref={title} tabIndex={-1}>
          Purged <Text>{name}</Text>
        </h2>
        <p>
          Deleted {count(result.sessions, 'session')} and {count(result.rules, 'path rule')}.
        </p>
        {result.unconfirmed.length > 0 && (
          <>
            <p>
              {count(result.unconfirmed.length, 'session')} were deleted while their host was away; the host closes them
              when it is back:
            </p>
            <SessionIds ids={result.unconfirmed} />
          </>
        )}
        <p>
          The agents’ own transcripts on the hosts: {t.removed} removed, {t.partial} removed in part, {t.pending} still to
          remove when their host is back.
        </p>
        <div className="card-actions">
          <button type="button" className="btn btn-ghost btn-sm" onClick={onClose}>
            Close
          </button>
        </div>
      </section>
    )
  }
  ```


- [ ] **Step 4: Run the checks**

  Run: `nix develop -c sh -c 'pnpm --dir web typecheck && pnpm --dir web test'`
  Expected: all pass. 274 Vitest tests.

- [ ] **Step 5: Revert-probes** (each must fail `Hats.test.tsx`; all were run)
  - `Hats.tsx`: drop the dialog's `disabled={blocked}`; the card's purge button enabled for a default hat.
  - `PathRules.tsx`:
    - keep the typed rows rather than the server's answer after a save;
    - `TEST_DELAY_MS` 0 (one question per key);
    - drop the aborted check on a success (an older answer lands);
    - the tester's error unescaped (`<bdi>` alone);
    - drop the tester's words for `host_offline`;
    - drop the disabled option for a rule's hat being purged;
    - drop the purge or the save from the tester's dependencies, or from the rules' key ("reads the rules again after a purge", "asks again after the rules are saved");
    - drop the blank-path check or its words; drop the tester's reset to idle; nest its answer in a second live region;
    - drop the focus move after removing a rule.
  - `Hats.tsx`: fold an answer into the list as it was when the action began (both updaters); keep the other hats' default mark when one is made the default; `replace` not called after a rename; `onPurged` dropped; drop the outcome's focus or its `tabIndex`; let a name of spaces be saved; drop each focus return (Edit, the title and its `tabIndex`, the name); drop the hosts' error; drop the reload when the purge dialog closes.
  - `manage.css`: drop `.colour-input` from the 44 px rule.

- [ ] **Step 6: Commit**

  `git add -A && git commit -m "feat(web): hats: create, recolour, path rules with a live tester, and purge"`

---

### Task 4: Playwright: a host, a path, a revoke

- [ ] **Step 1: The test host and the checks**

In `web/e2e/collector.ts`, replace:

  ```ts
  // writes. Stopped by its own process id.
  ```

with:

  ```ts
  // writes. Stopped by its own process id. Every binary the browser checks
  // start runs in `scratchEnv`: nothing of the runner's own hennery setup,
  // home or XDG directories reaches it.
  ```

In `web/e2e/collector.ts`, replace:

  ```ts
  const BIN = process.env.HENNERY_BIN ?? resolve(process.cwd(), '../target/debug/hennery')
  ```

with:

  ```ts
  export const BIN = process.env.HENNERY_BIN ?? resolve(process.cwd(), '../target/debug/hennery')

  /** The runner's environment without any `HENNERY_*` variable (a data or
   *  log directory, a service flag: each would send the binary to the
   *  runner's own files), with its home and XDG directories under `dir`. */
  export function scratchEnv(dir: string): NodeJS.ProcessEnv {
    const env = { ...process.env }
    for (const key of Object.keys(env)) if (/^HENNERY_/.test(key)) delete env[key]
    return {
      ...env,
      HOME: dir,
      XDG_DATA_HOME: join(dir, 'xdg-data'),
      XDG_CONFIG_HOME: join(dir, 'xdg-config'),
      XDG_CACHE_HOME: join(dir, 'xdg-cache'),
      XDG_STATE_HOME: join(dir, 'xdg-state'),
    }
  }
  ```

In `web/e2e/collector.ts`, replace:

  ```ts
      env: { ...process.env, RUST_LOG: 'warn' },
    })
    let stderr = ''
    child.stderr?.on('data', (chunk: Buffer) => (stderr += chunk.toString()))
  ```

with:

  ```ts
      env: { ...scratchEnv(dir), RUST_LOG: 'warn' },
    })
    let stderr = ''
    child.stderr?.on('data', (chunk: Buffer) => (stderr += chunk.toString()))
    // A binary that cannot start fails the test that asked for it, rather
    // than the worker.
    child.once('error', (err) => (stderr += `${err}`))
  ```

Create `web/e2e/host.ts`:

  ```ts
  // A test host for the browser checks: the built `hennery` binary paired
  // with `host join` (the exact command the Hosts screen shows, with the
  // options a hermetic run needs), then run with one stand-in agent, so no
  // adapter is ever downloaded. Its directory is fresh, and every process is
  // stopped by its own id.
  import { spawn, type ChildProcess } from 'node:child_process'
  import { mkdtempSync, rmSync } from 'node:fs'
  import { tmpdir } from 'node:os'
  import { join } from 'node:path'
  import { BIN, scratchEnv } from './collector'

  /** SIGTERM, then SIGKILL if it has not gone within 5 s. One that never
   *  started (no pid) has nothing to stop. */
  async function end(child: ChildProcess): Promise<void> {
    if (child.pid === undefined || child.exitCode !== null || child.signalCode !== null) return
    const gone = new Promise((done) => child.once('exit', done))
    child.kill('SIGTERM')
    const timer = setTimeout(() => child.kill('SIGKILL'), 5000)
    await gone
    clearTimeout(timer)
  }

  export interface TestHost {
    dir: string
    /** Runs `command` (as the page shows it) with `extra` options; resolves
     *  with its exit code. */
    join(command: string, extra: string[]): Promise<number>
    /** `host run` in the background, until `stop`. */
    run(): void
    stop(): Promise<void>
  }

  export function testHost(): TestHost {
    const dir = mkdtempSync(join(tmpdir(), 'hennery-e2e-host-'))
    // Its home is the fresh directory too: the host reads $HOME (for `~`).
    const env = { ...scratchEnv(dir), HENNERY_HOST_DATA_DIR: dir, RUST_LOG: 'warn' }
    const children: ChildProcess[] = []
    return {
      dir,
      join(command, extra) {
        const words = command.trim().split(/\s+/)
        if (words[0] !== 'hennery') throw new Error(`not a hennery command: ${command}`)
        const child = spawn(BIN, [...words.slice(1), ...extra], { stdio: ['ignore', 'ignore', 'pipe'], env })
        children.push(child)
        let stderr = ''
        child.stderr?.on('data', (chunk: Buffer) => (stderr += chunk.toString()))
        return new Promise((done, fail) => {
          child.once('error', fail)
          child.once('exit', (code) => {
            if (code !== 0) console.error(`host join exited ${code}: ${stderr}`)
            done(code ?? -1)
          })
        })
      },
      run() {
        // The stand-in agent is never started (no session runs here); any
        // path that exists will do, and the binary's own does everywhere.
        const runner = spawn(BIN, ['host', 'run', '--data-dir', dir, '--agent', `stand-in=${BIN}`], {
          stdio: ['ignore', 'ignore', 'pipe'],
          env,
        })
        let stderr = ''
        runner.stderr?.on('data', (chunk: Buffer) => (stderr += chunk.toString()))
        runner.once('error', (err) => console.error(`host run failed to start: ${err}`))
        runner.once('exit', (code) => {
          if (code !== 0 && code !== null) console.error(`host run exited ${code}: ${stderr}`)
        })
        children.push(runner)
      },
      async stop() {
        // Every process this host started, the join too, however the test
        // ended; then its directory, which holds the host's key.
        try {
          await Promise.all(children.map(end))
        } finally {
          rmSync(dir, { recursive: true, force: true })
        }
      },
    }
  }
  ```

Create `web/e2e/manage.spec.ts`:

  ```ts
  // Hosts and hats in a real browser, against the real binary (plan 4d): a
  // host paired with the command the page shows, a revoke that steps up and
  // goes through, and the path tester asking that host, at 1280 px and at
  // 390 px, each width with a collector and a host of its own.
  import { expect, test, type Page } from '@playwright/test'
  import { mkdirSync, mkdtempSync, readdirSync, realpathSync, rmSync } from 'node:fs'
  import { tmpdir } from 'node:os'
  import { join } from 'node:path'
  import { scratchEnv, startCollector, type Collector } from './collector'
  import { testHost, type TestHost } from './host'

  const PASSWORD = 'correct horse battery staple'

  for (const width of [1280, 390]) {
    test.describe(`at ${width} px`, () => {
      test.describe.configure({ mode: 'serial' })

      let collector: Collector
      let host: TestHost
      let page: Page
      const violations: string[] = []
      // A log directory in the runner's own environment, as a developer's
      // shell may have: no binary started here may write to it.
      let sentinel: string
      let runnerLogDir: string | undefined

      test.beforeAll(async ({ browser }) => {
        sentinel = mkdtempSync(join(tmpdir(), 'hennery-e2e-sentinel-'))
        runnerLogDir = process.env.HENNERY_LOG_DIR
        process.env.HENNERY_LOG_DIR = sentinel
        expect(scratchEnv(sentinel).HENNERY_LOG_DIR).toBeUndefined()
        collector = await startCollector()
        host = testHost()
        const context = await browser.newContext({ baseURL: collector.origin, viewport: { width, height: 844 } })
        page = await context.newPage()
        // Every CSP violation the page sees, whatever its source.
        await page.addInitScript(() => {
          document.addEventListener('securitypolicyviolation', (e) => {
            const seen = ((window as unknown as { __csp?: string[] }).__csp ??= [])
            seen.push(`${e.violatedDirective} ${e.blockedURI}`)
          })
        })
        page.on('console', (m) => {
          if (m.text().includes('Content Security Policy')) violations.push(m.text())
        })
        // Set up through the API, with the token from the setup link: the
        // session it opens is stepped up for five minutes.
        const token = new URL(collector.setupLink).hash.slice(1)
        const setup = await page.request.post('/api/setup', {
          headers: { Origin: collector.origin },
          data: { token, password: PASSWORD, public_url: collector.origin },
        })
        expect(setup.status()).toBe(201)
      })

      test.afterAll(async () => {
        try {
          await page?.context().close()
        } finally {
          try {
            await host?.stop()
          } finally {
            try {
              await collector?.stop()
            } finally {
              if (runnerLogDir === undefined) delete process.env.HENNERY_LOG_DIR
              else process.env.HENNERY_LOG_DIR = runnerLogDir
              if (sentinel) rmSync(sentinel, { recursive: true, force: true })
            }
          }
        }
      })

      test('a host pairs with the command the page shows, and the code goes', async () => {
        await page.goto('/hosts')
        await page.getByRole('button', { name: 'Add host' }).click()
        const command = (await page.getByLabel('Pairing command').textContent())!
        expect(command).toMatch(new RegExp(`^hennery host join ${collector.origin} [0-9A-Z]{4}-[0-9A-Z]{4}$`))
        const code = command.split(' ').at(-1)!
        expect(await host.join(command, ['--name', 'e2e host', '--no-runtime'])).toBe(0)
        await expect(page.getByText('Paired: e2e host')).toBeVisible({ timeout: 15_000 })
        const card = page.getByRole('listitem', { name: 'e2e host' })
        await expect(card.getByText('Offline')).toBeVisible()
        // The code is spent, and gone from the page, its storage and its URL.
        expect(await page.content()).not.toContain(code)
        expect(await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage }))).not.toContain(code)
        expect(page.url()).not.toContain(code)
        host.run()
        await expect(async () => {
          await page.reload()
          await expect(page.getByRole('listitem', { name: 'e2e host' }).getByText('Online')).toBeVisible({ timeout: 1000 })
        }).toPass({ timeout: 20_000 })
      })

      test('the path tester asks the host, and a saved rule changes its answer', async () => {
        const project = realpathSync(host.dir)
        mkdirSync(join(project, 'work'), { recursive: true })
        await page.goto('/hats')
        await page.getByRole('textbox', { name: 'Name' }).fill('Work')
        await page.getByRole('button', { name: 'Create' }).click()
        await expect(page.getByRole('listitem', { name: 'Work' })).toBeVisible()
        const tester = page.getByLabel('Test a path')
        await tester.fill(join(project, 'work'))
        const resolution = page.getByLabel('Resolution')
        await expect(resolution).toContainText(join(project, 'work'))
        await expect(resolution).toContainText('Personal')
        await expect(resolution).toContainText('the host’s default hat')
        await page.getByRole('button', { name: 'Add rule' }).click()
        await page.getByLabel('Path 1').fill(join(project, 'work'))
        await page.getByLabel('Hat 1').selectOption({ label: 'Work' })
        await page.getByRole('button', { name: 'Save rules' }).click()
        await expect(page.getByText('Saved.')).toBeVisible()
        await tester.fill(join(project, 'work', 'app'))
        await expect(resolution).toContainText(join(project, 'work', 'app'))
        await expect(resolution).toContainText('Work')
        await expect(resolution).toContainText('a path rule')
        await expect(resolution).toContainText('does not exist')
      })

      test('a revoke asks for a fresh confirmation, then goes through', async () => {
        await page.goto('/hosts')
        // The session is still stepped up from setup; the server's refusal is
        // what the browser would get five minutes on. The first DELETE is
        // answered so, and the retry reaches the server. That the server
        // refuses without a step-up is the Rust tests'.
        let refused = 0
        await page.route('**/api/hosts/*', async (route) => {
          if (route.request().method() === 'DELETE' && refused === 0) {
            refused++
            await route.fulfill({
              status: 403,
              contentType: 'application/json',
              body: JSON.stringify({ code: 'step_up_required', message: 'confirm it is you' }),
            })
          } else {
            await route.continue()
          }
        })
        const card = page.getByRole('listitem', { name: 'e2e host' })
        await card.getByRole('button', { name: 'Revoke' }).click()
        const confirm = page.getByRole('dialog', { name: 'Revoke this host?' })
        await expect(confirm).toContainText('stop only when it next connects')
        await confirm.getByRole('button', { name: 'Revoke' }).click()
        const stepUp = page.getByRole('dialog', { name: 'This needs a fresh confirmation' })
        await expect(stepUp).toBeVisible()
        // The page under it is inert while it is open: the confirmation
        // beneath can take no focus, by script or by keyboard.
        expect(await page.locator('.page').getAttribute('inert')).not.toBeNull()
        await expect(stepUp.getByLabel('Your password')).toBeFocused()
        // The confirmation itself, not its buttons: they are disabled while
        // its action waits, and a disabled button takes no focus anyway.
        await confirm.evaluate((d: HTMLElement) => d.focus())
        await expect(stepUp.getByLabel('Your password')).toBeFocused()
        // Shift+Tab from the dialog's first field would land on the page
        // before it, were the page not inert. The password is its first
        // field only while no passkey is offered.
        await expect(stepUp.getByRole('button', { name: 'Confirm with passkey' })).toHaveCount(0)
        await page.keyboard.press('Shift+Tab')
        expect(await page.evaluate(() => !!document.activeElement?.closest('.page'))).toBe(false)
        await stepUp.getByLabel('Your password').focus()
        await stepUp.getByLabel('Your password').fill(PASSWORD)
        await stepUp.getByRole('button', { name: 'Confirm' }).click()
        await expect(card.getByText('Revoked', { exact: true })).toBeVisible()
        expect(refused).toBe(1)
        expect(await page.locator('.page').getAttribute('inert')).toBeNull()
        await page.unroute('**/api/hosts/*')
      })

      test('broke no Content-Security-Policy rule', async () => {
        expect(violations).toEqual([])
        const seen = await page.evaluate(() => (window as unknown as { __csp?: string[] }).__csp ?? [])
        expect(seen).toEqual([])
      })

      test('wrote nothing where the runner’s own environment pointed', async () => {
        expect(readdirSync(sentinel)).toEqual([])
      })
    })
  }
  ```


- [ ] **Step 2: Run the checks**

  Run: `nix develop -c sh -c 'pnpm --dir web build && HENNERY_WEB_REQUIRE=1 cargo build -p hennery --locked && pnpm --dir web e2e'`
  Expected: 17 Playwright tests pass (4b's 7 and these 10), and `ps -ax | grep -i 'hennery '` shows none of the run's processes afterwards.

- [ ] **Step 3: Revert-probes** (each must fail `pnpm --dir web e2e`; all were run)
  - drop `inert={…}` in `App.tsx`: the revoke check finds the page not inert;
  - drop `inert={…}` and the check of the attribute: a script's `focus()` lands on the confirmation under the dialog;
  - drop `inert={…}`, the check of the attribute and the scripted focus: Shift+Tab from the dialog's first field lands on the page;
  - drop the paired `setStage` in `Pairing.tsx`: "Paired: e2e host" never shows;
  - drop the revoke dialog's sentence about running agents;
  - the collector, then `host join` and `host run`, given the runner's environment again: the sentinel log directory gets files;
  - (`code-kept`) keep the spent code in the Hosts screen's state and show it in the paired notice: the page still holds the spent code.

- [ ] **Step 4: Commit**

  `git add -A && git commit -m "test(web): pair a host, test a path and revoke with a step-up, in Chromium at two widths"`

---

## After this plan

**What 4c, 4d-ii and later parts can use:**
- `visible()` and `<Text>` (`lib/text.tsx`) for any server string; `ConfirmDialog` for any action that needs a confirmation before its step-up; `<When>` for a server time; `useResource` for a screen's read.
- `test-fixtures.ts`: one `HostItem`, `HatItem` and `PurgePreview` with every field set.
- `e2e/host.ts`: `testHost()` pairs and runs a real host with no adapter download, for 4c's session checks (with a fake adapter as its `--agent`).

**Backend follow-ups** (the lane's ruling of 2026-10-02; each is its own plan):
- **4d-B1:** a host's agents, their availability and login state, its adapter versions, its last doctor result, and the notices of frontend §8 (restart needed, adapter set off the pin, outbox over its bound, another connection refused). `HostItem` has none of them; the host card's facts list takes their rows in 4d-iii.
- **4d-B2:** hat logos (`GET/PUT /api/hats/{id}/logo`), rendered as `<img>` only, in 4d-iii.
- **4d-B3:** the deployment warning's signal (kernel §10), for 4d-iv.
- **4d-B4:** `PATCH /api/settings {public_url}`, for 4d-iv.
- **The count of parked sessions no rule covers** (5a's hand-off), for the default-hat warning: no route gives it.
- **`hennery host join` without `--data-dir`:** the command the page shows (frontend §8) needs `--data-dir` or `HENNERY_HOST_DATA_DIR` today. The distribution lane takes a default host data dir; then the page drops its hint, and the browser check runs the command with no variable set.
- **Revoking a pairing code** before it expires: no route; closing the panel only hides it.

**Deferred** (owner named):
- A stream of host changes, so a host coming online shows without a reload (4d-iii, with the agents).
- Hat names are unique in any case, but not under Unicode normalisation (5a's O3): two names can look alike. The UI shows them escaped, not normalised.

**Not tested here:**
- Browsers other than Chromium (4b's O8).
- A real 403 `step_up_required` in the browser: see decision 15.
- The countdown across a laptop's sleep: `performance.now()` stops in some browsers while asleep, so the code can look valid after it expired; the server refuses it then (`invalid_code`), and `host join` says so.

**Spec amendments** (to write back): frontend §8: the pairing panel's hint about `--data-dir` and standard input; a host's default hat changed from the host card with a confirmation; purge's result shown as listed.

**The security review's answers** (2026-10-02, on the maintainer's behalf):
1. **Step-up:** the server's layer covers exactly the actions this plan steps up; creating a hat, the purge preview and the tester rightly need none. Every action goes through the client's one retry, and each has a test of two identical sends (A6 completed the path rules' one).
2. **The pairing code:** minted on click, in state only, absent from the URL, title and storage, by unit and browser tests; `public_url` shown as stored is right (escaping would break copying). A1 and O1–O3 taken.
3. **Escaping:** `<Text>` and `visible()` are used for every server string; Mn, Co and unassigned code points rightly excluded. A2 and A3 taken.
4. **Colours:** only `^#[0-9a-f]{6}$`, matching the server's own check.
5. **`inert` and the dialog stack:** `inert` on a `display: contents` element works (inertness is the element's and its children's, not its box's). A4 taken; O4's Chromium check added (by focus since the re-confirmation).
6. **The failed sign-out:** honest and safe; the cookie is `HttpOnly`. Settings (4d-ii) adds revoking a device from another.
7. **The browser checks:** hermetic; faking the first 403 is acceptable, as `require_step_up` runs before the handler and `step_up.rs` covers the server's refusal. A5 and O4 taken.
8. **Nothing imported:** holds.
9. **Other:** every classifier outcome has its test; the stale-answer probe is valid; unmounted updates are guarded; every id in a path is encoded.

---

_Generated with Claude AI — please review before distribution._
