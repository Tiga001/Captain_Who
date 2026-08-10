import type {
  ProviderProfileConfig,
  ProviderProfileUiDescriptor,
  StorageProviderProfileUpdate
} from '@mycopilot/protocol'

export interface ModelConfig {
  /** Opaque model identifier sent verbatim as the provider API's `model` value. */
  id: string
  /** User-facing label. It never changes provider routing. */
  displayName: string
  apiUrlOverride?: string
  apiTokenOverride?: string
  supportsImage: boolean
  contextWindowTokens?: number
  /** Host-authoritative, normalized Provider Profile used for safe presentation only. */
  providerProfileConfig?: ProviderProfileConfig
  /** One-shot, explicit profile mutation sent to the authoritative Host save boundary. */
  providerProfileUpdate?: StorageProviderProfileUpdate
  /** One-shot previous identity used by Host while saving an edited model rename. */
  previousModelId?: string
  inputPrice: string
  outputPrice: string
  enabled: boolean
}

export type SearchMode = 'auto' | 'disabled' | 'tavily'

export const DEFAULT_MODEL_CONTEXT_WINDOW_TOKENS = 128_000

export interface ModelFormValues {
  id: string
  displayName: string
  apiUrlOverride: string
  apiTokenOverride: string
  contextWindowTokens: string
  inputPrice: string
  outputPrice: string
  supportsImage: boolean
  providerProfileUpdate?: StorageProviderProfileUpdate
}

function hasCompleteConnectionPair(apiUrl: string | undefined, apiToken: string | undefined) {
  const normalizedUrl = apiUrl?.trim() ?? ''
  if (!normalizedUrl || !apiToken?.trim()) return false

  try {
    const parsed = new URL(normalizedUrl)
    return parsed.protocol === 'http:' || parsed.protocol === 'https:'
  } catch {
    return false
  }
}

export function isModelConnectionAvailable(
  model: ModelConfig,
  globalApiUrl: string,
  globalApiToken: string
): boolean {
  const overrideUrl = model.apiUrlOverride?.trim() ?? ''
  const overrideToken = model.apiTokenOverride?.trim() ?? ''

  // A model either supplies a complete override or inherits the complete global pair.
  // Never mix one model-level value with one global value, because that can target the
  // wrong provider with the wrong credential.
  if (overrideUrl.length > 0 || overrideToken.length > 0) {
    return hasCompleteConnectionPair(overrideUrl, overrideToken)
  }

  return hasCompleteConnectionPair(globalApiUrl, globalApiToken)
}

function inheritsGlobalConnection(model: ModelConfig): boolean {
  return !model.apiUrlOverride?.trim() && !model.apiTokenOverride?.trim()
}

function hasRegisteredGenericProfile(
  model: ModelConfig,
  descriptors: readonly ProviderProfileUiDescriptor[]
): boolean {
  const config = model.providerProfileConfig
  if (!config) return true
  if (config.schemaVersion !== 1) return false
  const profileId = String(config.profile.id)
  if (profileId !== 'generic_openai_chat' && profileId !== 'generic_anthropic_messages') {
    return false
  }
  return descriptors.some(
    (descriptor) =>
      descriptor.profileId === config.profile.id &&
      descriptor.profileVersion === config.profile.version &&
      descriptor.settingsKind === 'none'
  )
}

/**
 * Atomically asks Host to resolve inherited Generic models against a user-edited global API URL.
 * Renderer never chooses the resulting dialect, Profile version, or runtime policy.
 */
export function prepareModelsForGlobalApiUrlChange(
  models: readonly ModelConfig[],
  descriptors: readonly ProviderProfileUiDescriptor[]
): ModelConfig[] {
  return models.map((model) => {
    if (!inheritsGlobalConnection(model) || !hasRegisteredGenericProfile(model, descriptors)) {
      return model
    }
    if (model.providerProfileUpdate && model.providerProfileUpdate.kind !== 'unchanged') {
      return model
    }
    return {
      ...model,
      providerProfileUpdate: { kind: 'select_generic' }
    }
  })
}

export const modelConfig = {
  api: {
    defaultUrl: '',
    defaultToken: ''
  },
  webSearch: {
    defaultMode: 'auto' satisfies SearchMode,
    defaultTavilyApiKey: ''
  },
  defaults: {
    selectedModelId: 'gpt-5.5'
  },
  models: [
    {
      id: 'claude-opus-4-7',
      displayName: 'claude-opus-4-7',
      supportsImage: true,
      inputPrice: '0.028',
      outputPrice: '0.14',
      enabled: true
    },
    {
      id: 'claude-sonnet-4.6',
      displayName: 'claude-sonnet-4.6',
      supportsImage: true,
      inputPrice: '0.0168',
      outputPrice: '0.084',
      enabled: true
    },
    {
      id: 'gpt-5.5',
      displayName: 'gpt-5.5',
      supportsImage: true,
      inputPrice: '0.021',
      outputPrice: '0.126',
      enabled: true
    },
    {
      id: 'deepseek/deepseek-v4-pro',
      displayName: 'deepseek/deepseek-v4-pro',
      supportsImage: false,
      inputPrice: '0.012',
      outputPrice: '0.024',
      enabled: true
    },
    {
      id: 'deepseek/deepseek-v4-flash',
      displayName: 'deepseek/deepseek-v4-flash',
      supportsImage: false,
      inputPrice: '0.00105',
      outputPrice: '0.0021',
      enabled: true
    },
    {
      id: 'minimax/minimax-m2.5',
      displayName: 'minimax/minimax-m2.5',
      supportsImage: false,
      inputPrice: '0.001407',
      outputPrice: '0.005628',
      enabled: true
    }
  ] satisfies ModelConfig[]
} as const

export const INITIAL_MODELS: ModelConfig[] = modelConfig.models.map((model) => ({ ...model }))
