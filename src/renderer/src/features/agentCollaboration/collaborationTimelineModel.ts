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
  createdAt: number
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
  return (
    group.semantic === activity.semantic &&
    group.rootAnchorMessageId === activity.rootAnchorMessageId &&
    group.rootTraceBoundarySequence === activity.rootTraceBoundarySequence
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
 * Coalesces only adjacent semantic rows. Repeated activity for the same Agent inside one
 * continuous row replaces that Agent's older snapshot while retaining the stable visual order.
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

function timestampMessageSlot(
  activity: CollaborationTimelineActivity,
  messages: readonly CollaborationTimelineMessageReference[]
): number {
  const nextMessageIndex = messages.findIndex((message) => message.createdAt > activity.occurredAt)
  return nextMessageIndex < 0 ? messages.length : nextMessageIndex
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(Math.max(value, minimum), maximum)
}

/**
 * Assigns root events to the shared Conversation timeline.
 *
 * A valid anchor always wins and is later placed at its trace boundary by ChatMessageItem. Events
 * without a usable anchor use their timestamp only to choose a coarse message slot. That fallback
 * slot is clamped between adjacent durable anchors and never moves backwards relative to an
 * earlier unanchored event, preserving root-local sequence through clock skew.
 */
export function projectCollaborationTimelineActivities(
  input: readonly CollaborationTimelineActivity[],
  messages: readonly CollaborationTimelineMessageReference[]
): CollaborationTimelineMessageProjection {
  const normalized = normalizeCollaborationTimelineActivities(input)
  const messageIndexById = new Map(messages.map((message, index) => [message.id, index]))
  const trustedAnchorMessageIndex = normalized.map((activity) => {
    if (activity.rootAnchorMessageId === null || activity.rootTraceBoundarySequence === null) {
      return null
    }
    return messageIndexById.get(activity.rootAnchorMessageId) ?? null
  })
  const previousAnchorIndex: Array<number | null> = []
  const nextAnchorIndex: Array<number | null> = []

  let previous: number | null = null
  for (let index = 0; index < normalized.length; index += 1) {
    previousAnchorIndex[index] = previous
    if (trustedAnchorMessageIndex[index] !== null) previous = trustedAnchorMessageIndex[index]
  }
  let next: number | null = null
  for (let index = normalized.length - 1; index >= 0; index -= 1) {
    nextAnchorIndex[index] = next
    if (trustedAnchorMessageIndex[index] !== null) next = trustedAnchorMessageIndex[index]
  }

  const anchoredMessage = new Map<string, CollaborationTimelineActivity[]>()
  const beforeMessage = new Map<string, CollaborationTimelineActivity[]>()
  const tail: CollaborationTimelineActivity[] = []
  let previousFallbackSlot = 0

  for (let index = 0; index < normalized.length; index += 1) {
    const activity = normalized[index]
    const trustedMessageIndex = trustedAnchorMessageIndex[index]
    if (trustedMessageIndex !== null && activity.rootAnchorMessageId !== null) {
      const slot = anchoredMessage.get(activity.rootAnchorMessageId) ?? []
      slot.push(activity)
      anchoredMessage.set(activity.rootAnchorMessageId, slot)
      continue
    }

    const priorAnchor = previousAnchorIndex[index]
    const followingAnchor = nextAnchorIndex[index]
    const lowerBound = Math.max(previousFallbackSlot, priorAnchor === null ? 0 : priorAnchor + 1)
    const upperBound = followingAnchor === null ? messages.length : followingAnchor
    const timestampSlot = timestampMessageSlot(activity, messages)
    // Contradictory anchors (for example two reverse-ordered trusted message identities) cannot be
    // repaired in the Renderer. Preserve exact anchors and keep this fallback monotonic instead of
    // fabricating an in-message trace boundary.
    const targetSlot =
      lowerBound <= upperBound
        ? clamp(timestampSlot, lowerBound, upperBound)
        : Math.max(previousFallbackSlot, timestampSlot)
    previousFallbackSlot = targetSlot

    const nextMessage = messages[targetSlot]
    if (!nextMessage) {
      tail.push(activity)
      continue
    }
    const slot = beforeMessage.get(nextMessage.id) ?? []
    slot.push(activity)
    beforeMessage.set(nextMessage.id, slot)
  }

  return { anchoredMessage, beforeMessage, tail }
}
