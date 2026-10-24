# hennery — UI redesign

- **Date:** 2026-10-20
- **Operator amendment, 2026-10-21:** per-hat Delegation is strictly binary.
  ON permits all live catalog-offered runtime/model/mode/option values; OFF
  denies delegation. No execution grants, policy snapshot or grant editor.
  Enabling writes only the flag and never probes hosts.
- **Status:** Proposed implementation contract; independent design/security review
  and scoped confirmation belong to the fleet parent. Recommendations below
  are not recorded as operator approvals.
- **Review:** Independent review of `b641f40f..0ebd3f7a` returned APPROVE
  AFTER AMENDMENTS. Findings 1–4 are addressed below; scoped re-confirmation
  remains with the parent. O8 records the operator's subsequent font decision.
- **Base:** `b641f40f`, the `redesign` integration branch.
- **Amends:** [frontend](2026-09-26-frontend-design.md) §§2, 5–8, 10 and
  [client view](2026-10-02-client-view-design.md) §5's visual reuse decision.
  The collector fold, ownership, authentication and delivery contracts remain
  authoritative. The [umbrella](2026-09-25-hennery-architecture-design.md)
  wins on architecture; [kernel](2026-09-26-kernel-design.md),
  [phone](2026-10-14-phone-design.md) and
  [delegation](2026-10-14-delegation-design.md) supply the security boundaries.
- **Design evidence:** the operator's local Claude Design bundle, especially
  `Hennery Mobile.dc.html`, then `Hennery Desktop.dc.html`,
  `Hennery Design System.dc.html`, `hennery-tokens.css`, `hicon.js` and the
  design system's `colors_and_type.css`. `Current UI (before).dc.html` is
  comparison evidence. The bundle stays outside version control; no prototype
  HTML, harness, fixture content or private example data is shipped.

This is an element-by-element contract for one redesigned release, plus
persisted per-hat session defaults. R1–R6 below are implementation boundaries,
not executable plans. Each receives its own reviewed plan after this spec is
approved. No application code or plan files are part of this change.

## 1. Decisions and authority

| Id | Decision | Authority / reason |
|---|---|---|
| O1 | Build Defaults with persisted per-hat model, permission mode and all supported catalog axes. Resolve agent default → hat override → New session choice. | Operator, 2026-10-20. |
| O2 | Notification policy remains per hat: `muted`, `details`, `generic_title`. | Operator, 2026-10-20. Drop global per-event switches. |
| O3 | No lock-screen answer actions. Notification taps must reach the specific session and, when supplied, its pending card. | Operator, 2026-10-20. |
| O4 | Delegation authorization lives in Settings, using existing GET/PATCH session-tools, step-up and truthful activation/disable copy. | Operator, 2026-10-20. |
| O5 | Retain the hat-logo backend and wire field; render no hat logos or logo editor in this UI. | Operator, 2026-10-20. |
| O6 | Accent comes from the hat. Implement both light and dark; use design surfaces, text and status colors. | Operator, 2026-10-20. Fixed prototype blue is not the application accent. |
| O7 | Stack all redesign work into `redesign`; release through one final integration PR to `main`. | Operator, 2026-10-20. No partial redesign release. |
| O8 | Use the design system's Roboto for headings, body/content (including bubbles and markdown), buttons, labels and links; Inter only for form values, table cells and dense data; commit self-hosted WOFF2 latin + latin-ext subsets, including Polish characters, with `font-display: swap` and OFL license/notice. No npm/fontsource package, remote font load or CSP change. R1 owns this. | Operator, 2026-10-21; corrects the prototype Inter-body mapping recorded 2026-10-20, replaces and removes D1. |
| D2 | Keep CSS variables and existing class-based CSS with Tailwind 4. Add semantic tokens and temporary aliases, rather than migrate components to another styling system. | **designer's recommendation**: fits `index.css`/`manage.css` and protects the Markdown scope. |
| D3 | Use system/light/dark appearance, browser-persisted, through the blocking external `/theme.js`; derive contrast-checked palettes per theme. | **designer's recommendation**; CSP/no-inline behavior is settled by the implementation. |
| D4 | Use `/needs-you`, `/sessions`, `/new`, `/more`, `/defaults` and existing management/session routes; desktop Sessions becomes an overview without automatic session selection. | **designer's recommendation**: follows the new overview and mobile tabs, superseding the old list/detail rail layout. |
| D5 | Count actual operator attention, never infer a question from assistant prose. Queue previews are read-only and open the canonical session card for answering. | **designer's recommendation**: current summaries cannot authorize a quick answer; avoids a second answer-book/stream implementation. |
| D6 | Show supported data only; omit invented progress, diagnostics, version/auth claims and password-change metadata. | **designer's recommendation**: this release adds defaults, not a diagnostics or account API. |
| D7 | Agent baseline is read-only and comes from the selected host/cwd catalog. Persist sparse overrides by owner × hat × agent, not host or display name; on a different effective model, prune inherited nonsecurity axes as in §6.3. | **designer's recommendation**: the same hat spans different adapter pins and model-dependent axes; no new global agent-preference layer. |
| D8 | Kernel owns defaults storage; sessions owns its operator HTTP facade. Require step-up on every PUT, including resets. | **designer's recommendation**: hat lifetime belongs to the kernel; all-write step-up covers permissive and unknown security modes without trusting labels. |
| D9 | The launcher resolves and sends explicit config using existing start/config APIs. Defaults do not implicitly change resume or delegated spawn. | **designer's recommendation**: keeps operator preferences separate from agent authorization. |
| D10 | Unsupported stored values are omitted with a visible, field-specific notice; unavailable explicit picks block Start until corrected. Hold a first prompt when requested config is not confirmed. | **designer's recommendation**: no silent alternate model or weaker permission setting. |
| D11 | Use bundled Heroicons SVG paths, native controls where practical, and no new runtime dependency. | **designer's recommendation**: matches the design within current CSP and bundle limits. |
| D12 | Use the responsive geometry, focus/shortcut contract, hat-only/all-lifecycle Needs you query, honest paging and exclusive file ownership in §§3–10. Delegation has one binary switch, with no grant editor. | **operator amendment, 2026-10-21** for Delegation; remaining layout is the designer's recommendation. |

## 2. Existing implementation boundaries

The current app is React 19/strict TypeScript/Vite/pnpm, with Tailwind 4
imported into `web/src/index.css`, hand-written CSS classes, and a small
History API router. Keep those choices. `web/src/deps.test.ts` forbids
`rehype-raw` and `hast-util-raw`; that guard and the sanitized Markdown
pipeline remain unchanged. Do not port `support.js`, `_ds_bundle.js`, custom
design-tool elements, inline event handlers or CDN-fetching `hicon.js`.

Use generated `protocol.ts`/`view.ts`, the shared `Client` (401 return path,
step-up and one retry), keyed summary/item stores and AnswerBook. Read
display items, never refold raw ACP in the browser. Preserve resync/version
fences, bounded history, error boundaries, attachment limits, per-session
drafts, secret handling, popup/OAuth behavior and capability gating.

There remains at most one list SSE and one selected-session item SSE per tab.
Changing layout, opening More or using the palette must not create another
store or EventSource. Loading a preview never subscribes to its transcript.
No list refetch on each token or invalidation hint.

Important source anchors at this base:

| Concern | Existing source / contract |
|---|---|
| Shell, routing, hat scope | `components/Shell.tsx`, `components/SessionScope.tsx`, `router.tsx`, `lib/views.ts`, `lib/hats.ts` |
| Appearance | `lib/theme.ts`, `web/public/theme.js`, `index.css`, `manage.css`; `/theme.js` already runs before React |
| Launcher catalog | `GET /api/hosts/{id}/runtime-catalog?agent=…&cwd=…` reads the saved snapshot; `POST` on the same URL explicitly refreshes it; `HostCatalog {agent, adapter_revision, config_options, defaults, security, freshness, saved_at?}` |
| Session configuration | `StartSessionRequest {host_id, agent, cwd, model?, mode?, axes?}`; `POST /api/sessions/{id}/config`; returned `SessionCatalog` is actual adapter state |
| Attention | `SessionSummary.operator_attention`, `parent_handled_questions`, `question_waits`, `family`; `api/attention.ts` for real delegation fallback records |
| Hat authorization | GET/PATCH `/api/hats/{id}/session-tools`; binary delegation enablement only |
| Push | `crates/hennery-sessions/src/notify.rs`, `web/worker.js`, `pwa/notification.ts`, `pwa/navigation.ts`, `components/NotificationNavigation.tsx` |
| HTTP security | `crates/hennery-kernel/src/csp.rs`: scripts, connections, workers and manifest same-origin; default-src also restricts fonts |

## 3. Tokens, assets and responsive geometry

### 3.1 CSS mapping

R1 adds `web/src/styles/tokens.css`, imported after Tailwind. Light tokens
live on `:root`; dark overrides on `:root[data-theme="dark"]`. The theme
attribute always contains the resolved theme, not `system`. Do not import
the prototype's stylesheet, its Google Fonts import or global element rules.

| Design group | Production semantic variables | Mapping / values |
|---|---|---|
| Surfaces | `--hn-bg`, `--hn-panel`, `--hn-surface`, `--hn-sunken`, `--hn-hover`, `--hn-selected` | Light: `#f8f9fb`, white, white, `#f1f2f5`, `#f1f3f7`, derived accent wash. Dark: `#141517`, `#1a1c1e`, `#202223`, `#17191b`, `#26292b`, derived selected tint. |
| Dividers/controls | `--hn-line`, `--hn-control`, `--hn-control-strong` | Light: `#e9e9eb`, `#d8d8da`, `#c3c3c5`; dark: `#2a2d2f`, `#383b3e`, `#49494a`. Strengthen an interactive outline when needed for §3.3. |
| Text/icons | `--hn-fg-1/2/3`, `--hn-icon` | Light: `#202020`, `#49494a`, `#686869`, `#89898a`; dark: `#f2f2f3`, `#c3c3c5`, `#a8a8aa`, `#89898a`. Decorative icon color is not sufficient for an essential icon-only control. |
| Hat accent | `--hn-accent`, `--hn-accent-hover`, `--hn-accent-fg`, `--hn-on-accent`, `--hn-accent-wash`, `--hn-accent-soft`, `--hn-focus` | Derived by §3.3, including focus halo. All prototype fixed-blue selected, link, radio and button treatments use these. |
| Code/terminal | `--hn-code-bg/fg`, `--hn-term-bg/fg` | Light code `#f4f5f7`/`#202020`; dark code `#17191b`/`#e6e6e7`. Terminal light `#16181a`/`#e6e6e7`, dark `#0f1012`/`#e6e6e7`. |
| Diffs | `--hn-add-bg/fg`, `--hn-del-bg/fg` | Light `#e6f5e9`/`#0f741f`, `#fce8e9`/`#9f2626`; dark green/red 14% tints with `#8bd999`/`#f99c9e`. Keep +/− text and plain output fallback. |
| Status | `--st-{wait,run,start,idle,park,fail,warn}-{fg,bg,solid}`, plus `--st-wait-line` | Preserve design status families and paired themes. Wait orange, run green, start blue, idle/park neutral, fail red, warn yellow. These never inherit the hat hue. Contrast-correct the supplied shade where necessary. |
| Radius | `--radius-xs/sm/md/lg/xl/full` | 4/6/8/12/16/9999 px. Mobile cards 14 px and sheets 20 px are explicit frame variants. |
| Spacing | `--space-1/2/3/4/5/6/8/10/12/16/20/24` | 4/8/12/16/20/24/32/40/48/64/80/100 px, matching the source scale's actual final value. |
| Shadow/motion | `--shadow-xs`, `--hn-pop-shadow`, `--hn-scrim`, durations/easings | Design's subtle xs shadow, paired light/dark popover shadows, 50% dark-text or 60% black scrim; 120/180/240 ms, standard/out easing. Disable animation and smooth scrolling under reduced motion. |
| Typography | `--font-display`, `--font-body`, `--font-data`, `--font-mono` plus size classes | Self-hosted Roboto headings/body/content/buttons/labels and Inter form values/table cells/dense data, with system fallbacks; existing system mono stack. Desktop heading 18/1.2 medium, row title 13/1.3 medium, prose 14/1.55–1.6, mono 12–13/1.5; mobile heading 20, title 16, prose/input 15–16, labels 12–13. Section labels 11 bold uppercase; tab labels 10. |

The source color scales are local numeric inputs, not application branding:
neutral 050–1000, primary/orange/success/error 050–800 and warning 500.
Keep only needed scale entries in the token file. For example, source
`success-700` is `#128c25`; if it misses 4.5:1 on a rendered light tint,
use `success-800` (`#0f741f`) there. A prototype comment claiming contrast
is not evidence of it.

Status role inputs (foreground / tint / solid; derive the final
contrast-valid foreground rather than treating these as exceptions):

