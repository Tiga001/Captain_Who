interface QueuedRead {
  promise: Promise<unknown>
  priority: number
  start(): void
}

/**
 * Keeps full conversation snapshots from arriving in competing multi-megabyte RPC responses.
 * Only reads still waiting in the queue share a result. A request received during an active
 * read gets a fresh trailing read, so a later write or notification cannot reuse an old cut.
 */
export class QueuedConversationReads {
  private readonly queued = new Map<string, QueuedRead>()
  private active = false
  private overtakes = 0

  /** Each key must identify both the request arguments and its result type. */
  run<Result>(key: string, read: () => Promise<Result>, priority = 0): Promise<Result> {
    const existing = this.queued.get(key)
    if (existing) {
      existing.priority = Math.max(existing.priority, priority)
      return existing.promise as Promise<Result>
    }

    let start!: () => void
    const promise = new Promise<Result>((resolve, reject) => {
      start = () => {
        const finish = (): void => {
          this.active = false
          this.startNext()
        }
        try {
          void read().then(
            (result) => {
              resolve(result)
              finish()
            },
            (error: unknown) => {
              reject(error)
              finish()
            }
          )
        } catch (error) {
          reject(error)
          finish()
        }
      }
    })
    this.queued.set(key, { promise, start, priority })
    this.startNext()
    return promise
  }

  private startNext(): void {
    if (this.active) return
    let next = this.queued.entries().next().value
    if (!next) return
    const oldest = next[0]
    // A recent observer page can pass queued bulk/history pages, without adding simultaneous
    // multi-megabyte reads. Periodically admit the oldest request to prevent starvation.
    if (this.overtakes < 4) {
      for (const candidate of this.queued.entries()) {
        if (candidate[1].priority > next[1].priority) next = candidate
      }
    }
    this.overtakes = next[0] === oldest ? 0 : this.overtakes + 1
    const [key, read] = next
    // Remove before dispatch: in-flight results are never shared with later callers.
    this.queued.delete(key)
    this.active = true
    read.start()
  }
}
