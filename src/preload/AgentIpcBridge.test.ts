import type { IpcRenderer } from 'electron'
import { describe, expect, it, vi } from 'vitest'
import { createAgentIpcBridge } from './AgentIpcBridge'

describe('Agent IPC bridge command Sessions', () => {
  it('uses dedicated, conversation-scoped list and get channels', async () => {
    const invoke = vi.fn().mockResolvedValue({ ok: true, value: { sessions: [] } })
    const ipcRenderer = {
      invoke,
      on: vi.fn(),
      removeListener: vi.fn()
    } as unknown as Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>
    const bridge = createAgentIpcBridge(ipcRenderer)

    await bridge.listCommandSessions({ conversationId: 'conversation-1' })
    await bridge.getCommandSession({
      conversationId: 'conversation-1',
      sessionId: 'cmd_1234567890abcdef1234567890abcdef',
      afterSequence: 7
    })

    expect(invoke).toHaveBeenNthCalledWith(1, 'host:agent.listCommandSessions', {
      conversationId: 'conversation-1'
    })
    expect(invoke).toHaveBeenNthCalledWith(2, 'host:agent.getCommandSession', {
      conversationId: 'conversation-1',
      sessionId: 'cmd_1234567890abcdef1234567890abcdef',
      afterSequence: 7
    })
  })
})

describe('Agent IPC bridge Provider transitions', () => {
  it('uses dedicated preflight, start and reload-status channels', async () => {
    const invoke = vi.fn().mockResolvedValue({ ok: true, value: {} })
    const ipcRenderer = {
      invoke,
      on: vi.fn(),
      removeListener: vi.fn()
    } as unknown as Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>
    const bridge = createAgentIpcBridge(ipcRenderer)

    await bridge.preflightProviderTransition({
      conversationId: 'conversation-1',
      targetModelId: 'generic-model'
    })
    await bridge.startProviderTransition({
      conversationId: 'conversation-1',
      targetModelId: 'generic-model',
      transitionToken: 'opaque-token'
    })
    await bridge.getProviderTransitionStatus({ conversationId: 'conversation-1' })

    expect(invoke).toHaveBeenNthCalledWith(1, 'host:agent.preflightProviderTransition', {
      conversationId: 'conversation-1',
      targetModelId: 'generic-model'
    })
    expect(invoke).toHaveBeenNthCalledWith(2, 'host:agent.startProviderTransition', {
      conversationId: 'conversation-1',
      targetModelId: 'generic-model',
      transitionToken: 'opaque-token'
    })
    expect(invoke).toHaveBeenNthCalledWith(3, 'host:agent.getProviderTransitionStatus', {
      conversationId: 'conversation-1'
    })
  })

  it('subscribes and unsubscribes the Provider transition notification channel', () => {
    const on = vi.fn()
    const removeListener = vi.fn()
    const ipcRenderer = {
      invoke: vi.fn(),
      on,
      removeListener
    } as unknown as Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>
    const bridge = createAgentIpcBridge(ipcRenderer)
    const handler = vi.fn()

    const unsubscribe = bridge.onProviderTransition(handler)
    const listener = on.mock.calls[0]?.[1]
    const event = {
      schemaVersion: 1,
      operationId: 'provider-transition-1',
      conversationId: 'conversation-1',
      targetModelId: 'generic-model',
      status: 'running',
      startedAt: 10
    }
    listener({}, event)

    expect(on).toHaveBeenCalledWith('host:agent.providerTransition', expect.any(Function))
    expect(handler).toHaveBeenCalledWith(event)
    unsubscribe()
    expect(removeListener).toHaveBeenCalledWith('host:agent.providerTransition', listener)
  })
})