| Status | Light | Dark |
|---|---|---|
| Wait | `#a44d0f` / `#fde6d7` / `#ff7918`; border `#fdbb8d` | `#fdbb8d` / orange solid at 15% / same solid; border orange at 45% |
| Run | `#0f741f` / `#f1f9f4` / `#17b530` | `#8bd999` / green solid at 13% / same solid |
| Start | `#2257be` / `#f5f8ff` / `#2c71f6` | `#95b7fa` / blue solid at 16% / same solid |
| Idle | `#49494a` / `#f0f1f3` / `#a8a8aa` | `#c3c3c5` / `#2a2d2f` / `#89898a` |
| Park/offline | `#686869` / `#f0f1f3` / `#c3c3c5` | `#a8a8aa` / `#2a2d2f` / `#686869` |
| Fail | `#bf2e2e` / `#fcf6f8` / `#f73b3b` | `#f99c9e` / red solid at 14% / same solid |
| Warn | `#7a5300` / `#fff4d6` / `#ffbe18` | `#ffd666` / yellow solid at 13% / same solid |

Composite translucent tints over their actual surface for contrast tests;
status solids are decorative bars/dots unless they also pass the essential
indicator threshold. An icon-only status uses its foreground role.

During migration alias existing `--bg/rail/surface/border/fg-*/accent` and
status variables to the corresponding semantic tokens. Remove gradient,
glowing-button and fixed light-only management colors. `--accent-2` may
remain a compatibility alias to the derived hover value; new buttons are
solid. Keep scoped Markdown lists/code and the media-query visibility
guards. No wholesale utility-class conversion is required.

### 3.2 Theme selection and bootstrap

Appearance is a browser preference with three radio values: System (default),
Light, Dark. Follow `prefers-color-scheme` live only in System. Persist the
preference and a versioned pair of derived light/dark accent palettes under
`hennery.theme`; keep the existing selected-hat key. Migrate the old two-color
record once by deriving from its validated `--accent`, preserving hat choice.
Invalid/missing storage uses System and the neutral fallback palette.

Extend the existing blocking `web/public/theme.js`, referenced by external
`src` before the module entry, to set `data-theme`, `color-scheme`, the
allowlisted per-theme accent variables and the theme-color meta value before
paint. The saved record contains version, preference, source `#rrggbb`, and
two palettes of opaque hex colors. Check its shape and every property;
never assign arbitrary CSS properties or CSS strings from storage. Derive
new palettes in `lib/accent.ts` and save/apply through `lib/theme.ts`.
The bootstrap selects already-derived palettes and has a built-in safe
fallback, so it needs no asynchronous module import or network read.

React reconciles with server hats after loading without first clearing the
persisted accent. A deleted hat falls back to All. All uses the existing
neutral default hat-color input (`#64748b`), passed through the same derivation.
An explicitly opened session uses its actual server hat for the session
surface/composer; this must not change the overview filter. New session uses
the resolved hat. Other views use the selected hat. Cache the effective
accent for the next load, then correct it when route/server data is known.
Dark/light changes update both document and any locally scoped session palette.

The rail's theme button cycles System → Light → Dark → System and announces
the selected preference; Settings exposes all three directly. A keyboard
theme toggle uses the same cycle. Storage denial still permits changes for
the current document. Storage events reconcile other windows without
changing their current session or drafts. Login and Setup use the same
bootstrap and neutral tokens.

### 3.3 Any hat color must remain readable

Accept exactly `#rrggbb`, case-insensitive, otherwise fall back to `#64748b`.
Use a deterministic dependency-free sRGB ↔ OKLCH conversion. Preserve hue;
near-achromatic colors (`C < .02`) stay neutral. As a starting range clamp
chroma to .08–.18/light or .06–.16/dark and lightness to .42–.56/light or
.65–.80/dark. Reduce chroma to bring each result into sRGB gamut, rather
than clip RGB channels and shift hue. These ranges are starting points;
the contrast constraints below decide the final values.

Derive link/text, solid button, hover, wash, soft, selected, focus-ring and
on-button roles independently. Search lightness deterministically until the
roles meet their targets, reducing chroma as needed; if no colored result
works, use a contrast-valid neutral for that role. Light washes start near
L=.96/C≤.03, dark washes near L=.28/C≤.05. Serialize final opaque sRGB hex
values; test final rounded values and composited halos, not just OKLCH inputs.
Choose dark or white `--hn-on-accent` by measured contrast, never assume white.

- Normal text, links, selected labels and button labels: at least **4.5:1**
  against every surface/tint on which that role is used.
- Essential glyphs, selection indicators, interactive boundaries and the
  solid focus ring: at least **3:1** against adjacent surfaces. Supplement
  the faint design control border with a stronger outline where necessary.
- Distinguish status by icon and words as well as color. Do not recolor
  warning/error status to match the hat. Decorative separators may stay faint.
- Hover and selected states must preserve text contrast. Focus remains a
  visible 2 px ring plus 3 px halo, including around warning/destructive
  controls. Reduced motion does not remove focus.

Unit-test both themes over near-white, near-black, white, black, pure yellow,
pure blue, red/green/cyan/magenta and a deterministic grid of RGB colors.
Assert valid hex, all assigned role contrasts, stable output, invalid input
fallback and neutral handling. Theme/bootstrap tests cover malformed/legacy
storage, System changes and per-theme palettes. Browser computed-style tests
verify the actual cascade, including on-accent and status tints.

### 3.4 Fonts, icons and CSP

Use **Roboto** for headings, body/content (including message bubbles and
markdown), buttons, labels and links; use **Inter** only for form control
values (`input`, `select`, `textarea`), table cells and dense data. Table
headers are labels and remain Roboto; code and paths (including path entry
and directory suggestions) remain mono. This follows the
design system body-1/2/3 tokens and README typography mapping, as confirmed
by the operator on **2026-10-21** (O8), replacing the prototype Inter body
mapping. `--font-display` and `--font-body` select Roboto; `--font-data`
selects Inter. Commit the actual `.woff2` font assets under
`web/public/fonts/`, including **latin + latin-ext** subsets for both
families; Polish characters must render from these faces. Include every
weight/style used by the UI: normal and italic 400/500/600/700 for both
families, including Roboto italic for thinking text and markdown emphasis,
using variable faces where appropriate. Define local
`@font-face` rules with matching weight/style and Unicode ranges in
`styles/fonts.css`; use `font-display: swap`. System stacks remain fallbacks
while fonts load, not the selected production fonts. Keep system mono.

Use pinned OFL-licensed upstream font files; R1 records their upstream
version/source and checksums in the font notices. Commit each family's full
OFL license text/copyright alongside its notice under
`web/third-party-licenses/`, referenced from the shipped
`web/THIRD_PARTY_NOTICES.txt` and distribution notices. Do not add npm,
`@fontsource` or another font-fetching runtime/build dependency. Font files
ship through the existing Vite/static embedding and PWA precache pipeline;
all font URLs are same-origin and need **no CSP change**.

R1 adds a post-build test over **every shipped CSS asset**, including
imports and `@font-face` URLs, asserting it references no external origin.
Inspect emitted CSS, not only source files; cover absolute and network-path
external references in the check's regression fixtures. Browser acceptance
waits for both families, verifies normal/italic/weight selection, and renders
the Polish sample “Zażółć gęślą jaźń” in Inter and Roboto with their latin-ext
font assets loaded. Confirm no remote font request or CSP violation.

Bundle the used Heroicons v2 SVG paths as React components in
`web/src/components/ui/Icon.tsx`, using currentColor and the source's
24 outline/24 solid/20 solid/16 solid variants. Do not fetch icons, inject
SVG markup with `innerHTML`, or depend on the prototype custom element.
Preserve Heroicons' MIT copyright/license in a shipped
`web/THIRD_PARTY_NOTICES.txt` and include it in distribution notices.
Do not replace the repository's own AGPL license.

No new runtime dependency is proposed. Existing React/native controls cover
dialogs, radios, search and the palette; pure math covers color derivation.
Any implementation proposal adding a library must state its purpose,
bundle/license impact and why the existing stack is insufficient. Keep
`deps.test.ts` and the <350 KiB initial gzip budget. No inline theme script,
CDN, remote font, new network origin or CSP relaxation is authorized.

### 3.5 Layout and density

Under 768 px use one column and the five-item bottom navigation; session
pages use their own back/header and bottom answer/composer region, with
the global tab bar hidden to give that region space. At 768–1023 px use
a 56 px rail and one content column. At 1024 px and above use the 240 px
expanded rail (user-collapsible to 56 px); the session details panel is
300 px when open. At 1280 px this leaves a usable transcript; cap its
content at 760 px. Collapse the details panel into a sheet below 1024 px.

Desktop chrome header is 52 px; outer content padding 24 px, mobile 16 px.
Desktop controls are normally 28–32 px, rows at least 44 px. Every mobile
interactive hit area is at least 44×44 px, even when a chip's visible face
is 32 px; answer buttons are 48–52 px and option rows at least 56 px.
Management tables become labeled stacked cards on mobile, not horizontally
compressed desktop grids. Defaults becomes hat sections with Setting/value
rows and a host-context picker. Code blocks may scroll horizontally;
page, navigation, forms and composer must not.

Use `100dvh`, `min-height:0`/`min-width:0`, actual scroll containers,
`viewport-fit=cover` and safe-area padding. The device border, fake clock,
signal/Wi-Fi/battery and home indicator are prototype framing, not app UI.
Bottom answer/composer controls remain reachable with the onscreen keyboard.
Sheets scroll within the available viewport; they do not assume the
prototype's fixed height or top offset. One responsive component tree owns
each header/control, preventing duplicate hidden keyboard targets.

## 4. Routes, overview and navigation

Navigation shortcuts (operator request, 2026-10-22): a session's actions offer
“Pin to sidebar” / “Unpin from sidebar”. Pins are persisted by the collector
as `sidebar_pinned`, separately from the “Do not park” idle hold. The desktop
rail shows a Pinned section above Hats, with session names, direct links and
the current selection; compact navigation retains named icon links and tooltips.
Pins span hats and lifecycles and survive overview filters and reloads. Deleting
a session removes its shortcut. `PATCH /api/sessions/{id}` accepts
`sidebar_pinned: boolean`; view list pages include `sidebar_pins` independently
of their paged/filter result, sharing that page's stream anchor. The existing
list stream updates names, pin state and removals without a separate subscription.

### 4.1 Routes and capability behavior

| URL | Full mode | Gateway mode |
|---|---|---|
| `/`, `/sessions` | Sessions overview; `/` replaces to `/sessions` | Redirect landing to `/mcp`, as today |
| `/needs-you` | Operator attention queue | Unavailable view, no session fetch |
| `/sessions/:id` with existing query parameters | Specific session, regardless of current filters | Unavailable view |
| `/new?host=…&cwd=…` | New session; preserve existing prefill semantics | Unavailable view |
| `/hosts`, `/hats`, `/defaults` | Management / per-hat defaults | Unavailable view |
| `/more` | Links to MCP, Hats, Defaults, Settings, plus sign-out | Links only to supported MCP/Settings and sign-out |
| `/mcp`, `/settings` | Existing capability-gated features | Existing gateway features only |
| `/setup`, `/login`, unknown paths | Restyled existing auth / Not found | Same |

Keep path/query through login via `safeNext`, including `pending`, and
retain modified-click/new-tab behavior. Do not turn session URLs into
hash routes. Direct loads, reloads and Back/Forward reproduce the route.
Unavailable views and unknown paths have different copy, both tokenized.

New session is a route-backed dialog/sheet when opened over a known app
route. Retain that route as its return destination. Direct `/new` opens
the same form with a safe Sessions background; Close replaces to Sessions
instead of calling Back into an unrelated site. Preserve its draft when
opening Change or returning from Edit defaults within this flow. Never
close an unsent form or overwrite it merely because attention changed.

### 4.2 Sessions and Needs you

Sessions becomes the overview rather than auto-selecting the first row.
**Redesign 2 amendment (operator/parent, 2026-10-21):** Group by Hat
(default) or Host, then Project (default) or Nothing. Persist both choices
and sorting per browser in the shared session scope. Project subheaders
are collapsible and show the directory name, full path (ellipsized with
the full value available), hosts, running count and number of loaded sessions
needing the operator. Primary headers summarize waiting/running/total loaded
sessions; desktop column headings appear once above the groups. Remember
collapsed projects in sessionStorage across row navigation and remounts;
prune ids absent from the current loaded project groups after loading
completes. Collapsed projects do not spend the visible row budget. Keep a complete held family in its root's primary/project
group, including children whose cwd, host or repository differs. Missing
parents become visible roots; duplicate rows and malformed cycles must
not lose sessions or loop. Counts are loaded counts until paging completes.
Local status/host filters retain a held parent as context when a child
matches, keeping the root's project/host placement. Ancestors come only
from the current hat/lifecycle/search/Hide closed baseline; unrelated
siblings are not added and no family page is fetched. The overview counter
counts actual filter matches, excluding context-only ancestors. Inline
family previews count and show direct children, not grandchildren.

Sort whole families by Needs you first (default: any loaded member
with actual operator attention, then latest member activity), Last activity
(latest member event), or Started (root creation time, newest first).
Creation time and deterministic session-id ties keep Started order stable
through token/status/attention updates, including expanded child previews.
Needs you first and Last activity deliberately move families as their
attention/activity changes; only Started promises stable order.
The design's grouped view replaces day headers; old DST helpers may remain
for other displays. Parent/child identity uses actual family metadata.
Preserve each child's link and displayed recency.
Never load an entire family merely to decorate a list row.

The desktop overview header, including its title, counter, Group Hat/Host,
then Project/Nothing and New session button, stays on one line at 1024 px
with the 240 px sidebar open. Only the counter shrinks and ellipsizes;
labels and actions stay intact. Phone controls adapt to the available width
and retain 44 px touch targets; sorting is also available in the filter sheet.

