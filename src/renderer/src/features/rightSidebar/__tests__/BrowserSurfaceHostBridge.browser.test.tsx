import type { BrowserHostApi, HostApi, MyCopilotGlobal } from '@mycopilot/host-api'
import {
  BROWSER_DATA_SCHEMA_VERSION,
  BROWSER_DOWNLOAD_SCHEMA_VERSION,
  type BrowserSurfaceCommand
} from '@mycopilot/protocol'
import { useState } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import {
  resolveBrowserSurfaceHostApi,
  useBrowserSurfaceCommand
} from '../../browser/browserSurface'
import { useBrowserWebview } from '../../browser/useBrowserWebview'

vi.mock('../../../host/hostClient', () => ({
  hostClient: {
    resources: { resolveFavicon: vi.fn(async () => ({ url: null })) }
  }
}))

const originalMyCopilot = Object.getOwnPropertyDescriptor(window, 'mycopilot')

afterEach(() => {
  if (originalMyCopilot) {
    Object.defineProperty(window, 'mycopilot', originalMyCopilot)
  } else {
    Reflect.deleteProperty(window, 'mycopilot')
  }
})

describe('browser surface Host bridge availability', () => {
  it('does not subscribe when the Electron Host API is absent', async () => {
    Reflect.deleteProperty(window, 'mycopilot')
    const openRightSidebar = vi.fn()
    const screen = await render(<Harness openRightSidebar={openRightSidebar} />)

    await expect.element(screen.getByTestId('command')).toHaveTextContent('none')
    expect(openRightSidebar).not.toHaveBeenCalled()
  })

  it('does not subscribe when an older Host mock has no browser bridge', async () => {
    exposeHost({})
    const openRightSidebar = vi.fn()
    const screen = await render(<Harness openRightSidebar={openRightSidebar} />)

    await expect.element(screen.getByTestId('command')).toHaveTextContent('none')
    expect(openRightSidebar).not.toHaveBeenCalled()
  })

  it('subscribes to a complete bridge and cleans up the exact listener', async () => {
    let listener: ((command: BrowserSurfaceCommand) => void) | undefined
    const unsubscribe = vi.fn()
    const browser = createBrowserApi({
      onSurfaceCommand: vi.fn((nextListener) => {
        listener = nextListener
        return unsubscribe
      })
    })
    exposeHost({ browser })
    const openRightSidebar = vi.fn()
    const screen = await render(<Harness openRightSidebar={openRightSidebar} />)

    expect(browser.onSurfaceCommand).toHaveBeenCalledTimes(1)
    listener?.({
      schemaVersion: 1,
      kind: 'ensureAttached',
      requestId: '11111111-1111-4111-8111-111111111111',
      surfaceId: 'right-sidebar-browser-test'
    })
    await expect.element(screen.getByTestId('command')).toHaveTextContent('ensureAttached')
    expect(openRightSidebar).toHaveBeenCalledTimes(1)

    screen.unmount()
    expect(unsubscribe).toHaveBeenCalledTimes(1)
  })

  it('restores only the persisted logical URL when a recreated surface is blank', async () => {
    const restoredUrl = 'https://example.test/restored?view=logical'
    const surfaceAction = vi.fn(async (input) => ({
      ...emptySurfaceState(input),
      stateRevision: 1,
      url: restoredUrl,
      title: 'Unable to load',
      presentation: 'host-fallback' as const,
      loadError: {
        kind: 'offline' as const,
        errorCode: -106,
        errorDescription: 'ERR_INTERNET_DISCONNECTED',
        failedUrl: restoredUrl,
        title: 'Unable to load',
        heading: 'Unable to load',
        summary: 'Offline',
        suggestions: []
      }
    }))
    const surfaceState = vi.fn(async (input) => emptySurfaceState(input))
    exposeHost({ browser: createBrowserApi({ surfaceAction, surfaceState }) })

    const screen = await render(
      <BrowserStateHarness
        initialLogicalUrl={restoredUrl}
        surfaceInstanceId="instance-restore-0001"
      />
    )

    await expect.element(screen.getByTestId('logical-url')).toHaveTextContent(restoredUrl)
    expect(surfaceState).toHaveBeenCalledOnce()
    expect(surfaceAction).toHaveBeenCalledWith({
      schemaVersion: 1,
      surfaceId: 'right-sidebar-browser-restore',
      surfaceInstanceId: 'instance-restore-0001',
      action: 'navigate',
      url: restoredUrl
    })
  })

  it('snapshots the persisted logical URL before the blank bootstrap state resolves', async () => {
    const restoredUrl = 'https://example.test/persisted-before-bootstrap'
    const stateInput = {
      surfaceId: 'right-sidebar-browser-restore',
      surfaceInstanceId: 'instance-restore-0003'
    }
    let resolveSurfaceState!: (state: ReturnType<typeof emptySurfaceState>) => void
    const surfaceState = vi.fn(
      async () =>
        await new Promise<ReturnType<typeof emptySurfaceState>>((resolve) => {
          resolveSurfaceState = resolve
        })
    )
    const surfaceAction = vi.fn(async (input) => ({
      ...emptySurfaceState(input),
      stateRevision: 1,
      url: restoredUrl
    }))
    exposeHost({ browser: createBrowserApi({ surfaceAction, surfaceState }) })

    const screen = await render(
      <BrowserStateHarness
        initialLogicalUrl={restoredUrl}
        surfaceInstanceId={stateInput.surfaceInstanceId}
      />
    )
    expect(surfaceState).toHaveBeenCalledOnce()

    await screen.rerender(
      <BrowserStateHarness initialLogicalUrl="" surfaceInstanceId={stateInput.surfaceInstanceId} />
    )
    resolveSurfaceState(emptySurfaceState(stateInput))

    await expect
      .poll(() => surfaceAction)
      .toHaveBeenCalledWith({
        schemaVersion: 1,
        ...stateInput,
        action: 'navigate',
        url: restoredUrl
      })
    await expect.element(screen.getByTestId('logical-url')).toHaveTextContent(restoredUrl)
  })

  it('never restores a persisted private implementation URL', async () => {
    const surfaceAction = vi.fn(async (input) => emptySurfaceState(input))
    exposeHost({ browser: createBrowserApi({ surfaceAction }) })

    const screen = await render(
      <BrowserStateHarness
        initialLogicalUrl={`mycopilot-browser-internal://page/${'A'.repeat(32)}`}
        surfaceInstanceId="instance-restore-0002"
      />
    )

    await expect.element(screen.getByTestId('logical-url')).toHaveTextContent('blank')
    expect(surfaceAction).not.toHaveBeenCalled()
  })

  it('rejects a present but malformed browser bridge', () => {
    exposeHost({
      browser: {
        getPreferences: vi.fn(),
        onSurfaceCommand: 'not-a-function',
        surfaceReady: vi.fn()
      } as unknown as BrowserHostApi
    })

    expect(() => resolveBrowserSurfaceHostApi()).toThrow(
      'MyCopilot browser surface API is malformed'
    )
  })
})

