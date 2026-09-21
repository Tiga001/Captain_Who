import { mkdir, mkdtemp, open, realpath, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { EventEmitter } from 'node:events'
import type { IpcMainInvokeEvent } from 'electron'
import type {
  AgentInputAttachment,
  AttachmentImportMetadata,
  AttachmentImportProgress
} from '@mycopilot/protocol'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'

const { showOpenDialog } = vi.hoisted(() => ({ showOpenDialog: vi.fn() }))
vi.mock('electron', () => ({
  BrowserWindow: { fromWebContents: () => undefined },
  dialog: { showOpenDialog }
}))
import { AttachmentDialogBridge } from '../attachments/AttachmentDialogBridge'

let directory: string
beforeEach(async () => {
  directory = await mkdtemp(join(tmpdir(), 'captain-attachment-import-'))
  showOpenDialog.mockReset()
})
afterEach(async () => {
  await rm(directory, { recursive: true, force: true })
})

function fixture() {
  let metadata: AttachmentImportMetadata
  let received = 0
  let maxChunk = 0
  const backend = {
    beginAttachmentImport: vi.fn(async (input: AttachmentImportMetadata) => {
      metadata = input
      received = 0
      return { importId: 'import-1' }
    }),
    appendAttachmentImport: vi.fn(async (input: { offset: number; data: string }) => {
      expect(input.offset).toBe(received)
      const bytes = Buffer.from(input.data, 'base64').length
      maxChunk = Math.max(maxChunk, bytes)
      received += bytes
      return { receivedBytes: received }
    }),
    finishAttachmentImport: vi.fn(async (): Promise<AgentInputAttachment> => ({
      ...metadata,
      encoding: 'managed',
      data: 'import-1'
    })),
    cancelAttachmentImport: vi.fn(async () => {})
  }
  const sender = Object.assign(new EventEmitter(), {
    id: 1,
    isDestroyed: () => false,
    send: vi.fn()
  })
  const event = { sender } as unknown as IpcMainInvokeEvent
  return {
    backend,
    sender,
    event,
    bridge: new AttachmentDialogBridge(backend),
    maxChunk: () => maxChunk
  }
}

it('imports a file larger than the former tool ceiling using bounded chunks and a short reference', async () => {
  const path = join(directory, 'large.csv')
  const file = await open(path, 'w')
  await file.truncate(65 * 1024 * 1024 + 3)
  await file.close()
  showOpenDialog.mockResolvedValue({ canceled: false, filePaths: [path] })
  const f = fixture()
  const result = await f.bridge.selectInputAttachments(f.event, {
    kind: 'file',
    requestId: 'pick-1'
  })
  expect(result).toEqual([
    expect.objectContaining({
      encoding: 'managed',
      data: 'import-1',
      sizeBytes: 65 * 1024 * 1024 + 3
    })
  ])
  expect(f.maxChunk()).toBe(512 * 1024)
  expect(f.backend.appendAttachmentImport).toHaveBeenCalledTimes(131)
  expect(JSON.stringify(result).length).toBeLessThan(400)
  expect(f.sender.send.mock.calls.at(-1)?.[1]).toMatchObject({
    requestId: 'pick-1',
    status: 'complete'
  })
})

it('canonicalizes selected folder references before exposing their model path', async () => {
  const selected = join(directory, 'selected')
  const alias = join(directory, 'alias')
  await mkdir(selected)
  await symlink(selected, alias)
  showOpenDialog.mockResolvedValue({ canceled: false, filePaths: [alias] })

  const f = fixture()
  const result = await f.bridge.selectInputFolders(f.event)
  expect(result).toEqual([
    expect.objectContaining({
      name: 'selected',
      rootPath: await realpath(selected)
    })
  ])
})

it('cancels the import between chunks without publishing a ready attachment', async () => {
  const path = join(directory, 'cancel.txt')
  await writeFile(path, Buffer.alloc(2 * 1024 * 1024))
  showOpenDialog.mockResolvedValue({ canceled: false, filePaths: [path] })
  const f = fixture()
  f.sender.send.mockImplementation((_channel: string, progress: AttachmentImportProgress) => {
    if (progress.receivedBytes > 0) void f.bridge.cancelImport(f.event, progress.attachment.id)
  })
  await expect(f.bridge.selectInputAttachments(f.event, { kind: 'file' })).resolves.toEqual([])
  expect(f.backend.appendAttachmentImport).toHaveBeenCalledTimes(1)
  expect(f.backend.finishAttachmentImport).not.toHaveBeenCalled()
  expect(f.backend.cancelAttachmentImport).toHaveBeenCalledWith({ importId: 'import-1' })
})

it('preserves completed files when a later selection is missing, and can retry the failed grant', async () => {
  const ready = join(directory, 'ready.txt')
  const missing = join(directory, 'missing.txt')
  await writeFile(ready, 'ready')
  showOpenDialog.mockResolvedValue({ canceled: false, filePaths: [ready, missing] })
  const f = fixture()
  const result = await f.bridge.selectInputAttachments(f.event, { kind: 'file' })
  expect(result).toHaveLength(1)
  const failed = f.sender.send.mock.calls
    .map((call) => call[1] as AttachmentImportProgress)
    .find((p) => p.status === 'failed')!
  await writeFile(missing, 'recovered')
  await expect(
    f.bridge.retryInputAttachment(f.event, failed.attachment.id, 'retry')
  ).resolves.toMatchObject({ name: 'missing.txt', sizeBytes: 9 })
  expect(f.sender.send.mock.calls.at(-1)?.[1]).toMatchObject({
    requestId: 'retry',
    status: 'complete'
  })
})

it('does not publish an attachment cancelled while finalization is in flight', async () => {
  const path = join(directory, 'finalizing.txt')
  await writeFile(path, 'finalizing')
  showOpenDialog.mockResolvedValue({ canceled: false, filePaths: [path] })
  const f = fixture()
  const finish = f.backend.finishAttachmentImport.getMockImplementation()!
  f.backend.finishAttachmentImport.mockImplementation(async () => {
    await f.bridge.cancelImport(f.event, 'finalizing-request')
    return finish()
  })
  await expect(
    f.bridge.selectInputAttachments(f.event, {
      kind: 'file',
      requestId: 'finalizing-request'
    })
  ).resolves.toEqual([])
  expect(f.sender.send.mock.calls.at(-1)?.[1]).toMatchObject({ status: 'cancelled' })
  expect(f.backend.cancelAttachmentImport).toHaveBeenCalledWith({ importId: 'import-1' })
})

it('cancels a native selection before its dialog finishes', async () => {
  let closeDialog!: (value: { canceled: boolean; filePaths: string[] }) => void
  showOpenDialog.mockReturnValue(
    new Promise((resolve) => {
      closeDialog = resolve
    })
  )
  const f = fixture()
  const pending = f.bridge.selectInputAttachments(f.event, {
    kind: 'file',
    requestId: 'late-dialog'
  })
  await f.bridge.cancelImport(f.event, 'late-dialog')
  closeDialog({ canceled: false, filePaths: [join(directory, 'never-read.txt')] })
  await expect(pending).resolves.toEqual([])
  expect(f.backend.beginAttachmentImport).not.toHaveBeenCalled()
})

it('rejects another renderer trying to reuse a native selection grant', async () => {
  showOpenDialog.mockResolvedValue({ canceled: false, filePaths: [join(directory, 'missing.txt')] })
  const f = fixture()
  await f.bridge.selectInputAttachments(f.event, { kind: 'file' })
  const id = (f.sender.send.mock.calls[0][1] as AttachmentImportProgress).attachment.id
  await expect(
    f.bridge.retryInputAttachment({ sender: { id: 2 } } as IpcMainInvokeEvent, id)
  ).rejects.toThrow('不可重试')
})
