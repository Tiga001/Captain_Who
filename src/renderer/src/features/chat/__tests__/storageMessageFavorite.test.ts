import type { StorageChatConversationRecord } from '@mycopilot/protocol'
import { beforeEach, expect, it, vi } from 'vitest'
import type { ChatMessage } from '../chatTypes'

const storage = vi.hoisted(() => ({
  loadConversation: vi.fn(),
  saveChatMessageState: vi.fn(),
  saveChatMessageUiState: vi.fn()
}))

vi.mock('../../../host/hostClient', () => ({
  hostClient: { storage }
}))

const { loadConversation, saveChatMessageState, saveChatMessageUiState } =
  await import('../../storage/storageClient')

beforeEach(() => {
  vi.clearAllMocks()
})

it('writes a favorite only through the dedicated UI-state API and restores its projection', async () => {
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
      agentRunJson: null
    }
  })

  storage.saveChatMessageUiState.mockResolvedValueOnce(undefined)
  await saveChatMessageUiState('conversation-1', message.id, message.uiState)

  expect(storage.saveChatMessageUiState).toHaveBeenCalledWith({
    conversationId: 'conversation-1',
    message: {
      id: 'user-1',
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

it('loads historical folder references with Rust storage identity field names', async () => {
  storage.loadConversation.mockResolvedValueOnce({
    id: 'conversation-folder-history',
    projectId: null,
    modelId: 'model-1',
    title: 'Folder history',
    createdAt: 1,
    updatedAt: 2,
    messages: [
      {
        id: 'user-folder-history',
        role: 'user',
        content: 'Inspect this folder',
        createdAt: 1,
        status: 'sent',
        folderReferencesJson: JSON.stringify([
          {
            schemaVersion: 1,
            id: 'folder-1',
            name: 'Playground',
            rootPath: '/Users/example/Playground',
            rootIdentity: {
              kind: 'unix',
              schema_version: 1,
              device: 1,
              inode: 2
            },
            status: 'available'
          }
        ])
      }
    ]
  })

  const restored = await loadConversation('conversation-folder-history')
  expect(restored?.messages[0]?.folderReferences).toEqual([
    {
      schemaVersion: 1,
      id: 'folder-1',
      name: 'Playground',
      rootPath: '/Users/example/Playground',
      rootIdentity: {
        kind: 'unix',
        schemaVersion: 1,
        device: 1,
        inode: 2
      },
      status: 'available'
    }
  ])
})

it('writes timeline collapse changes only through the dedicated UI-state API', async () => {
  storage.saveChatMessageUiState.mockResolvedValueOnce(undefined)

  await saveChatMessageUiState('conversation-1', 'assistant-1', { timelineCollapsed: true })

  expect(storage.saveChatMessageUiState).toHaveBeenCalledWith({
    conversationId: 'conversation-1',
    message: {
      id: 'assistant-1',
      uiStateJson: '{"timelineCollapsed":true}'
    }
  })
  expect(storage.saveChatMessageState).not.toHaveBeenCalled()
})
