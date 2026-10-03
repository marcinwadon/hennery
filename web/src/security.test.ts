// Guards over the sources (kernel spec §7.2, frontend spec §6.4): nothing
// here writes HTML from a string or runs a string as code, so a server's
// or an agent's text can only ever render as text.
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { HIDDEN } from './lib/text'

// Vitest runs from `web/`.
const SRC = join(process.cwd(), 'src')

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name)
    if (statSync(path).isDirectory()) return sources(path)
    return /\.(ts|tsx)$/.test(name) && !/\.test\.(ts|tsx)$/.test(name) && name !== 'test-server.ts' ? [path] : []
  })
}

const FORBIDDEN = [
  'dangerouslySetInnerHTML',
  'innerHTML',
  'outerHTML',
  'insertAdjacentHTML',
  'document.write',
  'srcdoc',
  'eval(',
  'new Function',
  "<style",
]

describe('the sources', () => {
  const files = sources(SRC)

  it('are all read', () => {
    expect(files.length).toBeGreaterThan(15)
  })

  it.each(FORBIDDEN)('never use %s', (word) => {
    const found = files.filter((f) => readFileSync(f, 'utf8').includes(word))
    expect(found).toEqual([])
  })

  it('hold no character `visible()` would escape, but tab and newlines: tests write them as escapes', () => {
    const all = (dir: string): string[] =>
      readdirSync(dir).flatMap((name) => {
        const path = join(dir, name)
        if (statSync(path).isDirectory()) return all(path)
        return /\.(ts|tsx|css|js|html|webmanifest)$/.test(name) ? [path] : []
      })
    const root = process.cwd()
    const files = [...all(SRC), ...all(join(root, 'public')), ...all(join(root, 'e2e')), join(root, 'index.html')]
    const found = files.filter((f) => [...readFileSync(f, 'utf8')].some((c) => !'\t\n\r'.includes(c) && HIDDEN.test(c)))
    expect(files.length).toBeGreaterThan(30)
    expect(found).toEqual([])
  })

  it('leave no inline script in the page', () => {
    const html = readFileSync(join(process.cwd(), 'index.html'), 'utf8')
    for (const tag of html.split('<script').slice(1)) {
      expect(tag.slice(0, tag.indexOf('>'))).toMatch(/src="\//)
    }
    expect(html).not.toMatch(/<style|style="/)
  })
})
