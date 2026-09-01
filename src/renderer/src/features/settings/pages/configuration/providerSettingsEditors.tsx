import { useState } from 'react'
import type {
  ProviderFamilySettingsDescriptor,
  ProviderReasoningEffort,
  ProviderReasoningMode
} from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { SettingsSelect, type SettingsSelectOption } from '../../components/SettingsSelect'
import type { DeepSeekFamilySettings, MoonshotFamilySettings } from './providerProfileForm'
import { ProviderSettingsDialogShell } from './ProviderSettingsDialogShell'

type DeepSeekDescriptor = Extract<
  ProviderFamilySettingsDescriptor,
  { kind: 'deepseek_v4_chat' | 'deepseek_v4_vision' }
>
type MoonshotDescriptor = Extract<
  ProviderFamilySettingsDescriptor,
  { kind: 'moonshot_k3_chat' | 'moonshot_k2_7_code_chat' | 'moonshot_k2_6_chat' }
>

interface DeepSeekProviderSettingsEditorProps {
  descriptor: DeepSeekDescriptor
  initialSettings: DeepSeekFamilySettings
  onCancel: () => void
  onConfirm: (settings: DeepSeekFamilySettings) => void
}

function normalizeDeepSeekDraft(
  settings: DeepSeekFamilySettings,
  descriptor: DeepSeekDescriptor
): DeepSeekFamilySettings {
  if (settings.kind !== descriptor.kind) return structuredClone(descriptor.defaultSettings)
  const mode = descriptor.reasoningModes.includes(settings.reasoning.mode)
    ? settings.reasoning.mode
    : descriptor.defaultSettings.reasoning.mode
  const selectedEffort = descriptor.reasoningEfforts.includes(settings.reasoning.effort)
    ? settings.reasoning.effort
    : descriptor.defaultSettings.reasoning.effort
  return {
    kind: descriptor.kind,
    reasoning: {
      mode,
      effort: mode === 'disabled' ? 'provider_default' : selectedEffort
    }
  } as DeepSeekFamilySettings
}

export function DeepSeekProviderSettingsEditor({
  descriptor,
  initialSettings,
  onCancel,
  onConfirm
}: DeepSeekProviderSettingsEditorProps) {
  const { t } = useFrontendConfig()
  const [settings, setSettings] = useState<DeepSeekFamilySettings>(() =>
    normalizeDeepSeekDraft(initialSettings, descriptor)
  )
  const modeLabel = (mode: ProviderReasoningMode) =>
    mode === 'provider_default'
      ? t('configuration.deepSeekSettings.thinkingProviderDefault')
      : mode === 'enabled'
        ? t('configuration.deepSeekSettings.thinkingEnabled')
        : t('configuration.deepSeekSettings.thinkingDisabled')
  const effortLabel = (effort: ProviderReasoningEffort) =>
    effort === 'provider_default'
      ? t('configuration.deepSeekSettings.effortProviderDefault')
      : effort === 'low'
        ? t('configuration.deepSeekSettings.effortLow')
        : effort === 'high'
          ? t('configuration.deepSeekSettings.effortHigh')
          : t('configuration.deepSeekSettings.effortMax')
  const thinkingModeOptions: ReadonlyArray<SettingsSelectOption<ProviderReasoningMode>> =
    descriptor.reasoningModes.map((value) => ({ value, label: modeLabel(value) }))
  const reasoningEffortOptions: ReadonlyArray<SettingsSelectOption<ProviderReasoningEffort>> =
    descriptor.reasoningEfforts.map((value) => ({ value, label: effortLabel(value) }))

  return (
    <ProviderSettingsDialogShell
      title={t('configuration.deepSeekSettings.title')}
      description={t('configuration.deepSeekSettings.defaultThinkingDescription')}
      onCancel={onCancel}
      onConfirm={() => onConfirm(normalizeDeepSeekDraft(settings, descriptor))}
    >
      <div className="provider-settings-dialog__field">
        <span>{t('configuration.deepSeekSettings.thinkingMode')}</span>
        <SettingsSelect
          ariaLabel={t('configuration.deepSeekSettings.thinkingMode')}
          className="provider-settings-dialog__select"
          options={thinkingModeOptions}
          value={settings.reasoning.mode}
          onChange={(mode) =>
            setSettings((current) =>
              normalizeDeepSeekDraft(
                { ...current, reasoning: { ...current.reasoning, mode } },
                descriptor
              )
            )
          }
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
              ...current,
              reasoning: { ...current.reasoning, effort }
            }))
          }
        />
      </div>
    </ProviderSettingsDialogShell>
  )
}

