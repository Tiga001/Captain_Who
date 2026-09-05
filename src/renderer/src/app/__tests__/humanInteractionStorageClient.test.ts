import { describe, expect, it, vi } from 'vitest'
import type { HumanInteractionResponseDisplay } from '@mycopilot/protocol'
const storage = vi.hoisted(() => ({
  loadConversation: vi.fn(),
  upsertChatMessages: vi.fn(),
  saveChatMessageState: vi.fn()
}))
vi.mock('../../host/hostClient', () => ({ hostClient: { storage } }))
const { loadConversation, upsertChatMessages, saveChatMessageState } =
  await import('../../features/storage/storageClient')
const response: HumanInteractionResponseDisplay = {
  type: 'human_interaction_response',
  schemaVersion: 1,
  requestId: 'request',
  responseId: 'response',
  answers: [{ kind: 'text', questionId: 'q', question: 'Which?', answer: 'Blue' }]
}

describe('Renderer storage human answer metadata', () => {
  it('loads Host proof before request notifications and omits it from every message write', async () => {
    storage.loadConversation.mockResolvedValue({
      id: 'chat',
      title: 'chat',
      createdAt: 1,
      updatedAt: 1,
      messages: [
        {
          id: 'answer',
          role: 'user',
          content: JSON.stringify(response),
          createdAt: 1,
          humanInteractionResponse: response
        }
      ]
    })
    const chat = (await loadConversation('chat'))!
    expect(chat.messages[0].humanInteractionDisplay).toEqual(response)
    await upsertChatMessages('chat', chat.messages, 0)
    await saveChatMessageState('chat', chat.messages[0])
    for (const payload of [
      storage.upsertChatMessages.mock.calls[0][0].messages[0],
      storage.saveChatMessageState.mock.calls[0][0].message
    ]) {
      expect(payload).not.toHaveProperty('humanInteractionResponse')
      expect(payload).not.toHaveProperty('humanInteractionDisplay')
      expect(payload.content).toBe(JSON.stringify(response))
    }
  })
  it('rejects a mismatched proof instead of authorizing display for copied JSON', async () => {
    storage.loadConversation.mockResolvedValue({
      id: 'chat',
      title: 'chat',
      createdAt: 1,
      updatedAt: 1,
      messages: [
        {
          id: 'answer',
          role: 'user',
          content: JSON.stringify({ ...response, responseId: 'other' }),
          createdAt: 1,
          humanInteractionResponse: response
        }
      ]
    })
    await expect(loadConversation('chat')).rejects.toThrow('complete User content')
  })
})
