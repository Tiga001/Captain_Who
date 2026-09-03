import type {
  ProviderProfileUiDescriptor,
  ProviderVendorDescriptor,
  ProviderVendorModelPolicyDescriptor,
  ProviderVendorModelPolicyInput,
  StorageModelSettingsRecord,
  StorageModelSettingsUpdateRecord
} from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const rpcRequest = vi.hoisted(() => vi.fn())

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = rpcRequest
  }
}))

import { CoreServer } from './coreServer'

describe('CoreServer model settings client', () => {
  beforeEach(() => rpcRequest.mockReset())

  it('loads the safe Provider Profile UI projection from its dedicated method', async () => {
    const descriptors: ProviderProfileUiDescriptor[] = [
      {
        profileId: 'deepseek_v4_chat',
        profileVersion: 1,
        displayName: '深度求索 / DeepSeek（V4 Chat）',
        compatibleDialects: ['openai_chat_completions'],
        settingsKind: 'deepseek_v4_chat',
        selectable: true
      }
    ]
    rpcRequest.mockResolvedValue(descriptors)

    await expect(new CoreServer().loadProviderProfileUiDescriptors()).resolves.toEqual(descriptors)
    expect(rpcRequest).toHaveBeenCalledWith('storage.loadProviderProfileUiDescriptors')
  })

  it('loads vendor descriptors and resolves a safe Host-authoritative model policy', async () => {
    const descriptors: ProviderVendorDescriptor[] = [
      { vendorId: 'generic', displayName: 'Generic', selectable: true },
      { vendorId: 'deepseek', displayName: 'DeepSeek', selectable: true },
      { vendorId: 'moonshot', displayName: 'Moonshot AI', selectable: true }
    ]
    rpcRequest.mockResolvedValueOnce(descriptors)

    await expect(new CoreServer().loadProviderVendorDescriptors()).resolves.toEqual(descriptors)
    expect(rpcRequest).toHaveBeenNthCalledWith(1, 'storage.loadProviderVendorDescriptors')

    const input: ProviderVendorModelPolicyInput = {
      vendorId: 'moonshot',
      modelId: 'kimi-k2.6',
      dialect: 'openai_chat_completions'
    }
    const resolution: ProviderVendorModelPolicyDescriptor = {
      status: 'supported',
      vendorId: 'moonshot',
      modelFamily: 'moonshot_k2_6_chat',
      settingsKind: 'moonshot',
      imageInput: 'supported',
      settings: {
        kind: 'moonshot_k2_6_chat',
        thinkingModes: ['provider_default', 'enabled', 'disabled', 'enabled_keep_all'],
        defaultSettings: {
          kind: 'moonshot_k2_6_chat',
          thinkingMode: 'provider_default'
        }
      }
    }
    rpcRequest.mockResolvedValueOnce(resolution)

    await expect(new CoreServer().resolveProviderVendorModelPolicy(input)).resolves.toEqual(
      resolution
    )
    expect(rpcRequest).toHaveBeenNthCalledWith(2, 'storage.resolveProviderVendorModelPolicy', input)
  })

  it('rejects private or unknown Provider descriptor fields returned by Core', async () => {
    rpcRequest.mockResolvedValueOnce([
      {
        profileId: 'deepseek_v4_chat',
        profileVersion: 1,
        displayName: 'DeepSeek',
        compatibleDialects: ['openai_chat_completions'],
        settingsKind: 'deepseek_v4_chat',
        selectable: true,
        runtimeCapabilities: ['private']
      }
    ])
    await expect(new CoreServer().loadProviderProfileUiDescriptors()).rejects.toThrow(
      /unexpected field runtimeCapabilities/
    )

    rpcRequest.mockResolvedValueOnce([
      {
        vendorId: 'moonshot',
        displayName: 'Moonshot AI',
        selectable: true,
        apiToken: 'must-not-cross-the-boundary'
      }
    ])
    await expect(new CoreServer().loadProviderVendorDescriptors()).rejects.toThrow(
      /unexpected field apiToken/
    )

    rpcRequest.mockResolvedValueOnce({
      status: 'supported',
      vendorId: 'moonshot',
      modelFamily: 'moonshot_k3_chat',
      settingsKind: 'moonshot',
      imageInput: 'supported',
      settings: {
        kind: 'moonshot_k3_chat',
        reasoningEfforts: ['provider_default', 'low', 'high', 'max'],
        defaultSettings: {
          kind: 'moonshot_k3_chat',
          reasoningEffort: 'max',
          credentialRef: 'private-reference'
        }
      }
    })
    await expect(
      new CoreServer().resolveProviderVendorModelPolicy({
        vendorId: 'moonshot',
        modelId: 'kimi-k3',
        dialect: 'openai_chat_completions'
      })
    ).rejects.toThrow(/unexpected field credentialRef/)
  })

  it('returns the Host-authoritative normalized settings after an explicit profile update', async () => {
    const input: StorageModelSettingsUpdateRecord = {
      expectedRevision: null,
      apiUrl: 'https://api.example/v1/chat/completions',
      apiTokenMutation: { type: 'replace', value: 'secret-token' },
      searchMode: 'auto',
      tavilyApiKeyMutation: { type: 'keep' },
      models: [
        {
          id: null,
          providerModelId: 'deepseek-chat',
          displayName: 'DeepSeek Chat',
          apiUrlOverride: null,
          apiTokenOverrideMutation: { type: 'keep' },
          supportsImage: false,
          contextWindowTokens: null,
          providerProfileUpdate: {
            kind: 'select_registered_profile',
            profileId: 'deepseek_v4_chat',
            settings: {
              kind: 'deepseek_v4_chat',
              reasoning: { mode: 'disabled', effort: 'provider_default' }
            }
          },
          inputPrice: '0',
          cachedInputPrice: '',
          outputPrice: '0',
          enabled: true
        }
      ]
    }
    const output: StorageModelSettingsRecord = {
      configurationRevision: 'model-settings-v1:00000000-0000-4000-8000-000000000001',
      apiUrl: input.apiUrl,
      apiTokenStatus: 'configured',
      searchMode: input.searchMode,
      tavilyApiKeyStatus: 'missing',
      models: [
        {
          id: 'model-config-deepseek-chat',
          providerModelId: 'deepseek-chat',
          displayName: 'DeepSeek Chat',
          apiUrlOverride: null,
          apiTokenOverrideStatus: 'missing',
          supportsImage: false,
          contextWindowTokens: null,
          providerProfileConfig: {
            schemaVersion: 1,
            profile: { id: 'deepseek_v4_chat', version: 1 },
            reasoning: { mode: 'disabled', effort: 'provider_default' }
          },
          inputPrice: '0',
          cachedInputPrice: '',
          outputPrice: '0',
          enabled: true
        }
      ]
    }
    rpcRequest.mockResolvedValue(output)

    await expect(new CoreServer().saveModelSettings(input)).resolves.toEqual(output)
    expect(rpcRequest).toHaveBeenCalledWith('storage.saveModelSettings', input)
  })

  it('rejects legacy secret projections and malformed mutations at the Main boundary', async () => {
    rpcRequest.mockResolvedValueOnce({
      apiUrl: '',
      apiTokenStatus: 'configured',
      apiToken: 'must-not-cross-the-boundary',
      searchMode: 'auto',
      tavilyApiKeyStatus: 'missing',
      models: []
    })
    await expect(new CoreServer().loadModelSettings()).rejects.toThrow(/credential field apiToken/)

    expect(() =>
      new CoreServer().saveModelSettings({
        expectedRevision: null,
        apiUrl: '',
        apiTokenMutation: { type: 'keep', value: 'smuggled-secret' } as never,
        searchMode: 'auto',
        tavilyApiKeyMutation: { type: 'keep' },
        models: []
      })
    ).toThrow(/apiTokenMutation/)
    expect(rpcRequest).toHaveBeenCalledTimes(1)
  })
})
