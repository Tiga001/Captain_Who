import { beforeEach, describe, expect, it, vi } from 'vitest'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const rpcRequest = vi.hoisted(() => vi.fn())
const onNotification = vi.hoisted(() => vi.fn())

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = rpcRequest
    readonly onNotification = onNotification
  }
}))

import { CoreServer } from './coreServer'

const round5Scenario = JSON.parse(
  readFileSync(
    fileURLToPath(
      new URL(
        '../../../packages/protocol/fixtures/agent-collaboration-round5-scenario-v1.json',
        import.meta.url
      )
    ),
    'utf8'
  )
) as {
  runningTree: unknown
  observerConversations: unknown[]
  settledEventPage: { events: unknown[] }
}

const root = {
  agentId: 'agent-root',
  rootAgentId: 'agent-root',
  rootConversationId: 'conversation-root',
  parentAgentId: null,
  conversationId: 'conversation-root',
  projectId: 'project-1',
  taskName: 'root',
  taskPath: '/root',
  lifecycle: 'active',
  displayStatus: 'idle',
  latestActivityAt: 1,
  model: null
}

describe('CoreServer collaboration client', () => {
  beforeEach(() => {
    rpcRequest.mockReset()
    onNotification.mockReset().mockReturnValue(() => undefined)
  })

  it('strictly validates a materialized tree and healthy legacy lookup', async () => {
    rpcRequest
      .mockResolvedValueOnce({
        schemaVersion: 1,
        materialized: true,
        tree: {
          schemaVersion: 1,
          workspaceId: 'project-1',
          projectId: 'project-1',
          rootAgentId: 'agent-root',
          rootConversationId: 'conversation-root',
          agents: [root],
          lastSequence: 4
        }
      })
      .mockResolvedValueOnce({ schemaVersion: 1, materialized: false, tree: null })
    const server = new CoreServer()

    await expect(
      server.getCollaborationTree({ rootConversationId: 'conversation-root' })
    ).resolves.toMatchObject({ materialized: true, tree: { lastSequence: 4 } })
    await expect(
      server.getCollaborationTree({ rootConversationId: 'legacy-conversation' })
    ).resolves.toEqual({ schemaVersion: 1, materialized: false, tree: null })
  })

  it('strictly accepts the shared Round 5 tree, observer and event identities at JSON-RPC', async () => {
    rpcRequest
      .mockResolvedValueOnce({
        schemaVersion: 1,
        materialized: true,
        tree: round5Scenario.runningTree
      })
      .mockResolvedValueOnce(round5Scenario.observerConversations[0])
    let receiver: ((value: unknown) => void) | undefined
    onNotification.mockImplementation((_method, handler) => {
      receiver = handler
      return () => undefined
    })
    const server = new CoreServer()

    await expect(
      server.getCollaborationTree({ rootConversationId: 'conversation-root' })
    ).resolves.toMatchObject({ tree: { agents: expect.any(Array), lastSequence: 13 } })
    await expect(
      server.loadCollaborationObserverConversation({
        rootConversationId: 'conversation-root',
        conversationId: 'conversation-review'
      })
    ).resolves.toMatchObject({
      agentId: 'agent-review',
      conversationId: 'conversation-review'
    })

    const handler = vi.fn()
    server.onCollaborationEvent(handler)
    receiver?.(round5Scenario.settledEventPage.events.at(-1))
    expect(handler).toHaveBeenCalledWith(
      expect.objectContaining({ rootConversationId: 'conversation-root', sequence: 15 })
    )
  })

  it('rejects a structurally valid response routed from another root', async () => {
    rpcRequest.mockResolvedValue({
      schemaVersion: 1,
      materialized: true,
      tree: {
        schemaVersion: 1,
        workspaceId: 'project-1',
        projectId: 'project-1',
        rootAgentId: 'other-agent',
        rootConversationId: 'other-conversation',
        agents: [
          {
            ...root,
            agentId: 'other-agent',
            rootAgentId: 'other-agent',
            rootConversationId: 'other-conversation',
            conversationId: 'other-conversation'
          }
        ],
        lastSequence: 1
      }
    })
    await expect(
      new CoreServer().getCollaborationTree({ rootConversationId: 'conversation-root' })
    ).rejects.toThrow('Invalid collaboration tree response identity')
  })

  it('rejects an observer origin with an impossible actor combination', async () => {
    rpcRequest.mockResolvedValue({
      schemaVersion: 1,
      agentId: 'agent-child',
      rootConversationId: 'conversation-root',
      conversationId: 'conversation-child',
      projectId: 'project-1',
      modelId: 'model-1',
      title: 'Child',
      createdAt: 1,
      updatedAt: 2,
      messages: [
        {
          messageId: 'message-1',
          role: 'user',
          content: 'task',
          createdAt: 1,
          status: 'sent',
          inputOrigin: {
            kind: 'human',
            senderAgentId: 'forged-agent',
            sourceAgentMessageId: null,
            snapshotSourceConversationId: null,
            snapshotSourceMessageId: null
          },
          attachments: [],
          agentRunJson: null,
          uiStateJson: null
        }
      ]
    })
    await expect(
      new CoreServer().loadCollaborationObserverConversation({
        rootConversationId: 'conversation-root',
        conversationId: 'conversation-child'
      })
    ).rejects.toThrow()
  })

  it('strictly parses a Core generation resync notification', () => {
    let receiver: ((value: unknown) => void) | undefined
    onNotification.mockImplementation((_method, handler) => {
      receiver = handler
      return () => undefined
    })
    const handler = vi.fn()
    new CoreServer().onCollaborationResync(handler)

    receiver?.({ schemaVersion: 1, reason: 'core_started' })
    receiver?.({ schemaVersion: 1, reason: 'core_started', rootId: 'forged' })

    expect(onNotification).toHaveBeenCalledWith('agent.collaboration.resync', expect.any(Function))
    expect(handler).toHaveBeenCalledTimes(1)
    expect(handler).toHaveBeenCalledWith({ schemaVersion: 1, reason: 'core_started' })
  })

  it('strictly parses the exact child observer notification and drops a forged one', () => {
    let receiver: ((value: unknown) => void) | undefined
    onNotification.mockImplementation((_method, handler) => {
      receiver = handler
      return () => undefined
    })
    const handler = vi.fn()
    new CoreServer().onCollaborationObserverEvent(handler)
    const notification = {
      schemaVersion: 1,
      rootAgentId: 'agent-root',
      rootConversationId: 'conversation-root',
      agentId: 'agent-child',
      conversationId: 'conversation-child',
      runId: 'run-child',
      assistantMessageId: 'assistant-child',
      event: { type: 'message_delta', runId: 'run-child', delta: 'hello' }
    }
    receiver?.(notification)
    receiver?.({ ...notification, event: { ...notification.event, secret: 'forged' } })

    expect(onNotification).toHaveBeenCalledWith(
      'agent.collaboration.observerEvent',
      expect.any(Function)
    )
    expect(handler).toHaveBeenCalledTimes(1)
    expect(handler).toHaveBeenCalledWith(notification)
  })
})
