import type { AgentInputAttachment, AgentPermissions, AgentPromptPreferences } from './agent'

export type KnownProviderProfileId =
  'generic_openai_chat' | 'generic_anthropic_messages' | 'deepseek_v4_chat'

/** Persisted ids are tolerant so unsupported future profiles can be displayed without reset. */
export type ProviderProfileId = KnownProviderProfileId | (string & {})

export type ProviderProtocolDialect = 'openai_chat_completions' | 'anthropic_messages'

export type ProviderReasoningMode = 'provider_default' | 'enabled' | 'disabled'
export type ProviderReasoningEffort = 'provider_default' | 'high' | 'max'

export interface ProviderProfileConfig {
  schemaVersion: number
  profile: {
    id: ProviderProfileId
    version: number
  }
  reasoning: {
    mode: ProviderReasoningMode
    effort: ProviderReasoningEffort
  }
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
    effort: ProviderReasoningEffort
  }
}

/** Public settings accepted by a registered Provider Profile. */
export type ProviderProfileSettings = DeepSeekV4ChatProviderSettings

/**
 * Explicit profile mutation intent. Omission is equivalent to `unchanged` for older clients.
 * Renderer never submits a profile version, protocol revision, or runtime capability.
 */
export type StorageProviderProfileUpdate =
  | { kind: 'unchanged' }
  | { kind: 'select_generic' }
  | {
      kind: 'select_registered_profile'
      profileId: ProviderProfileId
      settings: ProviderProfileSettings
    }

export interface StorageModelConfigRecord {
  /** Opaque model identifier sent verbatim as the provider API's `model` value. */
  id: string
  displayName: string
  /** A model-level connection override is valid only when URL and token are both present. */
  apiUrlOverride?: string | null
  apiTokenOverride?: string | null
  supportsImage: boolean
  contextWindowTokens?: number | null
  /** Hidden provider wire configuration; settings UIs must preserve it even before exposing it. */
  providerProfileConfig?: ProviderProfileConfig | null
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
  'providerProfileConfig'
> {
  /** Previous opaque model identity used to preserve profile state across an explicit rename. */
  previousModelId?: string
  /** Missing means preserve the currently saved profile and settings. */
  providerProfileUpdate?: StorageProviderProfileUpdate
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
  uiStateJson?: string | null
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

interface StorageForkConversationRequestBase {
  requestId: string
  sourceConversationId: string
}

export type StorageForkConversationRequest = StorageForkConversationRequestBase &
  (
    | {
        forkPoint: StorageConversationForkPoint
        throughAssistantMessageId?: never
      }
    | {
        /** Legacy wire shape. New clients must send an explicit forkPoint. */
        throughAssistantMessageId: string
        forkPoint?: never
      }
  )

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
  /**
   * Version of the permission-mode semantics under which this choice was made.
   * Missing/zero values are legacy records and must not grant upgraded privileges.
   */
  permissionModeVersion?: number
  modelId?: string | null
  projectId?: string | null
  attachmentsJson: string
  skillsJson: string
  queuedMessagesJson?: string
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
