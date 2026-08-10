import { HostInvocationError } from '@mycopilot/host-api'
import type { StorageChatConversationRecord } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'

const storage = vi.hoisted(() => ({ forkConversation: vi.fn() }))

vi.mock('../../../host/hostClient', () => ({ hostClient: { storage } }))

const { forkConversation } = await import('../../storage/storageClient')

const input = {
  sourceConversationId: 'conversation-1',
  forkPoint: { kind: 'assistant_reply', assistantMessageId: 'assistant-1' } as const,
  requestId: 'conversation-fork-request-1'
}

describe('conversation fork storage client', () => {
  it('unwraps a successful Host invocation before mapping the conversation', async () => {
    const conversation = {
      id: 'conversation-2',
      projectId: null,
      modelId: 'model-1',
      title: 'Fork',
      createdAt: 1,
      updatedAt: 1,
      messages: []
    } satisfies StorageChatConversationRecord
    storage.forkConversation.mockResolvedValueOnce({ ok: true, value: conversation })

    await expect(forkConversation(input)).resolves.toMatchObject({
      id: conversation.id,
      title: conversation.title
    })
    expect(storage.forkConversation).toHaveBeenCalledWith(input)
  })

  it('restores a typed HostInvocationError from the serializable failure envelope', async () => {
    const data = {
      type: 'conversation_fork',
      code: 'active_command_session',
      conversationId: input.sourceConversationId,
      activeSessionCount: 1
    }
    storage.forkConversation.mockResolvedValueOnce({
      ok: false,
      error: { message: 'Conversation fork was rejected.', code: -32000, data }
    })

    try {
      await forkConversation(input)
      expect.fail('a rejected fork must throw')
    } catch (error) {
      expect(error).toBeInstanceOf(HostInvocationError)
      expect(error).toMatchObject({ code: -32000, data })
    }
  })
})
