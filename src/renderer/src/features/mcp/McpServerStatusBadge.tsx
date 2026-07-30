import type {
  McpCatalogCompletenessView,
  McpLaunchAuthorizationState,
  McpServerStateView
} from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'

export function McpServerStatusBadge({ state }: { state: McpServerStateView }) {
  const { t } = useFrontendConfig()
  return (
    <span aria-live="polite" className="mcp-status-badge" data-state={state} role="status">
      {stateLabel(state, t)}
    </span>
  )
}

export function McpAuthorizationBadge({ state }: { state: McpLaunchAuthorizationState }) {
  const { t } = useFrontendConfig()
  return (
    <span aria-live="polite" className="mcp-authorization-badge" data-state={state} role="status">
      {authorizationLabel(state, t)}
    </span>
  )
}

export function McpCatalogCompletenessBadge({
  completeness
}: {
  completeness: McpCatalogCompletenessView
}) {
  const { t } = useFrontendConfig()
  return (
    <span aria-live="polite" className="mcp-catalog-badge" data-state={completeness} role="status">
      {catalogCompletenessLabel(completeness, t)}
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

function authorizationLabel(state: McpLaunchAuthorizationState, t: Translate): string {
  switch (state) {
    case 'required':
      return t('mcp.authorization.required')
    case 'authorized':
      return t('mcp.authorization.authorized')
    case 'stale':
      return t('mcp.authorization.stale')
    default:
      return t('mcp.state.unknown')
  }
}

function catalogCompletenessLabel(completeness: McpCatalogCompletenessView, t: Translate): string {
  switch (completeness) {
    case 'complete':
      return t('mcp.catalog.complete')
    case 'partial':
      return t('mcp.catalog.partial')
    case 'stale':
      return t('mcp.catalog.stale')
    case 'failed':
      return t('mcp.catalog.failed')
    default:
      return t('mcp.state.unknown')
  }
}
