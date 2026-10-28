import type { Client } from '../api/client'
import { forgetAllAttachments } from '../lib/attachments'
import { forgetAllDrafts } from '../lib/drafts'
import { forgetAllSends } from '../lib/sending'
import { forgetAllConfigChanges } from '../lib/configuring'
import { navigate } from '../router'
import { BrowserPush } from './device'
import { pushJournal } from './journal'
import { bounded, setCleanupWarning } from './lifecycle'
import { clearDismissedAttention } from '../lib/dismissedAttention'
import { clearSessionHistory } from '../lib/sessionHistory'

// Called only after confirmed logout/current-login revocation, or its
// same-origin broadcast. Expiry's 401 deliberately does not call this.
export async function completeLogout(client: Client, broadcast = true): Promise<void> {
  forgetAllDrafts(); forgetAllAttachments(); forgetAllSends(); forgetAllConfigChanges()
  clearDismissedAttention()
  clearSessionHistory()
  navigate('/login') // Unmount all authenticated stores and question inputs.
  if (broadcast && typeof BroadcastChannel !== 'undefined') {
    const channel = new BroadcastChannel('hennery-auth')
    channel.postMessage({ version: 1, kind: 'logout' }); channel.close()
  }
  let cleanupSucceeded = false
  try {
    if ('serviceWorker' in navigator) {
      const registration = await bounded(navigator.serviceWorker.getRegistration('/'))
      if (registration) {
        const cleanup = new BrowserPush(client, registration, pushJournal(), '').disable(true)
        // bounded() reports uncertainty; it does not cancel cleanup. Correct
        // that report when the real operation subsequently succeeds.
        void cleanup.then((result) => { if (result.browser) { cleanupSucceeded = true; setCleanupWarning(null) } }, () => {})
        const result = await bounded(cleanup)
        if (!result.browser) throw new Error('Browser unsubscribe or notification cleanup failed.')
      } else if (typeof indexedDB !== 'undefined') {
        await bounded(pushJournal().disable())
      }
    }
    setCleanupWarning(null)
  } catch {
    if (!cleanupSucceeded) setCleanupWarning('Signed out. Local notification cleanup is unconfirmed; retry cleanup in Settings after signing in.')
  }
}
