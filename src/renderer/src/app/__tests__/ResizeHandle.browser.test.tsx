import { useEffect, useRef, useState, type CSSProperties } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { ResizeHandle } from '../../components/layout/ResizeHandle'
import { SIDEBAR_COLLAPSE_THRESHOLD_RATIO, type SidebarSide } from '../../lib/sidebarResize'
import '../../styles/global.css'
import '../../features/automations/ScheduledPage.css'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

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
      pointerId: 17,
      pointerType: 'mouse'
    })
  )
}

function waitForAnimationFrame() {
  return new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
}

function ResizeHarness({
  minimum = 220,
  onCollapse = () => undefined,
  onCommit,
  onRender,
  side
}: {
  minimum?: number
  onCollapse?: () => void
  onCommit: (side: SidebarSide, width: number) => void
  onRender: () => void
  side: SidebarSide
}) {
  const targetRef = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(300)
  const [unrelatedRender, setUnrelatedRender] = useState(0)

  useEffect(() => {
    onRender()
  })

  return (
    <div
      ref={targetRef}
      className="app-shell"
      data-testid="resize-target"
      style={
        {
          '--left-panel-width': `${width}px`,
          '--right-panel-width': `${width}px`
        } as CSSProperties
      }
    >
      <output data-testid="committed-width">{width}</output>
      <output data-testid="unrelated-render">{unrelatedRender}</output>
      <button onClick={() => setUnrelatedRender((value) => value + 1)} type="button">
        unrelated render
      </button>
      <ResizeHandle
        metrics={{ maximum: 500, minimum, width }}
        onCollapse={onCollapse}
        onResizeCommit={(resizeSide, nextWidth) => {
          onCommit(resizeSide, nextWidth)
          setWidth(nextWidth)
        }}
        resizeTargetRef={targetRef}
        side={side}
      />
    </div>
  )
}

function RightSidebarGeometryHarness() {
  const targetRef = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(300)

  return (
    <div
      ref={targetRef}
      className="app-shell"
      data-testid="geometry-shell"
      style={
        {
          '--left-panel-width': '0px',
          '--right-panel-width': `${width}px`,
          height: '320px',
          minWidth: 0,
          width: '1000px'
        } as CSSProperties
      }
    >
      <aside className="side-panel side-panel--left" />
      <main className="main-panel" data-testid="geometry-main" />
      <aside className="side-panel side-panel--right" data-testid="geometry-right" />
      <ResizeHandle
        metrics={{ maximum: 700, minimum: 220, width }}
        onCollapse={() => undefined}
        onResizeCommit={(_side, nextWidth) => setWidth(nextWidth)}
        resizeTargetRef={targetRef}
        side="right"
      />
    </div>
  )
}

function ScheduledLeftResizeHarness() {
  const targetRef = useRef<HTMLDivElement>(null)
  return (
    <div
      ref={targetRef}
      className="app-shell"
      data-primary-view="scheduled"
      data-testid="scheduled-resize-shell"
      style={
        {
          '--left-panel-width': '300px',
          '--right-panel-width': '360px',
          height: '320px',
          minWidth: 0,
          width: '1000px'
        } as CSSProperties
      }
    >
      <aside className="side-panel side-panel--left" />
      <div className="scheduled-page-layer" data-testid="scheduled-resize-cover" />
      <ResizeHandle
        metrics={{ maximum: 500, minimum: 220, width: 300 }}
        onCollapse={() => undefined}
        onResizeCommit={() => undefined}
        resizeTargetRef={targetRef}
        side="left"
      />
    </div>
  )
}

