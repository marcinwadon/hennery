// The pieces items share: a speaker's row with its avatar, and the note a
// cut or elided item carries.
import type { ReactNode } from 'react'
import { avatarLetter } from '../../lib/agent'
import { clockTime } from '../../lib/time'
import { rawEventsHref } from './types'

interface SpeakerProps {
  /** Whose row: the operator's, or the agent's. */
  who: 'user' | 'agent'
  label: string
  ts?: string
  children: ReactNode
}

export function Speaker({ who, label, ts, children }: SpeakerProps) {
  const when = ts ? clockTime(ts) : ''
  return (
    <div className={'msg fade-in msg-' + who}>
      <div className={'avatar ' + (who === 'user' ? 'avatar-user' : 'avatar-ai')} aria-hidden="true">
        {avatarLetter(label)}
      </div>
      <div className="msg-body">
        <div className="msg-head">
          <span className="msg-who">{label}</span>
          {when && <span className="msg-when">{when}</span>}
        </div>
        {children}
      </div>
    </div>
  )
}

/** "Part of this was cut", with the raw events behind it. */
export function CutNote({ sessionId, children }: { sessionId: string; children: ReactNode }) {
  return (
    <p className="item-note">
      {children}{' '}
      <a href={rawEventsHref(sessionId)} target="_blank" rel="noopener noreferrer">
        Raw events
      </a>
    </p>
  )
}
