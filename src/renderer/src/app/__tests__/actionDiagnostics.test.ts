import { afterEach, describe, expect, it, vi } from 'vitest'
import { ActionDiagnostics } from '../../diagnostics/actionDiagnostics'

const collectors: ActionDiagnostics[] = []
afterEach(() => {
  collectors.forEach((collector) => collector.flush())
  collectors.length = 0
  vi.useRealTimers()
})

describe('Renderer action diagnostics', () => {
  it('measures preflush, RPC and apply independently on one local monotonic timeline', async () => {
    vi.useFakeTimers()
    let now = 10
    const emit = vi.fn()
    const diagnostics = new ActionDiagnostics(true, emit, () => now)
    collectors.push(diagnostics)
    const span = diagnostics.begin('approval.approve')
    await span.measureAsync('preflush', async () => {
      now += 300
    })
    expect(
      await span.measureAsync('rpc.wait', async () => {
        now += 20
        return 'durable response'
      })
    ).toBe('durable response')
    span.measureSync('state.apply', () => {
      now += 2
    })
    span.finish('completed')
    span.finish('failed') // A cleanup path must not double-count a settled operation.
    diagnostics.flush()
    expect(emit.mock.calls[0][0]).toMatchObject({
      clock: 'renderer.performance.now',
      offered: 1,
      pending: 0,
      outcomes: { 'approval.approve:completed': 1 },
      samples: {
        'approval.approve:preflush': { count: 1, totalMs: 300 },
        'approval.approve:rpc.wait': { count: 1, totalMs: 20 },
        'approval.approve:state.apply': { count: 1, totalMs: 2 },
        'approval.approve:total': { count: 1, totalMs: 322 }
      }
    })
    expect(
      emit.mock.calls[0][0].traces.map((trace: { actionSequence: number }) => trace.actionSequence)
    ).toEqual([1, 1, 1, 1])
  })

  it('preserves failures without recording Error objects, and reports incomplete actions', async () => {
    vi.useFakeTimers()
    let now = 0
    const emit = vi.fn()
    const diagnostics = new ActionDiagnostics(true, emit, () => now)
    collectors.push(diagnostics)
    const pending = diagnostics.begin('approval.projected')
    const failed = diagnostics.begin('approval.reject')
    const error = new Error('SECRET_PAYLOAD')
    await expect(
      failed.measureAsync('rpc.wait', async () => {
        now = 50
        throw error
      })
    ).rejects.toBe(error)
    failed.finish('failed')
    diagnostics.flush()
    expect(emit.mock.calls[0][0]).toMatchObject({
      offered: 2,
      pending: 1,
      peakPending: 2,
      outcomes: { 'approval.reject:failed': 1 }
    })
    expect(JSON.stringify(emit.mock.calls)).not.toContain('SECRET_PAYLOAD')
    pending.finish('completed')
  })

  it('bounds per-action traces and emits idle completions after 30 seconds', () => {
    vi.useFakeTimers()
    const emit = vi.fn()
    const diagnostics = new ActionDiagnostics(true, emit)
    collectors.push(diagnostics)
    for (let i = 0; i < 100; i += 1) diagnostics.begin('run.cancel_rpc').finish('completed')
    vi.advanceTimersByTime(30_000)
    expect(emit).toHaveBeenCalledOnce()
    expect(emit.mock.calls[0][0]).toMatchObject({ offered: 100, pending: 0, omittedTraces: 36 })
    expect(emit.mock.calls[0][0].traces).toHaveLength(64)
  })

  it('is inert when disabled, including promise identity and timers', () => {
    vi.useFakeTimers()
    const emit = vi.fn()
    const diagnostics = new ActionDiagnostics(false, emit)
    const promise = Promise.resolve('result')
    const span = diagnostics.begin('approval.approve')
    expect(span.measureAsync('rpc.wait', () => promise)).toBe(promise)
    span.finish('completed')
    diagnostics.flush()
    expect(vi.getTimerCount()).toBe(0)
    expect(emit).not.toHaveBeenCalled()
  })
  it.each([false, true])(
    'preserves promise identity, synchronous exceptions and continuation order when enabled=%s',
    async (enabled) => {
      vi.useFakeTimers()
      const diagnostics = new ActionDiagnostics(enabled, () => {})
      collectors.push(diagnostics)
      const span = diagnostics.begin('approval.approve')
      const error = new Error('synchronous failure')
      expect(() =>
        span.measureAsync('preflush', () => {
          throw error
        })
      ).toThrow(error)
      let resolve!: (value: string) => void
      const promise = new Promise<string>((done) => {
        resolve = done
      })
      const observed = span.measureAsync('rpc.wait', () => promise)
      expect(observed).toBe(promise)
      const order: string[] = []
      void observed.then(() => order.push('caller'))
      void promise.then(() => order.push('peer'))
      resolve('original')
      await expect(observed).resolves.toBe('original')
      expect(order).toEqual(['caller', 'peer'])
      span.finish('failed')
    }
  )
  it('creates a bounded correlation nonce only while enabled and keeps action sequences distinct', () => {
    vi.useFakeTimers()
    const disabled = new ActionDiagnostics(false)
    expect(disabled.begin('approval.approve').context).toBeUndefined()
    const emit = vi.fn()
    const diagnostics = new ActionDiagnostics(true, emit)
    collectors.push(diagnostics)
    const a = diagnostics.begin('approval.approve')
    const b = diagnostics.begin('approval.projected')
    expect(a.context?.rendererSessionNonce).toMatch(/^[a-f0-9]{32}$/)
    expect(b.context?.rendererSessionNonce).toBe(a.context?.rendererSessionNonce)
    expect(a.context?.actionSequence).toBe(1)
    expect(b.context?.actionSequence).toBe(2)
    a.finish('completed')
    b.finish('completed')
    diagnostics.flush()
    expect(emit.mock.calls[0][0].rendererSessionNonce).toBe(a.context?.rendererSessionNonce)
  })
})
