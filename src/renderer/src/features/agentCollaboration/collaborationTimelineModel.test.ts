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

  it('coalesces equal adjacent semantics across hidden boundaries in one anchored Assistant run', () => {
    const groups = groupCollaborationTimelineActivities([
      activity('start-a-old', 'agent-a', 1, 1_000, 'started', 'assistant-1', 3),
      activity('start-b', 'agent-b', 2, 1_100, 'started', 'assistant-1', 3),
      activity('start-a-new', 'agent-a', 3, 1_200, 'started', 'assistant-1', 3),
      activity('update-a', 'agent-a', 4, 1_300, 'updated', 'assistant-1', 3),
      activity('start-c', 'agent-c', 5, 1_400, 'started', 'assistant-1', 3),
      activity('start-d', 'agent-d', 6, 1_500, 'started', 'assistant-1', 4)
    ])

    expect(groups).toHaveLength(3)
    expect(groups[0]?.activities.map((candidate) => candidate.activityId)).toEqual([
      'start-a-new',
      'start-b'
    ])
    expect(groups[1]?.semantic).toBe('updated')
    expect(groups[2]?.activities.map((candidate) => candidate.agentId)).toEqual([
      'agent-c',
      'agent-d'
    ])
  })

  it('does not merge unanchored activities across distinct trace boundaries', () => {
    const groups = groupCollaborationTimelineActivities([
      activity('unanchored-a', 'agent-a', 1, 1_000, 'started', null, 3),
      activity('unanchored-b', 'agent-b', 2, 1_100, 'started', null, 4)
    ])

    expect(groups).toHaveLength(2)
  })

  it('projects only paired durable anchors that resolve in the current Conversation', () => {
    const messages = [{ id: 'message-1' }, { id: 'message-2' }, { id: 'message-3' }]
    const projection = projectCollaborationTimelineActivities(
      [
        activity('post-terminal-unanchored', 'agent-a', 1, 99_000, 'started'),
        activity('anchored-middle', 'agent-b', 2, 5_100, 'started', 'message-2', 7),
        activity('unknown-message-anchor', 'agent-c', 3, 100, 'updated', 'other-root-message', 8),
        activity('anchored-last', 'agent-d', 4, 9_100, 'completed', 'message-3', 9)
      ],
      messages
    )

    expect(projection.anchoredMessage.get('message-2')?.[0]?.activityId).toBe('anchored-middle')
    expect(projection.anchoredMessage.get('message-3')?.[0]?.activityId).toBe('anchored-last')
    expect(projection.anchoredMessage.has('other-root-message')).toBe(false)
    expect(projection.beforeMessage.size).toBe(0)
    expect(projection.tail).toEqual([])
  })

  it('keeps the anchored pre-terminal snapshot frozen when a later durable event is unanchored', () => {
    const projection = projectCollaborationTimelineActivities(
      [
        activity('started-before-terminal', 'agent-a', 1, 8_000, 'started', 'message-1', 2),
        activity('completed-after-terminal', 'agent-a', 2, 100, 'completed')
      ],
      [{ id: 'message-1' }]
    )

    expect(
      projection.anchoredMessage.get('message-1')?.map((candidate) => candidate.activityId)
    ).toEqual(['started-before-terminal'])
    expect(projection.beforeMessage.size).toBe(0)
    expect(projection.tail).toEqual([])
  })
})
