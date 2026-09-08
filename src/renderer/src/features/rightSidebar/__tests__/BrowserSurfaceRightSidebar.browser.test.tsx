import { useEffect } from 'react'
import { PanelTop } from 'lucide-react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render } from 'vitest-browser-react'
import { userEvent } from 'vitest/browser'
import type {
  BrowserSurfaceCommand,
  BrowserSurfaceReadyInput,
  BrowserSurfaceReadyOutput
} from '@mycopilot/protocol'
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
const SURFACE_INSTANCE_ID = 'instance-fixture-00001'
const targetUpdates = new Map<string, (instanceId: string, active: boolean) => void>()

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
  surfaceKind: 'webview',
  titleKey: 'rightSidebar.browser',
  unavailablePagePolicy: 'retain-page'
}

afterEach(() => {
  cleanup()
  surfaceLifecycle.mockClear()
  targetUpdates.clear()
})

describe('RightSidebar browser automation surface bridge', () => {
  it('uses live exact-instance target state instead of selection commands to mark tabs', async () => {
    const ready = vi.fn(async (input: BrowserSurfaceReadyInput) => appliedReady(input))
    const props = {
      isMaximized: false,
      isOpen: true,
      modules: [BROWSER_MODULE],
      onBrowserSurfaceReady: ready,
      onToggleMaximized: NOOP
    }
    const screen = await render(
      <RightSidebar
        {...props}
        browserSurfaceCommand={ensureCommand('11111111-1111-4111-8111-111111111111')}
      />
    )
    await expect.poll(() => ready).toHaveBeenCalledTimes(1)
    const marked = () => screen.container.querySelectorAll('[data-automation-active="true"]')
    expect(marked()).toHaveLength(0)
    targetUpdates.get(SURFACE_ID)!(SURFACE_INSTANCE_ID, true)
    await expect.poll(() => marked().length).toBe(1)

    await screen.rerender(
      <RightSidebar
        {...props}
        browserSurfaceCommand={{
          schemaVersion: 1,
          kind: 'createSurface',
          activate: false,
          requestId: '22222222-2222-4222-8222-222222222222',
          surfaceId: SECOND_SURFACE_ID
        }}
      />
    )
    await expect.poll(() => ready).toHaveBeenCalledTimes(2)
    expect(marked()).toHaveLength(1)
    targetUpdates.get(SECOND_SURFACE_ID)!(SURFACE_INSTANCE_ID, true)
    await expect.poll(() => marked().length).toBe(2)
    const tabs = [...screen.container.querySelectorAll<HTMLButtonElement>('[role="tab"]')]
    await userEvent.click(tabs[1]!)
    expect(tabs[1]).toHaveAttribute('aria-selected', 'true')
    expect(marked()).toHaveLength(2)
    const firstPage = getSurface(screen.container).closest<HTMLElement>('.right-sidebar__page')!
    expect(firstPage).toHaveAttribute('data-agent-rendering', 'true')
    expect(firstPage.inert).toBe(true)
    expect(getComputedStyle(firstPage).contentVisibility).toBe('visible')
    expect(getComputedStyle(firstPage).opacity).toBe('0')
    expect(getComputedStyle(firstPage).pointerEvents).toBe('none')

    targetUpdates.get(SURFACE_ID)!('instance-retired-0001', false)
    expect(marked()).toHaveLength(2)
    targetUpdates.get(SURFACE_ID)!(SURFACE_INSTANCE_ID, false)
    await expect.poll(() => marked().length).toBe(1)
    expect(getComputedStyle(firstPage).contentVisibility).toBe('hidden')
    await screen.rerender(
      <RightSidebar
        {...props}
        browserSurfaceCommand={{
          schemaVersion: 1,
          kind: 'closeSurface',
          requestId: '33333333-3333-4333-8333-333333333333',
          surfaceId: SECOND_SURFACE_ID,
          surfaceInstanceId: SURFACE_INSTANCE_ID
        }}
      />
    )
    await expect.poll(() => marked().length).toBe(0)
    expect(screen.container.querySelectorAll('[role="tab"]')).toHaveLength(1)
  })

  it('creates once, reuses the exact surface, and only closes the requested browser page', async () => {
    const ready = vi.fn(async (input: BrowserSurfaceReadyInput) => appliedReady(input))
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
      surfaceId,
      surfaceInstanceId: SURFACE_INSTANCE_ID
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

    const staleClose: BrowserSurfaceCommand = {
      schemaVersion: 1,
      kind: 'closeSurface',
      requestId: '33333333-3333-4333-8333-333333333333',
      surfaceInstanceId: 'instance-stale-0000001',
      surfaceId: surfaceId!
    }
    await screen.rerender(
      <RightSidebar
        browserSurfaceCommand={staleClose}
        isMaximized={false}
        isOpen
        modules={[BROWSER_MODULE]}
        onBrowserSurfaceReady={ready}
        onToggleMaximized={NOOP}
      />
    )
    expect(getSurface(screen.container)).toBe(surface)

    const close: BrowserSurfaceCommand = {
      schemaVersion: 1,
      kind: 'closeSurface',
      requestId: '33333333-3333-4333-8333-333333333334',
      surfaceInstanceId: SURFACE_INSTANCE_ID,
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

  it('retries a typed transient readiness response with the same exact instance', async () => {
    const ready = vi
      .fn<(input: BrowserSurfaceReadyInput) => Promise<BrowserSurfaceReadyOutput>>()
      .mockImplementationOnce(async (input) => ({
        schemaVersion: 1,
        accepted: false,
        status: 'noop',
        reason: 'not_registered',
        retryable: true,
        requestId: input.requestId,
        surfaceId: input.surfaceId,
        surfaceInstanceId: input.surfaceInstanceId
      }))
      .mockImplementation(async (input) => appliedReady(input))
    const command = ensureCommand('77777777-7777-4777-8777-777777777777')
    const screen = await render(
      <RightSidebar
        browserSurfaceCommand={command}
        isMaximized={false}
        isOpen
        modules={[BROWSER_MODULE]}
        onBrowserSurfaceReady={ready}
        onToggleMaximized={NOOP}
      />
    )

    await expect.poll(() => ready).toHaveBeenCalledTimes(2)
    expect(ready.mock.calls[0]?.[0]).toEqual(ready.mock.calls[1]?.[0])
    expect(getSurface(screen.container).dataset.surfaceId).toBe(SURFACE_ID)
  })

  it('creates a background popup tab and selects it only on an explicit Host command', async () => {
    const ready = vi.fn(async (input: BrowserSurfaceReadyInput) => appliedReady(input))
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
  const {
    browserSurfaceRequest,
    onBrowserSurfaceInstance,
    onBrowserSurfaceReady,
    onBrowserAutomationTargetChange
  } = useRightSidebarRuntimeContext()
  const requestId =
    browserSurfaceRequest?.pageId === page.id ? browserSurfaceRequest.requestId : null
  const surfaceId =
    page.moduleState?.kind === 'browser-surface'
      ? page.moduleState.surfaceId
      : browserSurfaceIdForPage(page.id)

  useEffect(() => {
    targetUpdates.set(surfaceId, (instanceId, active) => {
      onBrowserAutomationTargetChange?.(surfaceId, instanceId, active)
    })
    return () => {
      targetUpdates.delete(surfaceId)
    }
  }, [surfaceId, onBrowserAutomationTargetChange])

  useEffect(() => {
    surfaceLifecycle('mount', page.id)
    onBrowserSurfaceInstance?.(page.id, surfaceId, SURFACE_INSTANCE_ID, true)
    return () => {
      onBrowserSurfaceInstance?.(page.id, surfaceId, SURFACE_INSTANCE_ID, false)
      surfaceLifecycle('unmount', page.id)
    }
  }, [onBrowserSurfaceInstance, page.id, surfaceId])

  useEffect(() => {
    if (requestId) {
      onBrowserSurfaceReady?.(page.id, surfaceId, requestId, SURFACE_INSTANCE_ID)
    }
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

function appliedReady(input: BrowserSurfaceReadyInput): BrowserSurfaceReadyOutput {
  return {
    schemaVersion: 1,
    accepted: true,
    status: 'applied',
    reason: 'surface_ready',
    retryable: false,
    requestId: input.requestId,
    surfaceId: input.surfaceId,
    ...(input.surfaceInstanceId ? { surfaceInstanceId: input.surfaceInstanceId } : {})
  }
}
