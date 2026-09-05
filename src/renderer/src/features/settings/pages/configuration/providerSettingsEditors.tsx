import { renderSettingsNodes, settingLabel, settingDescription } from '../../settingsDefinition'
import {
  deepSeekProviderSettings,
  moonshotProviderSettings,
  deepSeekThinkingModeLabels,
  deepSeekEffortLabels,
  moonshotThinkingModeLabels,
  moonshotEffortLabels
} from './configuration.definition'
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
  definition?: typeof deepSeekProviderSettings
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
  definition = deepSeekProviderSettings,
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
      ? t(deepSeekThinkingModeLabels.provider_default)
      : mode === 'enabled'
        ? t(deepSeekThinkingModeLabels.enabled)
        : t(deepSeekThinkingModeLabels.disabled)
  const effortLabel = (effort: ProviderReasoningEffort) =>
    effort === 'provider_default'
      ? t(deepSeekEffortLabels.provider_default)
      : effort === 'low'
        ? t(deepSeekEffortLabels.low)
        : effort === 'high'
          ? t(deepSeekEffortLabels.high)
          : t(deepSeekEffortLabels.max)
  const thinkingModeOptions: ReadonlyArray<SettingsSelectOption<ProviderReasoningMode>> =
    descriptor.reasoningModes.map((value) => ({ value, label: modeLabel(value) }))
  const reasoningEffortOptions: ReadonlyArray<SettingsSelectOption<ProviderReasoningEffort>> =
    descriptor.reasoningEfforts.map((value) => ({ value, label: effortLabel(value) }))

  return (
    <ProviderSettingsDialogShell
      title={settingLabel(definition, t)}
      settingId={definition.id}
      description={settingDescription(definition, t) ?? ''}
      onCancel={onCancel}
      onConfirm={() => onConfirm(normalizeDeepSeekDraft(settings, descriptor))}
    >
      {renderSettingsNodes(definition.children, (node) => {
        switch (node.id) {
          case 'configuration.model.deepseek.thinkingMode':
            return (
              <div className="provider-settings-dialog__field">
                <span>{settingLabel(node, t)}</span>
                <SettingsSelect
                  ariaLabel={settingLabel(node, t)}
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
            )
          case 'configuration.model.deepseek.reasoningEffort':
            return (
              <div className="provider-settings-dialog__field">
                <span>{settingLabel(node, t)}</span>
                <SettingsSelect
                  ariaLabel={settingLabel(node, t)}
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
            )
        }
      })}
    </ProviderSettingsDialogShell>
  )
}

interface MoonshotProviderSettingsEditorProps {
  definition?: typeof moonshotProviderSettings
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
  definition = moonshotProviderSettings,
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
      ? t(moonshotEffortLabels.provider_default)
      : effort === 'low'
        ? t(moonshotEffortLabels.low)
        : effort === 'high'
          ? t(moonshotEffortLabels.high)
          : t(moonshotEffortLabels.max)
  const description =
    descriptor.kind === 'moonshot_k3_chat'
      ? t('configuration.moonshotSettings.k3Description')
      : descriptor.kind === 'moonshot_k2_7_code_chat'
        ? t('configuration.moonshotSettings.k27Description')
        : t('configuration.moonshotSettings.k26Description')

  return (
    <ProviderSettingsDialogShell
      title={settingLabel(definition, t)}
      settingId={definition.id}
      description={description}
      onCancel={onCancel}
      onConfirm={() => onConfirm(normalizeMoonshotDraft(settings, descriptor))}
    >
      {renderSettingsNodes(definition.children, (node) => {
        switch (node.id) {
          case 'configuration.model.moonshot.reasoningEffort':
            return (
              (descriptor.kind === 'moonshot_k3_chat' && settings.kind === 'moonshot_k3_chat' && (
                <>
                  <div className="provider-settings-dialog__field" data-setting-id={node.id}>
                    <span>{settingLabel(node, t)}</span>
                    <SettingsSelect
                      ariaLabel={settingLabel(node, t)}
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
              )) ||
              null
            )
          case 'configuration.model.moonshot.thinkingMode':
            return (
              (descriptor.kind === 'moonshot_k2_6_chat' &&
                settings.kind === 'moonshot_k2_6_chat' && (
                  <div className="provider-settings-dialog__field">
                    <span>{settingLabel(node, t)}</span>
                    <SettingsSelect
                      ariaLabel={settingLabel(node, t)}
                      className="provider-settings-dialog__select"
                      options={descriptor.thinkingModes.map((value) => ({
                        value,
                        label:
                          value === 'provider_default'
                            ? t(moonshotThinkingModeLabels.provider_default)
                            : value === 'enabled'
                              ? t(moonshotThinkingModeLabels.enabled)
                              : value === 'disabled'
                                ? t(moonshotThinkingModeLabels.disabled)
                                : t(moonshotThinkingModeLabels.enabled_keep_all)
                      }))}
                      value={settings.thinkingMode}
                      onChange={(thinkingMode) =>
                        setSettings({ kind: 'moonshot_k2_6_chat', thinkingMode })
                      }
                    />
                  </div>
                )) ||
              null
            )
        }
      })}
      {descriptor.kind === 'moonshot_k2_7_code_chat' && (
        <p className="provider-settings-dialog__notice" role="note">
          {t('configuration.moonshotSettings.alwaysPreservedThinking')}
        </p>
      )}
    </ProviderSettingsDialogShell>
  )
}
