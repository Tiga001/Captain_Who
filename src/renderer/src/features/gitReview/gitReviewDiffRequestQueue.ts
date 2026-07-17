export type GitReviewDiffPriority = 'high' | 'normal'

export interface GitReviewDiffQueueItem {
  fileId: string
  priority: GitReviewDiffPriority
  requestKey: string
  snapshotId: string
}

export type GitReviewDiffEnqueueResult =
  { accepted: true; evicted?: GitReviewDiffQueueItem } | { accepted: false }

/**
 * Small, deterministic priority queue for viewport-driven diff requests. It intentionally admits
 * at most one queued request per file; in-flight ownership remains in the hook that performs I/O.
 */
export class GitReviewDiffRequestQueue {
  readonly #capacity: number
  readonly #high: GitReviewDiffQueueItem[] = []
  readonly #normal: GitReviewDiffQueueItem[] = []
  readonly #itemsByFileId = new Map<string, GitReviewDiffQueueItem>()

  constructor(capacity: number) {
    if (!Number.isInteger(capacity) || capacity < 1) {
      throw new Error('Git diff queue capacity must be a positive integer.')
    }
    this.#capacity = capacity
  }

  get size(): number {
    return this.#itemsByFileId.size
  }

  clear(): void {
    this.#high.length = 0
    this.#normal.length = 0
    this.#itemsByFileId.clear()
  }

  enqueue(item: GitReviewDiffQueueItem): GitReviewDiffEnqueueResult {
    const existing = this.#itemsByFileId.get(item.fileId)
    if (existing) {
      if (
        existing.requestKey === item.requestKey &&
        existing.priority === 'normal' &&
        item.priority === 'high'
      ) {
        this.#removeFrom(this.#normal, existing)
        existing.priority = 'high'
        this.#high.unshift(existing)
      }
      return { accepted: existing.requestKey === item.requestKey }
    }

    let evicted: GitReviewDiffQueueItem | undefined
    if (this.size >= this.#capacity) {
      if (item.priority !== 'high') return { accepted: false }
      evicted = this.#normal.pop()
      if (!evicted) return { accepted: false }
      this.#itemsByFileId.delete(evicted.fileId)
    }

    this.#itemsByFileId.set(item.fileId, item)
    if (item.priority === 'high') this.#high.unshift(item)
    else this.#normal.push(item)
    return { accepted: true, evicted }
  }

  promote(fileId: string, requestKey: string): boolean {
    const existing = this.#itemsByFileId.get(fileId)
    if (!existing || existing.requestKey !== requestKey) return false
    if (existing.priority === 'high') return true
    this.#removeFrom(this.#normal, existing)
    existing.priority = 'high'
    this.#high.unshift(existing)
    return true
  }

  remove(fileId: string, requestKey?: string): GitReviewDiffQueueItem | undefined {
    const item = this.#itemsByFileId.get(fileId)
    if (!item || (requestKey !== undefined && item.requestKey !== requestKey)) return undefined
    this.#removeFrom(item.priority === 'high' ? this.#high : this.#normal, item)
    this.#itemsByFileId.delete(fileId)
    return item
  }

  shift(): GitReviewDiffQueueItem | undefined {
    const item = this.#high.shift() ?? this.#normal.shift()
    if (item) this.#itemsByFileId.delete(item.fileId)
    return item
  }

  #removeFrom(queue: GitReviewDiffQueueItem[], item: GitReviewDiffQueueItem): void {
    const index = queue.indexOf(item)
    if (index >= 0) queue.splice(index, 1)
  }
}
