import { randomUUID } from 'node:crypto'
import { homedir } from 'node:os'
import * as pty from 'node-pty'
import type { IDisposable, IPty } from 'node-pty'

import type {
  TerminalCreateSessionRequest,
  TerminalExitEvent,
  TerminalSessionSnapshot
} from '@mycopilot/protocol'
import { TerminalExitDrainController } from './TerminalExitDrainController'
import { TerminalOutputFlowController } from './TerminalOutputFlowController'
import type {
  TerminalServiceCommand,
  TerminalServiceInboundMessage,
  TerminalServiceNotification,
  TerminalServiceRequest,
  TerminalServiceResponse
} from './terminalTransportProtocol'

type TerminalSessionRecord = {
  closeSubscription: IDisposable | null
  exitDrain: TerminalExitDrainController<Pick<TerminalExitEvent, 'exitCode' | 'signal'>>
  exitSubscription: IDisposable
  outputFlow: TerminalOutputFlowController
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
  handleMessage(messageEvent.data as TerminalServiceInboundMessage)
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

function handleMessage(message: TerminalServiceInboundMessage): void {
  if (message.type === 'command') {
    handleCommand(message)
    return
  }
  handleRequest(message)
}

function handleCommand(command: TerminalServiceCommand): void {
  try {
    switch (command.method) {
      case 'terminal.writeInput':
        writeInput(command.params.sessionId, command.params.data)
        return
      case 'terminal.acknowledgeOutput':
        acknowledgeOutput(command.params.sessionId, command.params.sequence)
        return
      case 'terminal.disposeSession':
        killSession(command.params.sessionId, true)
        return
    }
  } catch (error) {
    console.warn(
      `Ignored terminal service command ${command.method}`,
      error instanceof Error ? error.message : String(error)
    )
  }
}

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

  const exitDrainState: {
    controller?: TerminalExitDrainController<Pick<TerminalExitEvent, 'exitCode' | 'signal'>>
  } = {}
  const outputFlow = new TerminalOutputFlowController({
    onBatch: (event) => {
      sendNotification({ event, method: 'terminal.output', type: 'notification' })
    },
    onPause: () => ptyProcess.pause(),
    onResume: () => {
      ptyProcess.resume()
      exitDrainState.controller?.noteOutputResumed()
    },
    sessionId
  })

  const outputSubscription = ptyProcess.onData((data) => {
    outputFlow.push(data)
    exitDrainState.controller?.noteData()
  })

  const exitSubscription = ptyProcess.onExit(({ exitCode, signal }) => {
    exitDrainState.controller?.begin({
      exitCode,
      signal: typeof signal === 'number' ? String(signal) : null
    })
  })

  // Unix node-pty exposes stream close separately from its waitpid-based exit;
  // Windows emits onExit from the ConPTY socket close path after its own flush.
  const closeSubscription =
    process.platform === 'win32'
      ? null
      : subscribeToPtyClose(ptyProcess, () => exitDrainState.controller?.noteStreamClosed())
  const exitDrain = new TerminalExitDrainController<Pick<TerminalExitEvent, 'exitCode' | 'signal'>>(
    {
      isOutputPaused: () => outputFlow.isPaused,
      onDrainComplete: (result) => finishSession(sessionId, result),
      waitForStreamClose: closeSubscription !== null
    }
  )
  exitDrainState.controller = exitDrain

  sessions.set(sessionId, {
    closeSubscription,
    exitDrain,
    exitSubscription,
    outputFlow,
    outputSubscription,
    ptyProcess,
    snapshot
  })

  return snapshot
}

function writeInput(sessionId: string, data: string): void {
  assertValidSessionId(sessionId)
  if (typeof data !== 'string' || data.length === 0) return
  sessions.get(sessionId)?.ptyProcess.write(data)
}

function acknowledgeOutput(sessionId: string, sequence: number): void {
  assertValidSessionId(sessionId)
  sessions.get(sessionId)?.outputFlow.acknowledge(sequence)
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
  assertValidSessionId(sessionId)
  const session = sessions.get(sessionId)
  if (!session) return false

  sessions.delete(sessionId)
  const finalOutputSequence = disposeSessionRecord(session)

  try {
    session.ptyProcess.kill()
  } catch (error) {
    console.warn(`Failed to kill terminal session ${sessionId}`, error)
  }

  if (notify) {
    sendNotification({
      event: {
        exitCode: null,
        finalOutputSequence,
        sessionId,
        signal: 'killed'
      },
      method: 'terminal.exit',
      type: 'notification'
    })
  }

  return true
}

function finishSession(
  sessionId: string,
  result: Pick<TerminalExitEvent, 'exitCode' | 'signal'>
): void {
  const session = sessions.get(sessionId)
  if (!session) return

  sessions.delete(sessionId)
  const finalOutputSequence = disposeSessionRecord(session)

  sendNotification({
    event: {
      ...result,
      finalOutputSequence,
      sessionId
    },
    method: 'terminal.exit',
    type: 'notification'
  })
}

function killAllSessions(notify: boolean): void {
  for (const sessionId of [...sessions.keys()]) {
    killSession(sessionId, notify)
  }
}

function disposeSessionRecord(session: TerminalSessionRecord): number {
  session.outputSubscription.dispose()
  session.exitSubscription.dispose()
  session.closeSubscription?.dispose()
  session.exitDrain.dispose()
  session.outputFlow.flush()
  const finalOutputSequence = session.outputFlow.finalOutputSequence
  session.outputFlow.dispose()
  return finalOutputSequence
}

function subscribeToPtyClose(ptyProcess: IPty, listener: () => void): IDisposable | null {
  const eventSource = ptyProcess as IPty & {
    on?: (eventName: string, handler: () => void) => void
    removeListener?: (eventName: string, handler: () => void) => void
  }
  if (typeof eventSource.on !== 'function' || typeof eventSource.removeListener !== 'function') {
    return null
  }

  try {
    eventSource.on('close', listener)
  } catch {
    return null
  }

  let subscribed = true
  return {
    dispose: () => {
      if (!subscribed) return
      subscribed = false
      try {
        eventSource.removeListener?.('close', listener)
      } catch {
        // The native PTY can already be disposed when terminal cleanup runs.
      }
    }
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
    TERM_PROGRAM: 'CaptainWho'
  }
}

function normalizeTerminalSize(value: number | undefined, fallback: number): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    return fallback
  }

  return Math.max(1, Math.floor(value))
}

function sendResponse(message: TerminalServiceResponse): void {
  parentPort.postMessage(message)
}

function sendNotification(message: TerminalServiceNotification): void {
  parentPort.postMessage(message)
}
