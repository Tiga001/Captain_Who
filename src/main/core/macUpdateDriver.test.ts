import { EventEmitter } from 'node:events'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { CancellationToken, UpdateCheckResult } from 'electron-updater'
import { MacUpdateDriver } from '../updates/MacUpdateDriver'

const FEED = new URL('https://updates.example.test/releases/')
const SHA512 = Buffer.alloc(64, 1).toString('base64')
type UpdateInfo = UpdateCheckResult['updateInfo']

function updateResult(patch: Partial<UpdateInfo> = {}): UpdateCheckResult {
  const updateInfo: UpdateInfo = {
    version: '1.2.3',
    files: [{ url: 'CaptainWho-1.2.3-arm64.zip', sha512: SHA512 }],
    path: 'CaptainWho-1.2.3-arm64.zip',
    sha512: SHA512,
    releaseDate: '2026-09-13T00:00:00.000Z',
    ...patch
  }
  return { isUpdateAvailable: true, updateInfo, versionInfo: updateInfo }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}

const drivers = new Set<MacUpdateDriver>()

function harness() {
  const zip = deferred<string[]>()
  const updater = Object.assign(new EventEmitter(), {
    checkForUpdates: vi.fn<() => Promise<UpdateCheckResult | null>>(() =>
      Promise.resolve(updateResult())
    ),
    downloadUpdate: vi.fn<(_token?: CancellationToken) => Promise<string[]>>(() => zip.promise),
    quitAndInstall: vi.fn()
  })
  const native = Object.assign(new EventEmitter(), { checkForUpdates: vi.fn() })
  // Electron-updater itself has a native error listener; keep an independent observer so that
  // late native error events remain ordinary events even after the driver's listener is removed.
  const nativeErrorObserver = vi.fn()
  native.on('error', nativeErrorObserver)
  // The SDK's chainable on() declares its return as the full MacUpdater class. These fakes
  // intentionally implement only the injected contract and never construct a real updater.
  const driver = new MacUpdateDriver(
    updater as unknown as ConstructorParameters<typeof MacUpdateDriver>[0],
    native as unknown as ConstructorParameters<typeof MacUpdateDriver>[1],
    FEED
  )
  drivers.add(driver)
  return { driver, native, nativeErrorObserver, updater, zip }
}

afterEach(() => {
  for (const driver of drivers) driver.dispose()
  drivers.clear()
  vi.useRealTimers()
})

