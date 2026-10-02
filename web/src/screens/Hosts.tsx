// Hosts (frontend spec §8, kernel spec §4): every paired host, revoked ones
// included, with its online state and versions; pairing a new one; and
// renaming, re-hatting or revoking one, each confirmed, then stepped up.
import { useEffect, useRef, useState } from 'react'
import { messageOf } from '../api/errors'
import { hats as listHats, hosts as listHosts, mintPairingCode, revokeHost, settings, updateHost } from '../api/manage'
import { useClient } from '../app-client'
import ConfirmDialog from '../components/ConfirmDialog'
import Pairing, { type Minted } from '../components/Pairing'
import type { HatItem, HostItem } from '../generated/protocol'
import { useResource } from '../hooks/useResource'
import { HOST_STATE_LABEL, hostState } from '../lib/manage'
import { Text, visible } from '../lib/text'
import When from '../components/When'
import '../manage.css'

/** Each carries where focus goes when the confirmation closes and what
 *  opened it is gone. */
type Pending =
  | { kind: 'rename'; host: HostItem; name: string; back: () => HTMLElement | null }
  | { kind: 'default_hat'; host: HostItem; hat: HatItem; back: () => HTMLElement | null }
  | { kind: 'revoke'; host: HostItem; back: () => HTMLElement | null }

/** The pairing panel: a new one per code (`n`), and the code itself until
 *  it is spent. */
interface Panel {
  n: number
  minted: Minted | null
}

export default function Hosts() {
  const client = useClient()
  const list = useResource(() => listHosts(client))
  const hatList = useResource(() => listHats(client))
  const [panel, setPanel] = useState<Panel | null>(null)
  const minted = panel?.minted ?? null
  const [adding, setAdding] = useState(false)
  const [addError, setAddError] = useState<string | null>(null)
  const [pending, setPending] = useState<Pending | null>(null)

  // The URL and the hosts already paired are read first: a failed read then
  // spends no code, and a host that pairs is told from the ones before it
  // even when the list above never loaded.
  const add = async () => {
    setAdding(true)
    setAddError(null)
    try {
      const s = await settings(client)
      const known = (await listHosts(client)).map((h) => h.host_id)
      const code = await mintPairingCode(client)
      setPanel((p) => ({ n: (p?.n ?? 0) + 1, minted: { code: code.code, publicUrl: s.public_url, known } }))
    } catch (err) {
      setAddError(messageOf(err))
    } finally {
      setAdding(false)
    }
  }

  // A page restored from the back-forward cache must not show a code: it
  // is dropped as the page is hidden. Switching tabs to paste it keeps it.
  useEffect(() => {
    const drop = () => setPanel(null)
    window.addEventListener('pagehide', drop)
    return () => window.removeEventListener('pagehide', drop)
  }, [])

  const replace = (host: HostItem) => list.set((prev) => (prev ?? []).map((h) => (h.host_id === host.host_id ? host : h)))

  const hatName = (id: string) => hatList.data?.find((h) => h.id === id)?.name

  return (
    <div className="manage">
      <header className="manage-head">
        <h1>Hosts</h1>
        <button type="button" className="btn btn-primary btn-sm" onClick={add} disabled={adding || minted !== null}>
          Add host
        </button>
      </header>
      {addError && (
        <p className="form-error" role="alert">
          <Text>{addError}</Text>
        </p>
      )}
      {panel && (
        <Pairing
          key={panel.n}
          minted={panel.minted}
          onPaired={list.reload}
          onSpent={() => setPanel((p) => p && { ...p, minted: null })}
          onClose={() => setPanel(null)}
        />
      )}
      {list.error && (
        <p className="form-error" role="alert">
          <Text>{list.error}</Text>
        </p>
      )}
      {hatList.error && (
        <p className="form-error" role="alert">
          <Text>{hatList.error}</Text>
        </p>
      )}
      {list.data && list.data.length === 0 && <p className="empty">No host is paired yet.</p>}
      <ul className="cards" aria-label="Hosts">
        {(list.data ?? []).map((host) => (
          <HostCard
            key={host.host_id}
            host={host}
            hats={hatList.data ?? []}
            hatName={hatName(host.default_hat_id)}
            onRename={(name, back) => setPending({ kind: 'rename', host, name, back })}
            onDefaultHat={(hat, back) => setPending({ kind: 'default_hat', host, hat, back })}
            onRevoke={(back) => setPending({ kind: 'revoke', host, back })}
          />
        ))}
      </ul>
      {pending?.kind === 'rename' && (
        <ConfirmDialog
          title="Rename this host?"
          confirm="Rename"
          action={async () => replace(await updateHost(client, pending.host.host_id, { name: pending.name }))}
          onClose={() => setPending(null)}
          returnFocus={pending.back}
        >
          <p>
            <Text>{pending.host.name}</Text> becomes <Text>{pending.name}</Text>.
          </p>
        </ConfirmDialog>
      )}
      {pending?.kind === 'default_hat' && (
        <ConfirmDialog
          title="Change this host’s default hat?"
          confirm="Change"
          action={async () => replace(await updateHost(client, pending.host.host_id, { default_hat_id: pending.hat.id }))}
          onClose={() => setPending(null)}
          returnFocus={pending.back}
        >
          <p>
            Sessions on <Text>{pending.host.name}</Text> that no path rule covers will belong to{' '}
            <Text>{pending.hat.name}</Text>.
          </p>
          <p>
            Parked or closed sessions there that no rule covers keep their hat: resuming one is refused until it is
            re-assigned.
          </p>
        </ConfirmDialog>
      )}
      {pending?.kind === 'revoke' && (
        <ConfirmDialog
          title="Revoke this host?"
          confirm="Revoke"
          danger
          action={async () => replace(await revokeHost(client, pending.host.host_id))}
          onClose={() => setPending(null)}
          returnFocus={pending.back}
        >
          <p>
            <Text>{pending.host.name}</Text> can no longer connect, and its sessions are parked. Pairing it again
            needs a new code.
          </p>
          <p>Agents it is running stop only when it next connects: it is then told it is revoked.</p>
        </ConfirmDialog>
      )}
    </div>
  )
}

