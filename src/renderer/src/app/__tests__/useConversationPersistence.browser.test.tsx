import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { useEffect } from 'react'
import { ensureAgentRun } from '../../features/agentRun/agentEventReducer'
import type { ChatConversation, ChatMessage } from '../../features/chat/chatTypes'

const storage = vi.hoisted(() => ({
  saveChatMessageUiState: vi.fn(),
  saveChatMessageState: vi.fn(),
  saveConversationMeta: vi.fn(),
  upsertChatMessages: vi.fn()
}))
vi.mock('../../features/storage/storageClient', () => storage)
const { useConversationPersistence } = await import('../useConversationPersistence')
let persistence: ReturnType<typeof useConversationPersistence>
function Harness() {
  const queue = useConversationPersistence()
  useEffect(() => {
    persistence = queue
  }, [queue])
  return null
}
function conversation(id = 'conversation-1', title = 'old'): ChatConversation {
  return {
    id,
    title,
    modelId: 'fake-model',
    projectId: null,
    messages: [],
    createdAt: 1,
    updatedAt: 2,
    pinnedAt: null,
    archivedAt: null,
    unreadAt: null
  }
}
function message(id: string): ChatMessage {
  return { id, content: id, role: 'user', status: 'sent', createdAt: 1 }
}
function deferred() {
  let resolve!: () => void
  let reject!: (error: unknown) => void
  const promise = new Promise<void>((nextResolve, nextReject) => {
    resolve = nextResolve
    reject = nextReject
  })
  return { promise, resolve, reject }
}

beforeEach(() => {
  for (const save of Object.values(storage)) save.mockReset().mockResolvedValue(undefined)
  vi.spyOn(console, 'error').mockImplementation(() => undefined)
})

afterEach(() => vi.restoreAllMocks())

