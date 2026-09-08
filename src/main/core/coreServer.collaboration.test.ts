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

const template = {
  schemaVersion: 1 as const,
  templateId: 'template-1',
  projectIds: ['project-1'],
  machineKey: 'reviewer',
  name: 'Reviewer',
  description: 'Review work',
  instructions: 'Review the assigned work.',
  modelConfigId: 'model-1',
  modelDisplayName: 'Model 1',
  enabled: true,
  revision: 1,
  createdAt: 1,
  updatedAt: 1
}

describe('CoreServer collaboration client', () => {
  beforeEach(() => {
    rpcRequest.mockReset()
    onNotification.mockReset().mockReturnValue(() => undefined)
  })

  it('validates settings requests, responses and change notifications', async () => {
    const server = new CoreServer()
    const settings = { enabled: true, revision: 1, updatedAt: 0 }
    rpcRequest
      .mockResolvedValueOnce(settings)
      .mockResolvedValueOnce({ ...settings, enabled: false, revision: 2 })
    await expect(server.getCollaborationSettings({})).resolves.toEqual(settings)
    await server.updateCollaborationSettings({ enabled: false, expectedRevision: 1 })
    expect(rpcRequest).toHaveBeenNthCalledWith(1, 'agent.collaboration.settings.get', {})
    expect(rpcRequest).toHaveBeenNthCalledWith(2, 'agent.collaboration.settings.update', {
      enabled: false,
      expectedRevision: 1
    })
    expect(() =>
      server.updateCollaborationSettings({ enabled: false, expectedRevision: 0 })
    ).toThrow()
    const handler = vi.fn()
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    server.onCollaborationSettingsChanged(handler)
    const callback = onNotification.mock.calls.at(-1)?.[1]
    callback({ ...settings, enabled: 'yes' })
    expect(handler).not.toHaveBeenCalled()
    callback(settings)
    expect(handler).toHaveBeenCalledWith(settings)
    warn.mockRestore()
  })

  it('uses global template requests and enforces canonical project assignment responses', async () => {
    rpcRequest
      .mockResolvedValueOnce({ schemaVersion: 1, templates: [template] })
      .mockResolvedValueOnce(template)
    const server = new CoreServer()

    await expect(server.listAgentTemplates({ includeDisabled: true })).resolves.toEqual({
      schemaVersion: 1,
      templates: [template]
    })
    await expect(
      server.setAgentTemplateProjectAssignment({
        projectId: 'project-1',
        templateId: 'template-1',
        assigned: true
      })
    ).resolves.toEqual(template)
    expect(rpcRequest).toHaveBeenNthCalledWith(1, 'agent.collaboration.templates.list', {
      includeDisabled: true
    })
    expect(rpcRequest).toHaveBeenNthCalledWith(
      2,
      'agent.collaboration.templates.setProjectAssignment',
      { projectId: 'project-1', templateId: 'template-1', assigned: true }
    )
  })

  it('rejects noncanonical global template project ids and mismatched template identities', async () => {
    rpcRequest
      .mockResolvedValueOnce({
        schemaVersion: 1,
        templates: [{ ...template, projectIds: ['project-b', 'project-a'] }]
      })
      .mockResolvedValueOnce({ ...template, templateId: 'template-other' })
    const server = new CoreServer()

    await expect(server.listAgentTemplates({ includeDisabled: true })).rejects.toThrow()
    await expect(
      server.setAgentTemplateProjectAssignment({
        projectId: 'project-1',
        templateId: 'template-1',
        assigned: true
      })
    ).rejects.toThrow('Invalid Agent template response identity')
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
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
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
    expect(warning).toHaveBeenCalledWith('Ignored invalid Agent observer event', {
      category: 'validation_failed',
      eventType: 'message_delta',
      count: 1
    })
    warning.mockRestore()
  })

  it('forwards sustained valid observer tool input progress without invalid-event warnings', () => {
    let receiver: ((value: unknown) => void) | undefined
    onNotification.mockImplementation((_method, handler) => {
      receiver = handler
      return () => undefined
    })
    const handler = vi.fn()
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    new CoreServer().onCollaborationObserverEvent(handler)
    const notification = {
      schemaVersion: 1,
      rootAgentId: 'agent-root',
      rootConversationId: 'conversation-root',
      agentId: 'agent-child',
      conversationId: 'conversation-child',
      runId: 'run-child',
      assistantMessageId: 'assistant-child',
      event: {
        type: 'tool_input_progress',
        runId: 'run-child',
        streamId: 'stream-child',
        attempt: 1,
        toolCallIndex: 0,
        toolCallId: 'call-child',
        tool: 'apply_patch',
        receivedBytes: 1
      }
    }

    for (let index = 0; index < 268; index += 1) {
      receiver?.({
        ...notification,
        event: { ...notification.event, receivedBytes: index + 1 }
      })
    }

    expect(handler).toHaveBeenCalledTimes(268)
    expect(warning).not.toHaveBeenCalled()
    warning.mockRestore()
  })

  it('rate-limits invalid observer warnings without logging rejected payloads or identities', () => {
    let receiver: ((value: unknown) => void) | undefined
    onNotification.mockImplementation((_method, handler) => {
      receiver = handler
      return () => undefined
    })
    const handler = vi.fn()
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    new CoreServer().onCollaborationObserverEvent(handler)
    const notification = {
      schemaVersion: 1,
      rootAgentId: 'fixed-canary-root-id',
      rootConversationId: 'fixed-canary-root-conversation',
      agentId: 'fixed-canary-agent-id',
      conversationId: 'fixed-canary-conversation',
      runId: 'fixed-canary-run-id',
      assistantMessageId: 'fixed-canary-message-id',
      event: {
        type: 'tool_input_progress',
        runId: 'fixed-canary-run-id',
        secret: 'fixed-canary-content'
      }
    }

    for (let index = 0; index < 268; index += 1) receiver?.(notification)
    receiver?.({
      ...notification,
      event: { type: 'fixed-canary-event-type', runId: 'fixed-canary-run-id' }
    })

    expect(handler).not.toHaveBeenCalled()
    expect(warning.mock.calls).toEqual([
      [
        'Ignored invalid Agent observer event',
        { category: 'validation_failed', eventType: 'tool_input_progress', count: 1 }
      ],
      [
        'Ignored invalid Agent observer event',
        { category: 'validation_failed', eventType: 'tool_input_progress', count: 100 }
      ],
      [
        'Ignored invalid Agent observer event',
        { category: 'validation_failed', eventType: 'tool_input_progress', count: 200 }
      ],
      [
        'Ignored invalid Agent observer event',
        { category: 'validation_failed', eventType: 'unknown', count: 1 }
      ]
    ])
    expect(JSON.stringify(warning.mock.calls)).not.toContain('fixed-canary')
    warning.mockRestore()
  })

  it('does not misreport or rethrow observer handler failures as validation failures', () => {
    let receiver: ((value: unknown) => void) | undefined
    onNotification.mockImplementation((_method, handler) => {
      receiver = handler
      return () => undefined
    })
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
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
    new CoreServer().onCollaborationObserverEvent(() => {
      throw new Error('observer handler failed')
    })

    expect(() => receiver?.(notification)).not.toThrow()
    expect(warning).toHaveBeenCalledWith('Agent observer handler failed', {
      category: 'handler_failed',
      eventType: 'message_delta',
      count: 1
    })
    warning.mockRestore()
  })
})
