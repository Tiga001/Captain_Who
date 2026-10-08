import { afterEach, describe, expect, it, vi } from 'vitest'
import type { AgentUsageDashboardOutput } from '@mycopilot/protocol'
import {
  UsageDashboardCache,
  UsageDashboardInvalidatedError,
  USAGE_DASHBOARD_FRESH_MS,
  usageDashboardKey
} from '../../features/agent/usageDashboardCache'

const windows = [{ from: 10, to: 20 }]
const dashboard = (count = 1): AgentUsageDashboardOutput => ({
  summary: { requestCount: count, messageCount: count, unpricedMessageCount: 0, models: [] },
  buckets: [{ requestCount: count, messageCount: count, unpricedMessageCount: 0, models: [] }]
})
function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}
afterEach(() => vi.restoreAllMocks())

describe('usage dashboard cache', () => {
  it('deduplicates in-flight reads and reuses fresh results across consumers', async () => {
    const cache = new UsageDashboardCache()
    const pending = deferred<AgentUsageDashboardOutput>()
    const load = vi.fn(() => pending.promise)
    const first = cache.load(windows, load)
    const second = cache.load(
      windows.map((window) => ({ ...window })),
      load
    )
    expect(first).toBe(second)
    await Promise.resolve()
    expect(load).toHaveBeenCalledTimes(1)
    pending.resolve(dashboard())
    await first
    expect(await cache.load(windows, load)).toEqual(dashboard())
    expect(load).toHaveBeenCalledTimes(1)
    expect(usageDashboardKey(windows)).toBe(usageDashboardKey([{ from: 10, to: 20 }]))
  })

  it('returns stale data immediately, refreshes once, and retains successful data on failure', async () => {
    const cache = new UsageDashboardCache()
    const clock = vi.spyOn(Date, 'now').mockReturnValue(100)
    await cache.load(windows, async () => dashboard(1))
    clock.mockReturnValue(100 + USAGE_DASHBOARD_FRESH_MS)
    const load = vi.fn(async () => {
      throw new Error('temporary read failure')
    })
    expect(cache.read(windows)?.data.summary.requestCount).toBe(1)
    await expect(cache.load(windows, load)).rejects.toThrow('temporary read failure')
    expect(cache.read(windows)?.data.summary.requestCount).toBe(1)
    expect(await cache.load(windows, async () => dashboard(2))).toEqual(dashboard(2))
  })

  it('keeps at most six LRU results and does not evict a recently read entry', async () => {
    const cache = new UsageDashboardCache()
    for (let index = 0; index < 6; index++)
      await cache.load([{ from: index, to: index }], async () => dashboard(index))
    cache.read([{ from: 0, to: 0 }])
    await cache.load([{ from: 6, to: 6 }], async () => dashboard(6))
    expect(cache.read([{ from: 0, to: 0 }])).toBeDefined()
    expect(cache.read([{ from: 1, to: 1 }])).toBeUndefined()
  })

  it('bounds distinct in-flight keys without preventing duplicate-key joins', async () => {
    const cache = new UsageDashboardCache()
    const pending = deferred<AgentUsageDashboardOutput>()
    const reads = Array.from({ length: 6 }, (_, index) =>
      cache.load([{ from: index, to: index }], () => pending.promise)
    )
    expect(cache.load([{ from: 0, to: 0 }], () => pending.promise)).toBe(reads[0])
    await expect(cache.load([{ from: 6, to: 6 }], () => pending.promise)).rejects.toThrow(
      'capacity'
    )
    pending.resolve(dashboard())
    await Promise.all(reads)
  })

  it('counts unresolved Host work across invalidation instead of admitting six more each generation', async () => {
    const cache = new UsageDashboardCache()
    const pending = deferred<AgentUsageDashboardOutput>()
    const load = vi.fn(() => pending.promise)
    const reads = Array.from({ length: 6 }, (_, index) =>
      cache.load([{ from: index, to: index }], load).catch((error: unknown) => error)
    )
    await Promise.resolve()
    expect(load).toHaveBeenCalledTimes(6)
    cache.invalidate()
    await expect(cache.load(windows, load)).rejects.toThrow('capacity')
    expect(load).toHaveBeenCalledTimes(6)
    pending.resolve(dashboard())
    for (const result of await Promise.all(reads))
      expect(result).toBeInstanceOf(UsageDashboardInvalidatedError)
    expect(await cache.load(windows, async () => dashboard(0))).toEqual(dashboard(0))
  })

  it('fences old success and error responses across clear, including overlapping clears', async () => {
    const cache = new UsageDashboardCache()
    const old = deferred<AgentUsageDashboardOutput>()
    const oldRead = cache.load(windows, () => old.promise)
    const oldRejected = expect(oldRead).rejects.toBeInstanceOf(UsageDashboardInvalidatedError)
    await Promise.resolve()
    const firstClear = cache.beginClear()
    const secondClear = cache.beginClear()
    expect(cache.read(windows)).toBeUndefined()
    await expect(cache.load(windows, async () => dashboard(5))).rejects.toBeInstanceOf(
      UsageDashboardInvalidatedError
    )
    firstClear()
    expect(cache.isClearing).toBe(true)
    secondClear()
    await cache.load(windows, async () => dashboard(0))
    old.resolve(dashboard(99))
    await oldRejected
    expect(cache.read(windows)?.data.summary.requestCount).toBe(0)

    cache.invalidate()
    const staleError = deferred<AgentUsageDashboardOutput>()
    const failing = cache.load(windows, () => staleError.promise)
    const rejected = expect(failing).rejects.toBeInstanceOf(UsageDashboardInvalidatedError)
    await Promise.resolve()
    cache.invalidate()
    staleError.reject(new Error('obsolete error'))
    await rejected
  })

  it('does not start a queued read after invalidation and notifies both clear boundaries', async () => {
    const cache = new UsageDashboardCache()
    const listener = vi.fn()
    const unsubscribe = cache.subscribe(listener)
    const load = vi.fn(async () => dashboard())
    const read = cache.load(windows, load)
    const done = cache.beginClear()
    await expect(read).rejects.toBeInstanceOf(UsageDashboardInvalidatedError)
    expect(load).not.toHaveBeenCalled()
    done()
    done()
    expect(listener).toHaveBeenCalledTimes(2)
    unsubscribe()
    cache.invalidate()
    expect(listener).toHaveBeenCalledTimes(2)
  })
})
