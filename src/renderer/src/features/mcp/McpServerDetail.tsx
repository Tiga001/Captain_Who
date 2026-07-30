import { ArrowLeft, KeyRound, Pencil, RotateCw, ShieldAlert } from 'lucide-react'
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
  onAuthorize: () => void
  onBack: () => void
  onEdit: () => void
  onRestart: () => void
  onSetEnabled: (enabled: boolean) => void
  pending?: McpServerPendingOperation
  server: McpServerDetailsView
}

export function McpServerDetail({
  onAuthorize,
  onBack,
  onEdit,
  onRestart,
  onSetEnabled,
  pending,
  server
}: McpServerDetailProps) {
  const { language, t } = useFrontendConfig()
  const busy = Boolean(pending)
  const displayName = toSafeMcpDisplayText(server.displayName, 256)

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
            <span className="mcp-transport-badge">STDIO</span>
            <McpServerStatusBadge state={server.state} />
            <McpAuthorizationBadge state={server.launchAuthorizationState} />
          </div>
        </div>
        <div className="mcp-detail-header__actions">
          <button className="mcp-secondary-button" disabled={busy} onClick={onEdit} type="button">
            <Pencil aria-hidden="true" />
            {t('mcp.actions.edit')}
          </button>
          <button
            className="mcp-secondary-button"
            disabled={busy}
            onClick={onAuthorize}
            type="button"
          >
            <KeyRound aria-hidden="true" />
            {t('mcp.actions.authorizeLaunch')}
          </button>
        </div>
      </header>

      <div className="mcp-detail-callout">
        <ShieldAlert aria-hidden="true" />
        <p>{t('mcp.authorization.separationHelp')}</p>
      </div>

      <dl className="mcp-detail-grid">
        <DetailItem label={t('mcp.detail.serverId')} value={server.serverId} />
        <DetailItem label={t('mcp.detail.scope')} value={t('mcp.scope.user')} />
        <DetailItem label={t('mcp.detail.source')} value={t('mcp.source.userManual')} />
        <DetailItem
          label={t('mcp.detail.trust')}
          value={
            server.trust === 'userApproved' ? t('mcp.trust.userApproved') : t('mcp.trust.untrusted')
          }
        />
        <DetailItem
          label={t('mcp.detail.enabled')}
          value={server.enabled ? t('mcp.enabled') : t('mcp.disabled')}
        />
        <DetailItem
          label={t('mcp.detail.approvalMode')}
          value={
            server.approvalMode === 'prompt'
              ? t('mcp.form.promptEveryCall')
              : t('mcp.form.denyCalls')
          }
        />
        <DetailItem
          label={t('mcp.detail.configFingerprint')}
          value={shortMcpFingerprint(server.configDigest)}
        />
        <DetailItem
          label={t('mcp.detail.configEpoch')}
          value={shortMcpFingerprint(server.configEpoch)}
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
        <DetailItem label={t('mcp.detail.toolCount')} value={String(server.toolCount)} />
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
        <p className="mcp-detail-caption">{t('mcp.form.noSecretsWarning')}</p>
      </section>

      <section className="mcp-detail-section">
        <h3>{t('mcp.detail.capabilities')}</h3>
        {server.capabilities ? (
          <ul className="mcp-capability-list">
            <Capability label={t('mcp.capability.tools')} supported={server.capabilities.tools} />
            <Capability
              label={t('mcp.capability.resources')}
              supported={server.capabilities.resources}
            />
            <Capability
              label={t('mcp.capability.prompts')}
              supported={server.capabilities.prompts}
            />
            <Capability
              label={t('mcp.capability.logging')}
              supported={server.capabilities.logging}
            />
            <Capability
              label={t('mcp.capability.completion')}
              supported={server.capabilities.completion}
            />
          </ul>
        ) : (
          <p className="mcp-detail-caption">{t('mcp.detail.unavailable')}</p>
        )}
      </section>

      {server.lastError && (
        <div className="mcp-safe-error" role="alert">
          <strong>{toSafeMcpDisplayText(server.lastError.code, 128)}</strong>
          <p>{toSafeMcpDisplayText(server.lastError.message)}</p>
        </div>
      )}

      <div className="mcp-detail-footer">
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
        <span>{server.enabled ? t('mcp.enabled') : t('mcp.disabled')}</span>
        <button
          className="mcp-secondary-button"
          disabled={busy || !server.enabled}
          onClick={onRestart}
          type="button"
        >
          <RotateCw aria-hidden="true" />
          {t('mcp.actions.restart')}
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

function Capability({ label, supported }: { label: string; supported: boolean }) {
  const { t } = useFrontendConfig()
  return (
    <li>
      <span>{label}</span>
      <strong>{supported ? t('mcp.capability.supported') : t('mcp.capability.unsupported')}</strong>
    </li>
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
