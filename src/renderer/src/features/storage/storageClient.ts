import type {
  AgentInputAttachment,
  AgentMcpServerScope,
  AgentPermissions,
  AgentPromptPreferences
} from '@mycopilot/protocol'
import { unwrapHostInvocation } from '@mycopilot/host-api'
import { parseAgentMcpProposedAction, parseAgentMcpToolInvocationEvent } from '@mycopilot/protocol'
import type {
  StorageAttachmentImageRecord,
  StorageChatConversationMetaRecord,
  StorageChatConversationRecord,
  StorageChatMessageRecord,
  StorageChatMessageStateRecord,
  StorageChatMessageUiStateRecord,
  StorageComposerDraftRecord,
  StorageImageFileRecord,
  StorageModelConfigRecord,
  StorageModelSettingsRecord,
  StorageProjectRecord,
  StorageUiPreferencesRecord
} from '@mycopilot/protocol'
import type { ModelConfig, SearchMode } from '../../config/modelConfig'
import type { AppProject } from '../../config/projectConfig'
import type {
  ChatAgentTimelineItem,
  ChatAgentRunView,
  ChatCommandSessionView,
  ChatComposerDraft,
  ChatConversation,
  ChatMessage,
  ChatMessageAttachment,
  ChatMessageUiState,
  ChatMcpToolInvocationView,
  ChatQueuedMessage
} from '../chat/chatTypes'
import { ensureAgentRun, settleAgentRunToolActivities } from '../agentRun/agentEventReducer'
import { normalizeSkillSelections, parseStoredSkillSelections } from '../skills/skillSelection'
import { hostClient } from '../../host/hostClient'
import {
  normalizeStoredComposerPermissionMode,
  serializeComposerPermissionMode
} from './composerPermissionModePersistence'

export interface ModelSettingsSnapshot {
  apiUrl: string
  apiToken: string
  searchMode: SearchMode
  tavilyApiKey: string
  models: ModelConfig[]
}

export interface AgentPromptPreferencesSnapshot extends AgentPromptPreferences {
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
  return mapModelSettingsFromStorage(await hostClient.storage.loadModelSettings())
}

export async function saveModelSettings(settings: ModelSettingsSnapshot): Promise<void> {
  await hostClient.storage.saveModelSettings(mapModelSettingsToStorage(settings))
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

export async function selectProjectDirectory(): Promise<AppProject | null> {
  const project = await hostClient.storage.selectProjectDirectory()
  return project ? mapProjectFromStorage(project) : null
}

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
  sourceConversationId: string,
  throughAssistantMessageId: string,
  requestId: string
): Promise<ChatConversation> {
  return mapConversationFromStorage(
    unwrapHostInvocation(
      await hostClient.storage.forkConversation({
        requestId,
        sourceConversationId,
        throughAssistantMessageId
      })
    )
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
      patch: 'require_approval'
    },
    updatedAt: 0
  }
}

function mapModelSettingsFromStorage(
  settings: StorageModelSettingsRecord | null
): ModelSettingsSnapshot | null {
  if (!settings) return null
  return {
    apiUrl: settings.apiUrl,
    apiToken: settings.apiToken,
    searchMode: isSearchMode(settings.searchMode) ? settings.searchMode : 'auto',
    tavilyApiKey: settings.tavilyApiKey,
    models: settings.models.map(mapModelFromStorage)
  }
}

function mapModelSettingsToStorage(settings: ModelSettingsSnapshot): StorageModelSettingsRecord {
  return {
    ...settings,
    models: settings.models.map(mapModelToStorage)
  }
}

function mapModelFromStorage(model: StorageModelConfigRecord): ModelConfig {
  return {
    id: model.id,
    displayName: model.displayName,
    apiUrlOverride: model.apiUrlOverride ?? undefined,
    apiTokenOverride: model.apiTokenOverride ?? undefined,
    supportsImage: model.supportsImage,
    contextWindowTokens: model.contextWindowTokens ?? undefined,
    providerProfileConfig: model.providerProfileConfig ?? undefined,
    inputPrice: model.inputPrice,
    outputPrice: model.outputPrice,
    enabled: model.enabled
  }
}

function mapModelToStorage(model: ModelConfig): StorageModelConfigRecord {
  return {
    ...model,
    apiUrlOverride: model.apiUrlOverride ?? null,
    apiTokenOverride: model.apiTokenOverride ?? null,
    contextWindowTokens: model.contextWindowTokens ?? null
  }
}

function mapProjectFromStorage(project: StorageProjectRecord): AppProject {
  return {
    id: project.id,
    name: project.name,
    path: project.path ?? undefined,
    createdAt: project.createdAt,
    pinnedAt: project.pinnedAt ?? null
  }
}

