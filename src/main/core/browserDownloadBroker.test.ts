import { EventEmitter } from 'node:events'
import {
  access,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  realpath,
  rm,
  writeFile
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, join } from 'node:path'
import type { DownloadItem, Session, WebContents } from 'electron'
import {
  BROWSER_DOWNLOAD_SCHEMA_VERSION,
  type BrowserDownloadRecord,
  type BrowserDownloadRegistrationInput,
  type BrowserDownloadSettingsRecord
} from '@mycopilot/protocol'
import { afterEach, describe, expect, it } from 'vitest'

import { BrowserDownloadBroker, type BrowserDownloadOwner } from '../browser/BrowserDownloadBroker'

const temporaryRoots = new Set<string>()
const OWNER: BrowserDownloadOwner = {
  conversationId: 'conversation-1',
  runId: 'run-1',
  activationId: 'activation-1',
  capabilityId: 'browser_automation',
  surfaceId: 'surface-1',
  generation: 1,
  toolCallId: 'call-1'
}

class FakeSession extends EventEmitter {}

class FakeDownloadItem extends EventEmitter {
  cancelled = false
  currentBytesPerSecond = 0
  paused = false
  saveDialogOptions?: { defaultPath?: string }
  savePath?: string
  receivedBytes = 0
  state: 'progressing' | 'completed' | 'cancelled' | 'interrupted' = 'progressing'

  constructor(
    private readonly filename = 'report.txt',
    private readonly mimeType = 'text/plain',
    private readonly totalBytes = 4,
    private readonly url = 'https://example.com/report.txt'
  ) {
    super()
  }

  cancel(): void {
    this.cancelled = true
    this.state = 'cancelled'
  }

  pause(): void {
    this.paused = true
  }

  resume(): void {
    this.paused = false
  }

  canResume(): boolean {
    return this.paused && this.state === 'progressing'
  }

  isPaused(): boolean {
    return this.paused
  }

  setSavePath(path: string): void {
    this.savePath = path
  }

  setSaveDialogOptions(options: { defaultPath?: string }): void {
    this.saveDialogOptions = options
  }

  getSavePath(): string {
    return this.savePath ?? ''
  }

  chooseSavePath(path: string): void {
    this.savePath = path
  }

  cancelByUser(): void {
    this.state = 'cancelled'
    this.emit('done', {}, 'cancelled')
  }

  getFilename(): string {
    return this.filename
  }

  getMimeType(): string {
    return this.mimeType
  }

  getTotalBytes(): number {
    return this.totalBytes
  }

  getReceivedBytes(): number {
    return this.receivedBytes
  }

  getCurrentBytesPerSecond(): number {
    return this.currentBytesPerSecond
  }

  getStartTime(): number {
    return 1
  }

  getState(): typeof this.state {
    return this.state
  }

  getURL(): string {
    return this.url
  }

  update(receivedBytes: number, bytesPerSecond = 0): void {
    this.receivedBytes = receivedBytes
    this.currentBytesPerSecond = bytesPerSecond
    this.emit('updated', {}, 'progressing')
  }

  async complete(bytes = Buffer.from('data')): Promise<void> {
    if (!this.savePath) throw new Error('Missing managed save path')
    await writeFile(this.savePath, bytes)
    this.receivedBytes = bytes.byteLength
    this.state = 'completed'
    this.emit('done', {}, 'completed')
  }
}

afterEach(async () => {
  await Promise.all([...temporaryRoots].map((root) => rm(root, { recursive: true, force: true })))
  temporaryRoots.clear()
})

