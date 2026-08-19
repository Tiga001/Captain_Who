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
      requestId: '2fd21ed7-4255-4f4d-8f74-23a4c95ee895'
    })
    expect(handler).toHaveBeenCalledTimes(1)

    listener?.({} as IpcRendererEvent, {
      schemaVersion: 1,
      kind: 'ensureAttached',
      requestId: '2fd21ed7-4255-4f4d-8f74-23a4c95ee895',
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
})
