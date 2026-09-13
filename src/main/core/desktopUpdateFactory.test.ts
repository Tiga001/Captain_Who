import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  app: { isPackaged: true },
  readFileSync: vi.fn(),
  updater: {
    checkForUpdates: vi.fn().mockResolvedValue(null),
    downloadUpdate: vi.fn(),
    quitAndInstall: vi.fn(),
    on: vi.fn(),
    removeListener: vi.fn(),
    netSession: { webRequest: { onBeforeRequest: vi.fn(), onBeforeSendHeaders: vi.fn() } }
  },
  constructed: vi.fn()
}))
vi.mock('electron', () => ({
  app: mocks.app,
  autoUpdater: { on: vi.fn(), removeListener: vi.fn(), checkForUpdates: vi.fn() }
}))
vi.mock('node:fs', () => ({ readFileSync: mocks.readFileSync }))
vi.mock('electron-updater', () => ({
  MacUpdater: class {
    constructor() {
      mocks.constructed()
      return mocks.updater
    }
  },
  CancellationToken: class {}
}))
import { createDesktopUpdateService } from '../updates/createDesktopUpdateService'

const lifecycle = {
  requestInstall: vi.fn(() => true),
  onInstallFailure: vi.fn(),
  isShuttingDown: () => false
}
beforeEach(() => {
  vi.clearAllMocks()
  mocks.app.isPackaged = true
  mocks.readFileSync.mockReturnValue(
    'provider: generic\nurl: https://updates.example.test/mac/arm64/\nchannel: latest\n'
  )
  vi.stubGlobal('process', {
    ...process,
    platform: 'darwin',
    arch: 'arm64',
    execPath: '/Applications/Captain Who.app/Contents/MacOS/Captain Who',
    resourcesPath: '/fixture/Contents/Resources'
  })
})
afterEach(() => vi.unstubAllGlobals())

describe('desktop update composition', () => {
  it('uses only signed app-update.yml and disables implicit downloads, installs and downgrades', () => {
    const service = createDesktopUpdateService(lifecycle)
    expect(mocks.readFileSync).toHaveBeenCalledWith(
      '/fixture/Contents/Resources/app-update.yml',
      'utf8'
    )
    expect(mocks.updater).toMatchObject({
      updateConfigPath: '/fixture/Contents/Resources/app-update.yml',
      autoDownload: false,
      autoInstallOnAppQuit: false,
      autoRunAppAfterInstall: true,
      allowDowngrade: false,
      allowPrerelease: false,
      disableDifferentialDownload: true,
      logger: null
    })
    expect(mocks.updater.checkForUpdates).not.toHaveBeenCalled()
    service.startOnce()
    expect(mocks.updater.checkForUpdates).toHaveBeenCalledOnce()
    service.dispose()
  })
  it.each([
    { platform: 'linux' },
    { platform: 'win32' },
    { arch: 'x64' },
    { execPath: '/Volumes/Captain Who/Captain Who.app/Contents/MacOS/Captain Who' },
    { execPath: '/private/var/AppTranslocation/uuid/d/Captain Who.app/Contents/MacOS/Captain Who' }
  ])('does not enable a driver on unsupported/read-only launch paths %#', (overrides) => {
    vi.stubGlobal('process', { ...process, ...overrides })
    const service = createDesktopUpdateService(lifecycle)
    service.startOnce()
    expect(service.getState().status).toBe('disabled')
    expect(mocks.readFileSync).not.toHaveBeenCalled()
    expect(mocks.constructed).not.toHaveBeenCalled()
  })
  it('does not read any update source or make requests in development', () => {
    mocks.app.isPackaged = false
    createDesktopUpdateService(lifecycle).startOnce()
    expect(mocks.readFileSync).not.toHaveBeenCalled()
    expect(mocks.constructed).not.toHaveBeenCalled()
  })
  it('fails closed for missing/invalid configuration even when a runtime env override is set', () => {
    vi.stubGlobal('process', {
      ...process,
      env: { ...process.env, CAPTAIN_WHO_UPDATE_URL: 'https://runtime.example.test/' }
    })
    for (const missing of [true, false]) {
      mocks.readFileSync.mockImplementation(() => {
        if (missing) throw new Error('ENOENT')
        return 'provider: generic\nurl: http://untrusted.example/'
      })
      const service = createDesktopUpdateService(lifecycle)
      service.startOnce()
      expect(service.getState().status).toBe('disabled')
    }
    expect(mocks.constructed).not.toHaveBeenCalled()
  })
  it('installs session-local allowlisting and strips account/device headers', () => {
    const service = createDesktopUpdateService(lifecycle)
    const guard = mocks.updater.netSession.webRequest.onBeforeRequest.mock.calls[0][0]
    const callback = vi.fn()
    guard({ url: 'https://other.example.test/archive.zip' }, callback)
    expect(callback).toHaveBeenLastCalledWith({ cancel: true })
    guard({ url: 'https://updates.example.test/mac/arm64/latest-mac.yml?noCache=1' }, callback)
    expect(callback).toHaveBeenLastCalledWith({ cancel: false })
    mocks.updater.netSession.webRequest.onBeforeSendHeaders.mock.calls[0][0](
      { requestHeaders: { 'x-user-staging-id': 'uuid', Authorization: 'secret', Accept: '*/*' } },
      callback
    )
    expect(callback).toHaveBeenLastCalledWith({ requestHeaders: { Accept: '*/*' } })
    service.dispose()
  })
})