async function createHarness(
  options: {
    askWhereToSave?: boolean
    maxSingleDownloadBytes?: number
    registerDownload?: (input: BrowserDownloadRegistrationInput) => Promise<BrowserDownloadRecord>
  } = {}
) {
  const root = await mkdtemp(join(tmpdir(), 'mycopilot-download-test-'))
  temporaryRoots.add(root)
  const systemDirectory = join(root, 'Downloads')
  await mkdir(systemDirectory)
  const records: BrowserDownloadRecord[] = []
  const session = new FakeSession()
  const settings: BrowserDownloadSettingsRecord = {
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    locationMode: 'system',
    customDirectory: null,
    askWhereToSave: options.askWhereToSave ?? false,
    revision: 0,
    updatedAt: 1
  }
  const broker = new BrowserDownloadBroker({
    expectedSession: session as unknown as Session,
    initialSettings: settings,
    registerDownload:
      options.registerDownload ??
      (async (input) => {
        const record: BrowserDownloadRecord = { ...input, projectId: 'project-1' }
        records.push(record)
        return record
      }),
    systemDownloadDirectory: systemDirectory,
    maxSingleDownloadBytes: options.maxSingleDownloadBytes
  })
  broker.install()
  const guest = {
    id: 41,
    session,
    getType: () => 'webview',
    isDestroyed: () => false
  } as unknown as WebContents
  broker.registerGuest({ guest, surfaceId: OWNER.surfaceId, generation: OWNER.generation })
  return { broker, guest, records, root, session, settings, systemDirectory }
}

function dispatchDownload(session: FakeSession, guest: WebContents, item: FakeDownloadItem): void {
  session.emit('will-download', {}, item as unknown as DownloadItem, guest)
}

async function waitForManagedPath(item: FakeDownloadItem): Promise<string> {
  await waitFor(() => Boolean(item.savePath))
  return item.savePath as string
}

async function waitFor(predicate: () => boolean): Promise<void> {
  for (let turn = 0; turn < 200; turn += 1) {
    if (predicate()) return
    await new Promise<void>((resolveImmediate) => setImmediate(resolveImmediate))
  }
  throw new Error('Timed out waiting for Browser Download state')
}

