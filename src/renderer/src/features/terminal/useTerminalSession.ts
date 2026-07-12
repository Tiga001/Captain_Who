import { useCallback, useEffect, useRef, useState } from 'react'
import type { RefObject } from 'react'
import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'
import { lightTheme } from '../../config/frontendTheme'
import {
  createTerminalSession,
  killTerminalSession,
  listenToTerminalExit,
  listenToTerminalOutput,
  resizeTerminalSession,
  writeTerminalInput
} from './terminalClient'
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

const MIN_TERMINAL_FIT_HEIGHT = 120
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

function getCssColor(name: string, fallback: string) {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim()
  return value || fallback
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

function getTerminalColor<Key extends keyof typeof TERMINAL_COLOR_VARIABLES>(key: Key) {
  return getCssColor(TERMINAL_COLOR_VARIABLES[key], lightTheme.colors.terminal[key])
}

function getTerminalTheme() {
  return {
    background: getTerminalColor('background'),
    black: getTerminalColor('black'),
    blue: getTerminalColor('blue'),
    brightBlack: getTerminalColor('brightBlack'),
    brightBlue: getTerminalColor('brightBlue'),
    brightCyan: getTerminalColor('brightCyan'),
    brightGreen: getTerminalColor('brightGreen'),
    brightMagenta: getTerminalColor('brightMagenta'),
    brightRed: getTerminalColor('brightRed'),
    brightWhite: getTerminalColor('brightWhite'),
    brightYellow: getTerminalColor('brightYellow'),
    cursor: getTerminalColor('cursor'),
    cyan: getTerminalColor('cyan'),
    foreground: getTerminalColor('foreground'),
    green: getTerminalColor('green'),
    magenta: getTerminalColor('magenta'),
    red: getTerminalColor('red'),
    selectionBackground: getTerminalColor('selectionBackground'),
    white: getTerminalColor('white'),
    yellow: getTerminalColor('yellow')
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
  const [errorMessage, setErrorMessage] = useState<string | null>(null)
  const [status, setStatus] = useState<TerminalSessionStatus>('starting')

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
    const container = containerRef.current
    if (!container) return undefined

    let isDisposed = false
    let resizeFrame = 0
    let resizeSettleTimer = 0
    const terminal = new Terminal({
      allowProposedApi: false,
      convertEol: false,
      cursorBlink: true,
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

      void writeTerminalInput(sessionId, data).catch((error) => {
        console.error('Failed to write embedded terminal input', error)
      })
    })

    let unlistenOutput: (() => void) | null = null
    let unlistenExit: (() => void) | null = null

    const startSession = async () => {
      const requestedSessionId = createLocalSessionId()
      sessionIdRef.current = requestedSessionId

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
        terminal.focus()
        queueFit()
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error)
        sessionIdRef.current = null
        terminal.write(`\r\n[terminal start failed: ${message}]\r\n`)
        setErrorMessage(message)
        setStatus('error')
      }
    }

    const initializeTerminalBridge = async () => {
      const [outputUnlisten, exitUnlisten] = await Promise.all([
        listenToTerminalOutput((event) => {
          if (event.sessionId !== sessionIdRef.current) return
          terminal.write(event.data)
        }),
        listenToTerminalExit((event) => {
          if (event.sessionId !== sessionIdRef.current) return
          terminal.write(formatExitMessage(event))
          sessionIdRef.current = null
          setStatus('exited')
        })
      ])

      if (isDisposed) {
        outputUnlisten()
        exitUnlisten()
        return
      }

      unlistenOutput = outputUnlisten
      unlistenExit = exitUnlisten
      await startSession()
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
      unlistenOutput?.()
      unlistenExit?.()

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
