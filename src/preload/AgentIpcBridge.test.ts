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
