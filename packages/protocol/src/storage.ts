// Protocol layer.
import type { AgentInputAttachment, AgentPermissions, AgentPromptPreferences } from "./agent";

export const STORAGE_LOAD_APP_DATA_METHOD = "storage.loadAppData";
export const STORAGE_LOAD_MODEL_SETTINGS_METHOD = "storage.loadModelSettings";
export const STORAGE_SAVE_MODEL_SETTINGS_METHOD = "storage.saveModelSettings";
export const STORAGE_LOAD_AGENT_PROMPT_PREFERENCES_METHOD = "storage.loadAgentPromptPreferences";
export const STORAGE_SAVE_AGENT_PROMPT_PREFERENCES_METHOD = "storage.saveAgentPromptPreferences";
export const STORAGE_LOAD_PROJECTS_METHOD = "storage.loadProjects";
export const STORAGE_SAVE_PROJECT_METHOD = "storage.saveProject";
export const STORAGE_DELETE_PROJECT_METHOD = "storage.deleteProject";
export const STORAGE_SHOW_PROJECT_IN_FOLDER_METHOD = "storage.showProjectInFolder";
export const STORAGE_REVEAL_PROJECT_FILE_METHOD = "storage.revealProjectFile";
export const STORAGE_SELECT_PROJECT_DIRECTORY_METHOD = "storage.selectProjectDirectory";
export const STORAGE_LOAD_CONVERSATIONS_METHOD = "storage.loadConversations";
export const STORAGE_SAVE_CONVERSATION_METHOD = "storage.saveConversation";
export const STORAGE_SAVE_CONVERSATION_META_METHOD = "storage.saveConversationMeta";
export const STORAGE_DELETE_CONVERSATION_METHOD = "storage.deleteConversation";
export const STORAGE_DELETE_CHAT_MESSAGES_METHOD = "storage.deleteChatMessages";
export const STORAGE_UPSERT_CHAT_MESSAGES_METHOD = "storage.upsertChatMessages";
export const STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD = "storage.saveChatMessageState";
export const STORAGE_LOAD_COMPOSER_DRAFTS_METHOD = "storage.loadComposerDrafts";
export const STORAGE_SAVE_COMPOSER_DRAFT_METHOD = "storage.saveComposerDraft";
export const STORAGE_DELETE_COMPOSER_DRAFT_METHOD = "storage.deleteComposerDraft";
export const STORAGE_LOAD_UI_PREFERENCES_METHOD = "storage.loadUiPreferences";
export const STORAGE_SAVE_UI_PREFERENCES_METHOD = "storage.saveUiPreferences";
export const STORAGE_SELECT_PROFILE_AVATAR_METHOD = "storage.selectProfileAvatar";
export const STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD = "storage.loadAttachmentImage";
export const STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD = "storage.loadInputAttachments";

export interface StorageModelConfigRecord {
  id: string;
  displayName: string;
  shortName?: string | null;
  providerPath?: string | null;
  supportsImage: boolean;
  inputPrice: string;
  outputPrice: string;
  enabled: boolean;
}

export interface StorageModelSettingsRecord {
  apiUrl: string;
  apiToken: string;
  searchMode: string;
  tavilyApiKey: string;
  models: StorageModelConfigRecord[];
}

export interface StorageProjectRecord {
  id: string;
  name: string;
  path?: string | null;
  createdAt: number;
  pinnedAt?: number | null;
}

export interface StorageChatMessageAttachmentRecord {
  id: string;
  kind: "file" | "image" | (string & {});
  name: string;
  mimeType?: string | null;
  sizeBytes: number;
  previewData?: string | null;
  previewMimeType?: string | null;
  createdAt: number;
}

export interface StorageAttachmentImageRecord {
  id: string;
  name: string;
  mimeType: string;
  sizeBytes: number;
  data: string;
  createdAt: number;
}

export interface StorageImageFileRecord {
  name: string;
  mimeType: string;
  sizeBytes: number;
  data: string;
}

export interface StorageChatMessageRecord {
  id: string;
  role: "user" | "assistant" | (string & {});
  content: string;
  createdAt: number;
  status?: "pending" | "sent" | "error" | null;
  attachments?: StorageChatMessageAttachmentRecord[];
  agentRunJson?: string | null;
  uiStateJson?: string | null;
}

export interface StorageChatMessageStateRecord {
  id: string;
  content: string;
  status?: "pending" | "sent" | "error" | null;
  agentRunJson?: string | null;
  uiStateJson?: string | null;
}

export interface StorageChatConversationMetaRecord {
  id: string;
  projectId?: string | null;
  modelId?: string | null;
  title: string;
  createdAt: number;
  updatedAt: number;
  pinnedAt?: number | null;
  archivedAt?: number | null;
  unreadAt?: number | null;
}

export interface StorageChatConversationRecord extends StorageChatConversationMetaRecord {
  messages: StorageChatMessageRecord[];
}

export interface StorageComposerDraftRecord {
  scopeId: string;
  message: string;
  permissionMode: string;
  modelId?: string | null;
  projectId?: string | null;
  attachmentsJson: string;
  updatedAt: number;
}

export interface StorageUiPreferencesRecord {
  profileAvatarDataUrl?: string | null;
  profileDisplayName: string;
  profileHandle: string;
  sidebarConversationSort: string;
  sidebarProjectSort: string;
  sidebarProjectOrder: string[];
  sidebarSectionOrder: string;
  nativeFontSmoothing: boolean;
  showTokenUsageDetails: boolean;
  translucentSidebar: boolean;
  translucentSidebarTransparency: number;
  fullPermissionEnabled: boolean;
  customPermissionEnabled: boolean;
  customPermissions: AgentPermissions;
  updatedAt: number;
}

export interface StorageAgentPromptPreferencesRecord extends AgentPromptPreferences {
  workMode: "coding" | "general";
  tone: "friendly" | "pragmatic";
  detailLevel: "low" | "medium" | "high";
  customInstructions: string;
  updatedAt: number;
}

export interface StorageAppDataSnapshot {
  modelSettings: StorageModelSettingsRecord | null;
  projects: StorageProjectRecord[];
  conversations: StorageChatConversationRecord[];
  composerDrafts: StorageComposerDraftRecord[];
  uiPreferences: StorageUiPreferencesRecord;
  agentPromptPreferences: StorageAgentPromptPreferencesRecord;
}

export interface StorageDeleteProjectRequest {
  projectId: string;
}

export interface StorageProjectFileRequest {
  projectId?: string | null;
  filePath: string;
}

export interface StorageDeleteConversationRequest {
  conversationId: string;
}

export interface StorageDeleteChatMessagesRequest {
  conversationId: string;
  messageIds: string[];
}

export interface StorageUpsertChatMessagesRequest {
  conversationId: string;
  messages: StorageChatMessageRecord[];
  positionOffset: number;
}

export interface StorageSaveChatMessageStateRequest {
  conversationId: string;
  message: StorageChatMessageStateRecord;
}

export interface StorageDeleteComposerDraftRequest {
  scopeId: string;
}

export interface StorageSaveComposerDraftRequest {
  draft: StorageComposerDraftRecord;
}

export type StorageInputAttachment = AgentInputAttachment;

export interface StorageLoadInputAttachmentsRequest {
  attachmentIds: string[];
}
