import { describe, expect, it, vi } from 'vitest'
import { hasTrustedAnchoredCollaborationActivity } from '../components/chatMessageItemUtils'

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))

describe('collaboration Timeline collapse eligibility', () => {
  it('requires a paired durable trace boundary anchored to the current root message', () => {
    expect(
      hasTrustedAnchoredCollaborationActivity(
        [{ anchorMessageId: 'assistant-root', traceBoundarySequence: 0 }],
        'assistant-root'
      )
    ).toBe(true)

    expect(
      hasTrustedAnchoredCollaborationActivity(
        [{ anchorMessageId: 'assistant-root', traceBoundarySequence: null }],
        'assistant-root'
      )
    ).toBe(false)
    expect(
      hasTrustedAnchoredCollaborationActivity(
        [{ anchorMessageId: 'assistant-other', traceBoundarySequence: 1 }],
        'assistant-root'
      )
    ).toBe(false)
    expect(hasTrustedAnchoredCollaborationActivity([], 'assistant-root')).toBe(false)
  })
})
