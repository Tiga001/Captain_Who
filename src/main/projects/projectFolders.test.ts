import { mkdir, mkdtemp, realpath, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { StorageProjectRecord } from '@mycopilot/protocol'
import {
  buildProjectRecordFromCreateInput,
  buildProjectRecordFromUpdateInput,
  primaryProjectFolderPath,
  projectFolderAliasFromPath,
  ProjectValidationError
} from './projectFolders'

async function rejection(promise: Promise<unknown>) {
  try {
    await promise
  } catch (error) {
    return error
  }
  throw new Error('expected the promise to reject')
}

describe('projectFolders', () => {
  let root = ''
  let primary = ''
  let docs = ''
  let nextFolderId = 0
  const options = { now: () => 1_700_000_000_000, folderId: () => `folder-${(nextFolderId += 1)}` }

  beforeEach(async () => {
    nextFolderId = 0
    root = await realpath(await mkdtemp(join(tmpdir(), 'mycopilot-project-folders-')))
    primary = join(root, 'app')
    docs = join(root, 'docs')
    await mkdir(primary)
    await mkdir(docs)
  })

  afterEach(async () => {
    await rm(root, { force: true, recursive: true })
  })

  describe('projectFolderAliasFromPath', () => {
    it('keeps letters, digits, dots, underscores and dashes from the last path segment', () => {
      expect(projectFolderAliasFromPath('/Users/me/My Repo (v2)/')).toBe('My-Repo-v2')
      expect(projectFolderAliasFromPath('/work/数据_分析.v1')).toBe('数据_分析.v1')
      expect(projectFolderAliasFromPath('/tmp/--weird--')).toBe('weird')
    })

    it('falls back to a stable alias when nothing usable remains', () => {
      expect(projectFolderAliasFromPath('/')).toBe('workspace')
      expect(projectFolderAliasFromPath('/tmp/***')).toBe('workspace')
    })

    it('bounds the alias length', () => {
      expect(projectFolderAliasFromPath(`/tmp/${'a'.repeat(200)}`)).toHaveLength(64)
    })
  })

  describe('buildProjectRecordFromCreateInput', () => {
    it('stores the trimmed name, ordered folders, aliases and a single primary folder', async () => {
      const project = await buildProjectRecordFromCreateInput(
        {
          name: '  Wire workspace  ',
          folders: [
            { path: primary, role: 'primary' },
            { path: `${docs}/`, role: 'auxiliary' }
          ]
        },
        options
      )

      expect(project).toEqual({
        id: 'project-wire-workspace-1700000000000',
        name: 'Wire workspace',
        folders: [
          {
            id: 'folder-1',
            path: primary,
            alias: 'app',
            role: 'primary',
            sortOrder: 0,
            createdAt: 1_700_000_000_000
          },
          {
            id: 'folder-2',
            path: docs,
            alias: 'docs',
            role: 'auxiliary',
            sortOrder: 1,
            createdAt: 1_700_000_000_000
          }
        ],
        createdAt: 1_700_000_000_000,
        pinnedAt: null
      })
      expect(primaryProjectFolderPath(project)).toBe(primary)
    })

    it('disambiguates folders that share a basename', async () => {
      const otherApp = join(docs, 'app')
      await mkdir(otherApp)
      const project = await buildProjectRecordFromCreateInput(
        {
          name: 'Twins',
          folders: [
            { path: primary, role: 'primary' },
            { path: otherApp, role: 'auxiliary' }
          ]
        },
        options
      )
      expect(project.folders.map((folder) => folder.alias)).toEqual(['app', 'app-2'])
    })

    it('rejects an empty name, no folders, and a missing primary folder', async () => {
      const emptyName = await rejection(
        buildProjectRecordFromCreateInput(
          { name: '   ', folders: [{ path: primary, role: 'primary' }] },
          options
        )
      )
      expect(emptyName).toBeInstanceOf(ProjectValidationError)
      expect((emptyName as ProjectValidationError).data).toEqual({
        kind: 'project_validation',
        code: 'name_required'
      })

      const noFolders = await rejection(
        buildProjectRecordFromCreateInput({ name: 'Empty', folders: [] }, options)
      )
      expect((noFolders as ProjectValidationError).data.code).toBe('folders_required')

      const noPrimary = await rejection(
        buildProjectRecordFromCreateInput(
          {
            name: 'No primary',
            folders: [
              { path: primary, role: 'auxiliary' },
              { path: docs, role: 'auxiliary' }
            ]
          },
          options
        )
      )
      expect((noPrimary as ProjectValidationError).data.code).toBe('primary_required')

      const twoPrimaries = await rejection(
        buildProjectRecordFromCreateInput(
          {
            name: 'Two primaries',
            folders: [
              { path: primary, role: 'primary' },
              { path: docs, role: 'primary' }
            ]
          },
          options
        )
      )
      expect((twoPrimaries as ProjectValidationError).data.code).toBe('primary_required')
    })

    it('rejects paths that do not exist or are files, reporting the offending path', async () => {
      const missingPath = join(root, 'missing')
      const missing = await rejection(
        buildProjectRecordFromCreateInput(
          { name: 'Missing', folders: [{ path: missingPath, role: 'primary' }] },
          options
        )
      )
      expect((missing as ProjectValidationError).data).toEqual({
        kind: 'project_validation',
        code: 'folder_missing',
        path: missingPath
      })

      const filePath = join(root, 'notes.txt')
      await writeFile(filePath, 'hello')
      const file = await rejection(
        buildProjectRecordFromCreateInput(
          { name: 'File', folders: [{ path: filePath, role: 'primary' }] },
          options
        )
      )
      expect((file as ProjectValidationError).data.code).toBe('folder_missing')
    })

    it('rejects duplicate folders even when they are reached through a symlink', async () => {
      const link = join(root, 'app-link')
      await symlink(primary, link)
      const duplicate = await rejection(
        buildProjectRecordFromCreateInput(
          {
            name: 'Duplicate',
            folders: [
              { path: primary, role: 'primary' },
              { path: link, role: 'auxiliary' }
            ]
          },
          options
        )
      )
      expect((duplicate as ProjectValidationError).data).toEqual({
        kind: 'project_validation',
        code: 'folder_duplicate',
        path: link
      })
    })

    it('rejects folders nested inside one another in either direction', async () => {
      const nested = join(primary, 'packages')
      await mkdir(nested)
      for (const folders of [
        [
          { path: primary, role: 'primary' as const },
          { path: nested, role: 'auxiliary' as const }
        ],
        [
          { path: nested, role: 'primary' as const },
          { path: primary, role: 'auxiliary' as const }
        ]
      ]) {
        const error = await rejection(
          buildProjectRecordFromCreateInput({ name: 'Nested', folders }, options)
        )
        expect((error as ProjectValidationError).data.code).toBe('folder_nested')
      }
    })

    it('rejects more than the folder limit before touching the filesystem', async () => {
      const folders = Array.from({ length: 33 }, (_, index) => ({
        path: join(root, `missing-${index}`),
        role: index === 0 ? ('primary' as const) : ('auxiliary' as const)
      }))
      const error = await rejection(
        buildProjectRecordFromCreateInput({ name: 'Too many', folders }, options)
      )
      expect((error as ProjectValidationError).data.code).toBe('too_many_folders')
    })
  })

  describe('buildProjectRecordFromUpdateInput', () => {
    let existing: StorageProjectRecord

    beforeEach(async () => {
      existing = {
        ...(await buildProjectRecordFromCreateInput(
          {
            name: 'Wire workspace',
            folders: [
              { path: primary, role: 'primary' },
              { path: docs, role: 'auxiliary' }
            ]
          },
          options
        )),
        pinnedAt: 42
      }
    })

    it('keeps identity, pin state and folder ids while swapping the primary folder', async () => {
      const updated = await buildProjectRecordFromUpdateInput(
        {
          projectId: existing.id,
          name: 'Renamed',
          folders: [
            { id: 'folder-2', path: docs, role: 'primary' },
            { id: 'folder-1', path: primary, role: 'auxiliary' }
          ]
        },
        existing,
        { ...options, now: () => 1_800_000_000_000 }
      )

      expect(updated).toEqual({
        ...existing,
        name: 'Renamed',
        folders: [
          { ...existing.folders[1], role: 'primary', sortOrder: 0 },
          { ...existing.folders[0], role: 'auxiliary', sortOrder: 1 }
        ]
      })
      expect(primaryProjectFolderPath(updated)).toBe(docs)
    })

    it('treats a re-pointed folder id as a new folder and keeps existing aliases unique', async () => {
      const otherDocs = join(root, 'other')
      await mkdir(join(otherDocs))
      await mkdir(join(otherDocs, 'docs'))
      const updated = await buildProjectRecordFromUpdateInput(
        {
          projectId: existing.id,
          name: existing.name,
          folders: [
            { id: 'folder-1', path: primary, role: 'primary' },
            { id: 'folder-2', path: join(otherDocs, 'docs'), role: 'auxiliary' },
            { path: docs, role: 'auxiliary' }
          ]
        },
        existing,
        { ...options, now: () => 1_800_000_000_000 }
      )

      expect(updated.folders.map(({ id, alias, createdAt }) => ({ id, alias, createdAt }))).toEqual(
        [
          { id: 'folder-1', alias: 'app', createdAt: 1_700_000_000_000 },
          { id: 'folder-3', alias: 'docs', createdAt: 1_800_000_000_000 },
          { id: 'folder-4', alias: 'docs-2', createdAt: 1_800_000_000_000 }
        ]
      )
    })

    it('rejects updates for projects that no longer exist', async () => {
      const error = await rejection(
        buildProjectRecordFromUpdateInput(
          {
            projectId: 'gone',
            name: 'Gone',
            folders: [{ path: primary, role: 'primary' }]
          },
          undefined,
          options
        )
      )
      expect((error as ProjectValidationError).data).toEqual({
        kind: 'project_validation',
        code: 'project_missing'
      })
    })
  })
})
