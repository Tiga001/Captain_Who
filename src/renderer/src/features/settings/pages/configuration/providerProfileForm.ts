import type {
  ProviderProfileConfig,
  ProviderProfileUiDescriptor,
  ProviderReasoningEffort,
  ProviderReasoningMode,
  StorageProviderProfileUpdate
} from '@mycopilot/protocol'

export type ProviderProfileSelection = 'generic' | 'deepseek_v4_chat' | 'unsupported'

export interface ProviderProfileFormState {
  selection: ProviderProfileSelection
  reasoning: {
    mode: ProviderReasoningMode
    effort: ProviderReasoningEffort
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

const REASONING_MODES: readonly ProviderReasoningMode[] = [
  'provider_default',
  'enabled',
  'disabled'
]
const REASONING_EFFORTS: readonly ProviderReasoningEffort[] = ['provider_default', 'high', 'max']

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
  effort: ProviderReasoningEffort
}): ProviderProfileFormState['reasoning'] {
  if (reasoning.mode !== 'disabled') return { ...reasoning }
  return { mode: 'disabled', effort: 'provider_default' }
}

function reasoningFromStoredConfig(
  config: ProviderProfileConfig
): ProviderProfileFormState['reasoning'] {
  const candidate = (config as { reasoning?: { mode?: unknown; effort?: unknown } }).reasoning
  if (
    !candidate ||
    !REASONING_MODES.includes(candidate.mode as ProviderReasoningMode) ||
    !REASONING_EFFORTS.includes(candidate.effort as ProviderReasoningEffort)
  ) {
    return { ...DEFAULT_REASONING }
  }
  return normalizeDeepSeekReasoning({
    mode: candidate.mode as ProviderReasoningMode,
    effort: candidate.effort as ProviderReasoningEffort
  })
}

export function initialProviderProfileFormState(
  config: ProviderProfileConfig | undefined,
  descriptors: readonly ProviderProfileUiDescriptor[]
): ProviderProfileFormState {
  if (!config) {
    return {
      selection: 'generic',
      reasoning: { ...DEFAULT_REASONING },
      // A current Renderer makes the legacy/no-profile effective default explicit on save.
      // Older clients that omit this update still retain the Host's `unchanged` semantics.
      update: { kind: 'select_generic' }
    }
  }

  const profileId = String(config.profile.id)
  if (config.schemaVersion !== 1) {
    return {
      selection: 'unsupported',
      reasoning: { ...DEFAULT_REASONING },
      update: { kind: 'unchanged' },
      unsupportedProfile: { id: profileId, version: config.profile.version }
    }
  }
  const descriptor = matchingDescriptor(config.profile, descriptors)
  if (!descriptor) {
    return {
      selection: 'unsupported',
      reasoning: reasoningFromStoredConfig(config),
      update: { kind: 'unchanged' },
      unsupportedProfile: { id: profileId, version: config.profile.version }
    }
  }

  if (isGenericProfileId(profileId)) {
    return {
      selection: 'generic',
      reasoning: { ...DEFAULT_REASONING },
      update: { kind: 'unchanged' }
    }
  }

  if (
    descriptor.profileId === 'deepseek_v4_chat' &&
    descriptor.settingsKind === 'deepseek_v4_chat' &&
    descriptor.selectable
  ) {
    return {
      selection: 'deepseek_v4_chat',
      reasoning: reasoningFromStoredConfig(config),
      update: { kind: 'unchanged' }
    }
  }

  return {
    selection: 'unsupported',
    reasoning: reasoningFromStoredConfig(config),
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
