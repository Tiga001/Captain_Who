import type { UpdateState } from '@mycopilot/host-api'

export interface DesktopUpdateDriver {
  check(): Promise<{ version: string } | null>
  /** Resolves only after both ZIP verification and native macOS staging have completed. */
  download(progress: (percent: number) => void): Promise<void>
  install(): void
  /** Cancel ZIP transfer / stop observing during shutdown; native staging is not cancellable. */
  cancel(): void
  onInstallError(handler: (error: unknown) => void): () => void
  dispose(): void
}

export interface UpdateLifecycle {
  requestInstall(install: () => void): boolean
  onInstallFailure(error: unknown): void
  isShuttingDown(): boolean
}

/** One check per launch, one explicit download at a time, one handoff to the shared quit path. */
export class UpdateService {
  private state: UpdateState = {
    revision: 0,
    status: 'disabled',
    version: null,
    percent: 0,
    error: null
  }
  private started = false
  private stopped = false
  private generation = 0
  private readonly listeners = new Set<(state: UpdateState) => void>()
  private readonly unsubscribeError: (() => void) | undefined

  constructor(
    private readonly driver: DesktopUpdateDriver | null,
    private readonly lifecycle: UpdateLifecycle
  ) {
    this.unsubscribeError = driver?.onInstallError((error) => {
      if (this.state.status === 'installing') this.lifecycle.onInstallFailure(error)
    })
  }

  getState = (): UpdateState => ({ ...this.state })
  subscribe(handler: (state: UpdateState) => void): () => void {
    this.listeners.add(handler)
    return () => {
      this.listeners.delete(handler)
    }
  }
  private publish(patch: Partial<UpdateState>): void {
    this.state = { ...this.state, ...patch, revision: this.state.revision + 1 }
    for (const handler of this.listeners) handler(this.getState())
  }
  startOnce(): void {
    if (this.started || this.stopped || this.lifecycle.isShuttingDown()) return
    this.started = true
    if (!this.driver) return
    this.publish({ status: 'checking' })
    const generation = ++this.generation
    void this.driver
      .check()
      .then((available) => {
        if (this.stopped || generation !== this.generation) return
        this.publish({
          status: available ? 'available' : 'idle',
          version: available?.version ?? null
        })
      })
      .catch(() => {
        if (!this.stopped && generation === this.generation)
          this.publish({ status: 'error', error: 'checkFailed' })
      })
  }
  download = (): UpdateState => {
    if (
      !this.driver ||
      this.stopped ||
      this.lifecycle.isShuttingDown() ||
      !this.state.version ||
      !['available', 'error'].includes(this.state.status)
    )
      return this.getState()
    const generation = ++this.generation
    this.publish({ status: 'downloading', percent: 0, error: null })
    void this.driver
      .download((percent) => {
        if (
          this.stopped ||
          generation !== this.generation ||
          this.state.status !== 'downloading' ||
          !Number.isFinite(percent)
        )
          return
        const next = Math.max(this.state.percent, Math.min(100, Math.max(0, Math.floor(percent))))
        if (next !== this.state.percent) this.publish({ percent: next })
      })
      .then(() => {
        if (this.stopped || generation !== this.generation || this.lifecycle.isShuttingDown())
          return
        this.publish({ status: 'installing', percent: 100 })
        if (!this.lifecycle.requestInstall(() => this.driver!.install())) {
          this.publish({ status: 'error', error: 'installFailed' })
        }
      })
      .catch(() => {
        if (!this.stopped && generation === this.generation)
          this.publish({
            status: 'error',
            error: this.state.status === 'installing' ? 'installFailed' : 'downloadFailed'
          })
      })
    return this.getState()
  }
  beginShutdown(): void {
    if (this.stopped) return
    this.stopped = true
    ++this.generation
    // Do not cancel a prepared installation accepted by the shared shutdown coordinator.
    if (this.state.status !== 'installing') this.driver?.cancel()
  }
  dispose(): void {
    this.beginShutdown()
    this.unsubscribeError?.()
    this.listeners.clear()
    this.driver?.dispose()
  }
}
