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

  it('performs durable CRUD, CAS, runNow, history, and attention calls through the real stack', async () => {
    coreServer = new CoreServer({ appDataRoot })
    const savedModelSettings = await coreServer.saveModelSettings({
      expectedRevision: null,
      apiUrl: 'https://example.invalid/v1/chat/completions',
      apiTokenMutation: { type: 'replace', value: 'automation-e2e-token' },
      searchMode: 'auto',
      tavilyApiKeyMutation: { type: 'keep' },
      models: [
        {
          id: null,
          providerModelId: 'automation-e2e-model',
          displayName: 'Automation E2E Model',
          apiUrlOverride: null,
          apiTokenOverrideMutation: { type: 'keep' },
          supportsImage: false,
          contextWindowTokens: 128_000,
          providerProfileUpdate: { kind: 'select_generic' },
          inputPrice: '0',
          cachedInputPrice: '',
          outputPrice: '0',
          enabled: true
        }
      ]
    })
    const modelConfigId = savedModelSettings.models[0].id
    const trustedIpc = {} as TrustedIpcMain
    const ipcRenderer = rendererTransport(trustedIpc)
    let executionRevision = 0
    disposeAutomationIpc = registerAutomationIpc(trustedIpc, coreServer, async () => {
      const issuedAt = Date.now()
      await coreServer!.setExecutionAccess({
        revision: ++executionRevision,
        identityEpoch: 1,
        reason: 'allowed',
        issuedAt,
        validUntil: issuedAt + 60_000
      })
    })
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

    const createResult = await automations.create({
      schemaVersion: 1,
      requestId: 'automation-e2e-create',
      status: 'paused',
      title: 'Durable frontend E2E',
      prompt: 'Return a short status update.',
      destination: {
        kind: 'new_chat',
        projectBinding: 'none',
        projectId: null,
        modelId: modelConfigId
      },
      permissionMode: 'default',
      permissionModeVersion: 2,
      schedule: {
        kind: 'daily',
        timeMinutes: 9 * 60,
        anchorAt: Date.now(),
        timezone: 'Asia/Shanghai'
      },
      notificationPolicy: 'all_runs'
    })
    expect(createResult.ok).toBe(true)
    if (!createResult.ok) throw new Error(createResult.error.message)
    expect(createResult.value).toMatchObject({
      schemaVersion: 1,
      status: 'paused',
      title: 'Durable frontend E2E'
    })
    const created = createResult.value

    const updateResult = await automations.update({
      schemaVersion: 1,
      automationId: created.automationId,
      expectedRevision: created.revision,
      title: 'Durable frontend E2E updated',
      prompt: created.prompt,
      destination: {
        kind: 'new_chat',
        projectBinding: 'none',
        projectId: null,
        modelId: modelConfigId
      },
      permissionMode: 'default',
      permissionModeVersion: 2,
      schedule: created.schedule,
      notificationPolicy: 'all_runs'
    })
    expect(updateResult.ok).toBe(true)
    if (!updateResult.ok) throw new Error(updateResult.error.message)
    expect(updateResult.value.revision).toBeGreaterThan(created.revision)

    const activeResult = await automations.setEnabled({
      schemaVersion: 1,
      automationId: created.automationId,
      expectedRevision: updateResult.value.revision,
      enabled: true
    })
    expect(activeResult.ok).toBe(true)
    if (!activeResult.ok) throw new Error(activeResult.error.message)
    expect(activeResult.value.status).toBe('active')

    const pausedResult = await automations.setEnabled({
      schemaVersion: 1,
      automationId: created.automationId,
      expectedRevision: activeResult.value.revision,
      enabled: false
    })
    expect(pausedResult.ok).toBe(true)
    if (!pausedResult.ok) throw new Error(pausedResult.error.message)
    expect(pausedResult.value.status).toBe('paused')

    const runNowResult = await automations.runNow({
      schemaVersion: 1,
      automationId: created.automationId,
      requestId: 'automation-e2e-run-now'
    })
    expect(runNowResult.ok).toBe(true)
    if (!runNowResult.ok) throw new Error(runNowResult.error.message)
    expect(runNowResult.value).toMatchObject({
      schemaVersion: 1,
      automationId: created.automationId,
      triggerKind: 'manual'
    })

    const runsResult = await automations.listRuns({
      schemaVersion: 1,
      automationId: created.automationId,
      limit: 20
    })
    expect(runsResult.ok).toBe(true)
    if (!runsResult.ok) throw new Error(runsResult.error.message)
    expect(runsResult.value.runs.map((run) => run.runId)).toContain(runNowResult.value.runId)

    const attentionResult = await automations.attentionSummary({
      schemaVersion: 1,
      limit: 20
    })
    expect(attentionResult.ok).toBe(true)

    const latestTaskResult = await automations.get({
      schemaVersion: 1,
      automationId: created.automationId
    })
    expect(latestTaskResult.ok).toBe(true)
    if (!latestTaskResult.ok) throw new Error(latestTaskResult.error.message)

    const deleteResult = await automations.delete({
      schemaVersion: 1,
      automationId: created.automationId,
      expectedRevision: latestTaskResult.value.revision
    })
    expect(deleteResult).toEqual({
      ok: true,
      value: {
        schemaVersion: 1,
        automationId: created.automationId,
        deletedAt: expect.any(Number)
      }
    })

    const finalList = await automations.list({ schemaVersion: 1, limit: 20 })
    expect(finalList).toMatchObject({
      ok: true,
      value: { tasks: [], counts: { all: 0, active: 0, paused: 0 } }
    })
  }, 120_000)
})
