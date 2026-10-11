import type { HostDiagnosticContext } from '@mycopilot/host-api'
import { LATENCY_BUCKETS_MS, LatencyHistogram } from '../../../shared/latencyHistogram'

export type ActionKind =
  | 'approval.approve'
  | 'approval.reject'
  | 'approval.cancel'
  | 'approval.projected'
  | 'turn.submit'
  | 'turn.start_rpc'
  | 'run.cancel_rpc'
type Stage =
  'preflush' | 'provider.preflight' | 'rpc.wait' | 'state.apply' | 'projection.refresh' | 'total'
type Outcome = 'completed' | 'failed' | 'skipped'

/** Local Renderer spans; total means callback completion, not DOM paint or tool completion. */
export interface ActionSpan {
  readonly context?: HostDiagnosticContext
  measureAsync<T>(stage: Stage, operation: () => Promise<T>): Promise<T>
  measureSync<T>(stage: Stage, operation: () => T): T
  finish(outcome: Outcome): void
}

const NOOP_SPAN: ActionSpan = {
  measureAsync: (_stage, operation) => operation(),
  measureSync: (_stage, operation) => operation(),
  finish: () => {}
}

export class ActionDiagnostics {
  private readonly samples = new Map<string, LatencyHistogram>()
  private readonly outcomes = new Map<string, number>()
  private readonly traces: {
    actionSequence: number
    action: ActionKind
    stage: Stage
    elapsedMs: number
  }[] = []
  private offered = 0
  private pending = 0
  private peakPending = 0
  private sequence = 0
  private rendererSessionNonce: string | undefined
  private omittedTraces = 0
  private timer: ReturnType<typeof setTimeout> | undefined
  private windowStart: number

  constructor(
    readonly enabled: boolean,
    private readonly emit: (summary: object) => void = (summary) =>
      console.info('[renderer-performance]', summary),
    private readonly now: () => number = () => performance.now()
  ) {
    this.windowStart = now()
  }

  begin(action: ActionKind): ActionSpan {
    if (!this.enabled) return NOOP_SPAN
    const actionSequence = ++this.sequence
    this.rendererSessionNonce ??= crypto.randomUUID().replaceAll('-', '')
    const correlation = Object.freeze({
      rendererSessionNonce: this.rendererSessionNonce,
      actionSequence
    })
    const start = this.now()
    this.offered += 1
    this.pending += 1
    this.peakPending = Math.max(this.peakPending, this.pending)
    this.scheduleFlush()
    let finished = false
    const record = (stage: Stage, elapsedMs: number): void => {
      const key = `${action}:${stage}`
      const histogram = this.samples.get(key) ?? new LatencyHistogram()
      histogram.record(elapsedMs)
      this.samples.set(key, histogram)
      if (this.traces.length < 64) this.traces.push({ actionSequence, action, stage, elapsedMs })
      else this.omittedTraces += 1
      this.scheduleFlush()
    }
    return {
      context: correlation,
      measureAsync: (stage, operation) => {
        const started = this.now()
        let promise
        try {
          promise = operation()
        } catch (error) {
          record(stage, this.now() - started)
          throw error
        }
        // Observe settlement without replacing the promise or converting synchronous throws
        // into asynchronous rejections. The caller keeps the original continuation chain.
        void promise.then(
          () => record(stage, this.now() - started),
          () => record(stage, this.now() - started)
        )
        return promise
      },
      measureSync: (stage, operation) => {
        const started = this.now()
        try {
          return operation()
        } finally {
          record(stage, this.now() - started)
        }
      },
      finish: (outcome) => {
        if (finished) return
        finished = true
        this.pending -= 1
        const key = `${action}:${outcome}`
        this.outcomes.set(key, (this.outcomes.get(key) ?? 0) + 1)
        record('total', this.now() - start)
      }
    }
  }

  flush(): void {
    if (this.timer !== undefined) clearTimeout(this.timer)
    this.timer = undefined
    if (!this.enabled || (!this.offered && !this.samples.size && !this.pending)) return
    this.emit({
      clock: 'renderer.performance.now',
      rendererSessionNonce: this.rendererSessionNonce,
      windowMs: this.now() - this.windowStart,
      offered: this.offered,
      pending: this.pending,
      peakPending: this.peakPending,
      percentileKind: 'fixed_bucket_upper_bound',
      bucketUpperBoundsMs: LATENCY_BUCKETS_MS,
      samples: Object.fromEntries([...this.samples].map(([key, value]) => [key, value.snapshot()])),
      outcomes: Object.fromEntries(this.outcomes),
      traces: [...this.traces],
      omittedTraces: this.omittedTraces
    })
    this.windowStart = this.now()
    this.offered = 0
    this.peakPending = this.pending
    this.samples.clear()
    this.outcomes.clear()
    this.traces.length = 0
    this.omittedTraces = 0
    if (this.pending) this.scheduleFlush()
  }

  private scheduleFlush(): void {
    if (this.timer === undefined) this.timer = setTimeout(() => this.flush(), 30_000)
  }
}

export const actionDiagnostics = new ActionDiagnostics(
  import.meta.env.VITE_CAPTAIN_PERFORMANCE_DIAGNOSTICS === '1'
)
