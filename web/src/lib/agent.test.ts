import { describe, expect, it } from 'vitest'
import { agentLabel, avatarLetter } from './agent'

describe('agentLabel', () => {
  it.each([
    ['claude', 'Claude'],
    ['codex', 'Codex'],
    ['gemini-cli', 'gemini-cli'],
    ['constructor', 'constructor'],
    [undefined, 'Agent'],
    ['', 'Agent'],
  ])('%s → %s', (agent, label) => {
    expect(agentLabel(agent)).toBe(label)
  })
})

describe('avatarLetter', () => {
  it('is the first letter of the label', () => {
    expect(avatarLetter('You')).toBe('Y')
    expect(avatarLetter('codex')).toBe('C')
    expect(avatarLetter('')).toBe('?')
  })
})
