import type { McpConnectionItem } from '../generated/protocol'
import { useState } from 'react'
import { useMcpOauth } from '../hooks/useMcpOauth'
import { callbackCommand, DEFAULT_LOCAL_CALLBACK, oauthAction, oauthGuidance } from '../lib/mcpOauth'
import { capped } from '../lib/mcp'
import { Text } from '../lib/text'

export default function OauthConnect({ connection: c, refresh, waiting }: {
  connection: McpConnectionItem; refresh: () => Promise<McpConnectionItem[]>; waiting?: (id: string, active: boolean) => void
}) {
  const { state, connect, cancel } = useMcpOauth(c, refresh, waiting)
  const [local, setLocal] = useState(false)
  const [redirect, setRedirect] = useState(DEFAULT_LOCAL_CALLBACK)
  const command = callbackCommand(c.oauth?.redirect_uri ?? '', redirect)
  const httpsLocal = command !== null && /^https:/i.test(redirect)
  const start = (resource?: string) => {
    if (local) { if (command) connect(resource, redirect) }
    else if (resource === undefined) connect()
    else connect(resource)
  }
  const action = oauthAction(c)
  const busy = state.phase === 'waiting' || state.phase === 'authorizing'
  const code = state.code ?? c.oauth_error?.code
  const mismatch = code === 'resource_mismatch' ? c.oauth?.resource_mismatch : undefined
  const retryButton = state.retry === 'accept' && mismatch ? 'Accept and connect' : action === 'reconnect' ? 'Reconnect' : action === 'connect' ? 'Connect' : mismatch ? 'Accept and connect' : null
  return <section className="oauth-connect" aria-label="OAuth sign-in">
    <label className="select-field"><span className="field-label">OAuth callback</span><select disabled={busy} value={local ? 'local' : 'collector'} onChange={e => setLocal(e.target.value === 'local')}>
      <option value="collector">Collector callback</option><option value="local">Localhost callback on this computer</option>
    </select></label>
    {local && <div className="notice">
      <label className="field"><span className="field-label">Local redirect URI</span><input className="text-input mono" value={redirect} maxLength={2048} disabled={busy} onChange={e => setRedirect(e.target.value)} autoComplete="off" spellCheck={false} /></label>
      <p>1. Choose the exact scheme, localhost host, port and path the vendor permits.</p>
      <p>2. Run this command on the computer running this browser. Keep the terminal open.</p>
      {command ? <pre style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }} aria-label="Local callback command">{command}</pre> : <p role="alert">Enter a valid http or https loopback callback. The collector address must be available.</p>}
      {httpsLocal && <p>3. <a href={redirect} target="_blank" rel="noopener noreferrer">Prepare HTTPS callback</a> in this browser profile. Accept the receiver’s temporary self-signed certificate for this local address and check that the page says the receiver is ready. Repeat after restarting the receiver.</p>}
      <p>{httpsLocal ? '4' : '3'}. Click Connect (or Reconnect).</p>
      <p className="hint">The receiver stops after 15 minutes; restart it if it times out. If the port is busy, stop the other receiver or choose another permitted port in both places. If sign-in cannot return, restart the receiver and Connect again. Stop waiting ends only the UI wait.</p>
      <p className="hint">The vendor must accept this exact redirect URI. A pre-registered client must already permit it; localhost does not bypass the vendor’s client policy. PKCE and tokens stay on the collector.</p>
    </div>}
    {(action === 'connect' || action === 'reconnect') && <button className="btn btn-primary btn-sm" type="button" disabled={local && !command} aria-disabled={busy} onClick={() => start()}>{action === 'connect' ? 'Connect' : 'Reconnect'}</button>}
    {action === 'client' && <p>Save the vendor client settings before connecting.</p>}
    {mismatch && <div className="notice">
      <p>Server URL: <Text className="mono">{c.url}</Text></p>
      <p>Advertised resource: <Text className="mono">{mismatch}</Text></p>
      <p>Accept only if this resource is the one you intend to authorize. Acceptance does not say which resource parameter was sent.</p>
      <button className="btn btn-primary btn-sm" type="button" disabled={local && !command} aria-disabled={busy} onClick={() => start(mismatch)}>Accept and connect</button>
    </div>}
    {busy && <p role="status">{state.phase === 'authorizing' ? 'Preparing sign-in…' : 'Waiting for sign-in. Return here after consent.'}</p>}
    {state.link && <p><a href={state.link} target="_blank" rel="noopener noreferrer">Open vendor sign-in</a></p>}
    {busy && <button type="button" className="btn btn-ghost btn-sm" onClick={cancel}>Stop waiting</button>}
    {state.phase === 'confirm' && <p role="status">Confirmed.{retryButton && <> Click {retryButton} again to open sign-in.</>}</p>}
    {state.phase === 'working' && <p role="status">The gateway now reports this connection as Working.</p>}
    {state.phase === 'unconfirmed' && <p role="status">Sign-in completion could not be confirmed. The card shows the gateway’s last report.</p>}
    {state.error && <p className="form-error" role="alert"><Text>{capped(state.error)}</Text></p>}
    {code && oauthGuidance(code) && <p className="hint"><Text>{oauthGuidance(code)}</Text></p>}
  </section>
}
