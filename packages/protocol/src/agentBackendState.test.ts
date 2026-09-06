import { describe, expect, it } from 'vitest'
import { parseConversationBackendState } from './agentBackendState'

const state = {
  type: 'backend_state',
  sequence: 12,
  eventId: 'ignored:request-1',
  createdAt: 42,
  content: JSON.stringify({
    type: 'human_interaction_status',
    requestId: 'request-1',
    status: 'ignored'
  }),
  placement: 'after_message'
}

describe('internal backend state trace contract', () => {
  it('preserves the exact host fact and temporal placement', () => {
    expect(parseConversationBackendState(state)).toEqual(state)
    expect(parseConversationBackendState({ ...state, placement: 'timeline' }).placement).toBe(
      'timeline'
    )
  })
  it('rejects malformed identities, payloads and additional authority fields', () => {
    for (const patch of [
      { eventId: '' },
      { sequence: -1 },
      { createdAt: -1 },
      { content: 'not JSON' },
      { content: '[]' },
      { content: JSON.stringify({ value: 'x'.repeat(16 * 1024) }) },
      { placement: 'system' },
      { approved: true }
    ])
      expect(() => parseConversationBackendState({ ...state, ...patch })).toThrow()
  })
})