describe('conversation persistence failure boundaries', () => {
  it('keeps Core checkpoint projections out of full saves while preserving independent UI state', async () => {
    await render(<Harness />)
    const assistant: ChatMessage = {
      ...message('core-assistant'),
      role: 'assistant',
      agentRun: {
        ...ensureAgentRun(undefined, 'core-run', 'running'),
        coreCheckpoint: {
          generation: '11111111-1111-4111-8111-111111111111',
          sequence: 1,
          fingerprint: 'a'.repeat(64)
        }
      }
    }
    persistence.enqueueChatMessageStateSave('conversation-1', assistant)
    persistence.enqueueChatMessageCheckpoint('conversation-1', assistant)
    persistence.enqueueChatMessageUiStateSave('conversation-1', assistant.id, { favorited: true })
    await persistence.sealAndFlushChatMessageStateSaves()
    expect(storage.saveChatMessageState).not.toHaveBeenCalled()
    expect(storage.saveChatMessageUiState).toHaveBeenCalledExactlyOnceWith(
      'conversation-1',
      assistant.id,
      { favorited: true }
    )
  })

  it('holds quit for metadata, UI and inserts even when no message-state write is pending', async () => {
    const metadata = deferred()
    const ui = deferred()
    storage.saveConversationMeta.mockReturnValueOnce(metadata.promise)
    storage.saveChatMessageUiState.mockReturnValueOnce(ui.promise)
    await render(<Harness />)
    persistence.enqueueConversationMetaSave(conversation())
    persistence.enqueueChatMessagesUpsert('conversation-1', [message('first')], 0)
    persistence.enqueueChatMessageUiStateSave('conversation-1', 'first', { favorited: true })
    const settled = vi.fn()
    const quit = persistence.sealAndFlushChatMessageStateSaves().then(settled)
    await Promise.resolve()
    expect(settled).not.toHaveBeenCalled()
    metadata.resolve()
    await expect.poll(() => storage.saveChatMessageUiState.mock.calls.length).toBe(1)
    expect(storage.upsertChatMessages).toHaveBeenCalledTimes(1)
    expect(settled).not.toHaveBeenCalled()
    persistence.enqueueChatMessageStateSave('conversation-1', message('second'))
    ui.resolve()
    await quit
    expect(storage.saveChatMessageState).toHaveBeenCalledTimes(1)
    expect(settled).toHaveBeenCalledOnce()
  })

  it('rejects quit on a UI-only failure and lets a later quit retry the same state', async () => {
    const ui = deferred()
    storage.saveChatMessageUiState.mockReturnValueOnce(ui.promise)
    await render(<Harness />)
    persistence.enqueueChatMessageUiStateSave('conversation-1', 'first', { favorited: true })
    const rejected = expect(persistence.sealAndFlushChatMessageStateSaves()).rejects.toThrow(
      'quit write failed'
    )
    await expect.poll(() => storage.saveChatMessageUiState.mock.calls.length).toBe(1)
    ui.reject(new Error('quit write failed'))
    await rejected
    expect(storage.saveChatMessageUiState).toHaveBeenCalledTimes(1)
    await persistence.sealAndFlushChatMessageStateSaves()
    expect(storage.saveChatMessageUiState.mock.calls[1]).toEqual(
      storage.saveChatMessageUiState.mock.calls[0]
    )
  })

  it('retains the latest metadata after failure and keeps another conversation independent', async () => {
    const first = deferred()
    storage.saveConversationMeta.mockReturnValueOnce(first.promise)
    await render(<Harness />)
    persistence.enqueueConversationMetaSave(conversation())
    const error = new Error('disk failure')
    const rejected = expect(persistence.waitForConversationSaves('conversation-1')).rejects.toBe(
      error
    )
    persistence.enqueueConversationMetaSave(conversation('conversation-1', 'newest'))
    persistence.enqueueConversationMetaSave(conversation('conversation-2'))
    await persistence.waitForConversationSaves('conversation-2')
    first.reject(error)
    await rejected

    expect(storage.saveConversationMeta).toHaveBeenCalledTimes(2)
    expect(persistence.pendingConversationSavesRef.current.get('conversation-1')?.title).toBe(
      'newest'
    )
    await persistence.waitForConversationSaves('conversation-1')
    expect(storage.saveConversationMeta).toHaveBeenLastCalledWith(
      conversation('conversation-1', 'newest')
    )
    expect(persistence.pendingConversationSavesRef.current.size).toBe(0)
  })

  it('retries the same upsert identities in FIFO order after a committed write loses its ACK', async () => {
    const first = deferred()
    const durable = new Map<string, ChatMessage>()
    storage.upsertChatMessages.mockImplementation(async (_id: string, messages: ChatMessage[]) => {
      for (const entry of messages) durable.set(entry.id, entry)
      if (storage.upsertChatMessages.mock.calls.length === 1) await first.promise
    })
    await render(<Harness />)
    persistence.enqueueChatMessagesUpsert('conversation-1', [message('first')], 0)
    const rejected = expect(persistence.waitForMessageUpserts('conversation-1')).rejects.toThrow(
      'ACK lost'
    )
    await expect.poll(() => storage.upsertChatMessages.mock.calls.length).toBe(1)
    persistence.enqueueChatMessagesUpsert('conversation-1', [message('second')], 1)
    first.reject(new Error('ACK lost'))
    await rejected
    expect(storage.upsertChatMessages).toHaveBeenCalledTimes(1)
    expect(durable.size).toBe(1)

    await persistence.waitForMessageUpserts('conversation-1')
    expect(storage.upsertChatMessages.mock.calls.map(([, messages]) => messages[0].id)).toEqual([
      'first',
      'first',
      'second'
    ])
    expect(storage.upsertChatMessages.mock.calls.map(([, , offset]) => offset)).toEqual([0, 0, 1])
    expect(durable.size).toBe(2)
    expect(persistence.pendingMessageUpsertsRef.current.size).toBe(0)
  })

  it('does not report a failed UI-state save as flushed and retries the latest UI state', async () => {
    const first = deferred()
    storage.saveChatMessageUiState.mockReturnValueOnce(first.promise)
    await render(<Harness />)
    persistence.enqueueChatMessageUiStateSave('conversation-1', 'assistant-1', { favorited: true })
    const rejected = expect(persistence.waitForMessageStateSaves('conversation-1')).rejects.toThrow(
      'UI unavailable'
    )
    await expect.poll(() => storage.saveChatMessageUiState.mock.calls.length).toBe(1)
    persistence.enqueueChatMessageUiStateSave('conversation-1', 'assistant-1', {
      favorited: false,
      timelineCollapsed: true
    })
    first.reject(new Error('UI unavailable'))
    await rejected
    expect(storage.saveChatMessageUiState).toHaveBeenCalledTimes(1)

    await persistence.waitForMessageStateSaves('conversation-1')
    expect(storage.saveChatMessageUiState).toHaveBeenLastCalledWith(
      'conversation-1',
      'assistant-1',
      {
        favorited: false,
        timelineCollapsed: true
      }
    )
  })

  it('keeps message state pending when its metadata prerequisite fails and retries in order', async () => {
    const first = deferred()
    storage.saveConversationMeta.mockReturnValueOnce(first.promise)
    await render(<Harness />)
    persistence.enqueueConversationMetaSave(conversation())
    persistence.enqueueChatMessageStateSave('conversation-1', message('assistant-1'))
    const rejected = expect(
      persistence.flushChatMessageStateSave('conversation-1', 'assistant-1')
    ).rejects.toThrow('metadata unavailable')
    first.reject(new Error('metadata unavailable'))
    await rejected
    expect(storage.saveChatMessageState).not.toHaveBeenCalled()
    expect(persistence.pendingMessageSavesRef.current.size).toBe(1)

    await persistence.flushChatMessageStateSave('conversation-1', 'assistant-1')
    expect(storage.saveConversationMeta).toHaveBeenCalledTimes(2)
    expect(storage.saveChatMessageState).toHaveBeenCalledTimes(1)
    expect(storage.saveConversationMeta.mock.invocationCallOrder[1]).toBeLessThan(
      storage.saveChatMessageState.mock.invocationCallOrder[0]
    )
  })
})
