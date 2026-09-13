import { describe, expect, it } from 'vitest'
import type {
  ProviderProfileConfig,
  ProviderVendorDescriptor,
  ProviderVendorModelPolicyDescriptor,
  StorageProviderProfileUpdate
} from './storage'

describe('Provider vendor protocol', () => {
  it('represents generic and family-owned Profile configurations', () => {
    const generic = {
      schemaVersion: 1,
      profile: { id: 'generic_openai_chat', version: 1 },
      reasoning: { mode: 'provider_default', effort: 'provider_default' }
    } as const satisfies ProviderProfileConfig
    const moonshot = {
      schemaVersion: 2,
      profile: { id: 'moonshot_k3_chat', version: 1 },
      vendorId: 'moonshot',
      settings: { kind: 'moonshot_k3_chat', reasoningEffort: 'max' }
    } as const satisfies ProviderProfileConfig

    expect(generic).toEqual({
      schemaVersion: 1,
      profile: { id: 'generic_openai_chat', version: 1 },
      reasoning: { mode: 'provider_default', effort: 'provider_default' }
    })
    expect(moonshot.settings).toEqual({
      kind: 'moonshot_k3_chat',
      reasoningEffort: 'max'
    })
  })

  it('keeps DeepSeek public family settings independent from its internal Profile identity', () => {
    const flash = {
      schemaVersion: 2,
      profile: { id: 'deepseek_v4_1_flash_chat', version: 1 },
      vendorId: 'deepseek',
      settings: {
        kind: 'deepseek_flash_chat',
        reasoning: { mode: 'enabled', effort: 'low' }
      }
    } as const satisfies ProviderProfileConfig
    const pro = {
      schemaVersion: 2,
      profile: { id: 'deepseek_v4_pro_0813_chat', version: 1 },
      vendorId: 'deepseek',
      settings: {
        kind: 'deepseek_pro_chat',
        reasoning: { mode: 'enabled', effort: 'max' }
      }
    } as const satisfies ProviderProfileConfig

    expect(flash.profile.id).not.toBe(flash.settings.kind)
    expect(pro.profile.id).not.toBe(pro.settings.kind)
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
      imageInput: 'supported',
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
