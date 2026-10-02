import { afterEach, describe, expect, it, vi } from 'vitest'
import { draftKey, loadDraft, saveDraft } from './drafts'

afterEach(() => {
  vi.restoreAllMocks()
  sessionStorage.clear()
})

describe('drafts', () => {
  it('keeps a draft per session in sessionStorage under hennery.draft.<id>', () => {
    expect(draftKey('s1')).toBe('hennery.draft.s1')
    saveDraft('s1', 'one')
    saveDraft('s2', 'two')
    expect(sessionStorage.getItem('hennery.draft.s1')).toBe('one')
    expect(loadDraft('s1')).toBe('one')
    expect(loadDraft('s2')).toBe('two')
    expect(loadDraft('s3')).toBe('')
  })

  it('removes an emptied draft', () => {
    saveDraft('s1', 'one')
    saveDraft('s1', '')
    expect(sessionStorage.getItem('hennery.draft.s1')).toBeNull()
  })

  it('lives in the page only when storage refuses', () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('quota')
    })
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('denied')
    })
    expect(() => saveDraft('s1', 'x')).not.toThrow()
    expect(loadDraft('s1')).toBe('')
  })
})