interface MoonshotProviderSettingsEditorProps {
  descriptor: MoonshotDescriptor
  initialSettings: MoonshotFamilySettings
  onCancel: () => void
  onConfirm: (settings: MoonshotFamilySettings) => void
}

function normalizeMoonshotDraft(
  settings: MoonshotFamilySettings,
  descriptor: MoonshotDescriptor
): MoonshotFamilySettings {
  if (settings.kind !== descriptor.kind) return structuredClone(descriptor.defaultSettings)
  if (descriptor.kind === 'moonshot_k3_chat' && settings.kind === 'moonshot_k3_chat') {
    return {
      kind: descriptor.kind,
      reasoningEffort: descriptor.reasoningEfforts.includes(settings.reasoningEffort)
        ? settings.reasoningEffort
        : descriptor.defaultSettings.reasoningEffort
    }
  }
  if (descriptor.kind === 'moonshot_k2_6_chat' && settings.kind === 'moonshot_k2_6_chat') {
    return {
      kind: descriptor.kind,
      thinkingMode: descriptor.thinkingModes.includes(settings.thinkingMode)
        ? settings.thinkingMode
        : descriptor.defaultSettings.thinkingMode
    }
  }
  return structuredClone(descriptor.defaultSettings)
}

export function MoonshotProviderSettingsEditor({
  descriptor,
  initialSettings,
  onCancel,
  onConfirm
}: MoonshotProviderSettingsEditorProps) {
  const { t } = useFrontendConfig()
  const [settings, setSettings] = useState<MoonshotFamilySettings>(() =>
    normalizeMoonshotDraft(initialSettings, descriptor)
  )
  const effortLabel = (effort: ProviderReasoningEffort) =>
    effort === 'provider_default'
      ? t('configuration.moonshotSettings.effortProviderDefault')
      : effort === 'low'
        ? t('configuration.moonshotSettings.effortLow')
        : effort === 'high'
          ? t('configuration.moonshotSettings.effortHigh')
          : t('configuration.moonshotSettings.effortMax')
  const description =
    descriptor.kind === 'moonshot_k3_chat'
      ? t('configuration.moonshotSettings.k3Description')
      : descriptor.kind === 'moonshot_k2_7_code_chat'
        ? t('configuration.moonshotSettings.k27Description')
        : t('configuration.moonshotSettings.k26Description')

  return (
    <ProviderSettingsDialogShell
      title={t('configuration.moonshotSettings.title')}
      description={description}
      onCancel={onCancel}
      onConfirm={() => onConfirm(normalizeMoonshotDraft(settings, descriptor))}
    >
      {descriptor.kind === 'moonshot_k3_chat' && settings.kind === 'moonshot_k3_chat' && (
        <>
          <div className="provider-settings-dialog__field">
            <span>{t('configuration.moonshotSettings.reasoningEffort')}</span>
            <SettingsSelect
              ariaLabel={t('configuration.moonshotSettings.reasoningEffort')}
              className="provider-settings-dialog__select"
              options={descriptor.reasoningEfforts.map((value) => ({
                value,
                label: effortLabel(value)
              }))}
              value={settings.reasoningEffort}
              onChange={(reasoningEffort) =>
                setSettings({ kind: 'moonshot_k3_chat', reasoningEffort })
              }
            />
          </div>
          <p className="provider-settings-dialog__notice" role="note">
            {t('configuration.moonshotSettings.alwaysPreservedThinking')}
          </p>
        </>
      )}
      {descriptor.kind === 'moonshot_k2_7_code_chat' && (
        <p className="provider-settings-dialog__notice" role="note">
          {t('configuration.moonshotSettings.alwaysPreservedThinking')}
        </p>
      )}
      {descriptor.kind === 'moonshot_k2_6_chat' && settings.kind === 'moonshot_k2_6_chat' && (
        <div className="provider-settings-dialog__field">
          <span>{t('configuration.moonshotSettings.thinkingMode')}</span>
          <SettingsSelect
            ariaLabel={t('configuration.moonshotSettings.thinkingMode')}
            className="provider-settings-dialog__select"
            options={descriptor.thinkingModes.map((value) => ({
              value,
              label:
                value === 'provider_default'
                  ? t('configuration.moonshotSettings.thinkingProviderDefault')
                  : value === 'enabled'
                    ? t('configuration.moonshotSettings.thinkingEnabled')
                    : value === 'disabled'
                      ? t('configuration.moonshotSettings.thinkingDisabled')
                      : t('configuration.moonshotSettings.thinkingEnabledKeepAll')
            }))}
            value={settings.thinkingMode}
            onChange={(thinkingMode) => setSettings({ kind: 'moonshot_k2_6_chat', thinkingMode })}
          />
        </div>
      )}
    </ProviderSettingsDialogShell>
  )
}
