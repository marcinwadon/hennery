// The app's one transport client, shared through context, and how it is
// wired to the browser: real `fetch`, in-app navigation, and the step-up
// dialog.
import { createContext, useContext } from 'react'
import { Client } from './api/client'
import { navigate } from './router'

export const ClientContext = createContext<Client | null>(null)

export function useClient(): Client {
  const client = useContext(ClientContext)
  if (!client) throw new Error('useClient outside <ClientContext.Provider>')
  return client
}

/** A client over `fetchImpl` (the browser's, unless a test gives one) that
 *  opens the step-up dialog through `stepUp`. */
export function browserClient(stepUp: () => Promise<void>, fetchImpl?: typeof fetch): Client {
  return new Client({
    fetch: fetchImpl ?? ((input, init) => window.fetch(input, init)),
    navigate: (to) => navigate(to),
    here: () => ({ pathname: location.pathname, search: location.search }),
    stepUp,
  })
}
