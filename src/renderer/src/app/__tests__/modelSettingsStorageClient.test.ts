import { beforeEach, describe, expect, it, vi } from 'vitest'
import { HostInvocationError } from '@mycopilot/host-api'
import type { ProviderProfileConfig, StorageModelSettingsRecord } from '@mycopilot/protocol'

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
  configurationRevision: 'model-settings-v1:00000000-0000-4000-8000-000000000002',
  apiUrl: 'https://api.example/v1/chat/completions',
  apiTokenStatus: 'configured',
  searchMode: 'auto',
  tavilyApiKeyStatus: 'missing',
  models: [
    {
      id: 'model-1',
      providerModelId: 'provider-model-1',
      displayName: 'Model 1',
      apiUrlOverride: null,
      apiTokenOverrideStatus: 'missing',
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
  storage.saveModelSettings.mockReset().mockResolvedValue({
    ok: true,
    value: authoritativeSettings
  })
})

describe('model settings storage client', () => {
  it('sends only an explicit profile update and accepts the Host-authoritative profile config', async () => {
    const saved = await saveModelSettings(
      {
        apiUrl: authoritativeSettings.apiUrl,
        apiTokenMutation: { type: 'keep' },
        searchMode: 'auto',
        tavilyApiKeyMutation: { type: 'keep' },
        models: [
          {
            id: 'model-1',
            providerModelId: 'provider-model-1',
            displayName: 'Model 1',
            apiTokenOverrideStatus: 'missing',
            apiTokenOverrideMutation: { type: 'keep' },
            supportsImage: false,
            contextWindowTokens: 128_000,
            providerProfileConfig: {
              schemaVersion: 999,
              profile: { id: 'deepseek_v4_chat', version: 999 },
              reasoning: { mode: 'enabled', effort: 'max' }
            } as unknown as ProviderProfileConfig,
            providerProfileUpdate: { kind: 'select_generic' },
            inputPrice: '0',
            cachedInputPrice: '',
            outputPrice: '0',
            enabled: true
          }
        ]
      },
      'model-settings-v1:00000000-0000-4000-8000-000000000001'
    )

    expect(storage.saveModelSettings).toHaveBeenCalledWith({
      expectedRevision: 'model-settings-v1:00000000-0000-4000-8000-000000000001',
      apiUrl: authoritativeSettings.apiUrl,
      apiTokenMutation: { type: 'keep' },
      searchMode: 'auto',
      tavilyApiKeyMutation: { type: 'keep' },
      models: [
        {
          id: 'model-1',
          providerModelId: 'provider-model-1',
          displayName: 'Model 1',
          apiUrlOverride: null,
          apiTokenOverrideMutation: { type: 'keep' },
          supportsImage: false,
          contextWindowTokens: 128_000,
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
    expect(saved.models[0]?.providerProfileUpdate).toEqual({ kind: 'unchanged' })
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

  it('restores a typed duplicate-display-name error from the Host envelope', async () => {
    const data = {
      kind: 'model_settings_validation',
      code: 'duplicate_display_name',
      displayName: 'DeepSeek V4'
    } as const
    storage.saveModelSettings.mockResolvedValue({
      ok: false,
      error: {
        message: 'Model settings validation failed.',
        code: -32000,
        data
      }
    })

    try {
      await saveModelSettings(
        {
          apiUrl: '',
          apiTokenMutation: { type: 'keep' },
          searchMode: 'disabled',
          tavilyApiKeyMutation: { type: 'keep' },
          models: []
        },
        null
      )
      expect.fail('a failed Host envelope must reject the Renderer client')
    } catch (error) {
      expect(error).toBeInstanceOf(HostInvocationError)
      expect(error).toMatchObject({ code: -32000, data })
    }
  })
})
