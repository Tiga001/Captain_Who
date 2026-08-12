import { describe, expect, it } from 'vitest'
import type { ProviderProfileConfig } from '@mycopilot/protocol'
import type { ModelConfig, ModelFormValues } from '../../config/modelConfig'
import { modelConfigFromForm } from '../../features/settings/pages/configuration/modelPersistence'

const deepSeekProfile: ProviderProfileConfig = {
  schemaVersion: 1,
  profile: {
    id: 'deepseek_v4_chat',
    version: 1
  },
  reasoning: {
    mode: 'enabled',
    effort: 'high'
  }
}

const existingModel: ModelConfig = {
  id: 'deepseek-v4',
  displayName: 'DeepSeek V4',
  supportsImage: false,
  contextWindowTokens: 128_000,
  providerProfileConfig: deepSeekProfile,
  inputPrice: '0.01',
  cachedInputPrice: '',
  outputPrice: '0.02',
  providerProfileUpdate: { kind: 'unchanged' },
  enabled: false
}

const editedValues: ModelFormValues = {
  id: 'deepseek-v4-new-alias',
  displayName: 'DeepSeek V4 edited',
  apiUrlOverride: '',
  apiTokenOverride: '',
  contextWindowTokens: '256,000',
  inputPrice: '0.03',
  cachedInputPrice: '0.005',
  outputPrice: '0.04',
  supportsImage: true,
  providerProfileUpdate: { kind: 'unchanged' }
}

describe('modelConfigFromForm', () => {
  it('preserves the Host-authoritative Provider Profile while editing visible fields', () => {
    const saved = modelConfigFromForm(editedValues, existingModel)

    expect(saved.providerProfileConfig).toEqual(deepSeekProfile)
    expect(saved.enabled).toBe(false)
    expect(saved.contextWindowTokens).toBe(256_000)
    expect(saved.cachedInputPrice).toBe('0.005')
  })

  it('does not invent a Provider Profile for a newly created model', () => {
    expect(modelConfigFromForm(editedValues).providerProfileConfig).toBeUndefined()
  })

  it('passes the explicit one-shot selection separately from the stored profile config', () => {
    const saved = modelConfigFromForm(
      {
        ...editedValues,
        providerProfileUpdate: {
          kind: 'select_registered_profile',
          profileId: 'deepseek_v4_chat',
          settings: {
            kind: 'deepseek_v4_chat',
            reasoning: { mode: 'enabled', effort: 'max' }
          }
        }
      },
      existingModel
    )

    expect(saved.providerProfileConfig).toEqual(deepSeekProfile)
    expect(saved.providerProfileUpdate).toEqual({
      kind: 'select_registered_profile',
      profileId: 'deepseek_v4_chat',
      settings: {
        kind: 'deepseek_v4_chat',
        reasoning: { mode: 'enabled', effort: 'max' }
      }
    })
  })
})
