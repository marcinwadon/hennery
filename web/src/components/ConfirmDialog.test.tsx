import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import ConfirmDialog from './ConfirmDialog'

describe('the confirmation', () => {
  it('keeps Tab among its own controls, whatever kind they are', async () => {
    render(
      <>
        <button type="button">Behind</button>
        <ConfirmDialog title="Pick one?" confirm="Go" action={async () => {}} onClose={() => {}}>
          <select aria-label="Choice">
            <option>a</option>
          </select>
          <textarea aria-label="Note" />
          <span tabIndex={0}>Focusable</span>
        </ConfirmDialog>
      </>,
    )
    const cancel = screen.getByRole('button', { name: 'Cancel' })
    const go = screen.getByRole('button', { name: 'Go' })
    const choice = screen.getByRole('combobox', { name: 'Choice' })
    expect(cancel).toHaveFocus()
    await userEvent.tab()
    expect(go).toHaveFocus()
    // Forward from the last control wraps to the first: the select.
    await userEvent.tab()
    expect(choice).toHaveFocus()
    await userEvent.tab()
    expect(screen.getByRole('textbox', { name: 'Note' })).toHaveFocus()
    await userEvent.tab()
    expect(screen.getByText('Focusable')).toHaveFocus()
    await userEvent.tab()
    expect(cancel).toHaveFocus()
    // Backward from the first control wraps to the last.
    choice.focus()
    await userEvent.tab({ shift: true })
    expect(go).toHaveFocus()
    expect(screen.getByRole('button', { name: 'Behind' })).not.toHaveFocus()
  })
})
