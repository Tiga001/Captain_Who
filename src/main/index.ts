// Electron main client.
import { app, shell, BrowserWindow } from 'electron'
import { join } from 'path'
import { electronApp, optimizer, is } from '@electron-toolkit/utils'
import icon from '../../resources/icon.png?asset'
import { BrowserWebContentsViewManager } from './browser/BrowserWebContentsViewManager'
import { CoreServer } from './core/coreServer'
import { registerHostIpc } from './ipc'
import { TerminalBridge } from './terminal/TerminalBridge'

const coreServer = new CoreServer()
const terminalBridge = new TerminalBridge()
let browserManager: BrowserWebContentsViewManager | null = null
let isQuittingAfterTerminalShutdown = false

function createWindow(): void {
  const mainWindow = new BrowserWindow({
    width: 1120,
    height: 760,
    minWidth: 920,
    minHeight: 640,
    show: false,
    autoHideMenuBar: true,
    ...(process.platform === 'linux' ? { icon } : {}),
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false
    }
  })

  browserManager = new BrowserWebContentsViewManager(mainWindow)

  mainWindow.on('closed', () => {
    browserManager?.destroyAll()
    browserManager = null
  })

  mainWindow.on('ready-to-show', () => {
    mainWindow.show()
  })

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
  electronApp.setAppUserModelId('com.mycopilot.next')
  coreServer.start()

  app.on('browser-window-created', (_, window) => {
    optimizer.watchWindowShortcuts(window)
  })

  registerHostIpc(coreServer, terminalBridge, () => {
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
  if (isQuittingAfterTerminalShutdown) {
    browserManager?.destroyAll()
    terminalBridge.killNow()
    coreServer.stop()
    return
  }

  event.preventDefault()
  isQuittingAfterTerminalShutdown = true
  void terminalBridge.stop().finally(() => {
    coreServer.stop()
    app.quit()
  })
})

app.on('will-quit', () => {
  browserManager?.destroyAll()
  terminalBridge.killNow()
  coreServer.stop()
})
