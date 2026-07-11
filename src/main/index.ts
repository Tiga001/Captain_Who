// Electron main client.
import { app, shell, BrowserWindow } from 'electron'
import { join } from 'path'
import { electronApp, optimizer, is } from '@electron-toolkit/utils'
import type { AppWindowState } from '@mycopilot/host-api'
import icon from '../../resources/icon.png?asset'
import { BrowserWebContentsViewManager } from './browser/BrowserWebContentsViewManager'
import { CoreServer } from './core/coreServer'
import { registerHostIpc } from './ipc'
import { FaviconResourceCache, registerResourceSchemes } from './resources/FaviconResourceCache'
import { TerminalBridge } from './terminal/TerminalBridge'

registerResourceSchemes()

const coreServer = new CoreServer()
const terminalBridge = new TerminalBridge()
const faviconResourceCache = new FaviconResourceCache()
let browserManager: BrowserWebContentsViewManager | null = null
let isQuittingAfterServiceShutdown = false

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

function createWindow(): void {
  const mainWindow = new BrowserWindow({
    title: 'MyCopilot',
    width: 1120,
    height: 760,
    minWidth: 920,
    minHeight: 640,
    show: false,
    autoHideMenuBar: true,
    ...macWindowChromeOptions,
    ...(process.platform === 'linux' ? { icon } : {}),
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false
    }
  })

  browserManager = new BrowserWebContentsViewManager(mainWindow)
  const handleWindowStateChange = (): void => sendAppWindowState(mainWindow)

  mainWindow.on('closed', () => {
    browserManager?.destroyAll()
    browserManager = null
  })

  mainWindow.on('ready-to-show', () => {
    mainWindow.show()
    sendAppWindowState(mainWindow)
  })

  mainWindow.webContents.on('did-finish-load', handleWindowStateChange)
  mainWindow.on('maximize', handleWindowStateChange)
  mainWindow.on('unmaximize', handleWindowStateChange)
  mainWindow.on('enter-full-screen', handleWindowStateChange)
  mainWindow.on('leave-full-screen', handleWindowStateChange)
  mainWindow.on('restore', handleWindowStateChange)

  mainWindow.webContents.setWindowOpenHandler((details) => {
    shell.openExternal(details.url)
    return { action: 'deny' }
  })

  // HMR for renderer base on electron-vite cli.
  // Load the remote URL for development or the local html file for production.
  if (is.dev && process.env['ELECTRON_RENDERER_URL']) {
    mainWindow.loadURL(process.env['ELECTRON_RENDERER_URL'])
  } else {
    mainWindow.loadFile(join(__dirname, '../renderer/index.html'))
  }
}

// This method will be called when Electron has finished
// initialization and is ready to create browser windows.
// Some APIs can only be used after this event occurs.
app.whenReady().then(() => {
  app.setName('MyCopilot')
  electronApp.setAppUserModelId('com.mycopilot.next')
  coreServer.start()
  faviconResourceCache.registerProtocol()

  app.on('browser-window-created', (_, window) => {
    optimizer.watchWindowShortcuts(window)
  })

  registerHostIpc(coreServer, terminalBridge, faviconResourceCache, () => {
    if (!browserManager) {
      throw new Error('Browser view manager is not available')
    }

    return browserManager
  })

  createWindow()

  app.on('activate', function () {
    // On macOS it's common to re-create a window in the app when the
    // dock icon is clicked and there are no other windows open.
    if (BrowserWindow.getAllWindows().length === 0) createWindow()
  })
})

// Quit when all windows are closed, except on macOS. There, it's common
// for applications and their menu bar to stay active until the user quits
// explicitly with Cmd + Q.
app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') {
    app.quit()
  }
})

app.on('before-quit', (event) => {
  if (isQuittingAfterServiceShutdown) {
    browserManager?.destroyAll()
    terminalBridge.killNow()
    coreServer.stop()
    return
  }

  event.preventDefault()
  isQuittingAfterServiceShutdown = true
  void Promise.allSettled([terminalBridge.stop(), coreServer.shutdown()]).finally(() => app.quit())
})

app.on('will-quit', () => {
  browserManager?.destroyAll()
  terminalBridge.killNow()
  coreServer.stop()
})
