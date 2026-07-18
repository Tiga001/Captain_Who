// Renderer settings page: manages globally bundled and installed Agent Skills through Host API.
import { AlertTriangle, LoaderCircle, RefreshCw, WandSparkles } from 'lucide-react'
import { useCallback, useState } from 'react'
import type { SkillManagementEntry } from '@mycopilot/protocol'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { useToast } from '../../../components/toast/ToastContext'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { SkillInstallationDialog } from '../../skills/management/SkillInstallationDialog'
import { SkillManagementList } from '../../skills/management/SkillManagementList'
import {
  getSkillOperationErrorDetails,
  shouldRefreshSkillsAfterError
} from '../../skills/management/skillManagementErrors'
import { useSkillInstallationWorkflow } from '../../skills/management/useSkillInstallationWorkflow'
import { useSkillManagement } from '../../skills/management/useSkillManagement'
import './SkillsSettingsPage.css'

export function SkillsSettingsPage() {
  const { t } = useFrontendConfig()
  const { showToast } = useToast()
  const { pendingOperations, refresh, setEnabled, state, uninstall } = useSkillManagement()
  const [pendingUninstall, setPendingUninstall] = useState<SkillManagementEntry | null>(null)

  const handleCommitted = useCallback(async () => {
    await refresh()
  }, [refresh])
  const handleCommitMayHaveSucceeded = useCallback(async () => {
    await refresh()
    showToast(t('skills.operationNeedsConfirmation'), { durationMs: 3200 })
  }, [refresh, showToast, t])
  const installation = useSkillInstallationWorkflow({
    onCommitMayHaveSucceeded: handleCommitMayHaveSucceeded,
    onCommitted: handleCommitted
  })

  const changeEnabled = async (entry: SkillManagementEntry, enabled: boolean) => {
    try {
      await setEnabled(entry, enabled)
    } catch (error) {
      const details = getSkillOperationErrorDetails(error)
      showToast(details.code === 'stateConflict' ? t('skills.stateChanged') : details.message, {
        durationMs: 3200
      })
    }
  }

  const confirmUninstall = async () => {
    const entry = pendingUninstall
    if (!entry) return
    try {
      await uninstall(entry)
      setPendingUninstall(null)
    } catch (error) {
      const details = getSkillOperationErrorDetails(error)
      if (shouldRefreshSkillsAfterError(details)) await refresh()
      showToast(
        details.commitMayHaveSucceeded
          ? t('skills.operationNeedsConfirmation')
          : details.code === 'revisionConflict' || details.code === 'installationNotFound'
            ? t('skills.stateChanged')
            : details.message,
        { durationMs: 3200 }
      )
    }
  }

  const output = state.output
  const hasProtocolIssue = Boolean(
    output?.skills.some(
      (entry) =>
        (entry.actions.canUpdate || entry.actions.canUninstall) && !entry.installationRevision
    )
  )

  return (
    <article className="settings-list-page skills-settings-page">
      <header className="skills-settings-header">
        <div>
          <h1>{t('settings.page.skills')}</h1>
          <p className="settings-list-page__description">{t('skills.pageDescription')}</p>
        </div>
        <button className="skills-install-button" onClick={installation.startInstall} type="button">
          <WandSparkles aria-hidden="true" />
          <span>{t('skills.install')}</span>
        </button>
      </header>

      {state.status === 'loading' && (
        <div className="skills-page-state" role="status">
          <LoaderCircle aria-hidden="true" className="skills-page-spinner" />
          <span>{t('skills.loading')}</span>
        </div>
      )}

      {state.status === 'error' && (
        <div className="skills-page-state skills-page-state--error" role="alert">
          <AlertTriangle aria-hidden="true" />
          <div>
            <strong>{t('skills.loadFailed')}</strong>
            <p>{state.errorMessage}</p>
          </div>
          <button type="button" onClick={() => void refresh()}>
            <RefreshCw aria-hidden="true" />
            <span>{t('skills.retry')}</span>
          </button>
        </div>
      )}

      {output && (
        <>
          {(output.truncated ||
            output.diagnostics.length > 0 ||
            hasProtocolIssue ||
            state.errorMessage) && (
            <div className="skills-page-notices" role="status">
              <AlertTriangle aria-hidden="true" />
              <div>
                {output.truncated && <p>{t('skills.truncated')}</p>}
                {output.diagnostics.map((diagnostic, index) => (
                  <p key={`${diagnostic.code}-${diagnostic.skillId ?? index}`}>
                    {diagnostic.message}
                  </p>
                ))}
                {hasProtocolIssue && <p>{t('skills.protocolStateIncomplete')}</p>}
                {state.errorMessage && <p>{state.errorMessage}</p>}
              </div>
              <button
                aria-label={t('skills.refresh')}
                disabled={state.isRefreshing}
                onClick={() => void refresh()}
                type="button"
              >
                <RefreshCw aria-hidden="true" />
              </button>
            </div>
          )}

          {output.skills.length === 0 ? (
            <div className="skills-empty-state">
              <WandSparkles aria-hidden="true" />
              <strong>{t('skills.empty')}</strong>
              <p>{t('skills.emptyDescription')}</p>
            </div>
          ) : (
            <SkillManagementList
              entries={output.skills}
              onSetEnabled={(entry, enabled) => void changeEnabled(entry, enabled)}
              onUninstall={setPendingUninstall}
              onUpdate={(entry) => {
                if (!entry.installationRevision) {
                  showToast(t('skills.protocolStateIncomplete'))
                  void refresh()
                  return
                }
                installation.startUpdate(entry)
              }}
              pendingOperations={pendingOperations}
            />
          )}
        </>
      )}

      <SkillInstallationDialog workflow={installation} />

      {pendingUninstall && (
        <ConfirmationDialog
          cancelLabel={t('skills.cancel')}
          confirmLabel={t('skills.confirmUninstall')}
          description={t('skills.uninstallDescription')}
          onCancel={() => setPendingUninstall(null)}
          onConfirm={confirmUninstall}
          title={replaceTokens(t('skills.uninstallTitle'), { name: pendingUninstall.name })}
        />
      )}
    </article>
  )
}

function replaceTokens(template: string, values: Record<string, string>): string {
  return Object.entries(values).reduce(
    (current, [key, value]) => current.replaceAll(`{${key}}`, value),
    template
  )
}
