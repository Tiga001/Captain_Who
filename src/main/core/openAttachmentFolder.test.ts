import { mkdir, mkdtemp, realpath, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, expect, it } from 'vitest'
import { resolveAttachmentFolderForOpen } from '../attachments/openAttachmentFolder'

let root: string
beforeEach(async () => {
  root = await realpath(await mkdtemp(join(tmpdir(), 'attachment-folder-open-')))
})
afterEach(async () => {
  await rm(root, { recursive: true, force: true })
})

it('opens the selected directory rather than its parent', async () => {
  const path = join(root, 'selected')
  await mkdir(path)
  expect(
    await resolveAttachmentFolderForOpen({
      schemaVersion: 1,
      id: 'folder',
      name: 'selected',
      rootPath: path
    })
  ).toBe(path)
})

it('rejects a file, missing directory, and unavailable folder', async () => {
  const path = join(root, 'file.txt')
  await writeFile(path, 'content')
  const folder = { schemaVersion: 1, id: 'folder', name: 'selected', rootPath: path }
  await expect(resolveAttachmentFolderForOpen(folder)).rejects.toThrow('not a folder')
  await expect(
    resolveAttachmentFolderForOpen({ ...folder, rootPath: join(root, 'missing') })
  ).rejects.toThrow()
  await expect(
    resolveAttachmentFolderForOpen({ ...folder, rootPath: root, status: 'unavailable' })
  ).rejects.toThrow('no longer available')
})

it('checks the persisted Unix folder identity before opening', async () => {
  const info = await stat(root)
  await expect(
    resolveAttachmentFolderForOpen({
      schemaVersion: 1,
      id: 'folder',
      name: 'selected',
      rootPath: root,
      rootIdentity: { kind: 'unix', schemaVersion: 1, device: info.dev, inode: info.ino + 1 }
    })
  ).rejects.toThrow('replaced')
})
