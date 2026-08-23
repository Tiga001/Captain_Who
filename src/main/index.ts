import {
  app,
  BrowserWindow,
  dialog,
  Menu,
  nativeTheme,
  session,
  type IpcMainInvokeEvent,
  type OpenDialogOptions
} from 'electron'
import { mkdirSync, realpathSync } from 'node:fs'
import { join, resolve } from 'path'
import { pathToFileURL } from 'url'
import { electronApp, optimizer, is } from '@electron-toolkit/utils'
import { HOST_CHANNELS, type AppWindowState } from '@mycopilot/host-api'
import { getAdaptiveAppIcon, installAdaptiveAppIcon } from './appIcon'
import {
  configureManagedWebviewHost,
  initializeManagedWebviewSessions
} from './webviews/managedWebviewSecurity'
import { CoreServer } from './core/coreServer'
import { openExternalUrl, registerHostIpc, type HostIpcRegistration } from './ipc'
import { FaviconResourceCache, registerResourceSchemes } from './resources/FaviconResourceCache'
import { TerminalBridge } from './terminal/TerminalBridge'
import { MainWindowLifecycleController } from './mainWindowLifecycle'
import { BrowserTargetBroker } from './browser/BrowserTargetBroker'
import { BrowserSurfaceManager } from './browser/BrowserSurfaceManager'
import { BrowserNetworkPolicy, ElectronSessionDnsResolver } from './browser/BrowserNetworkPolicy'
import { BrowserRiskCoordinator } from './browser/BrowserRiskCoordinator'
import { BrowserNetworkGuard } from './browser/BrowserNetworkGuard'
import { BrowserArtifactBroker } from './browser/BrowserArtifactBroker'
import { BrowserDownloadBroker } from './browser/BrowserDownloadBroker'
import { BrowserFileBroker } from './browser/BrowserFileBroker'
import { CoreBrowserRiskAuthorizer } from './browser/CoreBrowserRiskAuthorizer'
import { BROWSER_WEBVIEW_PARTITION } from '@mycopilot/protocol'
import {
  createManagedPlaywrightHostFactory,
  ManagedPlaywrightBridgeHost
} from './mcp/ManagedPlaywrightBridgeHost'
import { ManagedPlaywrightSensitiveTargetBindingBroker } from './mcp/ManagedPlaywrightSensitiveTargetBindingBroker'

// Electron is the sole authority for the application data location. Freeze it before
// app.setName() can affect path resolution so the entire process uses one root.
const requestedAppDataRoot = resolve(app.getPath('userData'))
mkdirSync(requestedAppDataRoot, { recursive: true, mode: 0o700 })
const appDataRoot = realpathSync(requestedAppDataRoot)
app.setPath('userData', appDataRoot)

registerResourceSchemes()

const coreServer = new CoreServer({ appDataRoot })
const terminalBridge = new TerminalBridge()
const faviconResourceCache = new FaviconResourceCache()
let isQuittingAfterServiceShutdown = false
let disposeAdaptiveAppIcon: (() => void) | null = null
let disposeHostIpc: HostIpcRegistration | null = null
let mainWindow: BrowserWindow | null = null
let browserSurfaceManager: BrowserSurfaceManager | null = null
let browserNetworkGuard: BrowserNetworkGuard | null = null
let browserArtifactBroker: BrowserArtifactBroker | null = null
let browserDownloadBroker: BrowserDownloadBroker | null = null
let browserFileBroker: BrowserFileBroker | null = null
let managedPlaywrightBridgeHost: ManagedPlaywrightBridgeHost | null = null
const mainWindowLifecycle = new MainWindowLifecycleController(process.platform)
const trustedRendererEntries = new Map<number, string>()

const macWindowChromeOptions =
  process.platform === 'darwin'
    ? {
        backgroundColor: '#00000000',
        titleBarStyle: 'hidden' as const,
        trafficLightPosition: { x: 18, y: 18 },
        transparent: true,
        vibrancy: 'sidebar' as const,
        visualEffectState: 'active' as const
      }
    : {}

function getAppWindowState(window: BrowserWindow): AppWindowState {
  return {
    isFullScreen: window.isFullScreen(),
    isMaximized: window.isMaximized()
  }
}

