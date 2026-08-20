import {
  lstat,
  mkdtemp,
  mkdir,
  readFile,
  readdir,
  rm,
  stat,
  symlink,
  writeFile
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'

import {
  BrowserFileBroker,
  BrowserFileBrokerError,
  type BrowserFileBrokerClock,
  type BrowserFileOwner
} from '../browser/BrowserFileBroker'

const roots: string[] = []
const owner: BrowserFileOwner = {
  runId: 'run-1',
  activationId: 'activation-1',
  capabilityId: 'browser_automation',
  toolCallId: 'call-1'
}
const EMPTY_SNAPSHOT = {
  handles: 0,
  bytes: 0,
  retained: { leases: 0, files: 0, bytes: 0 }
}

interface Deferred<T> {
  promise: Promise<T>
  resolve(value: T): void
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise
  })
  return { promise, resolve }
}

afterEach(async () => {
  await Promise.allSettled(
    roots.splice(0).map((root) => rm(root, { recursive: true, force: true }))
  )
})

async function fixture(
  files: readonly string[],
  options: Partial<ConstructorParameters<typeof BrowserFileBroker>[0]> = {}
): Promise<{ broker: BrowserFileBroker; root: string }> {
  const root = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-test-'))
  roots.push(root)
  const broker = new BrowserFileBroker({
    rootDirectory: join(root, 'browser-automation-files'),
    selectionProvider: { selectFiles: async () => files },
    ...options
  })
  return { broker, root }
}

async function expectNoBrokerResidue(broker: BrowserFileBroker, root: string): Promise<void> {
  expect(broker.snapshot()).toEqual(EMPTY_SNAPSHOT)
  const privateRoot = join(root, 'browser-automation-files')
  const entries = await readdir(privateRoot).catch((error: NodeJS.ErrnoException) => {
    if (error.code === 'ENOENT') return []
    throw error
  })
  expect(entries).toEqual([])
}

