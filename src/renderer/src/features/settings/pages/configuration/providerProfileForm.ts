import type {
  ProviderFamilySettings,
  ProviderFamilySettingsDescriptor,
  ProviderModelFamilyId,
  ProviderProfileConfig,
  ProviderProfileUiDescriptor,
  ProviderVendorModelPolicyDescriptor,
  StorageProviderProfileUpdate
} from '@mycopilot/protocol'

export type ProviderProfileSelection = 'generic' | 'deepseek' | 'moonshot' | 'unsupported'
export type SelectableProviderVendor = Exclude<ProviderProfileSelection, 'unsupported'>
export type DeepSeekFamilySettings = Extract<
  ProviderFamilySettings,
  { kind: 'deepseek_v4_chat' | 'deepseek_v4_vision' }
>
export type MoonshotFamilySettings = Extract<
  ProviderFamilySettings,
  {
    kind: 'moonshot_k3_chat' | 'moonshot_k2_7_code_chat' | 'moonshot_k2_6_chat'
  }
>

interface ProviderProfileFormStateBase {
  modelFamily: ProviderModelFamilyId | null
  update: StorageProviderProfileUpdate
  explicitSelection: boolean
  familyChanged: boolean
}

export type ProviderProfileFormState =
  | (ProviderProfileFormStateBase & {
      selection: 'generic'
      settings: Extract<ProviderFamilySettings, { kind: 'generic' }>
    })
  | (ProviderProfileFormStateBase & {
      selection: 'deepseek'
      settings: DeepSeekFamilySettings | null
    })
  | (ProviderProfileFormStateBase & {
      selection: 'moonshot'
      settings: MoonshotFamilySettings | null
    })
  | (ProviderProfileFormStateBase & {
      selection: 'unsupported'
      settings: null
    })

function cloneSettings<Settings extends ProviderFamilySettings>(settings: Settings): Settings {
  return structuredClone(settings)
}

function matchingRegisteredProfileDescriptor(
  config: ProviderProfileConfig,
  descriptors: readonly ProviderProfileUiDescriptor[]
): boolean {
  return descriptors.some(
    (descriptor) =>
      descriptor.profileId === config.profile.id &&
      descriptor.profileVersion === config.profile.version
  )
}

function unchangedBase(modelFamily: ProviderModelFamilyId | null): ProviderProfileFormStateBase {
  return {
    modelFamily,
    update: { kind: 'unchanged' },
    explicitSelection: false,
    familyChanged: false
  }
}

function initialV2State(
  config: Extract<ProviderProfileConfig, { schemaVersion: 2 }>,
  descriptors: readonly ProviderProfileUiDescriptor[]
): ProviderProfileFormState {
  if (!matchingRegisteredProfileDescriptor(config, descriptors)) {
    return { ...unchangedBase(null), selection: 'unsupported', settings: null }
  }
  const base = unchangedBase(config.profile.id as ProviderModelFamilyId)
  if (
    config.vendorId === 'generic' &&
    config.settings.kind === 'generic' &&
    (config.profile.id === 'generic_openai_chat' ||
      config.profile.id === 'generic_anthropic_messages')
  ) {
    return { ...base, selection: 'generic', settings: { kind: 'generic' } }
  }
  if (
    config.vendorId === 'deepseek' &&
    (config.settings.kind === 'deepseek_v4_chat' ||
      config.settings.kind === 'deepseek_v4_vision') &&
    config.profile.id === config.settings.kind
  ) {
    return { ...base, selection: 'deepseek', settings: cloneSettings(config.settings) }
  }
  if (
    config.vendorId === 'moonshot' &&
    (config.settings.kind === 'moonshot_k3_chat' ||
      config.settings.kind === 'moonshot_k2_7_code_chat' ||
      config.settings.kind === 'moonshot_k2_6_chat') &&
    config.profile.id === config.settings.kind
  ) {
    return { ...base, selection: 'moonshot', settings: cloneSettings(config.settings) }
  }
  return { ...unchangedBase(null), selection: 'unsupported', settings: null }
}

