/** Fixed storage: percentile values are bucket upper bounds, not retained raw samples. */
export const LATENCY_BUCKETS_MS = [
  0.05, 0.1, 0.25, 0.5, 1, 2, 4, 8, 16, 32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000, 32000,
  64000, 128000, 256000, 512000, 1024000
] as const

export class LatencyHistogram {
  private readonly buckets = new Array<number>(LATENCY_BUCKETS_MS.length + 1).fill(0)
  count = 0
  totalMs = 0
  maxMs = 0

  record(elapsedMs: number): void {
    if (!Number.isFinite(elapsedMs) || elapsedMs < 0) return
    const index = LATENCY_BUCKETS_MS.findIndex((bound) => elapsedMs <= bound)
    this.buckets[index === -1 ? LATENCY_BUCKETS_MS.length : index] += 1
    this.count += 1
    this.totalMs += elapsedMs
    this.maxMs = Math.max(this.maxMs, elapsedMs)
  }

  snapshot() {
    const percentile = (fraction: number): number | null => {
      if (!this.count) return null
      const rank = Math.ceil(this.count * fraction)
      let cumulative = 0
      for (let index = 0; index < this.buckets.length; index += 1) {
        cumulative += this.buckets[index]
        if (cumulative >= rank) return LATENCY_BUCKETS_MS[index] ?? this.maxMs
      }
      return this.maxMs
    }
    return {
      count: this.count,
      totalMs: this.totalMs,
      maxMs: this.maxMs,
      p50UpperMs: percentile(0.5),
      p95UpperMs: percentile(0.95),
      p99UpperMs: percentile(0.99),
      // The final count is the open overflow bucket (> last configured upper bound). For a
      // percentile falling there, maxMs supplies a finite conservative upper bound.
      overflowCount: this.buckets[LATENCY_BUCKETS_MS.length],
      // Explicitly expose the distribution for merging windows without averaging percentiles.
      bucketCounts: [...this.buckets]
    }
  }
}
