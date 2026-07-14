import {
  BrowserWindow,
  clipboard,
  dialog,
  ipcMain as electronIpcMain,
  nativeImage,
  nativeTheme,
  shell
} from 'electron'
import type {
  IpcMainInvokeEvent,
  NativeImage,
  OpenDialogOptions,
  OpenDialogReturnValue
} from 'electron'
import { execFile } from 'child_process'
import { homedir, tmpdir } from 'os'
import { promisify } from 'util'
import { basename, extname, isAbsolute, join, relative, resolve } from 'path'
import { mkdtemp, readFile, rm, writeFile } from 'fs/promises'
import type { StorageImageFileRecord } from '@mycopilot/protocol'
import type { StorageProjectRecord } from '@mycopilot/protocol'
import type { AppWindowState } from '@mycopilot/host-api'
import { BROWSER_WEBVIEW_PARTITION } from '@mycopilot/protocol'

import { CoreServer } from './core/coreServer'
import { clearManagedWebviewData } from './webviews/managedWebviewSecurity'
import { TerminalBridge } from './terminal/TerminalBridge'
import { AttachmentDialogBridge } from './attachments/AttachmentDialogBridge'
import { FaviconResourceCache } from './resources/FaviconResourceCache'

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
const CLIPBOARD_IMAGE_MAX_BYTES = 16 * 1024 * 1024
const CLIPBOARD_IMAGE_MAX_DATA_URL_CHARS = Math.ceil((CLIPBOARD_IMAGE_MAX_BYTES * 4) / 3) + 1024

type NativeThemeSource = 'system' | 'light' | 'dark'

function isNativeThemeSource(value: unknown): value is NativeThemeSource {
  return value === 'system' || value === 'light' || value === 'dark'
}

export function openExternalUrl(value: unknown): Promise<void> {
  if (typeof value !== 'string') {
    throw new Error('External URL must be a string')
  }
  const url = new URL(value)
  if (!['http:', 'https:', 'mailto:'].includes(url.protocol)) {
    throw new Error(`Unsupported external URL protocol: ${url.protocol}`)
  }
  return shell.openExternal(url.toString())
}

function getAppWindowState(window: BrowserWindow | undefined): AppWindowState {
  return {
    isFullScreen: window?.isFullScreen() ?? false,
    isMaximized: window?.isMaximized() ?? false
  }
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
  dataUrl: string
}): Promise<{ formats: string[]; width: number; height: number; method: string }> {
  const imageBuffer = imageBufferFromClipboardInput(input)
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

function imageBufferFromClipboardInput(input: { dataUrl: string }): Buffer {
  if (!input || typeof input.dataUrl !== 'string') {
    throw new Error('Image data URL is required')
  }
  const dataUrl = input.dataUrl.trim()
  if (!dataUrl.startsWith('data:image/') || dataUrl.length > CLIPBOARD_IMAGE_MAX_DATA_URL_CHARS) {
    throw new Error('Image data URL is invalid or too large')
  }
  return bufferFromDataUrl(dataUrl)
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

function writeElectronImageToClipboard(image: NativeImage): {
  formats: string[]
  width: number
  height: number
  method: string
} {
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
  const match = /^data:image\/[-+.\w]+(?:;[-\w=.+]+)*;base64,([a-z0-9+/]*={0,2})$/is.exec(dataUrl)
  if (!match) {
    throw new Error('Base64 image data URL is required')
  }
  const buffer = Buffer.from(match[1], 'base64')
  if (buffer.length === 0 || buffer.length > CLIPBOARD_IMAGE_MAX_BYTES) {
    throw new Error('Image data is empty or too large')
  }
  return buffer
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
  faviconResourceCache: FaviconResourceCache,
  isTrustedRenderer: (event: IpcMainInvokeEvent) => boolean
): void {
  const attachmentDialogBridge = new AttachmentDialogBridge()
  type InvokeHandler = Parameters<typeof electronIpcMain.handle>[1]
  const ipcMain = {
    handle(channel: string, handler: InvokeHandler): void {
      electronIpcMain.handle(channel, (event, ...args) => {
        if (!isTrustedRenderer(event)) {
          throw new Error(`Blocked untrusted IPC sender for ${channel}`)
        }
        return handler(event, ...args)
      })
    }
  }

  coreServer.onAgentEvent((event) => {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed() && !window.webContents.isDestroyed()) {
        window.webContents.send('host:agent.event', event)
      }
    }
  })

  ipcMain.handle('host:core.ping', (_event, input) => coreServer.ping(input))
  ipcMain.handle('host:app.getWindowState', (event) => getAppWindowState(getInvokeWindow(event)))
  ipcMain.handle('host:app.openExternal', (_event, url) => openExternalUrl(url))
  ipcMain.handle('host:app.setNativeThemeSource', (_event, themeSource) => {
    if (!isNativeThemeSource(themeSource)) {
      throw new Error('Invalid native theme source')
    }
    nativeTheme.themeSource = themeSource
  })
  ipcMain.handle('host:agent.startConversationTurn', (_event, input) =>
    coreServer.startConversationTurn(input)
  )
  ipcMain.handle('host:agent.getContextWindowSnapshot', (_event, input) =>
    coreServer.getContextWindowSnapshot(input)
  )
  ipcMain.handle('host:agent.getContextCompactionAudit', (_event, input) =>
    coreServer.getContextCompactionAudit(input)
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
  ipcMain.handle('host:agent.readFileDraft', (_event, input) => coreServer.readFileDraft(input))
  ipcMain.handle('host:agent.getFileWriteDiff', (_event, input) =>
    coreServer.getFileWriteDiff(input)
  )
  ipcMain.handle('host:search.searchChats', (_event, input) => coreServer.searchChats(input))
  ipcMain.handle('host:attachments.selectInputAttachments', (event, request) =>
    attachmentDialogBridge.selectInputAttachments(event, request)
  )
  ipcMain.handle('host:browser.clearBrowsingData', async () => {
    await Promise.all([
      clearManagedWebviewData(BROWSER_WEBVIEW_PARTITION),
      faviconResourceCache.clear()
    ])
  })
  ipcMain.handle('host:clipboard.writeImage', (_event, input) => writeImageToClipboard(input))
  ipcMain.handle('host:resources.resolveFavicon', (_event, input) =>
    faviconResourceCache.resolveFavicon(input)
  )
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
  ipcMain.handle('host:storage.forkConversation', (_event, input) =>
    coreServer.forkConversation(input)
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
  ipcMain.handle('host:storage.loadImageFile', (_event, input) => loadImageFile(coreServer, input))
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
