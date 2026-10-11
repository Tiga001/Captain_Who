/** Runs independent keyed persistence work without losing failures at explicit flush boundaries. */
export class PersistenceDrain {
  private readonly drains = new Map<string, Promise<void>>()

  constructor(
    private readonly pendingKeys: () => Iterable<string>,
    private readonly hasPending: (key: string) => boolean,
    private readonly drain: (key: string) => Promise<void>
  ) {}

  start(key: string): void {
    if (this.drains.has(key) || !this.hasPending(key)) return
    let succeeded = false
    const tracked = this.drain(key)
      .then(() => {
        succeeded = true
      })
      .finally(() => {
        if (this.drains.get(key) === tracked) this.drains.delete(key)
        // A write accepted between drain completion and its finalizer must not be stranded.
        // Failed work stays pending until another enqueue or explicit flush retries it.
        if (succeeded) this.start(key)
      })
    this.drains.set(key, tracked)
    // Enqueues are intentionally asynchronous; flush still observes the original rejection.
    void tracked.catch(() => undefined)
  }

  async flush(key: string): Promise<void> {
    while (true) {
      this.start(key)
      const pending = this.drains.get(key)
      if (!pending) return
      await pending
    }
  }

  async flushMatching(matches: (key: string) => boolean): Promise<void> {
    while (true) {
      const keys = [...new Set([...this.pendingKeys(), ...this.drains.keys()])].filter(matches)
      if (keys.length === 0) return
      await Promise.all(keys.map((key) => this.flush(key)))
    }
  }

  hasMatching(matches: (key: string) => boolean): boolean {
    return [...this.pendingKeys(), ...this.drains.keys()].some(matches)
  }
}
