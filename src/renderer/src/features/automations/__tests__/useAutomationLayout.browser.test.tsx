import { useRef, useState } from 'react'
import { describe, expect, it } from 'vitest'
import { render } from 'vitest-browser-react'
import { useAutomationLayout } from '../useAutomationLayout'

function LayoutHarness() {
  const containerRef = useRef<HTMLDivElement>(null)
  const [containerWidth, setContainerWidth] = useState(759)
  const layout = useAutomationLayout({ containerRef, drawerOpen: true })

  return (
    <div ref={containerRef} style={{ width: `${containerWidth}px` }}>
      <output data-testid="measured-width">{layout.containerWidth ?? 'unmeasured'}</output>
      <output data-testid="compact">{String(layout.compact)}</output>
      <output data-testid="drawer-width">{layout.drawerWidth}</output>
      <output data-testid="preferred-width">{layout.preferredDrawerWidth}</output>
      <button type="button" onClick={() => setContainerWidth(760)}>
        split
      </button>
      <button type="button" onClick={() => setContainerWidth(800)}>
        contract
      </button>
      <button type="button" onClick={() => setContainerWidth(1_200)}>
        expand
      </button>
      <button type="button" onClick={() => layout.commitDrawerWidth(560)}>
        prefer wide drawer
      </button>
    </div>
  )
}

describe('useAutomationLayout', () => {
  it('reacts to the scheduled container ResizeObserver instead of viewport media queries', async () => {
    const screen = await render(<LayoutHarness />)

    await expect.element(screen.getByTestId('measured-width')).toHaveTextContent('759')
    await expect.element(screen.getByTestId('compact')).toHaveTextContent('true')

    await screen.getByRole('button', { name: 'split' }).click()
    await expect.element(screen.getByTestId('measured-width')).toHaveTextContent('760')
    await expect.element(screen.getByTestId('compact')).toHaveTextContent('false')
    await expect.element(screen.getByTestId('drawer-width')).toHaveTextContent('400')
  })

  it('does not overwrite the preferred drawer width during temporary contraction', async () => {
    const screen = await render(<LayoutHarness />)

    await screen.getByRole('button', { name: 'prefer wide drawer' }).click()
    await expect.element(screen.getByTestId('preferred-width')).toHaveTextContent('560')

    await screen.getByRole('button', { name: 'contract' }).click()
    await expect.element(screen.getByTestId('drawer-width')).toHaveTextContent('440')
    await expect.element(screen.getByTestId('preferred-width')).toHaveTextContent('560')

    await screen.getByRole('button', { name: 'expand' }).click()
    await expect.element(screen.getByTestId('drawer-width')).toHaveTextContent('560')
    await expect.element(screen.getByTestId('preferred-width')).toHaveTextContent('560')
  })
})
