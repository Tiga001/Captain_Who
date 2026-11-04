import { describe, expect, it } from 'vitest'
import type {
  ProviderProfileConfig,
  ProviderProfileUiDescriptor,
  ProviderVendorModelPolicyDescriptor
} from '@mycopilot/protocol'
import { prepareModelsForGlobalApiUrlChange, type ModelConfig } from '../../config/modelConfig'
import {
  applyResolvedProviderPolicy,
  detectProviderProtocolDialect,
  initialNewProviderProfileFormState,
  initialProviderProfileFormState,
  selectProviderVendor,
  updateDeepSeekProviderSettings,
  updateMoonshotProviderSettings,
  updateZhipuProviderSettings
} from '../../features/settings/pages/configuration/providerProfileForm'

const descriptors: ProviderProfileUiDescriptor[] = [
  ...(['zhipu_glm5_3_chat', 'zhipu_glm5_3_flash_chat'] as const).map(
    (profileId): ProviderProfileUiDescriptor => ({
      profileId,
      profileVersion: 1,
      displayName: 'Zhipu',
      compatibleDialects: ['openai_chat_completions'],
      settingsKind: 'none',
      selectable: false
    })
  ),
  {
    profileId: 'generic_openai_chat',
    profileVersion: 1,
    displayName: 'Generic OpenAI Chat',
    compatibleDialects: ['openai_chat_completions'],
    settingsKind: 'none',
    selectable: true
  },
  {
    profileId: 'generic_anthropic_messages',
    profileVersion: 1,
    displayName: 'Generic Anthropic Messages',
    compatibleDialects: ['anthropic_messages'],
    settingsKind: 'none',
    selectable: true
  },
  {
    profileId: 'deepseek_v4_1_flash_chat',
    profileVersion: 1,
    displayName: 'DeepSeek V4.1 Flash Chat',
    compatibleDialects: ['openai_chat_completions'],
    settingsKind: 'none',
    selectable: false
  },
  {
    profileId: 'deepseek_v4_pro_0813_chat',
    profileVersion: 1,
    displayName: 'DeepSeek V4 Pro 0813 Chat',
    compatibleDialects: ['openai_chat_completions'],
    settingsKind: 'none',
    selectable: false
  },
  {
    profileId: 'moonshot_k3_chat',
    profileVersion: 1,
    displayName: 'Moonshot Kimi K3',
    compatibleDialects: ['openai_chat_completions'],
    settingsKind: 'none',
    selectable: false
  },
  {
    profileId: 'moonshot_k2_7_code_chat',
    profileVersion: 1,
    displayName: 'Moonshot Kimi K2.7 Code',
    compatibleDialects: ['openai_chat_completions'],
    settingsKind: 'none',
    selectable: false
  },
  {
    profileId: 'moonshot_k2_6_chat',
    profileVersion: 1,
    displayName: 'Moonshot Kimi K2.6',
    compatibleDialects: ['openai_chat_completions'],
    settingsKind: 'none',
    selectable: false
  }
]

const deepSeekPolicy: ProviderVendorModelPolicyDescriptor = {
  status: 'supported',
  vendorId: 'deepseek',
  modelFamily: 'deepseek_flash_chat',
  settingsKind: 'deepseek',
  imageInput: 'supported',
  settings: {
    kind: 'deepseek_flash_chat',
    reasoningModes: ['provider_default', 'enabled', 'disabled'],
    reasoningEfforts: ['provider_default', 'low', 'high', 'max'],
    defaultSettings: {
      kind: 'deepseek_flash_chat',
      reasoning: { mode: 'provider_default', effort: 'provider_default' }
    }
  }
}

const deepSeekProPolicy: ProviderVendorModelPolicyDescriptor = {
  status: 'supported',
  vendorId: 'deepseek',
  modelFamily: 'deepseek_pro_chat',
  settingsKind: 'deepseek',
  imageInput: 'unsupported',
  settings: {
    kind: 'deepseek_pro_chat',
    reasoningModes: ['provider_default', 'enabled', 'disabled'],
    reasoningEfforts: ['provider_default', 'low', 'high', 'max'],
    defaultSettings: {
      kind: 'deepseek_pro_chat',
      reasoning: { mode: 'provider_default', effort: 'provider_default' }
    }
  }
}

const k3Policy: ProviderVendorModelPolicyDescriptor = {
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
}