The host asynchronously resolves a project's repository root when it learns
the cwd. A bounded local Git process (no shell or network) reads
`git rev-parse --path-format=absolute --git-common-dir`. Only a common
directory named exactly `.git` uses its parent as the project identity.
Other common directories identify themselves, keeping bare repositories,
separate Git directories and submodules distinct. A submodule is its own
project, not the superproject (parent ruling, 2026-10-22); its common-dir
basename labels it. Labels omit a trailing `.git`. Normal repositories,
linked worktrees and subdirectories share the same identity. Cache definitive
success/non-repository results by cwd; transient errors/timeouts use a short
retry TTL. A definitive non-repository reports cwd. Unavailable Git and
transient errors/timeouts emit no project-root metadata: the UI falls back
to cwd, and a later successful attach reports the real identity. This avoids
persisting a transient fallback on a session that closes before recovery.
Persist definitive identity with the session (migration 30),
expose optional `project_root` in summaries and fall back to `cwd` in the
browser for older hosts. Root identity must remain complete rather than use
the summary cwd's display truncation; ingest bounds it to 16 KiB of JSON.
This metadata must not advance session activity or appear in the transcript.
It is display-only: it may name a directory outside the hat's workspace
roots and grants no filesystem or session authority. The same absolute path
on different hosts intentionally shares a project group.

Status chips: All, Needs you, Running, Idle, Parked / offline, Failed.
Keep Hide closed and lifecycle controls in the filter panel. Status and host
filters operate on loaded summaries; label their counts as loaded counts
until paging is complete. Search still runs on the server over title,
path, branch and id across all lifecycles; while searching, set aside
volume/status/lifecycle/host filters and state that only the hat scope
applies. A scoped search must not make a push-selected session disappear.

Only the server's waiting count is a complete attention total before all
pages load; never display prototype fixture totals as real counts.
**Operator amendment, 2026-10-22: Unseen.** Needs you selects only summaries
with `operator_attention > 0`; unseen activity is an independent axis.
Agent messages/thinking, tool activity, plans, questions and turn completion
or failure mark a session Unseen, including while it is running. Operator
prompts, metadata and acknowledgement receipts do not. Unseen sessions have
a bold title and a nonanimated accent dot in session rows, Recent, search
and family summaries, with an Unseen filter. Lifecycle/activity and their
status colours remain independent. No existing history is backfilled.
The server persists an owner-scoped monotonic seen cursor and synchronizes
it through list SSE. Only a visible transcript tail with the corresponding
content already loaded in a focused visible tab acknowledges activity;
opening a background tab, previews and reading older content do not.
Stale acknowledgements never consume newer updates, change ordering or
answer questions. State is shared across devices.

Completed and failed turns of non-delegated sessions also retain durable
result review metadata. The session offers "Mark as read" without requiring
a reply; results alone never enter Needs you or its count. The server
acknowledges a specific completion event monotonically; stale clients cannot
consume a newer result. The state survives restart and synchronizes through
ordinary list SSE. Automatic reading requires the actual final result's end
to be visible in the focused, visible tab; queue previews never acknowledge.
A successful operator prompt acknowledges only the result captured when the
send began. Reading never answers a formal pending request. Migration starts
with an empty review inbox, avoiding a flood of historical results.

**Operator amendment, 2026-10-23: compact attention cards.** Needs you shows
only the current request, even when the session also has an unread result.
Question and permission text is clamped to three lines at every width. The
session link is prominent: "Open to answer" for a question, "Open" otherwise;
existing inline permission choices remain available. Result excerpts and
"Mark as read" stay outside the queue. Manual result acknowledgement is a
secondary button with a visible border and background at the end of its
completed turn in the session transcript, not below the session header.
Automatic reading retains the focused, visible,
loaded-final-content requirements above.

Delegated children never produce operator review work, even after their
parent or family edge disappears. Their ordinary completion pushes remain
suppressed. Existing explicit operator-required questions and escalation
records remain authoritative; this amendment does not grant parents approval
authority. No version bump, tag, publication or deployment belongs to this
feature branch.

Preserve
`parent_handled_questions` and answer-pending-delivery distinctions; neither
becomes operator work. Do not regex assistant prose for a final question.
Unseen applies during work and after completion. A factual `question_waits` alone does not
override modern operator-attention routing.

**Needs you has its own effective page query:** the selected `hat` only,
across **all non-deleted lifecycles**, including closed sessions. Omit `q`
and `lifecycle` from its summary requests; set the store's effective
`hideClosed` to false and apply no overview host/status filter. Overview
search, Hide closed, lifecycle, host and status choices remain saved for
returning to Sessions; entering Needs you must not overwrite them or reuse
their filtered page/cursor as the queue's baseline. Its Search control opens
Sessions search rather than narrowing this attention dataset.

Keep one persistent list store and **one list SSE**. Switch/reset the page
query and its cursor with a distinct query key when entering/leaving Needs
you; abort/fence responses from the previous query while preserving summary
upserts and the hat-scoped waiting count. The queue is complete only after
this all-lifecycle query is exhausted, not after an overview-filtered cursor
ends. Its session predicate is `operator_attention > 0 || review != null`, so a closed
session with real status-only attention is reachable and parent-handled or
already-queued answers remain excluded as before.

Use existing paged summaries and list SSE. A queue visit may scan at most
three pages sequentially, then offers Load more; show “X loaded · N need
you” while incomplete, and distinguish “none loaded yet” from “nothing
needs you”. The budget is per deliberate visit/Load more, not every render
or token. Abort/fence reads when scope changes. Keep an explicit way to
load further pages until the queue is complete; do not claim the first
page contains every waiting session. The same honesty applies to host,
hat and status counts in the overview and rail.

Queue previews read only visible cards, with at most two concurrent
requests. Use session detail to locate pending ids, then the existing
bounded Question Item route; never parse `PendingItem.payload` into a new
browser renderer. Cache by session/pending/item version for the visit,
invalidate on relevant attention changes or focus, and fence late results.
No background polling per card, no extra item stream. Preview errors retain
the session link. A status-only delegation fallback uses its real attention
record and reason, with Open; its acknowledgement remains in the canonical
session view. Every queue action opens the session/card, where AnswerBook
checks current state. This deliberately replaces prototype inline approvals.

The “Doing” cell shows authoritative status, family counts and known
branch/path data. Current `SessionSummary` has no running-tool snippet,
plan count or arbitrary last-message excerpt. Do not fetch every transcript
or fabricate those fields. Progress bars and plan fractions render only
where actual plan entries are already available, principally the selected
session. Use accessible empty/unknown text for absent facts.

### 4.3 Push must reach the selected session

`crates/hennery-sessions/src/notify.rs` already produces these values in
the encrypted `PushPayload.url`:

- question: `/sessions/<encoded-session-id>?pending=<encoded-pending-id>`;
- completed/failed turn and legacy link: `/sessions/<encoded-session-id>`;
- status-only delegation attention: the affected session URL;
- gateway: `/mcp`.

IDs are encoded by the backend's `encoded_id`, never by agent-supplied URL
text. `web/worker.js` validates the payload and passes that path unchanged
into notification `data.url`; `notificationclick` closes the notification
and calls `clickNotification(data.url, clients, origin)`. Preserve the
current receipt/click allowlist and its canonical encoding checks. Invalid
or cross-origin targets fall back to `/sessions`; do not expand this
allowlist to arbitrary navigation when adding the new routes.

Retain the existing ready/dirty-client handshake: exact-target focus does
not reload; another window is navigated only when ready; dirty composer,
question draft, attachments or mutations remain untouched. A separate
window or the existing explicit Open notification hint handles dirty
installed-app contexts. A push never answers a card or changes a hat filter.

The session honors `pending` outside the newest page through
`GET /api/view/sessions/{id}/questions/{pending_id}`, deduplicates by item
id/version and focuses without choosing an option. A legacy session-only
link reaches its oldest live pending card, otherwise its current tail.
404, deleted session, withdrawn and already-answered questions retain their
actual state. Keep the coherent snapshot and removal/version fences from
the phone implementation.

R2 must retain a producer-to-click test: take the URL emitted by the real
notification producer/fixture, feed the emitted worker handler, click,
and assert the actual router displays that session/card at both widths.
Hand-writing a matching URL in the router test alone is insufficient.
Include signed-out return with query, current filter excluding the target,
an older card, exact-target no reload, and dirty-window preservation.
No `actions` array or lock-screen submission endpoint is added.

## 5. Prototype element inventory

Every visible production element in both prototypes is assigned below;
repeated instances use the same row. **Build** means new frontend behavior,
**Restyle** preserves an existing contract, **Drop** removes prototype-only
behavior, and **Needs backend** is the new defaults feature. Paths are
relative to `web/src` unless stated; unqualified component filenames mean
`components/`, and `items/` means `components/items/`. “New” marks proposed
components; screen filenames are qualified with `screens/` on first use.
Where the prototype claims data the current API lacks, the row specifies
the honest replacement instead of quietly adding another backend feature.

### 5.1 Shared design system and desktop navigation

| Element / variant | Disposition and behavior | Target |
|---|---|---|
| Six surfaces, text hierarchy, dividers, control borders, scrim/popover shadow | Restyle; paired tokens and measured contrast (§3) | `styles/tokens.css` (new), `index.css`, `manage.css` |
| Primary, secondary, ghost, reject, icon-only, small, focused, disabled buttons | Restyle; native disabled state, accent/on-accent roles; 44 px mobile targets | `components/ui/*` (new), existing `.btn` |
| Path/search/text/textarea inputs, focused/error state and hint | Restyle; real labels, error association, 16 px mobile input | `components/ui/*`, `ProjectPicker.tsx`, `Composer.tsx` |
| Model/mode menus, effort radios, booleans, inherits/override chips | Restyle/build from catalogs; no hard-coded choice lists | `components/ui/*`, `lib/catalog.ts`, `LaunchConfig.tsx` (new) |
| Status badges/glyphs: permission, question, running, starting, idle, parked, offline, failed, warning | Restyle; icon + text + semantic color | `lib/status.ts`, `SessionRow.tsx`, `SessionHeader.tsx` |
| “Your turn” inferred from a message | Drop; ordinary idle session remains idle; real status-only delegation attention stays | `lib/status.ts`, `screens/NeedsYou.tsx` (new) |
| Hat square/chip, host online/offline chip, count pill, selected row, keycaps | Restyle; escaped labels; derived swatches; counts distinguish loaded/complete | `HatSwitch.tsx`, `SessionRow.tsx`, `components/ui/*` |
| Typography, spacing, radii samples and component gallery | Build shared production primitives; drop the handoff/gallery page itself | `styles/tokens.css`, `components/ui/*` |
| Wordmark, online-host fraction and explanatory tooltip | Restyle/build; fraction from owned non-revoked hosts, unknown shown while loading | `components/NavigationRail.tsx` (new), `Shell.tsx` |
| Theme icon and collapse/expand rail control | Build; preference cycle and persisted expanded/collapsed state | `NavigationRail.tsx`, `lib/theme.ts` |
| Jump to field and compact-rail search icon | Build; real palette trigger, initially a functional search/navigation entry before R6 | `CommandEntry.tsx` (new), `CommandPalette.tsx` (new) |
| Needs you, All sessions, New session rail links and count/hint | Build/restyle; real links/current state; no automatic session selection | `NavigationRail.tsx`, `SessionScope.tsx`, `lib/views.ts` |
| Hat section, each swatch/name/waiting/total, compact attention dot | Build; scoped overview links, loaded counts and accessible names | `NavigationRail.tsx`, `HatSwitch.tsx` |
| Configure links: Defaults, Hosts, MCP, Hats, Settings, associated notes | Restyle/build; capability gated; show only actual counts/offline/auth state | `NavigationRail.tsx`, `lib/views.ts` |
| Owner/avatar and Push on rail footer | Restyle; neutral Owner mark, no fixed initials; push status only from verified browser registration, link to Settings | `NavigationRail.tsx`, `PwaStatus.tsx` |
| Compact rail icons, separators, labels/tooltips, badges | Build; every icon link has full accessible name, same destinations | `NavigationRail.tsx` |

### 5.2 Overview and Mobile frames 1 / 1b

