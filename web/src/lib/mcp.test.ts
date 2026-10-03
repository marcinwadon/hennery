import { describe, expect, it } from 'vitest'
import { connection } from '../test-fixtures'
import {
  NEW_FORM,
  NOTE_LIMIT,
  capped,
  changeOf,
  createRequest,
  dropsToken,
  formOf,
  kindLabel,
  mountsWith,
  statusLabel,
  originOf,
  toolList,
} from './mcp'

describe('statusLabel', () => {
  it('words each state, never asking to reconnect on an error', () => {
    expect(statusLabel('not_connected')).toBe('Not used yet')
    expect(statusLabel('ok')).toBe('Working')
    expect(statusLabel('needs_auth')).toBe('Needs sign-in again')
    expect(statusLabel('error')).toBe('Is failing')
    expect(statusLabel('from_the_future')).toBe('Unknown state')
  })
})

describe('kindLabel', () => {
  it('names each kind, and a new one as it is', () => {
    expect(kindLabel('none')).toBe('None')
    expect(kindLabel('static')).toBe('Token')
    expect(kindLabel('oauth_dcr')).toBe('OAuth')
    expect(kindLabel('oauth_client')).toBe('OAuth')
    expect(kindLabel('from_the_future')).toBe('from_the_future')
  })
})

describe('capped', () => {
  it('keeps a short text and cuts a long one at 300 characters', () => {
    expect(capped('short')).toBe('short')
    const exact = 'x'.repeat(NOTE_LIMIT)
    expect(capped(exact)).toBe(exact)
    expect(capped('y'.repeat(NOTE_LIMIT + 1))).toBe('y'.repeat(NOTE_LIMIT) + '…')
  })

  it('counts characters, not UTF-16 units', () => {
    const astral = '\u{1F600}'.repeat(NOTE_LIMIT + 1)
    expect([...capped(astral)]).toHaveLength(NOTE_LIMIT + 1)
  })
})

describe('toolList', () => {
  it('splits on lines and commas, trims, drops blanks and repeats, keeps the order', () => {
    expect(toolList(' b ,a\n\n b,c ')).toEqual(['b', 'a', 'c'])
    expect(toolList('  \n ,')).toEqual([])
  })
})

describe('createRequest', () => {
  it('sends the header and prefix for a token connection only', () => {
    const form = { ...NEW_FORM, label: ' Docs ', url: ' https://x.example/mcp ' }
    expect(createRequest(form, ' docs ', 'hat-a')).toEqual({
      slug: 'docs',
      label: 'Docs',
      url: 'https://x.example/mcp',
      hat_id: 'hat-a',
      cred_kind: 'static',
      static_header: 'Authorization',
      static_prefix: 'Bearer ',
      tool_allowlist: null,
      internal_network: false,
    })
    const none = createRequest({ ...form, credKind: 'none' }, 'docs', 'hat-a')
    expect(none).not.toHaveProperty('static_header')
    expect(none).not.toHaveProperty('static_prefix')
  })

  it('sends a list of tools, or none, when not every tool', () => {
    expect(createRequest({ ...NEW_FORM, everyTool: false, tools: 'a, b' }, 's', 'h').tool_allowlist).toEqual(['a', 'b'])
    expect(createRequest({ ...NEW_FORM, everyTool: false, tools: '' }, 's', 'h').tool_allowlist).toEqual([])
  })
})

