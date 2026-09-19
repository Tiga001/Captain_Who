import { HostInvocationError } from '@mycopilot/host-api'
import {
  parseStorageModelSettingsValidationErrorData,
  type StorageModelSettingsValidationErrorData
} from '@mycopilot/protocol'

export type ModelSettingsSaveError =
  | { code: 'duplicate_display_name'; displayName: string }
  | Omit<
      Extract<
        StorageModelSettingsValidationErrorData,
        { code: 'invalid_context_capacity_configuration' }
      >,
      'kind'
    >
  | { code: 'unknown' }

export function classifyModelSettingsSaveError(error: unknown): ModelSettingsSaveError {
  if (!(error instanceof HostInvocationError)) return { code: 'unknown' }

  try {
    const data = parseStorageModelSettingsValidationErrorData(error.data)
    if (data.code === 'invalid_context_capacity_configuration') {
      return {
        code: data.code,
        modelId: data.modelId,
        displayName: data.displayName,
        contextWindowTokens: data.contextWindowTokens,
        reservedOutputTokens: data.reservedOutputTokens,
        safetyMarginTokens: data.safetyMarginTokens,
        minimumContextWindowTokens: data.minimumContextWindowTokens
      }
    }
    return { code: data.code, displayName: data.displayName }
  } catch {
    return { code: 'unknown' }
  }
}
