import { renderSettingsNodes, settingLabel } from '../settingsDefinition'
import { humanInteractionSettingsNodes } from './PersonalizationSettingsPage.definition'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { useHumanInteractionSettings } from './useHumanInteractionSettings'

export function HumanInteractionSettingsSection() {
  const { t } = useFrontendConfig()
  const { settings, loading, saving, error, refresh, toggle } = useHumanInteractionSettings()

  return renderSettingsNodes(humanInteractionSettingsNodes, (section) => (
    <section
      className="settings-list-section personalization-human-interaction"
      aria-labelledby="human-interaction-settings-heading"
    >
      <div className="personalization-section-heading">
        <h2 id="human-interaction-settings-heading">{settingLabel(section, t)}</h2>
      </div>
      <div className="settings-list">
        {renderSettingsNodes(section.children, (node) => (
          <div className="settings-list-row personalization-human-interaction__row">
            <div className="settings-list-row__text">
              <span className="settings-list-row__title" id="human-interaction-settings-label">
                {settingLabel(node, t)}
              </span>
            </div>
            {settings ? (
              <button
                className="settings-switch"
                type="button"
                role="switch"
                aria-labelledby="human-interaction-settings-label"
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
        ))}
      </div>
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
  ))
}