function mapProjectToStorage(project: AppProject): StorageProjectRecord {
  return {
    id: project.id,
    name: project.name,
    path: project.path ?? null,
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
  return {
    id: conversation.id,
    projectId: conversation.projectId ?? null,
    modelId: conversation.modelId ?? null,
    title: conversation.title,
    createdAt: conversation.createdAt,
    updatedAt: conversation.updatedAt,
    pinnedAt: conversation.pinnedAt ?? null,
    archivedAt: conversation.archivedAt ?? null,
    unreadAt: conversation.unreadAt ?? null
  }
}

const STORED_MCP_EVENT_KEYS = [
  'actionId',
  'invocationId',
  'callId',
  'serverId',
  'serverDisplayName',
  'rawToolName',
  'modelToolName',
  'displayReason',
  'external',
  'state',
  'dispatchCertainty',
  'outcome',
  'isError',
  'errorCode',
  'durationMs',
  'outputTruncated'
] as const
const STORED_MCP_TERMINAL_STATES = new Set([
  'completed',
  'failed',
  'cancelled',
  'outcome_unknown',
  'rejected',
  'expired',
  'payload_unavailable',
  'policy_denied'
])

function isUnknownRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function projectStoredMcpScope(value: unknown): AgentMcpServerScope | undefined {
  if (!isUnknownRecord(value) || typeof value.type !== 'string') return undefined
  const keys = Object.keys(value)
  if (value.type === 'builtin' || value.type === 'user' || value.type === 'managed') {
    return keys.length === 1 ? { type: value.type } : undefined
  }
  if (
    value.type === 'project' &&
    keys.length === 2 &&
    typeof value.projectId === 'string' &&
    value.projectId.length > 0 &&
    value.projectId.length <= 1024
  ) {
    return { type: 'project', projectId: value.projectId }
  }
  if (
    value.type === 'plugin' &&
    keys.length === 2 &&
    typeof value.pluginId === 'string' &&
    value.pluginId.length > 0 &&
    value.pluginId.length <= 1024
  ) {
    return { type: 'plugin', pluginId: value.pluginId }
  }
  return undefined
}

function projectStoredMcpInvocation(value: unknown): ChatMcpToolInvocationView | undefined {
  if (!isUnknownRecord(value)) return undefined
  const eventCandidate: Record<string, unknown> = {}
  for (const key of STORED_MCP_EVENT_KEYS) {
    if (Object.prototype.hasOwnProperty.call(value, key)) {
      eventCandidate[key] = value[key]
    }
  }

  try {
    const event = parseAgentMcpToolInvocationEvent(eventCandidate)
    const scope = projectStoredMcpScope(value.scope)
    return {
      actionId: event.actionId,
      invocationId: event.invocationId,
      callId: event.callId,
      serverId: event.serverId,
      serverDisplayName: event.serverDisplayName,
      ...(scope ? { scope } : {}),
      rawToolName: event.rawToolName,
      modelToolName: event.modelToolName,
      displayReason: event.displayReason,
      external: true,
      state: event.state,
      dispatchCertainty: event.dispatchCertainty,
      outcome: event.outcome,
      isError: event.isError,
      errorCode: event.errorCode,
      durationMs: event.durationMs,
      outputTruncated: event.outputTruncated
    }
  } catch {
    return undefined
  }
}

const DURABLE_COMMAND_TERMINAL_STATUSES = new Set<ChatCommandSessionView['status']>([
  'exited',
  'interrupted',
  'timed_out',
  'failed',
  'outcome_unknown'
])

function optionalSafeInteger(value: unknown, minimum?: number): number | undefined {
  if (!Number.isSafeInteger(value)) return undefined
  const integer = value as number
  return minimum === undefined || integer >= minimum ? integer : undefined
}

/**
 * Persist only immutable terminal process metadata. Active state and transcript bytes remain
 * exclusively Host-owned, while this small projection keeps old Timeline cards truthful after the
 * Host's bounded operational Session row has aged out.
 */
function projectDurableTerminalCommandSession(
  value: unknown,
  expectedCallId: string
): ChatCommandSessionView | undefined {
  if (!isUnknownRecord(value)) return undefined
  if (
    value.callId !== expectedCallId ||
    expectedCallId.length === 0 ||
    expectedCallId.length > 1024
  ) {
    return undefined
  }
  if (
    typeof value.status !== 'string' ||
    !DURABLE_COMMAND_TERMINAL_STATUSES.has(value.status as ChatCommandSessionView['status'])
  ) {
    return undefined
  }

  const startedAt = optionalSafeInteger(value.startedAt, 0)
  const endedAt = optionalSafeInteger(value.endedAt, 0)
  const exitCode = optionalSafeInteger(value.exitCode)
  const latestSequence = optionalSafeInteger(value.latestSequence, 0) ?? 0
  return {
    callId: expectedCallId,
    status: value.status as ChatCommandSessionView['status'],
    ...(startedAt === undefined ? {} : { startedAt }),
    ...(endedAt === undefined ? {} : { endedAt }),
    ...(exitCode === undefined ? {} : { exitCode }),
    latestSequence,
    outputTruncated: value.outputTruncated === true
  }
}

function projectDurableTerminalCommandSessions(
  value: unknown,
  allowedCallIds: ReadonlySet<string>
): Record<string, ChatCommandSessionView> | undefined {
  if (!isUnknownRecord(value)) return undefined
  const projectedSessions: Record<string, ChatCommandSessionView> = {}
  for (const [callId, session] of Object.entries(value)) {
    if (!allowedCallIds.has(callId)) continue
    const projected = projectDurableTerminalCommandSession(session, callId)
    if (projected) projectedSessions[callId] = projected
  }
  return Object.keys(projectedSessions).length > 0 ? projectedSessions : undefined
}

function normalizeStoredAgentRun(storedRun: ChatAgentRunView): ChatAgentRunView {
  const normalized = ensureAgentRun(storedRun, storedRun.runId, storedRun.status)
  const runCommandCallIds = new Set(
    normalized.toolCalls.filter((call) => call.tool === 'run_command').map((call) => call.id)
  )
  const durableCommandSessions = projectDurableTerminalCommandSessions(
    storedRun.commandSessions,
    runCommandCallIds
  )
  // Active command state and transcript previews are Host-owned runtime projections. Retain only
  // the allowlisted immutable terminal metadata written by the current Renderer.
  const durableNormalized = { ...normalized }
  if (durableCommandSessions) {
    durableNormalized.commandSessions = durableCommandSessions
  } else {
    delete durableNormalized.commandSessions
  }
  delete durableNormalized.commandOutputPreviews
  delete durableNormalized.llmRetry
  const rawMcpInvocations = Array.isArray(storedRun.mcpInvocations) ? storedRun.mcpInvocations : []
  const referencedMcpCallIds = new Set<string>(
    rawMcpInvocations.flatMap((invocation) =>
      isUnknownRecord(invocation) &&
      typeof invocation.callId === 'string' &&
      invocation.callId.length <= 1024
        ? [invocation.callId]
        : []
    )
  )
  const approvals = normalized.approvals.filter((action) => {
    if (!isUnknownRecord(action) || action.type !== 'mcp_tool_call') return true
    try {
      referencedMcpCallIds.add(parseAgentMcpProposedAction(action).approval.identity.callId)
    } catch {
      // Corrupt MCP approvals are always discarded. Without a fully valid typed identity, do not
      // infer routing or retain any of their untrusted nested fields.
    }
    return false
  })
  const invocationById = new Map<string, ChatMcpToolInvocationView>()
  for (const rawInvocation of rawMcpInvocations) {
    const projected = projectStoredMcpInvocation(rawInvocation)
    if (!projected) continue
    const existing = invocationById.get(projected.invocationId)
    const existingIsTerminal = existing ? STORED_MCP_TERMINAL_STATES.has(existing.state) : false
    const projectedIsTerminal = STORED_MCP_TERMINAL_STATES.has(projected.state)
    if (!existing || projectedIsTerminal || !existingIsTerminal) {
      invocationById.set(projected.invocationId, projected)
    }
  }
  const mcpInvocations = [...invocationById.values()]
  const validInvocationIds = new Set(mcpInvocations.map((invocation) => invocation.invocationId))
  const invocationByCallId = new Map(
    mcpInvocations.map((invocation) => [invocation.callId, invocation] as const)
  )
  const emittedInvocationIds = new Set<string>()
  const timeline: ChatAgentTimelineItem[] = []
  for (const item of normalized.timeline) {
    if (!isUnknownRecord(item) || typeof item.type !== 'string') continue
    if (item.type === 'tool_call') {
      if (typeof item.callId !== 'string') continue
      const invocation = invocationByCallId.get(item.callId)
      if (invocation) {
        if (!emittedInvocationIds.has(invocation.invocationId)) {
          timeline.push({
            id: `mcp-invocation-${invocation.invocationId}`,
            type: 'mcp_tool_call',
            invocationId: invocation.invocationId
          })
          emittedInvocationIds.add(invocation.invocationId)
        }
        continue
      }
      if (referencedMcpCallIds.has(item.callId)) continue
      timeline.push(item)
      continue
    }
    if (item.type === 'mcp_tool_call') {
      if (
        typeof item.id !== 'string' ||
        typeof item.invocationId !== 'string' ||
        !validInvocationIds.has(item.invocationId)
      ) {
        continue
      }
      if (emittedInvocationIds.has(item.invocationId)) continue
      timeline.push({
        id: item.id,
        type: 'mcp_tool_call',
        invocationId: item.invocationId
      })
      emittedInvocationIds.add(item.invocationId)
      continue
    }
    timeline.push(item)
  }

  return {
    ...durableNormalized,
    approvals,
    toolCalls: normalized.toolCalls.filter(
      (call) =>
        isUnknownRecord(call) && typeof call.id === 'string' && !referencedMcpCallIds.has(call.id)
    ),
    toolResults: normalized.toolResults.filter(
      (result) =>
        isUnknownRecord(result) &&
        typeof result.callId === 'string' &&
        !referencedMcpCallIds.has(result.callId)
    ),
    timeline,
    mcpInvocations
  }
}

function mapMessageFromStorage(message: StorageChatMessageRecord): ChatMessage {
  const parsedRun = parseJson<ChatAgentRunView>(message.agentRunJson)
  const storedRun = parsedRun ? normalizeStoredAgentRun(parsedRun) : undefined
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
    content: message.content,
    createdAt: message.createdAt,
    status: normalizeMessageStatus(message.status),
    attachments: message.attachments?.map(mapMessageAttachmentFromStorage),
    agentRun,
    uiState: parseJson<ChatMessageUiState>(message.uiStateJson)
  }
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
    uiStateJson: stringifyJson(message.uiState)
  }
}

