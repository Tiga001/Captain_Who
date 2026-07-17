import { randomUUID } from 'node:crypto'
import { join } from 'node:path'
import { utilityProcess } from 'electron'
import type {
  Event as ElectronEvent,
  UtilityProcess,
  WebContents,
  WebContentsDidStartNavigationEventParams
} from 'electron'

import type {
  TerminalCreateSessionRequest,
  TerminalExitEvent,
  TerminalOutputEvent,
  TerminalSessionSnapshot
} from '@mycopilot/protocol'
import type {
  TerminalServiceCommand,
  TerminalServiceOutboundMessage,
  TerminalServiceRequest,
  TerminalServiceResponse
} from './terminalTransportProtocol'

interface ChildState {
  generation: number
  process: UtilityProcess
}

interface OwnerRecord {
  contents: WebContents
  onDestroyed: () => void
  onMainFrameNavigation: (event: ElectronEvent<WebContentsDidStartNavigationEventParams>) => void
  onRenderProcessGone: () => void
  sessionIds: Set<string>
}

interface OwnedSession {
  childGeneration: number
  lastOutputSequence: number
  ownerId: number
  startRequestId?: number
}

interface PendingRequest {
  childGeneration: number
  reject: (error: Error) => void
  resolve: (value: unknown) => void
  timer: ReturnType<typeof setTimeout>
}

export interface TerminalBridgeOptions {
  forkUtility?: () => UtilityProcess
  requestTimeoutMs?: number
}

const TERMINAL_OUTPUT_CHANNEL = 'host:terminal.output'
const TERMINAL_EXIT_CHANNEL = 'host:terminal.exit'
const DEFAULT_REQUEST_TIMEOUT_MS = 5000
const SHUTDOWN_TIMEOUT_MS = 1000
const SESSION_ID_PATTERN = /^[A-Za-z0-9._:-]{1,160}$/

export class TerminalBridge {
  private child: ChildState | null = null
  private nextChildGeneration = 1
  private nextRequestId = 1
  private readonly owners = new Map<number, OwnerRecord>()
  private readonly pendingRequests = new Map<number, PendingRequest>()
  private readonly sessions = new Map<string, OwnedSession>()
  private readonly forkUtility: () => UtilityProcess
  private readonly requestTimeoutMs: number

  constructor(options: TerminalBridgeOptions = {}) {
    this.forkUtility = options.forkUtility ?? forkTerminalUtility
    this.requestTimeoutMs = options.requestTimeoutMs ?? DEFAULT_REQUEST_TIMEOUT_MS
  }

  acknowledgeOutput(owner: WebContents, sessionId: string, sequence: number): void {
    const session = this.getOwnedSession(owner, sessionId)
    const child = session ? this.getChildForSession(session) : null
    if (!child || !Number.isSafeInteger(sequence) || sequence < 1) return

    this.sendCommand(child, {
      method: 'terminal.acknowledgeOutput',
      params: { sequence, sessionId },
      type: 'command'
    })
  }

  async createSession(
    owner: WebContents,
    request: TerminalCreateSessionRequest
  ): Promise<TerminalSessionSnapshot> {
    const sessionId = normalizeSessionId(request.sessionId)
    if (this.sessions.has(sessionId)) {
      throw new Error(`Terminal session already exists: ${sessionId}`)
    }
    if (owner.isDestroyed()) throw new Error('Terminal owner is already destroyed')

    const child = this.ensureStarted()
    const ownerRecord = this.ensureOwner(owner)
    const startRequestId = this.createRequestId()
    this.sessions.set(sessionId, {
      childGeneration: child.generation,
      lastOutputSequence: 0,
      ownerId: owner.id,
      startRequestId
    })
    ownerRecord.sessionIds.add(sessionId)

    try {
      const snapshot = await this.sendRequest<TerminalSessionSnapshot>(
        child,
        {
          id: startRequestId,
          method: 'terminal.createSession',
          params: { ...request, sessionId },
          type: 'request'
        },
        this.requestTimeoutMs
      )
      const currentSession = this.sessions.get(sessionId)
      if (
        !currentSession ||
        currentSession.ownerId !== owner.id ||
        currentSession.childGeneration !== child.generation
      ) {
        throw new Error('Terminal owner disappeared while the session was starting')
      }
      delete currentSession.startRequestId
      return snapshot
    } catch (error) {
      const removedSession = this.removeSessionOwnership(sessionId)
      if (removedSession && this.getChildForSession(removedSession)) {
        this.sendDisposeCommand(child, sessionId)
      }
      throw error
    }
  }

