// The New Session project picker's list (frontend spec §7): the host's
// recent projects and its enumerated repositories, merged, labelled and
// ranked against what the operator types.
import type { HostProjects } from '../generated/protocol'
import { basename } from './time'

/** One selectable project: a directory on the host. */
export interface ProjectEntry {
  /** The directory's name, or `parent/name` when another entry shares it. */
  name: string
  /** Absolute and canonical, as the host reported it. */
  path: string
  /** RFC 3339, for a recent project. */
  lastUsed?: string
}

function segments(p: string): string[] {
  return p.split('/').filter(Boolean)
}

/** `parent/name`: the label for names that collide. */
function qualified(p: string): string {
  const parts = segments(p)
  return parts.length >= 2 ? parts[parts.length - 2] + '/' + parts[parts.length - 1] : basename(p)
}

/** Every entry whose name another entry shares is labelled `parent/name`,
 *  both sides of the collision, so no two rows read the same. */
export function disambiguate(entries: ProjectEntry[]): ProjectEntry[] {
  const counts = new Map<string, number>()
  for (const e of entries) counts.set(e.name, (counts.get(e.name) ?? 0) + 1)
  return entries.map((e) => ((counts.get(e.name) ?? 0) > 1 ? { ...e, name: qualified(e.path) } : e))
}

/** Recents first, in the server's order (newest first), then the
 *  enumerated repositories alphabetically. A path that is both appears
 *  once, as the recent. */
export function mergeProjects(recents: ProjectEntry[], enumerated: ProjectEntry[]): ProjectEntry[] {
  const seen = new Set(recents.map((r) => r.path))
  const rest = enumerated.filter((e) => !seen.has(e.path)).sort((a, b) => a.name.localeCompare(b.name))
  return disambiguate([...recents, ...rest])
}

/** The picker's entries for one host's `GET /api/hosts/{id}/projects`. */
export function projectEntries(projects: HostProjects): ProjectEntry[] {
  return mergeProjects(
    projects.recents.map((r) => ({ name: basename(r.path), path: r.path, lastUsed: r.last_used_at })),
    projects.items.map((p) => ({ name: basename(p.path), path: p.path })),
  )
}

/** Every character of `q` appears in `s`, in order. */
function subsequence(s: string, q: string): boolean {
  let i = 0
  for (let k = 0; k < s.length && i < q.length; k++) if (s[k] === q[i]) i++
  return i === q.length
}

/** Filtered and ranked by how directly the name matches: exact, prefix,
 *  substring, subsequence; case-insensitive. Non-matches are dropped. The
 *  sort is stable, so recency order survives within a rank. */
export function filterProjects(entries: ProjectEntry[], query: string): ProjectEntry[] {
  const q = query.trim().toLowerCase()
  if (q === '') return entries
  const scored: { entry: ProjectEntry; rank: number }[] = []
  for (const entry of entries) {
    const name = entry.name.toLowerCase()
    let rank: number
    if (name === q) rank = 0
    else if (name.startsWith(q)) rank = 1
    else if (name.includes(q)) rank = 2
    else if (subsequence(name, q)) rank = 3
    else continue
    scored.push({ entry, rank })
  }
  return scored.sort((a, b) => a.rank - b.rank).map((x) => x.entry)
}

/** Text meant as a path, not a search: it starts with `/` or `~` (the
 *  host expands `~` to its user's home). */
export function isLiteralPath(text: string): boolean {
  return text.startsWith('/') || text.startsWith('~')
}

/** A subdirectory's path under a listed directory. */
export function childPath(dir: string, name: string): string {
  return dir.endsWith('/') ? dir + name : dir + '/' + name
}
