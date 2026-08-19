import { useEffect } from 'react'
import { PanelTop } from 'lucide-react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { BrowserSurfaceCommand } from '@mycopilot/protocol'
import { browserSurfaceIdForPage } from '../../browser/browserSurface'
import { useRightSidebarRuntimeContext } from '../RightSidebarRuntimeContext'
import type {
  RightSidebarModuleDefinition,
  RightSidebarModuleRenderProps
} from '../rightSidebarTypes'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

const { RightSidebar } = await import('../RightSidebar')
const surfaceLifecycle = vi.fn()
const NOOP = () => undefined
const SURFACE_ID = 'managed-browser-fixture'
const SECOND_SURFACE_ID = 'managed-browser-fixture-second'

const BROWSER_MODULE: RightSidebarModuleDefinition = {
  contextBinding: 'global',
  createPage: ({ pageId }) => ({
    id: pageId,
    moduleId: 'browser',
    title: 'Browser',
    workspaceKey: null
  }),
  icon: PanelTop,
  id: 'browser',
  instancePolicy: 'multiple',
  render: (props) => <BrowserSurfaceFixture {...props} />,
  retention: 'keep-alive',
  surfaceKind: 'react',
  titleKey: 'rightSidebar.browser',
  unavailablePagePolicy: 'retain-page'
}

afterEach(() => {
  surfaceLifecycle.mockClear()
})

