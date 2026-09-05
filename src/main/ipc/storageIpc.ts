import type { IpcMainInvokeEvent } from 'electron'
import {
  captureHostInvocation,
  HOST_CHANNELS,
  type HostInvocationResult
} from '@mycopilot/host-api'
import {
  assertNoHumanInteractionMessageProof,
  validateStorageHumanInteractionResponses,
  parseProviderVendorDescriptors,
  parseProviderVendorModelPolicyDescriptor,
  parseProviderProfileUiDescriptors,
  parseStorageForkConversationErrorData,
  parseStorageForkConversationRequest,
  parseStorageModelSettingsRecord,
  parseStorageModelSettingsUpdateRecord,
  parseStorageModelSettingsValidationErrorData,
  type StorageImageFileRecord,
  type StorageProjectRecord
} from '@mycopilot/protocol'
import type { CoreServer } from '../core/coreServer'
import type { TrustedIpcMain } from './trustedIpc'

interface StorageIpcPlatformActions {
  loadImageFile(
    coreServer: CoreServer,
    input: { projectId?: string | null; filePath?: string }
  ): Promise<StorageImageFileRecord | null>
  revealProjectFile(
    coreServer: CoreServer,
    input: { projectId?: string | null; filePath: string }
  ): Promise<void>
  selectProfileAvatar(event: IpcMainInvokeEvent): Promise<string | null>
  selectProjectDirectory(event: IpcMainInvokeEvent): Promise<StorageProjectRecord | null>
  showProjectInFolder(coreServer: CoreServer, projectId: string): Promise<void>
}