  async killSession(owner: WebContents, sessionId: string): Promise<boolean> {
    const session = this.getOwnedSession(owner, sessionId)
    const child = session ? this.getChildForSession(session) : null
    if (!session) return false
    if (!child) {
      this.removeSessionOwnership(sessionId)
      return false
    }

    try {
      return await this.sendRequest<boolean>(
        child,
        {
          id: this.createRequestId(),
          method: 'terminal.killSession',
          params: { sessionId },
          type: 'request'
        },
        this.requestTimeoutMs
      )
    } finally {
      this.removeSessionOwnership(sessionId)
    }
  }

  resizeSession(owner: WebContents, sessionId: string, cols: number, rows: number): Promise<void> {
    const session = this.requireOwnedSession(owner, sessionId)
    const child = this.requireChildForSession(session)
    return this.sendRequest<void>(
      child,
      {
        id: this.createRequestId(),
        method: 'terminal.resizeSession',
        params: { cols, rows, sessionId },
        type: 'request'
      },
      this.requestTimeoutMs
    )
  }

  writeInput(owner: WebContents, sessionId: string, data: string): void {
    const session = this.getOwnedSession(owner, sessionId)
    const child = session ? this.getChildForSession(session) : null
    if (!child || typeof data !== 'string' || data.length === 0) return

    this.sendCommand(child, {
      method: 'terminal.writeInput',
      params: { data, sessionId },
      type: 'command'
    })
  }

  async stop(): Promise<void> {
    const child = this.child
    if (!child) return

    try {
      await this.sendRequest<boolean>(
        child,
        {
          id: this.createRequestId(),
          method: 'terminal.shutdown',
          type: 'request'
        },
        SHUTDOWN_TIMEOUT_MS
      )
    } catch (error) {
      console.warn('Failed to request terminal service shutdown', error)
    }

    if (this.child === child) {
      this.failChild(child, new Error('Terminal service stopped'), true)
    }
  }

  killNow(): void {
    const child = this.child
    if (!child) return
    this.failChild(child, new Error('Terminal service terminated'), true)
  }

  private createRequestId(): number {
    const requestId = this.nextRequestId
    this.nextRequestId += 1
    return requestId
  }

  private detachOwner(ownerId: number): void {
    const owner = this.owners.get(ownerId)
    if (!owner) return
    this.owners.delete(ownerId)

    try {
      owner.contents.off('destroyed', owner.onDestroyed)
      owner.contents.off('did-start-navigation', owner.onMainFrameNavigation)
      owner.contents.off('render-process-gone', owner.onRenderProcessGone)
    } catch {
      // A destroyed WebContents can reject listener operations; ownership is already removed.
    }
  }

  private ensureOwner(contents: WebContents): OwnerRecord {
    const existing = this.owners.get(contents.id)
    if (existing?.contents === contents) return existing
    if (existing) this.releaseOwner(existing.contents.id)

    const ownerId = contents.id
    const onDestroyed = () => this.releaseOwner(ownerId)
    const onMainFrameNavigation = (
      event: ElectronEvent<WebContentsDidStartNavigationEventParams>
    ) => {
      if (event.isMainFrame && !event.isSameDocument) this.releaseOwner(ownerId)
    }
    const onRenderProcessGone = () => this.releaseOwner(ownerId)
    const owner: OwnerRecord = {
      contents,
      onDestroyed,
      onMainFrameNavigation,
      onRenderProcessGone,
      sessionIds: new Set()
    }
    this.owners.set(ownerId, owner)
    contents.on('destroyed', onDestroyed)
    contents.on('did-start-navigation', onMainFrameNavigation)
    contents.on('render-process-gone', onRenderProcessGone)
    return owner
  }

