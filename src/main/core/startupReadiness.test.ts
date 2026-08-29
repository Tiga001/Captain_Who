import type { IpcMainInvokeEvent } from 'electron'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it, vi } from 'vitest'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { registerStartupReadiness } from '../startupReadiness'

class FakeIpcMain {
  readonly handlers = new Map<string, (event: IpcMainInvokeEvent, ...args: unknown[]) => unknown>()
  readonly removeHandler = vi.fn((channel: string) => this.handlers.delete(channel))

  handle(
    channel: string,
    listener: (event: IpcMainInvokeEvent, ...args: unknown[]) => unknown
  ): void {
    this.handlers.set(channel, listener)
  }

  invoke(event = {} as IpcMainInvokeEvent): Promise<unknown> {
    const handler = this.handlers.get(HOST_CHANNELS.app.whenReady)
    if (!handler) throw new Error('Missing startup readiness handler')
    return Promise.resolve().then(() => handler(event))
  }
}

describe('application startup readiness', () => {
  it('creates the existing Renderer window before starting slow Host services', () => {
    const mainEntry = readFileSync(resolve('src/main/index.ts'), 'utf8')
    const readinessIndex = mainEntry.indexOf(
      'startupReadiness = registerStartupReadiness(ipcMain, isTrustedRendererEvent)'
    )
    const shortcutListenerIndex = mainEntry.indexOf("app.on('browser-window-created'")
    const createWindowIndex = mainEntry.indexOf('createWindow()', readinessIndex)
    const coreStartIndex = mainEntry.indexOf('coreServer.start()', createWindowIndex)
    const hostIpcIndex = mainEntry.indexOf('disposeHostIpc = registerHostIpc(', coreStartIndex)
    const readyIndex = mainEntry.indexOf('startupReadiness.markReady()', hostIpcIndex)

    expect(readinessIndex).toBeGreaterThan(-1)
    expect(shortcutListenerIndex).toBeGreaterThan(-1)
    expect(shortcutListenerIndex).toBeLessThan(createWindowIndex)
    expect(createWindowIndex).toBeGreaterThan(readinessIndex)
    expect(coreStartIndex).toBeGreaterThan(createWindowIndex)
    expect(hostIpcIndex).toBeGreaterThan(coreStartIndex)
    expect(readyIndex).toBeGreaterThan(hostIpcIndex)
  })

  it('holds trusted renderers until Host initialization is complete', async () => {
    const ipcMain = new FakeIpcMain()
    const readiness = registerStartupReadiness(ipcMain, () => true)
    const waiting = ipcMain.invoke()
    let settled = false
    void waiting.finally(() => {
      settled = true
    })

    await Promise.resolve()
    expect(settled).toBe(false)
    readiness.markReady()
    await expect(waiting).resolves.toBeUndefined()
    await expect(ipcMain.invoke()).resolves.toBeUndefined()
  })

  it('rejects untrusted callers without enrolling them as waiters', async () => {
    const ipcMain = new FakeIpcMain()
    const readiness = registerStartupReadiness(ipcMain, () => false)

    await expect(ipcMain.invoke()).rejects.toThrow('Blocked untrusted')
    readiness.markReady()
    await expect(ipcMain.invoke()).rejects.toThrow('Blocked untrusted')
  })

  it('fails closed with a stable message and no internal cause', async () => {
    const ipcMain = new FakeIpcMain()
    const readiness = registerStartupReadiness(ipcMain, () => true)
    const waiting = ipcMain.invoke()

    readiness.markFailed()

    await expect(waiting).rejects.toThrow('MyCopilot application startup failed')
    await expect(ipcMain.invoke()).rejects.toThrow('MyCopilot application startup failed')
  })

  it('disposes the exact handler and rejects outstanding waits', async () => {
    const ipcMain = new FakeIpcMain()
    const readiness = registerStartupReadiness(ipcMain, () => true)
    const waiting = ipcMain.invoke()

    readiness.dispose()

    await expect(waiting).rejects.toThrow('startup was cancelled')
    expect(ipcMain.removeHandler).toHaveBeenCalledWith(HOST_CHANNELS.app.whenReady)
    expect(ipcMain.handlers.has(HOST_CHANNELS.app.whenReady)).toBe(false)
  })
})