const k26Policy: ProviderVendorModelPolicyDescriptor = {
  status: 'supported',
  vendorId: 'moonshot',
  modelFamily: 'moonshot_k2_6_chat',
  settingsKind: 'moonshot',
  imageInput: 'supported',
  settings: {
    kind: 'moonshot_k2_6_chat',
    thinkingModes: ['provider_default', 'enabled', 'disabled', 'enabled_keep_all'],
    defaultSettings: { kind: 'moonshot_k2_6_chat', thinkingMode: 'provider_default' }
  }
}

function legacyProfile(
  id: ProviderProfileConfig['profile']['id'],
  version = 1
): ProviderProfileConfig {
  return {
    schemaVersion: 1,
    profile: { id, version },
    reasoning: { mode: 'enabled', effort: 'high' }
  }
}

describe('vendor-aware Provider form state', () => {
  it.each(['zhipu_glm5_3_chat', 'zhipu_glm5_3_flash_chat'] as const)(
    'loads %s unchanged and preserves its effort within the family',
    (kind) => {
      const state = initialProviderProfileFormState(
        {
          schemaVersion: 2,
          vendorId: 'zhipu',
          profile: { id: kind, version: 1 },
          settings: { kind, reasoningEffort: 'low' }
        },
        descriptors
      )
      const resolved = applyResolvedProviderPolicy(state, {
        status: 'supported',
        vendorId: 'zhipu',
        modelFamily: kind,
        settingsKind: 'zhipu',
        imageInput: kind === 'zhipu_glm5_3_chat' ? 'unsupported' : 'supported',
        settings: {
          kind,
          reasoningEfforts: ['provider_default', 'low', 'high', 'max'],
          defaultSettings: { kind, reasoningEffort: 'max' }
        } as Extract<ProviderVendorModelPolicyDescriptor, { status: 'supported' }>['settings']
      })
      expect(resolved).toMatchObject({
        selection: 'zhipu',
        settings: { kind, reasoningEffort: 'low' },
        update: { kind: 'unchanged' }
      })
      expect(
        updateZhipuProviderSettings(resolved, { kind, reasoningEffort: 'provider_default' }).update
      ).toEqual({
        kind: 'select_vendor',
        vendorId: 'zhipu',
        settings: { kind, reasoningEffort: 'provider_default' }
      })
    }
  )

  it('resets Zhipu settings only when Host resolves a different family', () => {
    const state = initialProviderProfileFormState(
      {
        schemaVersion: 2,
        vendorId: 'zhipu',
        profile: { id: 'zhipu_glm5_3_chat', version: 1 },
        settings: { kind: 'zhipu_glm5_3_chat', reasoningEffort: 'low' }
      },
      descriptors
    )
    const flashPolicy: ProviderVendorModelPolicyDescriptor = {
      status: 'supported',
      vendorId: 'zhipu',
      modelFamily: 'zhipu_glm5_3_flash_chat',
      settingsKind: 'zhipu',
      imageInput: 'supported',
      settings: {
        kind: 'zhipu_glm5_3_flash_chat',
        reasoningEfforts: ['provider_default', 'low', 'high', 'max'],
        defaultSettings: { kind: 'zhipu_glm5_3_flash_chat', reasoningEffort: 'max' }
      }
    }
    expect(applyResolvedProviderPolicy(state, flashPolicy)).toMatchObject({
      selection: 'zhipu',
      familyChanged: true,
      settings: { kind: 'zhipu_glm5_3_flash_chat', reasoningEffort: 'max' },
      update: { kind: 'select_vendor', vendorId: 'zhipu' }
    })
    const generic = initialNewProviderProfileFormState()
    expect(applyResolvedProviderPolicy(generic, flashPolicy)).toBe(generic)
    expect(
      applyResolvedProviderPolicy(selectProviderVendor(generic, 'zhipu'), flashPolicy)
    ).toMatchObject({ selection: 'zhipu', update: { kind: 'select_vendor', vendorId: 'zhipu' } })
  })

  it('preserves future Zhipu profiles as opaque unchanged configurations', () => {
    expect(
      initialProviderProfileFormState(
        {
          schemaVersion: 2,
          vendorId: 'zhipu',
          profile: { id: 'zhipu_glm5_3_chat', version: 99 },
          settings: { kind: 'zhipu_glm5_3_chat', reasoningEffort: 'high' }
        },
        descriptors
      )
    ).toMatchObject({ selection: 'unsupported', settings: null, update: { kind: 'unchanged' } })
  })

  it('starts new models with explicit Generic selection', () => {
    expect(initialNewProviderProfileFormState()).toMatchObject({
      selection: 'generic',
      settings: { kind: 'generic' },
      update: { kind: 'select_generic' }
    })
  })

  it('derives each DeepSeek public family from V2 settings', () => {
    const flash = initialProviderProfileFormState(
      {
        schemaVersion: 2,
        vendorId: 'deepseek',
        profile: { id: 'deepseek_v4_1_flash_chat', version: 1 },
        settings: {
          kind: 'deepseek_flash_chat',
          reasoning: { mode: 'enabled', effort: 'low' }
        }
      },
      descriptors
    )
    const pro = initialProviderProfileFormState(
      {
        schemaVersion: 2,
        vendorId: 'deepseek',
        profile: { id: 'deepseek_v4_pro_0813_chat', version: 1 },
        settings: {
          kind: 'deepseek_pro_chat',
          reasoning: { mode: 'enabled', effort: 'max' }
        }
      },
      descriptors
    )

    expect(flash).toMatchObject({
      selection: 'deepseek',
      modelFamily: 'deepseek_flash_chat',
      settings: {
        kind: 'deepseek_flash_chat',
        reasoning: { mode: 'enabled', effort: 'low' }
      },
      update: { kind: 'unchanged' }
    })
    expect(pro).toMatchObject({
      selection: 'deepseek',
      modelFamily: 'deepseek_pro_chat',
      settings: {
        kind: 'deepseek_pro_chat',
        reasoning: { mode: 'enabled', effort: 'max' }
      },
      update: { kind: 'unchanged' }
    })
  })

  it.each([
    {
      schemaVersion: 2,
      vendorId: 'moonshot',
      profile: { id: 'moonshot_k3_chat', version: 1 },
      settings: { kind: 'moonshot_k3_chat', reasoningEffort: 'max' }
    },
    {
      schemaVersion: 2,
      vendorId: 'moonshot',
      profile: { id: 'moonshot_k2_7_code_chat', version: 1 },
      settings: { kind: 'moonshot_k2_7_code_chat' }
    },
    {
      schemaVersion: 2,
      vendorId: 'moonshot',
      profile: { id: 'moonshot_k2_6_chat', version: 1 },
      settings: { kind: 'moonshot_k2_6_chat', thinkingMode: 'enabled_keep_all' }
    }
  ] as const)('loads Moonshot family config %# unchanged', (config) => {
    expect(initialProviderProfileFormState(config, descriptors)).toMatchObject({
      selection: 'moonshot',
      settings: config.settings,
      update: { kind: 'unchanged' }
    })
  })

  it('keeps unknown stored configs opaque and never exposes their identity in state', () => {
    const state = initialProviderProfileFormState(
      legacyProfile('future_private_profile', 9),
      descriptors
    )
    expect(state).toMatchObject({
      selection: 'unsupported',
      settings: null,
      update: { kind: 'unchanged' }
    })
    expect(JSON.stringify(state)).not.toContain('future_private_profile')
  })

  it('keeps a future version of a known V2 family opaque and unchanged', () => {
    const state = initialProviderProfileFormState(
      {
        schemaVersion: 2,
        vendorId: 'moonshot',
        profile: { id: 'moonshot_k3_chat', version: 99 },
        settings: { kind: 'moonshot_k3_chat', reasoningEffort: 'low' }
      },
      descriptors
    )
    expect(state).toEqual({
      selection: 'unsupported',
      settings: null,
      modelFamily: null,
      update: { kind: 'unchanged' },
      explicitSelection: false,
      familyChanged: false
    })
  })

  it('preserves compatible settings in-family and resets on a Host-resolved family change', () => {
    let state = applyResolvedProviderPolicy(
      initialProviderProfileFormState(
        {
          schemaVersion: 2,
          vendorId: 'moonshot',
          profile: { id: 'moonshot_k3_chat', version: 1 },
          settings: { kind: 'moonshot_k3_chat', reasoningEffort: 'low' }
        },
        descriptors
      ),
      k3Policy
    )
    expect(state).toMatchObject({
      settings: { kind: 'moonshot_k3_chat', reasoningEffort: 'low' },
      update: { kind: 'unchanged' }
    })

    state = applyResolvedProviderPolicy(state, k26Policy)
    expect(state).toMatchObject({
      settings: { kind: 'moonshot_k2_6_chat', thinkingMode: 'provider_default' },
      familyChanged: true,
      update: {
        kind: 'select_vendor',
        vendorId: 'moonshot',
        settings: { kind: 'moonshot_k2_6_chat', thinkingMode: 'provider_default' }
      }
    })
  })

  it('resets Flash settings when the Host resolves the Pro family', () => {
    const flash = applyResolvedProviderPolicy(
      initialProviderProfileFormState(
        {
          schemaVersion: 2,
          vendorId: 'deepseek',
          profile: { id: 'deepseek_v4_1_flash_chat', version: 1 },
          settings: {
            kind: 'deepseek_flash_chat',
            reasoning: { mode: 'enabled', effort: 'low' }
          }
        },
        descriptors
      ),
      deepSeekPolicy
    )
    expect(flash).toMatchObject({
      settings: {
        kind: 'deepseek_flash_chat',
        reasoning: { mode: 'enabled', effort: 'low' }
      },
      update: { kind: 'unchanged' }
    })

    const pro = applyResolvedProviderPolicy(flash, deepSeekProPolicy)
    expect(pro).toMatchObject({
      settings: {
        kind: 'deepseek_pro_chat',
        reasoning: { mode: 'provider_default', effort: 'provider_default' }
      },
      familyChanged: true,
      update: {
        kind: 'select_vendor',
        vendorId: 'deepseek',
        settings: {
          kind: 'deepseek_pro_chat',
          reasoning: { mode: 'provider_default', effort: 'provider_default' }
        }
      }
    })
  })

  it('writes only family-owned public settings after explicit confirmation', () => {
    const selected = applyResolvedProviderPolicy(
      selectProviderVendor(initialNewProviderProfileFormState(), 'deepseek'),
      deepSeekPolicy
    )
    const deepSeek = updateDeepSeekProviderSettings(selected, {
      kind: 'deepseek_flash_chat',
      reasoning: { mode: 'enabled', effort: 'max' }
    })
    expect(deepSeek.update).toEqual({
      kind: 'select_vendor',
      vendorId: 'deepseek',
      settings: {
        kind: 'deepseek_flash_chat',
        reasoning: { mode: 'enabled', effort: 'max' }
      }
    })

    const moonshot = updateMoonshotProviderSettings(
      applyResolvedProviderPolicy(selectProviderVendor(deepSeek, 'moonshot'), k3Policy),
      { kind: 'moonshot_k3_chat', reasoningEffort: 'high' }
    )
    expect(moonshot.update).toEqual({
      kind: 'select_vendor',
      vendorId: 'moonshot',
      settings: { kind: 'moonshot_k3_chat', reasoningEffort: 'high' }
    })
    expect(JSON.stringify(moonshot.update)).not.toMatch(/profile|version|capabilit/i)
  })

  it('mirrors the Host dialect classification without inferring model family', () => {
    expect(detectProviderProtocolDialect('https://api.anthropic.com/v1/messages')).toBe(
      'anthropic_messages'
    )
    expect(detectProviderProtocolDialect('https://api.moonshot.cn/v1/chat/completions')).toBe(
      'openai_chat_completions'
    )
  })
})

