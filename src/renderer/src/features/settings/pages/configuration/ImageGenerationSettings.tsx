// Renderer settings UI for the backend-owned image-generation provider profile.
import { AlertTriangle, ImagePlus, LoaderCircle, RefreshCw } from 'lucide-react'
import type { ImageGenerationConfigurationErrorCode } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import { useImageGenerationConfiguration } from '../../../imageGeneration/configuration/useImageGenerationConfiguration'
import { SecretInput } from './SecretInput'

const STORED_SECRET_PLACEHOLDER = '\u2022'.repeat(18)

const ERROR_KEY: Partial<Record<ImageGenerationConfigurationErrorCode, TranslationKey>> = {
  revisionConflict: 'configuration.imageGeneration.error.revisionConflict',
  unsupportedAdapter: 'configuration.imageGeneration.error.unsupportedAdapter',
  missingEndpoint: 'configuration.imageGeneration.error.missingEndpoint',
  invalidEndpoint: 'configuration.imageGeneration.error.invalidEndpoint',
  insecureEndpoint: 'configuration.imageGeneration.error.insecureEndpoint',
  missingModel: 'configuration.imageGeneration.error.missingModel',
  invalidModelId: 'configuration.imageGeneration.error.invalidModelId',
  missingCredential: 'configuration.imageGeneration.error.missingCredential',
  invalidCredential: 'configuration.imageGeneration.error.invalidCredential',
  credentialReplacementRequired:
    'configuration.imageGeneration.error.credentialReplacementRequired',
  credentialStoreUnavailable: 'configuration.imageGeneration.error.credentialStoreUnavailable',
  storageUnavailable: 'configuration.imageGeneration.error.storageUnavailable',
  commitIndeterminate: 'configuration.imageGeneration.error.commitIndeterminate',
  unavailable: 'configuration.imageGeneration.error.unavailable'
}

function feedbackText(
  feedback: NonNullable<
    Extract<
      ReturnType<typeof useImageGenerationConfiguration>['state'],
      { status: 'ready' }
    >['feedback']
  >,
  t: ReturnType<typeof useFrontendConfig>['t']
) {
  if (feedback.kind === 'saved') return t('configuration.imageGeneration.saved')
  if (feedback.kind === 'authoritativeRefresh') {
    return feedback.code === 'commitIndeterminate'
      ? t('configuration.imageGeneration.error.commitIndeterminateRefreshed')
      : t('configuration.imageGeneration.error.authoritativeRefresh')
  }
  return t(
    ERROR_KEY[feedback.code ?? 'invalidRequest'] ?? 'configuration.imageGeneration.error.generic'
  )
}

function SettingsSwitch({
  ariaLabel,
  checked,
  disabled,
  onClick
}: {
  ariaLabel: string
  checked: boolean
  disabled?: boolean
  onClick?: () => void
}) {
  return (
    <button
      aria-checked={checked}
      aria-label={ariaLabel}
      className="settings-switch"
      data-state={checked ? 'on' : 'off'}
      disabled={disabled}
      onClick={onClick}
      role="switch"
      type="button"
    >
      <span className="settings-switch__thumb" aria-hidden="true" />
    </button>
  )
}

