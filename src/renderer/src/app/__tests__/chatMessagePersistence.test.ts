import { afterEach, describe, expect, it, vi } from 'vitest'
import type { ChatMessage } from '../../features/chat/chatTypes'
import type { PendingMessageSave } from '../AppShellSupport'
import {
  CHAT_MESSAGE_CHECKPOINT_INTERVAL_MS,
  ChatMessagePersistenceQueue
} from '../chatMessagePersistence'

function assistantMessage(content: string, status: ChatMessage['status'] = 'pending'): ChatMessage {
  return {
    id: 'assistant-1',
    role: 'assistant',
    content,
    createdAt: 1,
    status
  }
}

function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve!: () => void
  const promise = new Promise<void>((nextResolve) => {
    resolve = nextResolve
  })
  return { promise, resolve }
}

afterEach(() => {
  vi.useRealTimers()
})

describe('ChatMessagePersistenceQueue', () => {
  it('periodically checkpoints a continuous 80ms render stream instead of trailing forever', async () => {
    vi.useFakeTimers()
    const save = vi.fn().mockResolvedValue(undefined)
    const queue = new ChatMessagePersistenceQueue(
      { save },
      new Map<string, PendingMessageSave>(),
      (error) => {
        throw error
      }
    )

    for (let elapsed = 0; elapsed < 720; elapsed += 80) {
      queue.scheduleCheckpoint('conversation-1', assistantMessage(`content-${elapsed}`))
      await vi.advanceTimersByTimeAsync(80)
    }
    await queue.flushAll()

    expect(save).toHaveBeenCalledTimes(3)
    expect(save.mock.calls.map(([payload]) => payload.message.content)).toEqual([
      'content-240',
      'content-560',
      'content-640'
    ])
  })

  it('keeps one save in flight and writes only the latest checkpoint queued behind it', async () => {
    vi.useFakeTimers()
    const firstSave = deferred()
    const save = vi
      .fn<(payload: PendingMessageSave) => Promise<void>>()
      .mockImplementationOnce(() => firstSave.promise)
      .mockResolvedValue(undefined)
    const queue = new ChatMessagePersistenceQueue(
      { save },
      new Map<string, PendingMessageSave>(),
      (error) => {
        throw error
      }
    )

    queue.scheduleCheckpoint('conversation-1', assistantMessage('one'))
    await vi.advanceTimersByTimeAsync(CHAT_MESSAGE_CHECKPOINT_INTERVAL_MS)
    expect(save).toHaveBeenCalledTimes(1)

    queue.scheduleCheckpoint('conversation-1', assistantMessage('two'))
    queue.scheduleCheckpoint('conversation-1', assistantMessage('three'))
    await vi.advanceTimersByTimeAsync(CHAT_MESSAGE_CHECKPOINT_INTERVAL_MS)
    expect(save).toHaveBeenCalledTimes(1)

    firstSave.resolve()
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(2))
    expect(save).toHaveBeenLastCalledWith(
      expect.objectContaining({ message: expect.objectContaining({ content: 'three' }) })
    )
  })

  it('fences a terminal immediate save behind an older in-flight checkpoint', async () => {
    vi.useFakeTimers()
    const firstSave = deferred()
    const save = vi
      .fn<(payload: PendingMessageSave) => Promise<void>>()
      .mockImplementationOnce(() => firstSave.promise)
      .mockResolvedValue(undefined)
    const queue = new ChatMessagePersistenceQueue(
      { save },
      new Map<string, PendingMessageSave>(),
      (error) => {
        throw error
      }
    )

    queue.scheduleCheckpoint('conversation-1', assistantMessage('streaming'))
    await vi.advanceTimersByTimeAsync(CHAT_MESSAGE_CHECKPOINT_INTERVAL_MS)
    queue.scheduleCheckpoint('conversation-1', assistantMessage('newer streaming'))
    queue.persistNow('conversation-1', assistantMessage('complete', 'sent'))

    expect(save).toHaveBeenCalledTimes(1)
    firstSave.resolve()
    await queue.flushMessage('conversation-1', 'assistant-1')

    expect(save).toHaveBeenCalledTimes(2)
    expect(save).toHaveBeenLastCalledWith({
      conversationId: 'conversation-1',
      message: assistantMessage('complete', 'sent')
    })
  })

  it('flushes only the requested conversation boundary', async () => {
    vi.useFakeTimers()
    const save = vi.fn().mockResolvedValue(undefined)
    const queue = new ChatMessagePersistenceQueue(
      { save },
      new Map<string, PendingMessageSave>(),
      (error) => {
        throw error
      }
    )

    queue.scheduleCheckpoint('conversation-1', assistantMessage('one'))
    queue.scheduleCheckpoint('conversation-2', {
      ...assistantMessage('two'),
      id: 'assistant-2'
    })
    await queue.flushConversation('conversation-1')

    expect(save).toHaveBeenCalledTimes(1)
    expect(save).toHaveBeenCalledWith(expect.objectContaining({ conversationId: 'conversation-1' }))
    await vi.advanceTimersByTimeAsync(CHAT_MESSAGE_CHECKPOINT_INTERVAL_MS)
    expect(save).toHaveBeenCalledTimes(2)
  })

  it('continues draining snapshots accepted while a quit flush is in flight', async () => {
    const firstSave = deferred()
    const save = vi
      .fn<(payload: PendingMessageSave) => Promise<void>>()
      .mockImplementationOnce(() => firstSave.promise)
      .mockResolvedValue(undefined)
    const queue = new ChatMessagePersistenceQueue(
      { save },
      new Map<string, PendingMessageSave>(),
      (error) => {
        throw error
      }
    )

    queue.scheduleCheckpoint('conversation-1', assistantMessage('one'))
    const quitFlush = queue.sealAndFlushAll()
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(1))
    queue.scheduleCheckpoint('conversation-1', assistantMessage('two'))

    firstSave.resolve()
    await quitFlush

    expect(save).toHaveBeenCalledTimes(2)
    expect(save).toHaveBeenLastCalledWith(
      expect.objectContaining({ message: expect.objectContaining({ content: 'two' }) })
    )
  })
})
