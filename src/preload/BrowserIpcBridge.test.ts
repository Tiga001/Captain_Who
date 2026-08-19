import type { IpcRenderer, IpcRendererEvent } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { describe, expect, it, vi } from 'vitest'
import { createBrowserIpcBridge } from './BrowserIpcBridge'

type BrowserIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>

describe('Browser IPC bridge', () => {
  it('strictly validates readiness on both sides of the invoke boundary', async () => {
    const input = {
      schemaVersion: 1,
      requestId: '2fd21ed7-4255-4f4d-8f74-23a4c95ee895',
      surfaceId: 'right-sidebar-browser-fixture'
    } as const
    const output = { schemaVersion: 1, accepted: true, surfaceId: input.surfaceId } as const
    const invoke = vi.fn(async (): Promise<unknown> => output)
    const bridge = createBrowserIpcBridge({
      invoke,
      on: vi.fn(),
      removeListener: vi.fn()
    } as unknown as BrowserIpcRenderer)

    await expect(bridge.surfaceReady(input)).resolves.toEqual({
      schemaVersion: 1,
      accepted: true,
      surfaceId: input.surfaceId
    })
    expect(invoke).toHaveBeenCalledWith(HOST_CHANNELS.browser.surfaceReady, input)

    const selection = { schemaVersion: 1, surfaceId: input.surfaceId } as const
    await expect(bridge.surfaceSelected(selection)).resolves.toEqual({
      schemaVersion: 1,
      accepted: true,
      surfaceId: input.surfaceId
    })
    expect(invoke).toHaveBeenCalledWith(HOST_CHANNELS.browser.surfaceSelected, selection)

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
      kind: 'ensureAttached',
      requestId: '2fd21ed7-4255-4f4d-8f74-23a4c95ee895',
      surfaceId: 'right-sidebar-browser-fixture',
      webContentsId: 99
    })
    listener?.({} as IpcRendererEvent, {
      schemaVersion: 1,
      kind: 'closeSurface',
      requestId: 'not-a-uuid',
      surfaceId: 'right-sidebar-browser-fixture'
    })
    expect(handler).toHaveBeenCalledTimes(1)

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
})