export function ImageGenerationSettings() {
  const { t } = useFrontendConfig()
  const workflow = useImageGenerationConfiguration()
  const { state } = workflow

  if (state.status === 'loading') {
    return (
      <section
        aria-labelledby="image-generation-heading"
        className="configuration-section image-generation-settings settings-list-page"
      >
        <h1 id="image-generation-heading">{t('configuration.imageGeneration.title')}</h1>
        <div className="image-generation-settings__skeleton" role="status">
          <LoaderCircle aria-hidden="true" />
          <span>{t('configuration.imageGeneration.loading')}</span>
        </div>
      </section>
    )
  }

  if (state.status === 'error') {
    return (
      <section
        aria-labelledby="image-generation-heading"
        className="configuration-section image-generation-settings settings-list-page"
      >
        <h1 id="image-generation-heading">{t('configuration.imageGeneration.title')}</h1>
        <div className="image-generation-settings__error" role="alert">
          <AlertTriangle aria-hidden="true" />
          <span>
            {t(
              ERROR_KEY[state.code ?? 'invalidRequest'] ??
                'configuration.imageGeneration.error.loadFailed'
            )}
          </span>
          <button onClick={() => void workflow.load()} type="button">
            <RefreshCw aria-hidden="true" />
            <span>{t('configuration.imageGeneration.retry')}</span>
          </button>
        </div>
      </section>
    )
  }

  const { configuration, form, apiKeyDraft, feedback, pendingOperation } = state
  const disabled = Boolean(pendingOperation)

  return (
    <section
      aria-labelledby="image-generation-heading"
      className="configuration-section image-generation-settings settings-list-page"
    >
      <div className="image-generation-settings__heading">
        <div>
          <h1 id="image-generation-heading">{t('configuration.imageGeneration.title')}</h1>
          <p className="settings-list-page__description">
            {t('configuration.imageGeneration.description')}
          </p>
        </div>
        <ImagePlus aria-hidden="true" />
      </div>

      <div className="settings-list-section">
        <div className="settings-list">
          <div className="settings-list-row">
            <div className="settings-list-row__text">
              <h2 className="settings-list-row__title">
                {t('configuration.imageGeneration.enabled')}
              </h2>
            </div>
            <SettingsSwitch
              ariaLabel={t('configuration.imageGeneration.enabled')}
              checked={configuration.enabled}
              disabled={disabled}
              onClick={() => void workflow.setEnabled(!configuration.enabled)}
            />
          </div>

          <label className="configuration-field settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">
                {t('configuration.imageGeneration.endpointUrl')}
              </span>
            </span>
            <span className="settings-list-row__control">
              <input
                className="settings-list-control"
                disabled={disabled}
                inputMode="url"
                onChange={(event) => workflow.updateForm('endpointUrl', event.target.value)}
                placeholder={t('configuration.imageGeneration.endpointUrlPlaceholder')}
                spellCheck={false}
                type="url"
                value={form.endpointUrl}
              />
            </span>
          </label>

          <div className="configuration-field settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">
                {t('configuration.imageGeneration.apiKey')}
              </span>
            </span>
            <span className="settings-list-row__control">
              <SecretInput
                ariaLabel={t('configuration.imageGeneration.apiKey')}
                disabled={disabled}
                onChange={workflow.setApiKeyDraft}
                placeholder={
                  configuration.credentialStatus === 'configured'
                    ? STORED_SECRET_PLACEHOLDER
                    : undefined
                }
                value={apiKeyDraft}
              />
            </span>
          </div>

          <label className="configuration-field settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">
                {t('configuration.imageGeneration.modelId')}
              </span>
            </span>
            <span className="settings-list-row__control">
              <input
                className="settings-list-control"
                disabled={disabled}
                onChange={(event) => workflow.updateForm('modelId', event.target.value)}
                placeholder={t('configuration.imageGeneration.modelIdPlaceholder')}
                spellCheck={false}
                value={form.modelId}
              />
            </span>
          </label>

          <div className="settings-list-row">
            <div className="settings-list-row__text">
              <h2 className="settings-list-row__title">
                {t('configuration.imageGeneration.textToImage')}
              </h2>
            </div>
            <SettingsSwitch
              ariaLabel={t('configuration.imageGeneration.textToImage')}
              checked
              disabled
            />
          </div>

          <div className="settings-list-row">
            <div className="settings-list-row__text">
              <h2 className="settings-list-row__title">
                {t('configuration.imageGeneration.imageToImage')}
              </h2>
            </div>
            <SettingsSwitch
              ariaLabel={t('configuration.imageGeneration.imageToImage')}
              checked={form.imageToImage}
              disabled={disabled}
              onClick={() => workflow.updateForm('imageToImage', !form.imageToImage)}
            />
          </div>

          <div className="settings-list-row">
            <div className="settings-list-row__text">
              <h2 className="settings-list-row__title">
                {t('configuration.imageGeneration.watermark')}
              </h2>
            </div>
            <SettingsSwitch
              ariaLabel={t('configuration.imageGeneration.watermark')}
              checked={form.watermark}
              disabled={disabled}
              onClick={() => workflow.updateForm('watermark', !form.watermark)}
            />
          </div>
        </div>
      </div>

      <div className="image-generation-settings__footer">
        <div aria-live="polite" className="image-generation-settings__feedback">
          {feedback ? feedbackText(feedback, t) : null}
        </div>
        <button
          className="primary-settings-button"
          disabled={disabled}
          onClick={() => void workflow.save()}
          type="button"
        >
          {pendingOperation === 'saving' ? (
            <LoaderCircle aria-hidden="true" className="image-generation-settings__spinner" />
          ) : null}
          <span>{t('configuration.imageGeneration.save')}</span>
        </button>
      </div>
    </section>
  )
}
