// The setup link's token (kernel spec §3.1): `/setup#<token>`. Read once,
// when this module loads — `main.tsx` imports it before anything else — and
// removed from the address bar at once, so it never stays in the tab's
// history entry, a bookmark, a `?next=` or another module's view of
// `location`. (The browser's global history still records the visit: an
// accepted risk, as the link is single use.) It is kept only here, in
// memory, until setup has its answer.

let token: string | null = null

if (location.pathname === '/setup' && location.hash.length > 1) {
  token = location.hash.slice(1)
}
if (location.pathname === '/setup' && location.hash !== '') {
  history.replaceState(null, '', '/setup')
}

/** The token the page was opened with, unless used up. */
export function setupToken(): string | null {
  return token
}

/** Setup has its final answer: the token is of no further use. */
export function forgetSetupToken(): void {
  token = null
}
