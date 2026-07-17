import { afterEach, describe, expect, it, vi } from 'vitest'
import { TerminalExitDrainController } from './TerminalExitDrainController'

afterEach(() => {
  vi.useRealTimers()
})

describe('TerminalExitDrainController', () => {
  it('waits for stream close and preserves data that arrives after process exit', () => {
    vi.useFakeTimers()
    const onDrainComplete = vi.fn()
    const controller = new TerminalExitDrainController({
      isOutputPaused: () => false,
      maxWaitMs: 500,
      onDrainComplete,
      quietPeriodMs: 20,
      waitForStreamClose: true
    })

    controller.begin({ exitCode: 0 })
    vi.advanceTimersByTime(100)
    controller.noteData()
    controller.noteStreamClosed()
    vi.advanceTimersByTime(19)
    expect(onDrainComplete).not.toHaveBeenCalled()

    vi.advanceTimersByTime(1)
    expect(onDrainComplete).toHaveBeenCalledOnce()
    expect(onDrainComplete).toHaveBeenCalledWith({ exitCode: 0 })
  })

  it('handles a PTY stream that closes before its process exit callback arrives', () => {
    vi.useFakeTimers()
    const onDrainComplete = vi.fn()
    const controller = new TerminalExitDrainController({
      isOutputPaused: () => false,
      maxWaitMs: 500,
      onDrainComplete,
      quietPeriodMs: 20,
      waitForStreamClose: true
    })

    controller.noteStreamClosed()
    controller.begin({ exitCode: 7 })
    vi.advanceTimersByTime(20)

    expect(onDrainComplete).toHaveBeenCalledWith({ exitCode: 7 })
  })

  it('does not finalize while output is paused and re-arms after output resumes', () => {
    vi.useFakeTimers()
    let outputPaused = true
    const onDrainComplete = vi.fn()
    const controller = new TerminalExitDrainController({
      isOutputPaused: () => outputPaused,
      maxWaitMs: 500,
      onDrainComplete,
      quietPeriodMs: 20,
      waitForStreamClose: true
    })

    controller.begin({ exitCode: 0 })
    controller.noteStreamClosed()
    vi.advanceTimersByTime(100)
    expect(onDrainComplete).not.toHaveBeenCalled()

    outputPaused = false
    controller.noteOutputResumed()
    controller.noteData()
    vi.advanceTimersByTime(20)
    expect(onDrainComplete).toHaveBeenCalledOnce()
  })

  it('uses a bounded fallback when a native PTY never reports stream close', () => {
    vi.useFakeTimers()
    const onDrainComplete = vi.fn()
    const controller = new TerminalExitDrainController({
      isOutputPaused: () => true,
      maxWaitMs: 100,
      onDrainComplete,
      quietPeriodMs: 20,
      waitForStreamClose: true
    })

    controller.begin({ exitCode: 0 })
    vi.advanceTimersByTime(99)
    expect(onDrainComplete).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    expect(onDrainComplete).toHaveBeenCalledOnce()
  })

  it('finalizes after a quiet period when stream-close observation is unavailable', () => {
    vi.useFakeTimers()
    const onDrainComplete = vi.fn()
    const controller = new TerminalExitDrainController({
      isOutputPaused: () => false,
      maxWaitMs: 500,
      onDrainComplete,
      quietPeriodMs: 20,
      waitForStreamClose: false
    })

    controller.begin({ exitCode: 0 })
    controller.noteData()
    vi.advanceTimersByTime(20)

    expect(onDrainComplete).toHaveBeenCalledOnce()
  })
})
