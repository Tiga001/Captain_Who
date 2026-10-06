import { describe, expect, it } from 'vitest'
import { parseAgentEventForHost } from './agentParsers/events'

const event = {
  type: 'model_activity_changed',
  runId: 'run-activity',
  streamId: 'stream-activity',
  attempt: 1,
  activity: 'reasoning'
}

describe('model activity event contract', () => {
  it('accepts only a content-free final reply marker with an exact run identity', () => {
    const ready = { type: 'final_answer_ready', runId: event.runId }
    expect(parseAgentEventForHost(ready)).toEqual(ready)
    for (const patch of [
      { runId: '' },
      { runId: null },
      { runId: undefined },
      { content: 'private canary' },
      { status: 'completed' },
      { streamId: 'guessed-stream' }
    ]) {
      expect(() => parseAgentEventForHost({ ...ready, ...patch })).toThrow()
    }
  })

  it.each(['reasoning', 'waiting'])('accepts content-free %s activity', (activity) => {
    expect(parseAgentEventForHost({ ...event, activity })).toEqual({ ...event, activity })
  })

  it.each([
    { activity: 'idle' },
    { activity: undefined },
    { streamId: '' },
    { runId: '' },
    { attempt: 0 },
    { attempt: 1.5 },
    { attempt: Number.MAX_SAFE_INTEGER + 1 },
    { reasoning: 'private model content' }
  ])('rejects invalid activity metadata %j', (patch) => {
    expect(() => parseAgentEventForHost({ ...event, ...patch })).toThrow()
  })
})
