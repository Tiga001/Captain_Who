import { useCallback, useEffect, useRef, useState } from 'react'
import type { RefObject } from 'react'
import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'
import { lightTheme } from '../../config/frontendTheme'
import {
  acknowledgeTerminalOutput,
  createTerminalSession,
  killTerminalSession,
  markTerminalUserInput,
  selectTerminalSourceDirectory,
  resizeTerminalSession,
  subscribeTerminalSession,
  writeTerminalInput
} from './terminalClient'
import { TerminalOutputWriter } from './TerminalOutputWriter'
import type {
  TerminalExitEvent,
  TerminalSessionStatus,
  TerminalSourceFolder
} from './terminalTypes'

interface UseTerminalSessionOptions {
  containerRef: RefObject<HTMLDivElement | null>
  initialCwd?: string
  projectId?: string
  isActive: boolean
  themeKey?: string
}

interface UseTerminalSessionResult {
  errorMessage: string | null
  status: TerminalSessionStatus
  hasUserInput: boolean
  sourceTop: number
  sourceFolders: readonly TerminalSourceFolder[]
  selectSourceDirectory: (folderId: string) => void
}

// Keep the compact bottom panel usable after its tab strip and terminal padding.
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
  projectId,
  isActive,
  themeKey
}: UseTerminalSessionOptions): UseTerminalSessionResult {
  const terminalRef = useRef<Terminal | null>(null)
  const fitAddonRef = useRef<FitAddon | null>(null)
  const sessionIdRef = useRef<string | null>(null)
  const initialCwdRef = useRef(initialCwd)
  const projectIdRef = useRef(projectId)
  const [sourceFolders, setSourceFolders] = useState<readonly TerminalSourceFolder[]>([])
  const hasUserInputRef = useRef(false)
  const selectSourceRef = useRef<(folderId: string) => void>(() => {})
  const [hasUserInput, setHasUserInput] = useState(false)
  const [sourceTop, setSourceTop] = useState(24)
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
    let sessionReady = false
    let sessionFinished = false
    let confirmedSourceFolders: readonly TerminalSourceFolder[] = []
    const requestedSessionId = createLocalSessionId()
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
    const isCurrentEffect = () => !isDisposed && terminalRef.current === terminal

    const runQueuedFit = () => {
      if (!isCurrentEffect() || !isActiveRef.current) return
      window.cancelAnimationFrame(resizeFrame)
      resizeFrame = window.requestAnimationFrame(() => {
        if (isCurrentEffect()) fitTerminal()
      })
    }
    const queueFit = () => {
      if (!isCurrentEffect()) return
      window.clearTimeout(resizeSettleTimer)
      resizeSettleTimer = window.setTimeout(runQueuedFit, TERMINAL_RESIZE_SETTLE_MS)
    }

    const resizeObserver = new ResizeObserver(queueFit)
    resizeObserver.observe(container)

    const pendingInput: { data: string; userInitiated: boolean }[] = []
    const noteUserInput = () => {
      if (!isCurrentEffect() || sessionFinished || hasUserInputRef.current) return
      hasUserInputRef.current = true
      setHasUserInput(true)
      if (sessionReady) markTerminalUserInput(requestedSessionId)
    }
    const keySubscription = terminal.onKey(noteUserInput)
    const onPaste = (event: ClipboardEvent) => {
      if (event.clipboardData?.getData('text/plain')) noteUserInput()
    }
    const onTextInput = (event: Event) => {
      if ((event as InputEvent).data || (event as InputEvent).inputType?.startsWith('delete'))
        noteUserInput()
    }
    container.addEventListener('paste', onPaste, true)
    container.addEventListener('input', onTextInput, true)
    container.addEventListener('compositionupdate', onTextInput, true)
    container.addEventListener('compositionend', onTextInput, true)

    const updateSourcePosition = () => {
      if (!isCurrentEffect() || hasUserInputRef.current) return
      const screen = container.querySelector('.xterm-screen')
      if (!screen) return
      const lineHeight = screen.getBoundingClientRect().height / terminal.rows
      const buffer = terminal.buffer.active
      const row = buffer.baseY + buffer.cursorY - buffer.viewportY + 1
      setSourceTop(Math.max(0, row * lineHeight))
    }
    const cursorSubscription = terminal.onCursorMove(updateSourcePosition)
    const renderSubscription = terminal.onRender(updateSourcePosition)
    const scrollSubscription = terminal.onScroll(updateSourcePosition)
    selectSourceRef.current = (folderId) => {
      if (
        !isCurrentEffect() ||
        !sessionReady ||
        sessionFinished ||
        hasUserInputRef.current ||
        !isActiveRef.current
      )
        return
      hasUserInputRef.current = true
      setHasUserInput(true)
      // Invocation is sent synchronously before subsequent onData events. The utility
      // serializes selection with input; no PTY replacement or inferred cwd update.
      void selectTerminalSourceDirectory(requestedSessionId, folderId).catch((error) => {
        if (isCurrentEffect())
          setErrorMessage(error instanceof Error ? error.message : String(error))
      })
      terminal.focus()
    }
    terminal.attachCustomKeyEventHandler((event) => {
      if (
        !isCurrentEffect() ||
        sessionFinished ||
        event.type !== 'keydown' ||
        !event.altKey ||
        event.ctrlKey ||
        event.metaKey ||
        event.shiftKey ||
        event.isComposing ||
        !isActiveRef.current ||
        hasUserInputRef.current ||
        !sessionReady ||
        confirmedSourceFolders.length < 2
      )
        return true
      const digit = /^Digit([1-9])$/.exec(event.code)
      const folder = digit ? confirmedSourceFolders[Number(digit[1]) - 1] : undefined
      if (!folder) return true
      event.preventDefault()
      event.stopPropagation()
      selectSourceRef.current(folder.id)
      return false
    })
    const inputSubscription = terminal.onData((data) => {
      if (!isCurrentEffect() || sessionFinished) return

      const userInitiated = hasUserInputRef.current
      if (!sessionReady) {
        pendingInput.push({ data, userInitiated })
        return
      }
      try {
        // DSR/cursor/focus replies also use onData. Only actual input events set this flag.
        writeTerminalInput(requestedSessionId, data, userInitiated)
      } catch (error) {
        console.error('Failed to write embedded terminal input', error)
      }
    })

    let outputWriter: TerminalOutputWriter | null = null
    let unsubscribeSession: (() => void) | null = null

    const startSession = async () => {
      try {
        if (canFitTerminal(container)) {
          fitAddon.fit()
        }
        const result = await createTerminalSession({
          cols: terminal.cols,
          cwd: initialCwdRef.current,
          projectId: projectIdRef.current,
          rows: terminal.rows,
          sessionId: requestedSessionId
        })

        if (result.status === 'cancelled') {
          if (!isCurrentEffect()) return
          sessionFinished = true
          sessionReady = false
          if (sessionIdRef.current === requestedSessionId) sessionIdRef.current = null
          unsubscribeSession?.()
          unsubscribeSession = null
          outputWriter?.dispose()
          outputWriter = null
          pendingInput.length = 0
          setStatus('exited')
          return
        }
        const nextSession = result.session
        if (!isCurrentEffect() || sessionFinished) {
          // A late result belongs only to this effect. StrictMode may already have
          // installed another session into the shared refs; never clear those here.
          void killTerminalSession(requestedSessionId).catch((error) => {
            console.error('Failed to clean up embedded terminal session', error)
          })
          return
        }

        sessionIdRef.current = requestedSessionId
        // Display labels, tooltips, ordering and shortcut ids must all come from
        // the exact Host snapshot that will validate the subsequent cd request.
        const confirmedSources = nextSession.sourceFolders?.map((folder) => ({ ...folder })) ?? []
        confirmedSourceFolders = confirmedSources
        setSourceFolders(confirmedSources)
        sessionReady = true
        // The panel may have resized while Host was loading project directories.
        // Bring the newly created PTY up to the local xterm size before later fits.
        if (nextSession.cols !== terminal.cols || nextSession.rows !== terminal.rows) {
          void resizeTerminalSession(requestedSessionId, terminal.cols, terminal.rows).catch(
            (error) => {
              console.error('Failed to resize newly created embedded terminal', error)
            }
          )
        }
        if (hasUserInputRef.current) markTerminalUserInput(requestedSessionId)
        for (const input of pendingInput.splice(0))
          writeTerminalInput(requestedSessionId, input.data, input.userInitiated)
        updateSourcePosition()
        setStatus('running')
        if (isActiveRef.current) terminal.focus()
        queueFit()
      } catch (error) {
        if (!isCurrentEffect()) return
        sessionFinished = true
        sessionReady = false
        unsubscribeSession?.()
        unsubscribeSession = null
        outputWriter?.dispose()
        outputWriter = null
        const message = error instanceof Error ? error.message : String(error)
        if (sessionIdRef.current === requestedSessionId) sessionIdRef.current = null
        terminal.write(`\r\n[terminal start failed: ${message}]\r\n`)
        setErrorMessage(message)
        setStatus('error')
      }
    }

    const initializeTerminalBridge = async () => {
      outputWriter = new TerminalOutputWriter({
        acknowledge: (sequence) => acknowledgeTerminalOutput(requestedSessionId, sequence),
        onProtocolError: (error) => {
          if (!isCurrentEffect()) return
          sessionFinished = true
          sessionReady = false
          setErrorMessage(error.message)
          setStatus('error')
          if (sessionIdRef.current === requestedSessionId) sessionIdRef.current = null
          void killTerminalSession(requestedSessionId).catch((killError) => {
            console.error('Failed to stop terminal after an output protocol error', killError)
          })
        },
        sessionId: requestedSessionId,
        write: (data, callback) => terminal.write(data, callback)
      })
      unsubscribeSession = subscribeTerminalSession(requestedSessionId, {
        onExit: (event) => {
          if (!isCurrentEffect()) return
          sessionFinished = true
          sessionReady = false
          outputWriter?.finish(event.finalOutputSequence, () => {
            if (!isCurrentEffect()) return
            terminal.write(formatExitMessage(event), () => {
              if (!isCurrentEffect()) return
              if (sessionIdRef.current === requestedSessionId) sessionIdRef.current = null
              setStatus('exited')
            })
          })
        },
        onOutput: (event) => outputWriter?.accept(event)
      })
      await startSession()
    }

    queueFit()
    void initializeTerminalBridge().catch((error) => {
      if (!isCurrentEffect()) return
      sessionFinished = true
      sessionReady = false
      if (sessionIdRef.current === requestedSessionId) sessionIdRef.current = null
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
      keySubscription.dispose()
      cursorSubscription.dispose()
      renderSubscription.dispose()
      scrollSubscription.dispose()
      container.removeEventListener('paste', onPaste, true)
      container.removeEventListener('input', onTextInput, true)
      container.removeEventListener('compositionupdate', onTextInput, true)
      container.removeEventListener('compositionend', onTextInput, true)
      sessionReady = false
      sessionFinished = true
      if (terminalRef.current === terminal) selectSourceRef.current = () => {}
      unsubscribeSession?.()
      outputWriter?.dispose()

      if (sessionIdRef.current === requestedSessionId) sessionIdRef.current = null
      void killTerminalSession(requestedSessionId).catch((error) => {
        console.error('Failed to kill embedded terminal session', error)
      })

      terminal.dispose()
      if (terminalRef.current === terminal) terminalRef.current = null
      if (fitAddonRef.current === fitAddon) fitAddonRef.current = null
    }
  }, [containerRef, fitTerminal])

  return {
    errorMessage,
    hasUserInput,
    sourceTop,
    sourceFolders,
    selectSourceDirectory: (folderId) => selectSourceRef.current(folderId),
    status
  }
}
