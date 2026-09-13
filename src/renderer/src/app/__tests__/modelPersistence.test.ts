import { describe, expect, it } from 'vitest'
import type { ProviderProfileConfig } from '@mycopilot/protocol'
import type { ModelConfig, ModelFormValues } from '../../config/modelConfig'
import { modelConfigFromForm } from '../../features/settings/pages/configuration/modelPersistence'

const deepSeekProfile: ProviderProfileConfig = {
  schemaVersion: 2,
  vendorId: 'deepseek',
  profile: {
    id: 'deepseek_v4_1_flash_chat',
    version: 1
  },
  settings: {
    kind: 'deepseek_flash_chat',
    reasoning: {
      mode: 'enabled',
      effort: 'high'
    }
  }
}

const existingModel: ModelConfig = {
  id: 'model-config-1',
  providerModelId: 'deepseek-flash',
  displayName: 'DeepSeek Flash',
  apiTokenOverrideStatus: 'missing',
  apiTokenOverrideMutation: { type: 'keep' },
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
  providerModelId: 'deepseek-flash',
  displayName: 'DeepSeek Flash edited',
  apiUrlOverride: '',
  apiTokenOverrideStatus: 'missing',
  apiTokenOverrideMutation: { type: 'keep' },
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
    expect(saved.id).toBe('model-config-1')
    expect(saved.displayName).toBe('DeepSeek Flash edited')
    expect(saved.providerModelId).toBe('deepseek-flash')
    expect(saved.enabled).toBe(false)
    expect(saved.contextWindowTokens).toBe(256_000)
    expect(saved.cachedInputPrice).toBe('0.005')
  })

  it('does not invent a Provider Profile for a newly created model', () => {
    const saved = modelConfigFromForm(editedValues)
    expect(saved.id).toBeNull()
    expect(saved.providerProfileConfig).toBeUndefined()
  })

  it('passes the explicit one-shot selection separately from the stored profile config', () => {
    const saved = modelConfigFromForm(
      {
        ...editedValues,
        providerProfileUpdate: {
          kind: 'select_vendor',
          vendorId: 'deepseek',
          settings: {
            kind: 'deepseek_flash_chat',
            reasoning: { mode: 'enabled', effort: 'max' }
          }
        }
      },
      existingModel
    )

    expect(saved.providerProfileConfig).toEqual(deepSeekProfile)
    expect(saved.providerProfileUpdate).toEqual({
      kind: 'select_vendor',
      vendorId: 'deepseek',
      settings: {
        kind: 'deepseek_flash_chat',
        reasoning: { mode: 'enabled', effort: 'max' }
      }
    })
  })
})
