import { useEffect, useRef, useState } from 'react'
import { useClient } from '../app-client'
import type { ConfigValue, SessionCatalog } from '../generated/protocol'
import type { ConfigOption } from '../lib/catalog'
import { changeConfig, configuringFor, type ConfigChange } from '../lib/configuring'

/** Owned by the keyed session screen, above its unmountable Details section. */
export function useSessionConfig(id: string, onCatalog: (catalog: SessionCatalog) => void) {
  const client = useClient()
  const [initial] = useState(() => configuringFor(id))
  const [change, setChange] = useState(initial)
  const [error, setError] = useState<string | null>(null)
  const alive = useRef(false)

  useEffect(() => {
    alive.current = true
    if (initial) follow(initial)
    return () => { alive.current = false }
    // This hook belongs to one keyed session; follow the operation it found.
  }, [])

  function follow(next: ConfigChange) {
    void next.result.then(result => {
      if (!alive.current || !next.valid) return
      if (result.catalog) onCatalog(result.catalog)
      setError(result.error)
      setChange(undefined)
    })
  }

  function pick(option: ConfigOption, value: ConfigValue) {
    if (configuringFor(id)) return
    const next = changeConfig(client, id, option, value)
    setError(null)
    setChange(next)
    follow(next)
  }

  return { pending: !!change, values: change?.values ?? {}, error, pick, isPending: () => !!configuringFor(id) }
}

export type SessionConfigState = ReturnType<typeof useSessionConfig>
