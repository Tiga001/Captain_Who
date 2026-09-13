import { HOST_CHANNELS } from '@mycopilot/host-api'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const getAllWindows = vi.hoisted(() => vi.fn())

vi.mock('electron', () => ({ BrowserWindow: { getAllWindows } }))

import { registerAgentIpc } from '../ipc/agentIpc'

describe('Main Agent IPC collaboration routing', () => {
  beforeEach(() => getAllWindows.mockReset())

  it('broadcasts profile invalidation to live windows only', () => {
    let listener: ((event: unknown) => void) | undefined
    const send = vi.fn()
    const destroyedSend = vi.fn()
    getAllWindows.mockReturnValue([
      { isDestroyed: () => false, webContents: { isDestroyed: () => false, send } },
      { isDestroyed: () => true, webContents: { isDestroyed: () => false, send: destroyedSend } }
    ])
    registerAgentIpc(
      { handle: vi.fn(), on: vi.fn() } as never,
      {
        onAgentEvent: vi.fn(),
        onProviderTransition: vi.fn(),
        onPromptPreferencesChanged: (handler: typeof listener) => {
          listener = handler
        }
      } as never
    )
    const event = { contextProfile: 'minimal', updatedAt: 7 }
    listener?.(event)
    expect(send).toHaveBeenCalledExactlyOnceWith(
      HOST_CHANNELS.agent.promptPreferencesChanged,
      event
    )
    expect(destroyedSend).not.toHaveBeenCalled()
  })

  it('routes global settings updates and broadcasts the persisted revision', async () => {
    let settingsListener: ((settings: unknown) => void) | undefined
    const send = vi.fn()
    getAllWindows.mockReturnValue([
      { isDestroyed: () => false, webContents: { isDestroyed: () => false, send } }
    ])
    const output = { enabled: false, revision: 2, updatedAt: 10 }
    const server = {
      onAgentEvent: vi.fn(),
      onProviderTransition: vi.fn(),
      onCollaborationSettingsChanged: vi.fn((listener) => {
        settingsListener = listener
      }),
      updateCollaborationSettings: vi.fn().mockResolvedValue(output)
    }
    const ipc = { handle: vi.fn(), on: vi.fn() }
    registerAgentIpc(ipc as never, server as never)
    const handler = ipc.handle.mock.calls.find(
      ([channel]) => channel === HOST_CHANNELS.agent.collaborationUpdateSettings
    )?.[1]
    const input = { enabled: false, expectedRevision: 1 }
    await expect(handler({}, input)).resolves.toEqual({ ok: true, value: output })
    expect(server.updateCollaborationSettings).toHaveBeenCalledWith(input)
    settingsListener?.(output)
    expect(send).toHaveBeenCalledWith(HOST_CHANNELS.agent.collaborationSettingsChanged, output)
  })

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
    registerAgentIpc(ipcMain as never, coreServer as never, () => undefined)
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

describe('Main Agent IPC approval-scope boundary', () => {
  function approvalHandler(coreServer: { approveAction: ReturnType<typeof vi.fn> }) {
    getAllWindows.mockReturnValue([])
    const server = {
      onAgentEvent: vi.fn(),
      onProviderTransition: vi.fn(),
      onCollaborationEvent: vi.fn(),
      onCollaborationObserverEvent: vi.fn(),
      onCollaborationResync: vi.fn(),
      ...coreServer
    }
    const ipcMain = { handle: vi.fn(), on: vi.fn() }
    registerAgentIpc(ipcMain as never, server as never)
    const registration = ipcMain.handle.mock.calls.find(
      ([channel]) => channel === HOST_CHANNELS.agent.approveAction
    )
    return registration?.[1] as ((event: unknown, input: unknown) => Promise<unknown>) | undefined
  }

  it.each(['singleAction', 'remainingApplyPatchInRun'] as const)(
    'forwards the strict %s scope unchanged',
    async (approvalScope) => {
      const approveAction = vi.fn().mockResolvedValue({ accepted: true })
      const handler = approvalHandler({ approveAction })
      const input = { runId: 'run-current', actionId: 'action-current', approvalScope }

      await expect(handler?.({}, input)).resolves.toEqual({ accepted: true })
      expect(approveAction).toHaveBeenCalledWith(input)
    }
  )

  it.each([
    { runId: 'run-current', actionId: 'action-current' },
    { runId: '', actionId: 'action-current', approvalScope: 'singleAction' },
    { runId: ' run-current', actionId: 'action-current', approvalScope: 'singleAction' },
    { runId: 'run-current', actionId: 'action-current ', approvalScope: 'singleAction' },
    { runId: 'run-current', actionId: 'action-current', approval_scope: 'singleAction' },
    {
      runId: 'run-current',
      actionId: 'action-current',
      approvalScope: 'singleAction',
      rememberForRun: true
    }
  ])('rejects an invalid approval request before Core dispatch: %#', (input) => {
    const approveAction = vi.fn()
    const handler = approvalHandler({ approveAction })

    expect(() => handler?.({}, input)).toThrow(/Invalid Agent approve action/)
    expect(approveAction).not.toHaveBeenCalled()
  })
})
