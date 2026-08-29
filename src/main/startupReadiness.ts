// Electron main process: hold the existing Renderer bootstrap shell until Host services are ready.

import type { IpcMainInvokeEvent } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'

type StartupReadinessState = 'pending' | 'ready' | 'failed' | 'disposed'

interface StartupIpcMain {
  handle(
    channel: string,
    listener: (event: IpcMainInvokeEvent, ...args: unknown[]) => unknown
  ): void
  removeHandler(channel: string): void
}

interface StartupWaiter {
  reject(error: Error): void
  resolve(): void
}

export interface StartupReadinessController {
  dispose(): void
  markFailed(): void
  markReady(): void
}

const STARTUP_FAILED_MESSAGE = 'MyCopilot application startup failed'
const STARTUP_DISPOSED_MESSAGE = 'MyCopilot application startup was cancelled'

export function registerStartupReadiness(
  ipcMain: StartupIpcMain,
  isTrustedRenderer: (event: IpcMainInvokeEvent) => boolean
): StartupReadinessController {
  let state: StartupReadinessState = 'pending'
  const waiters = new Set<StartupWaiter>()

  const settle = (nextState: Exclude<StartupReadinessState, 'pending'>): void => {
    if (state !== 'pending') return
    state = nextState
    const error =
      nextState === 'ready'
        ? null
        : new Error(nextState === 'failed' ? STARTUP_FAILED_MESSAGE : STARTUP_DISPOSED_MESSAGE)
    for (const waiter of waiters) {
      if (error) waiter.reject(error)
      else waiter.resolve()
    }
    waiters.clear()
  }

  ipcMain.handle(HOST_CHANNELS.app.whenReady, (event) => {
    if (!isTrustedRenderer(event)) {
      throw new Error('Blocked untrusted application startup readiness request')
    }
    if (state === 'ready') return undefined
    if (state === 'failed') throw new Error(STARTUP_FAILED_MESSAGE)
    if (state === 'disposed') throw new Error(STARTUP_DISPOSED_MESSAGE)

    return new Promise<void>((resolve, reject) => {
      waiters.add({ reject, resolve })
    })
  })

  return {
    dispose(): void {
      settle('disposed')
      ipcMain.removeHandler(HOST_CHANNELS.app.whenReady)
    },
    markFailed(): void {
      settle('failed')
    },
    markReady(): void {
      settle('ready')
    }
  }
}