describe('ResizeHandle', () => {
  it('keeps the conversation left-sidebar metrics reachable above the scheduled cover', async () => {
    const screen = await render(<ScheduledLeftResizeHarness />)
    const handle = screen.getByRole('separator', { name: 'app.resizeLeftSidebar' })

    await expect.element(handle).toHaveAttribute('aria-valuemin', '220')
    await expect.element(handle).toHaveAttribute('aria-valuemax', '500')
    await expect.element(handle).toHaveAttribute('aria-valuenow', '300')
    expect(getComputedStyle(handle.element()).zIndex).toBe('49')

    const handleElement = handle.element()
    const shell = screen.getByTestId('scheduled-resize-shell').element()
    const cover = screen.getByTestId('scheduled-resize-cover').element()
    vi.spyOn(handleElement, 'setPointerCapture').mockImplementation(() => undefined)
    vi.spyOn(handleElement, 'hasPointerCapture').mockReturnValue(true)
    vi.spyOn(handleElement, 'releasePointerCapture').mockImplementation(() => undefined)
    dispatchPointerEvent(handleElement, 'pointerdown', 300)
    dispatchPointerEvent(handleElement, 'pointermove', 360)
    await waitForAnimationFrame()

    expect(shell.style.getPropertyValue('--left-panel-live-width')).toBe('360px')
    expect(cover.getBoundingClientRect().left - shell.getBoundingClientRect().left).toBeCloseTo(
      360,
      0
    )
    dispatchPointerEvent(handleElement, 'pointerup', 360)
  })

  it('uses a wider transparent hit target while keeping the divider one pixel wide', async () => {
    const screen = await render(
      <ResizeHarness onCommit={() => undefined} onRender={() => undefined} side="left" />
    )
    const handle = screen.container.querySelector<HTMLDivElement>('.resize-handle')
    if (!handle) throw new Error('Resize handle did not render')

    expect(Number.parseFloat(getComputedStyle(handle).width)).toBeGreaterThanOrEqual(13)
    expect(getComputedStyle(handle, '::after').width).toBe('1px')
  })

  it('tracks the latest absolute pointer position without rendering React on every move', async () => {
    const onCommit = vi.fn()
    const onRender = vi.fn()
    const screen = await render(
      <ResizeHarness onCommit={onCommit} onRender={onRender} side="left" />
    )
    const handle = screen.container.querySelector<HTMLDivElement>('.resize-handle')
    const target = screen.getByTestId('resize-target').element()
    if (!handle) throw new Error('Resize handle did not render')

    vi.spyOn(handle, 'setPointerCapture').mockImplementation(() => undefined)
    vi.spyOn(handle, 'hasPointerCapture').mockReturnValue(true)
    vi.spyOn(handle, 'releasePointerCapture').mockImplementation(() => undefined)

    const initialRenderCount = onRender.mock.calls.length
    dispatchPointerEvent(handle, 'pointerdown', 300)
    dispatchPointerEvent(handle, 'pointermove', 305)
    dispatchPointerEvent(handle, 'pointermove', 340)
    dispatchPointerEvent(handle, 'pointermove', 380)
    await waitForAnimationFrame()

    expect(target.style.getPropertyValue('--left-panel-live-width')).toBe('380px')
    expect(onRender).toHaveBeenCalledTimes(initialRenderCount)
    expect(onCommit).not.toHaveBeenCalled()

    await screen.getByRole('button', { name: 'unrelated render' }).click()
    expect(target.style.getPropertyValue('--left-panel-live-width')).toBe('380px')

    dispatchPointerEvent(handle, 'pointerup', 380)

    await expect.element(screen.getByTestId('committed-width')).toHaveTextContent('380')
    expect(onCommit).toHaveBeenCalledOnce()
    expect(onCommit).toHaveBeenCalledWith('left', 380)
    expect(document.body.classList.contains('is-resizing')).toBe(false)
    await waitForAnimationFrame()
    expect(target.style.getPropertyValue('--left-panel-live-width')).toBe('')
  })

  it('reverses the pointer direction for the right sidebar and clamps to its bounds', async () => {
    const onCommit = vi.fn()
    const screen = await render(
      <ResizeHarness onCommit={onCommit} onRender={() => undefined} side="right" />
    )
    const handle = screen.container.querySelector<HTMLDivElement>('.resize-handle')
    if (!handle) throw new Error('Resize handle did not render')

    vi.spyOn(handle, 'setPointerCapture').mockImplementation(() => undefined)
    vi.spyOn(handle, 'hasPointerCapture').mockReturnValue(true)
    vi.spyOn(handle, 'releasePointerCapture').mockImplementation(() => undefined)

    dispatchPointerEvent(handle, 'pointerdown', 600)
    dispatchPointerEvent(handle, 'pointermove', 200)
    dispatchPointerEvent(handle, 'pointerup', 200)

    await expect.element(screen.getByTestId('committed-width')).toHaveTextContent('500')
    expect(onCommit).toHaveBeenCalledWith('right', 500)
  })

  it('keeps the right sidebar surface aligned with the live grid boundary while dragging', async () => {
    const screen = await render(<RightSidebarGeometryHarness />)
    const handle = screen.container.querySelector<HTMLDivElement>('.resize-handle')
    const shell = screen.getByTestId('geometry-shell').element()
    const main = screen.getByTestId('geometry-main').element()
    const right = screen.getByTestId('geometry-right').element()
    if (!handle) throw new Error('Resize handle did not render')

    vi.spyOn(handle, 'setPointerCapture').mockImplementation(() => undefined)
    vi.spyOn(handle, 'hasPointerCapture').mockReturnValue(true)
    vi.spyOn(handle, 'releasePointerCapture').mockImplementation(() => undefined)

    dispatchPointerEvent(handle, 'pointerdown', 700)
    dispatchPointerEvent(handle, 'pointermove', 500)
    await waitForAnimationFrame()

    expect(shell.style.getPropertyValue('--right-panel-live-width')).toBe('500px')
    expect(right.getBoundingClientRect().width).toBeCloseTo(500, 0)
    expect(main.getBoundingClientRect().right).toBeCloseTo(right.getBoundingClientRect().left, 0)

    dispatchPointerEvent(handle, 'pointerup', 500)
  })

  it('clears the transient width when a drag ends without changing the committed width', async () => {
    const onCommit = vi.fn()
    const screen = await render(
      <ResizeHarness onCommit={onCommit} onRender={() => undefined} side="left" />
    )
    const handle = screen.container.querySelector<HTMLDivElement>('.resize-handle')
    const target = screen.getByTestId('resize-target').element()
    if (!handle) throw new Error('Resize handle did not render')

    vi.spyOn(handle, 'setPointerCapture').mockImplementation(() => undefined)
    vi.spyOn(handle, 'hasPointerCapture').mockReturnValue(true)
    vi.spyOn(handle, 'releasePointerCapture').mockImplementation(() => undefined)

    dispatchPointerEvent(handle, 'pointerdown', 300)
    expect(target.style.getPropertyValue('--left-panel-live-width')).toBe('300px')
    dispatchPointerEvent(handle, 'pointerup', 300)
    await waitForAnimationFrame()

    expect(onCommit).toHaveBeenCalledWith('left', 300)
    expect(target.style.getPropertyValue('--left-panel-live-width')).toBe('')
  })

  it.each([
    {
      collapseClientX: 220 * SIDEBAR_COLLAPSE_THRESHOLD_RATIO,
      minimum: 220,
      side: 'left' as const
    },
    {
      collapseClientX: 300 + (300 - 280 * SIDEBAR_COLLAPSE_THRESHOLD_RATIO),
      minimum: 280,
      side: 'right' as const
    }
  ])('collapses the $side sidebar at half of its minimum width', async (testCase) => {
    const onCollapse = vi.fn()
    const onCommit = vi.fn()
    const screen = await render(
      <ResizeHarness
        minimum={testCase.minimum}
        onCollapse={onCollapse}
        onCommit={onCommit}
        onRender={() => undefined}
        side={testCase.side}
      />
    )
    const handle = screen.container.querySelector<HTMLDivElement>('.resize-handle')
    if (!handle) throw new Error('Resize handle did not render')

    vi.spyOn(handle, 'setPointerCapture').mockImplementation(() => undefined)
    vi.spyOn(handle, 'hasPointerCapture').mockReturnValue(true)
    vi.spyOn(handle, 'releasePointerCapture').mockImplementation(() => undefined)

    dispatchPointerEvent(handle, 'pointerdown', 300)
    dispatchPointerEvent(handle, 'pointermove', testCase.collapseClientX)
    await waitForAnimationFrame()

    expect(onCollapse).not.toHaveBeenCalled()
    expect(onCommit).not.toHaveBeenCalled()
    expect(document.body.classList.contains('is-resizing')).toBe(true)

    const target = screen.getByTestId('resize-target').element()
    const attribute =
      testCase.side === 'left'
        ? 'data-left-sidebar-drag-collapsed'
        : 'data-right-sidebar-drag-collapsed'
    const property =
      testCase.side === 'left' ? '--left-panel-live-width' : '--right-panel-live-width'
    expect(target.getAttribute(attribute)).toBe('true')
    expect(target.style.getPropertyValue(property)).toBe('0px')

    dispatchPointerEvent(handle, 'pointerup', testCase.collapseClientX)

    expect(onCollapse).toHaveBeenCalledOnce()
    expect(onCommit).not.toHaveBeenCalled()
    expect(document.body.classList.contains('is-resizing')).toBe(false)
  })

  it.each([
    {
      collapseClientX: 220 * SIDEBAR_COLLAPSE_THRESHOLD_RATIO,
      expandClientX: 220 * SIDEBAR_COLLAPSE_THRESHOLD_RATIO + 1,
      minimum: 220,
      side: 'left' as const
    },
    {
      collapseClientX: 300 + (300 - 280 * SIDEBAR_COLLAPSE_THRESHOLD_RATIO),
      expandClientX: 300 + (300 - 280 * SIDEBAR_COLLAPSE_THRESHOLD_RATIO) - 1,
      minimum: 280,
      side: 'right' as const
    }
  ])(
    'reopens the $side sidebar when the captured pointer crosses back over its remembered threshold',
    async (testCase) => {
      const onCollapse = vi.fn()
      const onCommit = vi.fn()
      const screen = await render(
        <ResizeHarness
          minimum={testCase.minimum}
          onCollapse={onCollapse}
          onCommit={onCommit}
          onRender={() => undefined}
          side={testCase.side}
        />
      )
      const handle = screen.container.querySelector<HTMLDivElement>('.resize-handle')
      const target = screen.getByTestId('resize-target').element()
      if (!handle) throw new Error('Resize handle did not render')

      vi.spyOn(handle, 'setPointerCapture').mockImplementation(() => undefined)
      vi.spyOn(handle, 'hasPointerCapture').mockReturnValue(true)
      vi.spyOn(handle, 'releasePointerCapture').mockImplementation(() => undefined)

      const attribute =
        testCase.side === 'left'
          ? 'data-left-sidebar-drag-collapsed'
          : 'data-right-sidebar-drag-collapsed'
      const property =
        testCase.side === 'left' ? '--left-panel-live-width' : '--right-panel-live-width'

      dispatchPointerEvent(handle, 'pointerdown', 300)
      dispatchPointerEvent(handle, 'pointermove', testCase.collapseClientX)
      await waitForAnimationFrame()
      expect(target.getAttribute(attribute)).toBe('true')
      expect(target.style.getPropertyValue(property)).toBe('0px')

      dispatchPointerEvent(handle, 'pointermove', testCase.expandClientX)
      await waitForAnimationFrame()
      expect(target.getAttribute(attribute)).toBe('false')
      expect(target.style.getPropertyValue(property)).toBe(`${testCase.minimum}px`)
      expect(getComputedStyle(target).transitionDuration).toContain('0.18s')
      expect(onCollapse).not.toHaveBeenCalled()

      dispatchPointerEvent(handle, 'pointerup', testCase.expandClientX)

      expect(onCollapse).not.toHaveBeenCalled()
      expect(onCommit).toHaveBeenCalledOnce()
      expect(onCommit).toHaveBeenCalledWith(testCase.side, testCase.minimum)
      expect(document.body.classList.contains('is-resizing')).toBe(false)
    }
  )

  it('uses the final threshold position after crossing it repeatedly in one drag', async () => {
    const onCollapse = vi.fn()
    const onCommit = vi.fn()
    const screen = await render(
      <ResizeHarness
        onCollapse={onCollapse}
        onCommit={onCommit}
        onRender={() => undefined}
        side="left"
      />
    )
    const handle = screen.container.querySelector<HTMLDivElement>('.resize-handle')
    if (!handle) throw new Error('Resize handle did not render')

    vi.spyOn(handle, 'setPointerCapture').mockImplementation(() => undefined)
    vi.spyOn(handle, 'hasPointerCapture').mockReturnValue(true)
    vi.spyOn(handle, 'releasePointerCapture').mockImplementation(() => undefined)

    const threshold = 220 * SIDEBAR_COLLAPSE_THRESHOLD_RATIO
    dispatchPointerEvent(handle, 'pointerdown', 300)
    dispatchPointerEvent(handle, 'pointermove', threshold)
    await waitForAnimationFrame()
    dispatchPointerEvent(handle, 'pointermove', threshold + 1)
    await waitForAnimationFrame()
    dispatchPointerEvent(handle, 'pointermove', threshold - 1)
    await waitForAnimationFrame()
    dispatchPointerEvent(handle, 'pointerup', threshold - 1)

    expect(onCollapse).toHaveBeenCalledOnce()
    expect(onCommit).not.toHaveBeenCalled()
  })

  it('keeps the sidebar open above half of its minimum width', async () => {
    const onCollapse = vi.fn()
    const onCommit = vi.fn()
    const screen = await render(
      <ResizeHarness
        onCollapse={onCollapse}
        onCommit={onCommit}
        onRender={() => undefined}
        side="left"
      />
    )
    const handle = screen.container.querySelector<HTMLDivElement>('.resize-handle')
    if (!handle) throw new Error('Resize handle did not render')

    vi.spyOn(handle, 'setPointerCapture').mockImplementation(() => undefined)
    vi.spyOn(handle, 'hasPointerCapture').mockReturnValue(true)
    vi.spyOn(handle, 'releasePointerCapture').mockImplementation(() => undefined)

    const clientX = 220 * SIDEBAR_COLLAPSE_THRESHOLD_RATIO + 1
    dispatchPointerEvent(handle, 'pointerdown', 300)
    dispatchPointerEvent(handle, 'pointermove', clientX)
    dispatchPointerEvent(handle, 'pointerup', clientX)

    expect(onCollapse).not.toHaveBeenCalled()
    expect(onCommit).toHaveBeenCalledWith('left', 220)
  })
})
