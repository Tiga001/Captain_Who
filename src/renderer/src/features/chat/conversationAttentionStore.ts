import { unwrapHostInvocation, type HumanInteractionHostApi } from '@mycopilot/host-api'
import {
  parseHumanInteractionAttentionSnapshot,
  parseHumanInteractionRequestSnapshot,
  type AgentEvent,
  type HumanInteractionAttentionRequest
} from '@mycopilot/protocol'

type RequestState = HumanInteractionAttentionRequest & { open: boolean }
export type PendingAttention = Readonly<
  Record<string, { waitingApproval?: boolean; waitingAnswer?: boolean }>
>
export const EMPTY_ATTENTION: PendingAttention = {}

/** Sparse global projection. Never retains question text, answers, deliveries or message bodies. */
export class ConversationAttentionStore {
  private ids = new Set<string>()
  private known = new Set<string>()
  private knownApprovals = new Set<string>()
  private requests = new Map<string, RequestState>()
  private approvals = new Set<string>()
  private watermark = 0
  private snapshot = EMPTY_ATTENTION
  private readonly listeners = new Set<() => void>()
  private readonly runStatuses = new Map<string, string>()
  private connected = false
  private generation = 0
  private approvalRevision = 0
  private approvalRetryStartedAt: number | null = null
  private inFlight = false
  private queued = false
  private timer: ReturnType<typeof setTimeout> | undefined
  private timerDueAt = 0
  private retryDelay = 250

  constructor(private readonly api: HumanInteractionHostApi) {}

