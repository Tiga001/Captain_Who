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
  const { resolvedThemeId, t } = useFrontendConfig()
  const terminalContainerRef = useTerminalSessionContainer()
  useTerminalSession({
    containerRef: terminalContainerRef,
    initialCwd,
    isActive,
    themeKey: resolvedThemeId
  })

  return (
    <section className="terminal-panel" aria-label={t('terminal.title')}>
      <div className="terminal-panel__surface">
        <div ref={terminalContainerRef} className="terminal-panel__xterm" />
      </div>
    </section>
  )
}

function useTerminalSessionContainer() {
  return useRef<HTMLDivElement>(null)
}