describe('RightSidebar browser automation surface bridge', () => {
  it('creates once, reuses the exact surface, and only closes the requested browser page', async () => {
    const ready = vi.fn(async () => undefined)
    const first = ensureCommand('11111111-1111-4111-8111-111111111111')
    const screen = await render(
      <RightSidebar
        browserSurfaceCommand={first}
        isMaximized={false}
        isOpen
        modules={[BROWSER_MODULE]}
        onBrowserSurfaceReady={ready}
        onToggleMaximized={NOOP}
      />
    )

    await expect.poll(() => ready).toHaveBeenCalledTimes(1)
    const surface = getSurface(screen.container)
    const surfaceId = surface.dataset.surfaceId
    expect(surfaceId).toBe(SURFACE_ID)
    expect(ready).toHaveBeenCalledWith({
      schemaVersion: 1,
      requestId: first.requestId,
      surfaceId
    })
    expect(surfaceLifecycle.mock.calls.filter(([event]) => event === 'mount')).toHaveLength(1)

    await screen.rerender(
      <RightSidebar
        browserSurfaceCommand={first}
        isMaximized={false}
        isOpen={false}
        modules={[BROWSER_MODULE]}
        onBrowserSurfaceReady={ready}
        onToggleMaximized={NOOP}
      />
    )
    expect(ready).toHaveBeenCalledTimes(1)
    expect(surfaceLifecycle.mock.calls.filter(([event]) => event === 'unmount')).toHaveLength(0)
    expect(getSurface(screen.container)).toBe(surface)

    const second = ensureCommand('22222222-2222-4222-8222-222222222222')
    await screen.rerender(
      <RightSidebar
        browserSurfaceCommand={second}
        isMaximized={false}
        isOpen
        modules={[BROWSER_MODULE]}
        onBrowserSurfaceReady={ready}
        onToggleMaximized={NOOP}
      />
    )
    await expect.poll(() => ready).toHaveBeenCalledTimes(2)
    expect(getSurface(screen.container)).toBe(surface)
    expect(surfaceLifecycle.mock.calls.filter(([event]) => event === 'mount')).toHaveLength(1)

    const close: BrowserSurfaceCommand = {
      schemaVersion: 1,
      kind: 'closeSurface',
      requestId: '33333333-3333-4333-8333-333333333333',
      surfaceId: surfaceId!
    }
    await screen.rerender(
      <RightSidebar
        browserSurfaceCommand={close}
        isMaximized={false}
        isOpen
        modules={[BROWSER_MODULE]}
        onBrowserSurfaceReady={ready}
        onToggleMaximized={NOOP}
      />
    )
    await expect
      .poll(() => screen.container.querySelector('[data-testid="browser-surface"]'))
      .toBeNull()
    expect(surfaceLifecycle.mock.calls.filter(([event]) => event === 'unmount')).toHaveLength(1)
  })

  it('creates a background popup tab and selects it only on an explicit Host command', async () => {
    const ready = vi.fn(async () => undefined)
    const first = ensureCommand('44444444-4444-4444-8444-444444444444')
    const screen = await render(
      <RightSidebar
        browserSurfaceCommand={first}
        isMaximized={false}
        isOpen
        modules={[BROWSER_MODULE]}
        onBrowserSurfaceReady={ready}
        onToggleMaximized={NOOP}
      />
    )
    await expect.poll(() => ready).toHaveBeenCalledTimes(1)

    const create: BrowserSurfaceCommand = {
      schemaVersion: 1,
      kind: 'createSurface',
      requestId: '55555555-5555-4555-8555-555555555555',
      surfaceId: SECOND_SURFACE_ID,
      activate: false
    }
    await screen.rerender(
      <RightSidebar
        browserSurfaceCommand={create}
        isMaximized={false}
        isOpen
        modules={[BROWSER_MODULE]}
        onBrowserSurfaceReady={ready}
        onToggleMaximized={NOOP}
      />
    )
    await expect.poll(() => ready).toHaveBeenCalledTimes(2)
    const secondSurface = [
      ...screen.container.querySelectorAll<HTMLElement>('[data-surface-id]')
    ].find((candidate) => candidate.dataset.surfaceId === SECOND_SURFACE_ID)
    expect(secondSurface).toBeDefined()
    expect(secondSurface?.closest('.right-sidebar__page')).toHaveAttribute('aria-hidden', 'true')

    const select: BrowserSurfaceCommand = {
      schemaVersion: 1,
      kind: 'selectSurface',
      requestId: '66666666-6666-4666-8666-666666666666',
      surfaceId: SECOND_SURFACE_ID
    }
    await screen.rerender(
      <RightSidebar
        browserSurfaceCommand={select}
        isMaximized={false}
        isOpen
        modules={[BROWSER_MODULE]}
        onBrowserSurfaceReady={ready}
        onToggleMaximized={NOOP}
      />
    )
    await expect.poll(() => ready).toHaveBeenCalledTimes(3)
    expect(secondSurface?.closest('.right-sidebar__page')).not.toHaveAttribute('aria-hidden')
  })
})

function BrowserSurfaceFixture({ page }: RightSidebarModuleRenderProps) {
  const { browserSurfaceRequest, onBrowserSurfaceReady } = useRightSidebarRuntimeContext()
  const requestId =
    browserSurfaceRequest?.pageId === page.id ? browserSurfaceRequest.requestId : null
  const surfaceId =
    page.moduleState?.kind === 'browser-surface'
      ? page.moduleState.surfaceId
      : browserSurfaceIdForPage(page.id)

  useEffect(() => {
    surfaceLifecycle('mount', page.id)
    return () => surfaceLifecycle('unmount', page.id)
  }, [page.id])

  useEffect(() => {
    if (requestId) onBrowserSurfaceReady?.(page.id, surfaceId, requestId)
  }, [onBrowserSurfaceReady, page.id, requestId, surfaceId])

  return <div data-surface-id={surfaceId} data-testid="browser-surface" />
}

function ensureCommand(requestId: string): BrowserSurfaceCommand {
  return { schemaVersion: 1, kind: 'ensureAttached', requestId, surfaceId: SURFACE_ID }
}

function getSurface(container: HTMLElement): HTMLElement {
  const surface = container.querySelector<HTMLElement>('[data-testid="browser-surface"]')
  if (!surface) throw new Error('browser surface missing')
  return surface
}
