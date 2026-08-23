import { BrowserWindow, dialog, nativeTheme, shell } from 'electron'
import type {
  IpcMainInvokeEvent,
  OpenDialogOptions,
  OpenDialogReturnValue,
  SaveDialogOptions,
  SaveDialogReturnValue
} from 'electron'
import { homedir } from 'os'
import { basename, extname, isAbsolute, join, relative, resolve } from 'path'
import { readFile } from 'fs/promises'
import type { StorageImageFileRecord, StorageProjectRecord } from '@mycopilot/protocol'
import { HOST_CHANNELS, type AppWindowState } from '@mycopilot/host-api'
import { BROWSER_WEBVIEW_PARTITION } from '@mycopilot/protocol'

import { CoreServer } from './core/coreServer'
import { clearManagedWebviewData } from './webviews/managedWebviewSecurity'
import { TerminalBridge } from './terminal/TerminalBridge'
import { AttachmentDialogBridge } from './attachments/AttachmentDialogBridge'
import { FaviconResourceCache } from './resources/FaviconResourceCache'
import { WorkspaceFilesService } from './workspaceFiles/WorkspaceFilesService'
import { registerAgentIpc } from './ipc/agentIpc'
import { registerAutomationIpc } from './ipc/automationIpc'
import { registerGitIpc } from './ipc/gitIpc'
import { registerSkillsIpc } from './ipc/skillsIpc'
import { registerMcpIpc } from './ipc/mcpIpc'
import { registerCoreServiceIpc } from './ipc/serviceIpc'
import { registerStorageIpc } from './ipc/storageIpc'
import { registerTerminalIpc } from './ipc/terminalIpc'
import { createTrustedIpcMain } from './ipc/trustedIpc'
import { registerWorkspaceFilesIpc } from './ipc/workspaceFilesIpc'
import type { BrowserSurfaceManager } from './browser/BrowserSurfaceManager'
import { registerBrowserSurfaceIpc } from './ipc/browserSurfaceIpc'
import { registerBrowserArtifactIpc } from './ipc/browserArtifactIpc'
import type { BrowserArtifactBroker } from './browser/BrowserArtifactBroker'

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

function showSaveDialog(
  event: IpcMainInvokeEvent,
  options: SaveDialogOptions
): Promise<SaveDialogReturnValue> {
  const window = getInvokeWindow(event)
  return window ? dialog.showSaveDialog(window, options) : dialog.showSaveDialog(options)
}

async function selectBrowserArtifactExportPath(
  event: IpcMainInvokeEvent,
  suggestedFileName: string
): Promise<string | null> {
  const result = await showSaveDialog(event, {
    defaultPath: suggestedFileName,
    properties: ['createDirectory', 'showOverwriteConfirmation']
  })
  return result.canceled || !result.filePath ? null : result.filePath
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

/** Selects one local Skill package root without granting the renderer broader filesystem access. */
export async function selectInstallationDirectory(
  event: IpcMainInvokeEvent
): Promise<string | null> {
  const result = await showOpenDialog(event, {
    title: 'Select Skill installation directory',
    properties: ['openDirectory']
  })

  const selectedPath = result.filePaths[0]
  if (result.canceled || !selectedPath) {
    return null
  }

  return resolve(selectedPath)
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

export interface HostIpcRegistration {
  (): void
  beginAutomationShutdown(): void
}

export function registerHostIpc(
  coreServer: CoreServer,
  terminalBridge: TerminalBridge,
  faviconResourceCache: FaviconResourceCache,
  isTrustedRenderer: (event: IpcMainInvokeEvent) => boolean,
  browserSurfaceManager?: BrowserSurfaceManager,
  browserArtifactBroker?: BrowserArtifactBroker
): HostIpcRegistration {
  const attachmentDialogBridge = new AttachmentDialogBridge()
  const workspaceFilesService = new WorkspaceFilesService((projectId) =>
    getProjectPath(coreServer, projectId)
  )
  const ipcMain = createTrustedIpcMain(isTrustedRenderer)

  registerCoreServiceIpc(ipcMain, coreServer)
  registerAgentIpc(ipcMain, coreServer)
  const disposeAutomationIpc = registerAutomationIpc(ipcMain, coreServer)
  registerSkillsIpc(ipcMain, coreServer, selectInstallationDirectory)
  const disposeMcpIpc = registerMcpIpc(ipcMain, coreServer)
  registerGitIpc(ipcMain, coreServer)
  registerStorageIpc(ipcMain, coreServer, {
    loadImageFile,
    revealProjectFile,
    selectProfileAvatar,
    selectProjectDirectory,
    showProjectInFolder
  })
  registerWorkspaceFilesIpc(ipcMain, workspaceFilesService)
  registerTerminalIpc(ipcMain, terminalBridge)

  ipcMain.handle(HOST_CHANNELS.app.getWindowState, (event) =>
    getAppWindowState(getInvokeWindow(event))
  )
  ipcMain.handle(HOST_CHANNELS.app.openExternal, (_event, url) => openExternalUrl(url))
  ipcMain.handle(HOST_CHANNELS.app.setNativeThemeSource, (_event, themeSource) => {
    if (!isNativeThemeSource(themeSource)) {
      throw new Error('Invalid native theme source')
    }
    nativeTheme.themeSource = themeSource
  })
  ipcMain.handle(HOST_CHANNELS.attachments.selectInputAttachments, (event, request) =>
    attachmentDialogBridge.selectInputAttachments(event, request)
  )
  ipcMain.handle(HOST_CHANNELS.browser.clearBrowsingData, async () => {
    await Promise.all([
      clearManagedWebviewData(BROWSER_WEBVIEW_PARTITION),
      faviconResourceCache.clear()
    ])
  })
  if (browserSurfaceManager) {
    registerBrowserSurfaceIpc(ipcMain, browserSurfaceManager)
  }
  if (browserArtifactBroker) {
    registerBrowserArtifactIpc(ipcMain, browserArtifactBroker, selectBrowserArtifactExportPath)
  }
  ipcMain.handle(HOST_CHANNELS.resources.resolveFavicon, (_event, input) =>
    faviconResourceCache.resolveFavicon(input)
  )
  const dispose = (): void => {
    disposeAutomationIpc()
    disposeMcpIpc()
  }
  dispose.beginAutomationShutdown = (): void => disposeAutomationIpc.beginShutdown()
  return dispose
}
