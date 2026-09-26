import type { AgentInputAttachment, AgentPromptPreferences } from '@mycopilot/protocol'
import {
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
  StorageChatMessageUiStateRecord,
  StorageComposerDraftMessageUpdate,
  StorageForkConversationRequest,
  StorageImageFileRecord,
  StorageProjectCreateInput,
  StorageProjectFolderPick,
  StorageProjectRecord,
  StorageProjectUpdateInput
} from '@mycopilot/protocol'
import type { AppProject } from '../../config/projectConfig'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatMessage,
  ChatMessageUiState
} from '../chat/chatTypes'
import {
  isProjectValidationErrorData,
  ProjectValidationError
} from '../projects/projectValidationError'
import { hostClient } from '../../host/hostClient'
import {
  type ModelSettingsSnapshot,
  type ModelSettingsSaveDraft,
  mapModelSettingsFromStorage,
  mapModelSettingsToStorage,
  mapProjectFromStorage,
  mapProjectToStorage
} from './storageConfigurationMapping'
import {
  type AgentPromptPreferencesSnapshot,
  type UiPreferencesSnapshot,
  normalizeAgentPromptPreferences,
  normalizeUiPreferences
} from './storagePreferences'
import {
  mapConversationFromStorage,
  mapConversationMetaFromStorage,
  mapConversationMetaToStorage,
  mapMessageToStorage,
  mapMessageStateToStorage,
  stringifyJson
} from './storageConversationMapping'
import {
  mapDraftsFromStorage,
  mapDraftFromStorage,
  mapDraftToStorage
} from './composerDraftPersistence'

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
  filePath: string,
  assistantMessageId?: string
): Promise<void> {
  await hostClient.storage.revealProjectFile({
    projectId,
    filePath,
    ...(assistantMessageId ? { assistantMessageId } : {})
  })
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
  assistantMessageId?: string
  projectId?: string | null
  filePath: string
}): Promise<StorageImageFileRecord | null> {
  const filePath = input.filePath.trim()
  if (!filePath) return null
  return hostClient.storage.loadImageFile({
    projectId: input.projectId,
    filePath,
    ...(input.assistantMessageId ? { assistantMessageId: input.assistantMessageId } : {})
  })
}

export type { ModelSettingsSnapshot, ModelSettingsSaveDraft } from './storageConfigurationMapping'
export type {
  AgentPromptPreferencesSnapshot,
  SidebarConversationSort,
  SidebarProjectSort,
  UiPreferencesSnapshot
} from './storagePreferences'
export {
  MIN_TRANSLUCENT_SIDEBAR_TRANSPARENCY,
  MAX_TRANSLUCENT_SIDEBAR_TRANSPARENCY,
  defaultAgentPromptPreferences,
  defaultUiPreferences,
  normalizeTranslucentSidebarTransparency,
  getTranslucentSidebarOpacityPercent
} from './storagePreferences'
