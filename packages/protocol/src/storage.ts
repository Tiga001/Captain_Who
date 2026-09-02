import type { AgentInputAttachment, AgentPermissions, AgentPromptPreferences } from './agent'

export type KnownProviderProfileId =
  | 'generic_openai_chat'
  | 'generic_anthropic_messages'
  | 'deepseek_v4_chat'
  | 'deepseek_v4_vision'
  | 'moonshot_k3_chat'
  | 'moonshot_k2_7_code_chat'
  | 'moonshot_k2_6_chat'

/** Persisted ids are tolerant so unsupported future profiles can be displayed without reset. */
export type ProviderProfileId = KnownProviderProfileId | (string & {})

export type ProviderProtocolDialect = 'openai_chat_completions' | 'anthropic_messages'

export type KnownProviderVendorId = 'generic' | 'deepseek' | 'moonshot'

/** Persisted ids are tolerant; Host registration remains authoritative for support. */
export type ProviderVendorId = KnownProviderVendorId | (string & {})

export type ProviderReasoningMode = 'provider_default' | 'enabled' | 'disabled'
/** Legacy v1 Profile settings never accepted `low`. */
export type LegacyProviderReasoningEffort = 'provider_default' | 'high' | 'max'
/** Family-owned Profile v2 effort. */
export type ProviderReasoningEffort = 'provider_default' | 'low' | 'high' | 'max'
export type MoonshotK26ThinkingMode =
  'provider_default' | 'enabled' | 'disabled' | 'enabled_keep_all'

export interface ProviderFamilyReasoningPolicy {
  mode: ProviderReasoningMode
  effort: ProviderReasoningEffort
}

export interface ProviderProfileConfigV1 {
  schemaVersion: 1
  profile: {
    id: ProviderProfileId
    version: number
  }
  reasoning: {
    mode: ProviderReasoningMode
    effort: LegacyProviderReasoningEffort
  }
}

/** Explicit legacy name for consumers that project only the original Profile shape. */
export type LegacyProviderProfileConfigV1 = ProviderProfileConfigV1

export interface GenericProviderSettingsV1 {
  kind: 'generic'
}

export interface DeepSeekV4ChatProviderSettingsV1 {
  kind: 'deepseek_v4_chat'
  reasoning: ProviderFamilyReasoningPolicy
}

export interface DeepSeekV4VisionProviderSettingsV1 {
  kind: 'deepseek_v4_vision'
  reasoning: ProviderFamilyReasoningPolicy
}

export interface MoonshotK3ChatProviderSettingsV1 {
  kind: 'moonshot_k3_chat'
  reasoningEffort: ProviderReasoningEffort
}

export interface MoonshotK27CodeChatProviderSettingsV1 {
  kind: 'moonshot_k2_7_code_chat'
}

export interface MoonshotK26ChatProviderSettingsV1 {
  kind: 'moonshot_k2_6_chat'
  thinkingMode: MoonshotK26ThinkingMode
}

/** Versioned, model-family-owned settings used by Provider Profile config schema v2. */
export type ProviderFamilySettings =
  | GenericProviderSettingsV1
  | DeepSeekV4ChatProviderSettingsV1
  | DeepSeekV4VisionProviderSettingsV1
  | MoonshotK3ChatProviderSettingsV1
  | MoonshotK27CodeChatProviderSettingsV1
  | MoonshotK26ChatProviderSettingsV1

export type ProviderVendorPublicSettings = ProviderFamilySettings

export interface ProviderProfileConfigV2 {
  schemaVersion: 2
  vendorId: ProviderVendorId
  profile: {
    id: ProviderProfileId
    version: number
  }
  settings: ProviderFamilySettings
}

/**
 * Persisted Profile config. V1 remains losslessly readable and is retained by ordinary
 * `unchanged` saves. An explicit family-aware Provider selection writes V2; the Host may also
 * reconcile an exact Provider-owned official endpoint/model pair to its canonical V2 family.
 */
export type ProviderProfileConfig = ProviderProfileConfigV1 | ProviderProfileConfigV2

export interface ProviderVendorDescriptor {
  vendorId: ProviderVendorId
  displayName: string
  selectable: boolean
}

export type ProviderModelFamilyId =
  | 'generic_openai_chat'
  | 'generic_anthropic_messages'
  | 'deepseek_v4_chat'
  | 'deepseek_v4_vision'
  | 'moonshot_k3_chat'
  | 'moonshot_k2_7_code_chat'
  | 'moonshot_k2_6_chat'

export type ProviderVendorSettingsKind = 'none' | 'deepseek' | 'moonshot'

/** Public model-editor policy; runtime Provider capabilities remain Host-private. */
export type ProviderImageInputPolicy = 'user_configurable' | 'supported' | 'unsupported'

