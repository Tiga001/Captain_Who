// Renderer UI.
import type { AgentInputAttachment, AgentPermissions, AgentPromptPreferences } from "@mycopilot/protocol";
import type {
  StorageAppDataSnapshot,
  StorageAttachmentImageRecord,
  StorageChatConversationMetaRecord,
  StorageChatConversationRecord,
  StorageChatMessageRecord,
  StorageChatMessageStateRecord,
  StorageComposerDraftRecord,
  StorageImageFileRecord,
  StorageModelConfigRecord,
  StorageModelSettingsRecord,
  StorageProjectRecord,
  StorageUiPreferencesRecord,
} from "@mycopilot/protocol";
import type { ModelConfig, SearchMode } from "../../config/modelConfig";
import type { AppProject } from "../../config/projectConfig";
import type {
  ChatAgentRunView,
  ChatComposerDraft,
  ChatConversation,
  ChatMessage,
  ChatMessageAttachment,
  ChatMessageUiState,
} from "../chat/chatTypes";
import { hostClient } from "../../host/hostClient";

export interface ModelSettingsSnapshot {
  apiUrl: string;
  apiToken: string;
  searchMode: SearchMode;
  tavilyApiKey: string;
  models: ModelConfig[];
}

export interface AgentPromptPreferencesSnapshot extends AgentPromptPreferences {
  workMode: "coding" | "general";
  tone: "friendly" | "pragmatic";
  detailLevel: "low" | "medium" | "high";
  customInstructions: string;
  updatedAt: number;
}

export type SidebarConversationSort = "created" | "updated";
export type SidebarProjectSort = "created" | "recent" | "manual";
export type SidebarSectionOrder = "projects_first" | "conversations_first";

export const MIN_TRANSLUCENT_SIDEBAR_TRANSPARENCY = 50;
export const MAX_TRANSLUCENT_SIDEBAR_TRANSPARENCY = 100;
export const DEFAULT_TRANSLUCENT_SIDEBAR_TRANSPARENCY = 54;

export interface UiPreferencesSnapshot {
  profileAvatarDataUrl: string | null;
  profileDisplayName: string;
  profileHandle: string;
  sidebarConversationSort: SidebarConversationSort;
  sidebarProjectSort: SidebarProjectSort;
  sidebarProjectOrder: string[];
  sidebarSectionOrder: SidebarSectionOrder;
  nativeFontSmoothing: boolean;
  showTokenUsageDetails: boolean;
  translucentSidebar: boolean;
  translucentSidebarTransparency: number;
  fullPermissionEnabled: boolean;
  customPermissionEnabled: boolean;
  customPermissions: AgentPermissions;
  updatedAt: number;
}

export interface AppDataSnapshot {
  modelSettings: ModelSettingsSnapshot | null;
  projects: AppProject[];
  conversations: ChatConversation[];
  composerDrafts: Record<string, ChatComposerDraft>;
  uiPreferences: UiPreferencesSnapshot;
  agentPromptPreferences: AgentPromptPreferencesSnapshot;
}

export async function loadAppData(): Promise<AppDataSnapshot> {
  return mapAppDataFromStorage(await hostClient.storage.loadAppData());
}

export async function loadModelSettings(): Promise<ModelSettingsSnapshot | null> {
  return mapModelSettingsFromStorage(await hostClient.storage.loadModelSettings());
}

export async function saveModelSettings(settings: ModelSettingsSnapshot): Promise<void> {
  await hostClient.storage.saveModelSettings(mapModelSettingsToStorage(settings));
}

export async function loadAgentPromptPreferences(): Promise<AgentPromptPreferencesSnapshot> {
  return normalizeAgentPromptPreferences(await hostClient.storage.loadAgentPromptPreferences());
}

export async function saveAgentPromptPreferences(
  preferences: AgentPromptPreferences,
): Promise<AgentPromptPreferencesSnapshot> {
  return normalizeAgentPromptPreferences(
    await hostClient.storage.saveAgentPromptPreferences(normalizeAgentPromptPreferences(preferences)),
  );
}