describe('BrowserDownloadBroker', () => {
  it('publishes an Agent download durably and returns only a path-free reference', async () => {
    const { broker, guest, records, session, systemDirectory } = await createHarness()
    const lease = broker.beginTool({ guest, owner: OWNER })
    lease.markDispatched()
    const item = new FakeDownloadItem('../../report.txt')

    dispatchDownload(session, guest, item)
    const temporaryPath = await waitForManagedPath(item)
    expect(basename(temporaryPath)).toMatch(/^\.mycopilot-download-.+\.part$/u)
    await item.complete()

    const [reference] = await lease.settle()
    expect(reference).toEqual(
      expect.objectContaining({
        displayName: 'report.txt',
        mimeType: 'text/plain',
        sizeBytes: 4,
        source: 'agent'
      })
    )
    expect(reference?.downloadId).toMatch(/^browser-download:/u)
    expect(JSON.stringify(reference)).not.toContain(systemDirectory)
    expect(lease.downloads()).toEqual([reference])
    expect(records).toHaveLength(1)
    expect(records[0]).toMatchObject({
      conversationId: OWNER.conversationId,
      runId: OWNER.runId,
      callId: OWNER.toolCallId,
      sourceOrigin: 'https://example.com'
    })
    await expect(readFile(records[0]!.absolutePath, 'utf8')).resolves.toBe('data')
    await expect(access(temporaryPath)).rejects.toBeDefined()

    lease.finish()
    await broker.shutdown()
  })

  it('detaches an admitted Agent transfer from its click and exposes task-scoped progress', async () => {
    const { broker, guest, records, session, systemDirectory } = await createHarness()
    const lease = broker.beginTool({ guest, owner: OWNER })
    lease.markDispatched()
    const item = new FakeDownloadItem('installer.dmg', 'application/x-apple-diskimage', 100)

    dispatchDownload(session, guest, item)
    await waitForManagedPath(item)
    item.update(25, 10)

    await expect(lease.settle()).resolves.toEqual([])
    expect(lease.progress?.()).toMatchObject([
      {
        displayName: 'installer.dmg',
        state: 'progressing',
        receivedBytes: 25,
        totalBytes: 100,
        bytesPerSecond: 10
      }
    ])
    lease.finish()
    await broker.releaseToolCall({ runId: OWNER.runId, toolCallId: OWNER.toolCallId })

    item.update(75, 20)
    expect(
      broker.agentDownloadSnapshot({
        runId: OWNER.runId,
        activationId: OWNER.activationId,
        conversationId: OWNER.conversationId
      }).downloads
    ).toMatchObject([{ state: 'progressing', receivedBytes: 75, bytesPerSecond: 20 }])
    expect(item.cancelled).toBe(false)

    await item.complete(Buffer.alloc(100))
    await waitFor(() => records.length === 1)
    const [completed] = broker.agentDownloadSnapshot({
      runId: OWNER.runId,
      activationId: OWNER.activationId,
      conversationId: OWNER.conversationId
    }).downloads
    expect(completed).toMatchObject({
      state: 'completed',
      receivedBytes: 100,
      reference: {
        displayName: 'installer.dmg',
        sizeBytes: 100,
        source: 'agent'
      }
    })
    expect(JSON.stringify(completed)).not.toContain(systemDirectory)
    expect(JSON.stringify(completed)).not.toContain('example.com')
    await broker.shutdown()
  })

  it('reports finalizing until hashing and durable registration publish the reference', async () => {
    let registrationInput: BrowserDownloadRegistrationInput | undefined
    let releaseRegistration!: (record: BrowserDownloadRecord) => void
    const registration = new Promise<BrowserDownloadRecord>((resolve) => {
      releaseRegistration = resolve
    })
    const { broker, guest, session } = await createHarness({
      registerDownload: async (input) => {
        registrationInput = input
        return await registration
      }
    })
    const lease = broker.beginTool({ guest, owner: OWNER })
    lease.markDispatched()
    const item = new FakeDownloadItem('verified.bin', 'application/octet-stream', 4)

    dispatchDownload(session, guest, item)
    await waitForManagedPath(item)
    await expect(lease.settle()).resolves.toEqual([])
    lease.finish()
    await item.complete()
    await waitFor(() => registrationInput !== undefined)

    const [finalizing] = broker.agentDownloadSnapshot({
      runId: OWNER.runId,
      activationId: OWNER.activationId,
      conversationId: OWNER.conversationId
    }).downloads
    expect(finalizing).toMatchObject({ state: 'finalizing' })
    expect(finalizing?.reference).toBeUndefined()

    releaseRegistration({ ...registrationInput!, projectId: 'project-1' })
    await waitFor(
      () =>
        broker.agentDownloadSnapshot({
          runId: OWNER.runId,
          activationId: OWNER.activationId,
          conversationId: OWNER.conversationId
        }).downloads[0]?.state === 'completed'
    )
    await broker.shutdown()
  })

  it('reports an Agent save prompt as awaiting a destination without showing a false transfer', async () => {
    const { broker, guest, root, session } = await createHarness({ askWhereToSave: true })
    const lease = broker.beginTool({ guest, owner: OWNER })
    lease.markDispatched()
    const item = new FakeDownloadItem('prompted.zip', 'application/zip', 50)

    dispatchDownload(session, guest, item)
    await waitFor(() => Boolean(item.saveDialogOptions))
    await expect(lease.settle()).resolves.toEqual([])
    expect(lease.progress?.()).toMatchObject([
      {
        displayName: 'prompted.zip',
        state: 'awaiting_destination',
        receivedBytes: 0,
        bytesPerSecond: 0
      }
    ])
    expect(broker.downloadCenterSnapshot().downloads).toEqual([])
    lease.finish()

    const selectedPath = join(root, 'prompted.zip')
    item.chooseSavePath(selectedPath)
    item.update(10, 5)
    expect(
      broker.agentDownloadSnapshot({
        runId: OWNER.runId,
        activationId: OWNER.activationId,
        conversationId: OWNER.conversationId
      }).downloads
    ).toMatchObject([{ state: 'progressing', receivedBytes: 10, bytesPerSecond: 5 }])
    await item.complete(Buffer.alloc(50))
    await waitFor(
      () =>
        broker.agentDownloadSnapshot({
          runId: OWNER.runId,
          activationId: OWNER.activationId,
          conversationId: OWNER.conversationId
        }).downloads[0]?.state === 'completed'
    )
    await broker.shutdown()
  })

  it('admits a 419 MiB Agent transfer under the production default budget', async () => {
    const { broker, guest, session } = await createHarness()
    const lease = broker.beginTool({ guest, owner: OWNER })
    lease.markDispatched()
    const item = new FakeDownloadItem(
      'WorkBuddy.dmg',
      'application/x-apple-diskimage',
      419 * 1024 * 1024
    )

    dispatchDownload(session, guest, item)
    await waitForManagedPath(item)
    await expect(lease.settle()).resolves.toEqual([])
    expect(item.cancelled).toBe(false)
    expect(lease.progress?.()).toMatchObject([
      { state: 'progressing', totalBytes: 419 * 1024 * 1024 }
    ])
    lease.finish()
    await broker.shutdown()
  })

  it('persists a manual download without assigning Agent ownership', async () => {
    const { broker, guest, records, session } = await createHarness()
    const item = new FakeDownloadItem('manual.txt')

    dispatchDownload(session, guest, item)
    await waitForManagedPath(item)
    await item.complete(Buffer.from('manual'))
    await waitFor(() => records.length === 1)

    expect(records[0]).toMatchObject({
      displayName: 'manual.txt',
      source: 'manual',
      conversationId: null,
      runId: null,
      callId: null
    })
    const [completed] = broker.downloadCenterSnapshot().downloads
    expect(completed).toMatchObject({
      displayName: 'manual.txt',
      state: 'completed',
      canReveal: true,
      canCopyPath: true
    })
    expect(JSON.stringify(completed)).not.toContain(records[0]!.absolutePath)
    expect(broker.downloadCenterResource(completed!.downloadId)?.absolutePath).toBe(
      records[0]!.absolutePath
    )
    await broker.shutdown()
  })

  it('publishes path-free live progress and controls pause, resume, stop, and removal', async () => {
    const { broker, guest, session } = await createHarness()
    const changed: unknown[] = []
    broker.onDownloadCenterChangedEvent((snapshot) => changed.push(snapshot))
    const item = new FakeDownloadItem('large.zip', 'application/zip', 100)

    dispatchDownload(session, guest, item)
    const temporaryPath = await waitForManagedPath(item)
    item.update(25, 10)

    expect(broker.downloadCenterSnapshot()).toMatchObject({
      downloads: [
        {
          displayName: 'large.zip',
          state: 'progressing',
          receivedBytes: 25,
          totalBytes: 100,
          bytesPerSecond: 10,
          canPause: true,
          canResume: false,
          canCancel: true,
          canCopyUrl: true,
          canCopyPath: false
        }
      ]
    })
    expect(JSON.stringify(broker.downloadCenterSnapshot())).not.toContain(temporaryPath)
    expect(JSON.stringify(broker.downloadCenterSnapshot())).not.toContain('example.com')

    const downloadId = broker.downloadCenterSnapshot().downloads[0]!.downloadId
    await expect(broker.performDownloadCenterAction(downloadId, 'pause')).resolves.toBe('performed')
    expect(item.paused).toBe(true)
    expect(broker.downloadCenterSnapshot().downloads[0]).toMatchObject({
      state: 'paused',
      canPause: false,
      canResume: true,
      bytesPerSecond: 0
    })

    await expect(broker.performDownloadCenterAction(downloadId, 'resume')).resolves.toBe(
      'performed'
    )
    expect(item.paused).toBe(false)
    expect(broker.downloadCenterSnapshot().downloads[0]).toMatchObject({
      state: 'progressing',
      canPause: true,
      canResume: false
    })

    await expect(broker.performDownloadCenterAction(downloadId, 'cancel')).resolves.toBe(
      'performed'
    )
    expect(item.cancelled).toBe(true)
    expect(broker.downloadCenterSnapshot().downloads[0]).toMatchObject({
      state: 'cancelled',
      canCancel: false,
      canCopyUrl: true,
      canCopyPath: false
    })
    expect(broker.downloadCenterResource(downloadId)).toMatchObject({
      absolutePath: null,
      sourceUrl: 'https://example.com/report.txt'
    })
    expect(changed.length).toBeGreaterThan(0)

    await expect(broker.performDownloadCenterAction(downloadId, 'remove')).resolves.toBe(
      'performed'
    )
    expect(broker.downloadCenterSnapshot().downloads).toEqual([])
    await broker.shutdown()
  })

  it('uses Electron native save selection when ask-where-to-save is enabled', async () => {
    const { broker, guest, records, root, session, systemDirectory } = await createHarness({
      askWhereToSave: true
    })
    const selectedDirectory = join(root, 'Selected')
    await mkdir(selectedDirectory)
    const selectedPath = join(selectedDirectory, 'chosen-name.txt')
    const item = new FakeDownloadItem('suggested-name.txt')

    dispatchDownload(session, guest, item)
    await waitFor(() => Boolean(item.saveDialogOptions))
    expect(item.savePath).toBeUndefined()
    expect(broker.downloadCenterSnapshot().downloads).toEqual([])
    expect(item.saveDialogOptions).toEqual({
      defaultPath: join(await realpath(systemDirectory), 'suggested-name.txt')
    })

    item.chooseSavePath(selectedPath)
    expect(broker.downloadCenterSnapshot().downloads).toEqual([])
    item.update(1, 1)
    expect(broker.downloadCenterSnapshot().downloads).toMatchObject([
      {
        displayName: 'suggested-name.txt',
        state: 'progressing',
        receivedBytes: 1
      }
    ])
    await item.complete(Buffer.from('chosen'))
    await waitFor(() => records.length === 1)

    expect(records[0]).toMatchObject({
      absolutePath: selectedPath,
      displayName: 'chosen-name.txt',
      source: 'manual'
    })
    await expect(readFile(selectedPath, 'utf8')).resolves.toBe('chosen')
    await broker.shutdown()
  })

  it('does not create history when the native save dialog is cancelled', async () => {
    const { broker, guest, records, session } = await createHarness({ askWhereToSave: true })
    const item = new FakeDownloadItem('cancelled.txt')

    dispatchDownload(session, guest, item)
    await waitFor(() => Boolean(item.saveDialogOptions))
    item.cancelByUser()
    await waitFor(() => broker.snapshot().downloads === 0)

    expect(records).toEqual([])
    expect(item.savePath).toBeUndefined()
    expect(broker.downloadCenterSnapshot().downloads).toEqual([])
    await broker.shutdown()
  })

  it('keeps a user-selected file when durable history registration fails', async () => {
    const { broker, guest, root, session } = await createHarness({
      askWhereToSave: true,
      registerDownload: async () => {
        throw new Error('database unavailable')
      }
    })
    const selectedPath = join(root, 'user-owned.txt')
    const item = new FakeDownloadItem('suggested.txt')

    dispatchDownload(session, guest, item)
    await waitFor(() => Boolean(item.saveDialogOptions))
    item.chooseSavePath(selectedPath)
    await item.complete(Buffer.from('keep me'))
    await waitFor(() => broker.snapshot().downloads === 0)

    await expect(readFile(selectedPath, 'utf8')).resolves.toBe('keep me')
    await broker.shutdown()
  })

  it('persists a valid empty file with its canonical digest', async () => {
    const { broker, guest, records, session } = await createHarness()
    const item = new FakeDownloadItem('empty.txt', 'text/plain', 0)

    dispatchDownload(session, guest, item)
    await waitForManagedPath(item)
    await item.complete(Buffer.alloc(0))
    await waitFor(() => records.length === 1)

    expect(records[0]).toMatchObject({
      displayName: 'empty.txt',
      sizeBytes: 0,
      sha256: 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'
    })
    await broker.shutdown()
  })

  it('allows ordinary Agent browsing without a conversation but rejects an owned download', async () => {
    const { broker, guest, records, session } = await createHarness()
    const ownerWithoutConversation: BrowserDownloadOwner = {
      runId: OWNER.runId,
      activationId: OWNER.activationId,
      capabilityId: OWNER.capabilityId,
      surfaceId: OWNER.surfaceId,
      generation: OWNER.generation,
      toolCallId: OWNER.toolCallId
    }
    const browsingLease = broker.beginTool({ guest, owner: ownerWithoutConversation })
    browsingLease.markDispatched()

    await expect(browsingLease.settle()).resolves.toEqual([])
    browsingLease.finish()

    const downloadLease = broker.beginTool({
      guest,
      owner: { ...ownerWithoutConversation, toolCallId: 'call-2' }
    })
    downloadLease.markDispatched()
    const item = new FakeDownloadItem('ownerless.txt')
    dispatchDownload(session, guest, item)

    expect(item.cancelled).toBe(true)
    await expect(downloadLease.settle()).rejects.toMatchObject({
      code: 'browser.download.registration_failed'
    })
    expect(records).toEqual([])
    downloadLease.finish()
    await broker.shutdown()
  })

  it('never reclassifies a download outside an active tool dispatch window as manual', async () => {
    const { broker, guest, records, session } = await createHarness()
    const lease = broker.beginTool({ guest, owner: OWNER })
    const item = new FakeDownloadItem('late.txt')

    dispatchDownload(session, guest, item)

    expect(item.cancelled).toBe(true)
    await expect(lease.settle()).rejects.toMatchObject({
      code: 'browser.download.outcome_unknown'
    })
    expect(records).toEqual([])
    lease.finish()
    await broker.shutdown()
  })

  it('never overwrites an existing file and records the actual published name', async () => {
    const { broker, guest, records, session, systemDirectory } = await createHarness()
    await writeFile(join(systemDirectory, 'report.txt'), 'existing')
    const item = new FakeDownloadItem('report.txt')

    dispatchDownload(session, guest, item)
    await waitForManagedPath(item)
    await item.complete(Buffer.from('new'))
    await waitFor(() => records.length === 1)

    expect(records[0]?.displayName).toBe('report (1).txt')
    await expect(readFile(join(systemDirectory, 'report.txt'), 'utf8')).resolves.toBe('existing')
    await expect(readFile(join(systemDirectory, 'report (1).txt'), 'utf8')).resolves.toBe('new')
    await broker.shutdown()
  })

  it('pins an in-flight download to the directory selected when it started', async () => {
    const { broker, guest, records, root, session, settings, systemDirectory } =
      await createHarness()
    const customDirectory = join(root, 'Custom')
    await mkdir(customDirectory)
    const item = new FakeDownloadItem('pinned.txt')

    dispatchDownload(session, guest, item)
    await waitForManagedPath(item)
    broker.updateSettings({
      ...settings,
      locationMode: 'custom',
      customDirectory,
      revision: 1,
      updatedAt: 2
    })
    await item.complete()
    await waitFor(() => records.length === 1)

    expect(records[0]?.absolutePath).toBe(join(await realpath(systemDirectory), 'pinned.txt'))
    expect(await readdir(customDirectory)).toEqual([])
    await broker.shutdown()
  })

  it('keeps Settings usable when a persisted custom directory is temporarily unavailable', async () => {
    const { broker, guest, records, root, session, settings } = await createHarness()
    const unavailableDirectory = join(root, 'DisconnectedVolume')
    broker.updateSettings({
      ...settings,
      locationMode: 'custom',
      customDirectory: unavailableDirectory,
      revision: 1,
      updatedAt: 2
    })
    expect(broker.downloadDirectory()).toBe(unavailableDirectory)

    const unavailable = new FakeDownloadItem('unavailable.txt')
    dispatchDownload(session, guest, unavailable)
    expect(unavailable.cancelled).toBe(true)
    expect(unavailable.savePath).toBeUndefined()
    expect(records).toEqual([])

    const recoveredDirectory = join(root, 'Recovered')
    await mkdir(recoveredDirectory)
    broker.updateSettings({
      ...settings,
      locationMode: 'custom',
      customDirectory: recoveredDirectory,
      revision: 2,
      updatedAt: 3
    })
    const recovered = new FakeDownloadItem('recovered.txt')
    dispatchDownload(session, guest, recovered)
    await waitForManagedPath(recovered)
    await recovered.complete(Buffer.from('restored'))
    await waitFor(() => records.length === 1)
    await expect(readFile(join(recoveredDirectory, 'recovered.txt'), 'utf8')).resolves.toBe(
      'restored'
    )
    await broker.shutdown()
  })

  it('rejects an oversized Agent download before publishing bytes', async () => {
    const { broker, guest, records, session, systemDirectory } = await createHarness({
      maxSingleDownloadBytes: 3
    })
    const lease = broker.beginTool({ guest, owner: OWNER })
    lease.markDispatched()
    const item = new FakeDownloadItem('large.bin', 'application/octet-stream', 4)

    dispatchDownload(session, guest, item)
    expect(item.cancelled).toBe(true)
    await expect(lease.settle()).rejects.toMatchObject({ code: 'browser.download.too_large' })
    expect(records).toEqual([])
    expect(await readdir(systemDirectory)).toEqual([])
    lease.finish()
    await broker.shutdown()
  })

  it('removes the published file when durable registration fails', async () => {
    const { broker, guest, session, systemDirectory } = await createHarness({
      registerDownload: async () => {
        throw new Error('database unavailable')
      }
    })
    const lease = broker.beginTool({ guest, owner: OWNER })
    lease.markDispatched()
    const item = new FakeDownloadItem('unregistered.txt')

    dispatchDownload(session, guest, item)
    await waitForManagedPath(item)
    await item.complete()
    await expect(lease.settle()).rejects.toMatchObject({
      code: 'browser.download.registration_failed'
    })
    expect(await readdir(systemDirectory)).toEqual([])
    lease.finish()
    await broker.shutdown()
  })

  it('rejects downloads from guests outside the registered managed surface set', async () => {
    const { broker, records, session } = await createHarness()
    const item = new FakeDownloadItem('foreign.txt')
    const foreignGuest = {
      id: 99,
      session,
      getType: () => 'webview',
      isDestroyed: () => false
    } as unknown as WebContents

    dispatchDownload(session, foreignGuest, item)
    expect(item.cancelled).toBe(true)
    expect(item.savePath).toBeUndefined()
    expect(records).toEqual([])
    await broker.shutdown()
  })

  it('cancels active transfers and removes temporary files during shutdown', async () => {
    const { broker, guest, session } = await createHarness()
    const item = new FakeDownloadItem('active.txt')
    dispatchDownload(session, guest, item)
    const temporaryPath = await waitForManagedPath(item)
    await writeFile(temporaryPath, 'partial')

    await broker.shutdown()

    expect(item.cancelled).toBe(true)
    expect(session.listenerCount('will-download')).toBe(0)
    await expect(access(temporaryPath)).rejects.toBeDefined()
  })
})
