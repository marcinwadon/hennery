import { useEffect, useRef, useState } from 'react'
import { useClient } from '../app-client'
import { messageOf } from '../api/errors'
import { editQueuedPrompt, removeQueuedPrompt } from '../api/turns'
import type { QueuedPrompt } from '../generated/protocol'

export function queuedText(item: QueuedPrompt): string {
  return item.content.filter(block => block.type === 'text').map(block => block.text).join('\n')
}

interface EditDraft {
  text: string
  number: number
  interrupted: boolean
}

/** Queue edits have their own draft; they never replace the ordinary composer. */
export default function PromptQueue({ sessionId, items, changed }: { sessionId: string; items: QueuedPrompt[]; changed?: () => Promise<void> }) {
  const client = useClient()
  const [drafts, setDrafts] = useState<Record<string, EditDraft>>({})
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [copied, setCopied] = useState<string | null>(null)
  const [manualCopy, setManualCopy] = useState<string | null>(null)
  const editors = useRef(new Map<string, HTMLTextAreaElement>())
  const mutationPending = useRef(false)

  useEffect(() => {
    // An SSE delivery/removal cannot discard a local edit. Once interrupted,
    // keep it separately even if reconciliation returns the item to queued.
    setDrafts(previous => {
      const interrupted = Object.keys(previous).filter(id => !previous[id].interrupted && items.find(item => item.id === id)?.state !== 'queued')
      if (!interrupted.length) return previous
      const next = { ...previous }
      for (const id of interrupted) next[id] = { ...next[id], interrupted: true }
      return next
    })
  }, [items])

  function dismissEdit(id: string) {
    setDrafts(previous => {
      const next = { ...previous }
      delete next[id]
      return next
    })
  }

  async function copy(id: string, value: string, edit = false) {
    try {
      if (!navigator.clipboard?.writeText) throw new Error('clipboard unavailable')
      await navigator.clipboard.writeText(value)
      setCopied(id)
    } catch {
      if (edit) {
        editors.current.get(id)?.focus()
        editors.current.get(id)?.select()
        setError('Copy was unavailable. Select the full edit text to copy it.')
      } else {
        setManualCopy(id)
        setError('Copy was unavailable. Select the full message text to copy it.')
      }
    }
  }

  if (!items.length && !Object.keys(drafts).length) return null

  async function mutate(work: () => Promise<unknown>, discardEdit?: string) {
    if (mutationPending.current) return
    mutationPending.current = true
    setBusy(true)
    setError(null)
    try {
      await work()
      if (discardEdit) dismissEdit(discardEdit)
    } catch (err) {
      setError(messageOf(err))
    } finally {
      // Refresh after refusals too: another window or the worker may have won.
      try {
        await changed?.()
      } finally {
        mutationPending.current = false
        setBusy(false)
      }
    }
  }

  return <section className="prompt-queue" aria-label="Queued messages">
    <h3>Queued <span>{items.length}</span></h3>
    <ol>
      {[...items.map((item, index) => ({ id: item.id, item, number: index + 1 })),
        ...Object.entries(drafts).filter(([id]) => !items.some(item => item.id === id)).map(([id, draft]) => ({ id, item: undefined, number: draft.number })),
      ].map(({ id, item, number }) => {
        const preview = item ? queuedText(item) : ''
        const images = item?.content.filter(block => block.type === 'image').length ?? 0
        const draft = drafts[id]
        const interrupted = !!draft && (draft.interrupted || item?.state !== 'queued')
        const editNumber = draft?.number ?? number
        const saveEdit = () => {
          if (draft && !interrupted && !busy) void mutate(() => editQueuedPrompt(client, sessionId, id, draft.text), id)
        }
        return <li key={id} className={item?.state === 'failed' ? 'queue-failed' : undefined}>
          <div className="queue-copy">
            <span className="queue-number">{number}</span>
            <div>
              {draft ? <textarea ref={element => { if (element) editors.current.set(id, element); else editors.current.delete(id) }} aria-label={`Edit queued message ${editNumber}`} value={draft.text} onChange={event => { const text = event.target.value; setDrafts(previous => ({ ...previous, [id]: { ...previous[id], text } })); setCopied(null) }} onKeyDown={event => {
                if (event.defaultPrevented || event.repeat || event.nativeEvent.isComposing || event.keyCode === 229 || event.altKey || event.shiftKey) return
                if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
                  event.preventDefault()
                  saveEdit()
                }
              }} disabled={busy} /> : <p><bdi>{preview.length > 240 ? `${preview.slice(0, 240)}…` : preview || 'Image message'}</bdi></p>}
              {interrupted && <p className="queue-reason" role="status">Already sending; your edit wasn’t applied.</p>}
              {images > 0 && <small>{images} {images === 1 ? 'image' : 'images'}</small>}
              {item?.state === 'delivering' && <small role="status">Delivering…</small>}
              {item?.reason && <p className="queue-reason"><bdi>{item.reason}</bdi></p>}
              {manualCopy === id && <textarea readOnly aria-label={`Full text of queued message ${number}`} value={preview} onFocus={event => event.currentTarget.select()} />}
            </div>
          </div>
          <div className="queue-actions">
            {draft ? interrupted ? <>
              <button className="btn btn-ghost btn-sm" onClick={() => void copy(id, draft.text, true)}>{copied === id ? 'Copied' : 'Copy edit'}</button>
              <button className="btn btn-ghost btn-sm" disabled={busy} onClick={() => dismissEdit(id)}>Dismiss edit</button>
            </> : <>
              <button className="btn btn-ghost btn-sm" disabled={busy} onClick={saveEdit}>Save</button>
              <button className="btn btn-ghost btn-sm" disabled={busy} onClick={() => dismissEdit(id)}>Cancel edit</button>
            </> : item?.state === 'queued' && <button className="btn btn-ghost btn-sm" disabled={busy} aria-label={`Edit queued message ${number}`} onClick={() => { setDrafts(previous => ({ ...previous, [id]: { text: preview, number, interrupted: false } })); setError(null); setCopied(null) }}>Edit</button>}
            {item?.state === 'failed' && <button className="btn btn-ghost btn-sm" onClick={() => void copy(id, preview)}>{copied === id ? 'Copied' : 'Copy'}</button>}
            {item && item.state !== 'delivering' && <button className="btn btn-ghost btn-sm" disabled={busy} aria-label={`${item.state === 'failed' ? 'Dismiss' : 'Remove'} queued message ${number}`} onClick={() => void mutate(() => removeQueuedPrompt(client, sessionId, id), interrupted ? undefined : id)}>{item.state === 'failed' ? 'Dismiss' : 'Remove'}</button>}
          </div>
        </li>
      })}
    </ol>
    {error && <p className="form-error" role="alert"><bdi>{error}</bdi></p>}
  </section>
}
