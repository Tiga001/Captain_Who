import type { HostDiagnosticContext } from '@mycopilot/host-api'
import { LATENCY_BUCKETS_MS, LatencyHistogram } from '../../shared/latencyHistogram'

/** Bounded, opt-in numeric diagnostics. Dynamic methods and all payloads are excluded. */
const METHODS = new Set([
  'core.ping',
  'core.shutdown',
  'storage.loadConversation',
  'storage.loadReadToolFileContext',
  'storage.loadConversations',
  'storage.loadConversationMetas',
  'storage.saveChatMessageState',
  'storage.saveChatMessageUiState',
  'storage.loadRunningConversationSummaries',
  'agent.listPendingActions',
  'agent.getUsageSummary',
  'agent.getUsageDashboard',
  'agent.cancelRun',
  'agent.approveAction',
  'agent.rejectAction',
  'agent.cancelAction',
  'agent.startConversationTurn',
  'agent.continueConversationTurn',
  'agent.rewriteConversationTurn',
  'agent.collaboration.approvals.decide',
  'agent.workflows.request',
  'agent.event',
  'agent.collaboration.observerEvent',
  'agent.collaboration.childEvent',
  'agent.workflows.runtime.changed'
])

export function diagnosticMethod(method: unknown): string {
  return typeof method === 'string' && METHODS.has(method) ? method : 'other'
}

type Stage =
  | 'request.serialize'
  | 'request.stdin_written'
  | 'response'
  | 'response.parse'
  | 'notification.dispatch'
  | 'stdin.backpressure'
  | 'event_loop.delay'
interface Sample {
  latency: LatencyHistogram
  bytes: number
  maxBytes: number
}
type RequestOutcome = 'offered' | 'completed' | 'rpc_error' | 'transport_failed'
interface RequestTrace {
  renderer?: Readonly<HostDiagnosticContext>
  rpcId: number
  method: string
  elapsedMs: number
  outcome: Exclude<RequestOutcome, 'offered'>
}

export class RpcDiagnostics {
  readonly enabled: boolean
  private readonly samples = new Map<string, Sample>()
  private lastFlush = performance.now()
  private pendingPeak = 0
  private stdinBytesPeak = 0
  private pendingCurrent = 0
  private stdinBytesCurrent = 0
  private oldestPendingMsPeak = 0
  private readonly outcomes = new Map<string, number>()
  private readonly traces: RequestTrace[] = []
  private omittedTraces = 0

  constructor(
    enabled = process.env.CAPTAIN_PERFORMANCE_DIAGNOSTICS === '1',
    private readonly emit: (summary: object) => void = (summary) =>
      console.info('[core-performance]', summary)
  ) {
    this.enabled = enabled
  }

  queues(pending: number, stdinBytes: number, oldestPendingMs = 0): void {
    if (!this.enabled) return
    this.pendingCurrent = pending
    this.stdinBytesCurrent = stdinBytes
    this.oldestPendingMsPeak = Math.max(this.oldestPendingMsPeak, oldestPendingMs)
    this.pendingPeak = Math.max(this.pendingPeak, pending)
    this.stdinBytesPeak = Math.max(this.stdinBytesPeak, stdinBytes)
  }

  record(stage: Stage, method: unknown, elapsedMs: number, bytes = 0): void {
    if (!this.enabled) return
    const key = `${stage}:${diagnosticMethod(method)}`
    const sample = this.samples.get(key) ?? {
      latency: new LatencyHistogram(),
      bytes: 0,
      maxBytes: 0
    }
    sample.latency.record(elapsedMs)
    sample.bytes += bytes
    sample.maxBytes = Math.max(sample.maxBytes, bytes)
    this.samples.set(key, sample)
    if (performance.now() - this.lastFlush >= 30_000) this.flush()
  }

  requestOutcome(
    outcome: RequestOutcome,
    method: unknown,
    rpcId: number,
    elapsedMs = 0,
    renderer?: Readonly<HostDiagnosticContext>
  ): void {
    if (!this.enabled) return
    const safeMethod = diagnosticMethod(method)
    const key = `${outcome}:${safeMethod}`
    this.outcomes.set(key, (this.outcomes.get(key) ?? 0) + 1)
    if (outcome !== 'offered') {
      // Correlation IDs are a bounded trace field, never metric labels. The existing numeric
      // JSON-RPC ID joins Host and Core logs within this child session; no payload is retained.
      if (this.traces.length < 32)
        this.traces.push({
          rpcId,
          method: safeMethod,
          elapsedMs,
          outcome,
          ...(renderer ? { renderer } : {})
        })
      else this.omittedTraces += 1
    }
  }

  flush(): void {
    if (
      !this.enabled ||
      (!this.samples.size && !this.outcomes.size && !this.pendingPeak && !this.stdinBytesPeak)
    )
      return
    const summary = {
      windowMs: performance.now() - this.lastFlush,
      pendingPeak: this.pendingPeak,
      stdinBytesPeak: this.stdinBytesPeak,
      clock: 'host.performance.now',
      percentileKind: 'fixed_bucket_upper_bound',
      bucketUpperBoundsMs: LATENCY_BUCKETS_MS,
      pendingCurrent: this.pendingCurrent,
      stdinBytesCurrent: this.stdinBytesCurrent,
      oldestPendingMsPeak: this.oldestPendingMsPeak,
      outcomes: Object.fromEntries(this.outcomes),
      traces: [...this.traces],
      omittedTraces: this.omittedTraces,
      samples: Object.fromEntries(
        [...this.samples].map(([key, sample]) => [
          key,
          { ...sample.latency.snapshot(), bytes: sample.bytes, maxBytes: sample.maxBytes }
        ])
      )
    }
    this.samples.clear()
    this.outcomes.clear()
    this.traces.length = 0
    this.omittedTraces = 0
    this.oldestPendingMsPeak = 0
    this.pendingPeak = 0
    this.stdinBytesPeak = 0
    this.lastFlush = performance.now()
    this.emit(summary)
  }
}
