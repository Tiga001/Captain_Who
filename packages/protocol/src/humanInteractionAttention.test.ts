import { describe, expect, it } from 'vitest'
import { parseHumanInteractionAttentionSnapshot } from './humanInteractionAttention'

const snapshot = {
  requestSequence: 10,
  requests: [{ requestId: 'request', conversationId: 'chat', sequence: 8, revision: 1 }],
  approvalConversationIds: ['chat']
}
describe('sparse human interaction attention contract', () => {
  it('accepts sparse request metadata with a deletion-resistant high-water mark', () => {
    expect(parseHumanInteractionAttentionSnapshot(snapshot)).toEqual(snapshot)
    expect(
      parseHumanInteractionAttentionSnapshot({ ...snapshot, requests: [] }).requestSequence
    ).toBe(10)
  })
  it.each([
    { ...snapshot, requestSequence: 7 },
    { ...snapshot, requestSequence: Number.MAX_SAFE_INTEGER + 1 },
    { ...snapshot, questions: [] },
    { ...snapshot, requests: [...snapshot.requests, ...snapshot.requests] },
    { ...snapshot, requests: [{ ...snapshot.requests[0], questions: ['private'] }] },
    { ...snapshot, requests: [{ ...snapshot.requests[0], revision: -1 }] },
    { ...snapshot, requests: [{ ...snapshot.requests[0], conversationId: ' chat' }] },
    { ...snapshot, approvalConversationIds: ['chat', 'chat'] }
  ])('rejects invalid/duplicate identities, versions or leaked detail fields', (value) => {
    expect(() => parseHumanInteractionAttentionSnapshot(value)).toThrow()
  })
})
