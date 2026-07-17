import type { TerminalOutputEvent } from './terminalTypes'

interface TerminalOutputWriterOptions {
  acknowledge: (sequence: number) => void
  maxReorderBuffer?: number
  onProtocolError: (error: Error) => void
  sessionId: string
  write: (data: string, callback: () => void) => void
}

interface FinishRequest {
  finalSequence: number
  onReady: () => void
}

const DEFAULT_MAX_REORDER_BUFFER = 256

/** Serializes output through xterm and ACKs only after its parser completes each batch. */
export class TerminalOutputWriter {
  private readonly acknowledge: (sequence: number) => void
  private disposed = false
  private failed = false
  private finalSequence: number | null = null
  private finishRequest: FinishRequest | null = null
  private readonly maxReorderBuffer: number
  private nextSequence = 1
  private readonly onProtocolError: (error: Error) => void
  private readonly pending = new Map<number, TerminalOutputEvent>()
  private readonly sessionId: string
  private readonly write: (data: string, callback: () => void) => void
  private writing = false
  private writingSequence: number | null = null

  constructor(options: TerminalOutputWriterOptions) {
    this.acknowledge = options.acknowledge
    this.maxReorderBuffer = options.maxReorderBuffer ?? DEFAULT_MAX_REORDER_BUFFER
    this.onProtocolError = options.onProtocolError
    this.sessionId = options.sessionId
    this.write = options.write
  }

  accept(event: TerminalOutputEvent): void {
    if (this.disposed || this.failed) return
    if (
      event.sessionId !== this.sessionId ||
      !Number.isSafeInteger(event.sequence) ||
      event.sequence < 1
    ) {
      this.fail(new Error('Received invalid terminal output sequence'))
      return
    }
    if (this.finalSequence !== null && event.sequence > this.finalSequence) {
      this.fail(new Error('Received terminal output after the final output sequence'))
      return
    }
    if (event.sequence === this.writingSequence) {
      // The original batch is still being parsed; its completion will ACK both copies.
      return
    }
    if (event.sequence < this.nextSequence) {
      this.acknowledge(this.nextSequence - 1)
      return
    }
    if (this.pending.has(event.sequence)) return

    this.pending.set(event.sequence, event)
    if (this.pending.size > this.maxReorderBuffer) {
      this.fail(new Error('Terminal output reorder buffer exceeded its safety limit'))
      return
    }
    this.drain()
  }

  dispose(): void {
    this.disposed = true
    this.pending.clear()
    this.finishRequest = null
  }

  finish(finalSequence: number, onReady: () => void): void {
    if (this.disposed || this.failed) return
    if (this.finalSequence !== null) {
      this.fail(new Error('Received duplicate terminal exit event'))
      return
    }
    const hasInvalidSequence =
      !Number.isSafeInteger(finalSequence) ||
      finalSequence < this.nextSequence - 1 ||
      (this.writingSequence !== null && this.writingSequence > finalSequence) ||
      [...this.pending.keys()].some(
        (sequence) => sequence < this.nextSequence || sequence > finalSequence
      )
    const expectedOutstanding = finalSequence - this.nextSequence + 1
    const coveredOutstanding =
      this.pending.size + (this.writingSequence === this.nextSequence ? 1 : 0)
    if (hasInvalidSequence || expectedOutstanding !== coveredOutstanding) {
      this.fail(new Error('Received invalid final terminal output sequence'))
      return
    }

    this.finalSequence = finalSequence
    this.finishRequest = { finalSequence, onReady }
    this.drain()
  }

  private drain(): void {
    if (this.disposed || this.failed || this.writing) return
    if (this.finalSequence !== null && this.nextSequence > this.finalSequence) {
      this.finishIfReady()
      return
    }

    const event = this.pending.get(this.nextSequence)
    if (!event) {
      this.finishIfReady()
      return
    }

    this.pending.delete(this.nextSequence)
    this.writing = true
    this.writingSequence = event.sequence
    let completed = false
    const complete = () => {
      if (completed) return
      completed = true
      if (this.disposed || this.failed) return
      this.acknowledge(event.sequence)
      this.nextSequence += 1
      this.writing = false
      this.writingSequence = null
      this.drain()
    }

    try {
      this.write(event.data, complete)
    } catch (error) {
      if (completed) return
      this.writing = false
      this.writingSequence = null
      this.fail(error instanceof Error ? error : new Error(String(error)))
    }
  }

  private fail(error: Error): void {
    if (this.disposed || this.failed) return
    this.failed = true
    this.pending.clear()
    this.finishRequest = null
    this.onProtocolError(error)
  }

  private finishIfReady(): void {
    const finishRequest = this.finishRequest
    if (!finishRequest || this.nextSequence <= finishRequest.finalSequence) return
    this.finishRequest = null
    finishRequest.onReady()
  }
}
