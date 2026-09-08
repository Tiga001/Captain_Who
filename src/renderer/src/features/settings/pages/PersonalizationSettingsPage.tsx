import { renderSettingsNodes, settingLabel, settingDescription } from '../settingsDefinition'
import {
  personalizationSettingsNodes,
  WORK_MODE_OPTIONS,
  TONE_OPTIONS
} from './PersonalizationSettingsPage.definition'
import { useEffect, useMemo, useRef, useState } from 'react'
import { Check, ChevronDown, MessageCircle, Terminal } from 'lucide-react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import {
  defaultAgentPromptPreferences,
  loadAgentPromptPreferences,
  saveAgentPromptPreferences
} from '../../storage/storageClient'
import type { AgentPromptPreferencesSnapshot } from '../../storage/storageClient'
import { HumanInteractionSettingsSection } from './HumanInteractionSettingsSection'
import './PersonalizationSettingsPage.css'

type ImmediatePreferences = Partial<
  Pick<AgentPromptPreferencesSnapshot, 'contextProfile' | 'workMode' | 'tone'>
>

export function PersonalizationSettingsPage() {
  const { t } = useFrontendConfig()
  const [preferences, setPreferences] = useState<AgentPromptPreferencesSnapshot>(() =>
    defaultAgentPromptPreferences()
  )
  const [customInstructions, setCustomInstructions] = useState('')
  const [hasLoaded, setHasLoaded] = useState(false)
  const [isLoading, setIsLoading] = useState(true)
  const [isSaving, setIsSaving] = useState(false)
  const saveInFlight = useRef(false)
  const [statusMessage, setStatusMessage] = useState('')
  const [isToneOpen, setToneOpen] = useState(false)

  useEffect(() => {
    let isCancelled = false

    async function loadPreferences() {
      setIsLoading(true)
      setStatusMessage('')
      try {
        const loadedPreferences = await loadAgentPromptPreferences()
        if (isCancelled) return
        setPreferences(loadedPreferences)
        setCustomInstructions(loadedPreferences.customInstructions)
        setHasLoaded(true)
      } catch {
        if (!isCancelled) setStatusMessage(t('personalization.loadFailed'))
      } finally {
        if (!isCancelled) setIsLoading(false)
      }
    }

    void loadPreferences()

    return () => {
      isCancelled = true
    }
  }, [t])

  const selectedTone = useMemo(
    () => TONE_OPTIONS.find((option) => option.value === preferences.tone) ?? TONE_OPTIONS[1],
    [preferences.tone]
  )
  const isDirty = customInstructions !== preferences.customInstructions
  const controlsDisabled = isLoading || !hasLoaded || isSaving

  const persistPreferences = async (
    patch: ImmediatePreferences,
    submittedInstructions?: string
  ) => {
    if (isLoading || !hasLoaded || saveInFlight.current) return

    // This API stores the whole record. Serialize writes and keep the editor draft separate so
    // changing a switch cannot publish unsaved instructions or overwrite another pending save.
    saveInFlight.current = true
    setIsSaving(true)
    setStatusMessage('')
    try {
      const saved = await saveAgentPromptPreferences({
        contextProfile: preferences.contextProfile,
        workMode: preferences.workMode,
        tone: preferences.tone,
        ...patch,
        detailLevel: preferences.detailLevel || 'medium',
        customInstructions: submittedInstructions ?? preferences.customInstructions
      })
      setPreferences(saved)
      if (submittedInstructions !== undefined) {
        setCustomInstructions((current) =>
          current === submittedInstructions ? saved.customInstructions : current
        )
      }
    } catch {
      setStatusMessage(t('personalization.saveFailed'))
    } finally {
      saveInFlight.current = false
      setIsSaving(false)
    }
  }

  return (
    <article className="settings-list-page personalization-settings-page">
      <h1>{t('settings.page.personalization')}</h1>
      {statusMessage && (
        <p className="personalization-status personalization-settings-status" role="alert">
          {statusMessage}
        </p>
      )}

      {renderSettingsNodes(personalizationSettingsNodes, (node) => {
        switch (node.id) {
          case 'personalization.workMode':
            return (
              <section
                className="personalization-work-mode"
                aria-labelledby="personalization-work-mode-heading"
              >
                <div className="personalization-section-heading">
                  <h2 id="personalization-work-mode-heading">{settingLabel(node, t)}</h2>
                  <p>{settingDescription(node, t)}</p>
                </div>

                <div className="personalization-work-mode__grid">
                  {WORK_MODE_OPTIONS.map((option) => {
                    const Icon = option.value === 'coding' ? Terminal : MessageCircle
                    const isSelected = preferences.workMode === option.value

                    return (
                      <button
                        className="personalization-work-mode-card"
                        data-selected={isSelected || undefined}
                        type="button"
                        key={option.value}
                        disabled={controlsDisabled}
                        onClick={() => {
                          if (!isSelected) void persistPreferences({ workMode: option.value })
                        }}
                      >
                        <Icon aria-hidden="true" />
                        <span className="personalization-work-mode-card__text">
                          <strong>{t(option.titleKey)}</strong>
                          <span>{t(option.descriptionKey)}</span>
                        </span>
                        <span
                          className="personalization-radio"
                          data-selected={isSelected || undefined}
                          aria-hidden="true"
                        />
                      </button>
                    )
                  })}
                </div>
                <div className="settings-list personalization-minimal-mode">
                  {renderSettingsNodes(node.children, (setting) => (
                    <div className="settings-list-row personalization-minimal-mode__row">
                      <span className="settings-list-row__text">
                        <span className="settings-list-row__title" id="minimal-mode-label">
                          {settingLabel(setting, t)}
                        </span>
                        <span
                          className="settings-list-row__description"
                          id="minimal-mode-description"
                        >
                          {settingDescription(setting, t)}
                        </span>
                      </span>
                      <button
                        className="settings-switch"
                        type="button"
                        role="switch"
                        aria-labelledby="minimal-mode-label"
                        aria-describedby="minimal-mode-description"
                        aria-checked={preferences.contextProfile === 'minimal'}
                        data-state={preferences.contextProfile === 'minimal' ? 'on' : 'off'}
                        disabled={controlsDisabled}
                        onClick={() =>
                          void persistPreferences({
                            contextProfile:
                              preferences.contextProfile === 'minimal' ? 'full' : 'minimal'
                          })
                        }
                      >
                        <span className="settings-switch__thumb" aria-hidden="true" />
                      </button>
                    </div>
                  ))}
                </div>
              </section>
            )
          case 'personalization.tone':
            return (
              <section
                className="settings-list-section personalization-tone-section"
                aria-labelledby="personalization-tone-heading"
              >
                <div className="settings-list personalization-tone-list">
                  <div className="settings-list-row personalization-tone-row">
                    <span className="settings-list-row__text">
                      <span className="settings-list-row__title" id="personalization-tone-heading">
                        {settingLabel(node, t)}
                      </span>
                      <span className="settings-list-row__description">
                        {settingDescription(node, t)}
                      </span>
                    </span>

                    <span className="settings-list-row__control personalization-tone-control">
                      <button
                        className="personalization-tone-button"
                        type="button"
                        aria-haspopup="listbox"
                        aria-expanded={isToneOpen}
                        disabled={controlsDisabled}
                        onClick={() => setToneOpen((current) => !current)}
                      >
                        <span>{t(selectedTone.titleKey)}</span>
                        <ChevronDown aria-hidden="true" />
                      </button>

                      {isToneOpen && (
                        <div
                          className="personalization-tone-menu"
                          role="listbox"
                          aria-label={settingLabel(node, t)}
                        >
                          {TONE_OPTIONS.map((option) => {
                            const isSelected = preferences.tone === option.value
                            return (
                              <button
                                className="personalization-tone-option"
                                data-selected={isSelected || undefined}
                                type="button"
                                role="option"
                                aria-selected={isSelected}
                                key={option.value}
                                disabled={controlsDisabled}
                                onClick={() => {
                                  if (!isSelected) void persistPreferences({ tone: option.value })
                                  setToneOpen(false)
                                }}
                              >
                                <span>
                                  <strong>{t(option.titleKey)}</strong>
                                  <small>{t(option.descriptionKey)}</small>
                                </span>
                                {isSelected && <Check aria-hidden="true" />}
                              </button>
                            )
                          })}
                        </div>
                      )}
                    </span>
                  </div>
                </div>
              </section>
            )
          case 'personalization.customInstructions':
            return (
              <section
                className="personalization-custom-instructions"
                aria-labelledby="personalization-custom-heading"
              >
                <div className="personalization-section-heading">
                  <div className="personalization-custom-instructions__heading-row">
                    <h2 id="personalization-custom-heading">{settingLabel(node, t)}</h2>
                    <button
                      className="personalization-save-button"
                      type="button"
                      disabled={controlsDisabled || !isDirty}
                      onClick={() => void persistPreferences({}, customInstructions)}
                    >
                      {t('personalization.save')}
                    </button>
                  </div>
                  <p>{settingDescription(node, t)}</p>
                </div>

                <textarea
                  className="personalization-custom-instructions__textarea"
                  value={customInstructions}
                  disabled={isLoading || !hasLoaded}
                  aria-labelledby="personalization-custom-heading"
                  placeholder={t('personalization.customInstructionsPlaceholder')}
                  onChange={(event) => {
                    setStatusMessage('')
                    setCustomInstructions(event.target.value)
                  }}
                />
              </section>
            )
          case 'personalization.humanInteraction':
            return <HumanInteractionSettingsSection />
          default:
            return null
        }
      })}
    </article>
  )
}