export function initialNewProviderProfileFormState(): ProviderProfileFormState {
  return {
    selection: 'generic',
    settings: { kind: 'generic' },
    modelFamily: null,
    update: { kind: 'select_generic' },
    explicitSelection: true,
    familyChanged: false
  }
}

export function initialProviderProfileFormState(
  config: ProviderProfileConfig,
  descriptors: readonly ProviderProfileUiDescriptor[]
): ProviderProfileFormState {
  if (config.schemaVersion === 2) return initialV2State(config, descriptors)
  if (config.schemaVersion !== 1 || !matchingRegisteredProfileDescriptor(config, descriptors)) {
    return { ...unchangedBase(null), selection: 'unsupported', settings: null }
  }

  if (
    config.profile.id === 'generic_openai_chat' ||
    config.profile.id === 'generic_anthropic_messages'
  ) {
    return {
      ...unchangedBase(config.profile.id as 'generic_openai_chat' | 'generic_anthropic_messages'),
      selection: 'generic',
      settings: { kind: 'generic' }
    }
  }
  if (config.profile.id === 'deepseek_v4_chat') {
    return {
      ...unchangedBase('deepseek_v4_chat'),
      selection: 'deepseek',
      settings: {
        kind: 'deepseek_v4_chat',
        reasoning: { ...config.reasoning }
      }
    }
  }
  return { ...unchangedBase(null), selection: 'unsupported', settings: null }
}

export function selectProviderVendor(
  current: ProviderProfileFormState,
  selection: SelectableProviderVendor
): ProviderProfileFormState {
  if (selection === 'generic') {
    return {
      selection,
      settings: { kind: 'generic' },
      modelFamily: current.modelFamily,
      update: { kind: 'select_generic' },
      explicitSelection: true,
      familyChanged: false
    }
  }
  if (selection === 'deepseek') {
    return {
      selection,
      settings: current.selection === 'deepseek' ? current.settings : null,
      modelFamily: current.modelFamily,
      update: current.selection === 'deepseek' ? current.update : { kind: 'unchanged' },
      explicitSelection: true,
      familyChanged: false
    }
  }
  return {
    selection,
    settings: current.selection === 'moonshot' ? current.settings : null,
    modelFamily: current.modelFamily,
    update: current.selection === 'moonshot' ? current.update : { kind: 'unchanged' },
    explicitSelection: true,
    familyChanged: false
  }
}

function normalizeDeepSeekSettings(
  settings: DeepSeekFamilySettings,
  descriptor: Extract<
    ProviderFamilySettingsDescriptor,
    { kind: 'deepseek_v4_chat' | 'deepseek_v4_vision' }
  >
): DeepSeekFamilySettings {
  if (settings.kind !== descriptor.kind) return cloneSettings(descriptor.defaultSettings)
  const mode = descriptor.reasoningModes.includes(settings.reasoning.mode)
    ? settings.reasoning.mode
    : descriptor.defaultSettings.reasoning.mode
  let effort = descriptor.reasoningEfforts.includes(settings.reasoning.effort)
    ? settings.reasoning.effort
    : descriptor.defaultSettings.reasoning.effort
  if (mode === 'disabled') effort = 'provider_default'
  return { kind: descriptor.kind, reasoning: { mode, effort } } as DeepSeekFamilySettings
}

function normalizeMoonshotSettings(
  settings: MoonshotFamilySettings,
  descriptor: Extract<
    ProviderFamilySettingsDescriptor,
    {
      kind: 'moonshot_k3_chat' | 'moonshot_k2_7_code_chat' | 'moonshot_k2_6_chat'
    }
  >
): MoonshotFamilySettings {
  if (settings.kind !== descriptor.kind) return cloneSettings(descriptor.defaultSettings)
  if (descriptor.kind === 'moonshot_k3_chat' && settings.kind === 'moonshot_k3_chat') {
    return {
      kind: descriptor.kind,
      reasoningEffort: descriptor.reasoningEfforts.includes(settings.reasoningEffort)
        ? settings.reasoningEffort
        : descriptor.defaultSettings.reasoningEffort
    }
  }
  if (descriptor.kind === 'moonshot_k2_6_chat' && settings.kind === 'moonshot_k2_6_chat') {
    return {
      kind: descriptor.kind,
      thinkingMode: descriptor.thinkingModes.includes(settings.thinkingMode)
        ? settings.thinkingMode
        : descriptor.defaultSettings.thinkingMode
    }
  }
  return cloneSettings(descriptor.defaultSettings)
}

