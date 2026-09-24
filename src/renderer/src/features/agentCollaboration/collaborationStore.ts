import type { AgentEvent, AgentTreeSnapshot, CollaborationEventEnvelope } from '@mycopilot/protocol'
import { hostCollaborationDataSource, type CollaborationDataSource } from './collaborationClient'
import type { CollaborationTimelineActivity } from './collaborationTimelineModel'
import type { AgentTreeTransmission } from './agentTreeTransmission'

const EVENT_PAGE_SIZE = 256
export const MAX_COLLABORATION_TIMELINE_ACTIVITIES = 2_048
export const TREE_TRANSMISSION_LIFETIME_MS = 5_000
const MAX_TREE_TRANSMISSIONS = 24
const MAX_SEEN_TRANSMISSIONS = 2_048

class CollaborationEventGapError extends Error {}

export interface CollaborationStoreSnapshot {
  /** Durable activity by direct parent; completed inline history also lives in message snapshots. */
  activities: readonly CollaborationTimelineActivity[]
  /** Latest validated durable invalidation sequence for each Agent in this root tree. */
  agentInvalidationSequences: Readonly<Record<string, number>>
  error: boolean
  hydrationRevision: number
  loading: boolean
  rootConversationId: string
  tree: AgentTreeSnapshot | null
  /** Ephemeral presentation only. Hydration/recovery never populates these pulses. */
  transmissions?: readonly AgentTreeTransmission[]
}

