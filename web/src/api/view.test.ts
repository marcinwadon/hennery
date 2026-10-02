import { describe, expect, it } from 'vitest'
import { json, routed } from '../test-stream'
import {
  anchorOf,
  catalog,
  itemPage,
  itemStreamPath,
  listStreamPath,
  sessionDetail,
  summaries,
  undeliveredTurn,
} from './view'

// Ids that would change a path's meaning if not encoded.
const ID = 'a/b?c#d'
const ENC = 'a%2Fb%3Fc%23d'
const TURN = 't/1%2'

function server() {
  return routed(() => json({}))
}

describe('the view client', () => {
  it('itemPage encodes the id and sends before_turn and limit', async () => {
    const s = server()
    await itemPage(s.client, ID)
    await itemPage(s.client, ID, { beforeTurn: TURN, limit: 8 })
    expect(s.calls.map((c) => c.path)).toEqual([
      `/api/view/sessions/${ENC}`,
      `/api/view/sessions/${ENC}?before_turn=t%2F1%252&limit=8`,
    ])
  })

  it('undeliveredTurn encodes both ids', async () => {
    const s = server()
    await undeliveredTurn(s.client, ID, TURN)
    expect(s.calls[0].path).toBe(`/api/view/sessions/${ENC}/turns/t%2F1%252`)
  })

  it('catalog and sessionDetail encode the id', async () => {
    const s = server()
    await catalog(s.client, ID)
    await sessionDetail(s.client, ID)
    expect(s.calls.map((c) => c.path)).toEqual([`/api/sessions/${ENC}/catalog`, `/api/sessions/${ENC}`])
  })

  it('the item stream path encodes the id', () => {
    expect(itemStreamPath(ID)).toBe(`/api/stream/view/sessions/${ENC}`)
  })

  it('the list stream path sends the hat encoded, and never an empty one', () => {
    expect(listStreamPath('h/1&x=2#y')).toBe('/api/stream/sessions?hat=h%2F1%26x%3D2%23y')
    for (const none of ['', null, undefined]) expect(listStreamPath(none)).toBe('/api/stream/sessions')
  })

  it('summaries sends what is set, and never an empty hat or search', async () => {
    const s = server()
    await summaries(s.client, { hat: '', q: '', lifecycle: [] })
    await summaries(s.client, { cursor: 'c 1', limit: 50, q: 'fix', hat: 'h/1', lifecycle: ['active', 'parked'] })
    expect(s.calls.map((c) => c.path)).toEqual([
      '/api/view/sessions',
      '/api/view/sessions?cursor=c+1&limit=50&q=fix&hat=h%2F1&lifecycle=active%2Cparked',
    ])
  })

  it('anchorOf is <epoch>:<revision>', () => {
    expect(anchorOf({ epoch: 'e1', revision: 42 })).toBe('e1:42')
  })
})
