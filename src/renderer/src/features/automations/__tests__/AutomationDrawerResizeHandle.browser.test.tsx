import { useRef, useState, type CSSProperties } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import {
  AUTOMATION_DRAWER_LIVE_WIDTH_PROPERTY,
  type AutomationDrawerResizeMetrics
} from '../automationLayout'
import { AutomationDrawerResizeHandle } from '../components/AutomationDrawerResizeHandle'
import '../ScheduledPage.css'

function dispatchPointerEvent(
  target: Element,
  type: 'pointerdown' | 'pointermove' | 'pointerup',
  clientX: number
) {
  target.dispatchEvent(
    new PointerEvent(type, {
      bubbles: true,
      button: 0,
      buttons: type === 'pointerup' ? 0 : 1,
      clientX,
      isPrimary: true,
      pointerId: 21,
      pointerType: 'mouse'
    })
  )
}

function waitForAnimationFrame() {
  return new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
}

function dispatchKeyboardEvent(target: Element, key: string, shiftKey = false) {
  target.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key, shiftKey }))
}

function ResizeHarness({ onCommit = () => undefined }: { onCommit?: (width: number) => void }) {
  const targetRef = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(440)
  const metrics: AutomationDrawerResizeMetrics = { maximum: 640, minimum: 380, width }

  return (
    <div
      ref={targetRef}
      data-testid="automation-resize-target"
      style={{ '--automation-drawer-width': `${width}px` } as CSSProperties}
    >
      <output data-testid="automation-committed-width">{width}</output>
      <div className="scheduled-page__list-pane" data-testid="automation-list-pane" />
      <div className="automation-drawer" data-testid="automation-drawer" />
      <AutomationDrawerResizeHandle
        ariaLabel="Resize task drawer"
        metrics={metrics}
        onResizeCommit={(nextWidth) => {
          onCommit(nextWidth)
          setWidth(nextWidth)
        }}
        resizeTargetRef={targetRef}
      />
    </div>
  )
}

describe('AutomationDrawerResizeHandle', () => {
  it('uses pointer capture and updates only its dedicated live width until commit', async () => {
    const onCommit = vi.fn()
    const screen = await render(<ResizeHarness onCommit={onCommit} />)
    const handle = screen.getByRole('separator', { name: 'Resize task drawer' }).element()
    const target = screen.getByTestId('automation-resize-target').element()

    vi.spyOn(handle, 'setPointerCapture').mockImplementation(() => undefined)
    vi.spyOn(handle, 'hasPointerCapture').mockReturnValue(true)
    vi.spyOn(handle, 'releasePointerCapture').mockImplementation(() => undefined)

    dispatchPointerEvent(handle, 'pointerdown', 600)
    expect(document.body.classList.contains('is-resizing')).toBe(true)
    expect(
      getComputedStyle(screen.getByTestId('automation-list-pane').element()).transitionDuration
    ).toBe('0s')
    expect(
      getComputedStyle(screen.getByTestId('automation-drawer').element()).transitionDuration
    ).toBe('0s')
    dispatchPointerEvent(handle, 'pointermove', 500)
    await waitForAnimationFrame()

    expect(target.style.getPropertyValue(AUTOMATION_DRAWER_LIVE_WIDTH_PROPERTY)).toBe('540px')
    expect(target.style.getPropertyValue('--right-panel-live-width')).toBe('')
    expect(target.style.getPropertyValue('--right-panel-width')).toBe('')
    expect(onCommit).not.toHaveBeenCalled()

    dispatchPointerEvent(handle, 'pointerup', 500)
    await expect.element(screen.getByTestId('automation-committed-width')).toHaveTextContent('540')
    expect(onCommit).toHaveBeenCalledWith(540)
    expect(document.body.classList.contains('is-resizing')).toBe(false)
    await waitForAnimationFrame()
    expect(target.style.getPropertyValue(AUTOMATION_DRAWER_LIVE_WIDTH_PROPERTY)).toBe('')
  })

  it('clamps pointer resizing instead of collapsing the drawer', async () => {
    const onCommit = vi.fn()
    const screen = await render(<ResizeHarness onCommit={onCommit} />)
    const handle = screen.getByRole('separator', { name: 'Resize task drawer' }).element()

    vi.spyOn(handle, 'setPointerCapture').mockImplementation(() => undefined)
    vi.spyOn(handle, 'hasPointerCapture').mockReturnValue(true)
    vi.spyOn(handle, 'releasePointerCapture').mockImplementation(() => undefined)

    dispatchPointerEvent(handle, 'pointerdown', 600)
    dispatchPointerEvent(handle, 'pointerup', 2_000)

    await expect.element(screen.getByTestId('automation-committed-width')).toHaveTextContent('380')
    expect(onCommit).toHaveBeenCalledWith(380)
  })

  it('supports arrows, shifted arrows, Home, and End with accessible values', async () => {
    const onCommit = vi.fn()
    const screen = await render(<ResizeHarness onCommit={onCommit} />)
    const handle = screen.getByRole('separator', { name: 'Resize task drawer' })

    await expect.element(handle).toHaveAttribute('aria-valuemin', '380')
    await expect.element(handle).toHaveAttribute('aria-valuemax', '640')
    await expect.element(handle).toHaveAttribute('aria-valuenow', '440')

    dispatchKeyboardEvent(handle.element(), 'ArrowLeft')
    await expect.element(screen.getByTestId('automation-committed-width')).toHaveTextContent('448')
    dispatchKeyboardEvent(handle.element(), 'ArrowLeft', true)
    await expect.element(screen.getByTestId('automation-committed-width')).toHaveTextContent('480')
    dispatchKeyboardEvent(handle.element(), 'ArrowRight')
    await expect.element(screen.getByTestId('automation-committed-width')).toHaveTextContent('472')
    dispatchKeyboardEvent(handle.element(), 'Home')
    await expect.element(screen.getByTestId('automation-committed-width')).toHaveTextContent('380')
    dispatchKeyboardEvent(handle.element(), 'End')
    await expect.element(screen.getByTestId('automation-committed-width')).toHaveTextContent('640')

    expect(onCommit.mock.calls.map(([width]) => width)).toEqual([448, 480, 472, 380, 640])
  })
})