function sendAppWindowState(window: BrowserWindow): void {
  if (window.isDestroyed() || window.webContents.isDestroyed()) {
    return
  }
  window.webContents.send(HOST_CHANNELS.app.windowStateChange, getAppWindowState(window))
}

function installNativeImageContextMenu(window: BrowserWindow): void {
  const contents = window.webContents
  const copyImageLabel = app.getLocale().toLowerCase().startsWith('zh') ? '复制图片' : 'Copy Image'

  contents.on('context-menu', (_event, params) => {
    if (params.mediaType !== 'image') return

    // Match Codex's Electron-native path: Chromium already knows which rendered image
    // was hit, so copy it directly instead of downloading and decoding its src again.
    const menu = Menu.buildFromTemplate([
      {
        label: copyImageLabel,
        click: () => {
          if (!contents.isDestroyed()) {
            contents.copyImageAt(params.x, params.y)
          }
        }
      }
    ])
    menu.popup({ window })
  })
}

function isAllowedRendererUrl(candidateValue: string, entryValue: string): boolean {
  try {
    const candidate = new URL(candidateValue)
    const entry = new URL(entryValue)
    if (entry.protocol === 'file:') {
      return candidate.protocol === 'file:' && candidate.pathname === entry.pathname
    }
    return candidate.origin === entry.origin
  } catch {
    return false
  }
}

function isTrustedRendererEvent(event: IpcMainInvokeEvent): boolean {
  const entryUrl = trustedRendererEntries.get(event.sender.id)
  return Boolean(
    entryUrl &&
    event.senderFrame === event.sender.mainFrame &&
    isAllowedRendererUrl(event.senderFrame.url, entryUrl)
  )
}

function createWindow(): void {
  if (!browserSurfaceManager) throw new Error('Browser surface manager is not initialized')
  const rendererEntryUrl = getRendererEntryUrl()
  const window = new BrowserWindow({
    title: 'MyCopilot',
    width: 1120,
    height: 760,
    minWidth: 920,
    minHeight: 640,
    show: false,
    autoHideMenuBar: true,
    // Non-macOS windows do not have native vibrancy. Their opaque native surface is the
    // compatibility fallback beneath the shared translucent startup CSS.
    backgroundColor: nativeTheme.shouldUseDarkColors ? '#171717' : '#f4f4f2',
    ...macWindowChromeOptions,
    ...(process.platform !== 'darwin' ? { icon: getAdaptiveAppIcon() } : {}),
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      webSecurity: true,
      webviewTag: true
    }
  })
  mainWindow = window

  const rendererWebContents = window.webContents
  const rendererWebContentsId = rendererWebContents.id
  trustedRendererEntries.set(rendererWebContentsId, rendererEntryUrl)
  configureManagedWebviewHost(rendererWebContents, {
    targetRegistry: browserSurfaceManager,
    ...(browserNetworkGuard ? { networkGuard: browserNetworkGuard } : {})
  })
  installNativeImageContextMenu(window)
  const handleWindowStateChange = (): void => sendAppWindowState(window)

  rendererWebContents.once('destroyed', () => {
    trustedRendererEntries.delete(rendererWebContentsId)
  })
  window.on('close', (event) => {
    if (mainWindowLifecycle.requestClose(window, isQuittingAfterServiceShutdown)) {
      event.preventDefault()
    }
  })
  window.once('closed', () => {
    trustedRendererEntries.delete(rendererWebContentsId)
    if (mainWindow === window) mainWindow = null
  })

  window.on('ready-to-show', () => {
    window.show()
    sendAppWindowState(window)
  })

  rendererWebContents.on('did-finish-load', handleWindowStateChange)
  window.on('maximize', handleWindowStateChange)
  window.on('unmaximize', handleWindowStateChange)
  window.on('enter-full-screen', handleWindowStateChange)
  window.on('leave-full-screen', () => {
    handleWindowStateChange()
    mainWindowLifecycle.handleLeaveFullScreen(window, isQuittingAfterServiceShutdown)
  })
  window.on('restore', handleWindowStateChange)

  rendererWebContents.setWindowOpenHandler((details) => {
    void openExternalUrl(details.url).catch((error: unknown) => {
      console.error('Failed to open external URL', error)
    })
    return { action: 'deny' }
  })

  rendererWebContents.on('will-navigate', (event, url) => {
    if (isAllowedRendererUrl(url, rendererEntryUrl)) return

    event.preventDefault()
    void openExternalUrl(url).catch((error: unknown) => {
      console.error('Blocked main-window navigation', error)
    })
  })

  void window.loadURL(rendererEntryUrl).catch(() => {
    if (!window.isDestroyed()) {
      console.error('Failed to load Renderer entry (safe error)')
    }
  })
}

