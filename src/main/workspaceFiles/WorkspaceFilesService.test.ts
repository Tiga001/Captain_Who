import { mkdtemp, mkdir, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { normalizeWorkspacePath, WorkspaceFilesService } from './WorkspaceFilesService'

describe('WorkspaceFilesService', () => {
  let root = ''
  let service: WorkspaceFilesService

  beforeEach(async () => {
    root = await mkdtemp(join(tmpdir(), 'mycopilot-workspace-files-'))
    service = new WorkspaceFilesService(async (projectId) =>
      projectId === 'project-1' ? root : null
    )
  })

  afterEach(async () => {
    await rm(root, { force: true, recursive: true })
  })

  it('lists directories first, preserves hidden files, and excludes Git internals', async () => {
    await mkdir(join(root, 'src'))
    await mkdir(join(root, '.git'))
    await writeFile(join(root, 'z.txt'), 'z')
    await writeFile(join(root, '.env'), 'A=1')

    const listing = await service.listDirectory({ projectId: 'project-1' })

    expect(listing).toMatchObject({ directoryPath: '', truncated: false })
    expect(listing.entries.map((entry) => [entry.kind, entry.path])).toEqual([
      ['directory', 'src/'],
      ['file', '.env'],
      ['file', 'z.txt']
    ])
  })

  it('reads UTF-8 text and classifies images and binary files without exposing paths', async () => {
    await writeFile(join(root, 'main.ts'), 'export const answer = 42\n')
    await writeFile(join(root, 'pixel.png'), Buffer.from([0x89, 0x50, 0x4e, 0x47]))
    await writeFile(join(root, 'archive.bin'), Buffer.from([0, 1, 2, 3]))

    await expect(
      service.readFileMetadata({ path: 'main.ts', projectId: 'project-1' })
    ).resolves.toMatchObject({ path: 'main.ts', previewKind: 'text' })
    await expect(
      service.readTextFile({ path: 'main.ts', projectId: 'project-1' })
    ).resolves.toMatchObject({ content: 'export const answer = 42\n', path: 'main.ts' })
    await expect(
      service.readFileMetadata({ path: 'pixel.png', projectId: 'project-1' })
    ).resolves.toMatchObject({ mimeType: 'image/png', previewKind: 'image' })
    await expect(
      service.readImageFile({ path: 'pixel.png', projectId: 'project-1' })
    ).resolves.toMatchObject({ data: 'iVBORw==', mimeType: 'image/png' })
    await expect(
      service.readFileMetadata({ path: 'archive.bin', projectId: 'project-1' })
    ).resolves.toMatchObject({ previewKind: 'binary' })
  })

  it('preserves exact workspace filenames and can omit hidden entries', async () => {
    await writeFile(join(root, ' spaced name .txt'), 'kept exactly')
    await writeFile(join(root, '.hidden'), 'hidden')

    await expect(
      service.readTextFile({ path: ' spaced name .txt', projectId: 'project-1' })
    ).resolves.toMatchObject({ content: 'kept exactly', path: ' spaced name .txt' })
    await expect(
      service.listDirectory({ includeHidden: false, projectId: 'project-1' })
    ).resolves.toMatchObject({ entries: [{ name: ' spaced name .txt' }] })
  })

  it('rejects traversal and marks oversized text as unavailable for preview', async () => {
    await writeFile(join(root, 'large.txt'), 'x'.repeat(1024 * 1024 + 1))

    expect(() => normalizeWorkspacePath('../outside.txt')).toThrow('invalid segment')
    expect(() => normalizeWorkspacePath('/outside.txt')).toThrow('relative')
    await expect(
      service.readFileMetadata({ path: 'large.txt', projectId: 'project-1' })
    ).resolves.toMatchObject({ previewKind: 'too-large' })
    await expect(
      service.readTextFile({ path: 'large.txt', projectId: 'project-1' })
    ).rejects.toThrow('not previewable text')
  })

  it.skipIf(process.platform === 'win32')(
    'does not follow symbolic links outside the project',
    async () => {
      const outside = await mkdtemp(join(tmpdir(), 'mycopilot-workspace-files-outside-'))
      try {
        await writeFile(join(outside, 'secret.txt'), 'secret')
        await symlink(join(outside, 'secret.txt'), join(root, 'linked.txt'))

        await expect(
          service.readFileMetadata({ path: 'linked.txt', projectId: 'project-1' })
        ).resolves.toMatchObject({ kind: 'symlink', previewKind: 'unsupported' })
        await expect(
          service.readTextFile({ path: 'linked.txt', projectId: 'project-1' })
        ).rejects.toThrow('not previewable text')
      } finally {
        await rm(outside, { force: true, recursive: true })
      }
    }
  )
})
