import { beforeEach, expect, it, vi } from 'vitest'
import type { AgentInputAttachment, AttachmentImportMetadata } from '@mycopilot/protocol'

const host = vi.hoisted(() => ({
  getPathForFile: vi.fn(),
  beginImport: vi.fn(),
  appendImport: vi.fn(),
  finishImport: vi.fn(),
  cancelImport: vi.fn(),
  loadPreview: vi.fn()
}))
vi.mock('../../../host/hostClient', () => ({ hostClient: { attachments: host } }))
import {
  createComposerAttachmentsFromFiles,
  getComposerDroppedFilePath,
  loadComposerAttachmentPreview
} from '../chatAttachments'

beforeEach(() => {
  for (const fn of Object.values(host)) fn.mockReset()
  let metadata: AttachmentImportMetadata
  host.beginImport.mockImplementation(async (input) => {
    metadata = input
    return { importId: 'managed-import' }
  })
  host.appendImport.mockImplementation(async ({ offset, data }) => ({
    receivedBytes: offset + atob(data).length
  }))
  host.finishImport.mockImplementation(async (): Promise<AgentInputAttachment> => ({
    ...metadata,
    encoding: 'managed',
    data: 'managed-import',
    contentSha256: `sha256:${'a'.repeat(64)}`
  }))
  host.cancelImport.mockResolvedValue(undefined)
  host.getPathForFile.mockImplementation((file: File) => `/native/${file.name}`)
})

it('resolves dropped native paths through the preload webUtils bridge', () => {
  const file = new File(['folder'], 'folder', { type: 'application/octet-stream' })
  expect(getComposerDroppedFilePath(file)).toBe('/native/folder')
  expect(host.getPathForFile).toHaveBeenCalledWith(file)
})

it('imports files above 8 MiB in bounded slices and returns only a managed reference', async () => {
  const file = new File([new Uint8Array(9 * 1024 * 1024 + 7)], 'large.csv', { type: 'text/csv' })
  const onProgress = vi.fn()
  const result = await createComposerAttachmentsFromFiles([file], { onProgress })
  expect(host.appendImport).toHaveBeenCalledTimes(19)
  expect(Math.max(...host.appendImport.mock.calls.map(([call]) => atob(call.data).length))).toBe(
    512 * 1024
  )
  expect(result[0].agentAttachment).toMatchObject({
    encoding: 'managed',
    data: 'managed-import',
    sizeBytes: file.size
  })
  expect(JSON.stringify(result).length).toBeLessThan(600)
  expect(onProgress.mock.calls.at(-1)?.[0]).toMatchObject({
    receivedBytes: file.size,
    status: 'complete'
  })
})

it('cancels between chunks and cleans the unfinished import', async () => {
  const abort = new AbortController()
  await expect(
    createComposerAttachmentsFromFiles([new File([new Uint8Array(1024 * 1024)], 'a.txt')], {
      signal: abort.signal,
      onProgress: (progress) => {
        if (progress.receivedBytes) abort.abort()
      }
    })
  ).rejects.toMatchObject({ name: 'AbortError' })
  expect(host.appendImport).toHaveBeenCalledTimes(1)
  expect(host.finishImport).not.toHaveBeenCalled()
  expect(host.cancelImport).toHaveBeenCalledWith({ importId: 'managed-import' })
})

it('keeps a successful preceding import when a later import fails', async () => {
  const onImported = vi.fn()
  host.appendImport
    .mockResolvedValueOnce({ receivedBytes: 1 })
    .mockRejectedValueOnce(new Error('Disk full'))
  await expect(
    createComposerAttachmentsFromFiles([new File(['a'], 'a.txt'), new File(['b'], 'b.txt')], {
      onImported
    })
  ).rejects.toThrow('Disk full')
  expect(onImported).toHaveBeenCalledTimes(1)
  expect(onImported.mock.calls[0][0].name).toBe('a.txt')
  expect(host.cancelImport).toHaveBeenCalledOnce()
})

it('loads a bounded managed image preview separately from its persisted input', async () => {
  host.loadPreview.mockResolvedValue({ mimeType: 'image/png', data: 'preview' })
  const attachment: AgentInputAttachment = {
    id: 'image-preview-test',
    kind: 'image',
    name: 'large.png',
    mimeType: 'image/png',
    sizeBytes: 99 * 1024 * 1024,
    encoding: 'managed',
    data: 'opaque-import'
  }
  expect(await loadComposerAttachmentPreview(attachment)).toBe('data:image/png;base64,preview')
  expect(await loadComposerAttachmentPreview(attachment)).toBe('data:image/png;base64,preview')
  expect(host.loadPreview).toHaveBeenCalledTimes(1)
  expect(attachment.data).toBe('opaque-import')
})
