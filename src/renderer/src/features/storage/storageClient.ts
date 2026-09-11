import type {
  CredentialMutation,
  CredentialStatus,
  AgentInputAttachment,
  AgentPermissions,
  AgentPromptPreferences
} from '@mycopilot/protocol'
import {
  parseStorageHumanInteractionResponse,
  parseStorageModelSettingsRecord,
  parseStorageModelSettingsUpdateRecord,
  parseProviderVendorDescriptors,
  parseProviderVendorModelPolicyDescriptor
} from '@mycopilot/protocol'
import {
  HostInvocationError,
  unwrapHostInvocation,
  type HostInvocationResult
} from '@mycopilot/host-api'
import type {
  ProviderProfileUiDescriptor,
  ProviderVendorDescriptor,
  ProviderVendorModelPolicyDescriptor,
  ProviderVendorModelPolicyInput,
  StorageAttachmentImageRecord,
  StorageChatConversationMetaRecord,
  StorageChatConversationRecord,
  StorageChatMessageRecord,
  StorageChatMessageWriteRecord,
  StorageChatMessageStateRecord,
  StorageChatMessageUiStateRecord,
  StorageComposerDraftRecord,
  StorageComposerDraftMessageUpdate,
  StorageForkConversationRequest,
  StorageImageFileRecord,
  StorageModelConfigRecord,
  StorageModelSettingsRecord,
  StorageModelSettingsUpdateRecord,
  StorageProjectCreateInput,
  StorageProjectFolderPick,
  StorageProjectFolderRecord,
  StorageProjectRecord,
  StorageProjectUpdateInput,
  StorageUiPreferencesRecord
} from '@mycopilot/protocol'
import type { ModelConfig, ModelConfigSaveDraft, SearchMode } from '../../config/modelConfig'
import type { AppProject, AppProjectFolder } from '../../config/projectConfig'
import type {
  ChatAgentRunView,
  ChatComposerDraft,
  ChatConversation,
  ChatMessage,
  ChatMessageAttachment,
  ChatMessageUiState,
  ChatQueuedMessage
} from '../chat/chatTypes'
import { settleAgentRunToolActivities } from '../agentRun/agentEventReducer'
import {
  isProjectValidationErrorData,
  ProjectValidationError
} from '../projects/projectValidationError'
import { normalizeSkillSelections } from '../skills/skillSelection'
import { hostClient } from '../../host/hostClient'
import { parsePersistedAgentRunJson, stringifyPersistedAgentRun } from './persistedAgentRun'
import {
  normalizeStoredComposerPermissionMode,
  serializeComposerPermissionMode
} from './composerPermissionModePersistence'

export interface ModelSettingsSnapshot {
  configurationRevision: string | null
  apiUrl: string
  apiTokenStatus: CredentialStatus
  searchMode: SearchMode
  tavilyApiKeyStatus: CredentialStatus
  models: ModelConfig[]
}

export interface ModelSettingsSaveDraft {
  apiUrl: string
  apiTokenMutation: CredentialMutation
  searchMode: SearchMode
  tavilyApiKeyMutation: CredentialMutation
  models: Array<ModelConfig | ModelConfigSaveDraft>
}

export interface AgentPromptPreferencesSnapshot extends AgentPromptPreferences {
  contextProfile: 'full' | 'minimal'
  workMode: 'coding' | 'general'
  tone: 'friendly' | 'pragmatic'
  detailLevel: 'low' | 'medium' | 'high'
  customInstructions: string
  updatedAt: number
}

export type SidebarConversationSort = 'created' | 'updated'
export type SidebarProjectSort = 'created' | 'recent' | 'manual'
type SidebarSectionOrder = 'projects_first' | 'conversations_first'

export const MIN_TRANSLUCENT_SIDEBAR_TRANSPARENCY = 50
export const MAX_TRANSLUCENT_SIDEBAR_TRANSPARENCY = 100
const DEFAULT_TRANSLUCENT_SIDEBAR_TRANSPARENCY = 54
const TRANSLUCENT_SIDEBAR_THEME_TINT_FLOOR = 32

