// The footer of a session that is not running (frontend spec §6.6), under
// its composer. The composer stays: a draft put there ("Answer as a new
// message", "Send again") is sent once the session runs again, and a prompt
// refused as `not_attached` offers its own "Resume and send".
//
// - Parked or closed: "Resume", with "the host is offline" while the host
//   is away (lib/hostAway.ts: the session is presumed parked, or the newest
//   host marker, else the view's hosts list, says the host is not there).
// - Starting: a spinner, in words.
// - Failed: the reason in words. A session the agent has no record of
//   cannot be resumed: it offers "Start a new session in this project"
//   (New Session with its host and directory filled in). One whose agent is
//   not logged in says how to log it in on the host, then offers Resume.
// - A refused resume says why, in the footer's own words. `hat_mismatch`
//   names the session's hat and the hat its directory now belongs to (the
//   hats' names from `GET /api/hats`, else their ids), and links to Hats.
//
// Mount it keyed by the lifecycle: a refusal belongs to the state it was
// made in.
import { useEffect, useState } from 'react'
import { ApiFailure } from '../api/errors'
import { hats as fetchHats, resolveHat } from '../api/manage'
import { namesOf } from '../api/names'
import { useClient } from '../app-client'
import { newSessionHref } from '../lib/start'
import { PATH } from '../lib/views'
import { Link } from '../router'
import type { HeaderInfo } from './SessionHeader'
import { failureWords, loginSteps, resumeRefusal } from './sessionWords'

/** Why a session failed, as a sentence. A reason this client does not know
 *  is shown as sent, in a <bdi>: it is the server's text, and its direction
 *  is its own. */
function FailureText({ reason }: { reason: string | undefined }) {
  if (!reason) return <>This session failed.</>
  const words = failureWords(reason)
  return <>This session failed: {words ?? <bdi>{reason}</bdi>}.</>
}

interface Props {
  info: HeaderInfo
  /** The host's name, when known. */
  hostName?: string
  /** Whether the host is away: the view's answer book says it, as it says
   *  it to the question cards. */
  hostAway?: boolean
  /** `POST …/resume`; rejects with the refusal. */
  onResume: () => Promise<unknown>
}

export default function SessionFooter({ info, hostName, hostAway = false, onResume }: Props) {
  const [busy, setBusy] = useState(false)
  const [refusal, setRefusal] = useState<unknown>(null)
  const lifecycle = info.lifecycle

  if (lifecycle === 'starting') {
    return (
      <div className="resume-bar session-footer" role="status">
        <span className="spinner" aria-hidden="true" />
        <span className="txt">The session is starting…</span>
      </div>
    )
  }
  if (lifecycle !== 'parked' && lifecycle !== 'closed' && lifecycle !== 'failed') return null

  const reason = lifecycle === 'failed' ? info.failure_reason : undefined
  const noRecord = reason === 'agent_has_no_record'
  const host = hostName ?? 'its host'

  const resume = async () => {
    setBusy(true)
    setRefusal(null)
    try {
      await onResume()
    } catch (err) {
      setRefusal(err)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="resume-bar session-footer">
      <div className="session-footer-text">
        <p className="txt">
          {lifecycle === 'parked'
            ? 'This session is parked.'
            : lifecycle === 'closed'
              ? 'This session is closed.'
              : <FailureText reason={reason} />}
        </p>
        {hostAway && (
          <p className="txt session-footer-offline">
            {info.presumed_parked
              ? 'The host is offline: it has been away, and may still be running this session.'
              : 'The host is offline: resume once it is back.'}
          </p>
        )}
        {reason === 'agent_not_logged_in' && <LoginSteps agent={info.agent} host={host} />}
        {refusal !== null && (
          <div className="form-error" role="alert">
            <Refusal err={refusal} info={info} host={host} />
          </div>
        )}
      </div>
      {noRecord ? (
        <Link to={newSessionHref(info.host_id, info.cwd)} className="btn btn-primary btn-sm">
          Start a new session in this project
        </Link>
      ) : (
        <button type="button" className="btn btn-primary btn-sm" disabled={busy} onClick={() => void resume()}>
          {busy ? 'Resuming…' : 'Resume'}
        </button>
      )}
    </div>
  )
}

function LoginSteps({ agent, host }: { agent: string; host: string }) {
  return (
    <p className="txt">
      The agent is not logged in on <bdi>{host}</bdi>. {loginSteps(agent)}
    </p>
  )
}

/** A refused resume, in words. */
function Refusal({ err, info, host }: { err: unknown; info: HeaderInfo; host: string }) {
  const code = err instanceof ApiFailure ? err.code : null
  if (code === 'hat_mismatch') return <HatMismatch info={info} />
  if (code === 'agent_has_no_record') {
    return (
      <p>
        {resumeRefusal(err)}{' '}
        <Link to={newSessionHref(info.host_id, info.cwd)}>Start a new session in this project</Link>
      </p>
    )
  }
  if (code === 'agent_not_logged_in') return <LoginSteps agent={info.agent} host={host} />
  return (
    <p>
      <bdi>{resumeRefusal(err)}</bdi>
    </p>
  )
}

/** The session's hat and the one its directory now resolves to, by name. */
function HatMismatch({ info }: { info: HeaderInfo }) {
  const client = useClient()
  const [names, setNames] = useState<Map<string, string>>(() => new Map())
  /** The hat the directory resolves to now; null while unknown. */
  const [now, setNow] = useState<string | null>(null)
  const { host_id: host, cwd } = info

  useEffect(() => {
    let live = true
    fetchHats(client).then(
      (list) => live && setNames(namesOf(list, 'id')),
      () => {},
    )
    resolveHat(client, host, cwd).then(
      (r) => live && typeof r?.hat_id === 'string' && setNow(r.hat_id),
      () => {},
    )
    return () => {
      live = false
    }
  }, [client, host, cwd])

  const hatName = (id: string) => (id === '' ? 'no hat' : (names.get(id) ?? id))
  return (
    <p>
      This session belongs to <bdi>{hatName(info.hat_id)}</bdi>, but its directory now belongs to{' '}
      {now === null ? 'another hat' : <bdi>{hatName(now)}</bdi>}. Change the path rules in <Link to={PATH.hats}>Hats</Link>,
      then resume.
    </p>
  )
}
