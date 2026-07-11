import { useRef } from 'react'
import '@xterm/xterm/css/xterm.css'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { useTerminalSession } from './useTerminalSession'
import './TerminalPanel.css'

interface TerminalPanelProps {
  initialCwd?: string
  isActive: boolean
}

export function TerminalPanel({ initialCwd, isActive }: TerminalPanelProps) {
  const { resolvedTheme, t } = useFrontendConfig()
  const terminalContainerRef = useTerminalSessionContainer()
  const { errorMessage, status } = useTerminalSession({
    containerRef: terminalContainerRef,
    initialCwd,
    isActive,
    themeKey: resolvedTheme
  })
  const statusLabel = {
    error: t('terminal.status.error'),
    exited: t('terminal.status.exited'),
    running: t('terminal.status.running'),
    starting: t('terminal.status.starting')
  }[status]

  return (
    <section className="terminal-panel" aria-label={t('terminal.title')}>
      <div className="terminal-panel__surface">
        <div ref={terminalContainerRef} className="terminal-panel__xterm" />
      </div>

      <footer className="terminal-panel__status" data-status={status}>
        <span>{statusLabel}</span>
        {errorMessage && <span className="terminal-panel__error">{errorMessage}</span>}
      </footer>
    </section>
  )
}

function useTerminalSessionContainer() {
  return useRef<HTMLDivElement>(null)
}