describe('changeOf', () => {
  it('sends nothing for an untouched form', () => {
    const item = connection({ tool_allowlist: ['a'], internal_network: true })
    expect(changeOf(item, formOf(item))).toEqual({})
  })

  it('sends only the label when only the label changed', () => {
    const item = connection()
    expect(changeOf(item, { ...formOf(item), label: ' New name ' })).toEqual({ label: 'New name' })
  })

  it('sends each field that changed', () => {
    const item = connection()
    const form = formOf(item)
    expect(changeOf(item, { ...form, url: 'https://other.example/mcp' })).toEqual({ url: 'https://other.example/mcp' })
    expect(changeOf(item, { ...form, header: 'X-Api-Key' })).toEqual({ static_header: 'X-Api-Key' })
    expect(changeOf(item, { ...form, prefix: '' })).toEqual({ static_prefix: '' })
    expect(changeOf(item, { ...form, internalNetwork: true })).toEqual({ internal_network: true })
    expect(changeOf(item, { ...form, credKind: 'none' })).toEqual({ cred_kind: 'none' })
  })

  it('leaves the header and prefix out of a change to no authentication', () => {
    const item = connection()
    expect(changeOf(item, { ...formOf(item), credKind: 'none', header: 'X', prefix: '' })).toEqual({ cred_kind: 'none' })
  })

  it('sends the allowlist as null, a list, or none', () => {
    const all = connection({ tool_allowlist: null })
    expect(changeOf(all, { ...formOf(all), everyTool: false, tools: 'a' })).toEqual({ tool_allowlist: ['a'] })
    expect(changeOf(all, { ...formOf(all), everyTool: false, tools: '' })).toEqual({ tool_allowlist: [] })
    const some = connection({ tool_allowlist: ['a', 'b'] })
    expect(changeOf(some, { ...formOf(some), everyTool: true })).toEqual({ tool_allowlist: null })
    expect(changeOf(some, { ...formOf(some), tools: 'b\na' })).toEqual({ tool_allowlist: ['b', 'a'] })
    const none = connection({ tool_allowlist: [] })
    expect(changeOf(none, { ...formOf(none), everyTool: true })).toEqual({ tool_allowlist: null })
  })
})

describe('dropsToken', () => {
  const item = connection({ url: 'https://mcp.example.com/mcp' })

  it('is true for another kind or another origin', () => {
    expect(dropsToken(item, { cred_kind: 'none' })).toBe(true)
    expect(dropsToken(item, { url: 'https://other.example.com/mcp' })).toBe(true)
    expect(dropsToken(item, { url: 'http://mcp.example.com/mcp' })).toBe(true)
    expect(dropsToken(item, { url: 'https://mcp.example.com:8443/mcp' })).toBe(true)
    // A stored URL that does not parse cannot be compared: it counts as
    // another origin, whatever is sent.
    expect(dropsToken(connection({ url: 'not a url' }), { url: 'not a url either' })).toBe(true)
  })

  it('is false for the same origin, or a change that names neither', () => {
    expect(dropsToken(item, { url: 'https://mcp.example.com/other?x=1' })).toBe(false)
    expect(dropsToken(item, { url: 'https://MCP.example.com:443/mcp' })).toBe(false)
    expect(dropsToken(item, { label: 'x', static_header: 'X' })).toBe(false)
    expect(dropsToken(item, { cred_kind: 'static' })).toBe(false)
    // The server refuses a URL that does not parse, and deletes nothing.
    expect(dropsToken(item, { url: 'not a url' })).toBe(false)
  })
})

describe('originOf', () => {
  it('is the scheme, host and port, or null', () => {
    expect(originOf('https://MCP.example.com:443/a?b#c')).toBe('https://mcp.example.com')
    expect(originOf('http://10.0.0.2:8080/mcp')).toBe('http://10.0.0.2:8080')
    expect(originOf('not a url')).toBeNull()
    expect(originOf('data:text/plain,x')).toBeNull()
  })
})

describe('mountsWith', () => {
  it('adds or removes one host, keeping the others, sorted', () => {
    expect(mountsWith(['host-b', 'host-z'], 'host-a', true)).toEqual(['host-a', 'host-b', 'host-z'])
    expect(mountsWith(['host-a', 'host-b'], 'host-a', false)).toEqual(['host-b'])
    expect(mountsWith(['host-a'], 'host-a', true)).toEqual(['host-a'])
    expect(mountsWith([], 'host-a', false)).toEqual([])
  })
})