export type ProviderFamilySettingsDescriptor =
  | {
      kind: 'generic'
      defaultSettings: GenericProviderSettingsV1
    }
  | {
      kind: 'deepseek_v4_chat'
      reasoningModes: ProviderReasoningMode[]
      reasoningEfforts: ProviderReasoningEffort[]
      defaultSettings: DeepSeekV4ChatProviderSettingsV1
    }
  | {
      kind: 'deepseek_v4_vision'
      reasoningModes: ProviderReasoningMode[]
      reasoningEfforts: ProviderReasoningEffort[]
      defaultSettings: DeepSeekV4VisionProviderSettingsV1
    }
  | {
      kind: 'moonshot_k3_chat'
      reasoningEfforts: ProviderReasoningEffort[]
      defaultSettings: MoonshotK3ChatProviderSettingsV1
    }
  | {
      kind: 'moonshot_k2_7_code_chat'
      defaultSettings: MoonshotK27CodeChatProviderSettingsV1
    }
  | {
      kind: 'moonshot_k2_6_chat'
      thinkingModes: MoonshotK26ThinkingMode[]
      defaultSettings: MoonshotK26ChatProviderSettingsV1
    }

export interface ProviderVendorModelPolicyInput {
  vendorId: ProviderVendorId
  modelId: string
  dialect: ProviderProtocolDialect
}

/** Safe Host-authoritative model-family/settings projection for the settings UI. */
export type ProviderVendorModelPolicyDescriptor =
  | {
      status: 'supported'
      vendorId: ProviderVendorId
      modelFamily: ProviderModelFamilyId
      settingsKind: ProviderVendorSettingsKind
      imageInput: ProviderImageInputPolicy
      settings: ProviderFamilySettingsDescriptor
    }
  | {
      status: 'unsupported'
      vendorId: ProviderVendorId
      reason: 'unsupported_vendor' | 'unsupported_model' | 'unsupported_dialect'
    }

/**
 * Safe, read-only projection of one code-owned Provider Profile registration.
 *
 * Runtime capabilities are intentionally absent: Renderer may use this descriptor for selection
 * and presentation only, while Host remains authoritative for profile versions and behavior.
 */
export interface ProviderProfileUiDescriptor {
  profileId: ProviderProfileId
  profileVersion: number
  displayName: string
  compatibleDialects: ProviderProtocolDialect[]
  settingsKind: ProviderProfileSettingsKind
  selectable: boolean
}

export type ProviderProfileSettingsKind = 'none' | 'deepseek_v4_chat'

export interface DeepSeekV4ChatProviderSettings {
  kind: 'deepseek_v4_chat'
  reasoning: {
    mode: ProviderReasoningMode
    effort: LegacyProviderReasoningEffort
  }
}

/** Public settings accepted by a registered Provider Profile. */
export type ProviderProfileSettings = DeepSeekV4ChatProviderSettings

/**
 * Explicit profile mutation intent. Renderer never submits a profile version, protocol revision,
 * or runtime capability.
 */
export type StorageProviderProfileUpdate =
  | { kind: 'unchanged' }
  | { kind: 'select_generic' }
  | {
      kind: 'select_registered_profile'
      profileId: ProviderProfileId
      settings: ProviderProfileSettings
    }
  | {
      /** Host resolves the exact family/Profile/version from vendor, model id and dialect. */
      kind: 'select_vendor'
      vendorId: ProviderVendorId
      settings: ProviderFamilySettings
    }

export interface StorageModelConfigRecord {
  /** Stable opaque identity of this local model configuration. */
  id: string
  /** Exact identifier sent verbatim as the provider API's `model` value. */
  providerModelId: string
  /** Required, user-facing identity. Unique within local settings. */
  displayName: string
  /** A model-level connection override is valid only when URL and token are both present. */
  apiUrlOverride: string | null
  apiTokenOverride: string | null
  supportsImage: boolean
  contextWindowTokens: number | null
  /** Hidden provider wire configuration; settings UIs preserve this Host-owned current value. */
  providerProfileConfig: ProviderProfileConfig
  inputPrice: string
  /** Empty means cached input inherits inputPrice. */
  cachedInputPrice: string
  outputPrice: string
  enabled: boolean
}

export interface StorageModelSettingsRecord {
  apiUrl: string
  apiToken: string
  searchMode: string
  tavilyApiKey: string
  models: StorageModelConfigRecord[]
}

/** Model payload accepted by the authoritative Host save boundary. */
export interface StorageModelConfigUpdateRecord extends Omit<
  StorageModelConfigRecord,
  'id' | 'providerProfileConfig'
> {
  /** Existing local identity, or null when Host must allocate a new configuration identity. */
  id: string | null
  providerProfileUpdate: StorageProviderProfileUpdate
}

