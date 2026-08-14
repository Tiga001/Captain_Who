import type { AgentTreeSnapshot, CollaborationEventEnvelope } from '@mycopilot/protocol'
import { hostCollaborationDataSource, type CollaborationDataSource } from './collaborationClient'
import type { CollaborationTimelineActivity } from './CollaborationTimelineActivity'

const EVENT_PAGE_SIZE = 256
export const MAX_COLLABORATION_TIMELINE_ACTIVITIES = 2_048

class CollaborationEventGapError extends Error {}

export interface CollaborationStoreSnapshot {
  /** Bounded semantic projection rebuilt from the durable root event log after restart/resync. */
  activities: readonly CollaborationTimelineActivity[]
  /** Latest validated durable invalidation sequence for each Agent in this root tree. */
  agentInvalidationSequences: Readonly<Record<string, number>>
  error: boolean
  hydrationRevision: number
  loading: boolean
  rootConversationId: string
  tree: AgentTreeSnapshot | null
}

/**
 * Root-scoped Agent index and semantic-activity store. Conversation messages remain owned by the
 * chat store; this class hydrates tree/display state plus the bounded typed activity projection
 * and closes notification gaps from the durable log.
 */
export class CollaborationStore {
  private catchUpRequested = false
  private destroyed = false
  /** Independent durable-log cursor; tree hydration alone never proves activity recovery. */
  private eventCursor = 0
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
      activities: [],
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
      const initialTree = await this.source.getTree({ rootConversationId: this.rootConversationId })
      if (!this.isCurrent(generation)) return
      if (!initialTree) {
        this.eventCursor = 0
        this.publish({
          activities: [],
          agentInvalidationSequences: {},
          error: false,
          hydrationRevision: this.snapshot.hydrationRevision + 1,
          loading: false,
          tree: null
        })
        return
      }
      if (initialTree.rootConversationId !== this.rootConversationId) throw new Error('Wrong root')
      const replay = await this.replayDurableEvents(generation, 0, [])
      if (!this.isCurrent(generation)) return
      let tree = await this.source.getTree({ rootConversationId: this.rootConversationId })
      if (!this.isCurrent(generation)) return
      if (!tree || tree.rootConversationId !== this.rootConversationId)
        throw new Error('Wrong root')
      let cursor = replay.cursor
      let activities = replay.activities
      let invalidationSequences = replay.invalidationSequences
      while (tree.lastSequence > cursor) {
        const next = await this.replayDurableEvents(
          generation,
          cursor,
          activities,
          invalidationSequences
        )
        if (!this.isCurrent(generation)) return
        if (next.cursor === cursor) throw new CollaborationEventGapError('Event log gap')
        cursor = next.cursor
        activities = next.activities
        invalidationSequences = next.invalidationSequences
        tree = await this.source.getTree({ rootConversationId: this.rootConversationId })
        if (!this.isCurrent(generation)) return
        if (!tree || tree.rootConversationId !== this.rootConversationId)
          throw new Error('Wrong root')
      }
      if (tree.lastSequence < cursor) throw new Error('Stale collaboration snapshot')
      this.eventCursor = cursor
      this.publish({
        activities,
        agentInvalidationSequences: seedAgentInvalidationSequences(invalidationSequences, tree),
        error: false,
        hydrationRevision: this.snapshot.hydrationRevision + 1,
        loading: false,
        tree
      })
      if (this.catchUpRequested) this.requestCatchUp()
    } catch {
      if (!this.isCurrent(generation)) return
      // The RPC currently reports legacy/no-graph and transient storage failures through the same
      // safe Host error envelope. Keep that distinction fail-closed and retry on the next durable
      // notification or explicit hydration rather than permanently hiding a tree.
      this.publish({ activities: [], error: true, loading: false, tree: null })
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
    if (event.sequence <= this.eventCursor) return
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
      let cursor = this.eventCursor
      let activities = [...this.snapshot.activities]
      let agentInvalidationSequences = { ...this.snapshot.agentInvalidationSequences }

      try {
        for (;;) {
          const replay = await this.replayDurableEvents(
            generation,
            cursor,
            activities,
            agentInvalidationSequences
          )
          if (!this.isCurrent(generation) || !this.snapshot.tree) return
          const previousCursor = cursor
          cursor = replay.cursor
          activities = replay.activities
          agentInvalidationSequences = replay.invalidationSequences
          const tree = await this.source.getTree({ rootConversationId: this.rootConversationId })
          if (!this.isCurrent(generation)) return
          if (!tree) {
            this.eventCursor = 0
            this.publish({
              activities: [],
              agentInvalidationSequences: {},
              error: false,
              hydrationRevision: this.snapshot.hydrationRevision + 1,
              loading: false,
              tree: null
            })
            return
          }
          if (tree.rootConversationId !== this.rootConversationId) throw new Error('Wrong root')
          if (tree.lastSequence < cursor) throw new Error('Stale collaboration snapshot')
          if (tree.lastSequence > cursor) {
            if (cursor === previousCursor) {
              void this.hydrate()
              return
            }
            continue
          }
          this.eventCursor = cursor
          this.publish({
            activities,
            agentInvalidationSequences: seedAgentInvalidationSequences(
              agentInvalidationSequences,
              tree
            ),
            error: false,
            loading: false,
            tree
          })
          break
        }
      } catch (error) {
        if (!this.isCurrent(generation)) return
        if (error instanceof CollaborationEventGapError) {
          void this.hydrate()
          return
        }
        this.publish({ error: true, loading: false })
      }
    }
  }

  private async replayDurableEvents(
    generation: number,
    startCursor: number,
    initialActivities: readonly CollaborationTimelineActivity[],
    initialInvalidationSequences: Readonly<Record<string, number>> = {}
  ): Promise<{
    activities: CollaborationTimelineActivity[]
    cursor: number
    invalidationSequences: Record<string, number>
  }> {
    let cursor = startCursor
    let activities = [...initialActivities]
    const invalidationSequences = { ...initialInvalidationSequences }
    for (;;) {
      const page = await this.source.listEvents({
        afterSequence: cursor,
        limit: EVENT_PAGE_SIZE,
        rootConversationId: this.rootConversationId
      })
      if (!this.isCurrent(generation)) return { activities, cursor, invalidationSequences }
      if (page.rootConversationId !== this.rootConversationId) throw new Error('Wrong root')
      for (const event of page.events) {
        if (event.rootConversationId !== this.rootConversationId || event.sequence !== cursor + 1) {
          throw new CollaborationEventGapError('Event log gap')
        }
        cursor = event.sequence
        invalidationSequences[event.agentId] = event.sequence
        if (event.activity) {
          activities.push({
            activityId: event.eventId,
            agentId: event.activity.agentId,
            occurredAt: event.occurredAt,
            rootAnchorMessageId: event.activity.rootAnchorMessageId,
            rootTraceBoundarySequence: event.activity.rootTraceBoundarySequence,
            runId: event.runId,
            semantic: event.activity.semantic,
            sequence: event.sequence,
            taskNameSnapshot: event.activity.taskNameSnapshot,
            turnId: event.turnId
          })
        }
      }
      activities = boundActivities(activities)
      if (!page.hasMore) return { activities, cursor, invalidationSequences }
      if (page.events.length === 0) throw new CollaborationEventGapError('Event log gap')
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

function boundActivities(
  activities: readonly CollaborationTimelineActivity[]
): CollaborationTimelineActivity[] {
  const byEventId = new Map<string, CollaborationTimelineActivity>()
  for (const activity of activities) byEventId.set(activity.activityId, activity)
  return [...byEventId.values()]
    .sort((left, right) => left.sequence - right.sequence)
    .slice(-MAX_COLLABORATION_TIMELINE_ACTIVITIES)
}