| Element / variant | Disposition and behavior | Target |
|---|---|---|
| Desktop overview title/count, Group Hat/Host then Project/Nothing, New session | Native radio groups; persistent grouping/sort; single line at 1024 px, ellipsized counter | `screens/Sessions.tsx`, `components/SessionList.tsx`, `SessionGroupingControls.tsx` |
| All/Needs you/Running/Idle/Parked-offline/Failed chips | Build; status predicates in one pure helper, loaded-count qualification | `components/SessionFilters.tsx` (new), `lib/status.ts` |
| All hosts selector, title/path/branch search, slash hint | Build/restyle; host filters loaded data, search uses existing server q contract | `SessionFilters.tsx`, `SessionScope.tsx` |
| Needs you section title, descriptive subtitle, total badge, recent-first text | Build; only actual operator attention and honest partial state | `screens/NeedsYou.tsx`, `components/NeedsYouQueue.tsx` (new) |
| Queue command/edit/question/status previews, age, title, hat/host/agent | Build; bounded typed reads, read-only previews (§4.2) | `NeedsYouQueue.tsx`, `components/AttentionPreview.tsx` (new) |
| Queue Allow/Always/Reject and quick answer options | Drop inline mutations; replace by Open request / Open question | `AttentionPreview.tsx` |
| Reply input, Reply, Mark as seen on inferred prose | Drop; no reply inference/read-state API | `AttentionPreview.tsx` |
| Queue keyboard instruction strip, focused-card outline | Build navigation/focus only; do not advertise global digit answers | `NeedsYouQueue.tsx`, `hooks/useShortcuts.ts` (new) |
| Queue-empty success line and push promise | Restyle; “Nothing needs you” only when complete; explain notification state conditionally | `NeedsYouQueue.tsx` |
| Desktop column headers: Session, Doing, Host, Agent/model, Last | Build responsive semantic list/table; Doing follows available data | `SessionList.tsx`, `SessionRow.tsx` |
| Hat/host group header, summary count, host note | Build; no invented full totals or cross-hat grouping | `SessionList.tsx` |
| Project subheader, name/path/hosts/waiting count and disclosure; three sort choices | Root-resolved project groups, whole-family sorting and collapse; stable newest-first Started order | `SessionList.tsx`, `SessionFilters.tsx`, `lib/sessionGroups.ts` |
| Parent row, child indentation/arrow, child-count badge, selected/wait/fail row | Restyle; actual family links, stable row keys and preserved separate actions | `SessionRow.tsx` |
| Row progress bar / plan fraction | Drop when unavailable; actual selected-session plan only, no transcript fan-out | `SessionRow.tsx`, `StepList.tsx` |
| Status glyph, Doing subtitle, host, agent/model, last-event age | Restyle; unknown fields omitted; failure explanation remains keyboard/touch reachable | `SessionRow.tsx` |
| Mobile 1 header: Needs you, host fraction, Search | Build; Search opens session search; R6 upgrades Jump to integration | `screens/NeedsYou.tsx`, `CommandEntry.tsx` |
| Mobile 1 horizontal All/hat chips and counts | Restyle; scrolling chip faces inside ≥44 px targets | `HatSwitch.tsx` |
| Mobile 1 “N waiting · most recent first”, stacked previews | Build; same queue and pagination as desktop | `NeedsYouQueue.tsx` |
| Mobile 1 running/idle/parked summary button | Build; opens Sessions, counts qualified while partial | `NeedsYouQueue.tsx` |
| Mobile 1b Sessions header, Search and Filter buttons | Build/restyle; filter sheet preserves current choices | `screens/Sessions.tsx`, `SessionFilters.tsx` |
| Mobile 1b By hat / By host segmented choice | Build; same state as desktop group radios | `SessionFilters.tsx` |
| Mobile 1b grouped cards, two-line rows, family label/indent, age | Restyle; no dropped host/model facts, available in row details on narrow screens | `SessionList.tsx`, `SessionRow.tsx` |
| Five bottom items: Needs you, Sessions, New, Hosts, More; attention badge | Build; route links under navigation landmark, More owns management destinations | `components/BottomNavigation.tsx` (new), `lib/views.ts` |
| Mobile device bezel/status bar/home indicator and canvas captions | Drop from application; retain frame names only for acceptance references | None |

### 5.3 Session, Desktop and Mobile frames 1c / 3 / 4

| Element / variant | Disposition and behavior | Target |
|---|---|---|
| Back button, hat breadcrumb/swatch, session title | Restyle; route-aware back fallback; actual agent title, no new title-edit API | `SessionHeader.tsx`, `SessionMenu.tsx` |
| Status pill; waiting position; previous/next waiting session | Build/restyle; use shared queue order; qualify partial position, disable absent neighbor | `SessionHeader.tsx`, `SessionScope.tsx` |
| Details button/pressed state, Park, ellipsis menu | Restyle/build; panel toggle and existing capability/lifecycle-gated Park/Close/Delete | `SessionHeader.tsx`, `SessionMenu.tsx` |
| Meta host dot/name, cwd, branch/changed, agent/model, MCP count, family link | Restyle; actual fields only; dirty boolean says “changed”, not an invented file count; MCP from delivered snapshot | `SessionHeader.tsx`, `McpDelivery.tsx` |
| Parent-driven banner and Take over | Restyle/build; current routing state and existing takeover API | `screens/Session.tsx`, `FamilyPanel.tsx` |
| “Prompts pause delegation” and Hand back toggle | Drop inaccurate promise/toggle; say operator prompts take over under current backend semantics, no hand-back API | `Session.tsx`, `FamilyPanel.tsx` |
| Parent-origin prompt, source link, speaker mark/name/time | Restyle; preserve typed provenance for parent/child/plugin/operator turns | `items/UserTurn.tsx`, `items/PluginContext.tsx`, `items/AgentMessage.tsx` |
| Assistant prose/Markdown, inline code, lists | Restyle; sanitized pipeline, safe external links, explicit list markers | `Markdown.tsx`, `items/AgentMessage.tsx` |
| Thinking disclosure and duration | Restyle; collapsed by default; show duration only if supplied, otherwise “Thinking” | `items/Thinking.tsx` |
| Collapsed Glob/Read/Edit/Bash tools, icons, summary/status | Restyle; `aria-expanded`, real input/output and fabrication warning | `items/ToolCall.tsx` |
| Expanded edit, +/- totals, line numbers and diff rows | Restyle existing typed tool content; no fabricated diff, retain plain-text fallback | `items/ToolCall.tsx`, `lib/highlight.ts` |
| Plan updated line, actual completed/total steps | Restyle; derive only from actual plan entries | `items/PlanItem.tsx`, `StepList.tsx` |
| Desktop permission card: header/type/age, command, cwd/host, options | Restyle; current Question Item and adapter option ids/order | `items/QuestionCard.tsx` |
| Always-allow command wildcard and reject-with-note hint | Drop fabricated pattern/note workflow; use adapter-provided option names and only supported request fields | `items/QuestionCard.tsx` |
| Sent/answered/stale/conflict state, host-offline note, Reset demo | Restyle delivery states; drop Reset demo and invented device attribution | `items/QuestionCard.tsx`, `lib/delivery.ts`, `store/useAnswer.ts` |
| Mobile 3 earlier-step disclosure, prose, highlighted permission card | Restyle; existing paged/grouped transcript, not invented summary text | `Transcript.tsx`, `screens/Session.tsx` |
| Mobile 3 bottom Allow once/Always/Reject and Next link | Restyle/build adaptive action region; supplied options only, same AnswerBook, Next never auto-answers/navigates | `QuestionCard.tsx`, `components/QuestionActions.tsx` (new) |
| Mobile 4 previous-answer success banner | Build transient local acknowledgement with real delivery wording; not “Allowed” on mere 202 | `Session.tsx`, `lib/delivery.ts` |
| Mobile 4 question header/age/body, choice labels/descriptions/radios | Restyle; real supported elicitation schema, semantic fields | `items/Elicitation.tsx`, `QuestionCard.tsx` |
| Mobile 4 own-words input and Decline/Answer bottom actions | Restyle; actual custom-field support/exclusivity; unsupported schemas decline/cancel only | `items/Elicitation.tsx`, `QuestionActions.tsx` |
| Mobile 1c scrim, sheet handle, Family title/progress, Close | Build responsive details sheet with Family/Plan/Details tabs; handle decorative | `components/SessionDetails.tsx` (new), `FamilyPanel.tsx` |
| Parent/current/sibling cards, connection line, selected border, Open/Take over | Restyle; own session links, current routing, bounded family paging | `FamilyPanel.tsx` |
| Family plan fractions and progress bars | Show only actual known plan; omit unsupported sibling/parent progress | `FamilyPanel.tsx` |
| Right panel Plan list: done/current/todo icons and strike-through | Restyle actual latest plan; show truncation/empty state | `SessionDetails.tsx`, `StepList.tsx` |
| Right panel Details: session ID, host, project, hat/rule, agent, model, MCP, started | Show the full Hennery `session_id` as selectable text on desktop and mobile; omit unknown rule/version/config-source claims; actual delivered MCP snapshot | `SessionDetails.tsx`, `McpDelivery.tsx` |
| Family status-only attention, fallback reason, acknowledgement/rearm | Restyle existing real controls; preserve prerequisites and pending links | `StatusAttention.tsx`, `FamilyPanel.tsx` |
| Composer textarea, attachment button, Model, mode, effort radios, Send hint/button | Restyle all actual catalog options; local send shortcut, preserve cancel/resume/draft behavior | `Composer.tsx`, `components/SessionConfig.tsx` (new) |
| Composer slash-command menu and “@ for files” placeholder | Restyle real slash commands; drop unsupported @-file promise | `Composer.tsx`, `lib/catalog.ts` |
| Attachment thumbnails/remove, drag/paste, unsupported/error states | Restyle existing capability/size/marker rules | `Composer.tsx` |
| Parked/closed/failed/starting/offline footer, Resume/send-again/new-project flow | Restyle existing behavior; still reachable without the panel | `SessionFooter.tsx`, `items/Marker.tsx`, `Composer.tsx` |
| Unsupported update, plugin context, boundaries, reconnect/load-earlier controls | Restyle; retain content, provenance, truncation and visible failures | `items/Unrecognised.tsx`, `ItemBoundary.tsx`, `Transcript.tsx` |

**Operator prompt queue (2026-10-21; sessions migration 29).** Send remains available beside Cancel
while a session is running or waiting. On a parked session its label, tooltip
and accessible name are "Resume and send"; this submits one collector queue
request, and the collector owns resume/delivery. Keep standalone Resume and
show the parked notice above the input, including "N queued messages will be
sent when you resume" when applicable. Explicit Park holds existing queued
work; Resume or a new Send releases it. Ctrl+Enter and Cmd+Enter use the same
submission handler in every state; ignore IME composition/keyCode 229 and
autorepeat. Plain Enter retains its existing newline behavior. Accepted messages appear in a bounded
"Queued" list under the composer, with a text preview, image count, Edit and
Remove. Editing uses a separate inline draft and retains images; FIFO order
cannot be changed. Once delivering, an item is immutable. If delivery wins
while editing, keep the textarea visible with "Already sending; your edit
wasn’t applied" and Copy edit/Dismiss edit actions, including a manual copy
fallback. Failed items retain text/images for seven days, explain why, and
offer Copy/Dismiss. Each submission carries one client request key, retained
for retries of the same checked content so an uncertain response cannot duplicate it.
The uncertain-send marker persists this key across page reload; an unchanged
draft reuses it after network failures or 5xx. A receipt conflict releases that
key for the next explicit submission, preserving a visible warning that the
earlier submission may already have been sent. Editing or a
definite refusal resolves that marker. A parked offline session's notice says
to resume and send once the host is back; the API refuses new offline sends
with `host_offline` and retains the draft. Queue-owned not-delivered transcript
markers offer no separate Send again action.
Deleting the session or purging its hat scrubs queued text and images with
the transcript; the delete confirmation names the number of unsent messages
that will also be deleted, excluding items already delivering. No recovery
list remains after deletion.
Queue state comes from the
collector and its existing session view stream (`queue_changed` snapshots),
so reload and multiple windows converge on one list; ordinary unsent drafts
remain per-window. A pending permission is answered on its canonical card,
never by a queued prompt. On a phone, "Write a prompt" exposes the composer
without resolving that pending request. Both themes and phone/desktop widths
keep Send reachable with the list visible.

The mobile bottom action region is a presentation of the focused live
question's existing controls, not a second independent form. Mount one
interactive action set, associate it with that card, and return to the
normal composer when no card is selected. Preserve drafts by pending id.
Multiple open questions remain individually reachable. Stream arrival
never replaces a form while typing or moves focus to another request.

### 5.4 Defaults and New session, including Mobile frame 5

| Element / variant | Disposition and behavior | Target |
|---|---|---|
| Defaults title/subtitle; agent tabs | Needs backend/build; configured agent ids, not a fixed two-agent list | `screens/Defaults.tsx` (new), `api/defaults.ts` (new) |
| Agent default → Hat override → Your pick explanation | Build; baseline is read-only selected-host catalog, no saved global base | `Defaults.tsx` |
| Host/cwd context absent from prototype | Build; explicit catalog preview context, current resolved hat and freshness | `components/CatalogContext.tsx` (new) |
| Desktop table: Setting, agent base, each hat; model/mode/effort rows | Needs backend; other supported axes join; unsupported cells explain absence | `Defaults.tsx`, `components/DefaultsEditor.tsx` (new) |
| Base cell, dashed Inherits, accent Override, reset marker/legend | Needs backend; edit through actual select/checkbox plus explicit Reset to inherited and Save | `DefaultsEditor.tsx` |
| Preview for each hat with value/source | Build; distinguish stored override from value usable on preview host; warnings stay visible | `DefaultsEditor.tsx`, `lib/defaults.ts` (new) |
| Defaults Save/Cancel, conflicts, step-up and permissive warning | Needs backend; no single-click cycling or reset-on-cell-click | `DefaultsEditor.tsx`, `api/defaults.ts`, `StepUpDialog.tsx` |
| New session desktop dialog/mobile sheet: scrim, handle, title/subtitle, Close | Restyle route-backed form; safe return and retained draft | `screens/NewSession.tsx`, `components/NewSessionFrame.tsx` (new) |
| Optional first prompt/caret/focus treatment | Restyle real textarea; preserve prompt and existing attachment rules if shared | `NewSession.tsx` |
| Host choices with online dot/offline disabled; Agent segmented choices/Other | Restyle; actual availability/auth and configured runtime ids | `NewSession.tsx`, `lib/agents.ts` |
| Project input/folder/Browse, Recent projects list/check/clock, hat/time metadata | Restyle; host-scoped recents/browser and resolved hat, no copied paths | `ProjectPicker.tsx`, `NewSession.tsx` |
| Hat preview/rule and MCP delivery note | Restyle; server resolution, not editable hat id; retain actual isolation/fallback prediction | `NewSession.tsx`, `McpPrediction.tsx` |
| Session settings Model/Permission/Effort cards, chevrons, provenance dots | Build; effective host catalog + stored defaults + explicit picks; support all other axes | `LaunchConfig.tsx`, `lib/defaults.ts` |
| Mobile defaults summary / Change | Build; opens accessible config section/sheet, includes host and agent selection | `LaunchConfig.tsx`, `NewSession.tsx` |
| Edit defaults link | Build; opens Defaults for current hat/agent, preserving launcher draft on return | `NewSession.tsx`, `Defaults.tsx` |
| Summary, Cancel, Start session / Start on host, local send hint | Restyle; disabled until prerequisites valid; actual config checked before first prompt | `NewSession.tsx`, `lib/start.ts` |

