import { describe, expect, it } from 'vitest'
import { parseAgentEventForHost } from './index'
import { parseMcpAgentChatOutput } from './agentParsers/agentOutput'

describe('waiting_for_user_input transport status', () => {
  it.each(['waiting_for_approval', 'waiting_for_user_input'] as const)(
    'accepts %s in authoritative state and worker-segment completion events',
    (status) => {
      const state = {
        type: 'state',
        runId: 'run-human-input',
        state: { status, activeRunId: 'run-human-input', lastError: null, updatedAt: 10 }
      }
      const done = {
        type: 'done',
        runId: 'run-human-input',
        success: true,
        status,
        usage: { inputTokens: 10, outputTokens: 2, totalTokens: 12, billableRequestCount: 1 }
      }
      expect(parseAgentEventForHost(state)).toEqual(state)
      expect(parseAgentEventForHost(done)).toEqual(done)
      expect(
        parseMcpAgentChatOutput(
          { content: '', status, runId: 'run-human-input', events: [], proposedActions: [] },
          'output'
        )
      ).toMatchObject({ status, runId: 'run-human-input', proposedActions: [] })
    }
  )

  it('still rejects unknown wait statuses instead of treating them as valid suspension', () => {
    expect(() =>
      parseAgentEventForHost({
        type: 'done',
        runId: 'run-human-input',
        success: true,
        status: 'waiting_for_human'
      })
    ).toThrow()
  })
})
