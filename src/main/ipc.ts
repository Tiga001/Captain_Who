// Electron main client.
import { ipcMain } from 'electron'

import { CoreServer } from './core/coreServer'
import { BrowserWebContentsViewManager } from './browser/BrowserWebContentsViewManager'
import { TerminalBridge } from './terminal/TerminalBridge'

export function registerHostIpc(
  coreServer: CoreServer,
  terminalBridge: TerminalBridge,
  getBrowserManager: () => BrowserWebContentsViewManager
): void {
  ipcMain.handle('host:core.ping', (_event, input) => coreServer.ping(input))
  ipcMain.handle('host:app.getVersion', () => coreServer.getVersion())
  ipcMain.handle('host:agent.startRun', (_event, input) => coreServer.startRun(input))
  ipcMain.handle('host:agent.cancelRun', (_event, input) => coreServer.cancelRun(input))
  ipcMain.handle('host:browser.create', (_event, input) => getBrowserManager().create(input))
  ipcMain.handle('host:browser.navigate', (_event, input) => getBrowserManager().navigate(input))
  ipcMain.handle('host:browser.setBounds', (_event, input) => getBrowserManager().setBounds(input))
  ipcMain.handle('host:browser.destroy', (_event, input) => getBrowserManager().destroy(input))
  ipcMain.handle('host:storage.loadAppData', () => coreServer.loadAppData())
  ipcMain.handle('host:storage.loadModelSettings', () => coreServer.loadModelSettings())
  ipcMain.handle('host:storage.saveModelSettings', (_event, settings) =>
    coreServer.saveModelSettings(settings)
  )
  ipcMain.handle('host:storage.loadAgentPromptPreferences', () =>
    coreServer.loadAgentPromptPreferences()
  )
  ipcMain.handle('host:storage.saveAgentPromptPreferences', (_event, preferences) =>
    coreServer.saveAgentPromptPreferences(preferences)
  )
  ipcMain.handle('host:storage.loadProjects', () => coreServer.loadProjects())
  ipcMain.handle('host:storage.selectProjectDirectory', () => coreServer.selectProjectDirectory())
  ipcMain.handle('host:storage.saveProject', (_event, project) => coreServer.saveProject(project))
  ipcMain.handle('host:storage.deleteProject', (_event, projectId) =>
    coreServer.deleteProject(projectId)
  )
  ipcMain.handle('host:storage.showProjectInFolder', (_event, projectId) =>
    coreServer.showProjectInFolder(projectId)
  )
  ipcMain.handle('host:storage.revealProjectFile', (_event, input) =>
    coreServer.revealProjectFile(input)
  )
  ipcMain.handle('host:storage.loadConversations', () => coreServer.loadConversations())
  ipcMain.handle('host:storage.saveConversation', (_event, conversation) =>
    coreServer.saveConversation(conversation)
  )
  ipcMain.handle('host:storage.saveConversationMeta', (_event, conversation) =>
    coreServer.saveConversationMeta(conversation)
  )
  ipcMain.handle('host:storage.deleteConversation', (_event, conversationId) =>
    coreServer.deleteConversation(conversationId)
  )
  ipcMain.handle('host:storage.upsertChatMessages', (_event, input) =>
    coreServer.upsertChatMessages(input)
  )
  ipcMain.handle('host:storage.saveChatMessageState', (_event, input) =>
    coreServer.saveChatMessageState(input)
  )
  ipcMain.handle('host:storage.loadComposerDrafts', () => coreServer.loadComposerDrafts())
  ipcMain.handle('host:storage.saveComposerDraft', (_event, draft) =>
    coreServer.saveComposerDraft(draft)
  )
  ipcMain.handle('host:storage.deleteComposerDraft', (_event, scopeId) =>
    coreServer.deleteComposerDraft(scopeId)
  )
  ipcMain.handle('host:storage.loadUiPreferences', () => coreServer.loadUiPreferences())
  ipcMain.handle('host:storage.saveUiPreferences', (_event, preferences) =>
    coreServer.saveUiPreferences(preferences)
  )
  ipcMain.handle('host:storage.selectProfileAvatar', () => coreServer.selectProfileAvatar())
  ipcMain.handle('host:terminal.createSession', (_event, request) =>
    terminalBridge.createSession(request)
  )
  ipcMain.handle('host:terminal.writeInput', (_event, sessionId, data) =>
    terminalBridge.writeInput(sessionId, data)
  )
  ipcMain.handle('host:terminal.resizeSession', (_event, sessionId, cols, rows) =>
    terminalBridge.resizeSession(sessionId, cols, rows)
  )
  ipcMain.handle('host:terminal.killSession', (_event, sessionId) =>
    terminalBridge.killSession(sessionId)
  )
}
