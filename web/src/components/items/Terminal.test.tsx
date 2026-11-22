import { render, screen } from '@testing-library/react'
import { expect, it } from 'vitest'
import Terminal from './Terminal'

it('renders terminal text literally and distinguishes explicit error from unknown outcome', () => {
  const item = { terminal_id: 'term-1', command: 'sleep 8', cwd: '/tmp/test', output: '<script>unsafe()</script>', exit_code: null, signal: null }
  const { rerender } = render(<Terminal item={item} />)
  expect(screen.getByText('<script>unsafe()</script>')).toBeInTheDocument()
  expect(screen.queryByText('Exit 0')).not.toBeInTheDocument()
  rerender(<Terminal item={{ ...item, exit_code: 7 }} />)
  expect(screen.getByText('Exit 7')).toBeInTheDocument()
})
