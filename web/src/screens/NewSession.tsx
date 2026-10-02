// New session (frontend spec §7): a host, an agent, a project, the hat the
// project resolves to, an optional first prompt, then Start.
//
// - Hosts: connected ones first; offline ones listed but disabled; revoked
//   ones never offered.
// - Changing the host resets the agent, the path and the browser (in the
//   click handler, never in an effect, so a prefilled path survives the
//   prefilled host being chosen on load).
// - `/new?host=<id>&cwd=<path>` prefills both ("Start a new session in this
//   project"). A prefilled host that cannot be used is not replaced by
//   another in silence: a notice says why.
// - Start: `POST /api/sessions`, then the first prompt as its own
//   `POST …/prompt` (the server takes no first prompt with a start yet),
//   then `/sessions/<id>`. A first prompt that does not go through, or a
//   start whose delivery is unknown, still goes to the session, with a
//   notice (lib/start.ts) and the prompt kept as that session's draft.
import { useEffect, useId, useMemo, useRef, useState, type FormEvent } from 'react'
import { ApiFailure, messageOf } from '../api/errors'
import { useClient } from '../app-client'
import ProjectPicker from '../components/ProjectPicker'
import type {
  HatItem,
  HatResolution,
  HostItem,
  HostProjects,
  StartSessionRequest,
  StartSessionResponse,
} from '../generated/protocol'
import { agentLabel } from '../lib/agent'
import { agentOptions, agentsFor, type HostAgentChoices } from '../lib/agents'
import { saveDraft } from '../lib/drafts'
import { projectEntries } from '../lib/projects'
import { sessionHref, startRefusal } from '../lib/start'
import { Icon } from '../lib/ui'
import { Link, navigate, useLocation } from '../router'

/** How long the path must stay unchanged before its hat is resolved. */
export const RESOLVE_DEBOUNCE_MS = 300

/** The other agent choice: a name typed by hand. */
const OTHER = '\u0000other'

/** Hosts to offer: never a revoked one; connected first, each group in the
 *  server's order. */
export function offeredHosts(hosts: HostItem[]): HostItem[] {
  const live = hosts.filter((h) => h.revoked_at == null)
  return [...live.filter((h) => h.connected), ...live.filter((h) => !h.connected)]
}

type Resolved =
  | { state: 'idle' }
  | { state: 'pending' }
  | { state: 'ok'; resolution: HatResolution }
  | { state: 'failed'; error: string }

function readPrefill(search: string): { host?: string; cwd?: string } {
  const query = new URLSearchParams(search)
  return { host: query.get('host') ?? undefined, cwd: query.get('cwd') ?? undefined }
}

