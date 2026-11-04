import { describe, expect, it } from 'vitest'
import type { ProviderProfileConfig, StorageProviderProfileUpdate } from './storage'
import {
  parseProviderProfileConfig,
  parseStorageProviderProfileUpdate
} from './storageParsers/provider'
import {
  parseProviderProfileUiDescriptors,
  parseProviderVendorDescriptors,
  parseProviderVendorModelPolicyDescriptor
} from './storageParsers'

const families = ['zhipu_glm5_3_chat', 'zhipu_glm5_3_flash_chat'] as const
const efforts = ['provider_default', 'low', 'high', 'max'] as const

function policy(kind: (typeof families)[number]) {
  return {
    status: 'supported',
    vendorId: 'zhipu',
    modelFamily: kind,
    settingsKind: 'zhipu',
    imageInput: kind === 'zhipu_glm5_3_chat' ? 'unsupported' : 'supported',
    settings: {
      kind,
      reasoningEfforts: efforts,
      defaultSettings: { kind, reasoningEffort: 'max' }
    }
  }
}

describe('Zhipu provider contracts', () => {
  it.each(families)('round trips schema v2 settings and explicit update intents for %s', (kind) => {
    for (const reasoningEffort of efforts) {
      const config = {
        schemaVersion: 2,
        vendorId: 'zhipu',
        profile: { id: kind, version: 1 },
        settings: { kind, reasoningEffort }
      } as const satisfies ProviderProfileConfig
      expect(parseProviderProfileConfig(config, 'Zhipu config')).toEqual(config)
      const update = {
        kind: 'select_vendor',
        vendorId: 'zhipu',
        settings: { kind, reasoningEffort }
      } as const satisfies StorageProviderProfileUpdate
      expect(parseStorageProviderProfileUpdate(update, 'Zhipu update')).toEqual(update)
      expect(update).not.toHaveProperty('profile')
      expect(update).not.toHaveProperty('runtimeCapabilities')
    }
  })

  it.each(families)('accepts the independent safe model policy for %s', (kind) => {
    const descriptor = policy(kind)
    expect(parseProviderVendorModelPolicyDescriptor(descriptor)).toEqual(descriptor)
  })

  it('accepts the fixed directory and two hidden registered profiles', () => {
    const vendors = [
      {
        vendorId: 'zhipu',
        displayName: '智谱',
        selectable: true,
        modelIdPolicy: {
          kind: 'fixed',
          modelIds: ['glm-5.3', 'glm-5.3-flash', 'glm-5.3-flashx', 'glm-5.3-flashx-legal']
        },
        fixedApiUrl: 'https://open.bigmodel.cn/api/paas/v4/chat/completions'
      }
    ]
    expect(parseProviderVendorDescriptors(vendors)).toEqual(vendors)
    const profiles = families.map((profileId) => ({
      profileId,
      profileVersion: 1,
      displayName: profileId,
      compatibleDialects: ['openai_chat_completions'],
      settingsKind: 'none',
      selectable: false
    }))
    expect(parseProviderProfileUiDescriptors(profiles)).toEqual(profiles)
  })

  it.each([
    { ...policy(families[0]), vendorId: 'moonshot' },
    { ...policy(families[0]), settingsKind: 'moonshot' },
    { ...policy(families[0]), modelFamily: families[1] },
    { ...policy(families[0]), settings: policy(families[1]).settings },
    {
      ...policy(families[0]),
      settings: {
        ...policy(families[0]).settings,
        defaultSettings: { kind: families[1], reasoningEffort: 'max' }
      }
    },
    {
      ...policy(families[0]),
      settings: { ...policy(families[0]).settings, reasoningEfforts: ['low'] }
    },
    {
      ...policy(families[0]),
      settings: { ...policy(families[0]).settings, reasoningEfforts: ['max', 'max'] }
    },
    {
      ...policy(families[0]),
      settings: { ...policy(families[0]).settings, reasoningEfforts: [] }
    },
    { ...policy(families[0]), runtimeCapabilities: ['private'] }
  ])('rejects mismatched or expanded Host policies %#', (value) => {
    expect(() => parseProviderVendorModelPolicyDescriptor(value)).toThrow()
  })

  it.each([
    { kind: families[0], reasoningEffort: 'medium' },
    { kind: families[1] },
    { kind: families[0], reasoningEffort: 'max', thinkingMode: 'disabled' },
    { kind: families[1], reasoningEffort: 'high', clearThinking: true },
    { kind: families[0], reasoningEffort: 'max', runtimeCapabilities: {} }
  ])('rejects illegal or expanded Zhipu settings in config and save intents %#', (settings) => {
    expect(() =>
      parseProviderProfileConfig(
        {
          schemaVersion: 2,
          vendorId: 'zhipu',
          profile: { id: settings.kind, version: 1 },
          settings
        },
        'Zhipu config'
      )
    ).toThrow()
    expect(() =>
      parseStorageProviderProfileUpdate(
        { kind: 'select_vendor', vendorId: 'zhipu', settings },
        'Zhipu update'
      )
    ).toThrow()
  })
})
