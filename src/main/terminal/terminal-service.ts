import { randomUUID } from 'node:crypto'
import { homedir } from 'node:os'
import * as pty from 'node-pty'
import type { IDisposable, IPty } from 'node-pty'

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

type TerminalSessionRecord = {
  exitSubscription: IDisposable
  outputSubscription: IDisposable
  ptyProcess: IPty
  snapshot: TerminalSessionSnapshot
}

const SESSION_ID_PATTERN = /^[A-Za-z0-9._:-]{1,160}$/
const parentPort = process.parentPort

if (!parentPort) {
  throw new Error('Terminal service must run as an Electron utility process')
}

const sessions = new Map<string, TerminalSessionRecord>()

parentPort.on('message', (messageEvent) => {
  handleRequest(messageEvent.data as TerminalServiceRequest)
})

process.on('exit', () => {
  killAllSessions(false)
})

process.on('SIGTERM', () => {
  killAllSessions(false)
  process.exit(0)
})

process.on('SIGINT', () => {
  killAllSessions(false)
  process.exit(0)
})

function handleRequest(request: TerminalServiceRequest): void {
  try {
    switch (request.method) {
      case 'terminal.createSession':
        sendResponse({
          id: request.id,
          result: createSession(request.params),
          success: true,
          type: 'response'
        })
        return
      case 'terminal.writeInput':
        writeInput(request.params.sessionId, request.params.data)
        sendResponse({ id: request.id, success: true, type: 'response' })
        return
      case 'terminal.resizeSession':
        resizeSession(request.params.sessionId, request.params.cols, request.params.rows)
        sendResponse({ id: request.id, success: true, type: 'response' })
        return
      case 'terminal.killSession':
        sendResponse({
          id: request.id,
          result: killSession(request.params.sessionId, true),
          success: true,
          type: 'response'
        })
        return
      case 'terminal.shutdown':
        killAllSessions(true)
        sendResponse({ id: request.id, result: true, success: true, type: 'response' })
        setTimeout(() => process.exit(0), 0)
        return
    }
  } catch (error) {
    sendResponse({
      error: error instanceof Error ? error.message : String(error),
      id: request.id,
      success: false,
      type: 'response'
    })
  }
}

function createSession(request: TerminalCreateSessionRequest): TerminalSessionSnapshot {
  const sessionId = normalizeSessionId(request.sessionId)
  if (sessions.has(sessionId)) {
    throw new Error(`Terminal session already exists: ${sessionId}`)
  }

  const cwd = request.cwd?.trim() || process.env.HOME || homedir()
  const shell = getDefaultShell()
  const cols = normalizeTerminalSize(request.cols, 80)
  const rows = normalizeTerminalSize(request.rows, 24)
  const ptyProcess = pty.spawn(shell, [], {
    cols,
    cwd,
    env: createTerminalEnvironment(),
    name: 'xterm-256color',
    rows
  })

  const snapshot: TerminalSessionSnapshot = {
    cols,
    cwd,
    processId: ptyProcess.pid,
    rows,
    sessionId,
    shell
  }

  const outputSubscription = ptyProcess.onData((data) => {
    sendNotification({
      event: {
        data,
        sessionId
      },
      method: 'terminal.output',
      type: 'notification'
    })
  })

  const exitSubscription = ptyProcess.onExit(({ exitCode, signal }) => {
    sessions.delete(sessionId)
    outputSubscription.dispose()
    exitSubscription.dispose()

    sendNotification({
      event: {
        exitCode,
        sessionId,
        signal: typeof signal === 'number' ? String(signal) : null
      },
      method: 'terminal.exit',
      type: 'notification'
    })
  })

  sessions.set(sessionId, {
    exitSubscription,
    outputSubscription,
    ptyProcess,
    snapshot
  })

  return snapshot
}

function writeInput(sessionId: string, data: string): void {
  getSession(sessionId).ptyProcess.write(data)
}

function resizeSession(sessionId: string, cols: number, rows: number): void {
  const session = getSession(sessionId)
  const nextCols = normalizeTerminalSize(cols, session.snapshot.cols)
  const nextRows = normalizeTerminalSize(rows, session.snapshot.rows)

  session.ptyProcess.resize(nextCols, nextRows)
  session.snapshot = {
    ...session.snapshot,
    cols: nextCols,
    rows: nextRows
  }
}

function killSession(sessionId: string, notify: boolean): boolean {
  const session = sessions.get(sessionId)
  if (!session) return false

  sessions.delete(sessionId)
  session.outputSubscription.dispose()
  session.exitSubscription.dispose()

  try {
    session.ptyProcess.kill()
  } catch (error) {
    console.warn(`Failed to kill terminal session ${sessionId}`, error)
  }

  if (notify) {
    sendNotification({
      event: {
        exitCode: null,
        sessionId,
        signal: 'killed'
      },
      method: 'terminal.exit',
      type: 'notification'
    })
  }

  return true
}

function killAllSessions(notify: boolean): void {
  for (const sessionId of [...sessions.keys()]) {
    killSession(sessionId, notify)
  }
}

function getSession(sessionId: string): TerminalSessionRecord {
  assertValidSessionId(sessionId)

  const session = sessions.get(sessionId)
  if (!session) {
    throw new Error(`Terminal session not found: ${sessionId}`)
  }

  return session
}

function normalizeSessionId(requestedSessionId: string | undefined): string {
  const sessionId = requestedSessionId?.trim() || `terminal-${randomUUID()}`
  assertValidSessionId(sessionId)
  return sessionId
}

function assertValidSessionId(sessionId: string): void {
  if (!SESSION_ID_PATTERN.test(sessionId)) {
    throw new Error('Invalid terminal session id')
  }
}

function getDefaultShell(): string {
  if (process.platform === 'darwin') {
    return process.env.SHELL || '/bin/zsh'
  }

  if (process.platform === 'win32') {
    return 'powershell.exe'
  }

  return process.env.SHELL || '/bin/sh'
}

function createTerminalEnvironment(): Record<string, string | undefined> {
  return {
    ...process.env,
    COLORTERM: 'truecolor',
    TERM: 'xterm-256color',
    TERM_PROGRAM: 'MyCopilot'
  }
}

function normalizeTerminalSize(value: number | undefined, fallback: number): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    return fallback
  }

  return Math.max(1, Math.floor(value))
}

function sendResponse(message: Extract<TerminalServiceMessage, { type: 'response' }>): void {
  parentPort.postMessage(message)
}

function sendNotification(
  message: Extract<TerminalServiceMessage, { type: 'notification' }>
): void {
  parentPort.postMessage(message)
}
