// Which hosts link a skill: every host not revoked, ticked or not, each
// with what that host last reported. A tick saves at once, as the whole set;
// one more tick while a save runs is ignored. Above the list, a hat's hosts
// can be added to the set in one go.
import { useEffect, useRef, useState } from 'react'
import { messageOf } from '../api/errors'
import { hatHosts, setSkillHosts, skillHosts } from '../api/skills'
import { useClient } from '../app-client'
import type { HatItem, HostItem, SkillHostState } from '../generated/protocol'
import { useResource } from '../hooks/useResource'
import { Text, visible } from '../lib/text'

function statusWord(state: SkillHostState | undefined): string | null {
  if (!state) return null
  switch (state.status) {
    case 'applied':
    case 'pending':
    case 'unknown':
      return state.status
    case 'unsupported':
      return 'unsupported (host too old)'
    case 'offline':
      return 'host offline'
    case 'failed': {
      const f = state.failure
      return f ? `failed: ${f.reason}${f.agent ? ` (${f.agent})` : ''}` : 'failed'
    }
    default:
      return 'unknown'
  }
}

interface Props {
  skillId: string
  /** `null` while they load or when they could not be read. */
  hosts: HostItem[] | null
  hats: HatItem[] | null
  /** Changes when the skill's archive was replaced: the states are read
   *  again, since they were for the old content. */
  refresh?: number
  onChanged?(): void
}

export default function SkillHosts({ skillId, hosts, hats, refresh = 0, onChanged }: Props) {
  const client = useClient()
  const states = useResource(() => skillHosts(client, skillId), [skillId])
  const { reload } = states
  const seen = useRef(refresh)
  useEffect(() => {
    if (seen.current === refresh) return
    seen.current = refresh
    reload()
  }, [refresh, reload])
  // A ref, not state: two ticks in one frame both see the first.
  const running = useRef(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [hatId, setHatId] = useState('')
  const live = (hosts ?? []).filter((h) => !h.revoked_at)
  const revokedIds = new Set((hosts ?? []).filter((h) => h.revoked_at).map((h) => h.host_id))
  const current = states.data?.hosts ?? null
  // A host known to be revoked is not sent: its id is refused as
  // `unknown_host`, and would block every later save. A host absent from
  // the loaded list stays in the set, so a save never unassigns it unseen.
  const ticked = (current ?? []).map((s) => s.host_id).filter((h) => !revokedIds.has(h))

  const save = async (next: (ticked: string[]) => Promise<string[]> | string[]) => {
    if (running.current || current === null || hosts === null) return
    running.current = true
    setBusy(true)
    setError(null)
    try {
      const ids = await next(ticked)
      states.set(await setSkillHosts(client, skillId, ids))
      onChanged?.()
    } catch (err) {
      setError(messageOf(err))
    } finally {
      running.current = false
      setBusy(false)
    }
  }

  const toggle = (hostId: string, on: boolean) =>
    save((now) => (on ? [...now.filter((h) => h !== hostId), hostId] : now.filter((h) => h !== hostId)))
  const addHat = () => {
    if (hatId === '') return
    void save(async (now) => {
      const { host_ids } = await hatHosts(client, hatId)
      return [...now, ...host_ids.filter((h) => !revokedIds.has(h) && !now.includes(h))]
    })
  }

  return (
    <div className="skill-hosts">
      <div className="card-actions">
        <select
          aria-label="Hat"
          value={hatId}
          onChange={(e) => setHatId(e.target.value)}
          disabled={hats === null || current === null}
        >
          <option value="">Pick a hat</option>
          {(hats ?? []).filter((h) => !h.purging).map((h) => (
            <option key={h.id} value={h.id}>{visible(h.name)}</option>
          ))}
        </select>
        <button type="button" className="btn btn-ghost btn-sm" onClick={addHat} aria-disabled={hatId === '' || busy}>
          Select this hat's hosts
        </button>
      </div>
      <fieldset className="mounts" disabled={hosts === null || current === null} aria-busy={busy}>
        <legend>Hosts that link this skill</legend>
        {hosts !== null && live.length === 0 && <p className="hint">No host is paired yet.</p>}
        {live.map((host) => {
          const word = statusWord(current?.find((s) => s.host_id === host.host_id))
          return (
            <label key={host.host_id} className="check">
              <input
                type="checkbox"
                checked={ticked.includes(host.host_id)}
                onChange={(e) => void toggle(host.host_id, e.target.checked)}
              />
              <Text>{host.name}</Text>
              {word && <span className="hint">{' '}{word}</span>}
            </label>
          )
        })}
        {[states.error, error].map(
          (shown, i) =>
            shown && (
              <p key={i} className="form-error" role="alert">
                <Text>{shown}</Text>
              </p>
            ),
        )}
      </fieldset>
    </div>
  )
}
