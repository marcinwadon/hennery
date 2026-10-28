// The composer (frontend spec §6.5): a prompt with images, slash commands,
// a settings shortcut, and Send beside Cancel while a turn runs.
//
// Mount it as `<Composer sessionId=… />`: the export is keyed by the
// session id, so each session gets its own instance, and nothing typed or
// attached for one session is ever shown in, or sent to, another (F-17).
// The draft lives in `sessionStorage` (`hennery.draft.<id>`), the images in
// memory (lib/attachments.ts); both come back when the session does. So
// does a send still in flight (lib/sending.ts): the composer mounted again
// stays read-only until it ends, and a 202 clears the draft it shows.
//
// A `handle` lets the session screen put words in the draft: a question's
// "Answer as a new message", a turn that was not delivered ("Send again").
// Neither sends on its own: the operator does.
import {
  useEffect,
  useId,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type ClipboardEvent,
  type KeyboardEvent,
  type Ref,
} from 'react'
import { useClient } from '../app-client'
import { ApiFailure, messageOf } from '../api/errors'
import { cancel, prompt } from '../api/turns'
import type { Capabilities, QueuedPrompt, SessionCatalog } from '../generated/protocol'
import type { SessionSummary } from '../generated/view'
import {
  admit,
  forgetAttachments,
  heldFor,
  highestMarker,
  hold,
  inertMarkers,
  live,
  marker,
  promptBlocks,
  splice,
  withoutMarker,
  type Attachment,
} from '../lib/attachments'
import { commands, matchCommands, type Command } from '../lib/catalog'
import { clearDraftSending, draftMayHaveBeenSent, draftSendingRequestKey, loadDraft, markDraftConflicted, markDraftSending, saveDraft } from '../lib/drafts'
import { sendingFor, track } from '../lib/sending'
import { navigationGate } from '../pwa/navigation'
import { undeliveredDraft, type DraftPart } from '../lib/sendAgain'
import { Icon } from '../lib/ui'
import type { SessionConfigState } from '../hooks/useSessionConfig'
import { Icon as Glyph } from './ui'
import PromptQueue from './PromptQueue'
import { CANCEL_OUTCOME, SEND_AGAIN_LABEL, cancelRefusal, isBusyTurn, promptRefusal } from './composerWords'

/** What the session screen may ask of its composer. */
export interface ComposerHandle {
  focus(): void
  /** Put `text` in the draft, after what is there (an empty draft is
   *  replaced), with `label` shown above it until it is sent; the cursor
   *  goes to its end. Sends nothing. */
  prefill(text: string, label?: string): void
  /** Nothing is written or attached: the draft holds no work. */
  isEmpty(): boolean
  /** Put the undelivered turn `turnId` back in the draft, its images
   *  fetched again, for the operator to send. All or nothing: a turn that
   *  cannot be rebuilt whole says why and leaves the draft as it was. */
  refill(turnId: string): Promise<void>
}

export interface ComposerProps {
  /** The session prompts go to. A new id is a new composer (see above). */
  sessionId: string
  /** The session as the list shows it, null while unknown. A turn in
   *  flight (`activity` is `running` or `blocked`) offers Cancel alongside
   *  Send, which queues input; while `starting`, a refusal never offers a resume. */
  session: Pick<SessionSummary, 'activity' | 'lifecycle'> | null
  /** The session's host's capabilities (`HostItem.capabilities`), null
   *  while unknown. Images are hidden only when they are known and lack
   *  `images`. */
  capabilities: Capabilities | null
  /** The session's catalogue (config options, slash commands); null while
   *  it loads. */
  catalog: SessionCatalog | null
  /** Shared with Details; the synchronous guard covers picks before rerender. */
  configuration: Pick<SessionConfigState, 'pending' | 'error' | 'isPending'>
  onSettings: (trigger: HTMLButtonElement) => void
  settingsExpanded?: boolean
  /** "Resume and send" resumes through this, then sends the draft; it
   *  rejects with the refusal to show. The view passes its one resume, the
   *  one its footer's Resume goes through. */
  onResume: () => Promise<unknown>
  onJumpToQuestion?: () => void
  queue?: QueuedPrompt[]
  onQueueChange?: () => Promise<void>
  /** Acknowledges only the result visible when this send began. */
  onSent?: () => void
  /** The handle above, bound to the composer of the session shown. */
  handle?: Ref<ComposerHandle>
}

