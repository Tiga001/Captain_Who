import { describe, expect, it } from 'vitest'
import {
  getFinalMessageContent,
  getFinalTimeline,
  getMessageContentAfterDelta
} from '../../features/agentRun/messageTimeline'
import { ensureAgentRun } from '../../features/agentRun/agentEventReducerShared'
import type { ChatAgentTimelineItem } from '../../features/chat/chatTypes'

describe('messageTimeline', () => {
  it('streams into an initially empty assistant message', () => {
    expect(getMessageContentAfterDelta('', 'Hello')).toBe('Hello')
  })

  it('treats Chinese prose as content instead of a hidden status sentinel', () => {
    expect(getMessageContentAfterDelta('正在思考...', '下一段')).toBe('正在思考...下一段')
  })

  it('preserves current content unless the terminal event supplies final content', () => {
    expect(getFinalMessageContent('streamed')).toBe('streamed')
    expect(getFinalMessageContent('streamed', 'final')).toBe('final')
  })

  it.each(['running', 'completed'] as const)(
    'projects existing duplicate narration once without changing its durable position (%s)',
    (status) => {
      const timeline: ChatAgentTimelineItem[] = [
        {
          id: 'message-stream-first',
          type: 'message',
          content: 'partial',
          streamId: 'first',
          traceSequence: 0
        },
        { id: 'call', type: 'tool_call', callId: 'call', traceSequence: 0 },
        { id: 'trace-message-0', type: 'message', content: 'Full narration', traceSequence: 0 },
        { id: 'trace-message-2', type: 'message', content: 'Next narration', traceSequence: 2 },
        {
          id: 'message-stream-second',
          type: 'message',
          content: 'Next narration',
          streamId: 'second',
          traceSequence: 2
        }
      ]
      const run = { ...ensureAgentRun(undefined, 'run'), status, timeline }
      const projected = getFinalTimeline(run)
      expect(projected).toEqual([timeline[1], timeline[2], timeline[3]])
      expect(run.timeline).toHaveLength(5)
      expect(getFinalTimeline({ ...run, timeline: projected })).toEqual(projected)
    }
  )

  it('retains identical prose in distinct events and uncommitted streams', () => {
    const timeline: ChatAgentTimelineItem[] = [
      { id: 'trace-message-0', type: 'message', content: 'Checking', traceSequence: 0 },
      { id: 'trace-message-2', type: 'message', content: 'Checking', traceSequence: 2 },
      { id: 'message-stream-live', type: 'message', content: 'Checking', streamId: 'live' },
      { id: 'legacy-message', type: 'message', content: 'Checking' }
    ]
    expect(getFinalTimeline({ ...ensureAgentRun(undefined, 'run', 'running'), timeline })).toBe(
      timeline
    )
  })
})
