import { contextBridge, ipcRenderer } from 'electron'
import type { IpcRendererEvent } from 'electron'
import type { HostApi } from '@mycopilot/host-api'
import type { AppWindowState } from '@mycopilot/host-api'
import type { AgentEvent, TerminalExitEvent, TerminalOutputEvent } from '@mycopilot/protocol'

const AGENT_EVENT_CHANNEL = 'host:agent.event'
const APP_WINDOW_STATE_CHANNEL = 'host:app.windowStateChange'
const TERMINAL_OUTPUT_CHANNEL = 'host:terminal.output'
const TERMINAL_EXIT_CHANNEL = 'host:terminal.exit'

function onAgentEvent(handler: (event: AgentEvent) => void): () => void {
  const listener = (_event: IpcRendererEvent, payload: AgentEvent): void => handler(payload)
  ipcRenderer.on(AGENT_EVENT_CHANNEL, listener)
  return () => ipcRenderer.removeListener(AGENT_EVENT_CHANNEL, listener)
}

function onAppWindowStateChange(handler: (state: AppWindowState) => void): () => void {
  const listener = (_event: IpcRendererEvent, payload: AppWindowState): void => handler(payload)
  ipcRenderer.on(APP_WINDOW_STATE_CHANNEL, listener)
  return () => ipcRenderer.removeListener(APP_WINDOW_STATE_CHANNEL, listener)
}

function onTerminalOutput(handler: (event: TerminalOutputEvent) => void): () => void {
  const listener = (_event: IpcRendererEvent, payload: TerminalOutputEvent): void =>
    handler(payload)
  ipcRenderer.on(TERMINAL_OUTPUT_CHANNEL, listener)
  return () => ipcRenderer.removeListener(TERMINAL_OUTPUT_CHANNEL, listener)
}

function onTerminalExit(handler: (event: TerminalExitEvent) => void): () => void {
  const listener = (_event: IpcRendererEvent, payload: TerminalExitEvent): void => handler(payload)
  ipcRenderer.on(TERMINAL_EXIT_CHANNEL, listener)
  return () => ipcRenderer.removeListener(TERMINAL_EXIT_CHANNEL, listener)
}

