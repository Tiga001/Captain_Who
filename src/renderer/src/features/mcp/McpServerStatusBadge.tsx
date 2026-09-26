import type { McpServerStateView } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'

export function McpServerStatusBadge({ state }: { state: McpServerStateView }) {
  const { t } = useFrontendConfig()
  return (
    <span aria-live="polite" className="mcp-status-badge" data-state={state} role="status">
      {stateLabel(state, t)}
    </span>
  )
}

type Translate = ReturnType<typeof useFrontendConfig>['t']

function stateLabel(state: McpServerStateView, t: Translate): string {
  switch (state) {
    case 'disabled':
    case 'stopping':
      return t('mcp.state.off')
    case 'starting':
    case 'discovering':
    case 'backoff':
      return t('mcp.state.connecting')
    case 'ready':
      return t('mcp.state.ready')
    case 'error':
    case 'degraded':
      return t('mcp.state.needsAttention')
    default:
      return t('mcp.state.needsAttention')
  }
}
