import { useEffect, useRef, useState, type CSSProperties } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { ResizeHandle } from '../../components/layout/ResizeHandle'
import type { SidebarSide } from '../../lib/sidebarResize'
import '../../styles/global.css'

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
  onCommit,
  onRender,
  side
}: {
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
        metrics={{ maximum: 500, minimum: 220, width }}
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
        onResizeCommit={(_side, nextWidth) => setWidth(nextWidth)}
        resizeTargetRef={targetRef}
        side="right"
      />
    </div>
  )
}

describe('ResizeHandle', () => {
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
})
