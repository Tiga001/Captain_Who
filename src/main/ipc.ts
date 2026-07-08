// Electron main client.
import { BrowserWindow, clipboard, dialog, ipcMain, nativeImage, nativeTheme, shell } from 'electron'
import type { IpcMainInvokeEvent, NativeImage, OpenDialogOptions, OpenDialogReturnValue } from 'electron'
import { execFile } from 'child_process'
import { homedir, tmpdir } from 'os'
import { promisify } from 'util'
import { basename, extname, isAbsolute, join, relative, resolve } from 'path'
import { mkdtemp, readFile, rm, writeFile } from 'fs/promises'
import { fileURLToPath } from 'url'
import type { StorageImageFileRecord } from '@mycopilot/protocol'
import type { StorageProjectRecord } from '@mycopilot/protocol'

import { CoreServer } from './core/coreServer'
import { BrowserWebContentsViewManager } from './browser/BrowserWebContentsViewManager'
import { TerminalBridge } from './terminal/TerminalBridge'
import { AttachmentDialogBridge } from './attachments/AttachmentDialogBridge'

const IMAGE_MIME_BY_EXTENSION: Record<string, string> = {
  '.avif': 'image/avif',
  '.bmp': 'image/bmp',
  '.gif': 'image/gif',
  '.jpeg': 'image/jpeg',
  '.jpg': 'image/jpeg',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.tif': 'image/tiff',
  '.tiff': 'image/tiff',
  '.webp': 'image/webp'
}

const execFileAsync = promisify(execFile)

type NativeThemeSource = 'system' | 'light' | 'dark'

function isNativeThemeSource(value: unknown): value is NativeThemeSource {
  return value === 'system' || value === 'light' || value === 'dark'
}

function getInvokeWindow(event: IpcMainInvokeEvent): BrowserWindow | undefined {
  return BrowserWindow.fromWebContents(event.sender) ?? undefined
}

