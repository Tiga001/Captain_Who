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

  it('expands the right sidebar beyond the former shared limit', async () => {
    const screen = await render(<LayoutHarness />)

    await screen.getByRole('button', { name: 'open right' }).click()
    await screen.getByRole('button', { name: 'expand right' }).click()

    await expect
      .element(screen.getByTestId('right-width'))
      .toHaveTextContent(/^(?:5[6-9][1-9]|[6-9]\d{2}|\d{4,})$/)
  })
})

function LayoutHarness() {
  const { openRightSidebar, resizeSide, rightOpen, rightWidth, shellRef } = useShellLayout()
  return (
    <div ref={shellRef} style={{ width: 1600 }}>
      <output data-testid="right-open">{String(rightOpen)}</output>
      <output data-testid="right-width">{Math.round(rightWidth)}</output>
      <button onClick={openRightSidebar} type="button">
        open right
      </button>
      <button onClick={() => resizeSide('right', -1000)} type="button">
        expand right
      </button>
    </div>
  )
}
