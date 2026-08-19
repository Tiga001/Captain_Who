import { EventEmitter } from 'node:events'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import type { DownloadItem, Session, WebContents } from 'electron'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { BrowserArtifactBroker, type BrowserArtifactOwner } from '../browser/BrowserArtifactBroker'
import { BrowserDownloadBroker } from '../browser/BrowserDownloadBroker'

const temporaryRoots = new Set<string>()
const OWNER: BrowserArtifactOwner = {
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
  paused = false
  resumed = false
  savePath?: string
  receivedBytes = 0

  constructor(
    private readonly filename = 'report.txt',
    private readonly mimeType = 'text/plain',
    private readonly totalBytes = 4
  ) {
    super()
  }

  pause(): void {
    this.paused = true
  }

  resume(): void {
    this.resumed = true
  }

  cancel(): void {
    this.cancelled = true
  }

  setSavePath(path: string): void {
    this.savePath = path
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

  update(receivedBytes: number): void {
    this.receivedBytes = receivedBytes
    this.emit('updated', {}, 'progressing')
  }

  async complete(bytes = Buffer.from('data')): Promise<void> {
    if (!this.savePath) throw new Error('Missing managed save path')
    await writeFile(this.savePath, bytes)
    this.receivedBytes = bytes.byteLength
    this.emit('done', {}, 'completed')
  }
}

afterEach(async () => {
  await Promise.all([...temporaryRoots].map((root) => rm(root, { recursive: true, force: true })))
  temporaryRoots.clear()
})

async function createHarness(options: { maxSingleDownloadBytes?: number } = {}) {
  const parent = await mkdtemp(join(tmpdir(), 'mycopilot-download-test-'))
  temporaryRoots.add(parent)
  const artifacts = new BrowserArtifactBroker({
    rootDirectory: join(parent, 'browser-automation-artifacts')
  })
  const session = new FakeSession()
  const broker = new BrowserDownloadBroker({
    artifacts,
    expectedSession: session as unknown as Session,
    ...options
  })
  broker.install()
  const guest = {
    id: 41,
    session,
    getType: () => 'webview',
    isDestroyed: () => false
  } as unknown as WebContents
  broker.registerGuest({ guest, surfaceId: OWNER.surfaceId, generation: OWNER.generation })
  return { artifacts, broker, guest, session }
}

function dispatchDownload(session: FakeSession, guest: WebContents, item: FakeDownloadItem): void {
  session.emit('will-download', {}, item as unknown as DownloadItem, guest)
}

async function waitForManagedPath(item: FakeDownloadItem): Promise<string> {
  for (let turn = 0; turn < 200; turn += 1) {
    if (item.savePath) return item.savePath
    await new Promise<void>((resolveImmediate) => setImmediate(resolveImmediate))
  }
  throw new Error(`Download was not admitted (cancelled=${String(item.cancelled)})`)
}

describe('BrowserDownloadBroker', () => {
  it('binds a download to the exact tool and waits for Artifact publication', async () => {
    const { artifacts, broker, guest, session } = await createHarness()
    const lease = broker.beginTool({ guest, owner: OWNER })
    await lease.ready?.()
    const preDispatch = new FakeDownloadItem('manual-before-dispatch.txt')
    dispatchDownload(session, guest, preDispatch)
    expect(preDispatch.cancelled).toBe(true)
    expect(preDispatch.savePath).toBeUndefined()
    lease.markDispatched()
    const item = new FakeDownloadItem('../../report.txt')
    dispatchDownload(session, guest, item)
    const managedPath = await waitForManagedPath(item)
    expect(managedPath).toContain('browser-automation-artifacts')
    expect(managedPath).not.toContain('../report.txt')
    expect(item.paused).toBe(false)
    expect(item.resumed).toBe(false)

    let settled = false
    const resultPromise = lease.settle().then((result) => {
      settled = true
      return result
    })
    await new Promise<void>((resolveImmediate) => setImmediate(resolveImmediate))
    expect(settled).toBe(false)
    await item.complete()
    const [artifact] = await resultPromise
    expect(artifact).toEqual(
      expect.objectContaining({
        kind: 'download',
        displayName: 'report.txt',
        mimeType: 'text/plain',
        sizeBytes: 4,
        preview: 'none'
      })
    )
    expect(JSON.stringify(artifact)).not.toContain(managedPath)
    expect(lease.artifacts()).toEqual([artifact])
    lease.finish()
    expect(broker.snapshot()).toEqual({ downloads: 0, guests: 1, tools: 0, activeBytes: 0 })
    expect(artifacts.snapshot().artifacts).toBe(1)
    await broker.shutdown()
    await artifacts.shutdown()
  })

  it('cancels downloads that have no exact active Agent owner', async () => {
    const { artifacts, broker, guest, session } = await createHarness()
    const item = new FakeDownloadItem()
    dispatchDownload(session, guest, item)
    expect(item.cancelled).toBe(true)
    expect(item.savePath).toBeUndefined()
    expect(artifacts.snapshot().artifacts).toBe(0)

    const unrelatedGuest = { ...guest, id: 99 } as unknown as WebContents
    const unrelatedItem = new FakeDownloadItem('unrelated.txt')
    dispatchDownload(session, unrelatedGuest, unrelatedItem)
    expect(unrelatedItem.cancelled).toBe(true)
    expect(unrelatedItem.savePath).toBeUndefined()
    await broker.shutdown()
    await artifacts.shutdown()
  })

  it('reports an advertised or progressing size overflow before tool success', async () => {
    const { artifacts, broker, guest, session } = await createHarness({
      maxSingleDownloadBytes: 3
    })
    const lease = broker.beginTool({ guest, owner: OWNER })
    await lease.ready?.()
    lease.markDispatched()
    const advertisedTooLarge = new FakeDownloadItem('large.bin', 'application/octet-stream', 4)
    dispatchDownload(session, guest, advertisedTooLarge)
    expect(advertisedTooLarge.cancelled).toBe(true)
    await expect(lease.settle()).rejects.toMatchObject({
      code: 'browser.download.too_large',
      dispatchCertainty: 'possibly_dispatched'
    })
    lease.finish()

    const second = broker.beginTool({
      guest,
      owner: { ...OWNER, toolCallId: 'call-2' }
    })
    await second.ready?.()
    second.markDispatched()
    const growing = new FakeDownloadItem('grow.bin', 'application/octet-stream', 0)
    dispatchDownload(session, guest, growing)
    await waitForManagedPath(growing)
    growing.update(4)
    await expect(second.settle()).rejects.toMatchObject({ code: 'browser.download.too_large' })
    expect(growing.cancelled).toBe(true)
    second.finish()
    expect(artifacts.snapshot().artifacts).toBe(0)
    await broker.shutdown()
    await artifacts.shutdown()
  })

  it('propagates caller cancellation and target close without publishing a partial file', async () => {
    const { artifacts, broker, guest, session } = await createHarness()
    const controller = new AbortController()
    const lease = broker.beginTool({ guest, owner: OWNER, signal: controller.signal })
    await lease.ready?.()
    lease.markDispatched()
    const item = new FakeDownloadItem()
    dispatchDownload(session, guest, item)
    await waitForManagedPath(item)
    controller.abort('cancelled')
    await expect(lease.settle()).rejects.toMatchObject({ code: 'browser.download.cancelled' })
    expect(item.cancelled).toBe(true)
    lease.finish()

    const targetLease = broker.beginTool({
      guest,
      owner: { ...OWNER, toolCallId: 'call-2' }
    })
    await targetLease.ready?.()
    targetLease.markDispatched()
    const targetItem = new FakeDownloadItem('target.txt')
    dispatchDownload(session, guest, targetItem)
    await waitForManagedPath(targetItem)
    await broker.releaseSurface({ surfaceId: OWNER.surfaceId, generation: OWNER.generation })
    await expect(targetLease.settle()).rejects.toMatchObject({
      code: 'browser.download.target_closed'
    })
    expect(targetItem.cancelled).toBe(true)
    expect(artifacts.snapshot().artifacts).toBe(0)
    targetLease.finish()
    await broker.shutdown()
    await artifacts.shutdown()
  })

  it('does not resume when cancellation wins during asynchronous Artifact reservation', async () => {
    const { artifacts, broker, guest, session } = await createHarness()
    const originalOpenSession = artifacts.openSession.bind(artifacts)
    let releaseReservation!: () => void
    const reservationGate = new Promise<void>((resolveGate) => {
      releaseReservation = resolveGate
    })
    vi.spyOn(artifacts, 'openSession').mockImplementation(async () => {
      await reservationGate
      return originalOpenSession()
    })
    const controller = new AbortController()
    const lease = broker.beginTool({ guest, owner: OWNER, signal: controller.signal })
    lease.markDispatched()
    const item = new FakeDownloadItem('racing.txt')
    dispatchDownload(session, guest, item)
    controller.abort('cancelled')
    const settled = lease.settle()
    releaseReservation()
    await expect(settled).rejects.toMatchObject({ code: 'browser.download.cancelled' })
    expect(item.cancelled).toBe(true)
    expect(item.resumed).toBe(false)
    expect(item.savePath).toBeUndefined()
    expect(artifacts.snapshot()).toMatchObject({ artifacts: 0, reservations: 0, sessions: 0 })
    lease.finish()
    await broker.shutdown()
    await artifacts.shutdown()
  })

  it('does not resume when cancellation wins after assigning the managed save path', async () => {
    const { artifacts, broker, guest, session } = await createHarness()
    const controller = new AbortController()
    const lease = broker.beginTool({ guest, owner: OWNER, signal: controller.signal })
    await lease.ready?.()
    lease.markDispatched()
    const item = new FakeDownloadItem('late-racing.txt')
    const originalSetSavePath = item.setSavePath.bind(item)
    vi.spyOn(item, 'setSavePath').mockImplementation((path) => {
      originalSetSavePath(path)
      controller.abort('cancelled-after-path')
    })

    dispatchDownload(session, guest, item)
    await expect(lease.settle()).rejects.toMatchObject({ code: 'browser.download.cancelled' })
    expect(item.savePath).toContain('browser-automation-artifacts')
    expect(item.cancelled).toBe(true)
    expect(item.resumed).toBe(false)
    expect(artifacts.snapshot()).toMatchObject({ artifacts: 0, reservations: 0, sessions: 0 })
    lease.finish()
    await broker.shutdown()
    await artifacts.shutdown()
  })

  it('cancels and fences the exact Tool call without affecting the next call', async () => {
    const { artifacts, broker, guest, session } = await createHarness()
    const lease = broker.beginTool({ guest, owner: OWNER })
    await lease.ready?.()
    lease.markDispatched()
    const item = new FakeDownloadItem('cancelled-tool.txt')
    dispatchDownload(session, guest, item)
    await waitForManagedPath(item)

    await broker.releaseToolCall({ runId: OWNER.runId, toolCallId: OWNER.toolCallId })
    await expect(lease.settle()).rejects.toMatchObject({ code: 'browser.download.cancelled' })
    expect(item.cancelled).toBe(true)
    expect(() => broker.beginTool({ guest, owner: OWNER })).toThrowError(
      expect.objectContaining({ code: 'browser.download.closed' })
    )
    lease.finish()

    const next = broker.beginTool({
      guest,
      owner: { ...OWNER, toolCallId: 'call-2' }
    })
    await next.ready?.()
    next.finish()
    await broker.shutdown()
    await artifacts.shutdown()
  })

  it('retains completed run Artifacts on done, but revoke removes them and fences late writes', async () => {
    const { artifacts, broker, guest, session } = await createHarness()
    const lease = broker.beginTool({ guest, owner: OWNER })
    await lease.ready?.()
    lease.markDispatched()
    const item = new FakeDownloadItem()
    dispatchDownload(session, guest, item)
    await waitForManagedPath(item)
    await item.complete()
    const [artifact] = await lease.settle()
    lease.finish()
    await broker.finalizeRun(OWNER.runId)
    expect(artifacts.snapshot().artifacts).toBe(1)
    await expect(artifacts.readPreview(artifact)).rejects.toMatchObject({
      code: 'browser.artifact.preview_unavailable'
    })
    await broker.releaseCapability(OWNER.activationId)
    expect(artifacts.snapshot().artifacts).toBe(0)
    expect(() => broker.beginTool({ guest, owner: OWNER })).toThrowError(
      expect.objectContaining({ code: 'browser.download.closed' })
    )
    await expect(
      artifacts.storeText({
        owner: OWNER,
        kind: 'text',
        mimeType: 'text/plain',
        suggestedFileName: 'late.txt',
        text: 'late'
      })
    ).rejects.toMatchObject({ code: 'browser.artifact.closed' })
    await broker.shutdown()
    await artifacts.shutdown()
  })

  it('removes listeners and cancels all active transfers at shutdown', async () => {
    const { artifacts, broker, guest, session } = await createHarness()
    const lease = broker.beginTool({ guest, owner: OWNER })
    await lease.ready?.()
    lease.markDispatched()
    const item = new FakeDownloadItem()
    dispatchDownload(session, guest, item)
    await waitForManagedPath(item)
    expect(session.listenerCount('will-download')).toBe(1)
    await broker.shutdown()
    expect(session.listenerCount('will-download')).toBe(0)
    expect(item.cancelled).toBe(true)
    await expect(lease.settle()).rejects.toMatchObject({ code: 'browser.download.cancelled' })
    await artifacts.shutdown()
  })
})