export async function loadProjects(): Promise<AppProject[]> {
  return (await hostClient.storage.loadProjects()).map(mapProjectFromStorage);
}

export async function selectProjectDirectory(): Promise<AppProject | null> {
  const project = await hostClient.storage.selectProjectDirectory();
  return project ? mapProjectFromStorage(project) : null;
}

export async function saveProject(project: AppProject): Promise<AppProject> {
  return mapProjectFromStorage(await hostClient.storage.saveProject(mapProjectToStorage(project)));
}

export async function deleteStoredProject(projectId: string): Promise<void> {
  await hostClient.storage.deleteProject(projectId);
}

export async function showStoredProjectInFolder(projectId: string): Promise<void> {
  await hostClient.storage.showProjectInFolder(projectId);
}

export async function revealStoredProjectFile(
  projectId: string | null | undefined,
  filePath: string,
): Promise<void> {
  await hostClient.storage.revealProjectFile({ projectId, filePath });
}

export async function loadConversations(): Promise<ChatConversation[]> {
  return (await hostClient.storage.loadConversations()).map(mapConversationFromStorage);
}

export async function saveConversation(conversation: ChatConversation): Promise<ChatConversation> {
  return mapConversationFromStorage(
    await hostClient.storage.saveConversation(mapConversationToStorage(conversation)),
  );
}

export async function saveConversationMeta(conversation: ChatConversation): Promise<void> {
  await hostClient.storage.saveConversationMeta(mapConversationMetaToStorage(conversation));
}

export async function upsertChatMessages(
  conversationId: string,
  messages: ChatMessage[],
  positionOffset: number,
): Promise<void> {
  await hostClient.storage.upsertChatMessages({
    conversationId,
    messages: messages.map(mapMessageToStorage),
    positionOffset,
  });
}

export async function saveChatMessageState(conversationId: string, message: ChatMessage): Promise<void> {
  await hostClient.storage.saveChatMessageState({
    conversationId,
    message: mapMessageStateToStorage(message),
  });
}

export async function deleteStoredConversation(conversationId: string): Promise<void> {
  await hostClient.storage.deleteConversation(conversationId);
}

export async function deleteChatMessages(conversationId: string, messageIds: string[]): Promise<void> {
  if (messageIds.length === 0) return;
  await hostClient.storage.deleteChatMessages({ conversationId, messageIds });
}

export async function loadComposerDrafts(): Promise<Record<string, ChatComposerDraft>> {
  return mapDraftsFromStorage(await hostClient.storage.loadComposerDrafts());
}

export async function saveComposerDraft(scopeId: string, draft: ChatComposerDraft): Promise<ChatComposerDraft> {
  return mapDraftFromStorage(
    await hostClient.storage.saveComposerDraft(mapDraftToStorage(scopeId, draft)),
  );
}

export async function deleteStoredComposerDraft(scopeId: string): Promise<void> {
  await hostClient.storage.deleteComposerDraft(scopeId);
}

export async function loadUiPreferences(): Promise<UiPreferencesSnapshot> {
  return normalizeUiPreferences(await hostClient.storage.loadUiPreferences());
}

export async function saveUiPreferences(preferences: UiPreferencesSnapshot): Promise<UiPreferencesSnapshot> {
  return normalizeUiPreferences(await hostClient.storage.saveUiPreferences(normalizeUiPreferences(preferences)));
}

export async function selectProfileAvatar(): Promise<string | null> {
  return hostClient.storage.selectProfileAvatar();
}

export async function loadAttachmentImage(
  attachmentId: string,
): Promise<StorageAttachmentImageRecord | null> {
  const normalizedId = attachmentId.trim();
  if (!normalizedId) return null;
  return hostClient.storage.loadAttachmentImage({ attachmentId: normalizedId });
}

