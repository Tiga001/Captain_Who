import { Buffer } from 'node:buffer'

import type { TerminalOutputEvent } from '@mycopilot/protocol'

export const TERMINAL_OUTPUT_BATCH_DELAY_MS = 12
export const TERMINAL_OUTPUT_BATCH_BYTES = 64 * 1024
export const TERMINAL_OUTPUT_HIGH_WATER_BYTES = 1024 * 1024
export const TERMINAL_OUTPUT_LOW_WATER_BYTES = 256 * 1024

interface TerminalOutputFlowControllerOptions {
  batchDelayMs?: number
  batchSizeBytes?: number
  highWaterBytes?: number
  lowWaterBytes?: number
  onBatch: (event: TerminalOutputEvent) => void
  onPause: () => void
  onResume: () => void
  sessionId: string
}

interface InFlightBatch {
  byteLength: number
  sequence: number
}

/**
 * Batches PTY output and bounds the amount delivered but not yet parsed by xterm.
 * ACKs are cumulative because both transport channels preserve session ordering.
 */
export class TerminalOutputFlowController {
  private readonly batchDelayMs: number
  private readonly batchSizeBytes: number
  private disposed = false
  private flushTimer: ReturnType<typeof setTimeout> | null = null
  private readonly highWaterBytes: number
  private inFlightBytes = 0
  private readonly inFlightBatches: InFlightBatch[] = []
  private lastAcknowledgedSequence = 0
  private lastEmittedSequence = 0
  private readonly lowWaterBytes: number
  private readonly onBatch: (event: TerminalOutputEvent) => void
  private readonly onPause: () => void
  private readonly onResume: () => void
  private paused = false
  private pendingByteLength = 0
  private readonly pendingChunks: string[] = []
  private readonly sessionId: string

  constructor(options: TerminalOutputFlowControllerOptions) {
    this.batchDelayMs = options.batchDelayMs ?? TERMINAL_OUTPUT_BATCH_DELAY_MS
    this.batchSizeBytes = options.batchSizeBytes ?? TERMINAL_OUTPUT_BATCH_BYTES
    this.highWaterBytes = options.highWaterBytes ?? TERMINAL_OUTPUT_HIGH_WATER_BYTES
    this.lowWaterBytes = options.lowWaterBytes ?? TERMINAL_OUTPUT_LOW_WATER_BYTES
    this.onBatch = options.onBatch
    this.onPause = options.onPause
    this.onResume = options.onResume
    this.sessionId = options.sessionId

    if (this.batchDelayMs < 0) throw new Error('Terminal output batch delay cannot be negative')
    if (this.batchSizeBytes < 1) throw new Error('Terminal output batch size must be positive')
    if (this.lowWaterBytes < 0 || this.highWaterBytes < 1) {
      throw new Error('Terminal output watermarks must be positive')
    }
    if (this.lowWaterBytes >= this.highWaterBytes) {
      throw new Error('Terminal output low watermark must be below the high watermark')
    }
  }

  get finalOutputSequence(): number {
    return this.lastEmittedSequence
  }

  get isPaused(): boolean {
    return this.paused
  }

  acknowledge(sequence: number): void {
    if (
      this.disposed ||
      !Number.isSafeInteger(sequence) ||
      sequence <= this.lastAcknowledgedSequence ||
      sequence > this.lastEmittedSequence
    ) {
      return
    }

    this.lastAcknowledgedSequence = sequence
    while (this.inFlightBatches[0]?.sequence <= sequence) {
      const batch = this.inFlightBatches.shift()
      if (!batch) break
      this.inFlightBytes = Math.max(0, this.inFlightBytes - batch.byteLength)
    }

    if (this.paused && this.inFlightBytes <= this.lowWaterBytes) {
      this.paused = false
      this.onResume()
    }
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    this.clearFlushTimer()
    this.pendingChunks.length = 0
    this.pendingByteLength = 0
    this.inFlightBatches.length = 0
    this.inFlightBytes = 0
  }

  flush(): void {
    if (this.disposed || this.pendingChunks.length === 0) return
    this.clearFlushTimer()

    const data = this.pendingChunks.join('')
    const byteLength = this.pendingByteLength
    this.pendingChunks.length = 0
    this.pendingByteLength = 0

    const sequence = this.lastEmittedSequence + 1
    this.lastEmittedSequence = sequence
    this.inFlightBatches.push({ byteLength, sequence })
    this.inFlightBytes += byteLength
    this.onBatch({ data, sequence, sessionId: this.sessionId })

    if (!this.paused && this.inFlightBytes >= this.highWaterBytes) {
      this.paused = true
      this.onPause()
    }
  }

  push(data: string): void {
    if (this.disposed || data.length === 0) return

    this.pendingChunks.push(data)
    this.pendingByteLength += Buffer.byteLength(data)
    if (this.pendingByteLength >= this.batchSizeBytes) {
      this.flush()
      return
    }

    if (this.flushTimer === null) {
      this.flushTimer = setTimeout(() => {
        this.flushTimer = null
        this.flush()
      }, this.batchDelayMs)
      this.flushTimer.unref?.()
    }
  }

  private clearFlushTimer(): void {
    if (this.flushTimer === null) return
    clearTimeout(this.flushTimer)
    this.flushTimer = null
  }
}
