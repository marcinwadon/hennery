// "Add host" (frontend spec §8, kernel spec §4.1): a one-time pairing code,
// minted behind a step-up, and the exact command that pairs a machine with
// it, with a countdown to its expiry.
//
// The code is shown once. It lives in the Hosts screen's state and this
// panel only: never in the address bar, the title, storage or the console.
// It is dropped from the page when it expires, when a new host pairs, and
// when the panel closes or unmounts.
import { useEffect, useRef, useState } from 'react'
import { hosts as listHosts } from '../api/manage'
import { useClient } from '../app-client'
import type { HostItem } from '../generated/protocol'
import { Text } from '../lib/text'

/** A code is dead 600 s after minting (kernel spec §4.1): the countdown
 *  never shows more, whatever the clocks say. */
export const CODE_LIFETIME_S = 600
/** How often the host list is read while a code waits for its host. */
export const PAIRED_POLL_MS = 3000

/** What "Add host" got: the minted code, where hosts reach the collector,
 *  and the hosts paired before it, read just before the mint. */
export interface Minted {
  code: string
  publicUrl: string
  known: string[]
}

type Stage = { kind: 'live' } | { kind: 'expired' } | { kind: 'paired'; host: HostItem }

interface Props {
  minted: Minted
  onPaired: () => void
  onClose: () => void
}

export default function Pairing({ minted, onPaired, onClose }: Props) {
  const client = useClient()
  const [stage, setStage] = useState<Stage>({ kind: 'live' })
  // Counted from the answer's arrival, on the monotonic clock: the server's
  // `expires_at` is minted on its clock, and this browser's may be off by
  // more than the lifetime. The code dies within the network's delay of
  // this.
  const [deadline] = useState(() => performance.now() + CODE_LIFETIME_S * 1000)
  const [left, setLeft] = useState(CODE_LIFETIME_S)
  const before = useRef(new Set(minted.known))
  const command = `hennery host join ${minted.publicUrl} ${minted.code}`

  // The countdown, and the code's end at zero.
  const counting = stage.kind === 'live'
  useEffect(() => {
    if (!counting) return
    const tick = () => {
      const s = Math.max(0, Math.ceil((deadline - performance.now()) / 1000))
      setLeft(s)
      if (s === 0) setStage({ kind: 'expired' })
    }
    tick()
    const timer = setInterval(tick, 1000)
    return () => clearInterval(timer)
  }, [counting, deadline])

  // A host that pairs while the code is live ends it here: the code is
  // spent.
  const waiting = stage.kind === 'live'
  useEffect(() => {
    if (!waiting) return
    let live = true
    const timer = setInterval(() => {
      listHosts(client).then(
        (list) => {
          const fresh = list.find((h) => !before.current.has(h.host_id))
          if (live && fresh) {
            setStage({ kind: 'paired', host: fresh })
            onPaired()
          }
        },
        () => {
          // A failed read is tried again at the next tick.
        },
      )
    }, PAIRED_POLL_MS)
    return () => {
      live = false
      clearInterval(timer)
    }
  }, [client, waiting, onPaired])

  return (
    <section className="card pairing" aria-labelledby="pairing-title">
      <h2 id="pairing-title" className="card-title">
        Add a host
      </h2>
      {stage.kind === 'live' && (
        <>
          <p>On the machine to pair, run:</p>
          <pre className="command" aria-label="Pairing command">
            <code>{command}</code>
          </pre>
          <p className="pairing-code">
            Code <code>{minted.code}</code>, valid for{' '}
            <span role="timer" aria-live="off">
              {formatLeft(left)}
            </span>
            , and once only.
          </p>
          <p className="hint">
            The host keeps its pairing in <code>--data-dir</code> (or <code>HENNERY_HOST_DATA_DIR</code>). Leave the
            code out and <code>host join</code> asks for it instead, which keeps it out of your shell history.
          </p>
          <p role="status" className="hint">
            Waiting for the host… Closing this hides the code; it stays valid until it expires.
          </p>
        </>
      )}
      {stage.kind === 'expired' && <p role="status">The code has expired. Add a host again for a new one.</p>}
      {stage.kind === 'paired' && (
        <p role="status">
          Paired: <Text>{stage.host.name}</Text>
        </p>
      )}
      <div className="card-actions">
        <button type="button" className="btn btn-ghost btn-sm" onClick={onClose}>
          Close
        </button>
      </div>
    </section>
  )
}

/** `m:ss`. */
export function formatLeft(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds))
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`
}
