import type { AutomationEvent, AutomationResync } from './automationTypes'
import { hasAutomationHostApi, onAutomationEvent, onAutomationResync } from './automationClient'
import { removeCachedAutomationTask } from './automationCache'

export type AutomationRealtimeSignal =
  | { type: 'event'; event: AutomationEvent; sequenceGap: boolean }
  | { type: 'resync'; event: AutomationResync }

type SignalListener = (signal: AutomationRealtimeSignal) => void

const listeners = new Set<SignalListener>()
let stopEvent: (() => void) | null = null
let stopResync: (() => void) | null = null
let lastSequence = 0

function dispatch(signal: AutomationRealtimeSignal): void {
  for (const listener of listeners) listener(signal)
}

function connect(): void {
  if (stopEvent || stopResync) return
  if (!hasAutomationHostApi()) return
  stopEvent = onAutomationEvent((event) => {
    if (event.sequence <= lastSequence) return
    const sequenceGap = lastSequence > 0 && event.sequence > lastSequence + 1
    lastSequence = event.sequence
    if (event.kind === 'deleted') {
      removeCachedAutomationTask(
        event.automationId,
        event.resourceRevision ?? Number.MAX_SAFE_INTEGER
      )
    }
    dispatch({ type: 'event', event, sequenceGap })
  })
  stopResync = onAutomationResync((event) => {
    lastSequence = Math.max(lastSequence, event.lastSequence)
    dispatch({ type: 'resync', event })
  })
}

function disconnect(): void {
  stopEvent?.()
  stopResync?.()
  stopEvent = null
  stopResync = null
}

export function subscribeAutomationRealtime(listener: SignalListener): () => void {
  listeners.add(listener)
  connect()
  return () => {
    listeners.delete(listener)
    if (listeners.size === 0) disconnect()
  }
}

export function synchronizeAutomationSequence(sequence: number): void {
  lastSequence = Math.max(lastSequence, sequence)
}

export function getAutomationSequence(): number {
  return lastSequence
}

export function resetAutomationRealtimeForTests(): void {
  disconnect()
  listeners.clear()
  lastSequence = 0
}
