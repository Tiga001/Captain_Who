import { ArrowLeft, Pencil, RefreshCw, Trash2 } from 'lucide-react'
import type { McpProtocolSnapshotView, McpServerDetailsView } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import {
  McpAuthorizationBadge,
  McpCatalogCompletenessBadge,
  McpServerStatusBadge
} from './McpServerStatusBadge'
import { shortMcpFingerprint, toSafeMcpDisplayText } from './mcpSafeDisplay'
import type { McpServerPendingOperation } from './useMcpManagement'

interface McpServerDetailProps {
  onBack: () => void
  onDelete: () => void
  onEdit: () => void
  onRetry: () => void
  onSetEnabled: (enabled: boolean) => void
  pending?: McpServerPendingOperation
  server: McpServerDetailsView
}

export function McpServerDetail({
  onBack,
  onDelete,
  onEdit,
  onRetry,
  onSetEnabled,
  pending,
  server
}: McpServerDetailProps) {
  const { language, t } = useFrontendConfig()
  const busy = Boolean(pending)
  const displayName = toSafeMcpDisplayText(server.displayName, 256)
  const canRetry =
    server.enabled &&
    (server.state === 'error' || server.state === 'backoff' || server.state === 'degraded')

  return (
    <section aria-busy={busy || undefined} className="mcp-server-detail">
      <header className="mcp-detail-header">
        <button
          aria-label={t('mcp.actions.backToServers')}
          className="mcp-icon-button"
          onClick={onBack}
          type="button"
        >
          <ArrowLeft aria-hidden="true" />
        </button>
        <div>
          <h2>{displayName}</h2>
          <div className="mcp-detail-badges">
            <McpServerStatusBadge state={server.state} />
            <span className="mcp-detail-tool-count">
              {t('mcp.tools.count')}: {server.toolCount}
            </span>
          </div>
        </div>
        <button className="mcp-secondary-button" disabled={busy} onClick={onEdit} type="button">
          <Pencil aria-hidden="true" />
          {t('mcp.actions.edit')}
        </button>
      </header>

      <div className="mcp-detail-overview">
        <div>
          <strong>{server.enabled ? t('mcp.enabled') : t('mcp.disabled')}</strong>
          <p>
            {server.enabled
              ? t('mcp.detail.enabledHelp')
              : t('mcp.detail.enableWithConfirmationHelp')}
          </p>
        </div>
        <button
          aria-checked={server.enabled}
          aria-label={server.enabled ? t('mcp.actions.disable') : t('mcp.actions.enable')}
          className="settings-switch"
          data-state={server.enabled ? 'on' : 'off'}
          disabled={busy}
          onClick={() => onSetEnabled(!server.enabled)}
          role="switch"
          type="button"
        >
          <span aria-hidden="true" className="settings-switch__thumb" />
        </button>
      </div>

      {server.lastError && (
        <div className="mcp-safe-error" role="alert">
          <p>{toSafeMcpDisplayText(server.lastError.message)}</p>
          {canRetry && (
            <button
              className="mcp-secondary-button"
              disabled={busy}
              onClick={onRetry}
              type="button"
            >
              <RefreshCw aria-hidden="true" />
              {t('mcp.actions.retryConnection')}
            </button>
          )}
        </div>
      )}

      <section className="mcp-detail-section">
        <h3>{t('mcp.detail.launchConfiguration')}</h3>
        <dl className="mcp-launch-details">
          <DetailItem label={t('mcp.form.executable')} value={server.executable} />
          <div className="mcp-detail-item mcp-detail-item--wide">
            <dt>{t('mcp.form.arguments')}</dt>
            <dd>
              {server.arguments.length === 0 ? (
                t('mcp.detail.none')
              ) : (
                <ol className="mcp-argv-list">
                  {server.arguments.map((argument, index) => (
                    <li key={index}>
                      <code>
                        {argument ? toSafeMcpDisplayText(argument) : t('mcp.detail.emptyArgument')}
                      </code>
                    </li>
                  ))}
                </ol>
              )}
            </dd>
          </div>
          <DetailItem label={t('mcp.form.cwd')} value={server.cwd} wide />
        </dl>
      </section>

      <details className="mcp-technical-details">
        <summary>{t('mcp.detail.technicalDetails')}</summary>
        <dl className="mcp-detail-grid">
          <DetailItem label={t('mcp.detail.serverId')} value={server.serverId} />
          <DetailItem label={t('mcp.detail.source')} value={t('mcp.source.userManual')} />
          <div className="mcp-detail-item">
            <dt>{t('mcp.authorization.title')}</dt>
            <dd>
              <McpAuthorizationBadge state={server.launchAuthorizationState} />
            </dd>
          </div>
          <DetailItem
            label={t('mcp.detail.approvalMode')}
            value={
              server.approvalMode === 'prompt'
                ? t('mcp.form.promptEveryCall')
                : server.approvalMode === 'auto'
                  ? t('mcp.form.autoExecute')
                  : t('mcp.form.denyCalls')
            }
          />
          <DetailItem
            label={t('mcp.detail.configFingerprint')}
            value={shortMcpFingerprint(server.configDigest)}
          />
          <DetailItem
            label={t('mcp.detail.registryRevision')}
            value={String(server.registryRevision)}
          />
          <DetailItem
            label={t('mcp.detail.catalogGeneration')}
            value={String(server.catalogGeneration)}
          />
          <div className="mcp-detail-item">
            <dt>{t('mcp.detail.catalogCompleteness')}</dt>
            <dd>
              <McpCatalogCompletenessBadge completeness={server.catalogCompleteness} />
            </dd>
          </div>
          <DetailItem
            label={t('mcp.detail.activeCallCount')}
            value={String(server.activeCallCount)}
          />
          <DetailItem
            label={t('mcp.detail.protocolVersion')}
            value={server.protocol?.protocolVersion ?? t('mcp.detail.unavailable')}
          />
          <DetailItem
            label={t('mcp.detail.lifecycle')}
            value={protocolLifecycleLabel(server.protocol?.lifecycle, t)}
          />
          <DetailItem
            label={t('mcp.detail.createdAt')}
            value={formatMcpDate(server.createdAtMs, language)}
          />
          <DetailItem
            label={t('mcp.detail.updatedAt')}
            value={formatMcpDate(server.updatedAtMs, language)}
          />
        </dl>
      </details>

      <div className="mcp-detail-footer">
        <button className="mcp-danger-button" disabled={busy} onClick={onDelete} type="button">
          <Trash2 aria-hidden="true" />
          {t('mcp.actions.delete')}
        </button>
      </div>
    </section>
  )
}

function DetailItem({
  label,
  value,
  wide = false
}: {
  label: string
  value: string
  wide?: boolean
}) {
  return (
    <div className={`mcp-detail-item${wide ? ' mcp-detail-item--wide' : ''}`}>
      <dt>{label}</dt>
      <dd>{toSafeMcpDisplayText(value)}</dd>
    </div>
  )
}

type Translate = ReturnType<typeof useFrontendConfig>['t']

function protocolLifecycleLabel(
  lifecycle: McpProtocolSnapshotView['lifecycle'] | undefined,
  t: Translate
): string {
  switch (lifecycle) {
    case 'discover':
      return t('mcp.lifecycle.discover')
    case 'initializeFallback':
      return t('mcp.lifecycle.initializeFallback')
    case 'unknown':
    case undefined:
      return t('mcp.detail.unavailable')
    default:
      return t('mcp.detail.unavailable')
  }
}

function formatMcpDate(timestamp: number, language: string): string {
  const date = new Date(timestamp)
  if (!Number.isFinite(timestamp) || Number.isNaN(date.getTime())) return '—'
  try {
    return new Intl.DateTimeFormat(language, {
      dateStyle: 'medium',
      timeStyle: 'short'
    }).format(date)
  } catch {
    return '—'
  }
}