export function registerStorageIpc(
  ipcMain: TrustedIpcMain,
  coreServer: CoreServer,
  actions: StorageIpcPlatformActions
): void {
  ipcMain.handle(HOST_CHANNELS.storage.loadModelSettings, async () => {
    const settings = await coreServer.loadModelSettings()
    return settings === null ? null : parseStorageModelSettingsRecord(settings)
  })
  ipcMain.handle(HOST_CHANNELS.storage.loadProviderProfileUiDescriptors, () =>
    coreServer.loadProviderProfileUiDescriptors().then(parseProviderProfileUiDescriptors)
  )
  ipcMain.handle(HOST_CHANNELS.storage.loadProviderVendorDescriptors, () =>
    coreServer.loadProviderVendorDescriptors().then(parseProviderVendorDescriptors)
  )
  ipcMain.handle(HOST_CHANNELS.storage.resolveProviderVendorModelPolicy, (_event, input) =>
    coreServer
      .resolveProviderVendorModelPolicy(input)
      .then(parseProviderVendorModelPolicyDescriptor)
  )
  ipcMain.handle(HOST_CHANNELS.storage.saveModelSettings, (_event, settings) =>
    captureModelSettingsSaveInvocation(() =>
      coreServer
        .saveModelSettings(parseStorageModelSettingsUpdateRecord(settings))
        .then(parseStorageModelSettingsRecord)
    )
  )
  ipcMain.handle(HOST_CHANNELS.storage.loadAgentPromptPreferences, () =>
    coreServer.loadAgentPromptPreferences()
  )
  ipcMain.handle(HOST_CHANNELS.storage.saveAgentPromptPreferences, (_event, preferences) =>
    coreServer.saveAgentPromptPreferences(preferences)
  )
  ipcMain.handle(HOST_CHANNELS.storage.loadProjects, () => coreServer.loadProjects())
  ipcMain.handle(HOST_CHANNELS.storage.selectProjectDirectory, (event) =>
    actions.selectProjectDirectory(event)
  )
  ipcMain.handle(HOST_CHANNELS.storage.saveProject, (_event, project) =>
    coreServer.saveProject(project)
  )
  ipcMain.handle(HOST_CHANNELS.storage.deleteProject, (_event, projectId) =>
    coreServer.deleteProject(projectId)
  )
  ipcMain.handle(HOST_CHANNELS.storage.showProjectInFolder, (_event, projectId) =>
    actions.showProjectInFolder(coreServer, projectId)
  )
  ipcMain.handle(HOST_CHANNELS.storage.revealProjectFile, (_event, input) =>
    actions.revealProjectFile(coreServer, input)
  )
  ipcMain.handle(HOST_CHANNELS.storage.loadConversations, async () =>
    (await coreServer.loadConversations()).map(validateStorageHumanInteractionResponses)
  )
  ipcMain.handle(HOST_CHANNELS.storage.loadConversationMetas, () =>
    coreServer.loadConversationMetas()
  )
  ipcMain.handle(HOST_CHANNELS.storage.loadConversation, (_event, conversationId) =>
    coreServer
      .loadConversation(conversationId)
      .then((conversation) =>
        conversation ? validateStorageHumanInteractionResponses(conversation) : null
      )
  )
  ipcMain.handle(HOST_CHANNELS.storage.forkConversation, (_event, input) =>
    captureConversationForkInvocation(() =>
      coreServer
        .forkConversation(parseStorageForkConversationRequest(input))
        .then(validateStorageHumanInteractionResponses)
    )
  )
  ipcMain.handle(HOST_CHANNELS.storage.saveConversationMeta, (_event, conversation) =>
    coreServer.saveConversationMeta(conversation)
  )
  ipcMain.handle(HOST_CHANNELS.storage.deleteConversation, (_event, conversationId) =>
    coreServer.deleteConversation(conversationId)
  )
  ipcMain.handle(HOST_CHANNELS.storage.deleteChatMessages, (_event, input) =>
    coreServer.deleteChatMessages(input)
  )
  ipcMain.handle(HOST_CHANNELS.storage.upsertChatMessages, (_event, input) => {
    for (const message of input.messages) assertNoHumanInteractionMessageProof(message)
    return coreServer.upsertChatMessages(input)
  })
  ipcMain.handle(HOST_CHANNELS.storage.saveChatMessageState, (_event, input) => {
    assertNoHumanInteractionMessageProof(input.message)
    return coreServer.saveChatMessageState(input)
  })
  ipcMain.handle(HOST_CHANNELS.storage.saveChatMessageUiState, (_event, input) => {
    assertNoHumanInteractionMessageProof(input.message)
    return coreServer.saveChatMessageUiState(input)
  })
  ipcMain.handle(HOST_CHANNELS.storage.loadComposerDrafts, () => coreServer.loadComposerDrafts())
  ipcMain.handle(HOST_CHANNELS.storage.saveComposerDraft, (_event, draft) =>
    coreServer.saveComposerDraft(draft)
  )
  ipcMain.handle(HOST_CHANNELS.storage.saveComposerDraftMessage, (_event, input) =>
    coreServer.saveComposerDraftMessage(input)
  )
  ipcMain.handle(HOST_CHANNELS.storage.loadUiPreferences, () => coreServer.loadUiPreferences())
  ipcMain.handle(HOST_CHANNELS.storage.saveUiPreferences, (_event, preferences) =>
    coreServer.saveUiPreferences(preferences)
  )
  ipcMain.handle(HOST_CHANNELS.storage.selectProfileAvatar, (event) =>
    actions.selectProfileAvatar(event)
  )
  ipcMain.handle(HOST_CHANNELS.storage.loadAttachmentImage, (_event, input) =>
    coreServer.loadAttachmentImage(input)
  )
  ipcMain.handle(HOST_CHANNELS.storage.loadInputAttachments, (_event, input) =>
    coreServer.loadInputAttachments(input)
  )
  ipcMain.handle(HOST_CHANNELS.storage.loadImageFile, (_event, input) =>
    actions.loadImageFile(coreServer, input)
  )
}

/**
 * Keeps Core diagnostics out of Electron's renderer-facing error surface while preserving the
 * narrow recovery contract that the UI understands.
 */
async function captureConversationForkInvocation<T>(
  operation: () => Promise<T>
): Promise<HostInvocationResult<T>> {
  const result = await captureHostInvocation(operation)
  if (result.ok) return result

  try {
    const data = parseStorageForkConversationErrorData(result.error.data)
    return {
      ok: false,
      error: {
        message: 'Conversation fork was rejected.',
        ...(result.error.code === undefined ? {} : { code: result.error.code }),
        data
      }
    }
  } catch {
    return {
      ok: false,
      error: { message: 'Conversation fork failed.' }
    }
  }
}

/**
 * Projects only the narrow validation recovery contract that model-settings UI can act on.
 * Core diagnostics, connection details, and provider responses never cross into Renderer.
 */
async function captureModelSettingsSaveInvocation<T>(
  operation: () => Promise<T>
): Promise<HostInvocationResult<T>> {
  const result = await captureHostInvocation(operation)
  if (result.ok) return result

  try {
    const data = parseStorageModelSettingsValidationErrorData(result.error.data)
    return {
      ok: false,
      error: {
        message: 'Model settings validation failed.',
        ...(result.error.code === undefined ? {} : { code: result.error.code }),
        data
      }
    }
  } catch {
    return {
      ok: false,
      error: { message: 'Model settings save failed.' }
    }
  }
}
