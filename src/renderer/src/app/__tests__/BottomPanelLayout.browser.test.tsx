import { useState } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { page } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { ResizeHandle } from '../../components/layout/ResizeHandle'
import type { Translate } from '../../config/translationFormat'
import { defaultUiPreferences } from '../../features/storage/storageClient'
import { AppShellCoveredRegion } from '../AppShellWorkspace'
import {
  getAppShellPanelStyle,
  MainPanelToolbar,
  MaximizedSidebarControls
} from '../AppShellSupport'
import { useShellLayout } from '../useShellLayout'
import { BOTTOM_PANEL_DEFAULT_HEIGHT, BOTTOM_PANEL_MIN_HEIGHT } from '../bottomPanelLayout'
import { SIDEBAR_COLLAPSE_THRESHOLD_RATIO } from '../../lib/sidebarResize'
import '../../styles/global.css'
import '../../features/rightSidebar/RightSidebar.css'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))
vi.mock('../../host/hostClient', () => ({ hostClient: {} }))

const t: Translate = (key) => key
beforeEach(() => page.viewport(1600, 1000))
afterEach(async () => {
  vi.restoreAllMocks()
  await page.viewport(1280, 720)
})

function LayoutFixture() {
  const {
    bottomOpen,
    leftOpen,
    toggleBottomPanel,
    toggleLeftSidebar,
    toggleRightSidebar,
    rightOpen,
    rightMaximized,
    shellRef,
    leftWidth,
    rightWidth,
    bottomHeight,
    leftResizeMetrics,
    commitSidebarResize,
    toggleRightSidebarMaximized,
    bottomResizeMetrics,
    closeBottomPanel,
    commitBottomPanelResize
  } = useShellLayout()
  const [height, setHeight] = useState(900)
  const controls = {
    bottomOpen: bottomOpen,
    hasUnreadConversations: false,
    leftOpen: leftOpen,
    onToggleBottomPanel: toggleBottomPanel,
    onToggleLeftSidebar: toggleLeftSidebar,
    onToggleRightSidebar: toggleRightSidebar,
    rightOpen: rightOpen,
    t
  }
  return (
    <div
      className="app-shell"
      data-bottom-open={String(bottomOpen)}
      data-left-open={String(leftOpen)}
      data-right-maximized={String(rightMaximized)}
      data-right-open={String(rightOpen)}
      ref={shellRef}
      style={{
        ...getAppShellPanelStyle(
          leftOpen,
          leftWidth,
          rightOpen,
          rightWidth,
          defaultUiPreferences(),
          bottomOpen,
          bottomHeight
        ),
        height,
        width: 1440
      }}
    >
      <aside className="side-panel side-panel--left">
        <button onClick={() => setHeight(600)} type="button">
          short window
        </button>
        <button onClick={() => setHeight(400)} type="button">
          compact window
        </button>
        <button onClick={() => setHeight(BOTTOM_PANEL_MIN_HEIGHT * 2)} type="button">
          minimum window
        </button>
        <button onClick={() => setHeight(BOTTOM_PANEL_MIN_HEIGHT * 2 - 1)} type="button">
          tiny window
        </button>
        <button onClick={() => setHeight(900)} type="button">
          tall window
        </button>
      </aside>
      {leftOpen && (
        <ResizeHandle
          metrics={leftResizeMetrics}
          onCollapse={toggleLeftSidebar}
          onResizeCommit={commitSidebarResize}
          resizeTargetRef={shellRef}
          side="left"
        />
      )}
      <AppShellCoveredRegion as="main" className="main-panel" covered={rightOpen && rightMaximized}>
        <MainPanelToolbar {...controls} title="Long conversation title" />
      </AppShellCoveredRegion>
      <aside className="side-panel side-panel--right">
        <div className="right-sidebar__toolbar-actions">
          {rightMaximized && <MaximizedSidebarControls {...controls} />}
          <button onClick={toggleRightSidebarMaximized} type="button">
            maximize right
          </button>
        </div>
      </aside>
      {bottomOpen && (
        <ResizeHandle
          metrics={bottomResizeMetrics}
          onCollapse={closeBottomPanel}
          onResizeCommit={commitBottomPanelResize}
          resizeTargetRef={shellRef}
          side="bottom"
        />
      )}
      <aside className="side-panel side-panel--bottom">terminal content</aside>
    </div>
  )
}

function box(container: HTMLElement, selector: string) {
  const element = container.querySelector<HTMLElement>(selector)
  if (!element) throw new Error(`Missing ${selector}`)
  return element.getBoundingClientRect()
}

function pointer(
  element: HTMLElement,
  type: 'pointerdown' | 'pointermove' | 'pointerup',
  clientY: number
) {
  element.dispatchEvent(
    new PointerEvent(type, {
      bubbles: true,
      button: 0,
      buttons: type === 'pointerup' ? 0 : 1,
      clientY,
      isPrimary: true,
      pointerId: 81,
      pointerType: 'mouse'
    })
  )
}

function mockCapture(element: HTMLElement) {
  vi.spyOn(element, 'setPointerCapture').mockImplementation(() => undefined)
  vi.spyOn(element, 'hasPointerCapture').mockReturnValue(true)
  vi.spyOn(element, 'releasePointerCapture').mockImplementation(() => undefined)
}

const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))

