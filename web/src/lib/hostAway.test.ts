import { describe, expect, it } from 'vitest'
import type { Item } from '../generated/view'
import { hostAway, newestTs, type HostAwayInput } from './hostAway'

const T0 = '2026-10-02T10:00:00.000Z'
const at = (minute: number) => `2026-10-02T10:${String(minute).padStart(2, '0')}:00.000Z`
const SINCE = Date.parse(T0)

const marker = (kind: string, ts: string, id = `${kind}@${ts}`): Item => ({ id, version: 1, ts, kind: 'marker', marker: kind }) as Item
const msg = (id: string, ts: string): Item => ({ id, version: 1, ts, turn_id: 't1', kind: 'message', text: id }) as Item

const away = (patch: Partial<HostAwayInput>) => hostAway({ presumedParked: false, seed: true, items: [], since: SINCE, ...patch })

describe('hostAway', () => {
  describe('the seed, with no marker since the first page', () => {
    it('a host the hosts list said was connected is not away', () => {
      expect(away({ seed: true })).toBe(false)
    })

    it('a host the hosts list said was not connected is away', () => {
      expect(away({ seed: false })).toBe(true)
    })

    it('a host the hosts list did not name, or no list yet, is not away', () => {
      expect(away({ seed: undefined })).toBe(false)
    })
  })

  describe('presumed parked', () => {
    it('is away, whatever the seed says', () => {
      expect(away({ presumedParked: true, seed: true })).toBe(true)
      expect(away({ presumedParked: true, seed: undefined })).toBe(true)
    })

    it('is away even after a newer marker says the host is back', () => {
      expect(away({ presumedParked: true, items: [marker('host_back', at(5))] })).toBe(true)
    })

    it('off, the seed and the markers say it', () => {
      expect(away({ presumedParked: false, seed: true, items: [marker('host_offline', at(5))] })).toBe(true)
      expect(away({ presumedParked: false, seed: false, items: [marker('host_back', at(5))] })).toBe(false)
    })
  })

  describe('a marker since the first page overrules the seed', () => {
    it('host_offline: away', () => {
      expect(away({ seed: true, items: [marker('host_offline', at(5))] })).toBe(true)
    })

    it.each(['host_back', 'host_restarted', 'resumed'])('%s: back', (kind) => {
      expect(away({ seed: false, items: [marker(kind, at(5))] })).toBe(false)
    })

    it('a marker of another kind says nothing of the host', () => {
      expect(away({ seed: false, items: [marker('parked', at(5)), marker('closed', at(6))] })).toBe(true)
      expect(away({ seed: true, items: [marker('turn_failed', at(5))] })).toBe(false)
    })

    it('a marker whose time does not parse is not read', () => {
      expect(away({ seed: true, items: [marker('host_offline', 'soon')] })).toBe(false)
    })
  })

  describe('the newest marker wins', () => {
    it('offline, then back: back', () => {
      expect(away({ seed: true, items: [marker('host_offline', at(5)), marker('host_back', at(7))] })).toBe(false)
    })

    it('back, then offline: away', () => {
      expect(away({ seed: false, items: [marker('host_back', at(5)), marker('host_offline', at(7))] })).toBe(true)
    })

    it('by time, not by place: an older marker later in the items does not win', () => {
      expect(away({ seed: true, items: [marker('host_offline', at(7)), marker('host_back', at(5))] })).toBe(true)
    })

    it('a tie goes to the later item', () => {
      expect(away({ seed: true, items: [marker('host_back', at(5)), marker('host_offline', at(5))] })).toBe(true)
      expect(away({ seed: true, items: [marker('host_offline', at(5)), marker('host_back', at(5))] })).toBe(false)
    })
  })

  describe('the first page is history the seed already says', () => {
    it('a host_back in the first page does not hide a seed that says not connected', () => {
      expect(away({ seed: false, items: [marker('host_back', at(0)), msg('m', T0)] })).toBe(true)
    })

    it('a host_offline in the first page does not overrule a seed that says connected', () => {
      expect(away({ seed: true, items: [marker('host_offline', T0)] })).toBe(false)
    })

    it('an older page loaded later is history too: a live host_offline still wins', () => {
      const older = marker('host_back', '2026-10-02T09:00:00.000Z')
      expect(away({ seed: true, items: [older, msg('m', T0), marker('host_offline', at(5))] })).toBe(true)
    })

    it('a resync’s marker newer than the first page counts', () => {
      expect(away({ seed: true, items: [msg('m', T0), marker('host_offline', at(3))] })).toBe(true)
    })

    it('with no first-page time, every marker counts', () => {
      expect(away({ seed: false, since: -Infinity, items: [marker('host_back', '2020-01-01T00:00:00.000Z')] })).toBe(false)
    })
  })
})

describe('newestTs', () => {
  it('is the newest time the items hold, a time that does not parse left out', () => {
    expect(newestTs([msg('a', at(3)), msg('b', 'soon'), msg('c', at(1))])).toBe(Date.parse(at(3)))
  })

  it('is -Infinity for no items', () => {
    expect(newestTs([])).toBe(-Infinity)
  })
})
