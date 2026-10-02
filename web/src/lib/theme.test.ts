import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { THEME_KEY, saveTheme } from './theme'

/** `/theme.js` as the page runs it, before anything else. */
function runBootstrap() {
  const source = readFileSync(join(process.cwd(), 'public/theme.js'), 'utf8')
  new Function(source)()
}

const root = () => document.documentElement.style

afterEach(() => {
  localStorage.clear()
  root().removeProperty('--accent')
  root().removeProperty('--accent-2')
})

describe('the theme bootstrap', () => {
  it('applies the saved colours', () => {
    localStorage.setItem(THEME_KEY, JSON.stringify({ '--accent': '#112233', '--accent-2': '#445566' }))
    runBootstrap()
    expect(root().getPropertyValue('--accent')).toBe('#112233')
    expect(root().getPropertyValue('--accent-2')).toBe('#445566')
  })

  it('applies only the listed properties, only as #rrggbb', () => {
    localStorage.setItem(
      THEME_KEY,
      JSON.stringify({
        '--accent': 'red; background:url(https://evil.example/x)',
        '--accent-2': '#12345',
        '--bg': '#000000',
        color: '#000000',
      }),
    )
    runBootstrap()
    expect(root().getPropertyValue('--accent')).toBe('')
    expect(root().getPropertyValue('--accent-2')).toBe('')
    expect(root().getPropertyValue('--bg')).toBe('')
    expect(root().cssText).toBe('')
  })

  it('survives nothing saved, or something damaged', () => {
    runBootstrap()
    for (const value of ['not json', 'null', '"#112233"', '[]']) {
      localStorage.setItem(THEME_KEY, value)
      expect(runBootstrap).not.toThrow()
    }
    expect(root().cssText).toBe('')
  })

  it('reads what the app saves', () => {
    saveTheme({ accent: '#0a0b0c', accent2: '#0d0e0f' })
    root().removeProperty('--accent')
    root().removeProperty('--accent-2')
    runBootstrap()
    expect(root().getPropertyValue('--accent')).toBe('#0a0b0c')
    expect(root().getPropertyValue('--accent-2')).toBe('#0d0e0f')
  })
})

describe('saveTheme', () => {
  it('refuses a colour that is not #rrggbb', () => {
    expect(() => saveTheme({ accent: 'red', accent2: '#000000' })).toThrow()
    expect(localStorage.getItem(THEME_KEY)).toBeNull()
  })

  it('forgets the theme', () => {
    saveTheme({ accent: '#0a0b0c', accent2: '#0d0e0f' })
    saveTheme(null)
    expect(localStorage.getItem(THEME_KEY)).toBeNull()
    expect(root().getPropertyValue('--accent')).toBe('')
  })
})
