import type { AgentUsageDashboardOutput, AgentUsageWindow } from '@mycopilot/protocol'

export const USAGE_DASHBOARD_FRESH_MS = 15_000
const CACHE_CAPACITY = 6

type CachedDashboard = { data: AgentUsageDashboardOutput; updatedAt: number }

export function usageDashboardKey(windows: readonly AgentUsageWindow[]): string {
  return JSON.stringify(windows.map(({ from, to }) => [from, to]))
}

export class UsageDashboardInvalidatedError extends Error {
  constructor() {
    super('Usage dashboard request was invalidated')
  }
}

/** Session-local results only; no timers, subscriptions to the Host, or hidden-page work. */
export class UsageDashboardCache {
  private readonly entries = new Map<string, CachedDashboard>()
  private readonly requests = new Map<string, Promise<AgentUsageDashboardOutput>>()
  // Invalidation forgets dedup keys, but cannot cancel Host RPCs already on the wire.
  private readonly liveRequests = new Set<Promise<AgentUsageDashboardOutput>>()
  private readonly listeners = new Set<() => void>()
  private epoch = 0
  private clears = 0

  get generation(): number {
    return this.epoch
  }

  get isClearing(): boolean {
    return this.clears > 0
  }

  read(windows: readonly AgentUsageWindow[]): CachedDashboard | undefined {
    const key = usageDashboardKey(windows)
    const entry = this.entries.get(key)
    if (entry) {
      this.entries.delete(key)
      this.entries.set(key, entry)
    }
    return entry
  }

  load(
    windows: readonly AgentUsageWindow[],
    loader: () => Promise<AgentUsageDashboardOutput>
  ): Promise<AgentUsageDashboardOutput> {
    if (this.isClearing) return Promise.reject(new UsageDashboardInvalidatedError())
    const key = usageDashboardKey(windows)
    const cached = this.read(windows)
    if (cached && Date.now() - cached.updatedAt < USAGE_DASHBOARD_FRESH_MS)
      return Promise.resolve(cached.data)
    const existing = this.requests.get(key)
    if (existing) return existing
    if (this.liveRequests.size >= CACHE_CAPACITY)
      return Promise.reject(new Error('Usage dashboard request capacity exceeded'))

    const generation = this.epoch
    const assertCurrent = (): void => {
      if (this.epoch !== generation || this.isClearing) throw new UsageDashboardInvalidatedError()
    }
    const request = Promise.resolve()
      .then(() => {
        assertCurrent()
        return loader()
      })
      .then((data) => {
        assertCurrent()
        this.entries.delete(key)
        this.entries.set(key, { data, updatedAt: Date.now() })
        while (this.entries.size > CACHE_CAPACITY)
          this.entries.delete(this.entries.keys().next().value!)
        return data
      })
      .catch((error: unknown) => {
        assertCurrent()
        throw error
      })
      .finally(() => {
        this.liveRequests.delete(request)
        if (this.requests.get(key) === request) this.requests.delete(key)
      })
    this.requests.set(key, request)
    this.liveRequests.add(request)
    return request
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  invalidate(): void {
    ++this.epoch
    this.entries.clear()
    this.requests.clear()
    for (const listener of this.listeners) listener()
  }

  beginClear(): () => void {
    ++this.clears
    this.invalidate()
    let finished = false
    return () => {
      if (finished) return
      finished = true
      --this.clears
      // Fence reads around both boundaries, including failed or overlapping clear requests.
      this.invalidate()
    }
  }
}

export const usageDashboardCache = new UsageDashboardCache()
