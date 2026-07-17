import type { TerminalSessionEventHandlers } from '@mycopilot/host-api'
import type { TerminalExitEvent, TerminalOutputEvent } from '@mycopilot/protocol'

/** One renderer-wide router; Electron IPC listeners do not scale with terminal count. */
export class TerminalEventRouter {
  private readonly subscribers = new Map<string, Set<TerminalSessionEventHandlers>>()

  dispatchExit(event: TerminalExitEvent): void {
    const handlers = this.subscribers.get(event.sessionId)
    if (!handlers) return

    // No output can follow an exit. Removing first also makes reentrant unsubscribe harmless.
    this.subscribers.delete(event.sessionId)
    for (const handler of [...handlers]) handler.onExit(event)
  }

  dispatchOutput(event: TerminalOutputEvent): void {
    const handlers = this.subscribers.get(event.sessionId)
    if (!handlers) return
    for (const handler of [...handlers]) handler.onOutput(event)
  }

  subscribe(sessionId: string, handlers: TerminalSessionEventHandlers): () => void {
    const existing = this.subscribers.get(sessionId)
    const subscribers = existing ?? new Set<TerminalSessionEventHandlers>()
    subscribers.add(handlers)
    if (!existing) this.subscribers.set(sessionId, subscribers)

    let subscribed = true
    return () => {
      if (!subscribed) return
      subscribed = false
      const current = this.subscribers.get(sessionId)
      current?.delete(handlers)
      if (current?.size === 0) this.subscribers.delete(sessionId)
    }
  }
}
