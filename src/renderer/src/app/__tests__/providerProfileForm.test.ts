import { describe, expect, it } from 'vitest'
import type { ProviderProfileConfig, ProviderProfileUiDescriptor } from '@mycopilot/protocol'
import { prepareModelsForGlobalApiUrlChange, type ModelConfig } from '../../config/modelConfig'
import {
  initialProviderProfileFormState,
  selectProviderProfile,
  updateDeepSeekProviderSettings
} from '../../features/settings/pages/configuration/providerProfileForm'

const descriptors: ProviderProfileUiDescriptor[] = [
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
    profileId: 'deepseek_v4_chat',
    profileVersion: 1,
    displayName: 'DeepSeek V4 Chat',
    compatibleDialects: ['openai_chat_completions'],
    settingsKind: 'deepseek_v4_chat',
    selectable: true
  }
]

function profile(
  id: ProviderProfileConfig['profile']['id'],
  version: number,
  mode: ProviderProfileConfig['reasoning']['mode'] = 'provider_default',
  effort: ProviderProfileConfig['reasoning']['effort'] = 'provider_default'
): ProviderProfileConfig {
  return { schemaVersion: 1, profile: { id, version }, reasoning: { mode, effort } }
}

describe('Provider Profile model form state', () => {
  it('shows the product Generic option and makes a missing legacy profile explicit on save', () => {
    const state = initialProviderProfileFormState(undefined, descriptors)

    expect(state.selection).toBe('generic')
    expect(state.update).toEqual({ kind: 'select_generic' })
  })

  it('preserves a supported stored profile until the user explicitly changes it', () => {
    const generic = initialProviderProfileFormState(
      profile('generic_anthropic_messages', 1),
      descriptors
    )
    const deepSeek = initialProviderProfileFormState(
      profile('deepseek_v4_chat', 1, 'enabled', 'high'),
      descriptors
    )

    expect(generic.update).toEqual({ kind: 'unchanged' })
    expect(deepSeek.update).toEqual({ kind: 'unchanged' })
  })

  it('shows an unsupported version without silently replacing it', () => {
    const state = initialProviderProfileFormState(
      profile('deepseek_v4_chat', 999, 'enabled', 'max'),
      descriptors
    )

    expect(state.selection).toBe('unsupported')
    expect(state.unsupportedProfile).toEqual({ id: 'deepseek_v4_chat', version: 999 })
    expect(state.update).toEqual({ kind: 'unchanged' })
    expect(selectProviderProfile(state, 'generic').update).toEqual({ kind: 'select_generic' })
  })

  it('treats an unknown config schema as unsupported even when its profile is registered', () => {
    const future = profile('deepseek_v4_chat', 1, 'enabled', 'max')
    future.schemaVersion = 99

    const state = initialProviderProfileFormState(future, descriptors)

    expect(state.selection).toBe('unsupported')
    expect(state.update).toEqual({ kind: 'unchanged' })
  })

  it('preserves an unknown future profile ID as unsupported presentation state', () => {
    const future = {
      schemaVersion: 1,
      profile: { id: 'future_vendor_chat', version: 7 }
    } as unknown as ProviderProfileConfig

    const state = initialProviderProfileFormState(future, descriptors)

    expect(state.selection).toBe('unsupported')
    expect(state.unsupportedProfile).toEqual({ id: 'future_vendor_chat', version: 7 })
    expect(state.update).toEqual({ kind: 'unchanged' })
  })

  it('normalizes disabled Thinking and emits only the public DeepSeek settings union', () => {
    const state = updateDeepSeekProviderSettings(
      selectProviderProfile(
        initialProviderProfileFormState(undefined, descriptors),
        'deepseek_v4_chat'
      ),
      { mode: 'disabled', effort: 'max' }
    )

    expect(state.reasoning).toEqual({ mode: 'disabled', effort: 'provider_default' })
    expect(state.update).toEqual({
      kind: 'select_registered_profile',
      profileId: 'deepseek_v4_chat',
      settings: {
        kind: 'deepseek_v4_chat',
        reasoning: { mode: 'disabled', effort: 'provider_default' }
      }
    })
    expect(JSON.stringify(state.update)).not.toMatch(/version|revision|capabilit/i)
  })

  it('drops the inactive DeepSeek draft when explicitly switching back to Generic', () => {
    const deepSeek = initialProviderProfileFormState(
      profile('deepseek_v4_chat', 1, 'enabled', 'max'),
      descriptors
    )
    const generic = selectProviderProfile(deepSeek, 'generic')

    expect(generic.reasoning).toEqual({
      mode: 'provider_default',
      effort: 'provider_default'
    })
    expect(generic.update).toEqual({ kind: 'select_generic' })
    expect(JSON.stringify(generic.update)).not.toContain('deepseek')
  })
})

describe('global API URL Provider Profile updates', () => {
  const baseModel: ModelConfig = {
    id: 'model',
    displayName: 'Model',
    supportsImage: false,
    inputPrice: '0',
    outputPrice: '0',
    enabled: true
  }

  it('requests Host Generic resolution only for inherited known Generic models', () => {
    const models: ModelConfig[] = [
      {
        ...baseModel,
        id: 'generic',
        providerProfileConfig: profile('generic_openai_chat', 1)
      },
      {
        ...baseModel,
        id: 'legacy-missing'
      },
      {
        ...baseModel,
        id: 'override',
        apiUrlOverride: 'https://override.example/v1',
        apiTokenOverride: 'token',
        providerProfileConfig: profile('generic_openai_chat', 1)
      },
      {
        ...baseModel,
        id: 'deepseek',
        providerProfileConfig: profile('deepseek_v4_chat', 1)
      },
      {
        ...baseModel,
        id: 'unsupported-version',
        providerProfileConfig: profile('generic_openai_chat', 99)
      }
    ]

    const updated = prepareModelsForGlobalApiUrlChange(models, descriptors)

    expect(updated.find((model) => model.id === 'generic')?.providerProfileUpdate).toEqual({
      kind: 'select_generic'
    })
    expect(updated.find((model) => model.id === 'legacy-missing')?.providerProfileUpdate).toEqual({
      kind: 'select_generic'
    })
    expect(updated.find((model) => model.id === 'override')?.providerProfileUpdate).toBeUndefined()
    expect(updated.find((model) => model.id === 'deepseek')?.providerProfileUpdate).toBeUndefined()
    expect(
      updated.find((model) => model.id === 'unsupported-version')?.providerProfileUpdate
    ).toBeUndefined()
  })

  it('does not overwrite an existing explicit Provider Profile update draft', () => {
    const registeredDraft: ModelConfig = {
      ...baseModel,
      id: 'registered-draft',
      providerProfileConfig: profile('generic_openai_chat', 1),
      providerProfileUpdate: {
        kind: 'select_registered_profile',
        profileId: 'deepseek_v4_chat',
        settings: {
          kind: 'deepseek_v4_chat',
          reasoning: { mode: 'enabled', effort: 'max' }
        }
      }
    }
    const genericDraft: ModelConfig = {
      ...baseModel,
      id: 'generic-draft',
      providerProfileConfig: profile('generic_openai_chat', 1),
      providerProfileUpdate: { kind: 'select_generic' }
    }

    const updated = prepareModelsForGlobalApiUrlChange([registeredDraft, genericDraft], descriptors)

    expect(updated[0]?.providerProfileUpdate).toEqual(registeredDraft.providerProfileUpdate)
    expect(updated[1]?.providerProfileUpdate).toEqual(genericDraft.providerProfileUpdate)
  })
})
