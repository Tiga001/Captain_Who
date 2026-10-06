import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const rpc = vi.hoisted(() => ({
  request: vi.fn(),
  stop: vi.fn(),
  beginShutdown: vi.fn(),
  isRunning: vi.fn()
}))

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = rpc.request
    readonly stop = rpc.stop
    readonly beginShutdown = rpc.beginShutdown
    readonly isRunning = rpc.isRunning
  }
}))

import { CoreServer } from './coreServer'

describe('Host pending approval refresh', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.resetAllMocks()
  })
  afterEach(() => vi.useRealTimers())

  it('coordinates all facade callers while still validating each fresh snapshot', async () => {
    rpc.request.mockResolvedValue([])
    const server = new CoreServer()
    const first = server.listPendingActions()
    const burst = Array.from({ length: 100 }, () => server.listPendingActions())
    expect(rpc.request).toHaveBeenCalledExactlyOnceWith('agent.listPendingActions')
    expect(burst.every((request) => request === burst[0])).toBe(true)
    await expect(first).resolves.toEqual([])
    await vi.advanceTimersByTimeAsync(100)
    await expect(Promise.all(burst)).resolves.toHaveLength(100)
    expect(rpc.request).toHaveBeenCalledTimes(2)

    rpc.request.mockResolvedValueOnce({ invalid: 'snapshot' })
    const invalid = expect(server.listPendingActions()).rejects.toThrow()
    await vi.advanceTimersByTimeAsync(100)
    await invalid
    const recovery = server.listPendingActions()
    await vi.advanceTimersByTimeAsync(999)
    expect(rpc.request).toHaveBeenCalledTimes(3)
    await vi.advanceTimersByTimeAsync(1)
    await expect(recovery).resolves.toEqual([])
  })

  it('does not revive Core from queued refreshes after stop or shutdown', async () => {
    rpc.request.mockResolvedValue([])
    rpc.isRunning.mockReturnValue(false)
    const server = new CoreServer()
    await server.listPendingActions()
    const stopped = expect(server.listPendingActions()).rejects.toThrow('refresh was stopped')
    server.stop()
    await stopped
    await vi.advanceTimersByTimeAsync(60_000)
    expect(rpc.stop).toHaveBeenCalledTimes(1)
    expect(rpc.request).toHaveBeenCalledTimes(1)

    await server.listPendingActions()
    const quitting = expect(server.listPendingActions()).rejects.toThrow('refresh was stopped')
    await server.shutdown()
    await quitting
    expect(rpc.beginShutdown).toHaveBeenCalledTimes(1)
    await expect(server.listPendingActions()).rejects.toThrow('refresh is closed')
    await vi.advanceTimersByTimeAsync(60_000)
    expect(rpc.request).toHaveBeenCalledTimes(2)
    expect(vi.getTimerCount()).toBe(0)
  })
})
