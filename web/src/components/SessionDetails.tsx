import { useEffect, useId, useRef, type ReactNode, type RefObject, type KeyboardEvent } from 'react'
import type { HeaderInfo } from './SessionHeader'
import type { Item } from '../generated/view'
import type { McpSessionDelivery, SessionCatalog } from '../generated/protocol'
import type { SessionConfigState } from '../hooks/useSessionConfig'
import SessionSettings from './SessionSettings'
import { agentLabel } from '../lib/agent'
import { useMediaQuery } from '../hooks/useMediaQuery'
import { useModalLayer } from './ui'
import McpDelivery from './McpDelivery'
import StepList from './StepList'
import { WaitingBadge } from './SessionScope'

const TABS = ['Family', 'Plan', 'Details'] as const
export type SessionDetailsTab = typeof TABS[number]

interface Props {
  open: boolean
  tab: SessionDetailsTab
  onTab: (tab: SessionDetailsTab) => void
  onClose: () => void
  trigger: RefObject<HTMLButtonElement | null>
  info?: HeaderInfo
  hostName?: string
  hatName?: string
  plan?: Extract<Item, { kind: 'plan' }>
  delivery?: McpSessionDelivery
  configuration?: SessionConfigState
  catalog?: SessionCatalog | null
  settingsFocus?: number
  children: ReactNode
}

/** Keep the family reader mounted while tabs are hidden: one bounded reader
 * supplies both canonical routing and the panel, with no transcript stream. */
export default function SessionDetails(props: Props) {
  const { open, onClose, trigger, tab, onTab: setTab } = props
  const sheet = useMediaQuery('(max-width: 1023px)')
  const prefix = useId()
  const wrapper = useRef<HTMLDivElement>(null)
  // Runs after SheetFocus: a settings shortcut targets the settings themselves,
  // while the ordinary Details/Family button keeps the selected-tab focus.
  useEffect(() => {
    if (!open || !props.settingsFocus) return
    const section = wrapper.current?.querySelector<HTMLElement>('.session-settings')
    const target = section?.querySelector<HTMLElement>('select:not([disabled]), input:not([disabled])') ?? section
    target?.focus()
    target?.scrollIntoView?.({ block: 'nearest' })
  }, [open, props.settingsFocus, sheet])
  const body = <>
    <div className="session-details-heading"><h2>Session details</h2>{sheet && <WaitingBadge />}<button type="button" className="btn btn-ghost btn-sm" onClick={onClose} aria-label="Close session details">×</button></div>
    {sheet && <div className="session-detail-tabs" role="tablist" aria-label="Session information">
      {TABS.map((name, i) => <button key={name} type="button" role="tab" id={`${prefix}-${name}`} aria-controls={`${prefix}-${name}-panel`} aria-selected={tab === name} tabIndex={tab === name ? 0 : -1}
        onClick={() => setTab(name)} onKeyDown={e => {
          const index = e.key === 'ArrowRight' ? (i + 1) % 3 : e.key === 'ArrowLeft' ? (i + 2) % 3 : e.key === 'Home' ? 0 : e.key === 'End' ? 2 : -1
          if (index < 0) return
          e.preventDefault(); setTab(TABS[index]); document.getElementById(`${prefix}-${TABS[index]}`)?.focus()
        }}>{name}</button>)}
    </div>}
    {TABS.map(name => <div key={name} role={sheet ? 'tabpanel' : 'region'} tabIndex={sheet ? 0 : undefined} id={`${prefix}-${name}-panel`} aria-label={sheet ? undefined : name} aria-labelledby={sheet ? `${prefix}-${name}` : undefined} hidden={sheet && tab !== name}>
      {!sheet && <h3 className="session-section-title">{name}</h3>}
      {name === 'Family' ? props.children : !open || (sheet && tab !== name) ? null : name === 'Plan' ? props.plan?.entries.length ? <StepList entries={props.plan.entries} truncated={props.plan.truncated} open /> : <p className="item-note">No plan has been shared.</p> : <Details {...props} />}
    </div>)}
  </>
  return <div ref={wrapper} className={'session-details-wrap' + (sheet ? ' is-sheet' : '')} hidden={!open}>
    {sheet ? <Sheet open={open} onClose={onClose} trigger={trigger}>{body}</Sheet> : <aside className="session-details" aria-label="Session details" onKeyDown={event => {
      if (event.key === 'Escape' && !event.defaultPrevented) { event.preventDefault(); event.stopPropagation(); onClose() }
    }}>{body}</aside>}
  </div>
}

function Sheet({ open, onClose, trigger, children }: { open: boolean; onClose: () => void; trigger: RefObject<HTMLButtonElement | null>; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null)
  const keys = useRef<((e: KeyboardEvent<HTMLDivElement>) => void) | null>(null)
  return <div className="session-details-scrim" onMouseDown={e => { if (e.target === e.currentTarget) onClose() }}>
    <div ref={ref} className="session-details" role="dialog" aria-modal="true" aria-label="Session details" tabIndex={-1} onKeyDown={e => keys.current?.(e)}>{open && <SheetFocus dialog={ref} keys={keys} onClose={onClose} trigger={trigger} />}{children}</div>
  </div>
}

function SheetFocus({ dialog: ref, keys, onClose, trigger }: { dialog: RefObject<HTMLDivElement | null>; keys: RefObject<((e: KeyboardEvent<HTMLDivElement>) => void) | null>; onClose: () => void; trigger: RefObject<HTMLButtonElement | null> }) {
  const { trap } = useModalLayer(ref)
  keys.current = e => trap(e, onClose)
  useEffect(() => {
    ref.current?.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')?.focus()
    return () => { keys.current = null; if (trigger.current?.isConnected) trigger.current.focus() }
  }, [trigger])
  return null
}

function Details({ info, hostName, hatName, delivery, configuration, catalog }: Props) {
  return <>
    {configuration && <SessionSettings catalog={catalog ?? null} config={configuration} />}
    {info && <dl className="session-facts">
      <dt>Session ID</dt><dd><bdi><code>{info.session_id}</code></bdi></dd>
      <dt>Host</dt><dd><bdi>{hostName ?? info.host_id}</bdi></dd>
      <dt>Project</dt><dd><bdi>{info.cwd}</bdi></dd>
      {hatName && <><dt>Hat</dt><dd><bdi>{hatName}</bdi></dd></>}
      <dt>Agent</dt><dd>{agentLabel(info.agent)}</dd>
      {!configuration && info.model && <><dt>Model</dt><dd><bdi>{info.model}</bdi></dd></>}
      {!configuration && info.mode && <><dt>Mode</dt><dd><bdi>{info.mode}</bdi></dd></>}
      <dt>Started</dt><dd><time dateTime={info.created_at}>{new Date(info.created_at).toLocaleString()}</time></dd>
    </dl>}
    <McpDelivery agent={info?.agent} delivery={delivery} />
  </>
}