describe('Bottom panel shell integration', () => {
  it('keeps the left sidebar full height and the maximized right sidebar above the bottom panel', async () => {
    const screen = await render(<LayoutFixture />)
    await screen.getByRole('button', { name: 'app.expandBottomPanel' }).click()
    await screen.getByRole('button', { name: 'app.expandRightSidebar' }).click()
    await expect.poll(() => box(screen.container, '.side-panel--bottom').height).toBeCloseTo(280, 0)
    const originalBottom = box(screen.container, '.side-panel--bottom')
    expect(box(screen.container, '.side-panel--left').height).toBe(900)
    expect(originalBottom.left).toBeCloseTo(box(screen.container, '.side-panel--left').right, 0)
    expect(originalBottom.right).toBeCloseTo(box(screen.container, '.app-shell').right, 0)
    expect(box(screen.container, '.main-panel').bottom).toBeCloseTo(originalBottom.top, 0)

    await screen.getByRole('button', { name: 'maximize right' }).click()
    await expect
      .poll(() => box(screen.container, '.side-panel--right').left)
      .toBeCloseTo(originalBottom.left, 0)
    expect(box(screen.container, '.side-panel--right').bottom).toBeCloseTo(originalBottom.top, 0)
    expect(box(screen.container, '.side-panel--bottom').height).toBeCloseTo(280, 0)
    expect(screen.container.querySelector('.main-panel')?.getAttribute('aria-hidden')).toBe('true')
    const bottomToggle = screen.getByRole('button', { name: 'app.collapseBottomPanel' }).element()
    const rightToggle = screen.getByRole('button', { name: 'app.collapseRightSidebar' }).element()
    expect(bottomToggle.closest('.side-panel--right')).not.toBeNull()
    expect(bottomToggle.getBoundingClientRect().right).toBeLessThanOrEqual(
      rightToggle.getBoundingClientRect().left
    )

    await screen.getByRole('button', { name: 'app.collapseBottomPanel' }).click()
    await expect.poll(() => box(screen.container, '.side-panel--right').height).toBeCloseTo(900, 0)
    await screen.getByRole('button', { name: 'app.expandBottomPanel' }).click()
    await expect.poll(() => box(screen.container, '.side-panel--bottom').height).toBeCloseTo(280, 0)
  })

  it('caps both dragging and keyboard resizing at half the actual shell height, preserving the preferred height across window resizing', async () => {
    const screen = await render(<LayoutFixture />)
    await screen.getByRole('button', { name: 'app.expandBottomPanel' }).click()
    const handle = screen
      .getByRole('separator', { name: 'app.resizeBottomPanel' })
      .element() as HTMLElement
    mockCapture(handle)
    expect(handle.getAttribute('aria-valuemin')).toBe('165')
    handle.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'Home' }))
    await expect.poll(() => box(screen.container, '.side-panel--bottom').height).toBeCloseTo(165, 0)
    pointer(handle, 'pointerdown', 650)
    pointer(handle, 'pointermove', -1000)
    await frame()
    expect(getComputedStyle(document.body).cursor).toBe('row-resize')
    expect(box(screen.container, '.side-panel--bottom').height).toBeLessThanOrEqual(450)
    pointer(handle, 'pointerup', -1000)
    await expect.poll(() => box(screen.container, '.side-panel--bottom').height).toBeCloseTo(450, 0)
    handle.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'ArrowUp' }))
    expect(handle.getAttribute('aria-valuenow')).toBe('450')
    expect(handle.getAttribute('aria-orientation')).toBe('horizontal')

    await screen.getByRole('button', { name: 'short window' }).click()
    await expect.poll(() => box(screen.container, '.side-panel--bottom').height).toBeCloseTo(300, 0)
    await screen.getByRole('button', { name: 'compact window' }).click()
    await expect.poll(() => box(screen.container, '.side-panel--bottom').height).toBeCloseTo(200, 0)
    await screen.getByRole('button', { name: 'minimum window' }).click()
    await expect.poll(() => box(screen.container, '.side-panel--bottom').height).toBeCloseTo(165, 0)
    await screen.getByRole('button', { name: 'tiny window' }).click()
    await expect
      .poll(() => screen.container.querySelector('.app-shell')?.getAttribute('data-bottom-open'))
      .toBe('false')
    await screen.getByRole('button', { name: 'tall window' }).click()
    await expect.poll(() => box(screen.container, '.side-panel--bottom').height).toBeCloseTo(450, 0)
  })

  it('allows reversing a threshold crossing in one drag, then restores the saved height after collapse', async () => {
    const screen = await render(<LayoutFixture />)
    await screen.getByRole('button', { name: 'app.expandBottomPanel' }).click()
    const handle = screen
      .getByRole('separator', { name: 'app.resizeBottomPanel' })
      .element() as HTMLElement
    mockCapture(handle)
    const collapseY =
      600 + BOTTOM_PANEL_DEFAULT_HEIGHT - BOTTOM_PANEL_MIN_HEIGHT * SIDEBAR_COLLAPSE_THRESHOLD_RATIO
    pointer(handle, 'pointerdown', 600)
    pointer(handle, 'pointermove', collapseY)
    await frame()
    expect(
      screen.container.querySelector('.app-shell')?.getAttribute('data-bottom-panel-drag-collapsed')
    ).toBe('true')
    pointer(handle, 'pointermove', 600)
    await frame()
    pointer(handle, 'pointerup', 600)
    await expect
      .element(screen.getByRole('button', { name: 'app.collapseBottomPanel' }))
      .toBeVisible()

    pointer(handle, 'pointerdown', 600)
    pointer(handle, 'pointerup', collapseY)
    await expect
      .element(screen.getByRole('button', { name: 'app.expandBottomPanel' }))
      .toBeVisible()
    await screen.getByRole('button', { name: 'app.expandBottomPanel' }).click()
    await expect.poll(() => box(screen.container, '.side-panel--bottom').height).toBeCloseTo(280, 0)
    const left = screen.getByRole('separator', { name: 'app.resizeLeftSidebar' }).element()
    const bottom = box(screen.container, '.side-panel--bottom')
    expect(document.elementFromPoint(bottom.left + 2, bottom.top + 20)).toBe(left)
  })
})
