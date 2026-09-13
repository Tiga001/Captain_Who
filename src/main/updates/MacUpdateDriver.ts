import type { autoUpdater as nativeUpdater } from 'electron'
import { CancellationToken, type MacUpdater } from 'electron-updater'
import type { DesktopUpdateDriver } from './UpdateService'

type Updater = Pick<
  MacUpdater,
  'checkForUpdates' | 'downloadUpdate' | 'quitAndInstall' | 'on' | 'removeListener'
>
type NativeUpdater = Pick<typeof nativeUpdater, 'checkForUpdates' | 'on' | 'removeListener'>

/** electron-updater 6 emits ZIP-ready before Squirrel.Mac finishes verifying/staging it. */
export class MacUpdateDriver implements DesktopUpdateDriver {
  private token: CancellationToken | null = null
  private ready = false
  private cancelled = false
  private cancelStaging: (() => void) | null = null
  private readonly errorListeners = new Set<(error: unknown) => void>()
  private readonly onError = (error: unknown): void => {
    if (!this.ready) return
    this.ready = false
    for (const listener of this.errorListeners) listener(error)
  }

  constructor(
    private readonly updater: Updater,
    private readonly native: NativeUpdater,
    private readonly feed: URL
  ) {
    updater.on('error', this.onError)
  }

  async check(): Promise<{ version: string } | null> {
    const result = await this.updater.checkForUpdates()
    if (!result?.isUpdateAvailable) return null
    const info = result.updateInfo
    if (!/^\d+\.\d+\.\d+$/.test(info.version) || info.version.length > 80)
      throw new Error('Invalid update version')
    // This phase only accepts signed arm64 ZIP releases inside the configured feed directory.
    const zipFiles = info.files?.filter((file) => {
      const url = new URL(file.url, this.feed)
      return url.pathname.toLowerCase().endsWith('.zip')
    })
    // The SDK's architecture filter also examines directory names. Require exactly one ZIP
    // so it cannot select another file than the one whose architecture/integrity we checked.
    const zip = zipFiles?.length === 1 ? zipFiles[0] : undefined
    const zipName = zip
      ? decodeURIComponent(new URL(zip.url, this.feed).pathname.split('/').pop() ?? '')
      : ''
    if (
      !zip ||
      !/(?:^|[-_.])arm64(?:[-_.]|$)/.test(zipName) ||
      typeof zip.sha512 !== 'string' ||
      !/^[A-Za-z0-9+/]{86}==$/.test(zip.sha512)
    )
      throw new Error('Missing update integrity metadata')
    for (const file of info.files) {
      const url = new URL(file.url, this.feed)
      if (
        url.protocol !== 'https:' ||
        url.origin !== this.feed.origin ||
        !url.pathname.startsWith(this.feed.pathname) ||
        url.username ||
        url.password ||
        url.search ||
        url.hash
      )
        throw new Error('Update file is outside configured source')
    }
    return { version: info.version }
  }

  async download(progress: (percent: number) => void): Promise<void> {
    this.cancelled = false
    this.ready = false
    const token = new CancellationToken()
    this.token = token
    const onProgress = (value: { percent: number }): void => progress(value.percent)
    this.updater.on('download-progress', onProgress)
    try {
      await this.updater.downloadUpdate(token)
      if (this.cancelled) throw new Error('Update cancelled')
      // Stage natively without closing windows. Squirrel cannot cancel this phase and may
      // apply a prepared update on next launch even without quitAndInstall. Do not invent a
      // timeout that reports failure/retry while native preparation is still running.
      await new Promise<void>((resolve, reject) => {
        const cleanup = (): void => {
          this.native.removeListener('update-downloaded', complete)
          this.native.removeListener('error', failed)
          this.native.removeListener('update-not-available', failed)
          this.cancelStaging = null
        }
        const complete = (): void => {
          cleanup()
          this.ready = true
          resolve()
        }
        const failed = (): void => {
          cleanup()
          reject(new Error('Native update preparation failed'))
        }
        this.cancelStaging = failed
        this.native.on('update-downloaded', complete)
        this.native.on('error', failed)
        this.native.on('update-not-available', failed)
        try {
          this.native.checkForUpdates()
        } catch {
          failed()
        }
      })
      if (this.cancelled) throw new Error('Update cancelled')
      if (!this.ready) throw new Error('Native update preparation failed')
    } finally {
      if (this.token === token) this.token = null
      this.updater.removeListener('download-progress', onProgress)
    }
  }
  install(): void {
    if (!this.ready || this.cancelled) throw new Error('Update is not ready for installation')
    this.updater.quitAndInstall()
  }
  cancel(): void {
    this.cancelled = true
    this.ready = false
    this.token?.cancel()
    // Only abandon JS observation during app shutdown. This cannot undo Squirrel staging.
    this.cancelStaging?.()
  }
  onInstallError(listener: (error: unknown) => void): () => void {
    this.errorListeners.add(listener)
    return () => {
      this.errorListeners.delete(listener)
    }
  }
  dispose(): void {
    this.cancel()
    this.updater.removeListener('error', this.onError)
    this.errorListeners.clear()
  }
}