  private ensureStarted(): ChildState {
    // `UtilityProcess.pid` stays undefined until the asynchronous spawn event. The
    // ChildState itself is the ownership token during that startup window.
    if (this.child) return this.child

    const child: ChildState = {
      generation: this.nextChildGeneration,
      process: this.forkUtility()
    }
    this.nextChildGeneration += 1

    child.process.on('message', (message) => {
      this.handleServiceMessage(child, message as TerminalServiceOutboundMessage)
    })
    child.process.on('exit', (code) => {
      this.failChild(child, new Error(`Terminal service exited with code ${code}`), false)
    })
    child.process.on('error', (type, location) => {
      const error = new Error(`Terminal service ${type} at ${location}`)
      // Electron guarantees a later exit event, but fail requests and sessions now.
      // Do not log the diagnostic report: it may contain process environment data.
      console.error(error.message)
      this.failChild(child, error, false)
    })
    child.process.stderr?.on('data', (chunk) => {
      console.warn(`[terminal-service] ${String(chunk).trimEnd()}`)
    })

    this.child = child
    return child
  }

  private failChild(child: ChildState, error: Error, kill: boolean): void {
    if (this.child === child) this.child = null

    for (const [requestId, pending] of this.pendingRequests) {
      if (pending.childGeneration !== child.generation) continue
      clearTimeout(pending.timer)
      this.pendingRequests.delete(requestId)
      pending.reject(error)
    }

    for (const [sessionId, session] of [...this.sessions]) {
      if (session.childGeneration !== child.generation) continue
      this.sendSessionEvent(session, TERMINAL_EXIT_CHANNEL, {
        exitCode: null,
        finalOutputSequence: session.lastOutputSequence,
        sessionId,
        signal: 'terminal-service-exit'
      })
      this.removeSessionOwnership(sessionId)
    }

    if (kill) {
      try {
        child.process.kill()
      } catch {
        // The process may already be gone.
      }
    }
  }

  private getChildForSession(session: OwnedSession): ChildState | null {
    return this.child?.generation === session.childGeneration ? this.child : null
  }

  private getOwnedSession(owner: WebContents, sessionId: string): OwnedSession | null {
    if (!SESSION_ID_PATTERN.test(sessionId)) return null
    const session = this.sessions.get(sessionId)
    const ownerRecord = this.owners.get(owner.id)
    return session?.ownerId === owner.id && ownerRecord?.contents === owner ? session : null
  }

  private handleServiceMessage(child: ChildState, message: TerminalServiceOutboundMessage): void {
    if (message.type === 'response') {
      this.resolvePendingRequest(child, message)
      return
    }

    const session = this.sessions.get(message.event.sessionId)
    if (!session || session.childGeneration !== child.generation) return

    if (message.method === 'terminal.output') {
      if (message.event.sequence > session.lastOutputSequence) {
        session.lastOutputSequence = message.event.sequence
      }
      this.sendSessionEvent(session, TERMINAL_OUTPUT_CHANNEL, message.event)
      return
    }

    session.lastOutputSequence = Math.max(
      session.lastOutputSequence,
      message.event.finalOutputSequence
    )
    this.sendSessionEvent(session, TERMINAL_EXIT_CHANNEL, message.event)
    this.removeSessionOwnership(message.event.sessionId)
  }

  private releaseOwner(ownerId: number): void {
    const owner = this.owners.get(ownerId)
    if (!owner) return

    const sessionIds = [...owner.sessionIds]
    this.detachOwner(ownerId)
    for (const sessionId of sessionIds) {
      const session = this.sessions.get(sessionId)
      const child = session ? this.getChildForSession(session) : null
      this.sessions.delete(sessionId)
      if (session?.startRequestId) {
        this.rejectPendingRequest(
          session.startRequestId,
          new Error('Terminal owner disappeared while the session was starting')
        )
      }
      if (child) this.sendDisposeCommand(child, sessionId)
    }
  }

