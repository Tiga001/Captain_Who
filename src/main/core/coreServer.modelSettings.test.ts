import type {
  ProviderProfileUiDescriptor,
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
