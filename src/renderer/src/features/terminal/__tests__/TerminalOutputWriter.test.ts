import { describe, expect, it, vi } from 'vitest'
import { TerminalOutputWriter } from '../TerminalOutputWriter'

describe('TerminalOutputWriter', () => {
  it('reorders batches, serializes xterm writes, and waits before finishing', () => {
    const acknowledgements: number[] = []
    const callbacks: Array<() => void> = []
    const writes: string[] = []
    const onReady = vi.fn()
    const writer = new TerminalOutputWriter({
      acknowledge: (sequence) => acknowledgements.push(sequence),
      onProtocolError: vi.fn(),
      sessionId: 'session',
      write: (data, callback) => {
        writes.push(data)
        callbacks.push(callback)
      }
    })

    writer.accept({ data: 'second', sequence: 2, sessionId: 'session' })
    writer.accept({ data: 'first', sequence: 1, sessionId: 'session' })
    writer.finish(2, onReady)

    expect(writes).toEqual(['first'])
    expect(onReady).not.toHaveBeenCalled()
    callbacks.shift()?.()
    expect(acknowledgements).toEqual([1])
    expect(writes).toEqual(['first', 'second'])
    callbacks.shift()?.()
    expect(acknowledgements).toEqual([1, 2])
    expect(onReady).toHaveBeenCalledOnce()
  })

  it('does not display duplicate batches and safely repeats their cumulative ACK', () => {
    const acknowledge = vi.fn()
    const callbacks: Array<() => void> = []
    const write = vi.fn((_data: string, callback: () => void) => callbacks.push(callback))
    const writer = new TerminalOutputWriter({
      acknowledge,
      onProtocolError: vi.fn(),
      sessionId: 'session',
      write
    })

    writer.accept({ data: 'first', sequence: 1, sessionId: 'session' })
    writer.accept({ data: 'second', sequence: 2, sessionId: 'session' })
    callbacks.shift()?.()
    callbacks.shift()?.()
    writer.accept({ data: 'duplicate', sequence: 1, sessionId: 'session' })

    expect(write).toHaveBeenCalledTimes(2)
    expect(acknowledge).toHaveBeenNthCalledWith(1, 1)
    expect(acknowledge).toHaveBeenNthCalledWith(2, 2)
    expect(acknowledge).toHaveBeenNthCalledWith(3, 2)
  })

  it('drops a duplicate of the batch currently being parsed without retaining it', () => {
    const acknowledge = vi.fn()
    const callbacks: Array<() => void> = []
    const write = vi.fn((_data: string, callback: () => void) => callbacks.push(callback))
    const onReady = vi.fn()
    const writer = new TerminalOutputWriter({
      acknowledge,
      onProtocolError: vi.fn(),
      sessionId: 'session',
      write
    })

    writer.accept({ data: 'first', sequence: 1, sessionId: 'session' })
    writer.accept({ data: 'duplicate', sequence: 1, sessionId: 'session' })
    writer.finish(1, onReady)
    callbacks.shift()?.()

    expect(write).toHaveBeenCalledOnce()
    expect(acknowledge).toHaveBeenCalledOnce()
    expect(onReady).toHaveBeenCalledOnce()
  })

  it('rejects invalid final sequences rather than displaying an out-of-order exit', () => {
    const onProtocolError = vi.fn()
    const writer = new TerminalOutputWriter({
      acknowledge: vi.fn(),
      onProtocolError,
      sessionId: 'session',
      write: vi.fn()
    })

    writer.finish(-1, vi.fn())

    expect(onProtocolError).toHaveBeenCalledOnce()
  })

  it('rejects buffered output beyond the final sequence', () => {
    const onProtocolError = vi.fn()
    const writer = new TerminalOutputWriter({
      acknowledge: vi.fn(),
      onProtocolError,
      sessionId: 'session',
      write: vi.fn()
    })

    writer.accept({ data: 'future', sequence: 2, sessionId: 'session' })
    writer.finish(1, vi.fn())

    expect(onProtocolError).toHaveBeenCalledOnce()
  })

  it('fails fast when an exit proves that an output sequence is missing', () => {
    const onProtocolError = vi.fn()
    const onReady = vi.fn()
    const writer = new TerminalOutputWriter({
      acknowledge: vi.fn(),
      onProtocolError,
      sessionId: 'session',
      write: vi.fn()
    })

    writer.accept({ data: 'second', sequence: 2, sessionId: 'session' })
    writer.finish(2, onReady)

    expect(onProtocolError).toHaveBeenCalledOnce()
    expect(onReady).not.toHaveBeenCalled()
  })

  it('rejects duplicate exits even after the first exit is ready', () => {
    const onProtocolError = vi.fn()
    const onReady = vi.fn()
    const writer = new TerminalOutputWriter({
      acknowledge: vi.fn(),
      onProtocolError,
      sessionId: 'session',
      write: vi.fn()
    })

    writer.finish(0, onReady)
    writer.finish(0, vi.fn())

    expect(onReady).toHaveBeenCalledOnce()
    expect(onProtocolError).toHaveBeenCalledOnce()
  })
})
