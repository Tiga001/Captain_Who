import { BrowserWindow, dialog, nativeTheme, shell } from 'electron'
import type {
  IpcMainInvokeEvent,
  OpenDialogOptions,
  OpenDialogReturnValue,
  SaveDialogOptions,
  SaveDialogReturnValue,
  WebContents
} from 'electron'
import { basename, extname, resolve } from 'path'
import { readFile } from 'fs/promises'
import type { StorageImageFileRecord, StorageProjectFolderPick } from '@mycopilot/protocol'
import { HOST_CHANNELS, type AppWindowState } from '@mycopilot/host-api'
import { CoreServer } from './core/coreServer'
import { primaryProjectFolderPath } from './projects/projectFolders'
import { resolveProjectFileReference, type ProjectFileReference } from './projects/projectFilePaths'
import { TerminalBridge } from './terminal/TerminalBridge'
import { AttachmentDialogBridge } from './attachments/AttachmentDialogBridge'
import { FaviconResourceCache } from './resources/FaviconResourceCache'
import { WorkspaceFilesService } from './workspaceFiles/WorkspaceFilesService'
import { registerAgentIpc } from './ipc/agentIpc'
import { registerAutomationIpc } from './ipc/automationIpc'
import { registerNotificationIpc } from './ipc/notificationIpc'
import { registerHumanInteractionIpc } from './ipc/humanInteractionIpc'
import {
  createVolatileNotificationLocaleMirror,
  type NotificationLocaleMirror
} from './notifications/notificationLocaleStore'
import {
  createVolatileAppearanceThemeMirror,
  isAppearanceThemePreference,
  type AppearanceThemeMirror
} from './appearance/appearanceThemeStore'
import { applyAdaptiveAppIcon } from './appIcon'
import { registerGitIpc } from './ipc/gitIpc'
import { registerSkillsIpc } from './ipc/skillsIpc'
import { registerMcpIpc } from './ipc/mcpIpc'
import { registerCoreServiceIpc } from './ipc/serviceIpc'
import { registerStorageIpc } from './ipc/storageIpc'
import { registerConfigurationNotifications } from './ipc/configurationNotifications'
import { registerTerminalIpc } from './ipc/terminalIpc'
import { createTrustedIpcMain } from './ipc/trustedIpc'
import { RendererQuitFlushCoordinator } from './ipc/rendererQuitFlush'
import { registerWorkspaceFilesIpc } from './ipc/workspaceFilesIpc'
import type { BrowserSurfaceManager } from './browser/BrowserSurfaceManager'
import { registerBrowserSurfaceIpc } from './ipc/browserSurfaceIpc'
import { registerBrowserArtifactIpc } from './ipc/browserArtifactIpc'
import { registerBrowserDownloadIpc } from './ipc/browserDownloadIpc'
import type { BrowserArtifactBroker } from './browser/BrowserArtifactBroker'
import type { BrowserDownloadBroker } from './browser/BrowserDownloadBroker'
import { registerBrowserDataIpc, type BrowserDataIpcDependencies } from './ipc/browserDataIpc'

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

/**
 * Lets the user pick one project folder. Nothing is persisted here: the renderer collects the
 * picks in the project dialog and submits them through createProject/updateProject.
 */
