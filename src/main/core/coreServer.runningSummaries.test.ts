import { beforeEach, describe, expect, it, vi } from 'vitest'
const rpc = vi.hoisted(() => ({ request: vi.fn() }))
vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    request = rpc.request
  }
}))
import { CoreServer } from './coreServer'

describe('running conversation summaries', () => {
  beforeEach(() => {
    rpc.request.mockReset()
  })

  it('requests only the summary endpoint and validates the wire shape', async () => {
    const server = new CoreServer()
    const rows = [{ id: 'active', title: '', updatedAt: 2 }]
    rpc.request.mockResolvedValueOnce(rows)
    expect(await server.loadRunningConversationSummaries()).toEqual(rows)
    expect(rpc.request).toHaveBeenCalledExactlyOnceWith('storage.loadRunningConversationSummaries')
    rpc.request.mockResolvedValueOnce([{ ...rows[0], updatedAt: '2' }])
    await expect(server.loadRunningConversationSummaries()).rejects.toThrow()
    rpc.request.mockResolvedValueOnce([{ ...rows[0], messages: [] }])
    await expect(server.loadRunningConversationSummaries()).rejects.toThrow()
  })

  it('invalidates metadata mutations but not reads or streaming checkpoints', async () => {
    const server = new CoreServer(),
      changed = vi.fn()
    const stop = server.onConversationSummariesInvalidated(changed)
    rpc.request.mockResolvedValue(undefined)
    await server.saveConversationMeta({
      id: 'active',
      title: 'renamed',
      createdAt: 1,
      updatedAt: 2,
      archivedAt: 3
    })
    await server.deleteConversation('active')
    await server.deleteProject('project')
    expect(changed).toHaveBeenCalledTimes(3)
    await server.saveChatMessageState({
      conversationId: 'active',
      message: { id: 'm', content: 'delta', status: 'pending' }
    })
    await server.loadConversations()
    expect(changed).toHaveBeenCalledTimes(3)
    stop()
    await server.deleteConversation('active')
    expect(changed).toHaveBeenCalledTimes(3)
  })

  it('invalidates uncertain writes without changing their error outcome', async () => {
    const server = new CoreServer(),
      changed = vi.fn()
    server.onConversationSummariesInvalidated(changed)
    server.onConversationSummariesInvalidated(() => {
      throw new Error('listener')
    })
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const error = new Error('connection lost after commit')
    rpc.request.mockRejectedValue(error)
    await expect(server.deleteConversation('active')).rejects.toBe(error)
    expect(changed).toHaveBeenCalledOnce()
    warn.mockRestore()
  })
})
