import type { WebContents } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import type { TrustedIpcMain } from './trustedIpc'

export const DEFAULT_RENDERER_QUIT_FLUSH_TIMEOUT_MS = 1_500

interface PendingRendererFlush {
  onTargetDestroyed: () => void
  resolve: () => void
  target: WebContents
  timer: ReturnType<typeof setTimeout>
}

/**
 * Gives the exact trusted Renderer being closed a bounded chance to finish durable writes before
 * Main shuts Core down. The timeout is deliberately fail-open so a crashed Renderer cannot block
 * application exit.
 */
export class RendererQuitFlushCoordinator {
  private readonly pending = new Map<string, PendingRendererFlush>()
  private nextRequestId = 0
  private disposed = false

  constructor(ipcMain: TrustedIpcMain) {
    ipcMain.on(HOST_CHANNELS.app.flushBeforeQuitAck, (event, requestId) => {
      if (typeof requestId !== 'string') return
      const pending = this.pending.get(requestId)
      if (!pending || pending.target !== event.sender) return
      this.settle(requestId)
    })
  }

  flush(
    target: WebContents | null | undefined,
    timeoutMs = DEFAULT_RENDERER_QUIT_FLUSH_TIMEOUT_MS
  ): Promise<void> {
    if (this.disposed || !target || target.isDestroyed()) return Promise.resolve()

    const requestId = `renderer-quit-flush:${++this.nextRequestId}`
    return new Promise((resolve) => {
      const onTargetDestroyed = (): void => this.settle(requestId)
      const timer = setTimeout(
        () => {
          console.warn('Timed out waiting for Renderer state to flush before quit')
          this.settle(requestId)
        },
        Math.max(0, timeoutMs)
      )
      this.pending.set(requestId, { onTargetDestroyed, resolve, target, timer })
      target.once('destroyed', onTargetDestroyed)

      try {
        target.send(HOST_CHANNELS.app.flushBeforeQuit, requestId)
      } catch {
        this.settle(requestId)
      }
    })
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    for (const requestId of [...this.pending.keys()]) this.settle(requestId)
  }

  private settle(requestId: string): void {
    const pending = this.pending.get(requestId)
    if (!pending) return
    this.pending.delete(requestId)
    clearTimeout(pending.timer)
    pending.target.removeListener('destroyed', pending.onTargetDestroyed)
    pending.resolve()
  }
}
