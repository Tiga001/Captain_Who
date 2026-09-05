import { renderSettingsNodes, settingLabel } from '../../settingsDefinition'
import { modelConfigurationSection } from './configuration.definition'
import { Check, CircleHelp } from 'lucide-react'
import { useEffect, useState } from 'react'
import type { CredentialMutation, CredentialStatus } from '@mycopilot/protocol'
import { ConfirmationDialog } from '../../../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import type { ModelConfig } from './configurationTypes'
import { formatContextWindow } from './modelPresentation'
import { formatModelConfigLabel } from '../../../modelSelection/modelConfigPresentation'
import { CredentialInput } from './CredentialInput'

interface ModelProviderSettingsProps {
  definition?: typeof modelConfigurationSection
  apiUrl: string
  apiTokenStatus: CredentialStatus
  models: ModelConfig[]
  onApiTokenCommit: (mutation: CredentialMutation) => Promise<void>
  onApiUrlChange: (value: string) => Promise<void>
  onManageModels: () => void
  onToggleModel: (modelId: string) => void
}

export function ModelProviderSettings({
  definition = modelConfigurationSection,
  apiUrl,
  apiTokenStatus,
  models,
  onApiTokenCommit,
  onApiUrlChange,
  onManageModels,
  onToggleModel
}: ModelProviderSettingsProps) {
  const { t } = useFrontendConfig()
  const [apiUrlDraft, setApiUrlDraft] = useState(apiUrl)
  const [isApiUrlCommitPending, setApiUrlCommitPending] = useState(false)
  const [apiTokenMutation, setApiTokenMutation] = useState<CredentialMutation>({ type: 'keep' })
  const [isDefaultApiHelpOpen, setDefaultApiHelpOpen] = useState(false)
  const helpDescription = `${t('configuration.modelSettingsHelp.description')} ${t(
    'configuration.modelSettingsHelp.note'
  )}`

  useEffect(() => {
    if (!isApiUrlCommitPending) setApiUrlDraft(apiUrl)
  }, [apiUrl, isApiUrlCommitPending])

  const commitApiUrl = async () => {
    if (isApiUrlCommitPending || apiUrlDraft === apiUrl) return
    setApiUrlCommitPending(true)
    try {
      await onApiUrlChange(apiUrlDraft)
    } catch {
      setApiUrlDraft(apiUrl)
    } finally {
      setApiUrlCommitPending(false)
    }
  }

  return (
    <section
      className="configuration-section settings-list-page"
      aria-labelledby="model-settings-heading"
      data-setting-id={definition.id}
    >
      <h1 id="model-settings-heading">{settingLabel(definition, t)}</h1>

      {renderSettingsNodes(definition.children, (node) => {
        switch (node.id) {
          case 'configuration.defaultApi':
            return (
              <div className="configuration-form-block settings-list-section">
                <div className="model-settings-heading">
                  <h2>{settingLabel(node, t)}</h2>
                  <button
                    className="model-settings-help-button"
                    type="button"
                    aria-expanded={isDefaultApiHelpOpen}
                    aria-haspopup="dialog"
                    aria-label={t('configuration.modelSettingsHelp.open')}
                    title={t('configuration.modelSettingsHelp.open')}
                    onClick={() => setDefaultApiHelpOpen(true)}
                  >
                    <CircleHelp aria-hidden="true" />
                  </button>
                </div>

                <div className="settings-list">
                  {renderSettingsNodes(node.children, (node) => {
                    switch (node.id) {
                      case 'configuration.apiUrl':
                        return (
                          <label className="configuration-field settings-list-row">
                            <span className="settings-list-row__text">
                              <span className="settings-list-row__title">
                                {settingLabel(node, t)}
                              </span>
                            </span>
                            <span className="settings-list-row__control">
                              <input
                                aria-label={settingLabel(node, t)}
                                className="settings-list-control"
                                disabled={isApiUrlCommitPending}
                                type="url"
                                value={apiUrlDraft}
                                onBlur={() => void commitApiUrl()}
                                onChange={(event) => setApiUrlDraft(event.target.value)}
                                onKeyDown={(event) => {
                                  if (event.key === 'Enter') {
                                    event.preventDefault()
                                    event.currentTarget.blur()
                                  }
                                }}
                              />
                            </span>
                          </label>
                        )
                      case 'configuration.apiToken':
                        return (
                          <div className="configuration-field settings-list-row">
                            <span className="settings-list-row__text">
                              <span className="settings-list-row__title">
                                {settingLabel(node, t)}
                              </span>
                            </span>
                            <span className="settings-list-row__control">
                              <CredentialInput
                                ariaLabel={settingLabel(node, t)}
                                mutation={apiTokenMutation}
                                onCommit={async (mutation) => {
                                  await onApiTokenCommit(mutation)
                                  setApiTokenMutation({ type: 'keep' })
                                }}
                                onMutationChange={setApiTokenMutation}
                                placeholder={t('configuration.credential.placeholder')}
                                status={apiTokenStatus}
                              />
                            </span>
                          </div>
                        )
                    }
                  })}
                </div>
              </div>
            )
          case 'configuration.models':
            return (
              <div className="available-models-heading settings-list-section__header">
                <h2>{settingLabel(node, t)}</h2>
                <button
                  className="secondary-settings-button"
                  type="button"
                  onClick={onManageModels}
                >
                  {t('configuration.manageModels')}
                </button>
              </div>
            )
        }
      })}

      <div className="available-model-list" aria-label={t('configuration.availableModelList')}>
        {models.map((model) => (
          <label className="available-model-row" key={model.id}>
            <input
              type="checkbox"
              checked={model.enabled}
              onChange={() => onToggleModel(model.id)}
              aria-label={`${t('configuration.enableModelPrefix')} ${formatModelConfigLabel(model)}`}
            />
            <span className="available-model-row__check" aria-hidden="true">
              <Check />
            </span>
            <span className="available-model-row__name">{formatModelConfigLabel(model)}</span>
            <span className="available-model-row__metadata">
              <span
                className="model-context-pill"
                title={
                  model.contextWindowTokens
                    ? `${model.contextWindowTokens.toLocaleString()} tokens`
                    : t('configuration.contextNotConfigured')
                }
              >
                {formatContextWindow(model.contextWindowTokens)}
              </span>
              <span
                className="model-capability-pill"
                data-supported={model.supportsImage || undefined}
              >
                {model.supportsImage ? t('configuration.image') : t('configuration.text')}
              </span>
            </span>
          </label>
        ))}
      </div>

      {isDefaultApiHelpOpen && (
        <ConfirmationDialog
          cancelLabel={t('configuration.modelSettingsHelp.close')}
          confirmLabel={t('configuration.modelSettingsHelp.acknowledge')}
          confirmVariant="primary"
          description={helpDescription}
          onCancel={() => setDefaultApiHelpOpen(false)}
          onConfirm={() => setDefaultApiHelpOpen(false)}
          showCancelButton={false}
          title={t('configuration.modelSettingsHelp.title')}
        />
      )}
    </section>
  )
}
