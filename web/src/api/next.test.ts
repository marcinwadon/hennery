import { describe, expect, it } from 'vitest'
import { HOME, loginHref, safeNext } from './next'

const ORIGIN = 'https://hennery.example'

describe('safeNext', () => {
  it('keeps a path of this origin, with its query', () => {
    expect(safeNext('/hosts', ORIGIN)).toBe('/hosts')
    expect(safeNext('/sessions/s-1?tab=x', ORIGIN)).toBe('/sessions/s-1?tab=x')
  })

  it('drops the fragment', () => {
    expect(safeNext('/hosts#secret', ORIGIN)).toBe('/hosts')
  })

  it.each([
    ['nothing', null],
    ['empty', ''],
    ['protocol-relative', '//evil.example/x'],
    ['backslash host', '/\\evil.example'],
    ['backslash first', '\\/evil.example'],
    ['absolute URL', 'https://evil.example/'],
    ['same origin, absolute', 'https://hennery.example/hosts'],
    ['javascript', 'javascript:alert(1)'],
    ['data', 'data:text/html,x'],
    ['dot segments to a double slash', '/..//evil.example'],
    ['dot to a double slash', '/.//evil.example'],
    ['encoded dots to a double slash', '/%2e%2e//evil.example'],
    ['tab inside', '/\t/evil.example'],
    ['newline inside', '/\n/evil.example'],
    ['relative', 'hosts'],
  ])('refuses %s', (_, raw) => {
    expect(safeNext(raw, ORIGIN)).toBe(HOME)
  })

  it('never returns to a sign-in page', () => {
    expect(safeNext('/login', ORIGIN)).toBe(HOME)
    expect(safeNext('/login?next=/login', ORIGIN)).toBe(HOME)
    expect(safeNext('/setup', ORIGIN)).toBe(HOME)
    expect(safeNext('/setup/x', ORIGIN)).toBe(HOME)
  })

  it('keeps an encoded slash inside a segment, which stays on this origin', () => {
    expect(safeNext('/%2F%2Fevil.example', ORIGIN)).toBe('/%2F%2Fevil.example')
  })
})

describe('loginHref', () => {
  it('encodes the return target', () => {
    expect(loginHref('/sessions/s-1?a=b&c=d')).toBe('/login?next=%2Fsessions%2Fs-1%3Fa%3Db%26c%3Dd')
  })
})
