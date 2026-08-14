import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { StorageChatConversationMetaRecord } from '@mycopilot/protocol'
import type { ChatConversation } from '../chatTypes'

const storage = vi.hoisted(() => ({ saveConversationMeta: vi.fn() }))

vi.mock('../../../host/hostClient', () => ({ hostClient: { storage } }))

const { saveConversationMeta } = await import('../../storage/storageClient')

const conversation: ChatConversation = {
  id: 'conversation-archive-fence',
  projectId: 'project-a',
  modelId: 'model-a',
  title: 'Archive fence',
  messages: [],
  messagesLoaded: true,
  createdAt: 1,
  updatedAt: 10,
  pinnedAt: null,
  archivedAt: null,
  unreadAt: 9
}

beforeEach(() => {
  storage.saveConversationMeta
    .mockReset()
    .mockImplementation(async (record: StorageChatConversationMetaRecord) => record)
})

describe('conversation archive write fence', () => {
  it('maps a renderer-only pending archive token without exposing it as archived UI state', async () => {
    await saveConversationMeta({ ...conversation, pendingArchivedAt: 11 })

    expect(storage.saveConversationMeta).toHaveBeenCalledWith({
      id: conversation.id,
      projectId: conversation.projectId,
      modelId: conversation.modelId,
      title: conversation.title,
      createdAt: conversation.createdAt,
      updatedAt: conversation.updatedAt,
      pinnedAt: null,
      archivedAt: 11,
      unreadAt: null
    })
    expect(conversation.archivedAt).toBeNull()
  })

  it('keeps the ordinary archived and unread fields unchanged without a pending fence', async () => {
    await saveConversationMeta(conversation)

    expect(storage.saveConversationMeta).toHaveBeenCalledWith(
      expect.objectContaining({ archivedAt: null, unreadAt: 9 })
    )
  })
})