const host: HostApi = {
  core: {
    ping: (input) => ipcRenderer.invoke('host:core.ping', input)
  },
  app: {
    getWindowState: () => ipcRenderer.invoke('host:app.getWindowState'),
    openExternal: (url) => ipcRenderer.invoke('host:app.openExternal', url),
    onWindowStateChange: onAppWindowStateChange,
    setNativeThemeSource: (themeSource) =>
      ipcRenderer.invoke('host:app.setNativeThemeSource', themeSource)
  },
  agent: {
    startConversationTurn: (input) => ipcRenderer.invoke('host:agent.startConversationTurn', input),
    getContextWindowSnapshot: (input) =>
      ipcRenderer.invoke('host:agent.getContextWindowSnapshot', input),
    getContextCompactionAudit: (input) =>
      ipcRenderer.invoke('host:agent.getContextCompactionAudit', input),
    cancelRun: (input) => ipcRenderer.invoke('host:agent.cancelRun', input),
    listPendingActions: () => ipcRenderer.invoke('host:agent.listPendingActions'),
    approveAction: (input) => ipcRenderer.invoke('host:agent.approveAction', input),
    rejectAction: (input) => ipcRenderer.invoke('host:agent.rejectAction', input),
    cancelAction: (input) => ipcRenderer.invoke('host:agent.cancelAction', input),
    getUsageSummary: (input) => ipcRenderer.invoke('host:agent.getUsageSummary', input),
    clearUsageRecords: (input) => ipcRenderer.invoke('host:agent.clearUsageRecords', input),
    readFileDraft: (input) => ipcRenderer.invoke('host:agent.readFileDraft', input),
    getFileWriteDiff: (input) => ipcRenderer.invoke('host:agent.getFileWriteDiff', input),
    onEvent: onAgentEvent
  },
  attachments: {
    selectInputAttachments: (request) =>
      ipcRenderer.invoke('host:attachments.selectInputAttachments', request)
  },
  browser: {
    clearBrowsingData: () => ipcRenderer.invoke('host:browser.clearBrowsingData')
  },
  clipboard: {
    writeImage: (input) => ipcRenderer.invoke('host:clipboard.writeImage', input)
  },
  resources: {
    resolveFavicon: (input) => ipcRenderer.invoke('host:resources.resolveFavicon', input)
  },
  search: {
    searchChats: (input) => ipcRenderer.invoke('host:search.searchChats', input)
  },
  storage: {
    loadModelSettings: () => ipcRenderer.invoke('host:storage.loadModelSettings'),
    saveModelSettings: (settings) => ipcRenderer.invoke('host:storage.saveModelSettings', settings),
    loadAgentPromptPreferences: () => ipcRenderer.invoke('host:storage.loadAgentPromptPreferences'),
    saveAgentPromptPreferences: (preferences) =>
      ipcRenderer.invoke('host:storage.saveAgentPromptPreferences', preferences),
    loadProjects: () => ipcRenderer.invoke('host:storage.loadProjects'),
    selectProjectDirectory: () => ipcRenderer.invoke('host:storage.selectProjectDirectory'),
    saveProject: (project) => ipcRenderer.invoke('host:storage.saveProject', project),
    deleteProject: (projectId) => ipcRenderer.invoke('host:storage.deleteProject', projectId),
    showProjectInFolder: (projectId) =>
      ipcRenderer.invoke('host:storage.showProjectInFolder', projectId),
    revealProjectFile: (input) => ipcRenderer.invoke('host:storage.revealProjectFile', input),
    loadConversations: () => ipcRenderer.invoke('host:storage.loadConversations'),
    forkConversation: (input) => ipcRenderer.invoke('host:storage.forkConversation', input),
    saveConversationMeta: (conversation) =>
      ipcRenderer.invoke('host:storage.saveConversationMeta', conversation),
    deleteConversation: (conversationId) =>
      ipcRenderer.invoke('host:storage.deleteConversation', conversationId),
    deleteChatMessages: (input) => ipcRenderer.invoke('host:storage.deleteChatMessages', input),
    upsertChatMessages: (input) => ipcRenderer.invoke('host:storage.upsertChatMessages', input),
    saveChatMessageState: (input) => ipcRenderer.invoke('host:storage.saveChatMessageState', input),
    loadComposerDrafts: () => ipcRenderer.invoke('host:storage.loadComposerDrafts'),
    saveComposerDraft: (draft) => ipcRenderer.invoke('host:storage.saveComposerDraft', draft),
    loadUiPreferences: () => ipcRenderer.invoke('host:storage.loadUiPreferences'),
    saveUiPreferences: (preferences) =>
      ipcRenderer.invoke('host:storage.saveUiPreferences', preferences),
    selectProfileAvatar: () => ipcRenderer.invoke('host:storage.selectProfileAvatar'),
    loadAttachmentImage: (input) => ipcRenderer.invoke('host:storage.loadAttachmentImage', input),
    loadInputAttachments: (input) => ipcRenderer.invoke('host:storage.loadInputAttachments', input),
    loadImageFile: (input) => ipcRenderer.invoke('host:storage.loadImageFile', input)
  },
  terminal: {
    createSession: (request) => ipcRenderer.invoke('host:terminal.createSession', request),
    writeInput: (sessionId, data) =>
      ipcRenderer.invoke('host:terminal.writeInput', sessionId, data),
    resizeSession: (sessionId, cols, rows) =>
      ipcRenderer.invoke('host:terminal.resizeSession', sessionId, cols, rows),
    killSession: (sessionId) => ipcRenderer.invoke('host:terminal.killSession', sessionId),
    onOutput: onTerminalOutput,
    onExit: onTerminalExit
  }
}

if (process.contextIsolated) {
  contextBridge.exposeInMainWorld('mycopilot', { host })
} else {
  throw new Error('MyCopilot preload requires context isolation')
}
