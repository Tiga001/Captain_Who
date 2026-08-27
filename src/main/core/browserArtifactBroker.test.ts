import { lstat, mkdir, mkdtemp, readFile, readdir, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'

import {
  BrowserArtifactBroker,
  BrowserArtifactBrokerError,
  type BrowserArtifactBrokerClock,
  type BrowserArtifactOwner
} from '../browser/BrowserArtifactBroker'

const temporaryRoots = new Set<string>()
const OWNER: BrowserArtifactOwner = {
  runId: 'run-1',
  activationId: 'activation-1',
  capabilityId: 'browser_automation',
  surfaceId: 'surface-1',
  generation: 1,
  toolCallId: 'call-1'
}

afterEach(async () => {
  await Promise.all([...temporaryRoots].map((root) => rm(root, { recursive: true, force: true })))
  temporaryRoots.clear()
})

async function createBroker(
  options: Partial<ConstructorParameters<typeof BrowserArtifactBroker>[0]> = {}
): Promise<{ broker: BrowserArtifactBroker; root: string }> {
  const parent = await mkdtemp(join(tmpdir(), 'mycopilot-artifact-test-'))
  temporaryRoots.add(parent)
  const root = join(parent, 'browser-automation-artifacts')
  return { broker: new BrowserArtifactBroker({ rootDirectory: root, ...options }), root }
}

describe('BrowserArtifactBroker', () => {
  it('publishes an immutable path-free reference from a private output session', async () => {
    const { broker, root } = await createBroker()
    const session = await broker.openSession()
    const reservation = await session.reserveFile({
      owner: OWNER,
      kind: 'image',
      mimeType: 'image/png',
      suggestedFileName: 'page.png'
    })
    expect(session.outputDirectory).toContain(root)
    expect(reservation.managedPath).toBe(join(session.outputDirectory, reservation.fileName))
    await writeFile(reservation.managedPath, Uint8Array.from([137, 80, 78, 71]), { mode: 0o600 })

    const reference = await reservation.commit()
    expect(reference).toEqual(
      expect.objectContaining({
        artifactId: expect.stringMatching(/^browser-artifact:/),
        displayName: 'page.png',
        kind: 'image',
        mimeType: 'image/png',
        owner: 'browser_automation',
        preview: 'image',
        sizeBytes: 4
      })
    )
    expect(JSON.stringify(reference)).not.toContain(root)
    expect(JSON.stringify(reference)).not.toContain('outputDirectory')
    await expect(readFile(reservation.managedPath)).rejects.toMatchObject({ code: 'ENOENT' })

    const preview = await broker.readPreview(reference)
    expect(preview.artifact).toEqual(reference)
    expect([...preview.bytes]).toEqual([137, 80, 78, 71])
    await session.close()
    expect(broker.snapshot()).toEqual({
      artifacts: 1,
      bytes: 4,
      reservations: 0,
      sessions: 0,
      runs: 1
    })
    await broker.shutdown()
    await expect(lstat(root)).rejects.toMatchObject({ code: 'ENOENT' })
  })

  it('rejects traversal, absolute names, symlinks, hard links, and incompatible MIME types', async () => {
    const { broker } = await createBroker()
    const session = await broker.openSession()
    for (const suggestedFileName of ['../secret.png', '/tmp/secret.png', '..\\secret.png']) {
      await expect(
        session.reserveFile({
          owner: OWNER,
          kind: 'image',
          mimeType: 'image/png',
          suggestedFileName
        })
      ).rejects.toMatchObject({ code: 'browser.artifact.invalid_name' })
    }
    await expect(
      session.reserveFile({
        owner: OWNER,
        kind: 'pdf',
        mimeType: 'text/html',
        suggestedFileName: 'page.pdf'
      })
    ).rejects.toMatchObject({ code: 'browser.artifact.invalid_type' })

    const outside = join(session.outputDirectory, '..', 'outside.png')
    await writeFile(outside, 'not an image')
    const reservation = await session.reserveFile({
      owner: OWNER,
      kind: 'image',
      mimeType: 'image/png',
      suggestedFileName: 'page.png'
    })
    await symlink(outside, reservation.managedPath)
    await expect(reservation.commit()).rejects.toMatchObject({
      code: 'browser.artifact.invalid_file'
    })
    expect(broker.snapshot().artifacts).toBe(0)
    await session.close()
    await broker.shutdown()
  })

  it('enforces per-file, per-run, count, and preview budgets', async () => {
    const { broker, root } = await createBroker({
      maxArtifactBytes: 4,
      maxArtifactsPerRun: 1,
      maxRunBytes: 4,
      maxTotalBytes: 4,
      maxTextPreviewBytes: 2
    })
    await expect(
      broker.storeText({
        owner: OWNER,
        kind: 'text',
        mimeType: 'text/plain',
        suggestedFileName: 'large.txt',
        text: '12345'
      })
    ).rejects.toMatchObject({ code: 'browser.artifact.too_large' })
    await expect(lstat(root)).rejects.toMatchObject({ code: 'ENOENT' })

    const artifact = await broker.storeText({
      owner: OWNER,
      kind: 'text',
      mimeType: 'text/plain',
      suggestedFileName: 'small.txt',
      text: '123'
    })
    expect(artifact.preview).toBe('none')
    await expect(broker.readPreview(artifact)).rejects.toMatchObject({
      code: 'browser.artifact.preview_unavailable'
    })
    await expect(
      broker.storeText({
        owner: OWNER,
        kind: 'text',
        mimeType: 'text/plain',
        suggestedFileName: 'second.txt',
        text: '1'
      })
    ).rejects.toMatchObject({ code: 'browser.artifact.capacity' })
    await broker.shutdown()
  })

  it('keeps sensitive JSON as metadata-only even when it is small enough to preview', async () => {
    const { broker, root } = await createBroker()
    const artifact = await broker.storeText({
      owner: OWNER,
      kind: 'json',
      mimeType: 'application/json',
      suggestedFileName: 'storage-state.json',
      allowPreview: false,
      text: JSON.stringify({
        cookies: [{ name: 'session', value: 'PRIVATE_COOKIE_CANARY' }],
        origins: [{ origin: 'https://fixture.example', localStorage: [] }]
      })
    })
    expect(artifact.preview).toBe('none')
    expect(JSON.stringify(artifact)).not.toContain('PRIVATE_COOKIE_CANARY')
    await expect(broker.readPreview(artifact)).rejects.toMatchObject({
      code: 'browser.artifact.preview_unavailable'
    })
    const exportDirectory = join(root, '..', 'exports')
    await mkdir(exportDirectory)
    const destination = join(exportDirectory, 'storage-state-copy.json')
    await expect(broker.exportArtifact(artifact, destination)).resolves.toEqual({
      displayName: 'storage-state-copy.json'
    })
    expect(await readFile(destination, 'utf8')).toContain('PRIVATE_COOKIE_CANARY')
    expect((await lstat(destination)).mode & 0o777).toBe(0o600)
    await broker.shutdown()
  })

  it('requires an exact live reference and fails closed for traversal and symlink destinations', async () => {
    const { broker, root } = await createBroker()
    const artifact = await broker.storeText({
      owner: OWNER,
      kind: 'snapshot',
      mimeType: 'text/plain',
      suggestedFileName: 'snapshot.txt',
      text: 'trusted snapshot'
    })
    const exportDirectory = join(root, '..', 'exports')
    await mkdir(exportDirectory)

    await expect(
      broker.exportArtifact(
        { ...artifact, displayName: 'forged.txt' },
        join(exportDirectory, 'forged.txt')
      )
    ).rejects.toMatchObject({ code: 'browser.artifact.identity_mismatch' })
    await expect(
      broker.exportArtifact(artifact, `${exportDirectory}/nested/../traversal.txt`)
    ).rejects.toMatchObject({ code: 'browser.artifact.invalid_name' })
    await expect(
      broker.exportArtifact(artifact, join(root, 'objects', 'forbidden.txt'))
    ).rejects.toMatchObject({ code: 'browser.artifact.invalid_file' })

    const symlinkTarget = join(exportDirectory, 'target.txt')
    const symlinkDestination = join(exportDirectory, 'copy.txt')
    await writeFile(symlinkTarget, 'must remain unchanged')
    await symlink(symlinkTarget, symlinkDestination)
    await expect(broker.exportArtifact(artifact, symlinkDestination)).rejects.toMatchObject({
      code: 'browser.artifact.invalid_file'
    })
    expect(await readFile(symlinkTarget, 'utf8')).toBe('must remain unchanged')
    expect((await lstat(symlinkDestination)).isSymbolicLink()).toBe(true)
    expect(await readdir(exportDirectory)).toEqual(['copy.txt', 'target.txt'])
    await broker.shutdown()
  })

  it('rechecks the source hash and expiry immediately before an export', async () => {
    let now = 1_000
    const clock: BrowserArtifactBrokerClock = {
      now: () => now,
      setTimeout: (handler, delayMs) => setTimeout(handler, delayMs),
      clearTimeout: (timer) => clearTimeout(timer)
    }
    const { broker, root } = await createBroker({ clock, ttlMs: 100 })
    const first = await broker.storeText({
      owner: OWNER,
      kind: 'text',
      mimeType: 'text/plain',
      suggestedFileName: 'first.txt',
      text: 'first'
    })
    const exportDirectory = join(root, '..', 'exports')
    await mkdir(exportDirectory)
    const [objectName] = await readdir(join(root, 'objects'))
    await writeFile(join(root, 'objects', objectName), 'other')
    await expect(
      broker.exportArtifact(first, join(exportDirectory, 'corrupt.txt'))
    ).rejects.toMatchObject({ code: 'browser.artifact.identity_mismatch' })

    const second = await broker.storeText({
      owner: { ...OWNER, toolCallId: 'call-2' },
      kind: 'text',
      mimeType: 'text/plain',
      suggestedFileName: 'second.txt',
      text: 'second'
    })
    now = second.expiresAt
    await expect(
      broker.exportArtifact(second, join(exportDirectory, 'expired.txt'))
    ).rejects.toMatchObject({ code: 'browser.artifact.expired' })
    expect(await readdir(exportDirectory)).toEqual([])
    await broker.shutdown()
  })

  it('does not export when TTL expires after the last asynchronous verification', async () => {
    let now = 2_000
    const clock: BrowserArtifactBrokerClock = {
      now: () => now,
      setTimeout: (handler, delayMs) => setTimeout(handler, delayMs),
      clearTimeout: (timer) => clearTimeout(timer)
    }
    const { broker, root } = await createBroker({
      clock,
      ttlMs: 100,
      beforeExportFinalPublish: () => {
        now += 100
      }
    })
    const artifact = await broker.storeText({
      owner: OWNER,
      kind: 'text',
      mimeType: 'text/plain',
      suggestedFileName: 'expires-at-publish.txt',
      text: 'bounded output'
    })
    const exportDirectory = join(root, '..', 'exports-final-expiry')
    await mkdir(exportDirectory)

    await expect(
      broker.exportArtifact(artifact, join(exportDirectory, 'must-not-exist.txt'))
    ).rejects.toMatchObject({ code: 'browser.artifact.expired' })
    expect(await readdir(exportDirectory)).toEqual([])
    await broker.shutdown()
  })

  it('does not publish or retain a temporary export across revocation and shutdown races', async () => {
    const fences = ['release', 'shutdown'] as const
    for (const fence of fences) {
      let enterExport!: () => void
      const exportEntered = new Promise<void>((resolveEntered) => {
        enterExport = resolveEntered
      })
      let releaseExport!: () => void
      const exportGate = new Promise<void>((resolveGate) => {
        releaseExport = resolveGate
      })
      const { broker, root } = await createBroker({
        beforeExportPublish: async () => {
          enterExport()
          await exportGate
        }
      })
      const artifact = await broker.storeText({
        owner: OWNER,
        kind: 'snapshot',
        mimeType: 'text/plain',
        suggestedFileName: `${fence}.txt`,
        text: fence
      })
      const exportDirectory = join(root, '..', `exports-${fence}`)
      await mkdir(exportDirectory)
      const exporting = broker.exportArtifact(artifact, join(exportDirectory, `${fence}.txt`))
      await exportEntered
      const fencing =
        fence === 'release' ? broker.releaseCapability(OWNER.activationId) : broker.shutdown()
      releaseExport()
      await expect(exporting, fence).rejects.toMatchObject({ code: 'browser.artifact.closed' })
      await fencing
      expect(await readdir(exportDirectory), fence).toEqual([])
      if (fence === 'release') await broker.shutdown()
    }
  })

  it('requires an exact reference and preserves committed previews across target close', async () => {
    const { broker } = await createBroker()
    const first = await broker.storeText({
      owner: OWNER,
      kind: 'snapshot',
      mimeType: 'text/plain',
      suggestedFileName: 'snapshot.txt',
      text: 'snapshot'
    })
    await expect(broker.readPreview({ ...first, displayName: 'other.txt' })).rejects.toMatchObject({
      code: 'browser.artifact.identity_mismatch'
    })
    await broker.releaseSurface({ surfaceId: OWNER.surfaceId, generation: OWNER.generation + 1 })
    expect(broker.snapshot().artifacts).toBe(1)
    await broker.releaseSurface({ surfaceId: OWNER.surfaceId, generation: OWNER.generation })
    expect(broker.snapshot().artifacts).toBe(1)
    await expect(broker.readPreview(first)).resolves.toMatchObject({ artifact: first })
    await expect(
      broker.storeText({
        owner: OWNER,
        kind: 'text',
        mimeType: 'text/plain',
        suggestedFileName: 'late-target.txt',
        text: 'late'
      })
    ).rejects.toMatchObject({ code: 'browser.artifact.closed' })

    await broker.storeText({
      owner: { ...OWNER, surfaceId: 'surface-2' },
      kind: 'console',
      mimeType: 'text/plain',
      suggestedFileName: 'console.txt',
      text: 'safe output'
    })
    await broker.releaseRun(OWNER.runId)
    expect(broker.snapshot().artifacts).toBe(0)
    await broker.shutdown()
  })

  it('expires Artifact bytes with an injectable clock', async () => {
    let now = 1_000
    const timers = new Set<ReturnType<typeof setTimeout>>()
    const clock: BrowserArtifactBrokerClock = {
      now: () => now,
      setTimeout: (handler, delayMs) => {
        const timer = setTimeout(handler, delayMs)
        timers.add(timer)
        return timer
      },
      clearTimeout: (timer) => {
        clearTimeout(timer)
        timers.delete(timer)
      }
    }
    const { broker } = await createBroker({ clock, ttlMs: 100 })
    const artifact = await broker.storeText({
      owner: OWNER,
      kind: 'text',
      mimeType: 'text/plain',
      suggestedFileName: 'ttl.txt',
      text: 'ttl'
    })
    expect(artifact.expiresAt).toBe(1_100)
    now = 1_101
    await expect(broker.sweepExpired()).resolves.toBe(1)
    expect(broker.snapshot()).toEqual({
      artifacts: 0,
      bytes: 0,
      reservations: 0,
      sessions: 0,
      runs: 0
    })
    await broker.shutdown()
    for (const timer of timers) clearTimeout(timer)
  })

  it('keeps committed previews after run finalization and rejects new run writes', async () => {
    const { broker } = await createBroker()
    const artifact = await broker.storeText({
      owner: OWNER,
      kind: 'snapshot',
      mimeType: 'text/plain',
      suggestedFileName: 'final.txt',
      text: 'final snapshot'
    })
    await broker.finalizeRun(OWNER.runId)
    await expect(broker.readPreview(artifact)).resolves.toMatchObject({ artifact })
    await expect(
      broker.storeText({
        owner: OWNER,
        kind: 'text',
        mimeType: 'text/plain',
        suggestedFileName: 'late.txt',
        text: 'late'
      })
    ).rejects.toMatchObject({ code: 'browser.artifact.closed' })
    await broker.shutdown()
  })

  it('fences and removes only the exact cancelled Tool call output', async () => {
    const { broker } = await createBroker()
    const cancelled = await broker.storeText({
      owner: OWNER,
      kind: 'snapshot',
      mimeType: 'text/plain',
      suggestedFileName: 'cancelled.txt',
      text: 'cancelled'
    })
    const retainedOwner = { ...OWNER, toolCallId: 'call-2' }
    const retained = await broker.storeText({
      owner: retainedOwner,
      kind: 'snapshot',
      mimeType: 'text/plain',
      suggestedFileName: 'retained.txt',
      text: 'retained'
    })

    await broker.releaseToolCall({ runId: OWNER.runId, toolCallId: OWNER.toolCallId })
    await expect(broker.readPreview(cancelled)).rejects.toMatchObject({
      code: 'browser.artifact.not_found'
    })
    await expect(broker.readPreview(retained)).resolves.toMatchObject({ artifact: retained })
    await expect(
      broker.storeText({
        owner: OWNER,
        kind: 'text',
        mimeType: 'text/plain',
        suggestedFileName: 'late.txt',
        text: 'late'
      })
    ).rejects.toMatchObject({ code: 'browser.artifact.closed' })
    await broker.shutdown()
  })

  it('rejects zero-byte and malformed JSON publications', async () => {
    const { broker } = await createBroker()
    const session = await broker.openSession()
    const empty = await session.reserveFile({
      owner: OWNER,
      kind: 'video',
      mimeType: 'video/mp4',
      suggestedFileName: 'empty.bin'
    })
    await writeFile(empty.managedPath, new Uint8Array())
    await expect(empty.commit()).rejects.toBeInstanceOf(BrowserArtifactBrokerError)

    await expect(
      broker.storeText({
        owner: OWNER,
        kind: 'json',
        mimeType: 'application/json',
        suggestedFileName: 'broken.json',
        text: '{broken'
      })
    ).rejects.toMatchObject({ code: 'browser.artifact.invalid_file' })
    await session.close()
    await broker.shutdown()
  })

  it('fences an in-flight commit before shutdown and leaves no late Artifact or file', async () => {
    let enterPublish!: () => void
    const publishEntered = new Promise<void>((resolveEntered) => {
      enterPublish = resolveEntered
    })
    let releasePublish!: () => void
    const publishGate = new Promise<void>((resolveGate) => {
      releasePublish = resolveGate
    })
    const { broker, root } = await createBroker({
      beforePublish: async () => {
        enterPublish()
        await publishGate
      }
    })
    const session = await broker.openSession()
    const reservation = await session.reserveFile({
      owner: OWNER,
      kind: 'text',
      mimeType: 'text/plain',
      suggestedFileName: 'racing.txt'
    })
    await writeFile(reservation.managedPath, 'racing')
    const committing = reservation.commit()
    await publishEntered
    const shuttingDown = broker.shutdown()
    releasePublish()
    await expect(committing).rejects.toMatchObject({ code: 'browser.artifact.closed' })
    await shuttingDown
    expect(broker.snapshot()).toEqual({
      artifacts: 0,
      bytes: 0,
      reservations: 0,
      sessions: 0,
      runs: 0
    })
    await expect(lstat(root)).rejects.toMatchObject({ code: 'ENOENT' })
  })

  it('rechecks every owner fence after the final rename and removes the unpublished object', async () => {
    const fences: ReadonlyArray<{
      name: string
      apply(broker: BrowserArtifactBroker): Promise<void>
    }> = [
      { name: 'finalizeRun', apply: (broker) => broker.finalizeRun(OWNER.runId) },
      { name: 'releaseRun', apply: (broker) => broker.releaseRun(OWNER.runId) },
      {
        name: 'releaseCapability',
        apply: (broker) => broker.releaseCapability(OWNER.activationId)
      },
      {
        name: 'releaseToolCall',
        apply: (broker) =>
          broker.releaseToolCall({ runId: OWNER.runId, toolCallId: OWNER.toolCallId })
      },
      {
        name: 'releaseSurface',
        apply: (broker) =>
          broker.releaseSurface({ surfaceId: OWNER.surfaceId, generation: OWNER.generation })
      }
    ]

    for (const fence of fences) {
      let enterPublish!: () => void
      const publishEntered = new Promise<void>((resolveEntered) => {
        enterPublish = resolveEntered
      })
      let releasePublish!: () => void
      const publishGate = new Promise<void>((resolveGate) => {
        releasePublish = resolveGate
      })
      const { broker, root } = await createBroker({
        beforePublish: async () => {
          enterPublish()
          await publishGate
        }
      })
      const session = await broker.openSession()
      const reservation = await session.reserveFile({
        owner: OWNER,
        kind: 'text',
        mimeType: 'text/plain',
        suggestedFileName: `${fence.name}.txt`
      })
      await writeFile(reservation.managedPath, fence.name)

      const committing = reservation.commit()
      await publishEntered
      const revoking = fence.apply(broker)
      releasePublish()

      await expect(committing, fence.name).rejects.toMatchObject({
        code: 'browser.artifact.closed'
      })
      await revoking
      expect(broker.snapshot(), fence.name).toMatchObject({
        artifacts: 0,
        bytes: 0,
        reservations: 0,
        runs: 0
      })
      await expect(readdir(join(root, 'objects')), fence.name).resolves.toEqual([])
      await session.close()
      await broker.shutdown()
    }
  })
})
