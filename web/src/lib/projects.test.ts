import { describe, expect, it } from 'vitest'
import {
  childPath,
  disambiguate,
  filterProjects,
  isLiteralPath,
  mergeProjects,
  projectEntries,
  type ProjectEntry,
} from './projects'

const entry = (name: string): ProjectEntry => ({ name, path: '/srv/work/' + name })

describe('filterProjects', () => {
  it('returns everything for an empty or whitespace query', () => {
    const all = [entry('harbor'), entry('dotfiles')]
    expect(filterProjects(all, '')).toEqual(all)
    expect(filterProjects(all, '   ')).toEqual(all)
  })

  it('matches a prefix', () => {
    const got = filterProjects([entry('harbor'), entry('dotfiles')], 'dot')
    expect(got.map((e) => e.name)).toEqual(['dotfiles'])
  })

  it('ranks exact over prefix over substring over subsequence', () => {
    const got = filterProjects([entry('web-api-kit'), entry('kanban-pkit'), entry('api-gw'), entry('api')], 'api')
    expect(got.map((e) => e.name)).toEqual(['api', 'api-gw', 'web-api-kit', 'kanban-pkit'])
  })

  it('matches a subsequence, so wsh finds web-shell', () => {
    const got = filterProjects([entry('web-shell'), entry('harbor')], 'wsh')
    expect(got.map((e) => e.name)).toEqual(['web-shell'])
  })

  it('is case-insensitive and keeps the incoming order within one rank', () => {
    const got = filterProjects([entry('Ledger-infra'), entry('ledger-k8s')], 'LEDGER')
    expect(got.map((e) => e.name)).toEqual(['Ledger-infra', 'ledger-k8s'])
  })

  it('keeps the incoming order within the subsequence rank too', () => {
    const got = filterProjects([entry('a-x-b'), entry('ab-not'), entry('axb')], 'ab')
    // ab-not is a prefix (rank 1); a-x-b and axb are subsequences, in order.
    expect(got.map((e) => e.name)).toEqual(['ab-not', 'a-x-b', 'axb'])
  })

  it('drops non-matches', () => {
    expect(filterProjects([entry('harbor')], 'zzz')).toEqual([])
  })
})

describe('mergeProjects', () => {
  const e = (name: string, path: string, lastUsed?: string): ProjectEntry => ({
    name,
    path,
    ...(lastUsed === undefined ? {} : { lastUsed }),
  })

  it('puts recents first, then the rest alphabetically', () => {
    const got = mergeProjects(
      [e('dotfiles', '/srv/work/dotfiles', '2026-07-28T09:00:00Z')],
      [e('zed', '/srv/work/zed'), e('dotfiles', '/srv/work/dotfiles'), e('harbor', '/srv/work/harbor')],
    )
    expect(got.map((x) => x.name)).toEqual(['dotfiles', 'harbor', 'zed'])
  })

  it('dedupes by path so a recent is not repeated', () => {
    const got = mergeProjects(
      [e('harbor', '/srv/work/harbor', '2026-07-28T09:00:00Z')],
      [e('harbor', '/srv/work/harbor')],
    )
    expect(got).toHaveLength(1)
    expect(got[0].lastUsed).toBe('2026-07-28T09:00:00Z')
  })

  it('qualifies name collisions across the merged set', () => {
    const got = mergeProjects(
      [e('harbor', '/srv/work/alpha/harbor', '2026-07-28T09:00:00Z')],
      [e('harbor', '/srv/work/beta/harbor')],
    )
    expect(got.map((x) => x.name)).toEqual(['alpha/harbor', 'beta/harbor'])
  })

  it('returns just the recents when the enumeration is empty', () => {
    const recents = [e('harbor', '/srv/work/harbor', '2026-07-28T09:00:00Z')]
    expect(mergeProjects(recents, [])).toEqual(recents)
  })
})

describe('disambiguate', () => {
  it('leaves unique names bare', () => {
    const got = disambiguate([
      { name: 'app', path: '/srv/work/one/app' },
      { name: 'app', path: '/srv/work/two/app' },
      { name: 'notes', path: '/srv/work/notes' },
    ])
    expect(got.map((x) => x.name)).toEqual(['one/app', 'two/app', 'notes'])
  })
})

describe('projectEntries', () => {
  it('names recents and repositories by their directory', () => {
    const got = projectEntries({
      recents_hat_id: 'hat-a',
      recents: [{ path: '/srv/work/ledger', last_used_at: '2026-10-01T10:00:00Z' }],
      items: [{ path: '/srv/work/atlas' }, { path: '/srv/work/ledger' }],
      partial: false,
    })
    expect(got).toEqual([
      { name: 'ledger', path: '/srv/work/ledger', lastUsed: '2026-10-01T10:00:00Z' },
      { name: 'atlas', path: '/srv/work/atlas' },
    ])
  })
})

describe('isLiteralPath', () => {
  it.each([
    ['/srv/work/app', true],
    ['~/work/app', true],
    ['~', true],
    ['app', false],
    ['', false],
  ])('%s → %s', (text, literal) => {
    expect(isLiteralPath(text)).toBe(literal)
  })
})

describe('childPath', () => {
  it('joins with one slash, at the root too', () => {
    expect(childPath('/srv/work', 'app')).toBe('/srv/work/app')
    expect(childPath('/', 'srv')).toBe('/srv')
  })
})
