import type { IpcRenderer } from 'electron'
import { HOST_CHANNELS, type HostInvocationResult, type StorageHostApi } from '@mycopilot/host-api'
import {
  assertNoHumanInteractionMessageProof,
  validateStorageHumanInteractionResponses,
  parseProviderVendorDescriptors,
  parseProviderVendorModelPolicyDescriptor,
  parseProviderProfileUiDescriptors,
  parseStorageModelSettingsRecord,
  parseStorageModelSettingsUpdateRecord,
  parseStorageProjectCreateInput,
  parseStorageProjectUpdateInput,
  type StorageModelSettingsRecord
} from '@mycopilot/protocol'

type StorageIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>

export function createStorageIpcBridge(ipcRenderer: StorageIpcRenderer): StorageHostApi {
  return {
    onModelSettingsChanged: (handler) => {
      const listener = (): void => handler()
      ipcRenderer.on(HOST_CHANNELS.storage.modelSettingsChanged, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.storage.modelSettingsChanged, listener)
    },
    loadModelSettings: async () => {
      const value: unknown = await ipcRenderer.invoke(HOST_CHANNELS.storage.loadModelSettings)
      return value === null ? null : parseStorageModelSettingsRecord(value)
    },
    loadProviderProfileUiDescriptors: async () =>
      parseProviderProfileUiDescriptors(
        await ipcRenderer.invoke(HOST_CHANNELS.storage.loadProviderProfileUiDescriptors)
      ),
    loadProviderVendorDescriptors: async () =>
      parseProviderVendorDescriptors(
        await ipcRenderer.invoke(HOST_CHANNELS.storage.loadProviderVendorDescriptors)
      ),
    resolveProviderVendorModelPolicy: async (input) =>
      parseProviderVendorModelPolicyDescriptor(
        await ipcRenderer.invoke(HOST_CHANNELS.storage.resolveProviderVendorModelPolicy, input)
      ),
    saveModelSettings: async (settings) => {
      const result: HostInvocationResult<unknown> = await ipcRenderer.invoke(
        HOST_CHANNELS.storage.saveModelSettings,
        parseStorageModelSettingsUpdateRecord(settings)
      )
      return result.ok
        ? { ok: true, value: parseStorageModelSettingsRecord(result.value) }
        : (result as HostInvocationResult<StorageModelSettingsRecord>)
    },
    loadAgentPromptPreferences: () =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.loadAgentPromptPreferences),
    saveAgentPromptPreferences: (preferences) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.saveAgentPromptPreferences, preferences),
    loadProjects: () => ipcRenderer.invoke(HOST_CHANNELS.storage.loadProjects),
    pickProjectFolder: () => ipcRenderer.invoke(HOST_CHANNELS.storage.pickProjectFolder),
    createProject: (input) =>
      ipcRenderer.invoke(
        HOST_CHANNELS.storage.createProject,
        parseStorageProjectCreateInput(input)
      ),
    updateProject: (input) =>
      ipcRenderer.invoke(
        HOST_CHANNELS.storage.updateProject,
        parseStorageProjectUpdateInput(input)
      ),
    saveProject: (project) => ipcRenderer.invoke(HOST_CHANNELS.storage.saveProject, project),
    deleteProject: (projectId) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.deleteProject, projectId),
    showProjectInFolder: (projectId) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.showProjectInFolder, projectId),
    revealProjectFile: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.revealProjectFile, input),
    loadConversationMetas: () => ipcRenderer.invoke(HOST_CHANNELS.storage.loadConversationMetas),
    loadConversation: async (conversationId) => {
      const value = await ipcRenderer.invoke(HOST_CHANNELS.storage.loadConversation, conversationId)
      return value === null ? null : validateStorageHumanInteractionResponses(value)
    },
    loadConversations: async () =>
      (await ipcRenderer.invoke(HOST_CHANNELS.storage.loadConversations)).map(
        validateStorageHumanInteractionResponses
      ),
    forkConversation: async (input) => {
      const result = await ipcRenderer.invoke(HOST_CHANNELS.storage.forkConversation, input)
      if (result.ok) validateStorageHumanInteractionResponses(result.value)
      return result
    },
    saveConversationMeta: (conversation) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.saveConversationMeta, conversation),
    deleteConversation: (conversationId) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.deleteConversation, conversationId),
    deleteChatMessages: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.deleteChatMessages, input),
    upsertChatMessages: (input) => {
      for (const message of input.messages) assertNoHumanInteractionMessageProof(message)
      return ipcRenderer.invoke(HOST_CHANNELS.storage.upsertChatMessages, input)
    },
    saveChatMessageState: (input) => {
      assertNoHumanInteractionMessageProof(input.message)
      return ipcRenderer.invoke(HOST_CHANNELS.storage.saveChatMessageState, input)
    },
    saveChatMessageUiState: (input) => {
      assertNoHumanInteractionMessageProof(input.message)
      return ipcRenderer.invoke(HOST_CHANNELS.storage.saveChatMessageUiState, input)
    },
    loadComposerDrafts: () => ipcRenderer.invoke(HOST_CHANNELS.storage.loadComposerDrafts),
    saveComposerDraft: (draft) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.saveComposerDraft, draft),
    saveComposerDraftMessage: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.saveComposerDraftMessage, input),
    loadUiPreferences: () => ipcRenderer.invoke(HOST_CHANNELS.storage.loadUiPreferences),
    saveUiPreferences: (preferences) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.saveUiPreferences, preferences),
    selectProfileAvatar: () => ipcRenderer.invoke(HOST_CHANNELS.storage.selectProfileAvatar),
    loadAttachmentImage: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.loadAttachmentImage, input),
    loadInputAttachments: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.loadInputAttachments, input),
    loadImageFile: (input) => ipcRenderer.invoke(HOST_CHANNELS.storage.loadImageFile, input)
  }
}
