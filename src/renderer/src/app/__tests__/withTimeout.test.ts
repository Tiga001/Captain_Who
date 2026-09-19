import { describe, expect, it, vi } from 'vitest'
import {
  START_TURN_TIMEOUT_MS,
  TurnAcceptanceTimeoutError,
  withTimeout
} from '../withTimeout'

describe('withTimeout', () => {
  it('passes through the resolved value when the promise settles in time', async () => {
    vi.useFakeTimers()
    try {
      let resolveOriginal: (value: string) => void = () => {}
      const original = new Promise<string>((resolve) => {
        resolveOriginal = resolve
      })

      const wrapped = withTimeout(original, START_TURN_TIMEOUT_MS)
      resolveOriginal('accepted')

      await expect(wrapped).resolves.toBe('accepted')
    } finally {
      vi.useRealTimers()
    }
  })

  it('passes through the original rejection (same reference) when it settles in time', async () => {
    vi.useFakeTimers()
    try {
      const originalError = new Error('host rejected the turn')
      let rejectOriginal: (error: Error) => void = () => {}
      const original = new Promise<string>((_resolve, reject) => {
        rejectOriginal = reject
      })

      const wrapped = withTimeout(original, START_TURN_TIMEOUT_MS)
      rejectOriginal(originalError)

      await expect(wrapped).rejects.toBe(originalError)
    } finally {
      vi.useRealTimers()
    }
  })

  it('rejects with TurnAcceptanceTimeoutError when the promise never settles in time', async () => {
    vi.useFakeTimers()
    try {
      const original = new Promise<never>(() => {})
      const wrapped = withTimeout(original, START_TURN_TIMEOUT_MS)

      vi.advanceTimersByTime(START_TURN_TIMEOUT_MS)

      await expect(wrapped).rejects.toBeInstanceOf(TurnAcceptanceTimeoutError)
      await expect(wrapped).rejects.toMatchObject({
        name: 'TurnAcceptanceTimeoutError'
      })
    } finally {
      vi.useRealTimers()
    }
  })

  it('consumes a late rejection after timing out without emitting unhandledRejection', async () => {
    const unhandled: unknown[] = []
    const handler = (reason: unknown): void => {
      unhandled.push(reason)
    }

    // Fake only the timers under test so real setImmediate stays available to flush
    // microtasks/macrotasks before Node reports an unhandled rejection.
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    process.on('unhandledRejection', handler)

    const flushMicrotasks = async (): Promise<void> => {
      for (let i = 0; i < 5; i += 1) {
        await Promise.resolve()
        await new Promise<void>((resolve) => setImmediate(resolve))
      }
    }

    try {
      let rejectLate: (error: Error) => void = () => {}
      const original = new Promise<never>((_resolve, reject) => {
        rejectLate = reject
      })

      const wrapped = withTimeout(original, START_TURN_TIMEOUT_MS)
      const timedOut = expect(wrapped).rejects.toBeInstanceOf(
        TurnAcceptanceTimeoutError
      )

      vi.advanceTimersByTime(START_TURN_TIMEOUT_MS)
      await timedOut

      rejectLate(new Error('late rejection from the original promise'))
      await flushMicrotasks()

      expect(unhandled).toEqual([])
    } finally {
      process.off('unhandledRejection', handler)
      vi.useRealTimers()
    }
  })
})
