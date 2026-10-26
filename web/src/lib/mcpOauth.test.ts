import { describe, expect, it } from 'vitest'
import { connection } from '../test-fixtures'
import { callbackCommand, clientRequest, completion, consentTimeout, consentUrl, localCallback, oauthAction, oauthGuidance, oauthHint } from './mcpOauth'

const item = connection({ cred_kind: 'oauth_dcr', status: 'ok', has_credential: true })
describe('local callbacks', () => {
  it.each(['http://localhost:8976/callback', 'https://127.0.0.1:9882/callback', 'https://localhost:443/callback', 'https://[::1]:9882/callback'])('keeps the exact permitted URI %s', uri => {
    expect(localCallback(uri)).toBe(true)
    expect(callbackCommand('https://collector.example/api/mcp/oauth/callback', uri)).toContain(`--redirect-uri '${uri}'`)
  })
  it.each(['https://localhost.evil/callback', 'https://192.168.1.1/callback', 'https://0.0.0.0/callback', 'https://localhost:0/callback', 'https://localhost/callback?code=x', 'https://localhost/callback#x', 'https://user:secret@localhost/callback', 'ftp://localhost/callback'])('refuses %s', uri => {
    expect(localCallback(uri)).toBe(false)
    expect(callbackCommand('https://collector.example/api/mcp/oauth/callback', uri)).toBeNull()
  })
})
describe('consent destinations', () => {
  it.each(['https://vendor.example/consent?state=x', 'http://localhost:123/a', 'http://localhost.:123/a', 'http://127.1.2.3:123/a', 'http://[::1]:123/a'])('takes %s', url => expect(consentUrl(url)).toBe(new URL(url).href))
  it.each(['http://vendor.example/a', 'http://localhost.evil/a', 'http://evil.localhost/a', 'http://192.168.1.2/a', 'javascript:alert(1)', '//vendor.example/a', 'bad', 'https://user:secret@vendor.example/a', 'http://[::2]/a'])('refuses %s', url => expect(consentUrl(url)).toBeNull())
})
describe('callback hints', () => {
  const data = { type: 'done', connection_id: item.id, outcome: 'connected', result: 'connected' }
  const event = (over: object = {}, origin = 'https://hennery.example') => new MessageEvent('message', { origin, data: { ...data, ...over } })
  it('takes only the matching return-origin hint', () => expect(oauthHint(event(), 'https://hennery.example', item.id)).toBe(true))
  it.each([{ type: 'other' }, { connection_id: 'other' }, { connection_id: '' }, { outcome: 'other' }, { result: 3 }])('refuses malformed or unrelated %j', over => expect(oauthHint(event(over), 'https://hennery.example', item.id)).toBe(false))
  it('refuses another origin and port', () => expect(oauthHint(event({}, 'https://hennery.example:444'), 'https://hennery.example', item.id)).toBe(false))
  it('refuses non-object data', () => expect(oauthHint(new MessageEvent('message', { origin: 'https://hennery.example', data: null }), 'https://hennery.example', item.id)).toBe(false))
})
describe('server-observed completion', () => {
  it('reports working on a newer ok stamp, without claiming consent', () => expect(completion(item, { ...item, status_at: '2026-10-03T10:00:01Z' })).toBe('working'))
  it('can observe recovered upstream use while consent is still pending', () => expect(completion({ ...item, status: 'needs_auth' }, { ...item, status_at: '2026-10-03T10:00:01Z' })).toBe('working'))
  it('is failed on a new server error', () => expect(completion(item, { ...item, oauth_error: { code: 'consent_denied', message: 'denied', at: item.status_at } })).toBe('failed'))
  it('is removed when the item went', () => expect(completion(item, undefined)).toBe('removed'))
  it.each([item, { ...item, status: 'error' as const, status_at: '2026-10-03T10:00:01Z' }, { ...item, status_at: 'invalid' }])('leaves identical, failing or unparseable reports pending', after => expect(completion(item, after)).toBe('pending'))
  it('does not mistake an identical old error for a new one', () => { const before = { ...item, oauth_error: { code: 'x', message: 'x', at: item.status_at } }; expect(completion(before, before)).toBe('pending') })
})
describe('local consent timeout', () => {
  it('uses server duration without the client clock', () => expect(consentTimeout('2026-10-13T10:02:00Z', Date.parse('2026-10-13T10:00:00Z'))).toBe(120000))
  it('uses the bounded flow lifetime when server time is absent', () => expect(consentTimeout('2000-01-01T00:00:00Z')).toBe(900000))
  it('caps excessive server durations', () => expect(consentTimeout('2027-10-13T10:00:00Z', Date.parse('2026-10-13T10:00:00Z'))).toBe(900000))
  it('never refuses an already elapsed server duration', () => expect(consentTimeout('2026-10-13T09:00:00Z', Date.parse('2026-10-13T10:00:00Z'))).toBe(1000))
  it('uses the bounded lifetime for unreadable stamps', () => expect(consentTimeout('bad', Number.NaN)).toBe(900000))
})
describe('client secret intent', () => {
  it('keeps only the same stored id', () => expect(clientRequest('id', 'id', 'keep', '')).toEqual({ client_id: 'id' }))
  it('explicitly selects public', () => expect(clientRequest(undefined, 'id', 'public', '')).toEqual({ client_id: 'id', client_secret: null }))
  it('sets a new secret', () => expect(clientRequest('old', 'new', 'replace', 'secret')).toEqual({ client_id: 'new', client_secret: 'secret' }))
  it.each([['old', 'new', 'keep', ''], [undefined, 'new', 'keep', ''], ['id', '', 'public', ''], ['id', 'id', 'replace', '']] as const)('refuses incomplete intent %j', (stored, id, mode, secret) => expect(clientRequest(stored, id, mode, secret)).toBeNull())
})
describe('OAuth action', () => {
  it.each([
    [connection(), 'none'], [connection({ cred_kind: 'oauth_client' }), 'client'],
    [connection({ cred_kind: 'oauth_dcr' }), 'connect'], [item, 'reconnect'],
    [{ ...item, status: 'needs_auth' }, 'reconnect'], [{ ...item, status: 'error' }, 'probe'],
  ] as const)('uses the reported kind/state', (c, action) => expect(oauthAction(c)).toBe(action))
})
describe('refusal guidance', () => {
  it.each(['issuer_changed', 'resource_mismatch', 'resource_foreign', 'no_oauth_client', 'no_registration_endpoint', 'too_many_flows', 'pkce_unsupported', 'insecure_metadata', 'egress_refused', 'upstream_unreachable', 'wrong_cred_kind', 'no_credential'])('explains %s', code => expect(oauthGuidance(code)).not.toBe(''))
  it('does not classify vendor wording or unknown codes', () => expect(oauthGuidance('vendor says invalid_target')).toBe(''))
})

it.each(['constructor', '__proto__', 'toString'])('does not inherit guidance for %s', code => expect(oauthGuidance(code)).toBe(''))
it.each(['http://127.1/a', 'http://2130706433/a', 'http://%6cocalhost/a', 'HTTP://LOCALHOST:80/a'])('uses URL-decoded destination %s', url => expect(consentUrl(url)).toBe(new URL(url).href))
it('takes failed hints and the bounded result, rejecting oversized and primitive envelopes', () => {
  const origin = 'https://hennery.example'
  const data = { type: 'done', connection_id: item.id, outcome: 'failed', result: 'x'.repeat(128) }
  const hint = (value: unknown) => oauthHint(new MessageEvent('message', { origin, data: value }), origin, item.id)
  expect(hint(data)).toBe(true); expect(hint({ ...data, result: 'x'.repeat(129) })).toBe(false)
  expect(hint('done')).toBe(false); expect(hint(1)).toBe(false)
})
