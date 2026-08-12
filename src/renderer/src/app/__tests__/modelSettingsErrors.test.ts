import { HostInvocationError } from '@mycopilot/host-api'
import { describe, expect, it } from 'vitest'
import { classifyModelSettingsSaveError } from '../../features/settings/pages/configuration/modelSettingsErrors'

describe('model settings save errors', () => {
  it('recognizes only the safe duplicate-model contract', () => {
    const error = new HostInvocationError({
      message: 'Model settings validation failed.',
      code: -32000,
      data: {
        kind: 'model_settings_validation',
        code: 'duplicate_model_id',
        modelId: 'deepseek-v4-flash'
      }
    })

    expect(classifyModelSettingsSaveError(error)).toEqual({
      code: 'duplicate_model_id',
      modelId: 'deepseek-v4-flash'
    })
  })

  it.each([
    new Error('UNIQUE constraint failed: models.id'),
    new HostInvocationError({
      message: 'Model settings save failed.',
      data: {
        kind: 'model_settings_validation',
        code: 'duplicate_model_id',
        modelId: 'model-a',
        apiToken: 'private-token'
      }
    })
  ])('reduces unknown or expanded failures to the neutral category', (error) => {
    expect(classifyModelSettingsSaveError(error)).toEqual({ code: 'unknown' })
  })
})
