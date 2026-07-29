import { describe, expect, it, vi } from 'vitest'
import { applyAuthoritativePendingActionDecision } from '../../features/agentRun/pendingActionDecision'

function deferred<TResult>() {
  let resolve!: (result: TResult) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<TResult>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, reject, resolve }
}

describe('applyAuthoritativePendingActionDecision', () => {
  it.each(['approve', 'reject'])(
    'does not apply an %s decision before the authoritative response',
    async () => {
      const response = deferred<{ actionId: string }>()
      const apply = vi.fn()
      const invoke = vi.fn(() => response.promise)

      const pendingDecision = applyAuthoritativePendingActionDecision({
        runId: 'run-1',
        invoke,
        apply,
        onError: vi.fn()
      })

      expect(invoke).toHaveBeenCalledWith('run-1')
      expect(apply).not.toHaveBeenCalled()

      const authoritativeResult = { actionId: 'action-1' }
      response.resolve(authoritativeResult)

      await expect(pendingDecision).resolves.toEqual({ status: 'applied' })
      expect(apply).toHaveBeenCalledOnce()
      expect(apply).toHaveBeenCalledWith(authoritativeResult)
    }
  )

  it.each(['approve', 'reject'])(
    'keeps the action unchanged when the %s request rejects',
    async () => {
      const error = new Error('request failed')
      const apply = vi.fn()
      const onError = vi.fn()

      const outcome = await applyAuthoritativePendingActionDecision({
        runId: 'run-1',
        invoke: vi.fn().mockRejectedValue(error),
        apply,
        onError
      })

      expect(outcome).toEqual({ status: 'failed', error })
      expect(apply).not.toHaveBeenCalled()
      expect(onError).toHaveBeenCalledOnce()
      expect(onError).toHaveBeenCalledWith(error)
    }
  )

  it('keeps the action unchanged when cancellation returns false', async () => {
    const apply = vi.fn()
    const onNotAccepted = vi.fn()

    const outcome = await applyAuthoritativePendingActionDecision({
      runId: 'run-1',
      invoke: vi.fn().mockResolvedValue(false),
      isAccepted: (cancelled) => cancelled,
      apply,
      onError: vi.fn(),
      onNotAccepted
    })

    expect(outcome).toEqual({ status: 'skipped', reason: 'not_accepted' })
    expect(apply).not.toHaveBeenCalled()
    expect(onNotAccepted).toHaveBeenCalledOnce()
    expect(onNotAccepted).toHaveBeenCalledWith(false)
  })

  it('keeps the action unchanged when cancellation throws', async () => {
    const error = new Error('cancel failed')
    const apply = vi.fn()
    const onError = vi.fn()

    const outcome = await applyAuthoritativePendingActionDecision({
      runId: 'run-1',
      invoke: vi.fn().mockRejectedValue(error),
      isAccepted: (cancelled) => cancelled,
      apply,
      onError
    })

    expect(outcome).toEqual({ status: 'failed', error })
    expect(apply).not.toHaveBeenCalled()
    expect(onError).toHaveBeenCalledWith(error)
  })

  it.each([undefined, null, '', '   '])(
    'does not invoke the backend without a usable run id (%s)',
    async (runId) => {
      const invoke = vi.fn()
      const apply = vi.fn()
      const onError = vi.fn()
      const onMissingRunId = vi.fn()

      const outcome = await applyAuthoritativePendingActionDecision({
        runId,
        invoke,
        apply,
        onError,
        onMissingRunId
      })

      expect(outcome).toEqual({ status: 'skipped', reason: 'missing_run_id' })
      expect(invoke).not.toHaveBeenCalled()
      expect(apply).not.toHaveBeenCalled()
      expect(onError).not.toHaveBeenCalled()
      expect(onMissingRunId).toHaveBeenCalledOnce()
    }
  )
})