/** The composer of one session, remounted whenever the session changes. */
export function Composer(props: ComposerProps) {
  return <SessionComposer key={props.sessionId} {...props} />
}

const STILL_STARTING = 'The session is still starting: send again once it runs.'

/** Said when work is added while a prompt is being sent: the 202 clears
 *  the draft, so whatever came in meanwhile would be lost. It stays after
 *  the send, so it is worded to be true then too. */
export const STILL_SENDING = 'Not added: a prompt was being sent. Add it again.'

interface Notice {
  text: string
  /** Offer "Resume and send" (a prompt refused with `not_attached`). */
  resume?: boolean
  busyTurn?: boolean
}

/** How a send ended: sent (a 202), or refused, saying why. */
type Outcome = { sent: true } | { sent: false; notice: Notice }

function SessionComposer({ sessionId, session, capabilities, catalog, configuration, onSettings, settingsExpanded = false, onResume, onJumpToQuestion, onSent, handle, queue = [], onQueueChange }: ComposerProps) {
  const client = useClient()
  const ids = useId()
  const [text, setText] = useState(() => loadDraft(sessionId))
  const [atts, setAtts] = useState<Attachment[]>(() => heldFor(sessionId).attachments)
  // The next image's number: past every number this draft has used, and
  // past any marker a restored draft still holds.
  const nextN = useRef(0)
  if (nextN.current === 0) nextN.current = Math.max(heldFor(sessionId).nextN, highestMarker(text) + 1)
  // A send in flight, this composer's or one made before it was mounted
  // again (lib/sending.ts). Read once: the flag and the mount's follower
  // below hold the same send, even one that ends before the effect runs.
  const [mountedSend] = useState(() => sendingFor<Outcome>(sessionId))
  const [restoredSend, setRestoredSend] = useState(() => !mountedSend && draftMayHaveBeenSent(sessionId))
  const [sending, setSending] = useState(mountedSend !== undefined)
  // The same, read synchronously: a refill answering later, or a paste in
  // the render a send started from, sees it at once.
  const sendingNow = useRef(sending)
  const setSendingBoth = (on: boolean) => {
    sendingNow.current = on
    setSending(on)
  }
  const [cancelling, setCancelling] = useState(false)
  const [notice, setNotice] = useState<Notice | null>(null)
  // What a paste, a drop, a pick or a put-in draft could not add: shown
  // until the next such action or the next send, never cleared by a send's
  // answer (a refusal made while it was in flight would vanish with it).
  const [draftError, setDraftError] = useState<string | null>(null)
  const [sel, setSel] = useState(0)
  const [dismissed, setDismissed] = useState(false)
  const [dragging, setDragging] = useState(false)
  // What a draft put in for the operator is (see the handle).
  const [label, setLabel] = useState<string | null>(null)
  const taRef = useRef<HTMLTextAreaElement>(null)
  const fileRef = useRef<HTMLInputElement>(null)
  const cursor = useRef<number | null>(null)

  useEffect(() => {
    saveDraft(sessionId, text)
    if (!draftMayHaveBeenSent(sessionId)) setRestoredSend(false)
  }, [sessionId, text])
  useLayoutEffect(() => {
    const ta = taRef.current
    if (!ta) return
    ta.style.height = 'auto'
    ta.style.height = text ? `${Math.min(ta.scrollHeight, 160)}px` : ''
  }, [text])

  // Put the cursor after markers just spliced in.
  useLayoutEffect(() => {
    const ta = taRef.current
    if (ta && cursor.current !== null) {
      ta.focus()
      ta.setSelectionRange(cursor.current, cursor.current)
      cursor.current = null
    }
  }, [text])

  const inFlight = session?.activity === 'running' || session?.activity === 'blocked'
  const hostImages = capabilities === null || capabilities.includes('images')
  // A session still starting refuses a resume too: it runs soon on its own.
  const canResume = session?.lifecycle !== 'starting'
  // Nothing to send: Send and "Resume and send" both wait for words.
  const blank = text.trim() === ''
  const canSend = !sending && !blank && !configuration.pending

  // The draft as last rendered, for the handle and for a refill that
  // answers later.
  const latest = useRef({ text, atts, hostImages })
  useLayoutEffect(() => {
    latest.current = { text, atts, hostImages }
  })
  // Aborted when the composer goes: a refill answering later lands nowhere.
  const alive = useRef<AbortController | null>(null)
  useEffect(() => {
    const controller = new AbortController()
    alive.current = controller
    // A send made before this composer was mounted again ends here.
    if (mountedSend) void follow(mountedSend)
    return () => controller.abort()
    // Once per composer: its session never changes (see Composer).
  }, [])

  const cmds = commands(catalog)
  // The slash menu: the text starts with `/` and has no space yet.
  const query = !dismissed && /^\/\S*$/.test(text) ? text.slice(1) : null
  const matches = query === null ? [] : matchCommands(cmds, query)
  const menuOpen = matches.length > 0
  const selIdx = Math.min(sel, matches.length - 1)
  const optionId = (i: number) => `${ids}-cmd-${i}`

  function keep(next: Attachment[]) {
    setAtts(next)
    hold(sessionId, { attachments: next, nextN: nextN.current })
  }

  function edit(next: string) {
    setText(next)
    setSel(0)
    setDismissed(false)
    if (next.trim() === '') setLabel(null)
  }

  /** Add `parts` to the draft as one action: after the text there (or in
   *  place of a blank one), each image as a new marker. All or nothing. */
  function addToDraft(parts: DraftPart[], why: string | null) {
    // A refill GET may settle after update readiness froze this document.
    if (navigationGate.blocked()) return
    if (sendingNow.current) {
      setDraftError(STILL_SENDING)
      return
    }
    const { text: current, atts: held, hostImages: images } = latest.current
    const files = parts.flatMap((p) => (p.type === 'image' ? [p.file] : []))
    if (files.length > 0 && !images) {
      setNotice({ text: 'This turn holds images, and this host takes no images.' })
      return
    }
    // Past every marker the draft or the words put in name: a literal
    // `[Image #N]` in them never comes to name a new image.
    const words = parts.flatMap((p) => (p.type === 'text' ? [p.text] : [])).join(' ')
    nextN.current = Math.max(nextN.current, highestMarker(`${current} ${words}`) + 1)
    const { accepted, refused } = admit(held, files, nextN.current)
    if (refused.length > 0) {
      setNotice({ text: refused.join(' ') })
      return
    }
    const numbers = new Map(accepted.map((a) => [a.file, a.n]))
    let added = ''
    for (const part of parts) {
      const n = part.type === 'image' ? numbers.get(part.file) : undefined
      const piece = part.type === 'text' ? part.text : n === undefined ? '' : marker(n)
      if (added !== '' && !/\s$/.test(added) && !/^\s/.test(piece)) added += ' '
      added += piece
    }
    if (added.trim() === '') return
    if (accepted.length > 0) nextN.current = accepted[accepted.length - 1].n + 1
    const next = current.trim() === '' ? added : current + (current.endsWith('\n') ? '\n' : '\n\n') + added
    cursor.current = next.length
    edit(next)
    setLabel(why)
    if (accepted.length > 0) keep([...held, ...accepted])
  }

  async function refill(turnId: string) {
    const signal = alive.current?.signal
    setNotice(null)
    let parts: DraftPart[]
    try {
      parts = await undeliveredDraft(client, sessionId, turnId, signal)
    } catch (err) {
      if (!signal?.aborted) setNotice({ text: `The turn could not be put back: ${messageOf(err)}` })
      return
    }
    if (!signal?.aborted) addToDraft(parts, SEND_AGAIN_LABEL)
  }

  useImperativeHandle(handle, () => ({
    focus: () => taRef.current?.focus(),
    // Words an agent wrote never link a held image. A turn put back is the
    // operator's own, and goes back as it was sent.
    prefill: (words: string, why?: string) => addToDraft([{ type: 'text', text: inertMarkers(words) }], why ?? null),
    isEmpty: () => latest.current.text.trim() === '' && latest.current.atts.length === 0,
    refill,
  }))

  function pickCommand(c: Command) {
    edit(`/${c.name} `)
    cursor.current = c.name.length + 2
  }

  /** Attach `files` as one action: every marker spliced in at once, at the
   *  cursor (over the selection). */
  function attach(files: File[]) {
    if (files.length === 0) return
    if (sendingNow.current) {
      setDraftError(STILL_SENDING)
      return
    }
    if (!hostImages) {
      setDraftError('This host takes no images.')
      return
    }
    const { accepted, refused } = admit(atts, files, nextN.current)
    setDraftError(refused.length > 0 ? refused.join(' ') : null)
    if (accepted.length === 0) return
    nextN.current = accepted[accepted.length - 1].n + 1
    const ta = taRef.current
    const start = ta ? ta.selectionStart : text.length
    const end = ta ? ta.selectionEnd : text.length
    const placed = splice(text, start, end, accepted.map((a) => marker(a.n)).join(' ') + ' ')
    cursor.current = placed.cursor
    edit(placed.text)
    keep([...atts, ...accepted])
  }

  function removeAttachment(n: number) {
    keep(atts.filter((a) => a.n !== n))
    edit(withoutMarker(text, n))
  }

  /** POST `draft` with `images`. Kept on any refusal; a 202 clears the
   *  draft at its source, as this may answer after the composer went. */
  async function deliver(draft: string, images: Attachment[], requestKey: string): Promise<Outcome> {
    try {
      const blocks = await promptBlocks(draft, images)
      markDraftSending(sessionId, draft, requestKey)
      await prompt(client, sessionId, blocks, requestKey)
      void onQueueChange?.()
    } catch (err) {
      if (err instanceof ApiFailure && err.code === 'request_key_conflict') markDraftConflicted(sessionId, draft)
      else if (err instanceof ApiFailure && err.status < 500 && err.code !== 'delivery_unknown') clearDraftSending(sessionId)
      return {
        sent: false,
        notice: { text: promptRefusal(err), resume: err instanceof ApiFailure && err.code === 'not_attached', busyTurn: isBusyTurn(err) },
      }
    }
    saveDraft(sessionId, '')
    forgetAttachments(sessionId)
    onSent?.()
    return { sent: true }
  }

  /** A send's outcome, taken by the composer shown when it ends (one that
   *  went takes nothing: React drops its updates). */
  async function follow(pending: Promise<Outcome>) {
    const outcome = await pending
    if (outcome.sent) {
      setRestoredSend(false)
      nextN.current = 1
      setAtts([])
      setText('')
      setLabel(null)
    } else {
      setRestoredSend(draftMayHaveBeenSent(sessionId))
      setNotice(outcome.notice)
    }
    // Whatever came of it, the draft is the operator's again.
    setSendingBoth(false)
  }

  /** Start `work` as this session's send, the draft as it stands. */
  function begin(work: (draft: string, images: Attachment[], requestKey: string) => Promise<Outcome>) {
    // One send at a time: a second click before the composer renders again
    // finds the first one here.
    if (sendingNow.current || configuration.isPending()) return
    setSendingBoth(true)
    setNotice(null)
    setDraftError(null)
    const draft = text
    const images = atts
    // The Client may retry after step-up. One submission keeps one key,
    // including after a reload left the previous response uncertain.
    const requestKey = draftSendingRequestKey(sessionId) ?? crypto.randomUUID()
    // Every path of `work` ends in an outcome; a throw is worded all the same.
    const thrown = (err: unknown): Outcome => ({ sent: false, notice: { text: messageOf(err) } })
    void follow(track(sessionId, () => work(draft, images, requestKey), thrown))
  }

  function send() {
    if (canSend && !configuration.isPending()) begin(deliver)
  }

  /** Resume, then send; tracked as one send, so a composer mounted again
   *  while the resume is answered waits for both. */
  function resumeAndSend() {
    if (!canSend || configuration.isPending()) return
    begin(async (draft, images, requestKey) => {
      try {
        await onResume()
      } catch (err) {
        return { sent: false, notice: { text: messageOf(err) } }
      }
      return deliver(draft, images, requestKey)
    })
  }

  async function stop() {
    setCancelling(true)
    setNotice(null)
    try {
      const answer = await cancel(client, sessionId)
      setNotice({
        text: Object.hasOwn(CANCEL_OUTCOME, answer.outcome)
          ? CANCEL_OUTCOME[answer.outcome]
          : `The turn ended: ${answer.outcome}.`,
      })
    } catch (err) {
      setNotice({ text: cancelRefusal(err) })
    } finally {
      setCancelling(false)
    }
  }

  function onKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229 || e.repeat) return
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault()
      send()
      return
    }
    if (!menuOpen) return
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setSel((s) => (Math.min(s, matches.length - 1) + 1) % matches.length)
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setSel((s) => (Math.min(s, matches.length - 1) - 1 + matches.length) % matches.length)
    } else if (e.key === 'Enter') {
      e.preventDefault()
      pickCommand(matches[selIdx])
    } else if (e.key === 'Escape') {
      e.preventDefault()
      setDismissed(true)
    }
  }

  function onPaste(e: ClipboardEvent<HTMLTextAreaElement>) {
    const files: File[] = []
    for (const item of Array.from(e.clipboardData.items)) {
      if (item.kind === 'file' && item.type.startsWith('image/')) {
        const file = item.getAsFile()
        if (file) files.push(file)
      }
    }
    if (files.length > 0) {
      e.preventDefault()
      attach(files)
    }
  }

  const liveNs = new Set(live(text, atts).map((a) => a.n))

  return (
    <div className="composer">
      <div className="composer-inner">
        {label && (
          <p className="composer-label" role="status">
            {label}
          </p>
        )}
        <PromptQueue sessionId={sessionId} items={queue} changed={onQueueChange} />
        {(configuration.pending || configuration.error) && <div className="composer-config-status" role="status">
          {configuration.pending ? 'Applying session settings…' : <><span>Settings need attention.</span><button type="button" className="btn btn-ghost btn-sm" onClick={e => onSettings(e.currentTarget)}>Check settings</button></>}
        </div>}
        <div
          className={'composer-box' + (dragging ? ' dragging' : '')}
          onDragOver={(e) => {
            if (!e.dataTransfer.types.includes('Files')) return
            e.preventDefault()
            setDragging(hostImages)
          }}
          onDragLeave={(e) => {
            if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false)
          }}
          onDrop={(e) => {
            if (e.dataTransfer.files.length === 0) return
            e.preventDefault()
            setDragging(false)
            attach(Array.from(e.dataTransfer.files))
          }}
        >
          {menuOpen && (
            <ul className="slash-menu" role="listbox" id={`${ids}-cmds`} aria-label="Slash commands">
              {matches.map((c, i) => (
                <li
                  key={c.name}
                  id={optionId(i)}
                  role="option"
                  aria-selected={i === selIdx}
                  className={'slash-item' + (i === selIdx ? ' sel' : '')}
                  onMouseEnter={() => setSel(i)}
                  // Before the textarea loses focus.
                  onMouseDown={(e) => {
                    e.preventDefault()
                    pickCommand(c)
                  }}
                >
                  <span className="slash-name">
                    <bdi>/{c.name}</bdi>
                  </span>
                  {(c.description || c.hint) && (
                    <span className="slash-desc">
                      <bdi>{c.description || c.hint}</bdi>
                    </span>
                  )}
                </li>
              ))}
            </ul>
          )}
          <textarea
            ref={taRef}
            aria-label="Prompt"
            aria-autocomplete="list"
            aria-controls={menuOpen ? `${ids}-cmds` : undefined}
            aria-activedescendant={menuOpen ? optionId(selIdx) : undefined}
            rows={1}
            value={text}
            readOnly={sending}
            placeholder={cmds.length > 0 ? 'Write a prompt… (/ for commands)' : 'Write a prompt…'}
            onChange={(e) => {
              edit(e.target.value)
            }}
            onKeyDown={onKeyDown}
            onPaste={onPaste}
          />
          <div className="composer-actions">
            {hostImages && (
              <>
                <input
                  ref={fileRef}
                  type="file"
                  accept="image/png,image/jpeg,image/gif,image/webp"
                  multiple
                  hidden
                  aria-hidden="true"
                  tabIndex={-1}
                  data-testid="image-input"
                  onChange={(e) => {
                    attach(Array.from(e.target.files ?? []))
                    e.target.value = ''
                  }}
                />
                <button className="attach-btn" type="button" aria-label="Attach images" onClick={() => fileRef.current?.click()}>
                  <Icon.Paperclip size={18} />
                </button>
              </>
            )}
            <button className="attach-btn composer-settings-button" type="button" aria-label="Session settings" title="Session settings" aria-expanded={settingsExpanded} onClick={e => onSettings(e.currentTarget)}><Glyph name="adjustments-horizontal" size={18} /></button>
            <span className="composer-actions-space" />
            {inFlight && (
              <button
                className="btn btn-danger btn-sm"
                type="button"
                disabled={cancelling}
                onClick={() => void stop()}
              >
                {cancelling ? 'Stopping…' : 'Cancel'}
              </button>
            )}
            {
              <button className="send-btn" type="button" aria-label={session?.lifecycle === 'parked' ? 'Resume and send' : 'Send'} title={session?.lifecycle === 'parked' ? 'Resume and send' : 'Send'} disabled={!canSend} onClick={send}>
                <Icon.ArrowUp size={18} />
              </button>
            }
          </div>
        </div>
        {atts.length > 0 && (
          <ul className="att-strip" aria-label="Images">
            {atts.map((a) => (
              <li
                className="att-chip"
                key={a.n}
                title={liveNs.has(a.n) ? undefined : `${marker(a.n)} is no longer in the text: it will not be sent.`}
              >
                <Thumb file={a.file} n={a.n} />
                <span className="att-n">#{a.n}</span>
                <button className="att-x" type="button" aria-label={`Remove image #${a.n}`} onClick={() => removeAttachment(a.n)}>
                  <Icon.X size={12} />
                </button>
              </li>
            ))}
          </ul>
        )}
        {draftError && (
          <p className="form-error" role="alert">
            <bdi>{draftError}</bdi>
          </p>
        )}
        {restoredSend && !sending && (
          <p className="composer-hint" role="status">This draft may already have been sent. Check the transcript before sending again.</p>
        )}
        {notice && (
          <div className="composer-hint" role="status">
            <span>
              <bdi>{notice.resume && !canResume ? STILL_STARTING : notice.text}</bdi>
            </span>
            {notice.resume && canResume && (
              <button className="btn btn-primary btn-sm" type="button" disabled={!canSend} onClick={resumeAndSend}>
                Resume and send
              </button>
            )}
            {notice.busyTurn && onJumpToQuestion && <button className="btn btn-ghost btn-sm" type="button" onClick={onJumpToQuestion}>Go to open request</button>}
          </div>
        )}
      </div>
    </div>
  )
}

/** An attached image's preview. Its object URL lives exactly as long as the
 *  chip: revoked when the image is removed, sent, or the composer goes. */
function Thumb({ file, n }: { file: File; n: number }) {
  const [url, setUrl] = useState<string | null>(null)
  useEffect(() => {
    const made = URL.createObjectURL(file)
    setUrl(made)
    return () => URL.revokeObjectURL(made)
  }, [file])
  return url ? <img src={url} alt={`Image #${n}`} /> : null
}