  private removeSessionOwnership(sessionId: string): OwnedSession | null {
    const session = this.sessions.get(sessionId)
    if (!session) return null
    this.sessions.delete(sessionId)

    const owner = this.owners.get(session.ownerId)
    owner?.sessionIds.delete(sessionId)
    if (owner?.sessionIds.size === 0) this.detachOwner(owner.contents.id)
    return session
  }

  private rejectPendingRequest(requestId: number, error: Error): boolean {
    const pending = this.pendingRequests.get(requestId)
    if (!pending) return false
    clearTimeout(pending.timer)
    this.pendingRequests.delete(requestId)
    pending.reject(error)
    return true
  }

  private requireChildForSession(session: OwnedSession): ChildState {
    const child = this.getChildForSession(session)
    if (!child) throw new Error('Terminal service is not available')
    return child
  }

  private requireOwnedSession(owner: WebContents, sessionId: string): OwnedSession {
    const session = this.getOwnedSession(owner, sessionId)
    if (!session) throw new Error(`Terminal session is not owned by renderer: ${sessionId}`)
    return session
  }

  private resolvePendingRequest(child: ChildState, response: TerminalServiceResponse): void {
    const pending = this.pendingRequests.get(response.id)
    if (!pending || pending.childGeneration !== child.generation) return

    clearTimeout(pending.timer)
    this.pendingRequests.delete(response.id)
    if (!response.success) {
      pending.reject(new Error(response.error || 'Terminal service request failed'))
      return
    }
    pending.resolve(response.result)
  }

  private sendCommand(child: ChildState, command: TerminalServiceCommand): boolean {
    try {
      child.process.postMessage(command)
      return true
    } catch (error) {
      this.failChild(child, error instanceof Error ? error : new Error(String(error)), true)
      return false
    }
  }

  private sendDisposeCommand(child: ChildState, sessionId: string): void {
    this.sendCommand(child, {
      method: 'terminal.disposeSession',
      params: { sessionId },
      type: 'command'
    })
  }

  private sendRequest<T>(
    child: ChildState,
    request: TerminalServiceRequest,
    timeoutMs: number
  ): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        const pending = this.pendingRequests.get(request.id)
        if (!pending || pending.childGeneration !== child.generation) return
        const error = new Error(`Terminal service request timed out: ${request.method}`)
        this.failChild(child, error, true)
      }, timeoutMs)
      timer.unref?.()

      this.pendingRequests.set(request.id, {
        childGeneration: child.generation,
        reject,
        resolve: (value) => resolve(value as T),
        timer
      })

      try {
        child.process.postMessage(request)
      } catch (error) {
        clearTimeout(timer)
        this.pendingRequests.delete(request.id)
        const normalizedError = error instanceof Error ? error : new Error(String(error))
        reject(normalizedError)
        this.failChild(child, normalizedError, true)
      }
    })
  }

  private sendSessionEvent(
    session: OwnedSession,
    channel: string,
    event: TerminalExitEvent | TerminalOutputEvent
  ): void {
    const owner = this.owners.get(session.ownerId)
    if (!owner || owner.contents.isDestroyed()) {
      this.releaseOwner(session.ownerId)
      return
    }

    try {
      owner.contents.send(channel, event)
    } catch {
      this.releaseOwner(session.ownerId)
    }
  }
}

function forkTerminalUtility(): UtilityProcess {
  return utilityProcess.fork(join(__dirname, 'terminal-service.js'), [], {
    serviceName: 'MyCopilot Terminal Service',
    stdio: 'pipe'
  })
}

function normalizeSessionId(requestedSessionId: string | undefined): string {
  const sessionId = requestedSessionId?.trim() || `terminal-${randomUUID()}`
  if (!SESSION_ID_PATTERN.test(sessionId)) throw new Error('Invalid terminal session id')
  return sessionId
}
