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
    expect(surfaceId).toMatch(/^right-sidebar-browser-browser-/)
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
})

function BrowserSurfaceFixture({ page }: RightSidebarModuleRenderProps) {
  const { browserSurfaceRequest, onBrowserSurfaceReady } = useRightSidebarRuntimeContext()
  const requestId =
    browserSurfaceRequest?.pageId === page.id ? browserSurfaceRequest.requestId : null
  const surfaceId = browserSurfaceIdForPage(page.id)

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
  return { schemaVersion: 1, kind: 'ensureAttached', requestId }
}

function getSurface(container: HTMLElement): HTMLElement {
  const surface = container.querySelector<HTMLElement>('[data-testid="browser-surface"]')
  if (!surface) throw new Error('browser surface missing')
  return surface
}