// These fakes verify JavaScript dispatch and cleanup only. Once native staging starts, cancelling
// this driver cannot cancel Squirrel.Mac; a successfully staged update may apply on a later launch.
describe('MacUpdateDriver staging and installation', () => {
  it('keeps ZIP download separate from native preparation and permits quitAndInstall only after readiness', async () => {
    const { driver, native, updater, zip } = harness()
    const progress = vi.fn()
    const prepared = vi.fn()
    const download = driver.download(progress)
    void download.then(prepared, () => undefined)

    updater.emit('download-progress', { percent: 42.5 })
    updater.emit('update-downloaded', updateResult().updateInfo)
    expect(progress).toHaveBeenCalledWith(42.5)
    expect(native.checkForUpdates).not.toHaveBeenCalled()
    expect(() => driver.install()).toThrow('not ready')

    zip.resolve(['/fixture/cache/CaptainWho-1.2.3-arm64.zip'])
    await zip.promise
    expect(progress).toHaveBeenLastCalledWith(100)
    expect(native.checkForUpdates).toHaveBeenCalledTimes(1)
    expect(prepared).not.toHaveBeenCalled()
    expect(updater.quitAndInstall).not.toHaveBeenCalled()
    expect(() => driver.install()).toThrow('not ready')

    native.emit('update-downloaded')
    await download
    expect(prepared).toHaveBeenCalledTimes(1)
    expect(updater.quitAndInstall).not.toHaveBeenCalled()
    driver.install()
    expect(updater.quitAndInstall).toHaveBeenCalledTimes(1)
    expect(updater.listenerCount('download-progress')).toBe(0)
    expect(native.listenerCount('update-downloaded')).toBe(0)
    expect(native.listenerCount('update-not-available')).toBe(0)
    expect(native.listenerCount('error')).toBe(1)
  })

  it('cleans only its own listeners and stops forwarding progress after staging completes', async () => {
    const { driver, native, updater, zip } = harness()
    const outsideProgress = vi.fn()
    const outsidePrepared = vi.fn()
    const outsideUnavailable = vi.fn()
    const progress = vi.fn()
    updater.on('download-progress', outsideProgress)
    native.on('update-downloaded', outsidePrepared)
    native.on('update-not-available', outsideUnavailable)
    const download = driver.download(progress)
    zip.resolve([])
    await zip.promise
    // Cache hits have no SDK transfer events; report completion before native staging finishes.
    expect(progress).toHaveBeenCalledExactlyOnceWith(100)
    expect(native.checkForUpdates).toHaveBeenCalledTimes(1)
    expect(() => driver.install()).toThrow('not ready')
    native.emit('update-downloaded')
    await download

    updater.emit('download-progress', { percent: 100 })
    expect(progress).toHaveBeenCalledExactlyOnceWith(100)
    expect(outsideProgress).toHaveBeenCalledTimes(1)
    expect(native.listeners('update-downloaded')).toEqual([outsidePrepared])
    expect(native.listeners('update-not-available')).toEqual([outsideUnavailable])
    expect(updater.listeners('download-progress')).toEqual([outsideProgress])

    driver.dispose()
    expect(updater.listenerCount('error')).toBe(0)
    expect(updater.listeners('download-progress')).toEqual([outsideProgress])
  })

  it('does not call native preparation or quitAndInstall when ZIP download fails', async () => {
    const { driver, native, updater, zip } = harness()
    const failure = new Error('ZIP integrity mismatch')
    const failed = expect(driver.download(vi.fn())).rejects.toBe(failure)
    zip.reject(failure)
    await failed

    expect(native.checkForUpdates).not.toHaveBeenCalled()
    expect(updater.quitAndInstall).not.toHaveBeenCalled()
    expect(updater.listenerCount('download-progress')).toBe(0)
    expect(() => driver.install()).toThrow('not ready')
  })

  it('rejects a native preparation error without dispatching quitAndInstall and ignores a late ready event', async () => {
    const { driver, native, updater, zip } = harness()
    const installError = vi.fn()
    driver.onInstallError(installError)
    const failed = expect(driver.download(vi.fn())).rejects.toThrow('Native update preparation')
    zip.resolve([])
    await zip.promise

    native.emit('error', new Error('native signature mismatch'))
    await failed
    native.emit('update-downloaded')

    expect(installError).not.toHaveBeenCalled()
    expect(updater.quitAndInstall).not.toHaveBeenCalled()
    expect(() => driver.install()).toThrow('not ready')
    expect(updater.listenerCount('download-progress')).toBe(0)
    expect(native.listenerCount('update-downloaded')).toBe(0)
    expect(native.listenerCount('update-not-available')).toBe(0)
    expect(native.listenerCount('error')).toBe(1)
  })

  it('rejects an explicit native no-update result and clears all preparation listeners', async () => {
    const { driver, native, updater, zip } = harness()
    const failed = expect(driver.download(vi.fn())).rejects.toThrow('Native update preparation')
    zip.resolve([])
    await zip.promise
    expect(native.listenerCount('update-not-available')).toBe(1)

    native.emit('update-not-available')
    await failed
    native.emit('update-downloaded')

    expect(updater.quitAndInstall).not.toHaveBeenCalled()
    expect(() => driver.install()).toThrow('not ready')
    expect(updater.listenerCount('download-progress')).toBe(0)
    expect(native.listenerCount('update-downloaded')).toBe(0)
    expect(native.listenerCount('update-not-available')).toBe(0)
    expect(native.listenerCount('error')).toBe(1)
  })

  it('keeps one native preparation pending after 30 minutes until a native error arrives', async () => {
    vi.useFakeTimers()
    const { driver, native, updater, zip } = harness()
    const settled = vi.fn()
    const download = driver.download(vi.fn())
    void download.then(settled, settled)
    const failed = expect(download).rejects.toThrow('Native update preparation')
    zip.resolve([])
    await zip.promise

    await vi.advanceTimersByTimeAsync(30 * 60_000)
    expect(settled).not.toHaveBeenCalled()
    expect(native.checkForUpdates).toHaveBeenCalledTimes(1)
    expect(updater.downloadUpdate).toHaveBeenCalledTimes(1)
    expect(native.listenerCount('update-downloaded')).toBe(1)
    expect(native.listenerCount('update-not-available')).toBe(1)
    expect(native.listenerCount('error')).toBe(2)
    expect(updater.listenerCount('download-progress')).toBe(1)
    expect(vi.getTimerCount()).toBe(0)

    native.emit('error', new Error('native preparation failed after a long wait'))
    await failed

    expect(settled).toHaveBeenCalledTimes(1)
    expect(updater.quitAndInstall).not.toHaveBeenCalled()
    expect(() => driver.install()).toThrow('not ready')
    expect(updater.listenerCount('download-progress')).toBe(0)
    expect(native.listenerCount('update-downloaded')).toBe(0)
    expect(native.listenerCount('update-not-available')).toBe(0)
    expect(native.listenerCount('error')).toBe(1)
    expect(vi.getTimerCount()).toBe(0)
  })

  it('handles a native check that throws without leaving preparation listeners registered', async () => {
    const { driver, native, updater, zip } = harness()
    native.checkForUpdates.mockImplementation(() => {
      throw new Error('native updater unavailable')
    })
    const failed = expect(driver.download(vi.fn())).rejects.toThrow('Native update preparation')
    zip.resolve([])
    await failed

    expect(updater.quitAndInstall).not.toHaveBeenCalled()
    expect(native.listenerCount('update-downloaded')).toBe(0)
    expect(native.listenerCount('error')).toBe(1)
    expect(updater.listenerCount('download-progress')).toBe(0)
  })

  it('registers preparation listeners before a native check that completes synchronously', async () => {
    const { driver, native, updater, zip } = harness()
    native.checkForUpdates.mockImplementation(() => {
      native.emit('update-downloaded')
    })
    const download = driver.download(vi.fn())
    zip.resolve([])
    await download

    driver.install()
    expect(updater.quitAndInstall).toHaveBeenCalledTimes(1)
    expect(native.listenerCount('update-downloaded')).toBe(0)
    expect(native.listenerCount('error')).toBe(1)
  })

  it('cancels the active ZIP token and fences a download completion that races cancellation', async () => {
    const { driver, native, updater, zip } = harness()
    const failed = expect(driver.download(vi.fn())).rejects.toThrow('Update cancelled')
    const token = updater.downloadUpdate.mock.calls[0]?.[0]
    expect(token?.cancelled).toBe(false)

    driver.cancel()
    expect(token?.cancelled).toBe(true)
    zip.resolve([])
    await failed

    expect(native.checkForUpdates).not.toHaveBeenCalled()
    expect(updater.quitAndInstall).not.toHaveBeenCalled()
    expect(updater.listenerCount('download-progress')).toBe(0)
    expect(() => driver.install()).toThrow('not ready')
  })

  it('does not start native staging when completion publication triggers shutdown', async () => {
    const { driver, native, updater, zip } = harness()
    const failed = expect(driver.download(() => driver.cancel())).rejects.toThrow(
      'Update cancelled'
    )
    zip.resolve([])
    await failed

    expect(native.checkForUpdates).not.toHaveBeenCalled()
    expect(updater.quitAndInstall).not.toHaveBeenCalled()
    expect(native.listenerCount('update-downloaded')).toBe(0)
    expect(updater.listenerCount('download-progress')).toBe(0)
  })

  it('stops the JS preparation wait and further install dispatch when cancelled during native staging', async () => {
    const { driver, native, updater, zip } = harness()
    const failed = expect(driver.download(vi.fn())).rejects.toThrow('Native update preparation')
    zip.resolve([])
    await zip.promise

    // This only stops the application's wait and explicit install dispatch. The native staging
    // work already started by checkForUpdates may continue and apply on a subsequent app launch.
    driver.cancel()
    await failed
    native.emit('update-downloaded')

    expect(() => driver.install()).toThrow('not ready')
    expect(updater.quitAndInstall).not.toHaveBeenCalled()
    expect(updater.listenerCount('download-progress')).toBe(0)
    expect(native.listenerCount('update-downloaded')).toBe(0)
    expect(native.listenerCount('error')).toBe(1)
  })

  it.each(['cancel', 'updater error'])(
    'rejects preparation when %s races the continuation after native readiness',
    async (failureKind) => {
      const { driver, native, updater, zip } = harness()
      const failed = expect(driver.download(vi.fn())).rejects.toThrow()
      zip.resolve([])
      await zip.promise

      native.emit('update-downloaded')
      if (failureKind === 'cancel') driver.cancel()
      else updater.emit('error', new Error('native preparation became unavailable'))
      await failed

      expect(() => driver.install()).toThrow('not ready')
      expect(updater.quitAndInstall).not.toHaveBeenCalled()
      expect(updater.listenerCount('download-progress')).toBe(0)
      expect(native.listenerCount('update-downloaded')).toBe(0)
    }
  )

  it('lets an explicit retry prepare after ZIP cancellation before native staging began', async () => {
    const { driver, native, updater, zip } = harness()
    const cancelled = expect(driver.download(vi.fn())).rejects.toThrow()
    driver.cancel()
    zip.resolve([])
    await cancelled
    const oldToken = updater.downloadUpdate.mock.calls[0]?.[0]

    const retry = driver.download(vi.fn())
    await zip.promise
    const newToken = updater.downloadUpdate.mock.calls[1]?.[0]
    expect(newToken).not.toBe(oldToken)
    expect(newToken?.cancelled).toBe(false)
    native.emit('update-downloaded')
    await retry
    driver.install()

    expect(updater.quitAndInstall).toHaveBeenCalledTimes(1)
    expect(updater.listenerCount('download-progress')).toBe(0)
  })

  it('forwards an asynchronous install error once and invalidates prepared installation', async () => {
    const { driver, native, updater, zip } = harness()
    const installError = vi.fn()
    driver.onInstallError(installError)
    const download = driver.download(vi.fn())
    updater.emit('error', new Error('unrelated error before preparation'))
    expect(installError).not.toHaveBeenCalled()
    zip.resolve([])
    await zip.promise
    native.emit('update-downloaded')
    await download
    driver.install()

    const failure = new Error('native installation failed asynchronously')
    updater.emit('error', failure)
    updater.emit('error', new Error('duplicate error event'))
    expect(installError).toHaveBeenCalledExactlyOnceWith(failure)
    expect(() => driver.install()).toThrow('not ready')
    expect(updater.quitAndInstall).toHaveBeenCalledTimes(1)
  })

  it('removes unsubscribed install observers and blocks further JS install dispatch after cancellation', async () => {
    const { driver, native, updater, zip } = harness()
    const removed = vi.fn()
    const unsubscribe = driver.onInstallError(removed)
    const download = driver.download(vi.fn())
    zip.resolve([])
    await zip.promise
    native.emit('update-downloaded')
    await download
    unsubscribe()
    updater.emit('error', new Error('late update error'))
    driver.cancel()

    expect(removed).not.toHaveBeenCalled()
    expect(() => driver.install()).toThrow('not ready')
    expect(updater.quitAndInstall).not.toHaveBeenCalled()
  })

  it('disposes JS callbacks during native staging and rejects further explicit install dispatch', async () => {
    const { driver, native, updater, zip } = harness()
    const failed = expect(driver.download(vi.fn())).rejects.toThrow('Native update preparation')
    zip.resolve([])
    await zip.promise
    driver.dispose()
    await failed
    native.emit('update-downloaded')

    expect(updater.listenerCount('error')).toBe(0)
    expect(updater.listenerCount('download-progress')).toBe(0)
    expect(native.listenerCount('update-downloaded')).toBe(0)
    expect(native.listenerCount('update-not-available')).toBe(0)
    expect(native.listenerCount('error')).toBe(1)
    expect(() => driver.install()).toThrow('not ready')
    expect(updater.quitAndInstall).not.toHaveBeenCalled()
  })
})

