import { mkdtemp, mkdir, realpath, rm, symlink, truncate, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { AgentWorkspaceContext, StorageProjectFolderRecord } from '@mycopilot/protocol'
import {
  normalizeWorkspacePath,
  PDF_PREVIEW_LIMIT_BYTES,
  WorkspaceFilesService
} from './WorkspaceFilesService'

describe('WorkspaceFilesService', () => {
  let root = ''
  let service: WorkspaceFilesService

  beforeEach(async () => {
    root = await mkdtemp(join(tmpdir(), 'mycopilot-workspace-files-'))
    service = new WorkspaceFilesService(async (projectId) =>
      projectId === 'project-1'
        ? {
            id: projectId,
            folders: [
              {
                id: 'primary',
                alias: 'app',
                path: root,
                role: 'primary',
                sortOrder: 0,
                createdAt: 1
              }
            ]
          }
        : null
    )
  })

  afterEach(async () => {
    await rm(root, { force: true, recursive: true })
  })

  it('keeps listing, preview, and reveal bound to the requested folder after a primary switch', async () => {
    const auxiliary = join(root, 'auxiliary')
    await mkdir(auxiliary)
    await writeFile(join(root, 'README.md'), 'primary document')
    await writeFile(join(auxiliary, 'README.md'), 'auxiliary document')
    await writeFile(join(auxiliary, 'only-auxiliary.txt'), 'auxiliary')
    const folders: StorageProjectFolderRecord[] = [
      { id: 'primary', alias: 'app', path: root, role: 'primary', sortOrder: 0, createdAt: 1 },
      {
        id: 'auxiliary',
        alias: 'docs',
        path: auxiliary,
        role: 'auxiliary',
        sortOrder: 1,
        createdAt: 1
      }
    ]
    service = new WorkspaceFilesService(async () => ({ id: 'project-1', folders }))
    const target = { projectId: 'project-1', folderId: 'auxiliary', path: 'README.md' }
    expect((await service.listDirectory(target)).entries.map((entry) => entry.name)).toEqual([
      'only-auxiliary.txt',
      'README.md'
    ])
    expect((await service.readPreview(target)).text?.content).toBe('auxiliary document')
    expect(await service.resolvePathForReveal(target)).toBe(
      await realpath(join(auxiliary, 'README.md'))
    )
    folders[0].role = 'auxiliary'
    folders[1].role = 'primary'
    expect((await service.readPreview({ ...target, folderId: 'primary' })).text?.content).toBe(
      'primary document'
    )
    expect(
      (await service.readPreview({ projectId: 'project-1', path: 'README.md' })).text?.content
    ).toBe('auxiliary document')

    folders.pop()
    await expect(service.readPreview(target)).rejects.toThrow('Workspace folder is not available')
    await expect(service.resolvePathForReveal(target)).rejects.toThrow(
      'Workspace folder is not available'
    )
    await expect(service.listDirectory({ ...target, projectId: 'other-project' })).rejects.toThrow(
      'Project is not available'
    )
    await expect(service.readPreview({ ...target, folderId: '  ' })).rejects.toThrow(
      'folder id is invalid'
    )
  })

  it('searches files and directories by workspace alias without exposing native paths', async () => {
    await mkdir(join(root, 'src', 'features'), { recursive: true })
    await writeFile(join(root, 'src', 'features', 'ChatComposer.tsx'), 'export {}')
    await writeFile(join(root, 'src', 'README.md'), 'readme')
    await mkdir(join(root, 'node_modules', 'hidden-package'), { recursive: true })
    await writeFile(join(root, 'node_modules', 'hidden-package', 'ChatComposer.tsx'), 'ignored')

    const result = await service.searchMentions({ projectId: 'project-1', query: 'chatcomp' })

    expect(result.truncated).toBe(false)
    expect(result.entries).toEqual([
      {
        alias: 'app',
        displayName: 'ChatComposer.tsx',
        folderId: 'primary',
        kind: 'file',
        path: 'src/features/ChatComposer.tsx',
        displayPath: 'app/src/features/ChatComposer.tsx'
      }
    ])
    expect(JSON.stringify(result)).not.toContain(root)
  })

  it('ranks matches across every configured root instead of stopping at the first page', async () => {
    const auxiliary = join(root, 'auxiliary')
    await mkdir(join(root, 'unrelated'), { recursive: true })
    await mkdir(auxiliary, { recursive: true })
    await writeFile(join(root, 'unrelated', 'target.txt'), 'primary')
    await writeFile(join(auxiliary, 'target.txt'), 'auxiliary')
    service = new WorkspaceFilesService(async () => ({
      id: 'project-1',
      folders: [
        {
          id: 'primary',
          alias: 'app',
          path: root,
          role: 'primary',
          sortOrder: 0,
          createdAt: 1
        },
        {
          id: 'auxiliary',
          alias: 'docs',
          path: auxiliary,
          role: 'auxiliary',
          sortOrder: 1,
          createdAt: 1
        }
      ]
    }))

    const result = await service.searchMentions({
      projectId: 'project-1',
      query: 'target',
      limit: 1
    })

    expect(result.entries).toHaveLength(1)
    expect(result.entries[0].displayPath).toBe('docs/target.txt')
    expect(JSON.stringify(result)).not.toContain(root)
  })

  it('skips roots that are unavailable or not directories', async () => {
    const file = join(root, 'not-a-folder')
    await writeFile(file, 'file')
    service = new WorkspaceFilesService(async () => ({
      id: 'project-1',
      folders: [
        {
          id: 'missing',
          alias: 'missing',
          path: join(root, 'missing'),
          role: 'primary',
          sortOrder: 0,
          createdAt: 1
        },
        {
          id: 'file',
          alias: 'file',
          path: file,
          role: 'auxiliary',
          sortOrder: 1,
          createdAt: 1
        }
      ]
    }))

    const result = await service.searchMentions({ projectId: 'project-1', query: 'anything' })

    expect(result).toEqual({ entries: [], truncated: false })
  })

  it('does not escape a selected auxiliary root through parent paths or symlinks', async () => {
    const auxiliary = join(root, 'auxiliary')
    await mkdir(auxiliary)
    await writeFile(join(root, 'secret.txt'), 'outside selected root')
    await symlink(root, join(auxiliary, 'outside'), 'dir')
    service = new WorkspaceFilesService(async () => ({
      id: 'project-1',
      folders: [
        {
          id: 'auxiliary',
          alias: 'docs',
          path: auxiliary,
          role: 'primary',
          sortOrder: 0,
          createdAt: 1
        }
      ]
    }))
    await expect(
      service.readPreview({ projectId: 'project-1', folderId: 'auxiliary', path: '../secret.txt' })
    ).rejects.toThrow('invalid segment')
    await expect(
      service.readPreview({
        projectId: 'project-1',
        folderId: 'auxiliary',
        path: 'outside/secret.txt'
      })
    ).rejects.toThrow('outside the project')
    await expect(
      service.listDirectory({
        projectId: 'project-1',
        folderId: 'auxiliary',
        directoryPath: 'outside'
      })
    ).rejects.toThrow('Symbolic-link directories')
  })

  it('resolves historical preview and reveal from the original folder alias without consulting current projects', async () => {
    await writeFile(join(root, 'README.md'), 'frozen document')
    const workspace: AgentWorkspaceContext = {
      projectId: 'project-1',
      displayName: 'Original project',
      rootPath: root,
      folders: [
        {
          id: 'original-folder',
          alias: 'old-alias',
          role: 'primary',
          path: root,
          canonicalPath: root,
          directoryIdentity: null
        }
      ]
    }
    const loadProject = vi.fn(async () => {
      throw new Error('Current configuration must not be consulted')
    })
    const runFiles = {
      loadRunWorkspace: vi.fn(async () => workspace),
      resolveRunWorkspacePath: vi.fn(async () => join(root, 'README.md'))
    }
    service = new WorkspaceFilesService(loadProject, runFiles)
    const request = {
      projectId: 'project-1',
      folderId: 'original-folder',
      assistantMessageId: 'original-turn',
      path: 'README.md'
    }
    expect((await service.readPreview(request)).text?.content).toBe('frozen document')
    expect(await service.resolvePathForReveal(request)).toBe(join(root, 'README.md'))
    expect(runFiles.loadRunWorkspace).toHaveBeenCalledWith({
      projectId: 'project-1',
      assistantMessageId: 'original-turn'
    })
    expect(runFiles.resolveRunWorkspacePath).toHaveBeenCalledWith({
      projectId: 'project-1',
      assistantMessageId: 'original-turn',
      filePath: '@workspace/old-alias/README.md'
    })
    await expect(service.readPreview({ ...request, folderId: 'new-folder' })).rejects.toThrow(
      'Historical workspace folder is not available'
    )
    expect(loadProject).not.toHaveBeenCalled()
  })

  it('never falls back to current files when the historical workspace or root is unavailable', async () => {
    const loadProject = vi.fn(async () => {
      throw new Error('Current configuration must not be consulted')
    })
    const runFiles = {
      loadRunWorkspace: vi.fn<() => Promise<AgentWorkspaceContext | null>>(async () => null),
      resolveRunWorkspacePath: vi.fn(async () => {
        throw new Error('Original folder identity changed')
      })
    }
    service = new WorkspaceFilesService(loadProject, runFiles)
    const request = {
      projectId: 'project-1',
      folderId: 'original-folder',
      assistantMessageId: 'original-turn',
      path: 'README.md'
    }
    await expect(service.readPreview(request)).rejects.toThrow(
      'Historical workspace is not available'
    )
    expect(runFiles.resolveRunWorkspacePath).not.toHaveBeenCalled()
    runFiles.loadRunWorkspace.mockResolvedValue({
      projectId: 'project-1',
      displayName: 'Old',
      rootPath: root,
      folders: [
        {
          id: 'original-folder',
          alias: 'old',
          role: 'primary',
          path: root,
          canonicalPath: root,
          directoryIdentity: null
        }
      ]
    })
    await expect(service.readPreview(request)).rejects.toThrow('Original folder identity changed')
    await expect(service.resolvePathForReveal(request)).rejects.toThrow(
      'Original folder identity changed'
    )
    expect(loadProject).not.toHaveBeenCalled()
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

  it('returns validated PDF bytes without exposing an absolute file path', async () => {
    const data = Buffer.from('%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n')
    await writeFile(join(root, 'paper.PDF'), data)

    const preview = await service.readPreview({ path: 'paper.PDF', projectId: 'project-1' })

    expect(preview.metadata).toMatchObject({
      mimeType: 'application/pdf',
      path: 'paper.PDF',
      previewKind: 'pdf',
      sizeBytes: data.byteLength
    })
    expect(preview.pdf).toMatchObject({
      mimeType: 'application/pdf',
      path: 'paper.PDF',
      sizeBytes: data.byteLength
    })
    expect(preview.pdf?.data).toBeInstanceOf(Uint8Array)
    expect(Buffer.from(preview.pdf?.data ?? [])).toEqual(data)
    expect(JSON.stringify(preview)).not.toContain(root)
  })

  it('rejects false PDF extensions and reports oversized PDFs before reading them', async () => {
    await writeFile(join(root, 'fake.pdf'), 'This is plain text, not a PDF document.')
    await writeFile(join(root, 'large.pdf'), '%PDF-1.7\n')
    await truncate(join(root, 'large.pdf'), PDF_PREVIEW_LIMIT_BYTES + 1)

    await expect(
      service.readPreview({ path: 'fake.pdf', projectId: 'project-1' })
    ).resolves.toMatchObject({
      metadata: { mimeType: 'application/pdf', previewKind: 'unsupported' }
    })
    await expect(
      service.readPreview({ path: 'large.pdf', projectId: 'project-1' })
    ).resolves.toMatchObject({
      metadata: { mimeType: 'application/pdf', previewKind: 'too-large' }
    })
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
