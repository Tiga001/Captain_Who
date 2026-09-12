import { useRef } from 'react'
import '@xterm/xterm/css/xterm.css'
import { Tooltip } from '../../components/overlay/Tooltip'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { useTerminalSession } from './useTerminalSession'
import './TerminalPanel.css'

interface TerminalPanelProps {
  initialCwd?: string
  projectId?: string
  isActive: boolean
}

export function TerminalPanel({ initialCwd, projectId, isActive }: TerminalPanelProps) {
  const { resolvedThemeId, t } = useFrontendConfig()
  const terminalContainerRef = useTerminalSessionContainer()
  const {
    hasUserInput,
    sourceTop,
    sourceFolders: sources,
    selectSourceDirectory,
    status,
    errorMessage
  } = useTerminalSession({
    containerRef: terminalContainerRef,
    initialCwd,
    projectId,
    isActive,
    themeKey: resolvedThemeId
  })

  return (
    <section className="terminal-panel" aria-label={t('terminal.title')}>
      <div className="terminal-panel__surface">
        <div ref={terminalContainerRef} className="terminal-panel__xterm" />
        {sources.length > 1 && !hasUserInput && status !== 'error' && status !== 'exited' && (
          <div
            className="terminal-panel__sources"
            style={{ top: sourceTop + 8, maxHeight: `calc(100% - ${sourceTop + 8}px)` }}
            aria-label={t('terminal.sourceDirectories')}
          >
            <span className="terminal-panel__sources-label">{t('terminal.sourceDirectories')}</span>
            {sources.map((folder, index) => (
              <Tooltip key={folder.id} content={folder.path} delayMs={0} describeTrigger>
                <button
                  type="button"
                  className="terminal-panel__source"
                  disabled={status !== 'running'}
                  onClick={() => selectSourceDirectory(folder.id)}
                >
                  <span className="terminal-panel__source-number">{index + 1}</span>
                  <span
                    className={
                      folder.role === 'primary' ? 'terminal-panel__source-primary' : undefined
                    }
                  >
                    {folder.alias}
                  </span>
                </button>
              </Tooltip>
            ))}
            <span className="terminal-panel__source-shortcut">
              {t('terminal.sourceShortcut').replace(
                '{modifier}',
                /Mac/.test(navigator.platform) ? '⌥ Opt' : 'Alt'
              )}
            </span>
          </div>
        )}
        {errorMessage && status === 'running' && (
          <div className="terminal-panel__source-error" role="alert" title={errorMessage}>
            {t('terminal.sourceDirectoryChangeFailed')}
          </div>
        )}
      </div>
    </section>
  )
}

function useTerminalSessionContainer() {
  return useRef<HTMLDivElement>(null)
}
