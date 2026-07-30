import {
  AlertTriangle,
  ChevronRight,
  LoaderCircle,
  RefreshCw,
  RotateCw,
  Trash2
} from 'lucide-react'
import type { McpServerListItem } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { McpServerStatusBadge } from './McpServerStatusBadge'
import { toSafeMcpDisplayText } from './mcpSafeDisplay'
import type { McpServerPendingOperation } from './useMcpManagement'

interface McpServerListProps {
  onDelete: (server: McpServerListItem) => void
  onOpen: (server: McpServerListItem) => void
  onRestart: (server: McpServerListItem) => void
  onSetEnabled: (server: McpServerListItem, enabled: boolean) => void
  pendingOperations: ReadonlyMap<string, McpServerPendingOperation>
  servers: readonly McpServerListItem[]
}

export function McpServerList({
  onDelete,
  onOpen,
  onRestart,
  onSetEnabled,
  pendingOperations,
  servers
}: McpServerListProps) {
  const { t } = useFrontendConfig()

  return (
    <div className="mcp-server-list">
      {servers.map((server) => {
        const pending = pendingOperations.get(server.serverId)
        const displayName = toSafeMcpDisplayText(server.displayName, 256)
        return (
          <article
            aria-busy={Boolean(pending) || undefined}
            className="mcp-server-row"
            key={server.serverId}
          >
            <button
              aria-label={t('mcp.actions.openDetails')}
              className="mcp-server-row__primary"
              onClick={() => onOpen(server)}
              type="button"
            >
              <span className="mcp-server-row__identity">
                <span className="mcp-server-row__name">{displayName}</span>
                <span className="mcp-transport-badge">STDIO</span>
                <span className="mcp-source-badge">{t('mcp.source.userManual')}</span>
              </span>
              <span className="mcp-server-row__summary">
                <McpServerStatusBadge state={server.state} />
                <span>{server.enabled ? t('mcp.enabled') : t('mcp.disabled')}</span>
                <span>
                  {t('mcp.tools.count')}: {server.toolCount}
                </span>
                {server.activeCallCount > 0 && (
                  <span>
                    {t('mcp.activeCalls')}: {server.activeCallCount}
                  </span>
                )}
              </span>
              {server.lastError && (
                <span className="mcp-server-row__error" role="status">
                  <AlertTriangle aria-hidden="true" />
                  {toSafeMcpDisplayText(server.lastError.message)}
                </span>
              )}
              <ChevronRight aria-hidden="true" className="mcp-server-row__chevron" />
            </button>

            <div className="mcp-server-row__controls">
              <button
                aria-label={t('mcp.actions.restart')}
                className="mcp-icon-button"
                disabled={Boolean(pending) || !server.enabled}
                onClick={() => onRestart(server)}
                type="button"
              >
                <RotateCw aria-hidden="true" />
              </button>
              <button
                aria-label={t('mcp.actions.delete')}
                className="mcp-icon-button mcp-icon-button--danger"
                disabled={Boolean(pending)}
                onClick={() => onDelete(server)}
                type="button"
              >
                <Trash2 aria-hidden="true" />
              </button>
              {pending && (
                <span className="mcp-row-pending" role="status">
                  <LoaderCircle aria-hidden="true" />
                  <span className="mcp-visually-hidden">{t('mcp.operation.pending')}</span>
                </span>
              )}
              <button
                aria-checked={server.enabled}
                aria-label={server.enabled ? t('mcp.actions.disable') : t('mcp.actions.enable')}
                className="settings-switch"
                data-state={server.enabled ? 'on' : 'off'}
                disabled={Boolean(pending)}
                onClick={() => onSetEnabled(server, !server.enabled)}
                role="switch"
                type="button"
              >
                <span aria-hidden="true" className="settings-switch__thumb" />
              </button>
            </div>
          </article>
        )
      })}
    </div>
  )
}

export function McpServerListRefreshButton({
  disabled,
  onRefresh
}: {
  disabled: boolean
  onRefresh: () => void
}) {
  const { t } = useFrontendConfig()
  return (
    <button
      aria-label={t('mcp.actions.refresh')}
      className="mcp-icon-button"
      disabled={disabled}
      onClick={onRefresh}
      type="button"
    >
      <RefreshCw aria-hidden="true" />
    </button>
  )
}