/** Where focus goes when a confirmation closes and what opened it is
 *  gone. */
type Back = () => HTMLElement | null

interface CardProps {
  host: HostItem
  hats: HatItem[]
  hatName?: string
  onRename: (name: string, back: Back) => void
  onDefaultHat: (hat: HatItem, back: Back) => void
  onRevoke: (back: Back) => void
}

function HostCard({ host, hats, hatName, onRename, onDefaultHat, onRevoke }: CardProps) {
  const state = hostState(host)
  const [renaming, setRenaming] = useState(false)
  const [name, setName] = useState(host.name)
  const titleId = `host-${host.host_id}`
  // The rename form hides as it closes, taking focus with it: focus goes
  // back to "Rename". A revoke takes every button away: focus goes to the
  // card's title.
  const title = useRef<HTMLHeadingElement>(null)
  const rename = useRef<HTMLButtonElement>(null)
  const refocus = useRef(false)
  const toRename = () => rename.current
  const toTitle = () => title.current

  useEffect(() => {
    if (!renaming && refocus.current) {
      refocus.current = false
      rename.current?.focus()
    }
  }, [renaming])

  const closeForm = () => {
    refocus.current = true
    setRenaming(false)
  }

  return (
    <li className={`card host host-${state}`} aria-labelledby={titleId}>
      <div className="card-head">
        <h2 className="card-title" id={titleId} ref={title} tabIndex={-1}>
          <Text>{host.name}</Text>
        </h2>
        <span className={`state state-${state}`}>{HOST_STATE_LABEL[state]}</span>
      </div>
      {/* One row per fact; the agents, the last doctor result and the
          host's notices join these when the host reports them. */}
      <dl className="facts">
        <dt>Platform</dt>
        <dd>
          <Text>{host.platform}</Text>
        </dd>
        <dt>hennery</dt>
        <dd>
          <Text>{host.host_version}</Text>
        </dd>
        <dt>Default hat</dt>
        <dd>{hatName !== undefined ? <Text>{hatName}</Text> : <Text className="mono">{host.default_hat_id}</Text>}</dd>
        <dt>Paired</dt>
        <dd>
          <When at={host.created_at} />
        </dd>
        <dt>Last connected</dt>
        <dd>{host.last_seen_at ? <When at={host.last_seen_at} /> : 'never'}</dd>
        {host.revoked_at && (
          <>
            <dt>Revoked on</dt>
            <dd>
              <When at={host.revoked_at} />
            </dd>
          </>
        )}
      </dl>
      {state !== 'revoked' && (
        <div className="card-actions">
          {renaming ? (
            <form
              className="inline-form"
              onSubmit={(e) => {
                e.preventDefault()
                const trimmed = name.trim()
                if (trimmed === '' || trimmed === host.name) {
                  closeForm()
                  return
                }
                // The confirmation returns focus to "Rename".
                setRenaming(false)
                onRename(trimmed, toRename)
              }}
            >
              <label className="field">
                <span className="field-label">New name</span>
                <input
                  className="text-input"
                  value={name}
                  maxLength={64}
                  onChange={(e) => setName(e.target.value)}
                  autoFocus
                />
              </label>
              <button type="submit" className="btn btn-primary btn-sm">
                Save
              </button>
              <button type="button" className="btn btn-ghost btn-sm" onClick={closeForm}>
                Cancel
              </button>
            </form>
          ) : (
            <>
              <button
                type="button"
                className="btn btn-ghost btn-sm"
                ref={rename}
                onClick={() => {
                  setName(host.name)
                  setRenaming(true)
                }}
              >
                Rename
              </button>
              <label className="select-field">
                <span className="field-label">Default hat</span>
                <select
                  value={host.default_hat_id}
                  onChange={(e) => {
                    const hat = hats.find((h) => h.id === e.target.value)
                    if (hat) onDefaultHat(hat, toTitle)
                  }}
                >
                  {hats
                    .filter((h) => !h.purging || h.id === host.default_hat_id)
                    .map((h) => (
                      <option key={h.id} value={h.id}>
                        {visible(h.name)}
                      </option>
                    ))}
                </select>
              </label>
              <span className="spacer" />
              <button type="button" className="btn btn-danger btn-sm" onClick={() => onRevoke(toTitle)}>
                Revoke
              </button>
            </>
          )}
        </div>
      )}
    </li>
  )
}
