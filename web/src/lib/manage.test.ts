import { describe, expect, it } from 'vitest'
import { hat, host, preview } from '../test-fixtures'
import { count, hostState, isDefault, purgeState, safeColour } from './manage'

describe('hostState', () => {
  it('is online when connected', () => {
    expect(hostState(host())).toBe('online')
  })

  it('is offline when not connected', () => {
    expect(hostState(host({ connected: false }))).toBe('offline')
  })

  it('is revoked when revoked, connected or not', () => {
    expect(hostState(host({ revoked_at: '2026-10-02T11:00:00Z' }))).toBe('revoked')
    expect(hostState(host({ connected: false, revoked_at: '2026-10-02T11:00:00Z' }))).toBe('revoked')
  })
})

describe('purgeState', () => {
  it('is default for the default for new hosts', () => {
    expect(purgeState(hat({ default_for_new_hosts: true }), [], preview())).toBe('default')
  })

  it('is default for a host’s default hat, a revoked host’s included', () => {
    const revoked = host({ default_hat_id: 'hat-b', revoked_at: '2026-10-02T11:00:00Z' })
    expect(purgeState(hat(), [revoked], preview())).toBe('default')
  })

  it('is running while a session may run, after the default check', () => {
    expect(purgeState(hat(), [host()], preview({ running: ['s-1'] }))).toBe('running')
    expect(purgeState(hat({ default_for_new_hosts: true }), [], preview({ running: ['s-1'] }))).toBe('default')
  })

  it('is resume for a hat whose purge began', () => {
    expect(purgeState(hat({ purging: true }), [host()], preview({ purging: true }))).toBe('resume')
  })

  it('is ready otherwise', () => {
    expect(purgeState(hat(), [host()], preview())).toBe('ready')
  })
})

describe('isDefault', () => {
  it('is false for a hat no host and no setting names', () => {
    expect(isDefault(hat(), [host()])).toBe(false)
  })
})

describe('safeColour', () => {
  it('takes a lowercase #rrggbb', () => {
    expect(safeColour('#0a1b2c')).toBe('#0a1b2c')
  })

  it.each(['#0A1B2C', 'red', '#abc', '#0a1b2c;background:url(x)', 'url(x)', ''])('refuses %j', (value) => {
    expect(safeColour(value)).toBeUndefined()
  })
})

describe('count', () => {
  it('says one and many', () => {
    expect(count(1, 'session')).toBe('1 session')
    expect(count(0, 'session')).toBe('0 sessions')
    expect(count(2, 'entry', 'entries')).toBe('2 entries')
  })
})
