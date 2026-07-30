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
      return t('mcp.state.disabled')
    case 'starting':
      return t('mcp.state.starting')
    case 'discovering':
      return t('mcp.state.discovering')
    case 'ready':
      return t('mcp.state.ready')
    case 'stopping':
      return t('mcp.state.stopping')
    case 'error':
      return t('mcp.state.error')
    case 'backoff':
      return t('mcp.state.backoff')
    case 'degraded':
      return t('mcp.state.degraded')
    default:
      return t('mcp.state.unknown')
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
