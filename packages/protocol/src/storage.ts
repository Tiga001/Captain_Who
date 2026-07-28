import type { AgentInputAttachment, AgentPermissions, AgentPromptPreferences } from './agent'

export interface StorageModelConfigRecord {
  /** Opaque model identifier sent verbatim as the provider API's `model` value. */
  id: string
  displayName: string
  /** A model-level connection override is valid only when URL and token are both present. */
  apiUrlOverride?: string | null
  apiTokenOverride?: string | null
  supportsImage: boolean
  contextWindowTokens?: number | null
  inputPrice: string
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

export interface StorageForkConversationRequest {
  requestId: string
  sourceConversationId: string
  throughAssistantMessageId: string
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
