import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
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

  it('keeps focus when its backdrop is clicked, so Tab and Escape still work', async () => {
    const onClose = vi.fn()
    render(
      <>
        <button type="button">Behind</button>
        <ConfirmDialog title="Pick one?" confirm="Go" action={async () => {}} onClose={onClose}>
          <p>Body</p>
        </ConfirmDialog>
      </>,
    )
    const dialog = screen.getByRole('dialog', { name: 'Pick one?' })
    await userEvent.click(dialog.parentElement!)
    expect(dialog.contains(document.activeElement)).toBe(true)
    await userEvent.tab()
    expect(dialog.contains(document.activeElement)).toBe(true)
    await userEvent.keyboard('{Escape}')
    expect(onClose).toHaveBeenCalledOnce()
  })

  it('is named by its own title, whatever else is on the page', () => {
    render(
      <>
        <ConfirmDialog title="First?" confirm="Go" action={async () => {}} onClose={() => {}}>
          <p>One</p>
        </ConfirmDialog>
        <ConfirmDialog title="Second?" confirm="Go" action={async () => {}} onClose={() => {}}>
          <p>Two</p>
        </ConfirmDialog>
      </>,
    )
    expect(screen.getByRole('dialog', { name: 'First?' })).toHaveTextContent('One')
    expect(screen.getByRole('dialog', { name: 'Second?' })).toHaveTextContent('Two')
  })

  it('returns focus to a fallback when what opened it is gone', async () => {
    function Page() {
      const [open, setOpen] = useState(false)
      const [opener, setOpener] = useState(true)
      return (
        <>
          <h2 tabIndex={-1} id="fallback">
            Fallback
          </h2>
          {opener && (
            <button type="button" onClick={() => setOpen(true)}>
              Open
            </button>
          )}
          {open && (
            <ConfirmDialog
              title="Go?"
              confirm="Go"
              action={async () => setOpener(false)}
              onClose={() => setOpen(false)}
              returnFocus={() => document.getElementById('fallback')}
            >
              <p>Body</p>
            </ConfirmDialog>
          )}
        </>
      )
    }
    render(<Page />)
    await userEvent.click(screen.getByRole('button', { name: 'Open' }))
    await userEvent.click(screen.getByRole('button', { name: 'Go' }))
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(screen.getByRole('heading', { name: 'Fallback' })).toHaveFocus()
  })
})