function showOpenDialog(
  event: IpcMainInvokeEvent,
  options: OpenDialogOptions
): Promise<OpenDialogReturnValue> {
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

async function writeImageToClipboard(input: {
  dataUrl?: string
  imageUrl?: string
}): Promise<{ formats: string[]; width: number; height: number; method: string }> {
  const imageBuffer = await imageBufferFromClipboardInput(input)
  const image = nativeImage.createFromBuffer(imageBuffer)
  if (image.isEmpty()) {
    throw new Error('Image data is invalid')
  }
  const pngBuffer = image.toPNG()
  if (pngBuffer.length === 0) {
    throw new Error('Image PNG data is invalid')
  }

  if (process.platform === 'darwin') {
    return writeMacImageToClipboard(image, pngBuffer)
  }

  return writeElectronImageToClipboard(image)
}

async function imageBufferFromClipboardInput(input: {
  dataUrl?: string
  imageUrl?: string
}): Promise<Buffer> {
  const dataUrl = input.dataUrl?.trim()
  if (dataUrl) {
    if (!dataUrl.startsWith('data:image/')) {
      throw new Error('Image data URL is required')
    }
    return bufferFromDataUrl(dataUrl)
  }

  const imageUrl = input.imageUrl?.trim()
  if (!imageUrl) {
    throw new Error('Image source is required')
  }

  let url: URL
  try {
    url = new URL(imageUrl)
  } catch {
    throw new Error('Image URL is invalid')
  }

  if (url.protocol === 'file:') {
    return readFile(fileURLToPath(url))
  }

  if (url.protocol === 'http:' || url.protocol === 'https:') {
    const response = await fetch(url.toString())
    if (!response.ok) {
      throw new Error(`Image request failed with status ${response.status}`)
    }
    return Buffer.from(await response.arrayBuffer())
  }

  throw new Error(`Unsupported image URL protocol: ${url.protocol}`)
}

async function writeMacImageToClipboard(
  image: NativeImage,
  pngBuffer: Buffer
): Promise<{ formats: string[]; width: number; height: number; method: string }> {
  clipboard.clear()
  clipboard.writeBuffer('public.png', pngBuffer)
  let method = 'public.png'

  if (clipboard.readBuffer('public.png').length === 0) {
    await writePngToMacPasteboard(pngBuffer)
    method = 'osascript-pngf'
  }

  const formats = clipboard.availableFormats()
  const hasPng = clipboard.readBuffer('public.png').length > 0 || formats.includes('image/png')
  if (!hasPng) {
    throw new Error(`Image clipboard write failed: ${clipboard.availableFormats().join(', ')}`)
  }

  const size = image.getSize()
  return {
    formats,
    width: size.width,
    height: size.height,
    method
  }
}

function writeElectronImageToClipboard(
  image: NativeImage
): { formats: string[]; width: number; height: number; method: string } {
  clipboard.clear()
  clipboard.writeImage(image)

  const clipboardImage = clipboard.readImage()
  if (clipboardImage.isEmpty()) {
    throw new Error(`Image clipboard write failed: ${clipboard.availableFormats().join(', ')}`)
  }

  const size = clipboardImage.getSize()
  return {
    formats: clipboard.availableFormats(),
    width: size.width,
    height: size.height,
    method: 'electron-write-image'
  }
}

function bufferFromDataUrl(dataUrl: string): Buffer {
  const match = /^data:(image\/[-+.\w]+)((?:;[-\w=.+]+)*),(.*)$/s.exec(dataUrl)
  if (!match) {
    throw new Error('Image data URL is invalid')
  }
  const metadata = match[2].toLowerCase()
  const payload = match[3]
  if (metadata.split(';').includes('base64')) {
    return Buffer.from(payload, 'base64')
  }
  return Buffer.from(decodeURIComponent(payload), 'utf8')
}

function escapeAppleScriptString(value: string): string {
  return value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')
}

async function writePngToMacPasteboard(pngBuffer: Buffer): Promise<void> {
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-clipboard-'))
  const filePath = join(directory, 'image.png')

  try {
    await writeFile(filePath, pngBuffer)
    await execFileAsync('/usr/bin/osascript', [
      '-e',
      `set imageFile to POSIX file "${escapeAppleScriptString(filePath)}"\nset the clipboard to (read imageFile as «class PNGf»)`
    ])
  } finally {
    await rm(directory, { recursive: true, force: true }).catch(() => undefined)
  }
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

async function loadImageFile(
  coreServer: CoreServer,
  input: { projectId?: string | null; filePath?: string }
): Promise<StorageImageFileRecord | null> {
  const rawFilePath = input.filePath?.trim()
  if (!rawFilePath) return null

  const attachmentId = attachmentIdFromReadPath(rawFilePath)
  if (attachmentId) {
    const attachment = await coreServer.loadAttachmentImage({ attachmentId })
    return attachment
      ? {
          name: attachment.name,
          mimeType: attachment.mimeType,
          sizeBytes: attachment.sizeBytes,
          data: attachment.data
        }
      : null
  }

  const filePath = await resolveReadableImageFilePath(coreServer, input.projectId, rawFilePath)
  if (!filePath) return null

  const mimeType = IMAGE_MIME_BY_EXTENSION[extname(filePath).toLowerCase()]
  if (!mimeType?.startsWith('image/')) return null

  try {
    const data = await readFile(filePath)
    return {
      name: basename(filePath),
      mimeType,
      sizeBytes: data.byteLength,
      data: data.toString('base64')
    }
  } catch (error) {
    if (typeof error === 'object' && error && 'code' in error && error.code === 'ENOENT') {
      return null
    }
    throw error
  }
}

function attachmentIdFromReadPath(filePath: string): string | null {
  const match = /^@attachments\/([^/\\]+)/.exec(filePath.trim())
  return match ? decodeURIComponent(match[1]) : null
}

async function resolveReadableImageFilePath(
  coreServer: CoreServer,
  projectId: string | null | undefined,
  rawFilePath: string
): Promise<string | null> {
  if (isAbsolute(rawFilePath)) return rawFilePath

  const aliasPath = expandSystemPathAlias(rawFilePath)
  if (aliasPath) return aliasPath

  if (!projectId) return null
  const projectPath = await getProjectPath(coreServer, projectId)
  if (!projectPath) return null

  const root = resolve(projectPath)
  const candidate = resolve(root, rawFilePath)
  const candidateRelative = relative(root, candidate)
  if (candidateRelative.startsWith('..') || isAbsolute(candidateRelative)) {
    return null
  }
  return candidate
}

function expandSystemPathAlias(rawFilePath: string): string | null {
  const aliases: Record<string, string> = {
    '@desktop': join(homedir(), 'Desktop'),
    '@documents': join(homedir(), 'Documents'),
    '@downloads': join(homedir(), 'Downloads'),
    '@home': homedir()
  }
  const normalized = rawFilePath.trim()
  const [alias, ...rest] = normalized.split(/[\\/]+/)
  const root = aliases[alias.toLowerCase()]
  if (!root) return null
  return rest.length > 0 ? join(root, ...rest) : root
}

export function registerHostIpc(
  coreServer: CoreServer,
  terminalBridge: TerminalBridge,
  getBrowserManager: () => BrowserWebContentsViewManager
): void {
  const attachmentDialogBridge = new AttachmentDialogBridge()

  coreServer.onAgentEvent((event) => {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed()) {
        window.webContents.send('host:agent.event', event)
      }
    }
  })

  ipcMain.handle('host:core.ping', (_event, input) => coreServer.ping(input))
  ipcMain.handle('host:app.getVersion', () => coreServer.getVersion())
  ipcMain.handle('host:app.setNativeThemeSource', (_event, themeSource) => {
    if (!isNativeThemeSource(themeSource)) {
      throw new Error('Invalid native theme source')
    }
    nativeTheme.themeSource = themeSource
  })
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
  ipcMain.handle('host:attachments.selectInputAttachments', (event, request) =>
    attachmentDialogBridge.selectInputAttachments(event, request)
  )
  ipcMain.handle('host:attachments.loadInputAttachmentsFromPaths', (_event, request) =>
    attachmentDialogBridge.loadInputAttachmentsFromPaths(request)
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
  ipcMain.handle('host:clipboard.writeImage', (_event, input) => writeImageToClipboard(input))
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
  ipcMain.handle('host:storage.deleteChatMessages', (_event, input) =>
    coreServer.deleteChatMessages(input)
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
  ipcMain.handle('host:storage.loadAttachmentImage', (_event, input) =>
    coreServer.loadAttachmentImage(input)
  )
  ipcMain.handle('host:storage.loadInputAttachments', (_event, input) =>
    coreServer.loadInputAttachments(input)
  )
  ipcMain.handle('host:storage.loadImageFile', (_event, input) =>
    loadImageFile(coreServer, input)
  )
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