  getSnapshot = () => this.snapshot
  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }

  watch(ids: string[]) {
    const next = new Set(ids)
    const added = ids.some((id) => !this.ids.has(id))
    this.ids = next
    if (next.size === 0) this.runStatuses.clear()
    for (const [id, request] of this.requests)
      if (!next.has(request.conversationId)) this.requests.delete(id)
    for (const id of this.known) if (!next.has(id)) this.known.delete(id)
    for (const id of this.knownApprovals) if (!next.has(id)) this.knownApprovals.delete(id)
    for (const id of this.approvals) if (!next.has(id)) this.approvals.delete(id)
    this.publish()
    if (added) this.refresh()
  }

  private publish() {
    const waiting = new Set<string>()
    for (const request of this.requests.values())
      if (request.open) waiting.add(request.conversationId)
    const next: Record<string, { waitingApproval?: boolean; waitingAnswer?: boolean }> = {}
    for (const id of new Set([...this.known, ...this.knownApprovals]))
      next[id] = {
        waitingApproval: this.knownApprovals.has(id) ? this.approvals.has(id) : undefined,
        waitingAnswer: this.known.has(id) ? waiting.has(id) : undefined
      }
    if (JSON.stringify(next) === JSON.stringify(this.snapshot)) return
    this.snapshot = next
    for (const listener of this.listeners) listener()
  }

  refresh = (delay = 0) => {
    this.queued = true
    if (!this.connected || this.inFlight) return
    const dueAt = Date.now() + delay
    // Keep the earliest deadline: continuous events must not postpone recovery forever.
    if (this.timer !== undefined) {
      if (this.timerDueAt <= dueAt) return
      clearTimeout(this.timer)
    }
    this.timerDueAt = dueAt
    this.timer = setTimeout(() => {
      this.timer = undefined
      void this.load()
    }, delay)
  }

  private async load() {
    if (!this.connected || this.inFlight || !this.queued || this.ids.size === 0) return
    this.inFlight = true
    this.queued = false
    const generation = this.generation
    const approvalRevision = this.approvalRevision
    const startedAt = Date.now()
    let failed = false
    try {
      const snapshot = parseHumanInteractionAttentionSnapshot(
        unwrapHostInvocation(await this.api.getAttention({}))
      )
      if (!this.connected || generation !== this.generation) return
      const next = new Map<string, RequestState>()
      for (const row of snapshot.requests) {
        if (!this.ids.has(row.conversationId)) continue
        const previous = this.requests.get(row.requestId)
        // Terminal events and newer revisions win over an in-flight older read.
        next.set(
          row.requestId,
          previous &&
            previous.sequence === row.sequence &&
            previous.conversationId === row.conversationId &&
            (!previous.open || previous.revision > row.revision)
            ? previous
            : { ...row, open: true }
        )
      }
      for (const [id, previous] of this.requests) {
        if (!next.has(id) && previous.sequence > snapshot.requestSequence) next.set(id, previous)
      }
      this.requests = next
      this.watermark = Math.max(this.watermark, snapshot.requestSequence)
      const approvalChanged = approvalRevision !== this.approvalRevision
      if (!approvalChanged || Date.now() - (this.approvalRetryStartedAt ?? startedAt) >= 1000) {
        this.approvals = new Set(snapshot.approvalConversationIds.filter((id) => this.ids.has(id)))
        this.knownApprovals = new Set(this.ids)
        this.approvalRetryStartedAt = null
      } else this.approvalRetryStartedAt ??= startedAt
      // Under continuous multi-run changes, publish the latest completed DB facts at least
      // once per second (plus RPC latency), then recover again instead of starving the UI.
      if (approvalChanged) this.queued = true
      this.known = new Set(this.ids)
      this.retryDelay = 250
      this.publish()
    } catch {
      // Keep the last successful projection during reconnect; retry with bounded backoff.
      failed = true
    } finally {
      this.inFlight = false
      if (this.connected) {
        if (failed) {
          this.refresh(this.retryDelay)
          this.retryDelay = Math.min(this.retryDelay * 2, 30_000)
        } else if (this.queued) this.refresh()
      }
    }
  }

  private onRequestChanged = (value: unknown) => {
    try {
      const request = parseHumanInteractionRequestSnapshot(value)
      if (!this.ids.has(request.conversationId)) return
      const previous = this.requests.get(request.requestId)
      if (
        previous &&
        (previous.sequence !== request.sequence ||
          previous.conversationId !== request.conversationId ||
          previous.revision > request.revision ||
          !previous.open)
      )
        return
      if (!previous && request.status === 'open' && request.sequence <= this.watermark) return
      this.requests.set(request.requestId, {
        requestId: request.requestId,
        conversationId: request.conversationId,
        sequence: request.sequence,
        revision: request.revision,
        open: request.status === 'open'
      })
      this.known.add(request.conversationId)
      this.publish()
      if (request.status !== 'open') this.refresh(100)
    } catch {
      // Invalid notifications cannot alter the projection; the next recovery read repairs it.
    }
  }

  private onAgentEvent = (event: AgentEvent) => {
    if (event.type === 'state') {
      if (this.runStatuses.get(event.runId) === event.state.status) return
      if (['completed', 'cancelled', 'failed'].includes(event.state.status))
        this.runStatuses.delete(event.runId)
      else {
        this.runStatuses.delete(event.runId)
        this.runStatuses.set(event.runId, event.state.status)
        // Deduplication is disposable. Lost terminal events must not retain every past Run.
        if (this.runStatuses.size > 1024)
          this.runStatuses.delete(this.runStatuses.keys().next().value!)
      }
    } else if (event.type === 'done' || event.type === 'error') {
      if (event.runId) this.runStatuses.delete(event.runId)
    } else if (event.type !== 'approval_required' && event.type !== 'started') return
    this.approvalRevision++
    this.refresh(100)
  }

  connect(subscribeAgent?: (listener: (event: AgentEvent) => void) => () => void) {
    this.connected = true
    this.generation++
    const unsubscribe = this.api.onRequestChanged(this.onRequestChanged)
    const unsubscribeAgent = subscribeAgent?.(this.onAgentEvent)
    const unsubscribeResync = this.api.onResync?.(() => {
      // A restarted/restored database may have a smaller AUTOINCREMENT watermark.
      this.generation++
      this.watermark = 0
      this.requests.clear()
      this.runStatuses.clear()
      this.approvalRetryStartedAt = null
      this.refresh()
    })
    const refreshVisible = () => {
      if (document.visibilityState !== 'hidden') this.refresh()
    }
    window.addEventListener('focus', refreshVisible)
    window.addEventListener('online', refreshVisible)
    document.addEventListener('visibilitychange', refreshVisible)
    this.refresh()
    return () => {
      this.connected = false
      this.generation++
      if (this.timer !== undefined) clearTimeout(this.timer)
      this.timer = undefined
      unsubscribe()
      unsubscribeAgent?.()
      unsubscribeResync?.()
      this.runStatuses.clear()
      window.removeEventListener('focus', refreshVisible)
      window.removeEventListener('online', refreshVisible)
      document.removeEventListener('visibilitychange', refreshVisible)
    }
  }
}
