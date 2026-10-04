import type { StorageChatConversationRecord } from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const rpcRequest = vi.hoisted(() => vi.fn())

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = rpcRequest
  }
}))

import { CoreServer } from './coreServer'

function deferred<Result>() {
  let resolve!: (result: Result) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<Result>((accept, fail) => {
    resolve = accept
    reject = fail
  })
  return { promise, resolve, reject }
}

function conversation(id: string, updatedAt = 1): StorageChatConversationRecord {
  return { id, title: id, createdAt: 1, updatedAt, messages: [] }
}

describe('full conversation read scheduling', () => {
  beforeEach(() => rpcRequest.mockReset())

  it('serializes different histories and coalesces queued refresh bursts in FIFO order', async () => {
    const active = deferred<StorageChatConversationRecord>()
    const next = deferred<StorageChatConversationRecord>()
    rpcRequest
      .mockReturnValueOnce(active.promise)
      .mockReturnValueOnce(next.promise)
      .mockResolvedValueOnce(conversation('third'))
    const server = new CoreServer()

    const first = server.loadConversation('first')
    const burst = Array.from({ length: 50 }, () => server.loadConversation('second'))
    const third = server.loadConversation('third')
    expect(rpcRequest).toHaveBeenCalledExactlyOnceWith('storage.loadConversation', {
      conversationId: 'first'
    })
    expect(burst.every((read) => read === burst[0])).toBe(true)

    active.resolve(conversation('first'))
    await expect(first).resolves.toEqual(conversation('first'))
    expect(rpcRequest).toHaveBeenCalledTimes(2)
    expect(rpcRequest).toHaveBeenLastCalledWith('storage.loadConversation', {
      conversationId: 'second'
    })

    next.resolve(conversation('second'))
    await expect(Promise.all(burst)).resolves.toEqual(
      Array.from({ length: 50 }, () => conversation('second'))
    )
    await expect(third).resolves.toEqual(conversation('third'))
    expect(rpcRequest).toHaveBeenCalledTimes(3)
  })

  it('preserves a fresh trailing read after a write and does not cache completed history', async () => {
    const oldSnapshot = deferred<StorageChatConversationRecord>()
    rpcRequest.mockImplementation((method: string) => {
      if (method === 'storage.loadConversation') return oldSnapshot.promise
      return Promise.resolve(undefined)
    })
    const server = new CoreServer()
    const before = server.loadConversation('chat')
    await server.saveChatMessageState({
      conversationId: 'chat',
      message: { id: 'new-message', content: 'new content', status: 'sent' }
    })
    const after = server.loadConversation('chat')
    const anotherRefresh = server.loadConversation('chat')
    expect(after).not.toBe(before)
    expect(anotherRefresh).toBe(after)

    rpcRequest.mockResolvedValue(conversation('chat', 2))
    oldSnapshot.resolve(conversation('chat', 1))
    await expect(before).resolves.toEqual(conversation('chat', 1))
    await expect(after).resolves.toEqual(conversation('chat', 2))
    expect(rpcRequest.mock.calls.map(([method]) => method)).toEqual([
      'storage.loadConversation',
      'storage.saveChatMessageState',
      'storage.loadConversation'
    ])

    rpcRequest.mockResolvedValueOnce(conversation('chat', 3))
    await expect(server.loadConversation('chat')).resolves.toEqual(conversation('chat', 3))
    expect(rpcRequest).toHaveBeenCalledTimes(4)
  })

  it('releases the queue on failure without retrying or replacing the original error', async () => {
    const failed = deferred<StorageChatConversationRecord>()
    const error = new Error('original transport failure')
    rpcRequest.mockReturnValueOnce(failed.promise).mockResolvedValueOnce(null)
    const server = new CoreServer()
    const first = server.loadConversation('first')
    const rejected = expect(first).rejects.toBe(error)
    const next = server.loadConversation('next')

    failed.reject(error)
    await rejected
    await expect(next).resolves.toBeNull()
    expect(rpcRequest).toHaveBeenCalledTimes(2)
    expect(rpcRequest.mock.calls.map(([, params]) => params.conversationId)).toEqual([
      'first',
      'next'
    ])
  })

  it('also serializes bulk and observer history while keeping observer roots isolated', async () => {
    const active = deferred<StorageChatConversationRecord>()
    rpcRequest.mockImplementation((method: string) => {
      if (method === 'storage.loadConversation') return active.promise
      if (method === 'storage.loadConversations') return Promise.resolve([])
      return Promise.resolve(null)
    })
    const server = new CoreServer()
    const first = server.loadConversation('first')
    const all = server.loadConversations()
    const allAgain = server.loadConversations()
    const observer = server.loadCollaborationObserverConversation({
      rootConversationId: 'root-a',
      conversationId: 'child'
    })
    const sameObserver = server.loadCollaborationObserverConversation({
      rootConversationId: 'root-a',
      conversationId: 'child'
    })
    const otherRoot = server.loadCollaborationObserverConversation({
      rootConversationId: 'root-b',
      conversationId: 'child'
    })
    expect(allAgain).toBe(all)
    expect(sameObserver).toBe(observer)
    expect(otherRoot).not.toBe(observer)
    expect(rpcRequest).toHaveBeenCalledTimes(1)

    active.resolve(conversation('first'))
    await Promise.all([first, all, observer, otherRoot])
    expect(rpcRequest.mock.calls).toEqual([
      ['storage.loadConversation', { conversationId: 'first' }],
      ['storage.loadConversations'],
      [
        'agent.collaboration.loadObserverConversation',
        { rootConversationId: 'root-a', conversationId: 'child' }
      ],
      [
        'agent.collaboration.loadObserverConversation',
        { rootConversationId: 'root-b', conversationId: 'child' }
      ]
    ])
  })

  it('does not put lightweight reads or writes behind a full history response', async () => {
    const active = deferred<StorageChatConversationRecord>()
    rpcRequest.mockImplementation((method: string) => {
      if (method === 'storage.loadConversation') return active.promise
      return Promise.resolve([])
    })
    const server = new CoreServer()
    const history = server.loadConversation('chat')
    await expect(server.loadConversationMetas()).resolves.toEqual([])
    await server.saveChatMessageState({
      conversationId: 'chat',
      message: { id: 'message', content: 'delta', status: 'pending' }
    })
    expect(rpcRequest.mock.calls.map(([method]) => method)).toEqual([
      'storage.loadConversation',
      'storage.loadConversationMetas',
      'storage.saveChatMessageState'
    ])
    active.resolve(conversation('chat'))
    await history
  })
})
