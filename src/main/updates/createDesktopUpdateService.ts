import { app, autoUpdater as nativeUpdater } from 'electron'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { MacUpdater } from 'electron-updater'
import { MacUpdateDriver } from './MacUpdateDriver'
import { UpdateService, type UpdateLifecycle } from './UpdateService'
import {
  isAllowedUpdateRequest,
  parseUpdateSource,
  stripUpdateIdentityHeaders
} from './updateSource'

export function createDesktopUpdateService(lifecycle: UpdateLifecycle): UpdateService {
  if (
    !app.isPackaged ||
    process.platform !== 'darwin' ||
    process.arch !== 'arm64' ||
    process.execPath.startsWith('/Volumes/') ||
    process.execPath.includes('/AppTranslocation/')
  )
    return new UpdateService(null, lifecycle)
  try {
    const configPath = join(process.resourcesPath, 'app-update.yml')
    const source = parseUpdateSource(readFileSync(configPath, 'utf8'))
    const updater = new MacUpdater()
    updater.updateConfigPath = configPath
    updater.autoDownload = false
    updater.autoInstallOnAppQuit = false
    updater.autoRunAppAfterInstall = true
    updater.allowDowngrade = false
    updater.allowPrerelease = false
    updater.disableDifferentialDownload = true
    updater.logger = null
    // The SDK owns a dedicated non-persistent network session, separate from user web content.
    updater.netSession.webRequest.onBeforeRequest((details, callback) => {
      callback({ cancel: !isAllowedUpdateRequest(details.url, source) })
    })
    updater.netSession.webRequest.onBeforeSendHeaders((details, callback) => {
      callback({ requestHeaders: stripUpdateIdentityHeaders(details.requestHeaders) })
    })
    return new UpdateService(new MacUpdateDriver(updater, nativeUpdater, source), lifecycle)
  } catch {
    // Optional unsigned/dev builds have no feed; never guess a server or print raw URLs/errors.
    return new UpdateService(null, lifecycle)
  }
}
