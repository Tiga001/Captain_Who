import { useCallback, useEffect, useRef, useState } from 'react'
import type { RefObject } from 'react'
import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'
import { lightTheme } from '../../config/frontendTheme'
import {
  acknowledgeTerminalOutput,
  createTerminalSession,
  killTerminalSession,
  resizeTerminalSession,
  subscribeTerminalSession,
  writeTerminalInput
} from './terminalClient'
import { TerminalOutputWriter } from './TerminalOutputWriter'
import type { TerminalExitEvent, TerminalSessionStatus } from './terminalTypes'

interface UseTerminalSessionOptions {
  containerRef: RefObject<HTMLDivElement | null>
  initialCwd?: string
  isActive: boolean
  themeKey?: string
}

interface UseTerminalSessionResult {
  errorMessage: string | null
  status: TerminalSessionStatus
}

// A 165px bottom panel leaves 111px after its tab strip and terminal padding.
const MIN_TERMINAL_FIT_HEIGHT = 100
const MIN_TERMINAL_FIT_WIDTH = 220
const TERMINAL_RESIZE_SETTLE_MS = 140

function formatExitMessage(event: TerminalExitEvent) {
  if (event.signal) {
    return `\r\n[terminal exited by ${event.signal}]\r\n`
  }
  if (typeof event.exitCode === 'number') {
    return `\r\n[terminal exited with code ${event.exitCode}]\r\n`
  }
  return '\r\n[terminal exited]\r\n'
}

function createLocalSessionId() {
  const randomValue = Math.random().toString(36).slice(2, 10)
  return `terminal-${Date.now().toString(36)}-${randomValue}`
}

const TERMINAL_COLOR_VARIABLES = {
  background: '--mc-color-terminal-background',
  foreground: '--mc-color-terminal-foreground',
  cursor: '--mc-color-terminal-cursor',
  selectionBackground: '--mc-color-terminal-selection-background',
  black: '--mc-color-terminal-black',
  red: '--mc-color-terminal-red',
  green: '--mc-color-terminal-green',
  yellow: '--mc-color-terminal-yellow',
  blue: '--mc-color-terminal-blue',
  magenta: '--mc-color-terminal-magenta',
  cyan: '--mc-color-terminal-cyan',
  white: '--mc-color-terminal-white',
  brightBlack: '--mc-color-terminal-bright-black',
  brightRed: '--mc-color-terminal-bright-red',
  brightGreen: '--mc-color-terminal-bright-green',
  brightYellow: '--mc-color-terminal-bright-yellow',
  brightBlue: '--mc-color-terminal-bright-blue',
  brightMagenta: '--mc-color-terminal-bright-magenta',
  brightCyan: '--mc-color-terminal-bright-cyan',
  brightWhite: '--mc-color-terminal-bright-white'
} as const satisfies Record<keyof typeof lightTheme.colors.terminal, string>

function getTerminalTheme() {
  const computedStyle = getComputedStyle(document.documentElement)
  const getColor = <Key extends keyof typeof TERMINAL_COLOR_VARIABLES>(key: Key) =>
    computedStyle.getPropertyValue(TERMINAL_COLOR_VARIABLES[key]).trim() ||
    lightTheme.colors.terminal[key]

  return {
    background: getColor('background'),
    black: getColor('black'),
    blue: getColor('blue'),
    brightBlack: getColor('brightBlack'),
    brightBlue: getColor('brightBlue'),
    brightCyan: getColor('brightCyan'),
    brightGreen: getColor('brightGreen'),
    brightMagenta: getColor('brightMagenta'),
    brightRed: getColor('brightRed'),
    brightWhite: getColor('brightWhite'),
    brightYellow: getColor('brightYellow'),
    cursor: getColor('cursor'),
    cyan: getColor('cyan'),
    foreground: getColor('foreground'),
    green: getColor('green'),
    magenta: getColor('magenta'),
    red: getColor('red'),
    selectionBackground: getColor('selectionBackground'),
    white: getColor('white'),
    yellow: getColor('yellow')
  }
}

function canFitTerminal(container: HTMLElement) {
  const rect = container.getBoundingClientRect()
  const style = getComputedStyle(container)

  return (
    rect.width >= MIN_TERMINAL_FIT_WIDTH &&
    rect.height >= MIN_TERMINAL_FIT_HEIGHT &&
    style.display !== 'none' &&
    style.visibility !== 'hidden'
  )
}

