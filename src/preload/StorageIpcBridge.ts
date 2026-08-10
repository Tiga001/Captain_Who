import type { IpcRenderer } from 'electron'
import { HOST_CHANNELS, type StorageHostApi } from '@mycopilot/host-api'

type StorageIpcRenderer = Pick<IpcRenderer, 'invoke'>

export function createStorageIpcBridge(ipcRenderer: StorageIpcRenderer): StorageHostApi {
  return {
    loadModelSettings: () => ipcRenderer.invoke(HOST_CHANNELS.storage.loadModelSettings),
    loadProviderProfileUiDescriptors: () =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.loadProviderProfileUiDescriptors),
    saveModelSettings: (settings) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.saveModelSettings, settings),
    loadAgentPromptPreferences: () =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.loadAgentPromptPreferences),
    saveAgentPromptPreferences: (preferences) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.saveAgentPromptPreferences, preferences),
    loadProjects: () => ipcRenderer.invoke(HOST_CHANNELS.storage.loadProjects),
    selectProjectDirectory: () => ipcRenderer.invoke(HOST_CHANNELS.storage.selectProjectDirectory),
    saveProject: (project) => ipcRenderer.invoke(HOST_CHANNELS.storage.saveProject, project),
    deleteProject: (projectId) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.deleteProject, projectId),
    showProjectInFolder: (projectId) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.showProjectInFolder, projectId),
    revealProjectFile: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.revealProjectFile, input),
    loadConversationMetas: () => ipcRenderer.invoke(HOST_CHANNELS.storage.loadConversationMetas),
    loadConversation: (conversationId) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.loadConversation, conversationId),
    loadConversations: () => ipcRenderer.invoke(HOST_CHANNELS.storage.loadConversations),
    forkConversation: (input) => ipcRenderer.invoke(HOST_CHANNELS.storage.forkConversation, input),
    saveConversationMeta: (conversation) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.saveConversationMeta, conversation),
    deleteConversation: (conversationId) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.deleteConversation, conversationId),
    deleteChatMessages: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.deleteChatMessages, input),
    upsertChatMessages: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.upsertChatMessages, input),
    saveChatMessageState: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.saveChatMessageState, input),
    saveChatMessageUiState: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.saveChatMessageUiState, input),
    loadComposerDrafts: () => ipcRenderer.invoke(HOST_CHANNELS.storage.loadComposerDrafts),
    saveComposerDraft: (draft) =>
      ipcRenderer.invoke(HOST_CHANNELS.storage.saveComposerDraft, draft),
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