export function applyResolvedProviderPolicy(
  current: ProviderProfileFormState,
  policy: ProviderVendorModelPolicyDescriptor
): ProviderProfileFormState {
  if (
    policy.status !== 'supported' ||
    current.selection === 'unsupported' ||
    current.selection !== policy.vendorId
  ) {
    return current
  }

  const familyChanged = current.modelFamily !== null && current.modelFamily !== policy.modelFamily
  const mustWrite =
    current.explicitSelection || familyChanged || current.update.kind !== 'unchanged'
  if (current.selection === 'generic' && policy.settings.kind === 'generic') {
    return {
      selection: 'generic',
      settings: { kind: 'generic' },
      modelFamily: policy.modelFamily,
      update: mustWrite ? { kind: 'select_generic' } : { kind: 'unchanged' },
      explicitSelection: current.explicitSelection,
      familyChanged
    }
  }
  if (
    current.selection === 'deepseek' &&
    (policy.settings.kind === 'deepseek_v4_chat' || policy.settings.kind === 'deepseek_v4_vision')
  ) {
    const settings =
      current.settings && current.modelFamily === policy.modelFamily
        ? normalizeDeepSeekSettings(current.settings, policy.settings)
        : cloneSettings(policy.settings.defaultSettings)
    return {
      selection: 'deepseek',
      settings,
      modelFamily: policy.modelFamily,
      update: mustWrite
        ? { kind: 'select_vendor', vendorId: 'deepseek', settings }
        : { kind: 'unchanged' },
      explicitSelection: current.explicitSelection,
      familyChanged
    }
  }
  if (
    current.selection === 'moonshot' &&
    (policy.settings.kind === 'moonshot_k3_chat' ||
      policy.settings.kind === 'moonshot_k2_7_code_chat' ||
      policy.settings.kind === 'moonshot_k2_6_chat')
  ) {
    const settings =
      current.settings && current.modelFamily === policy.modelFamily
        ? normalizeMoonshotSettings(current.settings, policy.settings)
        : cloneSettings(policy.settings.defaultSettings)
    return {
      selection: 'moonshot',
      settings,
      modelFamily: policy.modelFamily,
      update: mustWrite
        ? { kind: 'select_vendor', vendorId: 'moonshot', settings }
        : { kind: 'unchanged' },
      explicitSelection: current.explicitSelection,
      familyChanged
    }
  }
  return current
}

export function updateDeepSeekProviderSettings(
  current: ProviderProfileFormState,
  settings: DeepSeekFamilySettings
): ProviderProfileFormState {
  if (current.selection !== 'deepseek') return current
  const nextSettings = cloneSettings(settings)
  return {
    ...current,
    settings: nextSettings,
    update: { kind: 'select_vendor', vendorId: 'deepseek', settings: nextSettings }
  }
}

export function updateMoonshotProviderSettings(
  current: ProviderProfileFormState,
  settings: MoonshotFamilySettings
): ProviderProfileFormState {
  if (current.selection !== 'moonshot') return current
  const nextSettings = cloneSettings(settings)
  return {
    ...current,
    settings: nextSettings,
    update: { kind: 'select_vendor', vendorId: 'moonshot', settings: nextSettings }
  }
}

export function detectProviderProtocolDialect(apiUrl: string) {
  const normalized = apiUrl.trim().toLowerCase()
  return normalized.includes('anthropic') || normalized.endsWith('/messages')
    ? ('anthropic_messages' as const)
    : ('openai_chat_completions' as const)
}