In the project browser, keep “Use this directory” beside the current path in a
sticky header, visible without scrolling even for long listings. Enter from the
input or a browse control chooses the shown directory; Enter on a directory row
retains native button activation and descends, including `.. (up)`. Clicking or
tapping outside the picker chooses the shown directory and closes the browser.
While loading or after a failed listing, an outside pointer only closes it.
Escape closes without choosing, from any browse control. A choice displays the
full directory path in the project field. Suggestion mode keeps its existing
selection behavior. (Operator decision via fleet parent, 2026-10-21.)

Only a primary mouse click away from a scrollbar commits an outside choice.
Touch and pen commit on the click produced by a tap, never on a swipe's initial
pointerdown. After row navigation focus the choice button; an inside choice or
Escape returns focus to the input. Composition Enter and held-Enter repeats do not choose
or submit. Describe the choice button with the current path and announce a new
directory politely; keep the path to one ellipsized line with its full title.
Start or Cmd/Ctrl+Enter while browsing commits the shown directory, waits for its
hat, then starts there without waiting for the settings preview. Resolution failure or intervening
user interaction cancels this pending start. Close and Edit defaults retain the
outside-choice rule. (Parent review ruling, 2026-10-21.)

An outside choice preserves the focus of the tapped control. Only a row's
navigation moves focus to the choice button; an initial listing arriving while
the input is focused leaves it there. Composition Escape stays inside the picker
without closing it or its dialog. Touch click source is consumed after each click.
Repeated Start (including double-click and Cmd/Ctrl+Enter), key autorepeat and
modifier-only keys retain a pending start. Actual cancellation shows a muted,
polite note; a ready catalog needing an explicit configuration choice also
cancels, so no pending request waits indefinitely. (Parent re-review ruling,
2026-10-21.)

### 5.5 Management, settings and palette

| Element / variant | Disposition and behavior | Target |
|---|---|---|
| Hosts heading, online/offline count, Add host | Restyle; actual owned/non-revoked connectivity | `screens/Hosts.tsx` |
| Pair panel: single-use code, countdown/waiting, numbered instructions, command Copy, Close | Restyle; real server expiry and current public URL/command; no prototype tunnel recipe | `components/Pairing.tsx` |
| Host table/card: name/platform, status, host version, default hat, paired/last-seen | Restyle; current `HostItem` fields | `Hosts.tsx` |
| Agent version/login, host update, doctor/transport labels, session totals | Drop unsupported claims; available agent choices may be shown from existing agents API on demand, no new diagnostics backend | `Hosts.tsx` |
| Host actions Rename, Default hat select, Revoke | Restyle; confirmation/step-up and offline/revocation warnings unchanged | `Hosts.tsx`, `ConfirmDialog.tsx` |
| MCP heading/subtitle/Add connection | Restyle | `screens/Mcp.tsx` |
| Gateway rows: name/URL, auth kind, status/account, hat, action menu | Restyle; preserve actual health vs sign-in-required distinction | `Mcp.tsx`, `ConnectionForm.tsx`, `TokenForm.tsx`, `OauthClientForm.tsx` |
| Host mount checkbox chips for each connection, all hosts, fallback notes | Restyle; full-set save with step-up, no immediate fake toggle success | `Mounts.tsx`, `McpScope.tsx` |
| OAuth Connect/Reconnect, probe, blocked popup link, resource acceptance/errors | Restyle; preserve blank-popup gesture and write-only secrets | `OauthConnect.tsx`, `McpStatus.tsx`, `hooks/useMcpOauth.ts` |
| Local stdio host/hat selectors, name/command/args/env indicators, Edit/Add | Restyle; actual whole-set editor and secrets stay write-only | `StdioServers.tsx`, `StdioServerForm.tsx` |
| Local Review & save, next-start/resume notice, delivery/fallback warning | Restyle existing confirmation; no immediate running-session reconfiguration claim | `StdioServers.tsx`, `McpDelivery.tsx` |
| Hats heading, New hat, swatch/name/session-count/Edit cards | Restyle; counts only if loaded/qualified; retain create/rename/recolor | `screens/Hats.tsx` |
| Path rules with host scope and tester | Restyle actual canonical prefix rules; do not render prototype glob/“any host” as a supported rule language | `PathRules.tsx`, `Hats.tsx` |
| Hat MCP summary, “Default on” host list | Build read-only derived summary from mounts/owned hosts; empty/unknown states | `Hats.tsx` |
| Hat defaults summary/chevron link | Build; consume R4 API when available, otherwise functional Defaults link | `Hats.tsx`, `api/defaults.ts` |
| Default for new hosts, Purge/Resume purge, preview/result/transcript cleanup | Restyle existing controls and freeze/default protections | `Hats.tsx`, `ConfirmDialog.tsx` |
| Hat logo/display/upload | Drop frontend; retain `HatItem.logo` and backend routes/storage | `Hats.tsx`, `HatSwitch.tsx`, `SessionHeader.tsx` |
| Settings Sign-in panel and password Change/changed date | Restyle sign-in explanation; drop unsupported Change button/date, provide existing local-admin reset guidance | `screens/Settings.tsx` |
| Passkey rows: label, created, last used, Remove/Add | Build frontend using existing auth API; no new credential backend | `components/AccountSettings.tsx` (new), `api/auth.ts` |
| Global permission/question/failure/finished/hidden-tab switches | Drop; replace with per-hat privacy section below | `Settings.tsx` |
| Browser notification state, prepare/enable/disable/renew, install/denied hints | Restyle existing phone lifecycle; retain two gestures/step-up and honest partial failures | `components/NotificationSettings.tsx` (new), existing PWA modules |
| Push devices, labels, last acceptance/error, remove | Restyle; registrations are not physical device identities or delivery guarantees | `NotificationSettings.tsx` |
| Per-hat mute/details/generic title and preview/Save | Restyle existing `PushPolicyItem`; explain details can expose agent text/secrets | `NotificationSettings.tsx`, `api/device.ts` |
| Per-hat delegation switches, loading/error, activation/disable notices | Build frontend on GET/PATCH session-tools; Settings only, stepped up | `components/DelegationSettings.tsx` (new) |
| Per-hat binary Delegation switch | PATCH only `delegation_enabled`, one step-up confirmation, no catalog probes | `DelegationSettings.tsx`, `api/delegation.ts` |
| Signed-in devices/Revoke, current login/Sign out | Restyle existing behavior and cleanup | `AccountSettings.tsx`, `SignOut.tsx` |
| Collector Public URL, default hat, Theme | Build/restyle read-only collector info; default-for-new-hosts from hats, per-host defaults stay in Hosts; appearance radios in Settings | `components/CollectorSettings.tsx` (new), `Settings.tsx` |
| Push contact, attachment usage/cleanup status and deployment explanation omitted by prototype | Restyle/add reads from existing routes; display only reported values and existing isolation guidance | `CollectorSettings.tsx`, `Settings.tsx` |
| Public URL editor absent from current screen | Keep read-only URL in this redesign; no new URL-edit flow. Existing backend/admin reset remains | `CollectorSettings.tsx` |
| Palette overlay/scrim, Search input, Escape hint, grouped results/metadata/key hints | Build real dialog/combobox with Needs you, Actions, Recent sessions | `components/CommandPalette.tsx` (new) |
| Palette New session in project/defaults, New session, Add host, Edit defaults | Build navigation/prefill actions; quick start opens reviewable launcher, never starts a permissive session directly | `CommandPalette.tsx`, `lib/commands.ts` (new) |
| Palette recent session links and selected result | Build; owned loaded summaries, server session search on query, explicit partial/loading state | `CommandPalette.tsx` |
| Prototype ⌘⇧N immediate contextual launch | Drop reserved global binding; contextual launcher remains a palette action | `lib/commands.ts` |
| Mobile 2 lock-screen shell, grouped native notification, app icon/time | Native OS presentation only; keep generic bundled PWA icon and existing private push policy | `web/worker.js`, existing icons |
| Mobile 2 Allow once/Reject/Open card action list and long-press instruction | Drop all lock-screen action buttons; normal notification tap opens current card | `web/worker.js` |

### 5.6 Existing screens and states missing from the prototypes

| Existing element | Disposition | Target |
|---|---|---|
| Setup token removal, password/confirmation, public URL, default hat name, optional passkey | Restyle using tokens; preserve one-time secret removal and flow | `screens/Setup.tsx` |
| Login passkey/password, unavailable-passkey state, rate-limit/error, return target | Restyle; keep `safeNext` and explicit passkey gesture | `screens/Login.tsx`, `api/next.ts` |
| Step-up passkey/password/cancel, confirmation dialog, pending retry | Restyle; focus return and original form state survive | `StepUpDialog.tsx`, `ConfirmDialog.tsx`, `App.tsx` |
| Gateway-only shell and feature-unavailable pages | Restyle; no session/delegation/default calls or navigation in gateway mode | `Shell.tsx`, `lib/views.ts`, `screens/Placeholder.tsx` |
| Not found, deleted session, unknown/foreign session, empty/loading/error/retry states | Restyle; distinguish unavailable and deleted without exposing foreign data | `Placeholder.tsx`, `Session.tsx`, screen owners |
| PWA offline/reconnecting/update-required banner, dirty-window Open notification hint | Restyle; preserve build/cache/readiness lifecycle | `PwaStatus.tsx`, `NotificationNavigation.tsx` |
| Host default-hat editing and hat “Default on” / “Default for new hosts” | Restyle/build; keep three concepts distinct | `Hosts.tsx`, `Hats.tsx`, `CollectorSettings.tsx` |
| Session deletion confirmations, hat-mismatch recovery links and retained drafts after failure | Restyle existing behavior; no control lost behind a closed panel; no new session-rename API | `SessionMenu.tsx`, `ConfirmDialog.tsx`, `SessionFooter.tsx` |
| MCP shared polling/attention badges and OAuth callback/failure pages | Restyle app components; preserve callback's separate security/CSP contract | `McpScope.tsx`, `McpStatus.tsx`, existing gateway callback |

## 6. Per-hat defaults: storage, API and resolution

### 6.1 Ownership and wire shapes

Add the next **kernel component** migration in
`crates/hennery-kernel/src/schema.rs`; do not change an old migration or
choose a number from another component. Add
`crates/hennery-kernel/src/session_defaults.rs` for `hat_session_defaults`,
containing:

| Column | Contract |
|---|---|
| `owner_id` | Required on every row and in every lookup/write predicate |
| `hat_id` | Composite foreign key `(hat_id, owner_id)` → `hats(id, owner_id)`, ON DELETE CASCADE |
| `agent` | Configured runtime id, not friendly label; part of primary key `(owner_id, hat_id, agent)` |
| `config_json` | Sparse, validated `SessionConfig`: model/mode ids and boolean/string axes; no arbitrary JSON |
| `revision` | Positive integer, incremented transactionally on successful replacement; absent row reads revision 0 |

An empty config resets all fields to inheritance but keeps its row/revision,
so stale editors cannot reset another editor's later changes through an
absent-row ABA. New hats inherit entirely; no migration copies active
session configuration or permissive modes. Hat purge removes the rows with
the hat; frozen hats reject writes. Host revocation does not delete a
cross-host preference. No model credentials, cwd, host command/environment,
notification policy or delegation authorization is stored in this table.

Use Rust DTOs in `hennery-proto`, generated into TypeScript and JSON Schema:
`HatSessionDefaults {hat_id, agent, revision, config: SessionConfig}` and
`PutHatSessionDefaults {revision, config: SessionConfig}`. Reuse
`ConfigValue` (string or boolean). Register both in codegen; do not hand-edit
generated files. API handlers live in
`crates/hennery-sessions/src/session_defaults_api.rs`, using the kernel
store just as the sessions HTTP facade uses owned hats.
No kernel import of sessions/gateway is introduced.

| Endpoint | Result / authorization |
|---|---|
| `GET /api/hats/{id}/session-defaults?agent=<id>` | 200 typed owned record, sparse empty/revision 0 when unset; cookie/operator-only, no step-up |
| `PUT /api/hats/{id}/session-defaults?agent=<id>` | Replace this agent's sparse config atomically using supplied revision; 200 stored record; cookie, Origin and fresh step-up on **every** write |

