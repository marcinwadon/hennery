import { render } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { Text, visible } from './text'

describe('visible', () => {
  it('leaves ordinary text as it is, other scripts included', () => {
    expect(visible('build-box 2 · łódź · ビルド · مضيف')).toBe('build-box 2 · łódź · ビルド · مضيف')
  })

  it.each([
    ['soft hyphen', '\u00AD', '<U+00AD>'],
    ['zero-width space', '\u200B', '<U+200B>'],
    ['zero-width joiner', '\u200D', '<U+200D>'],
    ['byte order mark', '\uFEFF', '<U+FEFF>'],
    ['left-to-right embedding', '\u202A', '<U+202A>'],
    ['right-to-left override', '\u202E', '<U+202E>'],
    ['left-to-right isolate', '\u2066', '<U+2066>'],
    ['pop directional isolate', '\u2069', '<U+2069>'],
    ['Arabic letter mark', '\u061C', '<U+061C>'],
    ['a tag character', '\u{E0041}', '<U+E0041>'],
  ])('escapes the %s, a format character', (_, c, shown) => {
    expect(visible(`a${c}b`)).toBe(`a${shown}b`)
  })

  it.each([
    ['Hangul filler', '\u3164', '<U+3164>'],
    ['Hangul choseong filler', '\u115F', '<U+115F>'],
    ['halfwidth Hangul filler', '\uFFA0', '<U+FFA0>'],
    ['variation selector 16', '\uFE0F', '<U+FE0F>'],
    ['combining grapheme joiner', '\u034F', '<U+034F>'],
    ['unassigned ignorable U+2065', '\u2065', '<U+2065>'],
  ])('escapes the %s, which renders as nothing', (_, c, shown) => {
    expect(visible(`a${c}b`)).toBe(`a${shown}b`)
  })

  it('leaves combining marks and private-use characters, which render', () => {
    expect(visible('e\u0301 \uE000')).toBe('e\u0301 \uE000')
  })

  it('escapes control characters', () => {
    expect(visible('a\u0000b\nc\u001Bd')).toBe('a<U+0000>b<U+000A>c<U+001B>d')
  })

  it('escapes the line and paragraph separators', () => {
    expect(visible('a\u2028b\u2029c')).toBe('a<U+2028>b<U+2029>c')
  })
})

describe('Text', () => {
  it('renders escaped text in a <bdi>, never as markup', () => {
    const { container } = render(<Text>{'<img src=x onerror=alert(1)>\u202Eevil'}</Text>)
    const bdi = container.querySelector('bdi')!
    expect(bdi.textContent).toBe('<img src=x onerror=alert(1)><U+202E>evil')
    expect(container.querySelector('img')).toBeNull()
  })
})
