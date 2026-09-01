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

    await expect(new CoreServer().loadProviderProfileUiDescriptors()).resolves.toBe(descriptors)
    expect(rpcRequest).toHaveBeenCalledWith('storage.loadProviderProfileUiDescriptors')
  })

  it('loads vendor descriptors and resolves a safe Host-authoritative model policy', async () => {
    const descriptors: ProviderVendorDescriptor[] = [
      { vendorId: 'generic', displayName: 'Generic', selectable: true },
      { vendorId: 'deepseek', displayName: 'DeepSeek', selectable: true },
      { vendorId: 'moonshot', displayName: 'Moonshot AI', selectable: true }
    ]
    rpcRequest.mockResolvedValueOnce(descriptors)

    await expect(new CoreServer().loadProviderVendorDescriptors()).resolves.toBe(descriptors)
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

    await expect(new CoreServer().resolveProviderVendorModelPolicy(input)).resolves.toBe(resolution)
    expect(rpcRequest).toHaveBeenNthCalledWith(2, 'storage.resolveProviderVendorModelPolicy', input)
  })

  it('returns the Host-authoritative normalized settings after an explicit profile update', async () => {
    const input: StorageModelSettingsUpdateRecord = {
      apiUrl: 'https://api.example/v1/chat/completions',
      apiToken: 'secret-token',
      searchMode: 'auto',
      tavilyApiKey: '',
      models: [
        {
          id: 'deepseek-chat',
          displayName: 'DeepSeek Chat',
          previousModelId: null,
          apiUrlOverride: null,
          apiTokenOverride: null,
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
      apiUrl: input.apiUrl,
      apiToken: input.apiToken,
      searchMode: input.searchMode,
      tavilyApiKey: input.tavilyApiKey,
      models: [
        {
          id: 'deepseek-chat',
          displayName: 'DeepSeek Chat',
          apiUrlOverride: null,
          apiTokenOverride: null,
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

    await expect(new CoreServer().saveModelSettings(input)).resolves.toBe(output)
    expect(rpcRequest).toHaveBeenCalledWith('storage.saveModelSettings', input)
  })
})
