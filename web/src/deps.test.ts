// Raw HTML in an agent's Markdown stays text because nothing parses it
// (Markdown.tsx). A raw-HTML parser in the dependencies would be one plugin
// away from undoing that, and with it the reason ids need no prefix there.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

// Vitest runs from `web/`.
const FILES = ['package.json', 'pnpm-lock.yaml']
const PARSERS = ['rehype-raw', 'hast-util-raw']

describe('the dependencies', () => {
  it.each(FILES)('%s names no raw-HTML parser', (file) => {
    const text = readFileSync(join(process.cwd(), file), 'utf8')
    expect(text.length).toBeGreaterThan(100)
    expect(PARSERS.filter((name) => text.includes(name))).toEqual([])
  })
})
