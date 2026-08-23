import type { IpcRenderer } from 'electron'
import { describe, expect, it, vi } from 'vitest'
import { createAutomationIpcBridge } from './AutomationIpcBridge'

describe('Automation IPC bridge', () => {
  it('uses the dedicated automation channels', async () => {
    const invoke = vi.fn().mockResolvedValue({ ok: true, value: {} })
    const ipcRenderer = {
      invoke,
      on: vi.fn(),
      removeListener: vi.fn(),
      send: vi.fn()
    } as unknown as Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener' | 'send'>
    const bridge = createAutomationIpcBridge(ipcRenderer)
    const base = { schemaVersion: 1 as const, automationId: 'automation-1' }
    const config = {
      title: 'Daily brief',
      prompt: 'Summarize status.',
      destination: {
        kind: 'new_chat' as const,
        projectBinding: 'none' as const,
        projectId: null,
        modelId: 'model-1'
      },
      permissionMode: 'default' as const,
      permissionModeVersion: 2 as const,
      schedule: {
        kind: 'daily' as const,
        timeMinutes: 540,
        anchorAt: 1,
        timezone: 'Asia/Shanghai'
      },
      notificationPolicy: 'all_runs' as const
    }

    await bridge.list({ schemaVersion: 1, limit: 50 })
    await bridge.get(base)
    await bridge.create({
      schemaVersion: 1,
      requestId: 'create-request-1',
      status: 'active',
      ...config
    })
    await bridge.update({ ...base, expectedRevision: 1, ...config })
    await bridge.setEnabled({ ...base, expectedRevision: 2, enabled: false })
    await bridge.runNow({ ...base, requestId: 'request-1' })
    await bridge.delete({ ...base, expectedRevision: 3 })
    await bridge.listRuns({ ...base, limit: 20 })
    await bridge.attentionSummary({ schemaVersion: 1, limit: 20 })
    await bridge.acknowledgeAttention({ schemaVersion: 1, attentionId: 'task:automation-1' })

    expect(invoke).toHaveBeenNthCalledWith(1, 'host:automation.list', {
      schemaVersion: 1,
      limit: 50
    })
    expect(invoke).toHaveBeenNthCalledWith(2, 'host:automation.get', base)
    expect(invoke).toHaveBeenNthCalledWith(3, 'host:automation.create', {
      schemaVersion: 1,
      requestId: 'create-request-1',
      status: 'active',
      ...config
    })
    expect(invoke).toHaveBeenNthCalledWith(4, 'host:automation.update', {
      ...base,
      expectedRevision: 1,
      ...config
    })
    expect(invoke).toHaveBeenNthCalledWith(5, 'host:automation.setEnabled', {
      ...base,
      expectedRevision: 2,
      enabled: false
    })
    expect(invoke).toHaveBeenNthCalledWith(6, 'host:automation.runNow', {
      ...base,
      requestId: 'request-1'
    })
    expect(invoke).toHaveBeenNthCalledWith(7, 'host:automation.delete', {
      ...base,
      expectedRevision: 3
    })
    expect(invoke).toHaveBeenNthCalledWith(8, 'host:automation.runs.list', {
      ...base,
      limit: 20
    })
    expect(invoke).toHaveBeenNthCalledWith(9, 'host:automation.attention.summary', {
      schemaVersion: 1,
      limit: 20
    })
    expect(invoke).toHaveBeenNthCalledWith(10, 'host:automation.attention.acknowledge', {
      schemaVersion: 1,
      attentionId: 'task:automation-1'
    })
  })

  it('subscribes and unsubscribes event and resync channels', () => {
    const on = vi.fn()
    const removeListener = vi.fn()
    const send = vi.fn()
    const bridge = createAutomationIpcBridge({
      invoke: vi.fn(),
      on,
      removeListener,
      send
    } as unknown as Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener' | 'send'>)
    const onEvent = vi.fn()
    const onResync = vi.fn()
    const onOpenRequested = vi.fn()

    const unsubscribeEvent = bridge.onEvent(onEvent)
    const unsubscribeResync = bridge.onResync(onResync)
    const unsubscribeOpenRequested = bridge.onOpenRequested(onOpenRequested)
    const eventListener = on.mock.calls[0]?.[1]
    const resyncListener = on.mock.calls[1]?.[1]
    const openRequestedListener = on.mock.calls[2]?.[1]
    const event = {
      schemaVersion: 1,
      sequence: 1,
      eventId: 'event-1',
      kind: 'created',
      automationId: 'automation-1',
      runId: null,
      resourceRevision: 1,
      occurredAt: 10
    }
    const resync = { schemaVersion: 1, reason: 'core_started', lastSequence: 1, occurredAt: 11 }
    eventListener({}, event)
    resyncListener({}, resync)
    const openRequest = {
      schemaVersion: 1,
      automationId: 'automation-1',
      runId: 'run-1',
      destination: {
        kind: 'conversation',
        conversationId: 'conversation-1',
        messageId: 'message-1'
      }
    }
    openRequestedListener({}, openRequest)

    expect(onEvent).toHaveBeenCalledWith(event)
    expect(onResync).toHaveBeenCalledWith(resync)
    expect(onOpenRequested).toHaveBeenCalledWith(openRequest)
    openRequestedListener({}, { ...openRequest, privateState: true })
    expect(onOpenRequested).toHaveBeenCalledOnce()
    expect(send).toHaveBeenCalledTimes(2)
    expect(send).toHaveBeenNthCalledWith(1, 'host:automation.resyncReady')
    expect(send).toHaveBeenNthCalledWith(2, 'host:automation.resyncReady')
    unsubscribeEvent()
    unsubscribeResync()
    unsubscribeOpenRequested()
    expect(removeListener).toHaveBeenCalledWith('host:automation.event', eventListener)
    expect(removeListener).toHaveBeenCalledWith('host:automation.resync', resyncListener)
    expect(removeListener).toHaveBeenCalledWith(
      'host:automation.openRequested',
      openRequestedListener
    )
  })
})
