import type { IpcRenderer, IpcRendererEvent } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { describe, expect, it, vi } from 'vitest'
import { createBrowserIpcBridge } from './BrowserIpcBridge'

type BrowserIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>

describe('Browser IPC bridge', () => {
  it('validates surface actions, snapshots, and state events without admitting internal URLs', async () => {
    const state = {
      schemaVersion: 1 as const,
      surfaceId: 'right-sidebar-browser-fixture',
      surfaceInstanceId: 'instance-00000001',
      stateRevision: 3,
      url: 'https://example.test/path?q=value',
      title: 'Example',
      faviconUrl: null,
      canGoBack: false,
      canGoForward: false,
      isLoading: false,
      presentation: 'content' as const,
      loadError: null
    }
    const listeners = new Map<string, (event: IpcRendererEvent, value: unknown) => void>()
    const invoke = vi.fn(async (): Promise<unknown> => state)
    const bridge = createBrowserIpcBridge({
      invoke,
      on: vi.fn((channel, listener) => {
        listeners.set(channel, listener)
        return {} as IpcRenderer
      }),
      removeListener: vi.fn()
    })

    await expect(
      bridge.surfaceAction({
        schemaVersion: 1,
        surfaceId: state.surfaceId,
        surfaceInstanceId: state.surfaceInstanceId,
        action: 'navigate',
        url: state.url
      })
    ).resolves.toEqual(state)
    expect(invoke).toHaveBeenLastCalledWith(HOST_CHANNELS.browser.surfaceAction, {
      schemaVersion: 1,
      surfaceId: state.surfaceId,
      surfaceInstanceId: state.surfaceInstanceId,
      action: 'navigate',
      url: state.url
    })
    expect(() =>
      bridge.surfaceAction({
        schemaVersion: 1,
        surfaceId: state.surfaceId,
        surfaceInstanceId: state.surfaceInstanceId,
        action: 'navigate',
        url: 'data:text/html,unsafe'
      })
    ).toThrow('navigation URL')

    const handler = vi.fn()
    bridge.onSurfaceState(handler)
    listeners.get(HOST_CHANNELS.browser.surfaceStateChanged)?.({} as IpcRendererEvent, state)
    listeners.get(HOST_CHANNELS.browser.surfaceStateChanged)?.({} as IpcRendererEvent, {
      ...state,
      url: 'data:text/html,unsafe'
    })
    expect(handler).toHaveBeenCalledTimes(1)
    expect(handler).toHaveBeenCalledWith(state)
  })

  it('strictly validates readiness on both sides of the invoke boundary', async () => {
    const input = {
      schemaVersion: 1,
      requestId: '2fd21ed7-4255-4f4d-8f74-23a4c95ee895',
      surfaceId: 'right-sidebar-browser-fixture',
      surfaceInstanceId: 'instance-00000001'
    } as const
    const output = {
      schemaVersion: 1,
      accepted: true,
      status: 'applied',
      reason: 'surface_ready',
      retryable: false,
      requestId: input.requestId,
      surfaceId: input.surfaceId,
      surfaceInstanceId: input.surfaceInstanceId
    } as const
    const invoke = vi.fn(async (): Promise<unknown> => output)
    const bridge = createBrowserIpcBridge({
      invoke,
      on: vi.fn(),
      removeListener: vi.fn()
    } as unknown as BrowserIpcRenderer)

    await expect(bridge.surfaceReady(input)).resolves.toEqual({
      schemaVersion: 1,
      accepted: true,
      status: 'applied',
      reason: 'surface_ready',
      retryable: false,
      requestId: input.requestId,
      surfaceId: input.surfaceId,
      surfaceInstanceId: input.surfaceInstanceId
    })
    expect(invoke).toHaveBeenCalledWith(HOST_CHANNELS.browser.surfaceReady, input)

    invoke.mockResolvedValueOnce({
      schemaVersion: 1,
      accepted: false,
      status: 'stale',
      reason: 'request_expired',
      retryable: false,
      requestId: input.requestId,
      surfaceId: input.surfaceId,
      surfaceInstanceId: input.surfaceInstanceId
    })
    await expect(bridge.surfaceReady(input)).resolves.toEqual({
      schemaVersion: 1,
      accepted: false,
      status: 'stale',
      reason: 'request_expired',
      retryable: false,
      requestId: input.requestId,
      surfaceId: input.surfaceId,
      surfaceInstanceId: input.surfaceInstanceId
    })

    const selection = {
      schemaVersion: 1,
      surfaceId: input.surfaceId,
      surfaceInstanceId: null,
      selectionRevision: 1
    } as const
    invoke.mockResolvedValueOnce({
      schemaVersion: 1,
      status: 'noop',
      reason: 'instance_required',
      retryable: true,
      surfaceId: input.surfaceId,
      surfaceInstanceId: 'instance-00000001',
      selectionRevision: 1,
      authoritativeRevision: 0
    })
    await expect(bridge.surfaceSelected(selection)).resolves.toEqual({
      schemaVersion: 1,
      status: 'noop',
      reason: 'instance_required',
      retryable: true,
      surfaceId: input.surfaceId,
      surfaceInstanceId: 'instance-00000001',
      selectionRevision: 1,
      authoritativeRevision: 0
    })
    expect(invoke).toHaveBeenCalledWith(HOST_CHANNELS.browser.surfaceSelected, selection)

    invoke.mockResolvedValueOnce({
      schemaVersion: 1,
      status: 'stale',
      reason: 'selection_applied',
      retryable: false,
      surfaceId: input.surfaceId,
      surfaceInstanceId: 'instance-00000001',
      selectionRevision: 1,
      authoritativeRevision: 2
    })
    await expect(bridge.surfaceSelected(selection)).rejects.toThrow('selection reason')
    expect(() =>
      bridge.surfaceSelected({
        ...selection,
        surfaceId: null,
        surfaceInstanceId: 'instance-00000001'
      })
    ).toThrow('cannot carry an instance')

    invoke.mockResolvedValueOnce({ ...output, rawTargetId: 'forbidden' })
    await expect(bridge.surfaceReady(input)).rejects.toThrow('unknown fields')
    expect(() => bridge.surfaceReady({ ...input, requestId: 'renderer-chosen-id' })).toThrow(
      'request identity'
    )
  })

  it('drops malformed commands and removes the exact listener on unsubscribe', () => {
    let listener: ((event: IpcRendererEvent, value: unknown) => void) | undefined
    const on = vi.fn((_channel, nextListener) => {
      listener = nextListener
      return {} as IpcRenderer
    })
    const removeListener = vi.fn(() => ({}) as IpcRenderer)
    const bridge = createBrowserIpcBridge({
      invoke: vi.fn(),
      on,
      removeListener
    } as unknown as BrowserIpcRenderer)
    const handler = vi.fn()
    const unsubscribe = bridge.onSurfaceCommand(handler)
    expect(on).toHaveBeenCalledWith(HOST_CHANNELS.browser.surfaceCommand, listener)

    listener?.({} as IpcRendererEvent, {
      schemaVersion: 1,
      kind: 'ensureAttached',
      requestId: '2fd21ed7-4255-4f4d-8f74-23a4c95ee895',
      surfaceId: 'right-sidebar-browser-fixture'
    })
    expect(handler).toHaveBeenCalledTimes(1)

    listener?.({} as IpcRendererEvent, {
      schemaVersion: 1,
      kind: 'closeSurface',
      requestId: '2fd21ed7-4255-4f4d-8f74-23a4c95ee895',
      surfaceId: 'right-sidebar-browser-fixture',
      surfaceInstanceId: 'instance-00000001'
    })
    expect(handler).toHaveBeenCalledTimes(2)

    listener?.({} as IpcRendererEvent, {
      schemaVersion: 1,
      kind: 'ensureAttached',
      requestId: '2fd21ed7-4255-4f4d-8f74-23a4c95ee895',
      surfaceId: 'right-sidebar-browser-fixture',
      webContentsId: 99
    })
    listener?.({} as IpcRendererEvent, {
      schemaVersion: 1,
      kind: 'closeSurface',
      requestId: '2fd21ed7-4255-4f4d-8f74-23a4c95ee895',
      surfaceId: 'right-sidebar-browser-fixture'
    })
    expect(handler).toHaveBeenCalledTimes(3)
    expect(handler).toHaveBeenLastCalledWith({
      schemaVersion: 1,
      kind: 'closeSurface',
      requestId: '2fd21ed7-4255-4f4d-8f74-23a4c95ee895',
      surfaceId: 'right-sidebar-browser-fixture'
    })

    unsubscribe()
    expect(removeListener).toHaveBeenCalledWith(HOST_CHANNELS.browser.surfaceCommand, listener)
  })

  it('reads only a bounded exact Browser Artifact preview identity', async () => {
    const artifact = {
      schemaVersion: 1,
      artifactId: 'browser-artifact:123e4567-e89b-42d3-a456-426614174000',
      kind: 'image',
      displayName: 'page.png',
      mimeType: 'image/png',
      sizeBytes: 2,
      createdAt: 1_000,
      expiresAt: 2_000,
      lifecycle: 'run',
      owner: 'browser_automation',
      preview: 'image'
    } as const
    const response = {
      ok: true,
      value: { schemaVersion: 1, artifact, bytes: Uint8Array.from([1, 2]) }
    } as const
    const invoke = vi.fn(async (): Promise<unknown> => response)
    const bridge = createBrowserIpcBridge({
      invoke,
      on: vi.fn(),
      removeListener: vi.fn()
    } as unknown as BrowserIpcRenderer)
    await expect(bridge.readArtifactPreview({ schemaVersion: 1, artifact })).resolves.toEqual(
      response
    )
    expect(invoke).toHaveBeenCalledWith(HOST_CHANNELS.browser.artifactReadPreview, {
      schemaVersion: 1,
      artifact
    })

    invoke.mockResolvedValueOnce({
      ok: true,
      value: { ...response.value, bytes: new Uint8Array(3) }
    })
    await expect(bridge.readArtifactPreview({ schemaVersion: 1, artifact })).rejects.toThrow(
      /byte length/
    )
    expect(() =>
      bridge.readArtifactPreview({
        schemaVersion: 1,
        artifact: { ...artifact, managedPath: '/tmp/page.png' }
      } as never)
    ).toThrow(/managedPath/)
  })

  it('exports only a complete Artifact identity and rejects path-bearing responses', async () => {
    const artifact = {
      schemaVersion: 1,
      artifactId: 'browser-artifact:123e4567-e89b-42d3-a456-426614174000',
      kind: 'pdf',
      displayName: 'page.pdf',
      mimeType: 'application/pdf',
      sizeBytes: 2,
      createdAt: 1_000,
      expiresAt: 2_000,
      lifecycle: 'run',
      owner: 'browser_automation',
      preview: 'none'
    } as const
    const invoke = vi.fn(async (): Promise<unknown> => ({
      ok: true,
      value: { schemaVersion: 1, status: 'exported', displayName: 'saved-page.pdf' }
    }))
    const bridge = createBrowserIpcBridge({
      invoke,
      on: vi.fn(),
      removeListener: vi.fn()
    } as unknown as BrowserIpcRenderer)

    await expect(bridge.exportArtifact({ schemaVersion: 1, artifact })).resolves.toEqual({
      ok: true,
      value: { schemaVersion: 1, status: 'exported', displayName: 'saved-page.pdf' }
    })
    expect(invoke).toHaveBeenCalledWith(HOST_CHANNELS.browser.artifactExport, {
      schemaVersion: 1,
      artifact
    })

    invoke.mockResolvedValueOnce({
      ok: true,
      value: {
        schemaVersion: 1,
        status: 'exported',
        displayName: 'saved-page.pdf',
        path: '/tmp/saved-page.pdf'
      }
    })
    await expect(bridge.exportArtifact({ schemaVersion: 1, artifact })).rejects.toThrow(/path/)
    expect(() =>
      bridge.exportArtifact({ schemaVersion: 1, artifact, path: '/tmp/page.pdf' } as never)
    ).toThrow(/path/)
  })
})