/**
 * Renderer-to-Host save request. The stored ProviderProfileConfig is deliberately not writable;
 * Host resolves explicit update intents through its code-owned registration.
 */
export interface StorageModelSettingsUpdateRecord extends Omit<
  StorageModelSettingsRecord,
  'models'
> {
  models: StorageModelConfigUpdateRecord[]
}

/** Safe, renderer-visible rejection from the authoritative model-settings save boundary. */
export interface StorageModelSettingsValidationErrorData {
  kind: 'model_settings_validation'
  code: 'duplicate_display_name'
  displayName: string
}

export interface StorageProjectRecord {
  id: string
  name: string
  path?: string | null
  createdAt: number
  pinnedAt?: number | null
}

export interface StorageChatMessageAttachmentRecord {
  id: string
  kind: 'file' | 'image' | (string & {})
  name: string
  mimeType?: string | null
  sizeBytes: number
  previewData?: string | null
  previewMimeType?: string | null
  createdAt: number
}

export interface StorageAttachmentImageRecord {
  id: string
  name: string
  mimeType: string
  sizeBytes: number
  data: string
  createdAt: number
}

export interface StorageImageFileRecord {
  name: string
  mimeType: string
  sizeBytes: number
  data: string
}

export interface StorageChatMessageRecord {
  id: string
  role: 'user' | 'assistant' | (string & {})
  content: string
  createdAt: number
  status?: 'pending' | 'sent' | 'error' | null
  attachments?: StorageChatMessageAttachmentRecord[]
  agentRunJson?: string | null
  uiStateJson?: string | null
}

export interface StorageChatMessageStateRecord {
  id: string
  content: string
  status?: 'pending' | 'sent' | 'error' | null
  agentRunJson?: string | null
}

export interface StorageChatMessageUiStateRecord {
  id: string
  uiStateJson?: string | null
}

export interface StorageChatConversationMetaRecord {
  id: string
  projectId?: string | null
  modelId?: string | null
  title: string
  createdAt: number
  updatedAt: number
  pinnedAt?: number | null
  archivedAt?: number | null
  unreadAt?: number | null
}

export interface StorageConversationContinuationOriginRecord {
  sourceConversationId: string
  sourceMessageId: string
  boundaryMessageId: string
}

export interface StorageChatConversationRecord extends StorageChatConversationMetaRecord {
  messages: StorageChatMessageRecord[]
  continuationOrigin?: StorageConversationContinuationOriginRecord | null
}

export type StorageConversationForkPoint =
  | {
      kind: 'assistant_reply'
      assistantMessageId: string
    }
  | {
      kind: 'provider_transition_boundary'
      operationId: string
    }

export interface StorageForkConversationRequest {
  requestId: string
  sourceConversationId: string
  forkPoint: StorageConversationForkPoint
}

/** Stable recovery metadata returned when Core rejects a conversation fork. */
export interface StorageForkConversationErrorData {
  type: 'conversation_fork'
  code: 'active_command_session'
  conversationId: string
  activeSessionCount: number
}

export interface StorageComposerDraftRecord {
  scopeId: string
  message: string
  permissionMode: string
  /** Version of the permission-mode semantics under which this choice was made. */
  permissionModeVersion: number
  modelId: string | null
  projectId: string | null
  attachmentsJson: string
  skillsJson: string
  queuedMessagesJson: string
  updatedAt: number
}

/** Lightweight autosave payload for text-only Composer edits. */
export interface StorageComposerDraftMessageUpdate {
  scopeId: string
  message: string
  updatedAt: number
}

export interface StorageUiPreferencesRecord {
  profileAvatarDataUrl?: string | null
  profileDisplayName: string
  profileHandle: string
  sidebarConversationSort: string
  sidebarProjectSort: string
  sidebarProjectOrder: string[]
  sidebarSectionOrder: string
  nativeFontSmoothing: boolean
  showTokenUsageDetails: boolean
  showContextWindowUsage: boolean
  translucentSidebar: boolean
  translucentSidebarTransparency: number
  fullPermissionEnabled: boolean
  customPermissionEnabled: boolean
  customPermissions: AgentPermissions
  updatedAt: number
}

export interface StorageAgentPromptPreferencesRecord extends AgentPromptPreferences {
  workMode: 'coding' | 'general'
  tone: 'friendly' | 'pragmatic'
  detailLevel: 'low' | 'medium' | 'high'
  customInstructions: string
  updatedAt: number
}

export interface StorageDeleteChatMessagesRequest {
  conversationId: string
  messageIds: string[]
}

export type StorageInputAttachment = AgentInputAttachment

export interface StorageLoadInputAttachmentsRequest {
  attachmentIds: string[]
}
