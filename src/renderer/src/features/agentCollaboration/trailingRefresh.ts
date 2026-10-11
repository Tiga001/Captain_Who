/** One active read and one replaceable trailing read; no per-notification task queue. */
export class TrailingRefresh {
  private active = false
  private pending: {
    read: () => Promise<void>
    resolve: () => void
    reject: (error: unknown) => void
    promise: Promise<void>
  } | null = null

  request(read: () => Promise<void>): Promise<void> {
    if (!this.active) return this.start(read)
    if (this.pending) {
      this.pending.read = read
      return this.pending.promise
    }
    let resolve!: () => void
    let reject!: (error: unknown) => void
    const promise = new Promise<void>((accept, fail) => {
      resolve = accept
      reject = fail
    })
    this.pending = { read, resolve, reject, promise }
    return promise
  }

  private start(read: () => Promise<void>): Promise<void> {
    this.active = true
    const result = Promise.resolve().then(read)
    const finish = () => {
      this.active = false
      const pending = this.pending
      this.pending = null
      if (pending) void this.start(pending.read).then(pending.resolve, pending.reject)
    }
    void result.then(finish, finish)
    return result
  }
}
