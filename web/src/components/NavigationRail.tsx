import { useSessionScope, WaitingBadge } from './SessionScope'
import { Link } from '../router'
import { LABEL, PATH, type View } from '../lib/views'
import { AppearanceButton, HatChip, Icon, type IconName } from './ui'
import CommandEntry from './CommandEntry'
import { McpBadge } from './McpScope'
import SignOut from './SignOut'
import VersionInfo from './VersionInfo'
import { recentSessionTargets, sidebarSessions } from '../lib/sessionHistory'
import SessionName, { sessionName } from './SessionNames'
import { sessionPath } from './SessionRow'
import { recentStatus } from '../lib/recentStatus'
import { DEFAULT_ACCENT, HEX } from '../lib/accent'
import { isMac } from '../lib/commands'
import { useMediaQuery } from '../hooks/useMediaQuery'
export const NAV_ICON: Record<View, IconName> = { 'needs-you': 'hand-raised', sessions: 'rectangle-stack', new: 'plus', defaults: 'adjustments-horizontal', hosts: 'server-stack', mcp: 'puzzle-piece', skills: 'puzzle-piece', hats: 'rectangle-stack', settings: 'adjustments-horizontal', more: 'ellipsis-horizontal' }
export default function NavigationRail({ views, current, collapsed, toggle }: { views: readonly View[]; current: View | null; collapsed: boolean; toggle: () => void }) {
  const scope = useSessionScope()
  const narrowRail = useMediaQuery('(min-width: 768px) and (max-width: 1023px)')
  const compactFooter = collapsed || narrowRail
  const pinned = [...(scope?.list.all.values() ?? [])].filter(s => s.sidebar_pinned).sort((a, b) => a.created_at.localeCompare(b.created_at) || a.session_id.localeCompare(b.session_id))
  const hosts = scope?.hosts?.filter(h => h.revoked_at == null)
  const navigation = (view: View) => <Link key={view} to={PATH[view]} aria-label={LABEL[view]} title={LABEL[view]} className={'nav-item' + (current === view && !scope?.selectedId ? ' on' : '')} aria-current={current === view && !scope?.selectedId ? 'page' : undefined}><Icon name={NAV_ICON[view]} size={18} /><span className="nav-label">{view === 'sessions' ? 'All sessions' : LABEL[view]}</span>{view === 'needs-you' && <WaitingBadge />}{view === 'mcp' && <McpBadge />}</Link>
  const hatLinks = scope ? [{ id: '', name: 'All hats', colour: undefined }, ...(scope.hats ?? [])] : []
  const recent = sidebarSessions(scope?.recentSessions ?? [])
  const targets = recentSessionTargets(recent)
  return <aside className={'rail' + (collapsed ? ' rail-collapsed' : '')} aria-label="Views">
    <div className="rail-head"><span className="brand-name" title="hennery">hennery</span><button type="button" className="rail-collapse" aria-label={collapsed ? 'Expand navigation' : 'Collapse navigation'} onClick={toggle}><Icon name={collapsed ? 'chevron-double-right' : 'chevron-double-left'} size={18} /></button></div>
    <CommandEntry compact={collapsed} />
    <nav className="rail-nav" aria-label="Main navigation">
      <div>{views.filter(view => ['needs-you', 'sessions', 'new'].includes(view)).map(navigation)}</div>
      {scope && <section className="rail-recent" aria-labelledby="rail-recent-heading">
        <div className="recent-heading nav-label"><h2 id="rail-recent-heading" aria-label="Recent sessions">Recent</h2><span title="Jump back to the previous session"><kbd>`</kbd> back</span></div>
        {recent.length === 0 && <p className="recent-empty nav-label">Sessions appear as you visit them.</p>}
        {recent.map(row => {
          const status = recentStatus(row), name = sessionName(row), hat = scope.hats?.find(hat => hat.id === row.hat_id)
          const colour = hat?.colour && HEX.test(hat.colour) ? hat.colour : DEFAULT_ACCENT
          const current = scope.selectedId === row.session_id, index = targets.findIndex(target => target.session_id === row.session_id)
          const hint = index >= 0 ? `${isMac() ? '⌥' : 'Alt+'}${index + 1}` : ''
          const tip = [name.text, hat?.name ?? 'Unknown hat', status.label, row.unseen ? 'Unseen' : '', hint].filter(Boolean).join(' · ')
          return <Link key={row.session_id} to={sessionPath(row.session_id)} className={'recent-session' + (current ? ' on' : '') + (row.unseen ? ' session-unseen' : '')} aria-current={current ? 'page' : undefined} aria-label={tip} title={tip}>
            <span className="recent-status" role="img" aria-label={status.label} style={{ color: `var(--st-${status.colour}-fg)` }}><Icon name={status.icon} size={18} /><span className="recent-hat-badge" title={hat?.name ?? 'Unknown hat'} style={{ backgroundColor: colour }} /></span>
            <span className="recent-hat nav-label" title={hat?.name ?? 'Unknown hat'} style={{ backgroundColor: colour }} />
            {row.unseen && <span className="unseen-dot" role="img" aria-label="Unseen" />}
            <bdi className={'recent-title nav-label' + (name.provisional ? ' session-name-provisional' : '')}>{name.text}</bdi>
            {hint && <kbd className="nav-label">{hint}</kbd>}
          </Link>
        })}
      </section>}
      {pinned.length > 0 && <div className="rail-pins" role="group" aria-label="Pinned sessions"><div className="nav-label nav-section-label">Pinned</div>{pinned.map(session => <Link key={session.session_id} to={sessionPath(session.session_id)} className={'nav-item rail-pin' + (scope?.selectedId === session.session_id ? ' on' : '')} aria-label={sessionName(session).text} title={sessionName(session).text} aria-current={scope?.selectedId === session.session_id ? 'page' : undefined}><Icon name="bookmark" size={16} /><span className="nav-label rail-pin-name"><SessionName session={session} /></span></Link>)}</div>}
      {scope && <div className="rail-hats"><div className="nav-label nav-section-label">Hats</div>{hatLinks.map(hat => {
        const loaded = scope.list.shown.filter(s => !hat.id || s.hat_id === hat.id)
        const waiting = scope.attentionRows.filter(s => (!hat.id || s.hat_id === hat.id) && !scope.isAttentionDismissed(s.session_id)).length
        const count = `${loaded.length} ${loaded.length === 1 ? 'session' : 'sessions'}${scope.list.hasMore || scope.list.loading ? ' shown' : ''}`
        return <Link key={hat.id} to={`/sessions?hat=${encodeURIComponent(hat.id)}`} className={'rail-hat' + (scope.hat === hat.id ? ' on' : '')} aria-label={hat.name} title={`${hat.name} · ${count} · ${waiting} need you`}><HatChip colour={hat.colour}><span className="nav-label"><bdi>{hat.name}</bdi></span></HatChip><span className="nav-label hat-loaded">{count}</span>{waiting > 0 && <span className="hat-attention" aria-label={`${waiting} shown sessions need you`}>●</span>}</Link>
      })}</div>}
      <div><div className="nav-label nav-section-label">Configure</div>{views.filter(view => !['more', 'needs-you', 'sessions', 'new'].includes(view)).map(navigation)}</div>
    </nav>
    <div className="rail-foot"><VersionInfo /><span className="owner-mark">O<span className="nav-label">Owner</span></span>{scope && <span className="host-fraction" title="Online / owned non-revoked hosts">{hosts ? `${hosts.filter(h => h.connected).length}/${hosts.length} hosts online` : 'Hosts unknown'}</span>}<AppearanceButton compact={compactFooter} />{current !== 'settings' && current !== 'more' && <SignOut compact={compactFooter} />}</div>
  </aside>
}
