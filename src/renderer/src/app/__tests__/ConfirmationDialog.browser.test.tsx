import { useRef, useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { ConfirmationDialog } from '../../components/dialog/ConfirmationDialog'

function DialogHarness({ onCancel = vi.fn() }: { onCancel?: () => void }) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        Open confirmation
      </button>
      {open ? (
        <ConfirmationDialog
          cancelLabel="Keep editing"
          confirmLabel="Discard"
          description="Unsaved changes"
          onCancel={() => {
            onCancel()
            setOpen(false)
          }}
          onConfirm={() => setOpen(false)}
          title="Leave editor?"
        />
      ) : null}
    </>
  )
}

function RemovedTriggerHarness() {
  const [open, setOpen] = useState(false)
  const [showTrigger, setShowTrigger] = useState(true)
  const fallbackRef = useRef<HTMLHeadingElement | null>(null)
  return (
    <>
      <h1 ref={fallbackRef} tabIndex={-1}>
        Settings
      </h1>
      {showTrigger ? (
        <button type="button" onClick={() => setOpen(true)}>
          Delete server
        </button>
      ) : null}
      {open ? (
        <ConfirmationDialog
          cancelLabel="Cancel"
          confirmLabel="Delete"
          fallbackFocusRef={fallbackRef}
          onCancel={() => setOpen(false)}
          onConfirm={() => {
            setShowTrigger(false)
            setOpen(false)
          }}
          title="Delete?"
        />
      ) : null}
    </>
  )
}

function SlowConfirmHarness({ onConfirm }: { onConfirm: () => Promise<void> }) {
  return (
    <ConfirmationDialog
      cancelLabel="Cancel"
      confirmLabel="Delete"
      onCancel={vi.fn()}
      onConfirm={onConfirm}
      title="Delete?"
    />
  )
}

describe('ConfirmationDialog focus management', () => {
  it('moves focus into the dialog, traps keyboard focus, and restores the trigger', async () => {
    const screen = await render(<DialogHarness />)
    const trigger = screen.getByRole('button', { name: 'Open confirmation' })
    await trigger.click()

    const cancel = document.querySelector<HTMLButtonElement>('.app-confirm-dialog__button--cancel')!
    const close = document.querySelector<HTMLButtonElement>('.app-confirm-dialog__close')!
    const confirm = document.querySelector<HTMLButtonElement>(
      '.app-confirm-dialog__button--danger'
    )!
    await expect.poll(() => document.activeElement).toBe(cancel)

    close.focus()
    await userEvent.keyboard('{Shift>}{Tab}{/Shift}')
    await expect.poll(() => document.activeElement).toBe(confirm)

    confirm.focus()
    await userEvent.keyboard('{Tab}')
    await expect.poll(() => document.activeElement).toBe(close)

    await userEvent.click(cancel)
    await expect.element(trigger).toHaveFocus()
  })

  it('closes with Escape and restores focus without submitting', async () => {
    const onCancel = vi.fn()
    const screen = await render(<DialogHarness onCancel={onCancel} />)
    const trigger = screen.getByRole('button', { name: 'Open confirmation' })
    await trigger.click()
    await userEvent.keyboard('{Escape}')

    expect(onCancel).toHaveBeenCalledOnce()
    await expect.element(trigger).toHaveFocus()
    await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
  })

  it('restores focus to an explicit fallback when a successful action removes its trigger', async () => {
    const screen = await render(<RemovedTriggerHarness />)
    await screen.getByRole('button', { name: 'Delete server' }).click()
    await screen.getByRole('button', { name: 'Delete', exact: true }).click()

    await expect.element(screen.getByRole('heading', { name: 'Settings' })).toHaveFocus()
  })

  it('admits only one confirmation while an asynchronous action is pending', async () => {
    let resolve: () => void = () => undefined
    const pending = new Promise<void>((resolvePromise) => {
      resolve = () => resolvePromise()
    })
    const onConfirm = vi.fn(() => pending)
    const screen = await render(<SlowConfirmHarness onConfirm={onConfirm} />)
    const confirm = screen.getByRole('button', { name: 'Delete', exact: true })

    await userEvent.dblClick(confirm)
    expect(onConfirm).toHaveBeenCalledOnce()
    resolve()
  })
})
