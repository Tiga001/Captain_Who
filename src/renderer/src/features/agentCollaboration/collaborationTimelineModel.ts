import type { CollaborationActivitySemantic as ProtocolCollaborationActivitySemantic } from '@mycopilot/protocol'

export type CollaborationActivitySemantic = ProtocolCollaborationActivitySemantic

/** Renderer-safe projection of one typed durable collaboration event. */
export interface CollaborationTimelineActivity {
  activityId: string
  agentId: string
  occurredAt: number
  ownerAgentId: string
  ownerConversationId: string
  taskMessageId: string | null
  anchorMessageId: string | null
  traceBoundarySequence: number | null
  runId: string | null
  semantic: CollaborationActivitySemantic
  sequence: number
  taskNameSnapshot: string
  turnId: string | null
}

export interface CollaborationTimelineActivityGroup {
  ownerAgentId: string
  ownerConversationId: string
  activities: readonly CollaborationTimelineActivity[]
  anchorMessageId: string | null
  traceBoundarySequence: number | null
  semantic: CollaborationActivitySemantic
}

export interface CollaborationTimelineMessageReference {
  id: string
}

export interface CollaborationTimelineMessageProjection {
  anchoredMessage: ReadonlyMap<string, readonly CollaborationTimelineActivity[]>
  beforeMessage: ReadonlyMap<string, readonly CollaborationTimelineActivity[]>
  afterMessage: ReadonlyMap<string, readonly CollaborationTimelineActivity[]>
  tail: readonly CollaborationTimelineActivity[]
}

function compareActivity(
  left: CollaborationTimelineActivity,
  right: CollaborationTimelineActivity
): number {
  // `sequence` is the root-local durable event order. Wall clocks may jump or be restored from a
  // different process, so timestamps can only break malformed/equal-sequence ties.
  if (left.sequence !== right.sequence) return left.sequence - right.sequence
  if (left.occurredAt !== right.occurredAt) return left.occurredAt - right.occurredAt
  // Multiple projections of one event retain the Host array order, never an ID-derived order.
  return 0
}

function canMerge(
  group: CollaborationTimelineActivityGroup,
  activity: CollaborationTimelineActivity
): boolean {
  const sharesAnchoredAssistantRun =
    group.anchorMessageId !== null &&
    group.traceBoundarySequence !== null &&
    activity.traceBoundarySequence !== null &&
    group.anchorMessageId === activity.anchorMessageId
  return (
    group.ownerAgentId === activity.ownerAgentId &&
    group.ownerConversationId === activity.ownerConversationId &&
    group.semantic === activity.semantic &&
    group.anchorMessageId === activity.anchorMessageId &&
    (sharesAnchoredAssistantRun || group.traceBoundarySequence === activity.traceBoundarySequence)
  )
}

/**
 * Produces the deterministic conversation Timeline projection from durable typed events.
 *
 * Terminal-result and generic-update deduplication belongs to the trusted backend semantic
 * projection. The Renderer only deduplicates stable event identities; it never guesses that a
 * genuine earlier `updated` event became unimportant merely because a later terminal event exists.
 */
export function normalizeCollaborationTimelineActivities(
  input: readonly CollaborationTimelineActivity[]
): readonly CollaborationTimelineActivity[] {
  const seen = new Set<string>()
  return [...input].sort(compareActivity).filter((activity) => {
    if (seen.has(activity.activityId)) return false
    seen.add(activity.activityId)
    return true
  })
}

/**
 * Coalesces only adjacent semantic rows. Activities anchored to the same Assistant message may
 * cross hidden trace boundaries: the caller has already split the list at every visible Timeline
 * item, so those boundaries must not turn consecutive Harness events into separate full rows.
 * Unanchored events retain the stricter boundary rule. Repeated activity for the same Agent inside
 * one continuous row replaces that task's older snapshot while retaining the stable visual order.
 */
export function groupCollaborationTimelineActivities(
  input: readonly CollaborationTimelineActivity[]
): readonly CollaborationTimelineActivityGroup[] {
  const visible = normalizeCollaborationTimelineActivities(input)
  const groups: CollaborationTimelineActivityGroup[] = []

  for (const activity of visible) {
    const current = groups.at(-1)
    if (current && canMerge(current, activity)) {
      const existingIndex = current.activities.findIndex(
        (candidate) =>
          candidate.agentId === activity.agentId &&
          candidate.taskMessageId === activity.taskMessageId
      )
      const nextActivities = [...current.activities]
      if (existingIndex >= 0) nextActivities[existingIndex] = activity
      else nextActivities.push(activity)
      groups[groups.length - 1] = { ...current, activities: nextActivities }
      continue
    }

    groups.push({
      ownerAgentId: activity.ownerAgentId,
      ownerConversationId: activity.ownerConversationId,
      activities: [activity],
      anchorMessageId: activity.anchorMessageId,
      traceBoundarySequence: activity.traceBoundarySequence,
      semantic: activity.semantic
    })
  }
  return groups
}

/**
 * Routes a projection only to its Host-confirmed owner's conversation using its transactionally committed
 * message/trace boundary. Missing messages are not guessed from wall clocks or notification order.
 * A null trace denotes the gap after a committed message; a null anchor denotes before any message.
 */
export function projectCollaborationTimelineActivities(
  input: readonly CollaborationTimelineActivity[],
  messages: readonly CollaborationTimelineMessageReference[],
  conversationId: string,
  collaborationTreeAgentIds?: readonly string[]
): CollaborationTimelineMessageProjection {
  const normalized = normalizeCollaborationTimelineActivities(input)
  const messageIds = new Set(messages.map((message) => message.id))
  const anchoredMessage = new Map<string, CollaborationTimelineActivity[]>()
  const beforeMessage = new Map<string, CollaborationTimelineActivity[]>()
  const afterMessage = new Map<string, CollaborationTimelineActivity[]>()
  const tail: CollaborationTimelineActivity[] = []
  for (const activity of normalized) {
    if (
      activity.ownerConversationId !== conversationId ||
      (collaborationTreeAgentIds !== undefined &&
        (!collaborationTreeAgentIds.includes(activity.agentId) ||
          !collaborationTreeAgentIds.includes(activity.ownerAgentId)))
    )
      continue
    const anchor = activity.anchorMessageId
    if (anchor === null) {
      if (activity.traceBoundarySequence !== null) continue
      const firstMessageId = messages[0]?.id
      if (firstMessageId) {
        const slot = beforeMessage.get(firstMessageId) ?? []
        slot.push(activity)
        beforeMessage.set(firstMessageId, slot)
      } else tail.push(activity)
      continue
    }
    if (!messageIds.has(anchor)) continue
    const destination = activity.traceBoundarySequence === null ? afterMessage : anchoredMessage
    const slot = destination.get(anchor) ?? []
    slot.push(activity)
    destination.set(anchor, slot)
  }

  return { anchoredMessage, beforeMessage, afterMessage, tail }
}
