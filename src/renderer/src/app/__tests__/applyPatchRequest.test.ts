import { describe, expect, it } from 'vitest'
import { getApplyPatchRequest } from '../../features/agentRun/applyPatchRequest'

describe('getApplyPatchRequest', () => {
  it('returns the exact nested request', () => {
    const request = { action: 'commit', transactionId: 'file-change-1' }

    expect(getApplyPatchRequest({ request })).toBe(request)
  })

  it.each([
    null,
    {},
    { action: 'commit', transactionId: 'file-change-1' },
    { request: null },
    { request: [] },
    { request: { action: 'commit' }, extra: true }
  ])('rejects flat, malformed, and mixed roots: %j', (args) => {
    expect(getApplyPatchRequest(args)).toBeUndefined()
  })
})