describe('BrowserFileBroker', () => {
  it('refuses to own or sweep a non-dedicated directory', () => {
    expect(
      () =>
        new BrowserFileBroker({
          rootDirectory: join(tmpdir(), 'user-documents'),
          selectionProvider: { selectFiles: async () => [] }
        })
    ).toThrow(BrowserFileBrokerError)
  })

  it('freezes a native selection behind an opaque path-free handle', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-source-'))
    roots.push(root)
    const source = join(root, 'secret-canary.txt')
    await writeFile(source, 'selected contents')
    const { broker } = await fixture([source])

    const [reference] = await broker.selectForRead({ owner, multiple: false })
    expect(reference).toMatchObject({
      schemaVersion: 1,
      displayName: 'secret-canary.txt',
      mimeType: 'text/plain',
      sizeBytes: 17
    })
    expect(reference.handle).toMatch(/^browser-file:/)
    expect(JSON.stringify(reference)).not.toContain(source)

    const resolution = await broker.resolveForRead({ owner, handles: [reference.handle] })
    expect(resolution.paths).toHaveLength(1)
    expect(resolution.paths[0]).not.toBe(source)
    expect(await readFile(resolution.paths[0], 'utf8')).toBe('selected contents')
    expect(resolution.fileRevisionDigest).toMatch(/^sha256:[0-9a-f]{64}$/)
  })

  it('freezes an already-authorized absolute workspace path without invoking the picker', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-source-'))
    roots.push(root)
    const source = join(root, '浙江大学2026年招生资料汇编.pptx')
    await writeFile(source, 'fixture-presentation')
    const selectFiles = vi.fn(async () => null)
    const { broker } = await fixture([], {
      selectionProvider: { selectFiles }
    })

    const [reference] = await broker.freezeResolvedForRead({ owner, paths: [source] })
    expect(selectFiles).not.toHaveBeenCalled()
    expect(reference).toMatchObject({
      displayName: '浙江大学2026年招生资料汇编.pptx',
      sizeBytes: 20
    })
    expect(JSON.stringify(reference)).not.toContain(source)
    const lease = await broker.consumeForRead({ owner, handles: [reference.handle] })
    expect(await readFile(lease.paths[0], 'utf8')).toBe('fixture-presentation')
    await lease.finish()
    expect(broker.snapshot()).toEqual(EMPTY_SNAPSHOT)
  })

  it('rejects relative and symbolic-link paths at the resolved-path Main boundary', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-source-'))
    roots.push(root)
    const source = join(root, 'source.txt')
    const link = join(root, 'link.txt')
    await writeFile(source, 'payload')
    await symlink(source, link)
    const { broker } = await fixture([])

    await expect(
      broker.freezeResolvedForRead({ owner, paths: ['relative.txt'] })
    ).rejects.toMatchObject({ code: 'browser.file.invalid_file' })
    await expect(broker.freezeResolvedForRead({ owner, paths: [link] })).rejects.toMatchObject({
      code: 'browser.file.symlink_forbidden'
    })
    expect(broker.snapshot()).toEqual(EMPTY_SNAPSHOT)
  })

  it('rejects symlinks without reading their target', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-source-'))
    roots.push(root)
    const target = join(root, 'target.txt')
    const link = join(root, 'link.txt')
    await writeFile(target, 'do-not-read')
    await symlink(target, link)
    const { broker } = await fixture([link])

    await expect(broker.selectForRead({ owner, multiple: false })).rejects.toMatchObject({
      code: 'browser.file.symlink_forbidden'
    })
    expect(broker.snapshot()).toEqual(EMPTY_SNAPSHOT)
  })

  it('invalidates the handle if the selected source revision drifts', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-source-'))
    roots.push(root)
    const source = join(root, 'input.txt')
    await writeFile(source, 'first')
    const { broker } = await fixture([source])
    const [reference] = await broker.selectForRead({ owner, multiple: false })
    await writeFile(source, 'changed source')

    await expect(
      broker.resolveForRead({ owner, handles: [reference.handle] })
    ).rejects.toMatchObject({ code: 'browser.file.changed' })
    expect(broker.snapshot()).toEqual(EMPTY_SNAPSHOT)
  })

  it('binds handles to the task activation while allowing a later approved tool call to consume it', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-source-'))
    roots.push(root)
    const source = join(root, 'input.txt')
    await writeFile(source, 'payload')
    const { broker } = await fixture([source])
    const [reference] = await broker.selectForRead({ owner, multiple: false })

    const consumingOwner = { ...owner, toolCallId: 'call-2' }
    const lease = await broker.consumeForRead({
      owner: consumingOwner,
      handles: [reference.handle]
    })
    expect(lease.paths).toHaveLength(1)
    await expect(
      broker.resolveForRead({ owner: consumingOwner, handles: [reference.handle] })
    ).rejects.toMatchObject({ code: 'browser.file.invalid_handle' })
    await lease.finish()

    const [other] = await broker.selectForRead({ owner, multiple: false })
    await expect(
      broker.resolveForRead({
        owner: { ...owner, runId: 'run-2' },
        handles: [other.handle]
      })
    ).rejects.toMatchObject({ code: 'browser.file.identity_mismatch' })
  })

  it('allows only one concurrent approved call to claim a selected handle', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-source-'))
    roots.push(root)
    const source = join(root, 'input.txt')
    await writeFile(source, 'payload')
    const { broker } = await fixture([source])
    const [reference] = await broker.selectForRead({ owner, multiple: false })

    const attempts = await Promise.allSettled([
      broker.consumeForRead({
        owner: { ...owner, toolCallId: 'upload-a' },
        handles: [reference.handle]
      }),
      broker.consumeForRead({
        owner: { ...owner, toolCallId: 'upload-b' },
        handles: [reference.handle]
      })
    ])
    expect(attempts.filter((result) => result.status === 'fulfilled')).toHaveLength(1)
    expect(attempts.filter((result) => result.status === 'rejected')).toHaveLength(1)
    const fulfilled = attempts.find(
      (
        result
      ): result is PromiseFulfilledResult<Awaited<ReturnType<typeof broker.consumeForRead>>> =>
        result.status === 'fulfilled'
    )
    await fulfilled?.value.finish()
    expect(broker.snapshot()).toEqual(EMPTY_SNAPSHOT)
  })

  it('retains a dispatched upload across Tool cancellation and releases it on exact target close', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-source-'))
    roots.push(root)
    const source = join(root, 'lazy-upload.txt')
    const stagedDirectory = join(root, 'upstream-output', '.file-input-retained')
    const stagedPath = join(stagedDirectory, '0.txt')
    await writeFile(source, 'payload')
    await mkdir(stagedDirectory, { recursive: true })
    await writeFile(stagedPath, 'payload')
    let now = 50_000
    const { broker } = await fixture([source], {
      clock: { now: () => now },
      maxFileBytes: 16,
      maxHandles: 2,
      maxRunBytes: 16,
      ttlMs: 1_000
    })
    const [reference] = await broker.selectForRead({ owner, multiple: false })
    const consumingOwner = { ...owner, toolCallId: 'upload-retained' }
    const sourceLease = await broker.consumeForRead({
      owner: consumingOwner,
      handles: [reference.handle]
    })
    const dispose = vi.fn(async () => rm(stagedDirectory, { force: true, recursive: true }))
    await sourceLease.finish()
    const retained = await broker.retainConsumedFiles({
      owner: consumingOwner,
      surfaceId: 'managed-browser-retained',
      generation: 7,
      references: sourceLease.references,
      dispose
    })

    retained.markDispatched()
    now += 1_001
    await broker.purgeExpired()
    await retained.finish()
    await broker.releaseToolCall({ runId: owner.runId, toolCallId: consumingOwner.toolCallId })
    expect(dispose).not.toHaveBeenCalled()
    expect(await readFile(stagedPath, 'utf8')).toBe('payload')
    expect(broker.snapshot()).toEqual({
      handles: 0,
      bytes: 7,
      retained: { leases: 1, files: 1, bytes: 7 }
    })

    await broker.releaseSurface({ surfaceId: 'managed-browser-retained', generation: 6 })
    expect(dispose).not.toHaveBeenCalled()
    await broker.releaseSurface({ surfaceId: 'managed-browser-retained', generation: 7 })
    expect(dispose).toHaveBeenCalledOnce()
    await expect(stat(stagedPath)).rejects.toMatchObject({ code: 'ENOENT' })
    expect(broker.snapshot()).toEqual(EMPTY_SNAPSHOT)
  })

  it('cleans a definitely-not-dispatched retention and keeps retained bytes inside capacity', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-source-'))
    roots.push(root)
    const source = join(root, 'capacity.txt')
    await writeFile(source, '12345')
    const { broker } = await fixture([source], {
      maxFileBytes: 5,
      maxHandles: 1,
      maxRunBytes: 5
    })
    const [reference] = await broker.selectForRead({ owner, multiple: false })
    const sourceLease = await broker.consumeForRead({ owner, handles: [reference.handle] })
    await sourceLease.finish()
    const dispose = vi.fn(async () => undefined)
    const retained = await broker.retainConsumedFiles({
      owner,
      surfaceId: 'managed-browser-capacity',
      generation: 1,
      references: sourceLease.references,
      dispose
    })
    expect(broker.snapshot()).toEqual({
      handles: 0,
      bytes: 5,
      retained: { leases: 1, files: 1, bytes: 5 }
    })
    await expect(broker.selectForRead({ owner, multiple: false })).rejects.toMatchObject({
      code: 'browser.file.capacity'
    })

    await retained.finish()
    expect(dispose).toHaveBeenCalledOnce()
    expect(broker.snapshot()).toEqual(EMPTY_SNAPSHOT)
  })

  it('enforces item, selection, and run capacity before retaining authority', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-source-'))
    roots.push(root)
    const one = join(root, 'one.txt')
    const two = join(root, 'two.txt')
    await writeFile(one, '12345')
    await writeFile(two, '67890')
    const { broker } = await fixture([one, two], {
      maxFileBytes: 5,
      maxRunBytes: 8,
      maxFilesPerSelection: 2
    })

    await expect(broker.selectForRead({ owner, multiple: true })).rejects.toMatchObject({
      code: 'browser.file.capacity'
    })
    expect(broker.snapshot()).toEqual(EMPTY_SNAPSHOT)
  })

  it('expires, revokes, and cleans private copies without preserving paths', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-source-'))
    roots.push(root)
    const source = join(root, 'input.txt')
    await writeFile(source, 'payload')
    let now = 10_000
    const clock: BrowserFileBrokerClock = { now: () => now }
    const { broker, root: brokerRoot } = await fixture([source], { clock, ttlMs: 1_000 })
    const [reference] = await broker.selectForRead({ owner, multiple: false })
    const resolution = await broker.resolveForRead({ owner, handles: [reference.handle] })
    const privatePath = resolution.paths[0]
    now = 11_001

    await expect(
      broker.resolveForRead({ owner, handles: [reference.handle] })
    ).rejects.toMatchObject({ code: 'browser.file.expired' })
    await expect(stat(privatePath)).rejects.toMatchObject({ code: 'ENOENT' })

    const [second] = await broker.selectForRead({ owner, multiple: false })
    expect(await readdir(join(brokerRoot, 'browser-automation-files'))).toHaveLength(1)
    await broker.releaseToolCall(owner)
    await expect(broker.resolveForRead({ owner, handles: [second.handle] })).rejects.toBeInstanceOf(
      BrowserFileBrokerError
    )
    expect(broker.snapshot()).toEqual(EMPTY_SNAPSHOT)
    expect(await readdir(join(brokerRoot, 'browser-automation-files'))).toEqual([])
  })

  it('does not consume a handle that expires after asynchronous revision checks', async () => {
    const sourceRoot = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-source-'))
    roots.push(sourceRoot)
    const source = join(sourceRoot, 'expires-during-consume.txt')
    await writeFile(source, 'payload')
    let now = 20_000
    const clock: BrowserFileBrokerClock = { now: () => now }
    const { broker, root } = await fixture([source], {
      clock,
      ttlMs: 1_000,
      beforeConsumeFinal: () => {
        now += 1_000
      }
    })
    const [reference] = await broker.selectForRead({ owner, multiple: false })

    await expect(
      broker.consumeForRead({ owner, handles: [reference.handle] })
    ).rejects.toMatchObject({ code: 'browser.file.expired' })
    await expectNoBrokerResidue(broker, root)
    await broker.shutdown()
  })

  const latePickerRevocations: ReadonlyArray<{
    name: string
    revoke(broker: BrowserFileBroker): Promise<void>
  }> = [
    {
      name: 'run release',
      revoke: (broker) => broker.releaseRun(owner.runId)
    },
    {
      name: 'capability release',
      revoke: (broker) => broker.releaseCapability(owner.activationId)
    },
    {
      name: 'tool-call release',
      revoke: (broker) => broker.releaseToolCall(owner)
    }
  ]

  it.each(latePickerRevocations)(
    'fences a native selection that returns after $name',
    async ({ revoke }) => {
      const sourceRoot = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-source-'))
      roots.push(sourceRoot)
      const source = join(sourceRoot, 'late-selection.txt')
      await writeFile(source, 'must-not-be-frozen')
      const pickerEntered = deferred<void>()
      const pickerResult = deferred<readonly string[] | null>()
      const { broker, root } = await fixture([], {
        selectionProvider: {
          selectFiles: async () => {
            pickerEntered.resolve()
            return pickerResult.promise
          }
        }
      })

      const selection = broker.selectForRead({ owner, multiple: false })
      await pickerEntered.promise
      await revoke(broker)
      pickerResult.resolve([source])

      await expect(selection).rejects.toMatchObject({ code: 'browser.file.cancelled' })
      await expectNoBrokerResidue(broker, root)
    }
  )

  it('fences a native selection that returns after shutdown', async () => {
    const sourceRoot = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-source-'))
    roots.push(sourceRoot)
    const source = join(sourceRoot, 'late-selection.txt')
    await writeFile(source, 'must-not-be-frozen')
    const pickerEntered = deferred<void>()
    const pickerResult = deferred<readonly string[] | null>()
    const { broker, root } = await fixture([], {
      selectionProvider: {
        selectFiles: async () => {
          pickerEntered.resolve()
          return pickerResult.promise
        }
      }
    })

    const selection = broker.selectForRead({ owner, multiple: false })
    await pickerEntered.promise
    await broker.shutdown()
    pickerResult.resolve([source])

    await expect(selection).rejects.toMatchObject({ code: 'browser.file.closed' })
    await expectNoBrokerResidue(broker, root)
  })

  it('treats native picker cancellation as a normal empty selection', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-test-'))
    roots.push(root)
    await mkdir(join(root, 'browser-automation-files'))
    const broker = new BrowserFileBroker({
      rootDirectory: join(root, 'browser-automation-files'),
      selectionProvider: { selectFiles: async () => null }
    })
    await expect(broker.selectForRead({ owner, multiple: true })).resolves.toEqual([])
  })

  it('sweeps crash leftovers and never follows a symlinked Host root', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-test-'))
    roots.push(root)
    const privateRoot = join(root, 'browser-automation-files')
    await mkdir(privateRoot)
    await writeFile(join(privateRoot, 'crash-leftover.secret'), 'secret-canary')
    const broker = new BrowserFileBroker({
      rootDirectory: privateRoot,
      selectionProvider: { selectFiles: async () => null }
    })
    await broker.initialize()
    expect(await readdir(privateRoot)).toEqual([])

    await broker.shutdown()
    const target = join(root, 'must-survive')
    await mkdir(target)
    await writeFile(join(target, 'keep.txt'), 'keep')
    await symlink(target, privateRoot)
    const restarted = new BrowserFileBroker({
      rootDirectory: privateRoot,
      selectionProvider: { selectFiles: async () => null }
    })
    await restarted.initialize()
    expect((await lstat(privateRoot)).isDirectory()).toBe(true)
    expect(await readFile(join(target, 'keep.txt'), 'utf8')).toBe('keep')
  })

  it('streams and consumes 100 selections without retaining handles or private files', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-source-'))
    roots.push(root)
    const source = join(root, 'input.txt')
    await writeFile(source, 'payload')
    const { broker } = await fixture([source], { maxHandles: 1 })
    for (let index = 0; index < 100; index += 1) {
      const selectionOwner = { ...owner, toolCallId: `select-${index}` }
      const [reference] = await broker.selectForRead({ owner: selectionOwner, multiple: false })
      const lease = await broker.consumeForRead({
        owner: { ...owner, toolCallId: `upload-${index}` },
        handles: [reference.handle]
      })
      await lease.finish()
    }
    expect(broker.snapshot()).toEqual(EMPTY_SNAPSHOT)
  })

  it('rejects an oversized file before creating any private copy', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mycopilot-browser-file-source-'))
    roots.push(root)
    const source = join(root, 'too-large.txt')
    await writeFile(source, '123456')
    const { broker } = await fixture([source], { maxFileBytes: 5, maxRunBytes: 5 })
    await expect(broker.selectForRead({ owner, multiple: false })).rejects.toMatchObject({
      code: 'browser.file.too_large'
    })
    expect(broker.snapshot()).toEqual(EMPTY_SNAPSHOT)
  })
})
