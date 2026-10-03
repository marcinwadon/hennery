/** How long before `now` `iso` was, in words. A list passes its own ticking
 *  `now` (`useNow`), so its rows move on while nothing else renders. */
export function relTime(iso: string, now: number = Date.now()): string {
  if (!iso) return ''
  const s = Math.floor((now - new Date(iso).getTime()) / 1000)
  if (isNaN(s)) return ''
  if (s < 5) return 'just now'
  if (s < 60) return `${s}s ago`
  const m = Math.floor(s / 60); if (m < 60) return `${m}m ago`
  const h = Math.floor(m / 60); if (h < 24) return `${h}h ago`
  return `${Math.floor(h / 24)}d ago`
}
// One formatter for every call: `toLocaleTimeString` builds a new one each
// time, which a transcript of a thousand items pays a thousand times.
let clock: Intl.DateTimeFormat | undefined
export function clockTime(iso: string): string {
  if (!iso) return ''
  const d = new Date(iso)
  if (isNaN(d.getTime())) return ''
  clock ??= new Intl.DateTimeFormat([], { hour: '2-digit', minute: '2-digit' })
  return clock.format(d)
}
export function basename(p: string): string {
  if (!p) return ''
  const parts = p.split('/').filter(Boolean)
  return parts.length ? parts[parts.length - 1] : p
}

/** Which calendar-relative band a timestamp falls in. The session list's only
 *  grouping. */
export type Bucket = 'today' | 'yesterday' | 'week' | 'month' | 'earlier'

/** Display order, top to bottom. */
export const BUCKET_ORDER: readonly Bucket[] = [
  'today',
  'yesterday',
  'week',
  'month',
  'earlier',
]

export const BUCKET_LABEL: Record<Bucket, string> = {
  today: 'Today',
  yesterday: 'Yesterday',
  week: 'This week',
  month: 'This month',
  earlier: 'Earlier',
}

/** Local midnight of the day containing `ms`. Uses the local getters, which is
 *  what makes the buckets calendar-relative rather than rolling windows — a
 *  rolling 24h window makes the "Today" header lie at 00:30. */
function dayStart(ms: number): number {
  const d = new Date(ms)
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime()
}

/**
 * The bucket a timestamp belongs to, relative to `now`.
 *
 * Unparseable and empty inputs fall to `earlier` rather than throwing, so one
 * bad row cannot break the list.
 *
 * The day delta uses `Math.round`, NOT `Math.floor`: both operands are local
 * midnights, so a DST transition makes consecutive days 23h or 25h apart.
 * Measured in Europe/Warsaw, 2026-03-29 to 2026-03-30 is 23h, and
 * `Math.floor(23h / 24h)` is 0 — which would label yesterday's session "Today".
 * Rounding is exact for every real delta because the error is at most one hour.
 */
export function bucketOf(iso: string, now: number = Date.now()): Bucket {
  if (!iso) return 'earlier'
  const t = new Date(iso).getTime()
  if (isNaN(t)) return 'earlier'
  const days = Math.round((dayStart(now) - dayStart(t)) / 86_400_000)
  if (days <= 0) return 'today'
  if (days === 1) return 'yesterday'
  if (days < 7) return 'week'
  if (days < 30) return 'month'
  return 'earlier'
}
