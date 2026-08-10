import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { StorageModelSettingsRecord } from '@mycopilot/protocol'

const storage = vi.hoisted(() => ({
  loadModelSettings: vi.fn(),
  loadProviderProfileUiDescriptors: vi.fn(),
  saveModelSettings: vi.fn()
}))

vi.mock('../../host/hostClient', () => ({
  hostClient: { storage }
}))

const { loadProviderProfileUiDescriptors, saveModelSettings } =
  await import('../../features/storage/storageClient')

const authoritativeSettings: StorageModelSettingsRecord = {
  apiUrl: 'https://api.example/v1/chat/completions',
  apiToken: 'secret-token',
  searchMode: 'auto',
  tavilyApiKey: '',
  models: [
    {
      id: 'model-1',
      displayName: 'Model 1',
      apiUrlOverride: null,
      apiTokenOverride: null,
      supportsImage: false,
      contextWindowTokens: 128_000,
      providerProfileConfig: {
        schemaVersion: 1,
        profile: { id: 'generic_openai_chat', version: 1 },
        reasoning: { mode: 'provider_default', effort: 'provider_default' }
      },
      inputPrice: '0',
      cachedInputPrice: '',
      outputPrice: '0',
      enabled: true
    }
  ]
}

beforeEach(() => {
  storage.loadModelSettings.mockReset()
  storage.loadProviderProfileUiDescriptors.mockReset()
  storage.saveModelSettings.mockReset().mockResolvedValue(authoritativeSettings)
})

describe('model settings storage client', () => {
  it('sends only an explicit profile update and accepts the Host-authoritative profile config', async () => {
    const saved = await saveModelSettings({
      apiUrl: authoritativeSettings.apiUrl,
      apiToken: authoritativeSettings.apiToken,
      searchMode: 'auto',
      tavilyApiKey: '',
      models: [
        {
          id: 'model-1',
          displayName: 'Model 1',
          supportsImage: false,
          contextWindowTokens: 128_000,
          previousModelId: 'model-before-rename',
          providerProfileConfig: {
            schemaVersion: 999,
            profile: { id: 'deepseek_v4_chat', version: 999 },
            reasoning: { mode: 'enabled', effort: 'max' }
          },
          providerProfileUpdate: { kind: 'select_generic' },
          inputPrice: '0',
          cachedInputPrice: '',
          outputPrice: '0',
          enabled: true
        }
      ]
    })

    expect(storage.saveModelSettings).toHaveBeenCalledWith({
      apiUrl: authoritativeSettings.apiUrl,
      apiToken: authoritativeSettings.apiToken,
      searchMode: 'auto',
      tavilyApiKey: '',
      models: [
        {
          id: 'model-1',
          displayName: 'Model 1',
          apiUrlOverride: null,
          apiTokenOverride: null,
          supportsImage: false,
          contextWindowTokens: 128_000,
          previousModelId: 'model-before-rename',
          providerProfileUpdate: { kind: 'select_generic' },
          inputPrice: '0',
          cachedInputPrice: '',
          outputPrice: '0',
          enabled: true
        }
      ]
    })
    expect(saved.models[0]?.providerProfileConfig).toEqual(
      authoritativeSettings.models[0]?.providerProfileConfig
    )
    expect(saved.models[0]?.providerProfileUpdate).toBeUndefined()
  })

  it('passes the safe profile descriptor projection through unchanged', async () => {
    const descriptors = [
      {
        profileId: 'deepseek_v4_chat',
        profileVersion: 1,
        displayName: '深度求索 / DeepSeek（V4 Chat）',
        compatibleDialects: ['openai_chat_completions'],
        settingsKind: 'deepseek_v4_chat',
        selectable: true
      }
    ] as const
    storage.loadProviderProfileUiDescriptors.mockResolvedValue(descriptors)

    await expect(loadProviderProfileUiDescriptors()).resolves.toBe(descriptors)
  })
})
