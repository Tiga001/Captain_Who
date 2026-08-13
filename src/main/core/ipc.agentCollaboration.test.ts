import { HOST_CHANNELS } from '@mycopilot/host-api'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const getAllWindows = vi.hoisted(() => vi.fn())

vi.mock('electron', () => ({ BrowserWindow: { getAllWindows } }))

import { registerAgentIpc } from '../ipc/agentIpc'

describe('Main Agent IPC collaboration routing', () => {
  beforeEach(() => getAllWindows.mockReset())

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
})
