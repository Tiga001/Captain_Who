import { describe, expect, it, vi } from 'vitest'
import { TerminalEventRouter } from './TerminalEventRouter'

describe('TerminalEventRouter', () => {
  it('routes events only to subscribers of the matching session', () => {
    const router = new TerminalEventRouter()
    const first = { onExit: vi.fn(), onOutput: vi.fn() }
    const second = { onExit: vi.fn(), onOutput: vi.fn() }
    router.subscribe('first', first)
    router.subscribe('second', second)

    router.dispatchOutput({ data: 'one', sequence: 1, sessionId: 'first' })

    expect(first.onOutput).toHaveBeenCalledOnce()
    expect(second.onOutput).not.toHaveBeenCalled()
  })

  it('removes all session subscribers on exit and keeps unsubscribe idempotent', () => {
    const router = new TerminalEventRouter()
    const handlers = { onExit: vi.fn(), onOutput: vi.fn() }
    const unsubscribe = router.subscribe('session', handlers)

    router.dispatchExit({
      exitCode: 0,
      finalOutputSequence: 1,
      sessionId: 'session',
      signal: null
    })
    router.dispatchOutput({ data: 'late', sequence: 2, sessionId: 'session' })
    unsubscribe()
    unsubscribe()

    expect(handlers.onExit).toHaveBeenCalledOnce()
    expect(handlers.onOutput).not.toHaveBeenCalled()
  })
})
