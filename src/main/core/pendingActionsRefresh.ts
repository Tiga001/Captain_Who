interface RefreshRequest<Result> {
  promise: Promise<Result>
  resolve(value: Result): void
  reject(error: unknown): void
}

const MIN_REFRESH_INTERVAL_MS = 100
const INITIAL_FAILURE_DELAY_MS = 1_000
const MAX_FAILURE_DELAY_MS = 15_000

/**
 * Coordinates the global, read-only approval snapshot across all Host consumers. Calls arriving
 * after a read started share one fresh trailing read, never the earlier snapshot. The fixed
 * interval also batches separate IPC turns without a debounce that can postpone work forever.
 * Failed reads reject their callers; only a requested trailing/new read runs after the cooldown.
 */
export class PendingActionsRefresh<Result> {
  private active: RefreshRequest<Result> | null = null
  private queued: RefreshRequest<Result> | null = null
  private timer: ReturnType<typeof setTimeout> | null = null
  private nextReadAt = 0
  private failures = 0
  private closed = false

  constructor(private readonly read: () => Promise<Result>) {}

  request(): Promise<Result> {
    if (this.closed) return Promise.reject(new Error('Pending action refresh is closed'))
    if (this.queued) return this.queued.promise

    let resolve!: RefreshRequest<Result>['resolve']
    let reject!: RefreshRequest<Result>['reject']
    const promise = new Promise<Result>((accept, fail) => {
      resolve = accept
      reject = fail
    })
    this.queued = { promise, resolve, reject }
    this.startNext()
    return promise
  }

  /** Explicit Core stop retires old callers, but a later user request may start a fresh Core. */
  reset(): void {
    if (this.timer) clearTimeout(this.timer)
    this.timer = null
    const active = this.active
    const queued = this.queued
    this.active = null
    this.queued = null
    this.nextReadAt = 0
    this.failures = 0
    const error = new Error('Pending action refresh was stopped')
    active?.reject(error)
    queued?.reject(error)
  }

  /** Application shutdown must not leave a timer able to lazily restart Core. */
  close(): void {
    this.closed = true
    this.reset()
  }

  private startNext(): void {
    if (this.closed || this.active || !this.queued || this.timer) return
    const delay = this.nextReadAt - Date.now()
    if (delay > 0) {
      this.timer = setTimeout(() => {
        this.timer = null
        this.startNext()
      }, delay)
      this.timer.unref()
      return
    }

    const request = this.queued
    this.queued = null
    this.active = request
    this.nextReadAt = Date.now() + MIN_REFRESH_INTERVAL_MS
    // Defer only promise settlement, not dispatch: a later caller must receive a snapshot whose
    // read starts after that caller arrived. Synchronous transport/start failures use the same gate.
    let response: Promise<Result>
    try {
      response = this.read()
    } catch (error) {
      this.finishFailure(request, error)
      return
    }
    void response.then(
      (result) => {
        if (this.active !== request) return
        this.active = null
        this.failures = 0
        request.resolve(result)
        this.startNext()
      },
      (error: unknown) => this.finishFailure(request, error)
    )
  }

  private finishFailure(request: RefreshRequest<Result>, error: unknown): void {
    // Completion from a stopped Core must not alter the schedule of its replacement.
    if (this.active !== request) return
    this.active = null
    this.failures = Math.min(this.failures + 1, 5)
    this.nextReadAt =
      Date.now() +
      Math.min(INITIAL_FAILURE_DELAY_MS * 2 ** (this.failures - 1), MAX_FAILURE_DELAY_MS)
    request.reject(error)
    this.startNext()
  }
}
