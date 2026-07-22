import { app, BrowserWindow, Menu, type IpcMainInvokeEvent } from 'electron'
import { join } from 'path'
import { pathToFileURL } from 'url'
import { electronApp, optimizer, is } from '@electron-toolkit/utils'
import type { AppWindowState } from '@mycopilot/host-api'
import { getAdaptiveAppIcon, installAdaptiveAppIcon } from './appIcon'
import {
  configureManagedWebviewHost,
  initializeManagedWebviewSessions
} from './webviews/managedWebviewSecurity'
import { CoreServer } from './core/coreServer'
import { openExternalUrl, registerHostIpc } from './ipc'
import { FaviconResourceCache, registerResourceSchemes } from './resources/FaviconResourceCache'
import { TerminalBridge } from './terminal/TerminalBridge'

registerResourceSchemes()

const coreServer = new CoreServer()
const terminalBridge = new TerminalBridge()
const faviconResourceCache = new FaviconResourceCache()
let isQuittingAfterServiceShutdown = false
let disposeAdaptiveAppIcon: (() => void) | null = null
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

const APP_WINDOW_STATE_CHANNEL = 'host:app.windowStateChange'

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
  window.webContents.send(APP_WINDOW_STATE_CHANNEL, getAppWindowState(window))
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
  const rendererEntryUrl =
    is.dev && process.env['ELECTRON_RENDERER_URL']
      ? new URL(process.env['ELECTRON_RENDERER_URL']).toString()
      : pathToFileURL(join(__dirname, '../renderer/index.html')).toString()
  const mainWindow = new BrowserWindow({
    title: 'MyCopilot',
    width: 1120,
    height: 760,
    minWidth: 920,
    minHeight: 640,
    show: false,
    autoHideMenuBar: true,
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

  const rendererWebContents = mainWindow.webContents
  const rendererWebContentsId = rendererWebContents.id
  trustedRendererEntries.set(rendererWebContentsId, rendererEntryUrl)
  configureManagedWebviewHost(rendererWebContents)
  installNativeImageContextMenu(mainWindow)
  const handleWindowStateChange = (): void => sendAppWindowState(mainWindow)

  rendererWebContents.once('destroyed', () => {
    trustedRendererEntries.delete(rendererWebContentsId)
  })
  mainWindow.once('closed', () => {
    trustedRendererEntries.delete(rendererWebContentsId)
  })

  mainWindow.on('ready-to-show', () => {
    mainWindow.show()
    sendAppWindowState(mainWindow)
  })

  rendererWebContents.on('did-finish-load', handleWindowStateChange)
  mainWindow.on('maximize', handleWindowStateChange)
  mainWindow.on('unmaximize', handleWindowStateChange)
  mainWindow.on('enter-full-screen', handleWindowStateChange)
  mainWindow.on('leave-full-screen', handleWindowStateChange)
  mainWindow.on('restore', handleWindowStateChange)

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

  void mainWindow.loadURL(rendererEntryUrl).catch((error: unknown) => {
    if (!mainWindow.isDestroyed()) {
      console.error('Failed to load renderer entry', error)
    }
  })
}

app.whenReady().then(() => {
  app.setName('MyCopilot')
  electronApp.setAppUserModelId('com.mycopilot.next')
  disposeAdaptiveAppIcon = installAdaptiveAppIcon()
  coreServer.start()
  faviconResourceCache.registerProtocol()
  initializeManagedWebviewSessions()

  app.on('browser-window-created', (_, window) => {
    optimizer.watchWindowShortcuts(window)
  })

  registerHostIpc(coreServer, terminalBridge, faviconResourceCache, isTrustedRendererEvent)

  createWindow()

  app.on('activate', function () {
    if (BrowserWindow.getAllWindows().length === 0) createWindow()
  })
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
  void Promise.allSettled([terminalBridge.stop(), coreServer.shutdown()]).finally(() => app.quit())
})

app.on('will-quit', () => {
  disposeAdaptiveAppIcon?.()
  disposeAdaptiveAppIcon = null
  terminalBridge.killNow()
  coreServer.stop()
})
