import type { CollaborationActivitySemantic as ProtocolCollaborationActivitySemantic } from '@mycopilot/protocol'

export type CollaborationActivitySemantic = ProtocolCollaborationActivitySemantic

/** Renderer-safe projection of one typed durable collaboration event. */
export interface CollaborationTimelineActivity {
  activityId: string
  agentId: string
  occurredAt: number
  rootAnchorMessageId: string | null
  rootTraceBoundarySequence: number | null
  runId: string | null
  semantic: CollaborationActivitySemantic
  sequence: number
  taskNameSnapshot: string
  turnId: string | null
}

export interface CollaborationTimelineActivityGroup {
  activities: readonly CollaborationTimelineActivity[]
  rootAnchorMessageId: string | null
  rootTraceBoundarySequence: number | null
  semantic: CollaborationActivitySemantic
}

export interface CollaborationTimelineMessageReference {
  id: string
}

export interface CollaborationTimelineMessageProjection {
  anchoredMessage: ReadonlyMap<string, readonly CollaborationTimelineActivity[]>
  beforeMessage: ReadonlyMap<string, readonly CollaborationTimelineActivity[]>
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
  return left.activityId.localeCompare(right.activityId)
}

function canMerge(
  group: CollaborationTimelineActivityGroup,
  activity: CollaborationTimelineActivity
): boolean {
  const sharesAnchoredAssistantRun =
    group.rootAnchorMessageId !== null && group.rootAnchorMessageId === activity.rootAnchorMessageId
  return (
    group.semantic === activity.semantic &&
    group.rootAnchorMessageId === activity.rootAnchorMessageId &&
    (sharesAnchoredAssistantRun ||
      group.rootTraceBoundarySequence === activity.rootTraceBoundarySequence)
  )
}

/**
 * Produces the deterministic root Timeline projection from durable typed events.
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
 * one continuous row replaces that Agent's older snapshot while retaining the stable visual order.
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
        (candidate) => candidate.agentId === activity.agentId
      )
      const nextActivities = [...current.activities]
      if (existingIndex >= 0) nextActivities[existingIndex] = activity
      else nextActivities.push(activity)
      groups[groups.length - 1] = { ...current, activities: nextActivities }
      continue
    }

    groups.push({
      activities: [activity],
      rootAnchorMessageId: activity.rootAnchorMessageId,
      rootTraceBoundarySequence: activity.rootTraceBoundarySequence,
      semantic: activity.semantic
    })
  }
  return groups
}

/**
 * Assigns root events to the shared Conversation timeline.
 *
 * The backend captures the root Assistant message and trace boundary in the same transaction as
 * an activity only while that root Turn is still in progress. An activity committed after the
 * parent Turn settles is deliberately durable but unanchored: it continues to invalidate Agent
 * Center/detail state and must not append to or rewrite the frozen root chat Timeline.
 *
 * Consequently there is no timestamp or notification-arrival fallback here. A paired durable
 * anchor that resolves to a message in this exact Conversation is the only inline authority. A
 * notification delivered after settlement still renders when its event was causally committed
 * before settlement and therefore carries that durable anchor.
 */
export function projectCollaborationTimelineActivities(
  input: readonly CollaborationTimelineActivity[],
  messages: readonly CollaborationTimelineMessageReference[]
): CollaborationTimelineMessageProjection {
  const normalized = normalizeCollaborationTimelineActivities(input)
  const messageIds = new Set(messages.map((message) => message.id))
  const anchoredMessage = new Map<string, CollaborationTimelineActivity[]>()
  for (const activity of normalized) {
    const anchor = activity.rootAnchorMessageId
    if (anchor === null || activity.rootTraceBoundarySequence === null || !messageIds.has(anchor)) {
      continue
    }
    const slot = anchoredMessage.get(anchor) ?? []
    slot.push(activity)
    anchoredMessage.set(anchor, slot)
  }

  return { anchoredMessage, beforeMessage: new Map(), tail: [] }
}
