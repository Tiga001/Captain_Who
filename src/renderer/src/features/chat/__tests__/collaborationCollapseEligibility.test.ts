import { describe, expect, it, vi } from 'vitest'
import { hasTrustedAnchoredCollaborationActivity } from '../components/chatMessageItemUtils'

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))

describe('collaboration Timeline collapse eligibility', () => {
  it('requires a paired durable trace boundary anchored to the current root message', () => {
    expect(
      hasTrustedAnchoredCollaborationActivity(
        [{ rootAnchorMessageId: 'assistant-root', rootTraceBoundarySequence: 0 }],
        'assistant-root'
      )
    ).toBe(true)

    expect(
      hasTrustedAnchoredCollaborationActivity(
        [{ rootAnchorMessageId: 'assistant-root', rootTraceBoundarySequence: null }],
        'assistant-root'
      )
    ).toBe(false)
    expect(
      hasTrustedAnchoredCollaborationActivity(
        [{ rootAnchorMessageId: 'assistant-other', rootTraceBoundarySequence: 1 }],
        'assistant-root'
      )
    ).toBe(false)
    expect(hasTrustedAnchoredCollaborationActivity([], 'assistant-root')).toBe(false)
  })
})