function activateMainWindow(): void {
  const window = mainWindowLifecycle.showExisting(mainWindow)
  if (!window) {
    createWindow()
    return
  }

  sendAppWindowState(window)
}

app.whenReady().then(async () => {
  app.setName('MyCopilot')
  electronApp.setAppUserModelId('com.mycopilot.next')
  disposeAdaptiveAppIcon = installAdaptiveAppIcon()
  coreServer.start()
  faviconResourceCache.registerProtocol()
  const managedBrowserSession = session.fromPartition(BROWSER_WEBVIEW_PARTITION)
  browserArtifactBroker = new BrowserArtifactBroker({
    rootDirectory: join(appDataRoot, 'browser-automation-artifacts')
  })
  browserFileBroker = new BrowserFileBroker({
    rootDirectory: join(appDataRoot, 'browser-automation-files'),
    selectionProvider: {
      selectFiles: async ({ multiple }) => {
        const options: OpenDialogOptions = {
          title: app.getLocale().toLowerCase().startsWith('zh')
            ? '选择要交给浏览器自动化使用的文件'
            : 'Select files for browser automation',
          properties: multiple ? ['openFile', 'multiSelections'] : ['openFile']
        }
        const result = mainWindow
          ? await dialog.showOpenDialog(mainWindow, options)
          : await dialog.showOpenDialog(options)
        return result.canceled ? null : result.filePaths
      }
    }
  })
  await browserFileBroker.initialize()
  browserDownloadBroker = new BrowserDownloadBroker({
    artifacts: browserArtifactBroker,
    expectedSession: managedBrowserSession
  })
  const browserNetworkPolicy = new BrowserNetworkPolicy({
    blockedOrigins: [getRendererEntryUrl()],
    dnsResolver: new ElectronSessionDnsResolver(managedBrowserSession)
  })
  browserNetworkGuard = new BrowserNetworkGuard({
    accessPolicy: 'host_boundaries_only',
    coordinator: new BrowserRiskCoordinator({
      authorizer: new CoreBrowserRiskAuthorizer(coreServer),
      policy: browserNetworkPolicy
    }),
    downloadBroker: browserDownloadBroker,
    expectedSession: managedBrowserSession,
    policy: browserNetworkPolicy
  })
  initializeManagedWebviewSessions({ networkGuard: browserNetworkGuard })
  browserSurfaceManager = new BrowserSurfaceManager({
    broker: new BrowserTargetBroker(BROWSER_WEBVIEW_PARTITION, managedBrowserSession),
    resolveHost: () => mainWindow?.webContents ?? null,
    sendCommand: (host, command) => {
      if (!host.isDestroyed()) host.send(HOST_CHANNELS.browser.surfaceCommand, command)
    },
    networkGuard: browserNetworkGuard,
    releaseSurfaceResources: (input) =>
      browserFileBroker?.releaseSurface(input) ?? Promise.resolve()
  })
  const sensitiveTargetBindings = new ManagedPlaywrightSensitiveTargetBindingBroker({
    beginDispatchFence: (target) => {
      const manager = browserSurfaceManager
      if (!manager) throw new Error('browser.surface_unavailable')
      return manager.beginSensitiveDispatchFence(target)
    },
    getActiveTarget: () => browserSurfaceManager?.getSensitiveTargetIdentity() ?? null,
    releasePreparedFiles: ({ runId, callId }) => {
      void browserFileBroker?.releaseToolCall({ runId, toolCallId: callId })
    }
  })
  managedPlaywrightBridgeHost = new ManagedPlaywrightBridgeHost({
    core: coreServer,
    fileBroker: browserFileBroker,
    sensitiveTargetBindings,
    createHost: createManagedPlaywrightHostFactory({
      getBrowserContext: () =>
        getBrowserSurfaceManager().getBrowserContext({ createVisiblePage: false }),
      getActiveSurfaceIdentity: () => getBrowserSurfaceManager().getActiveSurfaceIdentity(),
      sensitiveTargetBindings,
      artifactBroker: browserArtifactBroker,
      fileBroker: browserFileBroker,
      finalizeBrowserRun: async (runId) => {
        await Promise.all([
          browserNetworkGuard?.finalizeRun(runId) ?? Promise.resolve(),
          browserFileBroker?.releaseRun(runId) ?? Promise.resolve()
        ])
      },
      releaseBrowserCapability: (activationId) =>
        Promise.all([
          browserNetworkGuard?.releaseCapability(activationId) ?? Promise.resolve(),
          browserFileBroker?.releaseCapability(activationId) ?? Promise.resolve()
        ]).then(() => undefined),
      releaseBrowserToolCall: (input) =>
        Promise.all([
          browserNetworkGuard?.releaseToolCall(input) ?? Promise.resolve(),
          browserFileBroker?.releaseToolCall(input) ?? Promise.resolve()
        ]).then(() => undefined),
      surfaceGroup: browserSurfaceManager,
      closeSurface: () => getBrowserSurfaceManager().closeSurface(),
      detachAutomation: () => getBrowserSurfaceManager().detachAutomation(),
      beginNetworkOperation: (input) => getBrowserSurfaceManager().beginNetworkOperation(input),
      beginTargetCreationOperation: async (input) => {
        const guard = browserNetworkGuard
        if (!guard) throw new Error('browser.surface_unavailable')
        const lease = guard.beginTargetCreationOperation(input)
        await lease.ready()
        return lease
      }
    })
  })

  app.on('browser-window-created', (_, window) => {
    optimizer.watchWindowShortcuts(window)
  })

  disposeHostIpc = registerHostIpc(
    coreServer,
    terminalBridge,
    faviconResourceCache,
    isTrustedRendererEvent,
    browserSurfaceManager,
    browserArtifactBroker
  )

  createWindow()

  app.on('activate', activateMainWindow)
})

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') {
    app.quit()
  }
})