describe('MacUpdateDriver manifest validation', () => {
  it.each([
    'CaptainWho-1.2.3-arm64.zip',
    'CaptainWho-1.2.3-arm64.ZIP',
    'https://updates.example.test/releases/CaptainWho-1.2.3-arm64.zip'
  ])('accepts a stable version and SHA-512 protected arm64 ZIP at %s', async (url) => {
    const { driver, updater } = harness()
    updater.checkForUpdates.mockResolvedValue(updateResult({ files: [{ url, sha512: SHA512 }] }))

    await expect(driver.check()).resolves.toEqual({ version: '1.2.3' })
    expect(updater.downloadUpdate).not.toHaveBeenCalled()
    expect(updater.quitAndInstall).not.toHaveBeenCalled()
  })

  it('returns no update for null results and unavailable releases', async () => {
    const { driver, updater } = harness()
    updater.checkForUpdates.mockResolvedValueOnce(null)
    updater.checkForUpdates.mockResolvedValueOnce({ ...updateResult(), isUpdateAvailable: false })

    await expect(driver.check()).resolves.toBeNull()
    await expect(driver.check()).resolves.toBeNull()
    expect(updater.downloadUpdate).not.toHaveBeenCalled()
  })

  it.each(['v1.2.3', '1.2', '1.2.3-beta.1', '1.2.3+build', '-1.2.3', `${'1'.repeat(80)}.2.3`])(
    'rejects invalid or non-stable version %s',
    async (version) => {
      const { driver, updater } = harness()
      updater.checkForUpdates.mockResolvedValue(updateResult({ version }))
      await expect(driver.check()).rejects.toThrow('Invalid update version')
      expect(updater.downloadUpdate).not.toHaveBeenCalled()
    }
  )

  it.each(
    [
      [],
      [{ url: 'CaptainWho-1.2.3-x64.zip', sha512: SHA512 }],
      [{ url: 'arm64/CaptainWho-1.2.3-x64.zip', sha512: SHA512 }],
      [{ url: 'CaptainWho-1.2.3-farm64.zip', sha512: SHA512 }],
      [{ url: 'CaptainWho-1.2.3-arm64e.zip', sha512: SHA512 }],
      [{ url: 'CaptainWho-1.2.3-arm64.dmg', sha512: SHA512 }],
      [{ url: 'CaptainWho-1.2.3-arm64.zip', sha512: '' }],
      [{ url: 'CaptainWho-1.2.3-arm64.zip', sha512: 'not-a-sha512' }],
      [{ url: 'CaptainWho-1.2.3-arm64.zip', sha512: SHA512.slice(0, -2) }],
      [{ url: 'CaptainWho-1.2.3-arm64.zip', sha512: `${'-'.repeat(86)}==` }]
    ].map((files) => ({ files }))
  )('rejects manifests without a valid SHA-512 protected arm64 ZIP: %j', async ({ files }) => {
    const { driver, updater } = harness()
    updater.checkForUpdates.mockResolvedValue(updateResult({ files }))
    await expect(driver.check()).rejects.toThrow('Missing update integrity metadata')
    expect(updater.downloadUpdate).not.toHaveBeenCalled()
  })

  it.each([
    'https://other.example.test/releases/CaptainWho-arm64.zip',
    'http://updates.example.test/releases/CaptainWho-arm64.zip',
    'https://updates.example.test/other/CaptainWho-arm64.zip',
    'https://updates.example.test/releases-other/CaptainWho-arm64.zip',
    '../CaptainWho-arm64.zip',
    'https://user:secret@updates.example.test/releases/CaptainWho-arm64.zip',
    'CaptainWho-arm64.zip?token=private',
    'CaptainWho-arm64.zip#fragment'
  ])('rejects a ZIP URL outside the exact configured public feed: %s', async (url) => {
    const { driver, updater } = harness()
    updater.checkForUpdates.mockResolvedValue(updateResult({ files: [{ url, sha512: SHA512 }] }))
    await expect(driver.check()).rejects.toThrow('outside configured source')
    expect(updater.downloadUpdate).not.toHaveBeenCalled()
  })

  it('validates every referenced file even when a valid arm64 ZIP is present', async () => {
    const { driver, updater } = harness()
    updater.checkForUpdates.mockResolvedValue(
      updateResult({
        files: [
          { url: 'CaptainWho-arm64.zip', sha512: SHA512 },
          { url: 'https://other.example.test/CaptainWho.dmg', sha512: SHA512 }
        ]
      })
    )

    await expect(driver.check()).rejects.toThrow('outside configured source')
    expect(updater.downloadUpdate).not.toHaveBeenCalled()
  })

  it.each([
    {
      label: 'x64 ZIP before arm64 ZIP',
      names: ['CaptainWho-1.2.3-x64.zip', 'CaptainWho-1.2.3-arm64.zip']
    },
    {
      label: 'arm64 ZIP before x64 ZIP',
      names: ['CaptainWho-1.2.3-arm64.zip', 'CaptainWho-1.2.3-x64.zip']
    },
    {
      label: 'multiple arm64 ZIPs',
      names: ['CaptainWho-1.2.3-arm64.zip', 'CaptainWho-1.2.3-arm64-alternate.zip']
    },
    {
      label: 'uppercase x64 ZIP before arm64 ZIP',
      names: ['CaptainWho-1.2.3-x64.ZIP', 'CaptainWho-1.2.3-arm64.zip']
    },
    {
      label: 'mixed-case extra ZIP',
      names: ['CaptainWho-1.2.3-arm64.zip', 'CaptainWho-1.2.3-arm64-alternate.Zip']
    }
  ])('rejects multiple ZIP files even with valid integrity metadata: $label', async ({ names }) => {
    const { driver, updater } = harness()
    updater.checkForUpdates.mockResolvedValue(
      updateResult({ files: names.map((url) => ({ url, sha512: SHA512 })) })
    )

    await expect(driver.check()).rejects.toThrow()
    expect(updater.downloadUpdate).not.toHaveBeenCalled()
  })
})