function mapMessageStateToStorage(message: ChatMessage): StorageChatMessageStateRecord {
  return {
    id: message.id,
    content: message.content,
    status: message.status ?? null,
    agentRunJson: stringifyAgentRun(message.agentRun),
    uiStateJson: stringifyJson(message.uiState)
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
    skills: parseStoredSkillSelections(draft.skillsJson),
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

  // Native macOS vibrancy only knows the system appearance, not the selected MyCopilot palette.
  // Keep a perceptual theme floor above it so maximum transparency remains themed glass instead
  // of visually collapsing to the native gray sidebar material.
  const effectiveTintOpacity =
    TRANSLUCENT_SIDEBAR_THEME_TINT_FLOOR +
    requestedTintOpacity * (1 - TRANSLUCENT_SIDEBAR_THEME_TINT_FLOOR / 100)

  return `${Math.round(effectiveTintOpacity)}%`
}

function parseDraftAttachments(value: string): AgentInputAttachment[] {
  try {
    const parsed = JSON.parse(value) as AgentInputAttachment[]
    return Array.isArray(parsed) ? parsed : []
  } catch {
    return []
  }
}

function parseQueuedMessages(value: string | undefined): ChatQueuedMessage[] {
  if (!value) return []
  try {
    const parsed = JSON.parse(value) as ChatQueuedMessage[]
    if (!Array.isArray(parsed)) return []
    return parsed
      .filter(
        (message) =>
          message &&
          typeof message.id === 'string' &&
          typeof message.clientMessageId === 'string' &&
          typeof message.content === 'string' &&
          Array.isArray(message.attachments) &&
          typeof message.modelId === 'string' &&
          (message.permissionMode === 'default' ||
            message.permissionMode === 'custom' ||
            message.permissionMode === 'full') &&
          (message.projectId === null || typeof message.projectId === 'string') &&
          Array.isArray(message.skills) &&
          typeof message.createdAt === 'number'
      )
      .map((message) => ({
        ...message,
        skills: normalizeSkillSelections(message.skills),
        status: message.status === 'error' ? 'error' : 'pending',
        error: message.status === 'error' ? message.error : undefined
      }))
  } catch {
    return []
  }
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
  if (!run) return null
  const persistedRun = { ...run }
  delete persistedRun.fileWritePreviews
  delete persistedRun.commandOutputPreviews
  delete persistedRun.llmRetry
  const runCommandCallIds = new Set(
    run.toolCalls.filter((call) => call.tool === 'run_command').map((call) => call.id)
  )
  const durableCommandSessions = projectDurableTerminalCommandSessions(
    run.commandSessions,
    runCommandCallIds
  )
  if (durableCommandSessions) {
    persistedRun.commandSessions = durableCommandSessions
  } else {
    delete persistedRun.commandSessions
  }
  return JSON.stringify(persistedRun)
}

function normalizeMessageStatus(status: StorageChatMessageRecord['status']): ChatMessage['status'] {
  if (status === 'pending' || status === 'sent' || status === 'error') return status
  return undefined
}

function isSearchMode(value: string): value is SearchMode {
  return value === 'auto' || value === 'disabled' || value === 'tavily'
}