describe('global API URL Generic rematch', () => {
  const baseModel: ModelConfig = {
    id: 'model',
    providerModelId: 'provider-model',
    displayName: 'Model',
    apiTokenOverrideStatus: 'missing',
    apiTokenOverrideMutation: { type: 'keep' },
    supportsImage: false,
    inputPrice: '0',
    cachedInputPrice: '',
    outputPrice: '0',
    providerProfileConfig: legacyProfile('generic_openai_chat'),
    providerProfileUpdate: { kind: 'unchanged' },
    enabled: true,
    execution: { status: 'available' }
  }

  it('rematches both V1 and V2 inherited Generic models but leaves vendor configs untouched', () => {
    const models: ModelConfig[] = [
      baseModel,
      {
        ...baseModel,
        id: 'generic-v2',
        providerProfileConfig: {
          schemaVersion: 2,
          vendorId: 'generic',
          profile: { id: 'generic_openai_chat', version: 1 },
          settings: { kind: 'generic' }
        }
      },
      {
        ...baseModel,
        id: 'deepseek',
        providerProfileConfig: {
          schemaVersion: 2,
          vendorId: 'deepseek',
          profile: { id: 'deepseek_v4_1_flash_chat', version: 1 },
          settings: {
            kind: 'deepseek_flash_chat',
            reasoning: { mode: 'enabled', effort: 'high' }
          }
        }
      }
    ]
    const updated = prepareModelsForGlobalApiUrlChange(models, descriptors)
    expect(updated[0]?.providerProfileUpdate).toEqual({ kind: 'select_generic' })
    expect(updated[1]?.providerProfileUpdate).toEqual({ kind: 'select_generic' })
    expect(updated[2]?.providerProfileUpdate).toEqual({ kind: 'unchanged' })
  })
})
