import { afterEach, describe, expect, it } from 'vitest'
import { currentRpcDiagnosticContext, withRpcDiagnosticContext } from './rpcDiagnosticContext'

const original = process.env.CAPTAIN_PERFORMANCE_DIAGNOSTICS
afterEach(() => {
  if (original === undefined) delete process.env.CAPTAIN_PERFORMANCE_DIAGNOSTICS
  else process.env.CAPTAIN_PERFORMANCE_DIAGNOSTICS = original
})
const first = { rendererSessionNonce: 'a'.repeat(32), actionSequence: 1 }
const second = { rendererSessionNonce: 'b'.repeat(32), actionSequence: 2 }

describe('optional diagnostic IPC context', () => {
  it('isolates overlapping asynchronous actions and clears scope after completion', async () => {
    process.env.CAPTAIN_PERFORMANCE_DIAGNOSTICS = '1'
    let releaseFirst!: () => void
    let releaseSecond!: () => void
    const firstGate = new Promise<void>((resolve) => {
      releaseFirst = resolve
    })
    const secondGate = new Promise<void>((resolve) => {
      releaseSecond = resolve
    })
    const firstCall = withRpcDiagnosticContext(first, async () => {
      await firstGate
      expect(currentRpcDiagnosticContext()).toEqual(first)
      await secondGate
      expect(currentRpcDiagnosticContext()).toEqual(first)
      return 'first result'
    })
    const secondCall = withRpcDiagnosticContext(second, async () => {
      releaseFirst()
      await Promise.resolve()
      expect(currentRpcDiagnosticContext()).toEqual(second)
      releaseSecond()
      return 'second result'
    })
    expect(await Promise.all([firstCall, secondCall])).toEqual(['first result', 'second result'])
    expect(currentRpcDiagnosticContext()).toBeUndefined()
  })

  it.each([
    undefined,
    null,
    [],
    {},
    { ...first, extra: 'secret' },
    { ...first, rendererSessionNonce: 'secret' },
    { ...first, rendererSessionNonce: 'a'.repeat(33) },
    { ...first, actionSequence: -1 },
    { ...first, actionSequence: 1.5 },
    { ...first, actionSequence: Number.MAX_SAFE_INTEGER + 1 }
  ])('ignores invalid metadata without inheriting another invocation: %#', (invalid) => {
    process.env.CAPTAIN_PERFORMANCE_DIAGNOSTICS = '1'
    withRpcDiagnosticContext(first, () => {
      expect(withRpcDiagnosticContext(invalid, () => currentRpcDiagnosticContext())).toBeUndefined()
      expect(currentRpcDiagnosticContext()).toEqual(first)
    })
  })

  it.each(['0', '1'])(
    'preserves original return identity and synchronous exceptions with enabled=%s',
    (enabled) => {
      process.env.CAPTAIN_PERFORMANCE_DIAGNOSTICS = enabled
      const promise = Promise.resolve('business result')
      expect(withRpcDiagnosticContext(first, () => promise)).toBe(promise)
      const error = new Error('original')
      expect(() =>
        withRpcDiagnosticContext(first, () => {
          throw error
        })
      ).toThrow(error)
      expect(withRpcDiagnosticContext(first, () => currentRpcDiagnosticContext())).toEqual(
        enabled === '1' ? first : undefined
      )
    }
  )
})
