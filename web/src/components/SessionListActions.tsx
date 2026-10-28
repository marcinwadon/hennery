import { createContext, useCallback, useContext, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import type { DeleteResult, HostItem } from '../generated/protocol'
import type { SessionSummary } from '../generated/view'
import { forgetAttachments } from '../lib/attachments'
import { saveDraft } from '../lib/drafts'
import SessionMenu from './SessionMenu'
import RenameSession from './RenameSession'
import { deleteNotes } from './sessionWords'
import { useModalLayer } from './ui'

type Target = { id: string; anchor: HTMLButtonElement; summary: SessionSummary }
interface ListActions {
  id?: string
  open: boolean
  menuId: string
  show: (session: SessionSummary, anchor: HTMLButtonElement) => void
  setOpen: (open: boolean) => void
}
const Context = createContext<ListActions | null>(null)
export const useListSessionActions = () => useContext(Context)

/** One owner outside all rows: removing a row via SSE cannot tear down a
 * delete in flight or discard its eventual host-removal result. No row reads
 * a transcript, queue or host list merely by mounting. */
export default function SessionListActions({ sessions, hosts, onRemoved, children }: {
  sessions: ReadonlyMap<string, SessionSummary>
  hosts: readonly HostItem[] | null
  onRemoved: (id: string) => void
  children: ReactNode
}) {
  const [target, setTarget] = useState<Target | null>(null)
  const [open, setOpen] = useState(false)
  const [deleted, setDeleted] = useState<{ id: string; result?: DeleteResult } | null>(null)
  const region = useRef<HTMLDivElement>(null)
  const menuId = useId()
  const show = useCallback((summary: SessionSummary, anchor: HTMLButtonElement) => {
    setTarget({ id: summary.session_id, summary, anchor }); setOpen(true)
  }, [])
  useLayoutEffect(() => {
    if (open && target && (!target.anchor.isConnected || !sessions.has(target.id))) {
      setOpen(false)
      // Keep typing in a filter, but do not strand focus on the body after
      // an SSE update/collapsed group removed the focused disclosure.
      if (document.activeElement === document.body || document.activeElement?.closest('.session-row-menu')) returnFocus()?.focus()
    }
  })
  const returnFocus = () => {
    if (target?.anchor.isConnected) return target.anchor
    return region.current?.querySelector<HTMLElement>('h1[tabindex], [data-session-link]') ?? region.current
  }
  const actions = useMemo(() => ({ id: target?.id, open, menuId, show, setOpen }), [target?.id, open, menuId, show])
  return <Context.Provider value={actions}>
    <div className="session-list-actions" ref={region} tabIndex={-1}>
      {deleted && <div className="list-note session-delete-result" role="status">
        <p>This session was deleted. <bdi>{deleted.id}</bdi></p>
        {deleteNotes(deleted.result).map(note => <p key={note}>{note}</p>)}
        {!!deleted.result?.host_transcript?.notes?.length && <ul>{deleted.result.host_transcript.notes.map((note, i) => <li key={i}><bdi>{String(note)}</bdi></li>)}</ul>}
        <button type="button" className="btn btn-ghost btn-sm" onClick={() => { setDeleted(null); returnFocus()?.focus() }}>Dismiss deletion result</button>
      </div>}
      {children}
      {target && <ListActionSession key={target.id} target={target} summary={sessions.get(target.id)} hosts={hosts}
        menuId={menuId} open={open} setOpen={setOpen} returnFocus={returnFocus}
        onDeleted={result => {
          saveDraft(target.id, ''); forgetAttachments(target.id)
          setDeleted({ id: target.id, result }); onRemoved(target.id)
        }} />}
    </div>
  </Context.Provider>
}

function ListActionSession({ target, summary, hosts, menuId, open, setOpen, returnFocus, onDeleted }: {
  target: Target; summary?: SessionSummary; hosts: readonly HostItem[] | null
  menuId: string; open: boolean; setOpen: (open: boolean) => void
  returnFocus: () => HTMLElement | null; onDeleted: (result: DeleteResult | undefined) => void
}) {
  const last = useRef(target.summary)
  if (summary) last.current = summary
  const info = summary ?? last.current
  const host = hosts?.find(host => host.host_id === info.host_id)
  const [renaming, setRenaming] = useState(false)
  return <>
    <SessionMenu id={target.id} lifecycle={summary?.lifecycle} family={info.family}
      sidebarPinned={info.sidebar_pinned ?? false} onRename={summary ? () => setRenaming(true) : undefined}
      canPark={host?.capabilities?.includes('park') ?? false}
      supportsIdleHold={host?.capabilities?.includes('idle_hold')} hostConnected={host?.connected}
      row={{ anchor: target.anchor, menuId, open, setOpen, available: !!summary && target.anchor.isConnected }}
      loadQueueBeforeDelete onDeleted={onDeleted} focusAfterDelete={returnFocus} />
    {renaming && createPortal(<RowRename id={target.id} summary={info} close={() => setRenaming(false)} returnFocus={returnFocus} />, document.body)}
  </>
}

function RowRename({ id, summary, close, returnFocus }: {
  id: string; summary: SessionSummary; close: () => void; returnFocus: () => HTMLElement | null
}) {
  const dialog = useRef<HTMLDivElement>(null)
  const { trap, holdFocus } = useModalLayer(dialog)
  const titleId = useId()
  const fallback = useRef(returnFocus)
  fallback.current = returnFocus
  useEffect(() => () => { fallback.current()?.focus() }, [])
  return <div className="overlay" onMouseDown={holdFocus}>
    <div className="modal dialog session-row-rename" role="dialog" aria-modal="true" aria-labelledby={titleId} ref={dialog} tabIndex={-1} onKeyDown={event => trap(event, close)}>
      <div className="modal-head"><h2 id={titleId}>Rename session</h2></div>
      <div className="modal-body"><RenameSession id={id} title={summary.title ?? ''} canReset={summary.title_source === 'operator'} saved={close} cancel={close} /></div>
    </div>
  </div>
}
