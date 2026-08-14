import { Bot } from 'lucide-react'
import { useMemo } from 'react'
import type { CollaborationActivitySemantic as ProtocolCollaborationActivitySemantic } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../config/translationFormat'
import './CollaborationTimelineActivity.css'

export type CollaborationActivitySemantic = ProtocolCollaborationActivitySemantic

/** Renderer-safe projection of one typed durable collaboration event. */
export interface CollaborationTimelineActivity {
  activityId: string
  agentId: string
  occurredAt: number
  rootAnchorMessageId: string | null
  runId: string | null
  semantic: CollaborationActivitySemantic
  sequence: number
  taskNameSnapshot: string
  turnId: string | null
}

export interface CollaborationTimelineActivityGroup {
  activities: readonly CollaborationTimelineActivity[]
  rootAnchorMessageId: string | null
  semantic: CollaborationActivitySemantic
}

function compareActivity(
  left: CollaborationTimelineActivity,
  right: CollaborationTimelineActivity
): number {
  if (left.occurredAt !== right.occurredAt) return left.occurredAt - right.occurredAt
  if (left.sequence !== right.sequence) return left.sequence - right.sequence
  return left.activityId.localeCompare(right.activityId)
}

function canMerge(
  group: CollaborationTimelineActivityGroup,
  activity: CollaborationTimelineActivity
): boolean {
  const previous = group.activities.at(-1)
  if (!previous) return false
  if (
    group.semantic !== activity.semantic ||
    group.rootAnchorMessageId !== activity.rootAnchorMessageId
  ) {
    return false
  }
  if (group.activities.some((candidate) => candidate.agentId === activity.agentId)) return false
  if (activity.rootAnchorMessageId) return true
  // Unanchored events may only coalesce inside the same narrow timestamp bucket. This keeps
  // simultaneous multi-Agent activity compact without merging unrelated later work.
  return Math.floor(previous.occurredAt / 1_000) === Math.floor(activity.occurredAt / 1_000)
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

export function groupCollaborationTimelineActivities(
  input: readonly CollaborationTimelineActivity[]
): readonly CollaborationTimelineActivityGroup[] {
  const visible = normalizeCollaborationTimelineActivities(input)

  const groups: CollaborationTimelineActivityGroup[] = []
  for (const activity of visible) {
    const current = groups.at(-1)
    if (current && canMerge(current, activity)) {
      groups[groups.length - 1] = {
        ...current,
        activities: [...current.activities, activity]
      }
      continue
    }
    groups.push({
      activities: [activity],
      rootAnchorMessageId: activity.rootAnchorMessageId,
      semantic: activity.semantic
    })
  }
  return groups
}

function semanticLabel(semantic: CollaborationActivitySemantic, t: Translate): string {
  switch (semantic) {
    case 'started':
      return t('collaboration.activity.status.started')
    case 'updated':
      return t('collaboration.activity.status.updated')
    case 'waiting_approval':
      return t('collaboration.activity.status.waitingApproval')
    case 'completed':
      return t('collaboration.activity.status.completed')
    case 'failed':
      return t('collaboration.activity.status.failed')
    case 'interrupted':
      return t('collaboration.activity.status.interrupted')
  }
}

export function CollaborationTimelineActivityList({
  activities,
  onOpenAgent
}: {
  activities: readonly CollaborationTimelineActivity[]
  onOpenAgent: (agentId: string) => void
}) {
  const { t } = useFrontendConfig()
  const groups = useMemo(() => groupCollaborationTimelineActivities(activities), [activities])
  if (groups.length === 0) return null

  return (
    <div className="collaboration-timeline" data-testid="collaboration-timeline">
      {groups.map((group) => {
        const statusLabel = semanticLabel(group.semantic, t)
        return (
          <div
            className="collaboration-timeline__activity"
            data-semantic={group.semantic}
            key={group.activities.map((activity) => activity.activityId).join(':')}
          >
            <span className="collaboration-timeline__chips">
              {group.activities.map((activity) => (
                <button
                  aria-label={formatTranslation(t, 'collaboration.activity.openAgentActivity', {
                    name: activity.taskNameSnapshot,
                    status: statusLabel
                  })}
                  className="collaboration-timeline__chip"
                  data-agent-id={activity.agentId}
                  key={activity.activityId}
                  onClick={() => onOpenAgent(activity.agentId)}
                  type="button"
                >
                  <Bot aria-hidden="true" />
                  <span>{activity.taskNameSnapshot}</span>
                </button>
              ))}
            </span>
            <span
              aria-atomic="true"
              aria-live="polite"
              className="collaboration-timeline__status"
              data-tone={group.semantic}
            >
              {statusLabel}
            </span>
          </div>
        )
      })}
    </div>
  )
}