No collection-wide PUT or new DELETE endpoint. UI saves a deliberately
edited hat/agent record; unrelated agent/hat defaults cannot be overwritten.
Read multiple hat columns with bounded concurrent GETs, not a new owner-wide
aggregation service. Full deployments advertise `session_defaults` only
when these routes exist; gateway-only deployments do not offer the screen.

Validation: 16 KiB maximum body; agent 1–128 UTF-8 bytes, no controls;
model/mode/axis ids and string values 1–256 bytes, no controls; at most
64 axes; no reserved model/mode entries in axes. Reject
unknown DTO fields and non-boolean/non-string values. Omitted/null
model/mode normalize to inheritance; removing an axes key inherits;
`false` is an explicit override. Use the existing `ApiJson` extractor and
typed validation once; no additional JSON parser/policy layer. Validate
revision as a nonnegative bounded integer. Respect existing configured
runtime/catalog limits; an unavailable runtime's stored record remains
resettable instead of forcing a host online for removal.

Return existing JSON error envelope: 401 unauthenticated; 403
`step_up_required` or normal Origin/auth refusal; 404 for unknown/foreign
hat; 409 `hat_purging` or `defaults_changed`; 400/422 for invalid shape/value
and 413 for body size. Responses are private/no-store. Re-check ownership,
hat freeze, current revision and fresh authorization at the transaction's
write boundary; no async probe while holding a DB guard. Add all new
statements/access paths to `crates/hennery-testkit/tests/owner_filter.rs`.

The storage API accepts well-formed opaque values without pretending one
host's catalog describes every host. The UI chooses values from an explicit
catalog context, and launch validation below decides applicability. Thus
stored records can survive a temporary adapter change and still be reset
offline; saving does not claim universal host support.

### 6.2 Permission defaults are a security boundary

Persisting a permissive mode such as bypass/full access changes what later
sessions may do without asking. Require fresh step-up to save it. Requiring
step-up for all PUTs is intentional: unknown custom axes may also control
sandbox/approval policy, and labels are untrusted. This is a preference
write, not the creation of a new source of agent authority.

The editor and New session show a persistent warning on a mode/security
axis outside the profile-owned `HostCatalog.security` baseline, or when
`security.known` is false. Use profile metadata, never a regex over display
names. The Save confirmation names the hat, agent and effective changes,
and explains reduced prompting. Loading or applying a previously saved
default does not ask for a second step-up: the deliberate persisted choice
is the authorization to prefill subsequent operator launches.

Saving defaults must never enable delegation or modify a running/parked
session. Operator decision, 2026-10-21 removes the execution policy entirely.
An enabled hat permits all live catalog-offered child configurations, including
unknown-security profiles; defaults are separate operator launch preferences.
Configuration must still be applied and confirmed before the child brief runs.

### 6.3 Deterministic inheritance and heterogeneous catalogs

For a selected online host, agent and validated cwd, GET the existing
runtime-catalog endpoint for its persistent snapshot. No catalog probe runs on
open, in the background or on a timer. An explicit Refresh POST probes and
saves a new snapshot. Both New session and Defaults show the saved age
(for example, “saved 2 h ago”) and a Refresh button. Real starts and resumes
also populate this exact-context cache. Cached previews survive reconnect and
collector restart but never count as Fresh for delegation.
The backend resolves the cwd/hat; the launcher
also uses its existing hat-resolution preview. Fence responses by the
whole context (host, agent, canonical cwd/resolved hat and request generation).
Only the active request generation may publish its response, including its
`adapter_revision`; the web does not compare revisions to reject responses.
The opaque revision identifies the adapter command/environment and installed
security profile, not `config_options` or the session's chosen settings.
Never use the last session of another
hat, a display model name or another host's probe as the baseline.

Resolve each field independently:

1. The chosen catalog's `defaults` supplies the read-only agent baseline:
   the adapter's own `session/new` values before any hennery switch, or the
   equivalent explicit Refresh result. Resume/restart and delegated attaches
   update offered options but never replace that baseline with current picks.
   If no baseline is known, `defaults` is empty; send only deliberate overrides.
