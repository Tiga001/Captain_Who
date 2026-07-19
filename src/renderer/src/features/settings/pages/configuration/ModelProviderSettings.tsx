import { Check } from 'lucide-react'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import type { ModelConfig } from './configurationTypes'
import { formatContextWindow } from './modelPresentation'
import { SecretInput } from './SecretInput'

interface ModelProviderSettingsProps {
  apiUrl: string
  apiToken: string
  models: ModelConfig[]
  onApiTokenChange: (value: string) => void
  onApiUrlChange: (value: string) => void
  onManageModels: () => void
  onToggleModel: (modelId: string) => void
}

export function ModelProviderSettings({
  apiUrl,
  apiToken,
  models,
  onApiTokenChange,
  onApiUrlChange,
  onManageModels,
  onToggleModel
}: ModelProviderSettingsProps) {
  const { t } = useFrontendConfig()

  return (
    <section
      className="configuration-section settings-list-page"
      aria-labelledby="model-settings-heading"
    >
      <h1 id="model-settings-heading">{t('configuration.model')}</h1>

      <div className="configuration-form-block settings-list-section">
        <h2>{t('configuration.modelSettings')}</h2>

        <div className="settings-list">
          <label className="configuration-field settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">API URL</span>
            </span>
            <span className="settings-list-row__control">
              <input
                className="settings-list-control"
                type="url"
                value={apiUrl}
                onChange={(event) => onApiUrlChange(event.target.value)}
              />
            </span>
          </label>

          <div className="configuration-field settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">API Token</span>
            </span>
            <span className="settings-list-row__control">
              <SecretInput ariaLabel="API Token" value={apiToken} onChange={onApiTokenChange} />
            </span>
          </div>
        </div>
      </div>

      <div className="available-models-heading settings-list-section__header">
        <h2>{t('configuration.availableModels')}</h2>
        <button className="secondary-settings-button" type="button" onClick={onManageModels}>
          {t('configuration.manageModels')}
        </button>
      </div>

      <div className="available-model-list" aria-label={t('configuration.availableModelList')}>
        {models.map((model) => (
          <label className="available-model-row" key={model.id}>
            <input
              type="checkbox"
              checked={model.enabled}
              onChange={() => onToggleModel(model.id)}
              aria-label={`${t('configuration.enableModelPrefix')} ${model.displayName}`}
            />
            <span className="available-model-row__check" aria-hidden="true">
              <Check />
            </span>
            <span className="available-model-row__name">{model.displayName}</span>
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
    </section>
  )
}
