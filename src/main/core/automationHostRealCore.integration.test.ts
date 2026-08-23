import { execFileSync } from 'node:child_process'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import type { IpcRenderer } from 'electron'
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest'
import type { TrustedIpcMain } from '../ipc/trustedIpc'

const electronApp = vi.hoisted(() => ({
  getPath: vi.fn(),
  isPackaged: true
}))

vi.mock('electron', () => ({
  app: electronApp,
  BrowserWindow: {
    fromWebContents: () => null,
    getAllWindows: () => []
  },
  Notification: class {
    static isSupported(): boolean {
      return false
    }
  }
}))

import { createAutomationIpcBridge } from '../../preload/AutomationIpcBridge'
import { registerAutomationIpc } from '../ipc/automationIpc'
import { CoreServer } from './coreServer'

const workspaceRoot = resolve(__dirname, '../../..')
const originalResourcesPathDescriptor = Object.getOwnPropertyDescriptor(process, 'resourcesPath')
let appDataRoot = ''
let coreServer: CoreServer | null = null
let disposeAutomationIpc: (() => void) | null = null

function restoreResourcesPath(): void {
  if (originalResourcesPathDescriptor) {
    Object.defineProperty(process, 'resourcesPath', originalResourcesPathDescriptor)
  } else {
    Reflect.deleteProperty(process, 'resourcesPath')
  }
}

/** In-process Electron transport adapter; every handler beyond it is production code. */
function rendererTransport(
  ipcMain: TrustedIpcMain
): Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener' | 'send'> {
  const handlers = new Map<string, (...args: unknown[]) => unknown>()
  const listeners = new Map<string, (...args: unknown[]) => unknown>()
  ipcMain.handle = (channel, handler): void => {
    handlers.set(channel, handler as never)
  }
  ipcMain.on = (channel, handler): void => {
    listeners.set(channel, handler as never)
  }
  const sender = { isDestroyed: () => false, send: vi.fn() }
  return {
    invoke: ((channel: string, ...args: unknown[]) => {
      const handler = handlers.get(channel)
      if (!handler) throw new Error(`No Main handler registered for ${channel}`)
      return Promise.resolve(handler({ sender }, ...args))
    }) as IpcRenderer['invoke'],
    on: vi.fn() as never,
    removeListener: vi.fn() as never,
    send: ((channel: string, ...args: unknown[]) => {
      listeners.get(channel)?.({ sender }, ...args)
    }) as IpcRenderer['send']
  }
}

describe('Automation Renderer Host API to real core-server', () => {
  beforeAll(async () => {
    execFileSync(
      'cargo',
      ['build', '--locked', '-p', 'mycopilot-core-server', '--bin', 'core-server'],
      { cwd: workspaceRoot, stdio: 'inherit' }
    )
    Object.defineProperty(process, 'resourcesPath', {
      configurable: true,
      value: join(workspaceRoot, 'target', 'debug')
    })
    appDataRoot = await mkdtemp(join(tmpdir(), 'mycopilot-automation-core-e2e-'))
  }, 600_000)

  afterAll(async () => {
    disposeAutomationIpc?.()
    if (coreServer) await coreServer.shutdown()
    restoreResourcesPath()
    if (appDataRoot) await rm(appDataRoot, { force: true, recursive: true })
  })

  it('lists durable automations through the preload bridge, Main parser, JSON-RPC, and Rust', async () => {
    coreServer = new CoreServer({ appDataRoot })
    const trustedIpc = {} as TrustedIpcMain
    const ipcRenderer = rendererTransport(trustedIpc)
    disposeAutomationIpc = registerAutomationIpc(trustedIpc, coreServer)
    const automations = createAutomationIpcBridge(ipcRenderer)

    await expect(automations.list({ schemaVersion: 1, limit: 20 })).resolves.toEqual({
      ok: true,
      value: {
        schemaVersion: 1,
        tasks: [],
        nextCursor: null,
        counts: { all: 0, active: 0, paused: 0 },
        attentionCount: 0,
        lastSequence: 0
      }
    })
  }, 120_000)
})