2. A supported stored field/key for this hat and agent replaces that baseline.
   Security axes (mode and the ids in the profile's `security.axes`) are strict:
   always request those hat overrides, even when this catalog cannot offer
   them. A refusal produces a readable configuration error and retains the
   first prompt. For nonsecurity overrides, a cached catalog that does not
   offer the value falls back to the supported baseline or agent default,
   with a visible notice explaining that fallback. Never erase the stored record.
   Without a catalog, including while loading, request every hat override.
3. An explicit New session choice replaces the inherited value while the
   context supports it. Show its source as “Your choice”.

Missing keys inherit; do not use truthiness, since a boolean false is real.
Use `lib/catalog.ts`'s category/id rules to identify model/mode and stable
remaining axes. Reasoning effort is one catalog axis, not a special
three-value hard-coded field. Render other select/boolean axes; unsupported
catalog shapes display unavailable and cannot be chosen here. Existing hat
security overrides remain requests even when this preview cannot represent them.

**Model-dependent axis rule:** the pre-start endpoint accepts agent/cwd,
not a model, and describes the probe's default model. Keep the origin of
each candidate value (catalog baseline, stored override, explicit pick).
When the effective model differs from `catalog.defaults.model`, mirror
`launch_configuration` in
`crates/hennery-sessions/src/delegation_runtime.rs:180`: remove inherited
baseline `axes` entries unless their id is a security axis named by
`catalog.security.axes`. No execution-policy read or merge exists. Preserve the baseline mode and all
security requirements; never prune a sandbox/approval requirement simply
because the model changed, or classify axes from display labels.

After that pruning, overlay applicable **stored and explicit** axes; they
are deliberate requests, not inherited probe defaults. A coarse pre-start
catalog cannot prove they exist for the newly selected model. Send only
explicit choices offered by that context, plus supported stored nonsecurity
overrides and every stored security requirement, then require the actual post-start
read-back to confirm every requested value before the first prompt (§6.4).
Do not call an invented per-model catalog endpoint or silently erase a
stored/explicit request merely to pass the read-back gate.

Concrete regression: the Claude probe's default is Opus with inherited
`fast=false`, but the operator or hat selects Sonnet, which omits `fast`.
With no stored/explicit `fast` request, omit that inherited nonsecurity axis;
retain the required mode/security fields and any deliberate supported
effort choice. The Sonnet session must start and receive its first prompt
after its actual configuration is confirmed, without `config_failed` from
the inherited `fast`. If `fast=false` was explicitly picked or stored as
an override and the preview still offers it, preserve its provenance/request;
Sonnet's refusal or missing read-back keeps the first prompt as a draft and
visibly explains the unsupported requested axis. If a later saved catalog
omits `fast`, show a fallback notice and omit that nonsecurity override.

Example without private fixtures: a hat stores model A and effort High;
the selected host offers only model B and Low/Medium. The launcher lists
the unavailable overrides: “Saved model A is unavailable on this host; using B”
and the equivalent effort fallback (or “using agent default” if unknown).
It requests supported baseline values instead. The stored record is not erased. Keep the
notice visible through Start, not only as a fleeting toast. If the
operator explicitly picked A and it later disappears, reject Start with a
request for a new choice rather than treating that explicit choice as implicit.

Do not map friendly labels or approximate model ids across pins. When a
model choice changes which axes apply, recompute applicability, preserve
still-valid explicit choices, and explain every discarded value. Offline,
stale/unknown/unavailable catalog contexts never promise supported config.
With no usable snapshot, show “Agent defaults”, enable Start as soon as its
host/runtime/cwd prerequisites hold, and send hat overrides and explicit picks.
Read the hat record independently of the preview; a start waits for that
record if necessary, never for a catalog probe or preview response.
Loading or refreshing a preview never gates Start. A usable cached snapshot
offers choices with its age visible; the real session still verifies them.
If a start without a preview ends `config_unconfirmed`, show the configuration
error and retain the first prompt. For each unconfirmed nonsecurity override,
offer one explicit “Start with agent default for X” action that creates a new
session omitting only that override, retaining other choices and all security
requirements, and verifies the new session before sending the prompt. Keep
the original started session accessible for review; never retry automatically.
Use the real session's catalog for review labels, then the cached catalog,
then the raw id. Protect mode and all security axes listed by the post-start
profile, including when its baseline is marked unknown. An unknown boolean
baseline displays “· agent default”, distinct from an explicit Off value.
Store editors may reset values without a catalog, but choosing a replacement
requires a usable saved or freshly refreshed context.

Changing host resets agent/path/browser as the existing launcher does;
show reset notices and recompute inherited config. Changing agent resets
its explicit config. A path/hat change recomputes hat defaults; retain only
valid explicit picks and show their source so a new hat's defaults are not
mistaken for the operator's choice. Manual hat selection is not supported:
the server resolves host/path and stores the actual hat at start.

The Defaults matrix is a view of these records. The baseline column is
read-only for the selected host/cwd, explicitly labeled as such. A hat
column whose values are not offered there shows stored/unavailable and its
fallback preview, not fake editable choices. A preview is catalog context,
not permission to start a session in a hat selected by the browser.
On mobile, use one hat section at a time with the same provenance and Reset
controls. Save is explicit; 409 keeps edits and offers Reload/review.
No automatic last-writer-wins merge after step-up or catalog refresh.

### 6.4 Starting and changing a session

Defaults resolve in the launcher; it sends only the reviewed effective
`model`, `mode`, `axes` through existing `StartSessionRequest`. No implicit
server merge is added to POST sessions, so existing clients and delegated
spawn/resume retain their semantics. Other clients may read these defaults
explicitly. Reset-to-agent in New session omits that field's override;
there is no second hidden merge that reintroduces it.

Start still resolves cwd/hat server-side. If the returned session's actual
hat differs from the preview, retain the first prompt as a draft and show
the mismatch before prompting; do not use the preview as ownership proof.
Start is followed by configuration confirmation, then a separate first
prompt POST. A 202 start alone proves neither applied config nor a
successful first prompt. Read the newly created session and its actual
post-start `SessionCatalog`; compare every requested field, including
security axes, after it becomes active. An empty/missing catalog is not
confirmation. If the actual config is absent/partial, a value is clamped,
or a configuration failure is shown, keep the first prompt as its draft and
show a configuration-review notice. Normally open the visible session;
the no-preview nonsecurity recovery in §6.3 keeps New session open with the
error, explicit retry actions and a link to that original session.
Never release it automatically when a later update happens to match.

Bound the post-start confirmation wait to 30 seconds and retain the draft
on timeout. A newly created session has no earlier attachment to confuse
with this read-back. Do not add automatic resume/repair or a new public
config-evidence API for this launcher; existing delegated resume/evidence
gates remain authoritative. The no-catalog path in §6.3 verifies every hat
override and explicit pick against the actual session. With no requested
values it checks the successful attachment and actual hat. Ambiguous delivery keeps the existing
session id and draft and never retries Start automatically. Tests must
exercise a real clamped/failed adapter config before the first prompt,
not just mock a successful POST response.

The session composer's config bar remains on
`POST /api/sessions/{id}/config` and its returned `SessionCatalog`.
Show optimistic pending state then actual adapter state; rollback and
explain refusal. It never writes hat defaults. No “save as default” gesture
is implied by changing the session's mode or effort.

## 7. Settings and retained operational behavior

Settings is sectioned into Appearance, Sign-in/passkeys, This browser's
notifications, Push devices, Notification privacy by hat, Delegation by hat,
Signed-in devices, Collector and This login. Use existing APIs for all
except §6's defaults screen. Loading failures in one section do not make
the others appear successfully loaded or erase dirty input.

For each hat, `DelegationSettings` reads
`GET /api/hats/{id}/session-tools`, shows the returned flag/revision, and
PATCHes `{delegation_enabled}` through normal step-up. Controls are disabled
while a change is pending; cancellation/refusal restores the saved value.
Show “Enabling takes effect at the next start or resume.” Explain that
disabling immediately denies built-in calls, cancels undispatched work and
routes outstanding attention to the operator; already delivered work keeps
running as ordinary sessions. Re-enabling does not replay cancelled work.
Do not say disabling stops all child processes, removes unrelated MCP
mounts, or that enabling changes unrelated permission settings. No hat-card toggle.

Operator decision, 2026-10-21: the switch is the entire delegation
configuration. ON means any runtime, model, mode and option offered by the
live fresh catalog at spawn time, including Generic/ClaudeOwnCli profiles.
There is no execution-policy route/type, grant editor, allowlist, snapshot or
per-runtime/axis narrowing. The switch writes only `delegation_enabled`
with one confirmation and never discovers hosts or probes runtimes. New
catalog values and runtimes signed in later need no re-toggle.

An unusable runtime fails only that child spawn. Catalog errors identify host,
runtime and readable reason; `auth_required` is “not signed in”. Preserve
model/mode/option read-back and complete current-generation evidence before
brief delivery. The switch help reads: “Sessions in this hat can start child sessions with any
model and mode, including modes that don't ask for permission. Children cannot
start their own children.” This keeps 0.2.0 ON's all-values authorization while
making “all” current.

Acceptance enables with an unauthenticated runtime present, signs it in later,
and spawns in any offered mode without toggling again. Unauthenticated spawns
return `auth_required` to the parent and the UI. Assert no catalog requests
or policy writes occur while enabling, and verify child configuration and
brief delivery after read-back. OFF retains immediate denial and cancellation.

Keep per-hat `PushPolicyItem` settings and their current unmuted/no-details/
session-title defaults. No per-event global switch or “only when hidden”
suppression is built. Always show a valid received push once; SSE supplies
badges without another native-notification path. “Push on” requires verified
browser/collector registration, not just permission or another device's row.
Preserve prepare/enable gestures, renewal, removal/sign-out cleanup, offline
shell behavior, no API caching and waiting-worker lifecycle from phone.

Collector information uses actual `GET /api/settings` public URL/contact;
the default for newly paired hosts comes from `HatItem.default_for_new_hosts`,
not a nonexistent global default in `SettingsResponse`. “Default on” on a
hat card is the list of hosts whose `default_hat_id` matches it; each host's
default remains editable in Hosts. Show attachment usage and host-transcript
cleanup only through their existing APIs. Operator decision, 2026-10-21:
show the actual collector release and short recorded source commit subtly in
the expanded sidebar footer and an About line at the bottom of Settings,
also at the bottom of More on phones. Read `collector_version` and `source_commit`
from authenticated `GET /api/capabilities`; the loaded page's version is
injected from the web package at build time. Refresh collector information
on window focus, visibility, browser reconnect and session-stream resync or
reconnection, coalescing paired focus/visibility events into one read.
When releases differ, add a tiny
muted `page 0.2.1 · server 0.2.2` hint; no banner or modal. Use small Roboto
text and `--hn-fg-3` in both themes. Development builds omit an unrecorded
commit; older collectors without identity show the page version and an
unavailable server version. Do not invent an
OS, password change date, doctor result, agent update/login result or cleanup
receipt. Public URL remains read-only in this scope; retain the existing
admin reset instructions and reset consequences where relevant.

MCP keeps actual delivery/health distinctions and current mount/stdio
whole-set save semantics, write-only credentials, step-up and next-start/
resume notices. Path rules remain host-specific canonical prefixes, not
glob patterns. Hat purge remains preview → confirmation/step-up → frozen
retryable purge with actual cleanup outcome. Theme and layout changes must
not weaken these contracts.

## 8. Keyboard and accessibility

### 8.1 Exact key contract

`Mod` means Meta on macOS, Ctrl elsewhere. Register global shortcuts once.
Ignore prevented events, repeats, composition/IME and Alt-modified keys;
also ignore every global shortcut when the event's composed path contains
an input, textarea, select, contenteditable, textbox/combobox/searchbox or
an editable descendant. That includes Mod+K and theme shortcuts. Never
capture a digit typed into a composer or question field.

| Key | Scope and action |
|---|---|
| `Mod+K` | Non-editing app focus: open/focus Command palette; if it is already open, retain it rather than close a query |
| `N` | Non-editing app focus: open New session; never submit it |
| `[` | Desktop non-editing focus: collapse/expand rail |
| `]` | Session with non-editing focus: toggle details panel/sheet |
| `/` | Sessions/Needs you, non-editing focus: reveal/focus Search sessions |
| `J` / `K` | Focus inside overview/queue list: move to next/previous visible session/card; no page navigation or answering |
| `Enter` | Native focused link/button activation; palette input selects its active result through its local handler |
| `R` | Session, non-editing focus: focus composer; no send, approve or takeover |
| `Mod+Shift+L` | Non-editing app focus: cycle System/Light/Dark, with announcement |
| `?` | Non-editing app focus: show shortcut help dialog |
| `1`–`9` | Existing desktop/fine-pointer permission card handler only while that card itself has focus; options in adapter order; no global queue handler |
| `Mod+Enter` | Local composer or New session form handler only: Send/Start if enabled; never handled by the global dispatcher |
| `Escape` | Local active overlay/menu: dismiss only the topmost dismissible layer; preserve its draft, restore trigger focus; no global draft clearing |

The prototype's global answer digits, E/Mark-as-seen and Mod+Shift+N quick
launch are not implemented. Elicitation radio/checkbox controls use native
arrow/Space behavior. Palette arrows/Home/End/Enter/Escape are local to its
combobox, so the global typing guard does not disable search interaction.
Displayed key hints must match active bindings; hide desktop hints on touch.

### 8.2 Semantics and focus

Rail: navigation landmark “Main navigation”; mobile bar: “Primary
navigation”; More list: “More navigation”. Use real anchors for routes and
`aria-current="page"`; do not use tab roles for cross-page navigation.
Session child/parent links remain independent links, with no button nested
inside an anchor. Status icons have words/accessible names; decorative
SVGs are aria-hidden. Every icon-only action has a name and visible focus.

Use native fieldset/legend/radio for Group by, Host, Agent, Appearance and
small effort choices, or a complete equivalent roving-tabindex radio
pattern. Agent tabs in Defaults and Family/Plan/Details tabs use tablist,
tab, tabpanel, ids/controls, selected state and arrow/Home/End behavior.
Table headers/cells use actual table semantics on desktop; mobile cards
retain explicit labels without duplicating the interactive form.

New session, mobile details/filters, palette, help and confirmations are
named modal dialogs with `aria-modal`, inert background, focus containment,
Escape, and return-to-trigger (or safe heading when it is gone). Step-up
stacks above the originating dialog and returns to it with edits intact.
Focus the first useful input/heading deliberately; do not trap focus under
the bottom bar or leave hidden desktop controls tabbable.

The palette contains an accessible search combobox, results listbox,
active-descendant and option selection; no result is executed merely by
focus/hover. Search stays scoped to authorized loaded summaries plus the
existing server search, debounced and fenced; do not scan transcripts or
claim complete results before paging. Navigation actions and contextual
New session use the same capability/current-state checks as visible UI.

Keep the existing card focus policy: a newly live question may focus only
when composer empty and focus free; it never steals typing or takes focus
from another live card/dialog. Explicit deep links focus the requested card
without activation. After an answer, announce queued/delivered state
accurately; Next requires an explicit click. Use polite status regions for
loads/save state, alerts for actionable failures, field-associated errors
and bidi-isolated untrusted names/text. Respect zoom, reduced motion,
safe areas and screen-reader order at both widths.

## 9. Verification and visual acceptance

This spec-only change needs document/link/scope checks, not application
builds. The following are requirements for the implementation plans.

### 9.1 Automated behavioral gates

| Area | Required evidence / affected tests |
|---|---|
| Theme/accent | Extend `lib/theme.test.ts`, `lib/hats.test.ts`; new `lib/accent.test.ts`; bootstrap VM/DOM tests. Hostile colors and final CSS contrast in both themes, storage migration/denial, system/explicit mode and initial paint. |
| Fonts / shipped CSS | R1 post-build check of every emitted CSS asset/import/font URL: no external origin; regression fixtures exercise external references. Browser coverage loads committed Inter/Roboto WOFF2 latin + latin-ext, used weights/italic, Polish glyphs, swap fallback, embedding/precache and unchanged CSP; check full OFL texts/notices. |
| Shell/router | `router.test.tsx`, `App.test.tsx`, Shell/view tests and `web/e2e/shell.spec.ts`: new URLs, More destinations, collapse, capabilities/gateway, auth return, direct reload and no desktop auto-selection. |
| Overview/queue | `SessionList.test.tsx`, `SessionRow.test.tsx`, scope/store tests and new NeedsYou tests: grouping, counts/paging/partial states, zero vs parent-handled attention, real status-only fallback, no transcript fan-out or extra SSE. Start from restrictive overview search/lifecycle/host/status/Hide closed, enter Needs you and reach an excluded active permission plus a closed-session status-only attention record through the hat-only/all-lifecycle query; exhaust pages and verify completion/counts. Return to Sessions and assert original filters restored; query-transition late responses cannot overwrite queue pages. |
| Push navigation | `pwa/notification.test.ts`, `pwa/navigation.test.ts`, `pwa/worker.test.ts`, `web/e2e/pwa.spec.ts`, existing producer tests: real URL → worker data.url → click → exact session/card, auth return, older question, dirty/exact window, gateway and invalid URL cases. |
| Session | Existing `screens/Session*.test.tsx`, Composer/Transcript/item/AnswerBook/FamilyPanel suites; `web/e2e/sessions.spec.ts`, `family.spec.ts`: panel/sheet, parent banner/takeover, actual option ids, multi-question input, queued/uncertain/conflict/withdrawn/delivered, cancel/resume/draft preservation. |
| Defaults backend | New kernel storage/migration tests and sessions HTTP integration tests: upgrade/reopen, owner/foreign hat, atomic revision race, empty reset/false, purge freeze/cascade, all PUT step-up/expiry/Origin/body bounds/invalid fields; add owner-audit cases and generator checks. |
| Defaults launch | New Defaults/NewSession/lib-defaults tests and `web/e2e/new-session.spec.ts`: save/reload/reset, inherited sources, two hosts with differing adapter catalogs, unsupported security overrides still sent, unsupported nonsecurity overrides fall back with a visible notice, explicit invalid picks rejected, immediate starts with every hat override without a catalog, cached age and explicit Refresh, stale responses, config clamp/failure prevents first prompt. Exercise real Opus→Sonnet without a Refresh workaround: inherited `fast=false` omitted, required security axes preserved, supported deliberate effort retained and first prompt delivered after read-back. A no-preview unsupported `fast` request holds the prompt and offers one-click new start with agent default for only that axis. Assert actual start frames, live review labels, unknown boolean labels and no prompt before confirmation. |
| Settings/management | Existing settings/manage/API tests, `web/e2e/manage.spec.ts`, `mcp.spec.ts`, `mcp-oauth.spec.ts`, `mcp-delivery.spec.ts` and new `delegation-settings.spec.ts`: new layout, default-hat controls, delegation enable/disable/step-up/cancel, binary delegation without catalog probes, per-hat privacy, secrets and device lifecycle intact. Actually spawn through enabled Settings with any offered mode, including an unknown-security profile. Enabling with an unauthenticated runtime succeeds; signing in later needs no re-toggle; an unauthenticated spawn returns auth_required. Defaults never enable delegation. |
| Keyboard | New command/shortcut and palette tests plus browser checks: every key, editable/composed-path/select/IME guards, card-focus digits, dialog focus/return, typing never mutates an answer or starts a session. |
| Rendering/CSP | Existing `security.test.ts`, `deps.test.ts`, browser CSP collectors and Markdown computed-style checks; no external assets, inline script, lost list markers or mobile controls. |

All new/changed Playwright/Testing Library text locators use
`{ exact: true }` or roles with precise accessible names, never short
substrings. Changes under `web/e2e/**` run locally before commit. Preserve
meaningful assertions when selectors change; do not weaken a failing test
to fit the new layout. Regression guards are observed RED and removal-probed
under fleet rules; timing/concurrency tests alone get the required 4×3 load
runs. CI cost and replay frequency follow the worktree's verification budget.

Run inside `nix develop -c`: targeted suites during work; final web
typecheck/test/build and `cargo build -p hennery -p hennery-testkit --bins
--locked` then `CI=1 pnpm --dir web e2e` against the embedded UI. Backend
R4 additionally runs fmt, both clippy commands, both generators and the
workspace test gate once for its plan. Each executable plan specifies its
exact checks/replay/probes; documentation does not claim them pre-run.
If a lockfile changes, update the Nix web dependency hash as required.

### 9.2 Screenshot and installed-device acceptance

Every R1–R6 report includes Playwright screenshots of **every screen and
overlay it touched**, at **390 px and 1280 px**, in **light and dark**.
Use deterministic synthetic data, stable timestamps, reduced animation and
the actual embedded application. Record viewport height (390×844 and
1280×900 recommended), theme, route/state, reference frame and intentional
deviations. Put captures/diff notes in that lane's gitignored ledger/artifact
directory, not the repository design bundle. Do not export private sample
content into fixtures/screenshots.

| Production capture | Prototype reference |
|---|---|
| Needs you, empty and populated; mobile navigation | Mobile `1 · Needs you tab`; Desktop `overview`, Needs you filter |
| Sessions, grouped by hat/host, filters and collapsed rail | Mobile `1b · Sessions tab`; Desktop `overview` |
| Running child with Family sheet / panel | Mobile `1c · Running session · family sheet`; Desktop `session`, panel open/closed |
| Permission card and bottom answer actions | Mobile `3 · Deep link → permission card`; Desktop `session`, permission live/done |
| Elicitation form and acknowledgement | Mobile `4 · Answer a question`; Design System `Question card` |
| New session and config Change | Mobile `5 · New session sheet`; Desktop `newOpen` |
| Defaults, including mobile adaptation | Desktop `defaults`; Design System inherits/override/config controls |
| Hosts/pairing, MCP/stdio, Hats, Settings | Desktop `hosts`, `mcp`, `hats`, `settings`; mobile adaptations of those screens |
| Palette/help | Desktop `paletteOpen`; Design System input/keycap primitives |
| Login, Setup, More, Not found, gateway, errors/offline | Design System light/dark primitives; explicitly state “no prototype frame” |

Mobile `2 · Push on the lock screen` is a native notification reference,
not an app screenshot target or promise of lock-screen buttons. Demonstrate
the tap-to-session flow instead. Compare geometry, density, hierarchy,
surfaces and responsive behavior; wait for the self-hosted Inter/Roboto
faces before capture. Document the deliberate hat-accent, read-only queue
and unsupported-data deviations. A screenshot
alone cannot prove contrast, focus or delivery semantics; pair it with the
behavior/computed-style checks above.

Final integration acceptance includes installed-iOS safe-area/keyboard and
push tap → specific child/session → live answer, in both appearances,
without lock-screen actions. Keep the existing phone acceptance limitations:
Playwright cannot prove actual OS push display. Use synthetic projects on
real pinned adapters for model/mode/effort read-back and a heterogeneous-host
defaults case. Parent owns release authorization and final platform evidence.

## 10. Ordered sub-plans and exclusive file ownership

Use **UI R1–R6** in titles to distinguish these from existing release and
memory plans with R identifiers. All branches target `redesign`; branch
from the reviewed shared foundation and integrate through the parent.
R1 → R2 is sequential. R3–R6 can start after R2. R5's defaults summary can
initially link to Defaults and consume R4's API after integration; its
acceptance of saved summaries waits for R4. R6's context actions similarly
wait for the corresponding provider for final integration, without editing
another lane's files.

| Plan | Scope and dependency | Owned files / verification focus |
|---|---|---|
| R1 — tokens, appearance, assets | Foundation; no dependency | `styles/tokens.css`, `styles/fonts.css`, `web/public/fonts/**`, `web/third-party-licenses/**`, `web/THIRD_PARTY_NOTICES.txt`, `lib/accent.ts`, `lib/theme.ts`, hat palette helpers, `web/public/theme.js`, `components/ui/**`; `screens/Login.tsx`, `screens/Setup.tsx`, `components/ConfirmDialog.tsx`, `components/StepUpDialog.tsx` and their tests. Own the emitted-CSS origin check/fixtures, font browser coverage and any required build-script wiring. Own `index.css`/`manage.css` only for token aliases/base cleanup in this step. Capture all existing screens after token/font adoption, both themes; prove Polish coverage, contrast/bootstrap/CSP. |
| R2 — shell, overview, queue, router, push | After R1; establishes all parallel seams | `App.tsx`/`App.test.tsx`, `app-actions.tsx` and its tests, `api/delegation.ts` and its tests; `components/Shell.tsx`, `SessionScope.tsx`, `NavigationRail.tsx`, `BottomNavigation.tsx`, `SessionList.tsx`, `SessionRow.tsx`, `HatSwitch.tsx`, `SessionFilters.tsx`, `NeedsYouQueue.tsx`, `AttentionPreview.tsx`, `CommandEntry.tsx`; `screens/Sessions.tsx`, `NeedsYou.tsx`, `More.tsx`, `Placeholder.tsx`; `router.tsx`, `lib/views.ts`, status/group helpers, `styles/shell.css`; shell/list/router/push tests, `web/e2e/shell.spec.ts`, `pwa.spec.ts`. Own worker/app-route allowlist integration, hat-only/all-lifecycle queue query and deep-link preservation. |
| R3 — session | After R2 | `screens/Session.tsx`, session header/footer/menu, Transcript/Composer/FamilyPanel/StepList/McpDelivery/StatusAttention, `components/items/**`, new SessionDetails/QuestionActions/SessionConfig, `styles/session.css`; corresponding tests; `web/e2e/sessions.spec.ts`, `family.spec.ts`, `windowing.spec.ts`. Own session action-provider registration, read-back display, mobile forms and preservation of windowing measurements; do not edit launcher or shared catalog parser. |
| R4 — launcher and defaults | After R2 | `screens/NewSession.tsx`, `Defaults.tsx`, ProjectPicker/NewSessionFrame/LaunchConfig/CatalogContext/DefaultsEditor, `api/defaults.ts`, `lib/defaults.ts`, `lib/start.ts`, `lib/catalog.ts`, `styles/launch.css`; new defaults backend/store/proto/schema/owner-audit changes; launcher/defaults tests and `web/e2e/new-session.spec.ts`. Own protocol generation and defaults capability; verify persistence/security/catalog divergence/first-prompt gate. |
| R5 — management and Settings | After R2; final defaults summaries after R4 | `screens/Hosts.tsx`, `Mcp.tsx`, `Hats.tsx`, `Settings.tsx`; Pairing/PathRules/Mounts/MCP/stdio/OAuth forms, new Account/Notification/Delegation/CollectorSettings, management API helpers, `manage.css`; related tests; `web/e2e/manage.spec.ts`, `mcp.spec.ts`, `mcp-oauth.spec.ts`, `mcp-delivery.spec.ts`, `delegation-settings.spec.ts`. Own the binary delegation-toggle UI using R2's shared API facade; preserve current backend authorization and write-only secrets. |
| R6 — commands and shortcuts | After R2; contextual acceptance with R3–R5 | `components/CommandPalette.tsx`, `ShortcutHelp.tsx`, `CommandHost.tsx`, `hooks/useShortcuts.ts`, `lib/commands.ts`, `styles/commands.css`; command tests and new `web/e2e/commands.spec.ts`. Consume registered actions, never copy another screen's mutation logic. |

R2 must finish these integration seams before parallel branches start:

1. Register the final URLs, capability checks and lazy entry points for
   Defaults, the palette and launcher overlays. R2 may supply neutral
   placeholders at those entry points; file ownership transfers to R4/R6
   after R2. No lane needs to edit `Shell.tsx`, `router.tsx` or `lib/views.ts`
   merely to insert its screen later.
2. Establish the action/context registry in `web/src/app-actions.tsx`
   (and its colocated tests), owned by R2. Its stable actions cover
   opening New session, focusing overview search, toggling rail/details,
   focusing composer and opening configured management destinations.
   Screen owners register enabled handlers in their own files; R6 only
   dispatches through those exports. R6 owns command definitions in
   `web/src/lib/commands.ts`, not the registry; it does not edit `App.tsx`
   or the provider. Unavailable actions are disabled/omitted, never guessed.
3. Extract shared shell CSS and existing session-specific CSS from
   `index.css` before ownership transfers, maintaining current behavior.
   Subsequent lane CSS imports live in each owned screen/module. R5 owns
   `manage.css`; R3/R4/R6 never append to it or shared `index.css`.
4. Keep accessible names for existing canonical card/composer/API controls
   stable where they are already meaningful. Assign each e2e file once
   as above; R4 adds launcher cases in its new file rather than editing
   R3's session suite. Integration selector conflicts go through the parent.
5. Document exports of common UI primitives and the action registry.
   R3 consumes `lib/catalog.ts` read-only; R4 owns additive parser changes
   and preserves existing exports/semantics. Session and launch config
   presentations have separate owned modules using the same primitives.
6. R2 provides `web/src/api/delegation.ts` with generated-type wrappers for
   GET/PATCH session-tools and the existing runtime-catalog GET. R4 consumes
   catalog metadata; R5 consumes session-tools reads/writes for Settings. Both use this
   facade read-only after R2, so parallel work does not duplicate wire types
   or collide in the helper file. Any API-contract change goes through
   the parent as a shared-foundation amendment.

The remaining ownership boundaries are explicit: R1 finishes Login/Setup
and shared confirmation/step-up presentation, focus containment and stable
props before R2; those components/tests remain R1-owned shared foundation,
with no transfer to R3/R5. R2 owns `App.tsx` and its root integration/tests
after R1, including mounting the R6 placeholder host; R6 only replaces its
owned placeholder implementation. `index.css` transfers from R1 to R2 for
the extraction above and is then frozen for parallel lanes. `manage.css`
transfers R1 → R5 after R2. R3 alone adapts `windowing.spec.ts` to the
session layout without weakening measurement assertions. No other implicit
transfer of a shared file is authorized; integration changes go through
the parent.

Generated protocol/schema files and the owner-audit suite are shared
hotspots owned only by R4 for this redesign; parent integrates any outside
lane changes and resolves migration numbers. Each implementation plan,
execution record and its `docs/README.md` entry is integrated serially by
the parent to avoid all six lanes editing the document index at once.

No lane pushes to `main`, opens a GitHub PR through the child account or
deploys. Reports carry commit ranges, command exit statuses/counts, screenshot
paths/reference frames, intentional deviations and review requests.
The parent performs independent reviews, checks full PR status (checks,
review decision, merge state and unresolved threads), merges the stack
into `redesign` and runs final integrated acceptance before its single
release PR to `main`.

## 11. Deferred and review request

Deferred: lock-screen mutations; global per-event push preferences; logo UI;
new diagnostics/update/doctor telemetry; inferred “your turn”/read state;
global editable agent defaults; automatic direct-start palette presets;
file-mention completion; new provider/model aliases; remote font/icon loads;
public-URL editing and a password-change API; new memory/board/workflow pages.
The existing backends for logos, public URL and optional
plug-ins remain available under their established contracts.

The parent should request scoped re-confirmation of review findings 1–4
and O8's font implementation contract, then record the outcome. D2–D12
remain designer's recommendations; D1 is replaced by operator decision O8.
Focus the review on sparse defaults storage and all-write step-up, catalog fallback
and first-prompt read-back, separation from delegation authority, partial
queue coverage, the read-only queue deviation, accent contrast and the
parallel ownership seams. Record approval/amendments and scoped
re-confirmation before writing R1's executable plan. No unresolved
operator-level product question is introduced by this specification.

## 12. Redesign v2 — session switching and hat scope (2026-10-21)

The operator's local Desktop v2 prototype defines session switching. The
sidebar has up to six recently **visited** sessions, independent of
agent activity. Each row has a status icon, an 8 px hat-colour dot before the
title (hat name in its tooltip), and a highlight for the current session.
Collapsed navigation retains 18 px status icons with a hat-colour badge,
accessible names and full tooltips. Distinct glyphs and semantic foregrounds
identify question, permission, reply, running, idle, parked, offline, failed,
starting, parent handling and waiting on children states. Phone uses
the existing Jump to palette rather than a
sidebar.

The collapsed rail footer uses icon buttons for appearance and sign-out, with
accessible names and tooltips; its version line is hidden. The automatic
narrow desktop rail follows the same rules. Answer pending delivery uses a
question bubble in the starting foreground, distinct from operator work.

The existing summary contract exposes a pending-question flag but not its
kind. An open pending question therefore uses the question bubble; blocked
attention without a pending question uses the permission hand. Distinguishing
permission requests from other pending questions requires a future summary
contract addition; this UI lane does not add backend requests or fields.

Operator amendment (2026-10-22): visible Recent entries keep their positions
when opened, renamed or updated. Selection only changes the highlight. Opening
a session outside the six visible entries inserts it first; the oldest slot
leaves the visible set but remains in the bounded history. Determine visibility
after hat filtering and authorized hydration. Existing persisted history is
used in place, without a reset or a new storage format. Other windows share
this order; new entries from either window can change the visible set.
Operator amendment (2026-10-23): closed sessions are excluded from sidebar
Recent, palette Recent and its navigation targets, including Back. Apply this
filter before filling visible slots, on hydration and live summary updates.
Parked and failed sessions remain eligible; closed transcripts remain reachable
through the session list, search and direct links.
Option/Alt+1–5 target the first five visible positions, including the current
session; the sixth entry remains clickable. Backtick opens
this window's previous visited session, using a two-id sessionStorage history.
Other windows share only the browser-wide Recent order and numbered targets.
The command palette puts **Recent** first in shared Recent order and
preselects that previous visit, so Command/Ctrl+K followed by Enter also goes
back. Searching still searches authorized sessions through the existing API;
unvisited loaded sessions are labelled Sessions rather than Recent. The
shortcut help lists only enabled bindings, hides numbered shortcuts on touch
layouts and labels backtick as US-layout only (dead keys do not fire it).
Global shortcuts do not fire inside
inputs, textareas, selects, editable content, active dialogs, during IME
composition (including key code 229), on repeat or after preventDefault.

The selected hat is shared browse scope: it filters the sessions list,
Needs you, sidebar Recent, palette Recent/search and displayed counts.
**All hats** restores all authorized sessions. A session opened by a direct
link may be outside the browse scope; its session-local action context must
not override that scope. Overview grouping, sorting and project subgroups
are implemented independently and consume the same scope.

Visit history is per browser in localStorage, bounded to 50 unique ids and
never synced to the collector. The server creates UUIDv7 session ids; history
validates UUID shape while retaining older/imported UUID versions. Summaries
are rehydrated through authorized id searches and reconciled with the existing
list stream; list filters and pagination cannot themselves prune a visit.
Exact-id misses on an exhausted search, authorization/not-found refusals,
stream removals and session deletion prune history. Transient read failures
retain ids and retry once after a 500 ms backoff; they never mark an id as
authoritatively checked. The retry resumes the failed page and preserves the
pagination budget. Further attempts wait for reconnect/resync or targeted scope
revalidation. Reads run on mount and after a
resync/reconnect, with at most four in flight and three pages per id; a capped
search does not prove deletion. Search, hat and Needs you query changes retain
checked state and cached hats; known sessions outside the selected hat are
skipped. The hat-scoped stream broadcasts removals for other hats as well.
A new query anchor can still miss an outside-hat deletion: when a scope
expands, absent cached entries that the old hat excluded are hidden and
revalidated through exact-id lookup. A filtered/page miss cannot itself delete
history, and entries included by the old scope are not needlessly reread.
Storage ids are not owner-scoped: sign-out clears them and a new
owner's authorized lookups prune inaccessible ids. Confirmed sign-out (including cross-tab
sign-out) clears history with other local authenticated state; ordinary
authentication expiry preserves it. Switching and scope need no backend addition;
the pending-question-kind icon limitation is recorded above.

Generated with Claude AI — please review before distribution.
