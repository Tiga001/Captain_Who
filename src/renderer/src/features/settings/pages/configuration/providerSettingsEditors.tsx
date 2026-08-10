import { X } from 'lucide-react'
import { useEffect, useId, useRef, useState, type ComponentType } from 'react'
import { createPortal } from 'react-dom'
import type { ProviderReasoningEffort, ProviderReasoningMode } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'

export interface DeepSeekProviderSettingsDraft {
  reasoning: {
    mode: ProviderReasoningMode
    effort: ProviderReasoningEffort
  }
}

export interface ProviderSettingsEditorProps {
  initialSettings: DeepSeekProviderSettingsDraft
  onCancel: () => void
  onConfirm: (settings: DeepSeekProviderSettingsDraft) => void
}

export type ProviderSettingsEditorKind = 'deepseek_v4_chat'

function normalizeDeepSeekSettings(
  settings: DeepSeekProviderSettingsDraft
): DeepSeekProviderSettingsDraft {
  if (settings.reasoning.mode !== 'disabled') return settings
  return {
    reasoning: {
      mode: 'disabled',
      effort: 'provider_default'
    }
  }
}

export function DeepSeekProviderSettingsEditor({
  initialSettings,
  onCancel,
  onConfirm
}: ProviderSettingsEditorProps) {
  const { t } = useFrontendConfig()
  const titleId = useId()
  const descriptionId = useId()
  const cardRef = useRef<HTMLElement>(null)
  const cancelButtonRef = useRef<HTMLButtonElement>(null)
  const previouslyFocusedRef = useRef<HTMLElement | null>(null)
  const [settings, setSettings] = useState<DeepSeekProviderSettingsDraft>(() => ({
    reasoning: { ...initialSettings.reasoning }
  }))

  useEffect(() => {
    previouslyFocusedRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null
    const frameId = window.requestAnimationFrame(() => cancelButtonRef.current?.focus())

    return () => {
      window.cancelAnimationFrame(frameId)
      if (previouslyFocusedRef.current?.isConnected) previouslyFocusedRef.current.focus()
    }
  }, [])

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        onCancel()
        return
      }
      if (event.key !== 'Tab') return

      const focusable = Array.from(
        cardRef.current?.querySelectorAll<HTMLElement>(
          'button:not(:disabled), select:not(:disabled)'
        ) ?? []
      )
      if (focusable.length === 0) return
      const first = focusable[0]
      const last = focusable[focusable.length - 1]
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first.focus()
      }
    }

    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [onCancel])

  const updateMode = (mode: ProviderReasoningMode) => {
    setSettings((current) =>
      normalizeDeepSeekSettings({
        reasoning: { ...current.reasoning, mode }
      })
    )
  }

  return createPortal(
    <div
      className="provider-settings-dialog__backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target) onCancel()
      }}
    >
      <section
        ref={cardRef}
        className="provider-settings-dialog__card"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
      >
        <button
          className="provider-settings-dialog__close"
          type="button"
          aria-label={t('configuration.providerSettings.cancel')}
          onClick={onCancel}
        >
          <X aria-hidden="true" />
        </button>

        <h2 id={titleId}>{t('configuration.deepSeekSettings.title')}</h2>
        <p id={descriptionId} className="provider-settings-dialog__intro">
          {t('configuration.deepSeekSettings.defaultThinkingDescription')}
        </p>

        <div className="provider-settings-dialog__fields">
          <label className="provider-settings-dialog__field">
            <span>{t('configuration.deepSeekSettings.thinkingMode')}</span>
            <select
              aria-label={t('configuration.deepSeekSettings.thinkingMode')}
              value={settings.reasoning.mode}
              onChange={(event) => updateMode(event.target.value as ProviderReasoningMode)}
            >
              <option value="provider_default">
                {t('configuration.deepSeekSettings.thinkingProviderDefault')}
              </option>
              <option value="enabled">{t('configuration.deepSeekSettings.thinkingEnabled')}</option>
              <option value="disabled">
                {t('configuration.deepSeekSettings.thinkingDisabled')}
              </option>
            </select>
          </label>

          <label className="provider-settings-dialog__field">
            <span>{t('configuration.deepSeekSettings.reasoningEffort')}</span>
            <select
              aria-label={t('configuration.deepSeekSettings.reasoningEffort')}
              value={settings.reasoning.effort}
              disabled={settings.reasoning.mode === 'disabled'}
              onChange={(event) =>
                setSettings((current) => ({
                  reasoning: {
                    ...current.reasoning,
                    effort: event.target.value as ProviderReasoningEffort
                  }
                }))
              }
            >
              <option value="provider_default">
                {t('configuration.deepSeekSettings.effortProviderDefault')}
              </option>
              <option value="high">{t('configuration.deepSeekSettings.effortHigh')}</option>
              <option value="max">{t('configuration.deepSeekSettings.effortMax')}</option>
            </select>
          </label>
        </div>

        <div className="provider-settings-dialog__actions">
          <button
            ref={cancelButtonRef}
            className="secondary-settings-button"
            type="button"
            onClick={onCancel}
          >
            {t('configuration.providerSettings.cancel')}
          </button>
          <button
            className="primary-settings-button"
            type="button"
            onClick={() => onConfirm(normalizeDeepSeekSettings(settings))}
          >
            {t('configuration.providerSettings.confirm')}
          </button>
        </div>
      </section>
    </div>,
    document.body
  )
}

export const providerSettingsEditors = {
  deepseek_v4_chat: DeepSeekProviderSettingsEditor
} as const satisfies Record<ProviderSettingsEditorKind, ComponentType<ProviderSettingsEditorProps>>
