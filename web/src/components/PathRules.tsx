// A host's path rules (kernel spec §5.2, §8): the whole set, edited here and
// sent back whole (step-up; the host must be connected, since it resolves
// each prefix). Beside it, a live tester: which hat a path on that host
// resolves to under the rules as SAVED, as a session started there would
// get: it asks again whenever they are saved, or a purge deleted some.
import { useEffect, useRef, useState } from 'react'
import { ApiFailure, messageOf } from '../api/errors'
import { pathRules, replacePathRules, resolveHat } from '../api/manage'
import { useClient } from '../app-client'
import type { HatItem, HatResolution, HostItem, PathRuleInput, PathRuleItem } from '../generated/protocol'
import { useResource } from '../hooks/useResource'
import { Text, visible } from '../lib/text'

/** How long typing pauses before the tester asks the host. */
export const TEST_DELAY_MS = 400

/** `purges` counts the purges tried: each may have deleted rules here. */
export default function PathRules({ hosts, hats, purges }: { hosts: HostItem[]; hats: HatItem[]; purges: number }) {
  const [hostId, setHostId] = useState<string | null>(null)
  const [saves, setSaves] = useState(0)
  const host = hosts.find((h) => h.host_id === hostId) ?? hosts[0]

  if (!host) {
    return (
      <section className="card" aria-labelledby="rules-title">
        <h2 className="card-title" id="rules-title">
          Path rules
        </h2>
        <p className="empty">Pair a host to give its paths a hat.</p>
      </section>
    )
  }
  return (
    <section className="card" aria-labelledby="rules-title">
      <h2 className="card-title" id="rules-title">
        Path rules
      </h2>
      <p className="hint">
        A session belongs to the hat of the longest rule whose path is its directory or a parent of it, by whole path
        segments; with no rule, to its host’s default hat.
      </p>
      <label className="select-field">
        <span className="field-label">Host</span>
        <select value={host.host_id} onChange={(e) => setHostId(e.target.value)}>
          {hosts.map((h) => (
            <option key={h.host_id} value={h.host_id}>
              {visible(h.name)}
            </option>
          ))}
        </select>
      </label>
      {/* After a purge the rules are read again; after a save, the answer
          is the set as stored already. The tester asks again after both. */}
      <Rules key={`${host.host_id}-${purges}`} host={host} hats={hats} onSaved={() => setSaves((n) => n + 1)} />
      <Tester key={`t-${host.host_id}`} host={host} hats={hats} rules={`${purges}.${saves}`} />
    </section>
  )
}

interface Row {
  key: number
  prefix: string
  hat_id: string
  verified?: boolean
}

function rowsOf(rules: PathRuleItem[], next: () => number): Row[] {
  return rules.map((r) => ({ key: next(), prefix: r.prefix, hat_id: r.hat_id, verified: r.verified }))
}

function Rules({ host, hats, onSaved }: { host: HostItem; hats: HatItem[]; onSaved: () => void }) {
  const client = useClient()
  const counter = useRef(0)
  const next = () => ++counter.current
  const [rows, setRows] = useState<Row[] | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [saved, setSaved] = useState(false)
  const stored = useResource(() => pathRules(client, host.host_id), [host.host_id])
  const usable = hats.filter((h) => !h.purging)
  // A removed row takes its focused button away: focus goes to the next
  // row's path, or to "Add rule" after the last.
  const inputs = useRef(new Map<number, HTMLInputElement>())
  const addRule = useRef<HTMLButtonElement>(null)
  const refocus = useRef<number | 'add' | null>(null)
  useEffect(() => {
    if (refocus.current === null) return
    const to = refocus.current === 'add' ? addRule.current : inputs.current.get(refocus.current)
    refocus.current = null
    to?.focus()
  })

  useEffect(() => {
    if (stored.data) setRows(rowsOf(stored.data, next))
  }, [stored.data])

  const edit = (key: number, change: Partial<Row>) => {
    setSaved(false)
    setRows((rows ?? []).map((r) => (r.key === key ? { ...r, ...change, verified: undefined } : r)))
  }

  const save = async () => {
    if (!rows) return
    setBusy(true)
    setError(null)
    try {
      const body: PathRuleInput[] = rows.map((r) => ({ prefix: r.prefix.trim(), hat_id: r.hat_id }))
      // The answer is the set as stored, resolved by the host: edit from
      // it, never from what was typed.
      stored.set(await replacePathRules(client, host.host_id, body))
      setSaved(true)
      onSaved()
    } catch (err) {
      setError(messageOf(err))
    } finally {
      setBusy(false)
    }
  }

  if (stored.error) {
    return (
      <p className="form-error" role="alert">
        <Text>{stored.error}</Text>
      </p>
    )
  }
  if (!rows) return <p role="status">Loading the rules…</p>
  // The server refuses a whole set for one blank path.
  const blank = rows.some((r) => r.prefix.trim() === '')

  return (
    <div className="rules">
      {rows.length === 0 && <p className="empty">No rules: every session on this host gets its default hat.</p>}
      <ul className="rule-list" aria-label="Rules">
        {rows.map((row, i) => (
          <li key={row.key} className="rule">
            <label className="field rule-prefix">
              <span className="field-label">Path {i + 1}</span>
              <input
                ref={(el) => {
                  if (el) inputs.current.set(row.key, el)
                  else inputs.current.delete(row.key)
                }}
                className="text-input mono"
                value={row.prefix}
                placeholder="/home/me/work"
                onChange={(e) => edit(row.key, { prefix: e.target.value })}
              />
            </label>
            <label className="select-field">
              <span className="field-label">Hat {i + 1}</span>
              <select value={row.hat_id} onChange={(e) => edit(row.key, { hat_id: e.target.value })}>
                {/* A rule's hat being purged is no choice, but it is the
                    rule's: shown, so the picker says what a save sends. */}
                {!usable.some((h) => h.id === row.hat_id) && (
                  <option value={row.hat_id} disabled>
                    {visible(hats.find((h) => h.id === row.hat_id)?.name ?? row.hat_id)}
                  </option>
                )}
                {usable.map((h) => (
                  <option key={h.id} value={h.id}>
                    {visible(h.name)}
                  </option>
                ))}
              </select>
            </label>
            {row.verified === false && (
              <span className="tag tag-warn" title="The path did not exist on the host when the rule was saved.">
                Unverified
              </span>
            )}
            <button
              type="button"
              className="btn btn-ghost btn-sm"
              aria-label={`Remove rule ${i + 1}`}
              onClick={() => {
                setSaved(false)
                refocus.current = rows[i + 1]?.key ?? 'add'
                setRows(rows.filter((r) => r.key !== row.key))
              }}
            >
              Remove
            </button>
          </li>
        ))}
      </ul>
      <div className="card-actions">
        <button
          type="button"
          className="btn btn-ghost btn-sm"
          ref={addRule}
          disabled={usable.length === 0}
          onClick={() => {
            setSaved(false)
            setRows([...rows, { key: next(), prefix: '', hat_id: usable[0].id }])
          }}
        >
          Add rule
        </button>
        <span className="spacer" />
        <button type="button" className="btn btn-primary btn-sm" onClick={save} disabled={busy || blank}>
          Save rules
        </button>
      </div>
      {blank && <p className="hint">Every rule needs a path.</p>}
      {!host.connected && <p className="hint">The host is offline: rules can be saved only while it is connected.</p>}
      {saved && <p role="status">Saved.</p>}
      {error && (
        <p className="form-error" role="alert">
          <Text>{error}</Text>
        </p>
      )}
    </div>
  )
}

