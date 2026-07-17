import { afterEach, describe, expect, it, vi } from 'vitest'
import { TerminalOutputFlowController } from './TerminalOutputFlowController'

afterEach(() => {
  vi.useRealTimers()
})

describe('TerminalOutputFlowController', () => {
  it('batches output by time while preserving exact order', () => {
    vi.useFakeTimers()
    const batches: Array<{ data: string; sequence: number; sessionId: string }> = []
    const controller = new TerminalOutputFlowController({
      batchDelayMs: 12,
      onBatch: (event) => batches.push(event),
      onPause: vi.fn(),
      onResume: vi.fn(),
      sessionId: 'session-1'
    })

    controller.push('first ')
    controller.push('second')
    vi.advanceTimersByTime(11)
    expect(batches).toEqual([])
    vi.advanceTimersByTime(1)

    expect(batches).toEqual([{ data: 'first second', sequence: 1, sessionId: 'session-1' }])
    expect(controller.finalOutputSequence).toBe(1)
  })

  it('flushes immediately at the byte threshold without changing data', () => {
    vi.useFakeTimers()
    const batches: Array<{ data: string; sequence: number; sessionId: string }> = []
    const controller = new TerminalOutputFlowController({
      batchSizeBytes: 5,
      onBatch: (event) => batches.push(event),
      onPause: vi.fn(),
      onResume: vi.fn(),
      sessionId: 'session-1'
    })

    controller.push('你')
    controller.push('好')

    expect(batches).toEqual([{ data: '你好', sequence: 1, sessionId: 'session-1' }])
  })

  it('pauses at the high watermark and resumes only after cumulative ACK reaches low water', () => {
    const onPause = vi.fn()
    const onResume = vi.fn()
    const controller = new TerminalOutputFlowController({
      batchSizeBytes: 1,
      highWaterBytes: 5,
      lowWaterBytes: 2,
      onBatch: vi.fn(),
      onPause,
      onResume,
      sessionId: 'session-1'
    })

    controller.push('abc')
    controller.push('de')
    expect(onPause).toHaveBeenCalledTimes(1)

    controller.acknowledge(99)
    expect(onResume).not.toHaveBeenCalled()
    controller.acknowledge(1)
    expect(onResume).toHaveBeenCalledTimes(1)
  })

  it('cancels delayed output after disposal', () => {
    vi.useFakeTimers()
    const onBatch = vi.fn()
    const controller = new TerminalOutputFlowController({
      onBatch,
      onPause: vi.fn(),
      onResume: vi.fn(),
      sessionId: 'session-1'
    })

    controller.push('pending')
    controller.dispose()
    vi.runAllTimers()

    expect(onBatch).not.toHaveBeenCalled()
  })
})
