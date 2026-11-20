// Hook sets: typed `hooks` JSON the collector keeps and the hosts it is
// assigned to. Create, replace, delete and a host change need a fresh step-up,
// which the client asks for when refused.
import { useRef, useState } from 'react'
import { createHookSet, deleteHookSet, hookEvents, hookSet as readHookSet, listHookSets, replaceHookSet } from '../api/hookSets'
import { hats as listHats, hosts as listHosts } from '../api/manage'
import { useClient } from '../app-client'
import type { HookSetConfig, HookSetItem } from '../generated/protocol'
import { useResource } from '../hooks/useResource'
import { Text } from '../lib/text'
import ConfirmDialog from './ConfirmDialog'
import HookSetEditor from './HookSetEditor'
import HookSetHosts from './HookSetHosts'

export default function HookSetsPanel() {
  const client = useClient()
  const list = useResource(() => listHookSets(client))
  const events = useResource(() => hookEvents(client))
  const hats = useResource(() => listHats(client))
  const hosts = useResource(() => listHosts(client))
  const [selected, setSelected] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)
  const detail = useResource(() => (selected ? readHookSet(client, selected) : Promise.resolve(null)), [selected])
  const [busy, setBusy] = useState(false)
  const [deleting, setDeleting] = useState<HookSetItem | null>(null)
  // A ref, not state: two presses in one frame both see the first.
  const working = useRef(false)

  const guarded = async (run: () => Promise<void>) => {
    if (working.current) return
    working.current = true
    setBusy(true)
    try {
      await run()
    } finally {
      working.current = false
      setBusy(false)
    }
  }

  const create = (name: string, hooks: HookSetConfig) =>
    guarded(async () => {
      const item = await createHookSet(client, name, hooks)
      list.set((prev) => [...(prev ?? []).filter((s) => s.id !== item.id), item])
      setCreating(false)
      setSelected(item.id)
    })

  const replace = (item: HookSetItem, hooks: HookSetConfig) =>
    guarded(async () => {
      const next = await replaceHookSet(client, item.id, hooks)
      // Read the saved set back before the list changes, so the editor shows what the server holds.
      detail.set(await readHookSet(client, item.id))
      list.set((prev) => (prev ?? []).map((s) => (s.id === next.id ? next : s)))
    })

  // The list holds the host count: read it again after a host change.
  const hostsChanged = () => {
    void listHookSets(client).then(list.set, () => {})
  }

  const current = list.data?.find((s) => s.id === selected) ?? null

  return (
    <>
      <div className="card-actions">
        <p className="manage-subtitle">Hooks your hosts run in Claude Code and Codex sessions.</p>
        <span className="spacer" />
        <button
          type="button"
          className="btn btn-primary btn-sm"
          onClick={() => {
            setCreating(true)
            setSelected(null)
          }}
        >
          New hook set
        </button>
      </div>
      {[list.error, events.error, hats.error, hosts.error].map(
        (shown, i) =>
          shown && (
            <p key={i} className="form-error" role="alert">
              <Text>{shown}</Text>
            </p>
          ),
      )}
      {list.data && list.data.length === 0 && <p className="empty">No hook set yet.</p>}
      <ul className="cards" aria-label="Hook sets" aria-busy={busy}>
        {(list.data ?? []).map((s) => (
          <li key={s.id} className="card" aria-labelledby={`hook-set-${s.id}`}>
            <h2 className="card-title" id={`hook-set-${s.id}`}>
              <button
                type="button"
                className="btn btn-ghost btn-sm"
                aria-pressed={selected === s.id}
                onClick={() => {
                  setCreating(false)
                  setSelected(selected === s.id ? null : s.id)
                }}
              >
                <Text>{s.name}</Text>
              </button>
            </h2>
            <p className="hint">
              Events: {s.events.length} · Hosts: {s.host_ids.length}
            </p>
          </li>
        ))}
      </ul>
      {creating && (
        <section className="card" aria-label="New hook set">
          <h2 className="section-heading">New hook set</h2>
          <HookSetEditor events={events.data?.events ?? []} onSubmit={create} busy={busy} />
        </section>
      )}
      {current && (
        <section className="card" aria-label="Hook set details">
          <h2 className="section-heading"><Text>{current.name}</Text></h2>
          {detail.error && (
            <p className="form-error" role="alert"><Text>{detail.error}</Text></p>
          )}
          {detail.data && (
            <HookSetEditor
              key={`${current.id}:${detail.data.item.content_hash}`}
              initial={detail.data.hooks}
              events={events.data?.events ?? []}
              onSubmit={(_, hooks) => replace(current, hooks)}
              busy={busy}
            />
          )}
          <HookSetHosts hookSetId={current.id} hosts={hosts.data} hats={hats.data} onChanged={hostsChanged} />
          <div className="card-actions">
            <span className="spacer" />
            <button type="button" className="btn btn-danger btn-sm" onClick={() => setDeleting(current)}>
              Delete
            </button>
          </div>
        </section>
      )}
      {deleting && (
        <ConfirmDialog
          title="Delete this hook set?"
          confirm="Delete"
          danger
          action={async () => {
            await deleteHookSet(client, deleting.id)
            list.set((prev) => (prev ?? []).filter((s) => s.id !== deleting.id))
            setSelected(null)
          }}
          onClose={() => setDeleting(null)}
          returnFocus={() => document.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')}
        >
          <p>
            <Text>{deleting.name}</Text> goes from the catalogue, and hosts stop getting it.
          </p>
        </ConfirmDialog>
      )}
    </>
  )
}
