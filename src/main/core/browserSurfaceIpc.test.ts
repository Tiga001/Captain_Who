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
    const attach = vi.fn(() => ({
      schemaVersion: 1 as const,
      accepted: true as const,
      surfaceId: 'right-sidebar-browser-fixture'
    }))
    const selectManualSurface = vi.fn((_sender: WebContents, input: { surfaceId: string }) => ({
      schemaVersion: 1 as const,
      accepted: true as const,
      surfaceId: input.surfaceId
    }))
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
      surfaceId: 'right-sidebar-browser-fixture'
    }
    expect(readyHandler({ sender }, input)).toEqual({
      schemaVersion: 1,
      accepted: true,
      surfaceId: input.surfaceId
    })
    expect(attach).toHaveBeenCalledWith(sender, input)

    expect(() => readyHandler({ sender }, { ...input, webContentsId: 42 })).toThrow(
      'unknown fields'
    )
    expect(attach).toHaveBeenCalledTimes(1)

    const selection = { schemaVersion: 1, surfaceId: input.surfaceId }
    expect(selectedHandler({ sender }, selection)).toEqual({
      schemaVersion: 1,
      accepted: true,
      surfaceId: input.surfaceId
    })
    expect(selectManualSurface).toHaveBeenCalledWith(sender, selection)
  })
})
