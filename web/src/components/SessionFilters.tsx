import { useEffect, useRef } from 'react'
import { useLocation } from '../router'
import { LIFECYCLES } from '../store/sessionList'
import { STATUS_FILTERS, STATUS_LABEL, matchesStatus } from '../lib/status'
import { SEARCH_MAX_CHARS, useSessionScope } from './SessionScope'
import SessionGroupingControls from './SessionGroupingControls'
export default function SessionFilters({ grouping = false }: { grouping?: boolean }) {
  const scope = useSessionScope(), searchRef = useRef<HTMLInputElement>(null)
  const { search } = useLocation()
  useEffect(() => { if (new URLSearchParams(search).has('search')) searchRef.current?.focus() }, [search])
  if (!scope) return null
  const { list, query, setQuery, hideClosed, setHideClosed, lifecycle, setLifecycle, host, setHost, status, setStatus, sort, setSort, hosts } = scope
  const sorts = [['attention', 'Needs you first'], ['activity', 'Last activity'], ['started', 'Started']] as const
  return <div className="overview-filters">
    {grouping && <SessionGroupingControls />}
    <div className="status-filters" role="group" aria-label="Status">{STATUS_FILTERS.map(value => <button type="button" key={value} aria-pressed={status === value} disabled={list.searching} className={'chip' + (status === value ? ' on' : '')} onClick={() => setStatus(value)}>{STATUS_LABEL[value]} <span>{value === 'attention' ? list.counts.waiting : list.shown.filter(s => matchesStatus(s, value)).length}{list.hasMore ? ' shown' : ''}</span></button>)}</div>
    <fieldset className="group-choice sort-choice"><legend>Sort</legend>{sorts.map(([value, label]) => <label key={value} title={value === 'started' ? 'Newest first; order stays stable while agents work' : undefined}><input type="radio" name="sort" value={value} checked={sort === value} onChange={() => setSort(value)} />{label}</label>)}</fieldset>
    <div className="filter-fields"><label>Search sessions<input ref={searchRef} type="search" aria-label="Search sessions" placeholder="Title, path, branch or session ID…" value={query} onChange={e => setQuery(e.target.value)} maxLength={SEARCH_MAX_CHARS} /></label>
    <label>Host<select aria-label="Host" value={host} onChange={e => setHost(e.target.value)} disabled={list.searching}><option value="">All hosts</option>{hosts?.filter(h => h.revoked_at == null).map(h => <option key={h.host_id} value={h.host_id}>{h.name}</option>)}</select></label>
    <button type="button" className={'toggle' + (hideClosed ? ' on' : '')} aria-pressed={hideClosed} disabled={list.searching} onClick={() => setHideClosed(!hideClosed)}>Hide closed</button>
    <label>Lifecycle<select aria-label="Lifecycle" value={lifecycle} disabled={list.searching} onChange={e => setLifecycle(e.target.value)}><option value="">Any lifecycle</option>{LIFECYCLES.map(value => <option key={value} value={value}>{value[0].toUpperCase() + value.slice(1)}</option>)}</select></label></div>
    {list.searching && <p className="list-note">Searching every session in this hat.</p>}
  </div>
}