async function pickProjectFolder(
  event: IpcMainInvokeEvent
): Promise<StorageProjectFolderPick | null> {
  const result = await showOpenDialog(event, {
    title: 'Select project folder',
    properties: ['openDirectory', 'createDirectory']
  })

  const selectedPath = result.filePaths[0]
  if (result.canceled || !selectedPath) {
    return null
  }

  const path = resolve(selectedPath)
  return { path, name: basename(path) || path }
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

/** The primary folder remains the project's working directory for host-side file access. */
async function getProjectPath(coreServer: CoreServer, projectId: string): Promise<string | null> {
  const projects = await coreServer.loadProjects()
  return primaryProjectFolderPath(projects.find((project) => project.id === projectId))
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
  input: ProjectFileReference
): Promise<void> {
  shell.showItemInFolder(await resolveProjectFileReference(coreServer, input))
}

async function loadImageFile(
  coreServer: CoreServer,
  input: { projectId?: string | null; filePath?: string; assistantMessageId?: string }
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

  const filePath = await resolveProjectFileReference(coreServer, {
    ...input,
    filePath: rawFilePath
  })

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

export interface HostIpcRegistration {
  (): void
  beginAutomationShutdown(): Promise<void>
  beginNotificationDelivery(): void
  beginNotificationShutdown(): Promise<void>
  flushRendererBeforeQuit(target: WebContents | null | undefined, timeoutMs?: number): Promise<void>
}

export function registerHostIpc(
  coreServer: CoreServer,
  terminalBridge: TerminalBridge,
  faviconResourceCache: FaviconResourceCache,
  isTrustedRenderer: (event: IpcMainInvokeEvent) => boolean,
  browserSurfaceManager?: BrowserSurfaceManager,
  browserArtifactBroker?: BrowserArtifactBroker,
  notificationLocaleMirror: NotificationLocaleMirror = createVolatileNotificationLocaleMirror(),
  browserDownloadBroker?: BrowserDownloadBroker,
  browserDataIpc?: BrowserDataIpcDependencies,
  appearanceThemeMirror: AppearanceThemeMirror = createVolatileAppearanceThemeMirror(),
  assertCanStartTurn: () => void = () => {
    throw new Error('ACCOUNT_LOGIN_REQUIRED')
  },
  syncExecutionAccess?: () => Promise<void>
): HostIpcRegistration {
  const attachmentDialogBridge = new AttachmentDialogBridge()
  const workspaceFilesService = new WorkspaceFilesService(
    async (projectId) =>
      (await coreServer.loadProjects()).find((project) => project.id === projectId),
    coreServer
  )
  const ipcMain = createTrustedIpcMain(isTrustedRenderer)
  const rendererQuitFlush = new RendererQuitFlushCoordinator(ipcMain)

  registerCoreServiceIpc(ipcMain, coreServer)
  const disposeConfigurationNotifications = registerConfigurationNotifications(coreServer)
  const prepareNewTurn = async (): Promise<void> => {
    assertCanStartTurn()
    await syncExecutionAccess?.()
    assertCanStartTurn()
  }
  registerAgentIpc(ipcMain, coreServer, prepareNewTurn)
  const disposeHumanInteractionIpc = registerHumanInteractionIpc(
    ipcMain,
    coreServer,
    syncExecutionAccess
  )
  const disposeAutomationIpc = registerAutomationIpc(ipcMain, coreServer, prepareNewTurn)
  const disposeNotificationIpc = registerNotificationIpc(ipcMain, coreServer, {
    localeMirror: notificationLocaleMirror,
    startPaused: true
  })
  registerSkillsIpc(ipcMain, coreServer, selectInstallationDirectory)
  const disposeMcpIpc = registerMcpIpc(ipcMain, coreServer)
  registerGitIpc(ipcMain, coreServer)
  registerStorageIpc(ipcMain, coreServer, {
    loadImageFile,
    pickProjectFolder,
    revealProjectFile,
    selectProfileAvatar,
    showProjectInFolder
  })
  registerWorkspaceFilesIpc(ipcMain, workspaceFilesService)
  registerTerminalIpc(ipcMain, terminalBridge, { loadProjects: () => coreServer.loadProjects() })

  ipcMain.handle(HOST_CHANNELS.app.getWindowState, (event) =>
    getAppWindowState(getInvokeWindow(event))
  )
  ipcMain.handle(HOST_CHANNELS.app.openExternal, (_event, url) =>
    browserDataIpc ? browserDataIpc.linkRouter.openAppUrl(url) : openExternalUrl(url)
  )
  ipcMain.handle(HOST_CHANNELS.app.setNativeThemeSource, async (_event, themeSource) => {
    if (!isAppearanceThemePreference(themeSource)) {
      throw new Error('Invalid native theme source')
    }
    nativeTheme.themeSource = themeSource
    await appearanceThemeMirror.setPreference(themeSource)
    applyAdaptiveAppIcon(themeSource)
  })
  ipcMain.handle(HOST_CHANNELS.attachments.selectInputAttachments, (event, request) =>
    attachmentDialogBridge.selectInputAttachments(event, request)
  )
  const disposeBrowserDataIpc = browserDataIpc
    ? registerBrowserDataIpc(ipcMain, browserDataIpc)
    : () => undefined
  if (browserSurfaceManager) {
    registerBrowserSurfaceIpc(ipcMain, browserSurfaceManager)
  }
  if (browserArtifactBroker) {
    registerBrowserArtifactIpc(ipcMain, browserArtifactBroker, selectBrowserArtifactExportPath)
  }
  const disposeBrowserDownloadIpc = browserDownloadBroker
    ? registerBrowserDownloadIpc(ipcMain, coreServer, browserDownloadBroker)
    : () => undefined
  ipcMain.handle(HOST_CHANNELS.resources.resolveFavicon, (_event, input) =>
    faviconResourceCache.resolveFavicon(input)
  )
  const dispose = (): void => {
    rendererQuitFlush.dispose()
    disposeNotificationIpc()
    disposeHumanInteractionIpc()
    disposeConfigurationNotifications()
    disposeAutomationIpc()
    disposeMcpIpc()
    disposeBrowserDataIpc()
    disposeBrowserDownloadIpc()
  }
  dispose.beginNotificationShutdown = (): Promise<void> => disposeNotificationIpc.beginShutdown()
  dispose.beginNotificationDelivery = (): void => disposeNotificationIpc.beginDelivery()
  dispose.flushRendererBeforeQuit = (target, timeoutMs): Promise<void> =>
    rendererQuitFlush.flush(target, timeoutMs)
  // Retain the old lifecycle name while callers migrate; it now fences the application-wide
  // notification pump rather than the removed Automation-only pump.
  dispose.beginAutomationShutdown = dispose.beginNotificationShutdown
  return dispose
}
