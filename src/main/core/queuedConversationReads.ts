interface QueuedRead {
  promise: Promise<unknown>
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

  /** Each key must identify both the request arguments and its result type. */
  run<Result>(key: string, read: () => Promise<Result>): Promise<Result> {
    const existing = this.queued.get(key)
    if (existing) return existing.promise as Promise<Result>

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
    this.queued.set(key, { promise, start })
    this.startNext()
    return promise
  }

  private startNext(): void {
    if (this.active) return
    const next = this.queued.entries().next().value
    if (!next) return
    const [key, read] = next
    // Remove before dispatch: in-flight results are never shared with later callers.
    this.queued.delete(key)
    this.active = true
    read.start()
  }
}
