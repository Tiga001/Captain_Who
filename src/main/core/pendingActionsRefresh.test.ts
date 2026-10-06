import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { PendingActionsRefresh } from './pendingActionsRefresh'

function deferred<Result>() {
  let resolve!: (result: Result) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<Result>((accept, fail) => {
    resolve = accept
    reject = fail
  })
  return { promise, resolve, reject }
}

describe('pending action refresh coordination', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('bounds a burst to one active and one fresh trailing read without caching completed results', async () => {
    const firstCut = deferred<string[]>()
    const read = vi.fn().mockReturnValueOnce(firstCut.promise).mockResolvedValue(['new-approval'])
    const refresh = new PendingActionsRefresh<string[]>(read)
    const first = refresh.request()
    const burst = Array.from({ length: 100 }, () => refresh.request())
    expect(read).toHaveBeenCalledTimes(1)
    expect(burst.every((request) => request === burst[0])).toBe(true)
    expect(burst[0]).not.toBe(first)

    // A newly created approval belongs to the trailing read, even if the old read settles later.
    firstCut.resolve(['old-approval'])
    await expect(first).resolves.toEqual(['old-approval'])
    await vi.advanceTimersByTimeAsync(99)
    expect(read).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(1)
    await expect(Promise.all(burst)).resolves.toEqual(
      Array.from({ length: 100 }, () => ['new-approval'])
    )
    expect(read).toHaveBeenCalledTimes(2)

    read.mockResolvedValueOnce([])
    const afterDecision = refresh.request()
    await vi.advanceTimersByTimeAsync(100)
    await expect(afterDecision).resolves.toEqual([])
    expect(read).toHaveBeenCalledTimes(3)
  })

  it('uses a fixed window across IPC turns so repeated invalidations cannot starve a refresh', async () => {
    const read = vi.fn().mockResolvedValue([])
    const refresh = new PendingActionsRefresh(read)
    await refresh.request()
    const waiting = refresh.request()
    for (let index = 0; index < 9; index += 1) {
      await vi.advanceTimersByTimeAsync(10)
      expect(refresh.request()).toBe(waiting)
    }
    expect(read).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(10)
    await expect(waiting).resolves.toEqual([])
    expect(read).toHaveBeenCalledTimes(2)
    expect(vi.getTimerCount()).toBe(0)
  })

  it('keeps failure cooldown across new callers, retains one requested retry and does not poll by itself', async () => {
    const firstCut = deferred<unknown[]>()
    const unavailable = new Error('Core overloaded')
    const stillUnavailable = new Error('Core restarting')
    const read = vi
      .fn()
      .mockReturnValueOnce(firstCut.promise)
      .mockRejectedValueOnce(stillUnavailable)
      .mockResolvedValueOnce(['recovered'])
    const refresh = new PendingActionsRefresh(read)
    const failed = expect(refresh.request()).rejects.toBe(unavailable)
    const trailing = refresh.request()
    const trailingFailure = expect(trailing).rejects.toBe(stillUnavailable)
    firstCut.reject(unavailable)
    await failed
    for (let index = 0; index < 10; index += 1) {
      expect(refresh.request()).toBe(trailing)
      await vi.advanceTimersByTimeAsync(99)
    }
    expect(read).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(10)
    await trailingFailure
    expect(read).toHaveBeenCalledTimes(2)
    expect(vi.getTimerCount()).toBe(0)

    const recovered = refresh.request()
    expect(refresh.request()).toBe(recovered)
    await vi.advanceTimersByTimeAsync(1_999)
    expect(read).toHaveBeenCalledTimes(2)
    await vi.advanceTimersByTimeAsync(1)
    await expect(recovered).resolves.toEqual(['recovered'])
    await vi.advanceTimersByTimeAsync(60_000)
    expect(read).toHaveBeenCalledTimes(3)
    expect(vi.getTimerCount()).toBe(0)
  })

  it('bounds exponential cooldown and resets it only after success, including synchronous failures', async () => {
    const unavailable = new Error('Core is not running')
    const read = vi.fn((): Promise<unknown[]> => {
      throw unavailable
    })
    const refresh = new PendingActionsRefresh(read)
    await expect(refresh.request()).rejects.toBe(unavailable)
    for (const delay of [1_000, 2_000, 4_000, 8_000, 15_000, 15_000]) {
      const callsBefore = read.mock.calls.length
      const rejected = expect(refresh.request()).rejects.toBe(unavailable)
      await vi.advanceTimersByTimeAsync(delay - 1)
      expect(read).toHaveBeenCalledTimes(callsBefore)
      await vi.advanceTimersByTimeAsync(1)
      await rejected
    }

    read.mockResolvedValueOnce([])
    const recovered = refresh.request()
    await vi.advanceTimersByTimeAsync(15_000)
    await recovered
    const failingAgain = expect(refresh.request()).rejects.toBe(unavailable)
    await vi.advanceTimersByTimeAsync(100)
    await failingAgain
    read.mockResolvedValueOnce([])
    const next = refresh.request()
    await vi.advanceTimersByTimeAsync(1_000)
    await expect(next).resolves.toEqual([])
  })

  it('retires old active and trailing callers on stop and ignores an old Core completion', async () => {
    const oldCut = deferred<string[]>()
    const newCut = deferred<string[]>()
    const read = vi.fn().mockReturnValueOnce(oldCut.promise).mockReturnValueOnce(newCut.promise)
    const refresh = new PendingActionsRefresh<string[]>(read)
    const oldRequest = expect(refresh.request()).rejects.toThrow('refresh was stopped')
    const oldTrailing = expect(refresh.request()).rejects.toThrow('refresh was stopped')
    refresh.reset()
    await Promise.all([oldRequest, oldTrailing])
    const newRequest = refresh.request()
    expect(read).toHaveBeenCalledTimes(2)
    oldCut.reject(new Error('late old-process failure'))
    await vi.advanceTimersByTimeAsync(5_000)
    expect(read).toHaveBeenCalledTimes(2)
    newCut.resolve(['current'])
    await expect(newRequest).resolves.toEqual(['current'])
    expect(vi.getTimerCount()).toBe(0)
  })

  it('clears delayed work on stop and permanently prevents work after application shutdown', async () => {
    const unavailable = new Error('Core overloaded')
    const read = vi.fn().mockRejectedValueOnce(unavailable).mockResolvedValue([])
    const refresh = new PendingActionsRefresh(read)
    await expect(refresh.request()).rejects.toBe(unavailable)
    const waiting = expect(refresh.request()).rejects.toThrow('refresh was stopped')
    expect(vi.getTimerCount()).toBe(1)
    refresh.reset()
    await waiting
    await vi.advanceTimersByTimeAsync(60_000)
    expect(read).toHaveBeenCalledTimes(1)

    await refresh.request()
    const beforeQuit = expect(refresh.request()).rejects.toThrow('refresh was stopped')
    refresh.close()
    await beforeQuit
    await expect(refresh.request()).rejects.toThrow('refresh is closed')
    refresh.reset()
    await expect(refresh.request()).rejects.toThrow('refresh is closed')
    await vi.advanceTimersByTimeAsync(60_000)
    expect(read).toHaveBeenCalledTimes(2)
    expect(vi.getTimerCount()).toBe(0)
  })
})