export async function loadInputAttachments(attachmentIds: string[]): Promise<AgentInputAttachment[]> {
  const normalizedIds = attachmentIds.map((id) => id.trim()).filter(Boolean);
  if (normalizedIds.length === 0) return [];
  return hostClient.storage.loadInputAttachments({ attachmentIds: normalizedIds });
}

export async function loadImageFile(input: {
  projectId?: string | null;
  filePath: string;
}): Promise<StorageImageFileRecord | null> {
  const filePath = input.filePath.trim();
  if (!filePath) return null;
  return hostClient.storage.loadImageFile({ projectId: input.projectId, filePath });
}

export function defaultAgentPromptPreferences(): AgentPromptPreferencesSnapshot {
  return {
    workMode: "coding",
    tone: "pragmatic",
    detailLevel: "medium",
    customInstructions: "",
    updatedAt: 0,
  };
}

export function defaultUiPreferences(): UiPreferencesSnapshot {
  return {
    profileAvatarDataUrl: null,
    profileDisplayName: "",
    profileHandle: "USER",
    sidebarConversationSort: "updated",
    sidebarProjectSort: "created",
    sidebarProjectOrder: [],
    sidebarSectionOrder: "projects_first",
    nativeFontSmoothing: false,
    showTokenUsageDetails: true,
    translucentSidebar: false,
    translucentSidebarTransparency: DEFAULT_TRANSLUCENT_SIDEBAR_TRANSPARENCY,
    fullPermissionEnabled: true,
    customPermissionEnabled: true,
    customPermissions: {
      read: "workspace_only",
      write: "workspace_only",
      command: "require_approval",
      patch: "require_approval",
    },
    updatedAt: 0,
  };
}

function mapAppDataFromStorage(snapshot: StorageAppDataSnapshot): AppDataSnapshot {
  return {
    modelSettings: mapModelSettingsFromStorage(snapshot.modelSettings),
    projects: snapshot.projects.map(mapProjectFromStorage),
    conversations: snapshot.conversations.map(mapConversationFromStorage),
    composerDrafts: mapDraftsFromStorage(snapshot.composerDrafts),
    uiPreferences: normalizeUiPreferences(snapshot.uiPreferences),
    agentPromptPreferences: normalizeAgentPromptPreferences(snapshot.agentPromptPreferences),
  };
}

function mapModelSettingsFromStorage(
  settings: StorageModelSettingsRecord | null,
): ModelSettingsSnapshot | null {
  if (!settings) return null;
  return {
    apiUrl: settings.apiUrl,
    apiToken: settings.apiToken,
    searchMode: isSearchMode(settings.searchMode) ? settings.searchMode : "auto",
    tavilyApiKey: settings.tavilyApiKey,
    models: settings.models.map(mapModelFromStorage),
  };
}

function mapModelSettingsToStorage(settings: ModelSettingsSnapshot): StorageModelSettingsRecord {
  return {
    ...settings,
    models: settings.models.map(mapModelToStorage),
  };
}

function mapModelFromStorage(model: StorageModelConfigRecord): ModelConfig {
  return {
    id: model.id,
    displayName: model.displayName,
    shortName: model.shortName ?? undefined,
    providerPath: model.providerPath ?? undefined,
    supportsImage: model.supportsImage,
    inputPrice: model.inputPrice,
    outputPrice: model.outputPrice,
    enabled: model.enabled,
  };
}

function mapModelToStorage(model: ModelConfig): StorageModelConfigRecord {
  return {
    ...model,
    shortName: model.shortName ?? null,
    providerPath: model.providerPath ?? null,
  };
}

function mapProjectFromStorage(project: StorageProjectRecord): AppProject {
  return {
    id: project.id,
    name: project.name,
    path: project.path ?? undefined,
    createdAt: project.createdAt,
    pinnedAt: project.pinnedAt ?? null,
  };
}

function mapProjectToStorage(project: AppProject): StorageProjectRecord {
  return {
    id: project.id,
    name: project.name,
    path: project.path ?? null,
    createdAt: project.createdAt,
    pinnedAt: project.pinnedAt ?? null,
  };
}

