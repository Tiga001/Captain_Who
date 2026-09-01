import { X } from 'lucide-react'
import { useEffect, useId, useRef, useState, type ComponentType } from 'react'
import { createPortal } from 'react-dom'
import type { LegacyProviderReasoningEffort, ProviderReasoningMode } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { SettingsSelect, type SettingsSelectOption } from '../../components/SettingsSelect'

export interface DeepSeekProviderSettingsDraft {
  reasoning: {
    mode: ProviderReasoningMode
    effort: LegacyProviderReasoningEffort
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
  const thinkingModeOptions: ReadonlyArray<SettingsSelectOption<ProviderReasoningMode>> = [
    {
      value: 'provider_default',
      label: t('configuration.deepSeekSettings.thinkingProviderDefault')
    },
    { value: 'enabled', label: t('configuration.deepSeekSettings.thinkingEnabled') },
    { value: 'disabled', label: t('configuration.deepSeekSettings.thinkingDisabled') }
  ]
  const reasoningEffortOptions: ReadonlyArray<SettingsSelectOption<LegacyProviderReasoningEffort>> =
    [
      {
        value: 'provider_default',
        label: t('configuration.deepSeekSettings.effortProviderDefault')
      },
      { value: 'high', label: t('configuration.deepSeekSettings.effortHigh') },
      { value: 'max', label: t('configuration.deepSeekSettings.effortMax') }
    ]

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
      if (event.defaultPrevented) return
      if (event.key === 'Escape') {
        if (cardRef.current?.querySelector('.settings-select[data-open="true"]')) return
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
          <div className="provider-settings-dialog__field">
            <span>{t('configuration.deepSeekSettings.thinkingMode')}</span>
            <SettingsSelect
              ariaLabel={t('configuration.deepSeekSettings.thinkingMode')}
              className="provider-settings-dialog__select"
              options={thinkingModeOptions}
              value={settings.reasoning.mode}
              onChange={updateMode}
            />
          </div>

          <div className="provider-settings-dialog__field">
            <span>{t('configuration.deepSeekSettings.reasoningEffort')}</span>
            <SettingsSelect
              ariaLabel={t('configuration.deepSeekSettings.reasoningEffort')}
              className="provider-settings-dialog__select"
              options={reasoningEffortOptions}
              value={settings.reasoning.effort}
              disabled={settings.reasoning.mode === 'disabled'}
              onChange={(effort) =>
                setSettings((current) => ({
                  reasoning: {
                    ...current.reasoning,
                    effort
                  }
                }))
              }
            />
          </div>
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
