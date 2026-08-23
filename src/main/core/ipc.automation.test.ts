import type { IpcMainInvokeEvent } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { TrustedIpcMain } from '../ipc/trustedIpc'

const getAllWindows = vi.hoisted(() => vi.fn())

vi.mock('electron', () => ({ BrowserWindow: { getAllWindows } }))

import { registerAutomationIpc } from '../ipc/automationIpc'

const event = { sender: {} } as IpcMainInvokeEvent

function createTrustedIpc(): {
  handlers: Map<string, (...args: unknown[]) => unknown>
  listeners: Map<string, (...args: unknown[]) => unknown>
  ipc: TrustedIpcMain
} {
  const handlers = new Map<string, (...args: unknown[]) => unknown>()
  const listeners = new Map<string, (...args: unknown[]) => unknown>()
  return {
    handlers,
    listeners,
    ipc: {
      handle: (channel, handler) => handlers.set(channel, handler as never),
      on: (channel, handler) => listeners.set(channel, handler as never)
    }
  }
}

function queuedRun() {
  return {
    schemaVersion: 1 as const,
    runId: 'automation-run-1',
    automationId: 'automation-1',
    configRevision: 1,
    triggerKind: 'manual' as const,
    scheduledFor: 100,
    status: 'queued' as const,
    conversationId: null,
    userMessageId: null,
    assistantMessageId: null,
    reportKind: 'unknown' as const,
    resultPreview: null,
    errorCode: null,
    errorMessage: null,
    attention: null,
    createdAt: 100,
    startedAt: null,
    completedAt: null,
    updatedAt: 100
  }
}

function createCore(overrides: Record<string, unknown> = {}) {
  return {
    onAutomationEvent: vi.fn(() => vi.fn()),
    onAutomationResync: vi.fn(() => vi.fn()),
    listAutomations: vi.fn(),
    getAutomation: vi.fn(),
    createAutomation: vi.fn(),
    updateAutomation: vi.fn(),
    setAutomationEnabled: vi.fn(),
    runAutomationNow: vi.fn().mockResolvedValue(queuedRun()),
    deleteAutomation: vi.fn(),
    listAutomationRuns: vi.fn(),
    getAutomationAttentionSummary: vi.fn(),
    acknowledgeAutomationAttention: vi.fn(),
    ...overrides
  }
}

describe('Main Automation IPC', () => {
  beforeEach(() => getAllWindows.mockReset().mockReturnValue([]))

  it('registers the exact invocation allowlist and parses both sides strictly', async () => {
    const trusted = createTrustedIpc()
    const core = createCore()
    registerAutomationIpc(trusted.ipc, core as never)

    expect([...trusted.handlers.keys()].sort()).toEqual(
      [
        HOST_CHANNELS.automations.list,
        HOST_CHANNELS.automations.get,
        HOST_CHANNELS.automations.create,
        HOST_CHANNELS.automations.update,
        HOST_CHANNELS.automations.setEnabled,
        HOST_CHANNELS.automations.runNow,
        HOST_CHANNELS.automations.delete,
        HOST_CHANNELS.automations.listRuns,
        HOST_CHANNELS.automations.attentionSummary,
        HOST_CHANNELS.automations.acknowledgeAttention
      ].sort()
    )

    await expect(
      trusted.handlers.get(HOST_CHANNELS.automations.list)?.(event, {
        schemaVersion: 1,
        limit: 20,
        privateState: true
      })
    ).resolves.toMatchObject({ ok: false })
    expect(core.listAutomations).not.toHaveBeenCalled()

    const input = {
      schemaVersion: 1 as const,
      automationId: 'automation-1',
      requestId: 'manual-request-1'
    }
    await expect(
      trusted.handlers.get(HOST_CHANNELS.automations.runNow)?.(event, input)
    ).resolves.toEqual({ ok: true, value: queuedRun() })
    expect(core.runAutomationNow).toHaveBeenCalledWith(input)

    core.runAutomationNow.mockResolvedValueOnce({ ...queuedRun(), privateState: true } as never)
    await expect(
      trusted.handlers.get(HOST_CHANNELS.automations.runNow)?.(event, input)
    ).resolves.toMatchObject({ ok: false })
  })

  it('isolates a closing window while broadcasting validated notifications', () => {
    let eventListener: ((value: unknown) => void) | undefined
    const unsubscribeEvent = vi.fn()
    const unsubscribeResync = vi.fn()
    const delivered = vi.fn()
    const failed = vi.fn(() => {
      throw new Error('window closed')
    })
    const destroyedSend = vi.fn()
    getAllWindows.mockReturnValue([
      { isDestroyed: () => false, webContents: { isDestroyed: () => false, send: failed } },
      { isDestroyed: () => false, webContents: { isDestroyed: () => false, send: delivered } },
      { isDestroyed: () => true, webContents: { isDestroyed: () => false, send: destroyedSend } }
    ])
    const core = createCore({
      onAutomationEvent: vi.fn((listener) => {
        eventListener = listener
        return unsubscribeEvent
      }),
      onAutomationResync: vi.fn(() => unsubscribeResync)
    })
    const trusted = createTrustedIpc()
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    const cleanup = registerAutomationIpc(trusted.ipc, core as never)
    const payload = {
      schemaVersion: 1,
      sequence: 1,
      eventId: 'automation-event-1',
      kind: 'created',
      automationId: 'automation-1',
      runId: null,
      resourceRevision: 1,
      occurredAt: 100
    }

    eventListener?.(payload)

    expect(failed).toHaveBeenCalledWith(HOST_CHANNELS.automations.event, payload)
    expect(delivered).toHaveBeenCalledWith(HOST_CHANNELS.automations.event, payload)
    expect(destroyedSend).not.toHaveBeenCalled()
    expect(warning).toHaveBeenCalledWith('Failed to broadcast Automation notification to a window')

    cleanup()
    expect(unsubscribeEvent).toHaveBeenCalledOnce()
    expect(unsubscribeResync).toHaveBeenCalledOnce()
  })

  it('replays the cached startup resync after the Renderer listener is ready', () => {
    let resyncListener: ((value: unknown) => void) | undefined
    const payload = {
      schemaVersion: 1,
      reason: 'core_started',
      lastSequence: 4,
      occurredAt: 100
    }
    const core = createCore({
      onAutomationResync: vi.fn((listener) => {
        resyncListener = listener
        return vi.fn()
      })
    })
    const trusted = createTrustedIpc()
    registerAutomationIpc(trusted.ipc, core as never)
    resyncListener?.(payload)
    expect(getAllWindows).toHaveBeenCalled()

    const send = vi.fn()
    const ready = trusted.listeners.get(HOST_CHANNELS.automations.resyncReady)
    ready?.({ sender: { isDestroyed: () => false, send } })

    expect(send).toHaveBeenCalledWith(HOST_CHANNELS.automations.resync, payload)
  })
})
