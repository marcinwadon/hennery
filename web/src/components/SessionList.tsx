import { Fragment, useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from 'react'
import { useNow } from '../hooks/useNow'
import type { SessionSummary } from '../generated/view'
import { BUCKET_ORDER, basename, bucketOf, type Bucket } from '../lib/time'
import { compareStarted, overviewGroups, projectLabel, withFamilyParents } from '../lib/sessionGroups'
import { matchesStatus, needsOperator } from '../lib/status'
import { Link, useLocation } from '../router'
import { useMediaQuery } from '../hooks/useMediaQuery'
import { Icon, useModalLayer } from './ui'
import { useRegisterAction } from '../app-actions'
import SessionRow from './SessionRow'
import SessionListActions from './SessionListActions'
import SessionBulkActions from './SessionBulkActions'
import HatSwitch from './HatSwitch'
import SessionFilters from './SessionFilters'
import { useSessionScope } from './SessionScope'
import NeedsYouQueue from './NeedsYouQueue'
import SessionGroupingControls from './SessionGroupingControls'
export const PAGE_ROWS = 50
export const CLOCK_MS = 60_000
const EXPANDED_KEY = 'hennery.expandedFamilies'
const COLLAPSED_KEY = 'hennery.collapsedProjects'
const COLUMNS = ['Session', 'Doing', 'Host', 'Agent / model', 'Last']
function rememberedSet(key: string): Set<string> {
  try { const value: unknown = JSON.parse(sessionStorage.getItem(key) ?? '[]'); return new Set(Array.isArray(value) ? value.filter((id): id is string => typeof id === 'string') : []) } catch { return new Set() }
}
function childPriority(s: SessionSummary) { return needsOperator(s) ? 0 : s.activity === 'running' ? 1 : 2 }
function childPreview(children: SessionSummary[], sort: string) {
  return [...children].sort((a, b) => (sort === 'attention' ? childPriority(a) - childPriority(b) : 0) || (sort !== 'started' ? (Date.parse(b.last_event_at) || 0) - (Date.parse(a.last_event_at) || 0) : 0) || compareStarted(a, b)).slice(0, 5)
}
// Retained for consumers of the old calendar helper; overview uses hats/hosts.
export function byDay(rows: readonly SessionSummary[], now: number): [Bucket, SessionSummary[]][] {
  const days = new Map<Bucket, SessionSummary[]>()
  for (const row of rows) { const key = bucketOf(row.last_event_at, now); const group = days.get(key) ?? []; group.push(row); days.set(key, group) }
  return BUCKET_ORDER.filter(key => days.has(key)).map(key => [key, days.get(key)!])
}
/** Local J/K only: focus moves within visible links, never activates them. */
export function moveListFocus(event: KeyboardEvent<HTMLElement>) {
  if (event.defaultPrevented || event.repeat || event.nativeEvent.isComposing || event.altKey || event.metaKey || event.ctrlKey || event.shiftKey || !['j', 'k'].includes(event.key)) return
  if ((event.target as HTMLElement).closest('input, textarea, select, [contenteditable], [role="textbox"], [role="combobox"], [role="searchbox"]')) return
  const links = [...event.currentTarget.querySelectorAll<HTMLElement>('[data-session-link]')].filter(link => !link.closest('[hidden]'))
  const index = links.indexOf(document.activeElement as HTMLElement)
  if (index < 0) return
  const next = links[index + (event.key === 'j' ? 1 : -1)]
  if (next) { event.preventDefault(); next.focus() }
}
function FilterDialog({ children, close }: { children: ReactNode; close: () => void }) {
  const ref = useRef<HTMLDivElement>(null)
  const { trap, holdFocus } = useModalLayer(ref)
  useEffect(() => {
    const before = document.activeElement as HTMLElement | null
    ref.current?.querySelector<HTMLInputElement>('input[type="search"]')?.focus()
    return () => { if (before?.isConnected) before.focus() }
  }, [])
  return <div className="overlay" onMouseDown={holdFocus}><div ref={ref} className="modal filter-sheet" role="dialog" aria-modal="true" aria-label="Session filters" tabIndex={-1} onKeyDown={event => trap(event, close)}><header className="overview-head"><h2>Session filters</h2><button type="button" className="btn btn-ghost" onClick={close}>Close filters</button></header>{children}</div></div>
}
export default function SessionList({ screen = false, attentionOnly = false }: { screen?: boolean; attentionOnly?: boolean }) {
  const scope = useSessionScope(), now = useNow(CLOCK_MS)
  const desktop = useMediaQuery('(min-width: 768px)')
  const { pathname, search } = useLocation()
  const [shownRows, setShownRows] = useState({ query: '', n: PAGE_ROWS })
  const [filters, setFilters] = useState(false)
  const [expanded, setExpanded] = useState(() => rememberedSet(EXPANDED_KEY))
  const [collapsedProjects, setCollapsedProjects] = useState(() => rememberedSet(COLLAPSED_KEY))
  const toggleProject = useCallback((id: string) => setCollapsedProjects(before => {
    const next = new Set(before); if (next.has(id)) next.delete(id); else next.add(id)
    try { sessionStorage.setItem(COLLAPSED_KEY, JSON.stringify([...next])) } catch { /* In-memory collapse still works. */ }
    return next
  }), [])
  const toggleFamily = useCallback((id: string) => setExpanded(before => {
    const next = new Set(before)
    if (next.has(id)) next.delete(id); else next.add(id)
    try { sessionStorage.setItem(EXPANDED_KEY, JSON.stringify([...next])) } catch { /* In-memory expansion still works. */ }
    return next
  }), [])
  useEffect(() => { if (new URLSearchParams(search).has('search')) setFilters(true) }, [search])
  useRegisterAction('search-sessions', { label: 'Search sessions', enabled: pathname === '/sessions', run: () => {
    setFilters(true)
    requestAnimationFrame(() => document.querySelector<HTMLInputElement>('[aria-label="Search sessions"]')?.focus())
  } })
  const matching = useMemo(() => {
    if (!scope) return []
    const { list, status, host } = scope
    return attentionOnly || (!list.searching && status === 'attention') ? scope.attentionRows.filter(s => !scope.isAttentionDismissed(s.session_id) && (!host || s.host_id === host)) : list.shown.filter(s => list.searching || ((!host || s.host_id === host) && matchesStatus(s, status)))
  }, [scope, attentionOnly])
  const filtered = useMemo(() => !scope || attentionOnly ? matching : withFamilyParents(matching, scope.list.shown.filter(s => !scope.hat || s.hat_id === scope.hat)), [matching, scope, attentionOnly])
  const allGroups = useMemo(() => overviewGroups(filtered, scope?.group ?? 'hat', scope?.subgroup ?? 'project', scope?.sort ?? 'attention'), [filtered, scope?.group, scope?.subgroup, scope?.sort])
  useEffect(() => {
    // Prune against every loaded group, not only the visible page. Loading
    // and failed reads cannot establish that a remembered project is gone.
    if (!scope || scope.list.loading || scope.list.error) return
    const present = new Set(allGroups.flatMap(group => group.projects.map(project => project.id)))
    setCollapsedProjects(before => {
      const next = new Set([...before].filter(id => present.has(id)))
      if (next.size === before.size) return before
      try { sessionStorage.setItem(COLLAPSED_KEY, JSON.stringify([...next])) } catch { /* In-memory pruning still works. */ }
      return next
    })
  }, [allGroups, scope])
  if (!scope) return null
  const { list, hats, hat, chooseHat, hostNames, group, sort, selectedId } = scope
  const limit = shownRows.query === list.queryKey ? shownRows.n : PAGE_ROWS
  let remaining = limit
  const groups = allGroups.map(g => ({ ...g, projects: g.projects.map(project => {
    if (collapsedProjects.has(project.id)) return { ...project, families: [] }
    const families = project.families.slice(0, remaining)
    remaining -= families.length
    return { ...project, families }
  }).filter(project => project.families.length || collapsedProjects.has(project.id)) })).filter(g => g.projects.length)
  const rootCount = allGroups.reduce((n, g) => n + g.projects.reduce((n, project) => n + (collapsedProjects.has(project.id) ? 0 : project.families.length), 0), 0)
  const more = rootCount > limit || list.hasMore
  const visibleIds = new Set(list.loading ? [] : groups.flatMap(group => group.projects.flatMap(project => project.families.flatMap(family => [family.root.session_id, ...(expanded.has(family.root.session_id) ? childPreview(family.members.filter(child => child.family?.parent?.session_id === family.root.session_id), sort).map(child => child.session_id) : [])]))))
  const selectionScope = JSON.stringify([scope.query, hat, scope.host, scope.status, scope.lifecycle, scope.hideClosed])
  return <SessionListActions sessions={list.all} hosts={scope.hosts} onRemoved={list.remove}><SessionBulkActions enabled={!attentionOnly && pathname === '/sessions'} scopeKey={selectionScope} sessions={list.all} hosts={scope.hosts} visibleIds={visibleIds}>{bulk => <section className={'slist overview' + (screen ? ' slist-screen' : '')} aria-label="Session list" onKeyDown={moveListFocus}>
    <header className="overview-head"><h1 tabIndex={-1} aria-label={attentionOnly ? 'Needs you' : 'Sessions'}>{attentionOnly ? 'Needs you' : desktop ? 'All sessions' : 'Sessions'}</h1><span className="overview-counter" title={`${matching.length}${list.hasMore ? ' shown' : ' sessions'} · ${scope.hosts?.filter(h => h.revoked_at == null).length ?? 0} hosts`}>{matching.length}{list.hasMore ? ' shown' : ' sessions'} · {scope.hosts?.filter(h => h.revoked_at == null).length ?? 0} hosts</span>{desktop && !attentionOnly && <SessionGroupingControls />}{attentionOnly && <button type="button" className="btn btn-ghost" onClick={() => document.querySelector<HTMLInputElement>('[aria-label="Search sessions"]')?.focus()}>Search</button>}<button type="button" className="btn btn-ghost filter-trigger" onClick={() => setFilters(!filters)} aria-expanded={filters}>Filters</button>{bulk.selectButton}<Link to="/new" className="btn btn-primary">New session</Link></header>
    <fieldset className="bulk-filter-controls" disabled={bulk.busy}><HatSwitch hats={hats} hat={hat} onHat={chooseHat} />
    {!desktop && filters ? <FilterDialog close={() => setFilters(false)}><SessionFilters grouping /></FilterDialog> : <div className="filters-default"><SessionFilters grouping={!desktop} /></div>}
    </fieldset>
    {bulk.panel}
    {desktop && !list.searching && <NeedsYouQueue embedded />}
    {list.error && <p role="alert" className="form-error list-note"><bdi>{list.error}</bdi></p>}
    {!list.loading && (list.stream === 'reconnecting' || list.resynced) && <p className="list-note" role="status">{list.stream === 'reconnecting' ? 'Reconnecting…' : 'Resynced'}</p>}
    {!attentionOnly && (list.loading ? <p className="list-note" role="status">Loading…</p> : !groups.length ? <p className="list-note">{list.searching ? 'No matches' : 'No sessions'}</p> : <>
    {desktop && <table className="session-table session-columns" aria-hidden="true"><thead><tr>{COLUMNS.map(label => <th key={label}>{label}</th>)}</tr></thead></table>}
    {groups.map(g => <div className="grp" key={g.id}>
      <h2 className="grp-head"><span className="grp-title"><bdi>{group === 'hat' ? hats?.find(h => h.id === g.id)?.name ?? 'Unknown hat' : hostNames.get(g.id) ?? 'Unknown host'}</bdi></span><span className="grp-count">{[g.waiting && `${g.waiting} need you`, g.running && `${g.running} running`, `${g.count} total${list.hasMore ? ' shown' : ''}`].filter(Boolean).join(' · ')}</span></h2>
      {g.projects.map(project => <div className="project-group" key={project.id}>
        {project.path !== null && <h3 className="project-heading"><button type="button" className="project-toggle" disabled={bulk.busy} aria-expanded={!collapsedProjects.has(project.id)} onClick={() => toggleProject(project.id)}>
          <span className="project-chevron" aria-hidden="true"><Icon name={collapsedProjects.has(project.id) ? 'chevron-right' : 'chevron-down'} size={12} /></span><Icon name="folder" size={14} /><bdi className="project-name">{projectLabel(project.path)}</bdi><bdi className="grp-path" title={project.path}>{basename(project.path) ? project.path.slice(0, -basename(project.path).length) : project.path}</bdi>
          {project.waiting > 0 && <span className="project-waiting" aria-label={`${project.waiting} ${project.waiting === 1 ? 'session needs' : 'sessions need'} you`}>{project.waiting} {project.waiting === 1 ? 'needs' : 'need'} you</span>}
          <span className="project-summary">{project.running > 0 && `${project.running} running · `}{project.count}{list.hasMore ? ' shown' : ''} {project.count === 1 ? 'session' : 'sessions'}</span><span className="grp-host" title={project.hosts.map(id => hostNames.get(id) ?? 'Unknown host').join(', ')}><bdi>{project.hosts.map(id => hostNames.get(id) ?? 'Unknown host').join(', ')}</bdi></span>
        </button></h3>}
        {!collapsedProjects.has(project.id) && <table className={project.path !== null ? 'session-table project-rows' : 'session-table'}><colgroup>{COLUMNS.map(label => <col key={label} />)}</colgroup><thead className="table-head-only"><tr>{COLUMNS.map(label => <th key={label} scope="col">{label}</th>)}</tr></thead><tbody>{project.families.map(family => {
        const s = family.root, children = family.members.filter(child => child.family?.parent?.session_id === s.session_id)
        const knownClosed = [...list.all.values()].filter(child => child.family?.parent?.session_id === s.session_id && child.lifecycle === 'closed').length
        const hideClosed = scope.hideClosed && !list.searching && !scope.lifecycle
        const total = hideClosed && !list.hasMore ? children.length : Math.max(children.length, (s.family?.child_counts.total ?? 0) - (hideClosed ? knownClosed : 0))
        const running = children.filter(c => c.activity === 'running').length, attention = children.filter(needsOperator).length, done = children.filter(c => c.lifecycle === 'closed').length
        const idle = children.filter(c => c.lifecycle === 'active' && c.activity === 'idle' && !needsOperator(c)).length
        const states = [[attention, 'need you'], [children.filter(c => c.unseen).length, 'unseen'], [running, 'running'], [done, 'done'], [idle, 'idle'], [children.filter(c => c.lifecycle === 'failed').length, 'failed']] as const
        const summary = states.filter(([count]) => count).slice(0, 3).map(([count, label]) => `${count} ${label}`).join(' · ') + (children.length < total && states.some(([count]) => count) ? ' shown' : '')
        const preview = childPreview(children, sort)
        return <Fragment key={s.session_id}>
          <SessionRow session={s} selected={s.session_id === selectedId} selection={bulk.selectionFor(s)} hostName={hostNames.get(s.host_id)} now={now} table childrenCount={total} childrenStatus={summary} expanded={expanded.has(s.session_id)} onToggleChildren={toggleFamily} />
          {expanded.has(s.session_id) && <>{preview.map(child => <SessionRow key={child.session_id} session={child} selected={child.session_id === selectedId} selection={bulk.selectionFor(child)} hostName={hostNames.get(child.host_id)} now={now} table indent />)}
            <tr className="family-show-all"><td colSpan={5}><Link to={`/sessions/${encodeURIComponent(s.session_id)}?family=1`}>Show all {total} {total === 1 ? 'child' : 'children'}</Link></td></tr></>}
        </Fragment>
      })}</tbody></table>}
      </div>)}
    </div>)}</>)}
    {!list.loading && more && <button type="button" className="btn btn-ghost list-more" disabled={list.loadingMore || bulk.busy} onClick={() => { setShownRows({ query: list.queryKey, n: limit + PAGE_ROWS }); if (rootCount <= limit) void list.loadMore() }}>{list.loadingMore ? 'Loading…' : 'Load more'}</button>}
  </section>}</SessionBulkActions></SessionListActions>
}
