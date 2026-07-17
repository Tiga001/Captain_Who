export const TERMINAL_EXIT_DRAIN_QUIET_MS = 20
export const TERMINAL_EXIT_DRAIN_MAX_WAIT_MS = 2000

interface TerminalExitDrainControllerOptions<Result> {
  isOutputPaused: () => boolean
  maxWaitMs?: number
  onDrainComplete: (result: Result) => void
  quietPeriodMs?: number
  waitForStreamClose: boolean
}

/**
 * Separates a PTY process exit from the point at which its output stream is drained.
 *
 * On Unix, node-pty reports process exit from an independent waitpid thread, so the
 * final socket data can arrive after `onExit`. The controller keeps the data listener
 * alive until the PTY stream closes, output backpressure clears, and no more data has
 * arrived for a short quiet period. A bounded fallback prevents a broken PTY stream
 * or renderer from retaining the session forever.
 */
export class TerminalExitDrainController<Result> {
  private completed = false
  private disposed = false
  private exitResult: Result | undefined
  private exitStarted = false
  private fallbackTimer: ReturnType<typeof setTimeout> | null = null
  private readonly isOutputPaused: () => boolean
  private readonly maxWaitMs: number
  private readonly onDrainComplete: (result: Result) => void
  private readonly quietPeriodMs: number
  private quietTimer: ReturnType<typeof setTimeout> | null = null
  private streamClosed = false
  private readonly waitForStreamClose: boolean

  constructor(options: TerminalExitDrainControllerOptions<Result>) {
    this.isOutputPaused = options.isOutputPaused
    this.maxWaitMs = options.maxWaitMs ?? TERMINAL_EXIT_DRAIN_MAX_WAIT_MS
    this.onDrainComplete = options.onDrainComplete
    this.quietPeriodMs = options.quietPeriodMs ?? TERMINAL_EXIT_DRAIN_QUIET_MS
    this.waitForStreamClose = options.waitForStreamClose

    if (!Number.isFinite(this.quietPeriodMs) || this.quietPeriodMs < 0) {
      throw new Error('Terminal exit drain quiet period cannot be negative')
    }
    if (!Number.isFinite(this.maxWaitMs) || this.maxWaitMs < this.quietPeriodMs) {
      throw new Error('Terminal exit drain maximum wait must cover the quiet period')
    }
  }

  begin(result: Result): void {
    if (this.disposed || this.completed || this.exitStarted) return
    this.exitStarted = true
    this.exitResult = result
    this.fallbackTimer = setTimeout(() => this.complete(), this.maxWaitMs)
    this.fallbackTimer.unref?.()
    this.scheduleQuietCompletion()
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    this.clearTimers()
    this.exitResult = undefined
  }

  noteData(): void {
    if (!this.exitStarted || this.disposed || this.completed) return
    this.scheduleQuietCompletion()
  }

  noteOutputResumed(): void {
    if (!this.exitStarted || this.disposed || this.completed) return
    this.scheduleQuietCompletion()
  }

  noteStreamClosed(): void {
    if (this.disposed || this.completed) return
    this.streamClosed = true
    this.scheduleQuietCompletion()
  }

  private clearQuietTimer(): void {
    if (this.quietTimer === null) return
    clearTimeout(this.quietTimer)
    this.quietTimer = null
  }

  private clearTimers(): void {
    this.clearQuietTimer()
    if (this.fallbackTimer === null) return
    clearTimeout(this.fallbackTimer)
    this.fallbackTimer = null
  }

  private complete(): void {
    if (this.disposed || this.completed || !this.exitStarted) return
    const result = this.exitResult as Result
    this.completed = true
    this.exitResult = undefined
    this.clearTimers()
    this.onDrainComplete(result)
  }

  private scheduleQuietCompletion(): void {
    this.clearQuietTimer()
    if (
      !this.exitStarted ||
      this.disposed ||
      this.completed ||
      this.isOutputPaused() ||
      (this.waitForStreamClose && !this.streamClosed)
    ) {
      return
    }

    this.quietTimer = setTimeout(() => {
      this.quietTimer = null
      if (this.isOutputPaused()) return
      this.complete()
    }, this.quietPeriodMs)
    this.quietTimer.unref?.()
  }
}
