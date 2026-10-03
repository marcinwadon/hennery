import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import When from './When'

describe('a server time', () => {
  it('shows one it cannot read as the server sent it, escaped, in its text and its attributes', () => {
    render(<When at={'2026\u202e-10-02'} />)
    const time = screen.getByText('2026<U+202E>-10-02')
    expect(time.tagName).toBe('TIME')
    expect(time).toHaveAttribute('dateTime', '2026<U+202E>-10-02')
    expect(time).toHaveAttribute('title', '2026<U+202E>-10-02')
  })

  it('shows one it can read in this browser’s own words, the exact value a hover away', () => {
    render(<When at="2026-10-02T12:00:00Z" />)
    const time = screen.getByTitle('2026-10-02T12:00:00Z')
    expect(time).toHaveAttribute('dateTime', '2026-10-02T12:00:00Z')
    expect(time).toHaveTextContent(new Date('2026-10-02T12:00:00Z').toLocaleString())
  })
})
