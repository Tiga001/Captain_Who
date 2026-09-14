import {
  app,
  BrowserWindow,
  dialog,
  ipcMain,
  Menu,
  nativeImage,
  nativeTheme,
  safeStorage,
  session,
  shell,
  Tray,
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
import { registerHostIpc, type HostIpcRegistration } from './ipc'
import { FaviconResourceCache, registerResourceSchemes } from './resources/FaviconResourceCache'
import { TerminalBridge } from './terminal/TerminalBridge'
import { MainWindowLifecycleController } from './mainWindowLifecycle'
import { BrowserTargetBroker } from './browser/BrowserTargetBroker'
import { BrowserSurfaceManager } from './browser/BrowserSurfaceManager'
import { BrowserNetworkPolicy, ElectronSessionDnsResolver } from './browser/BrowserNetworkPolicy'
import { BrowserRiskCoordinator } from './browser/BrowserRiskCoordinator'
import { BrowserNetworkGuard } from './browser/BrowserNetworkGuard'
import { BrowserInternalPageStore } from './browser/BrowserInternalPageStore'
import { BrowserArtifactBroker } from './browser/BrowserArtifactBroker'
import { BrowserDownloadBroker } from './browser/BrowserDownloadBroker'
import { BrowserHistoryService } from './browser/BrowserHistoryService'
import { BrowserLinkRouter } from './browser/BrowserLinkRouter'
import { BrowserFileBroker } from './browser/BrowserFileBroker'
import { CoreBrowserRiskAuthorizer } from './browser/CoreBrowserRiskAuthorizer'
import { BROWSER_WEBVIEW_PARTITION } from '@mycopilot/protocol'
import {
  createManagedPlaywrightHostFactory,
  ManagedPlaywrightBridgeHost
} from './mcp/ManagedPlaywrightBridgeHost'
import { ManagedPlaywrightSensitiveTargetBindingBroker } from './mcp/ManagedPlaywrightSensitiveTargetBindingBroker'
import { NotificationLocaleStore } from './notifications/notificationLocaleStore'
import {
  AppearanceThemeStore,
  resolveAppearanceColorScheme
} from './appearance/appearanceThemeStore'
import { registerStartupReadiness, type StartupReadinessController } from './startupReadiness'
import { AuthService } from './auth/AuthService'
import { CloudBaseAuthDriver } from './auth/CloudBaseAuthDriver'
import { SessionStore } from './auth/SessionStore'
import { ACCOUNT_SESSION_SCOPE } from './auth/accountConfig'
import { LicenseService } from './auth/LicenseService'
import { fetchAccountLicense } from './auth/LicenseApiClient'
import { registerLicenseIpc } from './auth/licenseIpc'
import { LicenseManagementService } from './auth/LicenseManagementService'
import { ExecutionAccessBridge } from './auth/ExecutionAccessBridge'
import { fetchAccountProfile } from './auth/AccountApiClient'
import { registerAuthIpc } from './auth/authIpc'
import { AppShutdownCoordinator } from './lifecycle/AppShutdownCoordinator'
import { createDesktopUpdateService } from './updates/createDesktopUpdateService'
import { registerUpdateIpc } from './updates/updateIpc'
import {
  DockRecentConversationsController,
  type DockRecentConversationLabels
} from './dockRecentConversations'
import {
  MenuBarRunningConversationsController,
  type MenuBarRunningConversationLabels
} from './menuBarRunningConversations'
import boatMarkPath from '../../resources/brand-mark-light.png?asset'

// Electron is the sole authority for the application data location. Freeze it before
// app.setName() can affect path resolution so the entire process uses one root.
const requestedAppDataRoot = resolve(app.getPath('userData'))
mkdirSync(requestedAppDataRoot, { recursive: true, mode: 0o700 })
const appDataRoot = realpathSync(requestedAppDataRoot)
app.setPath('userData', appDataRoot)

registerResourceSchemes()

const coreServer = new CoreServer({ appDataRoot })
const notificationLocaleStore = new NotificationLocaleStore(appDataRoot)
const appearanceThemeStore = new AppearanceThemeStore(appDataRoot)
nativeTheme.themeSource = appearanceThemeStore.getPreference()
const terminalBridge = new TerminalBridge()
let disposeAdaptiveAppIcon: (() => void) | null = null
let disposeHostIpc: HostIpcRegistration | null = null
let disposeUpdateIpc: (() => void) | null = null
let desktopUpdateService: ReturnType<typeof createDesktopUpdateService> | null = null
let mainWindow: BrowserWindow | null = null
let dockRecentConversations: DockRecentConversationsController | null = null
let menuBarTray: Tray | null = null
let pendingDockConversationId: string | null = null
let startupReadiness: StartupReadinessController | null = null
let hostInitializationReady = false
let accountAuth: AuthService | null = null
let startupInitializationFailed = false
let browserSurfaceManager: BrowserSurfaceManager | null = null
let browserNetworkGuard: BrowserNetworkGuard | null = null
let browserArtifactBroker: BrowserArtifactBroker | null = null
let browserDownloadBroker: BrowserDownloadBroker | null = null
let browserHistoryService: BrowserHistoryService | null = null
let browserLinkRouter: BrowserLinkRouter | null = null
let browserFileBroker: BrowserFileBroker | null = null
let managedPlaywrightBridgeHost: ManagedPlaywrightBridgeHost | null = null
const mainWindowLifecycle = new MainWindowLifecycleController(process.platform)
const trustedRendererEntries = new Map<number, string>()
const shutdownCoordinator = new AppShutdownCoordinator({
  shutdownServices: shutdownApplicationServices,
  quit: () => app.quit(),
  relaunch: () => app.relaunch(),
  onError: (message, error) => console.error(message, error)
})

const macWindowChromeOptions =
  process.platform === 'darwin'
    ? {
        backgroundColor: '#00000000',
        titleBarStyle: 'hidden' as const,
        trafficLightPosition: { x: 18, y: 12 },
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

function configureMainWindowWebviews(window: BrowserWindow): void {
  if (!browserSurfaceManager) return
  configureManagedWebviewHost(window.webContents, {
    targetRegistry: browserSurfaceManager,
    ...(browserNetworkGuard ? { networkGuard: browserNetworkGuard } : {})
  })
}

function dockRecentConversationLabels(): DockRecentConversationLabels {
  if (app.getLocale().toLowerCase().startsWith('zh')) {
    return {
      empty: '暂无最近聊天',
      header: '最近聊天',
      more: '更多',
      untitled: '未命名聊天'
    }
  }
  return {
    empty: 'No recent chats',
    header: 'Recent Chats',
    more: 'More',
    untitled: 'Untitled chat'
  }
}

function menuBarRunningConversationLabels(): MenuBarRunningConversationLabels {
  if (app.getLocale().toLowerCase().startsWith('zh')) {
    return {
      appName: 'Captain Who',
      empty: '没有正在运行的聊天',
      quit: '退出 Captain Who',
      running: '正在运行',
      untitled: '未命名聊天'
    }
  }
  return {
    appName: 'Captain Who',
    empty: 'No chats are running',
    quit: 'Quit Captain Who',
    running: 'Running',
    untitled: 'Untitled chat'
  }
}

function openDockConversation(conversationId: string): void {
  pendingDockConversationId = conversationId
  activateMainWindow()
  const window = mainWindow
  if (!window || window.isDestroyed()) return
  window.webContents.send(HOST_CHANNELS.app.dockOpenConversationPending)
}

function installDockRecentConversations(): void {
  const dock = process.platform === 'darwin' ? app.dock : undefined
  if (!dock) return
  dockRecentConversations?.dispose()
  dockRecentConversations = new DockRecentConversationsController({
    applyMenu: (template) => dock.setMenu(Menu.buildFromTemplate(template)),
    labels: dockRecentConversationLabels(),
    loadConversationMetas: () => coreServer.loadConversationMetas(),
    openConversation: openDockConversation
  })
  dockRecentConversations.start()
}

function installMenuBarRunningConversations(): void {
  if (process.platform !== 'darwin') return

  const sourceIcon = nativeImage.createFromPath(boatMarkPath)
  if (sourceIcon.isEmpty()) {
    console.error('Failed to load the Captain Who menu-bar icon')
    return
  }
  // The brand mark is designed for large surfaces and contains generous transparent padding.
  // Crop that padding before the status-bar resize so its boat reads at the same visual weight as
  // neighbouring menu-bar icons.
  const icon = sourceIcon
    .crop({ height: 392, width: 416, x: 48, y: 62 })
    .resize({ height: 20, width: 20 })
  icon.setTemplateImage(false)
  menuBarTray = new Tray(icon)
  menuBarTray.setToolTip('Captain Who')
  const controller = new MenuBarRunningConversationsController({
    applyMenu: (template) => menuBarTray?.setContextMenu(Menu.buildFromTemplate(template)),
    labels: menuBarRunningConversationLabels(),
    loadConversations: () => coreServer.loadConversations(),
    openConversation: openDockConversation,
    quit: () => app.quit()
  })
  controller.start()

  // The controller coalesces bursts from the root Agent's streaming events before reading storage.
  const stopAgentEvents = coreServer.onAgentEvent(() => controller.refresh())
  app.once('will-quit', () => {
    stopAgentEvents()
    controller.dispose()
    menuBarTray?.destroy()
    menuBarTray = null
  })
}

function createWindow(): void {
  const rendererEntryUrl = getRendererEntryUrl()
  const window = new BrowserWindow({
    title: 'Captain Who',
    width: 1120,
    height: 760,
    minWidth: 920,
    minHeight: 640,
    show: false,
    autoHideMenuBar: true,
    // Non-macOS windows do not have native vibrancy. Their opaque native surface is the
    // compatibility fallback beneath the shared translucent startup CSS.
    backgroundColor:
      resolveAppearanceColorScheme(
        appearanceThemeStore.getPreference(),
        nativeTheme.shouldUseDarkColors
      ) === 'dark'
        ? '#171717'
        : '#f4f4f2',
    ...macWindowChromeOptions,
    ...(process.platform !== 'darwin'
      ? { icon: getAdaptiveAppIcon(appearanceThemeStore.getPreference()) }
      : {}),
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
  configureMainWindowWebviews(window)
  installNativeImageContextMenu(window)
  const handleWindowStateChange = (): void => sendAppWindowState(window)

  rendererWebContents.once('destroyed', () => {
    trustedRendererEntries.delete(rendererWebContentsId)
  })
  window.on('close', (event) => {
    if (startupInitializationFailed && !shutdownCoordinator.isQuittingAfterServiceShutdown) {
      event.preventDefault()
      app.quit()
      return
    }
    // Keep the live Renderer available long enough for before-quit to drain debounced state on
    // platforms where closing the last window would otherwise destroy it before app.quit().
    if (process.platform !== 'darwin' && !shutdownCoordinator.isQuittingAfterServiceShutdown) {
      event.preventDefault()
      app.quit()
      return
    }
    if (
      mainWindowLifecycle.requestClose(window, shutdownCoordinator.isQuittingAfterServiceShutdown)
    ) {
      event.preventDefault()
    }
  })
  window.once('closed', () => {
    trustedRendererEntries.delete(rendererWebContentsId)
    if (mainWindow === window) mainWindow = null
  })

  window.on('ready-to-show', () => {
    if (
      shutdownCoordinator.isServiceShutdownInProgress ||
      shutdownCoordinator.isQuittingAfterServiceShutdown
    ) {
      return
    }
    window.show()
    sendAppWindowState(window)
    // A cold-start backlog must not race the window that the user just opened. Native delivery
    // begins only after the first app window is visible; the final focus check still runs directly
    // before every OS notification.
    if (hostInitializationReady) disposeHostIpc?.beginNotificationDelivery()
  })

  rendererWebContents.on('did-finish-load', handleWindowStateChange)
  window.on('maximize', handleWindowStateChange)
  window.on('unmaximize', handleWindowStateChange)
  window.on('enter-full-screen', handleWindowStateChange)
  window.on('leave-full-screen', () => {
    handleWindowStateChange()
    mainWindowLifecycle.handleLeaveFullScreen(
      window,
      shutdownCoordinator.isQuittingAfterServiceShutdown
    )
  })
  window.on('restore', handleWindowStateChange)

  rendererWebContents.setWindowOpenHandler((details) => {
    const linkRouter = browserLinkRouter
    if (linkRouter) {
      void linkRouter.openAppUrl(details.url).catch((error: unknown) => {
        console.error('Failed to open app URL', error)
      })
    }
    return { action: 'deny' }
  })

  rendererWebContents.on('will-navigate', (event, url) => {
    if (isAllowedRendererUrl(url, rendererEntryUrl)) return

    event.preventDefault()
    const linkRouter = browserLinkRouter
    if (linkRouter) {
      void linkRouter.openAppUrl(url).catch((error: unknown) => {
        console.error('Blocked main-window navigation', error)
      })
    }
  })

  void window.loadURL(rendererEntryUrl).catch(() => {
    if (!window.isDestroyed()) {
      console.error('Failed to load Renderer entry (safe error)')
    }
  })
}

function activateMainWindow(): void {
  if (
    shutdownCoordinator.isServiceShutdownInProgress ||
    shutdownCoordinator.isQuittingAfterServiceShutdown
  ) {
    return
  }
  const window = mainWindowLifecycle.showExisting(mainWindow)
  if (!window) {
    createWindow()
    return
  }

  sendAppWindowState(window)
  if (hostInitializationReady) disposeHostIpc?.beginNotificationDelivery()
}

async function initializeApplication(): Promise<void> {
  accountAuth = new AuthService(
    new CloudBaseAuthDriver(),
    new SessionStore(appDataRoot, safeStorage, ACCOUNT_SESSION_SCOPE),
    fetchAccountProfile
  )
  const disposeAuthIpc = registerAuthIpc(accountAuth, isTrustedRendererEvent)
  app.once('will-quit', disposeAuthIpc)
  const accountLicense = new LicenseService(accountAuth, fetchAccountLicense)
  app.once(
    'will-quit',
    registerLicenseIpc(
      accountLicense,
      isTrustedRendererEvent,
      new LicenseManagementService(accountAuth, accountLicense, (url) => shell.openExternal(url))
    )
  )
  const executionAccess = new ExecutionAccessBridge(accountAuth, accountLicense, coreServer)
  app.once('will-quit', () => executionAccess.dispose())
  // Account validation runs alongside existing service initialization, never owns its lifecycle.
  void accountAuth.restoreSession()
  app.setName('Captain Who')
  app.setAboutPanelOptions({
    credits: '开发者邮箱：sh_jiao@zju.edu.cn\nQQ 交流群：1125049092'
  })
  electronApp.setAppUserModelId('io.github.tiga001.captainwho')
  nativeTheme.themeSource = appearanceThemeStore.getPreference()
  disposeAdaptiveAppIcon = installAdaptiveAppIcon(() => appearanceThemeStore.getPreference())
  app.on('browser-window-created', (_, window) => {
    optimizer.watchWindowShortcuts(window)
  })
  startupReadiness = registerStartupReadiness(ipcMain, isTrustedRendererEvent)
  desktopUpdateService = createDesktopUpdateService({
    requestInstall: (install) =>
      hostInitializationReady && shutdownCoordinator.requestUpdateInstall(install),
    onInstallFailure: (error) => {
      shutdownCoordinator.recoverFromUpdateInstallFailure(error)
    },
    isShuttingDown: () =>
      shutdownCoordinator.isServiceShutdownInProgress ||
      shutdownCoordinator.isQuittingAfterServiceShutdown
  })
  disposeUpdateIpc = registerUpdateIpc(desktopUpdateService, isTrustedRendererEvent)
  createWindow()
  app.on('activate', activateMainWindow)
  coreServer.start()
  installDockRecentConversations()
  installMenuBarRunningConversations()
  app.once('will-quit', () => {
    dockRecentConversations?.dispose()
    dockRecentConversations = null
  })
  const managedBrowserSession = session.fromPartition(BROWSER_WEBVIEW_PARTITION)
  const faviconResourceCache = new FaviconResourceCache({
    networkSession: managedBrowserSession
  })
  faviconResourceCache.registerProtocol()
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
  const browserPreferences = await coreServer.loadBrowserPreferences()
  const browserDownloadSettings = await coreServer.loadBrowserDownloadSettings()
  browserDownloadBroker = new BrowserDownloadBroker({
    expectedSession: managedBrowserSession,
    initialSettings: browserDownloadSettings,
    registerDownload: (input) => coreServer.registerBrowserDownload(input),
    systemDownloadDirectory: app.getPath('downloads')
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
  const browserInternalPageStore = new BrowserInternalPageStore(managedBrowserSession)
  browserInternalPageStore.install()
  browserHistoryService = new BrowserHistoryService(coreServer)
  browserSurfaceManager = new BrowserSurfaceManager({
    broker: new BrowserTargetBroker(BROWSER_WEBVIEW_PARTITION, managedBrowserSession),
    internalPageStore: browserInternalPageStore,
    resolveHost: () => mainWindow?.webContents ?? null,
    sendCommand: (host, command) => {
      if (!host.isDestroyed()) host.send(HOST_CHANNELS.browser.surfaceCommand, command)
    },
    sendState: (host, state) => {
      if (!host.isDestroyed()) host.send(HOST_CHANNELS.browser.surfaceStateChanged, state)
    },
    getLocale: () => notificationLocaleStore.getLocale(),
    onHistoryMetadata: (event) => browserHistoryService?.updateMetadata(event),
    onHistoryNavigation: (event) => browserHistoryService?.recordNavigation(event),
    networkGuard: browserNetworkGuard,
    releaseSurfaceResources: (input) => {
      browserHistoryService?.forgetSurface(input)
      return browserFileBroker?.releaseSurface(input) ?? Promise.resolve()
    }
  })
  if (mainWindow && !mainWindow.isDestroyed()) configureMainWindowWebviews(mainWindow)
  browserLinkRouter = new BrowserLinkRouter({
    coreServer,
    initialPreferences: browserPreferences,
    surfaceManager: browserSurfaceManager
  })
  const sensitiveTargetBindings = new ManagedPlaywrightSensitiveTargetBindingBroker({
    beginDispatchFence: (target) => {
      const manager = browserSurfaceManager
      if (!manager) throw new Error('browser.surface_unavailable')
      return manager.beginSensitiveDispatchFence(target)
    },
    getActiveTarget: (owner) => browserSurfaceManager?.getSensitiveTargetIdentity(owner) ?? null,
    releasePreparedFiles: ({ runId, callId }) => {
      void browserFileBroker?.releaseToolCall({ runId, toolCallId: callId })
    }
  })
  managedPlaywrightBridgeHost = new ManagedPlaywrightBridgeHost({
    core: coreServer,
    releaseRunTarget: (runId) => browserSurfaceManager?.releaseRunTarget(runId),
    fileBroker: browserFileBroker,
    sensitiveTargetBindings,
    createHost: createManagedPlaywrightHostFactory({
      getBrowserContext: () =>
        getBrowserSurfaceManager().getBrowserContext({ createVisiblePage: false }),
      getAgentDownloadSnapshot: (input) => {
        const guard = browserNetworkGuard
        if (!guard) throw new Error('browser.surface_unavailable')
        return guard.agentDownloadSnapshot(input)
      },
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

  disposeHostIpc = registerHostIpc(
    coreServer,
    terminalBridge,
    faviconResourceCache,
    isTrustedRendererEvent,
    browserSurfaceManager,
    browserArtifactBroker,
    notificationLocaleStore,
    browserDownloadBroker,
    {
      coreServer,
      faviconResourceCache,
      historyService: browserHistoryService,
      linkRouter: browserLinkRouter,
      session: managedBrowserSession
    },
    appearanceThemeStore,
    () => {
      if (!accountAuth) throw new Error('ACCOUNT_LOGIN_REQUIRED')
      accountAuth.assertCanStartTurn()
      accountLicense.assertCanStartTurn()
    },
    () => {
      coreServer.start()
      return executionAccess.sync()
    }
  )
  ipcMain.handle(HOST_CHANNELS.app.takeDockOpenConversation, (event) => {
    if (!isTrustedRendererEvent(event)) return null
    const sourceWindow = BrowserWindow.fromWebContents(event.sender)
    if (!sourceWindow || sourceWindow !== mainWindow) return null
    const conversationId = pendingDockConversationId
    pendingDockConversationId = null
    return conversationId
  })
  app.once('will-quit', () => ipcMain.removeHandler(HOST_CHANNELS.app.takeDockOpenConversation))
  if (mainWindow?.isVisible()) disposeHostIpc.beginNotificationDelivery()
  hostInitializationReady = true
  startupReadiness.markReady()
  if (
    !shutdownCoordinator.isServiceShutdownInProgress &&
    !shutdownCoordinator.isQuittingAfterServiceShutdown
  ) {
    desktopUpdateService.startOnce()
  }
}

void app
  .whenReady()
  .then(initializeApplication)
  .catch((error: unknown) => {
    console.error('Failed to initialize application', error)
    startupInitializationFailed = true
    startupReadiness?.markFailed()
    if (!mainWindow || mainWindow.isDestroyed()) app.exit(1)
  })

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') {
    app.quit()
  }
})

app.on('before-quit', (event) => {
  if (shutdownCoordinator.isQuittingAfterServiceShutdown) {
    terminalBridge.killNow()
    coreServer.stop()
    return
  }
  event.preventDefault()
  shutdownCoordinator.requestQuit()
})

async function shutdownApplicationServices(): Promise<void> {
  // Stop the native system-notification producer before any await in the shutdown path. Keep
  // the remaining IPC and MCP reverse bridge registered until Core has completed its own bounded
  // shutdown; only this producer could otherwise issue a lazy request that respawns Core.
  const notificationShutdown = disposeHostIpc?.beginNotificationShutdown()
  desktopUpdateService?.beginShutdown()
  mainWindowLifecycle.prepareForQuit()
  // Once quit has been accepted, remove the remaining input surface while retaining its live
  // webContents for the bounded Renderer flush below.
  mainWindow?.hide()
  // Renderer owns debounced Composer state. Finish that bounded write while Core still accepts
  // requests; unloading the window after Core shutdown would otherwise lose the final keystrokes.
  await disposeHostIpc?.flushRendererBeforeQuit(mainWindow?.webContents)
  // Keep the exact Main reverse bridge and BrowserSurface alive until Core has stopped the
  // managed MCP Manager. Core shutdown sends a reviewed close command and awaits its bounded
  // completion; tearing down Main in parallel would turn a graceful close into an unknown
  // outcome and could strand an attachment.
  await Promise.allSettled([
    notificationShutdown ?? Promise.resolve(),
    appearanceThemeStore.beginShutdown(),
    terminalBridge.stop(),
    coreServer.shutdown()
  ])
  await managedPlaywrightBridgeHost?.close()
  managedPlaywrightBridgeHost = null
  await browserSurfaceManager?.shutdown().catch(() => undefined)
  browserSurfaceManager = null
  browserNetworkGuard = null
  browserDownloadBroker = null
  browserHistoryService = null
  browserLinkRouter = null
  await browserFileBroker?.shutdown().catch(() => undefined)
  browserFileBroker = null
  await browserArtifactBroker?.shutdown().catch(() => undefined)
  browserArtifactBroker = null
}

app.on('will-quit', () => {
  startupReadiness?.dispose()
  startupReadiness = null
  disposeUpdateIpc?.()
  disposeUpdateIpc = null
  desktopUpdateService?.dispose()
  desktopUpdateService = null
  disposeHostIpc?.()
  disposeHostIpc = null
  disposeAdaptiveAppIcon?.()
  disposeAdaptiveAppIcon = null
  terminalBridge.killNow()
  managedPlaywrightBridgeHost = null
  browserDownloadBroker = null
  browserHistoryService = null
  browserLinkRouter = null
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
