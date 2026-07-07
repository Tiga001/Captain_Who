// Electron main client.
import { BrowserWindow, dialog, ipcMain, shell } from 'electron'
import type { IpcMainInvokeEvent, OpenDialogOptions } from 'electron'
import { basename, extname, isAbsolute, join } from 'path'
import { readFile } from 'fs/promises'
import type { StorageProjectRecord } from '@mycopilot/protocol'

import { CoreServer } from './core/coreServer'
import { BrowserWebContentsViewManager } from './browser/BrowserWebContentsViewManager'
import { TerminalBridge } from './terminal/TerminalBridge'

const IMAGE_MIME_BY_EXTENSION: Record<string, string> = {
  '.gif': 'image/gif',
  '.jpeg': 'image/jpeg',
  '.jpg': 'image/jpeg',
  '.png': 'image/png',
  '.webp': 'image/webp'
}

function getInvokeWindow(event: IpcMainInvokeEvent): BrowserWindow | undefined {
  return BrowserWindow.fromWebContents(event.sender) ?? undefined
}

function showOpenDialog(event: IpcMainInvokeEvent, options: OpenDialogOptions) {
  const window = getInvokeWindow(event)
  return window ? dialog.showOpenDialog(window, options) : dialog.showOpenDialog(options)
}

function createProjectRecord(directoryPath: string): StorageProjectRecord {
  const now = Date.now()
  const name = basename(directoryPath) || directoryPath
  const slug = name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')

  return {
    id: `project-${slug || 'workspace'}-${now}`,
    name,
    path: directoryPath,
    createdAt: now,
    pinnedAt: null
  }
}

async function selectProjectDirectory(
  event: IpcMainInvokeEvent
): Promise<StorageProjectRecord | null> {
  const result = await showOpenDialog(event, {
    title: 'Select project directory',
    properties: ['openDirectory', 'createDirectory']
  })

  if (result.canceled || !result.filePaths[0]) {
    return null
  }

  return createProjectRecord(result.filePaths[0])
}

async function selectProfileAvatar(event: IpcMainInvokeEvent): Promise<string | null> {
  const result = await showOpenDialog(event, {
    title: 'Select profile avatar',
    properties: ['openFile'],
    filters: [
      {
        name: 'Images',
        extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif']
      }
    ]
  })

  if (result.canceled || !result.filePaths[0]) {
    return null
  }

  const filePath = result.filePaths[0]
  const mimeType = IMAGE_MIME_BY_EXTENSION[extname(filePath).toLowerCase()] ?? 'image/png'
  const data = await readFile(filePath)
  return `data:${mimeType};base64,${data.toString('base64')}`
}

async function getProjectPath(coreServer: CoreServer, projectId: string): Promise<string | null> {
  const projects = await coreServer.loadProjects()
  return projects.find((project) => project.id === projectId)?.path ?? null
}

async function showProjectInFolder(coreServer: CoreServer, projectId: string): Promise<void> {
  const projectPath = await getProjectPath(coreServer, projectId)
  if (!projectPath) {
    throw new Error('Project path is not available')
  }

  shell.showItemInFolder(projectPath)
}

async function revealProjectFile(
  coreServer: CoreServer,
  input: { projectId?: string | null; filePath: string }
): Promise<void> {
  const rawFilePath = input.filePath.trim()
  if (!rawFilePath) {
    throw new Error('File path is required')
  }

  if (isAbsolute(rawFilePath)) {
    shell.showItemInFolder(rawFilePath)
    return
  }

  if (!input.projectId) {
    throw new Error('Project id is required for relative file paths')
  }

  const projectPath = await getProjectPath(coreServer, input.projectId)
  if (!projectPath) {
    throw new Error('Project path is not available')
  }

  shell.showItemInFolder(join(projectPath, rawFilePath))
}

export function registerHostIpc(
  coreServer: CoreServer,
  terminalBridge: TerminalBridge,
  getBrowserManager: () => BrowserWebContentsViewManager
): void {
  coreServer.onAgentEvent((event) => {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed()) {
        window.webContents.send('host:agent.event', event)
      }
    }
  })

  ipcMain.handle('host:core.ping', (_event, input) => coreServer.ping(input))
  ipcMain.handle('host:app.getVersion', () => coreServer.getVersion())
  ipcMain.handle('host:agent.startRun', (_event, input) => coreServer.startRun(input))
  ipcMain.handle('host:agent.startConversationTurn', (_event, input) =>
    coreServer.startConversationTurn(input)
  )
  ipcMain.handle('host:agent.cancelRun', (_event, input) => coreServer.cancelRun(input))
  ipcMain.handle('host:agent.listPendingActions', () => coreServer.listPendingActions())
  ipcMain.handle('host:agent.approveAction', (_event, input) => coreServer.approveAction(input))
  ipcMain.handle('host:agent.rejectAction', (_event, input) => coreServer.rejectAction(input))
  ipcMain.handle('host:agent.cancelAction', (_event, input) => coreServer.cancelAction(input))
  ipcMain.handle('host:agent.getUsageSummary', (_event, input) => coreServer.getUsageSummary(input))
  ipcMain.handle('host:agent.clearUsageRecords', (_event, input) =>
    coreServer.clearUsageRecords(input)
  )
  ipcMain.handle('host:browser.createView', (_event, request) =>
    getBrowserManager().createView(request)
  )
  ipcMain.handle('host:browser.destroyView', (_event, id) => getBrowserManager().destroyView(id))
  ipcMain.handle('host:browser.setBounds', (_event, id, bounds) =>
    getBrowserManager().setBounds(id, bounds)
  )
  ipcMain.handle('host:browser.showView', (_event, id) => getBrowserManager().showView(id))
  ipcMain.handle('host:browser.hideView', (_event, id) => getBrowserManager().hideView(id))
  ipcMain.handle('host:browser.navigate', (_event, request) =>
    getBrowserManager().navigate(request)
  )
  ipcMain.handle('host:browser.reload', (_event, id) => getBrowserManager().reload(id))
  ipcMain.handle('host:browser.goBack', (_event, id) => getBrowserManager().goBack(id))
  ipcMain.handle('host:browser.goForward', (_event, id) => getBrowserManager().goForward(id))
  ipcMain.handle('host:browser.setZoom', (_event, id, zoomFactor) =>
    getBrowserManager().setZoom(id, zoomFactor)
  )
  ipcMain.handle('host:browser.clearBrowsingData', (_event, id) =>
    getBrowserManager().clearBrowsingData(id)
  )
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
  ipcMain.handle('host:storage.selectProjectDirectory', (event) => selectProjectDirectory(event))
  ipcMain.handle('host:storage.saveProject', (_event, project) => coreServer.saveProject(project))
  ipcMain.handle('host:storage.deleteProject', (_event, projectId) =>
    coreServer.deleteProject(projectId)
  )
  ipcMain.handle('host:storage.showProjectInFolder', (_event, projectId) =>
    showProjectInFolder(coreServer, projectId)
  )
  ipcMain.handle('host:storage.revealProjectFile', (_event, input) =>
    revealProjectFile(coreServer, input)
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
  ipcMain.handle('host:storage.selectProfileAvatar', (event) => selectProfileAvatar(event))
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
