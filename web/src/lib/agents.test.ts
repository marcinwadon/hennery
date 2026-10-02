import { describe, expect, it, vi } from 'vitest'
import { Client } from '../api/client'
import { agentOptions, agentsFor, takesImages, type AgentInfo } from './agents'

const info = (agent: string, patch: Partial<AgentInfo> = {}): AgentInfo => ({
  agent,
  available: true,
  auth: 'unknown',
  cli: 'bundled',
  ...patch,
})

describe('agentsFor', () => {
  it('offers claude and codex, unchecked, and a name typed by hand, without asking the server', async () => {
    const fetch = vi.fn()
    const client = new Client({
      fetch: fetch as unknown as typeof globalThis.fetch,
      navigate: vi.fn(),
      here: () => ({ pathname: '/new', search: '' }),
      stepUp: vi.fn(),
    })
    const got = await agentsFor(client, 'h1')
    expect(got.agents.map((a) => [a.agent, a.available, a.auth])).toEqual([
      ['claude', true, 'unknown'],
      ['codex', true, 'unknown'],
    ])
    expect(got.other).toBe(true)
    expect(fetch).not.toHaveBeenCalled()
  })
})

describe('agentOptions', () => {
  it('lists only available agents', () => {
    expect(agentOptions([info('claude'), info('codex', { available: false })]).map((o) => o.agent)).toEqual([
      'claude',
    ])
  })

  it('disables an agent whose auth is missing, with its note', () => {
    const [opt] = agentOptions([info('codex', { auth: 'missing', note: 'Run `codex login` on the host.' })])
    expect(opt.disabled).toBe('Run `codex login` on the host.')
  })

  it('disables an agent whose auth is missing even without a note', () => {
    const [opt] = agentOptions([info('codex', { auth: 'missing' })])
    expect(opt.disabled).toBe('Not signed in on this host.')
  })

  it('allows an agent whose auth is unknown or ok', () => {
    const opts = agentOptions([info('claude', { auth: 'unknown' }), info('codex', { auth: 'ok' })])
    expect(opts.map((o) => o.disabled)).toEqual([undefined, undefined])
  })
})

describe('takesImages', () => {
  it('hides images only for an agent known to take none', () => {
    expect(takesImages({ images: false })).toBe(false)
    expect(takesImages({ images: true })).toBe(true)
    expect(takesImages({})).toBe(true)
  })
})
