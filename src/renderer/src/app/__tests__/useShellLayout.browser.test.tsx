import { render } from 'vitest-browser-react'
import { describe, expect, it } from 'vitest'
import { useShellLayout } from '../useShellLayout'

describe('useShellLayout', () => {
  it('opens the bottom panel idempotently and preserves the resized height after collapse', async () => {
    const screen = await render(<LayoutHarness />)

    await expect.element(screen.getByTestId('bottom-open')).toHaveTextContent('false')
    await screen.getByRole('button', { name: 'open bottom' }).click()
    await expect.element(screen.getByTestId('bottom-open')).toHaveTextContent('true')
    await screen.getByRole('button', { name: 'resize bottom' }).click()
    await expect.element(screen.getByTestId('bottom-height')).toHaveTextContent('360')
    await screen.getByRole('button', { name: 'open bottom' }).click()
    await expect.element(screen.getByTestId('bottom-open')).toHaveTextContent('true')
    await expect.element(screen.getByTestId('bottom-height')).toHaveTextContent('360')
    await screen.getByRole('button', { name: 'close bottom' }).click()
    await expect.element(screen.getByTestId('bottom-open')).toHaveTextContent('false')
    await screen.getByRole('button', { name: 'open bottom' }).click()
    await expect.element(screen.getByTestId('bottom-open')).toHaveTextContent('true')
    await expect.element(screen.getByTestId('bottom-height')).toHaveTextContent('360')
  })

  it('reports the usable-height boundary and restores the preferred height when space returns', async () => {
    const screen = await render(<LayoutHarness />)
    await expect.element(screen.getByTestId('can-open-bottom')).toHaveTextContent('true')
    await screen.getByRole('button', { name: 'open bottom' }).click()
    await screen.getByRole('button', { name: 'resize bottom' }).click()
    await screen.rerender(<LayoutHarness height={329} />)
    await expect.element(screen.getByTestId('can-open-bottom')).toHaveTextContent('false')
    await expect.element(screen.getByTestId('bottom-open')).toHaveTextContent('false')
    await screen.getByRole('button', { name: 'open bottom' }).click()
    await expect.element(screen.getByTestId('bottom-open')).toHaveTextContent('false')
    await screen.rerender(<LayoutHarness height={330} />)
    await expect.element(screen.getByTestId('can-open-bottom')).toHaveTextContent('true')
    await expect.element(screen.getByTestId('bottom-open')).toHaveTextContent('true')
    await expect.element(screen.getByTestId('bottom-height')).toHaveTextContent('165')
    await screen.rerender(<LayoutHarness />)
    await expect.element(screen.getByTestId('bottom-height')).toHaveTextContent('360')
  })

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

function LayoutHarness({ height = 900 }: { height?: number }) {
  const {
    bottomHeight,
    bottomOpen,
    canOpenBottomPanel,
    closeBottomPanel,
    commitBottomPanelResize,
    commitSidebarResize,
    openBottomPanel,
    openRightSidebar,
    rightOpen,
    rightWidth,
    shellRef
  } = useShellLayout()
  return (
    <div ref={shellRef} style={{ width: 1600, height }}>
      <output data-testid="bottom-open">{String(bottomOpen)}</output>
      <output data-testid="bottom-height">{bottomHeight}</output>
      <output data-testid="can-open-bottom">{String(canOpenBottomPanel)}</output>
      <button onClick={openBottomPanel} type="button">
        open bottom
      </button>
      <button onClick={closeBottomPanel} type="button">
        close bottom
      </button>
      <button onClick={() => commitBottomPanelResize('bottom', 360)} type="button">
        resize bottom
      </button>
      <output data-testid="right-open">{String(rightOpen)}</output>
      <output data-testid="right-width">{Math.round(rightWidth)}</output>
      <button onClick={openRightSidebar} type="button">
        open right
      </button>
      <button onClick={() => commitSidebarResize('right', 1000)} type="button">
        expand right
      </button>
    </div>
  )
}
