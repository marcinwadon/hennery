import { useEffect, useRef, useState } from 'react'
import { ApiFailure } from '../api/errors'
import { useClient } from '../app-client'
import type { NewLikeThisResponse } from '../generated/protocol'
import { confirmStart, rememberConfigurationReview, sessionHref, startRefusal } from '../lib/start'
import { navigate } from '../router'
import { subscribeNavigation } from '../router-events'

// Like launcher drafts, sign-out clears this operation's browser lifetime.
const LOGIN = 'hennery.draft.__new_like_this_login__'

// A list owner may survive its source row; that must not revive navigation.
export function useNewLikeThis(id: string, sourceIsCurrent?: () => boolean) {
  const client = useClient()
  const live = useRef(false), pending = useRef(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => { live.current = true; return () => { live.current = false } }, [id])

  async function start() {
    if (pending.current) return
    pending.current = true
    setBusy(true); setError(null)
    let token: string | null = null
    try {
      token = sessionStorage.getItem(LOGIN) ?? crypto.randomUUID()
      sessionStorage.setItem(LOGIN, token)
    } catch { token = null /* The mounted view still has its navigation lifetime. */ }
    const path = location.pathname + location.search
    let navigated = false
    const unsubscribe = subscribeNavigation(() => { navigated = true })
    const current = () => {
      if (!live.current || sourceIsCurrent?.() === false || navigated || path !== location.pathname + location.search) return false
      if (token === null) return true
      try { return sessionStorage.getItem(LOGIN) === token } catch { return false }
    }
    const open = (href: string) => navigate(`${href}${href.includes('?') ? '&' : '?'}focus=prompt`)
    try {
      const created = await client.request<NewLikeThisResponse>('POST', `/api/sessions/${encodeURIComponent(id)}/new-like-this`)
      if (!current()) return
      let actual: import('../generated/protocol').SessionCatalog | null = null
      const confirmed = await confirmStart(client, created.session_id, created.hat_id, created.config, 30_000, true, catalog => { actual = catalog })
      if (!current()) return
      if (confirmed !== 'confirmed') {
        rememberConfigurationReview(created.session_id, null, created.config, actual)
        open(sessionHref(created.session_id, { kind: 'settings_unconfirmed' }))
      } else open(sessionHref(created.session_id))
    } catch (err) {
      if (!current()) return
      if (err instanceof ApiFailure && err.sessionId) {
        open(sessionHref(err.sessionId, err.code === 'delivery_unknown' ? { kind: 'start_unknown', kept: false } : { kind: 'settings_unconfirmed' }))
      } else {
        const words: Record<string, string> = {
          hat_mismatch: 'This project now belongs to a different hat. Review its path rules before creating a session like this one.',
          configuration_unavailable: 'This session has no reported settings to copy. Start a new session and choose its settings.',
          not_found: 'This session is no longer available. Refresh the list.',
        }
        setError(err instanceof ApiFailure && Object.hasOwn(words, err.code) ? words[err.code] : startRefusal(err))
      }
    } finally {
      unsubscribe()
      pending.current = false
      if (live.current) setBusy(false)
    }
  }
  return { start, busy, error }
}
