import { HostInvocationError } from '@mycopilot/host-api'
import { describe, expect, it } from 'vitest'
import { classifyModelSettingsSaveError } from '../../features/settings/pages/configuration/modelSettingsErrors'

describe('model settings save errors', () => {
  it('recognizes only the safe duplicate-display-name contract', () => {
    const error = new HostInvocationError({
      message: 'Model settings validation failed.',
      code: -32000,
      data: {
        kind: 'model_settings_validation',
        code: 'duplicate_display_name',
        displayName: 'DeepSeek V4'
      }
    })

    expect(classifyModelSettingsSaveError(error)).toEqual({
      code: 'duplicate_display_name',
      displayName: 'DeepSeek V4'
    })
  })

  it('preserves the Host-computed context capacity details without exposing raw errors', () => {
    const details = {
      code: 'invalid_context_capacity_configuration',
      modelId: 'model-1',
      displayName: 'DeepSeek Max',
      contextWindowTokens: 128000,
      reservedOutputTokens: 131072,
      safetyMarginTokens: 6400,
      minimumContextWindowTokens: 137972
    }
    const error = new HostInvocationError({
      message: 'private transport diagnostic',
      code: -32000,
      data: { kind: 'model_settings_validation', ...details }
    })

    expect(classifyModelSettingsSaveError(error)).toEqual(details)
  })

  it.each([
    new Error('UNIQUE constraint failed: models.id'),
    new HostInvocationError({
      message: 'Model settings save failed.',
      data: {
        kind: 'model_settings_validation',
        code: 'duplicate_display_name',
        displayName: 'Model A',
        apiToken: 'private-token'
      }
    }),
    new HostInvocationError({
      message: 'Invalid capacity.',
      data: {
        kind: 'model_settings_validation',
        code: 'invalid_context_capacity_configuration',
        modelId: 'model-1',
        displayName: 'Model A',
        contextWindowTokens: 128000,
        reservedOutputTokens: 131072,
        safetyMarginTokens: 6400,
        minimumContextWindowTokens: 137972,
        apiToken: 'private-token'
      }
    })
  ])('reduces unknown or expanded failures to the neutral category', (error) => {
    expect(classifyModelSettingsSaveError(error)).toEqual({ code: 'unknown' })
  })
})