type Verdict = { kind: 'idle' } | { kind: 'asking' } | { kind: 'resolved'; resolution: HatResolution } | { kind: 'failed'; error: string }

/** `rules` names the saved set: a new value asks again. */
function Tester({ host, hats, rules }: { host: HostItem; hats: HatItem[]; rules: string }) {
  const client = useClient()
  const [path, setPath] = useState('')
  const [verdict, setVerdict] = useState<Verdict>({ kind: 'idle' })

  // Each change waits for typing to pause, then asks; a newer path aborts
  // the older question, so an answer never lands on the wrong path, and
  // the last answer goes as soon as the path or the rules change.
  useEffect(() => {
    setVerdict({ kind: 'idle' })
    const typed = path.trim()
    if (typed === '') return
    const abort = new AbortController()
    const timer = setTimeout(() => {
      setVerdict({ kind: 'asking' })
      resolveHat(client, host.host_id, typed, abort.signal).then(
        (resolution) => {
          if (!abort.signal.aborted) setVerdict({ kind: 'resolved', resolution })
        },
        (err) => {
          if (abort.signal.aborted) return
          setVerdict({ kind: 'failed', error: testerMessage(err) })
        },
      )
    }, TEST_DELAY_MS)
    return () => {
      clearTimeout(timer)
      abort.abort()
    }
  }, [client, host.host_id, path, rules])

  const hatName = (id: string) => hats.find((h) => h.id === id)?.name ?? id

  return (
    <div className="tester">
      <label className="field">
        <span className="field-label">Test a path</span>
        <input
          className="text-input mono"
          value={path}
          placeholder="~/work/project"
          onChange={(e) => setPath(e.target.value)}
        />
      </label>
      <p className="hint">Resolved by the host, under the rules as saved.</p>
      {/* Each answer is its own live region, nested in none: announced
          once. */}
      <div className="tester-out">
        {verdict.kind === 'asking' && <p role="status">Asking the host…</p>}
        {verdict.kind === 'failed' && (
          <p className="form-error" role="alert">
            <Text>{verdict.error}</Text>
          </p>
        )}
        {verdict.kind === 'resolved' && (
          <div role="status">
            <dl className="facts" aria-label="Resolution">
              <dt>Resolves to</dt>
              <dd className="mono">
                <Text>{verdict.resolution.canonical}</Text>
              </dd>
              <dt>Hat</dt>
              <dd>
                <Text>{hatName(verdict.resolution.hat_id)}</Text>
              </dd>
              <dt>Decided by</dt>
              <dd>{verdict.resolution.rule_id ? 'a path rule' : 'the host’s default hat'}</dd>
              {!verdict.resolution.exists && (
                <>
                  <dt>Note</dt>
                  <dd>This path does not exist on the host.</dd>
                </>
              )}
              {verdict.resolution.exists && !verdict.resolution.is_dir && (
                <>
                  <dt>Note</dt>
                  <dd>This is not a directory: no session can start in it.</dd>
                </>
              )}
            </dl>
          </div>
        )}
      </div>
    </div>
  )
}

/** The tester's own words for the refusals a host's resolution can end in
 *  (plan 5b): the rest are the client's plain messages, or the host's text. */
function testerMessage(err: unknown): string {
  if (err instanceof ApiFailure && err.code === 'host_offline') return 'The host is offline: it resolves the path.'
  return messageOf(err)
}