function Harness({ openRightSidebar }: { openRightSidebar: () => void }) {
  const bridge = useBrowserSurfaceCommand(openRightSidebar)
  const [readyState, setReadyState] = useState('idle')
  return (
    <div>
      <output data-testid="command">{bridge.command?.kind ?? 'none'}</output>
      <output data-testid="ready">{readyState}</output>
      <button
        onClick={() => {
          void bridge
            .surfaceReady({
              schemaVersion: 1,
              requestId: '22222222-2222-4222-8222-222222222222',
              surfaceId: 'right-sidebar-browser-test'
            })
            .then(() => setReadyState('ready'))
        }}
        type="button"
      >
        ready
      </button>
    </div>
  )
}

function BrowserStateHarness({
  initialLogicalUrl,
  surfaceInstanceId
}: {
  initialLogicalUrl: string
  surfaceInstanceId: string
}) {
  const browser = useBrowserWebview({
    initialLogicalUrl,
    isActive: true,
    surfaceId: 'right-sidebar-browser-restore',
    surfaceInstanceId
  })
  return <output data-testid="logical-url">{browser.currentUrl ?? 'blank'}</output>
}

function createBrowserApi(overrides: Partial<BrowserHostApi> = {}): BrowserHostApi {
  return {
    getPreferences: vi.fn(async () => ({
      ok: true as const,
      value: {
        schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
        linkOpenTarget: 'system' as const,
        revision: 0,
        updatedAt: 0
      }
    })),
    updatePreferences: vi.fn(async (input) => ({
      ok: true as const,
      value: {
        schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
        linkOpenTarget: input.linkOpenTarget,
        revision: 1,
        updatedAt: 1
      }
    })),
    listHistory: vi.fn(async () => ({
      ok: true as const,
      value: { schemaVersion: BROWSER_DATA_SCHEMA_VERSION, entries: [], truncated: false }
    })),
    deleteHistory: vi.fn(async () => ({
      ok: true as const,
      value: { schemaVersion: BROWSER_DATA_SCHEMA_VERSION, deletedCount: 0 }
    })),
    openHistoryEntry: vi.fn(async () => ({ ok: true as const, value: undefined })),
    onHistoryChanged: vi.fn(() => () => undefined),
    getDataSummary: vi.fn(async () => ({
      ok: true as const,
      value: {
        schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
        historyCount: 0,
        historySiteCount: 0,
        downloadCount: 0,
        cookieSiteCount: 0,
        cacheBytes: 0
      }
    })),
    clearData: vi.fn(async () => ({
      ok: true as const,
      value: {
        schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
        deletedHistoryCount: 0,
        deletedDownloadCount: 0,
        clearedCookiesAndSiteData: false,
        clearedCache: false
      }
    })),
    getDownloadCenter: vi.fn(async () => ({
      ok: true as const,
      value: { schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION, revision: 0, downloads: [] }
    })),
    performDownloadCenterAction: vi.fn(async () => ({
      ok: true as const,
      value: {
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        status: 'unavailable' as const,
        snapshot: { schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION, revision: 0, downloads: [] }
      }
    })),
    openDownloadDirectory: vi.fn(async () => ({
      ok: true as const,
      value: { schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION, status: 'opened' as const }
    })),
    onDownloadCenterChanged: vi.fn(() => () => undefined),
    getDownloadSettings: vi.fn(async () => ({
      ok: true as const,
      value: {
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        locationMode: 'system' as const,
        displayPath: '~/Downloads',
        askWhereToSave: false,
        revision: 0,
        updatedAt: 1
      }
    })),
    chooseDownloadDirectory: vi.fn(async () => ({ ok: true as const, value: null })),
    resetDownloadDirectory: vi.fn(async () => ({
      ok: true as const,
      value: {
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        locationMode: 'system' as const,
        displayPath: '~/Downloads',
        askWhereToSave: false,
        revision: 0,
        updatedAt: 1
      }
    })),
    setDownloadAskWhereToSave: vi.fn(async () => ({
      ok: true as const,
      value: {
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        locationMode: 'system' as const,
        displayPath: '~/Downloads',
        askWhereToSave: false,
        revision: 0,
        updatedAt: 1
      }
    })),
    listDownloadHistory: vi.fn(async () => ({
      ok: true as const,
      value: { schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION, downloads: [], truncated: false }
    })),
    revealDownload: vi.fn(async () => ({
      ok: true as const,
      value: { schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION, status: 'shown' as const }
    })),
    clearDownloadHistory: vi.fn(async () => ({
      ok: true as const,
      value: { schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION, deletedCount: 0 }
    })),
    onDownloadHistoryChanged: vi.fn(() => () => undefined),
    exportArtifact: vi.fn(async () => ({
      ok: true as const,
      value: { schemaVersion: 1 as const, status: 'cancelled' as const }
    })),
    readArtifactPreview: vi.fn(async () => ({
      ok: false as const,
      error: { code: -32_001, message: 'not found' }
    })),
    onSurfaceCommand: vi.fn(() => () => undefined),
    onSurfaceState: vi.fn(() => () => undefined),
    surfaceAction: vi.fn(async (input) => emptySurfaceState(input)),
    surfaceReady: vi.fn(async () => ({
      schemaVersion: 1 as const,
      accepted: true as const,
      status: 'applied' as const,
      reason: 'surface_ready' as const,
      retryable: false as const,
      requestId: '22222222-2222-4222-8222-222222222222',
      surfaceId: 'right-sidebar-browser-test'
    })),
    surfaceSelected: vi.fn(async (input) => ({
      schemaVersion: 1 as const,
      status: 'noop' as const,
      reason: 'not_registered' as const,
      retryable: true as const,
      surfaceId: input.surfaceId,
      surfaceInstanceId: null,
      selectionRevision: input.selectionRevision,
      authoritativeRevision: 0
    })),
    surfaceState: vi.fn(async (input) => emptySurfaceState(input)),
    ...overrides
  }
}

function emptySurfaceState(input: { surfaceId: string; surfaceInstanceId: string }) {
  return {
    schemaVersion: 1 as const,
    surfaceId: input.surfaceId,
    surfaceInstanceId: input.surfaceInstanceId,
    stateRevision: 0,
    url: null,
    title: null,
    faviconUrl: null,
    canGoBack: false,
    canGoForward: false,
    isLoading: false,
    presentation: 'content' as const,
    loadError: null,
    crashError: null
  }
}

function exposeHost(host: Partial<HostApi>): void {
  Object.defineProperty(window, 'mycopilot', {
    configurable: true,
    value: { host } as unknown as MyCopilotGlobal
  })
}
