import { describe, expect, it } from 'vitest'
import type {
  ProviderProfileConfig,
  ProviderVendorDescriptor,
  ProviderVendorModelPolicyDescriptor,
  StorageProviderProfileUpdate
} from './storage'

describe('Provider vendor protocol', () => {
  it('represents legacy and family-owned Profile configurations without changing v1', () => {
    const legacy = {
      schemaVersion: 1,
      profile: { id: 'deepseek_v4_chat', version: 1 },
      reasoning: { mode: 'enabled', effort: 'high' }
    } as const satisfies ProviderProfileConfig
    const moonshot = {
      schemaVersion: 2,
      profile: { id: 'moonshot_k3_chat', version: 1 },
      vendorId: 'moonshot',
      settings: { kind: 'moonshot_k3_chat', reasoningEffort: 'max' }
    } as const satisfies ProviderProfileConfig

    expect(legacy).toEqual({
      schemaVersion: 1,
      profile: { id: 'deepseek_v4_chat', version: 1 },
      reasoning: { mode: 'enabled', effort: 'high' }
    })
    expect(moonshot.settings).toEqual({
      kind: 'moonshot_k3_chat',
      reasoningEffort: 'max'
    })
  })

  it('keeps vendor selection free of internal Profile identity and versions', () => {
    const update = {
      kind: 'select_vendor',
      vendorId: 'moonshot',
      settings: { kind: 'moonshot_k2_6_chat', thinkingMode: 'enabled_keep_all' }
    } as const satisfies StorageProviderProfileUpdate

    expect(update).not.toHaveProperty('profileId')
    expect(update).not.toHaveProperty('profileVersion')
    expect(update).not.toHaveProperty('providerConfigurationRevision')
  })

  it('projects only safe vendor and settings policy fields', () => {
    const vendors = [
      { vendorId: 'generic', displayName: 'Generic', selectable: true },
      { vendorId: 'deepseek', displayName: 'DeepSeek', selectable: true },
      { vendorId: 'moonshot', displayName: 'Moonshot AI', selectable: true }
    ] satisfies ProviderVendorDescriptor[]
    const policy = {
      status: 'supported',
      vendorId: 'moonshot',
      modelFamily: 'moonshot_k3_chat',
      settingsKind: 'moonshot',
      settings: {
        kind: 'moonshot_k3_chat',
        reasoningEfforts: ['provider_default', 'low', 'high', 'max'],
        defaultSettings: { kind: 'moonshot_k3_chat', reasoningEffort: 'max' }
      }
    } as const satisfies ProviderVendorModelPolicyDescriptor

    expect(vendors.map(({ vendorId }) => vendorId)).toEqual(['generic', 'deepseek', 'moonshot'])
    for (const privateField of [
      'profileId',
      'profileVersion',
      'adapter',
      'runtimeCapabilities',
      'continuationRequirement',
      'usage'
    ]) {
      expect(policy).not.toHaveProperty(privateField)
    }
  })
})
