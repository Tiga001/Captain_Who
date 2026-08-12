import { HostInvocationError } from '@mycopilot/host-api'
import { parseStorageModelSettingsValidationErrorData } from '@mycopilot/protocol'

export type ModelSettingsSaveError =
  { code: 'duplicate_model_id'; modelId: string } | { code: 'unknown' }

export function classifyModelSettingsSaveError(error: unknown): ModelSettingsSaveError {
  if (!(error instanceof HostInvocationError)) return { code: 'unknown' }

  try {
    const data = parseStorageModelSettingsValidationErrorData(error.data)
    return { code: data.code, modelId: data.modelId }
  } catch {
    return { code: 'unknown' }
  }
}
