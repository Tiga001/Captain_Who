import type { ModelConfig, ModelFormValues } from './configurationTypes'

export function modelConfigFromForm(
  values: ModelFormValues,
  editingModel?: ModelConfig
): ModelConfig {
  return {
    id: values.id,
    displayName: values.displayName || values.id,
    apiUrlOverride: values.apiUrlOverride || undefined,
    apiTokenOverride: values.apiTokenOverride || undefined,
    supportsImage: values.supportsImage,
    contextWindowTokens:
      values.contextWindowTokens.trim().length > 0
        ? Number(values.contextWindowTokens.replaceAll(',', ''))
        : undefined,
    // Provider Profile controls are intentionally not visible yet. Editing any visible field
    // must round-trip the existing Host-owned configuration instead of silently reverting it.
    providerProfileConfig: editingModel?.providerProfileConfig,
    inputPrice: values.inputPrice,
    outputPrice: values.outputPrice,
    enabled: editingModel?.enabled ?? true
  }
}