function mapConversationFromStorage(conversation: StorageChatConversationRecord): ChatConversation {
  return {
    id: conversation.id,
    projectId: conversation.projectId ?? null,
    modelId: conversation.modelId ?? null,
    title: conversation.title,
    messages: conversation.messages.map(mapMessageFromStorage),
    createdAt: conversation.createdAt,
    updatedAt: conversation.updatedAt,
    pinnedAt: conversation.pinnedAt ?? null,
    archivedAt: conversation.archivedAt ?? null,
    unreadAt: conversation.unreadAt ?? null,
  };
}

function mapConversationToStorage(conversation: ChatConversation): StorageChatConversationRecord {
  return {
    ...mapConversationMetaToStorage(conversation),
    messages: conversation.messages.map(mapMessageToStorage),
  };
}

function mapConversationMetaToStorage(conversation: ChatConversation): StorageChatConversationMetaRecord {
  return {
    id: conversation.id,
    projectId: conversation.projectId ?? null,
    modelId: conversation.modelId ?? null,
    title: conversation.title,
    createdAt: conversation.createdAt,
    updatedAt: conversation.updatedAt,
    pinnedAt: conversation.pinnedAt ?? null,
    archivedAt: conversation.archivedAt ?? null,
    unreadAt: conversation.unreadAt ?? null,
  };
}

function mapMessageFromStorage(message: StorageChatMessageRecord): ChatMessage {
  return {
    id: message.id,
    role: message.role === "user" ? "user" : "assistant",
    content: message.content,
    createdAt: message.createdAt,
    status: normalizeMessageStatus(message.status),
    attachments: message.attachments?.map(mapMessageAttachmentFromStorage),
    agentRun: parseJson<ChatAgentRunView>(message.agentRunJson),
    uiState: parseJson<ChatMessageUiState>(message.uiStateJson),
  };
}

function mapMessageToStorage(message: ChatMessage): StorageChatMessageRecord {
  return {
    id: message.id,
    role: message.role,
    content: message.content,
    createdAt: message.createdAt,
    status: message.status ?? null,
    attachments: [],
    agentRunJson: stringifyAgentRun(message.agentRun),
    uiStateJson: stringifyJson(message.uiState),
  };
}

function mapMessageStateToStorage(message: ChatMessage): StorageChatMessageStateRecord {
  return {
    id: message.id,
    content: message.content,
    status: message.status ?? null,
    agentRunJson: stringifyAgentRun(message.agentRun),
    uiStateJson: stringifyJson(message.uiState),
  };
}

function mapMessageAttachmentFromStorage(
  attachment: NonNullable<StorageChatMessageRecord["attachments"]>[number],
): ChatMessageAttachment {
  return {
    id: attachment.id,
    kind: attachment.kind === "image" ? "image" : "file",
    name: attachment.name,
    mimeType: attachment.mimeType,
    sizeBytes: attachment.sizeBytes,
    previewData: attachment.previewData,
    previewMimeType: attachment.previewMimeType,
    createdAt: attachment.createdAt,
  };
}

function mapDraftsFromStorage(drafts: StorageComposerDraftRecord[]): Record<string, ChatComposerDraft> {
  return Object.fromEntries(drafts.map((draft) => [draft.scopeId, mapDraftFromStorage(draft)]));
}

function mapDraftFromStorage(draft: StorageComposerDraftRecord): ChatComposerDraft {
  return {
    message: draft.message,
    permissionMode:
      draft.permissionMode === "full" || draft.permissionMode === "custom"
        ? draft.permissionMode
        : "default",
    modelId: draft.modelId ?? "",
    projectId: draft.projectId ?? null,
    attachments: parseDraftAttachments(draft.attachmentsJson),
    updatedAt: draft.updatedAt,
  };
}

