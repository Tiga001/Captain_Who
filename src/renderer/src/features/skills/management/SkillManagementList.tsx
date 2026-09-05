import { renderSettingsNodes, settingLabel } from '../../settings/settingsDefinition'
import {
  skillManagementActions,
  skillEnablementSettings
} from '../../settings/pages/managementSettings.definition'
// Renderer skills management UI: renders backend-authorized actions without inferring permissions.
import { AlertTriangle, LoaderCircle, RefreshCw, Trash2 } from 'lucide-react'
import type { SkillManagementEntry } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { SkillIcon } from '../SkillIcon'
import { getSkillPresentation, sortSkillsForDisplay } from '../skillPresentation'
import type { SkillRowPendingOperation } from './useSkillManagement'

interface SkillManagementListProps {
  entries: readonly SkillManagementEntry[]
  onSetEnabled: (entry: SkillManagementEntry, enabled: boolean) => void
  onUninstall: (entry: SkillManagementEntry) => void
  onUpdate: (entry: SkillManagementEntry, trigger: HTMLButtonElement) => void
  pendingOperations: ReadonlyMap<string, SkillRowPendingOperation>
}

export function SkillManagementList({
  entries,
  onSetEnabled,
  onUninstall,
  onUpdate,
  pendingOperations
}: SkillManagementListProps) {
  const { t } = useFrontendConfig()
  const orderedEntries = sortSkillsForDisplay(entries)

  return (
    <div className="skill-management-list">
      {orderedEntries.map((entry) => {
        const pendingOperation = pendingOperations.get(entry.id)
        const enablementBlocked =
          !entry.enabled && entry.enablementBlock === 'imageGenerationConfigurationRequired'
        const missingUpdateRevision = entry.actions.canUpdate && !entry.installationRevision
        const missingUninstallRevision = entry.actions.canUninstall && !entry.installationRevision
        const hasCompatibilityWarning =
          entry.compatibility.status !== 'compatible' || entry.compatibility.issues.length > 0
        const presentation = getSkillPresentation(entry, t)

        return (
          <article
            className="skill-management-row"
            key={entry.id}
            aria-busy={Boolean(pendingOperation)}
          >
            <SkillIcon
              className="skill-management-row__icon"
              skillId={entry.id}
              source={entry.source}
            />
            <div className="skill-management-row__content">
              <div className="skill-management-row__heading">
                <h2>{presentation.name}</h2>
                <span className="skill-source-badge">
                  {entry.source.kind === 'bundled'
                    ? t('skills.sourceBundled')
                    : t('skills.sourceInstalled')}
                </span>
              </div>
              {hasCompatibilityWarning && (
                <div className="skill-row-warning">
                  <AlertTriangle aria-hidden="true" />
                  <div>
                    <span>{t(`skills.compatibility.${entry.compatibility.status}`)}</span>
                    {entry.compatibility.issues.map((issue) => (
                      <small key={issue.id}>{issue.message}</small>
                    ))}
                  </div>
                </div>
              )}
              {(missingUpdateRevision || missingUninstallRevision) && (
                <div className="skill-row-warning" role="status">
                  <AlertTriangle aria-hidden="true" />
                  <span>{t('skills.missingInstallationRevision')}</span>
                </div>
              )}
            </div>

            <div className="skill-management-row__controls">
              <div className="skill-management-row__actions">
                {renderSettingsNodes(skillManagementActions, (node) => {
                  switch (node.id) {
                    case 'skills-update':
                      return entry.actions.canUpdate ? (
                        <button
                          aria-label={replaceTokens(t('skills.updateNamed'), {
                            name: presentation.name
                          })}
                          className="skill-row-action"
                          disabled={Boolean(pendingOperation) || missingUpdateRevision}
                          onClick={(event) => onUpdate(entry, event.currentTarget)}
                          type="button"
                        >
                          <RefreshCw aria-hidden="true" />
                          <span>{settingLabel(node, t)}</span>
                        </button>
                      ) : null
                    case 'skills-uninstall':
                      return entry.actions.canUninstall ? (
                        <button
                          aria-label={replaceTokens(t('skills.uninstallNamed'), {
                            name: presentation.name
                          })}
                          className="skill-row-action skill-row-action--danger"
                          disabled={Boolean(pendingOperation) || missingUninstallRevision}
                          onClick={() => onUninstall(entry)}
                          type="button"
                        >
                          <Trash2 aria-hidden="true" />
                          <span>{settingLabel(node, t)}</span>
                        </button>
                      ) : null
                    default:
                      return null
                  }
                })}
              </div>
              {pendingOperation && (
                <span
                  aria-label={
                    pendingOperation === 'enablement'
                      ? t('skills.savingEnablement')
                      : pendingOperation === 'update'
                        ? t('skills.updating')
                        : t('skills.uninstalling')
                  }
                  className="skill-row-pending"
                  role="status"
                >
                  <LoaderCircle aria-hidden="true" />
                </span>
              )}
              {renderSettingsNodes(skillEnablementSettings, (node) => (
                <button
                  aria-checked={entry.enabled}
                  aria-disabled={enablementBlocked || undefined}
                  aria-label={replaceTokens(settingLabel(node, t), {
                    name: presentation.name
                  })}
                  className="settings-switch"
                  data-state={entry.enabled ? 'on' : 'off'}
                  disabled={!entry.actions.canSetEnabled || Boolean(pendingOperation)}
                  onClick={() => onSetEnabled(entry, !entry.enabled)}
                  role="switch"
                  type="button"
                >
                  <span className="settings-switch__thumb" aria-hidden="true" />
                </button>
              ))}
            </div>
          </article>
        )
      })}
    </div>
  )
}

function replaceTokens(template: string, values: Record<string, string>): string {
  return Object.entries(values).reduce(
    (current, [key, value]) => current.replaceAll(`{${key}}`, value),
    template
  )
}
