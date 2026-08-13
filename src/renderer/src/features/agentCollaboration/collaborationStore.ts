import type { AgentTreeSnapshot, CollaborationEventEnvelope } from '@mycopilot/protocol'
import { hostCollaborationDataSource, type CollaborationDataSource } from './collaborationClient'

const EVENT_PAGE_SIZE = 256

export interface CollaborationStoreSnapshot {
  /** Latest validated durable invalidation sequence for each Agent in this root tree. */
  agentInvalidationSequences: Readonly<Record<string, number>>
  error: boolean
  hydrationRevision: number
  loading: boolean
  rootConversationId: string
  tree: AgentTreeSnapshot | null
}

/**
 * Root-scoped invalidation store. Conversation messages remain owned by the chat store; this
 * class only hydrates Agent tree/display state and closes notification gaps from the durable log.
 */
export class CollaborationStore {
  private catchUpRequested = false
  private destroyed = false
  private generation = 0
  private readonly listeners = new Set<() => void>()
  private runningCatchUp: Promise<void> | null = null
  private snapshot: CollaborationStoreSnapshot
  private unsubscribe: (() => void) | null = null
  private unsubscribeResync: (() => void) | null = null

  constructor(
    readonly rootConversationId: string,
    private readonly source: CollaborationDataSource = hostCollaborationDataSource
  ) {
    this.snapshot = {
      agentInvalidationSequences: {},
      error: false,
      hydrationRevision: 0,
      loading: true,
      rootConversationId,
      tree: null
    }
  }

  readonly getSnapshot = (): CollaborationStoreSnapshot => this.snapshot

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  start(): void {
    if (this.destroyed || this.unsubscribe) return
    this.unsubscribe = this.source.subscribe(this.handleEvent)
    this.unsubscribeResync = this.source.subscribeResync(this.handleResync)
    void this.hydrate()
  }

  async hydrate(): Promise<void> {
    if (this.destroyed) return
    const generation = ++this.generation
    this.publish({ error: false, loading: true })
    try {
      const tree = await this.source.getTree({ rootConversationId: this.rootConversationId })
      if (!this.isCurrent(generation)) return
      if (!tree) {
        this.publish({
          agentInvalidationSequences: {},
          error: false,
          hydrationRevision: this.snapshot.hydrationRevision + 1,
          loading: false,
          tree: null
        })
        return
      }
      if (tree.rootConversationId !== this.rootConversationId) throw new Error('Wrong root')
      this.publish({
        agentInvalidationSequences: seedAgentInvalidationSequences(
          this.snapshot.agentInvalidationSequences,
          tree
        ),
        error: false,
        hydrationRevision: this.snapshot.hydrationRevision + 1,
        loading: false,
        tree
      })
      this.requestCatchUp()
    } catch {
      if (!this.isCurrent(generation)) return
      // The RPC currently reports legacy/no-graph and transient storage failures through the same
      // safe Host error envelope. Keep that distinction fail-closed and retry on the next durable
      // notification or explicit hydration rather than permanently hiding a tree.
      this.publish({ error: true, loading: false, tree: null })
    }
  }

  destroy(): void {
    if (this.destroyed) return
    this.destroyed = true
    this.generation += 1
    this.unsubscribe?.()
    this.unsubscribe = null
    this.unsubscribeResync?.()
    this.unsubscribeResync = null
    this.listeners.clear()
  }

  private readonly handleEvent = (event: CollaborationEventEnvelope): void => {
    if (this.destroyed || event.rootConversationId !== this.rootConversationId) return
    if (!this.snapshot.tree) {
      void this.hydrate()
      return
    }
    const current = this.snapshot.tree?.lastSequence ?? 0
    if (event.sequence <= current) return
    this.requestCatchUp()
  }

  private readonly handleResync = (): void => {
    if (this.destroyed) return
    void this.hydrate()
  }

  private requestCatchUp(): void {
    if (this.destroyed || !this.snapshot.tree) return
    this.catchUpRequested = true
    if (this.runningCatchUp) return
    this.runningCatchUp = this.catchUp().finally(() => {
      this.runningCatchUp = null
      if (this.catchUpRequested) this.requestCatchUp()
    })
  }

  private async catchUp(): Promise<void> {
    while (!this.destroyed && this.catchUpRequested && this.snapshot.tree) {
      this.catchUpRequested = false
      const generation = this.generation
      let cursor = this.snapshot.tree.lastSequence
      let gap = false
      const agentInvalidationSequences = { ...this.snapshot.agentInvalidationSequences }

      try {
        for (;;) {
          const page = await this.source.listEvents({
            afterSequence: cursor,
            limit: EVENT_PAGE_SIZE,
            rootConversationId: this.rootConversationId
          })
          if (!this.isCurrent(generation) || !this.snapshot.tree) return
          if (page.rootConversationId !== this.rootConversationId) throw new Error('Wrong root')

          for (const event of page.events) {
            if (
              event.rootConversationId !== this.rootConversationId ||
              event.sequence !== cursor + 1
            ) {
              gap = true
              break
            }
            cursor = event.sequence
            agentInvalidationSequences[event.agentId] = event.sequence
          }
          if (gap || !page.hasMore) break
          if (page.events.length === 0) {
            gap = true
            break
          }
        }

        // Events are invalidations, not a second chat/state reducer. Rehydrate the authoritative
        // tree after replay validation; a gap also converges through this full snapshot path.
        const tree = await this.source.getTree({ rootConversationId: this.rootConversationId })
        if (!this.isCurrent(generation)) return
        if (!tree) {
          this.publish({
            agentInvalidationSequences: {},
            error: false,
            ...(gap ? { hydrationRevision: this.snapshot.hydrationRevision + 1 } : {}),
            loading: false,
            tree: null
          })
          return
        }
        if (!gap && tree.lastSequence < cursor) throw new Error('Stale collaboration snapshot')
        const requiresFullInvalidation = gap || tree.lastSequence > cursor
        this.publish({
          agentInvalidationSequences: seedAgentInvalidationSequences(
            requiresFullInvalidation ? {} : agentInvalidationSequences,
            tree
          ),
          error: false,
          ...(requiresFullInvalidation
            ? { hydrationRevision: this.snapshot.hydrationRevision + 1 }
            : {}),
          loading: false,
          tree
        })
      } catch {
        if (this.isCurrent(generation)) this.publish({ error: true, loading: false })
      }
    }
  }

  private isCurrent(generation: number): boolean {
    return !this.destroyed && generation === this.generation
  }

  private publish(changes: Partial<CollaborationStoreSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...changes }
    for (const listener of this.listeners) listener()
  }
}

function seedAgentInvalidationSequences(
  current: Readonly<Record<string, number>>,
  tree: AgentTreeSnapshot
): Readonly<Record<string, number>> {
  return Object.fromEntries(
    tree.agents.map((agent) => [agent.agentId, current[agent.agentId] ?? tree.lastSequence])
  )
}
