import { render, screen } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it, vi } from 'vitest'
import Markdown, { MarkdownImage } from './Markdown'

describe('Markdown', () => {
  it('highlights a fenced block in a common language', () => {
    const { container } = render(<Markdown>{'```js\nconst x = 1\n```'}</Markdown>)
    expect(container.querySelector('code.hljs')).not.toBeNull()
    expect(container.querySelector('.hljs-keyword, .hljs-number')).not.toBeNull()
  })

  it('leaves a fence in an unknown language plain, without throwing', () => {
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {})
    try {
      const { container } = render(<Markdown>{'```nix\npkgs.hello\n```\n\n```no-such-language\nx\n```'}</Markdown>)
      expect(container.querySelectorAll('code')).toHaveLength(2)
      expect(container.textContent).toContain('pkgs.hello')
      expect(container.querySelector('[class*="hljs-"]')).toBeNull()
      expect(errors).not.toHaveBeenCalled()
    } finally {
      errors.mockRestore()
    }
  })

  it('never guesses a language for a fence without one', () => {
    const { container } = render(<Markdown>{'```\nfunction f() { return 1 }\nconst x = f()\n```'}</Markdown>)
    const code = container.querySelector('code') as HTMLElement
    // Guessing would mark the block and name the language it guessed.
    expect(code.className).toBe('')
    expect(container.querySelector('[class*="hljs"]')).toBeNull()
  })

  it('renders a single newline as a line break', () => {
    const { container } = render(<Markdown>{'line one\nline two'}</Markdown>)
    expect(container.querySelector('br')).not.toBeNull()
  })

  it('renders GFM tables, task lists and strikethrough, and lists', () => {
    const md = ['| a | b |', '| - | - |', '| 1 | 2 |', '', '- [x] done', '- [ ] todo', '', '~~gone~~', '', '1. one', '2. two'].join('\n')
    const { container } = render(<Markdown>{md}</Markdown>)
    expect(container.querySelector('table')).not.toBeNull()
    expect(container.querySelectorAll('input[type="checkbox"]')).toHaveLength(2)
    expect(container.querySelector('del')).not.toBeNull()
    expect(container.querySelector('ol')).not.toBeNull()
  })

  it('renders a blockquote', () => {
    const { container } = render(<Markdown>{'> quoted'}</Markdown>)
    expect(container.querySelector('blockquote')).not.toBeNull()
  })

  describe('raw HTML is text, never markup', () => {
    it.each([
      ['a script block', '<script>alert(1)</script>', 'script'],
      ['an image with a handler', '<img src=x onerror=alert(1)>', 'img'],
      ['inline bold', 'a <b>x</b> b', 'b'],
      ['a details block', '<details><summary>more</summary>body</details>', 'details'],
    ])('%s', (_name, source, tag) => {
      const { container } = render(<Markdown>{source}</Markdown>)
      expect(container.querySelector(tag)).toBeNull()
      expect(container.querySelector('[onerror]')).toBeNull()
      // The characters are all still there, as text.
      expect(container.textContent).toContain(source.replace(/^a | b$/g, ''))
    })
  })

  it('opens links in a new tab without an opener', () => {
    render(<Markdown>{'[docs](https://example.com/a)'}</Markdown>)
    const a = screen.getByRole('link', { name: 'docs' }) as HTMLAnchorElement
    expect(a.href).toBe('https://example.com/a')
    expect(a.target).toBe('_blank')
    expect(a.rel).toBe('noopener noreferrer')
  })

  it('opens an autolink in a new tab without an opener too', () => {
    render(<Markdown>{'see https://example.com/b'}</Markdown>)
    const a = screen.getByRole('link', { name: 'https://example.com/b' }) as HTMLAnchorElement
    expect(a.rel).toBe('noopener noreferrer')
    expect(a.target).toBe('_blank')
  })

  it.each(['javascript:alert(1)', 'JaVaScRiPt:alert(1)', 'vbscript:x', 'data:text/html,hi'])('gives a %s link no href at all', (url) => {
    const { container } = render(<Markdown>{`[click](${url})`}</Markdown>)
    const a = container.querySelector('a')
    expect(a?.textContent).toBe('click')
    // Not even an empty one, which would link to this page.
    expect(a?.hasAttribute('href')).toBe(false)
  })

  it('links a footnote to its note, on the same page', () => {
    const { container } = render(<Markdown>{'a claim[^1]\n\n[^1]: the source'}</Markdown>)
    const ref = container.querySelector('a[data-footnote-ref]') as HTMLAnchorElement
    const target = ref.getAttribute('href')!.slice(1)
    expect(container.querySelector(`[id="${target}"]`)?.textContent).toContain('the source')
    expect(ref.hasAttribute('target')).toBe(false)
    // Every id Markdown makes carries the prefix that keeps it off `window`.
    for (const el of Array.from(container.querySelectorAll('[id]'))) {
      if (el.id !== 'footnote-label') expect(el.id).toMatch(/^user-content-/)
    }
  })

  it('makes no id but a footnote’s, and no name, from hostile input', () => {
    const source = [
      '# Heading one',
      '## user-content-x',
      'a claim[^note] and another[^2]',
      '<a name="x" id="y">anchor</a> and <a name=bare>b</a>',
      '<form id="login"><input name="password"></form>',
      '<div id="footnote-label">fake label</div>',
      '<img id="z" name="z" src="x">',
      '',
      '[^note]: the source',
      '[^2]: another <span id="inner">source</span>',
    ].join('\n')
    const { container } = render(<Markdown>{source}</Markdown>)
    const ids = Array.from(container.querySelectorAll('[id]')).map((el) => el.id)
    // The footnotes are there, so their ids were made.
    expect(ids.filter((id) => id.startsWith('user-content-fn')).length).toBeGreaterThan(0)
    for (const id of ids) expect(id === 'footnote-label' || /^user-content-/.test(id)).toBe(true)
    expect(ids.filter((id) => id === 'footnote-label')).toHaveLength(1)
    expect(container.querySelector('[name]')).toBeNull()
    expect(container.querySelector('form, input')).toBeNull()
  })

  it('never loads an image: it is a link with its alt text', () => {
    const { container } = render(<Markdown>{'![a chart](https://example.com/c.png)'}</Markdown>)
    expect(container.querySelector('img')).toBeNull()
    const a = screen.getByRole('link', { name: 'Image: a chart' }) as HTMLAnchorElement
    expect(a.href).toBe('https://example.com/c.png')
    expect(a.rel).toBe('noopener noreferrer')
    expect(a.target).toBe('_blank')
  })

  it.each([[''], [undefined]])('shows an image whose source is %o as its alt text, with no link', (src) => {
    const { container } = render(<MarkdownImage src={src} alt="a chart" />)
    expect(container.querySelector('a')).toBeNull()
    expect(container.textContent).toBe('Image: a chart')
  })

  it('shows an image with an unsafe source as its alt text, with no link', () => {
    const { container } = render(<Markdown>{'![x](javascript:alert(1))'}</Markdown>)
    expect(container.querySelector('img')).toBeNull()
    expect(container.querySelector('a')).toBeNull()
    expect(container.textContent).toContain('Image: x')
  })

  // jsdom computes no cascade (the browser checks do), so this reads the
  // rules themselves: Tailwind's preflight sets lists to no markers, and the
  // two places Markdown renders into must put them back (client view §5.3).
  it('has its containers restore list markers over the reset', () => {
    const css = readFileSync(join(process.cwd(), 'src/index.css'), 'utf8').replace(/\s+/g, ' ')
    for (const scope of ['.bubble', '.think-body']) {
      expect(css).toMatch(new RegExp(`${scope.replace('.', '\\.')} ul[^{]*\\{[^}]*list-style:disc`))
      expect(css).toMatch(new RegExp(`${scope.replace('.', '\\.')} ol[^{]*\\{[^}]*list-style:decimal`))
      expect(css).toMatch(new RegExp(`${scope.replace('.', '\\.')} ol, [^{]*\\{[^}]*padding-left:22px`))
    }
  })
})
