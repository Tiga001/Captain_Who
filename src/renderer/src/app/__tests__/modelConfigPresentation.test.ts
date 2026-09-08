import { describe, expect, it } from 'vitest'
import type { ProviderProfileConfigV2 } from '@mycopilot/protocol'
import { getTranslation } from '../../config/frontendTranslations'
import { DEFAULT_MODEL_CONTEXT_WINDOW_TOKENS, type ModelConfig } from '../../config/modelConfig'
import type { Translate } from '../../config/translationFormat'
import { createComposerModelMenuOption } from '../../features/modelSelection/composerModelPresentation'
import { formatModelConfigLabel } from '../../features/modelSelection/modelConfigPresentation'

const translate: Translate = (key) => getTranslation('zh-CN', key)
const genericProfile: ProviderProfileConfigV2 = {
  schemaVersion: 2,
  vendorId: 'generic',
  profile: { id: 'generic_openai_chat', version: 1 },
  settings: { kind: 'generic' }
}

function model(overrides: Partial<ModelConfig> = {}): ModelConfig {
  return {
    id: '01a09000-1111-2222-3333-444444444444',
    displayName: '  My selected model  ',
    providerModelId: 'vendor/model-api-id',
    providerProfileConfig: genericProfile,
    providerProfileUpdate: { kind: 'unchanged' },
    apiTokenOverrideStatus: 'missing',
    apiTokenOverrideMutation: { type: 'keep' },
    supportsImage: false,
    inputPrice: '0',
    cachedInputPrice: '',
    outputPrice: '0',
    enabled: true,
    ...overrides
  }
}

describe('model config presentation', () => {
  it('uses the required display name as the complete user-facing identity', () => {
    expect(formatModelConfigLabel({ displayName: 'DeepSeek' })).toBe('DeepSeek')
  })

  it('trims presentation whitespace without exposing any other identity', () => {
    expect(formatModelConfigLabel({ displayName: '  Kimi K3 High  ' })).toBe('Kimi K3 High')
  })
})

describe('composer model metadata', () => {
  it('keeps the local selection identity, display name and provider API model ID separate', () => {
    const configuration = model({ providerModelId: 'deepseek-v4', displayName: '  Fast coding  ' })
    const result = createComposerModelMenuOption(configuration, translate)
    expect(result.id).toBe(configuration.id)
    expect(result.label).toBe('Fast coding')
    expect(result.modelId).toBe('deepseek-v4')
    expect(result.modelId).not.toBe(result.id)
    expect(result.modelId).not.toBe(result.label)
    expect(configuration.displayName).toBe('  Fast coding  ')
  })

  it.each([
    { profile: genericProfile, label: '通用兼容' },
    {
      profile: {
        schemaVersion: 2,
        vendorId: 'deepseek',
        profile: { id: 'deepseek_v4_chat', version: 1 },
        settings: { kind: 'deepseek_v4_chat', reasoning: { mode: 'enabled', effort: 'high' } }
      },
      label: '深度求索'
    },
    {
      profile: {
        schemaVersion: 2,
        vendorId: 'moonshot',
        profile: { id: 'moonshot_k3_chat', version: 1 },
        settings: { kind: 'moonshot_k3_chat', reasoningEffort: 'max' }
      },
      label: '月之暗面'
    },
    {
      profile: {
        schemaVersion: 2,
        vendorId: 'another-registered-vendor',
        profile: { id: 'generic_anthropic_messages', version: 1 },
        settings: { kind: 'generic' }
      },
      label: '通用兼容'
    }
  ] satisfies Array<{ profile: ProviderProfileConfigV2; label: string }>)(
    'uses the authoritative v2 vendor $profile.vendorId for $label, independently of the API name and endpoint',
    ({ profile, label }) => {
      const first = createComposerModelMenuOption(
        model({
          providerProfileConfig: profile,
          providerModelId: 'deepseek-v4',
          apiUrlOverride: 'https://api.deepseek.com/v1',
          displayName: 'Moonshot model'
        }),
        translate
      )
      const second = createComposerModelMenuOption(
        model({
          providerProfileConfig: profile,
          providerModelId: 'kimi-k3',
          apiUrlOverride: 'https://api.moonshot.cn/v1',
          displayName: 'DeepSeek model'
        }),
        translate
      )
      expect(first.providerLabel).toBe(label)
      expect(second.providerLabel).toBe(label)
    }
  )

  it('uses the configured context size and the existing 128000-token default only when omitted', () => {
    expect(DEFAULT_MODEL_CONTEXT_WINDOW_TOKENS).toBe(128_000)
    expect(createComposerModelMenuOption(model(), translate).contextWindowLabel).toBe('128K')
    expect(
      createComposerModelMenuOption(model({ contextWindowTokens: 131_072 }), translate)
        .contextWindowLabel
    ).toBe('131.072K')
    expect(
      createComposerModelMenuOption(model({ contextWindowTokens: 256_000 }), translate)
        .contextWindowLabel
    ).toBe('256K')
    expect(
      createComposerModelMenuOption(model({ contextWindowTokens: 1_000_000 }), translate)
        .contextWindowLabel
    ).toBe('1000K')
  })

  it.each([0, -1, NaN, Infinity, 1.5, Number.MAX_SAFE_INTEGER + 1])(
    'shows unknown context for invalid configured capacity %s without guessing from the model name',
    (contextWindowTokens) => {
      const result = createComposerModelMenuOption(
        model({
          contextWindowTokens,
          providerModelId: 'model-256k',
          displayName: 'Million-token model'
        }),
        translate
      )
      expect(result.contextWindowLabel).toBe('上下文未知')
    }
  )

  it.each([
    { supportsImage: false, capabilityLabel: '文本' },
    { supportsImage: true, capabilityLabel: '图像' }
  ])(
    'retains the existing input modality indicator for supportsImage=$supportsImage',
    ({ supportsImage, capabilityLabel }) => {
      const result = createComposerModelMenuOption(model({ supportsImage }), translate)
      expect(result.capabilitySupported).toBe(supportsImage)
      expect(result.capabilityLabel).toBe(capabilityLabel)
    }
  )
})
