import { render } from 'vitest-browser-react'
import { describe, expect, it } from 'vitest'
import { useShellLayout } from '../useShellLayout'

describe('useShellLayout', () => {
  it('opens the right sidebar idempotently', async () => {
    const screen = await render(<LayoutHarness />)

    await expect.element(screen.getByTestId('right-open')).toHaveTextContent('false')
    await screen.getByRole('button', { name: 'open right' }).click()
    await expect.element(screen.getByTestId('right-open')).toHaveTextContent('true')
    await screen.getByRole('button', { name: 'open right' }).click()
    await expect.element(screen.getByTestId('right-open')).toHaveTextContent('true')
  })
})

function LayoutHarness() {
  const { openRightSidebar, rightOpen, shellRef } = useShellLayout()
  return (
    <div ref={shellRef} style={{ width: 1600 }}>
      <output data-testid="right-open">{String(rightOpen)}</output>
      <button onClick={openRightSidebar} type="button">
        open right
      </button>
    </div>
  )
}
