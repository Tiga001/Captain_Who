type ModelLabelSource = {
  displayName: string
}

/**
 * Model labels expose only the required user-facing display name. Internal configuration IDs and
 * provider wire IDs never belong in selection labels.
 */
export function formatModelConfigLabel(model: ModelLabelSource): string {
  return model.displayName.trim()
}
