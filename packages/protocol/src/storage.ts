import type { AgentInputAttachment, AgentPermissions, AgentPromptPreferences } from './agent'
import type { HumanInteractionResponseDisplay } from './humanInteraction'

export type KnownProviderProfileId =
  | 'generic_openai_chat'
  | 'generic_anthropic_messages'
  | 'deepseek_v4_1_flash_chat'
  | 'deepseek_v4_pro_0813_chat'
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

/** Disabling reasoning cannot be combined with a wire-level reasoning effort. */
export type ProviderFamilyReasoningPolicy =
  | {
      mode: Exclude<ProviderReasoningMode, 'disabled'>
      effort: ProviderReasoningEffort
    }
  | {
      mode: 'disabled'
      effort: 'provider_default'
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

export interface GenericProviderSettingsV1 {
  kind: 'generic'
}

export interface DeepSeekFlashChatProviderSettingsV1 {
  kind: 'deepseek_flash_chat'
  reasoning: ProviderFamilyReasoningPolicy
}

export interface DeepSeekProChatProviderSettingsV1 {
  kind: 'deepseek_pro_chat'
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
  | DeepSeekFlashChatProviderSettingsV1
  | DeepSeekProChatProviderSettingsV1
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
 * Persisted Profile config. Generic profiles continue to use V1; vendor-owned profiles use the
 * family-aware V2 shape and are resolved authoritatively by the Host.
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
  | 'deepseek_flash_chat'
  | 'deepseek_pro_chat'
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
      kind: 'deepseek_flash_chat'
      reasoningModes: ProviderReasoningMode[]
      reasoningEfforts: ProviderReasoningEffort[]
      defaultSettings: DeepSeekFlashChatProviderSettingsV1
    }
  | {
      kind: 'deepseek_pro_chat'
      reasoningModes: ProviderReasoningMode[]
      reasoningEfforts: ProviderReasoningEffort[]
      defaultSettings: DeepSeekProChatProviderSettingsV1
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

export type ProviderProfileSettingsKind = 'none'

/** Presentation-safe state for a secret held by the Host credential store. */
export type CredentialStatus = 'missing' | 'configured' | 'unavailable'

/** Explicit write intent for a secret. Existing values are never returned to Renderer. */
export type CredentialMutation =
  { type: 'keep' } | { type: 'replace'; value: string } | { type: 'clear' }

/**
 * Explicit profile mutation intent. Renderer never submits a profile version, protocol revision,
 * or runtime capability.
 */
export type StorageProviderProfileUpdate =
  | { kind: 'unchanged' }
  | { kind: 'select_generic' }
  | {
      /** Host resolves the exact family/Profile/version from vendor, model id and dialect. */
      kind: 'select_vendor'
      vendorId: ProviderVendorId
      settings: ProviderFamilySettings
    }

/**
 * Host-authoritative execution projection of one configured model, computed once per settings
 * snapshot by the Host. Selectors, save validators, and settings editors offer or validate only
 * `available` models; the Renderer never re-derives availability itself.
 */
export type StorageModelExecutionStatus =
  { status: 'available' } | { status: 'unavailable'; reason: StorageModelUnavailableReason }

/** Why a configured model cannot execute right now; mirrors the Core reason enum. */
export type StorageModelUnavailableReason =
  | 'settings_missing'
  | 'not_found'
  | 'disabled'
  | 'invalid_connection'
  | 'invalid_profile'
  | 'missing_connection_identity'
  | 'missing_protocol_identity'
  | 'unsupported_runtime'
  | 'capabilities_changed'
  | 'credential_missing'
  | 'credential_unavailable'

export interface StorageModelConfigRecord {
  /** Stable opaque identity of this local model configuration. */
  id: string
  /** Exact identifier sent verbatim as the provider API's `model` value. */
  providerModelId: string
  /** Required, user-facing identity. Unique within local settings. */
  displayName: string
  /** A model-level connection override is valid only when URL and token are both present. */
  apiUrlOverride: string | null
  apiTokenOverrideStatus: CredentialStatus
  supportsImage: boolean
  contextWindowTokens: number | null
  /** Hidden provider wire configuration; settings UIs preserve this Host-owned current value. */
  providerProfileConfig: ProviderProfileConfig
  inputPrice: string
  /** Empty means cached input inherits inputPrice. */
  cachedInputPrice: string
  outputPrice: string
  enabled: boolean
  /**
   * Host-authoritative execution projection. Consumers offer or validate only `available`
   * models; the Renderer never re-derives availability from credential or URL fields.
   */
  execution: StorageModelExecutionStatus
}

export interface StorageModelSettingsRecord {
  /** Opaque Host revision used only to reject stale full-catalog saves. */
  configurationRevision: string
  apiUrl: string
  apiTokenStatus: CredentialStatus
  searchMode: string
  tavilyApiKeyStatus: CredentialStatus
  models: StorageModelConfigRecord[]
}

/** Model payload accepted by the authoritative Host save boundary. */
export interface StorageModelConfigUpdateRecord extends Omit<
  StorageModelConfigRecord,
  'id' | 'providerProfileConfig' | 'apiTokenOverrideStatus' | 'execution'
> {
  /** Existing local identity, or null when Host must allocate a new configuration identity. */
  id: string | null
  apiTokenOverrideMutation: CredentialMutation
  providerProfileUpdate: StorageProviderProfileUpdate
}

/**
 * Renderer-to-Host save request. The stored ProviderProfileConfig is deliberately not writable;
 * Host resolves explicit update intents through its code-owned registration.
 */
export interface StorageModelSettingsUpdateRecord extends Omit<
  StorageModelSettingsRecord,
  'configurationRevision' | 'apiTokenStatus' | 'tavilyApiKeyStatus' | 'models'
> {
  /** Null is accepted only when creating the first settings record. */
  expectedRevision: string | null
  /** Explicit model-editor Save must validate this existing model even if its draft is unchanged. */
  validateContextCapacityModelId?: string
  apiTokenMutation: CredentialMutation
  tavilyApiKeyMutation: CredentialMutation
  models: StorageModelConfigUpdateRecord[]
}

/** Safe, renderer-visible rejection from the authoritative model-settings save boundary. */
export type StorageModelSettingsValidationErrorData =
  | {
      kind: 'model_settings_validation'
      code: 'duplicate_display_name'
      displayName: string
    }
  | {
      kind: 'model_settings_validation'
      code: 'invalid_context_capacity_configuration'
      modelId: string
      displayName: string
      contextWindowTokens: number
      reservedOutputTokens: number
      safetyMarginTokens: number
      minimumContextWindowTokens: number
    }

/** Exactly one folder of a project is `primary`; it stays the working directory. */
export type StorageProjectFolderRole = 'primary' | 'auxiliary'

/** Upper bound on the folders a single project may reference. Mirrors Core. */
export const MAX_PROJECT_FOLDERS = 32

/**
 * One filesystem root of a project. `alias` is assigned once by Main when the folder is added
 * and stays stable; it is the project-unique name used to address the folder.
 */
export interface StorageProjectFolderRecord {
  id: string
  path: string
  alias: string
  role: StorageProjectFolderRole
  sortOrder: number
  createdAt: number
}

export interface StorageProjectRecord {
  id: string
  name: string
  /** Display order. Empty only for projects without a workspace. */
  folders: StorageProjectFolderRecord[]
  createdAt: number
  pinnedAt?: number | null
}

/** One folder the user picked through the native directory dialog. */
export interface StorageProjectFolderPick {
  path: string
  name: string
}

export interface StorageProjectFolderInput {
  /** Existing folder id when editing a project; omitted for folders that were just added. */
  id?: string | null
  path: string
  role: StorageProjectFolderRole
}

export interface StorageProjectCreateInput {
  name: string
  folders: StorageProjectFolderInput[]
}

export interface StorageProjectUpdateInput {
  projectId: string
  name: string
  folders: StorageProjectFolderInput[]
}

export type StorageProjectValidationCode =
  | 'name_required'
  | 'folders_required'
  | 'primary_required'
  | 'too_many_folders'
  | 'folder_missing'
  | 'folder_duplicate'
  | 'folder_nested'
  | 'project_missing'

/** Safe, renderer-visible rejection from the Main project create/update boundary. */
export interface StorageProjectValidationErrorData {
  kind: 'project_validation'
  code: StorageProjectValidationCode
  /** The offending folder path for folder-level codes. */
  path?: string
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
  /** Host-derived immutable display proof. It is never accepted by a message write API. */
  readonly humanInteractionResponse?: HumanInteractionResponseDisplay | null
  readonly workflowInput?: import('./workflowRuntime').WorkflowMessageSource | null
  id: string
  role: 'user' | 'assistant' | (string & {})
  content: string
  createdAt: number
  status?: 'pending' | 'sent' | 'error' | null
  attachments?: StorageChatMessageAttachmentRecord[]
  /** Serialized folder references, including Host-only binding metadata. */
  folderReferencesJson?: string | null
  agentRunJson?: string | null
  uiStateJson?: string | null
}

export type StorageChatMessageWriteRecord = Omit<
  StorageChatMessageRecord,
  'humanInteractionResponse' | 'workflowInput'
>

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
  | { kind: 'latest' }
  | {
      kind: 'assistant_reply'
      assistantMessageId: string
    }
  | {
      kind: 'provider_transition_boundary'
      operationId: string
    }
  | {
      kind: 'manual_compaction_boundary'
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
  folderReferencesJson: string
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
