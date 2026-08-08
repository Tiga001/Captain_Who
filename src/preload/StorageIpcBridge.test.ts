import type { IpcRenderer } from 'electron'
import type { HostInvocationResult } from '@mycopilot/host-api'
import type { StorageChatConversationRecord } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { createStorageIpcBridge } from './StorageIpcBridge'

describe('Storage IPC bridge', () => {
  it('passes the structured conversation-fork result through without throwing', async () => {
    const response = {
      ok: false,
      error: {
        message: 'Conversation fork was rejected.',
        code: -32000,
        data: {
          type: 'conversation_fork',
          code: 'active_command_session',
          conversationId: 'conversation-1',
          activeSessionCount: 1
        }
      }
    } satisfies HostInvocationResult<StorageChatConversationRecord>
    const invoke = vi.fn().mockResolvedValue(response)
    const bridge = createStorageIpcBridge({ invoke } as unknown as Pick<IpcRenderer, 'invoke'>)
    const input = {
      requestId: 'conversation-fork-request-1',
      sourceConversationId: 'conversation-1',
      throughAssistantMessageId: 'assistant-1'
    }

    await expect(bridge.forkConversation(input)).resolves.toBe(response)
    expect(invoke).toHaveBeenCalledWith('host:storage.forkConversation', input)
  })
})
