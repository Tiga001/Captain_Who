import type {
  CredentialMutation,
  CredentialStatus,
  ProviderProfileConfig,
  ProviderProfileUiDescriptor,
  StorageModelExecutionStatus,
  StorageProviderProfileUpdate
} from '@mycopilot/protocol'

interface ModelConfigFields {
  /** Immutable local identity referenced by chats, automations, and Agent templates. */
  id: string
  /** Opaque provider identifier sent verbatim as the provider API's `model` value. */
  providerModelId: string
  /** Required, unique user-facing identity. It never changes provider routing. */
  displayName: string
  apiUrlOverride?: string
  apiTokenOverrideStatus: CredentialStatus
  apiTokenOverrideMutation: CredentialMutation
  supportsImage: boolean
  contextWindowTokens?: number
  /** One-shot, explicit profile mutation sent to the authoritative Host save boundary. */
  providerProfileUpdate: StorageProviderProfileUpdate
  inputPrice: string
  /** Empty means cached input is billed at inputPrice. */
  cachedInputPrice: string
  outputPrice: string
  enabled: boolean
}

/** A model returned by the Host's authoritative current-schema storage boundary. */
export interface ModelConfig extends ModelConfigFields {
  /** Host-authoritative, normalized Provider Profile used for safe presentation only. */
  providerProfileConfig: ProviderProfileConfig
  /**
   * Host-authoritative execution projection; the only availability judgement Renderer consumers
   * may offer or validate against.
   */
  execution: StorageModelExecutionStatus
}

/** A new model that has not yet received its versioned Profile from the Host. */
export interface ModelConfigSaveDraft extends Omit<ModelConfigFields, 'id'> {
  /** Host assigns the immutable local identity when a new model is first saved. */
  id: null
  providerProfileConfig?: never
}

export type SearchMode = 'auto' | 'disabled' | 'tavily'

export const DEFAULT_MODEL_CONTEXT_WINDOW_TOKENS = 128_000

export interface ModelFormValues {
  providerModelId: string
  displayName: string
  apiUrlOverride: string
  apiTokenOverrideStatus: CredentialStatus
  apiTokenOverrideMutation: CredentialMutation
  contextWindowTokens: string
  inputPrice: string
  cachedInputPrice: string
  outputPrice: string
  supportsImage: boolean
  providerProfileUpdate: StorageProviderProfileUpdate
}

function inheritsGlobalConnection(model: ModelConfig): boolean {
  return !model.apiUrlOverride?.trim() && model.apiTokenOverrideStatus === 'missing'
}

function hasRegisteredGenericProfile(
  model: ModelConfig,
  descriptors: readonly ProviderProfileUiDescriptor[]
): boolean {
  const config = model.providerProfileConfig
  if (
    config.schemaVersion === 2 &&
    (config.vendorId !== 'generic' || config.settings.kind !== 'generic')
  ) {
    return false
  }
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
    if (model.providerProfileUpdate.kind !== 'unchanged') {
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
    defaultUrl: ''
  },
  webSearch: {
    defaultMode: 'auto' satisfies SearchMode
  },
  defaults: {
    selectedModelId: ''
  }
} as const

export const INITIAL_MODEL_SAVE_DRAFTS: ModelConfigSaveDraft[] = []
