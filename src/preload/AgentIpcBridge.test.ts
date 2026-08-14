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

describe('Agent IPC bridge collaboration', () => {
  it('uses the dedicated snapshot, replay and observer channels', async () => {
    const invoke = vi.fn().mockResolvedValue({ ok: true, value: {} })
    const ipcRenderer = {
      invoke,
      on: vi.fn(),
      removeListener: vi.fn()
    } as unknown as Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>
    const bridge = createAgentIpcBridge(ipcRenderer)

    await bridge.getCollaborationTree({ rootConversationId: 'root-conversation' })
    await bridge.listCollaborationEvents({
      rootConversationId: 'root-conversation',
      afterSequence: 9,
      limit: 256
    })
    await bridge.loadCollaborationObserverConversation({
      rootConversationId: 'root-conversation',
      conversationId: 'child-conversation'
    })

    expect(invoke).toHaveBeenNthCalledWith(1, 'host:agent.collaboration.getTree', {
      rootConversationId: 'root-conversation'
    })
    expect(invoke).toHaveBeenNthCalledWith(2, 'host:agent.collaboration.listEvents', {
      rootConversationId: 'root-conversation',
      afterSequence: 9,
      limit: 256
    })
    expect(invoke).toHaveBeenNthCalledWith(3, 'host:agent.collaboration.loadObserverConversation', {
      rootConversationId: 'root-conversation',
      conversationId: 'child-conversation'
    })
  })

  it('subscribes and unsubscribes the collaboration event channel', () => {
    const on = vi.fn()
    const removeListener = vi.fn()
    const ipcRenderer = {
      invoke: vi.fn(),
      on,
      removeListener
    } as unknown as Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>
    const bridge = createAgentIpcBridge(ipcRenderer)
    const handler = vi.fn()
    const unsubscribe = bridge.onCollaborationEvent(handler)
    const listener = on.mock.calls[0]?.[1]
    const notification = {
      schemaVersion: 2,
      eventId: 'event-1',
      sequence: 1,
      workspaceId: 'project-1',
      projectId: 'project-1',
      rootAgentId: 'root-agent',
      rootConversationId: 'root-conversation',
      agentId: 'child-agent',
      conversationId: 'child-conversation',
      turnId: null,
      runId: null,
      messageId: null,
      kind: 'agent_created',
      resourceRevision: 1,
      activity: null,
      occurredAt: 100
    }
    listener({}, notification)

    expect(on).toHaveBeenCalledWith('host:agent.collaboration.event', expect.any(Function))
    expect(handler).toHaveBeenCalledWith(notification)
    unsubscribe()
    expect(removeListener).toHaveBeenCalledWith('host:agent.collaboration.event', listener)
  })

  it('subscribes and unsubscribes the dedicated child observer event channel', () => {
    const on = vi.fn()
    const removeListener = vi.fn()
    const ipcRenderer = {
      invoke: vi.fn(),
      on,
      removeListener
    } as unknown as Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>
    const bridge = createAgentIpcBridge(ipcRenderer)
    const handler = vi.fn()
    const unsubscribe = bridge.onCollaborationObserverEvent(handler)
    const listener = on.mock.calls[0]?.[1]
    const notification = {
      schemaVersion: 1,
      rootAgentId: 'root-agent',
      rootConversationId: 'root-conversation',
      agentId: 'child-agent',
      conversationId: 'child-conversation',
      runId: 'run-child',
      assistantMessageId: 'assistant-child',
      event: { type: 'message_delta', runId: 'run-child', delta: 'hello' }
    }
    listener({}, notification)

    expect(on).toHaveBeenCalledWith('host:agent.collaboration.observerEvent', expect.any(Function))
    expect(handler).toHaveBeenCalledWith(notification)
    unsubscribe()
    expect(removeListener).toHaveBeenCalledWith('host:agent.collaboration.observerEvent', listener)
  })

  it('releases 1,000 observer subscriptions without duplicate delivery or residue', () => {
    type Listener = (...args: unknown[]) => void
    const observerListeners = new Set<Listener>()
    const on = vi.fn((channel: string, listener: Listener) => {
      if (channel === 'host:agent.collaboration.observerEvent') {
        observerListeners.add(listener)
      }
    })
    const removeListener = vi.fn((channel: string, listener: Listener) => {
      if (channel === 'host:agent.collaboration.observerEvent') {
        observerListeners.delete(listener)
      }
    })
    const ipcRenderer = {
      invoke: vi.fn(),
      on,
      removeListener
    } as unknown as Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>
    const bridge = createAgentIpcBridge(ipcRenderer)
    const notification = {
      schemaVersion: 1 as const,
      rootAgentId: 'root-agent',
      rootConversationId: 'root-conversation',
      agentId: 'child-agent',
      conversationId: 'child-conversation',
      runId: 'run-child',
      assistantMessageId: 'assistant-child',
      event: { type: 'message_delta' as const, runId: 'run-child', delta: 'hello' }
    }
    let deliveries = 0

    for (let subscription = 0; subscription < 1_000; subscription += 1) {
      const unsubscribe = bridge.onCollaborationObserverEvent(() => {
        deliveries += 1
      })
      expect(observerListeners.size).toBe(1)
      for (const listener of observerListeners) listener({}, notification)
      unsubscribe()
      expect(observerListeners.size).toBe(0)
      for (const listener of observerListeners) listener({}, notification)
    }

    expect(deliveries).toBe(1_000)
    expect(on).toHaveBeenCalledTimes(1_000)
    expect(removeListener).toHaveBeenCalledTimes(1_000)
  })

  it('subscribes and unsubscribes the global collaboration resync channel', () => {
    const on = vi.fn()
    const removeListener = vi.fn()
    const ipcRenderer = {
      invoke: vi.fn(),
      on,
      removeListener
    } as unknown as Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>
    const bridge = createAgentIpcBridge(ipcRenderer)
    const handler = vi.fn()
    const unsubscribe = bridge.onCollaborationResync(handler)
    const listener = on.mock.calls[0]?.[1]
    const notification = { schemaVersion: 1, reason: 'core_started' }
    listener({}, notification)

    expect(on).toHaveBeenCalledWith('host:agent.collaboration.resync', expect.any(Function))
    expect(handler).toHaveBeenCalledWith(notification)
    unsubscribe()
    expect(removeListener).toHaveBeenCalledWith('host:agent.collaboration.resync', listener)
  })
})
