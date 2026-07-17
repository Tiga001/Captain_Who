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

    const textPreview = await service.readPreview({ path: 'main.ts', projectId: 'project-1' })
    expect(textPreview.metadata).toMatchObject({ path: 'main.ts', previewKind: 'text' })
    expect(textPreview.text).toMatchObject({
      content: 'export const answer = 42\n',
      path: 'main.ts'
    })

    const imagePreview = await service.readPreview({ path: 'pixel.png', projectId: 'project-1' })
    expect(imagePreview.metadata).toMatchObject({ mimeType: 'image/png', previewKind: 'image' })
    expect(imagePreview.image).toMatchObject({ data: 'iVBORw==', mimeType: 'image/png' })

    const binaryPreview = await service.readPreview({
      path: 'archive.bin',
      projectId: 'project-1'
    })
    expect(binaryPreview).toMatchObject({ metadata: { previewKind: 'binary' } })
    expect(binaryPreview.text).toBeUndefined()
  })

  it('validates the complete file when a UTF-8 character crosses the old sample boundary', async () => {
    const content = `${'a'.repeat(8_191)}中文内容\n`
    await writeFile(join(root, 'boundary.md'), content)

    const preview = await service.readPreview({ path: 'boundary.md', projectId: 'project-1' })

    expect(preview.metadata).toMatchObject({
      mimeType: 'text/plain; charset=utf-8',
      previewKind: 'text'
    })
    expect(preview.text?.content).toBe(content)
  })

  it('decodes BOM-marked UTF-16 text without weakening binary detection', async () => {
    const content = '标题\n完整内容'
    const utf16Le = Buffer.concat([Buffer.from([0xff, 0xfe]), Buffer.from(content, 'utf16le')])
    const utf16BeBody = Buffer.from(Buffer.from(content, 'utf16le')).swap16()
    const utf16Be = Buffer.concat([Buffer.from([0xfe, 0xff]), utf16BeBody])
    await writeFile(join(root, 'utf16-le.txt'), utf16Le)
    await writeFile(join(root, 'utf16-be.txt'), utf16Be)

    const littleEndianPreview = await service.readPreview({
      path: 'utf16-le.txt',
      projectId: 'project-1'
    })
    const bigEndianPreview = await service.readPreview({
      path: 'utf16-be.txt',
      projectId: 'project-1'
    })

    expect(littleEndianPreview.metadata).toMatchObject({
      mimeType: 'text/plain; charset=utf-16le',
      previewKind: 'text'
    })
    expect(littleEndianPreview.text?.content).toBe(content)
    expect(bigEndianPreview.metadata).toMatchObject({
      mimeType: 'text/plain; charset=utf-16be',
      previewKind: 'text'
    })
    expect(bigEndianPreview.text?.content).toBe(content)
  })

  it('keeps malformed UTF-8 and NUL-containing content classified as binary', async () => {
    await writeFile(
      join(root, 'malformed.txt'),
      Buffer.concat([Buffer.from('valid prefix\n'), Buffer.from([0xc3, 0x28])])
    )
    await writeFile(join(root, 'nul.txt'), Buffer.from('valid\0content'))

    await expect(
      service.readPreview({ path: 'malformed.txt', projectId: 'project-1' })
    ).resolves.toMatchObject({ metadata: { previewKind: 'binary' } })
    await expect(
      service.readPreview({ path: 'nul.txt', projectId: 'project-1' })
    ).resolves.toMatchObject({ metadata: { previewKind: 'binary' } })
  })

  it('preserves exact workspace filenames and can omit hidden entries', async () => {
    await writeFile(join(root, ' spaced name .txt'), 'kept exactly')
    await writeFile(join(root, '.hidden'), 'hidden')

    await expect(
      service.readPreview({ path: ' spaced name .txt', projectId: 'project-1' })
    ).resolves.toMatchObject({
      metadata: { previewKind: 'text' },
      text: { content: 'kept exactly', path: ' spaced name .txt' }
    })
    await expect(
      service.listDirectory({ includeHidden: false, projectId: 'project-1' })
    ).resolves.toMatchObject({ entries: [{ name: ' spaced name .txt' }] })
  })

  it('rejects traversal and marks oversized text as unavailable for preview', async () => {
    await writeFile(join(root, 'large.txt'), 'x'.repeat(1024 * 1024 + 1))

    expect(() => normalizeWorkspacePath('../outside.txt')).toThrow('invalid segment')
    expect(() => normalizeWorkspacePath('/outside.txt')).toThrow('relative')
    await expect(
      service.readPreview({ path: 'large.txt', projectId: 'project-1' })
    ).resolves.toMatchObject({ metadata: { previewKind: 'too-large' } })
  })

  it.skipIf(process.platform === 'win32')(
    'does not follow symbolic links outside the project',
    async () => {
      const outside = await mkdtemp(join(tmpdir(), 'mycopilot-workspace-files-outside-'))
      try {
        await writeFile(join(outside, 'secret.txt'), 'secret')
        await symlink(join(outside, 'secret.txt'), join(root, 'linked.txt'))

        await expect(
          service.readPreview({ path: 'linked.txt', projectId: 'project-1' })
        ).resolves.toMatchObject({
          metadata: { kind: 'symlink', previewKind: 'unsupported' }
        })
      } finally {
        await rm(outside, { force: true, recursive: true })
      }
    }
  )
})
