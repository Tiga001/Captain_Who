import { HOST_CHANNELS } from '@mycopilot/host-api'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const getAllWindows = vi.hoisted(() => vi.fn())

vi.mock('electron', () => ({ BrowserWindow: { getAllWindows } }))

import { registerAgentIpc } from '../ipc/agentIpc'

describe('Main Agent IPC collaboration routing', () => {
  beforeEach(() => getAllWindows.mockReset())

  it('routes template project assignment through the strict invocation envelope', async () => {
    getAllWindows.mockReturnValue([])
    const output = { schemaVersion: 1, templateId: 'template-1', projectIds: ['project-1'] }
    const coreServer = {
      onAgentEvent: vi.fn(),
      onProviderTransition: vi.fn(),
      onCollaborationEvent: vi.fn(),
      onCollaborationObserverEvent: vi.fn(),
      onCollaborationResync: vi.fn(),
      setAgentTemplateProjectAssignment: vi.fn().mockResolvedValue(output)
    }
    const ipcMain = { handle: vi.fn(), on: vi.fn() }
    registerAgentIpc(ipcMain as never, coreServer as never)
    const registration = ipcMain.handle.mock.calls.find(
      ([channel]) => channel === HOST_CHANNELS.agent.collaborationTemplateSetProjectAssignment
    )
    const handler = registration?.[1]
    const input = { projectId: 'project-1', templateId: 'template-1', assigned: true }

    await expect(handler?.({}, input)).resolves.toEqual({ ok: true, value: output })
    expect(coreServer.setAgentTemplateProjectAssignment).toHaveBeenCalledWith(input)
  })

  it('routes the strict observer envelope only to live allowlisted renderer windows', () => {
    let observerListener: ((event: unknown) => void) | undefined
    const send = vi.fn()
    const liveWindow = {
      isDestroyed: () => false,
      webContents: { isDestroyed: () => false, send }
    }
    const destroyedWindow = {
      isDestroyed: () => true,
      webContents: { isDestroyed: () => false, send: vi.fn() }
    }
    getAllWindows.mockReturnValue([liveWindow, destroyedWindow])
    const coreServer = {
      onAgentEvent: vi.fn(),
      onProviderTransition: vi.fn(),
      onCollaborationEvent: vi.fn(),
      onCollaborationObserverEvent: vi.fn((listener) => {
        observerListener = listener
        return () => undefined
      }),
      onCollaborationResync: vi.fn()
    }
    const ipcMain = { handle: vi.fn(), on: vi.fn() }
    registerAgentIpc(ipcMain as never, coreServer as never)

    const envelope = {
      schemaVersion: 1,
      rootAgentId: 'agent-root',
      rootConversationId: 'conversation-root',
      agentId: 'agent-review',
      conversationId: 'conversation-review',
      runId: 'run-review',
      assistantMessageId: 'assistant-review',
      event: { type: 'message_delta', runId: 'run-review', streamId: 'stream-1', delta: 'safe' }
    }
    observerListener?.(envelope)

    expect(coreServer.onCollaborationObserverEvent).toHaveBeenCalledOnce()
    expect(send).toHaveBeenCalledOnce()
    expect(send).toHaveBeenCalledWith(HOST_CHANNELS.agent.collaborationObserverEvent, envelope)
    expect(destroyedWindow.webContents.send).not.toHaveBeenCalled()
    expect(ipcMain.handle).toHaveBeenCalledWith(
      HOST_CHANNELS.agent.collaborationLoadObserverConversation,
      expect.any(Function)
    )
  })

  it('routes an atomic Turn rewrite through the Agent invocation envelope', async () => {
    getAllWindows.mockReturnValue([])
    const output = {
      runId: 'run-new',
      eventName: 'agent.event',
      conversationId: 'conversation-root',
      userMessageId: 'user-new',
      assistantMessageId: 'assistant-new',
      userMessage: {
        id: 'user-new',
        role: 'user',
        content: 'corrected',
        createdAt: 10,
        status: 'sent'
      },
      assistantMessage: {
        id: 'assistant-new',
        role: 'assistant',
        content: '',
        createdAt: 11,
        status: 'pending'
      },
      activatedSkills: []
    }
    const coreServer = {
      onAgentEvent: vi.fn(),
      onProviderTransition: vi.fn(),
      onCollaborationEvent: vi.fn(),
      onCollaborationObserverEvent: vi.fn(),
      onCollaborationResync: vi.fn(),
      rewriteConversationTurn: vi.fn().mockResolvedValue(output)
    }
    const ipcMain = { handle: vi.fn(), on: vi.fn() }
    registerAgentIpc(ipcMain as never, coreServer as never)
    const registration = ipcMain.handle.mock.calls.find(
      ([channel]) => channel === HOST_CHANNELS.agent.rewriteConversationTurn
    )
    const handler = registration?.[1]
    const input = {
      requestId: 'rewrite-request-1',
      sourceUserMessageId: 'user-old',
      sourceAssistantMessageId: 'assistant-old',
      turn: {
        conversationId: 'conversation-root',
        modelId: 'generic-model',
        content: 'corrected',
        userMessageId: 'user-new',
        assistantMessageId: 'assistant-new'
      }
    }

    expect(handler).toBeTypeOf('function')
    await expect(handler?.({}, input)).resolves.toEqual({ ok: true, value: output })
    expect(coreServer.rewriteConversationTurn).toHaveBeenCalledWith(input)
  })
})
