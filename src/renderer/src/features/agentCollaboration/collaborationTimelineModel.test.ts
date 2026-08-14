import { describe, expect, it } from 'vitest'
import {
  groupCollaborationTimelineActivities,
  normalizeCollaborationTimelineActivities,
  projectCollaborationTimelineActivities,
  type CollaborationTimelineActivity
} from './collaborationTimelineModel'

function activity(
  activityId: string,
  agentId: string,
  sequence: number,
  occurredAt: number,
  semantic: CollaborationTimelineActivity['semantic'] = 'started',
  rootAnchorMessageId: string | null = null,
  rootTraceBoundarySequence: number | null = rootAnchorMessageId === null ? null : sequence
): CollaborationTimelineActivity {
  return {
    activityId,
    agentId,
    occurredAt,
    rootAnchorMessageId,
    rootTraceBoundarySequence,
    runId: null,
    semantic,
    sequence,
    taskNameSnapshot: `${agentId}-${activityId}`,
    turnId: null
  }
}

describe('collaboration Timeline model', () => {
  it('uses durable root sequence before a skewed wall clock and deduplicates stable event IDs', () => {
    const first = activity('event-1', 'agent-a', 1, 9_000)
    const normalized = normalizeCollaborationTimelineActivities([
      activity('event-3', 'agent-c', 3, 1_000),
      activity('event-2', 'agent-b', 2, 2_000),
      first,
      first
    ])

    expect(normalized.map((candidate) => candidate.activityId)).toEqual([
      'event-1',
      'event-2',
      'event-3'
    ])
  })

  it('coalesces only equal adjacent semantics and keeps the latest snapshot per Agent', () => {
    const groups = groupCollaborationTimelineActivities([
      activity('start-a-old', 'agent-a', 1, 1_000, 'started', 'assistant-1', 3),
      activity('start-b', 'agent-b', 2, 1_100, 'started', 'assistant-1', 3),
      activity('start-a-new', 'agent-a', 3, 1_200, 'started', 'assistant-1', 3),
      activity('update-a', 'agent-a', 4, 1_300, 'updated', 'assistant-1', 3),
      activity('start-c', 'agent-c', 5, 1_400, 'started', 'assistant-1', 3),
      activity('start-d', 'agent-d', 6, 1_500, 'started', 'assistant-1', 4)
    ])

    expect(groups).toHaveLength(4)
    expect(groups[0]?.activities.map((candidate) => candidate.activityId)).toEqual([
      'start-a-new',
      'start-b'
    ])
    expect(groups[1]?.semantic).toBe('updated')
    expect(groups[2]?.activities.map((candidate) => candidate.agentId)).toEqual(['agent-c'])
    expect(groups[3]?.rootTraceBoundarySequence).toBe(4)
  })

  it('honors exact anchors and clamps timestamp fallback between adjacent durable anchors', () => {
    const messages = [
      { id: 'message-1', createdAt: 1_000 },
      { id: 'message-2', createdAt: 5_000 },
      { id: 'message-3', createdAt: 9_000 }
    ]
    const projection = projectCollaborationTimelineActivities(
      [
        activity('before-anchor', 'agent-a', 1, 99_000, 'started'),
        activity('anchored-middle', 'agent-b', 2, 5_100, 'started', 'message-2', 7),
        activity('after-anchor', 'agent-c', 3, 100, 'updated'),
        activity('anchored-last', 'agent-d', 4, 9_100, 'completed', 'message-3', 8)
      ],
      messages
    )

    expect(
      projection.beforeMessage.get('message-2')?.map((candidate) => candidate.activityId)
    ).toEqual(['before-anchor'])
    expect(projection.anchoredMessage.get('message-2')?.[0]?.activityId).toBe('anchored-middle')
    expect(
      projection.beforeMessage.get('message-3')?.map((candidate) => candidate.activityId)
    ).toEqual(['after-anchor'])
    expect(projection.anchoredMessage.get('message-3')?.[0]?.activityId).toBe('anchored-last')
    expect(projection.tail).toEqual([])
  })

  it('never lets clock skew reverse consecutive unanchored root events', () => {
    const projection = projectCollaborationTimelineActivities(
      [
        activity('sequence-first', 'agent-a', 1, 8_000, 'started'),
        activity('sequence-second', 'agent-b', 2, 100, 'updated')
      ],
      [
        { id: 'message-1', createdAt: 1_000 },
        { id: 'message-2', createdAt: 5_000 },
        { id: 'message-3', createdAt: 9_000 }
      ]
    )

    expect(
      projection.beforeMessage.get('message-3')?.map((candidate) => candidate.activityId)
    ).toEqual(['sequence-first', 'sequence-second'])
  })
})
