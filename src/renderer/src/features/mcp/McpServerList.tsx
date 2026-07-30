import { RefreshCw, Settings2 } from 'lucide-react'
import type { McpServerListItem } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { McpServerStatusBadge } from './McpServerStatusBadge'
import { toSafeMcpDisplayText } from './mcpSafeDisplay'
import type { McpServerPendingOperation } from './useMcpManagement'

interface McpServerListProps {
  onEdit: (server: McpServerListItem) => void
  onSetEnabled: (server: McpServerListItem, enabled: boolean) => void
  pendingOperations: ReadonlyMap<string, McpServerPendingOperation>
  servers: readonly McpServerListItem[]
}

export function McpServerList({
  onEdit,
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
            <div className="mcp-server-row__primary">
              <span className="mcp-server-row__name">{displayName}</span>
              <McpServerStatusBadge state={server.state} />
            </div>

            <div className="mcp-server-row__controls">
              <button
                aria-label={`${t('mcp.actions.edit')}: ${displayName}`}
                className="mcp-icon-button"
                disabled={Boolean(pending)}
                onClick={() => onEdit(server)}
                type="button"
              >
                <Settings2 aria-hidden="true" />
              </button>
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