export function useTerminalSession({
  containerRef,
  initialCwd,
  isActive,
  themeKey
}: UseTerminalSessionOptions): UseTerminalSessionResult {
  const terminalRef = useRef<Terminal | null>(null)
  const fitAddonRef = useRef<FitAddon | null>(null)
  const sessionIdRef = useRef<string | null>(null)
  const initialCwdRef = useRef(initialCwd)
  const isActiveRef = useRef(isActive)
  const [errorMessage, setErrorMessage] = useState<string | null>(null)
  const [status, setStatus] = useState<TerminalSessionStatus>('starting')

  useEffect(() => {
    isActiveRef.current = isActive
  }, [isActive])

  useEffect(() => {
    const terminal = terminalRef.current
    if (!terminal) return

    terminal.options.theme = getTerminalTheme()
  }, [themeKey])

  const fitTerminal = useCallback(() => {
    const terminal = terminalRef.current
    const fitAddon = fitAddonRef.current
    const container = containerRef.current
    if (!terminal || !fitAddon || !container || !canFitTerminal(container)) return

    try {
      const previousCols = terminal.cols
      const previousRows = terminal.rows
      fitAddon.fit()

      const sessionId = sessionIdRef.current
      if (sessionId && (terminal.cols !== previousCols || terminal.rows !== previousRows)) {
        void resizeTerminalSession(sessionId, terminal.cols, terminal.rows).catch((error) => {
          console.error('Failed to resize embedded terminal', error)
        })
      }
    } catch (error) {
      console.error('Failed to fit embedded terminal', error)
    }
  }, [containerRef])

  useEffect(() => {
    if (!isActive) return

    const frame = window.requestAnimationFrame(() => {
      fitTerminal()
    })
    const timer = window.setTimeout(fitTerminal, TERMINAL_RESIZE_SETTLE_MS)

    return () => {
      window.cancelAnimationFrame(frame)
      window.clearTimeout(timer)
    }
  }, [fitTerminal, isActive])

  useEffect(() => {
    const terminal = terminalRef.current
    if (!terminal) return
    terminal.options.cursorBlink = isActive
    if (!isActive) terminal.blur()
  }, [isActive])

  useEffect(() => {
    const container = containerRef.current
    if (!container) return undefined

    let isDisposed = false
    let resizeFrame = 0
    let resizeSettleTimer = 0
    const terminal = new Terminal({
      allowProposedApi: false,
      convertEol: false,
      cursorBlink: isActiveRef.current,
      fontFamily:
        'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace',
      fontSize: 13,
      lineHeight: 1.25,
      macOptionIsMeta: true,
      scrollback: 8000,
      theme: getTerminalTheme()
    })
    const fitAddon = new FitAddon()
    terminal.loadAddon(fitAddon)
    terminal.open(container)
    terminalRef.current = terminal
    fitAddonRef.current = fitAddon

    const runQueuedFit = () => {
      if (!isActiveRef.current) return
      window.cancelAnimationFrame(resizeFrame)
      resizeFrame = window.requestAnimationFrame(fitTerminal)
    }
    const queueFit = () => {
      window.clearTimeout(resizeSettleTimer)
      resizeSettleTimer = window.setTimeout(runQueuedFit, TERMINAL_RESIZE_SETTLE_MS)
    }

    const resizeObserver = new ResizeObserver(queueFit)
    resizeObserver.observe(container)

    const inputSubscription = terminal.onData((data) => {
      const sessionId = sessionIdRef.current
      if (!sessionId) return

      try {
        writeTerminalInput(sessionId, data)
      } catch (error) {
        console.error('Failed to write embedded terminal input', error)
      }
    })

    let outputWriter: TerminalOutputWriter | null = null
    let unsubscribeSession: (() => void) | null = null

    const startSession = async (requestedSessionId: string) => {
      try {
        if (canFitTerminal(container)) {
          fitAddon.fit()
        }
        const nextSession = await createTerminalSession({
          cols: terminal.cols,
          cwd: initialCwdRef.current,
          rows: terminal.rows,
          sessionId: requestedSessionId
        })

        if (isDisposed) {
          sessionIdRef.current = null
          void killTerminalSession(nextSession.sessionId).catch((error) => {
            console.error('Failed to clean up embedded terminal session', error)
          })
          return
        }

        sessionIdRef.current = nextSession.sessionId
        setStatus('running')
        if (isActiveRef.current) terminal.focus()
        queueFit()
      } catch (error) {
        if (isDisposed) return
        unsubscribeSession?.()
        unsubscribeSession = null
        outputWriter?.dispose()
        outputWriter = null
        const message = error instanceof Error ? error.message : String(error)
        sessionIdRef.current = null
        terminal.write(`\r\n[terminal start failed: ${message}]\r\n`)
        setErrorMessage(message)
        setStatus('error')
      }
    }

    const initializeTerminalBridge = async () => {
      const requestedSessionId = createLocalSessionId()
      sessionIdRef.current = requestedSessionId
      outputWriter = new TerminalOutputWriter({
        acknowledge: (sequence) => acknowledgeTerminalOutput(requestedSessionId, sequence),
        onProtocolError: (error) => {
          if (isDisposed) return
          setErrorMessage(error.message)
          setStatus('error')
          sessionIdRef.current = null
          void killTerminalSession(requestedSessionId).catch((killError) => {
            console.error('Failed to stop terminal after an output protocol error', killError)
          })
        },
        sessionId: requestedSessionId,
        write: (data, callback) => terminal.write(data, callback)
      })
      unsubscribeSession = subscribeTerminalSession(requestedSessionId, {
        onExit: (event) => {
          outputWriter?.finish(event.finalOutputSequence, () => {
            if (isDisposed) return
            terminal.write(formatExitMessage(event), () => {
              if (isDisposed) return
              if (sessionIdRef.current === requestedSessionId) sessionIdRef.current = null
              setStatus('exited')
            })
          })
        },
        onOutput: (event) => outputWriter?.accept(event)
      })
      await startSession(requestedSessionId)
    }

    queueFit()
    void initializeTerminalBridge().catch((error) => {
      const message = error instanceof Error ? error.message : String(error)
      terminal.write(`\r\n[terminal bridge failed: ${message}]\r\n`)
      setErrorMessage(message)
      setStatus('error')
    })

    return () => {
      isDisposed = true
      window.cancelAnimationFrame(resizeFrame)
      window.clearTimeout(resizeSettleTimer)
      resizeObserver.disconnect()
      inputSubscription.dispose()
      unsubscribeSession?.()
      outputWriter?.dispose()

      const sessionId = sessionIdRef.current
      sessionIdRef.current = null
      if (sessionId) {
        void killTerminalSession(sessionId).catch((error) => {
          console.error('Failed to kill embedded terminal session', error)
        })
      }

      terminal.dispose()
      terminalRef.current = null
      fitAddonRef.current = null
    }
  }, [containerRef, fitTerminal])

  return {
    errorMessage,
    status
  }
}
