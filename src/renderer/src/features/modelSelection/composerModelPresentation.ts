import type { ProviderProfileConfig } from '@mycopilot/protocol'
import { DEFAULT_MODEL_CONTEXT_WINDOW_TOKENS, type ModelConfig } from '../../config/modelConfig'
import type { Translate } from '../../config/translationFormat'
import type { ModelConfigPickerOption } from './ModelConfigPicker'
import { formatModelConfigLabel } from './modelConfigPresentation'

export interface ComposerModelMenuOption extends ModelConfigPickerOption {
  modelId: string
  providerLabel: string
  contextWindowLabel: string
}

type ComposerModelSource = Pick<
  ModelConfig,
  | 'id'
  | 'displayName'
  | 'providerModelId'
  | 'supportsImage'
  | 'contextWindowTokens'
  | 'providerProfileConfig'
>

function providerLabel(config: ProviderProfileConfig | undefined, t: Translate): string {
  if (config?.schemaVersion === 2) {
    if (config.vendorId === 'deepseek') return t('chat.models.providerDeepseek')
    if (config.vendorId === 'moonshot') return t('chat.models.providerMoonshot')
  }
  return t('chat.models.providerGeneric')
}

export function createComposerModelMenuOption(
  model: ComposerModelSource,
  t: Translate
): ComposerModelMenuOption {
  const contextWindow = model.contextWindowTokens ?? DEFAULT_MODEL_CONTEXT_WINDOW_TOKENS
  return {
    id: model.id,
    label: formatModelConfigLabel(model),
    // API identity is separate from the local configuration key used by onChange.
    modelId: model.providerModelId?.trim() || formatModelConfigLabel(model),
    providerLabel: providerLabel(model.providerProfileConfig, t),
    contextWindowLabel:
      Number.isSafeInteger(contextWindow) && contextWindow > 0
        ? `${Number((contextWindow / 1_000).toFixed(3))}K`
        : t('chat.models.contextUnknown'),
    capabilityLabel: model.supportsImage ? t('configuration.image') : t('configuration.text'),
    capabilitySupported: model.supportsImage
  }
}
