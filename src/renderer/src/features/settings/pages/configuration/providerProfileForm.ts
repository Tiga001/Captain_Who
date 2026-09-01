import type {
  LegacyProviderReasoningEffort,
  ProviderProfileConfig,
  ProviderProfileUiDescriptor,
  ProviderReasoningMode,
  StorageProviderProfileUpdate
} from '@mycopilot/protocol'

export type ProviderProfileSelection = 'generic' | 'deepseek_v4_chat' | 'unsupported'

export interface ProviderProfileFormState {
  selection: ProviderProfileSelection
  reasoning: {
    mode: ProviderReasoningMode
    effort: LegacyProviderReasoningEffort
  }
  update: StorageProviderProfileUpdate
  unsupportedProfile?: {
    id: string
    version: number
  }
}

const DEFAULT_REASONING = {
  mode: 'provider_default',
  effort: 'provider_default'
} as const

function isGenericProfileId(profileId: string): boolean {
  return profileId === 'generic_openai_chat' || profileId === 'generic_anthropic_messages'
}

function matchingDescriptor(
  profile: ProviderProfileConfig['profile'],
  descriptors: readonly ProviderProfileUiDescriptor[]
): ProviderProfileUiDescriptor | undefined {
  return descriptors.find(
    (descriptor) =>
      descriptor.profileId === profile.id && descriptor.profileVersion === profile.version
  )
}

export function normalizeDeepSeekReasoning(reasoning: {
  mode: ProviderReasoningMode
  effort: LegacyProviderReasoningEffort
}): ProviderProfileFormState['reasoning'] {
  if (reasoning.mode !== 'disabled') return { ...reasoning }
  return { mode: 'disabled', effort: 'provider_default' }
}

function deepSeekReasoningFromStoredConfig(
  config: ProviderProfileConfig
): ProviderProfileFormState['reasoning'] | null {
  if (config.schemaVersion === 1) return normalizeDeepSeekReasoning(config.reasoning)
  if (config.vendorId !== 'deepseek') return null
  if (config.settings.kind !== 'deepseek_v4_chat') return null
  const reasoning = config.settings.reasoning
  const effort = reasoning.effort
  if (effort === 'low') return null
  return normalizeDeepSeekReasoning({ mode: reasoning.mode, effort })
}

export function initialNewProviderProfileFormState(): ProviderProfileFormState {
  return {
    selection: 'generic',
    reasoning: { ...DEFAULT_REASONING },
    update: { kind: 'select_generic' }
  }
}

export function initialProviderProfileFormState(
  config: ProviderProfileConfig,
  descriptors: readonly ProviderProfileUiDescriptor[]
): ProviderProfileFormState {
  const profileId = String(config.profile.id)
  const schemaVersion: number = config.schemaVersion
  if (schemaVersion !== 1 && schemaVersion !== 2) {
    return {
      selection: 'unsupported',
      reasoning: { ...DEFAULT_REASONING },
      update: { kind: 'unchanged' },
      unsupportedProfile: { id: profileId, version: config.profile.version }
    }
  }
  const descriptor = matchingDescriptor(config.profile, descriptors)
  const deepSeekReasoning = deepSeekReasoningFromStoredConfig(config)
  if (!descriptor) {
    return {
      selection: 'unsupported',
      reasoning: deepSeekReasoning ?? { ...DEFAULT_REASONING },
      update: { kind: 'unchanged' },
      unsupportedProfile: { id: profileId, version: config.profile.version }
    }
  }

  if (isGenericProfileId(profileId)) {
    if (
      config.schemaVersion === 2 &&
      (config.vendorId !== 'generic' || config.settings.kind !== 'generic')
    ) {
      return {
        selection: 'unsupported',
        reasoning: { ...DEFAULT_REASONING },
        update: { kind: 'unchanged' },
        unsupportedProfile: { id: profileId, version: config.profile.version }
      }
    }
    return {
      selection: 'generic',
      reasoning: { ...DEFAULT_REASONING },
      update: { kind: 'unchanged' }
    }
  }

  if (
    descriptor.profileId === 'deepseek_v4_chat' &&
    descriptor.settingsKind === 'deepseek_v4_chat' &&
    descriptor.selectable &&
    deepSeekReasoning
  ) {
    return {
      selection: 'deepseek_v4_chat',
      reasoning: deepSeekReasoning,
      update: { kind: 'unchanged' }
    }
  }

  return {
    selection: 'unsupported',
    reasoning: deepSeekReasoning ?? { ...DEFAULT_REASONING },
    update: { kind: 'unchanged' },
    unsupportedProfile: { id: profileId, version: config.profile.version }
  }
}

export function selectProviderProfile(
  current: ProviderProfileFormState,
  selection: Exclude<ProviderProfileSelection, 'unsupported'>
): ProviderProfileFormState {
  if (selection === 'generic') {
    return {
      selection,
      reasoning: { ...DEFAULT_REASONING },
      update: { kind: 'select_generic' }
    }
  }

  const reasoning =
    current.selection === 'deepseek_v4_chat'
      ? normalizeDeepSeekReasoning(current.reasoning)
      : { ...DEFAULT_REASONING }
  return {
    selection,
    reasoning,
    update: {
      kind: 'select_registered_profile',
      profileId: 'deepseek_v4_chat',
      settings: {
        kind: 'deepseek_v4_chat',
        reasoning
      }
    }
  }
}

export function updateDeepSeekProviderSettings(
  current: ProviderProfileFormState,
  reasoning: ProviderProfileFormState['reasoning']
): ProviderProfileFormState {
  const normalizedReasoning = normalizeDeepSeekReasoning(reasoning)
  return {
    ...current,
    selection: 'deepseek_v4_chat',
    reasoning: normalizedReasoning,
    update: {
      kind: 'select_registered_profile',
      profileId: 'deepseek_v4_chat',
      settings: {
        kind: 'deepseek_v4_chat',
        reasoning: normalizedReasoning
      }
    },
    unsupportedProfile: undefined
  }
}
