/** Bounded, opt-in numeric diagnostics. Dynamic methods and all payloads are excluded. */
const METHODS = new Set([
  'core.ping',
  'core.shutdown',
  'storage.loadConversation',
  'storage.loadConversations',
  'storage.loadConversationMetas',
  'storage.saveChatMessageState',
  'storage.saveChatMessageUiState',
  'storage.loadRunningConversationSummaries',
  'agent.listPendingActions',
  'agent.cancelRun',
  'agent.workflows.request',
  'agent.event',
  'agent.collaboration.observerEvent',
  'agent.workflows.runtime.changed'
])

export function diagnosticMethod(method: unknown): string {
  return typeof method === 'string' && METHODS.has(method) ? method : 'other'
}

type Stage =
  | 'request.serialize'
  | 'response'
  | 'response.parse'
  | 'notification.dispatch'
  | 'stdin.backpressure'
interface Sample {
  count: number
  totalMs: number
  maxMs: number
  bytes: number
  maxBytes: number
}

export class RpcDiagnostics {
  readonly enabled: boolean
  private readonly samples = new Map<string, Sample>()
  private lastFlush = performance.now()
  private pendingPeak = 0
  private stdinBytesPeak = 0

  constructor(
    enabled = process.env.CAPTAIN_PERFORMANCE_DIAGNOSTICS === '1',
    private readonly emit: (summary: object) => void = (summary) =>
      console.info('[core-performance]', summary)
  ) {
    this.enabled = enabled
  }

  queues(pending: number, stdinBytes: number): void {
    if (!this.enabled) return
    this.pendingPeak = Math.max(this.pendingPeak, pending)
    this.stdinBytesPeak = Math.max(this.stdinBytesPeak, stdinBytes)
  }

  record(stage: Stage, method: unknown, elapsedMs: number, bytes = 0): void {
    if (!this.enabled) return
    const key = `${stage}:${diagnosticMethod(method)}`
    const sample = this.samples.get(key) ?? {
      count: 0,
      totalMs: 0,
      maxMs: 0,
      bytes: 0,
      maxBytes: 0
    }
    sample.count += 1
    sample.totalMs += elapsedMs
    sample.maxMs = Math.max(sample.maxMs, elapsedMs)
    sample.bytes += bytes
    sample.maxBytes = Math.max(sample.maxBytes, bytes)
    this.samples.set(key, sample)
    if (performance.now() - this.lastFlush >= 30_000) this.flush()
  }

  flush(): void {
    if (!this.enabled || (!this.samples.size && !this.pendingPeak && !this.stdinBytesPeak)) return
    const summary = {
      windowMs: performance.now() - this.lastFlush,
      pendingPeak: this.pendingPeak,
      stdinBytesPeak: this.stdinBytesPeak,
      samples: Object.fromEntries(this.samples)
    }
    this.samples.clear()
    this.pendingPeak = 0
    this.stdinBytesPeak = 0
    this.lastFlush = performance.now()
    this.emit(summary)
  }
}