function mapDraftToStorage(scopeId: string, draft: ChatComposerDraft): StorageComposerDraftRecord {
  return {
    scopeId,
    message: draft.message,
    permissionMode: draft.permissionMode,
    modelId: draft.modelId || null,
    projectId: draft.projectId,
    attachmentsJson: JSON.stringify(draft.attachments),
    updatedAt: draft.updatedAt,
  };
}

function normalizeAgentPromptPreferences(
  preferences: AgentPromptPreferences | null | undefined,
): AgentPromptPreferencesSnapshot {
  const defaults = defaultAgentPromptPreferences();
  return {
    workMode: preferences?.workMode === "general" ? "general" : defaults.workMode,
    tone: preferences?.tone === "friendly" ? "friendly" : defaults.tone,
    detailLevel:
      preferences?.detailLevel === "low" || preferences?.detailLevel === "high"
        ? preferences.detailLevel
        : defaults.detailLevel,
    customInstructions:
      typeof preferences?.customInstructions === "string" ? preferences.customInstructions.trim() : "",
    updatedAt: typeof preferences?.updatedAt === "number" ? preferences.updatedAt : Date.now(),
  };
}

function normalizeUiPreferences(preferences: StorageUiPreferencesRecord | null | undefined): UiPreferencesSnapshot {
  return {
    ...defaultUiPreferences(),
    ...preferences,
    profileAvatarDataUrl: preferences?.profileAvatarDataUrl ?? null,
    sidebarConversationSort: preferences?.sidebarConversationSort === "created" ? "created" : "updated",
    sidebarProjectSort:
      preferences?.sidebarProjectSort === "recent" || preferences?.sidebarProjectSort === "manual"
        ? preferences.sidebarProjectSort
        : "created",
    sidebarProjectOrder: Array.isArray(preferences?.sidebarProjectOrder)
      ? preferences.sidebarProjectOrder
      : [],
    sidebarSectionOrder:
      preferences?.sidebarSectionOrder === "conversations_first" ? "conversations_first" : "projects_first",
    translucentSidebarTransparency: normalizeTranslucentSidebarTransparency(
      preferences?.translucentSidebarTransparency,
    ),
    customPermissions: {
      ...defaultUiPreferences().customPermissions,
      ...preferences?.customPermissions,
    },
    updatedAt: typeof preferences?.updatedAt === "number" ? preferences.updatedAt : Date.now(),
  };
}

export function normalizeTranslucentSidebarTransparency(value: unknown): number {
  const numericValue =
    typeof value === "number" && Number.isFinite(value)
      ? Math.round(value)
      : DEFAULT_TRANSLUCENT_SIDEBAR_TRANSPARENCY;
  return Math.min(
    MAX_TRANSLUCENT_SIDEBAR_TRANSPARENCY,
    Math.max(MIN_TRANSLUCENT_SIDEBAR_TRANSPARENCY, numericValue),
  );
}

export function getTranslucentSidebarOpacityPercent(transparency: unknown): string {
  return `${100 - normalizeTranslucentSidebarTransparency(transparency)}%`;
}

function parseDraftAttachments(value: string): AgentInputAttachment[] {
  try {
    const parsed = JSON.parse(value) as AgentInputAttachment[];
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function parseJson<T>(value: string | null | undefined): T | undefined {
  if (!value) return undefined;
  try {
    return JSON.parse(value) as T;
  } catch {
    return undefined;
  }
}

function stringifyJson(value: unknown): string | null {
  return value === undefined ? null : JSON.stringify(value);
}

function stringifyAgentRun(run: ChatAgentRunView | undefined): string | null {
  if (!run) return null;
  const persistedRun = { ...run };
  delete persistedRun.fileWritePreviews;
  return JSON.stringify(persistedRun);
}

function normalizeMessageStatus(status: StorageChatMessageRecord["status"]): ChatMessage["status"] {
  if (status === "pending" || status === "sent" || status === "error") return status;
  return undefined;
}

function isSearchMode(value: string): value is SearchMode {
  return value === "auto" || value === "disabled" || value === "tavily";
}
