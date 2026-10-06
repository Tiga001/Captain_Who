import {
  AGENT_COLLABORATION_CHILD_EVENT_NOTIFICATION_METHOD,
  AGENT_COLLABORATION_OBSERVER_EVENT_NOTIFICATION_METHOD,
  AGENT_EVENT_NOTIFICATION_METHOD,
  parseAgentChildEventEnvelope,
  parseAgentEventForHost,
  parseAgentObserverEventEnvelope,
  type AgentEvent,
  type AgentObserverEventEnvelope
} from '@mycopilot/protocol'
import type { CoreJsonRpcClient } from './jsonRpcClient'

interface Listener<Event> {
  handle(event: Event): void
}

const AGENT_OBSERVER_EVENT_TYPES = {
  started: true,
  tool_set_changed: true,
  state: true,
  message_delta: true,
  message_stream_started: true,
  message_stream_reset: true,
  message_stream_committed: true,
  model_activity_changed: true,
  final_answer_ready: true,
  llm_retry: true,
  tool_input_progress: true,
  file_change_preview_updated: true,
  file_change_preview_cleared: true,
  message: true,
  guidance_queued: true,
  guidance_applied: true,
  workflow_delivery_applied: true,
  guidance_rejected: true,
  tool_call: true,
  tool_result: true,
  mcp_tool_invocation_state_changed: true,
  todo_updated: true,
  skill_activated: true,
  file_change_updated: true,
  context_window_updated: true,
  context_compaction_started: true,
  context_compaction_finished: true,
  approval_required: true,
  file_change_proposed: true,
  command_started: true,
  command_output: true,
  command_exited: true,
  command_interrupted: true,
  error: true,
  done: true
} as const satisfies Readonly<Record<AgentEvent['type'], true>>

type AgentObserverEventLogType = AgentEvent['type'] | 'unknown'

interface AgentObserverWarning {
  category: 'validation_failed' | 'handler_failed'
  eventType: AgentObserverEventLogType
  count: number
}

function readAgentObserverEventLogType(value: unknown): AgentObserverEventLogType {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return 'unknown'
  const event = 'event' in value ? value.event : undefined
  if (typeof event !== 'object' || event === null || Array.isArray(event)) return 'unknown'
  const eventType = 'type' in event ? event.type : undefined
  if (
    typeof eventType !== 'string' ||
    !Object.prototype.hasOwnProperty.call(AGENT_OBSERVER_EVENT_TYPES, eventType)
  ) {
    return 'unknown'
  }
  return eventType as AgentEvent['type']
}

function shouldLogAgentObserverWarning(count: number): boolean {
  return count === 1 || count % 100 === 0
}

/**
 * The child wire envelope crosses Core → Main once. Its already validated nested event feeds the
 * existing ordinary subscription, while its exact identities/cursor feed the observer subscription.
 * Legacy wire routes remain separate: deriving ordinary events from legacy observerEvent would
 * duplicate events from an older Core that still publishes both routes.
 */
export class CoreAgentEventStream {
  private readonly ordinary = new Set<Listener<AgentEvent>>()
  private readonly observers = new Set<Listener<AgentObserverEventEnvelope>>()
  private unsubscribeOrdinary: (() => void) | null = null
  private unsubscribeObserver: (() => void) | null = null
  private unsubscribeChild: (() => void) | null = null

  constructor(private readonly rpc: Pick<CoreJsonRpcClient, 'onNotification'>) {}

  onAgentEvent(handler: (event: AgentEvent) => void): () => void {
    const listener = { handle: handler }
    this.ordinary.add(listener)
    this.ensureChildSubscription()
    this.unsubscribeOrdinary ??= this.rpc.onNotification(
      AGENT_EVENT_NOTIFICATION_METHOD,
      (value) => {
        let event: AgentEvent
        try {
          event = parseAgentEventForHost(value)
        } catch {
          // Never log rejected event payloads or parser diagnostics.
          console.warn('Ignored invalid Agent event')
          return
        }
        this.emitOrdinary(event)
      }
    )
    return () => {
      this.ordinary.delete(listener)
      if (this.ordinary.size === 0) {
        this.unsubscribeOrdinary?.()
        this.unsubscribeOrdinary = null
      }
      this.releaseUnusedChildSubscription()
    }
  }

  onObserverEvent(handler: (event: AgentObserverEventEnvelope) => void): () => void {
    const listener = { handle: handler }
    this.observers.add(listener)
    this.ensureChildSubscription()
    this.unsubscribeObserver ??= this.rpc.onNotification(
      AGENT_COLLABORATION_OBSERVER_EVENT_NOTIFICATION_METHOD,
      (value) => {
        const event = this.parseEnvelope(value, false)
        if (event) this.emitObserver(event)
      }
    )
    return () => {
      this.observers.delete(listener)
      if (this.observers.size === 0) {
        this.unsubscribeObserver?.()
        this.unsubscribeObserver = null
      }
      this.releaseUnusedChildSubscription()
    }
  }

  private ensureChildSubscription(): void {
    this.unsubscribeChild ??= this.rpc.onNotification(
      AGENT_COLLABORATION_CHILD_EVENT_NOTIFICATION_METHOD,
      (value) => {
        const envelope = this.parseEnvelope(value, true)
        if (!envelope) return
        // Keep the previous ordinary-before-observer order. Do not cache, sort or collapse cursor
        // generations here: snapshots and late durable events are reconciled by their consumers.
        this.emitOrdinary(envelope.event)
        this.emitObserver(envelope)
      }
    )
  }

  private releaseUnusedChildSubscription(): void {
    if (this.ordinary.size !== 0 || this.observers.size !== 0) return
    this.unsubscribeChild?.()
    this.unsubscribeChild = null
  }

  private parseEnvelope(value: unknown, child: boolean): AgentObserverEventEnvelope | null {
    try {
      return child ? parseAgentChildEventEnvelope(value) : parseAgentObserverEventEnvelope(value)
    } catch {
      this.warnAgentObserverEvent('validation_failed', readAgentObserverEventLogType(value))
      return null
    }
  }

  private emitOrdinary(event: AgentEvent): void {
    for (const listener of [...this.ordinary]) {
      if (!this.ordinary.has(listener)) continue
      try {
        listener.handle(event)
      } catch {
        // A tray/IPC subscriber failure must not prevent other subscribers or observer delivery.
        console.warn('Agent event handler failed')
      }
    }
  }

  private emitObserver(envelope: AgentObserverEventEnvelope): void {
    for (const listener of [...this.observers]) {
      if (!this.observers.has(listener)) continue
      try {
        listener.handle(envelope)
      } catch {
        this.warnAgentObserverEvent('handler_failed', envelope.event.type)
      }
    }
  }

  private readonly agentObserverWarningCounts = new Map<string, number>()

  private warnAgentObserverEvent(
    category: AgentObserverWarning['category'],
    eventType: AgentObserverEventLogType
  ): void {
    const key = `${category}:${eventType}`
    const count = (this.agentObserverWarningCounts.get(key) ?? 0) + 1
    this.agentObserverWarningCounts.set(key, count)
    if (!shouldLogAgentObserverWarning(count)) return
    const warning: AgentObserverWarning = { category, eventType, count }
    console.warn(
      category === 'validation_failed'
        ? 'Ignored invalid Agent observer event'
        : 'Agent observer handler failed',
      warning
    )
  }
}
