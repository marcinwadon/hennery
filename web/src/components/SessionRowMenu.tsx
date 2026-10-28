import { useRef } from 'react'
import type { SessionSummary } from '../generated/view'
import { Icon } from './ui'
import { useListSessionActions } from './SessionListActions'

/** Only a trigger. Dialogs, requests and results belong to the list owner. */
export default function SessionRowMenu({ session }: { session: SessionSummary }) {
  const actions = useListSessionActions()
  const trigger = useRef<HTMLButtonElement>(null)
  const open = actions?.id === session.session_id && actions.open
  return <div className="session-row-actions">
    <button ref={trigger} type="button" className="session-menu-btn" aria-label="Session actions" aria-expanded={!!open} aria-controls={open ? actions?.menuId : undefined}
      onClick={() => {
        if (open) actions?.setOpen(false)
        else if (trigger.current) actions?.show(session, trigger.current)
      }}><Icon name="ellipsis-horizontal" size={18} /></button>
  </div>
}
