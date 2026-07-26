import type { StorageChatConversationRecord } from '@mycopilot/protocol'
import { expect, it, vi } from 'vitest'
import type { ChatMessage } from '../chatTypes'

const storage = vi.hoisted(() => ({
  loadConversation: vi.fn(),
  saveChatMessageState: vi.fn()
}))

vi.mock('../../../host/hostClient', () => ({
  hostClient: { storage }
}))

const { loadConversation, saveChatMessageState } = await import('../../storage/storageClient')

it('persists and restores a user message favorite through uiStateJson', async () => {
  const message: ChatMessage = {
    id: 'user-1',
    role: 'user',
    content: 'Keep this request',
    createdAt: 1,
    status: 'sent',
    uiState: { favorited: true }
  }

  storage.saveChatMessageState.mockResolvedValueOnce(undefined)
  await saveChatMessageState('conversation-1', message)

  expect(storage.saveChatMessageState).toHaveBeenCalledWith({
    conversationId: 'conversation-1',
    message: {
      id: 'user-1',
      content: 'Keep this request',
      status: 'sent',
      agentRunJson: null,
      uiStateJson: '{"favorited":true}'
    }
  })

  const storedConversation: StorageChatConversationRecord = {
    id: 'conversation-1',
    projectId: null,
    modelId: 'model-1',
    title: 'Favorites',
    createdAt: 1,
    updatedAt: 2,
    messages: [
      {
        id: 'user-1',
        role: 'user',
        content: 'Keep this request',
        createdAt: 1,
        status: 'sent',
        attachments: [],
        agentRunJson: null,
        uiStateJson: '{"favorited":true}'
      }
    ]
  }
  storage.loadConversation.mockResolvedValueOnce(storedConversation)

  const restored = await loadConversation('conversation-1')
  expect(restored?.messages[0]?.uiState).toEqual({ favorited: true })
})