export interface UiPreferencesSnapshot {
  profileAvatarDataUrl: string | null
  profileDisplayName: string
  profileHandle: string
  sidebarConversationSort: SidebarConversationSort
  sidebarProjectSort: SidebarProjectSort
  sidebarProjectOrder: string[]
  sidebarSectionOrder: SidebarSectionOrder
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

export async function loadModelSettings(): Promise<ModelSettingsSnapshot | null> {
  const settings = await hostClient.storage.loadModelSettings()
  return mapModelSettingsFromStorage(
    settings === null ? null : parseStorageModelSettingsRecord(settings)
  )
}

export async function loadProviderProfileUiDescriptors(): Promise<ProviderProfileUiDescriptor[]> {
  return hostClient.storage.loadProviderProfileUiDescriptors()
}

export async function loadProviderVendorDescriptors(): Promise<ProviderVendorDescriptor[]> {
  return parseProviderVendorDescriptors(await hostClient.storage.loadProviderVendorDescriptors())
}

export async function resolveProviderVendorModelPolicy(
  input: ProviderVendorModelPolicyInput
): Promise<ProviderVendorModelPolicyDescriptor> {
  return parseProviderVendorModelPolicyDescriptor(
    await hostClient.storage.resolveProviderVendorModelPolicy(input)
  )
}

export async function saveModelSettings(
  settings: ModelSettingsSaveDraft,
  expectedRevision: string | null
): Promise<ModelSettingsSnapshot> {
  const saved = unwrapHostInvocation(
    await hostClient.storage.saveModelSettings(
      parseStorageModelSettingsUpdateRecord(mapModelSettingsToStorage(settings, expectedRevision))
    )
  )
  const normalized = mapModelSettingsFromStorage(parseStorageModelSettingsRecord(saved))
  if (!normalized) {
    throw new Error('Host returned an empty model settings snapshot after save')
  }
  return normalized
}

export async function loadAgentPromptPreferences(): Promise<AgentPromptPreferencesSnapshot> {
  return normalizeAgentPromptPreferences(await hostClient.storage.loadAgentPromptPreferences())
}

export async function saveAgentPromptPreferences(
  preferences: AgentPromptPreferences
): Promise<AgentPromptPreferencesSnapshot> {
  return normalizeAgentPromptPreferences(
    await hostClient.storage.saveAgentPromptPreferences(
      normalizeAgentPromptPreferences(preferences)
    )
  )
}

export async function loadProjects(): Promise<AppProject[]> {
  return (await hostClient.storage.loadProjects()).map(mapProjectFromStorage)
}

export async function pickProjectFolder(): Promise<StorageProjectFolderPick | null> {
  return hostClient.storage.pickProjectFolder()
}

function unwrapProjectMutation(result: HostInvocationResult<StorageProjectRecord>): AppProject {
  if (result.ok) return mapProjectFromStorage(result.value)
  const data = result.error.data
  if (isProjectValidationErrorData(data)) throw new ProjectValidationError(data)
  throw new HostInvocationError(result.error)
}

export async function createProject(input: StorageProjectCreateInput): Promise<AppProject> {
  return unwrapProjectMutation(await hostClient.storage.createProject(input))
}

export async function updateProject(input: StorageProjectUpdateInput): Promise<AppProject> {
  return unwrapProjectMutation(await hostClient.storage.updateProject(input))
}

/** Persists pin state; folder membership and names are edited through updateProject. */
export async function saveProject(project: AppProject): Promise<AppProject> {
  return mapProjectFromStorage(await hostClient.storage.saveProject(mapProjectToStorage(project)))
}

export async function deleteStoredProject(projectId: string): Promise<void> {
  await hostClient.storage.deleteProject(projectId)
}

export async function showStoredProjectInFolder(projectId: string): Promise<void> {
  await hostClient.storage.showProjectInFolder(projectId)
}

export async function revealStoredProjectFile(
  projectId: string | null | undefined,
  filePath: string
): Promise<void> {
  await hostClient.storage.revealProjectFile({ projectId, filePath })
}

export async function loadConversationMetas(): Promise<ChatConversation[]> {
  return (await hostClient.storage.loadConversationMetas()).map(mapConversationMetaFromStorage)
}

export async function loadConversation(conversationId: string): Promise<ChatConversation | null> {
  const conversation = await hostClient.storage.loadConversation(conversationId)
  return conversation ? mapConversationFromStorage(conversation) : null
}

export async function forkConversation(
  input: StorageForkConversationRequest
): Promise<ChatConversation> {
  return mapConversationFromStorage(
    unwrapHostInvocation(await hostClient.storage.forkConversation(input))
  )
}

export async function saveConversationMeta(conversation: ChatConversation): Promise<void> {
  await hostClient.storage.saveConversationMeta(mapConversationMetaToStorage(conversation))
}

export async function upsertChatMessages(
  conversationId: string,
  messages: ChatMessage[],
  positionOffset: number
): Promise<void> {
  await hostClient.storage.upsertChatMessages({
    conversationId,
    messages: messages.map(mapMessageToStorage),
    positionOffset
  })
}

export async function saveChatMessageState(
  conversationId: string,
  message: ChatMessage
): Promise<void> {
  await hostClient.storage.saveChatMessageState({
    conversationId,
    message: mapMessageStateToStorage(message)
  })
}

export async function saveChatMessageUiState(
  conversationId: string,
  messageId: string,
  uiState: ChatMessageUiState | undefined
): Promise<void> {
  const message: StorageChatMessageUiStateRecord = {
    id: messageId,
    uiStateJson: stringifyJson(uiState)
  }
  await hostClient.storage.saveChatMessageUiState({ conversationId, message })
}

export async function deleteStoredConversation(conversationId: string): Promise<void> {
  await hostClient.storage.deleteConversation(conversationId)
}

export async function deleteChatMessages(
  conversationId: string,
  messageIds: string[]
): Promise<void> {
  if (messageIds.length === 0) return
  await hostClient.storage.deleteChatMessages({ conversationId, messageIds })
}

export async function loadComposerDrafts(): Promise<Record<string, ChatComposerDraft>> {
  return mapDraftsFromStorage(await hostClient.storage.loadComposerDrafts())
}

export async function saveComposerDraft(
  scopeId: string,
  draft: ChatComposerDraft
): Promise<ChatComposerDraft> {
  return mapDraftFromStorage(
    await hostClient.storage.saveComposerDraft(mapDraftToStorage(scopeId, draft))
  )
}

export async function saveComposerDraftMessage(
  input: StorageComposerDraftMessageUpdate
): Promise<boolean> {
  return hostClient.storage.saveComposerDraftMessage(input)
}

export async function loadUiPreferences(): Promise<UiPreferencesSnapshot> {
  return normalizeUiPreferences(await hostClient.storage.loadUiPreferences())
}

export async function saveUiPreferences(
  preferences: UiPreferencesSnapshot
): Promise<UiPreferencesSnapshot> {
  return normalizeUiPreferences(
    await hostClient.storage.saveUiPreferences(normalizeUiPreferences(preferences))
  )
}

export async function selectProfileAvatar(): Promise<string | null> {
  return hostClient.storage.selectProfileAvatar()
}

export async function loadAttachmentImage(
  attachmentId: string
): Promise<StorageAttachmentImageRecord | null> {
  const normalizedId = attachmentId.trim()
  if (!normalizedId) return null
  return hostClient.storage.loadAttachmentImage({ attachmentId: normalizedId })
}

export async function loadInputAttachments(
  attachmentIds: string[]
): Promise<AgentInputAttachment[]> {
  const normalizedIds = attachmentIds.map((id) => id.trim()).filter(Boolean)
  if (normalizedIds.length === 0) return []
  return hostClient.storage.loadInputAttachments({ attachmentIds: normalizedIds })
}

export async function loadImageFile(input: {
  projectId?: string | null
  filePath: string
}): Promise<StorageImageFileRecord | null> {
  const filePath = input.filePath.trim()
  if (!filePath) return null
  return hostClient.storage.loadImageFile({ projectId: input.projectId, filePath })
}

export function defaultAgentPromptPreferences(): AgentPromptPreferencesSnapshot {
  return {
    contextProfile: 'full',
    workMode: 'coding',
    tone: 'pragmatic',
    detailLevel: 'medium',
    customInstructions: '',
    updatedAt: 0
  }
}

export function defaultUiPreferences(): UiPreferencesSnapshot {
  return {
    profileAvatarDataUrl: null,
    profileDisplayName: '',
    profileHandle: 'USER',
    sidebarConversationSort: 'updated',
    sidebarProjectSort: 'created',
    sidebarProjectOrder: [],
    sidebarSectionOrder: 'projects_first',
    nativeFontSmoothing: false,
    showTokenUsageDetails: true,
    showContextWindowUsage: true,
    translucentSidebar: false,
    translucentSidebarTransparency: DEFAULT_TRANSLUCENT_SIDEBAR_TRANSPARENCY,
    fullPermissionEnabled: true,
    customPermissionEnabled: true,
    customPermissions: {
      read: 'workspace_only',
      write: 'workspace_only',
      command: 'require_approval',
      commandSafety: 'guarded',
      patch: 'require_approval',
      builtinExecution: 'require_approval'
    },
    updatedAt: 0
  }
}

function mapModelSettingsFromStorage(
  settings: StorageModelSettingsRecord | null
): ModelSettingsSnapshot | null {
  if (!settings) return null
  return {
    configurationRevision: settings.configurationRevision,
    apiUrl: settings.apiUrl,
    apiTokenStatus: settings.apiTokenStatus,
    searchMode: isSearchMode(settings.searchMode) ? settings.searchMode : 'auto',
    tavilyApiKeyStatus: settings.tavilyApiKeyStatus,
    models: settings.models.map(mapModelFromStorage)
  }
}

function mapModelSettingsToStorage(
  settings: ModelSettingsSaveDraft,
  expectedRevision: string | null
): StorageModelSettingsUpdateRecord {
  return {
    expectedRevision,
    apiUrl: settings.apiUrl,
    apiTokenMutation: settings.apiTokenMutation,
    searchMode: settings.searchMode,
    tavilyApiKeyMutation: settings.tavilyApiKeyMutation,
    models: settings.models.map(mapModelToStorage)
  }
}

function mapModelFromStorage(model: StorageModelConfigRecord): ModelConfig {
  return {
    id: model.id,
    providerModelId: model.providerModelId,
    displayName: model.displayName,
    apiUrlOverride: model.apiUrlOverride ?? undefined,
    apiTokenOverrideStatus: model.apiTokenOverrideStatus,
    apiTokenOverrideMutation: { type: 'keep' },
    supportsImage: model.supportsImage,
    contextWindowTokens: model.contextWindowTokens ?? undefined,
    providerProfileConfig: model.providerProfileConfig,
    providerProfileUpdate: { kind: 'unchanged' },
    inputPrice: model.inputPrice,
    cachedInputPrice: model.cachedInputPrice,
    outputPrice: model.outputPrice,
    enabled: model.enabled
  }
}

function mapModelToStorage(
  model: ModelConfig | ModelConfigSaveDraft
): StorageModelSettingsUpdateRecord['models'][number] {
  return {
    id: model.id,
    providerModelId: model.providerModelId,
    displayName: model.displayName,
    apiUrlOverride: model.apiUrlOverride ?? null,
    apiTokenOverrideMutation: model.apiTokenOverrideMutation,
    supportsImage: model.supportsImage,
    contextWindowTokens: model.contextWindowTokens ?? null,
    providerProfileUpdate: model.providerProfileUpdate,
    inputPrice: model.inputPrice,
    cachedInputPrice: model.cachedInputPrice,
    outputPrice: model.outputPrice,
    enabled: model.enabled
  }
}

function mapProjectFolderFromStorage(folder: StorageProjectFolderRecord): AppProjectFolder {
  return {
    id: folder.id,
    path: folder.path,
    alias: folder.alias,
    role: folder.role,
    sortOrder: folder.sortOrder,
    createdAt: folder.createdAt
  }
}

function mapProjectFromStorage(project: StorageProjectRecord): AppProject {
  return {
    id: project.id,
    name: project.name,
    folders: [...project.folders]
      .sort((left, right) => left.sortOrder - right.sortOrder)
      .map(mapProjectFolderFromStorage),
    createdAt: project.createdAt,
    pinnedAt: project.pinnedAt ?? null
  }
}

function mapProjectToStorage(project: AppProject): StorageProjectRecord {
  return {
    id: project.id,
    name: project.name,
    folders: project.folders.map((folder) => ({
      id: folder.id,
      path: folder.path,
      alias: folder.alias,
      role: folder.role,
      sortOrder: folder.sortOrder,
      createdAt: folder.createdAt
    })),
    createdAt: project.createdAt,
    pinnedAt: project.pinnedAt ?? null
  }
}

function mapConversationFromStorage(conversation: StorageChatConversationRecord): ChatConversation {
  return {
    id: conversation.id,
    projectId: conversation.projectId ?? null,
    modelId: conversation.modelId ?? null,
    title: conversation.title,
    messages: conversation.messages.map(mapMessageFromStorage),
    messagesLoaded: true,
    createdAt: conversation.createdAt,
    updatedAt: conversation.updatedAt,
    pinnedAt: conversation.pinnedAt ?? null,
    archivedAt: conversation.archivedAt ?? null,
    unreadAt: conversation.unreadAt ?? null,
    continuationOrigin: conversation.continuationOrigin
      ? {
          sourceConversationId: conversation.continuationOrigin.sourceConversationId,
          sourceMessageId: conversation.continuationOrigin.sourceMessageId,
          boundaryMessageId: conversation.continuationOrigin.boundaryMessageId
        }
      : null
  }
}

function mapConversationMetaFromStorage(
  conversation: StorageChatConversationMetaRecord
): ChatConversation {
  return {
    id: conversation.id,
    projectId: conversation.projectId ?? null,
    modelId: conversation.modelId ?? null,
    title: conversation.title,
    messages: [],
    messagesLoaded: false,
    createdAt: conversation.createdAt,
    updatedAt: conversation.updatedAt,
    pinnedAt: conversation.pinnedAt ?? null,
    archivedAt: conversation.archivedAt ?? null,
    unreadAt: conversation.unreadAt ?? null
  }
}

function mapConversationMetaToStorage(
  conversation: ChatConversation
): StorageChatConversationMetaRecord {
  const pendingArchive = conversation.pendingArchivedAt
  return {
    id: conversation.id,
    projectId: conversation.projectId ?? null,
    modelId: conversation.modelId ?? null,
    title: conversation.title,
    createdAt: conversation.createdAt,
    updatedAt: conversation.updatedAt,
    pinnedAt: conversation.pinnedAt ?? null,
    archivedAt: pendingArchive ?? conversation.archivedAt ?? null,
    unreadAt: pendingArchive === undefined ? (conversation.unreadAt ?? null) : null
  }
}

function mapMessageFromStorage(message: StorageChatMessageRecord): ChatMessage {
  const storedRun = parsePersistedAgentRunJson(message.agentRunJson)
  const agentRun =
    storedRun &&
    (storedRun.status === 'completed' ||
      storedRun.status === 'failed' ||
      storedRun.status === 'cancelled')
      ? settleAgentRunToolActivities(
          storedRun,
          storedRun.status,
          storedRun.completedAt ?? message.createdAt
        )
      : storedRun

  return {
    id: message.id,
    role: message.role === 'user' ? 'user' : 'assistant',
    humanInteractionDisplay: parseStorageHumanInteractionResponse(message),
    content: message.content,
    createdAt: message.createdAt,
    status: normalizeMessageStatus(message.status),
    attachments: message.attachments?.map(mapMessageAttachmentFromStorage),
    agentRun,
    uiState: parseJson<ChatMessageUiState>(message.uiStateJson)
  }
}

function mapMessageToStorage(message: ChatMessage): StorageChatMessageWriteRecord {
  return {
    id: message.id,
    role: message.role,
    content: message.content,
    createdAt: message.createdAt,
    status: message.status ?? null,
    attachments: [],
    agentRunJson: stringifyAgentRun(message.agentRun),
    uiStateJson: stringifyJson(message.uiState)
  }
}

function mapMessageStateToStorage(message: ChatMessage): StorageChatMessageStateRecord {
  return {
    id: message.id,
    content: message.content,
    status: message.status ?? null,
    agentRunJson: stringifyAgentRun(message.agentRun)
  }
}

function mapMessageAttachmentFromStorage(
  attachment: NonNullable<StorageChatMessageRecord['attachments']>[number]
): ChatMessageAttachment {
  return {
    id: attachment.id,
    kind: attachment.kind === 'image' ? 'image' : 'file',
    name: attachment.name,
    mimeType: attachment.mimeType,
    sizeBytes: attachment.sizeBytes,
    previewData: attachment.previewData,
    previewMimeType: attachment.previewMimeType,
    createdAt: attachment.createdAt
  }
}

function mapDraftsFromStorage(
  drafts: StorageComposerDraftRecord[]
): Record<string, ChatComposerDraft> {
  return Object.fromEntries(drafts.map((draft) => [draft.scopeId, mapDraftFromStorage(draft)]))
}

function mapDraftFromStorage(draft: StorageComposerDraftRecord): ChatComposerDraft {
  return {
    message: draft.message,
    permissionMode: normalizeStoredComposerPermissionMode(
      draft.permissionMode,
      draft.permissionModeVersion
    ),
    modelId: draft.modelId ?? '',
    projectId: draft.projectId ?? null,
    attachments: parseDraftAttachments(draft.attachmentsJson),
    skills: parseDraftSkills(draft.skillsJson),
    queuedMessages: parseQueuedMessages(draft.queuedMessagesJson),
    updatedAt: draft.updatedAt
  }
}

function mapDraftToStorage(scopeId: string, draft: ChatComposerDraft): StorageComposerDraftRecord {
  return {
    scopeId,
    message: draft.message,
    ...serializeComposerPermissionMode(draft.permissionMode),
    modelId: draft.modelId || null,
    projectId: draft.projectId,
    attachmentsJson: JSON.stringify(draft.attachments),
    skillsJson: JSON.stringify(normalizeSkillSelections(draft.skills)),
    queuedMessagesJson: JSON.stringify(draft.queuedMessages),
    updatedAt: draft.updatedAt
  }
}

function normalizeAgentPromptPreferences(
  preferences: AgentPromptPreferences | null | undefined
): AgentPromptPreferencesSnapshot {
  const defaults = defaultAgentPromptPreferences()
  return {
    contextProfile: preferences?.contextProfile === 'minimal' ? 'minimal' : 'full',
    workMode: preferences?.workMode === 'general' ? 'general' : defaults.workMode,
    tone: preferences?.tone === 'friendly' ? 'friendly' : defaults.tone,
    detailLevel:
      preferences?.detailLevel === 'low' || preferences?.detailLevel === 'high'
        ? preferences.detailLevel
        : defaults.detailLevel,
    customInstructions:
      typeof preferences?.customInstructions === 'string'
        ? preferences.customInstructions.trim()
        : '',
    updatedAt: typeof preferences?.updatedAt === 'number' ? preferences.updatedAt : Date.now()
  }
}

function normalizeUiPreferences(
  preferences: StorageUiPreferencesRecord | null | undefined
): UiPreferencesSnapshot {
  return {
    ...defaultUiPreferences(),
    ...preferences,
    profileAvatarDataUrl: preferences?.profileAvatarDataUrl ?? null,
    sidebarConversationSort:
      preferences?.sidebarConversationSort === 'created' ? 'created' : 'updated',
    sidebarProjectSort:
      preferences?.sidebarProjectSort === 'recent' || preferences?.sidebarProjectSort === 'manual'
        ? preferences.sidebarProjectSort
        : 'created',
    sidebarProjectOrder: Array.isArray(preferences?.sidebarProjectOrder)
      ? preferences.sidebarProjectOrder
      : [],
    sidebarSectionOrder:
      preferences?.sidebarSectionOrder === 'conversations_first'
        ? 'conversations_first'
        : 'projects_first',
    showContextWindowUsage: preferences?.showContextWindowUsage !== false,
    translucentSidebarTransparency: normalizeTranslucentSidebarTransparency(
      preferences?.translucentSidebarTransparency
    ),
    customPermissions: {
      ...defaultUiPreferences().customPermissions,
      ...preferences?.customPermissions,
      commandSafety: 'guarded'
    },
    updatedAt: typeof preferences?.updatedAt === 'number' ? preferences.updatedAt : Date.now()
  }
}

export function normalizeTranslucentSidebarTransparency(value: unknown): number {
  const numericValue =
    typeof value === 'number' && Number.isFinite(value)
      ? Math.round(value)
      : DEFAULT_TRANSLUCENT_SIDEBAR_TRANSPARENCY
  return Math.min(
    MAX_TRANSLUCENT_SIDEBAR_TRANSPARENCY,
    Math.max(MIN_TRANSLUCENT_SIDEBAR_TRANSPARENCY, numericValue)
  )
}

export function getTranslucentSidebarOpacityPercent(transparency: unknown): string {
  const requestedTintOpacity = 100 - normalizeTranslucentSidebarTransparency(transparency)

  // Native macOS vibrancy only knows the system appearance, not the selected Captain Who palette.
  // Keep a perceptual theme floor above it so maximum transparency remains themed glass instead
  // of visually collapsing to the native gray sidebar material.
  const effectiveTintOpacity =
    TRANSLUCENT_SIDEBAR_THEME_TINT_FLOOR +
    requestedTintOpacity * (1 - TRANSLUCENT_SIDEBAR_THEME_TINT_FLOOR / 100)

  return `${Math.round(effectiveTintOpacity)}%`
}

const COMPOSER_DRAFT_CORRUPTION_ERROR = 'Stored composer draft is malformed'

function isUnknownRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function parseDraftArray(value: string): unknown[] {
  try {
    const parsed = JSON.parse(value) as unknown
    if (Array.isArray(parsed)) return parsed
  } catch {
    // The stable error below deliberately excludes persisted content.
  }
  throw new Error(COMPOSER_DRAFT_CORRUPTION_ERROR)
}

function isExactRecord(
  value: unknown,
  required: readonly string[],
  optional: readonly string[] = []
): value is Record<string, unknown> {
  if (!isUnknownRecord(value)) return false
  const allowed = new Set([...required, ...optional])
  return (
    required.every((key) => Object.hasOwn(value, key)) &&
    Object.keys(value).every((key) => allowed.has(key))
  )
}

function parseDraftAttachments(value: string): AgentInputAttachment[] {
  return parseDraftArray(value).map((candidate) => {
    if (
      !isExactRecord(
        candidate,
        ['id', 'kind', 'name', 'sizeBytes', 'encoding', 'data'],
        ['mimeType', 'truncated']
      ) ||
      typeof candidate.id !== 'string' ||
      (candidate.kind !== 'file' && candidate.kind !== 'image') ||
      typeof candidate.name !== 'string' ||
      (candidate.mimeType !== undefined && typeof candidate.mimeType !== 'string') ||
      !Number.isSafeInteger(candidate.sizeBytes) ||
      (candidate.sizeBytes as number) < 0 ||
      (candidate.encoding !== 'utf8' && candidate.encoding !== 'base64') ||
      typeof candidate.data !== 'string' ||
      (candidate.truncated !== undefined && typeof candidate.truncated !== 'boolean')
    ) {
      throw new Error(COMPOSER_DRAFT_CORRUPTION_ERROR)
    }
    return candidate as unknown as AgentInputAttachment
  })
}

function parseDraftSkills(value: string): ChatComposerDraft['skills'] {
  const parsed = parseDraftArray(value)
  const normalized = normalizeSkillSelections(parsed)
  if (
    normalized.length !== parsed.length ||
    parsed.some(
      (candidate, index) =>
        !isExactRecord(candidate, ['id', 'revision']) ||
        candidate.id !== normalized[index]?.id ||
        candidate.revision !== normalized[index]?.revision
    )
  ) {
    throw new Error(COMPOSER_DRAFT_CORRUPTION_ERROR)
  }
  return normalized
}

function parseQueuedMessages(value: string): ChatQueuedMessage[] {
  return parseDraftArray(value).map((candidate) => {
    if (
      !isExactRecord(
        candidate,
        [
          'id',
          'clientMessageId',
          'content',
          'attachments',
          'modelId',
          'permissionMode',
          'projectId',
          'skills',
          'status',
          'createdAt'
        ],
        ['error']
      ) ||
      typeof candidate.id !== 'string' ||
      typeof candidate.clientMessageId !== 'string' ||
      typeof candidate.content !== 'string' ||
      !Array.isArray(candidate.attachments) ||
      typeof candidate.modelId !== 'string' ||
      !['default', 'custom', 'full'].includes(String(candidate.permissionMode)) ||
      (candidate.projectId !== null && typeof candidate.projectId !== 'string') ||
      !Array.isArray(candidate.skills) ||
      !['pending', 'submitting', 'error'].includes(String(candidate.status)) ||
      !Number.isSafeInteger(candidate.createdAt) ||
      (candidate.createdAt as number) < 0 ||
      (candidate.error !== undefined && typeof candidate.error !== 'string')
    ) {
      throw new Error(COMPOSER_DRAFT_CORRUPTION_ERROR)
    }
    const attachments = parseDraftAttachments(JSON.stringify(candidate.attachments))
    const skills = parseDraftSkills(JSON.stringify(candidate.skills))
    return {
      id: candidate.id,
      clientMessageId: candidate.clientMessageId,
      content: candidate.content,
      attachments,
      modelId: candidate.modelId,
      permissionMode: candidate.permissionMode as ChatQueuedMessage['permissionMode'],
      projectId: candidate.projectId,
      skills,
      status: candidate.status === 'error' ? 'error' : 'pending',
      ...(candidate.status === 'error' && candidate.error !== undefined
        ? { error: candidate.error }
        : {}),
      createdAt: candidate.createdAt as number
    }
  })
}

function parseJson<T>(value: string | null | undefined): T | undefined {
  if (!value) return undefined
  try {
    return JSON.parse(value) as T
  } catch {
    return undefined
  }
}

function stringifyJson(value: unknown): string | null {
  return value === undefined ? null : JSON.stringify(value)
}

function stringifyAgentRun(run: ChatAgentRunView | undefined): string | null {
  return stringifyPersistedAgentRun(run)
}

function normalizeMessageStatus(status: StorageChatMessageRecord['status']): ChatMessage['status'] {
  if (status === 'pending' || status === 'sent' || status === 'error') return status
  return undefined
}

function isSearchMode(value: string): value is SearchMode {
  return value === 'auto' || value === 'disabled' || value === 'tavily'
}
