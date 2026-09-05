import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { useHumanInteractionSettings } from './useHumanInteractionSettings'

export function HumanInteractionSettingsSection() {
  const { t } = useFrontendConfig()
  const { settings, loading, saving, error, refresh, toggle } = useHumanInteractionSettings()

  return (
    <section
      className="settings-list-section personalization-human-interaction"
      aria-labelledby="human-interaction-settings-heading"
    >
      <div className="personalization-section-heading">
        <h2 id="human-interaction-settings-heading">{t('humanInteraction.settings.title')}</h2>
      </div>
      <div className="settings-list">
        <div className="settings-list-row personalization-human-interaction__row">
          <div className="settings-list-row__text">
            <span className="settings-list-row__title" id="human-interaction-settings-label">
              {t('humanInteraction.settings.allowQuestions')}
            </span>
            <span
              className="settings-list-row__description"
              id="human-interaction-settings-description"
            >
              {t('humanInteraction.settings.description')}
            </span>
          </div>
          {settings ? (
            <button
              className="settings-switch"
              type="button"
              role="switch"
              aria-labelledby="human-interaction-settings-label"
              aria-describedby="human-interaction-settings-description"
              aria-checked={settings.enabled}
              aria-busy={saving || undefined}
              data-state={settings.enabled ? 'on' : 'off'}
              disabled={loading || saving}
              onClick={() => void toggle()}
            >
              <span className="settings-switch__thumb" aria-hidden="true" />
            </button>
          ) : loading ? (
            <span className="personalization-human-interaction__status" role="status">
              {t('humanInteraction.settings.loading')}
            </span>
          ) : null}
        </div>
      </div>
      {saving && (
        <p className="personalization-human-interaction__status" role="status">
          {t('humanInteraction.settings.saving')}
        </p>
      )}
      {error && (
        <div className="personalization-human-interaction__error">
          <p className="personalization-status" role="alert">
            {t(
              error === 'save'
                ? 'humanInteraction.settings.saveFailed'
                : 'humanInteraction.settings.loadFailed'
            )}
          </p>
          <button
            className="personalization-human-interaction__retry"
            type="button"
            disabled={loading || saving}
            onClick={() => void refresh()}
          >
            {t('humanInteraction.settings.retry')}
          </button>
        </div>
      )}
    </section>
  )
}