export default function NewSession() {
  const client = useClient()
  const { search } = useLocation()
  // Read once: the form owns its fields after that.
  const [prefill] = useState(() => readPrefill(search))
  const ids = { host: useId(), agent: useId(), project: useId(), prompt: useId() }

  const [hosts, setHosts] = useState<HostItem[] | null>(null)
  const [hostsError, setHostsError] = useState<string | null>(null)
  const [hostId, setHostId] = useState('')
  const [notice, setNotice] = useState<string | null>(null)

  const [choices, setChoices] = useState<HostAgentChoices | null>(null)
  const [agent, setAgent] = useState('')
  const [otherName, setOtherName] = useState('')

  const [projects, setProjects] = useState<HostProjects | null>(null)
  const [projectsError, setProjectsError] = useState<string | null>(null)
  /** The canonical path whose hat the recents are asked for, once it
   *  differs from the one they came for. */
  const [recentsFor, setRecentsFor] = useState<string | null>(null)
  const [cwd, setCwd] = useState('')
  const [initialText, setInitialText] = useState('')

  const [hats, setHats] = useState<HatItem[]>([])
  const [resolved, setResolved] = useState<Resolved>({ state: 'idle' })

  const [prompt, setPrompt] = useState('')
  const [busy, setBusy] = useState(false)
  const busyRef = useRef(false)
  const [error, setError] = useState<string | null>(null)

  // The hosts, once, and the prefilled (or first connected) host.
  useEffect(() => {
    let live = true
    client.request<HostItem[]>('GET', '/api/hosts').then(
      (all) => {
        if (!live) return
        const offered = offeredHosts(all)
        setHosts(offered)
        if (prefill.host !== undefined) {
          const wanted = offered.find((h) => h.host_id === prefill.host)
          if (wanted?.connected) {
            setHostId(wanted.host_id)
            if (prefill.cwd) {
              setCwd(prefill.cwd)
              setInitialText(prefill.cwd)
            }
            return
          }
          setNotice(
            wanted
              ? `${wanted.name} is offline, so the session cannot start there: pick another host.`
              : 'The host this link names is not paired any more: pick another host.',
          )
          return
        }
        const first = offered.find((h) => h.connected)
        if (first) setHostId(first.host_id)
      },
      (err) => live && setHostsError(messageOf(err)),
    )
    return () => {
      live = false
    }
  }, [client, prefill])

  // The hats, for names.
  useEffect(() => {
    let live = true
    client.request<HatItem[]>('GET', '/api/hats').then(
      (all) => live && setHats(all),
      () => {
        // Without names the preview shows the hat's id.
      },
    )
    return () => {
      live = false
    }
  }, [client])
  const hatName = (id: string) => hats.find((h) => h.id === id)?.name ?? id

  // The host's agents, the first usable one picked. A host change resets
  // the agent first (`chooseHost`), so no pick of another host is kept.
  useEffect(() => {
    if (!hostId) return
    let live = true
    setChoices(null)
    agentsFor(client, hostId).then(
      (got) => {
        if (!live) return
        setChoices(got)
        const usable = agentOptions(got.agents).filter((o) => o.disabled === undefined)
        setAgent((current) => (current !== '' ? current : (usable[0]?.agent ?? (got.other ? OTHER : ''))))
      },
      (err) => live && setError(messageOf(err)),
    )
    return () => {
      live = false
    }
  }, [client, hostId])

  // The host's projects: the recents of the hat the chosen path resolves to
  // (the host's default hat until one is resolved), and its repositories.
  useEffect(() => {
    if (!hostId) return
    let live = true
    setProjectsError(null)
    const path = `/api/hosts/${encodeURIComponent(hostId)}/projects`
    client
      .request<HostProjects>('GET', recentsFor === null ? path : `${path}?path=${encodeURIComponent(recentsFor)}`)
      .then(
        (got) => live && setProjects(got),
        (err) => live && setProjectsError(messageOf(err)),
      )
    return () => {
      live = false
    }
  }, [client, hostId, recentsFor])

  // The hat the path resolves to, once the path has rested a moment. An
  // answer for a path or host no longer chosen is dropped.
  useEffect(() => {
    if (!hostId || cwd.trim() === '') {
      setResolved({ state: 'idle' })
      return
    }
    let live = true
    const abort = new AbortController()
    setResolved({ state: 'pending' })
    const timer = setTimeout(() => {
      client
        .request<HatResolution>('POST', '/api/hats/resolve', { host_id: hostId, path: cwd.trim() }, { signal: abort.signal })
        .then(
          (resolution) => live && setResolved({ state: 'ok', resolution }),
          (err) => live && setResolved({ state: 'failed', error: startRefusal(err) }),
        )
    }, RESOLVE_DEBOUNCE_MS)
    return () => {
      live = false
      clearTimeout(timer)
      abort.abort()
    }
  }, [client, hostId, cwd])

  // Recents for the resolved hat, when the list holds another hat's.
  useEffect(() => {
    if (resolved.state !== 'ok' || !projects) return
    if (resolved.resolution.hat_id !== projects.recents_hat_id) setRecentsFor(resolved.resolution.canonical)
  }, [resolved, projects])

  const entries = useMemo(() => (projects ? projectEntries(projects) : []), [projects])
  const host = hosts?.find((h) => h.host_id === hostId)
  const browseRoot = projects?.home ?? host?.workspace_roots[0]

  function chooseHost(id: string) {
    if (id === hostId) return
    setHostId(id)
    // A path, an agent and a listing of one host mean nothing on another.
    setAgent('')
    setOtherName('')
    setCwd('')
    setInitialText('')
    setProjects(null)
    setRecentsFor(null)
    setNotice(null)
    setError(null)
  }

  const agentName = agent === OTHER ? otherName.trim() : agent
  const canStart = !busy && host?.connected === true && agentName !== '' && cwd.trim() !== ''

  async function start(e: FormEvent) {
    e.preventDefault()
    if (!canStart || busyRef.current) return
    busyRef.current = true
    setBusy(true)
    setError(null)
    const request: StartSessionRequest = { host_id: hostId, agent: agentName, cwd: cwd.trim() }
    let sessionId: string
    try {
      sessionId = (await client.request<StartSessionResponse>('POST', '/api/sessions', request)).session_id
    } catch (err) {
      if (err instanceof ApiFailure && err.code === 'delivery_unknown' && err.sessionId) {
        navigate(sessionHref(err.sessionId, { kind: 'start_unknown', kept: keepDraft(err.sessionId, prompt) }))
        return
      }
      busyRef.current = false
      setBusy(false)
      setError(startRefusal(err))
      return
    }
    // Images are not part of the first prompt yet: the composer's image
    // rules (Task 6) take them once it is shared with this form.
    if (prompt.trim() !== '') {
      try {
        await client.request('POST', `/api/sessions/${encodeURIComponent(sessionId)}/prompt`, {
          content: [{ type: 'text', text: prompt }],
        })
      } catch (err) {
        keepDraft(sessionId, prompt)
        const code = err instanceof ApiFailure ? err.code : 'unknown'
        navigate(sessionHref(sessionId, { kind: 'prompt_failed', code }))
        return
      }
    }
    navigate(sessionHref(sessionId))
  }

  const options = choices ? agentOptions(choices.agents) : []

  return (
    <form className="new-session" onSubmit={start} aria-labelledby="new-session-title">
      <div className="modal-head">
        <div className="modal-eyebrow">Start a session</div>
        <h1 className="modal-title" id="new-session-title">
          New session
        </h1>
      </div>
      <div className="modal-body">
        {notice !== null && (
          <p className="hint" role="status">
            <bdi>{notice}</bdi>
          </p>
        )}

        <div className="field" role="group" aria-labelledby={ids.host}>
          <div className="field-label" id={ids.host}>
            <Icon.Terminal size={16} /> Host <span className="hint">where the agent runs</span>
          </div>
          {hostsError !== null ? (
            <p className="form-error" role="alert">
              <bdi>{hostsError}</bdi>
            </p>
          ) : hosts === null ? (
            <p className="hint">Loading…</p>
          ) : hosts.length === 0 ? (
            <p className="hint">
              No host is paired yet. <Link to="/hosts">Pair one</Link>.
            </p>
          ) : (
            <div className="seg">
              {hosts.map((h) => (
                <button
                  key={h.host_id}
                  type="button"
                  className={'seg-item' + (hostId === h.host_id ? ' sel' : '')}
                  aria-pressed={hostId === h.host_id}
                  disabled={!h.connected}
                  title={h.connected ? undefined : 'Offline'}
                  onClick={() => chooseHost(h.host_id)}
                >
                  <span className={'seg-led ' + (h.connected ? 'led-run' : 'led-idle')} />
                  <span>
                    <span className="seg-name">
                      <bdi>{h.name}</bdi>
                    </span>
                    {!h.connected && <span className="seg-host"> offline</span>}
                  </span>
                </button>
              ))}
            </div>
          )}
        </div>

        {hostId !== '' && (
          <>
            <div className="field" role="group" aria-labelledby={ids.agent}>
              <div className="field-label" id={ids.agent}>
                <Icon.Cpu size={16} /> Agent
              </div>
              {choices === null ? (
                <p className="hint">Loading…</p>
              ) : (
                <div className="seg">
                  {options.map((o) => (
                    <button
                      key={o.agent}
                      type="button"
                      className={'seg-item' + (agent === o.agent ? ' sel' : '')}
                      aria-pressed={agent === o.agent}
                      disabled={o.disabled !== undefined}
                      title={o.disabled}
                      onClick={() => setAgent(o.agent)}
                    >
                      <span className="seg-name">
                        <bdi>{o.label}</bdi>
                      </span>
                      {o.disabled !== undefined && (
                        <span className="seg-host">
                          <bdi>{o.disabled}</bdi>
                        </span>
                      )}
                    </button>
                  ))}
                  {choices.other && (
                    <button
                      type="button"
                      className={'seg-item' + (agent === OTHER ? ' sel' : '')}
                      aria-pressed={agent === OTHER}
                      onClick={() => setAgent(OTHER)}
                    >
                      <span className="seg-name">Other…</span>
                    </button>
                  )}
                </div>
              )}
              {agent === OTHER && (
                <div className="path-input">
                  <input
                    aria-label="Agent name"
                    value={otherName}
                    onChange={(e) => setOtherName(e.target.value)}
                    placeholder="The agent's name on the host"
                    spellCheck={false}
                  />
                </div>
              )}
            </div>

            <div className="field">
              <div className="field-label" id={ids.project}>
                <Icon.Folder size={16} /> Project <span className="hint">type a name, or a path</span>
              </div>
              <ProjectPicker
                key={hostId}
                hostId={hostId}
                entries={entries}
                browseRoot={browseRoot}
                initialText={initialText}
                onChange={setCwd}
                labelledBy={ids.project}
              />
              {projects !== null && projects.recents.length > 0 && (
                <div className="picker-path">
                  Recent projects of the hat <bdi>{hatName(projects.recents_hat_id)}</bdi>
                </div>
              )}
              {projects?.partial && <div className="picker-path">The host listed only some of its repositories.</div>}
              {projectsError !== null && (
                <div className="picker-path" role="alert">
                  Projects could not be listed: <bdi>{projectsError}</bdi> You can still type a path.
                </div>
              )}
              <HatPreview resolved={resolved} hatName={hatName} />
            </div>

            <div className="field">
              <label className="field-label" htmlFor={ids.prompt}>
                <Icon.Sparkle size={16} /> First prompt <span className="hint">optional</span>
              </label>
              <div className="prompt-box">
                <textarea
                  id={ids.prompt}
                  value={prompt}
                  onChange={(e) => setPrompt(e.target.value)}
                  placeholder={`What should ${agentName && agent !== OTHER ? agentLabel(agentName) : 'the agent'} work on?`}
                />
              </div>
            </div>
          </>
        )}
      </div>
      <div className="modal-foot">
        {error !== null && (
          <p className="form-error" role="alert">
            <bdi>{error}</bdi>
          </p>
        )}
        <div className="spacer" />
        <button type="submit" className="btn btn-primary" disabled={!canStart}>
          <Icon.Bolt size={16} /> {busy ? 'Starting…' : 'Start session'}
        </button>
      </div>
    </form>
  )
}

/** Where the session will land: its hat, before it starts. */
function HatPreview({ resolved, hatName }: { resolved: Resolved; hatName: (id: string) => string }) {
  if (resolved.state === 'idle') return null
  if (resolved.state === 'pending') {
    return (
      <p className="picker-path" role="status">
        Finding the hat…
      </p>
    )
  }
  if (resolved.state === 'failed') {
    return (
      <p className="form-error" role="alert">
        <bdi>{resolved.error}</bdi>
      </p>
    )
  }
  const { resolution } = resolved
  return (
    <p className="picker-path" role="status">
      Starts in the hat <b><bdi>{hatName(resolution.hat_id)}</bdi></b> at <bdi>{resolution.canonical}</bdi>
      {!resolution.exists ? ': this directory does not exist yet' : !resolution.is_dir ? ': this is not a directory' : ''}
    </p>
  )
}

/** A prompt that was not sent becomes the session's draft: the composer
 *  opens with it (lib/drafts.ts, the composer's own store). Whether there
 *  was one to keep; storage refused, the notice still says it was not sent. */
function keepDraft(sessionId: string, prompt: string): boolean {
  if (prompt.trim() === '') return false
  saveDraft(sessionId, prompt)
  return true
}
