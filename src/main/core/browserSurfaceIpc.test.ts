import type { WebContents } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { describe, expect, it, vi } from 'vitest'
import type { BrowserSurfaceManager } from '../browser/BrowserSurfaceManager'
import { registerBrowserSurfaceIpc } from '../ipc/browserSurfaceIpc'
import type { TrustedIpcMain } from '../ipc/trustedIpc'

describe('browser surface Main IPC', () => {
  it('strictly parses Renderer readiness before handing it to the manager', () => {
    const registered = new Map<
      string,
      (event: { sender: WebContents }, input: unknown) => unknown
    >()
    const ipcMain = {
      handle: vi.fn((channel, handler) => {
        registered.set(channel, handler)
      }),
      on: vi.fn()
    } as unknown as TrustedIpcMain
    const sender = {} as WebContents
    const attach = vi.fn(
      (): ReturnType<BrowserSurfaceManager['attach']> => ({
        schemaVersion: 1,
        accepted: true,
        status: 'applied',
        reason: 'surface_ready',
        retryable: false,
        requestId: '2fd21ed7-4255-4f4d-8f74-23a4c95ee895',
        surfaceId: 'right-sidebar-browser-fixture',
        surfaceInstanceId: 'instance-00000001'
      })
    )
    const selectManualSurface = vi.fn(
      (
        _sender: WebContents,
        input: {
          surfaceId: string | null
          surfaceInstanceId: string | null
          selectionRevision: number
        }
      ) => ({
        schemaVersion: 1 as const,
        status: 'applied' as const,
        reason: 'selection_applied' as const,
        retryable: false as const,
        surfaceId: input.surfaceId,
        surfaceInstanceId: input.surfaceInstanceId,
        selectionRevision: input.selectionRevision,
        authoritativeRevision: input.selectionRevision
      })
    )
    registerBrowserSurfaceIpc(ipcMain, {
      attach,
      selectManualSurface
    } as unknown as BrowserSurfaceManager)
    const readyHandler = registered.get(HOST_CHANNELS.browser.surfaceReady)
    const selectedHandler = registered.get(HOST_CHANNELS.browser.surfaceSelected)
    if (!readyHandler || !selectedHandler) throw new Error('handlers were not registered')

    const input = {
      schemaVersion: 1,
      requestId: '2fd21ed7-4255-4f4d-8f74-23a4c95ee895',
      surfaceId: 'right-sidebar-browser-fixture',
      surfaceInstanceId: 'instance-00000001'
    }
    expect(readyHandler({ sender }, input)).toEqual({
      schemaVersion: 1,
      accepted: true,
      status: 'applied',
      reason: 'surface_ready',
      retryable: false,
      requestId: input.requestId,
      surfaceId: input.surfaceId,
      surfaceInstanceId: input.surfaceInstanceId
    })
    expect(attach).toHaveBeenCalledWith(sender, input)

    attach.mockReturnValueOnce({
      schemaVersion: 1,
      accepted: false,
      status: 'stale',
      reason: 'request_expired',
      retryable: false,
      requestId: input.requestId,
      surfaceId: input.surfaceId,
      surfaceInstanceId: input.surfaceInstanceId
    })
    expect(readyHandler({ sender }, input)).toEqual({
      schemaVersion: 1,
      accepted: false,
      status: 'stale',
      reason: 'request_expired',
      retryable: false,
      requestId: input.requestId,
      surfaceId: input.surfaceId,
      surfaceInstanceId: input.surfaceInstanceId
    })

    expect(() => readyHandler({ sender }, { ...input, webContentsId: 42 })).toThrow(
      'unknown fields'
    )
    expect(attach).toHaveBeenCalledTimes(2)

    const selection = {
      schemaVersion: 1,
      surfaceId: input.surfaceId,
      surfaceInstanceId: 'instance-00000001',
      selectionRevision: 1
    }
    expect(selectedHandler({ sender }, selection)).toEqual({
      schemaVersion: 1,
      status: 'applied',
      reason: 'selection_applied',
      retryable: false,
      surfaceId: input.surfaceId,
      surfaceInstanceId: 'instance-00000001',
      selectionRevision: 1,
      authoritativeRevision: 1
    })
    expect(selectManualSurface).toHaveBeenCalledWith(sender, selection)

    expect(() =>
      selectedHandler(
        { sender },
        { ...selection, surfaceId: null, surfaceInstanceId: 'instance-00000001' }
      )
    ).toThrow('cannot carry an instance')
    expect(selectManualSurface).toHaveBeenCalledTimes(1)
  })
})
