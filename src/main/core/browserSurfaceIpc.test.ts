import type { WebContents } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { describe, expect, it, vi } from 'vitest'
import type { BrowserSurfaceManager } from '../browser/BrowserSurfaceManager'
import { registerBrowserSurfaceIpc } from '../ipc/browserSurfaceIpc'
import type { TrustedIpcMain } from '../ipc/trustedIpc'

describe('browser surface Main IPC', () => {
  it('strictly parses Renderer readiness before handing it to the manager', () => {
    let registered: ((event: { sender: WebContents }, input: unknown) => unknown) | undefined
    const ipcMain = {
      handle: vi.fn((channel, handler) => {
        expect(channel).toBe(HOST_CHANNELS.browser.surfaceReady)
        registered = handler as typeof registered
      }),
      on: vi.fn()
    } as unknown as TrustedIpcMain
    const sender = {} as WebContents
    const attach = vi.fn(() => ({
      schemaVersion: 1 as const,
      accepted: true as const,
      surfaceId: 'right-sidebar-browser-fixture'
    }))
    registerBrowserSurfaceIpc(ipcMain, { attach } as unknown as BrowserSurfaceManager)
    if (!registered) throw new Error('handler was not registered')

    const input = {
      schemaVersion: 1,
      requestId: '2fd21ed7-4255-4f4d-8f74-23a4c95ee895',
      surfaceId: 'right-sidebar-browser-fixture'
    }
    expect(registered({ sender }, input)).toEqual({
      schemaVersion: 1,
      accepted: true,
      surfaceId: input.surfaceId
    })
    expect(attach).toHaveBeenCalledWith(sender, input)

    expect(() => registered?.({ sender }, { ...input, webContentsId: 42 })).toThrow(
      'unknown fields'
    )
    expect(attach).toHaveBeenCalledTimes(1)
  })
})
