import { describe, expect, it } from 'vitest'
import {
  captureHostInvocation,
  HostInvocationError,
  unwrapHostInvocation
} from '@mycopilot/host-api'

describe('structured host invocation transport', () => {
  it('round-trips successful values through a serializable envelope', async () => {
    const result = await captureHostInvocation(async () => ({ runId: 'run-1' }))

    expect(result).toEqual({ ok: true, value: { runId: 'run-1' } })
    expect(unwrapHostInvocation(result)).toEqual({ runId: 'run-1' })
  })

  it('preserves JSON-RPC code and typed recovery data for renderer callers', async () => {
    const source = Object.assign(new Error('Skill changed after selection'), {
      code: -32000,
      data: {
        type: 'skillActivation',
        code: 'stale',
        recovery: 'refreshCatalog',
        expectedRevision: 'old',
        actualRevision: 'new'
      }
    })
    const result = await captureHostInvocation(async () => {
      throw source
    })

    expect(result).toEqual({
      ok: false,
      error: {
        message: 'Skill changed after selection',
        code: -32000,
        data: source.data
      }
    })
    try {
      unwrapHostInvocation(result)
      expect.fail('a failed invocation must throw in the renderer layer')
    } catch (error) {
      expect(error).toBeInstanceOf(HostInvocationError)
      expect(error).toMatchObject({
        message: 'Skill changed after selection',
        code: -32000,
        data: source.data
      })
    }
  })
})
