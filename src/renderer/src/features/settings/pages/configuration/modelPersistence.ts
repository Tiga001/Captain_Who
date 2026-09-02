import type { ModelConfig, ModelConfigSaveDraft, ModelFormValues } from './configurationTypes'

export function modelConfigFromForm(values: ModelFormValues, editingModel: ModelConfig): ModelConfig
export function modelConfigFromForm(
  values: ModelFormValues,
  editingModel?: undefined
): ModelConfigSaveDraft

export function modelConfigFromForm(
  values: ModelFormValues,
  editingModel?: ModelConfig
): ModelConfig | ModelConfigSaveDraft {
  const fields = {
    providerModelId: values.providerModelId,
    displayName: values.displayName,
    apiUrlOverride: values.apiUrlOverride || undefined,
    apiTokenOverride: values.apiTokenOverride || undefined,
    supportsImage: values.supportsImage,
    contextWindowTokens:
      values.contextWindowTokens.trim().length > 0
        ? Number(values.contextWindowTokens.replaceAll(',', ''))
        : undefined,
    providerProfileUpdate: values.providerProfileUpdate,
    inputPrice: values.inputPrice,
    cachedInputPrice: values.cachedInputPrice,
    outputPrice: values.outputPrice,
    enabled: editingModel?.enabled ?? true
  }

  if (!editingModel) return { ...fields, id: null }
  return {
    ...fields,
    id: editingModel.id,
    // The persisted config is read-only presentation state. Host consumes the explicit update,
    // resolves its version/dialect, and returns the normalized authoritative config.
    providerProfileConfig: editingModel.providerProfileConfig
  }
}