app.on('before-quit', (event) => {
  if (isQuittingAfterServiceShutdown) {
    terminalBridge.killNow()
    coreServer.stop()
    return
  }

  event.preventDefault()
  isQuittingAfterServiceShutdown = true
  // Stop the native Automation notification producer before any await in the shutdown path. Keep
  // the remaining IPC and MCP reverse bridge registered until Core has completed its own bounded
  // shutdown; only this producer could otherwise issue a lazy request that respawns Core.
  disposeHostIpc?.beginAutomationShutdown()
  mainWindowLifecycle.prepareForQuit()
  void (async () => {
    // Keep the exact Main reverse bridge and BrowserSurface alive until Core has stopped the
    // managed MCP Manager. Core shutdown sends a reviewed close command and awaits its bounded
    // completion; tearing down Main in parallel would turn a graceful close into an unknown
    // outcome and could strand an attachment.
    await Promise.allSettled([terminalBridge.stop(), coreServer.shutdown()])
    await managedPlaywrightBridgeHost?.close()
    managedPlaywrightBridgeHost = null
    await browserSurfaceManager?.shutdown().catch(() => undefined)
    browserSurfaceManager = null
    browserNetworkGuard = null
    browserDownloadBroker = null
    await browserFileBroker?.shutdown().catch(() => undefined)
    browserFileBroker = null
    await browserArtifactBroker?.shutdown().catch(() => undefined)
    browserArtifactBroker = null
  })().finally(() => app.quit())
})

app.on('will-quit', () => {
  disposeHostIpc?.()
  disposeHostIpc = null
  disposeAdaptiveAppIcon?.()
  disposeAdaptiveAppIcon = null
  terminalBridge.killNow()
  managedPlaywrightBridgeHost = null
  browserDownloadBroker = null
  browserFileBroker = null
  browserNetworkGuard = null
  browserSurfaceManager = null
  browserArtifactBroker = null
  coreServer.stop()
})

/** Main-only composition hook for the managed Playwright MCP host. */
export function getBrowserSurfaceManager(): BrowserSurfaceManager {
  if (!browserSurfaceManager) throw new Error('Browser surface manager is not initialized')
  return browserSurfaceManager
}

function getRendererEntryUrl(): string {
  return is.dev && process.env['ELECTRON_RENDERER_URL']
    ? new URL(process.env['ELECTRON_RENDERER_URL']).toString()
    : pathToFileURL(join(__dirname, '../renderer/index.html')).toString()
}
