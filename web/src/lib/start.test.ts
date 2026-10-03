import { describe, expect, it } from 'vitest'
import { ApiFailure } from '../api/errors'
import { newSessionHref, readNewSessionPrefill, readStartNotice, sessionHref, startNoticeText, startRefusal } from './start'

const refused = (status: number, code: string, message: string) => new ApiFailure(status, { code, message })

describe('startRefusal', () => {
  it('words hat_ambiguous with the rule the server names', () => {
    const text = startRefusal(refused(409, 'hat_ambiguous', 'the rule for "/srv/Work" matches only in another case'))
    expect(text).toContain('more than one hat')
    expect(text).toContain('the rule for "/srv/Work"')
  })

  it('words host_offline in the server words, which say a new host installs first', () => {
    const text = startRefusal(refused(409, 'host_offline', 'the host has not connected since it was paired'))
    expect(text).toBe('the host has not connected since it was paired')
  })

  it('words invalid_cwd with the path the server names', () => {
    const text = startRefusal(refused(400, 'invalid_cwd', '"/srv/work/x" is not a directory on that host'))
    expect(text).toBe('"/srv/work/x" is not a directory on that host')
  })

  it('words unknown_host as a host no longer paired', () => {
    expect(startRefusal(refused(400, 'unknown_host', 'no host is paired with that id'))).toBe(
      'That host is no longer paired. Pick another host.',
    )
  })

  it('words a 502 as the host refusing, with its code', () => {
    const text = startRefusal(refused(502, 'agent_not_logged_in', 'log in first'))
    expect(text).toBe('The host refused to start the session (agent_not_logged_in): log in first')
  })

  it('words delivery_unknown without a session', () => {
    expect(startRefusal(refused(503, 'delivery_unknown', 'gone'))).toContain('its outcome is not known')
  })

  it('falls back to the error message for anything else', () => {
    expect(startRefusal(refused(503, 'host_jammed', 'the host is busy; try again'))).toBe('the host is busy; try again')
    expect(startRefusal(new Error('offline'))).toBe('offline')
  })
})

describe('sessionHref', () => {
  it('encodes the id and carries only a code', () => {
    expect(sessionHref('a/b')).toBe('/sessions/a%2Fb')
    expect(sessionHref('s1', { kind: 'start_unknown', kept: false })).toBe('/sessions/s1?notice=start_unknown')
    expect(sessionHref('s1', { kind: 'start_unknown', kept: true })).toBe('/sessions/s1?notice=start_unknown_draft')
    expect(sessionHref('s1', { kind: 'prompt_failed', code: 'host_offline' })).toBe(
      '/sessions/s1?notice=prompt_failed&code=host_offline',
    )
  })
})

describe('readStartNotice', () => {
  it('reads a start whose delivery is unknown, and mentions no draft when no first prompt was typed', () => {
    const notice = readStartNotice('?notice=start_unknown')
    expect(notice).toEqual({ kind: 'start_unknown', kept: false })
    expect(startNoticeText(notice!)).toBe(
      'The host went away while this session was starting: it may still start, and shows here when the host reconnects.',
    )
  })

  it('reads a start whose delivery is unknown with its first prompt kept as the draft', () => {
    const notice = readStartNotice('?notice=start_unknown_draft')
    expect(notice).toEqual({ kind: 'start_unknown', kept: true })
    expect(startNoticeText(notice!)).toBe(
      'The host went away while this session was starting: it may still start, and shows here when the host reconnects. A first prompt was not sent: it waits as this session’s draft.',
    )
  })

  it.each([
    ['not_attached', 'The session was no longer running when the first prompt arrived; resume it and send it again.'],
    ['turn_in_progress', 'A turn was already running; send the first prompt again once it ends.'],
    ['delivery_unknown', 'Delivery of the first prompt is unknown: the outcome shows when the host reconnects.'],
    ['content_too_large', 'The first prompt was too large to send.'],
    ['body_too_large', 'The first prompt was too large to send.'],
  ])('words a first prompt refused with %s', (code, words) => {
    const notice = readStartNotice(`?notice=prompt_failed&code=${code}`)
    expect(startNoticeText(notice!)).toBe(`${words} It waits as this session’s draft.`)
  })

  it('words a refused first prompt by its code', () => {
    const notice = readStartNotice('?notice=prompt_failed&code=host_offline')
    expect(notice?.kind).toBe('prompt_failed')
    expect(startNoticeText(notice!)).toContain('went offline before the first prompt')
    expect(startNoticeText(notice!)).toContain('draft')
  })

  it('words a code it does not know by the code, never by a message in the link', () => {
    const notice = readStartNotice('?notice=prompt_failed&code=brand_new&message=evil')
    expect(startNoticeText(notice!)).toBe('The first prompt was not sent (brand_new). It waits as this session’s draft.')
  })

  it('shows no free text from the link, and no inherited key', () => {
    const odd = readStartNotice('?notice=prompt_failed&code=' + encodeURIComponent('Call this number now'))
    expect(startNoticeText(odd!)).toBe('The first prompt was not sent (unknown). It waits as this session’s draft.')
    const inherited = readStartNotice('?notice=prompt_failed&code=constructor')
    expect(startNoticeText(inherited!)).toBe('The first prompt was not sent (constructor). It waits as this session’s draft.')
  })

  it('reads no notice from anything else', () => {
    expect(readStartNotice('')).toBeNull()
    expect(readStartNotice('?notice=other')).toBeNull()
  })
})

describe('newSessionHref', () => {
  it('names the host and the project, both encoded, and reads them back', () => {
    const href = newSessionHref('h/1&x', '/srv/work/a b&cwd=x#y')
    expect(href).toBe('/new?host=h%2F1%26x&cwd=%2Fsrv%2Fwork%2Fa+b%26cwd%3Dx%23y')
    expect(readNewSessionPrefill(href.slice('/new'.length))).toEqual({ host: 'h/1&x', cwd: '/srv/work/a b&cwd=x#y' })
  })

  it('reads nothing a link does not name', () => {
    expect(readNewSessionPrefill('')).toEqual({ host: undefined, cwd: undefined })
  })
})