/**
 * Root-scoped Agent index and semantic-activity store. Conversation messages remain owned by the
 * chat store; this class hydrates tree/display state plus a bounded semantic
 * activity projection and closes notification gaps from the durable log.
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
  private unsubscribeAgentEvents: (() => void) | null = null
  private transmissionExpiryTimer: ReturnType<typeof setTimeout> | null = null
  private readonly seenTransmissionIds = new Set<string>()
  private readonly rootRunIds = new Set<string>()
  private readonly pendingGuidanceTransmissions = new Map<
    string,
    { runId: string; receivedAt: number }
  >()

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
      tree: null,
      transmissions: []
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
    this.unsubscribeAgentEvents = this.source.subscribeAgentEvents?.(this.handleAgentEvent) ?? null
    void this.hydrate()
  }

  async hydrate(): Promise<void> {
    if (this.destroyed) return
    const generation = ++this.generation
    this.clearTransmissionTimer()
    this.pendingGuidanceTransmissions.clear()
    this.publish({ error: false, loading: true, transmissions: [] })
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
    this.unsubscribeAgentEvents?.()
    this.unsubscribeAgentEvents = null
    this.clearTransmissionTimer()
    this.rootRunIds.clear()
    this.seenTransmissionIds.clear()
    this.pendingGuidanceTransmissions.clear()
    this.listeners.clear()
  }

  private readonly handleEvent = (event: CollaborationEventEnvelope): void => {
    if (this.destroyed || event.rootConversationId !== this.rootConversationId) return
    if (this.snapshot.loading) {
      this.catchUpRequested = true
      return
    }
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

  private readonly handleAgentEvent = (event: AgentEvent): void => {
    const tree = this.snapshot.tree
    if (this.destroyed || this.snapshot.loading || !tree || event.type !== 'guidance_applied')
      return
    // Applied guidance has a committed Human message. Queued/rejected drafts are not transfers.
    const id = `user-message:${event.clientMessageId}`
    if (this.seenTransmissionIds.has(id)) return
    this.pendingGuidanceTransmissions.set(id, { runId: event.runId, receivedAt: Date.now() })
    if (this.pendingGuidanceTransmissions.size > MAX_TREE_TRANSMISSIONS) {
      this.pendingGuidanceTransmissions.delete(
        this.pendingGuidanceTransmissions.keys().next().value!
      )
    }
    this.publish({ transmissions: this.acceptTransmissions(this.takeRootGuidance(tree), tree) })
    this.scheduleTransmissionExpiry()
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
      let transmissions: AgentTreeTransmission[] = []

      try {
        for (;;) {
          const replay = await this.replayDurableEvents(
            generation,
            cursor,
            activities,
            agentInvalidationSequences,
            true
          )
          if (!this.isCurrent(generation) || !this.snapshot.tree) return
          const previousCursor = cursor
          cursor = replay.cursor
          activities = replay.activities
          agentInvalidationSequences = replay.invalidationSequences
          transmissions = [...transmissions, ...replay.transmissions].slice(-MAX_TREE_TRANSMISSIONS)
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
              tree: null,
              transmissions: []
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
            tree,
            transmissions: this.acceptTransmissions(
              [...transmissions, ...this.takeRootGuidance(tree)],
              tree
            )
          })
          this.scheduleTransmissionExpiry()
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
    initialInvalidationSequences: Readonly<Record<string, number>> = {},
    collectTransmissions = false
  ): Promise<{
    activities: CollaborationTimelineActivity[]
    cursor: number
    invalidationSequences: Record<string, number>
    transmissions: AgentTreeTransmission[]
  }> {
    let cursor = startCursor
    let activities = [...initialActivities]
    const invalidationSequences = { ...initialInvalidationSequences }
    let transmissions: AgentTreeTransmission[] = []
    for (;;) {
      const page = await this.source.listEvents({
        afterSequence: cursor,
        limit: EVENT_PAGE_SIZE,
        rootConversationId: this.rootConversationId
      })
      if (!this.isCurrent(generation))
        return { activities, cursor, invalidationSequences, transmissions }
      if (page.rootConversationId !== this.rootConversationId) throw new Error('Wrong root')
      for (const event of page.events) {
        if (event.rootConversationId !== this.rootConversationId || event.sequence !== cursor + 1) {
          throw new CollaborationEventGapError('Event log gap')
        }
        cursor = event.sequence
        if (
          event.agentId === event.rootAgentId &&
          event.runId &&
          (event.kind === 'turn_started' || event.kind === 'turn_updated')
        ) {
          this.rootRunIds.add(event.runId)
          if (this.rootRunIds.size > 64)
            this.rootRunIds.delete(this.rootRunIds.values().next().value!)
        }
        if (event.transmission) {
          const now = Date.now()
          if (!collectTransmissions) this.rememberTransmission(event.transmission.id)
          else if (now - event.occurredAt <= TREE_TRANSMISSION_LIFETIME_MS) {
            transmissions.push({ ...event.transmission, receivedAt: now })
          }
        }
        invalidationSequences[event.agentId] = event.sequence
        if (event.activity) {
          invalidationSequences[event.activity.parentAgentId] = event.sequence
          const activity: CollaborationTimelineActivity = {
            activityId: event.eventId,
            agentId: event.activity.agentId,
            occurredAt: event.occurredAt,
            parentAgentId: event.activity.parentAgentId,
            parentConversationId: event.activity.parentConversationId,
            anchorMessageId: event.activity.anchorMessageId,
            traceBoundarySequence: event.activity.traceBoundarySequence,
            runId: event.runId,
            semantic: event.activity.semantic,
            sequence: event.sequence,
            taskNameSnapshot: event.activity.taskNameSnapshot,
            turnId: event.turnId
          }
          activities.push(activity)
        }
      }
      activities = boundActivities(activities)
      transmissions = transmissions.slice(-MAX_TREE_TRANSMISSIONS)
      if (!page.hasMore) return { activities, cursor, invalidationSequences, transmissions }
      if (page.events.length === 0) throw new CollaborationEventGapError('Event log gap')
    }
  }

  private isCurrent(generation: number): boolean {
    return !this.destroyed && generation === this.generation
  }

  private rememberTransmission(id: string): void {
    this.seenTransmissionIds.add(id)
    if (this.seenTransmissionIds.size > MAX_SEEN_TRANSMISSIONS) {
      this.seenTransmissionIds.delete(this.seenTransmissionIds.values().next().value!)
    }
  }

  private takeRootGuidance(tree: AgentTreeSnapshot): AgentTreeTransmission[] {
    const now = Date.now()
    const pulses: AgentTreeTransmission[] = []
    for (const [id, pending] of this.pendingGuidanceTransmissions) {
      if (now - pending.receivedAt >= TREE_TRANSMISSION_LIFETIME_MS) {
        this.pendingGuidanceTransmissions.delete(id)
      } else if (this.rootRunIds.has(pending.runId)) {
        this.pendingGuidanceTransmissions.delete(id)
        pulses.push({
          id,
          kind: 'user_message',
          sourceAgentId: null,
          targetAgentId: tree.rootAgentId,
          receivedAt: pending.receivedAt
        })
      }
    }
    return pulses
  }

  private acceptTransmissions(
    candidates: readonly AgentTreeTransmission[],
    tree: AgentTreeSnapshot
  ): AgentTreeTransmission[] {
    const now = Date.now()
    const ids = new Set(tree.agents.map((agent) => agent.agentId))
    ids.add(tree.rootAgentId)
    const pulses = (this.snapshot.transmissions ?? []).filter(
      (pulse) => now - pulse.receivedAt < TREE_TRANSMISSION_LIFETIME_MS
    )
    for (const pulse of candidates) {
      if (this.seenTransmissionIds.has(pulse.id)) continue
      this.rememberTransmission(pulse.id)
      if (now - pulse.receivedAt >= TREE_TRANSMISSION_LIFETIME_MS) continue
      if (pulse.sourceAgentId === pulse.targetAgentId) continue
      if (pulse.sourceAgentId !== null && !ids.has(pulse.sourceAgentId)) continue
      if (pulse.targetAgentId !== null && !ids.has(pulse.targetAgentId)) continue
      if (
        pulse.sourceAgentId === null &&
        (pulse.kind !== 'user_message' || pulse.targetAgentId !== tree.rootAgentId)
      )
        continue
      if (
        pulse.targetAgentId === null &&
        (pulse.kind !== 'completion' || pulse.sourceAgentId !== tree.rootAgentId)
      )
        continue
      pulses.push(pulse)
    }
    return pulses.slice(-MAX_TREE_TRANSMISSIONS)
  }

  private clearTransmissionTimer(): void {
    if (this.transmissionExpiryTimer !== null) clearTimeout(this.transmissionExpiryTimer)
    this.transmissionExpiryTimer = null
  }

  private scheduleTransmissionExpiry(): void {
    this.clearTransmissionTimer()
    const oldest = this.snapshot.transmissions?.[0]
    if (!oldest || this.destroyed) return
    this.transmissionExpiryTimer = setTimeout(
      () => {
        this.transmissionExpiryTimer = null
        if (this.destroyed) return
        const now = Date.now()
        this.publish({
          transmissions: (this.snapshot.transmissions ?? []).filter(
            (pulse) => now - pulse.receivedAt < TREE_TRANSMISSION_LIFETIME_MS
          )
        })
        this.scheduleTransmissionExpiry()
      },
      Math.max(1, oldest.receivedAt + TREE_TRANSMISSION_LIFETIME_MS - Date.now())
    )
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
  const ordered = [...byEventId.values()].sort((left, right) => left.sequence - right.sequence)
  // Inline history is frozen into settled message snapshots. Between-message events have no run
  // snapshot, so keep them when trimming the live inline window, including across rehydration.
  const orderedInlineCount = ordered.filter(
    (activity) => activity.traceBoundarySequence !== null
  ).length
  let inlineCount = 0
  return ordered.filter((activity) => {
    if (activity.traceBoundarySequence === null) return true
    inlineCount += 1
    return inlineCount > orderedInlineCount - MAX_COLLABORATION_TIMELINE_ACTIVITIES
  })
}
