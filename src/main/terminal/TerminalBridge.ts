import { BrowserWindow, utilityProcess } from 'electron'
import type { UtilityProcess } from 'electron'
import { join } from 'node:path'

import type {
  TerminalCreateSessionRequest,
  TerminalExitEvent,
  TerminalOutputEvent,
  TerminalSessionSnapshot
} from '@mycopilot/protocol'

type TerminalServiceRequest =
  | {
      id: number
      method: 'terminal.createSession'
      params: TerminalCreateSessionRequest
    }
  | {
      id: number
      method: 'terminal.writeInput'
      params: {
        data: string
        sessionId: string
      }
    }
  | {
      id: number
      method: 'terminal.resizeSession'
      params: {
        cols: number
        rows: number
        sessionId: string
      }
    }
  | {
      id: number
      method: 'terminal.killSession'
      params: {
        sessionId: string
      }
    }
  | {
      id: number
      method: 'terminal.shutdown'
    }

type TerminalServiceMessage =
  | {
      error?: string
      id: number
      result?: unknown
      success: boolean
      type: 'response'
    }
  | {
      event: TerminalOutputEvent
      method: 'terminal.output'
      type: 'notification'
    }
  | {
      event: TerminalExitEvent
      method: 'terminal.exit'
      type: 'notification'
    }

type PendingRequest = {
  reject: (error: Error) => void
  resolve: (value: unknown) => void
}

const TERMINAL_OUTPUT_CHANNEL = 'host:terminal.output'
const TERMINAL_EXIT_CHANNEL = 'host:terminal.exit'

export class TerminalBridge {
  private child: UtilityProcess | null = null
  private nextRequestId = 1
  private readonly pendingRequests = new Map<number, PendingRequest>()

  createSession(request: TerminalCreateSessionRequest): Promise<TerminalSessionSnapshot> {
    return this.sendRequest<TerminalSessionSnapshot>({
      id: this.createRequestId(),
      method: 'terminal.createSession',
      params: request
    })
  }

  writeInput(sessionId: string, data: string): Promise<void> {
    return this.sendRequest<void>({
      id: this.createRequestId(),
      method: 'terminal.writeInput',
      params: { data, sessionId }
    })
  }

  resizeSession(sessionId: string, cols: number, rows: number): Promise<void> {
    return this.sendRequest<void>({
      id: this.createRequestId(),
      method: 'terminal.resizeSession',
      params: { cols, rows, sessionId }
    })
  }

  killSession(sessionId: string): Promise<boolean> {
    return this.sendRequest<boolean>({
      id: this.createRequestId(),
      method: 'terminal.killSession',
      params: { sessionId }
    })
  }

  async stop(): Promise<void> {
    const child = this.child
    if (!child) return

    const timeout = new Promise<void>((resolve) => {
      setTimeout(() => {
        if (this.child === child) {
          this.child = null
          child.kill()
        }
        resolve()
      }, 1000).unref()
    })

    const shutdown = this.sendRequest<boolean>({
      id: this.createRequestId(),
      method: 'terminal.shutdown'
    }).then(
      () => undefined,
      (error) => {
        console.warn('Failed to request terminal service shutdown', error)
      }
    )

    await Promise.race([shutdown, timeout])

    if (this.child === child) {
      this.child = null
      child.kill()
    }
  }

  killNow(): void {
    const child = this.child
    if (!child) return

    this.child = null
    child.kill()
  }

  private createRequestId(): number {
    const requestId = this.nextRequestId
    this.nextRequestId += 1
    return requestId
  }

  private sendRequest<T>(request: TerminalServiceRequest): Promise<T> {
    const child = this.ensureStarted()

    return new Promise<T>((resolve, reject) => {
      this.pendingRequests.set(request.id, {
        reject,
        resolve: (value) => resolve(value as T)
      })

      try {
        child.postMessage(request)
      } catch (error) {
        this.pendingRequests.delete(request.id)
        reject(error instanceof Error ? error : new Error(String(error)))
      }
    })
  }

  private ensureStarted(): UtilityProcess {
    if (this.child?.pid) {
      return this.child
    }

    const servicePath = join(__dirname, 'terminal-service.js')
    const child = utilityProcess.fork(servicePath, [], {
      serviceName: 'MyCopilot Terminal Service',
      stdio: 'pipe'
    })

    child.on('message', (message) => this.handleServiceMessage(message as TerminalServiceMessage))
    child.on('exit', (code) => {
      if (this.child === child) {
        this.child = null
      }

      const error = new Error(`Terminal service exited with code ${code}`)
      for (const pendingRequest of this.pendingRequests.values()) {
        pendingRequest.reject(error)
      }
      this.pendingRequests.clear()
    })
    child.on('error', (_type, location, report) => {
      console.error('Terminal service error', { location, report })
    })
    child.stderr?.on('data', (chunk) => {
      console.warn(`[terminal-service] ${String(chunk).trimEnd()}`)
    })

    this.child = child
    return child
  }

  private handleServiceMessage(message: TerminalServiceMessage): void {
    if (message.type === 'response') {
      this.resolvePendingRequest(message)
      return
    }

    if (message.method === 'terminal.output') {
      this.broadcast(TERMINAL_OUTPUT_CHANNEL, message.event)
      return
    }

    this.broadcast(TERMINAL_EXIT_CHANNEL, message.event)
  }

  private resolvePendingRequest(
    response: Extract<TerminalServiceMessage, { type: 'response' }>
  ): void {
    const pendingRequest = this.pendingRequests.get(response.id)
    if (!pendingRequest) return

    this.pendingRequests.delete(response.id)

    if (!response.success) {
      pendingRequest.reject(new Error(response.error || 'Terminal service request failed'))
      return
    }

    pendingRequest.resolve(response.result)
  }

  private broadcast(channel: string, payload: TerminalOutputEvent | TerminalExitEvent): void {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed() && !window.webContents.isDestroyed()) {
        window.webContents.send(channel, payload)
      }
    }
  }
}
