import { beforeEach, expect, it, vi } from 'vitest'
const { request, onNotification } = vi.hoisted(() => ({
  request: vi.fn(),
  onNotification: vi.fn()
}))
vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = request
    readonly onNotification = onNotification
  }
}))
import { CoreServer } from './coreServer'
const operation = {
  schemaVersion: 1,
  conversationId: 'chat',
  requestId: 'request',
  operationId: 'operation',
  status: 'running',
  phase: 'generating',
  startedAt: 10,
  updatedAt: 20
}
beforeEach(() => {
  request.mockReset()
  onNotification.mockReset().mockReturnValue(() => {})
})
it('routes the complete manual operation contract and validates ownership', async () => {
  const server = new CoreServer()
  request
    .mockResolvedValueOnce(operation)
    .mockResolvedValueOnce({ operations: [operation] })
    .mockResolvedValueOnce(operation)
  await server.startManualContextCompaction({ conversationId: 'chat', requestId: 'request' })
  await server.getManualContextCompactionStatus({ conversationId: 'chat' })
  await server.cancelManualContextCompaction({ conversationId: 'chat', operationId: 'operation' })
  expect(request.mock.calls.map((call) => call[0])).toEqual([
    'agent.startManualContextCompaction',
    'agent.getManualContextCompactionStatus',
    'agent.cancelManualContextCompaction'
  ])
  request.mockResolvedValue({ ...operation, conversationId: 'other' })
  await expect(
    server.startManualContextCompaction({ conversationId: 'chat', requestId: 'request' })
  ).rejects.toThrow('Unable to')
  request.mockRejectedValue(new Error('private provider body'))
  await expect(
    server.cancelManualContextCompaction({ conversationId: 'chat', operationId: 'operation' })
  ).rejects.toThrow('Unable to')
})
it('filters invalid notifications before reaching renderer', () => {
  const handler = vi.fn()
  new CoreServer().onManualContextCompaction(handler)
  const receive = onNotification.mock.calls[0]![1]
  receive(operation)
  receive({ ...operation, rawHistory: 'private' })
  expect(handler).toHaveBeenCalledExactlyOnceWith(operation)
})
