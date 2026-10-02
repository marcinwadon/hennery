import { describe, expect, it } from 'vitest'
import { answerAsMessage } from './composerWords'

describe('answerAsMessage', () => {
  it('ends a question without a stop of its own with one', () => {
    expect(answerAsMessage('Which branch')).toBe('You asked: Which branch. My answer: ')
  })

  it.each([
    ['Run it?', 'You asked: Run it? My answer: '],
    ['Pick a name.', 'You asked: Pick a name. My answer: '],
    ['Careful!', 'You asked: Careful! My answer: '],
    ['Run it?  ', 'You asked: Run it? My answer: '],
  ])('never doubles the stop of %j', (question, draft) => {
    expect(answerAsMessage(question)).toBe(draft)
  })
})
