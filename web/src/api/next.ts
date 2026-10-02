// The return target after a sign-in (frontend spec §3): a path of this
// origin only, so `?next=` can never send the browser elsewhere.

/** Where a signed-in browser goes when `next` is absent or refused. */
export const HOME = '/sessions'

/** The pages a return target never names: going back to them would loop. */
const NOT_A_TARGET = ['/login', '/setup']

/**
 * `raw` as a path of `origin` (path and query, never the fragment), or
 * `HOME`. Accepted only when it starts with exactly one `/` and, parsed
 * against `origin`, stays on it and still starts with exactly one `/`: a
 * parser turns `/\host`, `/..//host` and their encodings into another
 * host's path, which this refuses.
 */
export function safeNext(raw: string | null | undefined, origin: string): string {
  if (!raw || !raw.startsWith('/') || raw.startsWith('//') || raw.startsWith('/\\')) return HOME
  let url: URL
  try {
    url = new URL(raw, origin)
  } catch {
    return HOME
  }
  if (url.origin !== origin) return HOME
  const path = url.pathname
  if (!path.startsWith('/') || path.startsWith('//') || path.startsWith('/\\')) return HOME
  if (NOT_A_TARGET.some((p) => path === p || path.startsWith(p + '/'))) return HOME
  return path + url.search
}

/** The login page that returns to `path` (a path and query) once signed in. */
export function loginHref(path: string): string {
  return '/login?next=' + encodeURIComponent(path)
}
